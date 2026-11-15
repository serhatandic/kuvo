#!/bin/sh
# Whether what's built from sources and kept in git is what its sources build now: the box's link
# (kuvo-link.js), its agents' bridge (kuvo-agents.js), its browser (kuvo-browser.js) and the route
# vectors (features.json, answers.json). Run it after every merge, rebase or cherry-pick: git keeps
# one side's built files there without a conflict (`merge=kuvo-built`, scripts/merge-setup.sh), and
# with CI paused nothing else checks them before they're built into the app.
#   scripts/check-built.sh          each script's --check; 1 if one is stale, 2 if one couldn't run
#                                   for want of a Bun (the others checked, nothing stale found)
#   scripts/check-built.sh --build  builds all four again (then commit what changed)
#   BUN   the Bun to build with (as scripts/build-box.sh finds it)
set -u
cd "$(dirname "$0")/.."

SCRIPTS="build-box build-agents build-browser route-shapes"
if [ "${1:-}" = "--build" ]; then
    for script in $SCRIPTS; do "scripts/$script.sh" || exit 1; done
    exit 0
fi

stale="" unchecked=""
for script in $SCRIPTS; do
    out=$("scripts/$script.sh" --check 2>&1) && continue
    echo "$out" >&2
    case "$out" in
        *"install it"*) unchecked="$unchecked $script.sh" ;;
        *) stale="$stale $script.sh" ;;
    esac
done
if [ -n "$stale" ]; then
    echo "Built files don't match their sources (scripts:$stale): run scripts/check-built.sh --build and commit what it writes." >&2
    exit 1
fi
if [ -n "$unchecked" ]; then
    echo "Couldn't run$unchecked --check, for want of a Bun; the others match their sources." >&2
    exit 2
fi
echo "Built files match their sources."
