# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Each release's notes on GitHub come
from its section below.

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

[0.1.1]: https://github.com/benjweaver/quietmouse/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/benjweaver/quietmouse/releases/tag/v0.1.0
