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
# ALWAYS signed ad hoc, here and in everything it carries: a release's signing (a Developer ID,
# the hardened runtime's timestamp), notarization, the update feed and the disk image are the
# release's own step, never this script's (.claude/notes/713-p6a-bundle.md says what's left).
#   scripts/build-desktop-app.sh
#   VERSION     the version to stamp (default 0.0.0: a local build); a beta's is like 0.8.0-beta.1.
#               A box takes the release an app carries only when it's newer than itself.
#   APP_ID      the bundle ID (default run.kuvo.code, Kuvo Code's own: such a bundle installs into
#               the real ~/.kuvo-code when it's opened, so never open it on a Mac in use). A copy
#               of its own (run.kuvo.code.trial1) has its helper and kuvo-keys named after it too,
#               and installs only into a root it's given (KUVO_DEMO_MACHINE_ROOT=<a folder's name>).
#   OUT         where the bundle goes (default build)
#   ZIP         1: <OUT>/Kuvo-<version>.zip too, to carry it to another Mac or account as it is
#   KUVO_BOX_SIGNING_KEY   the box releases' key (PEM). Without it, the release is signed with a
#               key made for this build and kept nowhere: the app installs and moves from it all
#               the same (the installer doesn't check it: a Mac's app seals the folder), but a box
#               that's there already won't take it as an update.
#   BOX_URL     where the pack is published (default: this version's GitHub release)
#   BUN, NODE   as scripts/box-release.sh takes them
#   MIN_SYSTEM  the oldest macOS it runs on (default 26.0)
# Needs Xcode (the helper, the icon), Rust as gpui/README.md sets it up, Bun and Node. It's a
# heavy build: run it through scripts/heavy.sh mac-build. When it's built it's checked
# (scripts/check-desktop-app.sh), without being run.
set -eu
cd "$(dirname "$0")/.."

if [ $# -ne 0 ]; then
    sed -n '2,33p' "$0" >&2
    exit 2
fi
if [ "${SIGN_IDENTITY:--}" != "-" ]; then
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
NEW="$APP.new"
trap 'rm -rf "$work" "$NEW"' EXIT
rm -rf "$NEW"

# The app, saying the bundle's version where it names its own (to the relay and your computers).
(cd gpui && KUVO_APP_VERSION="$VERSION" cargo build --release --locked -p kuvo --bin kuvo)
BUILT="${CARGO_TARGET_DIR:-gpui/target}/release/kuvo"
case "$BUILT" in /*) ;; *) [ -z "${CARGO_TARGET_DIR:-}" ] || BUILT="gpui/$BUILT" ;; esac
test -x "$BUILT" || { echo "No kuvo at $BUILT" >&2; exit 1; }

# Kuvo Code's icon, as Xcode made it in the Mac app: the .icns, and the layered one macOS 26 draws.
mkdir "$work/icon"
xcrun actool swift/KuvoCode/AppIcon.icon --compile "$work/icon" --app-icon AppIcon --platform macosx \
    --minimum-deployment-target "$MIN_SYSTEM" --output-partial-info-plist "$work/icon.plist" >"$work/actool" || { cat "$work/actool" >&2; exit 1; }
test -s "$work/icon/AppIcon.icns" || { echo "actool made no AppIcon.icns from swift/KuvoCode/AppIcon.icon." >&2; exit 1; }

# Kuvo Helper, for the box's release, and kuvo-keys: each signed ad hoc, as its own.
env -u HELPER_EXECUTABLE -u KEYS_EXECUTABLE -u KUVO_SIGNED_KEYS SIGN_IDENTITY=- APP_ID="$APP_ID" VERSION="$VERSION" BUILD="$BUILD" \
    MIN_SYSTEM="$MIN_SYSTEM" ICON="$work/icon/AppIcon.icns" ./scripts/build-helper.sh "$work/helper"

mkdir -p "$NEW/Contents/MacOS" "$NEW/Contents/Resources"
cp "$BUILT" "$NEW/Contents/MacOS/kuvo"
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
plutil -lint "$INFO" >/dev/null

# The box's release, from this checkout's own sources, with the helper in its pack. Signed with
# the box releases' key where it's given; else with one made here and gone with this build.
if [ -z "${KUVO_BOX_SIGNING_KEY:-}" ]; then
    KUVO_BOX_SIGNING_KEY=$("$NODE" -e 'process.stdout.write(require("node:crypto").generateKeyPairSync("ed25519").privateKey.export({ type: "pkcs8", format: "pem" }))')
    echo "No KUVO_BOX_SIGNING_KEY: the box's release is signed with a key made for this build, which no box trusts for an update."
fi
env -u BOX_SOURCES KUVO_BOX_SIGNING_KEY="$KUVO_BOX_SIGNING_KEY" NODE="$NODE" ./scripts/box-release.sh "$VERSION" "$BOX_URL" "$NEW/Contents/Resources/box-release" "$work/helper/Kuvo Helper.app"

# The app over all of it (kuvo-keys is signed already, as Kuvo Code itself to the Keychain: its
# identifier, and no entitlements). The app asks for none either: what macOS asks about is asked
# of Kuvo Helper.
codesign --force --sign - --options runtime --identifier "$APP_ID" "$NEW"

rm -rf "$APP"
mv "$NEW" "$APP"
./scripts/check-desktop-app.sh "$APP"
if [ "${ZIP:-}" = 1 ]; then
    rm -f "$OUT/Kuvo-$VERSION.zip"
    ditto -c -k --keepParent "$APP" "$OUT/Kuvo-$VERSION.zip"
    echo "Packed $OUT/Kuvo-$VERSION.zip"
fi
echo "Built $APP $VERSION ($APP_ID, signed ad hoc, $(du -sh "$APP" | cut -f1 | tr -d " "))"
if [ "$APP_ID" = run.kuvo.code ]; then
    echo "It's Kuvo Code itself: opened, it sets up or moves into the real ~/.kuvo-code. Never open it on a Mac in use."
fi
