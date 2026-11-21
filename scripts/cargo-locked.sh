#!/bin/sh
# Run Cargo from the caller's directory with the machine-wide Rust lane for this user.
set -eu
exec "$(dirname "$0")/heavy.sh" rust cargo "$@"
