#!/bin/sh
# shellcheck disable=SC2016 # the single-quoted strings are PowerShell or guest-shell code
# that must reach the VMs unexpanded.
# Retakes the README's screenshots from the latest release, in Ben's UTM VMs:
# docs/screenshots/quietmouse-cli.png, Terminal on the "macOS 27" VM in dark mode, and
# docs/screenshots/quietmouse-windows-config.png, Windows Terminal on "Windows 11".
# sign-macos-release.sh runs this after each release, and it can run by hand.
#
# Needs both VMs, UTM's utmctl, and agent-guides' tools mac.sh and win.sh (MAC_SH and
# WIN_SH override where they are). The macOS shot runs first, and that VM goes down before
# the Windows VM is used. The Mac takes quietmouse from the live Homebrew tap, through
# mac.sh's SHA-256 check. The Windows VM runs the release's own install.ps1, which checks
# SHA256SUMS. Windows Terminal must be installed in the Windows VM, as it is by default on
# Windows 11.
#
# Each VM is left as it was found: the Mac's appearance, desktop icons, and Terminal's
# marks go back, and so do the Windows scale and config. A VM this script started is shut
# down again, and one that was already running stays up. quietmouse stays installed in
# both VMs, as the install leaves it.
#
# Environment (all optional):
#   QM_EXPECT_VERSION=0.1.31  wait until the tap offers this version, and check the Windows
#                             install has it. sign-macos-release.sh sets this.
#   QM_MAC_DIR=/path          install the unpacked macOS folder (quietmouse and
#                             quietmouse.app) instead of the tap's release, to test a build
#                             that isn't released
#   QM_WIN_DIR=/path          after the Windows installer, copy quietmouse.exe and
#                             quietmoused.exe from this folder over the installed ones
#
# Writes the two PNGs into docs/screenshots and prints each file. It doesn't commit them.
set -eu

cd "$(dirname "$0")/.."
out=$(pwd)/docs/screenshots
MAC_SH=${MAC_SH:-$HOME/Dev/agent-guides/machines/bens-macbook-pro/tools/mac.sh}
WIN_SH=${WIN_SH:-$HOME/Dev/agent-guides/machines/bens-macbook-pro/tools/win.sh}
UTMCTL=/Applications/UTM.app/Contents/MacOS/utmctl
MAC_HOST=macos27
WIN_HOST=win11
TAP=benjweaver/homebrew-quietmouse
tmp=$(mktemp -d)

for tool in "$MAC_SH" "$WIN_SH" "$UTMCTL"; do
	[ -x "$tool" ] || { echo "screenshots.sh: missing $tool" >&2; exit 1; }
done

# State for the cleanup, which runs on any exit.
mac_on=0
win_on=0
win_started=0
win_orig_scale=''
win_hwnd=''

cleanup() {
	status=$?
	trap - EXIT
	if [ "$mac_on" = 1 ]; then mac_teardown; fi
	if [ "$win_on" = 1 ]; then win_teardown; fi
	rm -rf "$tmp"
	exit $status
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# Runs AppleScript in the guest. The script goes over stdin, so no quoting reaches the shell.
mac_osa() {
	printf '%s\n' "$1" > "$tmp/osa.scpt"
	"$MAC_SH" run 'cat > /tmp/qm-shot.scpt && osascript /tmp/qm-shot.scpt' < "$tmp/osa.scpt"
}

# Shows or hides Terminal's command marks: $1 is "Hide Marks" or "Show Marks".
terminal_marks() {
	mac_osa "tell application \"System Events\" to tell process \"Terminal\"
	set view_menu to menu 1 of menu bar item \"View\" of menu bar 1
	if exists menu item \"$1\" of view_menu then click menu item \"$1\" of view_menu
end tell"
}

# Types one line into the front Terminal window, then presses Return.
type_line() {
	"$MAC_SH" run "/usr/local/bin/gui type '$1'"
	sleep 1
	"$MAC_SH" key return
	sleep 1
}

# The tap's formula line: "URL SHA256 VERSION". With QM_EXPECT_VERSION set, waits up to
# 15 minutes for the tap's update to land, since tap.yml runs after the release is published.
latest_tap() {
	i=0
	while :; do
		info=$("$MAC_SH" formula "$TAP" quietmouse)
		version=$(echo "$info" | cut -d' ' -f3)
		if [ -z "${QM_EXPECT_VERSION:-}" ] || [ "$version" = "$QM_EXPECT_VERSION" ]; then
			echo "$info"
			return 0
		fi
		i=$((i + 1))
		if [ "$i" -ge 60 ]; then
			echo "screenshots.sh: the tap still offers $version, not $QM_EXPECT_VERSION" >&2
			return 1
		fi
		sleep 15
	done
}

mac_install() {
	if [ -n "${QM_MAC_DIR:-}" ]; then
		"$MAC_SH" run 'rm -rf /tmp/qm-stage'
		scp -rq "$QM_MAC_DIR" "$MAC_HOST:/tmp/qm-stage"
		dir=/tmp/qm-stage
	else
		info=$(latest_tap)
		version=$(echo "$info" | cut -d' ' -f3)
		url=$(echo "$info" | cut -d' ' -f1 | sed "s/#{version}/$version/g")
		sha=$(echo "$info" | cut -d' ' -f2)
		dir=$("$MAC_SH" install "$url" "$sha" | tail -n 1)
	fi
	# The same places the formula installs to, so quietmouse and quietmoused are on PATH.
	bin=/usr/local/opt/quietmouse/bin
	"$MAC_SH" run "sudo rm -rf $bin && sudo mkdir -p $bin && sudo ditto '$dir/quietmouse' $bin/quietmouse && sudo ditto '$dir/quietmouse.app' $bin/quietmouse.app && sudo ln -sf $bin/quietmouse /usr/local/bin/quietmouse && sudo ln -sf $bin/quietmouse.app/Contents/MacOS/quietmoused /usr/local/bin/quietmoused"
}

mac_shots() {
	mac_on=1
	"$MAC_SH" up
	mac_install
	"$MAC_SH" prep dark

	# A throwaway zsh config: a plain green prompt, and the folder in Terminal's title
	# (through OSC 7) instead of the user's name.
	cat > "$tmp/zshrc" <<'EOF'
PROMPT='%F{green}$%f '
qm_osc7() { printf "\e]7;file://%s%s\a" "$HOST" "${PWD// /%20}"; }
precmd_functions+=(qm_osc7)
EOF
	"$MAC_SH" run 'mkdir -p /tmp/qm-zdot && cat > /tmp/qm-zdot/.zshrc' < "$tmp/zshrc"
	"$MAC_SH" run '/usr/local/bin/quietmouse config --init >/dev/null 2>&1 || true'

	"$MAC_SH" run 'open -a Terminal "$HOME/Library/Application Support/quietmouse"'
	sleep 3
	"$MAC_SH" front Terminal
	terminal_marks 'Hide Marks'

	type_line 'export ZDOTDIR=/tmp/qm-zdot; exec zsh'
	type_line 'printf "\e[8;27;100t\e[3;40;101t"; clear'
	type_line 'quietmouse --version'
	type_line 'quietmouse --help'
	sleep 2

	id=$("$MAC_SH" windows Terminal | awk -F'\t' '$3 ~ /quietmouse/ { print $1; exit }')
	if [ -z "$id" ]; then
		id=$("$MAC_SH" windows Terminal | head -n 1 | cut -f1)
	fi
	if [ -z "$id" ]; then
		echo "screenshots.sh: no Terminal window in the macOS VM" >&2
		exit 1
	fi
	"$MAC_SH" shot "$tmp/quietmouse-cli.png" "$id" > /dev/null
	cp "$tmp/quietmouse-cli.png" "$out/quietmouse-cli.png"
	echo "wrote $out/quietmouse-cli.png"

	terminal_marks 'Show Marks'
	# Closing a window that runs a shell raises a sheet, so each close is followed by a
	# click on its Terminate button. A plain quit waits on that sheet, so quit only after.
	mac_osa 'tell application "System Events" to tell process "Terminal"
	repeat 5 times
		if (count of windows) is 0 then exit repeat
		click button 1 of window 1
		delay 1
		if exists sheet 1 of window 1 then click button "Terminate" of sheet 1 of window 1
		delay 1
	end repeat
end tell'
	"$MAC_SH" quit Terminal
	mac_teardown
}

mac_teardown() {
	mac_on=0
	terminal_marks 'Show Marks' > /dev/null 2>&1 || true
	"$MAC_SH" quit Terminal > /dev/null 2>&1 || true
	"$MAC_SH" restore || true
	"$MAC_SH" down || true
}

# Puts a local file into the Windows VM's %LOCALAPPDATA%, as base64, so no quoting survives.
win_put() {
	b=$(base64 < "$1" | tr -d '\n')
	"$WIN_SH" run "[IO.File]::WriteAllBytes((Join-Path \$env:LOCALAPPDATA '$(basename "$1")'), [Convert]::FromBase64String('$b'))" > /dev/null
}

# Runs one of the helper scripts put in %LOCALAPPDATA$: $1 is its name, $2 its arguments.
win_script() {
	"$WIN_SH" run "& (Join-Path \$env:LOCALAPPDATA '$1') $2" | tr -d '\r'
}

win_windows() {
	"$WIN_SH" windows WindowsTerminal | tr -d '\r'
}

win_teardown() {
	win_on=0
	if [ -n "$win_hwnd" ]; then
		win_script qm-place.ps1 "-Hwnd $win_hwnd -Close" > /dev/null 2>&1 || true
		win_hwnd=''
	fi
	win_script qm-config.ps1 '-Restore' > /dev/null 2>&1 || true
	if [ -n "$win_orig_scale" ]; then
		"$WIN_SH" scale "$win_orig_scale" > /dev/null 2>&1 || true
	fi
	"$WIN_SH" run 'Remove-Item (Join-Path $env:LOCALAPPDATA "qm-*.ps1") -Force -ErrorAction SilentlyContinue; Remove-Item (Join-Path $env:LOCALAPPDATA "qm-stage") -Recurse -Force -ErrorAction SilentlyContinue' > /dev/null 2>&1 || true
	if [ "$win_started" = 1 ]; then
		win_started=0
		ssh "$WIN_HOST" 'shutdown /s /t 0' > /dev/null 2>&1 || true
		for i in $(seq 36); do
			[ "$("$UTMCTL" status 'Windows 11')" = stopped ] && break
			sleep 5
		done
		[ "$("$UTMCTL" status 'Windows 11')" = stopped ] || "$UTMCTL" stop 'Windows 11' || true
	fi
}

# The helper scripts that run inside the Windows VM's terminal and desktop.
write_win_helpers() {
	cat > "$tmp/qm-shot.ps1" <<'EOF'
# The screenshot's terminal: a plain prompt, then the example config's MX Master profile.
function global:prompt { "PS> " }
$Host.UI.RawUI.WindowTitle = 'quietmouse'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location (Join-Path $env:LOCALAPPDATA 'quietmouse')
Clear-Host
Write-Host 'PS> ' -NoNewline
Write-Host 'Get-Content -Encoding UTF8 config.toml | Select -Skip 9 -First 31'
Get-Content -Encoding UTF8 config.toml | Select-Object -Skip 9 -First 31
EOF
	cat > "$tmp/qm-open.ps1" <<'EOF'
# Opens the screenshot's Windows Terminal window, 92 columns by 33 rows.
$shot = Join-Path $env:LOCALAPPDATA 'qm-shot.ps1'
Start-Process wt.exe -ArgumentList ('-w new --size 92,33 --title quietmouse --suppressApplicationTitle powershell.exe -NoLogo -NoExit -ExecutionPolicy Bypass -File "{0}"' -f $shot)
EOF
	cat > "$tmp/qm-config.ps1" <<'EOF'
# Swaps quietmouse's example config in for the shot, keeping the old one aside, or puts
# the old one back with -Restore.
param([switch]$Restore)
$ErrorActionPreference = 'Stop'
$cfg = Join-Path $env:LOCALAPPDATA 'quietmouse\config.toml'
$bak = "$cfg.screenshot-backup"
if ($Restore) {
    if (Test-Path $bak) { Move-Item $bak $cfg -Force }
    'restored'
    return
}
if (Test-Path $cfg) { Move-Item $cfg $bak -Force }
& (Join-Path $env:LOCALAPPDATA 'Programs\quietmouse\quietmouse.exe') config --init
EOF
	cat > "$tmp/qm-place.ps1" <<'EOF'
# Moves a window up so its frame sits above the taskbar, or closes it with -Close.
param([long]$Hwnd, [switch]$Close)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class QmPlace {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern IntPtr SetProcessDpiAwarenessContext(IntPtr c);
  [DllImport("user32.dll")] public static extern bool SystemParametersInfo(uint action, uint param, ref RECT rect, uint winIni);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT rect);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT rect, int size);
}
'@
$h = [IntPtr]$Hwnd
if ($Close) {
    # WM_CLOSE to this window alone, so nothing else in the session is affected.
    [QmPlace]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    'closed'
    return
}
# Per-monitor aware, as capture.ps1 is, so the work area and the frame are in physical pixels.
[QmPlace]::SetProcessDpiAwarenessContext([IntPtr]-4) | Out-Null
$work = New-Object QmPlace+RECT
[QmPlace]::SystemParametersInfo(0x0030, 0, [ref]$work, 0) | Out-Null
$frame = New-Object QmPlace+RECT
[QmPlace]::DwmGetWindowAttribute($h, 9, [ref]$frame, 16) | Out-Null
$over = $frame.B - $work.B
if ($over -le 0) { 'fits'; return }
$rect = New-Object QmPlace+RECT
[QmPlace]::GetWindowRect($h, [ref]$rect) | Out-Null
$top = [Math]::Max($work.T, $rect.T - $over)
# SWP_NOSIZE | SWP_NOZORDER
[QmPlace]::SetWindowPos($h, [IntPtr]::Zero, $rect.L, $top, 0, 0, 0x0005) | Out-Null
"moved up $over"
EOF
	for f in qm-shot.ps1 qm-open.ps1 qm-config.ps1 qm-place.ps1; do
		win_put "$tmp/$f"
	done
}

win_shots() {
	win_on=1
	if [ "$("$UTMCTL" status 'Windows 11')" != started ]; then
		"$UTMCTL" start 'Windows 11'
		win_started=1
	fi
	i=0
	until ssh "$WIN_HOST" 'echo up' > /dev/null 2>&1; do
		i=$((i + 1))
		if [ "$i" -ge 72 ]; then
			echo "screenshots.sh: no ssh to $WIN_HOST after six minutes" >&2
			exit 1
		fi
		sleep 5
	done

	# The release's own installer, which takes the latest release from GitHub.
	WIN_TIMEOUT=600 "$WIN_SH" run 'irm https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/windows/install.ps1 | iex'
	if [ -n "${QM_WIN_DIR:-}" ]; then
		"$WIN_SH" run 'New-Item -ItemType Directory -Force (Join-Path $env:LOCALAPPDATA "qm-stage") | Out-Null' > /dev/null
		scp -q "$QM_WIN_DIR/quietmouse.exe" "$QM_WIN_DIR/quietmoused.exe" "$WIN_HOST:AppData/Local/qm-stage/"
		"$WIN_SH" run '$p = Join-Path $env:LOCALAPPDATA "Programs\quietmouse"; & (Join-Path $p "quietmouse.exe") stop; Copy-Item (Join-Path $env:LOCALAPPDATA "qm-stage\*.exe") $p -Force; Remove-Item (Join-Path $env:LOCALAPPDATA "qm-stage") -Recurse -Force; & (Join-Path $p "quietmouse.exe") start' > /dev/null
	fi
	ver=$("$WIN_SH" run '& (Join-Path $env:LOCALAPPDATA "Programs\quietmouse\quietmouse.exe") --version' | tr -d '\r')
	echo "Windows VM: $ver"
	if [ -n "${QM_EXPECT_VERSION:-}" ] && [ -z "${QM_WIN_DIR:-}" ]; then
		case $ver in
		*" $QM_EXPECT_VERSION") ;;
		*) echo "screenshots.sh: Windows has '$ver', not $QM_EXPECT_VERSION" >&2; exit 1 ;;
		esac
	fi

	write_win_helpers
	win_orig_scale=$("$WIN_SH" scale | tr -d '\r' | sed -n 's/^scale \([0-9][0-9]*\)%.*/\1/p')
	"$WIN_SH" scale 150 > /dev/null
	win_script qm-config.ps1 '' > /dev/null
	win_script qm-open.ps1 '' > /dev/null
	sleep 6

	row=''
	for i in 1 2 3 4 5; do
		row=$(win_windows | awk -F'\t' '$4 == "quietmouse" { print; exit }')
		[ -n "$row" ] && break
		sleep 2
	done
	if [ -z "$row" ]; then
		echo "screenshots.sh: no Windows Terminal window titled quietmouse" >&2
		exit 1
	fi
	win_hwnd=$(echo "$row" | cut -f1)
	# Above the taskbar if the frame runs under it. Then the window is read again, since
	# it may have moved.
	if [ "$(win_script qm-place.ps1 "-Hwnd $win_hwnd")" != fits ]; then
		row=$(win_windows | awk -F'\t' -v h="$win_hwnd" '$1 == h { print; exit }')
	fi
	pos=$(echo "$row" | cut -f5)
	x=$(echo "$pos" | cut -d, -f1)
	y=$(echo "$pos" | cut -d, -f2)
	# Clicking the tab strip makes the window active, the way a person would.
	"$WIN_SH" click $((x + 120)) $((y + 18)) > /dev/null

	"$WIN_SH" shot "$tmp/quietmouse-windows-config.png" "$win_hwnd" > /dev/null
	cp "$tmp/quietmouse-windows-config.png" "$out/quietmouse-windows-config.png"
	echo "wrote $out/quietmouse-windows-config.png"

	win_script qm-place.ps1 "-Hwnd $win_hwnd -Close" > /dev/null
	win_hwnd=''
	win_teardown
}

if [ -n "${QM_MAC_DIR:-}" ]; then
	echo "screenshots.sh: installing the macOS folder $QM_MAC_DIR"
fi
mac_shots
win_shots
echo "screenshots.sh: done"
