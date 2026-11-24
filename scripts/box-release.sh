#!/bin/sh
# Builds the box's release (protocol/PROTOCOL.md, "Updates") from the box's own sources, with no
# app built or run: what every box runs (box/src/release/files.ts: the keeper's files from
# box/src/keeper, the link, the agents' bridge and the browser as built, the catalogs from
# box/src/shared/integrations, the bots' words, and box.json naming them and the engine from
# box/src/shared/engine-pin.json), with each Mac bundle given (Kuvo Helper, signed already,
# scripts/build-helper.sh) as `bin/<name>.app`, packed as `box-<version>.tar.gz`, and
# `boxes.json`, its manifest, signed (scripts/sign-box.mjs). The release workflow publishes both,
# for boxes to update themselves with; an app that carries them offers them to the box on its
# computer (`kuvo/box/update {from}`), and installs a computer's first box from them with
# `kuvo-install` (box/src/install), written beside them.
#   scripts/box-release.sh --files <version> <folder>
#       Only the files, laid out as ~/.kuvo-code is, into <folder> (new or empty). No key.
#   scripts/box-release.sh --rust-pack <target> <executable> <version> <folder> [<bundle>.app…]
#       An unsigned Rust target pack and its box.json, ready for sign-box.mjs. No key.
#   KUVO_BOX_SIGNING_KEY=<PEM> scripts/box-release.sh <version> <pack's URL> <folder> [<bundle>.app…]
#       The pack and boxes.json, into <folder>.
#   <pack's URL>   where the pack is published, ending in box-<version>.tar.gz: a box finds the pack
#                  by that name beside boxes.json when it's given the folder
#   BUN            the Bun to write the files with (as scripts/route-shapes.sh finds it)
#   BOX_SOURCES    another checkout's box sources to write them from (its tests': a keeper and
#                  an engine of their own), by this checkout's builder and table. Never in a
#                  release: a pack is made from them only when its URL is under `.invalid`, a
#                  name nothing is ever published at (RFC 6761), and refused otherwise.
#   NODE           the Node to sign with (default: node)
# The same files pack to the same bytes, whenever they're packed (on one system: a Mac's tar and
# Linux's write their headers their own ways): each entry in name order, with one fixed time and
# no owner, and gzip keeping neither a name nor a time.
# Build this checkout before packing. BOX_SOURCES supplies already prepared test fixtures.
set -eu

usage() {
    echo "Usage: scripts/box-release.sh --files <version> <folder>" >&2
    echo "       scripts/box-release.sh --rust-pack <target> <executable> <version> <folder> [<bundle>.app…]" >&2
    echo "       KUVO_BOX_SIGNING_KEY=<PEM> scripts/box-release.sh <version> <pack's URL> <folder> [<bundle>.app…]" >&2
    exit 2
}
ONLY_FILES=0
RUST_TARGET=""
if [ "${1:-}" = "--rust-pack" ]; then
    [ $# -ge 5 ] || usage
    RUST_TARGET=$2 VERSION=$4
    case "$RUST_TARGET" in
        aarch64-apple-darwin | x86_64-apple-darwin | aarch64-unknown-linux-musl | x86_64-unknown-linux-musl) RUST_KIND=unix ;;
        x86_64-pc-windows-gnullvm) RUST_KIND=windows ;;
        *) usage ;;
    esac
    case "$RUST_TARGET" in
        *-apple-darwin) ;;
        *) [ $# -eq 5 ] || usage ;;
    esac
    RUST_EXECUTABLE=$(cd "$(dirname "$3")" && pwd -P)/$(basename "$3")
    [ -f "$RUST_EXECUTABLE" ] || { echo "No Rust executable at $RUST_EXECUTABLE." >&2; exit 2; }
    shift 4
elif [ "${1:-}" = "--files" ]; then
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
    if [ -n "${BOX_SOURCES:-}" ]; then
        host=${URL#https://}
        case "${host%%/*}" in
            *.invalid) ;;
            *) echo "BOX_SOURCES is for tests: a release is packed from this checkout's own sources." >&2; exit 2 ;;
        esac
    fi
fi
# Keep the key in this shell until the manifest signer needs it.
signing_key=${KUVO_BOX_SIGNING_KEY:-}
unset KUVO_BOX_SIGNING_KEY
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

if [ -z "${BOX_SOURCES:-}" ]; then
    . scripts/build-bun.sh
    scripts/build-built.sh
fi

. scripts/real-app-guard.sh
if [ -z "${BUN:-}" ]; then
    if command -v bun >/dev/null 2>&1; then
        BUN=bun
    elif [ -x "$HOME/.bun/bin/bun" ]; then
        BUN="$HOME/.bun/bin/bun"
    else
        for engine in "$HOME/Library/Application Support/run.kuvo.code."*/engine/*/opencode; do
            [ -x "$engine" ] && BUN=$engine
        done
    fi
fi
[ -n "${BUN:-}" ] || { echo "scripts/box-release.sh needs Bun: install it, or name one with BUN=." >&2; exit 1; }
real_app_guard "scripts/box-release.sh" "$BUN"

# (Never with the key: writing the files needs none.)
# (ENGINE_CHECK, where given: what scripts/engine-check.mjs found, for box.json's `engines` and
# `downgradeSafe`; without it, box.json names the pinned engine alone.)
files() {
    if [ -n "$RUST_TARGET" ]; then
        BUN_BE_BUN=1 "$BUN" box/src/release/files.ts "${BOX_SOURCES:-$PWD}" "$VERSION" "$1" "$RUST_KIND" "$RUST_EXECUTABLE"
    else
        BUN_BE_BUN=1 "$BUN" box/src/release/files.ts "${BOX_SOURCES:-$PWD}" "$VERSION" "$1"
    fi
}

if [ "$ONLY_FILES" = 1 ]; then
    files "$OUT"
    echo "Wrote the box's $VERSION files into $OUT"
    exit 0
fi

# (Without the last slash a Mac's temporary folder ends in.)
temporary=${TMPDIR:-/tmp}
work=$(mktemp -d "${temporary%/}/kuvo-box-release.XXXXXXXX")
pack=""
trap 'rm -rf "$work"; [ -z "$pack" ] || rm -f "$pack.new"; rm -f "$OUT/box.json.new" "$OUT/boxes.json.new" "$OUT/kuvo-install.new" "$OUT/kuvo-install-bin.new"' EXIT
box="$work/box"
files "$box"
pack="$OUT/box-$VERSION${RUST_TARGET:+-$RUST_TARGET}.tar.gz"

# Its bundles, whole, and named in box.json's `bundles` file by file, as a box checks and swaps
# them (a box from before bundles passes them over).
for bundle in "$@"; do
    [ ! -e "$box/bin/$(basename "$bundle")" ] || { echo "Two bundles named $(basename "$bundle")." >&2; exit 2; }
    cp -Rp "$bundle" "$box/bin/"
done
"$NODE" --input-type=module - "$box" <<'EOF'
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

# Every entry by name, in order, with the one time (see the top).
(cd "$box" && find . -exec env TZ=UTC touch -t 202601010000 {} + && find . | LC_ALL=C sort >"$work/entries")
# No owners, and on a Mac no extended attributes or AppleDouble files (so a helper's signature has
# to be in its files, as a Mach-O program's is).
if tar --version 2>/dev/null | grep -q bsdtar; then
    COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata --format ustar --uid 0 --gid 0 --uname "" --gname "" -n -cf "$work/pack.tar" -C "$box" -T "$work/entries"
else
    tar --format=ustar --owner=0 --group=0 --numeric-owner --no-recursion -cf "$work/pack.tar" -C "$box" -T "$work/entries"
fi
# All of them in it: a name too long for the format is left out by a Mac's tar with only a word
# about it, and the pack would be one no box takes.
tar -tf "$work/pack.tar" | sed 's|/$||' | LC_ALL=C sort >"$work/packed"
cmp -s "$work/entries" "$work/packed" || { echo "Not everything could be packed (a name too long?):" >&2; diff "$work/entries" "$work/packed" >&2 || true; exit 1; }
env -u GZIP gzip -n -9 <"$work/pack.tar" >"$pack.new"
# Each bundle, as a box unpacks it, still signed as it was (a signature kept in extended attributes
# doesn't survive packing). Where there's codesign: a Mac, as a release is built.
if command -v codesign >/dev/null 2>&1 && [ $# -gt 0 ]; then
    mkdir "$work/unpacked"
    tar -xzf "$pack.new" -C "$work/unpacked"
    for helper in "$work/unpacked/bin/"*.app; do
        codesign --verify --strict --deep "$helper" || { rm -f "$pack.new"; echo "$(basename "$helper") isn't signed as it was once packed." >&2; exit 1; }
    done
fi
if [ -n "$RUST_TARGET" ]; then
    cp "$box/box.json" "$OUT/box.json.new"
    if [ "$RUST_KIND" = unix ]; then
        cp box/src/install/kuvo-install "$OUT/kuvo-install.new"
        cp "$RUST_EXECUTABLE" "$OUT/kuvo-install-bin.new"
        chmod 755 "$OUT/kuvo-install.new" "$OUT/kuvo-install-bin.new"
        mv -f "$OUT/kuvo-install.new" "$OUT/kuvo-install"
        mv -f "$OUT/kuvo-install-bin.new" "$OUT/kuvo-install-bin"
    fi
    mv -f "$pack.new" "$pack"
    mv -f "$OUT/box.json.new" "$OUT/box.json"
    echo "Packed $VERSION for $RUST_TARGET into $OUT"
    exit 0
fi
# Signed before either is put in place: a signing that fails leaves no pack without its
# boxes.json, nor a new pack beside an older one's.
KUVO_BOX_SIGNING_KEY="$signing_key" "$NODE" scripts/sign-box.mjs "$VERSION" "$pack.new" "$URL" "$box/box.json" > "$OUT/boxes.json.new"
mv -f "$pack.new" "$pack"
mv -f "$OUT/boxes.json.new" "$OUT/boxes.json"
# The installer beside them, for an app that carries the release to run on a computer with no box
# yet (`sh kuvo-install status|new`, PROTOCOL.md, "Installing this computer's box"). Never in the
# pack: a box that updates itself has no use for it.
cp box/src/install/kuvo-install "$OUT/kuvo-install.new"
chmod 755 "$OUT/kuvo-install.new"
mv -f "$OUT/kuvo-install.new" "$OUT/kuvo-install"
echo "Packed the box's $VERSION release into $OUT"
