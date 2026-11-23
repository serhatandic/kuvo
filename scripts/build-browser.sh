#!/bin/sh
# Build the browser keeper and agents' browser tools.
set -eu
cd "$(dirname "$0")/.."
[ "$#" -eq 0 ] || { echo "Usage: scripts/build-browser.sh" >&2; exit 2; }
. scripts/build-bun.sh
BUN_BE_BUN=1 "$BUN" --config=/dev/null --no-env-file scripts/build-browser.ts "$PWD"
