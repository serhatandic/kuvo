#!/bin/sh
# Run Cargo in the box workspace with its own build lane.
set -eu
scripts=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$scripts/../box"
# Several worktrees build at once; incremental caches cost 7–14 GB each.
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
# The lane has two slots: half the cores each keeps the Mini responsive.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
exec "$scripts/heavy.sh" rust-box cargo "$@"
