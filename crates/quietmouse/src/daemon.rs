//! `quietmouse run` and the background agent: watches for receivers and devices
//! and runs a worker for each.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossbeam_channel::{Receiver, Sender, select};
use hidapi::HidApi;

use crate::config::Config;
use crate::hid::{self, HidLink};
use crate::inject::Injector;
use crate::worker::{self, Outcome, Shared};
use crate::{hotplug, permissions, service};

/// How often to rescan when the system won't say when devices arrive and leave.
/// A mouse switched back from another computer comes back as a new device, and
/// its buttons do nothing special until the next look finds it, so this is most
/// of the wait after switching. Only Logitech's devices are listed, which took
/// about a millisecond on Windows against 25 to 90 for every HID device, since
/// Windows opens each one to read it; looking this often costs next to nothing.
const SCAN_INTERVAL: Duration = Duration::from_millis(500);
/// How long to wait for more changes before rescanning. One device arrives as
/// several collections in quick succession (each its own arrival on Windows),
/// and a scan partway through would open it without all of them.
const SETTLE: Duration = Duration::from_millis(100);
/// The longest a steady stream of changes can put off a rescan.
const SETTLE_AT_MOST: Duration = Duration::from_secs(1);
/// How long after a worker ends to rescan, which starts a new one if its device
/// is still there. The pause keeps a worker that fails straight away from
/// being restarted in a tight loop.
const RESTART_DELAY: Duration = Duration::from_millis(500);
/// How often to retry devices that wouldn't open. Nothing announces that one
/// can now be opened, so this is the one case the agent looks again unprompted.
const RETRY_OPEN_EVERY: Duration = Duration::from_secs(2);
/// How often to check for macOS permissions while one is still missing. Nothing
/// announces that one was granted either.
const PERMISSION_CHECK_EVERY: Duration = Duration::from_secs(1);
/// How often to repeat a complaint about a device that won't open, usually
/// because macOS hasn't been given Input Monitoring yet. Opening is retried
/// until it works, so granting it takes effect without restarting quietmouse.
const REPEAT_WARNING_EVERY: Duration = Duration::from_secs(60);

/// Runs until stopped. `agent` is set for the background agent, which launchd
/// or systemd starts again when it exits with a failure; `quietmouse run` in a
/// terminal has nothing to do that.
pub fn run(config: Config, agent: bool) -> anyhow::Result<()> {
    let _instance = service::lock_instance()?;
    let shutdown = Shutdown::new();
    let stopper = Arc::clone(&shutdown.sender);
    service::listen_for_stop(move || {
        log::info!("stop requested");
        release(&stopper);
    })
    .context("can't listen for stop requests")?;
    let injector = Injector::spawn(config.desktop_switch_gap()).context("can't start keystroke output")?;
    let shared = Arc::new(Shared {
        config,
        injector,
        known: Mutex::default(),
    });
    let mut api = HidApi::new().context("can't start HID access")?;
    let mut workers: HashMap<String, JoinHandle<Outcome>> = HashMap::new();
    // Each worker sends on this as it ends, however it ends.
    let (ended_tx, ended) = crossbeam_channel::unbounded();
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
    // Started before the first scan so that nothing arriving during it is missed.
    let mut changes = hotplug::watch();

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
            let ended = Ended(ended_tx.clone());
            let handle = thread::Builder::new()
                .name(format!("endpoint {:#06x}", endpoint.product_id))
                .spawn(move || {
                    let _ended = ended;
                    worker::run(link, &endpoint, shared)
                })?;
            workers.insert(key, handle);
        }
        if !permissions.all_granted() {
            let now = permissions::request();
            if now.newly_granted_since(permissions) {
                // macOS keeps to the answer it gave this process, so a fresh one
                // is the only way to use what was just granted. Exiting hands
                // that to launchd or systemd, which start quietmouse again.
                if agent {
                    log::info!("a permission was granted; restarting to pick it up");
                    shutdown.trigger();
                    restart = true;
                } else {
                    log::warn!("a permission was granted; stop and start `quietmouse run` again to pick it up");
                }
            }
            permissions = now;
        }
        // Nothing else wakes the loop for these, so they get timers, but only
        // while they're needed. An agent with every device open and nothing
        // missing sleeps until something changes.
        let look_again = [
            changes.is_none().then_some(SCAN_INTERVAL),
            (!unopened.is_empty()).then_some(RETRY_OPEN_EVERY),
            (!permissions.all_granted()).then_some(PERMISSION_CHECK_EVERY),
        ]
        .into_iter()
        .flatten()
        .min();
        let timer = look_again.map_or_else(crossbeam_channel::never, crossbeam_channel::after);
        let watched = changes.clone().unwrap_or_else(crossbeam_channel::never);
        select! {
            recv(shutdown.receiver) -> message => if message.is_err() {
                break;
            },
            recv(watched) -> message => if message.is_ok() {
                settle(&watched);
                log::debug!("devices changed; rescanning");
            } else {
                log::warn!("rescanning every {}ms instead", SCAN_INTERVAL.as_millis());
                changes = None;
            },
            recv(ended) -> _ => {
                while ended.try_recv().is_ok() {}
                if shutdown.receiver.recv_timeout(RESTART_DELAY).is_err_and(|error| error.is_disconnected()) {
                    break;
                }
            },
            recv(timer) -> _ => {}
        }
    }

    log::info!("stopping");
    for (_, handle) in workers {
        if handle.join().is_err() {
            log::warn!("a device worker panicked while stopping");
        }
    }
    // A non-zero exit is what asks launchd and systemd to start quietmouse again.
    if restart {
        log::info!("restarting");
        drop(_instance);
        std::process::exit(1);
    }
    Ok(())
}

/// Tells the daemon a worker has ended, when the worker's thread drops it,
/// including by panicking.
struct Ended(Sender<()>);

impl Drop for Ended {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// Waits for a burst of changes to end, so the rescan sees each device whole.
fn settle(changes: &Receiver<()>) {
    let deadline = Instant::now() + SETTLE_AT_MOST;
    while Instant::now() < deadline && changes.recv_timeout(SETTLE).is_ok() {}
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
