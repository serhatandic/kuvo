#!/bin/sh
# Debug build, signed with the virtualization entitlement, then launched.
set -eu
cd "$(dirname "$0")/.."
swift build
codesign --force --sign - --entitlements Terrarium.entitlements .build/debug/Terrarium
exec .build/debug/Terrarium "$@"
