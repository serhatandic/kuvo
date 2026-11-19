#!/bin/sh
# Builds the agents bridge (box/src/agents) into box/dist/kuvo-agents.js, the
# one file the app ships and runs with the engine's Bun. Run it after changing anything in
# box/src/agents or src/shared, and commit the result. The agents' SDKs aren't in it: the
# bridge downloads each one the first time it's needed. Its last line names the actual bundle
# content, so changing a box service alone does not cause the agents bridge to hand over.
#   BUN   the Bun to build with (default: `bun` on PATH, else the engine a demo, perf or e2e copy
#         installed, which is Bun with BUN_BE_BUN=1; never the real Kuvo Code's)
set -eu
cd "$(dirname "$0")/.."

. scripts/real-app-guard.sh
if [ -z "${BUN:-}" ]; then
  if command -v bun >/dev/null 2>&1; then
    BUN=bun
  else
    BUN=$(newest_copy_engine)
    [ -n "$BUN" ] || { echo "No Bun: install it, or launch a demo copy of Kuvo Code once so it downloads its engine." >&2; exit 1; }
  fi
fi
real_app_guard "scripts/build-agents.sh" "$BUN"

out=box/dist/kuvo-agents.js
built=$(mktemp "${TMPDIR:-/tmp}/kuvo-agents.XXXXXX")
trap 'rm -f "$built"' EXIT
BUN_BE_BUN=1 "$BUN" build box/src/agents/entry.ts --target=bun --outfile "$built"
hash=$(shasum -a 256 <"$built" | cut -d' ' -f1)
# These words are part of the bridge handover protocol. Older builds recognize them too.
printf '// Box services ownership v1\n// Built from Packages/Box/src/agents %s\n' "$hash" >>"$built"
if [ "${1:-}" = "--check" ]; then
  if [ -e swift/KuvoCode/Resources/kuvo-agents.js ]; then
    echo "swift/KuvoCode/Resources/kuvo-agents.js is back: the bundle is $out now." >&2
    exit 1
  fi
  cmp -s "$out" "$built" && exit 0
  echo "$out isn't built from its sources: run ./scripts/build-agents.sh and commit it." >&2
  exit 1
fi
cat "$built" >"$out"
echo "Built $out"
