//! One-shot commands: list, info, dpi, events, pair, unpair and config.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use crossbeam_channel::Receiver;
use hidapi::HidApi;
use hidpp::features::battery::{Battery, ChargeState};
use hidpp::features::reprog::{self, Reporting};
use hidpp::features::smartshift::{self, NEVER_DISENGAGE, SmartShift, WheelMode};
use hidpp::features::wheel::{self, ThumbReporting};
use hidpp::features::{self, battery, dpi, host};
use hidpp::receiver::{self, Connection, Paired, Pairing, PairingError, PairingStep, Protocol};
use hidpp::{Device, DeviceEvent, Error, Session};

use crate::config::{ButtonId, Config, example};
use crate::connect::{self, Role};
use crate::daemon;
use crate::hid::{self, Endpoint, HidLink};
use crate::passkey::Prompt;

/// Time allowed for a receiver to announce its paired devices.
const ANNOUNCE_WAIT: Duration = Duration::from_millis(800);
const EVENT_WAIT: Duration = Duration::from_secs(3600);

#[cfg(target_os = "macos")]
const ACCESS_HINT: &str = "If a device is connected, allow this app under System Settings → Privacy & Security → \
                           Input Monitoring, or Device Control and Data Access on newer macOS.";
#[cfg(target_os = "linux")]
const ACCESS_HINT: &str = "If a device is connected, install packaging/linux/70-quietmouse.rules and replug it.";
#[cfg(target_os = "windows")]
const ACCESS_HINT: &str = "Check the device is connected and switched on.";

struct Found {
    endpoint: Endpoint,
    receiver: bool,
    session: Session<HidLink>,
    devices: Vec<Device>,
    /// Devices paired with a receiver that it says aren't connected.
    asleep: Vec<Connection>,
}

fn scan(stop: Receiver<()>) -> anyhow::Result<Vec<Found>> {
    let api = HidApi::new().context("can't start HID access")?;
    let mut found = Vec::new();
    for endpoint in hid::discover(&api) {
        let link = match HidLink::open(&api, &endpoint, stop.clone()) {
            Ok(link) => link,
            Err(error) => {
                eprintln!("{error:#}");
                continue;
            }
        };
        let mut session = Session::new(link);
        let (receiver, indices, asleep) = match connect::probe(&mut session, &endpoint, hidpp::DEFAULT_TIMEOUT) {
            Ok(Role::Receiver) => match connect::receiver_devices(&mut session, ANNOUNCE_WAIT) {
                Ok(announced) => {
                    let (online, asleep): (Vec<_>, Vec<_>) = announced.into_iter().partition(|c| c.online);
                    (true, online.iter().map(|c| c.index).collect(), asleep)
                }
                Err(error) => {
                    log::warn!("{}: {error}", endpoint.describe());
                    (true, Vec::new(), Vec::new())
                }
            },
            Ok(Role::Direct(index)) => (false, vec![index], Vec::new()),
            Ok(Role::Foreign | Role::Silent) => continue,
            Err(error) => {
                log::warn!("{}: {error}", endpoint.describe());
                continue;
            }
        };
        let mut devices = Vec::new();
        for index in indices {
            match Device::open(&mut session, index) {
                Ok(device) => devices.push(device),
                Err(error) => log::warn!("{}: device {index:#04x}: {error}", endpoint.describe()),
            }
        }
        found.push(Found {
            endpoint,
            receiver,
            session,
            devices,
            asleep,
        });
    }
    Ok(found)
}

/// The first device whose name contains `wanted`, or the first device at all.
fn pick<'a>(found: &'a mut [Found], wanted: Option<&str>) -> anyhow::Result<(&'a mut Session<HidLink>, &'a Device)> {
    let names: Vec<String> = found
        .iter()
        .flat_map(|f| f.devices.iter().map(|d| d.name().to_owned()))
        .collect();
    let wanted_lower = wanted.map(str::to_lowercase);
    for Found { session, devices, .. } in found.iter_mut() {
        let matching = devices.iter().find(|device| {
            wanted_lower
                .as_ref()
                .is_none_or(|w| device.name().to_lowercase().contains(w))
        });
        if let Some(device) = matching {
            return Ok((session, device));
        }
    }
    if names.is_empty() {
        bail!("no Logitech HID++ devices found. {ACCESS_HINT}");
    }
    bail!(
        "no device matches {:?}; found: {}",
        wanted.unwrap_or_default(),
        names.join(", ")
    )
}

pub fn list() -> anyhow::Result<()> {
    let mut found = scan(crossbeam_channel::never())?;
    if found.is_empty() {
        println!("No Logitech HID++ devices found. {ACCESS_HINT}");
    }
    for f in &mut found {
        if f.receiver {
            let kind = receiver_name(f);
            println!("{kind} (USB, product {:#06x})", f.endpoint.product_id);
            let slots = receiver_slots(f);
            if slots.is_empty() {
                println!("  no paired devices");
            }
            for slot in slots {
                match f.devices.iter().find(|device| device.index() == slot.index) {
                    Some(device) => {
                        let battery = describe_battery(battery::read(&mut f.session, device).ok().flatten());
                        println!("  #{} {} — battery {battery}", slot.index, slot.name);
                    }
                    None => println!("  #{} {} — asleep or connected elsewhere", slot.index, slot.name),
                }
            }
            continue;
        }
        for device in &f.devices {
            let battery = describe_battery(battery::read(&mut f.session, device).ok().flatten());
            let via = if f.endpoint.bluetooth { "Bluetooth" } else { "USB" };
            println!("{} — {via} — battery {battery}", device.name());
        }
    }
    Ok(())
}

pub fn info(wanted: Option<&str>) -> anyhow::Result<()> {
    let mut found = scan(crossbeam_channel::never())?;
    let (session, device) = pick(&mut found, wanted)?;
    let (major, minor) = device.protocol();
    println!("{}", device.name());
    println!(
        "  {:<14} HID++ {major}.{minor}, device index {:#04x}",
        "Protocol",
        device.index()
    );
    row(
        "Battery",
        battery::read(session, device).and_then(|b| b.ok_or(Error::Unsupported(features::UNIFIED_BATTERY))),
        |b| describe_battery(Some(b)),
    );
    row("DPI", dpi::read(session, device), |d| {
        format!("{} (default {}; supports {})", d.current, d.default, d.choices)
    });
    row(
        "SmartShift",
        smartshift::read(session, device).and_then(|s| s.ok_or(Error::Unsupported(features::SMART_SHIFT))),
        describe_smartshift,
    );
    row("Scroll wheel", wheel::scroll_mode(session, device), |m| {
        format!("hi-res {}, inverted {}", on_off(m.hires), on_off(m.inverted))
    });
    let thumb = wheel::thumb_info(session, device)
        .and_then(|info| wheel::thumb_reporting(session, device).map(|reporting| (info, reporting)));
    row("Thumb wheel", thumb, |(info, reporting)| {
        format!(
            "resolution {} native / {} diverted, inverted {}, diverted {}",
            info.native_resolution,
            info.diverted_resolution,
            on_off(reporting.inverted),
            on_off(reporting.diverted)
        )
    });
    row("Easy-Switch", host::read(session, device), |h| {
        format!("channel {} of {}", h.current + 1, h.count)
    });

    if device.has(features::REPROG_CONTROLS_V4) {
        println!("\n  Buttons (config name, control id, description, capabilities):");
        for control in reprog::controls(session, device)? {
            let mut traits = Vec::new();
            if control.divertable() {
                traits.push("remappable");
            }
            if control.supports_raw_xy() {
                traits.push("gestures");
            }
            if control.is_virtual() {
                traits.push("virtual");
            }
            if reprog::reporting(session, device, control.cid).is_ok_and(|r| r.diverted) {
                traits.push("currently diverted");
            }
            println!(
                "    {:<11} {:#06x}  {:<24} {}",
                ButtonId(control.cid).to_string(),
                control.cid,
                control.name().unwrap_or("-"),
                traits.join(", ")
            );
        }
    }

    println!("\n  Features:");
    for (id, index) in device.all_features(session)? {
        println!("    [{index:>2}] {id:#06x}  {}", features::name(id).unwrap_or("-"));
    }
    Ok(())
}

pub fn set_dpi(value: u16, wanted: Option<&str>) -> anyhow::Result<()> {
    let mut found = scan(crossbeam_channel::never())?;
    let (session, device) = pick(&mut found, wanted)?;
    let current = dpi::read(session, device)?;
    let target = current.choices.nearest(value).unwrap_or(value);
    dpi::set(session, device, target)?;
    if target == value {
        println!("{}: DPI set to {target}", device.name());
    } else {
        println!("{}: {value} DPI isn't supported; set to {target}", device.name());
    }
    Ok(())
}

/// Diverts the named controls and prints what the device reports until Ctrl+C,
/// then hands the controls back.
pub fn events(wanted: Option<&str>, divert: &[String]) -> anyhow::Result<()> {
    let mut thumb = false;
    let mut ids = Vec::new();
    for name in divert {
        if name.eq_ignore_ascii_case("thumbwheel") {
            thumb = true;
        } else {
            ids.push(ButtonId::parse(name).with_context(|| format!("unknown control `{name}`"))?);
        }
    }

    let mut found = scan(daemon::stop_signal()?)?;
    let (session, device) = pick(&mut found, wanted)?;
    let controls = if ids.is_empty() {
        Vec::new()
    } else {
        reprog::controls(session, device)?
    };
    let mut diverted = Vec::new();
    for id in &ids {
        let control = controls
            .iter()
            .find(|control| control.cid == id.0 && control.divertable())
            .with_context(|| format!("{} has no remappable `{id}` control", device.name()))?;
        let reporting = Reporting {
            diverted: true,
            raw_xy: control.supports_raw_xy(),
        };
        reprog::set_reporting(session, device, control, reporting)?;
        diverted.push(*control);
    }
    let thumb_before = if thumb {
        let before = wheel::thumb_reporting(session, device)?;
        wheel::set_thumb_reporting(
            session,
            device,
            ThumbReporting {
                diverted: true,
                ..before
            },
        )?;
        Some(before)
    } else {
        None
    };

    println!("Watching {}. Use the diverted controls; Ctrl+C to stop.", device.name());
    let ended = loop {
        match session.next_event(EVENT_WAIT) {
            Ok(Some(report)) => match device.parse_event(&report) {
                Some(event) => println!("{}", describe_event(&event)),
                None => log::debug!("unhandled report {report:?}"),
            },
            Ok(None) => {}
            Err(error) => break error,
        }
    };
    if ended != Error::Stopped {
        bail!("{}: {ended}", device.name());
    }
    for control in &diverted {
        reprog::set_reporting(session, device, control, Reporting::default())?;
    }
    if let Some(before) = thumb_before {
        wheel::set_thumb_reporting(session, device, before)?;
    }
    println!("Controls handed back to {}.", device.name());
    Ok(())
}

/// Receivers whose kind or product id contains `wanted`, or all of them.
fn receivers<'a>(found: &'a mut [Found], wanted: Option<&str>) -> anyhow::Result<Vec<&'a mut Found>> {
    let wanted_lower = wanted.map(str::to_lowercase);
    let receivers: Vec<&mut Found> = found
        .iter_mut()
        .filter(|f| f.receiver)
        .filter(|f| {
            wanted_lower.as_ref().is_none_or(|w| {
                receiver_name(f).to_lowercase().contains(w) || format!("{:04x}", f.endpoint.product_id).contains(w)
            })
        })
        .collect();
    if receivers.is_empty() {
        match wanted {
            Some(wanted) => bail!("no receiver matches {wanted:?}"),
            None => bail!("no Logitech receiver found. {ACCESS_HINT}"),
        }
    }
    Ok(receivers)
}

fn receiver_name(found: &Found) -> &'static str {
    found.endpoint.receiver_kind().unwrap_or("Receiver")
}

/// Pairs a device with a receiver and reports what paired.
pub fn pair(wanted: Option<&str>, seconds: u8) -> anyhow::Result<()> {
    let mut found = scan(daemon::stop_signal()?)?;
    let mut receivers = receivers(&mut found, wanted)?;
    if receivers.len() > 1 {
        let names: Vec<String> = receivers
            .iter()
            .map(|f| format!("{} ({:04x})", receiver_name(f), f.endpoint.product_id))
            .collect();
        bail!(
            "found several receivers: {}; pick one with --receiver",
            names.join(", ")
        );
    }
    let target = receivers.remove(0);
    let kind = receiver_name(target);
    let protocol = Protocol::of(target.endpoint.product_id);
    let session = &mut target.session;

    match protocol {
        Protocol::Bolt => println!(
            "{kind} is looking for devices for {seconds} seconds. Hold the device's connect \
             or Easy-Switch button until its light blinks quickly. Ctrl+C to stop."
        ),
        Protocol::Unifying => println!(
            "{kind} is listening for {seconds} seconds. Turn the device off and on again, \
             or press its connect button. Ctrl+C to stop."
        ),
    }
    let mut prompt: Option<Prompt> = None;
    let outcome = receiver::pair(session, protocol, seconds, |step| match step {
        PairingStep::Passkey(passkey) => {
            let mut shown = Prompt::new(&passkey);
            shown.show();
            prompt = Some(shown);
        }
        PairingStep::Entered(entered) => {
            if let Some(prompt) = &mut prompt {
                prompt.update(entered);
            }
        }
        PairingStep::Submitted => {
            if let Some(prompt) = &mut prompt {
                prompt.submitted();
            }
        }
    });
    if let Some(prompt) = &mut prompt {
        prompt.close();
    }
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            // Stopped, timed out or failed part way: don't leave the receiver listening.
            receiver::cancel_pairing(session, protocol);
            if error == Error::Stopped {
                println!("Stopped; the receiver is no longer listening.");
                return Ok(());
            }
            bail!("pairing failed: {error}");
        }
    };
    match outcome {
        Paired::Device(connection) => {
            let name = match Device::open(session, connection.index) {
                Ok(device) => device.name().to_owned(),
                // Not answering yet: fall back to the name it gave the receiver.
                Err(_) => pairing_records(&target.endpoint, session)
                    .into_iter()
                    .find(|p| p.index == connection.index)
                    .map_or_else(|| format!("device #{}", connection.index), |p| record_name(&p)),
            };
            println!("Paired {name} as #{}.", connection.index);
            Ok(())
        }
        Paired::Failed(PairingError::Failed) if let Some(prompt) = &prompt => bail!("{}", prompt.rejection_hint()),
        Paired::Failed(error) => bail!("nothing paired: {error}"),
        Paired::Nothing => bail!("nothing paired before the receiver stopped listening"),
    }
}

/// The receiver's pairing records, or none if it doesn't keep them.
fn pairing_records(endpoint: &Endpoint, session: &mut Session<HidLink>) -> Vec<Pairing> {
    receiver::paired(session, Protocol::of(endpoint.product_id)).unwrap_or_else(|error| {
        log::debug!("{}: can't read its pairings: {error}", endpoint.describe());
        Vec::new()
    })
}

/// A slot on a receiver that holds a device.
struct Slot {
    index: u8,
    name: String,
}

/// Every slot on a receiver that holds a device, awake or not, in order.
///
/// Sleeping devices are named from the receiver's pairing records where it keeps
/// them. Otherwise, as on some Lightspeed receivers, only their wireless product
/// id is known, from the receiver's connection notices.
fn receiver_slots(f: &mut Found) -> Vec<Slot> {
    let records = pairing_records(&f.endpoint, &mut f.session);
    let mut indices: Vec<u8> = records
        .iter()
        .map(|r| r.index)
        .chain(f.devices.iter().map(Device::index))
        .chain(f.asleep.iter().map(|c| c.index))
        .collect();
    indices.sort_unstable();
    indices.dedup();
    indices
        .into_iter()
        .map(|index| {
            let name = f
                .devices
                .iter()
                .find(|d| d.index() == index)
                .map(|d| d.name().to_owned())
                .or_else(|| records.iter().find(|r| r.index == index).map(record_name))
                .or_else(|| {
                    f.asleep
                        .iter()
                        .find(|c| c.index == index)
                        .map(|c| format!("device {:#06x}", c.wireless_pid))
                })
                .unwrap_or_else(|| format!("device #{index}"));
            Slot { index, name }
        })
        .collect()
}

/// The name a device gave the receiver when it paired, or its wireless product id.
fn record_name(record: &Pairing) -> String {
    record
        .name
        .clone()
        .unwrap_or_else(|| format!("device {:#06x}", record.wireless_pid))
}

#[cfg(target_os = "macos")]
const FORGET_HINT: &str = "Remove it under System Settings → Bluetooth.";
#[cfg(target_os = "linux")]
const FORGET_HINT: &str = "Remove it in your Bluetooth settings, or with `bluetoothctl remove`.";
#[cfg(target_os = "windows")]
const FORGET_HINT: &str = "Remove it under Settings → Bluetooth & devices.";

/// A device a receiver has paired, for choosing one to unpair.
struct PairedDevice {
    receiver: usize,
    index: u8,
    name: String,
}

/// Unpairs the device named by `wanted`: part of its name, or its slot number.
pub fn unpair(wanted: &str, receiver_wanted: Option<&str>, yes: bool) -> anyhow::Result<()> {
    let mut found = scan(crossbeam_channel::never())?;
    let slot = wanted.trim_start_matches('#').parse::<u8>().ok();
    let wanted_lower = wanted.to_lowercase();

    // Bluetooth pairings belong to the computer, and a cable needs none: say so
    // rather than that nothing matched.
    let direct = found
        .iter()
        .filter(|f| !f.receiver && slot.is_none())
        .flat_map(|f| f.devices.iter().map(|d| (d.name().to_owned(), f.endpoint.bluetooth)))
        .find(|(name, _)| name.to_lowercase().contains(&wanted_lower));
    let not_on_a_receiver = |(name, bluetooth): (String, bool)| {
        if bluetooth {
            anyhow::anyhow!(
                "{name} is connected over Bluetooth, so the computer holds its pairing, not a receiver. {FORGET_HINT}"
            )
        } else {
            anyhow::anyhow!("{name} is connected by USB cable; there's no pairing to remove")
        }
    };
    let mut receivers = match receivers(&mut found, receiver_wanted) {
        Ok(receivers) => receivers,
        Err(error) => return Err(direct.map_or(error, not_on_a_receiver)),
    };

    let mut devices = Vec::new();
    for (position, f) in receivers.iter_mut().enumerate() {
        for Slot { index, name } in receiver_slots(f) {
            devices.push(PairedDevice {
                receiver: position,
                index,
                name,
            });
        }
    }

    let matches: Vec<&PairedDevice> = devices
        .iter()
        .filter(|d| slot.map_or_else(|| d.name.to_lowercase().contains(&wanted_lower), |slot| d.index == slot))
        .collect();
    let describe = |d: &PairedDevice| {
        format!(
            "{} (#{} on the {})",
            d.name,
            d.index,
            receiver_name(receivers[d.receiver])
        )
    };
    if matches.is_empty()
        && let Some(direct) = direct
    {
        return Err(not_on_a_receiver(direct));
    }
    let target = match matches.as_slice() {
        [target] => *target,
        [] if devices.is_empty() => bail!("no paired devices found"),
        [] => {
            let names: Vec<String> = devices.iter().map(describe).collect();
            bail!("no paired device matches {wanted:?}; found: {}", names.join(", "))
        }
        several => {
            let names: Vec<String> = several.iter().map(|d| describe(d)).collect();
            bail!(
                "{wanted:?} matches several devices: {}; be more specific, or pick the receiver with --receiver",
                names.join(", ")
            )
        }
    };

    let label = describe(target);
    let f = &mut receivers[target.receiver];
    if !yes && let Some(path) = unconfigured(&target.name) {
        eprintln!(
            "warning: no [[device]] in {} matches {}, so quietmouse isn't setting it up. \
             It may be a keyboard or another device you rely on.",
            path.display(),
            target.name
        );
        if !confirm(&format!("Unpair {label} anyway?"))? {
            bail!("left {label} paired");
        }
    }
    match receiver::unpair(&mut f.session, Protocol::of(f.endpoint.product_id), target.index) {
        Ok(()) => {
            println!("Unpaired {label}. Pair it again with `quietmouse pair`.");
            Ok(())
        }
        Err(Error::Hidpp10(_)) if f.endpoint.receiver_kind() == Some("Nano receiver") => bail!(
            "the Nano receiver refused to unpair {}; Nano receivers don't unpair, \
             but pairing another device replaces it",
            target.name
        ),
        Err(error) => bail!("couldn't unpair {label}: {error}"),
    }
}

/// The config's path, if there is a config and no profile in it matches `device_name`.
/// Without a config quietmouse sets nothing up, so there's nothing to compare against.
fn unconfigured(device_name: &str) -> Option<PathBuf> {
    let path = Config::resolve_path(None).ok()?;
    if !path.exists() {
        return None;
    }
    match Config::load(&path) {
        Ok(config) => config.profile_for(device_name).is_none().then_some(path),
        Err(error) => {
            log::debug!("can't read {}: {error:#}", path.display());
            None
        }
    }
}

/// Asks a yes/no question on the terminal; anything but yes is no. Without a
/// terminal to ask on, it stops and says to pass `--yes`.
fn confirm(question: &str) -> anyhow::Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not unpairing without confirmation; pass --yes to go ahead");
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

pub fn config(path: Option<PathBuf>, init: bool, check: bool) -> anyhow::Result<()> {
    let path = Config::resolve_path(path)?;
    if init {
        if path.exists() {
            bail!("{} already exists", path.display());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("can't create {}", dir.display()))?;
        }
        std::fs::write(&path, example()).with_context(|| format!("can't write {}", path.display()))?;
        println!("Wrote an example config to {}", path.display());
    } else if check {
        let config = Config::load(&path)?;
        println!(
            "{} is valid ({} device profile(s))",
            path.display(),
            config.devices.len()
        );
    } else {
        println!("{}", path.display());
        if !path.exists() {
            eprintln!("(doesn't exist yet; create it with `quietmouse config --init`)");
        }
    }
    Ok(())
}

/// Prints one `label value` line, skipping features the device doesn't have.
fn row<T>(label: &str, value: hidpp::Result<T>, describe: impl FnOnce(T) -> String) {
    match value {
        Ok(value) => println!("  {label:<14} {}", describe(value)),
        Err(Error::Unsupported(_)) => {}
        Err(error) => println!("  {label:<14} unavailable ({error})"),
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn describe_battery(battery: Option<Battery>) -> String {
    let Some(battery) = battery else {
        return "unknown".to_owned();
    };
    let level = match (battery.percent, battery.millivolts) {
        (Some(percent), _) => format!("{percent}%"),
        (None, Some(millivolts)) => format!("{millivolts} mV"),
        (None, None) => "level unknown".to_owned(),
    };
    let state = match battery.state {
        ChargeState::Discharging => "",
        ChargeState::Charging => ", charging",
        ChargeState::Full => ", full",
        ChargeState::Error => ", charging error",
        ChargeState::Unknown => "",
    };
    format!("{level}{state}")
}

fn describe_smartshift(state: SmartShift) -> String {
    let mode = match (state.mode, state.auto_disengage) {
        (Some(WheelMode::Freespin), _) => "free-spin".to_owned(),
        (Some(WheelMode::Ratchet), NEVER_DISENGAGE) => "always ratchet".to_owned(),
        (Some(WheelMode::Ratchet), threshold) => {
            format!(
                "auto, frees at {threshold} (device default {})",
                state.default_auto_disengage
            )
        }
        (None, _) => "unknown mode".to_owned(),
    };
    match state.torque {
        Some(torque) => format!("{mode}; ratchet torque {torque}%"),
        None => mode,
    }
}

fn describe_event(event: &DeviceEvent) -> String {
    match event {
        DeviceEvent::Buttons(held) if held.is_empty() => "released".to_owned(),
        DeviceEvent::Buttons(held) => {
            let names: Vec<String> = held.iter().map(|&cid| ButtonId(cid).to_string()).collect();
            format!("held: {}", names.join(", "))
        }
        DeviceEvent::RawXy { dx, dy } => format!("move  dx {dx:+5}  dy {dy:+5}"),
        DeviceEvent::ThumbWheel { rotation, start } => {
            format!(
                "thumb wheel {rotation:+}{}",
                if *start { "  (new movement)" } else { "" }
            )
        }
        DeviceEvent::Battery(battery) => format!("battery {}", describe_battery(Some(*battery))),
        DeviceEvent::Reconnected => "reconnected: the device reset its diversion; restart to divert again".to_owned(),
    }
}
