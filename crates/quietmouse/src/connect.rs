//! Working out what an endpoint is, and which devices are reachable through it.

use std::time::{Duration, Instant};

use hidpp::receiver::{self, Connection};
use hidpp::{DIRECT, Device, Error, Link, Session};

use crate::hid::Endpoint;

/// What an endpoint turned out to be.
pub enum Role {
    Receiver,
    /// A device connected directly, answering at this index.
    Direct(u8),
    /// Something answered, but not HID++ 2.0: there's nothing here to drive.
    Foreign,
    /// Nothing answered at all. A directly connected device that's asleep looks
    /// exactly like this, so it's worth asking again rather than writing it off.
    Silent,
}

/// Indices a directly connected device may answer on: most use 0xFF, some wired devices 0x00.
const DIRECT_INDICES: [u8; 2] = [DIRECT, 0x00];

/// Identifies the endpoint, allowing each ping `timeout` to answer.
///
/// Silence is kept apart from a reply we don't like: an endpoint that says
/// nothing may simply be a device that's asleep, which the caller should ask
/// again rather than skip for good. Somewhere that asks repeatedly can pass a
/// short `timeout` and let the next attempt do the waiting; a one-shot command
/// should pass [`hidpp::DEFAULT_TIMEOUT`], since it has no next attempt.
pub fn probe<L: Link>(session: &mut Session<L>, endpoint: &Endpoint, timeout: Duration) -> hidpp::Result<Role> {
    if endpoint.receiver_kind().is_some() {
        return Ok(Role::Receiver);
    }
    let mut answered = false;
    for index in DIRECT_INDICES {
        match session.with_timeout(timeout, |session| Device::ping(session, index)) {
            Ok(_) => return Ok(Role::Direct(index)),
            // Receivers answer a HID++ 2.0 ping with a HID++ 1.0 error.
            Err(Error::Hidpp10(_)) if index == DIRECT => return Ok(Role::Receiver),
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => {
                answered |= error != Error::Timeout;
                log::debug!("{}: nothing at index {index:#04x}: {error}", endpoint.describe());
            }
        }
    }
    Ok(if answered { Role::Foreign } else { Role::Silent })
}

/// What a receiver says of each device paired with it, from the connection
/// notices it sends within `wait` of being asked: the latest one per slot, in
/// slot order. Devices that are asleep, or connected elsewhere, come back offline.
pub fn receiver_devices<L: Link>(session: &mut Session<L>, wait: Duration) -> hidpp::Result<Vec<Connection>> {
    receiver::enable_notifications(session)?;
    receiver::announce_devices(session)?;
    let deadline = Instant::now() + wait;
    let mut announced: Vec<Connection> = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(report) = session.next_event(remaining)? else {
            break;
        };
        if let Some(connection) = receiver::parse_connection(&report) {
            announced.retain(|earlier| earlier.index != connection.index);
            announced.push(connection);
        }
    }
    announced.sort_unstable_by_key(|connection| connection.index);
    Ok(announced)
}
