#!/bin/sh
# Sets up this clone's merge driver for what's built from sources and kept in git (`merge=kuvo-built`
# in .gitattributes): the box's bundles and the route vectors. Two branches that both rebuilt one
# always conflict on it, and neither side is right after the merge, so the driver keeps the side
# checked out (in a rebase or cherry-pick, the branch replayed onto, not the commit replayed) and
# the merge goes on. After a merge, rebase or cherry-pick, run scripts/check-built.sh (--build to
# build them again, then commit what it writes): CONTRIBUTING.md, "What branches don't edit by hand".
# It goes in the clone's own config (.git/config), which every worktree of it shares: once per
# clone; running it again changes nothing. Without it, git merges those files as text, and they
# conflict as before.
set -eu
cd "$(dirname "$0")/.."
git config --local merge.kuvo-built.name "Kuvo Code's built files: keep the side checked out, then build again"
# Keeps %A (the side checked out) as it is, and says what to run.
git config --local merge.kuvo-built.driver 'echo "Kept the checked-out side of %P, which is built from sources: once this merge, rebase or cherry-pick is done, run scripts/check-built.sh --build and commit what it writes." >&2'
echo "Set merge.kuvo-built in $(git rev-parse --path-format=absolute --git-common-dir)/config."
