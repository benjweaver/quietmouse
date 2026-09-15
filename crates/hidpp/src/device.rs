//! A HID++ 2.0 device: feature discovery, identity, and notification parsing.

use std::collections::HashMap;

use crate::features::{self, battery, battery::Battery};
use crate::{Error, Link, Report, Result, Session};

/// Features looked up when a device is opened: everything this crate drives.
const KNOWN_FEATURES: &[u16] = &[
    features::DEVICE_NAME,
    features::BATTERY_STATUS,
    features::BATTERY_VOLTAGE,
    features::UNIFIED_BATTERY,
    features::CHANGE_HOST,
    features::REPROG_CONTROLS_V4,
    features::WIRELESS_DEVICE_STATUS,
    features::SMART_SHIFT,
    features::SMART_SHIFT_ENHANCED,
    features::HIRES_WHEEL,
    features::THUMB_WHEEL,
    features::ADJUSTABLE_DPI,
];

/// Sub-ids from here up are HID++ 1.0 notifications, never feature indices.
const FIRST_HIDPP10_NOTIFICATION: u8 = 0x40;

const PING_DATA: u8 = 0x5A;

/// Wireless device status value announcing a reconnection.
const STATUS_RECONNECTED: u8 = 0x01;

/// Thumb wheel rotation status announcing the start of a new movement.
const THUMB_START: u8 = 0x01;

#[derive(Debug, Clone)]
pub struct Device {
    index: u8,
    protocol: (u8, u8),
    name: String,
    features: HashMap<u16, u8>,
}

/// Something a device reported on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    /// Diverted controls now held down; empty once all are released.
    Buttons(Vec<u16>),
    /// Pointer movement while a raw-XY-diverted control is held.
    RawXy {
        dx: i16,
        dy: i16,
    },
    /// Diverted thumb wheel rotation. `start` marks the first report of a new movement.
    ThumbWheel {
        rotation: i16,
        start: bool,
    },
    Battery(Battery),
    /// The device reconnected and has forgotten its volatile settings.
    Reconnected,
}

impl Device {
    /// Pings the device at `index` and discovers the features this crate uses.
    pub fn open<L: Link>(session: &mut Session<L>, index: u8) -> Result<Self> {
        let protocol = Self::ping(session, index)?;
        let mut features = HashMap::new();
        for &id in KNOWN_FEATURES {
            if let Some(feature_index) = lookup(session, index, id)? {
                features.insert(id, feature_index);
            }
        }
        let mut device = Self {
            index,
            protocol,
            name: String::new(),
            features,
        };
        device.name = device.read_name(session)?;
        Ok(device)
    }

    /// Returns the HID++ protocol version of the device at `index`.
    ///
    /// A receiver, or a HID++ 1.0 device, answers with [`Error::Hidpp10`].
    pub fn ping<L: Link>(session: &mut Session<L>, index: u8) -> Result<(u8, u8)> {
        let reply = session.call(index, 0, 1, &[0, 0, PING_DATA])?;
        Ok((reply.param(0), reply.param(1)))
    }

    pub fn index(&self) -> u8 {
        self.index
    }

    pub fn protocol(&self) -> (u8, u8) {
        self.protocol
    }

    pub fn name(&self) -> &str {
        if self.name.is_empty() {
            "Unnamed device"
        } else {
            &self.name
        }
    }

    pub fn has(&self, feature: u16) -> bool {
        self.features.contains_key(&feature)
    }

    pub fn feature_index(&self, feature: u16) -> Option<u8> {
        self.features.get(&feature).copied()
    }

    fn feature_at(&self, index: u8) -> Option<u16> {
        self.features.iter().find(|&(_, &i)| i == index).map(|(&id, _)| id)
    }

    /// Calls `function` of `feature`.
    pub fn call<L: Link>(&self, session: &mut Session<L>, feature: u16, function: u8, params: &[u8]) -> Result<Report> {
        let index = self.feature_index(feature).ok_or(Error::Unsupported(feature))?;
        session.call(self.index, index, function, params)
    }

    /// Calls `function` of `feature` without waiting for a reply.
    pub fn call_no_reply<L: Link>(
        &self,
        session: &mut Session<L>,
        feature: u16,
        function: u8,
        params: &[u8],
    ) -> Result<()> {
        let index = self.feature_index(feature).ok_or(Error::Unsupported(feature))?;
        session.call_no_reply(self.index, index, function, params)
    }

    /// Every feature the device implements, as `(id, index)` pairs.
    pub fn all_features<L: Link>(&self, session: &mut Session<L>) -> Result<Vec<(u16, u8)>> {
        let set =
            lookup(session, self.index, features::FEATURE_SET)?.ok_or(Error::Unsupported(features::FEATURE_SET))?;
        let count = session.call(self.index, set, 0, &[])?.param(0);
        let mut all = vec![(features::ROOT, 0)];
        for index in 1..=count {
            all.push((session.call(self.index, set, 1, &[index])?.u16_at(0), index));
        }
        Ok(all)
    }

    fn read_name<L: Link>(&self, session: &mut Session<L>) -> Result<String> {
        if !self.has(features::DEVICE_NAME) {
            return Ok(String::new());
        }
        let len = self.call(session, features::DEVICE_NAME, 0, &[])?.param(0);
        let mut bytes = Vec::with_capacity(usize::from(len));
        // `bytes.len() < len <= u8::MAX` keeps every offset in range.
        while let Ok(offset) = u8::try_from(bytes.len()) {
            if offset >= len {
                break;
            }
            let chunk = self.call(session, features::DEVICE_NAME, 1, &[offset])?;
            let remaining = usize::from(len - offset);
            let part: Vec<u8> = chunk
                .params()
                .iter()
                .copied()
                .take(remaining)
                .take_while(|&b| b != 0)
                .collect();
            if part.is_empty() {
                break;
            }
            bytes.extend(part);
        }
        Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
    }

    /// Interprets a notification from this device, if it's one this crate understands.
    pub fn parse_event(&self, report: &Report) -> Option<DeviceEvent> {
        if report.device_index() != self.index
            || report.sub_id() >= FIRST_HIDPP10_NOTIFICATION
            || report.software_id() != 0
        {
            return None;
        }
        match (self.feature_at(report.sub_id())?, report.function()) {
            (features::REPROG_CONTROLS_V4, 0) => Some(DeviceEvent::Buttons(
                (0..4).map(|i| report.u16_at(i * 2)).filter(|&cid| cid != 0).collect(),
            )),
            (features::REPROG_CONTROLS_V4, 1) => Some(DeviceEvent::RawXy {
                dx: report.i16_at(0),
                dy: report.i16_at(2),
            }),
            (features::THUMB_WHEEL, 0) => Some(DeviceEvent::ThumbWheel {
                rotation: report.i16_at(0),
                start: report.param(4) == THUMB_START,
            }),
            (features::UNIFIED_BATTERY, 0) => Some(DeviceEvent::Battery(battery::parse_unified(report))),
            (features::BATTERY_STATUS, 0) => Some(DeviceEvent::Battery(battery::parse_status(report))),
            (features::WIRELESS_DEVICE_STATUS, 0) if report.param(0) == STATUS_RECONNECTED => {
                Some(DeviceEvent::Reconnected)
            }
            _ => None,
        }
    }
}

/// Looks up the index of `feature` through the root feature; `None` if absent.
fn lookup<L: Link>(session: &mut Session<L>, device: u8, feature: u16) -> Result<Option<u8>> {
    let reply = session.call(device, 0, 0, &feature.to_be_bytes())?;
    Ok(match reply.param(0) {
        0 => None,
        index => Some(index),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{DEVICE_NAME, REPROG_CONTROLS_V4, THUMB_WHEEL, WIRELESS_DEVICE_STATUS};
    use crate::testing::fake_device;

    const NAME: &[u8] = b"MX Master 3S";

    fn open_fake() -> (Session<crate::testing::MockLink>, Device) {
        let link = fake_device(
            0xFF,
            &[DEVICE_NAME, REPROG_CONTROLS_V4, THUMB_WHEEL, WIRELESS_DEVICE_STATUS],
            |feature, function, params| match (feature, function) {
                (DEVICE_NAME, 0) => Some(vec![u8::try_from(NAME.len()).unwrap()]),
                (DEVICE_NAME, 1) => Some(NAME[usize::from(params[0])..].iter().copied().take(16).collect()),
                _ => None,
            },
        );
        let mut session = Session::new(link);
        let device = Device::open(&mut session, 0xFF).unwrap();
        (session, device)
    }

    #[test]
    fn open_discovers_features_and_name() {
        let (_, device) = open_fake();
        assert_eq!(device.name(), "MX Master 3S");
        assert_eq!(device.protocol(), (4, 5));
        assert_eq!(device.feature_index(REPROG_CONTROLS_V4), Some(2));
        assert!(!device.has(features::ADJUSTABLE_DPI));
    }

    #[test]
    fn unsupported_features_fail_without_a_request() {
        let (mut session, device) = open_fake();
        let sent = session.link_mut().sent.len();
        assert_eq!(
            device.call(&mut session, features::ADJUSTABLE_DPI, 2, &[0]),
            Err(Error::Unsupported(features::ADJUSTABLE_DPI))
        );
        assert_eq!(session.link_mut().sent.len(), sent);
    }

    #[test]
    fn parses_button_and_movement_events() {
        let (_, device) = open_fake();
        let pressed = Report::long(0xFF, 2, 0x00, &[0x00, 0xC3, 0x00, 0x56]);
        assert_eq!(
            device.parse_event(&pressed),
            Some(DeviceEvent::Buttons(vec![0xC3, 0x56]))
        );
        let released = Report::long(0xFF, 2, 0x00, &[]);
        assert_eq!(device.parse_event(&released), Some(DeviceEvent::Buttons(vec![])));
        let moved = Report::long(0xFF, 2, 0x10, &[0xFF, 0xF6, 0x00, 0x14]);
        assert_eq!(device.parse_event(&moved), Some(DeviceEvent::RawXy { dx: -10, dy: 20 }));
    }

    #[test]
    fn parses_thumb_wheel_and_reconnect() {
        let (_, device) = open_fake();
        let rotation = Report::long(0xFF, 3, 0x00, &[0xFF, 0xFE, 0x12, 0x34, 0x01, 0x00]);
        assert_eq!(
            device.parse_event(&rotation),
            Some(DeviceEvent::ThumbWheel {
                rotation: -2,
                start: true
            })
        );
        let reconnect = Report::long(0xFF, 4, 0x00, &[0x01, 0x01, 0x01]);
        assert_eq!(device.parse_event(&reconnect), Some(DeviceEvent::Reconnected));
    }

    #[test]
    fn ignores_replies_and_other_devices() {
        let (_, device) = open_fake();
        let late_reply = Report::long(0xFF, 2, 0x05, &[0x00, 0xC3]);
        let other_device = Report::long(0x01, 2, 0x00, &[0x00, 0xC3]);
        let receiver_notice = Report::short(0xFF, 0x41, 0x04, &[0x00, 0x34, 0xB0]);
        assert_eq!(device.parse_event(&late_reply), None);
        assert_eq!(device.parse_event(&other_device), None);
        assert_eq!(device.parse_event(&receiver_notice), None);
    }
}
