# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Each release's notes on GitHub come
from its section below.

## [Unreleased]

### Fixed

- A tap of the gesture button straight after moving the mouse no longer fires
  as a swipe. When the button goes down, the mouse hands over the movement its
  sensor gathered just before the press in the first report of that press. On
  an MX Master 3S that was 100 to 750 counts after moving over to click a
  window, against one or two per report while the button is held, which crossed
  the swipe threshold at once and swiped back the way the hand had come. With
  a single desktop, the misfired switch did nothing visible, so it looked like
  the first press after switching windows was simply ignored and only the
  second worked. The first report of each press is now set aside; a real swipe
  is a stream of reports, so it loses nothing. This comes from the mouse, not
  the operating system, so it applies on macOS and Linux as well.

## [0.1.12] - 2026-09-21

### Added

- A one-line install for Windows:
  `irm https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/windows/install.ps1 | iex`.
  It fetches the latest release for x64 or ARM64, checks it against
  `SHA256SUMS`, installs it in `%LOCALAPPDATA%\Programs\quietmouse` and on your
  PATH, writes an example config if there isn't one, and starts quietmouse now
  and at sign-in, all without admin rights. Until now Windows meant unzipping by
  hand and running `autostart on` yourself.
- The same for macOS and Linux, for anyone not using Homebrew or a
  distribution package:
  `curl -fsSL https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/install.sh | sh`.
  It checks the release against `SHA256SUMS` too. On macOS it installs into
  `~/.local/bin` and runs `autostart on`; on Linux it runs the tarball's own
  `install.sh`, udev rule included. Run from Git Bash on Windows, it hands over
  to the PowerShell installer.
- One-line updates, `update.ps1` and `update.sh` beside the installers.
  quietmouse never goes online, so nothing else tells you a new release is out
  or fetches it. Updating checks the installed version first and downloads
  nothing when it's already the latest, stops the running copy before replacing
  it, and leaves a copy from Homebrew or a Linux package alone, pointing you at
  `brew upgrade` or your package manager rather than installing a second one.
- One-line removal, `uninstall.ps1` and `uninstall.sh`, which undo everything
  the installers did except your config.
- The Windows executables have an icon and version details, so Task Manager,
  Explorer and a file's Properties show a quietmouse icon, its name, its version
  and Ben Weaver as the company, instead of a blank program icon and nothing
  else. The icon is a placeholder until there's a proper one. Task Manager's
  Publisher column stays empty: it only shows that for packaged apps, which is
  why it's also blank for Steam, Firefox and Windows' own security tray icon.

## [0.1.11] - 2026-09-21

### Changed

- The gesture deadzone now follows the device's pointer resolution instead of
  being a fixed number of sensor counts. Counts are DPI, so one number meant a
  different distance on every device and moved under you when the resolution
  did: the default 150 is about 4mm at 1000 dpi but 2.4mm at 1600, which is the
  difference between a deliberate flick and a twitch. Left unset, `threshold` is
  now scaled to whatever the pointer is set to, including by a button that
  changes DPI mid-session, so the flick stays the same length. Setting
  `threshold` yourself still means sensor counts exactly as written, and a device
  that won't report its resolution keeps the unscaled default.
- The folder inside the Windows zip is now always named `quietmouse`, rather
  than after the release, so upgrading by extracting over the previous copy puts
  the new binaries where the old ones were instead of beside them. A versioned
  folder moved `quietmoused.exe` on every upgrade, with the same effect on the
  Run key as the fix below. The archive still carries the version in its own
  name.

### Fixed

- The config no longer sits in a different directory from everything else on
  Windows. `dirs::config_dir` is the roaming profile there, so the config lived
  in `%APPDATA%\quietmouse` while the log and the lock were in
  `%LOCALAPPDATA%\quietmouse`, a directory the documentation also pointed at for
  everything else. Editing the file you'd reasonably expect to be the config
  changed nothing, silently, and the daemon carried on reading a file you hadn't
  touched. It now sits beside the log. macOS never showed this because both
  directories are `~/Library/Application Support` there; Linux keeps its own
  split, since XDG puts configuration in `~/.config` and people expect to find
  it there.
- Starting at sign-in no longer breaks on Windows when quietmouse is upgraded.
  `autostart on` records the full path to `quietmoused.exe` in the per-user Run
  key, and that path was resolved through any symlink or junction first.
  Resolving is there for macOS, which ties privacy permissions to the exact
  binary; Windows ties nothing to a binary, so all it did there was turn a
  stable shortcut into whatever it pointed at that day, leaving the Run key
  naming a version that a later upgrade had removed. It also produced an
  extended-length `\\?\C:\...` path, which not everything that reads the Run
  key copes with. Windows now keeps the path as it was reached.

## [0.1.10] - 2026-09-20

### Fixed

- A device that was merely asleep when quietmouse first reached it is no longer
  written off. Probing an endpoint that didn't answer was treated the same as
  finding nothing that speaks HID++ 2.0, and the endpoint was then skipped until
  it detached entirely. Waking a Mac is exactly the case that hits this: macOS
  re-enumerates a Bluetooth mouse the moment the link is back, a little before
  the mouse itself will answer, so the buttons stayed dead until the mouse next
  slept and woke. Silence and a reply we can't use are now told apart, and a
  silent endpoint is asked again rather than skipped.
- Working out what's on an endpoint no longer waits two seconds per attempt for
  a device that isn't awake. An awake device answers in milliseconds, so the
  daemon now allows 300ms and asks again rather than waiting out a timeout that
  only exists for slow first replies. One-shot commands (`list`, `info`,
  `events`) keep the full wait, having no second attempt to fall back on.
- A device that wouldn't configure is no longer given up on after five tries.
  Since only a receiver's connection notice could start it off again, a directly
  connected mouse that was slow to wake stayed unconfigured for as long as it
  remained attached. Retries now back off from 250ms to five seconds and carry
  on for as long as the device is there, so settings go back on within a few
  seconds of it waking.

### Changed

- Swipes take a firmer movement to fire. The default gesture `threshold` rises
  from 30 to 150 sensor counts — about 4mm at 1000 dpi, where 30 was under a
  millimetre and the nudge from pressing the button could reach it on its own.
- A swipe now has to clear the threshold in one unbroken movement: pausing for
  200ms mid-press forgets the travel so far. Holding the gesture button still
  for a tap used to accumulate drift until it crossed the threshold and fired a
  swipe instead.
- Swipes need less of a lean before they pick a direction: the default
  `straightness` drops from 2.0 to 1.5, after testing on an MX Master 3S. A
  sideways flick that arcs a little now fires as you commit to it, instead of
  waiting for one axis to get twice as far as the other. Set `straightness`
  yourself to keep the old behaviour.
- Desktop actions on macOS no longer read and parse
  `com.apple.symbolichotkeys.plist` on every press. It's kept between presses and
  re-read when its timestamp moves, so a rebound shortcut still takes effect
  without a restart, and a run of quick swipes doesn't queue behind the disk each
  time. The CoreGraphics event source is built once too, rather than twice per
  keystroke.

## [0.1.9] - 2026-09-15

### Added

- `THIRD-PARTY-NOTICES.md` in every release archive and package, listing the
  licences of the crates compiled into the binaries. The binaries are statically
  linked, and the MIT, BSD, MPL and Unicode licences involved all ask for their
  notices to travel with them; only quietmouse's own LICENSE was being shipped.
  The macOS binaries also link HIDAPI's C library, whose BSD notice is now
  reproduced.

## [0.1.8] - 2026-09-15

### Added

- `desktop_switch_gap_ms`, the gap left between desktop switches. Lower it until
  swipes start going missing.

### Changed

- Gestures fire sooner and pick directions more carefully, after testing on an
  MX Master 3S: swipes now fire after 30 counts of movement rather than 50, and must
  go twice as far one way as the other, where before whichever axis moved more won. A
  near-diagonal flick now does nothing instead of guessing. The gap between desktop
  switches drops from 450ms to 100ms.

## [0.1.7] - 2026-09-15

### Fixed

- Granting a permission on macOS now really does take effect on its own. macOS decides
  once per process whether it may read input devices and keeps to that answer, so
  retrying was never going to help; quietmouse now notices the permission arriving and
  restarts itself to use it.
- `autostart on` points at the exact versioned binary rather than a package manager's
  shortcut. macOS ties permissions to the exact binary, and a shortcut that survives
  upgrades left behind an entry that looked switched on but matched nothing: no prompt,
  no permission, and no explanation. Each version now asks properly. After
  `brew upgrade quietmouse`, run `quietmouse autostart on`.

## [0.1.6] - 2026-09-15

### Added

- On macOS, quietmouse now asks for Input Monitoring and Accessibility itself, so macOS
  prompts and lists it in both places ready to switch on, instead of leaving you to find
  the binary in Finder. This matters after every upgrade, since macOS ties the
  permissions to the exact binary.

### Changed

- `unsafe` is now denied rather than forbidden, with one exception:
  `crates/quietmouse/src/permissions.rs`, which declares the three macOS permission
  calls. They take and return plain integers, with no pointers and nothing to free.

### Fixed

- Granting a permission takes effect without restarting quietmouse. Devices that
  wouldn't open are retried on every scan rather than given up on, and keystroke output
  is retried too, with both complaining only once a minute instead of every attempt.

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

[Unreleased]: https://github.com/benjweaver/quietmouse/compare/v0.1.12...HEAD
[0.1.12]: https://github.com/benjweaver/quietmouse/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/benjweaver/quietmouse/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/benjweaver/quietmouse/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/benjweaver/quietmouse/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/benjweaver/quietmouse/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/benjweaver/quietmouse/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/benjweaver/quietmouse/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/benjweaver/quietmouse/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/benjweaver/quietmouse/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/benjweaver/quietmouse/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/benjweaver/quietmouse/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/benjweaver/quietmouse/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/benjweaver/quietmouse/releases/tag/v0.1.0
