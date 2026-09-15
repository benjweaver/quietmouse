//! Raw HID++ reports and the framing rules shared by HID++ 1.0 and 2.0.
//!
//! Every message is `[report id, device index, sub-id, address, params...]`.
//! In HID++ 2.0 the sub-id is a feature index and the address packs the
//! function number (high nibble) with a software id (low nibble).

use std::fmt;

use crate::{Error, Result};

/// Short report: 7 bytes, up to 3 parameter bytes.
pub const SHORT_ID: u8 = 0x10;
/// Long report: 20 bytes, up to 16 parameter bytes.
pub const LONG_ID: u8 = 0x11;
/// Very long report: 64 bytes, used by a few newer devices.
pub const VERY_LONG_ID: u8 = 0x12;

const SHORT_LEN: usize = 7;
const LONG_LEN: usize = 20;
const MAX_LEN: usize = 64;
const HEADER_LEN: usize = 4;

/// Sub-id that marks a HID++ 1.0 error reply.
const HIDPP10_ERROR: u8 = 0x8F;
/// Feature index that marks a HID++ 2.0 error reply.
const HIDPP20_ERROR: u8 = 0xFF;

/// One HID++ message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Report {
    buf: [u8; MAX_LEN],
    len: usize,
}

impl Report {
    /// Parses a report as read from a HID interface (report id first).
    /// Trailing bytes beyond the report's length, e.g. Windows padding, are ignored.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let len = report_len(*data.first()?)?;
        let bytes = data.get(..len)?;
        let mut buf = [0; MAX_LEN];
        buf[..len].copy_from_slice(bytes);
        Some(Self { buf, len })
    }

    /// Builds a short report. Panics if `params` exceeds 3 bytes.
    pub fn short(device: u8, sub_id: u8, address: u8, params: &[u8]) -> Self {
        Self::build(SHORT_ID, device, sub_id, address, params)
    }

    /// Builds a long report. Panics if `params` exceeds 16 bytes.
    pub fn long(device: u8, sub_id: u8, address: u8, params: &[u8]) -> Self {
        Self::build(LONG_ID, device, sub_id, address, params)
    }

    fn build(report_id: u8, device: u8, sub_id: u8, address: u8, params: &[u8]) -> Self {
        let len = report_len(report_id).expect("report id has a known length");
        assert!(
            params.len() <= len - HEADER_LEN,
            "{} parameter bytes do not fit report {report_id:#04x}",
            params.len()
        );
        let mut buf = [0; MAX_LEN];
        buf[..HEADER_LEN].copy_from_slice(&[report_id, device, sub_id, address]);
        buf[HEADER_LEN..HEADER_LEN + params.len()].copy_from_slice(params);
        Self { buf, len }
    }

    /// The same message framed as a long report, for interfaces that only take long reports
    /// (Bluetooth devices, and the long-report collection on Windows).
    pub fn to_long(&self) -> Self {
        if self.report_id() == SHORT_ID {
            Self::long(self.device_index(), self.sub_id(), self.address(), self.params())
        } else {
            *self
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn report_id(&self) -> u8 {
        self.buf[0]
    }

    pub fn device_index(&self) -> u8 {
        self.buf[1]
    }

    /// HID++ 1.0 sub-id, or HID++ 2.0 feature index.
    pub fn sub_id(&self) -> u8 {
        self.buf[2]
    }

    /// HID++ 1.0 register address, or HID++ 2.0 function and software id.
    pub fn address(&self) -> u8 {
        self.buf[3]
    }

    /// HID++ 2.0 function number (for notifications: the event number).
    pub fn function(&self) -> u8 {
        self.address() >> 4
    }

    /// HID++ 2.0 software id: zero for unsolicited notifications.
    pub fn software_id(&self) -> u8 {
        self.address() & 0x0F
    }

    pub fn params(&self) -> &[u8] {
        &self.buf[HEADER_LEN..self.len]
    }

    /// Parameter byte `i`, or zero past the end of the report.
    pub fn param(&self, i: usize) -> u8 {
        self.params().get(i).copied().unwrap_or(0)
    }

    /// Big-endian `u16` starting at parameter byte `i`.
    pub fn u16_at(&self, i: usize) -> u16 {
        u16::from_be_bytes([self.param(i), self.param(i + 1)])
    }

    /// Big-endian `i16` starting at parameter byte `i`.
    pub fn i16_at(&self, i: usize) -> i16 {
        i16::from_be_bytes([self.param(i), self.param(i + 1)])
    }

    /// Classifies `self` against an outstanding `request`: `None` if unrelated,
    /// otherwise whether it is a successful reply or an error reply.
    pub(crate) fn reply_to(&self, request: &Report) -> Option<Result<()>> {
        if self.device_index() != request.device_index() {
            return None;
        }
        if self.sub_id() == request.sub_id() && self.address() == request.address() {
            return Some(Ok(()));
        }
        // Error replies: [id, device, 0x8F/0xFF, request sub-id, request address, code].
        let error = match self.sub_id() {
            HIDPP10_ERROR => Error::Hidpp10(self.param(1)),
            HIDPP20_ERROR => Error::Hidpp20(self.param(1)),
            _ => return None,
        };
        (self.address() == request.sub_id() && self.param(0) == request.address()).then_some(Err(error))
    }
}

fn report_len(report_id: u8) -> Option<usize> {
    match report_id {
        SHORT_ID => Some(SHORT_LEN),
        LONG_ID => Some(LONG_LEN),
        VERY_LONG_ID => Some(MAX_LEN),
        _ => None,
    }
}

impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.as_bytes().iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_report_ids_and_ignores_padding() {
        let mut padded = [0u8; 64];
        padded[..7].copy_from_slice(&[0x10, 0xFF, 0x81, 0x02, 1, 2, 3]);
        let report = Report::from_bytes(&padded).unwrap();
        assert_eq!(report.as_bytes().len(), 7);
        assert_eq!(report.params(), &[1, 2, 3]);

        assert!(Report::from_bytes(&[0x20, 1, 2, 3, 4, 5, 6]).is_none());
        assert!(Report::from_bytes(&[0x11, 0xFF, 0x00]).is_none());
        assert!(Report::from_bytes(&[]).is_none());
    }

    #[test]
    fn splits_function_and_software_id() {
        let report = Report::long(0xFF, 0x09, 0x3A, &[0x00, 0xC3]);
        assert_eq!(report.function(), 3);
        assert_eq!(report.software_id(), 0xA);
        assert_eq!(report.u16_at(0), 0x00C3);
        assert_eq!(report.param(15), 0);
        assert_eq!(report.param(99), 0);
    }

    #[test]
    fn short_reports_reframe_as_long() {
        let long = Report::short(0xFF, 0x81, 0x02, &[7]).to_long();
        assert_eq!(long.report_id(), LONG_ID);
        assert_eq!(&long.as_bytes()[..5], &[0x11, 0xFF, 0x81, 0x02, 7]);
        assert_eq!(long.as_bytes().len(), 20);
    }

    #[test]
    fn matches_replies_and_errors() {
        let request = Report::long(0xFF, 0x05, 0x1A, &[]);
        let reply = Report::long(0xFF, 0x05, 0x1A, &[0x0F]);
        assert_eq!(reply.reply_to(&request), Some(Ok(())));

        let error20 = Report::long(0xFF, 0xFF, 0x05, &[0x1A, 0x02]);
        assert_eq!(error20.reply_to(&request), Some(Err(Error::Hidpp20(0x02))));

        let error10 = Report::short(0xFF, 0x8F, 0x05, &[0x1A, 0x09]);
        assert_eq!(error10.reply_to(&request), Some(Err(Error::Hidpp10(0x09))));

        let other_device = Report::long(0x01, 0x05, 0x1A, &[]);
        let other_request = Report::long(0xFF, 0xFF, 0x05, &[0x2A, 0x02]);
        let notification = Report::long(0xFF, 0x05, 0x10, &[]);
        assert_eq!(other_device.reply_to(&request), None);
        assert_eq!(other_request.reply_to(&request), None);
        assert_eq!(notification.reply_to(&request), None);
    }
}
