//! Receivers (Unifying, Bolt, Lightspeed, Nano): HID++ 1.0 register access and
//! the connection notifications that say when a paired device wakes up.

use crate::{DIRECT, Link, Report, Result, Session};

const REG_NOTIFICATIONS: u8 = 0x00;
const REG_CONNECTION_STATE: u8 = 0x02;
/// Notification flags: wireless connection events, plus "host software is present".
const NOTIFY_WIRELESS_AND_SOFTWARE: [u8; 3] = [0x00, 0x09, 0x00];
/// Connection-state command asking the receiver to re-announce every paired device.
const ANNOUNCE_DEVICES: [u8; 3] = [0x02, 0x00, 0x00];

const CONNECTION_NOTIFICATION: u8 = 0x41;
const LINK_NOT_ESTABLISHED: u8 = 0x40;
/// Highest device index a receiver hands out.
const MAX_DEVICE_INDEX: u8 = 0x0F;

/// Receiver family for USB product ids known to be receivers. Unknown receivers
/// still work: they're recognised by answering a HID++ 2.0 ping with a 1.0 error.
pub fn kind(product_id: u16) -> Option<&'static str> {
    match product_id {
        0xC52B | 0xC532 => Some("Unifying receiver"),
        0xC548 => Some("Bolt receiver"),
        0xC539 | 0xC53A | 0xC53F | 0xC547 => Some("Lightspeed receiver"),
        0xC52F | 0xC534 => Some("Nano receiver"),
        _ => None,
    }
}

/// A paired device coming online or dropping off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connection {
    pub index: u8,
    pub online: bool,
    /// Wireless product id, which differs from the device's USB/Bluetooth product id.
    pub wireless_pid: u16,
}

/// Parses a receiver's connection notification: `[id, index, 0x41, protocol, flags, wpid lo, wpid hi]`.
pub fn parse_connection(report: &Report) -> Option<Connection> {
    let index = report.device_index();
    (report.sub_id() == CONNECTION_NOTIFICATION && (1..=MAX_DEVICE_INDEX).contains(&index)).then(|| Connection {
        index,
        online: report.param(0) & LINK_NOT_ESTABLISHED == 0,
        wireless_pid: u16::from_le_bytes([report.param(1), report.param(2)]),
    })
}

/// Turns on connection notifications and tells the receiver host software is running.
pub fn enable_notifications<L: Link>(session: &mut Session<L>) -> Result<()> {
    session.write_register(DIRECT, REG_NOTIFICATIONS, &NOTIFY_WIRELESS_AND_SOFTWARE)?;
    Ok(())
}

/// Asks the receiver to send a connection notification for every paired device.
pub fn announce_devices<L: Link>(session: &mut Session<L>) -> Result<()> {
    session.write_register(DIRECT, REG_CONNECTION_STATE, &ANNOUNCE_DEVICES)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connection_notifications() {
        let online = Report::short(0x02, 0x41, 0x10, &[0x00, 0x34, 0xB0]);
        assert_eq!(
            parse_connection(&online),
            Some(Connection {
                index: 2,
                online: true,
                wireless_pid: 0xB034
            })
        );
        let offline = Report::short(0x02, 0x41, 0x10, &[0x60, 0x34, 0xB0]);
        assert_eq!(parse_connection(&offline).map(|c| c.online), Some(false));
    }

    #[test]
    fn ignores_other_reports() {
        assert_eq!(parse_connection(&Report::short(0xFF, 0x41, 0x04, &[0, 0, 0])), None);
        assert_eq!(parse_connection(&Report::long(0x01, 0x05, 0x00, &[])), None);
    }

    #[test]
    fn knows_common_receivers() {
        assert_eq!(kind(0xC548), Some("Bolt receiver"));
        assert_eq!(kind(0xC52B), Some("Unifying receiver"));
        assert_eq!(kind(0xB034), None);
    }
}
