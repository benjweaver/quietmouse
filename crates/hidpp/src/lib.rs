//! Logitech HID++ 1.0/2.0 protocol.
//!
//! Transport-agnostic: everything runs over a [`Link`], so the protocol logic
//! has no OS dependencies and is tested against scripted devices. Protocol
//! details follow the behaviour documented by Solaar and logiops, the
//! long-standing open-source Linux implementations.

mod device;
mod error;
pub mod features;
pub mod receiver;
mod report;
mod session;
#[cfg(test)]
mod testing;

pub use device::{Device, DeviceEvent};
pub use error::{Error, Result, hidpp10};
pub use report::{LONG_ID, Report, SHORT_ID, VERY_LONG_ID};
pub use session::{Link, Session};

/// Logitech's USB vendor id.
pub const LOGITECH_VID: u16 = 0x046D;

/// Device index of a receiver itself, and of a device connected directly over USB or Bluetooth.
pub const DIRECT: u8 = 0xFF;
