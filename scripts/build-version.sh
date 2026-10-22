#!/bin/sh
# Prints the build version Sparkle compares (CFBundleVersion, an update's sparkle:version) for a
# version: a release's as it is ("0.7.0"), a beta's with "-beta.N" as "bN" ("0.7.0-beta.1" is
# "0.7.0b1"), as Sparkle reads nothing after a dash and orders 0.7.0b1 < 0.7.0b2 < 0.7.0.
#   scripts/build-version.sh 0.7.0-beta.1
set -eu
case "$1" in *"
"*) echo "Versions are one line." >&2; exit 1 ;; esac
if printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$'; then
    printf '%sb%s\n' "${1%-beta.*}" "${1##*-beta.}"
elif printf '%s' "$1" | grep -Eq '^[0-9]+(\.[0-9]+)*$'; then
    printf '%s\n' "$1"
else
    echo "Versions look like 0.7.0, or 0.7.0-beta.1 for a beta, not $1." >&2
    exit 1
fi
