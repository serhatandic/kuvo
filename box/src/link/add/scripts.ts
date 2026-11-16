// The scripts a box runs on a computer it sets up, over SSH (`ssh.ts`), each whole and run by `sh`
// there: what's on it, the release's files and the engine put in place, its start and its start
// after a restart, its link's own commands, and its removal. They need `sh`, `tar`, `sed` and a
// SHA-256 tool there (`sha256sum`, or a Mac's `shasum`), nothing else.
// Platform: each handles a Mac and Linux in the one script (the tools' names, `launchd` beside cron).
// The computer's folder is always `~/.kuvo-code` there: its keeper names no other.

import { appDataProbe } from "../install/app-data";
import { quote } from "../../shared/text";
import { LAUNCH_AGENT, REBOOT_TAG } from "../marks";

const K = 'K="$HOME/.kuvo-code"';
/** A dollar sign, where a script's own `${…}` stands in a template here. */
const D = "$";

/** A file's SHA-256, by whichever tool is there. */
const SUM = `sum_of() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }`;

/** The engine to run the link's script with: this version's, or while it isn't there, the newest there is. */
const engineOf = (version: string): string =>
  `E="$K/engine/${version}/opencode"; [ -x "$E" ] || E=$(for d in "$K"/engine/*/; do [ -x "\${d}opencode" ] && printf '%s\\n' "\${d}opencode"; done | tail -n 1)`;

/** `kuvo-link.js <arguments>` there, as its keeper runs it. */
const link = (args: string): string =>
  `(cd "$K" && KUVO_HOME="$K" BUN_BE_BUN=1 BUN_RUNTIME_TRANSPILER_CACHE_PATH=0 "$E" --config=/dev/null --no-env-file "$K/bin/kuvo-link.js" ${args})`;

// What's on the computer already (PROTOCOL.md, "Your setup", Already in a setup?), for
// `setup/judge.ts`: each box there by its card, an older one by a few fields of its link.json, never
// its keys; and each of Kuvo Code's app folders on a Mac. Only `sed`.
const FOUND = String.raw`for b in "$HOME/.kuvo-code" "$HOME/Library/Application Support"/run.kuvo.code*/box; do
  [ -f "$b/run/link.json" ] || [ -f "$b/run/setup-card.json" ] || [ -f "$b/run/serve.json" ] || [ -f "$b/run/keeper.json" ] || continue
  if [ "$b" = "$HOME/.kuvo-code" ]; then { [ -f "$b/bin/kuvo-start" ] && [ ! -f "$b/run/off" ]; } || continue; fi
  if [ "$b" = "$HOME/.kuvo-code" ]; then echo "box machine ${D}{b#"$HOME"/}"; else echo "box app ${D}{b#"$HOME"/}"; fi
  [ -f "$b/run/setup-card.json" ] && printf 'card %s\n' "$(sed -n 1p "$b/run/setup-card.json")"
  [ -f "$b/run/link.json" ] && sed -n -e 's/^  "mac": "\([A-Za-z0-9_-]*\)",\{0,1\}$/link mac \1/p' \
    -e '/^  "macs": \[$/,/^  \]/s/^    "\([A-Za-z0-9_-]*\)",\{0,1\}$/link macs \1/p' \
    -e '/^  "macSeen": {$/,/^  }/s/^    "\([A-Za-z0-9_-]*\)": \([0-9][0-9.e+]*\),\{0,1\}$/link seen \1 \2/p' \
    -e '/^  "macNames": {$/,/^  }/s/^    "\([A-Za-z0-9_-]*\)": \(".*"\),\{0,1\}$/link name \1 \2/p' \
    -e 's/^  "phones": \[\],\{0,1\}$/link phones none/p' -e 's/^  "phones": \[$/link phones some/p' "$b/run/link.json"
done
for a in "$HOME/Library/Application Support"/run.kuvo.code*; do [ -d "$a" ] && echo "app ${D}{a#"$HOME"/}"; done`;

/** The probe: the computer's kind and what the engine needs of it, what Kuvo Code it has (its
 * version, whether it's updating itself, whether `version`'s engine is there and runs), its id
 * where it has one (`machine-id`, made as it's started: the same computer under another address
 * is told by it), and what's found there. Changes nothing. Lines: `uname -sm`'s; `glibc`/`musl`, or a Mac's
 * `tools`/`no-tools` and `macos <version>`; `kuvo version <v>`, `kuvo updating`, `kuvo engine`,
 * `kuvo id <32 hex>`; then the found lines. */
export const probe = (version: string): string => String.raw`${K}
uname -sm
if [ "$(uname -s)" = Darwin ]; then
  if xcode-select -p >/dev/null 2>&1 && /usr/bin/git --version >/dev/null 2>&1; then echo tools; else echo no-tools; fi
  echo "macos $(sw_vers -productVersion 2>/dev/null)"
elif ldd --version 2>&1 | grep -qi musl; then echo musl; else echo glibc; fi
echo "kuvo version $(cat "$K/bin/box-version" 2>/dev/null | tr -cd '0-9a-z.-')"
[ -d "$K/run/.updating" ] && echo "kuvo updating"
[ -x "$K/engine/${version}/opencode" ] && "$K/engine/${version}/opencode" --version >/dev/null 2>&1 && echo "kuvo engine"
[ -s "$K/machine-id" ] && echo "kuvo id $(cat "$K/machine-id")"
${FOUND}
A="$HOME/Library/Application Support/run.kuvo.code"
${appDataProbe()}
# A current Mac keeps its old folder for going back, just as the installer permits.
if [ "$(cat "$K/box/kind" 2>/dev/null)" != mac ]; then
  old_app_verdict
fi
true`;

/** Whether the computer has `files` already, each as it is here (`same`), or not (`differs`). */
export const filesCheck = (files: readonly { path: string; sum: string }[]): string => `${K}
${SUM}
while read -r sum name; do
  [ -n "$name" ] || continue
  [ -f "$K/$name" ] && [ "$(sum_of "$K/$name")" = "$sum" ] || { echo differs; exit 0; }
done <<'KUVO_SUMS'
${files.map(file => `${file.sum} ${file.path}`).join("\n")}
KUVO_SUMS
echo same`;

/** Puts the release's files in place, from the tar on its standard input: unpacked beside them,
 * each checked against its sum here (one that didn't arrive whole puts nothing in place), then
 * each put in its place by name, in the order given, with its mode. */
export const filesInstall = (files: readonly { path: string; sum: string; mode: number }[]): string => `set -e
${K}
mkdir -p "$K" && chmod 700 "$K"
${SUM}
mkdir -p "$K/tmp"
S="$K/tmp/.kuvo-files.$$"
rm -rf "$S"; mkdir "$S"
trap 'rm -rf "$S"' EXIT
tar xf - -C "$S"
while read -r sum mode name; do
  [ -n "$name" ] || continue
  [ -f "$S/$name" ] && [ "$(sum_of "$S/$name")" = "$sum" ] || { echo "Kuvo Code's files didn't arrive whole." >&2; exit 9; }
done <<'KUVO_SUMS'
${files.map(file => `${file.sum} ${file.mode.toString(8)} ${file.path}`).join("\n")}
KUVO_SUMS
while read -r sum mode name; do
  [ -n "$name" ] || continue
  d="$K/$name"; mkdir -p "\${d%/*}"
  chmod "$mode" "$S/$name" && mv -f "$S/$name" "$K/$name"
done <<'KUVO_SUMS'
${files.map(file => `${file.sum} ${file.mode.toString(8)} ${file.path}`).join("\n")}
KUVO_SUMS
echo placed`;

/** Puts the engine in place, from its standard input (the binary, or its package: `packed`):
 * kept beside it first, checked against `sum`, tried (`--version`), then moved into place. Other
 * versions there stay: one may be at work. */
export const engineInstall = (version: string, sum: string, packed: boolean): string => `set -e
${K}
${SUM}
mkdir -p "$K/engine"
S="$K/engine/.staging.$$"
rm -rf "$S"; mkdir "$S"
trap 'rm -rf "$S"' EXIT
cat >"$S/sent"
[ "$(sum_of "$S/sent")" = "${sum}" ] || { echo "The engine didn't arrive whole." >&2; exit 9; }
${packed ? `tar xzf "$S/sent" -C "$S" package/bin/opencode && mv "$S/package/bin/opencode" "$S/opencode"` : `mv "$S/sent" "$S/opencode"`}
chmod 755 "$S/opencode"
"$S/opencode" --version >/dev/null 2>&1 || { echo "The engine doesn't run on this computer." >&2; exit 8; }
mkdir -p "$K/engine/${version}"
mv -f "$S/opencode" "$K/engine/${version}/opencode"
echo placed`;

const REBOOT_LINE = `@reboot sh "$HOME/.kuvo-code/bin/kuvo-start" >/dev/null 2>&1 ${REBOOT_TAG}`;

const LAUNCH_AGENT_PLIST = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
<key>Label</key><string>${LAUNCH_AGENT}</string>
<key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string><string>exec sh "$HOME/.kuvo-code/bin/kuvo-start"</string></array>
<key>RunAtLoad</key><true/>
<key>AbandonProcessGroup</key><true/>
</dict>
</plist>`;

// What starts the box after a restart, added once: its line in your crontab (one that can't be read
// is left as it is, never written over; `crontab` given fifteen seconds, should a Mac ask something
// on a screen nobody's at), and on a Mac its launch agent, which launchd reads at the next login.
const START_AT_BOOT = String.raw`if command -v crontab >/dev/null 2>&1; then
  cron_tab() { if command -v perl >/dev/null 2>&1; then perl -e 'alarm 15; exec @ARGV' crontab "$@"; else crontab "$@"; fi; }
  cron_now=$(cron_tab -l 2>"$R/.crontab.err"); cron_read=$?
  if [ $cron_read -eq 0 ] || grep -qi 'no crontab' "$R/.crontab.err" 2>/dev/null; then
    case "$cron_now" in
      *"${REBOOT_TAG}"*) ;;
      *) { [ -z "$cron_now" ] || printf '%s\n' "$cron_now"; printf '%s\n' ${quote(REBOOT_LINE)}; } | cron_tab - >/dev/null 2>&1 ;;
    esac
  fi
  rm -f "$R/.crontab.err"
fi
if [ "$(uname -s)" = Darwin ] && [ ! -f "$HOME/Library/LaunchAgents/${LAUNCH_AGENT}.plist" ]; then
  mkdir -p "$HOME/Library/LaunchAgents" && printf '%s\n' ${quote(LAUNCH_AGENT_PLIST)} >"$HOME/Library/LaunchAgents/${LAUNCH_AGENT}.plist"
fi`;

/** Starts the box there, to keep itself from then on: its settings made its own (on a computer
 * with none, from what's on its standard input, `{settings, keys}`, which never shows anywhere
 * else: `seeded` is said then; one with its own keeps them), how its keeper starts its engine noted once (the session's PATH, shell
 * and language, which cron doesn't give it, and the name you know it by), its start after a
 * restart, and then its own keeper's start, which gives its engine a port and passwords of its own.
 * All in one script, so a removal is never between its parts. Its id (`id <32 hex>`, made here
 * once) then `started`, or exit 3 with its
 * engine's last words; 6 when its settings couldn't be made its own; 7 when it was removed
 * meanwhile. `notes`: who started it, as its keeper notes each start (`{…}`, JSON on one line). */
export const start = (version: string, name: string, notes: string): string => `${K}; R="$K/run"
mkdir -p "$R" && chmod 700 "$R"
${engineOf(version)}
[ -x "$E" ] || { echo "It has no engine yet." >&2; exit 3; }
adopted=$( ${link("box adopt")} 2>"$R/.adopt.err") || { tail -n 2 "$R/.adopt.err" >&2; rm -f "$R/.adopt.err"; exit 6; }
rm -f "$R/.adopt.err"
# (A space after "$(": with a subshell right behind it, sh on Linux reads "$((" as arithmetic.)
case "$adopted" in *'"seeded":true'*) echo seeded ;; esac
unset adopted
if [ ! -f "$R/launch.env" ]; then
  (umask 077; { env | grep -E '^(PATH|SHELL|LANG|LC_[A-Z_]+)='; printf '%s\\n' ${quote(`KUVO_MACHINE_NAME=${name}`)} 'OPENCODE_CLIENT=desktop'; } | grep -v '^$' >"$R/launch.env.new" && mv -f "$R/launch.env.new" "$R/launch.env")
fi
[ -s "$R/launch.notes" ] || (umask 077; printf '%s' ${quote(notes)} >"$R/launch.notes")
${START_AT_BOOT}
F="$K/machine-id"
if [ ! -s "$F" ]; then
  T="$F.$$"
  (umask 077; od -An -N16 -tx1 /dev/urandom | tr -d ' \n' >"$T") || exit 1
  ln "$T" "$F" 2>/dev/null || [ -s "$F" ] || mv -f "$T" "$F"
  rm -f "$T"
fi
rm -f "$R/off"
if sh "$K/bin/kuvo-start" start </dev/null >/dev/null 2>&1; then
  [ -f "$R/off" ] && exit 7
  echo "id $(cat "$F")"
  echo started
else
  [ -f "$R/off" ] && exit 7
  tail -n 5 "$R/engine.log" >&2 2>/dev/null
  exit 3
fi`;

/** `kuvo-link.js setup` there, its input (JSON) on its standard input: its answer is the last line. */
export const join = (version: string): string => `${K}
${engineOf(version)}
${link("setup")}`;

/** The accounts a computer starts with, on its standard input (`{credentials}`), taken by its own
 * box (`kuvo-link.js box seed`): its answer is the last line, which says how many, never what. */
export const seed = (version: string): string => `${K}
${engineOf(version)}
${link("box seed")}`;

/** One request to the box there, answered once (`kuvo-link.js box route`, the request on its
 * standard input): its answer is the last line, `{status, body}`. */
export const route = (version: string): string => `${K}
${engineOf(version)}
${link("box route")}`;

/** Takes Kuvo Code off the computer: it lets this computer (`key`) go; and unless another Mac
 * still uses it (`only: false` leaves it running for them), its own `uninstall` (nothing of it
 * runs or starts again: its launch agent and crontab line too), then the sign-ins and keys it
 * keeps, and the folder of Kuvo Code's app there (`desktop`, the key it's paired by as one of
 * your devices). The rest of its folder stays, with its sessions. Prints `kept` (another Mac uses it), or what its
 * uninstall said (JSON); exit 1 when that couldn't stop everything. */
export const uninstall = (version: string, key: string, only: boolean): string => String.raw`${K}
${engineOf(version)}
# (The key Kuvo Code's app there is known by among your devices, which the box that removes this
# computer takes off them too: with or without a Kuvo Code that runs. On a Mac the app keeps it in
# its Keychain, which isn't reached from here: there, being taken off your devices keeps it out.)
${only ? 'rm -rf "$K/desktop"' : ""}
if [ ! -x "$E" ] || [ ! -f "$K/bin/kuvo-link.js" ]; then echo '{"result":"none"}'; exit 0; fi
${link(`leave ${quote(key)}`)} >/dev/null 2>&1 || true
${only ? "" : "echo kept; exit 0"}
${link("uninstall")} 2>/dev/null; did=$?
# (What it keeps of sign-ins and keys goes whether or not everything stopped just now.)
if [ -f "$K/data/opencode/opencode.db" ]; then
  KUVO_DB="$K/data/opencode/opencode.db" BUN_BE_BUN=1 "$E" -e 'const { Database } = require("bun:sqlite"); const db = new Database(process.env.KUVO_DB); try { db.run("DELETE FROM credential"); } catch {} db.close()' >/dev/null 2>&1 || true
fi
rm -rf "$K/integrations" "$K/keys" "$K/box/keys" "$K/box/bot-keys"
rm -f "$K/run/launch.env" "$K/run/launch.notes"
if [ -d "$K/agents" ]; then
  find "$K/agents" -path "$K/agents/runtime" -prune -o -type f \( -name .credentials.json -o -name 'auth.json*' -o -name 'acp_token.json*' -o -name 'oauth-token.json*' -o -path '*/claude/tokens/*' \) -exec rm -f {} + 2>/dev/null
fi
[ $did -eq 0 ] || exit 1
true`;
