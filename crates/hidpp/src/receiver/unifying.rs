//! Unifying-style pairing, which Lightspeed and Nano receivers share: the
//! receiver opens a pairing lock for a while, and a device switched on while
//! it's open pairs without further steps.

use std::time::{Duration, Instant};

use super::{Connection, Paired, Pairing, PairingError, REG_RECEIVER_INFO, parse_connection};
use crate::{DIRECT, Error, Link, Report, Result, Session};

/// Pairing lock: `[action, device index, seconds]`.
const REG_PAIRING: u8 = 0xB2;
const OPEN_LOCK: u8 = 0x01;
const CLOSE_LOCK: u8 = 0x02;
const UNPAIR: u8 = 0x03;
/// Sub-registers of [`REG_RECEIVER_INFO`] for slot 1; slot `n` adds `n - 1`.
const PAIRING_INFO: u8 = 0x20;
const DEVICE_NAME: u8 = 0x40;
/// Slots on a Unifying receiver, the most any of these receivers has.
const MAX_PAIRED: u8 = 6;
const LOCK_NOTIFICATION: u8 = 0x4A;

/// Every device the receiver has a pairing record for. Empty slots, and
/// receivers that don't keep records (some Lightspeed ones), give nothing.
pub(super) fn paired<L: Link>(session: &mut Session<L>) -> Result<Vec<Pairing>> {
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

/// The receiver opening or closing its pairing lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LockStatus {
    open: bool,
    error: Option<PairingError>,
}

/// Parses a pairing-lock notification: `[id, 0xFF, 0x4A, open flag, error, ...]`.
fn parse_lock(report: &Report) -> Option<LockStatus> {
    (report.device_index() == DIRECT && report.sub_id() == LOCK_NOTIFICATION).then(|| LockStatus {
        open: report.address() & 0x01 != 0,
        error: error(report.param(0)),
    })
}

/// Pairing errors as the lock notification reports them.
fn error(code: u8) -> Option<PairingError> {
    Some(match code {
        0x00 => return None,
        0x01 => PairingError::DeviceTimeout,
        0x02 => PairingError::Unsupported,
        0x03 => PairingError::TooManyDevices,
        0x06 => PairingError::SequenceTimeout,
        other => PairingError::Other(other),
    })
}

/// Opens the pairing lock for `seconds` and waits for the receiver to close it.
///
/// Turn the device on, or press its connect button, while the lock is open.
/// On an error, including [`Error::Stopped`], the lock may still be open;
/// [`close_lock`] shuts it.
pub(super) fn pair<L: Link>(session: &mut Session<L>, seconds: u8) -> Result<Paired> {
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
pub(super) fn close_lock<L: Link>(session: &mut Session<L>) -> Result<()> {
    session.write_register(DIRECT, REG_PAIRING, &[CLOSE_LOCK, 0x00, 0x00])?;
    Ok(())
}

/// Removes the pairing in slot `index`. The device stops working with this
/// receiver until it's paired again.
pub(super) fn unpair<L: Link>(session: &mut Session<L>, index: u8) -> Result<()> {
    session.write_register(DIRECT, REG_PAIRING, &[UNPAIR, index, 0x00])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receiver::CONNECTION_NOTIFICATION;
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
}
