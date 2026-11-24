#!/bin/sh
# Run Cargo in the box workspace with its own build lane.
set -eu
scripts=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$scripts/../box"
# Several worktrees build at once; incremental caches cost 7–14 GB each.
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
exec "$scripts/heavy.sh" rust-box cargo "$@"
