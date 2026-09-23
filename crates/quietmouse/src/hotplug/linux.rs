//! Linux: inotify on `/dev`, where each HID interface is a `hidraw` node.
//!
//! udev gives access to a node just after it's created, so opening it the moment
//! it appears can be refused. The access change is an attribute change on the
//! node, which is watched too, so the rescan after it opens the device.

use std::thread;

use anyhow::Context;
use nix::errno::Errno;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};

pub fn start() -> anyhow::Result<()> {
    let inotify = Inotify::init(InitFlags::IN_CLOEXEC).context("inotify")?;
    inotify
        .add_watch(
            "/dev",
            AddWatchFlags::IN_CREATE | AddWatchFlags::IN_DELETE | AddWatchFlags::IN_ATTRIB,
        )
        .context("can't watch /dev")?;
    thread::Builder::new().name("hotplug".into()).spawn(move || {
        loop {
            match inotify.read_events() {
                Ok(events) => {
                    // An overflow means events were dropped, any of which could have been a hidraw node.
                    let hidraw = events.iter().any(|event| {
                        event.mask.contains(AddWatchFlags::IN_Q_OVERFLOW)
                            || event
                                .name
                                .as_ref()
                                .is_some_and(|name| name.as_encoded_bytes().starts_with(b"hidraw"))
                    });
                    if hidraw {
                        super::changed();
                    }
                }
                Err(Errno::EINTR) => {}
                Err(error) => {
                    log::warn!("stopped watching /dev: {error}");
                    super::lost();
                    return;
                }
            }
        }
    })?;
    Ok(())
}
