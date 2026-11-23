#!/bin/bash
# Usage: scripts/lokum-bridge-check.sh <checkout> <name> [--test-name-pattern=PATTERN] [test files…]
# Runs the chosen bridge tests (all of them when none are named) on lokum, from the checkout as it is.
# Under a pinned Node: the engine's Bun (1.4.4) fails dozens of bridge tests that pass under Node
# (bots-work, reach-elsewhere-rules…). Bun is still on the PATH for the tests that start a box on it.
# All remote writes belong to the scratch folder, but for the run's own home the guard gives it in
# ~/.kuvo-code-test and removes (test-safety.js), and the sandbox tests' probes of your own caches in
# ~/.cache, which they remove (a run cut short can leave them: cleanup lists what's new there).
set -eo pipefail
HOST=${HOST:-lokum-local}
REPO=${1:?checkout}; NAME=${2:?name}; shift 2
case "$NAME" in ''|*[!a-zA-Z0-9_-]*) echo 'name must contain only letters, digits, _ or -' >&2; exit 64;; esac
# (The guard removes folders in ~/.kuvo-code-test named like a run of its own whose process is gone.)
if [[ "$NAME" =~ ^(ku|sb|sa|kv|outside)-[0-9]+(-|$) ]]; then echo "name mustn't look like a test run's own folder: $NAME" >&2; exit 64; fi
cd "$REPO"
scripts/build-built.sh
pattern=
case "${1:-}" in --test-name-pattern=*) pattern=$1; shift ;; esac
files=()
for file in "$@"; do
    file=${file#box/}; file=${file#bridge-tests/}
    case "$file" in */*|.*|*[!a-zA-Z0-9_.-]*|-* ) echo "Invalid test file: $file" >&2; exit 64;; esac
    [[ "$file" == *.test.js && -f "box/bridge-tests/$file" ]] || { echo "No bridge test: $file" >&2; exit 64; }
    files+=("bridge-tests/$file")
done
IFS= read -r -d '' remote <<'REMOTE' || true
set -eo pipefail
name=$1; pattern=$2; shift 2
real_home=$HOME
T=$HOME/.kuvo-code-test/$name
# Refuse existing runs, including symlinks; never remove someone else's scratch.
[ ! -e "$T" ] && [ ! -L "$T" ] || { echo "Scratch exists: $T" >&2; exit 1; }
tmp_before=$(ls -A /tmp); home_before=$(ls -A "$HOME")
caches() { [ -d "$real_home/.cache" ] && find "$real_home/.cache" -maxdepth 3 2>/dev/null | LC_ALL=C sort; }
cache_before=$(caches)
parent_created=0
if [ ! -d "$HOME/.kuvo-code-test" ]; then
    mkdir "$HOME/.kuvo-code-test" && parent_created=1
fi
mkdir -m 700 "$T"
cleanup() {
    status=$?
    set +e
    # (A closed connection mustn't stop it before it has stopped what's running and removed $T.)
    trap '' PIPE
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
    printf '~/.cache new:\n'
    comm -13 <(printf '%s\n' "$cache_before") <(caches)
    if [ ! -e "$T" ]; then echo cleaned; fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM
mkdir -p "$T/repo" "$T/tmp" "$T/cache"
printf '%s\n' "$tmp_before" > "$T/tmp-before"
printf '%s\n' "$home_before" > "$T/home-before"
tar xzf - -C "$T/repo"
# Discover/execute only the engine binary; no live box state is inspected.
engine=$(printf '%s\n' "$real_home"/.kuvo-code/engine/*/opencode | sort -V | tail -1)
for var in $(compgen -e); do
    case "$var" in KUVO_*|OPENCODE_*|XDG_*|SSH_AUTH_SOCK|CLAUDE*|CODEX_HOME|BUN_*|NODE_OPTIONS) unset "$var";; esac
done
# HOME stays yours, as on the Mac and in scripts/lokum-box-check.sh: the guard (test-safety.js) gives
# the run a home of its own in ~/.kuvo-code-test and removes it, and the tests that judge your own
# home (the sandbox's tool caches) need HOME to be the account's, as the system has it. A scratch
# HOME put this scratch's name in every test folder's path, so a long name took the sockets in them
# past Linux's 108 bytes.
export TMPDIR=$T/tmp XDG_CACHE_HOME=$T/cache KUVO_TEST_WRAPPER_PID=$$
export BUN_INSTALL_CACHE_DIR=$T/cache/bun npm_config_cache=$T/cache/npm UV_CACHE_DIR=$T/cache/uv
export BUN_RUNTIME_TRANSPILER_CACHE_PATH=$T/cache/transpiler
mkdir "$T/bin"
printf '#!/bin/bash\nBUN_BE_BUN=1 exec %q "$@"\n' "$engine" > "$T/bin/bun"
chmod +x "$T/bin/bun"
export PATH=$T/bin:/usr/local/bin:/usr/bin:/bin
(cd "$T/repo/box" && BUN_BE_BUN=1 "$engine" install --frozen-lockfile)
cd "$T/repo/box"
[ $# -gt 0 ] || set -- bridge-tests/*.test.js
# Node 24, as the Mac runs the bridge tests (the box's server takes
# options Node 22 ignores), checked against nodejs.org's published checksums; never installed system-wide.
node=v24.22.2
case $(uname -m) in
    x86_64) arch=x64; sha=48cc0e21e048790e54c0cf5749c2725005313d652651c40b7a21feb9697aaf75;;
    aarch64) arch=arm64; sha=eb187837a2230d4047161ac0be008584e16b46f726e9c9490ee35dc6b17d9ba8;;
    *) echo 'Unsupported Linux architecture' >&2; exit 1;;
esac
curl -fsSL "https://nodejs.org/dist/$node/node-$node-linux-$arch.tar.xz" -o "$T/node.tar.xz"
printf '%s  %s\n' "$sha" "$T/node.tar.xz" | sha256sum -c -
mkdir "$T/node"
tar xJf "$T/node.tar.xz" -C "$T/node" --strip-components=1
export PATH=$T/node/bin:$PATH
echo "Runtime: Node $node, downloaded into the scratch folder (engine Bun $(BUN_BE_BUN=1 "$engine" --version) fails bridge tests that pass under Node)"
if [ -n "$pattern" ]; then
    set -- "$pattern" "$@"
fi
node --test --test-reporter=tap "$@"
REMOTE
printf -v command 'bash -c %q -- %q %q' "$remote" "$NAME" "$pattern"
for file in "${files[@]}"; do printf -v quoted ' %q' "$file"; command+=$quoted; done
# compat.test.js inspects source markers on every side, including Swift and shell code.
# Send those tracked sources too, without assets, dependency folders or build output.
sources=()
while IFS= read -r -d '' source; do
    case "$source" in
        *.swift|*.ts|*.js|*.mjs|*.sh|*.py) sources+=("$source");;
    esac
done < <(git --no-optional-locks ls-files -z mac/helper/Sources scripts)
# Transfer the exact historical build used by the failure bench. The remote snapshot has no .git.
history=()
fixture_dir=$(mktemp -d)
trap 'rm -r -- "$fixture_dir"' EXIT
mkdir -p "$fixture_dir/box/bridge-tests"
old_link() {
    dist=$(BUILT_AT_CACHE="$fixture_dir/cache" scripts/built-at.sh "$1")
    cat "$dist/kuvo-link.js"
}
ref_file=box/bridge-tests/failure-bench-legacy-ref.txt
if [[ -f "$ref_file" ]] && { [[ ${#files[@]} -eq 0 ]] || [[ " ${files[*]} " == *" bridge-tests/devices-two-boxes.test.js "* || " ${files[*]} " == *" bridge-tests/failure-bench-legacy.test.js "* ]]; }; then
    read -r ref < "$ref_file"
    [[ "$ref" =~ ^[0-9a-f]{40}$ ]] || { echo "Invalid legacy build reference" >&2; exit 1; }
    legacy=box/bridge-tests/.failure-bench-legacy.mjs
    old_link "$ref" > "$fixture_dir/$legacy"
    history=(-C "$fixture_dir" "$legacy")
fi
if [[ ${#files[@]} -eq 0 ]] || [[ " ${files[*]} " == *" bridge-tests/folder-version.test.js "* || " ${files[*]} " == *" bridge-tests/updates.test.js "* ]]; then
    layout=box/bridge-tests/.layout-4.mjs
    old_link "4330569c2f1c39eef166f65d2abb1b7256ef7dcb" > "$fixture_dir/$layout"
    history+=(-C "$fixture_dir" "$layout")
fi
if [[ ${#files[@]} -eq 0 ]] || [[ " ${files[*]} " == *" bridge-tests/update-real-keeper.test.js "* ]]; then
    baseline=box/bridge-tests/.layout-0.mjs
    old_link "11323b39316e0ec1f9b6281ab9bba9153db8e822" > "$fixture_dir/$baseline"
    history+=(-C "$fixture_dir" "$baseline")
fi
if [[ ${#files[@]} -eq 0 ]] || [[ " ${files[*]} " == *" bridge-tests/setup-handover.test.js "* ]]; then
    read -r neighbor < box/bridge-tests/recovery-neighbor-ref.txt
    [[ "$neighbor" =~ ^[0-9a-f]{40}$ ]] || { echo "Invalid recovery neighbor reference" >&2; exit 1; }
    recovery=box/bridge-tests/.recovery-neighbor.mjs
    old_link "$neighbor" > "$fixture_dir/$recovery"
    history+=(-C "$fixture_dir" "$recovery")
    beta35=box/bridge-tests/.beta35-owner.mjs
    old_link "fccda2de1" > "$fixture_dir/$beta35"
    history+=(-C "$fixture_dir" "$beta35")
fi
if [[ ${#files[@]} -eq 0 ]] || [[ " ${files[*]} " == *" bridge-tests/setup-handover.test.js "* || " ${files[*]} " == *" bridge-tests/resident-update-pilot.test.js "* ]]; then
    read -r split < box/bridge-tests/split-launch-ref.txt
    [[ "$split" =~ ^[0-9a-f]{40}$ ]] || { echo "Invalid split launch reference" >&2; exit 1; }
    link=box/bridge-tests/.split-launch.mjs
    keeper=box/bridge-tests/.split-keeper.sh
    old_link "$split" > "$fixture_dir/$link"
    git --no-optional-locks show "$split:box/src/keeper/kuvo-start" > "$fixture_dir/$keeper"
    history+=(-C "$fixture_dir" "$link" "$keeper")
fi
COPYFILE_DISABLE=1 tar --no-xattrs --exclude node_modules --exclude .build -czf - \
    box/bridge-tests box/test/fixtures box/test/relay-lease.ts protocol/vectors catalog box/.bun-version \
    protocol/PROTOCOL.md protocol/compat-kinds.txt protocol/gate-required.txt box/src box/dist box/package.json box/bun.lock \
    "${sources[@]}" "${history[@]}" |
    ssh -o BatchMode=yes "$HOST" "$command"
