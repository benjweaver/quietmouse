//! HID++ 2.0 features: ids, names, and typed access to the ones this crate drives.

pub mod battery;
pub mod dpi;
pub mod host;
pub mod reprog;
pub mod smartshift;
pub mod wheel;

pub const ROOT: u16 = 0x0000;
pub const FEATURE_SET: u16 = 0x0001;
pub const DEVICE_NAME: u16 = 0x0005;
pub const BATTERY_STATUS: u16 = 0x1000;
pub const BATTERY_VOLTAGE: u16 = 0x1001;
pub const UNIFIED_BATTERY: u16 = 0x1004;
pub const CHANGE_HOST: u16 = 0x1814;
pub const REPROG_CONTROLS_V4: u16 = 0x1B04;
pub const WIRELESS_DEVICE_STATUS: u16 = 0x1D4B;
pub const SMART_SHIFT: u16 = 0x2110;
pub const SMART_SHIFT_ENHANCED: u16 = 0x2111;
pub const HIRES_WHEEL: u16 = 0x2121;
pub const THUMB_WHEEL: u16 = 0x2150;
pub const ADJUSTABLE_DPI: u16 = 0x2201;

/// Human-readable name of a feature id, for diagnostics.
pub fn name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0000 => "Root",
        0x0001 => "Feature set",
        0x0002 => "Feature info",
        0x0003 => "Firmware version",
        0x0004 => "Device unit ID",
        0x0005 => "Device name",
        0x0006 => "Device groups",
        0x0007 => "Device friendly name",
        0x0008 => "Keep alive",
        0x0020 => "Config change",
        0x0030 => "Target software",
        0x0080 => "Wireless signal strength",
        0x00C0..=0x00C3 => "Firmware update control",
        0x00D0 => "Firmware update",
        0x1000 => "Battery status",
        0x1001 => "Battery voltage",
        0x1004 => "Unified battery",
        0x1010 => "Charging control",
        0x1300 => "LED control",
        0x1802 => "Device reset",
        0x1805 => "OOB state",
        0x1806 => "Configurable device properties",
        0x1814 => "Change host (Easy-Switch)",
        0x1815 => "Hosts info",
        0x1981..=0x1983 => "Backlight",
        0x1B00..=0x1B03 => "Reprogrammable controls (legacy)",
        0x1B04 => "Reprogrammable controls v4",
        0x1C00 => "Persistent remappable action",
        0x1D4B => "Wireless device status",
        0x1DF0 => "Remaining pairings",
        0x1F20 => "ADC measurement",
        0x2001 => "Left/right swap",
        0x2006 => "Pointer axis orientation",
        0x2100 => "Vertical scrolling",
        0x2110 => "SmartShift",
        0x2111 => "SmartShift with tunable torque",
        0x2120 => "Hi-res scrolling (legacy)",
        0x2121 => "Hi-res wheel",
        0x2130 => "Low-res wheel",
        0x2150 => "Thumb wheel",
        0x2200 => "Mouse pointer",
        0x2201 => "Adjustable DPI",
        0x2202 => "Extended adjustable DPI",
        0x2205 => "Pointer speed",
        0x2230 => "Angle snapping",
        0x2240 => "Surface tuning",
        0x2250 => "XY stats",
        0x2251 => "Wheel stats",
        0x2400 => "Hybrid tracking",
        0x40A0 | 0x40A2 | 0x40A3 => "Fn inversion",
        0x4100 => "Encryption",
        0x4220 => "Lock key state",
        0x4520 | 0x4540 => "Keyboard layout",
        0x4521 => "Disable keys",
        0x4522 => "Disable keys by usage",
        0x4530 => "Dual platform",
        0x4531 => "Multi-platform",
        0x4600 => "Crown",
        _ => return None,
    })
}
