//! The TOML config: per-device profiles with settings and button bindings.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use hidpp::features::reprog::cid;
use serde::Deserialize;

use crate::gesture::Direction;
use crate::keys::{Chord, MediaKey, MouseButton};

/// Movement, in sensor counts, before a held gesture button counts as a swipe.
pub const DEFAULT_GESTURE_THRESHOLD: u16 = 50;
/// Highest Easy-Switch channel a device can have.
const MAX_HOST: u8 = 6;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, rename = "device")]
    pub devices: Vec<Profile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Case-insensitive part of the device name; `*` matches any device.
    #[serde(rename = "match")]
    pub name_match: String,
    pub dpi: Option<u16>,
    pub smartshift: Option<SmartShiftConfig>,
    pub scroll: Option<ScrollConfig>,
    pub thumbwheel: Option<ThumbWheelConfig>,
    #[serde(default)]
    pub buttons: BTreeMap<ButtonId, ButtonConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShiftMode {
    /// Ratchet, switching to free-spin when the wheel is flicked.
    #[serde(alias = "smartshift")]
    Auto,
    /// Always ratchet.
    Ratchet,
    /// Always free-spin.
    Freespin,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmartShiftConfig {
    pub mode: Option<ShiftMode>,
    /// How fast the wheel must spin to release the ratchet in `auto` mode (1–254).
    pub threshold: Option<u8>,
    /// Ratchet force in percent (1–100), on wheels with tunable torque.
    pub torque: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScrollConfig {
    /// Reverse this mouse's scrolling on the device, leaving trackpads alone.
    pub invert: Option<bool>,
    /// High-resolution wheel reports.
    pub hires: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThumbWheelConfig {
    pub invert: Option<bool>,
    pub left: Option<Action>,
    pub right: Option<Action>,
    /// Rotation per action; defaults to one native scroll step.
    pub step: Option<u16>,
}

impl ThumbWheelConfig {
    /// Actions are bound, so the wheel must report to us instead of scrolling.
    pub fn diverted(&self) -> bool {
        self.left.is_some() || self.right.is_some()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonConfig {
    /// Fires when pressed.
    pub press: Option<Action>,
    /// Fires on release when the button was held without swiping.
    pub tap: Option<Action>,
    pub up: Option<Action>,
    pub down: Option<Action>,
    pub left: Option<Action>,
    pub right: Option<Action>,
    /// Swipe distance in sensor counts.
    pub threshold: Option<u16>,
}

impl ButtonConfig {
    /// Uses tap/swipe semantics rather than firing on press.
    pub fn is_gesture(&self) -> bool {
        self.tap.is_some() || self.has_swipes()
    }

    /// Needs pointer movement reported while held.
    pub fn has_swipes(&self) -> bool {
        [&self.up, &self.down, &self.left, &self.right]
            .iter()
            .any(|a| a.is_some())
    }

    pub fn swipe(&self, direction: Direction) -> Option<&Action> {
        match direction {
            Direction::Up => self.up.as_ref(),
            Direction::Down => self.down.as_ref(),
            Direction::Left => self.left.as_ref(),
            Direction::Right => self.right.as_ref(),
        }
    }

    fn actions(&self) -> impl Iterator<Item = &Action> {
        [&self.press, &self.tap, &self.up, &self.down, &self.left, &self.right]
            .into_iter()
            .flatten()
    }

    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.actions().next().is_some(), "no action set");
        ensure!(
            self.press.is_none() || !self.is_gesture(),
            "`press` can't be combined with `tap`/`up`/`down`/`left`/`right`; put the no-swipe action in `tap`"
        );
        ensure!(self.threshold != Some(0), "`threshold` must be at least 1");
        self.actions().try_for_each(Action::validate)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// A key chord such as `cmd+shift+4`.
    Keys(Chord),
    Media(MediaKey),
    Click(MouseButton),
    /// A command run by the system shell.
    Shell(String),
    Dpi(u16),
    CycleDpi(Vec<u16>),
    ToggleSmartshift,
    /// Easy-Switch channel, counting from 1.
    Host(u8),
    /// Mission Control, Task View, or the Activities overview.
    #[serde(alias = "mission_control")]
    Overview,
    /// The current app's windows (App Exposé).
    #[serde(alias = "app_expose")]
    AppWindows,
    ShowDesktop,
    /// Switch to the desktop (Space, workspace) on the left.
    #[serde(alias = "space_left")]
    DesktopLeft,
    #[serde(alias = "space_right")]
    DesktopRight,
    /// Swallow the input.
    #[serde(rename = "none")]
    Ignore,
}

impl Action {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Action::Dpi(0) => bail!("`dpi` must be above 0"),
            Action::CycleDpi(values) if values.is_empty() || values.contains(&0) => {
                bail!("`cycle_dpi` needs one or more values above 0")
            }
            Action::Host(host) if !(1..=MAX_HOST).contains(host) => bail!("`host` counts from 1 to {MAX_HOST}"),
            Action::Shell(command) if command.trim().is_empty() => bail!("`shell` command is empty"),
            _ => Ok(()),
        }
    }
}

/// A control, by friendly name or control id (CID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ButtonId(pub u16);

const BUTTON_NAMES: &[(&str, u16)] = &[
    ("gesture", cid::GESTURE),
    ("mode_shift", cid::MODE_SHIFT),
    ("middle", cid::MIDDLE),
    ("back", cid::BACK),
    ("forward", cid::FORWARD),
];

impl ButtonId {
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        if let Some(&(_, cid)) = BUTTON_NAMES.iter().find(|&&(n, _)| n == name) {
            return Some(Self(cid));
        }
        let cid = match name.strip_prefix("0x") {
            Some(hex) => u16::from_str_radix(hex, 16).ok()?,
            None => name.parse().ok()?,
        };
        Some(Self(cid))
    }
}

impl fmt::Display for ButtonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match BUTTON_NAMES.iter().find(|&&(_, cid)| cid == self.0) {
            Some((name, _)) => f.write_str(name),
            None => write!(f, "{:#06x}", self.0),
        }
    }
}

impl<'de> Deserialize<'de> for ButtonId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        ButtonId::parse(&name).ok_or_else(|| {
            let names: Vec<&str> = BUTTON_NAMES.iter().map(|&(n, _)| n).collect();
            serde::de::Error::custom(format!(
                "unknown button `{name}`; use {} or a control id such as 0x00c3 (see `quietmouse info`)",
                names.join(", ")
            ))
        })
    }
}

impl Config {
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("quietmouse").join("config.toml"))
    }

    /// `explicit`, or the platform's default location.
    pub fn resolve_path(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
        explicit
            .or_else(Self::default_path)
            .context("this system has no config directory; pass --config")
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid config {}", path.display()))
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let config: Config = toml::from_str(text)?;
        for profile in &config.devices {
            profile
                .validate()
                .with_context(|| format!("in the [[device]] matching {:?}", profile.name_match))?;
        }
        Ok(config)
    }

    /// The first profile whose `match` fits `device_name`.
    pub fn profile_for(&self, device_name: &str) -> Option<&Profile> {
        self.devices.iter().find(|profile| profile.matches(device_name))
    }
}

impl Profile {
    pub fn matches(&self, device_name: &str) -> bool {
        self.name_match == "*" || device_name.to_lowercase().contains(&self.name_match.to_lowercase())
    }

    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.name_match.trim().is_empty(),
            "`match` is empty; use \"*\" to match any device"
        );
        ensure!(self.dpi != Some(0), "`dpi` must be above 0");
        if let Some(shift) = &self.smartshift {
            if let Some(threshold) = shift.threshold {
                ensure!((1..=254).contains(&threshold), "smartshift `threshold` must be 1–254");
                ensure!(
                    matches!(shift.mode, None | Some(ShiftMode::Auto)),
                    "smartshift `threshold` only applies to mode = \"auto\""
                );
            }
            if let Some(torque) = shift.torque {
                ensure!((1..=100).contains(&torque), "smartshift `torque` must be 1–100");
            }
        }
        if let Some(thumb) = &self.thumbwheel {
            ensure!(thumb.step != Some(0), "thumbwheel `step` must be at least 1");
            [&thumb.left, &thumb.right]
                .into_iter()
                .flatten()
                .try_for_each(Action::validate)?;
        }
        for (id, button) in &self.buttons {
            button.validate().with_context(|| format!("in buttons.{id}"))?;
        }
        Ok(())
    }
}

const EXAMPLE: &str = r#"# quietmouse config. Restart `quietmouse run` after editing.
# Every setting is optional: anything left out stays as the device has it.
# `quietmouse info` shows what your device supports.

[[device]]
match = "MX Master"          # part of the device name, any case ("*" = any device)
dpi = 1600

# Scroll wheel clutch. "auto" ratchets until you flick the wheel; "ratchet" and
# "freespin" lock it. threshold = 1–254: higher needs a harder flick to free-spin.
smartshift = { mode = "auto" }

# invert = true reverses the wheel inside the mouse. On macOS that means the trackpad
# keeps natural scrolling while this mouse scrolls the traditional way.
scroll = { invert = false }

# Thumb wheel: leave unset for normal sideways scrolling, reverse it like the wheel
# above, or bind actions to it.
# thumbwheel = { invert = true }
# thumbwheel = { left = { keys = "ctrl+shift+tab" }, right = { keys = "ctrl+tab" } }

# Hold the thumb button and swipe. Pressing without swiping is a tap.
# Desktop actions use each system's own shortcuts: Mission Control and Spaces on
# macOS, Task View and virtual desktops on Windows, GNOME's defaults on Linux.
[device.buttons.gesture]
tap = "overview"
up = "overview"
down = "app_windows"
left = "desktop_left"        # swipe left to go to the desktop on the left
right = "desktop_right"

# The button behind the wheel.
[device.buttons.mode_shift]
press = "toggle_smartshift"

# Buttons: gesture, mode_shift, middle, back, forward, or a control id like 0x00c3.
# Actions:
#   { keys = "cmd+shift+4" }         modifiers: ctrl shift alt/option cmd/win/super
#   { media = "play_pause" }         play_pause next previous volume_up volume_down mute
#   { click = "middle" }             left right middle back forward
#   { shell = "open -a Calculator" }
#   { dpi = 800 }  or  { cycle_dpi = [800, 1600, 3200] }
#   { host = 2 }                     switch Easy-Switch channel
#   "overview"  "app_windows"  "show_desktop"  "desktop_left"  "desktop_right"
#   "toggle_smartshift"  "none"
"#;

/// A commented example config.
pub fn example() -> &'static str {
    EXAMPLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_is_valid() {
        let config = Config::parse(example()).unwrap();
        let profile = config.profile_for("MX Master 3S").unwrap();
        assert_eq!(profile.dpi, Some(1600));
        let gesture = &profile.buttons[&ButtonId(cid::GESTURE)];
        assert!(gesture.has_swipes());
        assert_eq!(gesture.left, Some(Action::DesktopLeft));
        assert_eq!(gesture.right, Some(Action::DesktopRight));
        assert_eq!(
            profile.buttons[&ButtonId(cid::MODE_SHIFT)].press,
            Some(Action::ToggleSmartshift)
        );
    }

    #[test]
    fn desktop_actions_accept_platform_names() {
        let config = Config::parse(
            r#"
            [[device]]
            match = "*"
            [device.buttons.gesture]
            tap = "mission_control"
            up = "app_expose"
            down = "show_desktop"
            left = "space_left"
            right = "desktop_right"
            "#,
        )
        .unwrap();
        let gesture = &config.profile_for("any").unwrap().buttons[&ButtonId(cid::GESTURE)];
        assert_eq!(gesture.tap, Some(Action::Overview));
        assert_eq!(gesture.up, Some(Action::AppWindows));
        assert_eq!(gesture.down, Some(Action::ShowDesktop));
        assert_eq!(gesture.left, Some(Action::DesktopLeft));
        assert_eq!(gesture.right, Some(Action::DesktopRight));
    }

    #[test]
    fn parses_every_action_form() {
        let config = Config::parse(
            r#"
            [[device]]
            match = "*"
            thumbwheel = { left = { media = "volume_down" }, right = { media = "volume_up" }, step = 3 }
            [device.buttons.back]
            press = { cycle_dpi = [800, 1600] }
            [device.buttons.forward]
            press = "none"
            [device.buttons.middle]
            press = { click = "middle" }
            [device.buttons.0x00c4]
            press = { host = 2 }
            "#,
        )
        .unwrap();
        let profile = config.profile_for("anything").unwrap();
        assert!(profile.thumbwheel.as_ref().unwrap().diverted());
        assert_eq!(
            profile.buttons[&ButtonId(cid::BACK)].press,
            Some(Action::CycleDpi(vec![800, 1600]))
        );
        assert_eq!(profile.buttons[&ButtonId(cid::FORWARD)].press, Some(Action::Ignore));
        assert_eq!(profile.buttons[&ButtonId(cid::MODE_SHIFT)].press, Some(Action::Host(2)));
    }

    #[test]
    fn rejects_mistakes_with_useful_messages() {
        let cases = [
            (
                "[[device]]\nmatch = \"x\"\n[device.buttons.gesture]\npress = \"none\"\nup = \"none\"",
                "put the no-swipe action in `tap`",
            ),
            (
                "[[device]]\nmatch = \"x\"\n[device.buttons.thumbs]\npress = \"none\"",
                "unknown button `thumbs`",
            ),
            (
                "[[device]]\nmatch = \"x\"\n[device.buttons.back]\npress = { keys = \"ctrl+nope\" }",
                "unknown key `nope`",
            ),
            (
                "[[device]]\nmatch = \"x\"\nsmartshift = { mode = \"ratchet\", threshold = 5 }",
                "only applies to mode",
            ),
            (
                "[[device]]\nmatch = \"x\"\n[device.buttons.back]\npress = { host = 9 }",
                "counts from 1",
            ),
            ("[[device]]\nmatch = \"x\"\ndpii = 800", "unknown field"),
        ];
        for (text, expected) in cases {
            let error = format!("{:#}", Config::parse(text).unwrap_err());
            assert!(error.contains(expected), "{error:?} should mention {expected:?}");
        }
    }

    #[test]
    fn matches_names_case_insensitively_in_order() {
        let config =
            Config::parse("[[device]]\nmatch = \"master 3s\"\ndpi = 800\n[[device]]\nmatch = \"*\"\ndpi = 400")
                .unwrap();
        assert_eq!(config.profile_for("MX Master 3S").unwrap().dpi, Some(800));
        assert_eq!(config.profile_for("MX Anywhere 3").unwrap().dpi, Some(400));
    }

    #[test]
    fn button_ids_accept_names_and_numbers() {
        assert_eq!(ButtonId::parse("Gesture"), Some(ButtonId(cid::GESTURE)));
        assert_eq!(ButtonId::parse("0x00C3"), Some(ButtonId(cid::GESTURE)));
        assert_eq!(ButtonId::parse("195"), Some(ButtonId(cid::GESTURE)));
        assert_eq!(ButtonId::parse("wat"), None);
        assert_eq!(ButtonId(0x00D7).to_string(), "0x00d7");
    }
}
