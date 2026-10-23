//! Keyboard settings: what the F-row sends without Fn (the three FN_INVERSION
//! variants), and switching off keys such as Caps Lock (DISABLE_KEYS).

use std::fmt;

use crate::features::{CHANGE_HOST, DISABLE_KEYS, FN_INVERSION, FN_INVERSION_HOSTS, FN_INVERSION_NEW, host};
use crate::{Device, Error, Link, Result, Session};

/// Host index meaning "whichever channel is in use", for devices that won't say.
const CURRENT_HOST: u8 = 0xFF;
const INVERTED: u8 = 0x01;

/// What F1–F12 do when pressed on their own. Holding Fn gives the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKeys {
    /// F1–F12.
    Standard,
    /// The media, brightness and other functions printed on the keys.
    Special,
}

impl FunctionKeys {
    fn from_inverted(inverted: bool) -> Self {
        if inverted { Self::Special } else { Self::Standard }
    }

    fn inverted(self) -> u8 {
        match self {
            Self::Standard => 0,
            Self::Special => INVERTED,
        }
    }
}

/// Which of the three feature versions the device has. The newest keeps a
/// setting per Easy-Switch channel, so every call names the channel first,
/// and its replies echo it back ahead of the value.
fn fn_feature(device: &Device) -> Option<(u16, bool)> {
    [
        (FN_INVERSION_HOSTS, true),
        (FN_INVERSION_NEW, false),
        (FN_INVERSION, false),
    ]
    .into_iter()
    .find(|&(feature, _)| device.has(feature))
}

fn current_host<L: Link>(session: &mut Session<L>, device: &Device) -> Result<u8> {
    if !device.has(CHANGE_HOST) {
        return Ok(CURRENT_HOST);
    }
    Ok(host::read(session, device)?.current)
}

pub fn function_keys<L: Link>(session: &mut Session<L>, device: &Device) -> Result<FunctionKeys> {
    let (feature, per_host) = fn_feature(device).ok_or(Error::Unsupported(FN_INVERSION))?;
    let inverted = if per_host {
        let host = current_host(session, device)?;
        device.call(session, feature, 0, &[host])?.param(1)
    } else {
        device.call(session, feature, 0, &[])?.param(0)
    };
    Ok(FunctionKeys::from_inverted(inverted & INVERTED != 0))
}

/// Sets the F-row for the channel in use; other Easy-Switch channels keep their own.
pub fn set_function_keys<L: Link>(session: &mut Session<L>, device: &Device, keys: FunctionKeys) -> Result<()> {
    let (feature, per_host) = fn_feature(device).ok_or(Error::Unsupported(FN_INVERSION))?;
    if per_host {
        let host = current_host(session, device)?;
        device.call(session, feature, 1, &[host, keys.inverted()])?;
    } else {
        device.call(session, feature, 1, &[keys.inverted()])?;
    }
    Ok(())
}

/// A set of keys that can be switched off, as the device's bit mask.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Keys(pub u8);

impl Keys {
    pub const CAPS_LOCK: Self = Self(0x01);
    pub const NUM_LOCK: Self = Self(0x02);
    pub const SCROLL_LOCK: Self = Self(0x04);
    pub const INSERT: Self = Self(0x08);
    /// The Windows key, or Super.
    pub const WIN: Self = Self(0x10);

    const NAMED: [(Self, &str); 5] = [
        (Self::CAPS_LOCK, "Caps Lock"),
        (Self::NUM_LOCK, "Num Lock"),
        (Self::SCROLL_LOCK, "Scroll Lock"),
        (Self::INSERT, "Insert"),
        (Self::WIN, "Win"),
    ];

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    #[must_use]
    pub fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl fmt::Display for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        let mut names: Vec<String> = Self::NAMED
            .iter()
            .filter(|(key, _)| self.contains(*key))
            .map(|&(_, name)| name.to_owned())
            .collect();
        let unnamed = self.0 & !Self::NAMED.iter().fold(0, |all, (key, _)| all | key.0);
        if unnamed != 0 {
            names.push(format!("unknown keys {unnamed:#04x}"));
        }
        f.write_str(&names.join(", "))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisabledKeys {
    /// The keys this keyboard can switch off.
    pub supported: Keys,
    pub disabled: Keys,
}

pub fn disabled_keys<L: Link>(session: &mut Session<L>, device: &Device) -> Result<DisabledKeys> {
    let supported = device.call(session, DISABLE_KEYS, 0, &[])?.param(0);
    let disabled = device.call(session, DISABLE_KEYS, 1, &[])?.param(0);
    Ok(DisabledKeys {
        supported: Keys(supported),
        disabled: Keys(disabled),
    })
}

/// Switches off exactly `keys`, and switches every other key back on.
pub fn set_disabled_keys<L: Link>(session: &mut Session<L>, device: &Device, keys: Keys) -> Result<()> {
    device.call(session, DISABLE_KEYS, 2, &[keys.0])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::testing::fake_device;

    type Calls = Rc<RefCell<Vec<(u16, u8, Vec<u8>)>>>;

    /// A keyboard with `features` whose Fn setting starts `inverted` and whose
    /// DISABLE_KEYS supports Caps Lock and Insert, recording every call.
    fn keyboard(features: &[u16], inverted: u8) -> (Session<crate::testing::MockLink>, Device, Calls) {
        let calls: Calls = Rc::default();
        let log = Rc::clone(&calls);
        let link = fake_device(0xFF, features, move |feature, function, params| {
            log.borrow_mut().push((feature, function, params[..3].to_vec()));
            match (feature, function) {
                (CHANGE_HOST, 0) => Some(vec![3, 1]),
                (FN_INVERSION_HOSTS, 0) => Some(vec![params[0], inverted]),
                (FN_INVERSION | FN_INVERSION_NEW, 0) => Some(vec![inverted, 0]),
                (FN_INVERSION | FN_INVERSION_NEW | FN_INVERSION_HOSTS, 1) => Some(params[..2].to_vec()),
                (DISABLE_KEYS, 0) => Some(vec![0x09]),
                (DISABLE_KEYS, 1) => Some(vec![0x01]),
                (DISABLE_KEYS, 2) => Some(vec![params[0]]),
                _ => None,
            }
        });
        let mut session = Session::new(link);
        let device = Device::open(&mut session, 0xFF).unwrap();
        calls.borrow_mut().clear();
        (session, device, calls)
    }

    fn writes(calls: &Calls, feature: u16) -> Vec<Vec<u8>> {
        calls
            .borrow()
            .iter()
            .filter(|&&(f, function, _)| f == feature && function == 1)
            .map(|(_, _, params)| params.clone())
            .collect()
    }

    #[test]
    fn older_keyboards_read_and_write_one_global_setting() {
        for feature in [FN_INVERSION, FN_INVERSION_NEW] {
            let (mut session, device, calls) = keyboard(&[feature], INVERTED);
            assert_eq!(function_keys(&mut session, &device).unwrap(), FunctionKeys::Special);
            set_function_keys(&mut session, &device, FunctionKeys::Standard).unwrap();
            assert_eq!(writes(&calls, feature), vec![vec![0, 0, 0]], "{feature:#06x}");
        }
    }

    #[test]
    fn per_channel_keyboards_name_the_channel_in_use() {
        let (mut session, device, calls) = keyboard(&[CHANGE_HOST, FN_INVERSION_HOSTS], 0);
        assert_eq!(function_keys(&mut session, &device).unwrap(), FunctionKeys::Standard);
        set_function_keys(&mut session, &device, FunctionKeys::Special).unwrap();
        // Channel 2 of 3 is in use, which is 1 counting from zero.
        assert_eq!(writes(&calls, FN_INVERSION_HOSTS), vec![vec![1, INVERTED, 0]]);
    }

    #[test]
    fn per_channel_keyboards_without_easy_switch_ask_for_the_current_channel() {
        let (mut session, device, calls) = keyboard(&[FN_INVERSION_HOSTS], 0);
        set_function_keys(&mut session, &device, FunctionKeys::Special).unwrap();
        assert_eq!(
            writes(&calls, FN_INVERSION_HOSTS),
            vec![vec![CURRENT_HOST, INVERTED, 0]]
        );
    }

    #[test]
    fn the_newest_fn_feature_wins() {
        let (mut session, device, calls) = keyboard(&[FN_INVERSION, FN_INVERSION_HOSTS], 0);
        set_function_keys(&mut session, &device, FunctionKeys::Special).unwrap();
        assert!(writes(&calls, FN_INVERSION).is_empty());
        assert_eq!(writes(&calls, FN_INVERSION_HOSTS).len(), 1);
    }

    #[test]
    fn a_keyboard_without_fn_inversion_says_so() {
        let (mut session, device, _) = keyboard(&[DISABLE_KEYS], 0);
        assert_eq!(
            function_keys(&mut session, &device),
            Err(Error::Unsupported(FN_INVERSION))
        );
    }

    #[test]
    fn disabled_keys_are_a_bit_mask() {
        let (mut session, device, calls) = keyboard(&[DISABLE_KEYS], 0);
        let state = disabled_keys(&mut session, &device).unwrap();
        assert_eq!(state.supported, Keys::CAPS_LOCK.union(Keys::INSERT));
        assert_eq!(state.disabled, Keys::CAPS_LOCK);
        set_disabled_keys(&mut session, &device, Keys::INSERT).unwrap();
        let sent: Vec<_> = calls
            .borrow()
            .iter()
            .filter(|&&(_, function, _)| function == 2)
            .map(|(_, _, params)| params[0])
            .collect();
        assert_eq!(sent, vec![0x08]);
    }

    #[test]
    fn keys_print_their_names() {
        assert_eq!(Keys::empty().to_string(), "none");
        assert_eq!(Keys::CAPS_LOCK.union(Keys::WIN).to_string(), "Caps Lock, Win");
        assert_eq!(Keys(0x21).to_string(), "Caps Lock, unknown keys 0x20");
    }
}
