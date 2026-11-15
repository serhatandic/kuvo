#!/bin/sh
# Builds Kuvo Helper (protocol/PROTOCOL.md, "This Mac's box: its helper"), the small program a
# Mac's box runs under in ~/.kuvo-code, and kuvo-keys, the one-time hand-over of Kuvo Code 0.7's
# Keychain items to it, from their own package (mac/helper), on their own: no app is built or
# needed. Both signed, so they can be notarized as they are and packed into the box's release.
#   scripts/build-helper.sh <folder>      makes <folder>/Kuvo Helper.app and <folder>/kuvo-keys
#   SIGN_IDENTITY      codesigning identity (default "-": ad hoc, for local use); with one, a
#                      secure timestamp too, as notarization requires. The hardened runtime always.
#   APP_ID             Kuvo Code's bundle ID (default run.kuvo.code): kuvo-keys is signed with it,
#                      and the helper's ID is it with .box after it. A test's own family
#                      (run.kuvo.code.trial4) never meets a real one in the Keychain or launchd.
#   VERSION, BUILD     the helper's version and build number (default 0.0.0 and 0: a local build)
#   ICON               an .icns for the helper (Kuvo Code's: what Login Items and macOS's
#                      questions show); none, and it has macOS's plain one
#   MIN_SYSTEM         the oldest macOS it runs on (default 26.0)
#   HELPER_EXECUTABLE, KEYS_EXECUTABLE   each built already (a test's debug build), not built here
# The helper's identifier, its signer and its place stay the same from version to version: macOS
# keeps what you allowed it, and its Keychain items, by them. Nothing here installs or runs it.
#
# To notarize what a Developer ID signed (the release's step, never a session's):
#   ditto -c -k --keepParent "<folder>/Kuvo Helper.app" helper.zip
#   xcrun notarytool submit helper.zip --wait …   then   xcrun stapler staple "<folder>/Kuvo Helper.app"
#   (kuvo-keys, a bare program, is notarized inside the app that carries it: it can't be stapled.)
set -eu

if [ $# -ne 1 ] || [ -z "$1" ]; then
    echo "Usage: scripts/build-helper.sh <folder>" >&2
    exit 2
fi
mkdir -p "$1"
FOLDER=$(cd "$1" && pwd -P)
cd "$(dirname "$0")/.."
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
APP_ID="${APP_ID:-run.kuvo.code}"
case "$APP_ID" in
    *[!A-Za-z0-9.-]* | "" | .* | *.box) echo "Not Kuvo Code's bundle ID: $APP_ID" >&2; exit 2 ;;
esac
HELPER_ID="$APP_ID.box"
VERSION="${VERSION:-0.0.0}"
BUILD="${BUILD:-0}"
NAME="Kuvo Helper"
PACKAGE=mac/helper
OUT="$FOLDER/$NAME.app"

built() {
    swift build -c release --package-path "$PACKAGE" --product "$1" --arch arm64 -q
    echo "$(swift build -c release --package-path "$PACKAGE" --arch arm64 --show-bin-path)/$1"
}
if [ -n "${HELPER_EXECUTABLE:-}" ]; then EXECUTABLE=$HELPER_EXECUTABLE; else EXECUTABLE=$(built KuvoHelper); fi
if [ -n "${KEYS_EXECUTABLE:-}" ]; then KEYS=$KEYS_EXECUTABLE; else KEYS=$(built kuvo-keys); fi
test -x "$EXECUTABLE" || { echo "No KuvoHelper at $EXECUTABLE" >&2; exit 1; }
test -x "$KEYS" || { echo "No kuvo-keys at $KEYS" >&2; exit 1; }

# Each made beside where it goes, then put in place whole.
NEW="$OUT.new"
NEW_KEYS="$FOLDER/kuvo-keys.new"
rm -rf "$NEW" "$NEW_KEYS"
trap 'rm -rf "$NEW" "$NEW_KEYS"' EXIT
mkdir -p "$NEW/Contents/MacOS" "$NEW/Contents/Resources"
cp "$EXECUTABLE" "$NEW/Contents/MacOS/$NAME"
cp "$KEYS" "$NEW_KEYS"
[ -n "${HELPER_EXECUTABLE:-}" ] || strip -rSTx "$NEW/Contents/MacOS/$NAME"
[ -n "${KEYS_EXECUTABLE:-}" ] || strip -rSTx "$NEW_KEYS"
INFO="$NEW/Contents/Info.plist"
# Its names, and what macOS says as it asks, for what the box's agents and integrations reach.
cp "$PACKAGE/Info.plist" "$INFO"
put() { plutil -insert "$1" "-$2" "$3" "$INFO"; }
put CFBundleIdentifier string "$HELPER_ID"
put CFBundleShortVersionString string "$VERSION"
put CFBundleVersion string "$BUILD"
put LSMinimumSystemVersion string "${MIN_SYSTEM:-26.0}"
if [ -n "${ICON:-}" ]; then
    test -f "$ICON" || { echo "No icon at $ICON" >&2; exit 1; }
    cp "$ICON" "$NEW/Contents/Resources/AppIcon.icns"
    put CFBundleIconFile string AppIcon
fi

if [ "$SIGN_IDENTITY" = "-" ]; then
    set -- --force --sign - --options runtime
else
    set -- --force --sign "$SIGN_IDENTITY" --options runtime --timestamp
fi
codesign "$@" --identifier "$HELPER_ID" --entitlements "$PACKAGE/KuvoHelper.entitlements" "$NEW"
codesign --verify --strict --deep "$NEW"
# kuvo-keys is Kuvo Code itself to the Keychain: its identifier, and no entitlements.
codesign "$@" --identifier "$APP_ID" "$NEW_KEYS"
codesign --verify --strict "$NEW_KEYS"

rm -rf "$OUT"
mv "$NEW" "$OUT"
mv -f "$NEW_KEYS" "$FOLDER/kuvo-keys"
trap - EXIT
echo "Built $OUT ($HELPER_ID) and $FOLDER/kuvo-keys ($APP_ID)"
