//! Word from the system that a HID device arrived or left, so the agent rescans
//! when something changes instead of every half second.
//!
//! Each platform has its own way to say so: inotify on `/dev` on Linux, IOKit
//! service notifications on macOS, and configuration manager notifications on
//! Windows. None of them needs admin rights or opens a device. macOS and Windows
//! only offer C callbacks, so their files are the two places besides
//! [`permissions`](crate::permissions) where quietmouse uses `unsafe`.

use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender};

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;

/// Where [`changed`] sends to. The platform callbacks can't carry a sender of
/// their own without passing a pointer through C, so they reach it here.
static CHANGES: Mutex<Option<Sender<()>>> = Mutex::new(None);

/// A channel that gets a message soon after a HID device arrives or leaves, and
/// disconnects if the system stops saying. `None` when the system won't say at
/// all, which leaves rescanning on a timer. Meant to be called once, by the agent.
pub fn watch() -> Option<Receiver<()>> {
    // Room for one: a burst of changes becomes a single message.
    let (tx, changes) = crossbeam_channel::bounded(1);
    *CHANGES.lock().ok()? = Some(tx);
    match platform::start() {
        Ok(()) => Some(changes),
        Err(error) => {
            log::warn!("can't watch for devices arriving and leaving: {error:#}");
            lost();
            None
        }
    }
}

/// Called by the platform watcher when a device arrives or leaves.
fn changed() {
    if let Ok(changes) = CHANGES.lock()
        && let Some(tx) = changes.as_ref()
    {
        // Full means a rescan is already due, which covers this change too.
        let _ = tx.try_send(());
    }
}

/// Called when the platform watcher stops, disconnecting the channel from [`watch`].
fn lost() {
    if let Ok(mut changes) = CHANGES.lock() {
        changes.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watches_on_this_platform() {
        let changes = watch().expect("the system should say when devices arrive and leave");
        // Nothing is reported for the devices already attached.
        assert!(changes.try_recv().is_err_and(|error| error.is_empty()));
    }
}
