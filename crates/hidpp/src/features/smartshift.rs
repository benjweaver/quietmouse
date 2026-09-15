//! SmartShift: the scroll wheel's ratchet/free-spin clutch. SMART_SHIFT_ENHANCED
//! adds adjustable ratchet torque and moves every function up by one.

use crate::features::{SMART_SHIFT, SMART_SHIFT_ENHANCED};
use crate::{Device, Error, Link, Result, Session};

/// Auto-disengage value meaning "stay ratcheted; never switch to free-spin on its own".
pub const NEVER_DISENGAGE: u8 = 0xFF;

const CAN_TUNE_TORQUE: u8 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelMode {
    Freespin,
    Ratchet,
}

impl WheelMode {
    fn code(self) -> u8 {
        match self {
            WheelMode::Freespin => 1,
            WheelMode::Ratchet => 2,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(WheelMode::Freespin),
            2 => Some(WheelMode::Ratchet),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmartShift {
    pub mode: Option<WheelMode>,
    /// Wheel speed that releases the ratchet; [`NEVER_DISENGAGE`] keeps it engaged.
    pub auto_disengage: u8,
    pub default_auto_disengage: u8,
    /// Ratchet force in percent, on devices with tunable torque.
    pub torque: Option<u8>,
}

pub fn read<L: Link>(session: &mut Session<L>, device: &Device) -> Result<Option<SmartShift>> {
    if device.has(SMART_SHIFT_ENHANCED) {
        let caps = device.call(session, SMART_SHIFT_ENHANCED, 0, &[])?;
        let status = device.call(session, SMART_SHIFT_ENHANCED, 1, &[])?;
        return Ok(Some(SmartShift {
            mode: WheelMode::from_code(status.param(0)),
            auto_disengage: status.param(1),
            default_auto_disengage: caps.param(1),
            torque: (caps.param(0) & CAN_TUNE_TORQUE != 0).then(|| status.param(2)),
        }));
    }
    if device.has(SMART_SHIFT) {
        let status = device.call(session, SMART_SHIFT, 0, &[])?;
        return Ok(Some(SmartShift {
            mode: WheelMode::from_code(status.param(0)),
            auto_disengage: status.param(1),
            default_auto_disengage: status.param(2),
            torque: None,
        }));
    }
    Ok(None)
}

/// Changes the clutch. `None` (sent as zero) leaves that setting as it is.
pub fn write<L: Link>(
    session: &mut Session<L>,
    device: &Device,
    mode: Option<WheelMode>,
    auto_disengage: Option<u8>,
    torque: Option<u8>,
) -> Result<()> {
    let mode = mode.map_or(0, WheelMode::code);
    let auto_disengage = auto_disengage.unwrap_or(0);
    if device.has(SMART_SHIFT_ENHANCED) {
        device.call(
            session,
            SMART_SHIFT_ENHANCED,
            2,
            &[mode, auto_disengage, torque.unwrap_or(0)],
        )?;
    } else if device.has(SMART_SHIFT) {
        device.call(session, SMART_SHIFT, 1, &[mode, auto_disengage, 0])?;
    } else {
        return Err(Error::Unsupported(SMART_SHIFT));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::testing::fake_device;

    #[test]
    fn enhanced_reads_and_writes_shifted_functions() {
        let writes = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&writes);
        let link = fake_device(
            0xFF,
            &[SMART_SHIFT_ENHANCED],
            move |_, function, params| match function {
                0 => Some(vec![CAN_TUNE_TORQUE, 10, 50, 0]),
                1 => Some(vec![2, 12, 75]),
                2 => {
                    log.borrow_mut().push(params[..3].to_vec());
                    Some(params[..3].to_vec())
                }
                _ => None,
            },
        );
        let mut session = Session::new(link);
        let device = Device::open(&mut session, 0xFF).unwrap();

        let state = read(&mut session, &device).unwrap().unwrap();
        assert_eq!(
            state,
            SmartShift {
                mode: Some(WheelMode::Ratchet),
                auto_disengage: 12,
                default_auto_disengage: 10,
                torque: Some(75)
            }
        );

        write(&mut session, &device, Some(WheelMode::Freespin), None, Some(40)).unwrap();
        assert_eq!(*writes.borrow(), vec![vec![1, 0, 40]]);
    }
}
