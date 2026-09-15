//! Easy-Switch host channels (CHANGE_HOST).

use crate::features::CHANGE_HOST;
use crate::{Device, Link, Result, Session};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hosts {
    pub count: u8,
    /// Zero-based channel in use.
    pub current: u8,
}

pub fn read<L: Link>(session: &mut Session<L>, device: &Device) -> Result<Hosts> {
    let reply = device.call(session, CHANGE_HOST, 0, &[])?;
    Ok(Hosts {
        count: reply.param(0),
        current: reply.param(1),
    })
}

/// Switches to zero-based channel `host`. The device drops off this computer without replying.
pub fn switch<L: Link>(session: &mut Session<L>, device: &Device, host: u8) -> Result<()> {
    device.call_no_reply(session, CHANGE_HOST, 1, &[host])
}
