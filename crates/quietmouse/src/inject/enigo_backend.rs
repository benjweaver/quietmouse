//! macOS and Windows output through enigo (CGEvent / SendInput). On macOS,
//! named keys and desktop actions go through [`super::macos`] instead.

use anyhow::Context;
use enigo::{Button, Direction, Enigo, Key as EnigoKey, Keyboard, Mouse, Settings};

use super::Output;
#[cfg(target_os = "macos")]
use super::macos;
use crate::keys::{Chord, Desktop, Key, MAX_FUNCTION_KEY, MediaKey, Modifier, MouseButton};

pub struct Backend {
    enigo: Enigo,
}

impl Backend {
    pub fn new() -> anyhow::Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .context("on macOS, allow this app under System Settings → Privacy & Security → Accessibility")?;
        Ok(Self { enigo })
    }

    pub fn perform(&mut self, output: &Output) -> anyhow::Result<()> {
        match output {
            Output::Chord(chord) => {
                if let Some(posted) = post_natively(chord) {
                    return posted;
                }
                self.chord(chord)?;
            }
            Output::Media(media_key) => self.enigo.key(media(*media_key), Direction::Click)?,
            Output::Click(mouse_button) => self.enigo.button(button(*mouse_button), Direction::Click)?,
            Output::Desktop(desktop) => self.desktop(*desktop)?,
        }
        Ok(())
    }

    fn chord(&mut self, chord: &Chord) -> anyhow::Result<()> {
        let modifiers: Vec<EnigoKey> = chord.modifiers.iter().map(|&m| modifier(m)).collect();
        for &m in &modifiers {
            self.enigo.key(m, Direction::Press)?;
        }
        let pressed = chord.key.map_or(Ok(()), |k| self.enigo.key(key(k), Direction::Click));
        // Release modifiers even if the key failed, so none stay stuck down.
        for &m in modifiers.iter().rev() {
            self.enigo.key(m, Direction::Release)?;
        }
        Ok(pressed?)
    }

    /// The shortcut macOS has registered for the action.
    #[cfg(target_os = "macos")]
    fn desktop(&mut self, desktop: Desktop) -> anyhow::Result<()> {
        macos::perform_desktop(desktop)
    }

    /// Task View and virtual desktop shortcuts.
    #[cfg(target_os = "windows")]
    fn desktop(&mut self, desktop: Desktop) -> anyhow::Result<()> {
        let (modifiers, key) = match desktop {
            Desktop::Overview | Desktop::AppWindows => (vec![Modifier::Meta], Key::Tab),
            Desktop::Show => (vec![Modifier::Meta], Key::Char('d')),
            Desktop::Left => (vec![Modifier::Ctrl, Modifier::Meta], Key::Left),
            Desktop::Right => (vec![Modifier::Ctrl, Modifier::Meta], Key::Right),
        };
        self.chord(&Chord {
            modifiers,
            key: Some(key),
        })
    }
}

#[cfg(target_os = "macos")]
fn post_natively(chord: &Chord) -> Option<anyhow::Result<()>> {
    macos::post_named_key(chord)
}

#[cfg(target_os = "windows")]
fn post_natively(_chord: &Chord) -> Option<anyhow::Result<()>> {
    None
}

fn modifier(modifier: Modifier) -> EnigoKey {
    match modifier {
        Modifier::Ctrl => EnigoKey::Control,
        Modifier::Shift => EnigoKey::Shift,
        Modifier::Alt => EnigoKey::Alt,
        Modifier::Meta => EnigoKey::Meta,
    }
}

const FUNCTION_KEYS: [EnigoKey; 20] = [
    EnigoKey::F1,
    EnigoKey::F2,
    EnigoKey::F3,
    EnigoKey::F4,
    EnigoKey::F5,
    EnigoKey::F6,
    EnigoKey::F7,
    EnigoKey::F8,
    EnigoKey::F9,
    EnigoKey::F10,
    EnigoKey::F11,
    EnigoKey::F12,
    EnigoKey::F13,
    EnigoKey::F14,
    EnigoKey::F15,
    EnigoKey::F16,
    EnigoKey::F17,
    EnigoKey::F18,
    EnigoKey::F19,
    EnigoKey::F20,
];

fn key(key: Key) -> EnigoKey {
    match key {
        Key::Char(c) => char_key(c),
        Key::Tab => EnigoKey::Tab,
        Key::Space => EnigoKey::Space,
        Key::Enter => EnigoKey::Return,
        Key::Escape => EnigoKey::Escape,
        Key::Backspace => EnigoKey::Backspace,
        Key::Delete => EnigoKey::Delete,
        Key::Up => EnigoKey::UpArrow,
        Key::Down => EnigoKey::DownArrow,
        Key::Left => EnigoKey::LeftArrow,
        Key::Right => EnigoKey::RightArrow,
        Key::Home => EnigoKey::Home,
        Key::End => EnigoKey::End,
        Key::PageUp => EnigoKey::PageUp,
        Key::PageDown => EnigoKey::PageDown,
        Key::F(n) => FUNCTION_KEYS[usize::from(n.clamp(1, MAX_FUNCTION_KEY)) - 1],
    }
}

/// Letters and digits go out as virtual-key codes on Windows so shortcuts like
/// ctrl+c register as shortcuts rather than typed text.
#[cfg(target_os = "windows")]
fn char_key(c: char) -> EnigoKey {
    if c.is_ascii_alphanumeric() {
        EnigoKey::Other(u32::from(c.to_ascii_uppercase()))
    } else {
        EnigoKey::Unicode(c)
    }
}

#[cfg(target_os = "macos")]
fn char_key(c: char) -> EnigoKey {
    EnigoKey::Unicode(c)
}

fn media(media_key: MediaKey) -> EnigoKey {
    match media_key {
        MediaKey::PlayPause => EnigoKey::MediaPlayPause,
        MediaKey::Next => EnigoKey::MediaNextTrack,
        MediaKey::Previous => EnigoKey::MediaPrevTrack,
        MediaKey::VolumeUp => EnigoKey::VolumeUp,
        MediaKey::VolumeDown => EnigoKey::VolumeDown,
        MediaKey::Mute => EnigoKey::VolumeMute,
    }
}

fn button(mouse_button: MouseButton) -> Button {
    match mouse_button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
        MouseButton::Back => Button::Back,
        MouseButton::Forward => Button::Forward,
    }
}
