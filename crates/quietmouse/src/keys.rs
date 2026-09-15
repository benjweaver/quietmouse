//! Platform-neutral key chords, media keys and mouse buttons, as written in the config.

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    /// Command on macOS, the Windows key on Windows, Super on Linux.
    Meta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A lowercase letter, a digit, or US-layout punctuation from [`PUNCTUATION`].
    Char(char),
    Tab,
    Space,
    Enter,
    Escape,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 to F20.
    F(u8),
}

pub const PUNCTUATION: &str = "-=[]\\;',./`";
pub const MAX_FUNCTION_KEY: u8 = 20;

/// Modifiers held while one key is pressed, e.g. `cmd+shift+4`. A chord of only
/// modifiers (`super`) taps them on their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Vec<Modifier>,
    pub key: Option<Key>,
}

impl FromStr for Chord {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let mut modifiers = Vec::new();
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            let name = part.to_ascii_lowercase();
            if let Some(modifier) = parse_modifier(&name) {
                if modifiers.contains(&modifier) {
                    return Err(format!("`{part}` appears twice in `{text}`"));
                }
                modifiers.push(modifier);
            } else if key.is_some() {
                return Err(format!("`{text}` has more than one non-modifier key"));
            } else {
                key = Some(parse_key(&name).ok_or_else(|| format!("unknown key `{part}` in `{text}`"))?);
            }
        }
        Ok(Chord { modifiers, key })
    }
}

impl<'de> Deserialize<'de> for Chord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = self.modifiers.iter().map(|m| m.to_string()).collect();
        parts.extend(self.key.map(|k| k.to_string()));
        f.write_str(&parts.join("+"))
    }
}

impl fmt::Display for Modifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Modifier::Ctrl => "ctrl",
            Modifier::Shift => "shift",
            Modifier::Alt => "alt",
            Modifier::Meta => "meta",
        })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "f{n}"),
            other => f.write_str(match other {
                Key::Tab => "tab",
                Key::Space => "space",
                Key::Enter => "enter",
                Key::Escape => "escape",
                Key::Backspace => "backspace",
                Key::Delete => "delete",
                Key::Up => "up",
                Key::Down => "down",
                Key::Left => "left",
                Key::Right => "right",
                Key::Home => "home",
                Key::End => "end",
                Key::PageUp => "pageup",
                Key::PageDown => "pagedown",
                Key::Char(_) | Key::F(_) => unreachable!("handled above"),
            }),
        }
    }
}

fn parse_modifier(name: &str) -> Option<Modifier> {
    match name {
        "ctrl" | "control" => Some(Modifier::Ctrl),
        "shift" => Some(Modifier::Shift),
        "alt" | "opt" | "option" => Some(Modifier::Alt),
        "cmd" | "command" | "super" | "win" | "windows" | "meta" => Some(Modifier::Meta),
        _ => None,
    }
}

fn parse_key(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return (c.is_ascii_alphanumeric() || PUNCTUATION.contains(c)).then_some(Key::Char(c));
    }
    if let Some(number) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=MAX_FUNCTION_KEY).contains(&number).then_some(Key::F(number));
    }
    Some(match name {
        "tab" => Key::Tab,
        "space" => Key::Space,
        "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" => Key::PageUp,
        "pagedown" | "page_down" => Key::PageDown,
        "minus" => Key::Char('-'),
        "equal" | "equals" => Key::Char('='),
        "comma" => Key::Char(','),
        "period" | "dot" => Key::Char('.'),
        "slash" => Key::Char('/'),
        "backslash" => Key::Char('\\'),
        "semicolon" => Key::Char(';'),
        "grave" | "backtick" => Key::Char('`'),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKey {
    PlayPause,
    Next,
    #[serde(alias = "prev")]
    Previous,
    VolumeUp,
    VolumeDown,
    Mute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Desktop actions that each platform performs its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    /// Mission Control, Task View, or the Activities overview.
    Overview,
    /// The current app's windows (App Exposé).
    AppWindows,
    /// Move windows aside to show the desktop.
    Show,
    /// Switch to the desktop (Space, workspace) on the left.
    Left,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        text.parse().unwrap()
    }

    #[test]
    fn parses_chords_in_any_case_and_order() {
        assert_eq!(
            chord("Cmd+Shift+4"),
            Chord {
                modifiers: vec![Modifier::Meta, Modifier::Shift],
                key: Some(Key::Char('4'))
            }
        );
        assert_eq!(
            chord("ctrl + Up"),
            Chord {
                modifiers: vec![Modifier::Ctrl],
                key: Some(Key::Up)
            }
        );
        assert_eq!(chord("tab+alt").modifiers, vec![Modifier::Alt]);
        assert_eq!(chord("f13").key, Some(Key::F(13)));
        assert_eq!(chord("ctrl+minus").key, Some(Key::Char('-')));
    }

    #[test]
    fn allows_modifier_only_chords() {
        assert_eq!(
            chord("super"),
            Chord {
                modifiers: vec![Modifier::Meta],
                key: None
            }
        );
    }

    #[test]
    fn rejects_bad_chords() {
        for bad in [
            "",
            "ctrl+",
            "ctrl+ctrl+a",
            "a+b",
            "hyper+x",
            "f0",
            "f21",
            "ctrl+é",
            "cmd+win+q",
        ] {
            assert!(bad.parse::<Chord>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn displays_normalised_form() {
        assert_eq!(chord("Command+Shift+PageDown").to_string(), "meta+shift+pagedown");
    }
}
