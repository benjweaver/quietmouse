//! Linux output through a uinput virtual device, which works on X11 and Wayland alike.

use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Context, ensure};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};

use super::Output;
use crate::keys::{Chord, Desktop, Key, MAX_FUNCTION_KEY, MediaKey, Modifier, MouseButton, PUNCTUATION};

/// Time for the display server to pick up a newly created input device.
const SETTLE: Duration = Duration::from_millis(200);

pub struct Backend {
    device: VirtualDevice,
}

impl Backend {
    pub fn new() -> anyhow::Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in all_codes() {
            keys.insert(code);
        }
        // Relative axes make libinput treat the device as a mouse too, so clicks work.
        let mut axes = AttributeSet::<RelativeAxisCode>::new();
        axes.insert(RelativeAxisCode::REL_X);
        axes.insert(RelativeAxisCode::REL_Y);
        let device = VirtualDevice::builder()
            .and_then(|builder| builder.name("quietmouse virtual input").with_keys(&keys))
            .and_then(|builder| builder.with_relative_axes(&axes))
            .and_then(|builder| builder.build())
            .context("can't create a uinput device; install packaging/linux/70-quietmouse.rules")?;
        thread::sleep(SETTLE);
        Ok(Self { device })
    }

    pub fn perform(&mut self, output: &Output) -> anyhow::Result<()> {
        let codes: Vec<KeyCode> = match output {
            Output::Chord(chord) => chord_codes(chord),
            Output::Media(media_key) => vec![media(*media_key)],
            Output::Click(mouse_button) => vec![button(*mouse_button)],
            Output::Desktop(desktop) => {
                if kde_session() {
                    match invoke_kwin_shortcut(*desktop) {
                        Ok(()) => return Ok(()),
                        Err(error) => log::debug!("{error:#}; pressing the shortcut keys instead"),
                    }
                }
                chord_codes(&desktop_chord(*desktop))
            }
        };
        for &code in &codes {
            self.device.emit(&[key_event(code, 1)])?;
        }
        for &code in codes.iter().rev() {
            self.device.emit(&[key_event(code, 0)])?;
        }
        Ok(())
    }
}

fn chord_codes(chord: &Chord) -> Vec<KeyCode> {
    chord
        .modifiers
        .iter()
        .map(|&m| modifier(m))
        .chain(chord.key.map(key))
        .collect()
}

/// Whether this is a KDE Plasma session, which SteamOS's desktop mode, CachyOS
/// and Bazzite use by default.
fn kde_session() -> bool {
    is_kde(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref())
}

/// `XDG_CURRENT_DESKTOP` is a colon-separated list, such as `KDE` or `ubuntu:GNOME`.
fn is_kde(current_desktop: Option<&str>) -> bool {
    current_desktop.is_some_and(|desktops| desktops.split(':').any(|name| name.eq_ignore_ascii_case("KDE")))
}

/// KWin's name for each desktop action. Invoking a shortcut by name follows the
/// user's own binding and works on Wayland, like the registered-shortcut path on macOS.
fn kwin_shortcut(desktop: Desktop) -> &'static str {
    match desktop {
        Desktop::Overview => "Overview",
        Desktop::AppWindows => "ExposeClass",
        Desktop::Show => "Show Desktop",
        Desktop::Left => "Switch One Desktop to the Left",
        Desktop::Right => "Switch One Desktop to the Right",
    }
}

/// Asks KWin to run the shortcut, over the session bus with systemd's `busctl`.
fn invoke_kwin_shortcut(desktop: Desktop) -> anyhow::Result<()> {
    let shortcut = kwin_shortcut(desktop);
    let output = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.kde.kglobalaccel",
            "/component/kwin",
            "org.kde.kglobalaccel.Component",
            "invokeShortcut",
            "s",
            shortcut,
        ])
        .output()
        .context("can't run busctl")?;
    ensure!(
        output.status.success(),
        "KWin didn't run \"{shortcut}\": {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// GNOME's default shortcuts, used outside KDE Plasma. Other desktops can bind
/// `keys` or `shell` actions instead.
fn desktop_chord(desktop: Desktop) -> Chord {
    let (modifiers, key) = match desktop {
        Desktop::Overview | Desktop::AppWindows => (vec![Modifier::Meta], None),
        Desktop::Show => (vec![Modifier::Meta], Some(Key::Char('d'))),
        Desktop::Left => (vec![Modifier::Ctrl, Modifier::Alt], Some(Key::Left)),
        Desktop::Right => (vec![Modifier::Ctrl, Modifier::Alt], Some(Key::Right)),
    };
    Chord { modifiers, key }
}

fn key_event(code: KeyCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, value)
}

const LETTERS: [KeyCode; 26] = [
    KeyCode::KEY_A,
    KeyCode::KEY_B,
    KeyCode::KEY_C,
    KeyCode::KEY_D,
    KeyCode::KEY_E,
    KeyCode::KEY_F,
    KeyCode::KEY_G,
    KeyCode::KEY_H,
    KeyCode::KEY_I,
    KeyCode::KEY_J,
    KeyCode::KEY_K,
    KeyCode::KEY_L,
    KeyCode::KEY_M,
    KeyCode::KEY_N,
    KeyCode::KEY_O,
    KeyCode::KEY_P,
    KeyCode::KEY_Q,
    KeyCode::KEY_R,
    KeyCode::KEY_S,
    KeyCode::KEY_T,
    KeyCode::KEY_U,
    KeyCode::KEY_V,
    KeyCode::KEY_W,
    KeyCode::KEY_X,
    KeyCode::KEY_Y,
    KeyCode::KEY_Z,
];

const DIGITS: [KeyCode; 10] = [
    KeyCode::KEY_0,
    KeyCode::KEY_1,
    KeyCode::KEY_2,
    KeyCode::KEY_3,
    KeyCode::KEY_4,
    KeyCode::KEY_5,
    KeyCode::KEY_6,
    KeyCode::KEY_7,
    KeyCode::KEY_8,
    KeyCode::KEY_9,
];

const FUNCTION_KEYS: [KeyCode; 20] = [
    KeyCode::KEY_F1,
    KeyCode::KEY_F2,
    KeyCode::KEY_F3,
    KeyCode::KEY_F4,
    KeyCode::KEY_F5,
    KeyCode::KEY_F6,
    KeyCode::KEY_F7,
    KeyCode::KEY_F8,
    KeyCode::KEY_F9,
    KeyCode::KEY_F10,
    KeyCode::KEY_F11,
    KeyCode::KEY_F12,
    KeyCode::KEY_F13,
    KeyCode::KEY_F14,
    KeyCode::KEY_F15,
    KeyCode::KEY_F16,
    KeyCode::KEY_F17,
    KeyCode::KEY_F18,
    KeyCode::KEY_F19,
    KeyCode::KEY_F20,
];

const NAMED: [Key; 14] = [
    Key::Tab,
    Key::Space,
    Key::Enter,
    Key::Escape,
    Key::Backspace,
    Key::Delete,
    Key::Up,
    Key::Down,
    Key::Left,
    Key::Right,
    Key::Home,
    Key::End,
    Key::PageUp,
    Key::PageDown,
];

const MODIFIERS: [Modifier; 4] = [Modifier::Ctrl, Modifier::Shift, Modifier::Alt, Modifier::Meta];

const MEDIA: [MediaKey; 6] = [
    MediaKey::PlayPause,
    MediaKey::Next,
    MediaKey::Previous,
    MediaKey::VolumeUp,
    MediaKey::VolumeDown,
    MediaKey::Mute,
];

const BUTTONS: [MouseButton; 5] = [
    MouseButton::Left,
    MouseButton::Right,
    MouseButton::Middle,
    MouseButton::Back,
    MouseButton::Forward,
];

/// Every code this backend can emit; uinput devices must declare them up front.
fn all_codes() -> impl Iterator<Item = KeyCode> {
    LETTERS
        .into_iter()
        .chain(DIGITS)
        .chain(FUNCTION_KEYS.into_iter().take(usize::from(MAX_FUNCTION_KEY)))
        .chain(PUNCTUATION.chars().map(char_code))
        .chain(NAMED.into_iter().map(key))
        .chain(MODIFIERS.into_iter().map(modifier))
        .chain(MEDIA.into_iter().map(media))
        .chain(BUTTONS.into_iter().map(button))
}

fn modifier(modifier: Modifier) -> KeyCode {
    match modifier {
        Modifier::Ctrl => KeyCode::KEY_LEFTCTRL,
        Modifier::Shift => KeyCode::KEY_LEFTSHIFT,
        Modifier::Alt => KeyCode::KEY_LEFTALT,
        Modifier::Meta => KeyCode::KEY_LEFTMETA,
    }
}

fn key(key: Key) -> KeyCode {
    match key {
        Key::Char(c) => char_code(c),
        Key::Tab => KeyCode::KEY_TAB,
        Key::Space => KeyCode::KEY_SPACE,
        Key::Enter => KeyCode::KEY_ENTER,
        Key::Escape => KeyCode::KEY_ESC,
        Key::Backspace => KeyCode::KEY_BACKSPACE,
        Key::Delete => KeyCode::KEY_DELETE,
        Key::Up => KeyCode::KEY_UP,
        Key::Down => KeyCode::KEY_DOWN,
        Key::Left => KeyCode::KEY_LEFT,
        Key::Right => KeyCode::KEY_RIGHT,
        Key::Home => KeyCode::KEY_HOME,
        Key::End => KeyCode::KEY_END,
        Key::PageUp => KeyCode::KEY_PAGEUP,
        Key::PageDown => KeyCode::KEY_PAGEDOWN,
        Key::F(n) => FUNCTION_KEYS[usize::from(n.clamp(1, MAX_FUNCTION_KEY)) - 1],
    }
}

/// US-layout key for a character accepted by chord parsing.
fn char_code(c: char) -> KeyCode {
    match c {
        'a'..='z' => LETTERS[c as usize - 'a' as usize],
        '0'..='9' => DIGITS[c as usize - '0' as usize],
        '-' => KeyCode::KEY_MINUS,
        '=' => KeyCode::KEY_EQUAL,
        '[' => KeyCode::KEY_LEFTBRACE,
        ']' => KeyCode::KEY_RIGHTBRACE,
        '\\' => KeyCode::KEY_BACKSLASH,
        ';' => KeyCode::KEY_SEMICOLON,
        '\'' => KeyCode::KEY_APOSTROPHE,
        ',' => KeyCode::KEY_COMMA,
        '.' => KeyCode::KEY_DOT,
        '/' => KeyCode::KEY_SLASH,
        '`' => KeyCode::KEY_GRAVE,
        _ => KeyCode::KEY_UNKNOWN,
    }
}

fn media(media_key: MediaKey) -> KeyCode {
    match media_key {
        MediaKey::PlayPause => KeyCode::KEY_PLAYPAUSE,
        MediaKey::Next => KeyCode::KEY_NEXTSONG,
        MediaKey::Previous => KeyCode::KEY_PREVIOUSSONG,
        MediaKey::VolumeUp => KeyCode::KEY_VOLUMEUP,
        MediaKey::VolumeDown => KeyCode::KEY_VOLUMEDOWN,
        MediaKey::Mute => KeyCode::KEY_MUTE,
    }
}

fn button(mouse_button: MouseButton) -> KeyCode {
    match mouse_button {
        MouseButton::Left => KeyCode::BTN_LEFT,
        MouseButton::Right => KeyCode::BTN_RIGHT,
        MouseButton::Middle => KeyCode::BTN_MIDDLE,
        MouseButton::Back => KeyCode::BTN_SIDE,
        MouseButton::Forward => KeyCode::BTN_EXTRA,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_plasma_sessions() {
        assert!(is_kde(Some("KDE")));
        assert!(is_kde(Some("ubuntu:kde")));
        assert!(!is_kde(Some("GNOME")));
        assert!(!is_kde(Some("ubuntu:GNOME")));
        assert!(!is_kde(Some("")));
        assert!(!is_kde(None));
    }

    #[test]
    fn desktop_actions_use_kwin_and_gnome_names() {
        assert_eq!(kwin_shortcut(Desktop::Left), "Switch One Desktop to the Left");
        assert_eq!(kwin_shortcut(Desktop::Overview), "Overview");
        let left = desktop_chord(Desktop::Left);
        assert_eq!(left.modifiers, vec![Modifier::Ctrl, Modifier::Alt]);
        assert_eq!(left.key, Some(Key::Left));
    }

    #[test]
    fn every_chord_key_has_a_code() {
        for c in "abcdefghijklmnopqrstuvwxyz0123456789"
            .chars()
            .chain(PUNCTUATION.chars())
        {
            assert_ne!(char_code(c), KeyCode::KEY_UNKNOWN, "{c:?}");
        }
    }
}
