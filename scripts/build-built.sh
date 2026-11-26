#!/bin/sh
# Build the box bundles, wire and route vectors and route reference from this checkout.
# --if-needed reuses a complete build whose inputs have not changed.
set -eu
cd "$(dirname "$0")/.."

stamp=box/dist/.built
case "${1:-}" in
    "") [ "$#" -eq 0 ] || exit 2 ;;
    --if-needed)
        [ "$#" -eq 1 ] || exit 2
        if [ -f "$stamp" ] &&
            [ -s box/dist/kuvo-link.js ] && [ -s box/dist/kuvo-agents.js ] && [ -s box/dist/kuvo-browser.js ] && [ -s box/dist/browser-config.json ] &&
            [ -s protocol/vectors/routes/features.json ] && [ -s protocol/vectors/routes/shapes/answers.json ] &&
            [ -s protocol/vectors/wire.json ] && [ -s protocol/ROUTES.md ] &&
            [ -z "$(find box/src box/.bun-version box/package.json box/bun.lock catalog \
                scripts/build-built.sh scripts/build-bun.sh scripts/build-box.sh scripts/build-agents.sh \
                scripts/build-browser.sh scripts/build-browser.ts \
                scripts/route-shapes.sh scripts/route-shapes.ts scripts/routes-md.ts box/test/fixtures/wire-vectors.ts \
                -type f -newer "$stamp" -print)" ]; then
            exit 0
        fi ;;
    *) echo "Usage: scripts/build-built.sh [--if-needed]" >&2; exit 2 ;;
esac

. scripts/build-bun.sh
mkdir -p box/dist
# Timestamp the start so an edit made during a build triggers a rebuild on the next use.
started=$(mktemp box/dist/.building.XXXXXX)
trap 'rm -f "$started"' EXIT
if [ ! -f box/node_modules/@modelcontextprotocol/sdk/package.json ] ||
    [ ! -f box/node_modules/mdast-util-from-markdown/package.json ] ||
    [ box/bun.lock -nt box/node_modules ] || [ box/package.json -nt box/node_modules ]; then
    (cd box && BUN_BE_BUN=1 "$BUN" install --frozen-lockfile --production && touch node_modules)
fi
scripts/build-box.sh
scripts/build-agents.sh
scripts/build-browser.sh
scripts/route-shapes.sh
BUN_BE_BUN=1 "$BUN" --config=/dev/null --no-env-file box/test/fixtures/wire-vectors.ts
mv -f "$started" "$stamp"
