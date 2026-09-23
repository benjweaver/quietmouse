//! Request/response handling on top of a raw [`Link`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::report::Report;
use crate::{Error, Result};

/// A bidirectional HID++ transport, such as a hidapi handle.
pub trait Link {
    /// Writes one report.
    fn send(&mut self, report: &Report) -> Result<()>;
    /// Waits up to `timeout` for the next inbound report; `Ok(None)` if none arrived.
    fn recv(&mut self, timeout: Duration) -> Result<Option<Report>>;
}

/// How long a request waits for its reply. Generous, because a device that has
/// just woken can take a moment to answer its first one.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// Notifications held back while waiting for replies; past this the oldest are dropped.
const BACKLOG_LIMIT: usize = 256;

/// HID++ 1.0 sub-ids for short register access.
const SET_REGISTER: u8 = 0x80;
const GET_REGISTER: u8 = 0x81;
/// HID++ 1.0 sub-id for reading a long register: a short request, a long reply.
const GET_LONG_REGISTER: u8 = 0x83;

/// One conversation with a HID++ endpoint (a receiver or a directly connected device).
///
/// Requests block until the matching reply arrives. Anything else that arrives
/// meanwhile is kept, in order, for [`Session::next_event`].
pub struct Session<L> {
    link: L,
    backlog: VecDeque<Report>,
    timeout: Duration,
    software_id: u8,
}

impl<L: Link> Session<L> {
    pub fn new(link: L) -> Self {
        Self {
            link,
            backlog: VecDeque::new(),
            timeout: DEFAULT_TIMEOUT,
            software_id: 0,
        }
    }

    pub fn link_mut(&mut self) -> &mut L {
        &mut self.link
    }

    /// Sends `request` and waits for its reply, or its error reply.
    pub fn request(&mut self, request: Report) -> Result<Report> {
        log::trace!("-> {request:?}");
        self.link.send(&request)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Some(report) = self.link.recv(remaining)? else {
                return Err(Error::Timeout);
            };
            log::trace!("<- {report:?}");
            match report.reply_to(&request) {
                Some(result) => return result.map(|()| report),
                None => self.defer(report),
            }
        }
    }

    /// Runs `requests` with a different reply timeout, restoring the usual one
    /// afterwards. For requests where a slow answer is better retried than
    /// waited out, such as asking whether a device is awake yet.
    pub fn with_timeout<T>(&mut self, timeout: Duration, requests: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let usual = std::mem::replace(&mut self.timeout, timeout);
        let result = requests(self);
        self.timeout = usual;
        result
    }

    /// Sends `report` without waiting for a reply.
    pub fn send(&mut self, report: Report) -> Result<()> {
        log::trace!("-> {report:?}");
        self.link.send(&report)
    }

    /// Next unsolicited report, waiting up to `timeout`.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Report>> {
        if let Some(report) = self.backlog.pop_front() {
            return Ok(Some(report));
        }
        let report = self.link.recv(timeout)?;
        if let Some(report) = &report {
            log::trace!("<- {report:?}");
        }
        Ok(report)
    }

    /// Calls HID++ 2.0 `function` on the feature at `feature_index`.
    pub fn call(&mut self, device: u8, feature_index: u8, function: u8, params: &[u8]) -> Result<Report> {
        let address = self.next_address(function);
        self.request(Report::long(device, feature_index, address, params))
    }

    /// Calls a HID++ 2.0 function that gets no reply, e.g. switching hosts.
    pub fn call_no_reply(&mut self, device: u8, feature_index: u8, function: u8, params: &[u8]) -> Result<()> {
        let address = self.next_address(function);
        self.send(Report::long(device, feature_index, address, params))
    }

    /// Reads a HID++ 1.0 short register.
    pub fn read_register(&mut self, device: u8, register: u8, params: &[u8]) -> Result<Report> {
        self.request(Report::short(device, GET_REGISTER, register, params))
    }

    /// Writes a HID++ 1.0 short register.
    pub fn write_register(&mut self, device: u8, register: u8, params: &[u8]) -> Result<Report> {
        self.request(Report::short(device, SET_REGISTER, register, params))
    }

    /// Reads a HID++ 1.0 long register; `params` usually picks the sub-register.
    pub fn read_long_register(&mut self, device: u8, register: u8, params: &[u8]) -> Result<Report> {
        self.request(Report::short(device, GET_LONG_REGISTER, register, params))
    }

    /// Rotates the software id through 1..=15 so a late reply to an abandoned
    /// request can't be mistaken for the reply to a newer one. Zero is reserved
    /// for device notifications.
    fn next_address(&mut self, function: u8) -> u8 {
        self.software_id = self.software_id % 0x0F + 1;
        (function << 4) | self.software_id
    }

    fn defer(&mut self, report: Report) {
        if self.backlog.len() == BACKLOG_LIMIT {
            self.backlog.pop_front();
            log::warn!("HID++ event backlog full; dropping the oldest event");
        }
        self.backlog.push_back(report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockLink;

    #[test]
    fn keeps_unrelated_reports_for_later() {
        let event = Report::long(0xFF, 0x08, 0x00, &[0x00, 0xC3]);
        let link = MockLink::new(move |request| {
            vec![
                event,
                Report::long(request.device_index(), request.sub_id(), request.address(), &[0x42]),
            ]
        });
        let mut session = Session::new(link);

        let reply = session.call(0xFF, 0x03, 0x1, &[]).unwrap();
        assert_eq!(reply.param(0), 0x42);
        assert_eq!(session.next_event(Duration::ZERO).unwrap(), Some(event));
        assert_eq!(session.next_event(Duration::ZERO).unwrap(), None);
    }

    #[test]
    fn surfaces_error_replies() {
        let link = MockLink::new(|request| {
            vec![Report::long(
                request.device_index(),
                0xFF,
                request.sub_id(),
                &[request.address(), 0x03],
            )]
        });
        let mut session = Session::new(link);
        assert_eq!(session.call(0xFF, 0x03, 0x1, &[]), Err(Error::Hidpp20(0x03)));
    }

    #[test]
    fn times_out_without_a_reply() {
        let mut session = Session::new(MockLink::new(|_| Vec::new()));
        assert_eq!(session.call(0x01, 0x03, 0x1, &[]), Err(Error::Timeout));
    }

    #[test]
    fn a_shorter_timeout_only_lasts_for_the_requests_it_wraps() {
        let mut session = Session::new(MockLink::new(|_| Vec::new()));
        let brief = Duration::from_millis(10);
        let inside = session.with_timeout(brief, |session| Ok(session.timeout));
        assert_eq!(inside, Ok(brief));
        assert_eq!(session.timeout, DEFAULT_TIMEOUT);
        // And it is put back even when the requests fail.
        let failed = session.with_timeout(brief, |session| session.call(0x01, 0x03, 0x1, &[]));
        assert_eq!(failed, Err(Error::Timeout));
        assert_eq!(session.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn software_ids_rotate_and_skip_zero() {
        let mut session = Session::new(MockLink::new(|_| Vec::new()));
        let ids: Vec<u8> = (0..16).map(|_| session.next_address(0x2) & 0x0F).collect();
        assert_eq!(ids[0], 1);
        assert_eq!(ids[14], 15);
        assert_eq!(ids[15], 1);
        assert!(ids.iter().all(|&id| id != 0));
    }
}
