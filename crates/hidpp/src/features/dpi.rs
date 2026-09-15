//! Pointer resolution (ADJUSTABLE_DPI, first sensor).

use std::fmt;

use crate::features::ADJUSTABLE_DPI;
use crate::{Device, Link, Result, Session};

const SENSOR: u8 = 0;
/// Entries in a DPI list with the top three bits set are step sizes, not values.
const STEP_MARKER: u16 = 0xE000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dpi {
    pub current: u16,
    pub default: u16,
    pub choices: DpiChoices,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpiChoices {
    List(Vec<u16>),
    Range { min: u16, max: u16, step: u16 },
}

impl DpiChoices {
    /// The supported value closest to `dpi`.
    pub fn nearest(&self, dpi: u16) -> Option<u16> {
        match *self {
            DpiChoices::List(ref values) => values.iter().copied().min_by_key(|v| v.abs_diff(dpi)),
            DpiChoices::Range { min, max, step } => {
                let step = u32::from(step.max(1));
                let offset = u32::from(dpi.clamp(min, max) - min);
                let snapped = u32::from(min) + (offset + step / 2) / step * step;
                Some(u16::try_from(snapped).map_or(max, |v| v.min(max)))
            }
        }
    }
}

impl fmt::Display for DpiChoices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DpiChoices::Range { min, max, step } => write!(f, "{min}–{max} in steps of {step}"),
            DpiChoices::List(values) => {
                let values: Vec<String> = values.iter().map(u16::to_string).collect();
                f.write_str(&values.join(", "))
            }
        }
    }
}

pub fn read<L: Link>(session: &mut Session<L>, device: &Device) -> Result<Dpi> {
    let list = device.call(session, ADJUSTABLE_DPI, 1, &[SENSOR])?;
    let current = device.call(session, ADJUSTABLE_DPI, 2, &[SENSOR])?;
    Ok(Dpi {
        current: current.u16_at(1),
        default: current.u16_at(3),
        choices: parse_choices(list.params().get(1..).unwrap_or_default()),
    })
}

pub fn set<L: Link>(session: &mut Session<L>, device: &Device, dpi: u16) -> Result<()> {
    let [hi, lo] = dpi.to_be_bytes();
    device.call(session, ADJUSTABLE_DPI, 3, &[SENSOR, hi, lo])?;
    Ok(())
}

/// Parses a zero-terminated list of big-endian values, where `[min, step marker, max]` is a range.
fn parse_choices(bytes: &[u8]) -> DpiChoices {
    let mut values = Vec::new();
    let mut step = None;
    for &pair in bytes.as_chunks::<2>().0 {
        let value = u16::from_be_bytes(pair);
        if value == 0 {
            break;
        }
        if value & STEP_MARKER == STEP_MARKER {
            step = Some(value & !STEP_MARKER);
        } else {
            values.push(value);
        }
    }
    match (step, values.as_slice()) {
        (Some(step), &[min, max]) => DpiChoices::Range { min, max, step },
        _ => DpiChoices::List(values),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        // 200..=8000 in steps of 50, as reported by MX Master mice.
        let choices = parse_choices(&[0x00, 0xC8, 0xE0, 0x32, 0x1F, 0x40, 0x00, 0x00]);
        assert_eq!(
            choices,
            DpiChoices::Range {
                min: 200,
                max: 8000,
                step: 50
            }
        );
        assert_eq!(choices.nearest(1234), Some(1250));
        assert_eq!(choices.nearest(10), Some(200));
        assert_eq!(choices.nearest(u16::MAX), Some(8000));
        assert_eq!(choices.to_string(), "200–8000 in steps of 50");
    }

    #[test]
    fn parses_lists() {
        let choices = parse_choices(&[0x01, 0x90, 0x03, 0x20, 0x06, 0x40, 0x00, 0x00, 0x12, 0x34]);
        assert_eq!(choices, DpiChoices::List(vec![400, 800, 1600]));
        assert_eq!(choices.nearest(1000), Some(800));
        assert_eq!(DpiChoices::List(vec![]).nearest(1000), None);
    }
}
