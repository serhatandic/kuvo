#!/bin/bash
# Usage: scripts/lokum-box-check.sh <checkout> <name> [bun test args…]
# Builds locally, then runs the box tests on the pinned engine's Bun in an isolated scratch folder.
# It prints what failed and skipped, with the counts; FULL=1 prints all the run says. HOST defaults
# to lokum-local (`HOST=lokum-tailscale` away from home).
set -eo pipefail
HOST=${HOST:-lokum-local}
REPO=${1:?checkout}; NAME=${2:?name}; shift 2
case "$NAME" in ''|*[!a-zA-Z0-9_-]*) echo 'name must contain only letters, digits, _ or -' >&2; exit 64;; esac
# (The guard removes folders in ~/.kuvo-code-test named like a run of its own whose process is gone.)
if [[ "$NAME" =~ ^(ku|sb|sa|kv|outside)-[0-9]+(-|$) ]]; then echo "name mustn't look like a test run's own folder: $NAME" >&2; exit 64; fi
cd "$REPO"
scripts/build-built.sh
IFS= read -r -d '' remote <<'REMOTE' || true
set -eo pipefail
name=$1 full=$2; shift 2
real_home=$HOME
T=$HOME/.kuvo-code-test/$name
# Refuse existing runs, including symlinks; never remove someone else's scratch.
[ ! -e "$T" ] && [ ! -L "$T" ] || { echo "Scratch exists: $T" >&2; exit 1; }
tmp_before=$(ls -A /tmp); home_before=$(ls -A "$HOME")
parent_created=0
if [ ! -d "$HOME/.kuvo-code-test" ]; then
    mkdir "$HOME/.kuvo-code-test" && parent_created=1
fi
mkdir -m 700 "$T"
cleanup() {
    status=$?
    set +e
    cd "$real_home"
    ls -A /tmp > "$T/tmp-after"
    ls -A "$real_home" > "$T/home-after"
    # A child finishing its cleanup can race removal; retry only this run's folder.
    for attempt in 1 2 3; do
        rm -rf -- "$T" && break
    done
    if [ -e "$T" ]; then echo "Cleanup failed: $T" >&2; status=1; fi
    if [ "$parent_created" -eq 1 ]; then rmdir "$real_home/.kuvo-code-test" 2>/dev/null || true; fi
    printf '/tmp new:\n'
    comm -13 <(printf '%s\n' "$tmp_before" | LC_ALL=C sort) <(ls -A /tmp | LC_ALL=C sort)
    printf 'home new:\n'
    comm -13 <(printf '%s\n' "$home_before" | LC_ALL=C sort) <(ls -A "$real_home" | LC_ALL=C sort)
    if [ ! -e "$T" ]; then echo cleaned; fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM
mkdir -p "$T/repo" "$T/tmp" "$T/cache"
printf '%s\n' "$tmp_before" > "$T/tmp-before"
printf '%s\n' "$home_before" > "$T/home-before"
tar xzf - -C "$T/repo"
for var in $(compgen -e); do
    case "$var" in KUVO_*|OPENCODE_*|XDG_*|SSH_AUTH_SOCK|CLAUDE*|CODEX_HOME|BUN_*|NODE_OPTIONS) unset "$var";; esac
done
# HOME stays yours, as on the Mac (scripts/test-box.sh): the guard (test-safety.js) gives the run a
# home of its own in ~/.kuvo-code-test and removes it, and the tests that judge your own home (Auto's
# reviewer, the walls' HOME check) need HOME to be the account's, as the system has it. A scratch HOME
# failed 19 tests, the SSH agents' one too (its sockets' paths, deeper, past Linux's 108 bytes).
export TMPDIR=$T/tmp XDG_CACHE_HOME=$T/cache KUVO_TEST_WRAPPER_PID=$$
export BUN_INSTALL_CACHE_DIR=$T/cache/bun npm_config_cache=$T/cache/npm UV_CACHE_DIR=$T/cache/uv
export BUN_RUNTIME_TRANSPILER_CACHE_PATH=$T/cache/transpiler
export PATH=/usr/local/bin:/usr/bin:/bin
# Run the same engine as a new box, copied into this run from its pinned package.
# The installed engine and the live box's files are outside this run.
pin="$T/repo/box/src/shared/engine-pin.json"
build=$(python3 -c 'import json, platform, sys; data = json.load(open(sys.argv[1])); build = data["linux"][platform.machine()]; print(build["url"], build["integrity"], sep="\n")' "$pin")
url=${build%%$'\n'*}
integrity=${build##*$'\n'}
curl -fsSL "$url" -o "$T/engine.tgz"
[ "$integrity" = "sha512-$(openssl dgst -sha512 -binary "$T/engine.tgz" | openssl base64 -A)" ]
mkdir "$T/bin"
tar xzf "$T/engine.tgz" -C "$T/bin" --strip-components=2 package/bin/opencode
engine="$T/bin/opencode"
export BUN_BE_BUN=1
cd "$T/repo/box"
if [ "$full" = 1 ]; then
    "$engine" install --frozen-lockfile
    "$engine" test --timeout 120000 "$@"
else
    "$engine" install --frozen-lockfile > "$T/install.log" 2>&1 || { cat "$T/install.log"; exit 1; }
    "$engine" test --timeout 120000 "$@" 2>&1 | grep -E '^\(fail\)|^\(skip\)|error:|Expected|Received|^ [0-9]+ (pass|fail|skip)|^Ran '
fi

REMOTE
printf -v command 'bash -c %q -- %q %q' "$remote" "$NAME" "${FULL:+1}"
for file in "$@"; do printf -v quoted ' %q' "$file"; command+=$quoted; done
# The declaration coverage test compares policy families with the checked-in inventory.
inventory=docs/architecture/stored-formats.md
extra=()
if [[ -f "$inventory" ]]; then extra+=("$inventory"); fi
COPYFILE_DISABLE=1 tar --no-xattrs -czf - \
    box/.bun-version box/package.json box/bun.lock box/bunfig.toml box/tsconfig.json \
    box/src box/test box/eval catalog box/dist box/bridge-tests \
    protocol/ROUTES.md protocol/vectors protocol/gate-required.txt \
    scripts/build-built.sh scripts/build-bun.sh scripts/build-box.sh scripts/build-agents.sh \
    scripts/build-browser.sh scripts/build-browser.ts scripts/real-app-guard.sh \
    scripts/route-shapes.sh scripts/route-shapes.ts scripts/routes-md.ts \
    scripts/box-release.sh scripts/sign-box.mjs ${extra[@]+"${extra[@]}"} |
    ssh -o BatchMode=yes "$HOST" "$command"
