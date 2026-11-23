#!/bin/bash
set -eu
here=$(cd "$(dirname "$0")" && pwd -P)
heavy=${KUVO_HEAVY_SH:-$here/../heavy.sh}
if ! "$heavy" --holds box; then
    exec "$heavy" box "$0" "$@"
fi
exec python3 "$here/transfer.py" "$@"
