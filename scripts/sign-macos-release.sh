#!/bin/sh
# Signs and notarises the macOS build of a draft release, then publishes it.
#
# A pushed tag makes the release workflow build everything and leave the release as
# a draft. This takes its macOS archive, signs quietmouse and quietmoused.app with the
# Developer ID in this Mac's keychain, has Apple notarise them, staples the ticket to
# the bundle, and swaps the signed archive and its checksum into the release.
# Publishing the release then points the Homebrew tap at it (.github/workflows/tap.yml).
#
# The signing key never leaves the keychain. Notarising uses the credentials saved as
# the notarytool profile "notary":
#   xcrun notarytool store-credentials notary --key AuthKey_<key id>.p8 --key-id <key id> --issuer <issuer id>
#
# Usage: sh scripts/sign-macos-release.sh v0.1.29
set -eu
cd "$(dirname "$0")/.."

repo=benjweaver/quietmouse
identity="Developer ID Application: Ben Weaver (AR25V66TVY)"
profile=notary
tag=${1:?usage: sh scripts/sign-macos-release.sh <tag>}
name="quietmouse-$tag-macos-universal"

if [ "$(gh release view "$tag" --repo "$repo" --json isDraft --jq .isDraft)" != true ]; then
    echo "$tag isn't a draft release" >&2
    exit 1
fi
if ! security find-identity -v -p codesigning | grep -qF "\"$identity\""; then
    echo "\"$identity\" isn't in the keychain" >&2
    exit 1
fi
if ! xcrun notarytool history --keychain-profile "$profile" > /dev/null; then
    echo "the notarytool profile \"$profile\" doesn't work" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh release download "$tag" --repo "$repo" --dir "$work" --pattern "$name.tar.gz" --pattern SHA256SUMS
tar -xzf "$work/$name.tar.gz" -C "$work"
dir="$work/$name"
bundle="$dir/quietmoused.app"

# The hardened runtime and a secure timestamp are what notarisation requires. The
# bundle keeps the ID it was built with. Its entitlement lets shell commands bound to
# buttons (osascript, say) control other apps, which the hardened runtime otherwise
# blocks.
sign() {
    codesign --force --options runtime --timestamp --sign "$identity" "$@"
}
sign --identifier io.github.benjweaver.quietmouse.quietmouse "$dir/quietmouse"
sign --identifier "$(plutil -extract CFBundleIdentifier raw "$bundle/Contents/Info.plist")" \
    --entitlements packaging/macos/quietmoused.entitlements "$bundle"
codesign --verify --all-architectures --strict "$dir/quietmouse"
codesign --verify --all-architectures --strict "$bundle"

# Notarise both in one submission, then staple the ticket to the bundle. A bare binary
# can't hold one, so Gatekeeper looks quietmouse's up online the first time it checks.
ditto -c -k --keepParent "$dir" "$work/notarize.zip"
result=$(xcrun notarytool submit "$work/notarize.zip" --keychain-profile "$profile" --wait --output-format json)
if [ "$(printf '%s' "$result" | plutil -extract status raw -o - -)" != Accepted ]; then
    echo "notarisation failed: $result" >&2
    id=$(printf '%s' "$result" | plutil -extract id raw -o - -)
    echo "details: xcrun notarytool log $id --keychain-profile $profile" >&2
    exit 1
fi
xcrun stapler staple -q "$bundle"
spctl --assess --type execute "$bundle"

# Repack under the same name, without macOS metadata, and give it its new checksum.
rm "$work/$name.tar.gz"
COPYFILE_DISABLE=1 tar --no-mac-metadata -czf "$work/$name.tar.gz" -C "$work" "$name"
hash=$(shasum -a 256 "$work/$name.tar.gz" | cut -d ' ' -f 1)
awk -v file="$name.tar.gz" -v hash="$hash" '$2 == file { $1 = hash } { print }' OFS='  ' \
    "$work/SHA256SUMS" > "$work/SHA256SUMS.signed"
if ! grep -qF "$hash  $name.tar.gz" "$work/SHA256SUMS.signed"; then
    echo "SHA256SUMS has no line for $name.tar.gz" >&2
    exit 1
fi
mv "$work/SHA256SUMS.signed" "$work/SHA256SUMS"

gh release upload "$tag" "$work/$name.tar.gz" "$work/SHA256SUMS" --repo "$repo" --clobber
gh release edit "$tag" --repo "$repo" --draft=false
echo "Published $tag with the macOS build signed and notarised; the tap update is starting."
