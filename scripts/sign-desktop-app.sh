#!/bin/sh
# Signs a desktop app bundle (scripts/build-desktop-app.sh) inside out: what it carries first, the
# app over them last, so the signature holds for all of it (codesign --verify --deep --strict) and
# notarization takes it. Everything with the hardened runtime, but an ad hoc app itself: under it,
# a process loads only libraries signed by its own team, and ad hoc code has none, so an ad hoc
# app with it can't load its own Sparkle ("mapping process and mapped file (non-platform) have
# different Team IDs"). Signed with a Developer ID, the app and Sparkle are one team's, and the
# app has the hardened runtime as notarization requires. Whatever it's signed with, check it loads
# its Sparkle as the app will: `<Kuvo.app>/Contents/MacOS/kuvo --updater-status` (it starts and
# reaches nothing; 0 where Sparkle loads, with the feed it names).
#   scripts/sign-desktop-app.sh <Kuvo.app>
#   SIGN_IDENTITY     "-" (the default): ad hoc, as every local build is. A Developer ID (its
#                     SHA-1, as release.yml names it) is the release's step: with a secure
#                     timestamp too, as notarization requires.
#   KUVO_SIGNED_KEYS  1: kuvo-keys is signed with that Developer ID too. Without it, a release
#                     signing takes kuvo-keys OUT of the app, and says so: signed as Kuvo Code by
#                     its team it reads Kuvo Code 0.7's Keychain items with no prompt, and no such
#                     kuvo-keys is to exist until #713 piece 4 settles what keeps it to the box's
#                     move (scripts/build-helper.sh). Such an app can't hand 0.7's Keychain items
#                     over, so it isn't one to publish (release.yml checks).
# In order:
#   Contents/Frameworks/Sparkle.framework   Autoupdate and Updater.app (the programs that install
#                                           an update and show its progress), then the framework
#   Contents/MacOS/kuvo-keys                as the app itself to the Keychain: its identifier, and
#                                           no entitlements
#   the app                                 as its bundle ID; no entitlements (what macOS asks
#                                           about is asked of Kuvo Helper, signed in the box's
#                                           release, which this never changes)
set -eu

if [ $# -ne 1 ] || [ ! -d "$1/Contents" ]; then
    echo "Usage: scripts/sign-desktop-app.sh <Kuvo.app>" >&2
    exit 2
fi
APP=$1
IDENTITY="${SIGN_IDENTITY:--}"
ID=$(plutil -extract CFBundleIdentifier raw -o - "$APP/Contents/Info.plist")
if [ "$IDENTITY" = "-" ]; then
    set -- --force --sign - --options runtime
    APP_RUNTIME=""
else
    set -- --force --sign "$IDENTITY" --options runtime --timestamp
    APP_RUNTIME=runtime
fi

FRAMEWORK="$APP/Contents/Frameworks/Sparkle.framework"
if [ -d "$FRAMEWORK" ]; then
    codesign "$@" "$FRAMEWORK/Versions/B/Autoupdate"
    codesign "$@" "$FRAMEWORK/Versions/B/Updater.app"
    codesign "$@" "$FRAMEWORK"
fi

KEYS="$APP/Contents/MacOS/kuvo-keys"
if [ -e "$KEYS" ]; then
    if [ "$IDENTITY" = "-" ] || [ "${KUVO_SIGNED_KEYS:-}" = 1 ]; then
        codesign "$@" --identifier "$ID" "$KEYS"
    else
        rm -f "$KEYS"
        echo "No kuvo-keys in this app: signed with a real identity only with KUVO_SIGNED_KEYS=1 (#713 piece 4). It can't hand Kuvo Code 0.7's Keychain items over." >&2
    fi
fi

if [ -n "$APP_RUNTIME" ]; then
    codesign "$@" --identifier "$ID" "$APP"
else
    codesign --force --sign - --identifier "$ID" "$APP"
fi
