#!/bin/sh
# Build the agents bridge. It downloads each agent SDK when first needed.
set -eu
cd "$(dirname "$0")/.."
[ "$#" -eq 0 ] || { echo "Usage: scripts/build-agents.sh" >&2; exit 2; }
. scripts/build-bun.sh

mkdir -p box/dist
out=box/dist/kuvo-agents.js
built=$(mktemp "$out.XXXXXX")
trap 'rm -f "$built"' EXIT
BUN_BE_BUN=1 "$BUN" build box/src/agents/entry.ts --target=bun --outfile "$built"
hash=$(shasum -a 256 <"$built" | cut -d' ' -f1)
# These words are part of the bridge handover protocol. Older builds recognize them too.
printf '// Kuvo release switch v1\n// Box services ownership v1\n// Built from Packages/Box/src/agents %s\n' "$hash" >>"$built"
chmod 644 "$built"
mv -f "$built" "$out"
echo "Built $out"
