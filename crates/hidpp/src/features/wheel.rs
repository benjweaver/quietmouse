//! Scroll wheel resolution and direction (HIRES_WHEEL) and the thumb wheel (THUMB_WHEEL).

use crate::features::{HIRES_WHEEL, THUMB_WHEEL};
use crate::{Device, Link, Result, Session};

const MODE_DIVERTED: u8 = 0x01;
const MODE_HIRES: u8 = 0x02;
const MODE_INVERTED: u8 = 0x04;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollMode {
    /// High-resolution reports; smoother, but only useful where the OS understands them.
    pub hires: bool,
    /// Reversed direction, done on the device so it affects this mouse only.
    pub inverted: bool,
    /// Wheel reported over HID++ instead of as normal scrolling.
    pub diverted: bool,
}

impl ScrollMode {
    fn from_bits(bits: u8) -> Self {
        Self {
            hires: bits & MODE_HIRES != 0,
            inverted: bits & MODE_INVERTED != 0,
            diverted: bits & MODE_DIVERTED != 0,
        }
    }

    fn bits(self) -> u8 {
        [
            (self.hires, MODE_HIRES),
            (self.inverted, MODE_INVERTED),
            (self.diverted, MODE_DIVERTED),
        ]
        .into_iter()
        .filter(|&(on, _)| on)
        .fold(0, |bits, (_, bit)| bits | bit)
    }
}

pub fn scroll_mode<L: Link>(session: &mut Session<L>, device: &Device) -> Result<ScrollMode> {
    Ok(ScrollMode::from_bits(
        device.call(session, HIRES_WHEEL, 1, &[])?.param(0),
    ))
}

pub fn set_scroll_mode<L: Link>(session: &mut Session<L>, device: &Device, mode: ScrollMode) -> Result<()> {
    device.call(session, HIRES_WHEEL, 2, &[mode.bits()])?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbInfo {
    /// Increments per revolution when reported as normal horizontal scrolling.
    pub native_resolution: u16,
    /// Increments per revolution when diverted.
    pub diverted_resolution: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThumbReporting {
    pub diverted: bool,
    pub inverted: bool,
}

pub fn thumb_info<L: Link>(session: &mut Session<L>, device: &Device) -> Result<ThumbInfo> {
    let reply = device.call(session, THUMB_WHEEL, 0, &[])?;
    Ok(ThumbInfo {
        native_resolution: reply.u16_at(0),
        diverted_resolution: reply.u16_at(2),
    })
}

pub fn thumb_reporting<L: Link>(session: &mut Session<L>, device: &Device) -> Result<ThumbReporting> {
    let reply = device.call(session, THUMB_WHEEL, 1, &[])?;
    Ok(ThumbReporting {
        diverted: reply.param(0) & 0x01 != 0,
        inverted: reply.param(1) & 0x01 != 0,
    })
}

pub fn set_thumb_reporting<L: Link>(
    session: &mut Session<L>,
    device: &Device,
    reporting: ThumbReporting,
) -> Result<()> {
    device.call(
        session,
        THUMB_WHEEL,
        2,
        &[u8::from(reporting.diverted), u8::from(reporting.inverted)],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_mode_bits_round_trip() {
        for bits in 0..8 {
            assert_eq!(ScrollMode::from_bits(bits).bits(), bits);
        }
        let natural = ScrollMode {
            hires: false,
            inverted: true,
            diverted: false,
        };
        assert_eq!(natural.bits(), MODE_INVERTED);
    }
}
