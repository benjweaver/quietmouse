//! Bolt receivers: pairing through discovery and a passkey, unpairing, and
//! their pairing records.
//!
//! Pairing runs in three steps. Discovery finds a device in pairing mode and
//! reports its address, kind and name. Pairing with that address makes the
//! receiver hand out a passkey, which the person enters on the device: typed
//! on a keyboard, or as left and right clicks on a mouse. The device reports
//! each key or click as it's entered, and the receiver then reports the slot
//! the device went into, or why it failed.

use std::time::{Duration, Instant};

use super::{Connection, MAX_SLOTS, Paired, Pairing, PairingError, PairingStep, REG_RECEIVER_INFO};
use crate::{DIRECT, Error, Link, Report, Result, Session};

/// Short register that starts and stops discovery: `[seconds, action]`.
const REG_DISCOVERY: u8 = 0xC0;
/// Long register for pairing and unpairing: `[action, slot, address x6, authentication, entropy]`.
const REG_PAIRING: u8 = 0xC1;
const START: u8 = 0x01;
const CANCEL: u8 = 0x02;
const UNPAIR: u8 = 0x03;

/// Sub-registers of [`REG_RECEIVER_INFO`]; slot `n` adds `n`.
const PAIRING_INFO: u8 = 0x50;
const DEVICE_NAME: u8 = 0x60;

const PASSKEY_REQUEST: u8 = 0x4D;
/// A key or click of the passkey; the address says what happened, using
/// Bluetooth's keypress notification types.
const PASSKEY_PRESSED: u8 = 0x4E;
const ENTRY_STARTED: u8 = 0x00;
const DIGIT_ENTERED: u8 = 0x01;
const DIGIT_ERASED: u8 = 0x02;
const ENTRY_CLEARED: u8 = 0x03;
const ENTRY_COMPLETED: u8 = 0x04;
const DEVICE_DISCOVERED: u8 = 0x4F;
const DISCOVERY_STATUS: u8 = 0x53;
const PAIRING_STATUS: u8 = 0x54;
/// [`PAIRING_STATUS`] address once a device has paired.
const PAIRED: u8 = 0x02;

const KEYBOARD: u8 = 0x01;
/// Authentication flag: the passkey is typed, rather than clicked.
const TYPED_PASSKEY: u8 = 0x01;

/// Every device the receiver has a pairing record for, awake or not.
pub(super) fn paired<L: Link>(session: &mut Session<L>) -> Result<Vec<Pairing>> {
    let mut paired = Vec::new();
    for index in 1..=MAX_SLOTS {
        // Params: [sub-register, kind, wpid lo, wpid hi, serial x4, ...].
        let info = match session.read_long_register(DIRECT, REG_RECEIVER_INFO, &[PAIRING_INFO + index]) {
            Ok(info) => info,
            Err(Error::Hidpp10(_)) => continue,
            Err(error) => return Err(error),
        };
        let wireless_pid = u16::from_le_bytes([info.param(2), info.param(3)]);
        if wireless_pid == 0 {
            continue;
        }
        // Params: [sub-register, part, length, name...]; part 1 holds the first 13 characters.
        let name = session
            .read_long_register(DIRECT, REG_RECEIVER_INFO, &[DEVICE_NAME + index, 0x01])
            .ok()
            .and_then(|reply| text(&reply, 2));
        paired.push(Pairing {
            index,
            wireless_pid,
            name,
        });
    }
    Ok(paired)
}

/// Removes the pairing in slot `index`.
pub(super) fn unpair<L: Link>(session: &mut Session<L>, index: u8) -> Result<()> {
    session.write_long_register(DIRECT, REG_PAIRING, &[UNPAIR, index])?;
    Ok(())
}

/// How a device wants its passkey entered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passkey {
    /// The device's name, as it gave it during discovery.
    pub device: String,
    /// Six decimal digits.
    pub digits: String,
    /// Typed on the device and followed by Enter; otherwise clicked, see [`Passkey::clicks`].
    pub typed: bool,
}

impl Passkey {
    /// For a mouse: the passkey as ten clicks, `true` for right and `false`
    /// for left, the bits of its number from the most significant. Pressing
    /// both buttons together afterwards finishes it.
    pub fn clicks(&self) -> Option<[bool; 10]> {
        let number: u16 = self.digits.parse().ok()?;
        (number < 1 << 10).then(|| std::array::from_fn(|bit| number & (1 << (9 - bit)) != 0))
    }
}

/// A device in pairing mode that discovery found.
#[derive(Debug, Clone, Default)]
struct Discovered {
    counter: u16,
    kind: u8,
    address: Option<[u8; 6]>,
    authentication: u8,
    name: Option<String>,
}

/// Discovers a device for `seconds`, pairs with the first one found, and waits
/// for the result. `step` hears the passkey to enter, and each key or click of it.
///
/// On an error, including [`Error::Stopped`], discovery or pairing may still be
/// running; [`cancel`] stops both.
pub(super) fn pair<L: Link>(
    session: &mut Session<L>,
    seconds: u8,
    mut step: impl FnMut(PairingStep),
) -> Result<Paired> {
    while session.next_event(Duration::ZERO)?.is_some() {}
    session.write_register(DIRECT, REG_DISCOVERY, &[seconds, START])?;

    let wait = Duration::from_secs(u64::from(seconds) + 5);
    let mut deadline = Instant::now() + wait;
    let mut found: Option<Discovered> = None;
    let mut pairing = false;
    let mut entered: u8 = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(report) = session.next_event(remaining)? else {
            return Err(Error::Timeout);
        };
        if report.device_index() != DIRECT {
            continue;
        }
        match report.sub_id() {
            DEVICE_DISCOVERED if !pairing => {
                let counter = u16::from(report.address()) | u16::from(report.param(0)) << 8;
                // Stay with the first device found; others in pairing mode are ignored.
                let device = found.get_or_insert_with(|| Discovered {
                    counter,
                    ..Discovered::default()
                });
                if device.counter != counter {
                    continue;
                }
                match report.param(1) {
                    0 => {
                        device.kind = report.param(3);
                        device.address = report.params().get(6..12).and_then(|a| a.try_into().ok());
                        device.authentication = report.param(14);
                    }
                    1 => device.name = text(&report, 2),
                    _ => {}
                }
                if let Some(address) = device.address
                    && device.name.is_some()
                {
                    let entropy = if device.kind & 0x0F == KEYBOARD { 20 } else { 10 };
                    let mut params = vec![START, 0x00];
                    params.extend_from_slice(&address);
                    params.extend_from_slice(&[device.authentication, entropy]);
                    session.write_long_register(DIRECT, REG_PAIRING, &params)?;
                    pairing = true;
                    // Entering the passkey takes the person a while.
                    deadline = Instant::now() + wait;
                }
            }
            // Discovery ended before a device turned up.
            DISCOVERY_STATUS if report.address() != 0x00 && !pairing => {
                return Ok(match error(report.param(0)) {
                    Some(error) => Paired::Failed(error),
                    None => Paired::Nothing,
                });
            }
            PASSKEY_REQUEST => {
                let device = found.as_ref();
                step(PairingStep::Passkey(Passkey {
                    device: device
                        .and_then(|d| d.name.clone())
                        .unwrap_or_else(|| "the device".to_owned()),
                    digits: String::from_utf8_lossy(report.params().get(..6).unwrap_or_default()).into_owned(),
                    typed: device.is_some_and(|d| d.authentication & TYPED_PASSKEY != 0),
                }));
            }
            PASSKEY_PRESSED if pairing => {
                // Still at it: don't give up on someone entering the passkey slowly.
                deadline = Instant::now() + wait;
                entered = match report.address() {
                    ENTRY_STARTED | ENTRY_CLEARED => 0,
                    DIGIT_ENTERED => entered.saturating_add(1),
                    DIGIT_ERASED => entered.saturating_sub(1),
                    ENTRY_COMPLETED => {
                        step(PairingStep::Submitted);
                        continue;
                    }
                    _ => continue,
                };
                step(PairingStep::Entered(entered));
            }
            // Address 0x00 is the lock opening; anything else is it closing.
            PAIRING_STATUS if report.address() != 0x00 => {
                if let Some(error) = error(report.param(0)) {
                    return Ok(Paired::Failed(error));
                }
                if report.address() != PAIRED {
                    return Ok(Paired::Nothing);
                }
                let index = report.param(7);
                let wireless_pid = paired(session)
                    .ok()
                    .and_then(|paired| paired.into_iter().find(|p| p.index == index))
                    .map_or(0, |p| p.wireless_pid);
                return Ok(Paired::Device(Connection {
                    index,
                    online: true,
                    wireless_pid,
                }));
            }
            _ => {}
        }
    }
}

/// Stops discovery and any pairing under way. Errors are ignored: the
/// receiver refuses to cancel what isn't running.
pub(super) fn cancel<L: Link>(session: &mut Session<L>) {
    let _ = session.write_long_register(DIRECT, REG_PAIRING, &[CANCEL]);
    let _ = session.write_register(DIRECT, REG_DISCOVERY, &[0x00, CANCEL]);
}

fn error(code: u8) -> Option<PairingError> {
    match code {
        0x00 => None,
        0x01 => Some(PairingError::DeviceTimeout),
        0x02 => Some(PairingError::Failed),
        other => Some(PairingError::Other(other)),
    }
}

/// Text whose length is at parameter byte `at`, with the characters after it.
fn text(report: &Report, at: usize) -> Option<String> {
    let bytes = report.params().get(at + 1..)?;
    let bytes = &bytes[..usize::from(report.param(at)).min(bytes.len())];
    let text = String::from_utf8_lossy(bytes).trim_end_matches('\0').trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockLink;

    const ADDRESS: [u8; 6] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    const MOUSE: u16 = 0xB034;

    fn discovered(counter: u8, part: u8, rest: &[u8]) -> Report {
        let mut params = vec![0x00, part];
        params.extend_from_slice(rest);
        Report::long(DIRECT, DEVICE_DISCOVERED, counter, &params)
    }

    fn mouse_found(counter: u8) -> Vec<Report> {
        // Params from 2: [-, kind (mouse), -, -, address x6, -, -, authentication].
        let mut part0 = vec![0x00, 0x02, 0x00, 0x00];
        part0.extend_from_slice(&ADDRESS);
        part0.extend_from_slice(&[0x00, 0x00, 0x00]);
        let name = b"MX Master 3S";
        let mut part1 = vec![u8::try_from(name.len()).unwrap()];
        part1.extend_from_slice(name);
        vec![discovered(counter, 0, &part0), discovered(counter, 1, &part1)]
    }

    /// A Bolt receiver that finds `found` once discovery starts and, once asked
    /// to pair, asks for passkey 000613 and ends with `status`.
    fn fake_receiver(found: Vec<Report>, status: Report) -> MockLink {
        MockLink::new(
            move |request| match (request.sub_id(), request.address(), request.param(0)) {
                (0x80, REG_DISCOVERY, _) => {
                    let mut replies = vec![
                        Report::short(DIRECT, 0x80, REG_DISCOVERY, &[]),
                        Report::short(DIRECT, DISCOVERY_STATUS, 0x00, &[0x00]),
                    ];
                    replies.extend(found.iter().copied());
                    replies
                }
                (0x82, REG_PAIRING, START) => vec![
                    Report::short(DIRECT, 0x82, REG_PAIRING, &[]),
                    Report::long(DIRECT, PAIRING_STATUS, 0x00, &[0x00]),
                    Report::long(DIRECT, PASSKEY_REQUEST, 0x00, b"000613"),
                    Report::long(DIRECT, PASSKEY_PRESSED, ENTRY_STARTED, &[]),
                    Report::long(DIRECT, PASSKEY_PRESSED, DIGIT_ENTERED, &[]),
                    Report::long(DIRECT, PASSKEY_PRESSED, DIGIT_ENTERED, &[]),
                    Report::long(DIRECT, PASSKEY_PRESSED, DIGIT_ERASED, &[]),
                    Report::long(DIRECT, PASSKEY_PRESSED, ENTRY_COMPLETED, &[]),
                    status,
                ],
                (0x83, REG_RECEIVER_INFO, sub) if sub == PAIRING_INFO + 2 => {
                    let [lo, hi] = MOUSE.to_le_bytes();
                    vec![Report::long(DIRECT, 0x83, REG_RECEIVER_INFO, &[sub, 0x02, lo, hi])]
                }
                (0x83, REG_RECEIVER_INFO, sub) if sub == DEVICE_NAME + 2 => vec![Report::long(
                    DIRECT,
                    0x83,
                    REG_RECEIVER_INFO,
                    &[sub, 0x01, 0x04, b'M', b'X', b' ', b'3'],
                )],
                (0x83, REG_RECEIVER_INFO, sub) => {
                    vec![Report::short(DIRECT, 0x8F, 0x83, &[REG_RECEIVER_INFO, 0x0B, sub])]
                }
                _ => Vec::new(),
            },
        )
    }

    fn paired_in(slot: u8) -> Report {
        Report::long(DIRECT, PAIRING_STATUS, PAIRED, &[0x00, 0, 0, 0, 0, 0, 0, slot])
    }

    #[test]
    fn pairs_the_discovered_device_and_asks_for_its_passkey() {
        let mut session = Session::new(fake_receiver(mouse_found(7), paired_in(2)));
        let mut steps = Vec::new();
        let outcome = pair(&mut session, 30, |step| steps.push(step));
        assert_eq!(
            outcome,
            Ok(Paired::Device(Connection {
                index: 2,
                online: true,
                wireless_pid: MOUSE
            }))
        );
        assert_eq!(
            steps,
            vec![
                PairingStep::Passkey(Passkey {
                    device: "MX Master 3S".to_owned(),
                    digits: "000613".to_owned(),
                    typed: false,
                }),
                PairingStep::Entered(0),
                PairingStep::Entered(1),
                PairingStep::Entered(2),
                PairingStep::Entered(1),
                PairingStep::Submitted,
            ]
        );
        let sent = &session.link_mut().sent;
        assert_eq!(&sent[0].params()[..2], &[30, START]);
        let pair_request = sent.iter().find(|r| r.sub_id() == 0x82).unwrap();
        assert_eq!(
            &pair_request.params()[..10],
            &[START, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x00, 10]
        );
    }

    #[test]
    fn ignores_a_second_device_in_pairing_mode() {
        let mut found = mouse_found(7);
        found.insert(1, discovered(8, 1, &[3, b'K', b'1', b'0']));
        let mut session = Session::new(fake_receiver(found, paired_in(2)));
        let mut device = String::new();
        pair(&mut session, 30, |step| {
            if let PairingStep::Passkey(passkey) = step {
                device = passkey.device;
            }
        })
        .unwrap();
        assert_eq!(device, "MX Master 3S");
    }

    #[test]
    fn reports_failures() {
        let wrong_passkey = Report::long(DIRECT, PAIRING_STATUS, 0x01, &[0x02]);
        let mut session = Session::new(fake_receiver(mouse_found(7), wrong_passkey));
        assert_eq!(pair(&mut session, 30, |_| {}), Ok(Paired::Failed(PairingError::Failed)));

        let nothing_found = vec![Report::short(DIRECT, DISCOVERY_STATUS, 0x01, &[0x01])];
        let mut session = Session::new(fake_receiver(nothing_found, paired_in(2)));
        assert_eq!(
            pair(&mut session, 30, |_| {}),
            Ok(Paired::Failed(PairingError::DeviceTimeout))
        );
    }

    #[test]
    fn reads_pairing_records() {
        let mut session = Session::new(fake_receiver(Vec::new(), paired_in(2)));
        assert_eq!(
            paired(&mut session).unwrap(),
            vec![Pairing {
                index: 2,
                wireless_pid: MOUSE,
                name: Some("MX 3".to_owned())
            }]
        );
    }

    #[test]
    fn unpairs_through_the_pairing_register() {
        let mut session = Session::new(MockLink::new(|request| {
            vec![Report::short(DIRECT, request.sub_id(), request.address(), &[])]
        }));
        unpair(&mut session, 3).unwrap();
        let sent = session.link_mut().sent[0];
        assert_eq!((sent.sub_id(), sent.address()), (0x82, REG_PAIRING));
        assert_eq!(&sent.params()[..2], &[UNPAIR, 3]);
    }

    #[test]
    fn passkeys_become_clicks() {
        let passkey = Passkey {
            device: String::new(),
            digits: "000613".to_owned(),
            typed: false,
        };
        // 613 = 0b10_0110_0101
        let clicks = passkey.clicks().unwrap();
        let bits: String = clicks.iter().map(|&right| if right { '1' } else { '0' }).collect();
        assert_eq!(bits, "1001100101");
        let too_big = Passkey {
            digits: "999999".to_owned(),
            ..passkey
        };
        assert_eq!(too_big.clicks(), None);
    }
}
