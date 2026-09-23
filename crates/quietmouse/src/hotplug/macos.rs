//! macOS: IOKit notifications for HID devices being registered and terminated.
//!
//! These come from the IOKit registry, so they need neither Input Monitoring nor
//! an open device. Every `IOHIDDevice` is watched, not just Logitech's: a
//! Bluetooth device's vendor ID is only on a property, and matching on it would
//! mean building Core Foundation dictionaries here. Another vendor's device
//! costing a one-millisecond rescan is the cheaper trade.

#![allow(
    unsafe_code,
    reason = "IOKit's device notifications are C callbacks with no safe wrapper"
)]

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::thread;

use anyhow::{anyhow, bail};

/// `MACH_PORT_NULL`, which IOKit takes as its default main port.
const DEFAULT_MAIN_PORT: u32 = 0;
/// `kIOFirstMatchNotification` and `kIOTerminatedNotification`.
const NOTIFICATIONS: [&CStr; 2] = [c"IOServiceFirstMatch", c"IOServiceTerminate"];

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IONotificationPortCreate(main_port: u32) -> *mut c_void;
    fn IONotificationPortGetRunLoopSource(port: *mut c_void) -> *mut c_void;
    fn IOServiceMatching(class: *const c_char) -> *mut c_void;
    fn IOServiceAddMatchingNotification(
        port: *mut c_void,
        notification: *const c_char,
        matching: *mut c_void,
        callback: extern "C" fn(*mut c_void, u32),
        context: *mut c_void,
        iterator: *mut u32,
    ) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOObjectRelease(object: u32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRun();
}

/// Starts a thread that runs the notifications' run loop, once they're set up.
pub fn start() -> anyhow::Result<()> {
    let (ready_tx, ready) = crossbeam_channel::bounded(1);
    thread::Builder::new().name("hotplug".into()).spawn(move || {
        let registered = register();
        let ok = registered.is_ok();
        let _ = ready_tx.send(registered);
        if ok {
            // Returns only if the run loop is left with nothing to watch.
            unsafe { CFRunLoopRun() };
            log::warn!("stopped watching for devices");
            super::lost();
        }
    })?;
    ready
        .recv()
        .map_err(|_| anyhow!("the device watcher ended while starting"))?
}

/// Sets up both notifications on this thread's run loop. The port and the
/// iterators are never released: they last as long as the agent does.
fn register() -> anyhow::Result<()> {
    let port = unsafe { IONotificationPortCreate(DEFAULT_MAIN_PORT) };
    if port.is_null() {
        bail!("IONotificationPortCreate failed");
    }
    for notification in NOTIFICATIONS {
        // Adding the notification takes this dictionary over, so each needs its own.
        let matching = unsafe { IOServiceMatching(c"IOHIDDevice".as_ptr()) };
        if matching.is_null() {
            bail!("IOServiceMatching failed");
        }
        let mut iterator = 0;
        let result = unsafe {
            IOServiceAddMatchingNotification(
                port,
                notification.as_ptr(),
                matching,
                changed,
                ptr::null_mut(),
                &mut iterator,
            )
        };
        if result != 0 {
            bail!("IOServiceAddMatchingNotification failed with {result:#x}");
        }
        // The iterator starts with every device already there, and the
        // notification only arms once they've been read off.
        drain(iterator);
    }
    unsafe {
        CFRunLoopAddSource(
            CFRunLoopGetCurrent(),
            IONotificationPortGetRunLoopSource(port),
            kCFRunLoopDefaultMode,
        );
    }
    Ok(())
}

/// Called on the watcher's run loop with the devices that arrived or left.
extern "C" fn changed(_context: *mut c_void, iterator: u32) {
    // Reading them off re-arms the notification.
    drain(iterator);
    super::changed();
}

fn drain(iterator: u32) {
    loop {
        let service = unsafe { IOIteratorNext(iterator) };
        if service == 0 {
            break;
        }
        unsafe { IOObjectRelease(service) };
    }
}
