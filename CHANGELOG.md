# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Each release's notes on GitHub come
from its section below.

## [Unreleased]

## [0.1.5] - 2026-09-15

### Added

- `straightness` for gesture buttons: how much further along one axis than the other a
  swipe must be before it counts, so a diagonal flick doesn't fire the wrong direction.
  A swipe too diagonal to place does nothing, and doesn't count as a tap either. The
  default, 1.0, keeps the old behaviour of taking whichever axis moved more.

### Fixed

- Quick back-to-back desktop swipes are no longer lost. macOS drops a Space switch
  requested while the previous one is still sliding, so quietmouse now keeps switches
  about half a second apart and queues any that come sooner.

## [0.1.4] - 2026-09-15

### Fixed

- The macOS universal binaries are now signed for both architectures. Before, the
  Intel half was unsigned, which can stop macOS from matching the Input Monitoring and
  Accessibility permissions you grant to the program that actually runs. The release
  build now fails if either architecture isn't validly signed.

## [0.1.3] - 2026-09-15

### Added

- Linux packages for Debian and Ubuntu (`.deb`), Fedora and openSUSE (`.rpm`), and
  Arch and CachyOS (`.pkg.tar.zst`). Each installs the udev rule and loads `uinput` at
  boot. Before publishing, every release installs them on Debian, Ubuntu, Fedora and
  Arch in CI.
- `install.sh` in the Linux tarball, for SteamOS, Bazzite, Fedora Silverblue and any
  other distribution. It installs to `~/.local/bin` and only needs sudo for the udev
  rule.
- On KDE Plasma, desktop actions invoke KWin's own shortcuts by name, so they follow
  your bindings. Other Linux desktops still get GNOME's default shortcuts.
- A Homebrew tap: `brew install benjweaver/quietmouse/quietmouse`.

### Fixed

- The Linux tarball puts the udev rule at `packaging/linux/`, where the README says it
  is.

## [0.1.2] - 2026-09-15

### Changed

- The example config now maps a left swipe to the desktop on the left, and a right
  swipe to the desktop on the right, instead of the trackpad-style reverse.
- The example config explains how to reverse the wheel and thumb wheel on the mouse
  itself, so on macOS the trackpad keeps natural scrolling while the mouse doesn't.

## [0.1.1] - 2026-09-15

### Added

- `quietmouse autostart on|off` on macOS, as a LaunchAgent in `~/Library/LaunchAgents`,
  and on Linux, as a systemd user service. Every platform can now start the agent at
  log-in without admin rights. On Windows, `autostart on` also starts the agent
  straight away.

### Changed

- The install instructions use a folder you own, such as `~/.local/bin`, instead of
  `sudo install`.
- The hand-written LaunchAgent and systemd unit are gone from `packaging/`. `autostart`
  now generates them, pointing at wherever the binaries actually are.

### Fixed

- An agent started while another is running now exits quietly, instead of failing and
  being restarted over and over by launchd or systemd.

## [0.1.0] - 2026-09-15

First release.

- HID++ 1.0/2.0 support for Unifying, Bolt and Lightspeed receivers, and for devices
  connected directly over Bluetooth or USB.
- Per-device profiles: DPI, SmartShift, scroll direction and resolution, and the thumb
  wheel.
- Button remapping and tap/swipe gestures, plus desktop actions (overview, app windows,
  show desktop, switch desktop) that use each OS's own mechanism.
- The `quietmouse` CLI and the windowless `quietmoused` agent. On Windows the agent
  runs per user without admin rights, and `autostart` registers it in the per-user Run
  key.
- No network access, enforced in CI.

[Unreleased]: https://github.com/benjweaver/quietmouse/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/benjweaver/quietmouse/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/benjweaver/quietmouse/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/benjweaver/quietmouse/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/benjweaver/quietmouse/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/benjweaver/quietmouse/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/benjweaver/quietmouse/releases/tag/v0.1.0
