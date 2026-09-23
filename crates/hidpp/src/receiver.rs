//! Receivers (Unifying, Bolt, Lightspeed, Nano): HID++ 1.0 register access, the
//! connection notifications that say when a paired device wakes up, and pairing.

use std::fmt;
use std::time::{Duration, Instant};

use crate::{DIRECT, Error, Link, Report, Result, Session};

const REG_NOTIFICATIONS: u8 = 0x00;
const REG_CONNECTION_STATE: u8 = 0x02;
/// Pairing lock: `[action, device index, seconds]`.
const REG_PAIRING: u8 = 0xB2;
/// Long register of receiver information, one sub-register per record.
const REG_RECEIVER_INFO: u8 = 0xB5;
const OPEN_LOCK: u8 = 0x01;
const CLOSE_LOCK: u8 = 0x02;
const UNPAIR: u8 = 0x03;
/// Sub-registers of [`REG_RECEIVER_INFO`] for slot 1; slot `n` adds `n - 1`.
const PAIRING_INFO: u8 = 0x20;
const DEVICE_NAME: u8 = 0x40;
/// Slots on a Unifying receiver, the most any of the pairing-lock receivers has.
const MAX_PAIRED: u8 = 6;
/// Notification flags: wireless connection events, plus "host software is present".
const NOTIFY_WIRELESS_AND_SOFTWARE: [u8; 3] = [0x00, 0x09, 0x00];
/// Connection-state command asking the receiver to re-announce every paired device.
const ANNOUNCE_DEVICES: [u8; 3] = [0x02, 0x00, 0x00];

const CONNECTION_NOTIFICATION: u8 = 0x41;
const LOCK_NOTIFICATION: u8 = 0x4A;
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

/// Whether the receiver takes the pairing lock (Unifying, Lightspeed, Nano).
/// Bolt pairs through discovery and a passkey instead, which isn't done here.
pub fn uses_pairing_lock(product_id: u16) -> bool {
    product_id != 0xC548
}

/// A device the receiver has a pairing record for, awake or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub index: u8,
    pub wireless_pid: u16,
    /// The name the device gave when it paired, if the receiver keeps one.
    pub name: Option<String>,
}

/// Every device the receiver has a pairing record for. Empty slots, and
/// receivers that don't keep records (some Lightspeed ones), give nothing.
pub fn paired<L: Link>(session: &mut Session<L>) -> Result<Vec<Pairing>> {
    let mut paired = Vec::new();
    for index in 1..=MAX_PAIRED {
        let slot = index - 1;
        // Params: [sub-register, destination id, report interval, wpid hi, wpid lo, ...].
        let info = match session.read_long_register(DIRECT, REG_RECEIVER_INFO, &[PAIRING_INFO + slot]) {
            Ok(info) => info,
            Err(Error::Hidpp10(_)) => continue,
            Err(error) => return Err(error),
        };
        let wireless_pid = info.u16_at(3);
        if wireless_pid == 0 {
            continue;
        }
        // Params: [sub-register, length, name...].
        let name = session
            .read_long_register(DIRECT, REG_RECEIVER_INFO, &[DEVICE_NAME + slot])
            .ok()
            .and_then(|reply| {
                let bytes = reply.params().get(2..)?;
                let bytes = &bytes[..usize::from(reply.param(1)).min(bytes.len())];
                let name = String::from_utf8_lossy(bytes).trim().to_owned();
                (!name.is_empty()).then_some(name)
            });
        paired.push(Pairing {
            index,
            wireless_pid,
            name,
        });
    }
    Ok(paired)
}

/// Why the receiver closed its pairing lock without pairing anything.
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
    Other(u8),
}

impl PairingError {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0x00 => return None,
            0x01 => Self::DeviceTimeout,
            0x02 => Self::Unsupported,
            0x03 => Self::TooManyDevices,
            0x06 => Self::SequenceTimeout,
            other => Self::Other(other),
        })
    }
}

impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceTimeout => f.write_str("no device asked to pair in time"),
            Self::Unsupported => f.write_str("the device isn't supported by this receiver"),
            Self::TooManyDevices => f.write_str("the receiver has no free slots; unpair a device first"),
            Self::SequenceTimeout => f.write_str("the device started pairing but didn't finish"),
            Self::Other(code) => write!(f, "pairing error {code:#04x}"),
        }
    }
}

/// The receiver opening or closing its pairing lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockStatus {
    pub open: bool,
    pub error: Option<PairingError>,
}

/// Parses a pairing-lock notification: `[id, 0xFF, 0x4A, open flag, error, ...]`.
pub fn parse_lock(report: &Report) -> Option<LockStatus> {
    (report.device_index() == DIRECT && report.sub_id() == LOCK_NOTIFICATION).then(|| LockStatus {
        open: report.address() & 0x01 != 0,
        error: PairingError::from_code(report.param(0)),
    })
}

/// How a pairing attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paired {
    /// A device paired and connected at this slot.
    Device(Connection),
    /// The receiver closed the lock, reporting why nothing paired.
    Failed(PairingError),
    /// The receiver closed the lock without saying anything paired.
    Nothing,
}

/// Opens the pairing lock for `seconds` and waits for the receiver to close it.
///
/// Turn the device on, or press its connect button, while the lock is open.
/// On an error, including [`Error::Stopped`], the lock may still be open;
/// [`close_pairing_lock`] shuts it.
pub fn pair<L: Link>(session: &mut Session<L>, seconds: u8) -> Result<Paired> {
    let before = paired(session)?;
    // Connection notices from before the lock opened would look like the new device.
    while session.next_event(Duration::ZERO)?.is_some() {}
    session.write_register(DIRECT, REG_PAIRING, &[OPEN_LOCK, 0x00, seconds])?;

    // The receiver closes the lock itself; this is only in case it never says so.
    let deadline = Instant::now() + Duration::from_secs(u64::from(seconds) + 5);
    // A device whose record is new beats one that merely woke up while the lock was open.
    let mut joined: Option<(Connection, bool)> = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(report) = session.next_event(remaining)? else {
            return Err(Error::Timeout);
        };
        if let Some(connection) = parse_connection(&report)
            && connection.online
        {
            let fresh = !before
                .iter()
                .any(|p| p.index == connection.index && p.wireless_pid == connection.wireless_pid);
            if fresh || joined.is_none_or(|(_, earlier_fresh)| !earlier_fresh) {
                joined = Some((connection, fresh));
            }
        } else if let Some(lock) = parse_lock(&report)
            && !lock.open
        {
            return Ok(match (lock.error, joined) {
                (Some(error), _) => Paired::Failed(error),
                (None, Some((connection, _))) => Paired::Device(connection),
                (None, None) => Paired::Nothing,
            });
        }
    }
}

/// Closes the pairing lock, ending an attempt early.
pub fn close_pairing_lock<L: Link>(session: &mut Session<L>) -> Result<()> {
    session.write_register(DIRECT, REG_PAIRING, &[CLOSE_LOCK, 0x00, 0x00])?;
    Ok(())
}

/// Removes the pairing in slot `index`. The device stops working with this
/// receiver until it's paired again.
pub fn unpair<L: Link>(session: &mut Session<L>, index: u8) -> Result<()> {
    session.write_register(DIRECT, REG_PAIRING, &[UNPAIR, index, 0x00])?;
    Ok(())
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

    const OLD_MOUSE: u16 = 0x4082;
    const NEW_MOUSE: u16 = 0x405E;

    /// A Unifying receiver with `OLD_MOUSE` in slot 1 that, once the lock opens,
    /// sends `after_open` and then closes the lock with `error`.
    fn fake_receiver(after_open: Vec<Report>, error: u8) -> MockLink {
        MockLink::new(
            move |request| match (request.sub_id(), request.address(), request.param(0)) {
                (0x83, REG_RECEIVER_INFO, PAIRING_INFO) => {
                    let [hi, lo] = OLD_MOUSE.to_be_bytes();
                    vec![Report::long(
                        DIRECT,
                        0x83,
                        REG_RECEIVER_INFO,
                        &[PAIRING_INFO, 0x61, 0x08, hi, lo],
                    )]
                }
                (0x83, REG_RECEIVER_INFO, DEVICE_NAME) => vec![Report::long(
                    DIRECT,
                    0x83,
                    REG_RECEIVER_INFO,
                    &[DEVICE_NAME, 5, b'M', b'X', b' ', b'3', b'S'],
                )],
                (0x83, REG_RECEIVER_INFO, sub) => {
                    vec![Report::short(DIRECT, 0x8F, 0x83, &[REG_RECEIVER_INFO, 0x02, sub])]
                }
                (0x80, REG_PAIRING, OPEN_LOCK) => {
                    let mut replies = vec![
                        Report::short(DIRECT, 0x80, REG_PAIRING, &[]),
                        Report::short(DIRECT, LOCK_NOTIFICATION, 0x01, &[0x00]),
                    ];
                    replies.extend(after_open.iter().copied());
                    replies.push(Report::short(DIRECT, LOCK_NOTIFICATION, 0x00, &[error]));
                    replies
                }
                _ => Vec::new(),
            },
        )
    }

    fn online(index: u8, wireless_pid: u16) -> Report {
        let [lo, hi] = wireless_pid.to_le_bytes();
        Report::short(index, CONNECTION_NOTIFICATION, 0x04, &[0x00, lo, hi])
    }

    #[test]
    fn reads_pairing_records() {
        let mut session = Session::new(fake_receiver(Vec::new(), 0));
        assert_eq!(
            paired(&mut session).unwrap(),
            vec![Pairing {
                index: 1,
                wireless_pid: OLD_MOUSE,
                name: Some("MX 3S".to_owned())
            }]
        );
    }

    #[test]
    fn pairs_the_new_device_not_one_that_woke_up() {
        let link = fake_receiver(vec![online(2, NEW_MOUSE), online(1, OLD_MOUSE)], 0);
        let mut session = Session::new(link);
        assert_eq!(
            pair(&mut session, 30).unwrap(),
            Paired::Device(Connection {
                index: 2,
                online: true,
                wireless_pid: NEW_MOUSE
            })
        );
        let open = session.link_mut().sent.last().copied().unwrap();
        assert_eq!(open.params(), &[OPEN_LOCK, 0x00, 30]);
    }

    #[test]
    fn a_repaired_device_still_counts() {
        let mut session = Session::new(fake_receiver(vec![online(1, OLD_MOUSE)], 0));
        assert!(matches!(pair(&mut session, 30), Ok(Paired::Device(c)) if c.index == 1));
    }

    #[test]
    fn reports_why_pairing_failed() {
        let mut session = Session::new(fake_receiver(Vec::new(), 0x03));
        assert_eq!(pair(&mut session, 30), Ok(Paired::Failed(PairingError::TooManyDevices)));
        let mut session = Session::new(fake_receiver(Vec::new(), 0x00));
        assert_eq!(pair(&mut session, 30), Ok(Paired::Nothing));
    }

    #[test]
    fn unpairs_a_slot() {
        let mut session = Session::new(fake_receiver(Vec::new(), 0));
        // The fake receiver doesn't answer unpair requests.
        assert_eq!(unpair(&mut session, 2), Err(Error::Timeout));
        assert_eq!(session.link_mut().sent[0].params(), &[UNPAIR, 2, 0x00]);

        let refusing = MockLink::new(|request| {
            vec![Report::short(
                DIRECT,
                0x8F,
                request.sub_id(),
                &[request.address(), 0x08],
            )]
        });
        let mut session = Session::new(refusing);
        assert_eq!(unpair(&mut session, 3), Err(Error::Hidpp10(0x08)));
    }

    #[test]
    fn parses_lock_notifications() {
        let open = Report::short(DIRECT, LOCK_NOTIFICATION, 0x01, &[0x00]);
        assert_eq!(
            parse_lock(&open),
            Some(LockStatus {
                open: true,
                error: None
            })
        );
        let failed = Report::short(DIRECT, LOCK_NOTIFICATION, 0x00, &[0x01]);
        assert_eq!(
            parse_lock(&failed).and_then(|l| l.error),
            Some(PairingError::DeviceTimeout)
        );
        assert_eq!(parse_lock(&Report::short(0x01, LOCK_NOTIFICATION, 0x01, &[])), None);
    }

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
        assert!(!uses_pairing_lock(0xC548));
        assert!(uses_pairing_lock(0xC52B));
    }
}
