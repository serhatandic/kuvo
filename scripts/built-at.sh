#!/bin/sh
# Print the directory of bundles for a revision. Tests set BUILT_AT_CACHE to their scratch folder.
set -eu
cd "$(dirname "$0")/.."
[ "$#" -eq 1 ] || { echo "Usage: scripts/built-at.sh <rev>" >&2; exit 2; }
rev=$(git rev-parse --verify --end-of-options "$1^{commit}")
cache=${BUILT_AT_CACHE:-build/built-at}
mkdir -p "$cache"
cache=$(cd "$cache" && pwd -P)
build=$cache/$rev
if [ -f "$build/.ready" ]; then
    printf '%s\n' "$build"
    exit 0
fi

work=$(mktemp -d "$cache/.build.XXXXXX")
cleanup() {
    if [ -f "$work/tree/.git" ]; then git worktree remove --force "$work/tree" >&2; fi
    rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

legacy=swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js
if git cat-file -e "$rev:$legacy" 2>/dev/null; then
    git show "$rev:$legacy" >"$work/kuvo-link.js"
    for bundle in kuvo-agents.js kuvo-browser.js; do
        if git cat-file -e "$rev:box/dist/$bundle" 2>/dev/null; then
            git show "$rev:box/dist/$bundle" >"$work/$bundle"
        fi
    done
    bundles=$work
else
    git worktree add --detach "$work/tree" "$rev" >&2
    (cd "$work/tree" && scripts/build-built.sh) >&2
    bundles=$work/tree/box/dist
fi

mkdir -p "$build"
for bundle in "$bundles"/kuvo-*.js; do
    mv -f "$bundle" "$build/"
done
touch "$build/.ready"
printf '%s\n' "$build"
