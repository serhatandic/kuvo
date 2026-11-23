# Source from the repository root. Select the pinned build runtime, including for nested builders.
. scripts/real-app-guard.sh
bun_version=$(cat box/.bun-version)
build_bun_version() { BUN_BE_BUN=1 "$1" --version 2>/dev/null || true; }

if [ -z "${BUN:-}" ]; then
    for candidate in bun "$HOME/.bun/bin/bun" "$HOME/Library/Application Support/run.kuvo.code."*/engine/*/opencode; do
        candidate=$(command -v "$candidate") || continue
        if [ "$(build_bun_version "$candidate")" = "$bun_version" ]; then
            BUN=$candidate
            break
        fi
    done
fi
if [ -n "${BUN:-}" ]; then
    BUN=$(command -v "$BUN") || BUN=
    case "$BUN" in ""|/*) ;; *) BUN="$PWD/$BUN" ;; esac
    real_app_guard "$0" "$BUN" || exit 1
fi
if [ -z "${BUN:-}" ] || [ "$(build_bun_version "$BUN")" != "$bun_version" ]; then
    echo "$0 needs Bun $bun_version (box/.bun-version). Install it or set BUN to that version." >&2
    exit 1
fi
export BUN
