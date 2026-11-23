#!/bin/bash
# Usage: scripts/windows-box-check.sh <worktree> <item> [--wsl] [test files…]
exec "$(dirname "$0")/windows-checks/check.sh" box "$@"
