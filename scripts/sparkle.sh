#!/bin/sh
# Prints the folder of Sparkle's release as this checkout pins it: Sparkle.framework, which the
# desktop app carries to update itself (scripts/build-desktop-app.sh), and its tools in bin/
# (sign_update for the feed, scripts/appcast.sh; generate_keys, scripts/set-update-key.sh).
#   scripts/sparkle.sh            downloads it the first time, checked against its SHA-256, into
#                                 build/sparkle/<version> (git ignores build/), and prints that
# The version is the one Kuvo Code 0.7 shipped with (its Package.resolved), so 0.7's copies and
# the desktop app read the same feed the same way. A new version is its own commit: VERSION and
# SHA256 together, from the release's Sparkle-<version>.tar.xz.
set -eu
cd "$(dirname "$0")/.."

VERSION=2.10.0
SHA256=c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c
URL="https://github.com/sparkle-project/Sparkle/releases/download/$VERSION/Sparkle-$VERSION.tar.xz"
FOLDER="$PWD/build/sparkle/$VERSION"

if [ -d "$FOLDER/Sparkle.framework" ] && [ -x "$FOLDER/bin/sign_update" ]; then
    echo "$FOLDER"
    exit 0
fi
mkdir -p build/sparkle
temporary=${TMPDIR:-/tmp}
work=$(mktemp -d "${temporary%/}/kuvo-sparkle.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
curl -fsSL --retry 2 -o "$work/sparkle.tar.xz" "$URL" >&2 || { echo "Sparkle $VERSION couldn't be downloaded from $URL." >&2; exit 1; }
actual=$(shasum -a 256 "$work/sparkle.tar.xz" | cut -d ' ' -f 1)
if [ "$actual" != "$SHA256" ]; then
    echo "Sparkle $VERSION's download isn't the one pinned (SHA-256 $actual, not $SHA256)." >&2
    exit 1
fi
mkdir "$work/unpacked"
tar -xJf "$work/sparkle.tar.xz" -C "$work/unpacked"
test -d "$work/unpacked/Sparkle.framework" && test -x "$work/unpacked/bin/sign_update" || { echo "Sparkle $VERSION's download has no Sparkle.framework or bin/sign_update." >&2; exit 1; }
# Whole or not at all: another build finds it complete, or not there.
rm -rf "$FOLDER"
mv "$work/unpacked" "$FOLDER"
echo "$FOLDER"
