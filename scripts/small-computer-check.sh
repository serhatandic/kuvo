#!/usr/bin/env bash
# Run from the checkout to be measured. ENGINE points to an opencode binary on the Docker host.
set -euo pipefail
cd "$(dirname "$0")/.."
host=${HOST:-lokum-local}
engine=${ENGINE:-}
seconds=${IDLE_SECONDS:-300}
mode=${1:---check}
revision=${REVISION:-}
case "$mode" in --check|--measure) ;; *) echo 'Usage: scripts/small-computer-check.sh [--check|--measure]' >&2; exit 64;; esac
[[ "$seconds" =~ ^[1-9][0-9]*$ ]] || exit 64
if [[ "$mode" = --check && "$seconds" -lt 300 ]]; then
    echo 'The budget check needs at least 300 seconds of idle sampling.' >&2
    exit 64
fi
remote=$(cat <<'REMOTE'
set -euo pipefail
engine=$1 seconds=$2 mode=$3
if [[ -z "$engine" ]]; then
    engine=$(printf '%s\n' "$HOME"/.kuvo-code/engine/*/opencode | sort -V | tail -1)
fi
[[ -x "$engine" ]]
version=$(basename "$(dirname "$engine")")
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
tmp_before=$(ls -A /tmp)
home_before=$(ls -A "$HOME")
test_root="$HOME/.kuvo-code-test"
mkdir -p "$test_root"
scratch=$(mktemp -d "$test_root/small-computer.XXXXXXXX")
container=
cleanup() {
    status=$?
    trap - EXIT
    if [[ -n "$container" ]]; then sudo -n docker rm -f "$container" >/dev/null || status=1; fi
    rm -rf -- "$scratch" || status=1
    printf '/tmp new:\n'
    comm -13 <(printf '%s\n' "$tmp_before" | LC_ALL=C sort) <(ls -A /tmp | LC_ALL=C sort)
    printf 'home new:\n'
    comm -13 <(printf '%s\n' "$home_before" | LC_ALL=C sort) <(ls -A "$HOME" | LC_ALL=C sort)
    exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
build=$scratch/build
box=$scratch/home/.kuvo-code
mkdir -p "$build" "$box/versions/1.0.0-000000000000/bin" "$box/engine/$version" "$box/run" "$box/workspace" "$box/config/opencode" "$box/plugins"
ln -s versions/1.0.0-000000000000/bin "$box/bin"
printf '1.0.0\n' >"$box/bin/box-version"
tar xzf - -C "$build"
cp "$engine" "$box/engine/$version/opencode"
cp "$build"/box/src/keeper/kuvo-{start,launch,wake,npx,setsid} "$box/bin/"
sed -i "s/@ENGINE@/$version/g" "$box/bin/kuvo-start" "$box/bin/kuvo-launch"
cp "$build"/box/dist/kuvo-{link,agents,browser}.js "$box/bin/"
cp "$build"/box/src/plugins/kuvo-code.js "$box/plugins/"
printf 'PATH=/usr/local/bin:/usr/bin:/bin\nSHELL=/bin/bash\nLANG=C.UTF-8\nOPENCODE_CLIENT=desktop\n' >"$box/run/launch.env"
container=$(sudo -n docker create --cpus=2 --memory=2g --memory-swap=2g --init \
    --user "$(id -u):$(id -g)" -e HOME=/bench -e BUN_RUNTIME_TRANSPILER_CACHE_PATH=0 \
    --mount "type=bind,src=$scratch/home,dst=/bench" \
    --mount "type=bind,src=$build,dst=/build,readonly" \
    debian:trixie-slim /bin/bash -ec '
        cd "$HOME/.kuvo-code/workspace"
        sh ../bin/kuvo-start keep &
        exec env BUN_BE_BUN=1 "../engine/$1/opencode" --config=/dev/null --no-env-file /build/scripts/small-computer-scenario.mjs "$2" "$3"
    ' small-computer "$version" "$seconds" "$mode")
sudo -n docker start -a "$container"
exit "$(sudo -n docker inspect --format '{{.State.ExitCode}}' "$container")"
REMOTE
)
printf -v command 'bash -c %q -- %q %q %q' "$remote" "$engine" "$seconds" "$mode"
sources=(box/src/keeper box/src/plugins/kuvo-code.js)
bundles=(box/dist/kuvo-{link,agents,browser}.js)
if [[ -n "$revision" ]]; then
    # Overlay the current sampler onto an earlier release for comparison.
    staging=$(mktemp -d "${TMPDIR:-/tmp}/kuvo-sampler.XXXXXXXX")
    trap 'rm -rf -- "$staging"' EXIT
    git archive "$revision" "${sources[@]}" | tar xf - -C "$staging"
    dist=$(BUILT_AT_CACHE="$staging/cache" scripts/built-at.sh "$revision")
    mkdir -p "$staging/box/dist" "$staging/scripts"
    cp "$dist"/kuvo-{link,agents,browser}.js "$staging/box/dist/"
    cp scripts/small-computer-scenario.mjs "$staging/scripts/"
    cd "$staging"
else
    scripts/build-built.sh
fi
COPYFILE_DISABLE=1 tar --no-xattrs -czf - "${sources[@]}" "${bundles[@]}" scripts/small-computer-scenario.mjs |
    ssh -o BatchMode=yes "$host" "$command"
