//! macOS output that bypasses enigo: named keys and desktop actions, posted
//! straight through CoreGraphics with the flags a physical keyboard sets.
//!
//! Desktop actions (Mission Control, Spaces, ...) are triggered through the
//! shortcut macOS has registered for them in Keyboard Shortcuts. A rebound
//! shortcut keeps working, and a disabled one is reported instead of silently
//! doing nothing. macOS has no public API to switch Spaces directly.

use std::path::PathBuf;

use anyhow::{anyhow, bail};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use crate::keys::{Chord, Desktop, Key, MAX_FUNCTION_KEY, Modifier};

/// Where macOS keeps changes to its system shortcuts ("symbolic hot keys").
/// Only shortcuts the user has touched have an entry.
const PREFERENCES: &str = "Library/Preferences/com.apple.symbolichotkeys.plist";

/// Control + Fn: how macOS registers its arrow-key shortcuts.
const CONTROL_FN: u64 = 0x0084_0000;
/// Fn alone, as on F11 for Show Desktop.
const FN: u64 = 0x0080_0000;

/// Virtual key codes for F1–F20 (HIToolbox `kVK_F*`).
const FUNCTION_KEYS: [CGKeyCode; 20] = [
    0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71, 0x6A, 0x40, 0x4F, 0x50,
    0x5A,
];

/// A system shortcut, with the binding macOS ships with.
struct Hotkey {
    id: u32,
    name: &'static str,
    code: CGKeyCode,
    flags: u64,
}

fn hotkey(desktop: Desktop) -> Hotkey {
    let (id, name, code, flags) = match desktop {
        Desktop::Overview => (32, "Mission Control", 0x7E, CONTROL_FN),
        Desktop::AppWindows => (33, "Application windows", 0x7D, CONTROL_FN),
        Desktop::Show => (36, "Show Desktop", 0x67, FN),
        Desktop::Left => (79, "Move left a space", 0x7B, CONTROL_FN),
        Desktop::Right => (81, "Move right a space", 0x7C, CONTROL_FN),
    };
    Hotkey { id, name, code, flags }
}

/// Triggers `desktop` through the shortcut macOS has registered for it: the
/// user's own binding if they changed it, the system default otherwise.
pub fn perform_desktop(desktop: Desktop) -> anyhow::Result<()> {
    let hotkey = hotkey(desktop);
    let registered = read_preferences().and_then(|preferences| registered(&preferences, hotkey.id));
    if registered.is_some_and(|entry| !entry.enabled) {
        bail!(
            "the \"{}\" shortcut is off; turn it on in System Settings → Keyboard → Keyboard Shortcuts → Mission Control",
            hotkey.name
        );
    }
    let (code, flags) = registered
        .and_then(|entry| entry.binding)
        .unwrap_or((hotkey.code, hotkey.flags));
    post(code, CGEventFlags::from_bits_retain(flags))
}

/// Posts `chord` if its key is a named key; `None` leaves it to enigo, which
/// maps letters and digits through the active keyboard layout.
pub fn post_named_key(chord: &Chord) -> Option<anyhow::Result<()>> {
    let (code, key_flags) = named_key(chord.key?)?;
    let flags = chord
        .modifiers
        .iter()
        .fold(key_flags, |flags, &modifier| flags | modifier_flag(modifier));
    Some(post(code, flags))
}

fn post(code: CGKeyCode, flags: CGEventFlags) -> anyhow::Result<()> {
    for key_down in [true, false] {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|()| anyhow!("can't create a CoreGraphics event source"))?;
        let event = CGEvent::new_keyboard_event(source, code, key_down)
            .map_err(|()| anyhow!("can't create a key event for code {code:#04x}"))?;
        event.set_flags(flags);
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Registered {
    enabled: bool,
    /// Key code and modifier flags, when the user changed the binding.
    binding: Option<(CGKeyCode, u64)>,
}

fn read_preferences() -> Option<plist::Value> {
    let path: PathBuf = dirs::home_dir()?.join(PREFERENCES);
    match plist::Value::from_file(&path) {
        Ok(preferences) => Some(preferences),
        Err(error) => {
            log::debug!("can't read {}: {error}", path.display());
            None
        }
    }
}

/// The user's entry for hotkey `id`:
/// `{ enabled, value = { parameters = (character, key code, modifier flags) } }`.
fn registered(preferences: &plist::Value, id: u32) -> Option<Registered> {
    let entry = preferences
        .as_dictionary()?
        .get("AppleSymbolicHotKeys")?
        .as_dictionary()?
        .get(&id.to_string())?
        .as_dictionary()?;
    let enabled = entry
        .get("enabled")
        .and_then(|value| {
            value
                .as_boolean()
                .or_else(|| value.as_unsigned_integer().map(|n| n != 0))
        })
        .unwrap_or(true);
    let binding = entry
        .get("value")
        .and_then(plist::Value::as_dictionary)
        .and_then(|value| value.get("parameters"))
        .and_then(plist::Value::as_array)
        .and_then(|parameters| {
            let code = CGKeyCode::try_from(parameters.get(1)?.as_unsigned_integer()?).ok()?;
            Some((code, parameters.get(2)?.as_unsigned_integer()?))
        });
    Some(Registered { enabled, binding })
}

fn modifier_flag(modifier: Modifier) -> CGEventFlags {
    match modifier {
        Modifier::Ctrl => CGEventFlags::CGEventFlagControl,
        Modifier::Shift => CGEventFlags::CGEventFlagShift,
        Modifier::Alt => CGEventFlags::CGEventFlagAlternate,
        Modifier::Meta => CGEventFlags::CGEventFlagCommand,
    }
}

/// Key code, and the flags a physical keyboard adds to it.
fn named_key(key: Key) -> Option<(CGKeyCode, CGEventFlags)> {
    let function = CGEventFlags::CGEventFlagSecondaryFn;
    let arrow = function | CGEventFlags::CGEventFlagNumericPad;
    let plain = CGEventFlags::empty();
    Some(match key {
        Key::Up => (0x7E, arrow),
        Key::Down => (0x7D, arrow),
        Key::Left => (0x7B, arrow),
        Key::Right => (0x7C, arrow),
        Key::Home => (0x73, function),
        Key::End => (0x77, function),
        Key::PageUp => (0x74, function),
        Key::PageDown => (0x79, function),
        Key::Delete => (0x75, function),
        Key::F(n) => (FUNCTION_KEYS[usize::from(n.clamp(1, MAX_FUNCTION_KEY)) - 1], function),
        Key::Tab => (0x30, plain),
        Key::Space => (0x31, plain),
        Key::Enter => (0x24, plain),
        Key::Escape => (0x35, plain),
        Key::Backspace => (0x33, plain),
        Key::Char(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFERENCES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>AppleSymbolicHotKeys</key>
  <dict>
    <key>79</key>
    <dict>
      <key>enabled</key><true/>
      <key>value</key>
      <dict>
        <key>parameters</key>
        <array><integer>65535</integer><integer>123</integer><integer>11796480</integer></array>
        <key>type</key><string>standard</string>
      </dict>
    </dict>
    <key>81</key>
    <dict><key>enabled</key><false/></dict>
    <key>82</key>
    <dict><key>enabled</key><integer>1</integer></dict>
  </dict>
</dict>
</plist>"#;

    #[test]
    fn reads_rebound_disabled_and_default_shortcuts() {
        let preferences = plist::Value::from_reader_xml(PREFERENCES_XML.as_bytes()).unwrap();
        assert_eq!(
            registered(&preferences, 79),
            Some(Registered {
                enabled: true,
                binding: Some((123, 11_796_480))
            })
        );
        assert_eq!(
            registered(&preferences, 81),
            Some(Registered {
                enabled: false,
                binding: None
            })
        );
        assert_eq!(
            registered(&preferences, 82),
            Some(Registered {
                enabled: true,
                binding: None
            })
        );
        assert_eq!(registered(&preferences, 32), None);
    }

    #[test]
    fn spaces_default_to_control_arrow_with_fn() {
        let left = hotkey(Desktop::Left);
        assert_eq!((left.id, left.code, left.flags), (79, 0x7B, 0x0084_0000));
        let flags = CGEventFlags::from_bits_retain(left.flags);
        assert!(flags.contains(CGEventFlags::CGEventFlagControl | CGEventFlags::CGEventFlagSecondaryFn));
    }

    #[test]
    fn arrows_carry_fn_and_keypad_flags() {
        let chord: Chord = "ctrl+left".parse().unwrap();
        let (code, flags) = named_key(chord.key.unwrap()).unwrap();
        assert_eq!(code, 0x7B);
        let all = flags | modifier_flag(Modifier::Ctrl);
        assert_eq!(all.bits(), CONTROL_FN | CGEventFlags::CGEventFlagNumericPad.bits());
    }

    #[test]
    fn characters_are_left_to_the_layout_aware_path() {
        let chord: Chord = "cmd+c".parse().unwrap();
        assert!(post_named_key(&chord).is_none());
    }
}
