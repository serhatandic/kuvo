#!/bin/sh
# Writes what's kept from box/src/shared, so it's held by hand in one place only
# (scripts/route-shapes.ts says how): protocol/vectors/routes/shapes/answers.json, every
# route's answers, used by both boxes' contract tests (contract-runner.js); and protocol/vectors/routes/features.json,
# the scenario that holds each box's `kuvo/info` to its features; and protocol/ROUTES.md, every route
# with its comment, for people (not kept in git, and not checked). Run it after changing a route, a
# shape or a feature, and commit the vectors it writes. `--check` says whether the files are what the sources write, as
# CI checks. Either refuses a list of features out of order or with a name twice.
#   BUN   the Bun to run with (as scripts/build-box.sh finds it)
set -eu
cd "$(dirname "$0")/.."

. scripts/real-app-guard.sh
if [ -z "${BUN:-}" ]; then
  if command -v bun >/dev/null 2>&1; then
    BUN=bun
  else
    for engine in "$HOME/Library/Application Support/run.kuvo.code."*/engine/*/opencode; do
      [ -x "$engine" ] && BUN=$engine
    done
  fi
fi
[ -n "${BUN:-}" ] || { echo "scripts/route-shapes.sh needs Bun: install it, or name one with BUN=." >&2; exit 1; }
real_app_guard "scripts/route-shapes.sh" "$BUN"

if [ "${1:-}" = "--check" ]; then
  BUN_BE_BUN=1 "$BUN" scripts/route-shapes.ts "$PWD" --check
else
  BUN_BE_BUN=1 "$BUN" scripts/route-shapes.ts "$PWD"
fi
