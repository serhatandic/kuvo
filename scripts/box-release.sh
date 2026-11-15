#!/bin/sh
# Builds the box's release (protocol/PROTOCOL.md, "Updates") from the box's own sources, with no
# app built or run: what every box runs (box/src/release/files.ts: the keeper's files from
# box/src/keeper, the link, the agents' bridge and the browser as built, the catalogs from
# box/src/shared/integrations, the bots' words, and box.json naming them and the engine from
# box/src/shared/engine-pin.json), with each Mac bundle given (Kuvo Helper, signed already,
# scripts/build-helper.sh) as `bin/<name>.app`, packed as `box-<version>.tar.gz`, and
# `boxes.json`, its manifest, signed (scripts/sign-box.mjs). The release workflow publishes both,
# for boxes to update themselves with; an app that carries them offers them to the box on its
# computer (`kuvo/box/update {from}`).
#   scripts/box-release.sh --files <version> <folder>
#       Only the files, laid out as ~/.kuvo-code is, into <folder> (new or empty). No key.
#   KUVO_BOX_SIGNING_KEY=<PEM> scripts/box-release.sh <version> <pack's URL> <folder> [<bundle>.app…]
#       The pack and boxes.json, into <folder>.
#   <pack's URL>   where the pack is published, ending in box-<version>.tar.gz: a box finds the pack
#                  by that name beside boxes.json when it's given the folder
#   BUN            the Bun to write the files with (as scripts/route-shapes.sh finds it)
#   BOX_SOURCES    another checkout's box sources to write them from (its tests': a keeper and
#                  an engine of their own), by this checkout's builder and table. Never for a
#                  release: the workflow sets none.
#   NODE           the Node to sign with (default: node)
# What's built and kept in git (kuvo-link.js, kuvo-agents.js, kuvo-browser.js) goes in as it is:
# scripts/check-built.sh says whether it's what the sources build.
set -eu

usage() {
    echo "Usage: scripts/box-release.sh --files <version> <folder>" >&2
    echo "       KUVO_BOX_SIGNING_KEY=<PEM> scripts/box-release.sh <version> <pack's URL> <folder> [<bundle>.app…]" >&2
    exit 2
}
ONLY_FILES=0
if [ "${1:-}" = "--files" ]; then
    [ $# -eq 3 ] || usage
    ONLY_FILES=1 VERSION=$2 URL=""
    shift 2
else
    [ $# -ge 3 ] && [ -n "${KUVO_BOX_SIGNING_KEY:-}" ] || usage
    VERSION=$1 URL=$2
    shift 2
    case "$URL" in
        */"box-$VERSION.tar.gz") ;;
        *) echo "The pack's URL must end in box-$VERSION.tar.gz." >&2; exit 2 ;;
    esac
fi
OUT_GIVEN=$1
shift
# The bundles, by their own paths, before going to the checkout (and before anything is made).
n=$#
while [ "$n" -gt 0 ]; do
    bundle=$1
    shift
    n=$((n - 1))
    case "$bundle" in
        *.app | *.app/) ;;
        *) echo "Not a bundle: $bundle" >&2; exit 2 ;;
    esac
    [ -d "$bundle" ] || { echo "No bundle at $bundle" >&2; exit 2; }
    set -- "$@" "$(cd "$bundle" && pwd -P)"
done
mkdir -p "$OUT_GIVEN"
OUT=$(cd "$OUT_GIVEN" && pwd -P)
NODE=${NODE:-node}
cd "$(dirname "$0")/.."

. scripts/real-app-guard.sh
if [ -z "${BUN:-}" ]; then
    if command -v bun >/dev/null 2>&1; then
        BUN=bun
    else
        for engine in "$HOME/Library/Application Support/run.kuvo.code."*/engine/*/opencode; do
            [ -x "$engine" ] && BUN=$engine
        done
    fi
fi
[ -n "${BUN:-}" ] || { echo "scripts/box-release.sh needs Bun: install it, or name one with BUN=." >&2; exit 1; }
real_app_guard "scripts/box-release.sh" "$BUN"

# (Never with the key: writing the files needs none.)
files() { env -u KUVO_BOX_SIGNING_KEY BUN_BE_BUN=1 "$BUN" box/src/release/files.ts "${BOX_SOURCES:-$PWD}" "$VERSION" "$1"; }

if [ "$ONLY_FILES" = 1 ]; then
    files "$OUT"
    echo "Wrote the box's $VERSION files into $OUT"
    exit 0
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/kuvo-box-release.XXXXXXXX")
trap 'rm -rf "$work"; rm -f "$OUT/box-$VERSION.tar.gz.new" "$OUT/boxes.json.new"' EXIT
box="$work/box"
files "$box"

# Its bundles, whole, and named in box.json's `bundles` file by file, as a box checks and swaps
# them (a box from before bundles passes them over).
for bundle in "$@"; do
    [ ! -e "$box/bin/$(basename "$bundle")" ] || { echo "Two bundles named $(basename "$bundle")." >&2; exit 2; }
    cp -Rp "$bundle" "$box/bin/"
done
env -u KUVO_BOX_SIGNING_KEY "$NODE" --input-type=module - "$box" <<'EOF'
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
const box = process.argv[2];
// As a box takes them (box/src/link/updates/update.ts, BUNDLE and BUNDLE_PART): no part
// starting with a dot, no links, nothing but folders and files.
const bundleName = /^bin\/[A-Za-z0-9][A-Za-z0-9._ -]*\.app$/;
const part = /^(?:[A-Za-z0-9_][A-Za-z0-9._+ -]*\/)*[A-Za-z0-9_][A-Za-z0-9._+ -]*$/;
const record = path.join(box, "box.json");
const manifest = JSON.parse(fs.readFileSync(record, "utf8"));
for (const name of fs.readdirSync(path.join(box, "bin")).filter(name => name.endsWith(".app")).sort()) {
  const bundle = `bin/${name}`;
  if (!bundleName.test(bundle)) {
    console.error(`A box can't take a bundle named ${name}.`);
    process.exit(1);
  }
  const files = {};
  for (const entry of fs.readdirSync(path.join(box, bundle), { recursive: true, encoding: "utf8" }).sort()) {
    const stat = fs.lstatSync(path.join(box, bundle, entry));
    if (stat.isDirectory()) continue;
    if (!stat.isFile() || !part.test(entry)) {
      console.error(`A box can't take ${bundle}/${entry} in a bundle: only plain files, named without a leading dot.`);
      process.exit(1);
    }
    files[entry] = crypto.createHash("sha256").update(fs.readFileSync(path.join(box, bundle, entry))).digest("hex");
  }
  (manifest.bundles ??= {})[bundle] = files;
}
fs.writeFileSync(record, JSON.stringify(manifest, null, 2) + "\n");
EOF

pack="$OUT/box-$VERSION.tar.gz"
# No owners, and on a Mac no extended attributes or AppleDouble files (so a helper's signature has
# to be in its files, as a Mach-O program's is).
if tar --version 2>/dev/null | grep -q bsdtar; then
    COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata --uid 0 --gid 0 -czf "$pack.new" -C "$box" .
else
    tar --owner=0 --group=0 --numeric-owner -czf "$pack.new" -C "$box" .
fi
# Each bundle, as a box unpacks it, still signed as it was (a signature kept in extended attributes
# doesn't survive packing). Where there's codesign: a Mac, as a release is built.
if command -v codesign >/dev/null 2>&1 && [ $# -gt 0 ]; then
    mkdir "$work/unpacked"
    tar -xzf "$pack.new" -C "$work/unpacked"
    for helper in "$work/unpacked/bin/"*.app; do
        codesign --verify --strict --deep "$helper" || { rm -f "$pack.new"; echo "$(basename "$helper") isn't signed as it was once packed." >&2; exit 1; }
    done
fi
# Signed before either is put in place: a signing that fails leaves no pack without its
# boxes.json, nor a new pack beside an older one's.
"$NODE" scripts/sign-box.mjs "$VERSION" "$pack.new" "$URL" "$box/box.json" > "$OUT/boxes.json.new"
mv -f "$pack.new" "$pack"
mv -f "$OUT/boxes.json.new" "$OUT/boxes.json"
echo "Packed the box's $VERSION release into $OUT"
