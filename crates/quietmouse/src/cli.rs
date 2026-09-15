//! One-shot commands: list, info, dpi, events and config.

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
use hidpp::{Device, DeviceEvent, Error, Session};

use crate::config::{ButtonId, Config, example};
use crate::connect::{self, Role};
use crate::daemon;
use crate::hid::{self, Endpoint, HidLink};

/// Time allowed for a receiver to announce its paired devices.
const ANNOUNCE_WAIT: Duration = Duration::from_millis(800);
const EVENT_WAIT: Duration = Duration::from_secs(3600);

#[cfg(target_os = "macos")]
const ACCESS_HINT: &str =
    "If a device is connected, allow this app under System Settings → Privacy & Security → Input Monitoring.";
#[cfg(target_os = "linux")]
const ACCESS_HINT: &str = "If a device is connected, install packaging/linux/70-quietmouse.rules and replug it.";
#[cfg(target_os = "windows")]
const ACCESS_HINT: &str = "Check the device is connected and switched on.";

struct Found {
    endpoint: Endpoint,
    receiver: bool,
    session: Session<HidLink>,
    devices: Vec<Device>,
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
        let (receiver, indices) = match connect::probe(&mut session, &endpoint) {
            Ok(Some(Role::Receiver)) => match connect::receiver_devices(&mut session, ANNOUNCE_WAIT) {
                Ok(indices) => (true, indices),
                Err(error) => {
                    log::warn!("{}: {error}", endpoint.describe());
                    (true, Vec::new())
                }
            },
            Ok(Some(Role::Direct(index))) => (false, vec![index]),
            Ok(None) => continue,
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
    for Found {
        endpoint,
        receiver,
        session,
        devices,
    } in &mut found
    {
        if *receiver {
            let kind = endpoint.receiver_kind().unwrap_or("Receiver");
            println!("{kind} (USB, product {:#06x})", endpoint.product_id);
            if devices.is_empty() {
                println!("  no paired devices are awake");
            }
        }
        for device in devices.iter() {
            let battery = describe_battery(battery::read(session, device).ok().flatten());
            let slot = if *receiver {
                format!("  #{} ", device.index())
            } else {
                String::new()
            };
            let via = if *receiver {
                String::new()
            } else if endpoint.bluetooth {
                " — Bluetooth".to_owned()
            } else {
                " — USB".to_owned()
            };
            println!("{slot}{}{via} — battery {battery}", device.name());
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
