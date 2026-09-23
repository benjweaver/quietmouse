//! One thread per endpoint: applies the config to each device behind it, and
//! turns diverted buttons, gestures and thumb wheel movement into actions.

use std::collections::{BTreeMap, HashMap};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hidpp::features::reprog::{self, Control, Reporting};
use hidpp::features::smartshift::{self, NEVER_DISENGAGE, WheelMode};
use hidpp::features::wheel::{self, ScrollMode, ThumbInfo, ThumbReporting};
use hidpp::features::{self, dpi, host};
use hidpp::{Device, DeviceEvent, Error, Link, Report, Session, receiver};

use crate::config::{
    Action, ButtonConfig, ButtonId, Config, DEFAULT_GESTURE_STRAIGHTNESS, Profile, ScrollConfig, ShiftMode,
    SmartShiftConfig, ThumbWheelConfig, default_gesture_threshold,
};
use crate::connect::{self, Role};
use crate::gesture::{self, Gesture, Ticker};
use crate::hid::Endpoint;
use crate::inject::{Injector, Output};
use crate::keys::Desktop;

/// How long to give an endpoint to say whether anything is there. Short,
/// because the answer comes back in milliseconds when a device is awake, and
/// when it isn't, asking again shortly beats waiting out the usual timeout.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);
/// How soon to ask again after a device doesn't answer, usually because it's
/// still waking up...
const FIRST_RETRY: Duration = Duration::from_millis(250);
/// ...doubling up to this. A mouse left asleep then costs one request every few
/// seconds, and starts working within that of being woken.
const MAX_RETRY: Duration = Duration::from_secs(5);
/// Wait when nothing is scheduled; any report or shutdown wakes the worker sooner.
const IDLE_WAIT: Duration = Duration::from_secs(3600);

/// A retry schedule that keeps doubling up to a cap.
///
/// Asleep and broken look alike over HID++: both just stop answering. So nothing
/// is ever written off, it's only asked less often, and the moment a device
/// starts answering again its settings go back on.
#[derive(Debug, Clone, Copy)]
struct Backoff {
    due: Instant,
    delay: Duration,
    /// Set on the first wait to reach [`MAX_RETRY`], so a device that has gone
    /// quiet for good is reported once instead of on every retry.
    just_capped: bool,
}

impl Backoff {
    fn first() -> Self {
        Self::after(FIRST_RETRY, FIRST_RETRY >= MAX_RETRY)
    }

    /// The next wait, twice as long, up to [`MAX_RETRY`].
    fn next(self) -> Self {
        let delay = (self.delay * 2).min(MAX_RETRY);
        Self::after(delay, delay >= MAX_RETRY && self.delay < MAX_RETRY)
    }

    fn after(delay: Duration, just_capped: bool) -> Self {
        Self {
            due: Instant::now() + delay,
            delay,
            just_capped,
        }
    }

    fn is_due(self, now: Instant) -> bool {
        self.due <= now
    }
}

/// State shared by every worker.
pub struct Shared {
    pub config: Config,
    pub injector: Injector,
    /// Devices connected directly rather than through a receiver, by product ID.
    /// Their worker ends whenever they go away, so what it learned is kept here.
    pub known: Mutex<HashMap<u16, Learned>>,
}

/// What setting a device up taught quietmouse about it: its features and its
/// buttons. Neither changes while it's away, and asking again took 27 of the 33
/// round trips of setting up an MX Master 3S over Bluetooth, half a second
/// during which its buttons did nothing.
#[derive(Clone)]
pub struct Learned {
    device: Device,
    controls: Option<Vec<Control>>,
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
        endpoint,
        devices: HashMap::new(),
        retries: HashMap::new(),
        reprobe: None,
    };
    let error = match worker.start(&mut session) {
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

struct Worker<'a> {
    shared: Arc<Shared>,
    endpoint: &'a Endpoint,
    devices: HashMap<u8, DeviceState>,
    retries: HashMap<u8, Retry>,
    /// Set while the endpoint itself hasn't answered yet, so it's asked again.
    reprobe: Option<Backoff>,
}

struct Retry {
    backoff: Backoff,
    wireless_pid: Option<u16>,
}

impl Worker<'_> {
    /// Returns `false` if the endpoint isn't worth serving.
    fn start<L: Link>(&mut self, session: &mut Session<L>) -> hidpp::Result<bool> {
        match self.probe(session)? {
            Some(true) => Ok(true),
            // Nothing answered. Most likely a device that's asleep, so the
            // worker stays put and keeps asking rather than giving the endpoint
            // up: waking the mouse is then all it takes.
            None => {
                log::debug!("{} isn't answering yet; waiting for it", self.endpoint.describe());
                self.reprobe = Some(Backoff::first());
                Ok(true)
            }
            Some(false) => Ok(false),
        }
    }

    /// Works out what the endpoint is and sets it up. `None` means nothing
    /// answered; `Some(false)` that it answered but doesn't speak HID++ 2.0.
    fn probe<L: Link>(&mut self, session: &mut Session<L>) -> hidpp::Result<Option<bool>> {
        match connect::probe(session, self.endpoint, PROBE_TIMEOUT)? {
            Role::Foreign => Ok(Some(false)),
            Role::Silent => Ok(None),
            Role::Receiver => {
                log::info!("found {}", self.endpoint.describe());
                // Paired devices answer with connection notices, handled in `handle`.
                tolerate(receiver::enable_notifications(session), "enable receiver notifications")?;
                tolerate(receiver::announce_devices(session), "list the receiver's devices")?;
                Ok(Some(true))
            }
            Role::Direct(index) => {
                self.connect(session, index, None)?;
                Ok(Some(true))
            }
        }
    }

    /// Handles events until the link fails or shuts down.
    fn serve<L: Link>(&mut self, session: &mut Session<L>) -> Error {
        loop {
            let wait = self
                .next_due()
                .map_or(IDLE_WAIT, |due| due.saturating_duration_since(Instant::now()));
            let handled = match session.next_event(wait) {
                Ok(Some(report)) => self.handle(session, &report),
                Ok(None) => Ok(()),
                Err(error) => return error,
            };
            if let Err(error) = handled.and_then(|()| self.work_due(session)) {
                if error.is_fatal() {
                    return error;
                }
                log::warn!("{error}");
            }
        }
    }

    /// When the next retry falls due, of any kind.
    fn next_due(&self) -> Option<Instant> {
        self.retries
            .values()
            .map(|retry| retry.backoff.due)
            .chain(self.reprobe.map(|backoff| backoff.due))
            .min()
    }

    /// Runs whatever is due: asking a silent endpoint again, and configuring
    /// devices that weren't ready last time.
    fn work_due<L: Link>(&mut self, session: &mut Session<L>) -> hidpp::Result<()> {
        if let Some(backoff) = self.reprobe.filter(|backoff| backoff.is_due(Instant::now())) {
            self.reprobe = None;
            match self.probe(session)? {
                // Still nothing; ask again later.
                None => {
                    let next = backoff.next();
                    if next.just_capped {
                        log::info!("{} still isn't answering; still trying", self.endpoint.describe());
                    }
                    self.reprobe = Some(next);
                }
                Some(true) => log::debug!("{} started answering", self.endpoint.describe()),
                // It answered at last, but with nothing we can drive.
                Some(false) => log::debug!("{} doesn't speak HID++ 2.0; ignoring it", self.endpoint.describe()),
            }
        }
        self.retry_due(session)
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
                if state.still_configured(session) {
                    log::info!("{}: reconnected, with its settings still in place", state.device.name());
                    return Ok(());
                }
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
        let backoff = self
            .retries
            .get(&index)
            .map_or_else(Backoff::first, |retry| retry.backoff.next());
        log::debug!(
            "device {index:#04x} not ready ({error}); asking again in {:?}",
            backoff.delay
        );
        if backoff.just_capped {
            log::info!("device {index:#04x} isn't answering ({error}); still trying");
        }
        self.retries.insert(index, Retry { backoff, wireless_pid });
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
            .filter(|state| wireless_pid.is_none() || state.wireless_pid == wireless_pid)
            .map(|state| {
                let learned = Learned {
                    device: state.device,
                    controls: state.controls,
                };
                (learned, state.wireless_pid)
            });
        let (learned, known_pid, from_memory) = match known {
            Some((learned, known_pid)) => (learned, known_pid, false),
            None => match self.remembered(session, index)? {
                Some(learned) => (learned, None, true),
                None => {
                    let device = Device::open(session, index)?;
                    (Learned { device, controls: None }, None, false)
                }
            },
        };
        let profile = self.shared.config.profile_for(learned.device.name()).cloned();
        let mut state = DeviceState::new(learned, wireless_pid.or(known_pid), profile);
        let applied = state.apply(session);
        // What worked is kept for next time. What didn't, having come from an
        // earlier connection, may be out of date, say after a firmware update,
        // so it's forgotten and the retry learns everything afresh.
        match &applied {
            Ok(()) => self.remember(state.learned()),
            Err(_) if from_memory => self.forget(),
            Err(_) => {}
        }
        self.devices.insert(index, state);
        applied
    }

    /// The key a directly connected device is remembered by. A receiver's
    /// devices are remembered by its own worker, which outlives their visits.
    fn memory_key(&self) -> Option<u16> {
        self.endpoint
            .receiver_kind()
            .is_none()
            .then_some(self.endpoint.product_id)
    }

    /// What was learned last time this device was connected, if it still holds:
    /// one request checks its features haven't moved since.
    fn remembered<L: Link>(&self, session: &mut Session<L>, index: u8) -> hidpp::Result<Option<Learned>> {
        let Some(key) = self.memory_key() else {
            return Ok(None);
        };
        let learned = self.shared.known.lock().ok().and_then(|known| known.get(&key).cloned());
        let Some(mut learned) = learned else {
            return Ok(None);
        };
        learned.device = learned.device.at(index);
        if learned.device.still_matches(session)? {
            return Ok(Some(learned));
        }
        log::info!(
            "{}: its features have moved; learning them again",
            learned.device.name()
        );
        self.forget();
        Ok(None)
    }

    fn remember(&self, learned: Learned) {
        if let Some(key) = self.memory_key()
            && let Ok(mut known) = self.shared.known.lock()
        {
            known.insert(key, learned);
        }
    }

    fn forget(&self) {
        if let Some(key) = self.memory_key()
            && let Ok(mut known) = self.shared.known.lock()
        {
            known.remove(&key);
        }
    }

    fn retry_due<L: Link>(&mut self, session: &mut Session<L>) -> hidpp::Result<()> {
        let now = Instant::now();
        let due: Vec<(u8, Option<u16>)> = self
            .retries
            .iter()
            .filter(|(_, retry)| retry.backoff.is_due(now))
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
    /// Its buttons, once listed; see [`Learned`].
    controls: Option<Vec<Control>>,
    wireless_pid: Option<u16>,
    profile: Option<Profile>,
    online: bool,
    diverted: Vec<Control>,
    thumb: Option<ThumbState>,
    held: Vec<u16>,
    gesture: Option<(u16, Gesture)>,
    /// What the pointer is set to, for scaling the gesture deadzone. `None`
    /// while the device hasn't said, or won't.
    dpi: Option<u16>,
}

struct ThumbState {
    ticker: Ticker,
    left: Option<Action>,
    right: Option<Action>,
}

impl DeviceState {
    /// Whether the device still holds what was set up on it, judged by its first
    /// diverted control. A mouse announces each reconnection, having dropped its
    /// diversions when the link came back; found and set up on that same link
    /// before the announcement arrives, as a Bluetooth mouse switched back from
    /// another computer usually is, it has lost nothing, and setting it all up
    /// again would only repeat the work. With nothing diverted there's nothing
    /// to ask, so that counts as lost.
    fn still_configured<L: Link>(&self, session: &mut Session<L>) -> bool {
        self.diverted.first().is_some_and(|control| {
            reprog::reporting(session, &self.device, control.cid).is_ok_and(|reporting| reporting.diverted)
        })
    }

    fn learned(&self) -> Learned {
        Learned {
            device: self.device.clone(),
            controls: self.controls.clone(),
        }
    }

    fn new(learned: Learned, wireless_pid: Option<u16>, profile: Option<Profile>) -> Self {
        Self {
            device: learned.device,
            controls: learned.controls,
            wireless_pid,
            profile,
            online: true,
            diverted: Vec::new(),
            thumb: None,
            held: Vec::new(),
            gesture: None,
            dpi: None,
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
        // Buttons first: they're what someone presses the moment a device comes
        // back, and a press arriving while the rest is set up is kept for later.
        let diverting = divert_buttons(
            session,
            device,
            &mut self.controls,
            &profile.buttons,
            &mut self.diverted,
        );
        setting(diverting, name, "buttons")?;
        // Gesture counts are DPI, so the deadzone is scaled to what the pointer
        // is actually set to. Asking costs one request, and nothing at all on a
        // device whose resolution isn't adjustable.
        self.dpi = match profile.dpi {
            Some(value) => match apply_dpi(session, device, value) {
                Ok(settled) => Some(settled),
                Err(error) => {
                    setting(Err(error), name, "DPI")?;
                    None
                }
            },
            None => dpi::read(session, device).ok().map(|current| current.current),
        };
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
        log::debug!(
            "{name}: pointer at {} dpi, so a swipe travels {} counts by default",
            self.dpi.map_or_else(|| "an unknown".to_owned(), |dpi| dpi.to_string()),
            default_gesture_threshold(self.dpi)
        );
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
                let dpi = self.dpi;
                for cid in pressed {
                    let Some(binding) = self.binding(cid) else {
                        continue;
                    };
                    if binding.is_gesture() {
                        let threshold = binding.threshold.unwrap_or_else(|| default_gesture_threshold(dpi));
                        let straightness = binding.straightness.unwrap_or(DEFAULT_GESTURE_STRAIGHTNESS);
                        self.gesture = Some((cid, Gesture::new(threshold, straightness)));
                    } else if let Some(action) = binding.press.clone() {
                        self.perform(session, &action, injector)?;
                    }
                }
            }
            DeviceEvent::RawXy { dx, dy } => {
                if let Some((cid, gesture)) = &mut self.gesture
                    && let Some(direction) = gesture.movement(dx, dy, Instant::now())
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

    fn perform<L: Link>(
        &mut self,
        session: &mut Session<L>,
        action: &Action,
        injector: &Injector,
    ) -> hidpp::Result<()> {
        log::debug!("{}: {action:?}", self.device.name());
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
            Action::Dpi(value) => self.change_dpi(session, *value),
            Action::CycleDpi(values) => match next_dpi(session, &self.device, values) {
                Ok(next) => self.change_dpi(session, next),
                Err(error) => Err(error),
            },
            Action::ToggleSmartshift => toggle_smartshift(session, &self.device),
            Action::Host(channel) => host::switch(session, &self.device, channel.saturating_sub(1)),
        };
        setting(result, self.device.name(), "action")
    }

    /// Sets the pointer resolution and remembers what the device settled on, so
    /// a button that changes DPI takes the gesture deadzone with it rather than
    /// leaving it measuring a different distance than it did a moment ago.
    fn change_dpi<L: Link>(&mut self, session: &mut Session<L>, wanted: u16) -> hidpp::Result<()> {
        let settled = apply_dpi(session, &self.device, wanted)?;
        self.dpi = Some(settled);
        Ok(())
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

/// Sets the pointer resolution, returning what the device settled on, which is
/// the nearest it supports when it can't give exactly what was asked for.
fn apply_dpi<L: Link>(session: &mut Session<L>, device: &Device, wanted: u16) -> hidpp::Result<u16> {
    let current = dpi::read(session, device)?;
    let target = current.choices.nearest(wanted).unwrap_or(wanted);
    if target != wanted {
        log::warn!("{}: {wanted} DPI isn't supported; using {target}", device.name());
    }
    if current.current != target {
        dpi::set(session, device, target)?;
    }
    Ok(target)
}

/// The resolution after the current one in `values`, starting over at the end.
/// The config guarantees `values` isn't empty.
fn next_dpi<L: Link>(session: &mut Session<L>, device: &Device, values: &[u16]) -> hidpp::Result<u16> {
    let current = dpi::read(session, device)?;
    let next = next_in_cycle(values, current.current, |value| {
        current.choices.nearest(value).unwrap_or(value)
    });
    log::info!("{}: DPI {}", device.name(), values[next]);
    Ok(values[next])
}

/// Index of the value after `current` in `values`, or the first. A value the
/// device can't give exactly is set as the nearest it can, so `current` is
/// looked for among what each value `settles` to. The last match is taken, so
/// values that settle alike don't keep cycling back to one another.
fn next_in_cycle(values: &[u16], current: u16, settles: impl Fn(u16) -> u16) -> usize {
    values
        .iter()
        .rposition(|&value| settles(value) == current)
        .map_or(0, |i| (i + 1) % values.len())
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

/// Diverts the profile's buttons, listing the device's controls first if
/// `controls` doesn't hold them yet. Each one diverted goes into `diverted` as
/// it's done, so the ones before a failure are still handed back on exit. A
/// button the device refuses is skipped; one that times out stops the rest.
fn divert_buttons<L: Link>(
    session: &mut Session<L>,
    device: &Device,
    controls: &mut Option<Vec<Control>>,
    buttons: &BTreeMap<ButtonId, ButtonConfig>,
    diverted: &mut Vec<Control>,
) -> hidpp::Result<()> {
    if buttons.is_empty() {
        return Ok(());
    }
    if controls.is_none() {
        *controls = Some(reprog::controls(session, device)?);
    }
    let controls = controls.as_deref().unwrap_or_default();
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
        match reprog::set_reporting(session, device, control, Reporting { diverted: true, raw_xy }) {
            Ok(()) => diverted.push(*control),
            Err(error) => setting(Err(error), device.name(), &format!("the `{id}` button"))?,
        }
    }
    Ok(())
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
    fn dpi_cycles_past_values_the_device_rounds() {
        let steps_of_50 = |value: u16| (value + 25) / 50 * 50;
        // 1234 is set as 1250, which must still count as being at 1234.
        assert_eq!(next_in_cycle(&[1234, 2000], 1250, steps_of_50), 1);
        assert_eq!(next_in_cycle(&[1234, 2000], 2000, steps_of_50), 0);
        // Two values that both land on 1250 move on to the next distinct one.
        assert_eq!(next_in_cycle(&[1234, 1240, 2000], 1250, steps_of_50), 2);
        // Somewhere not in the cycle starts it from the top.
        assert_eq!(next_in_cycle(&[800, 1600], 1000, steps_of_50), 0);
    }

    #[test]
    fn backoff_doubles_to_the_cap_and_reports_it_once() {
        let mut backoff = Backoff::first();
        assert_eq!(backoff.delay, FIRST_RETRY);
        assert!(!backoff.just_capped);
        let mut capped = 0;
        for _ in 0..20 {
            backoff = backoff.next();
            capped += u32::from(backoff.just_capped);
        }
        assert_eq!(backoff.delay, MAX_RETRY);
        assert_eq!(capped, 1, "a long silence should be reported exactly once");
    }

    #[test]
    fn a_retry_is_only_due_once_its_wait_has_passed() {
        let backoff = Backoff::first();
        assert!(!backoff.is_due(Instant::now()));
        assert!(backoff.is_due(Instant::now() + FIRST_RETRY * 2));
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
