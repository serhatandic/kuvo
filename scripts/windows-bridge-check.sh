#!/bin/bash
# Usage: scripts/windows-bridge-check.sh <worktree> <item> [--wsl] [--test-name-pattern=PATTERN] [test files…]
exec "$(dirname "$0")/windows-checks/check.sh" bridge "$@"
