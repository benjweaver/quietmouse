//! One thread per endpoint: applies the config to each device behind it, and
//! turns diverted buttons, gestures and thumb wheel movement into actions.

use std::collections::{BTreeMap, HashMap};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use hidpp::features::reprog::{self, Control, Reporting};
use hidpp::features::smartshift::{self, NEVER_DISENGAGE, WheelMode};
use hidpp::features::wheel::{self, ScrollMode, ThumbInfo, ThumbReporting};
use hidpp::features::{self, dpi, host};
use hidpp::{Device, DeviceEvent, Error, Link, Report, Session, receiver};

use crate::config::{
    Action, ButtonConfig, ButtonId, Config, DEFAULT_GESTURE_STRAIGHTNESS, DEFAULT_GESTURE_THRESHOLD, Profile,
    ScrollConfig, ShiftMode, SmartShiftConfig, ThumbWheelConfig,
};
use crate::connect::{self, Role};
use crate::gesture::{self, Gesture, Ticker};
use crate::hid::Endpoint;
use crate::inject::{Injector, Output};
use crate::keys::Desktop;

/// A device that couldn't be configured (usually still waking up) is retried this often...
const RETRY_DELAY: Duration = Duration::from_secs(1);
/// ...this many times.
const MAX_ATTEMPTS: u32 = 5;
/// Wait when nothing is scheduled; any report or shutdown wakes the worker sooner.
const IDLE_WAIT: Duration = Duration::from_secs(3600);

/// State shared by every worker.
pub struct Shared {
    pub config: Config,
    pub injector: Injector,
}

/// Why a worker stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Disconnected,
    Stopped,
    /// Nothing on the endpoint speaks HID++ 2.0.
    NotHidpp,
}

pub fn run<L: Link>(link: L, endpoint: &Endpoint, shared: Arc<Shared>) -> Outcome {
    let mut session = Session::new(link);
    let mut worker = Worker {
        shared,
        devices: HashMap::new(),
        retries: HashMap::new(),
    };
    let error = match worker.start(&mut session, endpoint) {
        Ok(true) => worker.serve(&mut session),
        Ok(false) => {
            log::debug!("{} doesn't speak HID++ 2.0; ignoring it", endpoint.describe());
            return Outcome::NotHidpp;
        }
        Err(error) => error,
    };
    if error == Error::Stopped {
        worker.restore(&mut session);
        return Outcome::Stopped;
    }
    log::info!("{} went away ({error})", endpoint.describe());
    Outcome::Disconnected
}

struct Worker {
    shared: Arc<Shared>,
    devices: HashMap<u8, DeviceState>,
    retries: HashMap<u8, Retry>,
}

struct Retry {
    due: Instant,
    attempts: u32,
    wireless_pid: Option<u16>,
}

impl Worker {
    /// Returns `false` if the endpoint isn't worth serving.
    fn start<L: Link>(&mut self, session: &mut Session<L>, endpoint: &Endpoint) -> hidpp::Result<bool> {
        match connect::probe(session, endpoint)? {
            None => Ok(false),
            Some(Role::Receiver) => {
                log::info!("found {}", endpoint.describe());
                // Paired devices answer with connection notices, handled in `handle`.
                tolerate(receiver::enable_notifications(session), "enable receiver notifications")?;
                tolerate(receiver::announce_devices(session), "list the receiver's devices")?;
                Ok(true)
            }
            Some(Role::Direct(index)) => {
                self.connect(session, index, None)?;
                Ok(true)
            }
        }
    }

    /// Handles events until the link fails or shuts down.
    fn serve<L: Link>(&mut self, session: &mut Session<L>) -> Error {
        loop {
            let wait = self
                .retries
                .values()
                .map(|retry| retry.due)
                .min()
                .map_or(IDLE_WAIT, |due| due.saturating_duration_since(Instant::now()));
            let handled = match session.next_event(wait) {
                Ok(Some(report)) => self.handle(session, &report),
                Ok(None) => Ok(()),
                Err(error) => return error,
            };
            if let Err(error) = handled.and_then(|()| self.retry_due(session)) {
                if error.is_fatal() {
                    return error;
                }
                log::warn!("{error}");
            }
        }
    }

    fn handle<L: Link>(&mut self, session: &mut Session<L>, report: &Report) -> hidpp::Result<()> {
        if let Some(connection) = receiver::parse_connection(report) {
            self.retries.remove(&connection.index);
            if connection.online {
                return self.connect(session, connection.index, Some(connection.wireless_pid));
            }
            if let Some(state) = self.devices.get_mut(&connection.index) {
                log::info!("{}: asleep or out of range", state.device.name());
                state.online = false;
                state.reset_input();
            }
            return Ok(());
        }
        let index = report.device_index();
        let Some(state) = self.devices.get_mut(&index) else {
            return Ok(());
        };
        match state.device.parse_event(report) {
            Some(DeviceEvent::Reconnected) => {
                log::info!("{}: reconnected; applying settings again", state.device.name());
                self.connect(session, index, None)
            }
            Some(event) => state.handle(session, event, &self.shared.injector),
            None => Ok(()),
        }
    }

    /// Configures the device at `index`, scheduling a retry if it isn't answering yet.
    fn connect<L: Link>(
        &mut self,
        session: &mut Session<L>,
        index: u8,
        wireless_pid: Option<u16>,
    ) -> hidpp::Result<()> {
        let Err(error) = self.configure(session, index, wireless_pid) else {
            self.retries.remove(&index);
            return Ok(());
        };
        if error.is_fatal() {
            return Err(error);
        }
        let attempts = self.retries.get(&index).map_or(0, |retry| retry.attempts) + 1;
        if attempts < MAX_ATTEMPTS {
            log::debug!("device {index:#04x} not ready ({error}); retrying");
            let due = Instant::now() + RETRY_DELAY;
            self.retries.insert(
                index,
                Retry {
                    due,
                    attempts,
                    wireless_pid,
                },
            );
        } else {
            self.retries.remove(&index);
            log::warn!("giving up on device {index:#04x}: {error}");
        }
        Ok(())
    }

    fn configure<L: Link>(
        &mut self,
        session: &mut Session<L>,
        index: u8,
        wireless_pid: Option<u16>,
    ) -> hidpp::Result<()> {
        // A device that just woke up keeps what we learned about it; a different
        // device paired to the same slot is read afresh.
        let known = self
            .devices
            .remove(&index)
            .filter(|state| wireless_pid.is_none() || state.wireless_pid == wireless_pid);
        let (device, known_pid) = match known {
            Some(state) => (state.device, state.wireless_pid),
            None => (Device::open(session, index)?, None),
        };
        let profile = self.shared.config.profile_for(device.name()).cloned();
        let mut state = DeviceState::new(device, wireless_pid.or(known_pid), profile);
        let applied = state.apply(session);
        self.devices.insert(index, state);
        applied
    }

    fn retry_due<L: Link>(&mut self, session: &mut Session<L>) -> hidpp::Result<()> {
        let now = Instant::now();
        let due: Vec<(u8, Option<u16>)> = self
            .retries
            .iter()
            .filter(|(_, retry)| retry.due <= now)
            .map(|(&index, retry)| (index, retry.wireless_pid))
            .collect();
        for (index, wireless_pid) in due {
            self.connect(session, index, wireless_pid)?;
        }
        Ok(())
    }

    /// Hands diverted controls back to the device so it behaves normally once we exit.
    fn restore<L: Link>(&mut self, session: &mut Session<L>) {
        for state in self.devices.values().filter(|state| state.online) {
            let device = &state.device;
            for control in &state.diverted {
                if let Err(error) = reprog::set_reporting(session, device, control, Reporting::default()) {
                    log::debug!(
                        "{}: couldn't restore control {:#06x}: {error}",
                        device.name(),
                        control.cid
                    );
                }
            }
            if state.thumb.is_some() {
                let restored = wheel::thumb_reporting(session, device).and_then(|current| {
                    wheel::set_thumb_reporting(
                        session,
                        device,
                        ThumbReporting {
                            diverted: false,
                            ..current
                        },
                    )
                });
                if let Err(error) = restored {
                    log::debug!("{}: couldn't restore the thumb wheel: {error}", device.name());
                }
            }
            if !state.diverted.is_empty() || state.thumb.is_some() {
                log::info!("{}: buttons handed back to the device", device.name());
            }
        }
    }
}

/// Everything known about one device, and the input it's in the middle of.
struct DeviceState {
    device: Device,
    wireless_pid: Option<u16>,
    profile: Option<Profile>,
    online: bool,
    diverted: Vec<Control>,
    thumb: Option<ThumbState>,
    held: Vec<u16>,
    gesture: Option<(u16, Gesture)>,
}

struct ThumbState {
    ticker: Ticker,
    left: Option<Action>,
    right: Option<Action>,
}

impl DeviceState {
    fn new(device: Device, wireless_pid: Option<u16>, profile: Option<Profile>) -> Self {
        Self {
            device,
            wireless_pid,
            profile,
            online: true,
            diverted: Vec::new(),
            thumb: None,
            held: Vec::new(),
            gesture: None,
        }
    }

    fn reset_input(&mut self) {
        self.held.clear();
        self.gesture = None;
        if let Some(thumb) = &mut self.thumb {
            thumb.ticker.reset();
        }
    }

    /// Pushes the profile to the device. Settings the device lacks are logged and
    /// skipped; a device that stops answering aborts so the whole thing is retried.
    fn apply<L: Link>(&mut self, session: &mut Session<L>) -> hidpp::Result<()> {
        self.reset_input();
        self.diverted.clear();
        self.thumb = None;
        let device = &self.device;
        let name = device.name();
        let Some(profile) = &self.profile else {
            log::info!("{name}: connected; no [[device]] in the config matches it");
            return Ok(());
        };
        if let Some(value) = profile.dpi {
            setting(apply_dpi(session, device, value), name, "DPI")?;
        }
        if let Some(shift) = &profile.smartshift {
            setting(apply_smartshift(session, device, shift), name, "SmartShift")?;
        }
        if let Some(scroll) = &profile.scroll {
            setting(apply_scroll(session, device, scroll), name, "scrolling")?;
        }
        if let Some(thumb) = &profile.thumbwheel {
            match apply_thumbwheel(session, device, thumb) {
                Ok(state) => self.thumb = state,
                Err(error) => setting(Err(error), name, "the thumb wheel")?,
            }
        }
        match divert_buttons(session, device, &profile.buttons) {
            Ok(diverted) => self.diverted = diverted,
            Err(error) => setting(Err(error), name, "buttons")?,
        }
        log::info!("{name}: settings applied (profile matching {:?})", profile.name_match);
        Ok(())
    }

    fn binding(&self, cid: u16) -> Option<&ButtonConfig> {
        self.profile.as_ref()?.buttons.get(&ButtonId(cid))
    }

    fn handle<L: Link>(
        &mut self,
        session: &mut Session<L>,
        event: DeviceEvent,
        injector: &Injector,
    ) -> hidpp::Result<()> {
        match event {
            DeviceEvent::Buttons(now) => {
                let (pressed, released) = gesture::button_changes(&self.held, &now);
                self.held = now;
                for cid in released {
                    if let Some((_, held)) = self.gesture.take_if(|(gesture_cid, _)| *gesture_cid == cid)
                        && held.is_tap()
                        && let Some(action) = self.binding(cid).and_then(|binding| binding.tap.clone())
                    {
                        self.perform(session, &action, injector)?;
                    }
                }
                for cid in pressed {
                    let Some(binding) = self.binding(cid) else {
                        continue;
                    };
                    if binding.is_gesture() {
                        let threshold = binding.threshold.unwrap_or(DEFAULT_GESTURE_THRESHOLD);
                        let straightness = binding.straightness.unwrap_or(DEFAULT_GESTURE_STRAIGHTNESS);
                        self.gesture = Some((cid, Gesture::new(threshold, straightness)));
                    } else if let Some(action) = binding.press.clone() {
                        self.perform(session, &action, injector)?;
                    }
                }
            }
            DeviceEvent::RawXy { dx, dy } => {
                if let Some((cid, gesture)) = &mut self.gesture
                    && let Some(direction) = gesture.movement(dx, dy)
                {
                    let cid = *cid;
                    log::debug!("{}: swipe {direction:?}", self.device.name());
                    if let Some(action) = self.binding(cid).and_then(|binding| binding.swipe(direction).cloned()) {
                        self.perform(session, &action, injector)?;
                    }
                }
            }
            DeviceEvent::ThumbWheel { rotation, start } => {
                let Some(thumb) = &mut self.thumb else {
                    return Ok(());
                };
                if start {
                    thumb.ticker.reset();
                }
                let steps = thumb.ticker.feed(rotation);
                let action = if steps > 0 {
                    thumb.right.clone()
                } else {
                    thumb.left.clone()
                };
                if let Some(action) = action {
                    for _ in 0..steps.unsigned_abs() {
                        self.perform(session, &action, injector)?;
                    }
                }
            }
            DeviceEvent::Battery(battery) => log::debug!("{}: battery {battery:?}", self.device.name()),
            DeviceEvent::Reconnected => {}
        }
        Ok(())
    }

    fn perform<L: Link>(&self, session: &mut Session<L>, action: &Action, injector: &Injector) -> hidpp::Result<()> {
        let device = &self.device;
        log::debug!("{}: {action:?}", device.name());
        let send = |output: Output| -> hidpp::Result<()> {
            injector.send(output);
            Ok(())
        };
        let result = match action {
            Action::Keys(chord) => send(Output::Chord(chord.clone())),
            Action::Media(key) => send(Output::Media(*key)),
            Action::Click(button) => send(Output::Click(*button)),
            Action::Overview => send(Output::Desktop(Desktop::Overview)),
            Action::AppWindows => send(Output::Desktop(Desktop::AppWindows)),
            Action::ShowDesktop => send(Output::Desktop(Desktop::Show)),
            Action::DesktopLeft => send(Output::Desktop(Desktop::Left)),
            Action::DesktopRight => send(Output::Desktop(Desktop::Right)),
            Action::Shell(command) => {
                spawn_shell(command);
                Ok(())
            }
            Action::Ignore => Ok(()),
            Action::Dpi(value) => apply_dpi(session, device, *value),
            Action::CycleDpi(values) => cycle_dpi(session, device, values),
            Action::ToggleSmartshift => toggle_smartshift(session, device),
            Action::Host(channel) => host::switch(session, device, channel.saturating_sub(1)),
        };
        setting(result, device.name(), "action")
    }
}

/// Logs a non-fatal failure and carries on.
fn tolerate(result: hidpp::Result<()>, what: &str) -> hidpp::Result<()> {
    match result {
        Err(error) if error.is_fatal() => Err(error),
        Err(error) => {
            log::warn!("couldn't {what}: {error}");
            Ok(())
        }
        Ok(()) => Ok(()),
    }
}

/// Like [`tolerate`], but a timeout also aborts, since the device has stopped answering.
fn setting(result: hidpp::Result<()>, device: &str, what: &str) -> hidpp::Result<()> {
    match result {
        Err(error @ (Error::Timeout | Error::Disconnected | Error::Stopped)) => Err(error),
        Err(error) => {
            log::warn!("{device}: couldn't set {what}: {error}");
            Ok(())
        }
        Ok(()) => Ok(()),
    }
}

fn apply_dpi<L: Link>(session: &mut Session<L>, device: &Device, wanted: u16) -> hidpp::Result<()> {
    let current = dpi::read(session, device)?;
    let target = current.choices.nearest(wanted).unwrap_or(wanted);
    if target != wanted {
        log::warn!("{}: {wanted} DPI isn't supported; using {target}", device.name());
    }
    if current.current != target {
        dpi::set(session, device, target)?;
    }
    Ok(())
}

fn cycle_dpi<L: Link>(session: &mut Session<L>, device: &Device, values: &[u16]) -> hidpp::Result<()> {
    let current = dpi::read(session, device)?.current;
    let next = values
        .iter()
        .position(|&value| value == current)
        .map_or(0, |i| (i + 1) % values.len());
    log::info!("{}: DPI {}", device.name(), values[next]);
    apply_dpi(session, device, values[next])
}

fn apply_smartshift<L: Link>(
    session: &mut Session<L>,
    device: &Device,
    config: &SmartShiftConfig,
) -> hidpp::Result<()> {
    let current = smartshift::read(session, device)?.ok_or(Error::Unsupported(features::SMART_SHIFT))?;
    let (mode, auto_disengage) = match config.mode {
        None => (None, config.threshold),
        Some(ShiftMode::Freespin) => (Some(WheelMode::Freespin), None),
        Some(ShiftMode::Ratchet) => (Some(WheelMode::Ratchet), Some(NEVER_DISENGAGE)),
        Some(ShiftMode::Auto) => {
            // Coming from "always ratchet", fall back to the device's default threshold.
            let fallback = (current.auto_disengage == NEVER_DISENGAGE).then_some(current.default_auto_disengage);
            (Some(WheelMode::Ratchet), config.threshold.or(fallback))
        }
    };
    if config.torque.is_some() && current.torque.is_none() {
        log::warn!("{}: this wheel's ratchet torque isn't adjustable", device.name());
    }
    let torque = config.torque.filter(|_| current.torque.is_some());
    smartshift::write(session, device, mode, auto_disengage, torque)
}

fn toggle_smartshift<L: Link>(session: &mut Session<L>, device: &Device) -> hidpp::Result<()> {
    let current = smartshift::read(session, device)?.ok_or(Error::Unsupported(features::SMART_SHIFT))?;
    let mode = match current.mode {
        Some(WheelMode::Freespin) => WheelMode::Ratchet,
        _ => WheelMode::Freespin,
    };
    smartshift::write(session, device, Some(mode), None, None)
}

fn apply_scroll<L: Link>(session: &mut Session<L>, device: &Device, config: &ScrollConfig) -> hidpp::Result<()> {
    let current = wheel::scroll_mode(session, device)?;
    let wanted = ScrollMode {
        hires: config.hires.unwrap_or(current.hires),
        inverted: config.invert.unwrap_or(current.inverted),
        ..current
    };
    if wanted != current {
        wheel::set_scroll_mode(session, device, wanted)?;
    }
    Ok(())
}

fn apply_thumbwheel<L: Link>(
    session: &mut Session<L>,
    device: &Device,
    config: &ThumbWheelConfig,
) -> hidpp::Result<Option<ThumbState>> {
    let current = wheel::thumb_reporting(session, device)?;
    let wanted = ThumbReporting {
        diverted: config.diverted(),
        inverted: config.invert.unwrap_or(current.inverted),
    };
    if wanted != current {
        wheel::set_thumb_reporting(session, device, wanted)?;
    }
    if !wanted.diverted {
        return Ok(None);
    }
    let step = match config.step {
        Some(step) => step,
        None => default_thumb_step(wheel::thumb_info(session, device)?),
    };
    Ok(Some(ThumbState {
        ticker: Ticker::new(step),
        left: config.left.clone(),
        right: config.right.clone(),
    }))
}

/// One action per native scroll increment.
fn default_thumb_step(info: ThumbInfo) -> u16 {
    (info.diverted_resolution / info.native_resolution.max(1)).max(1)
}

fn divert_buttons<L: Link>(
    session: &mut Session<L>,
    device: &Device,
    buttons: &BTreeMap<ButtonId, ButtonConfig>,
) -> hidpp::Result<Vec<Control>> {
    if buttons.is_empty() {
        return Ok(Vec::new());
    }
    let controls = reprog::controls(session, device)?;
    let mut diverted = Vec::new();
    for (id, binding) in buttons {
        let Some(control) = controls.iter().find(|control| control.cid == id.0) else {
            log::warn!("{}: has no `{id}` button (see `quietmouse info`)", device.name());
            continue;
        };
        if !control.divertable() {
            log::warn!("{}: `{id}` can't be remapped", device.name());
            continue;
        }
        let raw_xy = binding.has_swipes();
        if raw_xy && !control.supports_raw_xy() {
            log::warn!(
                "{}: `{id}` can't report movement, so its swipes won't fire",
                device.name()
            );
        }
        reprog::set_reporting(session, device, control, Reporting { diverted: true, raw_xy })?;
        diverted.push(*control);
    }
    Ok(diverted)
}

fn spawn_shell(command: &str) {
    match shell(command).spawn() {
        // Reap the child in the background so it doesn't linger as a zombie.
        Ok(mut child) => {
            thread::spawn(move || child.wait());
        }
        Err(error) => log::warn!("couldn't run `{command}`: {error}"),
    }
}

#[cfg(target_os = "windows")]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("cmd");
    shell.args(["/C", command]);
    shell
}

#[cfg(not(target_os = "windows"))]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.args(["-c", command]);
    shell
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_step_follows_native_resolution() {
        let info = ThumbInfo {
            native_resolution: 18,
            diverted_resolution: 180,
        };
        assert_eq!(default_thumb_step(info), 10);
        let odd = ThumbInfo {
            native_resolution: 0,
            diverted_resolution: 0,
        };
        assert_eq!(default_thumb_step(odd), 1);
    }

    #[test]
    fn only_timeouts_and_link_failures_abort_settings() {
        assert_eq!(setting(Err(Error::Unsupported(0x2110)), "mouse", "SmartShift"), Ok(()));
        assert_eq!(setting(Err(Error::Hidpp20(0x02)), "mouse", "DPI"), Ok(()));
        assert_eq!(setting(Err(Error::Timeout), "mouse", "DPI"), Err(Error::Timeout));
        assert_eq!(tolerate(Err(Error::Timeout), "list devices"), Ok(()));
        assert_eq!(tolerate(Err(Error::Stopped), "list devices"), Err(Error::Stopped));
    }
}
