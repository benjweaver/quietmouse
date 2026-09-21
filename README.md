# quietmouse

Offline, telemetry-free settings for Logitech mice and receivers: DPI, SmartShift,
scroll direction, the thumb wheel, button remapping and gestures. One small binary
for macOS, Linux and Windows. No account, no cloud, no updater, no network access
at all. It talks HID++ directly to the device, and nothing else.

> Not affiliated with Logitech. Quit Logi Options+, Solaar or logiops before running
> quietmouse; they configure the same device settings and will fight over them.

## What it does

- **Pointer:** set DPI, or cycle through a list of DPIs from a button.
- **Scroll wheel:** SmartShift mode (auto, always ratchet, always free-spin), its
  sensitivity, and ratchet torque on wheels that support it. Button toggle between
  ratchet and free-spin.
- **Scroll direction on the device:** reverse this mouse's scrolling without touching
  the trackpad (macOS ties the two together; this doesn't).
- **Thumb wheel:** leave it scrolling sideways, or bind each direction to actions.
- **Buttons:** remap gesture, mode shift, middle, back, forward, or any divertable
  control by ID, to key chords, media keys, clicks, shell commands, DPI changes, or
  switching Easy-Switch channel.
- **Gestures:** hold a button and swipe up, down, left or right, with a separate
  action for a plain tap. A swipe has to clear a deadzone in one unbroken
  movement, so pressing the button doesn't fire one by accident.
- **Receivers and Bluetooth:** Unifying, Bolt and Lightspeed receivers, plus direct
  Bluetooth and USB connections. Settings are re-applied when a device wakes or
  reconnects, and diverted buttons are handed back to the device when quietmouse exits.

## Install

On macOS, or on Linux if you use Homebrew:

```sh
brew install benjweaver/quietmouse/quietmouse
```

Otherwise, download from the [Releases page](https://github.com/benjweaver/quietmouse/releases).
Checksums are in `SHA256SUMS`.

- **Windows** (x64 or ARM64): unzip `quietmouse.exe` and `quietmoused.exe` into a folder
  you own, such as `%LOCALAPPDATA%\Programs\quietmouse`. No installer and no admin rights
  (see [Running at login](#running-at-login)). The files aren't code-signed, so
  SmartScreen may warn about an unrecognised app.
- **macOS** (one universal binary for Apple Silicon and Intel): unpack into a folder you
  own, such as `~/.local/bin`, then run `xattr -d com.apple.quarantine quietmouse quietmoused`.
  The binaries aren't signed or notarised, so Gatekeeper blocks them otherwise.
- **Linux** (x86_64): pick the file for your distribution. Each package installs the
  programs and the udev rule that lets your user open Logitech devices, and loads
  `uinput` at boot. The binaries are statically linked, so they don't depend on your
  distribution's libraries.

  | Distribution | File | Install with |
  |---|---|---|
  | Debian, Ubuntu, Mint, Pop!_OS | `.deb` | `sudo apt install ./quietmouse_*.deb` |
  | Fedora, Nobara | `.rpm` | `sudo dnf install ./quietmouse-*.rpm` |
  | openSUSE | `.rpm` | `sudo zypper install ./quietmouse-*.rpm` |
  | Arch, CachyOS, Manjaro, EndeavourOS | `.pkg.tar.zst` | `sudo pacman -U ./quietmouse-*.pkg.tar.zst` |
  | SteamOS, Bazzite, Fedora Silverblue, anything else | `.tar.gz` | unpack, then `./install.sh` |

  `install.sh` puts the programs in `~/.local/bin`, so it works on read-only systems
  too. Only the udev rule needs sudo, and it goes in `/etc`, which system updates keep.
  On SteamOS, set a password with `passwd` first. `install.sh` also starts quietmouse
  and sets it to start at log-in; `./install.sh --uninstall` removes it.

Or build from source with `cargo build --release`.

## Quick start

```sh
cargo build --release
./target/release/quietmouse list            # receivers, devices, battery
./target/release/quietmouse info            # settings, buttons and features of a device
./target/release/quietmouse config --init   # write an example config
./target/release/quietmouse run             # apply it and handle buttons and gestures
```

Other commands: `quietmouse dpi 1600`, `quietmouse events --divert gesture,back`
(prints what the device reports, which is useful for finding control IDs and tuning
gestures), and `quietmouse config --check`. Add `-v` for detail, or `-vv` to see
every HID++ report.

## Permissions

| OS | Needed |
|----|--------|
| macOS | **Input Monitoring**, to open the mouse, and **Accessibility**, to send keystrokes. Both are under System Settings → Privacy & Security. quietmouse asks for both when the agent starts, so macOS prompts and lists it there ready to switch on, and it restarts itself once you do. They're tied to the exact binary, so each version asks again: after `brew upgrade quietmouse`, run `quietmouse autostart on` and switch the new entries on when macOS asks. On your own Mac this is just the switches. If your account isn't an administrator, as on many managed work Macs, macOS needs an administrator or your organisation's device management to approve Accessibility, and by default Input Monitoring too. |
| Linux | The packages and `install.sh` install [`packaging/linux/70-quietmouse.rules`](packaging/linux/70-quietmouse.rules) for you; otherwise copy it into `/etc/udev/rules.d` once, with sudo. It gives the logged-in user the Logitech hidraw devices and `/dev/uinput`, which the kernel otherwise keeps for root. Nothing else needs root, and if your distribution already ships Logitech udev rules (for example with Solaar) you may not need this step at all. Keystrokes go through uinput, so they work on X11 and Wayland. |
| Windows | Nothing extra, and no admin rights. HID++ lives on a vendor-specific HID collection any user can open, and keystrokes go through `SendInput`. One Windows rule applies: a normal program can't send keys into windows running as administrator. |

## Config

`quietmouse config` prints the path: `~/Library/Application Support/quietmouse/config.toml`
on macOS, `~/.config/quietmouse/config.toml` on Linux,
`%LOCALAPPDATA%\quietmouse\config.toml` on Windows, beside the log. `config --init`
writes a commented example with your platform's shortcuts. A short version:

```toml
[[device]]
match = "MX Master"                  # part of the device name; "*" matches any
dpi = 1600
smartshift = { mode = "auto", threshold = 20, torque = 60 }
scroll = { invert = true }
thumbwheel = { left = { media = "volume_down" }, right = { media = "volume_up" } }

[device.buttons.gesture]             # hold and swipe; a press without a swipe is a tap
tap = "overview"                     # Mission Control / Task View / Activities
left = "desktop_left"                # the Space / virtual desktop / workspace on the left
right = "desktop_right"
threshold = 150                      # how far to move before a swipe counts, in sensor
                                     # counts at 1000 dpi (about 4mm). Leave it out to
                                     # follow the device's own resolution.
straightness = 1.5                   # how much further one way than the other it must be

[device.buttons.mode_shift]
press = "toggle_smartshift"

[device.buttons.back]
press = { cycle_dpi = [800, 1600, 3200] }
```

Actions: `{ keys = "cmd+shift+4" }`, `{ media = "play_pause" }`, `{ click = "middle" }`,
`{ shell = "…" }`, `{ dpi = 800 }`, `{ cycle_dpi = […] }`, `{ host = 2 }`,
`"toggle_smartshift"`, `"none"`, and the desktop actions `"overview"`, `"app_windows"`,
`"show_desktop"`, `"desktop_left"` and `"desktop_right"`.

Desktop actions use each system's own mechanism. On macOS they trigger the shortcut
registered in Keyboard Shortcuts, with the exact flags a real keyboard sends. If you
rebind the shortcut, quietmouse follows; if you turn it off, quietmouse says so. (macOS
has no public API to switch Spaces.) Windows uses Task View and the virtual-desktop
shortcuts. On KDE Plasma (SteamOS desktop mode, CachyOS, Bazzite and others) they
invoke KWin's own shortcuts by name, so your bindings are followed. Other Linux desktops
get GNOME's default shortcuts. On Hyprland or Sway, use shell actions, such as
`{ shell = "hyprctl dispatch workspace e-1" }`.

Anything you leave out stays as the device has it.
Mistakes such as unknown keys, buttons or fields, or out-of-range values are reported
with the line they're on.

## Running at login

Everything here is per user, with no admin rights, on every platform. Put `quietmouse`
and `quietmoused` (`.exe` on Windows) in a folder you own: `~/.local/bin` on macOS and
Linux, `%LOCALAPPDATA%\Programs\quietmouse` on Windows. Then:

```sh
quietmouse config --init    # write a config, then edit it
quietmouse autostart on     # start now, and whenever you log in
```

`quietmoused` is the background agent. It has no window, and it logs to `quietmouse.log`
in `~/Library/Application Support/quietmouse`, `~/.local/share/quietmouse` or
`%LOCALAPPDATA%\quietmouse`. `autostart on` registers it for your account only: a
LaunchAgent in `~/Library/LaunchAgents` on macOS, a systemd user service on Linux, or
the `HKCU\…\Run` key on Windows. `quietmouse autostart off` removes it.

`quietmouse start` and `quietmouse stop` start and stop the agent by hand. Only one
instance runs at a time. Stopping, whether by `quietmouse stop`, Ctrl+C or SIGTERM,
hands diverted buttons back to the device first. Quit Logi Options+ if it's installed.
If your organisation blocks unsigned programs, quietmouse won't try to get around
that; ask IT.

## How it works

```
crates/hidpp       HID++ 1.0/2.0 protocol. No OS dependencies; tested against scripted devices.
  report, session    framing, request/reply matching, notification backlog
  device, receiver   feature discovery, events; receiver registers and connection notices
  features/          reprogrammable controls, SmartShift, hi-res and thumb wheels, DPI, battery, Easy-Switch
crates/quietmouse  the app
  hid                hidapi discovery and transport (one reader thread per interface)
  daemon, worker     one worker thread per receiver or device; applies profiles and re-applies on wake
  service            single instance, stop requests, agent log, per-user start at log-in
  bin/quietmoused    the windowless background agent
  gesture            swipe detection (deadzone, direction, drift) and thumb wheel steps
  inject             keystrokes: enigo on macOS and Windows, uinput on Linux
  config, keys       TOML schema, validation, key chords
```

Button remapping uses the device's own *diversion*: the device reports a diverted
control to software instead of acting on it, and for gestures it also reports pointer
movement while the button is held. Diversion is volatile, so quietmouse re-applies it
whenever a receiver says a device came online, or a device reports that it reconnected.

Movement arrives in sensor counts, which are DPI, so the same `threshold` is a
different distance on every device and moves under you when the resolution does:
150 counts is about 4mm at 1000 dpi but 2.4mm at 1600. Left unset, the deadzone
follows whatever the pointer is actually set to, including a button that changes
it, so the flick stays the same length. Set `threshold` yourself and it's taken
as counts exactly as written.

A swipe is one unbroken movement past `threshold`, and it fires the moment it gets
there rather than on release, so the action lands while you're still moving. Two
things stop that firing by accident: the deadzone itself, which the nudge from
pressing the button doesn't reach, and a 200ms pause resetting the travel, so
holding the button still can't slowly drift into a swipe. `straightness` decides
how much further one axis has to go than the other before a direction counts; a
flick that stays too diagonal does nothing rather than guessing.

Asleep and broken look the same over HID++ — both just stop answering — so
quietmouse never writes a device off. A device that doesn't answer is asked again
on a backoff that tops out at five seconds, for as long as it stays attached, and
its settings go back on within that of it waking up.

### Offline, and staying that way

No networking code is in the tree. CI fails if a network-capable crate (HTTP clients,
TLS, async runtimes, telemetry SDKs) shows up in the dependency tree on any platform;
see [`scripts/check-offline.sh`](scripts/check-offline.sh). Every compiler and clippy
warning is an error, and `unsafe` is denied everywhere except one file,
[`permissions.rs`](crates/quietmouse/src/permissions.rs), which declares the three
macOS calls that ask for Input Monitoring and Accessibility. They take and return plain
integers, with no pointers and nothing to free.

## Status

Early. Works today: the protocol layer, config, gestures and the CLI, all
unit-tested, with CI on macOS, Linux and Windows. Tested on hardware so far: an
MX Master 3S over Bluetooth on macOS. That covers reading settings, remapped buttons,
tap and swipe gestures, and handing the buttons back on exit. Receivers, and running
on Linux and Windows, still need testing on real devices. Planned: per-application profiles,
a tray/menu-bar settings app, and native hotplug notifications instead of a two-second
rescan.

## Credits

[Solaar](https://github.com/pwr-Solaar/Solaar) and
[logiops](https://github.com/PixlOne/logiops) have documented Logitech's HID++
behaviour over the years, and that work is what made this possible. quietmouse
is an independent implementation and takes no code from either project, which is
why it can be MIT-licensed while both of them are GPL.

## License

MIT. See [LICENSE](LICENSE).

The binaries are statically linked, so they carry code from the crates
quietmouse builds on. Every release archive and package ships
`THIRD-PARTY-NOTICES.md` with those crates' licences, generated by
[`scripts/third-party-notices.sh`](scripts/third-party-notices.sh) from the
crates actually compiled for that platform. Where a crate offers a choice of
licences, MIT is elected. The macOS binaries also include HIDAPI's C library,
used under its BSD-style licence.
