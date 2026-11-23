#!/bin/sh
# Builds the desktop app (gpui/) as a Mac app bundle, <OUT>/Kuvo.app, for trying on a Mac: the
# app's program, Kuvo Code's icon, and what it sets this Mac up from as it opens
# (gpui/kuvo_box/src/install.rs; protocol/PROTOCOL.md, "Installing this computer's box"):
#   Contents/MacOS/kuvo                    the app (cargo build --release -p kuvo)
#   Contents/MacOS/kuvo-keys               the one-time hand-over of Kuvo Code 0.7's Keychain items,
#                                          which only the box's move runs (scripts/build-helper.sh)
#   Contents/Resources/box-release/        the box's release (scripts/box-release.sh): boxes.json,
#                                          box-<version>.tar.gz with Kuvo Helper in it
#                                          (bin/Kuvo Helper.app), and kuvo-install beside them
#   Contents/Frameworks/Sparkle.framework  the app's updater, as 0.7 had it (scripts/sparkle.sh's
#                                          pinned release, trimmed as 0.7's was), with its feed and
#                                          key in Info.plist: SUFeedURL, SUPublicEDKey
# ALWAYS signed ad hoc, here and in everything it carries (scripts/sign-desktop-app.sh, inside
# out): a release's signing with a Developer ID is scripts/sign-desktop-app.sh again, then
# notarization, the disk image and the update feed: the release's own steps (release.yml), never
# this script's.
#   scripts/build-desktop-app.sh
#   VERSION     the version to stamp (default 0.0.0: a local build); a beta's is like 0.8.0-beta.1.
#               A box takes the release an app carries only when it's newer than itself.
#   APP_ID      the bundle ID (default run.kuvo.code, Kuvo Code's own: such a bundle installs into
#               the real ~/.kuvo-code when it's opened, and `kuvo --install-status` from it stages
#               its release there, so neither on a Mac in use; and macOS may open it for
#               anything that asks for Kuvo Code by its ID, so don't keep it where Finder or
#               Spotlight look: carry the zip to where it's tried, and remove the bundle). A copy
#               of its own (run.kuvo.code.trial1) has its helper and kuvo-keys named after it too,
#               and installs only into a root it's given (KUVO_DEMO_MACHINE_ROOT=<a folder's name>).
#   OUT         where the bundle goes (default build)
#   ZIP         1: <OUT>/Kuvo-<version>.zip too, to carry it to another Mac or account as it is
#   KUVO_BOX_SIGNING_KEY   the box releases' key (PEM). Without it, the release is signed with a
#               key made for this build and kept nowhere: the app installs and moves from it all
#               the same (the installer doesn't check it: a Mac's app seals the folder), but a box
#               that's there already won't take it as an update.
#   BOX_URL     where the pack is published (default: this version's GitHub release)
#   BOX_RELEASE a box release packed already (boxes.json, box-<version>.tar.gz and kuvo-install in a
#               folder: the release workflow's, with Kuvo Helper signed and notarized in it), carried
#               as it is, so the app carries the very files published beside it (the helper built
#               here goes in none).
#   FEED        1: the app reads Kuvo Code's own update feed, with its own key (which 0.7 reads
#               too), and updates itself from the releases. Only for run.kuvo.code, and only the
#               release asks for it (release.yml). Without it the app carries Sparkle all the same
#               but names no feed, so it never updates itself and says so: a local build, a demo
#               copy or a test's is never offered a real release (0.7's own builds had none either).
#   FEED_URL, UPDATE_PUBLIC_KEY   a feed of its own and the EdDSA key its updates are signed with,
#               both, for a copy of its own (APP_ID not run.kuvo.code): a test's
#               (scripts/try-desktop-update.sh), never the real app's.
#   PRIVATE_FEED 1: the user's own Kuvo Code (run.kuvo.code) before the first release with a feed,
#               reading a private feed of its own (scripts/private-feed.sh, the only one to ask):
#               FEED_URL (https) and UPDATE_PUBLIC_KEY, never the real ones, and SIGN_IDENTITY, a
#               real identity (Apple Development) the helper, kuvo-keys and the app are signed with.
#               For a copy of its own (private-feed.sh dry-run), any FEED_URL, and ad hoc or not.
#   BUN, NODE   as scripts/box-release.sh takes them
#   MIN_SYSTEM  the oldest macOS it runs on (default 26.0)
# Needs Xcode (the helper, the icon), Rust as gpui/README.md sets it up, Bun and Node. It's a
# heavy build: run it through scripts/heavy.sh mac-build. When it's built it's checked
# (scripts/check-desktop-app.sh), without being run.
set -eu
# (A release given as a path from where this was run, before going to the checkout.)
if [ -n "${BOX_RELEASE:-}" ]; then
    BOX_RELEASE=$(cd "$BOX_RELEASE" && pwd -P)
fi
cd "$(dirname "$0")/.."

if [ $# -ne 0 ]; then
    awk 'NR > 1 && /^set -eu/ { exit } NR > 1' "$0" >&2
    exit 2
fi
# The box releases' key, out of the environment at once: only scripts/box-release.sh is given it,
# never cargo, the helper's build or anything else this runs.
BOX_SIGNING_KEY="${KUVO_BOX_SIGNING_KEY:-}"
unset KUVO_BOX_SIGNING_KEY
IDENTITY="${SIGN_IDENTITY:--}"
if [ "${PRIVATE_FEED:-}" = 1 ]; then
    if [ "$IDENTITY" = "-" ] && [ "${APP_ID:-run.kuvo.code}" = run.kuvo.code ]; then
        echo "A private feed's build of Kuvo Code itself is signed with a real identity: SIGN_IDENTITY (scripts/private-feed.sh)." >&2
        exit 2
    fi
elif [ "$IDENTITY" != "-" ]; then
    echo "scripts/build-desktop-app.sh signs ad hoc only: signing with an identity is the release's step." >&2
    exit 2
fi
[ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] || { echo "The desktop app's bundle is built on a Mac with Apple silicon." >&2; exit 1; }
VERSION="${VERSION:-0.0.0}"
BUILD=$(./scripts/build-version.sh "$VERSION")
APP_ID="${APP_ID:-run.kuvo.code}"
case "$APP_ID" in
    *[!A-Za-z0-9.-]* | "" | .* | *.box) echo "Not a bundle ID for Kuvo Code: $APP_ID" >&2; exit 2 ;;
esac
# Updates: Sparkle reads the feed from the newest GitHub release and installs only updates signed
# with the private half of this key, the one every Kuvo app shares (0.7 read the same feed with the
# same key, so it updates into this app). scripts/set-update-key.sh uploads it for the release.
REAL_FEED_URL="https://github.com/serhatandic/kuvo-code/releases/latest/download/appcast.xml"
REAL_UPDATE_PUBLIC_KEY="uG/YbzL4kf+DAYCiTTfK8ztEOBo96hkEAuKk9rw9BE4="
if [ "${PRIVATE_FEED:-}" = 1 ]; then
    # (The user's own copy, before the first release with a feed: its own feed and key, both.)
    # (A copy of its own takes this path too: scripts/private-feed.sh dry-run's, over http on 127.0.0.1.)
    [ "${FEED:-}" != 1 ] || { echo "PRIVATE_FEED and FEED are one or the other." >&2; exit 2; }
    if [ "$APP_ID" = run.kuvo.code ]; then
        case "${FEED_URL:-}" in https://?*) ;; *) echo "A private feed is read over https: FEED_URL." >&2; exit 2 ;; esac
    else
        [ -n "${FEED_URL:-}" ] || { echo "A private feed takes its address: FEED_URL." >&2; exit 2; }
    fi
    [ -n "${UPDATE_PUBLIC_KEY:-}" ] || { echo "A private feed takes the key its updates are signed with: UPDATE_PUBLIC_KEY." >&2; exit 2; }
    if [ "$UPDATE_PUBLIC_KEY" = "$REAL_UPDATE_PUBLIC_KEY" ] || [ "$FEED_URL" = "$REAL_FEED_URL" ]; then
        echo "A private feed is never Kuvo Code's own feed or key: that's FEED=1." >&2
        exit 2
    fi
elif [ "$APP_ID" = run.kuvo.code ]; then
    if [ -n "${FEED_URL:-}" ] || [ -n "${UPDATE_PUBLIC_KEY:-}" ]; then
        echo "Kuvo Code itself reads only its own feed, with its own key: FEED_URL and UPDATE_PUBLIC_KEY are for a copy of its own (APP_ID)." >&2
        exit 2
    fi
    if [ "${FEED:-}" = 1 ]; then FEED_URL=$REAL_FEED_URL UPDATE_PUBLIC_KEY=$REAL_UPDATE_PUBLIC_KEY; fi
else
    if [ "${FEED:-}" = 1 ]; then
        echo "Only Kuvo Code itself reads its own feed: a copy of its own (APP_ID) names one of its own, with FEED_URL and UPDATE_PUBLIC_KEY." >&2
        exit 2
    fi
    if { [ -n "${FEED_URL:-}" ] && [ -z "${UPDATE_PUBLIC_KEY:-}" ]; } || { [ -z "${FEED_URL:-}" ] && [ -n "${UPDATE_PUBLIC_KEY:-}" ]; }; then
        echo "A copy's feed takes both FEED_URL and UPDATE_PUBLIC_KEY." >&2
        exit 2
    fi
    if [ "${UPDATE_PUBLIC_KEY:-}" = "$REAL_UPDATE_PUBLIC_KEY" ] || [ "${FEED_URL:-}" = "$REAL_FEED_URL" ]; then
        echo "A copy of its own never reads Kuvo Code's own feed or trusts its key: they'd offer it the real app." >&2
        exit 2
    fi
fi
FEED_URL="${FEED_URL:-}"
UPDATE_PUBLIC_KEY="${UPDATE_PUBLIC_KEY:-}"
if [ -n "${BOX_RELEASE:-}" ]; then
    for name in boxes.json kuvo-install "box-$VERSION.tar.gz"; do
        test -f "$BOX_RELEASE/$name" || { echo "BOX_RELEASE has no $name for $VERSION: $BOX_RELEASE" >&2; exit 2; }
    done
fi
MIN_SYSTEM="${MIN_SYSTEM:-26.0}"
NODE="${NODE:-node}"
OUT_GIVEN="${OUT:-build}"
mkdir -p "$OUT_GIVEN"
OUT=$(cd "$OUT_GIVEN" && pwd -P)
APP="$OUT/Kuvo.app"
BOX_URL="${BOX_URL:-https://github.com/serhatandic/kuvo-code/releases/download/v$VERSION/box-$VERSION.tar.gz}"

# Rust where gpui/README.md keeps it, when it isn't on the PATH already.
if ! command -v cargo >/dev/null 2>&1; then
    export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.kuvo-code/tools/rustup}" CARGO_HOME="${CARGO_HOME:-$HOME/.kuvo-code/tools/cargo}"
    PATH="$CARGO_HOME/bin:$PATH"
    command -v cargo >/dev/null 2>&1 || { echo "scripts/build-desktop-app.sh needs Rust: gpui/README.md, Setting up." >&2; exit 1; }
fi
command -v "$NODE" >/dev/null 2>&1 || { echo "scripts/build-desktop-app.sh needs Node: install it, or name one with NODE=." >&2; exit 1; }

temporary=${TMPDIR:-/tmp}
work=$(mktemp -d "${temporary%/}/kuvo-desktop-app.XXXXXXXX")
work=$(cd "$work" && pwd -P)
NEW="$APP.new"
trap 'rm -rf "$work" "$NEW"' EXIT
# (Nor an earlier build's zip of this version, to be carried in this one's place.)
rm -rf "$NEW" "$OUT/Kuvo-$VERSION.zip"

# Generate the box's bundles and vectors before compiling the app.
./scripts/build-built.sh

# Snapshot the versioned executable while still holding the Rust slot, before another build
# can replace it. Ask Cargo where it built, including when the caller sets a target directory.
(
    cd gpui
    # shellcheck disable=SC2016 # Expand arguments inside the locked shell.
    KUVO_APP_VERSION="$VERSION" ../scripts/heavy.sh rust sh -ec '
        cargo build --release --locked -p kuvo --bin kuvo
        target=$(cargo metadata --format-version 1 --no-deps --locked | "$1" -e "$3")
        test -x "$target/release/kuvo" || { echo "No kuvo at $target/release/kuvo" >&2; exit 1; }
        cp "$target/release/kuvo" "$2"
    ' sh "$NODE" "$work/kuvo" 'process.stdout.write(JSON.parse(require("node:fs").readFileSync(0, "utf8")).target_directory)'
)

# Kuvo Code's icon, as Xcode made it in the Mac app: the .icns, and the layered one macOS 26 draws.
mkdir "$work/icon"
xcrun actool mac/AppIcon.icon --compile "$work/icon" --app-icon AppIcon --platform macosx \
    --minimum-deployment-target "$MIN_SYSTEM" --output-partial-info-plist "$work/icon.plist" >"$work/actool" || { cat "$work/actool" >&2; exit 1; }
test -s "$work/icon/AppIcon.icns" || { echo "actool made no AppIcon.icns from mac/AppIcon.icon." >&2; exit 1; }

# Kuvo Helper, for the box's release, and kuvo-keys: each signed ad hoc, as its own.
# (A private feed's build: both signed with its identity, kuvo-keys kept, as the user's copy needs it.)
if [ "$IDENTITY" = "-" ]; then SIGNED_KEYS=""; else SIGNED_KEYS=1; fi
env -u HELPER_EXECUTABLE -u KEYS_EXECUTABLE KUVO_SIGNED_KEYS="$SIGNED_KEYS" SIGN_IDENTITY="$IDENTITY" APP_ID="$APP_ID" VERSION="$VERSION" BUILD="$BUILD" \
    ICON="$work/icon/AppIcon.icns" ./scripts/build-helper.sh "$work/helper"
# Sparkle, as this checkout pins it.
SPARKLE=$(./scripts/sparkle.sh)

mkdir -p "$NEW/Contents/MacOS" "$NEW/Contents/Resources"
cp "$work/kuvo" "$NEW/Contents/MacOS/kuvo"
strip -rSTx "$NEW/Contents/MacOS/kuvo"
cp "$work/helper/kuvo-keys" "$NEW/Contents/MacOS/kuvo-keys"
cp "$work/icon/AppIcon.icns" "$NEW/Contents/Resources/AppIcon.icns"
[ ! -f "$work/icon/Assets.car" ] || cp "$work/icon/Assets.car" "$NEW/Contents/Resources/Assets.car"
# What the app is made with, and under which terms, as the Mac app carried them.
cp LICENSE NOTICE THIRD_PARTY_LICENSES.txt "$NEW/Contents/Resources/"
cp gpui/LICENSES-DEPS.md "$NEW/Contents/Resources/LICENSES-DEPS.md"

INFO="$NEW/Contents/Info.plist"
cp gpui/kuvo/mac/Info.plist "$INFO"
put() { plutil -insert "$1" "-$2" "$3" "$INFO"; }
put CFBundleIdentifier string "$APP_ID"
put CFBundleShortVersionString string "$VERSION"
put CFBundleVersion string "$BUILD"
put LSMinimumSystemVersion string "$MIN_SYSTEM"
put CFBundleIconFile string AppIcon
[ ! -f "$NEW/Contents/Resources/Assets.car" ] || put CFBundleIconName string AppIcon
# Its updates, as 0.7's release's, where a feed is named: checked once a day from the start (no
# question first), and an update checked against the key before it's unpacked.
if [ -n "$FEED_URL" ]; then
    put SUFeedURL string "$FEED_URL"
    put SUPublicEDKey string "$UPDATE_PUBLIC_KEY"
    put SUEnableAutomaticChecks bool true
    put SUVerifyUpdateBeforeExtraction bool true
fi
plutil -lint "$INFO" >/dev/null

# Sparkle, trimmed to what the app uses, as 0.7's was: Apple silicon only, no XPC services (only
# sandboxed apps need them), no headers, English only. (Signed again below, inside out.)
mkdir -p "$NEW/Contents/Frameworks"
ditto "$SPARKLE/Sparkle.framework" "$NEW/Contents/Frameworks/Sparkle.framework"
(
    cd "$NEW/Contents/Frameworks/Sparkle.framework"
    rm -rf XPCServices Headers PrivateHeaders Modules \
        Versions/B/XPCServices Versions/B/Headers Versions/B/PrivateHeaders Versions/B/Modules
    find Versions/B/Resources -name '*.lproj' ! -name 'Base.lproj' ! -name 'en.lproj' -exec rm -rf {} +
    for binary in Versions/B/Sparkle Versions/B/Autoupdate Versions/B/Updater.app/Contents/MacOS/Updater; do
        if lipo "$binary" -verify_arch x86_64 2>/dev/null; then
            lipo "$binary" -thin arm64 -output "$binary.arm64" && mv "$binary.arm64" "$binary"
        fi
    done
)

# The box's release, from this checkout's own sources, with the helper in its pack. Signed with
# the box releases' key where it's given; else with one made here and gone with this build.
if [ -n "${BOX_RELEASE:-}" ]; then
    mkdir "$NEW/Contents/Resources/box-release"
    cp -p "$BOX_RELEASE/boxes.json" "$BOX_RELEASE/kuvo-install" "$BOX_RELEASE/box-$VERSION.tar.gz" "$NEW/Contents/Resources/box-release/"
else
    if [ -z "$BOX_SIGNING_KEY" ]; then
        BOX_SIGNING_KEY=$("$NODE" -e 'process.stdout.write(require("node:crypto").generateKeyPairSync("ed25519").privateKey.export({ type: "pkcs8", format: "pem" }))')
        echo "No KUVO_BOX_SIGNING_KEY: the box's release is signed with a key made for this build, which no box trusts for an update."
    fi
    env -u BOX_SOURCES KUVO_BOX_SIGNING_KEY="$BOX_SIGNING_KEY" NODE="$NODE" ./scripts/box-release.sh "$VERSION" "$BOX_URL" "$NEW/Contents/Resources/box-release" "$work/helper/Kuvo Helper.app"
fi

# Signed inside out: Sparkle's programs and the framework, kuvo-keys (as Kuvo Code itself to the
# Keychain), and the app over all of it.
KUVO_SIGNED_KEYS="$SIGNED_KEYS" SIGN_IDENTITY="$IDENTITY" ./scripts/sign-desktop-app.sh "$NEW"

# Checked before it takes the last build's place: one that doesn't pass is never left there.
PRIVATE_FEED="${PRIVATE_FEED:-}" ./scripts/check-desktop-app.sh "$NEW"
rm -rf "$APP"
mv "$NEW" "$APP"
if [ "${ZIP:-}" = 1 ]; then
    ditto -c -k --keepParent "$APP" "$OUT/Kuvo-$VERSION.zip"
    echo "Packed $OUT/Kuvo-$VERSION.zip"
fi
if [ "$IDENTITY" = "-" ]; then SIGNED_SAY="signed ad hoc"; else SIGNED_SAY="signed with $IDENTITY"; fi
echo "Built $APP $VERSION ($APP_ID, $SIGNED_SAY, $(du -sh "$APP" | cut -f1 | tr -d " "))"
if [ "$APP_ID" = run.kuvo.code ]; then
    echo "It's Kuvo Code itself: opened, it sets up or moves into the real ~/.kuvo-code. Never open it on a Mac in use."
fi
