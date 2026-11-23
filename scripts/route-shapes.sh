#!/bin/sh
# Generate route vectors and protocol/ROUTES.md from box/src/shared.
set -eu
cd "$(dirname "$0")/.."
[ "$#" -eq 0 ] || { echo "Usage: scripts/route-shapes.sh" >&2; exit 2; }
. scripts/build-bun.sh
BUN_BE_BUN=1 "$BUN" --config=/dev/null --no-env-file scripts/route-shapes.ts "$PWD"
