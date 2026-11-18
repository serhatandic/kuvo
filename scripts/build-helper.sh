#!/bin/sh
# Builds Kuvo Helper (protocol/PROTOCOL.md, "This Mac's box: its helper"), the small program a
# Mac's box runs under in ~/.kuvo-code, and kuvo-keys, the one-time hand-over of Kuvo Code 0.7's
# Keychain items to it, from their own package (mac/helper), on their own: no app is built or
# needed. The helper signed, so it can be notarized as it is and packed into the box's release;
# kuvo-keys only as KUVO_SIGNED_KEYS says, below.
#   scripts/build-helper.sh <folder>      makes <folder>/Kuvo Helper.app, and <folder>/kuvo-keys
#                                         for an ad hoc build (or when asked: KUVO_SIGNED_KEYS)
#   SIGN_IDENTITY      codesigning identity (default "-": ad hoc, for local use); with one, a
#                      secure timestamp too, as notarization requires. The hardened runtime always.
#   APP_ID             Kuvo Code's bundle ID (default run.kuvo.code): kuvo-keys is signed with it,
#                      and the helper's ID is it with .box after it. A test's own family
#                      (run.kuvo.code.trial4) never meets a real one in the Keychain or launchd.
#   VERSION, BUILD     the helper's version and build number (default 0.0.0 and 0: a local build)
#   ICON               an .icns for the helper (Kuvo Code's: what Login Items and macOS's
#                      questions show); none, and it has macOS's plain one
#   MIN_SYSTEM         the oldest macOS it runs on (default 26.0)
#   KUVO_SIGNED_KEYS   1: kuvo-keys built and signed with SIGN_IDENTITY too. Without it, a build
#                      signed with a real identity makes NO kuvo-keys, and nothing asks for one
#                      yet (scripts/build-app.sh never does): signed as Kuvo Code by its team, it
#                      reads Kuvo Code's Keychain items with no prompt, and what keeps it to the
#                      box's move (the move's lock) holds only against a sandboxed command, not
#                      against another program of yours. No signed kuvo-keys is to exist until
#                      #713 piece 4 settles that guard. An ad hoc build (tests) always has one:
#                      it's no one to the Keychain.
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
    product=$1
    set -- --arch arm64
    # The machine engine also ships for Intel Macs. The same signed Helper starts both.
    [ "$product" != KuvoHelper ] || set -- "$@" --arch x86_64
    swift build -c release --package-path "$PACKAGE" --product "$product" "$@" -q >&2
    echo "$(swift build -c release --package-path "$PACKAGE" "$@" --show-bin-path)/$product"
}
# (One built elsewhere is a test's: never signed as the real thing, which would give a test build,
# with its test-only options, Kuvo Code's identity.)
if [ "$SIGN_IDENTITY" != "-" ] && [ -n "${HELPER_EXECUTABLE:-}${KEYS_EXECUTABLE:-}" ]; then
    echo "HELPER_EXECUTABLE and KEYS_EXECUTABLE are for ad hoc builds only: a signed one is built here." >&2
    exit 2
fi
if [ -n "${HELPER_EXECUTABLE:-}" ]; then EXECUTABLE=$HELPER_EXECUTABLE; else EXECUTABLE=$(built KuvoHelper); fi
WITH_KEYS=0
if [ "$SIGN_IDENTITY" = "-" ] || [ "${KUVO_SIGNED_KEYS:-}" = 1 ]; then WITH_KEYS=1; fi
KEYS_BUILT=""
if [ "$WITH_KEYS" = 1 ]; then
    if [ -n "${KEYS_EXECUTABLE:-}" ]; then KEYS_BUILT=$KEYS_EXECUTABLE; else KEYS_BUILT=$(built kuvo-keys); fi
    test -x "$KEYS_BUILT" || { echo "No kuvo-keys at $KEYS_BUILT" >&2; exit 1; }
fi
test -x "$EXECUTABLE" || { echo "No KuvoHelper at $EXECUTABLE" >&2; exit 1; }

# Each made beside where it goes, then put in place whole.
NEW="$OUT.new"
NEW_KEYS="$FOLDER/kuvo-keys.new"
# (And never a kuvo-keys left from an earlier build, whatever becomes of this one.)
rm -rf "$NEW" "$NEW_KEYS" "$FOLDER/kuvo-keys"
trap 'rm -rf "$NEW" "$NEW_KEYS"' EXIT
mkdir -p "$NEW/Contents/MacOS" "$NEW/Contents/Resources"
cp "$EXECUTABLE" "$NEW/Contents/MacOS/$NAME"
[ -n "${HELPER_EXECUTABLE:-}" ] || strip -rSTx "$NEW/Contents/MacOS/$NAME"
if [ "$WITH_KEYS" = 1 ]; then
    cp "$KEYS_BUILT" "$NEW_KEYS"
    [ -n "${KEYS_EXECUTABLE:-}" ] || strip -rSTx "$NEW_KEYS"
fi
INFO="$NEW/Contents/Info.plist"
# Its names, and what macOS says as it asks, for what the box's agents and integrations reach.
cp "$PACKAGE/Info.plist" "$INFO"
put() { plutil -insert "$1" "-$2" "$3" "$INFO"; }
put CFBundleIdentifier string "$HELPER_ID"
put CFBundleShortVersionString string "$VERSION"
put CFBundleVersion string "$BUILD"
put LSMinimumSystemVersion string "${MIN_SYSTEM:-26.0}"
# The services' marks for its menu bar panel's rings: the desktop app's own (gpui/assets/marks),
# one set for both, read by the service's id (`ServiceMark`).
mkdir -p "$NEW/Contents/Resources/Marks"
cp "$PACKAGE/../../gpui/assets/marks/"*.svg "$NEW/Contents/Resources/Marks/"
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
if [ "$WITH_KEYS" = 1 ]; then
    codesign "$@" --identifier "$APP_ID" "$NEW_KEYS"
    codesign --verify --strict "$NEW_KEYS"
fi

rm -rf "$OUT"
mv "$NEW" "$OUT"
if [ "$WITH_KEYS" = 1 ]; then
    mv -f "$NEW_KEYS" "$FOLDER/kuvo-keys"
    echo "Built $OUT ($HELPER_ID) and $FOLDER/kuvo-keys ($APP_ID)"
else
    echo "Built $OUT ($HELPER_ID); no kuvo-keys (signed with a real identity only with KUVO_SIGNED_KEYS=1)"
fi
trap - EXIT
