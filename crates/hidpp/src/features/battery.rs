//! Battery level and charging state, from whichever battery feature the device has.

use crate::features::{BATTERY_STATUS, BATTERY_VOLTAGE, UNIFIED_BATTERY};
use crate::{Device, Link, Report, Result, Session};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// State of charge. Approximate when the device only reports a coarse level.
    pub percent: Option<u8>,
    pub state: ChargeState,
    /// Cell voltage, for devices that only report voltage.
    pub millivolts: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeState {
    Discharging,
    Charging,
    Full,
    Error,
    Unknown,
}

/// Coarse level flags in UNIFIED_BATTERY status.
const LEVEL_FULL: u8 = 0x08;
const LEVEL_GOOD: u8 = 0x04;
const LEVEL_LOW: u8 = 0x02;
const LEVEL_CRITICAL: u8 = 0x01;
/// BATTERY_VOLTAGE flag: external power connected.
const VOLTAGE_CHARGING: u8 = 0x80;

pub fn read<L: Link>(session: &mut Session<L>, device: &Device) -> Result<Option<Battery>> {
    if device.has(UNIFIED_BATTERY) {
        return Ok(Some(parse_unified(&device.call(session, UNIFIED_BATTERY, 1, &[])?)));
    }
    if device.has(BATTERY_STATUS) {
        return Ok(Some(parse_status(&device.call(session, BATTERY_STATUS, 0, &[])?)));
    }
    if device.has(BATTERY_VOLTAGE) {
        let reply = device.call(session, BATTERY_VOLTAGE, 0, &[])?;
        return Ok(Some(Battery {
            percent: None,
            state: if reply.param(2) & VOLTAGE_CHARGING != 0 {
                ChargeState::Charging
            } else {
                ChargeState::Discharging
            },
            millivolts: Some(reply.u16_at(0)),
        }));
    }
    Ok(None)
}

/// UNIFIED_BATTERY status: `[state of charge %, level flags, charging status, external power]`.
pub(crate) fn parse_unified(report: &Report) -> Battery {
    let percent = match report.param(0) {
        0 => approximate(report.param(1)),
        soc => Some(soc),
    };
    let state = match report.param(2) {
        0 => ChargeState::Discharging,
        1 | 2 => ChargeState::Charging,
        3 => ChargeState::Full,
        4 => ChargeState::Error,
        _ => ChargeState::Unknown,
    };
    Battery {
        percent,
        state,
        millivolts: None,
    }
}

/// BATTERY_STATUS: `[discharge level %, next level %, status]`.
pub(crate) fn parse_status(report: &Report) -> Battery {
    let state = match report.param(2) {
        0 => ChargeState::Discharging,
        1 | 2 | 4 => ChargeState::Charging,
        3 => ChargeState::Full,
        5..=7 => ChargeState::Error,
        _ => ChargeState::Unknown,
    };
    Battery {
        percent: Some(report.param(0)).filter(|&p| p != 0),
        state,
        millivolts: None,
    }
}

fn approximate(level: u8) -> Option<u8> {
    [
        (LEVEL_FULL, 100),
        (LEVEL_GOOD, 50),
        (LEVEL_LOW, 20),
        (LEVEL_CRITICAL, 5),
    ]
    .into_iter()
    .find(|&(flag, _)| level & flag != 0)
    .map(|(_, percent)| percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_prefers_state_of_charge() {
        let report = Report::long(0xFF, 5, 0x10, &[85, LEVEL_GOOD, 0, 0]);
        assert_eq!(
            parse_unified(&report),
            Battery {
                percent: Some(85),
                state: ChargeState::Discharging,
                millivolts: None
            }
        );
        let coarse = Report::long(0xFF, 5, 0x10, &[0, LEVEL_LOW, 1, 1]);
        assert_eq!(parse_unified(&coarse).percent, Some(20));
        assert_eq!(parse_unified(&coarse).state, ChargeState::Charging);
    }

    #[test]
    fn legacy_status_maps_states() {
        let full = Report::long(0xFF, 5, 0x00, &[100, 0, 3]);
        assert_eq!(parse_status(&full).state, ChargeState::Full);
        let unknown_level = Report::long(0xFF, 5, 0x00, &[0, 0, 0]);
        assert_eq!(parse_status(&unknown_level).percent, None);
    }
}
