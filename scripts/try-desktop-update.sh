#!/bin/sh
# Proves the desktop app updates itself through Sparkle, end to end, with nothing real: a copy with
# a test bundle ID (run.kuvo.code.sparkletest), a throwaway update key made here, two builds (OLD
# and NEW), and a feed served from 127.0.0.1. The old copy, in a temporary folder, finds the new
# one, downloads it, checks its signature against the key, and installs it over itself once it's
# quit (by its PID); then the copy in that folder is checked to be the new one, signed whole.
#   scripts/try-desktop-update.sh
#   OLD, NEW    the two versions (default 9.0.0 and 9.0.1)
#   PORT        the feed's port (default 8743)
#   PROMPT=1    instead: the old copy finds the update and Sparkle's own window offers it (with
#               no automatic install); it's left open for you to look at, and Ctrl-C ends it all
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
SERVER="" APP_PID=""
finish() {
    set +e
    [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null && wait "$APP_PID" 2>/dev/null
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

# Waits, on the file system's word (no clock), for what's asked: a line in a log, or a version in a
# bundle; gives up after a time.
cat >"$work/wait.py" <<'EOF'
import os, select, sys, time, plistlib
kind, path, wanted, limit = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4])
def done():
    try:
        if kind == "log":
            return wanted in open(path, errors="replace").read()
        with open(os.path.join(path, "Kuvo.app/Contents/Info.plist"), "rb") as info:
            return plistlib.load(info).get("CFBundleShortVersionString") == wanted
    except OSError:
        return False
fd = os.open(path, os.O_RDONLY)
queue = select.kqueue()
event = select.kevent(fd, select.KQ_FILTER_VNODE, select.KQ_EV_ADD | select.KQ_EV_CLEAR, select.KQ_NOTE_WRITE | select.KQ_NOTE_EXTEND | select.KQ_NOTE_RENAME | select.KQ_NOTE_LINK)
end = time.monotonic() + limit
while not done():
    left = end - time.monotonic()
    if left <= 0:
        sys.exit(f"gave up after {limit:.0f}s waiting for {wanted}")
    queue.control([event], 1, min(left, 5))
EOF

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
env -u KUVO_DEMO_MACHINE_ROOT "$INSTALLED/Contents/MacOS/kuvo" ${APP_ARGS:-} >>"$work/app.log" 2>&1 &
APP_PID=$!
echo "Opened the $OLD copy ($APP_PID), reading $FEED."

if [ "${PROMPT:-}" = 1 ]; then
    python3 "$work/wait.py" log "$work/app.log" "Kuvo $NEW is available" 120
    echo "Sparkle's window offers $NEW. Ctrl-C ends it."
    wait "$APP_PID"
    exit 0
fi

python3 "$work/wait.py" log "$work/app.log" "installs when Kuvo quits" 300 || { cat "$work/app.log" >&2; exit 1; }
cat "$work/app.log"
kill "$APP_PID"
wait "$APP_PID" 2>/dev/null || true
APP_PID=""
echo "Quit the $OLD copy; Sparkle installs $NEW."
python3 "$work/wait.py" version "$work/Applications" "$NEW" 180 || { echo "The copy is still $(version_of "$INSTALLED")." >&2; exit 1; }
codesign --verify --deep --strict "$INSTALLED"
./scripts/check-desktop-app.sh "$INSTALLED"
echo "Updated: the copy at $INSTALLED went from $OLD to $(version_of "$INSTALLED") by itself, through Sparkle, checked with the update key."
