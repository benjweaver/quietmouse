//! Errors produced while talking HID++.

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The device didn't answer in time: asleep, out of range, or busy.
    #[error("no reply from device")]
    Timeout,
    /// The HID interface went away.
    #[error("device disconnected")]
    Disconnected,
    /// The link was asked to shut down.
    #[error("shutting down")]
    Stopped,
    /// A HID++ 1.0 error reply (receivers and register access).
    #[error("HID++ 1.0 error {:#04x} ({})", .0, hidpp10_reason(*.0))]
    Hidpp10(u8),
    /// A HID++ 2.0 error reply (feature calls).
    #[error("HID++ 2.0 error {:#04x} ({})", .0, hidpp20_reason(*.0))]
    Hidpp20(u8),
    /// The device doesn't implement the feature a request needs.
    #[error("device does not support feature {0:#06x}")]
    Unsupported(u16),
    /// The transport failed to read or write.
    #[error("I/O error: {0}")]
    Io(String),
}

impl Error {
    /// Whether the link itself is unusable, as opposed to one request failing.
    pub fn is_fatal(&self) -> bool {
        matches!(self, Error::Disconnected | Error::Stopped)
    }
}

/// HID++ 1.0 error code meanings.
pub mod hidpp10 {
    pub const INVALID_SUB_ID: u8 = 0x01;
    pub const UNKNOWN_DEVICE: u8 = 0x08;
    pub const RESOURCE_ERROR: u8 = 0x09;
}

fn hidpp10_reason(code: u8) -> &'static str {
    match code {
        0x01 => "invalid sub-ID",
        0x02 => "invalid address",
        0x03 => "invalid value",
        0x04 => "connection request failed",
        0x05 => "too many devices",
        0x06 => "already exists",
        0x07 => "busy",
        0x08 => "unknown device",
        0x09 => "resource error",
        0x0A => "request unavailable",
        0x0B => "invalid parameter value",
        0x0C => "wrong PIN code",
        _ => "unrecognised",
    }
}

fn hidpp20_reason(code: u8) -> &'static str {
    match code {
        0x01 => "unknown",
        0x02 => "invalid argument",
        0x03 => "out of range",
        0x04 => "hardware error",
        0x05 => "internal error",
        0x06 => "invalid feature index",
        0x07 => "invalid function",
        0x08 => "busy",
        0x09 => "unsupported",
        _ => "unrecognised",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_code() {
        assert_eq!(
            Error::Hidpp20(0x02).to_string(),
            "HID++ 2.0 error 0x02 (invalid argument)"
        );
        assert_eq!(
            Error::Hidpp10(0x09).to_string(),
            "HID++ 1.0 error 0x09 (resource error)"
        );
    }
}
