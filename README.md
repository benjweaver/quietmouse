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
  action for a plain tap.
- **Receivers and Bluetooth:** Unifying, Bolt and Lightspeed receivers, plus direct
  Bluetooth and USB connections. Settings are re-applied when a device wakes or
  reconnects, and diverted buttons are handed back to the device when quietmouse exits.

## Install

Download an archive from the [Releases page](https://github.com/benjweaver/quietmouse/releases).
Checksums are in `SHA256SUMS`.

- **Windows** (x64 or ARM64): unzip `quietmouse.exe` and `quietmoused.exe` into a folder
  you own, such as `%LOCALAPPDATA%\Programs\quietmouse`. No installer and no admin rights
  (see [Running at login](#running-at-login)). The files aren't code-signed, so
  SmartScreen may warn about an unrecognised app.
- **macOS** (one universal binary for Apple Silicon and Intel): unpack, then run
  `xattr -d com.apple.quarantine quietmouse quietmoused`. The binaries aren't signed or
  notarised, so Gatekeeper blocks them otherwise.
- **Linux** (x86_64, statically linked): unpack, and install
  `packaging/linux/70-quietmouse.rules`.

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
| macOS | **Input Monitoring**, to open the mouse, and **Accessibility**, to send keystrokes. Both are under System Settings → Privacy & Security. Grant them to the app that runs quietmouse: your terminal while testing, `/usr/local/bin/quietmouse` when it runs at login. An unsigned binary needs granting again after each rebuild. |
| Linux | Install [`packaging/linux/70-quietmouse.rules`](packaging/linux/70-quietmouse.rules). It gives the logged-in user access to Logitech hidraw devices and `/dev/uinput`. Keystrokes go through uinput, so they work on X11 and Wayland. |
| Windows | Nothing extra, and no admin rights. HID++ lives on a vendor-specific HID collection any user can open, and keystrokes go through `SendInput`. One Windows rule applies: a normal program can't send keys into windows running as administrator. |

## Config

`quietmouse config` prints the path: `~/Library/Application Support/quietmouse/config.toml`
on macOS, `~/.config/quietmouse/config.toml` on Linux, `%APPDATA%\quietmouse\config.toml`
on Windows. `config --init` writes a commented example with your platform's
shortcuts. A short version:

```toml
[[device]]
match = "MX Master"                  # part of the device name; "*" matches any
dpi = 1600
smartshift = { mode = "auto", threshold = 20, torque = 60 }
scroll = { invert = true }
thumbwheel = { left = { media = "volume_down" }, right = { media = "volume_up" } }

[device.buttons.gesture]             # hold and swipe; a press without a swipe is a tap
tap = "overview"                     # Mission Control / Task View / Activities
left = "desktop_right"               # next Space / virtual desktop / workspace
right = "desktop_left"
threshold = 50                       # swipe distance in sensor counts

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
shortcuts; Linux uses GNOME's defaults, and a `keys` action covers other desktops.

Anything you leave out stays as the device has it.
Mistakes such as unknown keys, buttons or fields, or out-of-range values are reported
with the line they're on.

## Running at login

- **macOS:** [`packaging/macos/local.quietmouse.plist`](packaging/macos/local.quietmouse.plist) (LaunchAgent).
- **Linux:** [`packaging/linux/quietmouse.service`](packaging/linux/quietmouse.service) (systemd user service).
- **Windows**, per user and without admin rights: put `quietmouse.exe` and
  `quietmoused.exe` in a folder you own, such as `%LOCALAPPDATA%\Programs\quietmouse`, then:

  ```bat
  quietmouse config --init
  quietmouse autostart on
  quietmouse start
  ```

  `quietmoused.exe` is the background agent. It has no window and logs to
  `%LOCALAPPDATA%\quietmouse\quietmouse.log`. `quietmouse autostart on` registers it in
  your own `HKCU\…\Run` key; `quietmouse autostart off` removes it. Quit Logi Options+
  if it's installed. If your organisation blocks unsigned programs, quietmouse won't
  try to get around that; ask IT.

`quietmouse start` and `quietmouse stop` work on every platform. Only one instance runs
at a time. Stopping, whether by `quietmouse stop`, Ctrl+C or SIGTERM, hands diverted
buttons back to the device first.

## How it works

```
crates/hidpp       HID++ 1.0/2.0 protocol. No OS dependencies; tested against scripted devices.
  report, session    framing, request/reply matching, notification backlog
  device, receiver   feature discovery, events; receiver registers and connection notices
  features/          reprogrammable controls, SmartShift, hi-res and thumb wheels, DPI, battery, Easy-Switch
crates/quietmouse  the app
  hid                hidapi discovery and transport (one reader thread per interface)
  daemon, worker     one worker thread per receiver or device; applies profiles and re-applies on wake
  service            single instance, stop requests, agent log, Windows sign-in start
  bin/quietmoused    the windowless background agent
  gesture            swipe detection and thumb wheel steps
  inject             keystrokes: enigo on macOS and Windows, uinput on Linux
  config, keys       TOML schema, validation, key chords
```

Button remapping uses the device's own *diversion*: the device reports a diverted
control to software instead of acting on it, and for gestures it also reports pointer
movement while the button is held. Diversion is volatile, so quietmouse re-applies it
whenever a receiver says a device came online, or a device reports that it reconnected.

### Offline, and staying that way

No networking code is in the tree. CI fails if a network-capable crate (HTTP clients,
TLS, async runtimes, telemetry SDKs) shows up in the dependency tree on any platform;
see [`scripts/check-offline.sh`](scripts/check-offline.sh). The workspace forbids
`unsafe` and treats every compiler and clippy warning as an error.

## Status

Early. Works today: the protocol layer, config, gestures and the CLI, all
unit-tested, with CI on macOS, Linux and Windows. Tested on hardware so far: an
MX Master 3S over Bluetooth on macOS. That covers reading settings, remapped buttons,
tap and swipe gestures, and handing the buttons back on exit. Receivers, and running
on Linux and Windows, still need testing on real devices. Planned: per-application profiles,
a tray/menu-bar settings app, and native hotplug notifications instead of a two-second
rescan.

## Credits

HID++ behaviour follows what [Solaar](https://github.com/pwr-Solaar/Solaar) and
[logiops](https://github.com/PixlOne/logiops) have documented over the years.

## License

MIT. See [LICENSE](LICENSE).
