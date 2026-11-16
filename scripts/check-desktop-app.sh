#!/bin/sh
# Checks a desktop app bundle scripts/build-desktop-app.sh built, without opening it: that it's
# signed whole, and carries what the app sets a Mac up from, each where the app and the box's
# installer look for it and as they check it.
#   scripts/check-desktop-app.sh <Kuvo.app>
#       Info.plist reads, and names its ID, version and program; the signature holds for the
#       bundle and everything in it (codesign --verify --deep --strict); kuvo-keys is there, signed
#       as the app; the box's release is whole: boxes.json's signature is its key's, its version
#       the app's, its pack the one it names (SHA-256), every file in the pack what the pack's
#       box.json says (as `kuvo-install` and the box check it), and Kuvo Helper in it still signed,
#       as <ID>.box.
#   scripts/check-desktop-app.sh --run <Kuvo.app>
#       That, then a dry run: a COPY of the bundle with another bundle ID (<ID>.check, signed ad
#       hoc again), in a home folder made for it, is asked what the computer holds
#       (`kuvo --install-status`: the app runs its `kuvo-install status`, and nothing after it).
#       A new home's answer is `fresh`, with `new` to run. Without a root of its own the copy must
#       refuse before any installer runs. The bundle itself is never run, and nothing is read or
#       written in your own home folder: the copy, its home and the engine `status` fetches (it
#       needs the network) are in a temporary folder that goes when this ends.
#   NODE   the Node to read the release with (default: node)
set -eu

RUN=0
if [ "${1:-}" = "--run" ]; then RUN=1; shift; fi
if [ $# -ne 1 ] || [ ! -d "$1/Contents" ]; then
    echo "Usage: scripts/check-desktop-app.sh [--run] <Kuvo.app>" >&2
    exit 2
fi
APP=$(cd "$1" && pwd -P)
cd "$(dirname "$0")/.."
NODE="${NODE:-node}"
fail() { echo "check-desktop-app: $1" >&2; exit 1; }

INFO="$APP/Contents/Info.plist"
plutil -lint "$INFO" >/dev/null || fail "its Info.plist doesn't read."
info() { plutil -extract "$1" raw -o - "$INFO" 2>/dev/null; }
ID=$(info CFBundleIdentifier) || fail "its Info.plist names no bundle ID."
VERSION=$(info CFBundleShortVersionString) || fail "its Info.plist names no version."
EXECUTABLE=$(info CFBundleExecutable) || fail "its Info.plist names no program."
test -x "$APP/Contents/MacOS/$EXECUTABLE" || fail "its program ($EXECUTABLE) isn't in it."
icon=$(info CFBundleIconFile) || fail "its Info.plist names no icon."
test -s "$APP/Contents/Resources/$icon.icns" || fail "its icon ($icon.icns) isn't in it."

codesign --verify --deep --strict "$APP" || fail "its signature doesn't hold."
signed() { codesign -d --verbose=2 "$1" 2>&1; }
identifier() { signed "$1" | sed -n 's/^Identifier=//p'; }
[ "$(identifier "$APP")" = "$ID" ] || fail "it's signed as $(identifier "$APP"), not as $ID."
if signed "$APP" | grep -q '^Signature=adhoc$'; then SIGNER="ad hoc"; else SIGNER=$(signed "$APP" | sed -n 's/^Authority=//p' | head -n 1); fi

# kuvo-keys, where the box's move looks for it (<bundle>/Contents/MacOS/kuvo-keys), as the app
# itself to the Keychain.
KEYS="$APP/Contents/MacOS/kuvo-keys"
test -x "$KEYS" || fail "it carries no kuvo-keys (Contents/MacOS/kuvo-keys): the move from Kuvo Code 0.7 couldn't hand over its Keychain items."
codesign --verify --strict "$KEYS" || fail "kuvo-keys's signature doesn't hold."
[ "$(identifier "$KEYS")" = "$ID" ] || fail "kuvo-keys is signed as $(identifier "$KEYS"), not as $ID."

# The box's release, where the app looks for it.
RELEASE="$APP/Contents/Resources/box-release"
for name in boxes.json kuvo-install "box-$VERSION.tar.gz"; do
    test -f "$RELEASE/$name" || fail "its release has no $name (Contents/Resources/box-release)."
done
sh -n "$RELEASE/kuvo-install" || fail "its kuvo-install isn't a shell script that reads."
temporary=${TMPDIR:-/tmp}
work=$(mktemp -d "${temporary%/}/kuvo-check-app.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir "$work/pack"
tar -xzf "$RELEASE/box-$VERSION.tar.gz" -C "$work/pack" || fail "its release's pack doesn't unpack."
PINNED=$(sed -n 's/^export const UPDATE_KEYS = \(.*\);$/\1/p' box/src/link/updates/update.ts)
[ -n "$PINNED" ] || fail "the keys boxes trust (UPDATE_KEYS, box/src/link/updates/update.ts) couldn't be read."
cat >"$work/release.mjs" <<'EOF'
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
const [release, pack, version, pinned] = process.argv.slice(2);
const fail = words => { console.error(`check-desktop-app: ${words}`); process.exit(1); };
const sha256 = file => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
// boxes.json, as a box reads it (PROTOCOL.md, "Updates"): the manifest, signed by the key it names.
const boxes = JSON.parse(fs.readFileSync(path.join(release, "boxes.json"), "utf8"));
const manifestBytes = Buffer.from(boxes.manifest, "base64");
const manifest = JSON.parse(manifestBytes);
const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(boxes.key, "base64url")]);
const key = crypto.createPublicKey({ key: spki, format: "der", type: "spki" });
if (!crypto.verify(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifestBytes]), key, Buffer.from(boxes.signature, "base64"))) fail("its release's boxes.json isn't signed by the key it names.");
if (manifest.version !== version) fail(`its release is ${manifest.version}'s, and the app is ${version}.`);
const packed = path.join(release, `box-${version}.tar.gz`);
if (manifest.pack.sha256 !== sha256(packed) || manifest.pack.size !== fs.statSync(packed).size) fail("its release's pack isn't the one boxes.json names.");
// The pack's own record, as `kuvo-link.js install` checks it before anything is made.
const box = JSON.parse(fs.readFileSync(path.join(pack, "box.json"), "utf8"));
if (box.version !== version || box.engine !== manifest.engine) fail("its release's box.json and boxes.json name different versions.");
for (const [name, sum] of Object.entries(box.files)) {
  if (!fs.lstatSync(path.join(pack, name), { throwIfNoEntry: false })?.isFile() || sha256(path.join(pack, name)) !== sum) fail(`its release's ${name} isn't what box.json says.`);
}
for (const [bundle, files] of Object.entries(box.bundles ?? {})) {
  for (const [name, sum] of Object.entries(files)) {
    if (sha256(path.join(pack, bundle, name)) !== sum) fail(`its release's ${bundle}/${name} isn't what box.json says.`);
  }
}
for (const name of ["bin/kuvo-link.js", "bin/kuvo-start", "bin/box-version"]) if (!Object.hasOwn(box.files, name)) fail(`its release's box.json names no ${name}.`);
if (!box.bundles?.["bin/Kuvo Helper.app"]) fail("its release's box.json names no Kuvo Helper.");
if (!manifest.darwin?.aarch64?.url) fail("its release names no engine for a Mac.");
const trusted = JSON.parse(pinned).map(each => Buffer.from(each, "base64").toString("base64url")).includes(boxes.key);
console.log(trusted ? "the box releases' key" : "a key no box trusts for an update");
EOF
KEY_SAYS=$("$NODE" "$work/release.mjs" "$RELEASE" "$work/pack" "$VERSION" "$PINNED") || exit 1
HELPER="$work/pack/bin/Kuvo Helper.app"
test -d "$HELPER" || fail "its release's pack has no Kuvo Helper (bin/Kuvo Helper.app)."
codesign --verify --deep --strict "$HELPER" || fail "Kuvo Helper, as packed, isn't signed as it was."
[ "$(identifier "$HELPER")" = "$ID.box" ] || fail "Kuvo Helper is signed as $(identifier "$HELPER"), not as $ID.box."

echo "Checked $APP: $ID $VERSION, signed $SIGNER; kuvo-keys; the box's $VERSION release (signed with $KEY_SAYS) with Kuvo Helper."
[ "$RUN" = 1 ] || exit 0

# The dry run: a copy with an ID of its own, in a home of its own. Never the bundle itself, which
# as Kuvo Code would set up the real ~/.kuvo-code.
. scripts/real-app-guard.sh
COPY_ID="$ID.check"
ROOT=".kuvo-code-check"
COPY="$work/copy/Kuvo.app"
COPY_HOME="$work/home"
real_app_guard "scripts/check-desktop-app.sh" "$COPY_ID" "$ROOT" "$COPY_HOME/$ROOT" "$COPY"
mkdir -p "$work/copy" "$COPY_HOME" "$work/tmp"
ditto "$APP" "$COPY"
plutil -replace CFBundleIdentifier -string "$COPY_ID" "$COPY/Contents/Info.plist"
codesign --force --sign - --options runtime --identifier "$COPY_ID" "$COPY"
# (Only what it needs of this shell's: no KUVO_* variables, and no home but its own.)
asked() { env -i PATH="/usr/bin:/bin:/usr/sbin:/sbin" HOME="$COPY_HOME" TMPDIR="$work/tmp" "$@" "$COPY/Contents/MacOS/$EXECUTABLE" --install-status; }

if asked env >"$work/refused" 2>&1; then
    cat "$work/refused" >&2
    fail "a copy of the app with no root of its own ran its installer."
fi
[ ! -e "$COPY_HOME/.kuvo-code" ] && [ ! -e "$COPY_HOME/$ROOT" ] || fail "a copy of the app with no root of its own wrote in its home."
echo "A copy with no root of its own installs nothing: $(tail -n 1 "$work/refused")"

asked env KUVO_DEMO_MACHINE_ROOT="$ROOT" >"$work/said" 2>&1 || { cat "$work/said" >&2; fail "the copy's installer gave no answer."; }
cat "$work/said"
grep -qx "verdict: fresh" "$work/said" && grep -qx "verb: new" "$work/said" || fail "a new home wasn't judged fresh, with new to run."
[ ! -e "$COPY_HOME/.kuvo-code" ] || fail "the copy wrote in its home's .kuvo-code, not in its own root."
echo "The copy ($COPY_ID, in a home of its own) reached its installer: a new Mac, to be set up with new."
