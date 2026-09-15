//! Working out what an endpoint is, and which devices are reachable through it.

use std::time::{Duration, Instant};

use hidpp::{DIRECT, Device, Error, Link, Session, receiver};

use crate::hid::Endpoint;

pub enum Role {
    Receiver,
    /// A device connected directly, answering at this index.
    Direct(u8),
}

/// Indices a directly connected device may answer on: most use 0xFF, some wired devices 0x00.
const DIRECT_INDICES: [u8; 2] = [DIRECT, 0x00];

/// Identifies the endpoint, or `None` if nothing on it speaks HID++ 2.0.
pub fn probe<L: Link>(session: &mut Session<L>, endpoint: &Endpoint) -> hidpp::Result<Option<Role>> {
    if endpoint.receiver_kind().is_some() {
        return Ok(Some(Role::Receiver));
    }
    for index in DIRECT_INDICES {
        match Device::ping(session, index) {
            Ok(_) => return Ok(Some(Role::Direct(index))),
            // Receivers answer a HID++ 2.0 ping with a HID++ 1.0 error.
            Err(Error::Hidpp10(_)) if index == DIRECT => return Ok(Some(Role::Receiver)),
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => log::debug!("{}: nothing at index {index:#04x}: {error}", endpoint.describe()),
        }
    }
    Ok(None)
}

/// Indices of the devices online behind a receiver, gathered from the
/// connection notices it sends within `wait` of being asked.
pub fn receiver_devices<L: Link>(session: &mut Session<L>, wait: Duration) -> hidpp::Result<Vec<u8>> {
    receiver::enable_notifications(session)?;
    receiver::announce_devices(session)?;
    let deadline = Instant::now() + wait;
    let mut online = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(report) = session.next_event(remaining)? else {
            break;
        };
        if let Some(connection) = receiver::parse_connection(&report)
            && connection.online
            && !online.contains(&connection.index)
        {
            online.push(connection.index);
        }
    }
    online.sort_unstable();
    Ok(online)
}
