#!/bin/sh
# Installs the latest quietmouse release for your user on macOS or Linux, and on
# Windows from Git Bash, where it hands over to the PowerShell installer.
#
#   curl -fsSL https://raw.githubusercontent.com/benjweaver/quietmouse/main/packaging/install.sh | sh
#
# update.sh and uninstall.sh beside it run this with --update or --uninstall.
# Updating does nothing but make sure quietmouse is running when the latest
# release is already installed, and won't touch a copy that Homebrew or a
# distribution package put there. Uninstalling keeps your config.
#
# On macOS the programs go in ~/.local/bin and start now and at log-in. On Linux
# this runs the install.sh from the release tarball, which also installs the udev
# rule that lets your user open Logitech devices, and asks for sudo to do it.
set -eu

repo=benjweaver/quietmouse
raw=https://raw.githubusercontent.com/$repo/main/packaging
bin_dir=$HOME/.local/bin

fail() {
    echo "$*" >&2
    exit 1
}

sha256() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

uninstall_macos() {
    if [ -x "$bin_dir/quietmouse" ]; then
        "$bin_dir/quietmouse" autostart off || true
        "$bin_dir/quietmouse" stop || true
    fi
    rm -f "$bin_dir/quietmouse" "$bin_dir/quietmoused"
    rm -rf "$bin_dir/quietmoused.app"
    echo "quietmouse is removed. Your config in ~/Library/Application Support/quietmouse is still there."
}

install_macos() {
    dir=$1
    other=$(command -v quietmouse 2> /dev/null || true)
    if [ -n "$other" ] && [ "$other" != "$bin_dir/quietmouse" ]; then
        echo "Another quietmouse is installed at $other (Homebrew?). The copy in $bin_dir"
        echo "is the one that will start at log-in; remove the other to avoid confusion."
    fi
    # Stop a running copy, wherever it came from, so the new one starts in its place.
    "$dir/quietmouse" stop > /dev/null 2>&1 || true
    mkdir -p "$bin_dir"
    install -m 755 "$dir/quietmouse" "$bin_dir/"
    # quietmoused's real binary lives inside a .app bundle, so Privacy & Security has a
    # real icon for it under Accessibility and Input Monitoring; quietmoused itself is
    # a symlink into it. ditto, not cp, so the bundle's code signature and resource
    # fork survive the copy intact; cp -P for the symlink so it's copied as a link,
    # not followed and expanded into a second copy of the binary.
    rm -rf "$bin_dir/quietmoused.app"
    ditto "$dir/quietmoused.app" "$bin_dir/quietmoused.app"
    cp -P "$dir/quietmoused" "$bin_dir/quietmoused"
    # curl doesn't quarantine what it downloads, but a copy replaced here might
    # have come from a browser, and Gatekeeper would block that one.
    xattr -dr com.apple.quarantine "$bin_dir/quietmouse" "$bin_dir/quietmoused" "$bin_dir/quietmoused.app" 2> /dev/null || true
    echo "Installed quietmouse and quietmoused in $bin_dir"

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
}

# Everything happens in here, called on the last line, so a download cut short
# can't run half the script.
main() {
    uninstall=false
    update=false
    case "${1:-}" in
        --uninstall) uninstall=true ;;
        --update) update=true ;;
        "") ;;
        *) fail "usage: install.sh [--update | --uninstall]" ;;
    esac

    case "$(uname -s)" in
        MINGW* | MSYS* | CYGWIN*)
            # Git Bash and its relatives run the Windows build, so hand over to the
            # Windows installer rather than doing a second, worse job of it here.
            flag=
            if $uninstall; then flag=-Uninstall; fi
            if $update; then flag=-Update; fi
            # Saved and run as a file. Starting PowerShell with a download on its
            # command line is what droppers do, and Defender treats it that way.
            work=$(mktemp -d)
            trap 'rm -rf "$work"' EXIT
            curl -fsSL -o "$work/install.ps1" "$raw/windows/install.ps1"
            powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$work/install.ps1")" ${flag:+"$flag"}
            return
            ;;
        Darwin)
            flavour=macos-universal
            if $uninstall; then
                uninstall_macos
                return
            fi
            ;;
        Linux)
            case "$(uname -m)" in
                x86_64 | amd64) flavour=linux-x86_64 ;;
                *) fail "quietmouse has no Linux build for $(uname -m) yet" ;;
            esac
            ;;
        *) fail "quietmouse doesn't run on $(uname -s)" ;;
    esac

    installed=$bin_dir/quietmouse
    if $update && [ ! -x "$installed" ]; then
        other=$(command -v quietmouse 2> /dev/null || true)
        if [ -n "$other" ]; then
            # Replacing it here would leave two copies fighting over log-in.
            fail "quietmouse at $other wasn't installed by this script; update it the way you installed it (brew upgrade quietmouse, or your package manager)."
        fi
        fail "quietmouse isn't installed. Install it with: curl -fsSL $raw/install.sh | sh"
    fi

    tag=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" |
        sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "$tag" ] || fail "can't find the latest quietmouse release"
    if ! $uninstall && [ -x "$installed" ]; then
        current=$("$installed" --version 2> /dev/null | sed 's/^quietmouse *//')
        if [ "v$current" = "$tag" ]; then
            echo "quietmouse $current is already the latest release."
            # Still make sure it's registered and running, which is the one thing
            # someone running this again may be hoping to fix.
            "$installed" autostart on
            return
        fi
        echo "Updating quietmouse $current to $tag"
    fi
    name=quietmouse-$tag-$flavour
    base=https://github.com/$repo/releases/download/$tag

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    echo "Downloading quietmouse $tag for $flavour"
    curl -fsSL -o "$work/$name.tar.gz" "$base/$name.tar.gz"
    curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS"
    # The file lists "<hash>  <name>", one per line.
    expected=$(awk -v file="$name.tar.gz" '$2 == file || $2 == "*" file { print $1 }' "$work/SHA256SUMS")
    [ -n "$expected" ] || fail "SHA256SUMS for $tag doesn't list $name.tar.gz"
    [ "$(sha256 "$work/$name.tar.gz")" = "$expected" ] ||
        fail "$name.tar.gz doesn't match its checksum; nothing was installed"
    tar -xzf "$work/$name.tar.gz" -C "$work"

    case $flavour in
        macos-*) install_macos "$work/$name" ;;
        linux-*)
            if $uninstall; then
                sh "$work/$name/install.sh" --uninstall
            else
                sh "$work/$name/install.sh"
            fi
            ;;
    esac
}

main "$@"
