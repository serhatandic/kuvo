#!/bin/sh
# A private update feed on this Mac for the user's own copy of the desktop app, before the first
# release with a feed (0.8): their Kuvo (run.kuvo.code, signed with their Apple Development
# identity) reads it over the tailnet and updates itself from it like anyone's, through Sparkle.
# Nothing is published: the feed and its disk images stay in a folder on this Mac, served on
# 127.0.0.1 and, through `tailscale serve`, over https to the user's tailnet only.
# (.claude/notes/private-feed.md: why, the one-time setup, and the move to the real feed at 0.8.)
#
#   scripts/private-feed.sh setup      once, in your Terminal: the folder, the feed's update key
#                                      (made here, kept in the login keychain), and a login agent
#                                      serving the folder on 127.0.0.1:$PORT; then prints the
#                                      `tailscale serve` command to run yourself
#   scripts/heavy.sh mac-build scripts/private-feed.sh build [VERSION]
#                                      in your Terminal, for each build: Kuvo for your own Mac,
#                                      reading the private feed, signed with your Apple
#                                      Development identity, its box release with the box key,
#                                      then its disk image signed with the feed's key and put in
#                                      the feed. VERSION like 0.8.0-beta.4; by default the next
#                                      beta after the feed's newest. A heavy build.
#   scripts/private-feed.sh status     what's in the feed, and whether it's served
#   scripts/private-feed.sh uninstall [--key]
#                                      the login agent and the folder go (and it prints the
#                                      command that stops `tailscale serve`); the feed's key only
#                                      with --key: a copy built for it trusts no other
#   scripts/private-feed.sh dry-run    the proof, with nothing real (through scripts/heavy.sh
#                                      mac-build): a temporary keychain and key, a test bundle ID,
#                                      ad hoc signing, a feed on 127.0.0.1; two builds put in the
#                                      feed by `build`, and the first, opened by its program,
#                                      finds the second and installs it over itself. All of it
#                                      goes when it ends. With SIGN_IDENTITY (in your Terminal),
#                                      the copies are signed with it, as your own copy is.
#
#   SIGN_IDENTITY   the identity your copy is signed with (default "Apple Development", as
#                   codesign finds it in your keychain: name one by its SHA-1 when there are two)
#   PORT            the local server's port (default 8790; the dry run's 8791); the tailnet's is 8443
#
# Where it keeps things: ~/Library/Application Support/KuvoPrivateFeed (feed/ is what's served:
# appcast.xml, boxes.json, the last three disk images and their box packs; feed-url; server.log), the login
# agent ~/Library/LaunchAgents/run.kuvo.private-feed.plist, and the keychain item "Kuvo private
# update feed key". Keys never go on a command line or in a file: each is read from the keychain
# inside this script.
set -eu
SELF=$(cd "$(dirname "$0")" && pwd -P)/$(basename "$0")
cd "$(dirname "$SELF")/.."

if [ "${1:-}" = dry-run ]; then PORT="${PORT:-8791}"; else PORT="${PORT:-8790}"; fi
TAILNET_PORT=8443
KEEP=3
LABEL=run.kuvo.private-feed
KEY_SERVICE="Kuvo private update feed key"
KEY_ACCOUNT=kuvo-private-feed
TAILSCALE=/Applications/Tailscale.app/Contents/MacOS/Tailscale
NODE="${NODE:-node}"

# The dry run (PRIVATE_FEED_TEST: its own folder) changes only what makes it real: the bundle ID,
# the signing, the keychain, the feed's address, and where the login agent's plist goes.
TEST="${PRIVATE_FEED_TEST:-}"
if [ -n "$TEST" ]; then
    ROOT="$TEST/root"
    KEYCHAIN="$TEST/test.keychain-db"
    AGENT="$TEST/$LABEL.plist"
    APP_ID=run.kuvo.code.privatefeedtest
    IDENTITY="${SIGN_IDENTITY:--}"
    KEEP=1
else
    ROOT="$HOME/Library/Application Support/KuvoPrivateFeed"
    KEYCHAIN=""
    AGENT="$HOME/Library/LaunchAgents/$LABEL.plist"
    APP_ID=run.kuvo.code
    IDENTITY="${SIGN_IDENTITY:-Apple Development}"
fi
FEED_DIR="$ROOT/feed"

say() { printf '%s\n' "$*"; }
die() { printf 'private-feed: %s\n' "$*" >&2; exit 1; }

# The feed's update key: an Ed25519 key's 32-byte seed, in base64, as Sparkle's sign_update reads
# one. Only in the keychain; its public half is worked out from it when it's needed.
has_key() { security find-generic-password -s "$KEY_SERVICE" -a "$KEY_ACCOUNT" ${KEYCHAIN:+"$KEYCHAIN"} >/dev/null 2>&1; }
private_key() { security find-generic-password -s "$KEY_SERVICE" -a "$KEY_ACCOUNT" -w ${KEYCHAIN:+"$KEYCHAIN"}; }
public_key() {
    private_key | "$NODE" -e '
const crypto = require("node:crypto");
const seed = Buffer.from(require("node:fs").readFileSync(0, "utf8").trim(), "base64");
if (seed.length !== 32) { console.error("private-feed: the key in the keychain is not an Ed25519 seed."); process.exit(1); }
const key = crypto.createPrivateKey({ key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), seed]), format: "der", type: "pkcs8" });
process.stdout.write(Buffer.from(crypto.createPublicKey(key).export({ format: "jwk" }).x, "base64url").toString("base64"));
'
}
make_key() {
    if has_key; then
        say "The feed's update key is in the keychain already (\"$KEY_SERVICE\"): kept."
        return
    fi
    seed=$("$NODE" -e 'process.stdout.write(Buffer.from(require("node:crypto").generateKeyPairSync("ed25519").privateKey.export({ format: "jwk" }).d, "base64url").toString("base64"))')
    # (Through security's standard input, never its command line.)
    printf 'add-generic-password -s "%s" -a "%s" -l "%s" -w "%s" %s\n' "$KEY_SERVICE" "$KEY_ACCOUNT" "$KEY_SERVICE" "$seed" "${KEYCHAIN:+\"$KEYCHAIN\"}" | security -i
    seed=""
    has_key || die "the feed's update key couldn't be put in the keychain."
    say "Made the feed's update key, in the keychain as \"$KEY_SERVICE\"."
}

# The login agent: Python's file server, on 127.0.0.1 only, serving feed/ (the tailnet reaches it
# through `tailscale serve`, which Tailscale.app can do for a port, not a folder).
write_agent() {
    mkdir -p "$(dirname "$AGENT")"
    cat >"$AGENT.new" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/python3</string><string>-I</string><string>-m</string><string>http.server</string>
    <string>$PORT</string><string>--bind</string><string>127.0.0.1</string>
    <string>--directory</string><string>$FEED_DIR</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$ROOT/server.log</string>
  <key>StandardErrorPath</key><string>$ROOT/server.log</string>
</dict>
</plist>
EOF
    plutil -lint "$AGENT.new" >/dev/null
    mv "$AGENT.new" "$AGENT"
}

feed_url() { cat "$ROOT/feed-url" 2>/dev/null || die "no private feed here yet: scripts/private-feed.sh setup first."; }
# The feed's folder on the web: where its disk images and box packs are downloaded from.
feed_base() { u=$(feed_url); printf '%s' "${u%/appcast.xml}"; }

serve_command() { printf '%s serve --bg --https=%s http://127.0.0.1:%s' "$TAILSCALE" "$TAILNET_PORT" "$PORT"; }

setup() {
    mkdir -p "$FEED_DIR"
    make_key
    if [ -n "$TEST" ]; then
        url="http://127.0.0.1:$PORT/appcast.xml"
    else
        # This Mac's name on the tailnet, as Tailscale has it (MagicDNS, with its https certificate).
        name=$("$TAILSCALE" status --json | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))') ||
            die "Tailscale didn't say this Mac's name on the tailnet: is it running and signed in?"
        url="https://$name:$TAILNET_PORT/appcast.xml"
    fi
    printf '%s\n' "$url" >"$ROOT/feed-url"
    write_agent
    if [ -z "$TEST" ]; then
        launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
        launchctl bootstrap "gui/$(id -u)" "$AGENT"
        curl -fsS --retry 10 --retry-connrefused --retry-delay 1 -o /dev/null "http://127.0.0.1:$PORT/" ||
            die "the login agent ($LABEL) isn't serving on 127.0.0.1:$PORT: $ROOT/server.log says why."
        say "Serving $FEED_DIR on 127.0.0.1:$PORT (login agent $LABEL)."
        say ""
        say "Once, to reach it from your other Macs on the tailnet (https, tailnet only, nothing public), run:"
        say "  $(serve_command)"
        say "Your copy will read $url"
    fi
}

# The newest version in the feed (its shortVersionString), or nothing.
newest_in_feed() {
    [ -f "$FEED_DIR/appcast.xml" ] || return 0
    sed -n 's|.*<sparkle:shortVersionString>\(.*\)</sparkle:shortVersionString>.*|\1|p' "$FEED_DIR/appcast.xml" | head -n 1
}

# Sparkle's order (scripts/merge-appcast.py's): exits 1 when $1 isn't newer than $2.
newer() {
    python3 -I - "$PWD/scripts/merge-appcast.py" "$(./scripts/build-version.sh "$1")" "$(./scripts/build-version.sh "$2")" <<'EOF'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("merge", sys.argv[1])
merge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(merge)
key = lambda build: merge.build(f"<sparkle:version>{build}</sparkle:version>")
sys.exit(0 if key(sys.argv[2]) > key(sys.argv[3]) else 1)
EOF
}

# The builds in the feed's folder past the newest $KEEP, in Sparkle's order.
past_keep() {
    ls "$FEED_DIR" | sed -n 's/^Kuvo-\(.*\)\.dmg$/\1/p' | python3 -I -c '
import importlib.util, subprocess, sys
spec = importlib.util.spec_from_file_location("merge", sys.argv[1])
merge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(merge)
def key(version):
    build = subprocess.run(["./scripts/build-version.sh", version], capture_output=True, text=True, check=True).stdout.strip()
    return merge.build(f"<sparkle:version>{build}</sparkle:version>")
versions = sorted((line.strip() for line in sys.stdin if line.strip()), key=key, reverse=True)
print("\n".join(versions[int(sys.argv[2]):]))
' "$PWD/scripts/merge-appcast.py" "$KEEP"
}

build() {
    [ -d "$FEED_DIR" ] && has_key || die "no private feed here yet: scripts/private-feed.sh setup first."
    lock="$ROOT/build.lock"
    /usr/bin/shlock -f "$lock" -p "$$" || die "another build of this private feed is running."
    work=""
    trap '[ -z "$work" ] || rm -rf "$work"; rm -f "$lock"' EXIT
    trap 'exit 130' INT TERM HUP
    url=$(feed_url)
    base=$(feed_base)
    current=$(newest_in_feed)
    # An interrupted publish may have exposed the box manifest before the appcast. Never
    # replace a pack under a version either manifest has already advertised.
    if [ -e "$FEED_DIR/boxes.json" ]; then
        box_current=$("$NODE" -e '
const fs = require("node:fs");
const envelope = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
const { version } = JSON.parse(Buffer.from(envelope.manifest, "base64"));
if (typeof version !== "string" || !/^\d+\.\d+\.\d+-beta\.\d+$/.test(version)) process.exit(1);
process.stdout.write(version);
' "$FEED_DIR/boxes.json") || die "can't read the current box release in the private feed."
        if [ -z "$current" ] || newer "$box_current" "$current"; then current=$box_current; fi
    fi
    if [ $# -ge 1 ]; then
        VERSION=$1
    else
        case "$current" in
            *-beta.*) VERSION="${current%-beta.*}-beta.$((${current##*-beta.} + 1))" ;;
            *) die "name the version: scripts/private-feed.sh build 0.8.0-beta.3 (the feed has ${current:-nothing yet})." ;;
        esac
    fi
    # (Betas only: a box that took a private 0.8.0 would never take the real 0.8.0 as newer.)
    printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$' ||
        die "a private build's version is a beta, like 0.8.0-beta.3, never $VERSION: the release keeps its own versions."
    if [ -n "$current" ] && ! newer "$VERSION" "$current"; then
        die "$VERSION isn't newer than the feed's $current."
    fi

    temporary=${TMPDIR:-/tmp}
    work=$(mktemp -d "${temporary%/}/kuvo-private-feed.XXXXXXXX")
    PUBLIC=$(public_key)
    say "Building Kuvo $VERSION ($APP_ID) reading $url."
    if [ -n "$TEST" ]; then
        env -u KUVO_BOX_SIGNING_KEY PRIVATE_FEED=1 KUVO_BOX_SPREADS=1 SIGN_IDENTITY="$IDENTITY" APP_ID="$APP_ID" VERSION="$VERSION" OUT="$work/build" FEED_URL="$url" UPDATE_PUBLIC_KEY="$PUBLIC" \
            BOX_URL="$base/box-$VERSION.tar.gz" ./scripts/build-desktop-app.sh
    else
        # The box releases' key, as every signed build reads it (never on a command line): without
        # it the box on your Mac wouldn't take the release the app carries.
        BOX_KEY="$(security find-generic-password -s "Kuvo Code box signing key" -a kuvo-box -w | base64 -d)" || BOX_KEY=""
        case "$BOX_KEY" in *"PRIVATE KEY"*) ;; *) die "the box releases' key (\"Kuvo Code box signing key\") isn't in the keychain, or couldn't be read." ;; esac
        KUVO_BOX_SIGNING_KEY="$BOX_KEY" PRIVATE_FEED=1 KUVO_BOX_SPREADS=1 SIGN_IDENTITY="$IDENTITY" APP_ID="$APP_ID" VERSION="$VERSION" OUT="$work/build" FEED_URL="$url" \
            UPDATE_PUBLIC_KEY="$PUBLIC" BOX_URL="$base/box-$VERSION.tar.gz" ./scripts/build-desktop-app.sh
    fi
    APP="$work/build/Kuvo.app"

    # The disk image, as the release makes it, and its feed item signed with the feed's key. The
    # item is for every copy reading this feed (no beta channel: a copy with betas off still gets
    # it) and links to no release page (there's none).
    DMG="Kuvo-$VERSION.dmg"
    APP="$APP" DMG="$work/$DMG" SIGN_IDENTITY="$IDENTITY" ./scripts/build-dmg.sh
    commit=$(git rev-parse --short HEAD)
    [ -z "$(git status --porcelain --untracked-files=no)" ] || commit="$commit with changes not committed"
    printf 'Kuvo %s for your own Mac, built from %s.\n' "$VERSION" "$commit" >"$work/notes.md"
    # (The feed's key, read here: never an empty one, which appcast.sh would take as "sign with
    # Sparkle's own key from the keychain", the real one on a Mac that has it.)
    feed_key=$(private_key) || die "the feed's update key couldn't be read from the keychain."
    [ -n "$feed_key" ] || die "the feed's update key in the keychain is empty."
    SPARKLE_PRIVATE_KEY="$feed_key" ./scripts/appcast.sh "$VERSION" "$work/$DMG" "$base/$DMG" "$work/notes.md" >"$work/signed.xml"
    feed_key=""
    sed -e '/<sparkle:channel>/d' -e '/<sparkle:fullReleaseNotesLink>/d' "$work/signed.xml" >"$work/item.xml"
    python3 scripts/merge-appcast.py "$work/item.xml" "$FEED_DIR/appcast.xml" >"$work/appcast.xml"
    [ "$(sed -n 's|.*<sparkle:shortVersionString>\(.*\)</sparkle:shortVersionString>.*|\1|p' "$work/appcast.xml" | head -n 1)" = "$VERSION" ] ||
        die "the feed written doesn't offer $VERSION."

    # Publish the files before the two feed manifests that name them.
    release="$APP/Contents/Resources/box-release"
    cp "$release/boxes.json" "$FEED_DIR/boxes.json.new"
    cp "$work/$DMG" "$FEED_DIR/$DMG.partial" && mv "$FEED_DIR/$DMG.partial" "$FEED_DIR/$DMG"
    cp "$release/box-$VERSION.tar.gz" "$FEED_DIR/box-$VERSION.tar.gz.partial"
    mv "$FEED_DIR/box-$VERSION.tar.gz.partial" "$FEED_DIR/box-$VERSION.tar.gz"
    mv "$FEED_DIR/boxes.json.new" "$FEED_DIR/boxes.json"
    mv "$work/appcast.xml" "$FEED_DIR/appcast.xml.new" && mv "$FEED_DIR/appcast.xml.new" "$FEED_DIR/appcast.xml"

    # Only the newest few builds stay (a copy downloading the last one when this lands still finds it).
    for old in $(past_keep); do
        rm -f "$FEED_DIR/Kuvo-$old.dmg" "$FEED_DIR/box-$old.tar.gz"
        say "Removed $old from the feed's folder."
    done
    # (The server's log, kept small.)
    if [ -f "$ROOT/server.log" ] && [ "$(wc -c <"$ROOT/server.log")" -gt 1000000 ]; then : >"$ROOT/server.log"; fi

    say "In the private feed: Kuvo $VERSION ($url)."
    say "Signed box release: $base/boxes.json"
    say "Your copy finds it at its next check, or now with Kuvo › Check for Updates…"
    say "A first install, on your MacBook: $base/$DMG"
}

status() {
    say "Folder: $ROOT"
    if [ -f "$ROOT/feed-url" ]; then say "Feed: $(feed_url)"; else say "Feed: none (scripts/private-feed.sh setup)"; fi
    say "Newest: $(newest_in_feed)"
    ls -l "$FEED_DIR" 2>/dev/null || true
    if has_key; then say "Key: \"$KEY_SERVICE\" in the keychain, public half $(public_key)"; else say "Key: none"; fi
    if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/"; then say "Served on 127.0.0.1:$PORT."; else say "Not served on 127.0.0.1:$PORT."; fi
    "$TAILSCALE" serve status 2>/dev/null || true
}

uninstall() {
    if [ -z "$TEST" ]; then launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true; fi
    rm -f "$AGENT"
    rm -rf "$ROOT"
    if [ "${1:-}" = --key ]; then
        if has_key; then security delete-generic-password -s "$KEY_SERVICE" -a "$KEY_ACCOUNT" ${KEYCHAIN:+"$KEYCHAIN"} >/dev/null; fi
        say "Removed the feed's key: a copy built for this feed takes no update from a new one."
    elif has_key; then
        say "Kept the feed's key (\"$KEY_SERVICE\"), which your copy trusts: a new setup uses it again. --key removes it."
    fi
    say "Removed the login agent and $ROOT. To stop serving it on the tailnet:"
    say "  $TAILSCALE serve --https=$TAILNET_PORT off"
    say "A copy built for it keeps reading the feed's address, and finds nothing there."
}

# The proof (see the top). Everything in one temporary folder, and the test ID's defaults and
# caches, go when it ends; the copy it opens is quit by its PID.
dry_run() {
    . scripts/real-app-guard.sh
    ID=run.kuvo.code.privatefeedtest
    real_app_guard scripts/private-feed.sh "$ID"
    temporary=${TMPDIR:-/tmp}
    test_dir=$(mktemp -d "${temporary%/}/kuvo-private-feed-test.XXXXXXXX")
    test_dir=$(cd "$test_dir" && pwd -P)
    SERVER="" APP_PID=""
    finish() {
        set +e
        [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null && wait "$APP_PID" 2>/dev/null
        [ -n "$SERVER" ] && kill "$SERVER" 2>/dev/null && wait "$SERVER" 2>/dev/null
        security delete-keychain "$test_dir/test.keychain-db" 2>/dev/null
        defaults delete "$ID" >/dev/null 2>&1
        for kept in "Caches/$ID" "HTTPStorages/$ID" "HTTPStorages/$ID.binarycookies" "Saved Application State/$ID.savedState" "WebKit/$ID" "Preferences/$ID.plist"; do
            rm -rf "$HOME/Library/$kept"
        done
        rm -rf "$test_dir"
    }
    trap finish EXIT
    trap 'exit 130' INT TERM HUP
    run() { env PRIVATE_FEED_TEST="$test_dir" PORT="$PORT" "$SELF" "$@"; }

    # A keychain of its own (never on the search list), unlocked for the test.
    keychains_before=$(security list-keychains -d user)
    password=$("$NODE" -e 'process.stdout.write(require("node:crypto").randomBytes(18).toString("base64"))')
    printf 'create-keychain -p "%s" "%s"\nunlock-keychain -p "%s" "%s"\n' "$password" "$test_dir/test.keychain-db" "$password" "$test_dir/test.keychain-db" | security -i
    [ "$(security list-keychains -d user)" = "$keychains_before" ] || die "the test keychain went on the search list."
    # (No locking after a while idle: the builds take longer than its default.)
    security set-keychain-settings "$test_dir/test.keychain-db"

    run setup
    grep -q "<string>$test_dir/root/feed</string>" "$test_dir/$LABEL.plist" || die "the login agent doesn't serve the feed's folder."
    # The login agent's own command, by its PID (python's exec keeps it), as launchd would start it.
    python3 -I -c 'import os, plistlib, sys; args = plistlib.load(open(sys.argv[1], "rb"))["ProgramArguments"]; os.execv(args[0], args)' \
        "$test_dir/$LABEL.plist" >"$test_dir/server.log" 2>&1 &
    SERVER=$!

    OLD=9.0.0-beta.1
    NEW=9.0.0-beta.2
    run build "$OLD"
    # (Its server, still up: it took the port, and nothing else answers there.)
    kill -0 "$SERVER" 2>/dev/null || { cat "$test_dir/server.log" >&2; die "the login agent's command didn't keep serving on 127.0.0.1:$PORT."; }
    curl -fsS -o /dev/null "http://127.0.0.1:$PORT/appcast.xml" || die "the feed isn't served on 127.0.0.1:$PORT."
    check_box_release() {
        base="http://127.0.0.1:$PORT"
        curl -fsS -o "$test_dir/boxes.json" "$base/boxes.json"
        curl -fsS -o "$test_dir/pack.tgz" "$base/box-$1.tar.gz"
        "$NODE" --input-type=module - "$test_dir/boxes.json" "$test_dir/pack.tgz" "$1" "$base" <<'EOF'
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";

const [manifestFile, packFile, version, base] = process.argv.slice(2);
const envelope = JSON.parse(fs.readFileSync(manifestFile, "utf8"));
const bytes = Buffer.from(envelope.manifest, "base64");
const manifest = JSON.parse(bytes.toString("utf8"));
const key = crypto.createPublicKey({ key: Buffer.concat([
    Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(envelope.key, "base64"),
]), format: "der", type: "spki" });
assert.ok(crypto.verify(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), bytes]), key, Buffer.from(envelope.signature, "base64")));
assert.equal(manifest.version, version);
assert.equal(manifest.spreads, true);
assert.equal(manifest.pack.url, `${base}/box-${version}.tar.gz`);
assert.equal(manifest.pack.sha256, crypto.createHash("sha256").update(fs.readFileSync(packFile)).digest("hex"));
EOF
    }
    check_box_release "$OLD"
    # (Not a beta, or not newer than the feed's: refused before anything is built.)
    if run build 9.1.0 2>"$test_dir/refused"; then die "a build that isn't a beta was taken."; fi
    grep -q "is a beta" "$test_dir/refused" || { cat "$test_dir/refused" >&2; die "a build that isn't a beta wasn't refused as such."; }
    # (Not newer than the feed's: refused before anything is built.)
    if run build "$OLD" 2>"$test_dir/refused"; then die "a build no newer than the feed's was taken."; fi
    grep -q "isn't newer" "$test_dir/refused" || { cat "$test_dir/refused" >&2; die "a build no newer than the feed's wasn't refused as such."; }
    # The old one, as it was built, is the installed copy.
    mkdir "$test_dir/Applications"
    curl -fsS -o "$test_dir/old.dmg" "http://127.0.0.1:$PORT/Kuvo-$OLD.dmg"
    mkdir "$test_dir/mount"
    hdiutil attach -nobrowse -readonly -mountpoint "$test_dir/mount" "$test_dir/old.dmg" >/dev/null
    ditto "$test_dir/mount/Kuvo.app" "$test_dir/Applications/Kuvo.app"
    hdiutil detach "$test_dir/mount" >/dev/null
    # The next: its version found from the feed's.
    cp "$test_dir/root/feed/appcast.xml" "$test_dir/old-appcast.xml"
    run build
    feed="$test_dir/root/feed"
    echo "The feed:"
    cat "$feed/appcast.xml"
    grep -q "<sparkle:shortVersionString>$NEW<" "$feed/appcast.xml" || die "the feed doesn't offer $NEW."
    [ "$(grep -c '<item>' "$feed/appcast.xml")" = 1 ] || die "the feed holds more than its newest build."
    ! grep -q '<sparkle:channel>' "$feed/appcast.xml" || die "the feed's item is in a channel some copies don't read."
    [ ! -e "$feed/Kuvo-$OLD.dmg" ] && [ ! -e "$feed/box-$OLD.tar.gz" ] || die "the feed's folder kept more than its newest builds."
    [ -f "$feed/Kuvo-$NEW.dmg" ] && [ -f "$feed/box-$NEW.tar.gz" ] || die "the feed's folder has no $NEW."
    check_box_release "$NEW"
    # A publish interrupted between the two manifests must not rebuild the exposed box version.
    cp "$feed/appcast.xml" "$test_dir/new-appcast.xml"
    cp "$test_dir/old-appcast.xml" "$feed/appcast.xml"
    if run build "$NEW" 2>"$test_dir/refused"; then die "a published box version was rebuilt."; fi
    grep -q "isn't newer" "$test_dir/refused" || die "the newer box manifest did not prevent reuse of its version."
    mv "$test_dir/new-appcast.xml" "$feed/appcast.xml"
    check_box_release "$NEW"
    ls "$feed"

    INSTALLED="$test_dir/Applications/Kuvo.app"
    version_of() { plutil -extract CFBundleShortVersionString raw -o - "$1/Contents/Info.plist"; }
    [ "$(version_of "$INSTALLED")" = "$OLD" ] || die "the installed copy isn't $OLD."
    # Waits on the file system's word (no clock) for a line in a log or a version in a bundle.
    cat >"$test_dir/wait.py" <<'EOF'
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
    defaults write "$ID" SUAutomaticallyUpdate -bool YES
    : >"$test_dir/app.log"
    env -i HOME="$HOME" PATH="/usr/bin:/bin:/usr/sbin:/sbin" TMPDIR="${TMPDIR:-/tmp}" USER="${USER:-}" "$INSTALLED/Contents/MacOS/kuvo" >>"$test_dir/app.log" 2>&1 &
    APP_PID=$!
    echo "Opened the $OLD copy ($APP_PID), reading the private feed."
    python3 -I "$test_dir/wait.py" log "$test_dir/app.log" "installs when Kuvo quits" 300 || { cat "$test_dir/app.log" >&2; exit 1; }
    cat "$test_dir/app.log"
    kill "$APP_PID"
    wait "$APP_PID" 2>/dev/null || true
    APP_PID=""
    echo "Quit the $OLD copy; Sparkle installs $NEW."
    python3 -I "$test_dir/wait.py" version "$test_dir/Applications" "$NEW" 180 || die "the copy is still $(version_of "$INSTALLED")."
    codesign --verify --deep --strict "$INSTALLED"
    ./scripts/check-desktop-app.sh "$INSTALLED"
    grep -q "GET /appcast.xml" "$test_dir/server.log" && grep -q "GET /Kuvo-$NEW.dmg" "$test_dir/server.log" ||
        die "the copy didn't read the feed and its disk image from the login agent's server."
    run uninstall --key >/dev/null
    [ ! -e "$test_dir/root" ] && ! security find-generic-password -s "$KEY_SERVICE" -a "$KEY_ACCOUNT" "$test_dir/test.keychain-db" >/dev/null 2>&1 ||
        die "uninstall left the folder or the key."
    echo "Updated: the $OLD copy found $NEW in the private feed, checked it with the feed's key, and installed it over itself; uninstall took the folder and the key."
}

case "${1:-}" in
    setup) setup ;;
    build) shift; [ $# -le 1 ] || die "build takes one version at most."; build "$@" ;;
    status) status ;;
    uninstall) shift; [ $# -eq 0 ] || [ "$1" = --key ] || die "uninstall takes --key at most."; uninstall "$@" ;;
    dry-run)
        [ -z "$TEST" ] || die "dry-run runs on its own."
        dry_run
        ;;
    *) awk 'NR > 1 && /^set -eu/ { exit } NR > 1' "$0" >&2; exit 2 ;;
esac
