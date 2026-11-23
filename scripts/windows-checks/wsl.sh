#!/bin/bash
set -euo pipefail
umask 077

if [ "$1" = --inside ]; then
    shift
    run=$1 tools=$2 config=$3
    cd /
    mount --bind "$tools" /opt
    mount -o remount,bind,ro /opt
    mount --bind "$run" /tmp
    mount --bind /tmp/private-homes /home
    mount --bind /tmp/host-mounts /mnt
    mount --bind /tmp/runtime /run
    mount --bind /tmp/shm /dev/shm
    # Disable Windows interop even for a child that tries the launcher by absolute path.
    mount --bind /dev/null /init
    cd /tmp/repo/box
    test_home=/home/$(id -un)
    bun=/opt/bun-$4/bun
    node=/opt/node-$5/bin/node
    # The parent is PID 1 of the new namespace. All descendants, even detached holders,
    # are terminated by the kernel when it exits. Capabilities are only used for setup.
    exec setpriv --no-new-privs --bounding-set=-all --inh-caps=-all --ambient-caps=-all \
        env -i PATH="${bun%/*}:${node%/*}:/usr/bin:/bin" LANG=C.UTF-8 \
        HOME="$test_home" USERPROFILE="$test_home" APPDATA="$test_home/appdata" LOCALAPPDATA="$test_home/localdata" \
        TMPDIR=/tmp/t TEMP=/tmp/t TMP=/tmp/t XDG_CONFIG_HOME="$test_home/config" XDG_CACHE_HOME="$test_home/cache" \
        XDG_DATA_HOME="$test_home/data" XDG_STATE_HOME="$test_home/state" XDG_RUNTIME_DIR=/run \
        BUN_INSTALL_CACHE_DIR="$test_home/cache/bun" BUN_RUNTIME_TRANSPILER_CACHE_PATH=0 \
        npm_config_cache="$test_home/cache/npm" UV_CACHE_DIR="$test_home/cache/uv" \
        GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
        "$bun" /tmp/repo/.windows-checks/run.mjs "$config"
fi

run=$1 tools=$2 config=$3
repo=$run/repo
user=$(id -un)
[ "$(uname -m)" = x86_64 ] || { echo 'Only WSL x64 is pinned.' >&2; exit 1; }
[ ! -L "$tools" ] || { echo "Refusing linked tools root: $tools" >&2; exit 1; }
mkdir -p "$tools" "$run/download" "$run/private-homes/$user"/{appdata,localdata,config,cache,data,state} \
    "$run/host-mounts/wsl" "$run/runtime" "$run/t" "$run/shm"
# Ubuntu's resolver points through /mnt/wsl. Keep the resolver while masking Windows drives and WSLg.
cat /etc/resolv.conf > "$run/host-mounts/wsl/resolv.conf"
export TMPDIR="$run/download"

install_tool() {
    local name=$1 version=$2 suffix=$3 url=$4 sha=$5 binary=$6
    local target="$tools/$name-$version" archive="$run/download/$name.$suffix"
    if [ -L "$target" ]; then echo "Refusing linked tool: $target" >&2; return 1; fi
    if [ ! -x "$target/$binary" ]; then
        curl -q -fsSL "$url" -o "$archive"
        printf '%s  %s\n' "$sha" "$archive" | sha256sum -c -
        case "$suffix" in
            zip)
                python3 -m zipfile -e "$archive" "$run/download"
                chmod +x "$run/download/bun-linux-x64/bun"
                ;;
            tar.xz) tar -xJf "$archive" -C "$run/download" ;;
        esac
        # Publish a complete directory atomically. Another run may have won the same race.
        if ! mv -T -- "$run/download/$name"-* "$target" && [ ! -x "$target/$binary" ]; then
            return 1
        fi
        rm -- "$archive"
    fi
    [ "$(realpath -- "$target/$binary")" = "$target/$binary" ] || {
        echo "Refusing linked runtime: $target/$binary" >&2
        return 1
    }
}

# Read the shared versions and hashes without using an unpinned JS runtime.
mapfile -t pins < <(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["bun"]["linux-x64"], d["node"]["version"], d["node"]["linux-x64"], sep="\n")' "$repo/.windows-checks/pins.json")
bun=$(cat "$repo/box/.bun-version")
node=${pins[1]}
install_tool bun "$bun" zip "https://github.com/oven-sh/bun/releases/download/bun-v$bun/bun-linux-x64.zip" "${pins[0]}" bun
install_tool node "$node" tar.xz "https://nodejs.org/dist/v$node/node-v$node-linux-x64.tar.xz" "${pins[2]}" bin/node
# Pass versioned paths explicitly so a future pin can coexist with earlier caches.
exec /usr/bin/time -f 'Peak RSS (largest process): %M KiB' \
    unshare --map-current-user --keep-caps --mount --pid --fork --kill-child --mount-proc \
    /bin/bash "$repo/.windows-checks/wsl.sh" --inside "$run" "$tools" "$config" "$bun" "$node"
