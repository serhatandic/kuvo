#!/bin/sh
# Build the box's link (box/src/link) for the engine and the Node bridge tests.
set -eu
cd "$(dirname "$0")/.."
[ "$#" -eq 0 ] || { echo "Usage: scripts/build-box.sh" >&2; exit 2; }
. scripts/build-bun.sh

OUT=box/dist/kuvo-link.js
mkdir -p box/dist
BUN_BE_BUN=1 "$BUN" --config=/dev/null --no-env-file catalog/build.ts --check

# For Node too, not only Bun: the bridge's tests import it with Node, and it uses Node's modules only.
# Its version (BRIDGE_VERSION, box/src/link/version.ts) is the first 12 hex digits of the
# SHA-256 of what's built with the version as its source has it, written in its place: it changes
# with every change to the link, the same on every computer, and no branch bumps it by hand.
SOURCE_VERSION=0.0.0-from-its-sources
raw=$(mktemp "${TMPDIR:-/tmp}/kuvo-link-raw.XXXXXX")
built=$(mktemp "$OUT.XXXXXX")
trap 'rm -f "$raw" "$built"' EXIT
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi; }
build() {
  BUN_BE_BUN=1 "$BUN" --config=/dev/null --no-env-file run - "$raw" <<'BUILD'
const worker = await Bun.build({
  entrypoints: ["box/src/link/server/history-worker.ts"],
  target: "bun",
  format: "esm",
  minify: true,
});
const link = await Bun.build({
  entrypoints: ["box/src/link/main.ts"],
  target: "node",
  external: ["bun:sqlite"],
  format: "esm",
  banner: [
    "// Kuvo resident serve v1",
    "// Kuvo resident workers v1",
    "// Built from box/src by scripts/build-box.sh: change those, then run it. Never edit this file.",
  ].join("\n"),
  define: { __KUVO_HISTORY_WORKER__: JSON.stringify(await worker.outputs[0].text()) },
});
await Bun.write(process.argv[2], link.outputs[0]);
BUILD
  if ! grep -q "\"$SOURCE_VERSION\"" "$raw"; then
    echo "scripts/build-box.sh: the built link doesn't have \"$SOURCE_VERSION\" (box/src/link/version.ts) to write its version in." >&2
    exit 1
  fi
  hash=$(sha256 "$raw" | cut -c1-12)
  if ! printf '%s' "$hash" | grep -Eq '^[0-9a-f]{12}$'; then
    echo "scripts/build-box.sh: couldn't hash the built link (sha256sum or shasum)." >&2
    exit 1
  fi
  layout=$(BUN_BE_BUN=1 "$BUN" --config=/dev/null --no-env-file -e 'import { KUVO_FOLDER_LAYOUT } from "./box/src/shared/folder-version"; console.log(KUVO_FOLDER_LAYOUT)')
  {
    printf '// Kuvo folder layout %s %s\n' "$layout" "$hash"
    sed "s/\"$(printf '%s' "$SOURCE_VERSION" | sed 's/\./\\./g')\"/\"$hash\"/g" "$raw"
  } >"$1"
}

build "$built"
chmod 644 "$built"
mv -f "$built" "$OUT"
echo "Built $OUT"
