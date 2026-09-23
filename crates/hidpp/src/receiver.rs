//! Receivers (Unifying, Bolt, Lightspeed, Nano): HID++ 1.0 register access, the
//! connection notifications that say when a paired device wakes up, and pairing.
//!
//! Pairing comes in two protocols, picked by [`Protocol::of`]: Bolt receivers
//! discover a device and have a passkey entered on it ([`bolt`]); the others
//! open a pairing lock ([`unifying`]).

use std::fmt;

use crate::{DIRECT, Error, Link, Report, Result, Session};

mod bolt;
mod unifying;

pub use bolt::Passkey;

const REG_NOTIFICATIONS: u8 = 0x00;
const REG_CONNECTION_STATE: u8 = 0x02;
/// Long register of receiver information, one sub-register per record.
const REG_RECEIVER_INFO: u8 = 0xB5;
/// Notification flags: wireless connection events, plus "host software is present".
const NOTIFY_WIRELESS_AND_SOFTWARE: [u8; 3] = [0x00, 0x09, 0x00];
/// Connection-state command asking the receiver to re-announce every paired device.
const ANNOUNCE_DEVICES: [u8; 3] = [0x02, 0x00, 0x00];

const CONNECTION_NOTIFICATION: u8 = 0x41;
const LINK_NOT_ESTABLISHED: u8 = 0x40;
/// Highest device index a receiver hands out.
const MAX_DEVICE_INDEX: u8 = 0x0F;
/// Bolt receiver's USB product id.
const BOLT: u16 = 0xC548;
/// Nano receivers' USB product ids.
const NANO: [u16; 2] = [0xC52F, 0xC534];
/// Sub-register of [`REG_RECEIVER_INFO`] with the receiver's serial and slot count:
/// `[sub-register, serial x4, -, slots, ...]`.
const RECEIVER_INFORMATION: u8 = 0x03;
/// The most devices any receiver holds: six on Unifying and Bolt ones.
const MAX_SLOTS: u8 = 6;

/// Receiver family for USB product ids known to be receivers. Unknown receivers
/// still work: they're recognised by answering a HID++ 2.0 ping with a 1.0 error.
pub fn kind(product_id: u16) -> Option<&'static str> {
    match product_id {
        0xC52B | 0xC532 => Some("Unifying receiver"),
        BOLT => Some("Bolt receiver"),
        0xC539 | 0xC53A | 0xC53F | 0xC547 => Some("Lightspeed receiver"),
        id if NANO.contains(&id) => Some("Nano receiver"),
        _ => None,
    }
}

/// How a receiver pairs devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// A pairing lock, opened for a while (Unifying, Lightspeed, Nano).
    Unifying,
    /// Discovery, then a passkey entered on the device.
    Bolt,
}

impl Protocol {
    /// The protocol of the receiver with this USB product id. Unknown receivers
    /// are taken to be older ones, which all use the pairing lock.
    pub fn of(product_id: u16) -> Self {
        if product_id == BOLT { Self::Bolt } else { Self::Unifying }
    }
}

/// Whether pairing a new device replaces one already paired, as on Nano
/// receivers, so a receiver that's full needs no room made.
pub fn replaces_pairings(product_id: u16) -> bool {
    NANO.contains(&product_id)
}

/// How many devices the receiver can hold, if it says.
pub fn capacity<L: Link>(session: &mut Session<L>, protocol: Protocol) -> Result<Option<u8>> {
    // Bolt receivers don't report it, and always hold six.
    if protocol == Protocol::Bolt {
        return Ok(Some(MAX_SLOTS));
    }
    match session.read_long_register(DIRECT, REG_RECEIVER_INFO, &[RECEIVER_INFORMATION]) {
        // Receivers that answer with nonsense are treated as not saying.
        Ok(info) => Ok(Some(info.param(6)).filter(|slots| (1..=MAX_SLOTS).contains(slots))),
        Err(Error::Hidpp10(_)) => Ok(None),
        Err(error) => Err(error),
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

/// A device the receiver has a pairing record for, awake or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub index: u8,
    pub wireless_pid: u16,
    /// The name the device gave when it paired, if the receiver keeps one.
    pub name: Option<String>,
}

/// Why a receiver finished pairing without pairing anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingError {
    /// No device asked to pair before the receiver stopped listening.
    DeviceTimeout,
    /// A device tried, but the receiver can't take it.
    Unsupported,
    /// Every slot is taken.
    TooManyDevices,
    /// A device started pairing but didn't finish.
    SequenceTimeout,
    /// The receiver gave up on the device, e.g. after a wrong passkey (Bolt).
    Failed,
    Other(u8),
}

impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceTimeout => f.write_str("no device asked to pair in time"),
            Self::Unsupported => f.write_str("the device isn't supported by this receiver"),
            Self::TooManyDevices => f.write_str("the receiver has no free slots; unpair a device first"),
            Self::SequenceTimeout => f.write_str("the device started pairing but didn't finish"),
            Self::Failed => f.write_str("the device didn't pair; check the passkey and try again"),
            Self::Other(code) => write!(f, "pairing error {code:#04x}"),
        }
    }
}

/// Something the person has to do, or has done, while a device pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingStep {
    /// The device needs this passkey entered (Bolt).
    Passkey(Passkey),
    /// How many of the passkey's digits or clicks the device has had so far.
    /// It can go down: a keyboard can erase a digit.
    Entered(u8),
    /// The device finished entering the passkey; the receiver is checking it.
    Submitted,
}

/// How a pairing attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paired {
    /// A device paired and connected at this slot.
    Device(Connection),
    /// The receiver stopped, reporting why nothing paired.
    Failed(PairingError),
    /// The receiver stopped without saying anything paired.
    Nothing,
}

/// Every device the receiver has a pairing record for. Empty slots, and
/// receivers that don't keep records (some Lightspeed ones), give nothing.
pub fn paired<L: Link>(session: &mut Session<L>, protocol: Protocol) -> Result<Vec<Pairing>> {
    match protocol {
        Protocol::Unifying => unifying::paired(session),
        Protocol::Bolt => bolt::paired(session),
    }
}

/// Has the receiver look for a device for `seconds`, pairs it, and waits for
/// the result. `step` hears what the person needs to do along the way: with
/// Bolt, the passkey to enter and how far they've got with it.
///
/// On an error, including [`crate::Error::Stopped`], the receiver may still be
/// looking; [`cancel_pairing`] stops it.
pub fn pair<L: Link>(
    session: &mut Session<L>,
    protocol: Protocol,
    seconds: u8,
    step: impl FnMut(PairingStep),
) -> Result<Paired> {
    match protocol {
        Protocol::Unifying => unifying::pair(session, seconds),
        Protocol::Bolt => bolt::pair(session, seconds, step),
    }
}

/// Stops a pairing attempt. Errors are ignored: a receiver refuses to stop
/// what isn't running.
pub fn cancel_pairing<L: Link>(session: &mut Session<L>, protocol: Protocol) {
    match protocol {
        Protocol::Unifying => {
            let _ = unifying::close_lock(session);
        }
        Protocol::Bolt => bolt::cancel(session),
    }
}

/// Removes the pairing in slot `index`. The device stops working with this
/// receiver until it's paired again.
pub fn unpair<L: Link>(session: &mut Session<L>, protocol: Protocol, index: u8) -> Result<()> {
    match protocol {
        Protocol::Unifying => unifying::unpair(session, index),
        Protocol::Bolt => bolt::unpair(session, index),
    }
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
    use crate::testing::MockLink;

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
    fn reads_how_many_devices_a_receiver_holds() {
        let receiver = |slots: u8| {
            MockLink::new(move |request| {
                vec![Report::long(
                    DIRECT,
                    request.sub_id(),
                    request.address(),
                    &[RECEIVER_INFORMATION, 0x12, 0x34, 0x56, 0x78, 0x00, slots],
                )]
            })
        };
        assert_eq!(
            capacity(&mut Session::new(receiver(6)), Protocol::Unifying),
            Ok(Some(6))
        );
        assert_eq!(capacity(&mut Session::new(receiver(0)), Protocol::Unifying), Ok(None));
        let refusing = MockLink::new(|request| {
            vec![Report::short(
                DIRECT,
                0x8F,
                request.sub_id(),
                &[request.address(), 0x02],
            )]
        });
        assert_eq!(capacity(&mut Session::new(refusing), Protocol::Unifying), Ok(None));
        let silent = MockLink::new(|_| Vec::new());
        assert_eq!(capacity(&mut Session::new(silent), Protocol::Bolt), Ok(Some(6)));
    }

    #[test]
    fn knows_common_receivers() {
        assert_eq!(kind(0xC548), Some("Bolt receiver"));
        assert_eq!(kind(0xC52B), Some("Unifying receiver"));
        assert_eq!(kind(0xB034), None);
        assert_eq!(Protocol::of(0xC548), Protocol::Bolt);
        assert_eq!(Protocol::of(0xC52B), Protocol::Unifying);
        assert_eq!(Protocol::of(0xC999), Protocol::Unifying);
        assert!(replaces_pairings(0xC534));
        assert!(!replaces_pairings(0xC52B));
    }
}
