//! `quietmouse run` and the background agent: watches for receivers and devices
//! and runs a worker for each.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use hidapi::HidApi;

use crate::config::Config;
use crate::hid::{self, HidLink};
use crate::inject::Injector;
use crate::worker::{self, Outcome, Shared};
use crate::{permissions, service};

/// How often to look for newly attached receivers and devices, and for stop
/// requests. A mouse switched back from another computer comes back as a new
/// device, and its buttons do nothing special until the next look finds it, so
/// this is most of the wait after switching. Only Logitech's devices are listed,
/// which took about a millisecond on Windows against 25 to 90 for every HID
/// device, since Windows opens each one to read it; looking this often costs
/// next to nothing.
const SCAN_INTERVAL: Duration = Duration::from_millis(500);
/// How often to repeat a complaint about a device that won't open, usually
/// because macOS hasn't been given Input Monitoring yet. Opening is retried on
/// every scan, so granting it takes effect without restarting quietmouse.
const REPEAT_WARNING_EVERY: Duration = Duration::from_secs(60);

pub fn run(config: Config) -> anyhow::Result<()> {
    let _instance = service::lock_instance()?;
    service::clear_stop_request();
    let shutdown = Shutdown::new();
    let injector = Injector::spawn(config.desktop_switch_gap()).context("can't start keystroke output")?;
    let shared = Arc::new(Shared { config, injector });
    let mut api = HidApi::new().context("can't start HID access")?;
    let mut workers: HashMap<String, JoinHandle<Outcome>> = HashMap::new();
    // Endpoints left alone until they disappear, because nothing on them speaks HID++.
    let mut skipped: HashSet<String> = HashSet::new();
    // Endpoints that wouldn't open, and when that was last logged.
    let mut unopened: HashMap<String, Instant> = HashMap::new();
    // Set when quietmouse stops to pick up a permission it has just been given.
    let mut restart = false;
    log::info!(
        "quietmouse {} running; stop with Ctrl+C or `quietmouse stop`",
        env!("CARGO_PKG_VERSION")
    );
    // Asking here, from the agent itself, is what makes macOS prompt and list
    // quietmouse in its privacy settings, ready to be switched on.
    let mut permissions = permissions::request();
    for advice in permissions::advice(permissions) {
        log::error!("{advice}");
    }

    loop {
        reap(&mut workers, &mut skipped);
        // Only Logitech's devices are listed. HID++ over USB already needs that
        // check, since other vendors use the same page; over Bluetooth the page
        // is Logitech's own, and its devices report Logitech's vendor ID there
        // too (an MX Master 3S does on Windows).
        let scanned = api
            .reset_devices()
            .and_then(|()| api.add_devices(hidpp::LOGITECH_VID, 0));
        if let Err(error) = scanned {
            log::warn!("device scan failed: {error}");
        }
        let endpoints = hid::discover(&api);
        skipped.retain(|key| endpoints.iter().any(|endpoint| &endpoint.key == key));
        unopened.retain(|key, _| endpoints.iter().any(|endpoint| &endpoint.key == key));
        for endpoint in endpoints {
            if workers.contains_key(&endpoint.key) || skipped.contains(&endpoint.key) {
                continue;
            }
            let link = match HidLink::open(&api, &endpoint, shutdown.receiver.clone()) {
                Ok(link) => {
                    unopened.remove(&endpoint.key);
                    link
                }
                Err(error) => {
                    let complained = unopened.get(&endpoint.key);
                    if complained.is_none_or(|last| last.elapsed() >= REPEAT_WARNING_EVERY) {
                        log::warn!("{error:#}");
                        unopened.insert(endpoint.key.clone(), Instant::now());
                    }
                    continue;
                }
            };
            log::debug!("opened {}", endpoint.describe());
            let key = endpoint.key.clone();
            let shared = Arc::clone(&shared);
            let handle = thread::Builder::new()
                .name(format!("endpoint {:#06x}", endpoint.product_id))
                .spawn(move || worker::run(link, &endpoint, shared))?;
            workers.insert(key, handle);
        }
        if !permissions.all_granted() {
            let now = permissions::request();
            if now.newly_granted_since(permissions) {
                // macOS keeps to the answer it gave this process, so a fresh one
                // is the only way to use what was just granted. Exiting hands
                // that to launchd or systemd, which start quietmouse again.
                log::info!("a permission was granted; restarting to pick it up");
                shutdown.trigger();
                restart = true;
            }
            permissions = now;
        }
        if service::stop_requested() {
            log::info!("stop requested");
            shutdown.trigger();
        }
        match shutdown.receiver.recv_timeout(SCAN_INTERVAL) {
            Err(RecvTimeoutError::Disconnected) => break,
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
        }
    }

    log::info!("stopping");
    for (_, handle) in workers {
        if handle.join().is_err() {
            log::warn!("a device worker panicked while stopping");
        }
    }
    service::clear_stop_request();
    // A non-zero exit is what asks launchd and systemd to start quietmouse again.
    if restart {
        log::info!("restarting");
        drop(_instance);
        std::process::exit(1);
    }
    Ok(())
}

fn reap(workers: &mut HashMap<String, JoinHandle<Outcome>>, skipped: &mut HashSet<String>) {
    let finished: Vec<String> = workers
        .iter()
        .filter(|(_, handle)| handle.is_finished())
        .map(|(key, _)| key.clone())
        .collect();
    for key in finished {
        if let Some(handle) = workers.remove(&key)
            && matches!(handle.join(), Ok(Outcome::NotHidpp))
        {
            skipped.insert(key);
        }
    }
}

/// Ends the run on Ctrl+C, SIGTERM, or [`Shutdown::trigger`]. Links watch the
/// receiver, which disconnects on shutdown, so every worker wakes, hands its
/// buttons back to the device, and exits.
struct Shutdown {
    sender: Arc<Mutex<Option<Sender<()>>>>,
    receiver: Receiver<()>,
}

impl Shutdown {
    fn new() -> Self {
        let (tx, receiver) = crossbeam_channel::bounded(0);
        let sender = Arc::new(Mutex::new(Some(tx)));
        let handler = Arc::clone(&sender);
        // The windowless agent may have no console to get Ctrl+C from; stop requests still work.
        if let Err(error) = ctrlc::set_handler(move || release(&handler)) {
            log::warn!("can't watch for Ctrl+C: {error}");
        }
        Self { sender, receiver }
    }

    fn trigger(&self) {
        release(&self.sender);
    }
}

fn release(sender: &Mutex<Option<Sender<()>>>) {
    if let Ok(mut sender) = sender.lock() {
        sender.take();
    }
}

/// A channel that disconnects on Ctrl+C or SIGTERM.
pub fn stop_signal() -> anyhow::Result<Receiver<()>> {
    Ok(Shutdown::new().receiver)
}
