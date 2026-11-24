#!/bin/sh
# Runs a heavy job (a build, a test run) once this Mac has room for it:
#
#   scripts/heavy.sh <mac|mac-build|box|rust|rust-box> <command> [arguments…]
#
#   mac        the legacy single-slot lane for older callers
#   mac-build  desktop bundles and Swift helper builds or tests. Two slots.
#   box        the box's whole suites (bun test, node --test) and runs on a test machine. Two slots,
#              as mac-build has (below).
#   rust       Desktop Cargo builds, tests and shots. One slot across all checkouts.
#   rust-box   Box Cargo builds and tests. Two slots across all checkouts (each worktree has its own target).
#
# Jobs of one kind share their build folders, which two builds at once break, so each slot of a kind
# (mac, rust and rust-box have one; mac-build and box two, below) runs one job at a time, each waiting on its
# slot's lock in this user's own temporary folder (getconf's, not $TMPDIR, which a caller may point
# elsewhere). Jobs of different kinds run side by side. Before it joins its lane's line, a new job waits while the Mac is thrashing: swapping more than
# KUVO_HEAVY_SWAP MB a second (10 unless set) or its memory pressure critical, for
# KUVO_HEAVY_MEMORY_WAIT_FOR (2 minutes) at most (below; KUVO_HEAVY_MEMORY=0 for never). It waits
# for a busy Mac (its load over the last minute above KUVO_HEAVY_LOAD) or for less than
# KUVO_HEAVY_MEMORY% of memory available only when those are set: low priority (below) is
# what keeps the Mac usable, and on the Mini, with a dozen sessions, both were true all day, so jobs
# only waited. These waits happen before a job holds its lane's lock.
#
# mac-build has two slots the same way, for every caller (no older script says mac-build, so there is
# no one to keep apart): slot 1 is kuvo-code-heavy-mac-build-<uid>.lock, slot 2
# kuvo-code-heavy-mac-build-2-<uid>.lock, and a job takes whichever is free first. It finds out which
# in KUVO_HEAVY_SLOT (1 or 2) and KUVO_HEAVY_BUILD_SUFFIX ("-1" or "-2"): build in
# KuvoShared-MacBuild$KUVO_HEAVY_BUILD_SUFFIX. Neither slot uses KuvoShared-MacBuild itself, which a
# legacy caller may still build in, so
# nothing builds twice in one folder. (A package's .build and a checkout's build/ are its own to each
# checkout.) Never start a build in a folder that isn't the slot's.
#
# box has two slots the same way, for every caller: slot 1 is kuvo-code-heavy-box-<uid>.lock, the
# lock it always had (an older copy's job holds it alone, and a new one runs beside it in slot 2),
# slot 2 kuvo-code-heavy-box-2-<uid>.lock. A job finds out which in KUVO_HEAVY_SLOT (1 or 2); it has
# no build folder or simulator to choose. Two of its runs share nothing that either changes: each
# run of the box's suite (bun test) and the bridge's (node --test) has a home of its own in
# ~/.kuvo-code-test (test-safety.js, named by its pid) and scripts/test-box.sh a temporary folder of
# its own; their servers listen on port 0, their sockets are in those folders, and the built
# kuvo-link.js and kuvo-agents.js are only read; a run on a test machine (scripts/test-linux.sh) has
# a folder of its own in that machine's ~/.kuvo-code-test. Keep it so: a box or bridge test listens
# on port 0 and keeps to folders of its own.
#
# Jobs waiting for a lane wait in line, in its queue (kuvo-code-heavy-<kind>-<uid>.queue): first come,
# first served, except that a high-priority job goes before every normal one waiting (and after the
# high ones that came before it). It never stops or holds up a job already running: it only gets the
# next free slot. Whoever looks at the queue or changes it does so under its guard lock, one at a
# time, and a slot given to a job is its claim until its `lockf` holds it, so no job that came later
# slips in between (nor waits, told not to). A job that's gone (killed, even), or hasn't looked for
# 30 seconds (stopped), leaves the line. Older copies of this helper (another checkout's) don't
# queue: they wait on a slot's lock, as before, and take it the moment it frees, before the jobs in
# line, which look once a second. The order holds among these.
#
# A job inside one that holds its kind's lock already (a test run its caller started with this
# helper, in either slot) runs at once. So does one under the single lock every heavy job took before
# (kuvo-code-heavy-<uid>.lock). Its day is over (KUVO_HEAVY_OLD_UNTIL, the last day a job took it
# as it ran, so one in an older checkout couldn't start beside it): kinds run side by side.
#
#   scripts/heavy.sh --holds <kind>   whether this runs inside such a job already (exit 0), for
#                                     a script that takes its turn itself (scripts/test-app.sh)
#
#   KUVO_HEAVY_PRIORITY=high  go before the normal jobs waiting in line (normal, the default)
#   KUVO_HEAVY_WAIT=0   don't wait: say what it would wait for (a job running, or one in line before
#                       it with a slot free), and exit 75, without waiting for a job (for the
#                       queue's guard, two seconds at most)
#   KUVO_HEAVY_DIR      where the locks are (for the helper's own tests)
#
# Jobs never starve the box: every
# job runs at low priority, its children too (a job inside another gets it too). macOS:
# `taskpolicy -c utility nice -n 20`, utility QoS at the lowest nice: below the box's own processes,
# at default priority, which win over it, but on every core. Not background QoS (`taskpolicy -b`,
# as before #557): on Apple silicon that runs only on the efficiency cores, with its I/O throttled,
# and a cold Swift package build took 87 s under it against 49 s at utility and nice 20 on an idle Mini
# (six of its twelve cores efficiency ones), while a stand-in for the box (six CPU-bound processes
# at default priority) took 3.6 s alone, 3.5 s beside the background build, 4.4 s beside the utility
# one at nice 20 and 7 s beside one at utility alone (tried with xcodebuild builds and tests, and swift test). Linux: `nice -n 15 ionice -c3` (just `nice -n 15` without
# ionice). A job on an idle Mac runs as fast as ever; on a busy one it gives way.
#
# Jobs of the Xcode lanes (mac and mac-build) build with Xcode's compilation cache,
# one for every checkout's builds (scripts/xcode-cache.sh, beside this copy): content-addressed, so
# a build in a new worktree replays what an earlier build of the same code compiled, wherever it was,
# and never what another branch's code made. Their xcodebuild gets it through XCODE_XCCONFIG_FILE
# (unless the caller has one); KUVO_XCODE_CACHE=0 builds without it.
#
# Every job has a time limit, so one that hangs can't hold its kind's turn for hours: past it, the
# job is stopped (its process group, as below: TERM, then KILL), it says what was running, and
# heavy.sh exits 124. The clock starts when the command does: not while it waits in line for its
# slot (for as long as that takes), nor while it waits for room on the Mac. Generous, per kind: mac 45
# minutes, mac-build 90 (a build, a package's tests), box 40 (the
# box's and the bridge's suites, which may go together), rust and rust-box 90.
#
#   KUVO_HEAVY_LIMIT    this job's limit instead: 90m, 600s, 2h, or plain minutes; 0 for none
#
# A job inside one of its kind (above) runs within that one's limit; one of another kind has its
# own as well. In a terminal a non-Rust job has none: you're there to stop it. Older copies of this helper
# start their jobs with no limit, and take turns with these as before.
#
# The canonical copy runs jobs when it supports their lane: $HOME/Projects/kuvo-ai/scripts/heavy.sh (or
# KUVO_HEAVY_CANONICAL). Any other copy (a worktree's, a checkout of an older commit's) that finds it
# there hands the job to it at once, with the same arguments and environment, so a fix to how jobs
# take turns reaches every caller, whichever checkout it runs from. KUVO_HEAVY_CANONICAL= (empty)
# runs the copy itself, for the helper's own tests and for trying a change to it.
#
#   scripts/heavy.sh status   a line for each lane: who holds each slot, since when, whether its
#                             command runs, and how many wait in line, the longest since when
#
# A job that holds a slot starts its command at once: nothing in this copy waits while it holds a
# lane's lock. Each job says so in its lane's queue folder (holder-<slot>: its pid, when it took the
# slot and when its command started, and its folder), which status reads; a holder that says
# nothing is an older copy, and status looks at what it runs instead. Every job that waits in line
# says, once, when its slot's holder has held it for 2 minutes without its command running.
set -eu

# During rollout, a worktree with a new Cargo lane can use it before the canonical copy gets it.
case "${1:-}" in --supports-rust | --supports-rust-box) exit 0 ;; esac

# Not the canonical copy: the job is the canonical copy's (once: that one never hands it on).
CANONICAL="${KUVO_HEAVY_CANONICAL-$HOME/Projects/kuvo-ai/scripts/heavy.sh}"
if [ -n "$CANONICAL" ] && [ -z "${KUVO_HEAVY_HANDED:-}" ] && [ -f "$CANONICAL" ] && ! [ "$0" -ef "$CANONICAL" ]; then
    case "${1:-}:${2:-}" in
        rust-box:* | --holds:rust-box | status:)
            if /bin/sh "$CANONICAL" --supports-rust-box >/dev/null 2>&1; then
                KUVO_HEAVY_HANDED=1 exec /bin/sh "$CANONICAL" "$@"
            fi
            ;;
        rust:* | --holds:rust)
            if /bin/sh "$CANONICAL" --supports-rust >/dev/null 2>&1; then
                KUVO_HEAVY_HANDED=1 exec /bin/sh "$CANONICAL" "$@"
            fi
            ;;
        *) KUVO_HEAVY_HANDED=1 exec /bin/sh "$CANONICAL" "$@" ;;
    esac
fi
# (What it runs hands its jobs on again: a job inside this one may be another checkout's.)
unset KUVO_HEAVY_HANDED

usage() {
    echo "Usage: scripts/heavy.sh <mac|mac-build|box|rust|rust-box> <command> [arguments…], or --holds <kind>, or status" >&2
    exit 64
}

# How long ago, said short.
ago() { if [ "$1" -ge 3600 ]; then echo "$(($1 / 3600))h$((($1 % 3600) / 60))m"; elif [ "$1" -ge 60 ]; then echo "$(($1 / 60))m"; else echo "$1s"; fi; }

# Who holds slot $2 of a lane (its lock $1, its queue folder $3), in `slot_said`, and in `idle` whether
# its command isn't running (since `idle_from`, when its own say tells): its own say (holder-<slot>, which this copy writes), or, from an older
# copy, which says nothing, what runs under it (only `sleep`: it waits for room under the lock).
# Empty when the slot is free.
describe_slot() {
    slot_said=
    idle=
    idle_from=
    if lockf -ks -t 0 "$1" true 2>/dev/null; then
        if [ -f "$1.running" ] && ! lockf -ks -t 0 "$1.running" true 2>/dev/null; then
            slot_said="stopping its command"
        fi
        return 0
    fi
    pid=$(lsof -t "$1" 2>/dev/null | head -n 1)
    now=$(date +%s)
    if [ -f "$3/holder-$2" ] && read -r job holder taken started folder < "$3/holder-$2" 2>/dev/null &&
        [ "$holder" = "${pid:-$holder}" ] && kill -0 "$holder" 2>/dev/null; then
        if [ "${started:-0}" -gt 0 ]; then
            slot_said="pid $holder ($folder), held $(ago $((now - taken))), running $(ago $((now - started)))"
        else
            idle=1
            idle_from=$taken
            slot_said="pid $holder ($folder), held $(ago $((now - taken))), not running its command"
        fi
    elif [ -n "$pid" ]; then
        inner=$(pgrep -P "$pid" 2>/dev/null | head -n 1)
        kids=$(pgrep -l -P "${inner:-0}" 2>/dev/null | awk '{ print $2 }' | sort -u | tr '\n' ' ')
        since=$(ps -o etime= -p "$pid" 2>/dev/null | tr -d ' ' || true)
        case "$kids" in
            "" | "sleep ")
                idle=1
                slot_said="pid $pid (an older heavy.sh)${since:+, held for $since}, not running its command: it waits for room under the lock"
                ;;
            *) slot_said="pid $pid (an older heavy.sh)${since:+, held for $since}, running" ;;
        esac
    else
        slot_said="held (lsof can't say by whom)"
    fi
}

# Each lane's health (`status`): for each slot, who holds it, since when and whether its command
# runs, and the line's length and its longest wait. It only reads: locks, queue folders, processes.
status() {
    dir="${KUVO_HEAVY_DIR:-$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null || echo /tmp/)}"
    dir="${dir%/}"
    uid=$(id -u)
    for lane in mac mac-build box rust rust-box; do
        queue="$dir/kuvo-code-heavy-$lane-$uid.queue"
        line="$lane:"
        for n in 1 2; do
            lock="$dir/kuvo-code-heavy-$lane-$uid.lock"
            [ "$n" = 1 ] || lock="$dir/kuvo-code-heavy-$lane-2-$uid.lock"
            [ -f "$lock" ] || continue
            describe_slot "$lock" "$n" "$queue"
            line="$line slot $n ${slot_said:-free};"
        done
        waiting=0
        oldest=
        for entry in "$queue"/[01]-*; do
            [ -f "$entry" ] || continue
            waiting=$((waiting + 1))
            at=$(stat -f %B "$entry" 2>/dev/null || stat -c %W "$entry" 2>/dev/null || echo 0)
            if [ "$at" -gt 0 ] && { [ -z "$oldest" ] || [ "$at" -lt "$oldest" ]; }; then oldest=$at; fi
        done
        if [ "$waiting" -gt 0 ]; then
            line="$line $waiting waiting${oldest:+, the longest for $(ago $(($(date +%s) - oldest)))}"
        else
            line="$line none waiting"
        fi
        echo "$line"
    done
}
if [ "${1:-}" = status ] && [ $# -eq 1 ]; then
    status
    exit 0
fi

[ $# -ge 2 ] || usage
holds=
if [ "$1" = --holds ]; then
    holds=1
    shift
fi
kind=$1
shift
case "$kind" in mac | mac-build | box | rust | rust-box) ;; *) usage ;; esac
cargo_lane=
case "$kind" in rust | rust-box) cargo_lane=1 ;; esac

DIR="${KUVO_HEAVY_DIR:-$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null || echo /tmp/)}"
DIR="${DIR%/}"
LOCK="$DIR/kuvo-code-heavy-$kind-$(id -u).lock"
# The mac-build, box and rust-box lanes have a second slot.
LOCK2=
SLOTS=1
case "$kind" in
    mac-build | box | rust-box)
        LOCK2="$DIR/kuvo-code-heavy-$kind-2-$(id -u).lock"
        SLOTS=2
        ;;
esac
OLD="$DIR/kuvo-code-heavy-$(id -u).lock"
MAX_LOAD="${KUVO_HEAVY_LOAD:-}"
WAITS="${KUVO_HEAVY_WAIT:-1}"
case "$kind" in mac) DEFAULT_LIMIT=45m ;; mac-build | rust | rust-box) DEFAULT_LIMIT=90m ;; box) DEFAULT_LIMIT=40m ;; esac

# What every job runs under, so it never takes the CPU or the disk from the box: low priority (a
# prefix: `$LOW "$@"`; its children keep it). macOS: utility QoS (lower CPU and I/O priority than
# the box's default, on every core: `nice` alone leaves the I/O alone) at nice 20. Linux: nice and
# ionice.
LOW=
case "$(uname -s 2>/dev/null)" in
    Darwin)
        if command -v taskpolicy >/dev/null 2>&1; then LOW="taskpolicy -c utility nice -n 20"; else LOW="nice -n 15"; fi
        ;;
    *)
        # (ionice can be refused, in a container: then nice alone.)
        if command -v ionice >/dev/null 2>&1 && ionice -c3 true >/dev/null 2>&1; then LOW="nice -n 15 ionice -c3"; else LOW="nice -n 15"; fi
        ;;
esac

# Whether `ps` answers. A sandbox can refuse it (Auto mode), and then no process can be asked what
# it runs under.
can_ps() { [ -n "$(ps -o command= -p $$ 2>/dev/null)" ]; }

# Whether a process this one runs under holds `lock` (a `lockf … <lock> …` above it). Without `ps`,
# the jobs this helper started tell who holds what (KUVO_HEAVY_HELD, "<lock>:<pid of its lockf>"
# entries), and the claim counts only while that process still runs and the lock is held: a leftover
# one doesn't.
held() {
    if can_ps; then
        pid=$$
        while [ "${pid:-1}" -gt 1 ]; do
            case "$(ps -o command= -p "$pid" 2>/dev/null)" in "lockf "*" $1 "*) return 0 ;; esac
            pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
        done
        return 1
    fi
    for claim in ${KUVO_HEAVY_HELD:-}; do
        [ "${claim%:*}" = "$1" ] || continue
        kill -0 "${claim##*:}" 2>/dev/null && ! lockf -ks -t 0 "$1" true && return 0
    done
    return 1
}

# Whether this process is the one a job's `lockf` just started for its turn on lock $1 (below):
# `ps` says so from the lock above it; without it, the job named its own process, which is lockf's
# parent now.
just_taken() {
    if can_ps; then
        held "$1"
    else
        [ "${KUVO_HEAVY_HOLDING:-}" = "$1:$PPID" ] && ! lockf -ks -t 0 "$1" true
    fi
}

# Read the slot from the lock, including for jobs nested in the same lane.
slot_env() {
    case "$kind" in
        box)
            unset KUVO_HEAVY_BUILD_SUFFIX
            if [ "$1" = "$LOCK2" ]; then export KUVO_HEAVY_SLOT=2; else export KUVO_HEAVY_SLOT=1; fi
            ;;
        mac-build)
            if [ "$1" = "$LOCK2" ]; then
                export KUVO_HEAVY_SLOT=2 KUVO_HEAVY_BUILD_SUFFIX=-2
            else
                export KUVO_HEAVY_SLOT=1 KUVO_HEAVY_BUILD_SUFFIX=-1
            fi
            ;;
    esac
}

# Whether this runs inside a job of this kind (in either slot) or of the old single lock.
holds_any() {
    held "$LOCK" || { [ -n "$LOCK2" ] && held "$LOCK2"; } || { [ -z "$cargo_lane" ] && held "$OLD"; }
}

# Not waiting (KUVO_HEAVY_WAIT=0): says what for, and stops.
cannot_wait() {
    echo "$1" >&2
    exit 75
}

if [ -n "$holds" ]; then
    holds_any
    exit
fi

# The Xcode lanes' jobs build with Xcode's compilation cache, one for every checkout
# (scripts/xcode-cache.sh and its xcconfig, beside this copy: XCODE_XCCONFIG_FILE, unless the caller
# has one), so a build in a new worktree replays what another worktree's build compiled instead of
# compiling it all again. Not the box's: it runs no xcodebuild.
case "$kind" in
    mac | mac-build)
        if [ -f "$(dirname "$0")/xcode-cache.sh" ] && . "$(dirname "$0")/xcode-cache.sh"; then xcode_cache "$(dirname "$0")"; fi
        ;;
esac

# A time like 90m, 600s or 2h (plain: minutes), in seconds; $2 names the variable it came from.
# (Its number in base 10, whatever zeros lead it, and at most six digits.)
seconds_of() {
    case "$1" in
        *[!0-9smh]* | "" | [smh]* | *[smh]?*) ;;
        *)
            number=${1%[smh]}
            while [ "${number#0}" != "$number" ] && [ "${#number}" -gt 1 ]; do number=${number#0}; done
            if [ "${#number}" -le 6 ]; then
                case "$1" in *s) echo "$number" ;; *h) echo $((number * 3600)) ;; *) echo $((number * 60)) ;; esac
                return
            fi
            ;;
    esac
    echo "$2 is a time like 90m, 600s or 2h, or 0 for none (it's $1)." >&2
    exit 64
}
GIVEN_LIMIT="${KUVO_HEAVY_LIMIT:-$DEFAULT_LIMIT}"
LIMIT=$(seconds_of "$GIVEN_LIMIT" KUVO_HEAVY_LIMIT) || exit 64
# How long a slot's holder may hold it without running its command before those in line say so.
STUCK_AFTER="${KUVO_HEAVY_STUCK_AFTER:-120}"
# Memory a new job waits for (below), if asked: the share of it available, in percent (unset: none,
# only swapping and critical pressure count; 0: it doesn't wait for memory at all).
MIN_FREE="${KUVO_HEAVY_MEMORY:-}"
case "$MIN_FREE" in
    "") ;;
    *[!0-9]* | ????*) MIN_FREE=bad ;;
esac
if [ "$MIN_FREE" = bad ] || [ "${MIN_FREE:-0}" -gt 100 ]; then
    echo "KUVO_HEAVY_MEMORY is a share of memory in percent, 0 to 100 (it's ${KUVO_HEAVY_MEMORY:-})." >&2
    exit 64
fi
# Swapping, in MB a second, that counts as short of memory too.
SWAP_MB="${KUVO_HEAVY_SWAP:-10}"
case "$SWAP_MB" in "" | *[!0-9]* | ???????*) echo "KUVO_HEAVY_SWAP is MB a second (it's $SWAP_MB)." >&2; exit 64 ;; esac
MEMORY_SHORT="$DIR/kuvo-code-heavy-memory-short-$(id -u)"
GIVEN_MEMORY_WAIT="${KUVO_HEAVY_MEMORY_WAIT_FOR:-2m}"
MEMORY_WAIT_FOR=$(seconds_of "$GIVEN_MEMORY_WAIT" KUVO_HEAVY_MEMORY_WAIT_FOR) || exit 64
# A job's place in its lane's queue: high goes before normal (0 sorts before 1).
case "${KUVO_HEAVY_PRIORITY:-normal}" in
    normal) PRIORITY=1 ;;
    high) PRIORITY=0 ;;
    *) echo "KUVO_HEAVY_PRIORITY is high or normal (it's $KUVO_HEAVY_PRIORITY)." >&2; exit 64 ;;
esac

# The lane's queue: a folder of its own.
#   <priority>-<number>-<pid>-<slots>  a job in line: 0 high, 1 normal; its number, in the order
#                     jobs came; its process; the slots it may take (2: either, 1: slot 1 only).
#                     It holds when the job last looked (each look, about once a second).
#   sequence          the last number given
#   claim-1, claim-2  the process a slot was just given to and when, until its `lockf` holds it
#   guard             the lock whoever reads or changes the queue holds, for milliseconds
QUEUE="$DIR/kuvo-code-heavy-$kind-$(id -u).queue"
# Whose turn it is, run under the guard ($1 the queue, $2 and $3 the slots' locks, $4 this job's
# entry or "new", $5 its priority, $6 its pid, $7 the slots it may take, $8 0 when it doesn't wait).
# A slot given to this job in the last 30 seconds (its last look was cut short) is its own still; a
# new job gets its entry (or finds the one it had, as it was). A job's look keeps its place; one
# that's gone, or hasn't looked for 30 seconds (stopped, or its pid now another process's), leaves
# the line, and comes back with its number when it looks again. A process is gone only when there's no such process: a
# sandbox may refuse to signal one that runs. A slot is free when its lock isn't held and no one has
# a claim on it (one older than 30 seconds, on a lock no one holds, is none). The free slots go to the
# jobs in line in order, each to the first that may take it; a job that may take either takes slot
# 1, or slot 2 when both are free and one that may take only slot 1 waits behind it. Prints "slot
# <n>" when one is this job's (its claim written, its entry gone), else "wait <entry>", or, told not
# to wait, "busy" (no slot it may take is free) or "ahead" (one is, for a job in line before it).
# shellcheck disable=SC2016 # (its own script: it expands its own arguments)
TURN='
queue=$1 lock1=$2 lock2=$3 ticket=$4 priority=$5 me=$6 slots=$7 waits=$8
export LC_ALL=C
cd "$queue" || exit 1
now=$(date +%s)
gone() {
    ! kill -0 "$1" 2>/dev/null && case "$(kill -0 "$1" 2>&1)" in *"No such process"*) true ;; *) false ;; esac
}
number() {
    num=$1
    case "$num" in "" | *[!0-9]*) num=0 ;; esac
    while [ "${num#0}" != "$num" ]; do num=${num#0}; done
    num=${num:-0}
}
for n in 1 2; do
    owner=
    since=
    [ ! -f "claim-$n" ] || read -r owner since < "claim-$n" || true
    [ "$owner" = "$me" ] || continue
    number "$since"
    if [ "$num" -gt 0 ] && [ $((now - num)) -le 30 ]; then
        for entry in [01]-*; do
            rest=${entry#?-}
            owner=${rest#*-}
            [ "${owner%-*}" != "$me" ] || rm -f "$entry"
        done
        echo "slot $n"
        exit 0
    fi
    rm -f "claim-$n"
done
last=0
[ ! -f sequence ] || read -r last < sequence || true
number "$last"
last=$num
for entry in [01]-*; do
    [ -f "$entry" ] || continue
    rest=${entry#?-}
    owner=${rest#*-}
    owner=${owner%-*}
    seen=
    read -r seen < "$entry" || true
    number "$seen"
    seen=$num
    if gone "$owner" || [ "$seen" = 0 ] || [ $((now - seen)) -gt 30 ]; then
        rm -f "$entry"
    elif [ "$ticket" = new ] && [ "$owner" = "$me" ] && [ "${entry%%-*}" = "$priority" ] && [ "${entry##*-}" = "$slots" ]; then
        ticket=$entry
    fi
    number "${rest%%-*}"
    [ "$num" -le "$last" ] || last=$num
done
if [ "$ticket" = new ]; then
    last=$((last + 1))
    echo "$last" > "sequence.$me" && mv -f "sequence.$me" sequence
    ticket=$(printf "%s-%012d-%s-%s" "$priority" "$last" "$me" "$slots")
fi
echo "$now" > "$ticket"
free=
for n in 1 2; do
    lock=$lock1
    [ "$n" = 1 ] || lock=$lock2
    [ -n "$lock" ] || continue
    if [ -f "claim-$n" ]; then
        owner=
        since=
        read -r owner since < "claim-$n" || true
        number "$since"
        since=$num
        if [ -n "$owner" ] && ! gone "$owner"; then
            [ $((now - since)) -gt 30 ] || continue
            lockf -ks -t 0 "$lock" true || continue
        fi
        rm -f "claim-$n"
    fi
    lockf -ks -t 0 "$lock" true || continue
    # The Rust command may still be unwinding after its outer lockf was cancelled.
    [ ! -f "$lock.running" ] || lockf -ks -t 0 "$lock.running" true || continue
    free=$free$n
done
case "$slots:$free" in 2:?* | 1:*1*) open=1 ;; *) open= ;; esac
singles=0
for entry in [01]-*; do
    if [ -f "$entry" ] && [ "${entry##*-}" = 1 ]; then singles=$((singles + 1)); fi
done
mine=
for entry in [01]-*; do
    [ -n "$free" ] || break
    [ -f "$entry" ] || continue
    take=
    if [ "${entry##*-}" = 2 ]; then
        case "$free" in
            12) if [ "$singles" -gt 0 ]; then take=2; else take=1; fi ;;
            *) take=$free ;;
        esac
    else
        singles=$((singles - 1))
        case "$free" in *1*) take=1 ;; esac
    fi
    [ -n "$take" ] || continue
    case "$take" in 1) free=${free#1} ;; 2) free=${free%2} ;; esac
    if [ "$entry" = "$ticket" ]; then
        mine=$take
        break
    fi
done
if [ -n "$mine" ]; then
    echo "$me $now" > "claim-$mine"
    rm -f "$ticket"
    echo "slot $mine"
elif [ "$waits" = 0 ]; then
    rm -f "$ticket"
    if [ -n "$open" ]; then echo ahead; else echo busy; fi
else
    echo "wait $ticket"
fi
'

# The Mac busy: wait until it isn't.
load() { sysctl -n vm.loadavg | awk '{ print $2 }'; }
busy() { [ -n "$MAX_LOAD" ] && [ "$(load | awk -v max="$MAX_LOAD" '{ print ($1 > max) }')" = 1 ]; }

# Short of memory: with builds and the engine going, the Mac ran
# out of memory long before CPU (15 of 16 GB used, 9 GB swapped, the CPU half idle), and the load
# grew with the swapping. So a new job (not one started inside a job already running) also waits
# while the Mac thrashes, by what the system does and says: it's swapping (pages in and out, more
# than $SWAP_MB MB a second since the last look, from vm_stat) or its memory pressure is critical
# (kern.memorystatus_vm_pressure_level), and, when KUVO_HEAVY_MEMORY asks, while less than
# $MIN_FREE% of it is available (kern.memorystatus_level, what `memory_pressure -Q` calls free; with
# 16 GB and a dozen sessions it's below 20% all day, so it isn't asked unless set). Not swap used:
# it stays long after the pressure is gone. Short, it waits until two looks in a row find it isn't,
# so the jobs that waited don't all start at once, and for $MEMORY_WAIT_FOR at most from when any
# job found it short ($MEMORY_SHORT): then the load alone decides (when it's asked to).
memory_level() { sysctl -n kern.memorystatus_vm_pressure_level 2>/dev/null || echo 1; }
memory_free() { sysctl -n kern.memorystatus_level 2>/dev/null || echo 100; }
page_size=$(vm_stat 2>/dev/null | sed -n 's/.*page size of \([0-9]*\) bytes.*/\1/p')
page_size=${page_size:-16384}
# Pages swapped in and out so far.
swapped() { vm_stat 2>/dev/null | awk '/^Swapins/ { gsub("\\.", "", $2); n += $2 } /^Swapouts/ { gsub("\\.", "", $2); n += $2 } END { print n + 0 }'; }
swap_pages=
swap_at=
swap_rate=0
# Looks again: how much it swapped a second since the last look (the first, over three seconds).
look_at_memory() {
    if [ -z "$swap_pages" ]; then
        swap_pages=$(swapped)
        swap_at=$(date +%s)
        sleep 3
    fi
    pages=$(swapped)
    at=$(date +%s)
    seconds=$((at - swap_at))
    [ "$seconds" -gt 0 ] || seconds=1
    swap_rate=$(((pages - swap_pages) * page_size / 1000000 / seconds))
    swap_pages=$pages
    swap_at=$at
}
memory_short() {
    [ "$swap_rate" -gt "$SWAP_MB" ] || [ "$(memory_level)" -ge 4 ] || { [ -n "$MIN_FREE" ] && [ "$(memory_free)" -lt "$MIN_FREE" ]; }
}
memory_said() {
    case "$(memory_level)" in 1) pressure=normal ;; 2) pressure=warn ;; 4) pressure=critical ;; *) pressure="level $(memory_level)" ;; esac
    echo "swapping ${swap_rate} MB a second, pressure $pressure, $(memory_free)% available"
}
# How long memory has been short, for any job: since the first that found it so.
short_for() {
    since=
    [ -f "$MEMORY_SHORT" ] && read -r since < "$MEMORY_SHORT" 2>/dev/null
    if [ -z "${since:-}" ]; then
        since=$(date +%s)
        printf '%s\n' "$since" > "$MEMORY_SHORT.$$" && mv -f "$MEMORY_SHORT.$$" "$MEMORY_SHORT"
    fi
    echo $(($(date +%s) - since))
}
# Why it waits now, if it does (in `why`): memory, or the load.
was_short=
calm=0
decide() {
    why=
    if [ "${MIN_FREE:-1}" -gt 0 ] && [ -z "${NESTED:-}" ]; then
        look_at_memory
        if memory_short; then
            calm=0
            was_short=1
            [ "$(short_for)" -lt "$MEMORY_WAIT_FOR" ] && why=memory && return
        elif [ -n "$was_short" ] && [ "$calm" -lt 1 ]; then
            # (Once more, so it's not a moment's calm.)
            calm=$((calm + 1))
            why=memory
            return
        else
            rm -f "$MEMORY_SHORT"
        fi
    fi
    if busy; then why=load; fi
}

# Waits until there's room for a new job: called before it queues for its lane, never while it
# holds a lane's lock, so a job waiting here holds up no one behind it in its lane.
wait_for_room() {
    said=
    decide
    while [ -n "$why" ]; do
        if [ "$why" = memory ]; then
            [ "$WAITS" = 0 ] && cannot_wait "The Mac is short of memory ($(memory_said))."
            [ "$said" = memory ] || echo "Waiting for memory: the Mac is short of it ($(memory_said)), for $GIVEN_MEMORY_WAIT at most…" >&2
        else
            [ "$WAITS" = 0 ] && cannot_wait "The Mac is busy (load $(load), over $MAX_LOAD)."
            [ "$said" = load ] || echo "Waiting while the Mac is busy (load $(load), over $MAX_LOAD)…" >&2
        fi
        said=$why
        sleep 15
        decide
    done
}


# Started inside another heavy job (of another kind: one of its own kind runs at once, below),
# which says so in KUVO_HEAVY_HELD while it runs: a part of a job already running.
NESTED=
for claim in ${KUVO_HEAVY_HELD:-}; do kill -0 "${claim##*:}" 2>/dev/null && NESTED=1; done

# The lock whose turn this process has just taken (below), if any.
TAKEN=
for candidate in "$LOCK" "$LOCK2"; do
    [ -n "$candidate" ] || continue
    case "${KUVO_HEAVY_HOLDING:-}" in "$candidate" | "$candidate":*) ;; *) continue ;; esac
    if just_taken "$candidate"; then
        TAKEN=$candidate
        break
    fi
done

# A Rust worker whose lockf died during handoff must not queue itself as a new job.
if [ -n "$cargo_lane" ] && [ -n "${KUVO_HEAVY_HOLDING:-}" ] && [ -z "$TAKEN" ]; then
    exit 143
fi

if [ -n "$TAKEN" ]; then
    # Its turn, just taken (below). Its claim on the slot is over: the slot's lock holds it now.
    # (Nothing else claims a slot whose lock is held, so this reads and removes it safely.)
    claim="$QUEUE/claim-1"
    [ "$TAKEN" = "$LOCK" ] || claim="$QUEUE/claim-2"
    if [ -f "$claim" ] && read -r owner since < "$claim" 2>/dev/null && [ "$owner" = "$PPID" ]; then
        rm -f "$claim"
    fi
    unset KUVO_HEAVY_HOLDING
    if [ -n "$cargo_lane" ]; then
        # The outer lockf exits immediately on cancellation. This worker and
        # its watchdog keep a second lock until Cargo's process group is cleaned up.
        exec 8>"$TAKEN.running"
        if [ "$WAITS" = 0 ]; then lockf -t 0 8; else lockf 8; fi
    fi
    # What runs under this turn knows it holds the lock, for when `ps` can't say.
    KUVO_HEAVY_HELD="${KUVO_HEAVY_HELD:-} $TAKEN:$PPID"
    [ -z "$LOCK2" ] || slot_env "$TAKEN"
    # It says it holds the slot (status reads it), and, once it runs, since when its command does.
    holder_file="$QUEUE/holder-1"
    [ "$TAKEN" = "$LOCK" ] || holder_file="$QUEUE/holder-2"
    taken_at=$(date +%s)
    say_holder() {
        { printf '%s %s %s %s %s\n' "$$" "$PPID" "$taken_at" "$1" "$PWD" > "$holder_file.$$" && mv -f "$holder_file.$$" "$holder_file"; } 2>/dev/null || true
    }
    say_holder 0
    # While jobs were still started the old way, under one lock for every kind (older checkouts'
    # test-app.sh), one of these waited for them, and held that lock too as it ran.
    if [ -z "$cargo_lane" ] && [ "$(date +%Y%m%d)" -le "${KUVO_HEAVY_OLD_UNTIL:-20261026}" ]; then
        if ! lockf -ks -t 0 "$OLD" true; then
            [ "$WAITS" = 0 ] && cannot_wait "A heavy job started the old way is running."
            echo "Waiting for a heavy job started the old way to finish…" >&2
        fi
        old_way=1
        KUVO_HEAVY_HELD="$KUVO_HEAVY_HELD $OLD:$$"
    fi
    export KUVO_HEAVY_HELD
else
    # Inside a job that holds the lock already: on at once.
    if holds_any; then
        if [ -n "$LOCK2" ]; then
            if held "$LOCK2"; then slot_env "$LOCK2"; elif held "$LOCK"; then slot_env "$LOCK"; fi
        fi
        exec $LOW "$@"
    fi
    # Says once, for each slot it may take, when the slot's holder hasn't run its command for 2
    # minutes (an older copy waiting for room under the lock, say): a lane held that way stalls.
    idle_1=
    idle_2=
    told_1=
    told_2=
    stuck_say() {
        for n in 1 2; do
            [ "$n" = 1 ] || [ "$SLOTS" = 2 ] || continue
            lock=$LOCK
            [ "$n" = 1 ] || lock=$LOCK2
            [ -n "$lock" ] || continue
            describe_slot "$lock" "$n" "$QUEUE"
            if [ -z "$idle" ]; then
                eval "idle_$n="
                continue
            fi
            since=$(eval echo "\${idle_$n}")
            if [ -z "$since" ]; then
                since=$(date +%s)
                # (Its own say gives when it took the slot.)
                [ -z "$idle_from" ] || since=$idle_from
                eval "idle_$n=$since"
            fi
            if [ $(($(date +%s) - since)) -ge "$STUCK_AFTER" ] && [ -z "$(eval echo "\${told_$n}")" ]; then
                echo "heavy.sh: slot $n of the $kind lane has been held for $(ago $(($(date +%s) - since))) without its command running: $slot_said." >&2
                eval "told_$n=1"
            fi
        done
    }
    # Room on the Mac first, while it holds nothing.
    wait_for_room
    # Its kind's turn, through the lane's queue (TURN, above): in line until a slot is its own,
    # then that slot's lock, which it holds as it runs.
    ticket=new
    trap 'rm -f "$QUEUE/$ticket"; exit 129' HUP
    trap 'rm -f "$QUEUE/$ticket"; exit 130' INT
    trap 'rm -f "$QUEUE/$ticket"; exit 143' TERM
    said=
    failed=0
    while :; do
        mkdir -p "$QUEUE" 2>/dev/null || true
        # Told not to wait, it doesn't wait for the guard either, past a moment (its holders only
        # look at files, for milliseconds; lockf says 75 when its time is up).
        looked=0
        if [ "$WAITS" = 0 ]; then
            turn=$(lockf -k -t 2 "$QUEUE/guard" /bin/sh -c "$TURN" sh "$QUEUE" "$LOCK" "$LOCK2" "$ticket" "$PRIORITY" "$$" "$SLOTS" 0) || looked=$?
        else
            turn=$(lockf -k "$QUEUE/guard" /bin/sh -c "$TURN" sh "$QUEUE" "$LOCK" "$LOCK2" "$ticket" "$PRIORITY" "$$" "$SLOTS" 1) || looked=$?
        fi
        case "$turn" in
            "slot "*)
                slot=$LOCK
                [ "$turn" = "slot 1" ] || slot=$LOCK2
                # Its claim keeps every other queued job off the slot until this `lockf` holds it.
                # Only a caller outside the queue (an older script's plain `lockf`) can take it
                # meanwhile: then this one waits for it, or, told not to wait, exits 75.
                if [ "$WAITS" = 0 ]; then
                    KUVO_HEAVY_HOLDING="$slot:$$" exec lockf -k -t 0 "$slot" /bin/sh "$0" "$kind" "$@"
                fi
                KUVO_HEAVY_HOLDING="$slot:$$" exec lockf -k "$slot" /bin/sh "$0" "$kind" "$@"
                ;;
            "wait "*) ticket=${turn#wait } ;;
            busy)
                [ "$SLOTS" = 2 ] && cannot_wait "Another $kind job is running in each of its two slots."
                cannot_wait "Another $kind job is running."
                ;;
            ahead) cannot_wait "Another $kind job is waiting for its turn ahead of this one." ;;
            *)
                [ "$WAITS" = 0 ] && [ "$looked" = 75 ] && cannot_wait "Another $kind job is taking its turn (its queue is busy)."
                [ "$WAITS" = 0 ] && cannot_wait "heavy.sh can't read the $kind queue ($QUEUE)."
                failed=$((failed + 1))
                [ "$failed" != 10 ] || echo "heavy.sh: can't read the $kind queue ($QUEUE); still trying…" >&2
                ;;
        esac
        [ "$turn" = "" ] || failed=0
        [ -n "$said" ] || echo "Waiting for another $kind job to finish…" >&2
        said=1
        # Now and then, whether a slot it may take is held by a job whose command doesn't run.
        looks=$((${looks:-0} + 1))
        [ $((looks % 30)) != 1 ] || stuck_say
        sleep 1
    done
fi

# The job runs in a process group of its own, and the group ends with the job: once the command is
# over, or this helper is stopped, everything the command started that stays in its group is
# stopped too, so nothing is left running behind a finished or cancelled job. The group is stopped
# by its id, which is the job's pid: no `ps`, no search by name (a sandbox refuses `ps`, and a name
# matches other people's processes). TERM first, KILL for what's still there after about 3 seconds.
# A shell that gave the job no group of its own has no group by that id, and the kill finds nothing.
group=
watch=
early=
end_group() {
    [ -n "$group" ] && kill -0 -- "-$group" 2>/dev/null || return 0
    kill -s "$1" -- "-$group" 2>/dev/null || true
    tries=0
    while [ "$tries" -lt 30 ] && kill -0 -- "-$group" 2>/dev/null; do
        sleep 0.1
        tries=$((tries + 1))
    done
    ! kill -0 -- "-$group" 2>/dev/null || kill -s KILL -- "-$group" 2>/dev/null || true
}
end_watch() {
    [ -n "$watch" ] || return 0
    { kill -s KILL -- "-$watch" && wait "$watch"; } 2>/dev/null || true
}
stopped() {
    # Stopped before the job has started (between its start and its id being known): once it has.
    if [ -z "$group" ]; then
        early="$1 $2"
        return 0
    fi
    trap '' HUP INT TERM USR1
    end_group "$1"
    end_watch
    exit "$2"
}
# Past its time limit (the watcher, below, says so).
over_time() {
    trap '' HUP INT TERM USR1
    echo "heavy.sh: this $kind job ran past its time limit ($GIVEN_LIMIT) and was stopped: $*" >&2
    echo "heavy.sh: if it wasn't stuck, give it longer with KUVO_HEAVY_LIMIT (like KUVO_HEAVY_LIMIT=90m)." >&2
    end_group TERM
    end_watch
    exit 124
}

# In a terminal the job stays in the terminal's own group: one in a group of its own couldn't read
# the keyboard (a passphrase prompt would stop it), and Ctrl-C reaches the whole group there anyway.
# Supervise Rust jobs even in a terminal so cancelling the wrapper PID stops Cargo and frees its slot.
# (Its command starts now: whoever looks at its slot sees it running.)
[ -z "${holder_file:-}" ] || say_holder "$(date +%s)"
if [ -z "$cargo_lane" ] && { [ -t 0 ] || { true </dev/tty; } 2>/dev/null; }; then
    [ -n "${old_way:-}" ] && exec lockf -k "$OLD" $LOW "$@"
    exec $LOW "$@"
fi

trap 'stopped HUP 129' HUP
trap 'stopped INT 130' INT
trap 'stopped TERM 143' TERM
# (The command as it was given, for what over_time says.)
command_line=$*
trap 'over_time "$command_line"' USR1
# With job control (set -m) both this shell and the child it forks put the child in its group, and
# on a busy Mac the child's try now and then fails though the group is already its own: bash then
# says "child setpgid (N to N): Operation not permitted" on the stderr it forked with, which is the
# job's (#562: a test that expects a quiet job failed under load). The two children are forked with
# stderr going nowhere, and the job gets the real one back (kept as fd 9) as it starts. (`command`,
# so a stderr that's closed doesn't end this shell.) Until `set +m`, below, what this shell says
# goes nowhere too: nothing it runs there says anything.
command exec 9>&2 2>/dev/null || exec 9>/dev/null 2>/dev/null
set -m
if [ -n "$cargo_lane" ]; then
    # A detached child must not inherit the lock and keep the lane indefinitely.
    $LOW "$@" 8>&- 2>&9 9>&- &
elif [ -n "${old_way:-}" ]; then
    lockf -k "$OLD" $LOW "$@" 2>&9 9>&- &
else
    $LOW "$@" 2>&9 9>&- &
fi
group=$!
# `lockf` doesn't pass a signal on to its command, and lets go of the lock as it ends: this helper
# (the job's `lockf` is its parent) being stopped, even killed, would leave the job running. A
# watcher of its own, in a group of its own too, notices when the parent is gone, stops this shell
# if it's still there, and then the job's group itself if nothing else has: this shell may have been
# killed, and a helper above it may have stopped it before it was done.
parent=$PPID
me=$$
# The watcher keeps its time limit too: once it's up, it tells this shell (USR1), only while the
# parent still runs, so never a process that took a pid since.
(
    ticks=$((LIMIT * 5))
    while kill -0 "$parent" 2>/dev/null; do
        if [ "$LIMIT" -gt 0 ] && [ "$ticks" -ge 0 ]; then
            [ "$ticks" -gt 0 ] || kill -s USR1 "$me" 2>/dev/null || true
            ticks=$((ticks - 1))
        fi
        sleep 0.2
    done
    kill -s TERM "$me" 2>/dev/null || true
    waits=0
    while [ "$waits" -lt 100 ] && kill -0 "$me" 2>/dev/null; do
        sleep 0.1
        waits=$((waits + 1))
    done
    end_group TERM
) >/dev/null 2>&1 9>&- &
watch=$!
set +m
exec 2>&9 9>&-
[ -z "$early" ] || stopped ${early}
status=0
wait "$group" || status=$?
# (Over, by itself: the limit no longer counts.)
trap '' USR1
end_group TERM
end_watch
exit "$status"
