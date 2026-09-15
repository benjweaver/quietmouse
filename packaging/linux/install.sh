#!/bin/sh
# Installs quietmouse for your user from this folder. It also works on read-only
# systems such as SteamOS, Bazzite and Fedora Silverblue. The programs go in
# ~/.local/bin. Only the udev rule, which lets your user open Logitech devices,
# needs sudo, and it goes in /etc, which system updates keep.
#
#   ./install.sh              install, then start quietmouse now and at log-in
#   ./install.sh --uninstall  remove it again (your config is kept)
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
bin_dir="$HOME/.local/bin"
rule=/etc/udev/rules.d/70-quietmouse.rules
modules=/etc/modules-load.d/quietmouse.conf

as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif command -v sudo > /dev/null 2>&1; then
        sudo "$@"
    else
        echo "sudo isn't available; run this as root: $*" >&2
        return 1
    fi
}

reload_udev() {
    as_root udevadm control --reload
    as_root udevadm trigger --subsystem-match=hidraw --subsystem-match=misc
}

if [ "${1:-}" = "--uninstall" ]; then
    if [ -x "$bin_dir/quietmouse" ]; then
        "$bin_dir/quietmouse" autostart off || true
    fi
    rm -f "$bin_dir/quietmouse" "$bin_dir/quietmoused"
    if [ -e "$rule" ] || [ -e "$modules" ]; then
        echo "Removing the udev rule (needs sudo)."
        as_root rm -f "$rule" "$modules"
        reload_udev
    fi
    echo "quietmouse is removed. Your config in ~/.config/quietmouse is still there."
    exit 0
fi

# Stop a running copy so the new one starts in its place.
if [ -x "$bin_dir/quietmouse" ]; then
    "$bin_dir/quietmouse" stop || true
fi
mkdir -p "$bin_dir"
install -m 755 "$here/quietmouse" "$here/quietmoused" "$bin_dir/"
echo "Installed quietmouse and quietmoused in $bin_dir"

if [ -e /usr/lib/udev/rules.d/70-quietmouse.rules ]; then
    echo "The udev rule is already installed by a package."
elif cmp -s "$here/packaging/linux/70-quietmouse.rules" "$rule" 2> /dev/null; then
    echo "The udev rule is already installed."
else
    echo "Installing the udev rule that lets your user open Logitech devices (needs sudo)."
    echo "On SteamOS, set a password first with 'passwd' if you haven't."
    as_root install -D -m 644 "$here/packaging/linux/70-quietmouse.rules" "$rule"
    as_root install -D -m 644 "$here/packaging/linux/modules-load.conf" "$modules"
    as_root modprobe uinput || true
    reload_udev
fi

config=$("$bin_dir/quietmouse" config 2> /dev/null)
if [ ! -e "$config" ]; then
    "$bin_dir/quietmouse" config --init
    echo "Edit $config to change settings."
fi
"$bin_dir/quietmouse" autostart on

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to your PATH to run quietmouse from a terminal." ;;
esac
