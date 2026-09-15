//! `quietmouse run` and the background agent: watches for receivers and devices
//! and runs a worker for each.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Context;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use hidapi::HidApi;

use crate::config::Config;
use crate::hid::{self, HidLink};
use crate::inject::Injector;
use crate::service;
use crate::worker::{self, Outcome, Shared};

/// How often to look for newly attached receivers and devices, and for stop requests.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

pub fn run(config: Config) -> anyhow::Result<()> {
    let _instance = service::lock_instance()?;
    service::clear_stop_request();
    let shutdown = Shutdown::new();
    let injector = Injector::spawn().context("can't start keystroke output")?;
    let shared = Arc::new(Shared { config, injector });
    let mut api = HidApi::new().context("can't start HID access")?;
    let mut workers: HashMap<String, JoinHandle<Outcome>> = HashMap::new();
    // Endpoints left alone until they disappear: not HID++, or couldn't be opened.
    let mut skipped: HashSet<String> = HashSet::new();
    log::info!(
        "quietmouse {} running; stop with Ctrl+C or `quietmouse stop`",
        env!("CARGO_PKG_VERSION")
    );

    loop {
        reap(&mut workers, &mut skipped);
        if let Err(error) = api.refresh_devices() {
            log::warn!("device scan failed: {error}");
        }
        let endpoints = hid::discover(&api);
        skipped.retain(|key| endpoints.iter().any(|endpoint| &endpoint.key == key));
        for endpoint in endpoints {
            if workers.contains_key(&endpoint.key) || skipped.contains(&endpoint.key) {
                continue;
            }
            let link = match HidLink::open(&api, &endpoint, shutdown.receiver.clone()) {
                Ok(link) => link,
                Err(error) => {
                    log::warn!("{error:#}");
                    skipped.insert(endpoint.key);
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
