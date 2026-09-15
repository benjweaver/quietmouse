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

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// Notifications held back while waiting for replies; past this the oldest are dropped.
const BACKLOG_LIMIT: usize = 256;

/// HID++ 1.0 sub-ids for short register access.
const SET_REGISTER: u8 = 0x80;
const GET_REGISTER: u8 = 0x81;

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
    fn software_ids_rotate_and_skip_zero() {
        let mut session = Session::new(MockLink::new(|_| Vec::new()));
        let ids: Vec<u8> = (0..16).map(|_| session.next_address(0x2) & 0x0F).collect();
        assert_eq!(ids[0], 1);
        assert_eq!(ids[14], 15);
        assert_eq!(ids[15], 1);
        assert!(ids.iter().all(|&id| id != 0));
    }
}
