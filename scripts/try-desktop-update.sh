#!/bin/sh
# Proves the desktop app updates itself through Sparkle, end to end, with nothing real: a copy with
# a test bundle ID (run.kuvo.code.sparkletest), a throwaway update key made here, two builds (OLD
# and NEW), and a feed served from 127.0.0.1. The old copy, in a temporary folder, finds the new
# one, downloads it, checks its signature against the key, and installs it over itself once it's
# quit (by its PID); then the copy in that folder is checked to be the new one, signed whole.
#   scripts/try-desktop-update.sh
#   OLD, NEW    the two versions (default 9.0.0 and 9.0.1)
#   PORT        the feed's port (default 8743)
#   PROMPT=1    Install and Relaunch, as most people get updates: Sparkle's own window offers the
#               update (no automatic install); you click Install and Relaunch; the copy quits,
#               Sparkle installs the new one in its place and opens it; that's checked (found by
#               its exact test bundle ID, from that place), and you quit it with ⌘Q to end. Needs
#               you at the Mac.
# It opens the old copy's window on this Mac for a minute (it refuses to set anything up: a copy
# with no root of its own). Everything goes when it ends: the copies, the key, the feed, and what
# Sparkle and macOS kept for the test ID (its defaults, caches). Never the real app's: its ID,
# feed and key are refused (scripts/build-desktop-app.sh, scripts/real-app-guard.sh). A heavy
# build (two bundles): run it through scripts/heavy.sh mac-build.
set -eu
cd "$(dirname "$0")/.."
. scripts/real-app-guard.sh

ID=run.kuvo.code.sparkletest
OLD="${OLD:-9.0.0}"
NEW="${NEW:-9.0.1}"
PORT="${PORT:-8743}"
real_app_guard scripts/try-desktop-update.sh "$ID"
NODE="${NODE:-node}"

temporary=${TMPDIR:-/tmp}
work=$(mktemp -d "${temporary%/}/kuvo-try-update.XXXXXXXX")
work=$(cd "$work" && pwd -P)
SERVER="" APP_PID="" RELAUNCHED=""
WAIT="$PWD/scripts/try-desktop-update-wait.py"
finish() {
    set +e
    [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null && wait "$APP_PID" 2>/dev/null
    # (The copy Sparkle opened, where this ended before you quit it: by its PID alone, before its
    # folder goes.)
    if [ -n "$RELAUNCHED" ] && kill "$RELAUNCHED" 2>/dev/null; then
        python3 "$WAIT" exited "$RELAUNCHED" 30
    fi
    [ -n "$SERVER" ] && kill "$SERVER" 2>/dev/null && wait "$SERVER" 2>/dev/null
    defaults delete "$ID" >/dev/null 2>&1
    for kept in "Caches/$ID" "HTTPStorages/$ID" "HTTPStorages/$ID.binarycookies" "Saved Application State/$ID.savedState" "WebKit/$ID" "Preferences/$ID.plist"; do
        rm -rf "$HOME/Library/$kept"
    done
    rm -rf "$work"
}
trap finish EXIT
trap 'exit 130' INT TERM HUP

# The throwaway update key, as Sparkle's tools keep one: the private key's 32-byte seed, and the
# public key, each in base64. Never the keychain's.
mkdir "$work/key"
"$NODE" -e '
const crypto = require("node:crypto"), fs = require("node:fs");
const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
const b64 = text => Buffer.from(text, "base64url").toString("base64");
fs.writeFileSync(process.argv[1] + "/private", b64(privateKey.export({ format: "jwk" }).d), { mode: 0o600 });
fs.writeFileSync(process.argv[1] + "/public", b64(publicKey.export({ format: "jwk" }).x));
' "$work/key"
PUBLIC=$(cat "$work/key/public")
FEED="http://127.0.0.1:$PORT/appcast.xml"

for version in "$OLD" "$NEW"; do
    APP_ID="$ID" VERSION="$version" OUT="$work/build-$version" FEED_URL="$FEED" UPDATE_PUBLIC_KEY="$PUBLIC" ./scripts/build-desktop-app.sh
done

# The feed, as the release writes it: the new version's disk image, its item signed with the key,
# merged as the release merges it.
mkdir "$work/feed"
APP="$work/build-$NEW/Kuvo.app" DMG="$work/feed/KuvoCode.dmg" ./scripts/build-dmg.sh
printf 'Made by scripts/try-desktop-update.sh.\n' > "$work/notes.md"
SPARKLE_PRIVATE_KEY=$(cat "$work/key/private") ./scripts/appcast.sh "$NEW" "$work/feed/KuvoCode.dmg" "http://127.0.0.1:$PORT/KuvoCode.dmg" "$work/notes.md" > "$work/item.xml"
python3 scripts/merge-appcast.py "$work/item.xml" > "$work/feed/appcast.xml"
echo "The feed:"
cat "$work/feed/appcast.xml"

# The old copy, where it's installed for the test.
mkdir "$work/Applications"
ditto "$work/build-$OLD/Kuvo.app" "$work/Applications/Kuvo.app"
INSTALLED="$work/Applications/Kuvo.app"
version_of() { plutil -extract CFBundleShortVersionString raw -o - "$1/Contents/Info.plist"; }
[ "$(version_of "$INSTALLED")" = "$OLD" ]

python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$work/feed" >"$work/server.log" 2>&1 &
SERVER=$!

# The old copy's Sparkle loads as the app loads it, and it names the test's feed.
"$INSTALLED/Contents/MacOS/kuvo" --updater-status

# Sparkle's settings, in the test ID's defaults alone: what it finds downloads and installs as the
# copy quits (PROMPT=1: it asks, in its own window).
if [ "${PROMPT:-}" = 1 ]; then
    defaults write "$ID" SUAutomaticallyUpdate -bool NO
else
    defaults write "$ID" SUAutomaticallyUpdate -bool YES
fi
: >"$work/app.log"
# (APP_ARGS: for a look at Sparkle's window in dark, -AppleInterfaceStyle Dark.)
# shellcheck disable=SC2086
# (Only what it needs of this shell's: no KUVO_* variables. Your home folder, where Sparkle keeps
# the test ID's settings and caches, and the cleanup removes them.)
env -i HOME="$HOME" PATH="/usr/bin:/bin:/usr/sbin:/sbin" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" "$INSTALLED/Contents/MacOS/kuvo" ${APP_ARGS:-} >>"$work/app.log" 2>&1 &
APP_PID=$!
echo "Opened the $OLD copy ($APP_PID), reading $FEED."

if [ "${PROMPT:-}" = 1 ]; then
    python3 "$WAIT" log "$work/app.log" "Kuvo $NEW is available" 120 || { cat "$work/app.log" >&2; exit 1; }
    echo "Sparkle's window offers $NEW: click Install and Relaunch."
    # (However long you take: the copy ends once you've clicked, or when you quit it.)
    wait "$APP_PID" 2>/dev/null || true
    APP_PID=""
else
    python3 "$WAIT" log "$work/app.log" "installs when Kuvo quits" 300 || { cat "$work/app.log" >&2; exit 1; }
    kill "$APP_PID"
    wait "$APP_PID" 2>/dev/null || true
    APP_PID=""
fi
cat "$work/app.log"
echo "The $OLD copy quit; Sparkle installs $NEW."
python3 "$WAIT" version "$work/Applications" "$NEW" 180 || { echo "The copy is still $(version_of "$INSTALLED")." >&2; exit 1; }
codesign --verify --deep --strict "$INSTALLED"
./scripts/check-desktop-app.sh "$INSTALLED"
"$INSTALLED/Contents/MacOS/kuvo" --updater-status
if [ "${PROMPT:-}" = 1 ]; then
    # Opened again by Sparkle, from the same place, as the new version.
    RELAUNCHED=$(python3 "$WAIT" launched "$ID" "$INSTALLED" 120) || { echo "Sparkle didn't open $NEW again after installing it." >&2; exit 1; }
    echo "Sparkle opened $NEW again ($RELAUNCHED, from $INSTALLED). Quit it with ⌘Q to finish."
    python3 "$WAIT" exited "$RELAUNCHED" 3600 || { echo "It's still open; it's quit by its PID as this ends." >&2; exit 1; }
    RELAUNCHED=""
    echo "Installed and relaunched: the copy went from $OLD to $NEW through Sparkle's Install and Relaunch, and Sparkle opened it again."
    exit 0
fi
echo "Updated: the copy at $INSTALLED went from $OLD to $(version_of "$INSTALLED") by itself, through Sparkle, checked with the update key."
