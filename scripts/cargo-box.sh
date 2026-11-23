#!/bin/sh
# Run Cargo in the box workspace with its own build lane.
set -eu
scripts=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$scripts/../box"
exec "$scripts/heavy.sh" rust-box cargo "$@"
