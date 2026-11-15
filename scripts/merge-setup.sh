#!/bin/sh
# Sets up this clone's merge driver for what's built from sources and kept in git (`merge=kuvo-built`
# in .gitattributes): the box's bundles and the route vectors. Two branches that both rebuilt one
# always conflict on it, and neither side is right after the merge, so the driver keeps this side's
# and the merge goes on; build again after it and commit what that writes (CONTRIBUTING.md, "What
# branches don't edit by hand"), as each script's `--check` fails on a file its sources don't build.
# It goes in the clone's own config (.git/config), which every worktree of it shares: once per
# clone; running it again changes nothing. Without it, git merges those files as text, and they
# conflict as before.
set -eu
cd "$(dirname "$0")/.."
git config --local merge.kuvo-built.name "Kuvo Code's built files: keep this side's, then build again"
# Keeps %A (this side's) as it is, and says what to run.
git config --local merge.kuvo-built.driver 'echo "Kept this side of %P, which is built: run its script again after the merge (scripts/build-box.sh, build-agents.sh, build-browser.sh or route-shapes.sh) and commit it." >&2'
echo "Set merge.kuvo-built in $(git rev-parse --path-format=absolute --git-common-dir)/config."
