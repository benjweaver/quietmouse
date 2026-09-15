//! Reprogrammable controls (REPROG_CONTROLS_V4): listing buttons and diverting
//! them, so presses arrive as HID++ notifications instead of normal input.
//!
//! Diversion is volatile: the device forgets it when it sleeps or reconnects.

use crate::features::REPROG_CONTROLS_V4;
use crate::{Device, Link, Report, Result, Session};

/// Control ids (CIDs) of common mouse buttons.
pub mod cid {
    pub const LEFT: u16 = 0x0050;
    pub const RIGHT: u16 = 0x0051;
    pub const MIDDLE: u16 = 0x0052;
    pub const BACK: u16 = 0x0053;
    pub const FORWARD: u16 = 0x0056;
    pub const GESTURE: u16 = 0x00C3;
    pub const MODE_SHIFT: u16 = 0x00C4;
}

// Capability flags from getCidInfo, with the second flags byte in the high byte.
const MOUSE_BUTTON: u16 = 0x0001;
const REPROGRAMMABLE: u16 = 0x0010;
const DIVERTABLE: u16 = 0x0020;
const VIRTUAL: u16 = 0x0080;
const RAW_XY: u16 = 0x0100;

// Reporting flags for get/setCidReporting. When setting, each flag has a
// "valid" bit just above it that tells the device to apply it.
const REPORT_DIVERTED: u8 = 0x01;
const REPORT_DIVERTED_VALID: u8 = 0x02;
const REPORT_RAW_XY: u8 = 0x10;
const REPORT_RAW_XY_VALID: u8 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Control {
    pub cid: u16,
    /// Task id: what the control does natively.
    pub task: u16,
    pub flags: u16,
}

impl Control {
    pub fn is_mouse_button(&self) -> bool {
        self.flags & MOUSE_BUTTON != 0
    }

    pub fn reprogrammable(&self) -> bool {
        self.flags & REPROGRAMMABLE != 0
    }

    pub fn divertable(&self) -> bool {
        self.flags & DIVERTABLE != 0
    }

    pub fn is_virtual(&self) -> bool {
        self.flags & VIRTUAL != 0
    }

    /// Can report pointer movement while held, which is what gestures need.
    pub fn supports_raw_xy(&self) -> bool {
        self.flags & RAW_XY != 0
    }

    pub fn name(&self) -> Option<&'static str> {
        name(self.cid)
    }
}

pub fn name(cid: u16) -> Option<&'static str> {
    match cid {
        cid::LEFT => Some("Left click"),
        cid::RIGHT => Some("Right click"),
        cid::MIDDLE => Some("Middle click"),
        cid::BACK => Some("Back"),
        cid::FORWARD => Some("Forward"),
        cid::GESTURE => Some("Gesture button"),
        cid::MODE_SHIFT => Some("Mode shift (SmartShift)"),
        _ => None,
    }
}

/// How a control currently reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reporting {
    pub diverted: bool,
    pub raw_xy: bool,
}

pub fn controls<L: Link>(session: &mut Session<L>, device: &Device) -> Result<Vec<Control>> {
    let count = device.call(session, REPROG_CONTROLS_V4, 0, &[])?.param(0);
    (0..count)
        .map(|i| Ok(parse_info(&device.call(session, REPROG_CONTROLS_V4, 1, &[i])?)))
        .collect()
}

/// getCidInfo: `[cid, task id, flags, position, group, group mask, more flags]`.
fn parse_info(report: &Report) -> Control {
    Control {
        cid: report.u16_at(0),
        task: report.u16_at(2),
        flags: u16::from(report.param(4)) | (u16::from(report.param(8)) << 8),
    }
}

pub fn reporting<L: Link>(session: &mut Session<L>, device: &Device, cid: u16) -> Result<Reporting> {
    let flags = device
        .call(session, REPROG_CONTROLS_V4, 2, &cid.to_be_bytes())?
        .param(2);
    Ok(Reporting {
        diverted: flags & REPORT_DIVERTED != 0,
        raw_xy: flags & REPORT_RAW_XY != 0,
    })
}

pub fn set_reporting<L: Link>(
    session: &mut Session<L>,
    device: &Device,
    control: &Control,
    reporting: Reporting,
) -> Result<()> {
    device.call(session, REPROG_CONTROLS_V4, 3, &reporting_params(control, reporting))?;
    Ok(())
}

/// setCidReporting: `[cid, flags, remap cid]`, where a zero remap leaves the mapping alone.
fn reporting_params(control: &Control, reporting: Reporting) -> [u8; 5] {
    let mut flags = REPORT_DIVERTED_VALID;
    if reporting.diverted {
        flags |= REPORT_DIVERTED;
    }
    // Only touch raw XY on controls that have it; others reject the valid bit.
    if control.supports_raw_xy() {
        flags |= REPORT_RAW_XY_VALID;
        if reporting.raw_xy {
            flags |= REPORT_RAW_XY;
        }
    }
    let [hi, lo] = control.cid.to_be_bytes();
    [hi, lo, flags, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gesture() -> Control {
        Control {
            cid: cid::GESTURE,
            task: 0x00AD,
            flags: REPROGRAMMABLE | DIVERTABLE | RAW_XY,
        }
    }

    #[test]
    fn parses_cid_info_flags_from_both_bytes() {
        let info = Report::long(0xFF, 9, 0x10, &[0x00, 0xC3, 0x00, 0xAD, 0x30, 0, 0, 0, 0x03]);
        let control = parse_info(&info);
        assert_eq!(control.cid, cid::GESTURE);
        assert!(control.divertable());
        assert!(control.supports_raw_xy());
        assert!(!control.is_mouse_button());
    }

    #[test]
    fn diversion_flags_carry_valid_bits() {
        let divert = Reporting {
            diverted: true,
            raw_xy: true,
        };
        assert_eq!(reporting_params(&gesture(), divert), [0x00, 0xC3, 0x33, 0, 0]);
        assert_eq!(
            reporting_params(&gesture(), Reporting::default()),
            [0x00, 0xC3, 0x22, 0, 0]
        );

        let back = Control {
            cid: cid::BACK,
            task: 0x0038,
            flags: REPROGRAMMABLE | DIVERTABLE,
        };
        assert_eq!(reporting_params(&back, divert), [0x00, 0x53, 0x03, 0, 0]);
        assert_eq!(reporting_params(&back, Reporting::default()), [0x00, 0x53, 0x02, 0, 0]);
    }
}
