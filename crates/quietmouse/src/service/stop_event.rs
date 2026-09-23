//! Windows: a named event that `quietmouse stop` sets to stop the agent at once.
//! A windowless process has no console to send Ctrl+C to, and Windows has no
//! SIGTERM, so this stands in for the signal the other platforms use.

#![allow(
    unsafe_code,
    reason = "named events are Win32 calls with no safe wrapper in the standard library"
)]

use std::ptr;
use std::thread;

use anyhow::bail;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, ResetEvent, SetEvent, WaitForSingleObject,
};

/// `Local\` keeps it to this log-in session, so another user's `stop` can't
/// reach this agent, and nothing is needed to tell users apart.
const NAME: &str = r"Local\quietmouse.stop";

fn name() -> Vec<u16> {
    NAME.encode_utf16().chain([0]).collect()
}

/// Calls `on_stop` from a thread of its own once the event is set. The event
/// is never closed: it lasts as long as the agent does.
pub fn listen(on_stop: impl FnOnce() + Send + 'static) -> anyhow::Result<()> {
    let name = name();
    // Manual reset, so a stop set before the wait begins still counts.
    let event = unsafe { CreateEventW(ptr::null(), 1, 0, name.as_ptr()) };
    if event.is_null() {
        bail!("can't create the stop event: {}", std::io::Error::last_os_error());
    }
    // Clears a stop meant for an agent that had already exited, if a `stop`
    // still has the event open from then.
    unsafe { ResetEvent(event) };
    // A raw handle isn't `Send`; the address is, and the event stays open.
    let event = event as usize;
    thread::Builder::new().name("stop".into()).spawn(move || {
        if unsafe { WaitForSingleObject(event as HANDLE, INFINITE) } == WAIT_OBJECT_0 {
            on_stop();
        }
    })?;
    Ok(())
}

/// Sets the running agent's event. `false` when there isn't one: no agent, or
/// one from before this event existed, which only watches for the stop file.
pub fn request() -> bool {
    let name = name();
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if event.is_null() {
        return false;
    }
    let set = unsafe { SetEvent(event) } != 0;
    unsafe { CloseHandle(event) };
    set
}
