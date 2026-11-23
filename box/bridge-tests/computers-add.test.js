// Adding a computer over SSH from a box (PROTOCOL.md, "Computers", Adding one; #636): a client
// only gives the address and answers what the box asks; the box checks the host's key, puts its
// own files and engine there, starts the computer's own keeper, and signs it into your setup. The
// real box code as a Mac's box, the real keeper, launcher and link on the computer added, in a home
// of the test's own. Stand-ins only for what's outside: `ssh` (a script that runs what it's given
// in that home, with a host key and refusals of its own), the engine's binary, the agents' bridge,
// and `crontab`/`launchctl`.
//
// Run with `node --test box/bridge-tests/computers-add.test.js`.
import { removeTestHomeAtExit } from "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd } from "./at-end.js";
import { relayStandIn } from "./relay-stand-in.js";

// (Short: a shared connection's socket goes under the box's folder on Linux.)
const base = fs.realpathSync(fs.mkdtempSync("/tmp/k"));
removeTestHomeAtExit(base);
const home = path.join(base, "here", ".kuvo-code");
const there = path.join(base, "there");
const remote = path.join(there, ".kuvo-code");
const primaryHome = path.join(base, "primary");
const fake = path.join(base, "fake");
const tools = path.join(base, "tools");
for (const folder of [home, there, fake, tools, path.join(base, "here", ".ssh")]) fs.mkdirSync(folder, { recursive: true });
process.env.HOME = path.join(base, "here");
process.env.KUVO_HOME = home;
// (A Mac's box in a home of its own, as the desktop app's Mac has it: `box/kind` says so.)
fs.mkdirSync(path.join(home, "box"), { recursive: true });
fs.writeFileSync(path.join(home, "box", "kind"), "mac\n");
process.env.KUVO_TEST_UNINSTALL_TOOLS = tools;
process.env.KUVO_TEST_SSH = path.join(fake, "ssh");
process.env.FAKE_SSH = fake;
process.env.FAKE_SSH_HOME = there;
process.env.PATH = `${tools}${path.delimiter}${process.env.PATH}`;

const checkout = fileURLToPath(new URL("../..", import.meta.url));
const script = fileURLToPath(LINK_BUNDLE);
const write = (file, text, mode = 0o644) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
};
const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const sha256 = data => crypto.createHash("sha256").update(data).digest("hex");
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const command = pid => spawnSync("ps", ["-ww", "-o", "command=", "-p", String(pid)], { encoding: "utf8" }).stdout.trim();

// `crontab` keeps what it's given, as the real one does; the rest only note they were asked.
const crontabFile = path.join(fake, "crontab");
write(path.join(tools, "crontab"), `#!/bin/sh\ncase "$1" in -l) [ -f '${crontabFile}' ] && cat '${crontabFile}' || { echo "no crontab for you" >&2; exit 1; } ;; -) cat >'${crontabFile}' ;; esac\n`, 0o755);
for (const tool of ["pkill", "caffeinate", "codesign", "open"]) write(path.join(tools, tool), `#!/bin/sh\necho "${tool} $*" >>'${path.join(fake, "reached")}'\n`, 0o755);

// IDs belong to simulated hardware, outside the simulated OS account's Kuvo data.
write(path.join(tools, "ioreg"), `#!/bin/sh
[ -f "$HOME/hardware-id" ] && printf '"IOPlatformUUID" = "%s"\\n' "$(/bin/cat "$HOME/hardware-id")"
`, 0o755);
write(path.join(tools, "cat"), `#!/bin/sh
case "$1" in
  /sys/class/dmi/id/product_uuid) exec /bin/cat "$HOME/hardware-id" ;;
  /etc/machine-id) exec /bin/cat "$HOME/os-image-id" ;;
esac
exec /bin/cat "$@"
`, 0o755);

// The signed bundle and launchctl are stand-ins. No program in this fixture reaches TCC.
const helperBundle = "bin/Kuvo Helper.app";
write(path.join(tools, "launchctl"), `#!/bin/sh
HOME=$(cd -P -- "$HOME" && pwd -P) || exit 1
case "$HOME" in '${base}'/*) ;; *) exit 1 ;; esac
[ -d '${base}' ] && [ ! -e '${base}/.closing' ] || exit 1
J="$HOME/.kuvo-code/run/fake-launchd"
P="$HOME/Library/LaunchAgents/run.kuvo.code.machine.plist"
H="$HOME/.kuvo-code/${helperBundle}/Contents/MacOS/Kuvo Helper"
printf '%s\n' "$*" >>"$HOME/launchctl.log"
mode=keep-machine
case "$2 $3" in
  *.permissions*) J="$J.permissions"; P="$HOME/Library/LaunchAgents/run.kuvo.code.machine.permissions.plist"; mode=watch-machine ;;
esac
case "$1" in
  print)
    [ -f "$J" ] || exit 1
    if grep -qx legacy "$J"; then echo "program = /bin/sh"; else echo "program = $H"; echo "pid = $(cat "$J")"; fi
    ;;
  bootout)
    pid=$(cat "$J" 2>/dev/null)
    if [ -n "$pid" ] && ps -ww -o command= -p "$pid" | grep -Fq "$H $mode"; then kill "$pid"; fi
    rm -f "$J"; : >"\${J}.booted-out"
    ;;
  bootstrap|kickstart)
    [ -f "$P" ] || exit 1
    if [ "$mode" = keep-machine ] && ! grep -q keep-machine "$P"; then
      if [ -f "$HOME/refuse-legacy" ]; then echo "legacy job refused" >&2; exit 1; fi
      echo legacy >"$J"
      keeper=$(cat "$HOME/.kuvo-code/run/box.pid" 2>/dev/null)
      if [ -z "$keeper" ] || ! ps -ww -o command= -p "$keeper" | grep -q 'kuvo-start.*keep'; then
        KUVO_MACHINE_HELPER_PID= KUVO_TEST_KEEP_ROUND=1 /bin/sh "$HOME/.kuvo-code/bin/kuvo-start" keep </dev/null >/dev/null 2>&1 &
      fi
      exit 0
    fi
    if [ "$mode" = keep-machine ] && [ -f "$HOME/refuse-helper" ]; then
      if [ -f "$HOME/fail-bridge-on-bootstrap" ]; then : >"$HOME/fail-legacy-bridge"; fi
      exit 1
    fi
    if [ "$mode" = keep-machine ] && [ -f "$HOME/pause-helper-bootstrap" ]; then
      : >"$HOME/entered-helper-bootstrap"
      while [ -f "$HOME/pause-helper-bootstrap" ]; do sleep 0.1; done
      exit 1
    fi
    pid=$(cat "$J" 2>/dev/null)
    if [ -n "$pid" ] && ps -ww -o command= -p "$pid" | grep -Fq "$H $mode"; then exit 0; fi
    "$H" "$mode" --home "$HOME/.kuvo-code" </dev/null >/dev/null 2>&1 &
    echo "$!" >"$J"
    ;;
  *) exit 1 ;;
esac
`, 0o755);
write(crontabFile, "");

write(path.join(tools, "ln"), `#!/bin/sh
if [ "$3" = "$HOME/.kuvo-code-adding" ] && [ -f "$HOME/refuse-reservation" ]; then
  echo "ln: test account can't create the reservation" >&2
  exit 1
fi
exec /bin/ln "$@"
`, 0o755);

// The engine: it says it's listening, as the launcher waits to read, and goes when its home does.
const fakeEngine = path.join(fake, "engine.mjs");
fs.writeFileSync(fakeEngine, `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
fs.writeFileSync(process.env.HOME + "/engine-helper", process.env.KUVO_MACHINE_HELPER_PID || "");
// (Its sign-ins, kept as an engine keeps them: listed, and stored as they're given.)
const kept = process.env.HOME + "/engine-credentials.json";
const inboxFile = process.env.HOME + "/engine-inbox.json";
const receiptFile = process.env.HOME + "/engine-prompt-ids";
const promptIDs = () => fs.existsSync(receiptFile) ? fs.readFileSync(receiptFile, "utf8").trim().split("\\n") : [];
const credentials = () => { try { return JSON.parse(fs.readFileSync(kept, "utf8")); } catch { return []; } };
const server = http.createServer((request, response) => {
  const chunks = [];
  request.on("data", chunk => chunks.push(chunk));
  request.on("end", () => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (fs.existsSync(inboxFile)) {
      if (url.pathname === "/api/session" && !url.searchParams.has("archived")) return response.end(JSON.stringify({ data: [{ id: "ses_retry" }] }));
      if (url.pathname === "/api/session/ses_retry/inbox") return response.end(JSON.stringify({ data: JSON.parse(fs.readFileSync(inboxFile, "utf8")) }));
      if (request.method === "DELETE" && url.pathname.startsWith("/api/session/ses_retry/inbox/")) {
        const id = url.pathname.split("/").at(-1);
        const failed = process.env.HOME + "/inbox-delete-refused";
        if (id === "msg_B" && !fs.existsSync(failed)) {
          fs.writeFileSync(failed, "");
          response.statusCode = 503;
          return response.end("{}");
        }
        const inbox = JSON.parse(fs.readFileSync(inboxFile, "utf8"));
        fs.writeFileSync(inboxFile, JSON.stringify(inbox.filter(item => item.id !== id)));
        return response.end("{}");
      }
    }
    const receipt = url.pathname.match(/^\\/api\\/session\\/[^/]+\\/message\\/([^/]+)$/);
    if (request.method === "GET" && receipt) {
      const id = receipt[1];
      if (!promptIDs().includes(id)) {
        response.statusCode = 404;
        return response.end("{}");
      }
      return response.end(JSON.stringify({ data: { info: { id } } }));
    }
    response.writeHead(200, { "content-type": "application/json" });
    if (request.url.endsWith("/prompt") && request.method === "POST") {
      const given = JSON.parse(Buffer.concat(chunks).toString());
      fs.appendFileSync(receiptFile, given.id + "\\n");
      fs.appendFileSync(process.env.HOME + "/delivered-prompts", given.text + "\\n");
    }
    if (request.url === "/api/credential" && request.method === "POST") {
      const given = JSON.parse(Buffer.concat(chunks).toString());
      fs.writeFileSync(kept, JSON.stringify([...credentials(), { id: "cred_" + credentials().length, integrationID: given.integrationID, label: given.label, value: given.value, active: given.activate }]), { mode: 0o600 });
      return response.end("{}");
    }
    response.end(JSON.stringify({ data: request.url === "/api/credential" ? credentials() : request.url.startsWith("/api/session/active") ? (fs.existsSync(process.env.HOME + "/busy") ? { ses_a: { type: "busy" }, ses_b: { type: "busy" } } : {}) : [] }));
  });
});
server.listen(port, "127.0.0.1", () => console.log("server listening on http://127.0.0.1:" + port));
setInterval(() => { if (!fs.existsSync(process.env.HOME)) process.exit(0); }, 300);
`);
// Its binary, the same on both computers (they're the same kind: this box gives its own): its
// version, the link's script run as the engine's Bun runs it (here this Node), and the engine.
const engineBinary = `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo 9.9.9; exit 0 ;;
-e) BUN_BE_BUN=1 exec true ;;
*.js)
  case "$2:$3" in
    browser:*) exit 0 ;;
    box:seed)
      if [ -f "$HOME/hold-seed" ]; then
        : >"$HOME/writing"
        while [ -f "$HOME/hold-seed" ]; do sleep 0.05; done
      fi ;;
    uninstall:*)
      if [ -f "$HOME/hold-uninstall" ]; then
        : >"$HOME/deleting"
        while [ -f "$HOME/hold-uninstall" ]; do sleep 0.05; done
      fi ;;
  esac
  BUN_BE_BUN=1 exec '${process.execPath}' "$@" ;;
serve) trap 'kill "$c" 2>/dev/null' TERM INT HUP; BUN_BE_BUN=1 '${process.execPath}' '${fakeEngine}' "$@" & c=$!; wait "$c" ;;
esac
`;

// This box's own release files, in its folder, as an installed box has them: this checkout's own
// keeper, launcher, tools and link as built, with a bridge that only says where it answers.
const root = path.join(base, "sources");
for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/src/shared/engine-pin.json", "box/dist/kuvo-browser.js"]) {
  let source = fs.readFileSync(path.join(checkout, file));
  if (file === "box/src/keeper/kuvo-start") {
    // A legacy keeper starts with an empty environment. Bind this copy to the fake service
    // even there, and pass the same stub to the workers the keeper starts later.
    const keeper = source.toString();
    assert.match(keeper, /^  ctl=\/bin\/launchctl$/m);
    source = keeper.replace(/^  ctl=\/bin\/launchctl$/m, '  ctl="$KUVO_TEST_UNINSTALL_TOOLS/launchctl"')
      .replace(/^#![^\n]+\n/, `$&export KUVO_TEST_UNINSTALL_TOOLS='${tools}'\n`);
  }
  write(path.join(root, file), source);
}
const builtLink = fs.readFileSync(script, "utf8");
// These boxes test SSH updates. A real release check must not take their update lock.
const offlineLink = builtLink.replace(/^(\s*UPDATE_KEYS = )\[[^\n]*\];$/m, "$1[];");
assert.notEqual(offlineLink, builtLink);
write(path.join(root, "box/dist/kuvo-link.js"), offlineLink);
write(path.join(root, "box/dist/kuvo-agents.js"), `
import fs from "node:fs";
import http from "node:http";
const run = process.env.KUVO_HOME + "/run";
const port = Number(process.env.KUVO_AGENTS_PORT);
const has = name => fs.existsSync(process.env.HOME + "/" + name);
const server = http.createServer((_, res) => {
  if (has(process.env.KUVO_MACHINE_HELPER_PID ? "fail-helper-bridge" : "fail-legacy-bridge")) res.statusCode = 503;
  res.end(JSON.stringify({ data: [] }));
});
server.listen(port, "127.0.0.1", () => {
  fs.writeFileSync(run + "/bridge.json", JSON.stringify({ pid: process.pid, port, password: "test", machineHandover: true, helper: Number(process.env.KUVO_MACHINE_HELPER_PID || 0) }));
  console.log(JSON.stringify({url: "http://127.0.0.1:" + port}));
});
setInterval(() => {
  if (has("crash-bridge")) {
    fs.unlinkSync(process.env.HOME + "/crash-bridge");
    process.exit(0);
  }
  if (!fs.existsSync(run) || fs.existsSync(run + "/.machine-handover.pid")) process.exit(0);
}, 100);
`);
// Keep the packed release apart from the SSH target named "release", which runs live workers.
const release = path.join(base, "box-release-fixture");
const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), "--files", "1.0.0", release], {
  env: { ...process.env, BUN: process.env.BUN || process.env.KUVO_TEST_BUN || (process.versions.bun ? process.execPath : "bun"), BOX_SOURCES: root }, encoding: "utf8",
});
assert.equal(packed.status, 0, packed.stderr);
fs.cpSync(release, home, { recursive: true });
fs.rmSync(path.join(home, "box.json"));
write(path.join(home, helperBundle, "Contents/Info.plist"), "<key>KuvoHelperKeepsMachine</key><true/>");
write(path.join(home, helperBundle, "Contents/MacOS/Kuvo Helper"), `#!/bin/sh
export KUVO_MACHINE_HELPER_PID=$$ KUVO_TEST_KEEP_ROUND=1
keeper=
if [ "$1" != watch-machine ]; then sh "$HOME/.kuvo-code/bin/kuvo-start" keep & keeper=$!; fi
trap 'kill "$keeper" 2>/dev/null; exit 0' TERM
R="$HOME/.kuvo-code/run"
started=$(date +%s)
while [ -d "$R" ] && { [ -z "$keeper" ] || kill -0 "$keeper" 2>/dev/null; }; do
  grant=$(cat "$HOME/files-access" 2>/dev/null || printf null)
  printf '{"v":1,"pid":%s,"started":%s,"observedAt":%s,"fullDiskAccess":%s}\n' "$$" "$started" "$(($(date +%s) * 1000))" "$grant" >"$R/permissions.new"
  mv "$R/permissions.new" "$R/permissions.json"
  sleep 0.2
done

`, 0o755);
const engineVersion = read(path.join(checkout, "box/src/shared/engine-pin.json")).version;
write(path.join(home, "engine", engineVersion, "opencode"), engineBinary, 0o755);

// `ssh`: its options read as ssh reads them, a host key of its own checked against the known
// hosts files it's given, and what it's told to run, run in the other home.
const hostKey = blob => `ssh-ed25519 ${blob}`;
const keyOne = Buffer.concat([Buffer.from("0000000b7373682d6564323535313900000020", "hex"), crypto.randomBytes(32)]).toString("base64");
const keyTwo = Buffer.concat([Buffer.from("0000000b7373682d6564323535313900000020", "hex"), crypto.randomBytes(32)]).toString("base64");
fs.writeFileSync(path.join(fake, "hostkey"), hostKey(keyOne));
write(path.join(fake, "ssh"), `#!/bin/sh
F="$FAKE_SSH"
strict=; known=; pubkey=yes; control=; alias=; config=
while [ $# -gt 0 ]; do
  case "$1" in
    -G) config=1; shift ;;
    -o) echo "$2" >>"$F/options"; case "$2" in
          HostKeyAlias=*) alias=\${2#*=} ;;
          StrictHostKeyChecking=*) strict=\${2#*=} ;;
          UserKnownHostsFile=*) known=\${2#*=} ;;
          PubkeyAuthentication=*) pubkey=\${2#*=} ;;
        esac; shift 2 ;;
    -O) control=$2; shift 2 ;;
    --) shift; break ;;
    *) shift ;;
  esac
done
dest=$1; cmd=$2
[ -n "$config" ] && { printf 'hostname %s\\nport 22\\n' "\${dest#*@}"; exit 0; }
echo "$dest $strict \${control:-run}" >>"$F/log"
[ -n "$control" ] && exit 0
case "$dest" in nowhere) echo "ssh: Could not resolve hostname nowhere: Name or service not known" >&2; exit 255 ;; esac
[ -f "$F/away-$dest" ] && { echo "ssh: connect to host $dest port 22: No route to host" >&2; exit 255; }
# (Reached at another of its addresses: its key is checked as the one known for the first's.)
[ -n "$alias" ] && dest=$alias
key=$(cat "$F/hostkey")
eval "set -- $known"
if [ "$strict" = accept-new ]; then
  echo "$dest $key" >>"$1"
  echo "Warning: Permanently added '$dest' (ED25519) to the list of known hosts." >&2
else
  found=; other=
  for file in "$@"; do
    [ -f "$file" ] || continue
    if grep -qxF "$dest $key" "$file"; then found=1; elif grep -q "^$dest " "$file"; then other=$file; fi
  done
  if [ -z "$found" ] && [ -n "$other" ]; then
    printf '%s\\n' "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@" "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @" "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@" "Offending ED25519 key in $other:1" "Host key for $dest has changed and you have requested strict checking." "Host key verification failed." >&2
    exit 255
  fi
  if [ -z "$found" ]; then
    printf '%s\\n' "No ED25519 host key is known for $dest and you have requested strict checking." "Host key verification failed." >&2
    exit 255
  fi
fi
if [ "$pubkey" = no ] || [ -f "$F/refuse" ]; then echo "you@$dest: Permission denied (publickey)." >&2; exit 255; fi
if [ -f "$F/windows-$dest" ]; then
  case "$cmd" in
    cmd.exe*) printf 'KUVO-WINDOWS-DESKTOP-TEST\\r\\n'; exit 0 ;;
    'wsl.exe --list --verbose') printf '  NAME  STATE  VERSION\\r\\n* Ubuntu  Stopped  1\\r\\n' | iconv -f UTF-8 -t UTF-16LE; exit 0 ;;
    *) echo "'exec' is not recognized as an internal or external command." >&2; exit 255 ;;
  esac
fi
[ -f "$F/home-$dest" ] && FAKE_SSH_HOME=$(cat "$F/home-$dest")
# A detached simulated SSH command stays identifiable by its script even when macOS
# hides the environment of /bin/sh and its SSH parent has gone away.
remote_script=$(mktemp "$F/ssh-command.XXXXXXXX") || exit 1
(umask 077; printf '%s\\n' "$cmd" >"$remote_script") || exit 1
if [ -f "$F/trace" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in
    *'uname -sm'*) action=probe ;;
    *'kuvo-link.js" box seed'*) action=seed ;;
    *'kuvo-link.js" setup'*) action=join ;;
    *'kuvo-link.js" uninstall'*) action=uninstall ;;
    *'then rm -f "$R"; fi'*|*'if [ "$prior" = '*'rm -f "$A"'*) action=release ;;
    *) action=other ;;
  esac
  echo "$dest $action" >>"$F/trace"
  if [ "$action" = release ] && [ -f "$F/fail-release-$dest" ]; then
    echo "rm: test reservation could not be cleared" >&2
    exit 1
  fi
  if [ "$action" = seed ] && [ -f "$F/drop-seed-$dest" ]; then
    cat >"$F/input-$dest"
    (
      HOME="$FAKE_SSH_HOME" sh "$remote_script" <"$F/input-$dest"
      : >"$FAKE_SSH_HOME/writer-ended"
    ) >/dev/null 2>&1 &
    while [ ! -f "$FAKE_SSH_HOME/writing" ] && [ ! -f "$FAKE_SSH_HOME/writer-ended" ]; do sleep 0.05; done
    if [ "$(cat "$F/drop-seed-$dest")" = 255 ]; then
      echo "Connection to $dest closed by remote host." >&2
      exit 255
    fi
    kill -TERM "$$"
  fi
fi
# Exercise a remote bridge that still returns the revoked credentials on SSH Add.
if [ -f "$F/stale-join-$dest" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in *'kuvo-link.js" setup'*) cat >/dev/null; cat "$F/stale-join-$dest"; exit 0 ;; esac
fi
if [ -f "$F/track" ] || [ -f "$F/lose-uninstall-reply" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  if [ -f "$F/track" ]; then
    case "$text" in *'kuvo kind'*) echo probe >>"$F/track" ;; *'bin/kuvo-start" start'*) echo start >>"$F/track" ;; *'kuvo-link.js" box seed'*) echo accounts >>"$F/track" ;; esac
  fi
  if [ -f "$F/lose-uninstall-reply" ]; then
    case "$text" in *'kuvo-link.js" uninstall'*)
      if [ "$(cat "$F/lose-uninstall-reply")" = before ]; then
        : >"$F/uninstall-entered"
        while [ ! -f "$F/uninstall-continue" ]; do sleep 0.05; done
        echo "the connection closed before the command ran" >&2
        exit 255
      fi
      HOME="$FAKE_SSH_HOME" sh "$remote_script" >/dev/null 2>&1
      echo "lost the reply after cleaning $FAKE_SSH_HOME/.kuvo-code" >&2
      exit 255 ;;
    esac
  fi
fi
if [ -f "$F/fail" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in *"$(cat "$F/fail")"*) cat >/dev/null; echo "it broke" >&2; exit 1 ;; esac
fi
hold="$F/hold-$dest"; [ -f "$hold" ] || hold="$F/hold"
if [ -f "$hold" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in *"$(cat "$hold")"*) while [ -f "$hold" ]; do sleep 0.05; done ;; esac
fi
HOME="$FAKE_SSH_HOME" exec sh "$remote_script"
`, 0o755);

// (Its `~/.ssh/config`: the hosts Add a Machine suggests.)
fs.writeFileSync(path.join(base, "here", ".ssh", "config"), "Host studio\n  HostName 10.0.0.5\nHost *.internal !bastion\n  User me\nHost gpu  # the big one\nHost github.com\n");
// (The keeper's own tools there, where its PATH starts: none of the system's is reached.)
for (const tool of ["pkill", "caffeinate"]) write(path.join(remote, "bin", tool), `#!/bin/sh\nexit 0\n`, 0o755);

// What this box has to give a new computer: its settings, an integration's key (in the Keychain,
// through its helper, a stand-in here), and its engine's accounts, a stand-in too: a provider's API
// key, GitHub Copilot's sign-in (which stays the same), ChatGPT's (which renews itself), an
// integration's own sign-in, and a second key that isn't the one in use.
const mine = {
  keys: { KUVO_KEY_NOTION_TOKEN: "secret-notion-token", KUVO_KEY_NOTION_TOKEN__BOT_ABC: "a-bots-own", KUVO_KEY_APPLE_CALENDAR_KEY: "this-macs-own" },
  integrations: [{ id: "anthropic" }, { id: "github-copilot" }, { id: "openai" }, { id: "notion-mcp", metadata: { source: "mcp" } }],
  credentials: [
    { id: "c1", integrationID: "anthropic", label: "Work", active: true, value: { type: "api", key: "sk-ant-secret-key" } },
    { id: "c2", integrationID: "anthropic", label: "Old", active: false, value: { type: "api", key: "sk-ant-old-key" } },
    { id: "c3", integrationID: "github-copilot", label: "me", active: true, value: { type: "oauth", refresh: "gho_stays_the_same", access: "gho_stays_the_same", expires: 0 } },
    { id: "c4", integrationID: "openai", label: "me@example.com", active: true, value: { type: "oauth", refresh: "rt-renews-itself", access: "at", expires: 1 } },
    { id: "c5", integrationID: "notion-mcp", label: "Notion", active: true, value: { type: "oauth", access: "mcp-sign-in" } },
  ],
};
const here = http.createServer((request, response) => {
  const chunks = [];
  request.on("data", chunk => chunks.push(chunk));
  request.on("end", () => {
    const given = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
    const json = value => response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(value));
    if (request.url === "/hello") return json({ proof: crypto.createHmac("sha256", "host-password").update(`kuvo-host-hello\u0000${given.nonce}\u0000127.0.0.1:${here.address().port}`).digest("hex") });
    if (request.url === "/keys/names") return json({ names: Object.keys(mine.keys) });
    if (request.url === "/keys/get") return given.name in mine.keys ? json({ value: mine.keys[given.name] }) : response.writeHead(404).end();
    if (request.url?.startsWith("/api/session")) return json({ data: [] });
    if (request.url === "/api/integration") return json({ data: mine.integrations });
    if (request.url === "/api/credential") return json({ data: mine.credentials });
    response.writeHead(404).end();
  });
});
await new Promise(resolve => here.listen(0, "127.0.0.1", resolve));
atEnd(() => { here.closeAllConnections(); here.close(); });
write(path.join(home, "run", "host.json"), JSON.stringify({ url: `http://127.0.0.1:${here.address().port}`, password: "host-password" }), 0o600);
write(path.join(home, "run", "engine.json"), JSON.stringify({ pid: process.pid, port: here.address().port, password: "pw", version: engineVersion }), 0o600);
write(path.join(home, "config", "opencode", "opencode.json"), JSON.stringify({
  $schema: "https://opencode.ai/config.json", shell: "/bin/zsh", model: "anthropic/claude-opus-5-5",
  mcp: {
    notion: { type: "local", command: ["notion-mcp"], environment: { NOTION_TOKEN: `{file:${path.join(home, "keys")}/KUVO_KEY_NOTION_TOKEN}` } },
    "chrome-devtools": { type: "local", command: ["chrome-devtools-mcp"] },
  },
}));
const secrets = ["secret-notion-token", "sk-ant-secret-key", "gho_stays_the_same", "rt-renews-itself", "a-bots-own", "this-macs-own", "mcp-sign-in"];

const migrated = spawnSync(process.execPath, [script, "folder-upgrade"], { env: process.env, encoding: "utf8", timeout: 15_000 });
assert.equal(migrated.status, 0, migrated.stderr);
const link = await import(LINK_BUNDLE.href);
const k = key => key.toString("base64url");
const mac = link.newKey();
link.macLink.overlay = { key: k(mac), secret: k(crypto.randomBytes(32)), sign: link.newSigningSeed(), relay: "ws://127.0.0.1:1", name: "Studio" };
link.writeLink({ phones: [] });
link.macLink.config = { phones: true, machines: false, relay: "ws://127.0.0.1:1", name: "Studio", app: "1.0.0" };

const handle = link.engineHandler(() => ({ port: 1, password: "pw" }), { name: "Studio" });
const ask = async (method, route, body, phone) => {
  const pieces = [];
  for await (const piece of handle({ method, path: route, headers: {}, fromApp: !phone, ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) }, phone)) pieces.push(piece);
  return { status: pieces.find(each => each.head).head.status, body: JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString() || "{}") };
};

test("failure bench: addressed SSH cannot start, answer, or cancel an Add in its transient process", async () => {
  const actor = { key: link.publicOf(link.newKey()), kind: "desktop", name: "Paired desktop", pairing: "fixture" };
  for (const [method, route, body] of [
    ["POST", "kuvo/computers/add", { destination: "--invalid" }],
    ["POST", "kuvo/computers/add/add_fixture/answer", { answer: "trust", at: 1 }],
    ["DELETE", "kuvo/computers/add/add_fixture"],
  ]) {
    const parts = [];
    for await (const part of handle({ method, path: route, headers: {}, addressedSsh: true,
      ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) }, actor)) parts.push(part);
    assert.equal(parts[0].head.status, 409);
    const result = JSON.parse(Buffer.concat(parts.filter(part => part.body).map(part => part.body)));
    assert.match(result.message, /a single request can’t stay to do it/);
  }
});
async function checkedCredentials(request = ask) {
  const result = await request("GET", "kuvo/computers/add/credentials");
  assert.equal(result.status, 200, JSON.stringify(result.body));
  return result.body.items.filter(item => item.checked).map(item => item.id);
}
// What a client follows (`kuvo/computers/add/events`), until the try `id` is as `wanted` says: its
// line then. (Gone from the list: `null` is what `wanted` gets.)
const until = async (id, wanted) => {
  const stop = new AbortController();
  let held = "";
  try {
    for await (const piece of handle({ method: "GET", path: "kuvo/computers/add/events", headers: {}, fromApp: true }, undefined, stop.signal)) {
      if (!piece.body) continue;
      held += piece.body.toString();
      const lines = held.split("\n");
      held = lines.pop();
      for (const line of lines) {
        const found = JSON.parse(line).tries.find(each => each.id === id) ?? null;
        if (wanted(found)) return found;
        // (Never waited on for good: one that failed, or asks what the test didn't mean to answer.)
        if (found?.state === "failed" || found?.state === "asking") throw new Error(`The try stands at ${found.state}: ${JSON.stringify(found.failure ?? found.question)}`);
      }
    }
  } finally {
    stop.abort();
  }
  throw new Error("The box stopped telling of its tries.");
};
// (Until it's over; one that asks something instead is handed back as it is, for the test to
// fail on rather than wait for an answer no one gives.)
const over = id => until(id, found => !found || found.state === "done" || found.state === "failed" || found.state === "asking");
const steps = found => found.steps.map(step => `${step.id}:${step.state}`);
const sshLog = () => (fs.existsSync(path.join(fake, "log")) ? fs.readFileSync(path.join(fake, "log"), "utf8").trim().split("\n") : []);
// (Two more computers, each a home of its own: a Mac that has Kuvo Code's app, and one with nothing.)
const homes = Object.fromEntries(["macbook", "mini", "nas", "attic", "race", "claim", "rotate", "interrupted", "network-drop", "release", "add-first", "uninstall-first", "prepare", "old-reservation", "prepare-error", "probe-removal", "stop-uninstall", "primary-member", "late-enrollment", "replacement-standby", "backup-uninstall", "rejoin", "rejoin-neighbor", "rejoin-neighbor-return", "rejoin-clone-name", "rejoin-image", "rejoin-image-copy"].map(name => [name, path.join(base, name)]));
for (const [name, folder] of Object.entries(homes)) {
  fs.mkdirSync(folder, { recursive: true });
  fs.writeFileSync(path.join(fake, `home-${name}`), folder);
  for (const tool of ["pkill", "caffeinate"]) write(path.join(folder, ".kuvo-code", "bin", tool), `#!/bin/sh\nexit 0\n`, 0o755);
}
const pidsThere = (kuvo = remote) => {
  const pids = [];
  try { pids.push(read(path.join(kuvo, "run", "engine.json")).pid); } catch {}
  for (const file of ["box.pid", "link.pid", "agents.pid"]) try { pids.push(Number(fs.readFileSync(path.join(kuvo, "run", file), "utf8"))); } catch {}
  return pids.filter(pid => Number.isInteger(pid) && pid > 1);
};

atEnd(async () => {
  fs.rmSync(path.join(fake, "hold"), { force: true });
  let timer;
  try {
    await Promise.race([
      link.forgetTries(),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("Fixture SSH attempts did not stop")), 5000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
});

test("a Darwin fixture keeper with an empty environment reaches only its fake launchctl", { skip: process.platform !== "darwin" }, () => {
  const account = path.join(base, "keeper-scope");
  const box = path.join(account, ".kuvo-code");
  const keeper = path.join(box, "bin", "kuvo-start");
  const source = fs.readFileSync(path.join(release, "bin", "kuvo-start"), "utf8");
  assert.doesNotMatch(source, /\/bin\/launchctl\b/);
  write(keeper, source, 0o755);
  write(path.join(box, "run", "machine-helper"), "");
  const result = spawnSync("/bin/sh", [keeper, "box"], {
    env: { HOME: account, PATH: "/usr/bin:/bin" }, encoding: "utf8", timeout: 5000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 1, result.stderr);
  assert.equal(fs.readFileSync(path.join(account, "launchctl.log"), "utf8"),
    `kickstart gui/${process.getuid()}/run.kuvo.code.machine\nbootstrap gui/${process.getuid()} ${account}/Library/LaunchAgents/run.kuvo.code.machine.plist\n`);
});

test("a box says whether it adds computers, and suggests the hosts in ~/.ssh/config that aren't patterns", async () => {
  const said = await ask("GET", "kuvo/computers/add");
  assert.equal(said.status, 200);
  assert.deepEqual(said.body, { can: true, hosts: ["studio", "gpu", "github.com"], tries: [] });
  // An address ssh would read as something else never reaches it.
  for (const destination of ["-oProxyCommand=sh", "host:22", "a b", "ssh://x/y", "", "me@", "this-mac.localhost"]) {
    const refused = await ask("POST", "kuvo/computers/add", { destination });
    assert.equal(refused.status, 400, destination);
    assert.ok(refused.body.message.length > 10);
  }
  assert.deepEqual(sshLog(), [], "nothing reached ssh");
});

test("adding a computer: the box shows a host key it doesn't know, and on Trust copies itself there, starts it and signs it in", async () => {
  const begun = await ask("POST", "kuvo/computers/add", { destination: "studio", credentials: await checkedCredentials() });
  assert.equal(begun.status, 202);
  const id = begun.body.try.id;
  assert.equal(begun.body.try.title, "Adding studio…");

  // The host isn't known: read without signing in, and asked about, with its fingerprint.
  const asking = await until(id, found => found?.state === "asking");
  assert.equal(asking.question.id, "hostKey");
  assert.equal(asking.question.title, "Is this studio?");
  assert.ok(Number.isInteger(asking.question.at));
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at: asking.question.at + 1 })).status, 409, "an answer meant for another ask isn't taken");
  assert.deepEqual(asking.question.facts, [{ label: "Address", value: "studio" }, { label: "Fingerprint (ED25519)", value: link.fingerprintOf(keyOne), code: true }]);
  assert.deepEqual(asking.question.answers.map(each => each.id), ["cancel", "trust"]);
  assert.equal(fs.existsSync(path.join(remote, "bin", "kuvo-link.js")), false, "nothing put there before you answer");
  assert.equal(fs.existsSync(link.KNOWN_HOSTS), false, "and its key not kept yet");
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "sure", at: asking.question.at })).status, 409, "only one of its answers");
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust" })).status, 409, "nor one that doesn't say which ask it answers");
  assert.equal(fs.existsSync(link.KNOWN_HOSTS), false, "nothing trusted by any of those");

  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at: asking.question.at })).status, 200);
  const done = await over(id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.deepEqual(steps(done), ["checking:done", "files:done", "engine:done", "starting:done", "seeding:done", "joining:done"]);
  assert.equal(done.title, "studio is one of your computers");

  // Its key, in the box's own file; yours untouched.
  assert.equal(fs.readFileSync(link.KNOWN_HOSTS, "utf8"), `studio ${hostKey(keyOne)}\n`);
  assert.equal((fs.statSync(link.KNOWN_HOSTS).mode & 0o777).toString(8), "600");
  assert.equal(fs.existsSync(path.join(base, "here", ".ssh", "known_hosts")), false);

  // This box's own files there, each as it is here, with its mode; and the engine.
  for (const [name, mode] of link.RELEASE_FILES) {
    assert.equal(sha256(fs.readFileSync(path.join(remote, name))), sha256(fs.readFileSync(path.join(home, name))), name);
    assert.equal((fs.statSync(path.join(remote, name)).mode & 0o777).toString(8), mode.toString(8), name);
  }
  assert.equal(fs.readFileSync(path.join(remote, "engine", engineVersion, "opencode"), "utf8"), engineBinary);
  assert.deepEqual(fs.readdirSync(path.join(remote, "tmp")).filter(name => name.startsWith(".kuvo-files")), [], "nothing left of the copy");

  // Started by its own keeper: an engine with a port and password of its own, which never left it.
  const noted = read(path.join(remote, "run", "engine.json"));
  assert.ok(alive(noted.pid) && command(noted.pid).includes(path.join(remote, "engine", engineVersion, "opencode")), "its engine runs, from its own folder");
  assert.ok(Number.isInteger(noted.port) && noted.password);
  assert.equal(noted.notes.macName, "Studio");
  assert.equal(noted.notes.box, "1.0.0");
  const env = fs.readFileSync(path.join(remote, "run", "launch.env"), "utf8");
  assert.match(env, /^KUVO_MACHINE_NAME=studio$/m);
  assert.match(env, /^PATH=/m);
  assert.doesNotMatch(env, /PASSWORD|KUVO_KEY/, "no secret in how it starts again");
  assert.ok(alive(Number(fs.readFileSync(path.join(remote, "run", "box.pid"), "utf8"))), "its keeper keeps it from here on");
  // What it starts with, once: this box's settings and its integration's key, in its own box; and
  // its accounts' key and the sign-in that stays the same, in its own engine. Never the sign-in
  // that renews itself, an integration's own sign-in, a bot's key, or what only this Mac can use.
  assert.equal(done.steps.find(step => step.id === "seeding").detail, "Copied 1 key and 2 accounts. One sign-in renews itself, so sign in to it on studio.");
  const settings = read(path.join(remote, "box", "opencode.json"));
  assert.equal(settings.model, "anthropic/claude-opus-5-5");
  assert.deepEqual(settings.mcp.notion.environment, { NOTION_TOKEN: "{file:~/.kuvo-code/box/keys/KUVO_KEY_NOTION_TOKEN}" });
  assert.deepEqual([settings.shell, settings.$schema, settings.mcp["chrome-devtools"]], [undefined, undefined, undefined]);
  assert.deepEqual(fs.readdirSync(path.join(remote, "box", "keys")), ["KUVO_KEY_NOTION_TOKEN"]);
  assert.equal(fs.readFileSync(path.join(remote, "box", "keys", "KUVO_KEY_NOTION_TOKEN"), "utf8"), "secret-notion-token");
  assert.equal((fs.statSync(path.join(remote, "box", "keys", "KUVO_KEY_NOTION_TOKEN")).mode & 0o777).toString(8), "600");
  const accounts = read(path.join(there, "engine-credentials.json"));
  assert.deepEqual(accounts.map(each => [each.integrationID, each.label, each.value, each.active]), [
    ["anthropic", "Work", { type: "api", key: "sk-ant-secret-key" }, true],
    ["github-copilot", "me", { type: "oauth", refresh: "gho_stays_the_same", access: "gho_stays_the_same", expires: 0 }, true],
  ]);
  // None of it is written anywhere but there: not in what's said of the try, how it starts again, or its logs.
  const said = [fs.readFileSync(path.join(home, "run", "adding.json"), "utf8"), JSON.stringify(done), env, fs.readFileSync(path.join(fake, "log"), "utf8"),
    ...["launch.notes", "engine.log", "link.log"].map(name => { try { return fs.readFileSync(path.join(remote, "run", name), "utf8"); } catch { return ""; } })].join("\n");
  for (const secret of secrets) assert.equal(said.includes(secret), false, secret);
  // And after a restart.
  if (process.platform !== "darwin") assert.match(fs.readFileSync(crontabFile, "utf8"), /^@reboot sh "\$HOME\/\.kuvo-code\/bin\/kuvo-start" >\/dev\/null 2>&1 # kuvo-code: starts its engine again after a restart$/m);
  if (process.platform === "darwin") {
    const plist = fs.readFileSync(path.join(there, "Library", "LaunchAgents", "run.kuvo.code.machine.plist"), "utf8");
    assert.match(plist, /Kuvo Helper<\/string><string>keep-machine<\/string>/);
    assert.doesNotMatch(plist, /\/bin\/sh/);
    assert.match(fs.readFileSync(path.join(fake, "reached"), "utf8"), /codesign .*identifier "run\.kuvo\.code\.box".*subject\.OU.*C8Y523R7T5/);
    assert.doesNotMatch(fs.readFileSync(crontabFile, "utf8"), /kuvo-code/);
    const [keeper, helper] = fs.readFileSync(path.join(remote, "run", "machine-keeper"), "utf8").trim().split(/\s+/).map(Number);
    assert.equal(keeper, Number(fs.readFileSync(path.join(remote, "run", "box.pid"), "utf8")));
    assert.match(command(helper), /Kuvo Helper keep-machine/);
  }

  // Signed in: it knows this box as its Mac, and this box names it among your computers.
  const theirs = read(path.join(remote, "run", "link.json"));
  const own = k(link.publicOf(mac));
  assert.equal(theirs.mac, own);
  assert.equal(theirs.relay, "ws://127.0.0.1:1");
  assert.equal(theirs.id, "studio");
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  assert.equal(peer.name, "studio");
  assert.deepEqual(done.computer, { key: peer.key, name: "studio" });
  assert.deepEqual(read(path.join(home, "machines.json")).map(each => each.destination), ["studio"]);
  assert.equal(read(path.join(home, "machines.json"))[0].identity, fs.readFileSync(path.join(remote, "machine-id"), "utf8"));
  assert.equal(read(path.join(home, "phone-link.json")).machines.studio.key, peer.key);

  // Every client's list has it, with what this box can do with it over SSH; it's suggested no more.
  const listed = (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  assert.deepEqual(listed.actions.map(action => action.id), ["rename", "remove", "setup", "uninstall"]);
  assert.match(listed.actions.find(action => action.id === "uninstall").confirm.message, /Studio connects to studio over SSH/);
  assert.deepEqual((await ask("GET", "kuvo/computers/add")).body.hosts, ["gpu", "github.com"]);
  // Every connection, whatever your config says: no host key written to your own known hosts, no X11.
  const options = fs.readFileSync(path.join(fake, "options"), "utf8").split("\n");
  const count = option => options.filter(each => each === option).length;
  assert.ok(count("BatchMode=yes") > 5);
  assert.deepEqual([count("UpdateHostKeys=no"), count("ForwardX11=no"), count("ForwardAgent=no")], [count("BatchMode=yes"), count("BatchMode=yes"), count("BatchMode=yes")]);
  // One connection shared by its scripts, and let go of at the end.
  // (Where a socket's path fits: a Mac's short folder in /tmp; on Linux the box's own `run/ssh`,
  // which a test's deep home can be too long for, and then each script connects by itself.)
  const shares = process.platform === "darwin" || Buffer.byteLength(path.join(home, "run", "ssh")) <= 55;
  assert.equal(sshLog().at(-1), shares ? "studio  exit" : "studio yes run");
});

test("adding a Windows PC with only WSL 1 tells you how to get WSL 2 before copying Kuvo Code", async t => {
  const destination = "windows-no-wsl2";
  const folder = path.join(base, destination);
  fs.mkdirSync(folder);
  write(path.join(fake, `home-${destination}`), folder);
  write(path.join(fake, `windows-${destination}`), "");
  t.after(() => fs.rmSync(path.join(fake, `windows-${destination}`)));
  const begun = await ask("POST", "kuvo/computers/add", { destination });
  assert.equal(begun.status, 202);
  const id = begun.body.try.id;
  const asking = await until(id, found => found?.state === "asking");
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { at: asking.question.at, answer: "trust" })).status, 200);
  const failed = await over(id);
  assert.equal(failed.failure?.message, "Install Ubuntu with WSL 2 on desktop-test, then open Ubuntu once and try again.");
  assert.deepEqual(fs.readdirSync(folder), []);
});

test("the same computer under another address is the one already here, not a second one", async () => {
  fs.appendFileSync(link.KNOWN_HOSTS, `studio.lan ${hostKey(keyOne)}\n`);
  const begun = await ask("POST", "kuvo/computers/add", { destination: "studio.lan" });
  const done = await over(begun.body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.equal(done.known, true);
  assert.equal(done.title, "studio.lan is studio, already one of your computers");
  assert.equal(done.computer.name, "studio");
  assert.deepEqual(read(path.join(home, "machines.json")).map(each => [each.destination, each.otherAddresses]), [["studio", ["studio.lan"]]]);
  assert.equal(link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine").length, 1);
});

test("a host that can't be reached, one that refuses your keys, and one whose key changed are each said in words, and nothing is trusted", async () => {
  const fails = async destination => over((await ask("POST", "kuvo/computers/add", { destination })).body.try.id);
  const nowhere = await fails("nowhere");
  assert.equal(nowhere.state, "failed");
  assert.deepEqual(steps(nowhere), ["checking:failed", "files:waiting", "engine:waiting", "starting:waiting", "seeding:waiting", "joining:waiting"]);
  // (In words, with what to do next: what ssh said is in the log alone.)
  assert.equal(nowhere.failure.message, "Couldn’t reach nowhere. Make sure it’s on and connected, then Try Again.");
  assert.equal(nowhere.failure.again, true);

  // A key in your own known hosts that isn't the host's now: never asked about, never replaced.
  fs.writeFileSync(path.join(base, "here", ".ssh", "known_hosts"), `gpu ${hostKey(keyTwo)}\n`);
  const changed = await fails("gpu");
  assert.equal(changed.failure.title, "gpu’s host key changed");
  assert.match(changed.failure.message, /remove the old key in a terminal on Studio with ssh-keygen -R 'gpu', then try again\. If not, someone may be intercepting the connection\./);
  assert.doesNotMatch(fs.readFileSync(link.KNOWN_HOSTS, "utf8"), /^gpu /m);
  fs.rmSync(path.join(base, "here", ".ssh", "known_hosts"));

  // Refused: its key was trusted (you said so), and your keys weren't taken. The vault has none.
  fs.writeFileSync(path.join(fake, "refuse"), "");
  try {
    const begun = await ask("POST", "kuvo/computers/add", { destination: "gpu" });
    const asking = await until(begun.body.try.id, found => found?.state === "asking");
    assert.equal(asking.question.id, "hostKey");
    await ask("POST", `kuvo/computers/add/${begun.body.try.id}/answer`, { answer: "trust", at: asking.question.at });
    const refused = await over(begun.body.try.id);
    assert.equal(refused.failure.title, "gpu refused the connection");
    assert.match(refused.failure.message, /didn’t accept Studio’s SSH keys/);
  } finally {
    fs.rmSync(path.join(fake, "refuse"));
  }
  // What failed stays to be read, and is forgotten when you say.
  const kept = (await ask("GET", "kuvo/computers/add")).body.tries.map(each => each.destination);
  assert.deepEqual(kept.sort(), ["gpu", "nowhere", "studio", "studio.lan"]);
  assert.equal((await ask("DELETE", `kuvo/computers/add/${nowhere.id}`)).status, 200);
  assert.equal((await ask("GET", "kuvo/computers/add")).body.tries.some(each => each.id === nowhere.id), false);
});

test("a host key that changed from the one you trusted here is asked about again; Cancel leaves everything as it was", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const before = fs.readFileSync(link.KNOWN_HOSTS, "utf8");
  fs.writeFileSync(path.join(fake, "hostkey"), hostKey(keyTwo));
  try {
    const begun = await ask("POST", `kuvo/computers/${peer.key}/setup`);
    assert.equal(begun.status, 202);
    assert.equal(begun.body.try.title, "Setting up studio…");
    const asking = await until(begun.body.try.id, found => found?.state === "asking");
    assert.equal(asking.question.id, "hostKeyChanged");
    assert.equal(asking.question.facts[1].value, link.fingerprintOf(keyTwo));
    assert.deepEqual(asking.question.answers.map(each => [each.id, each.kind]), [["cancel", "plain"], ["trust", "danger"]]);
    await ask("POST", `kuvo/computers/add/${begun.body.try.id}/answer`, { answer: "cancel", at: asking.question.at });
    assert.equal(await over(begun.body.try.id), null, "stopped, and gone from the list");
    assert.equal(fs.readFileSync(link.KNOWN_HOSTS, "utf8"), before);
    // Trust New Key: the new key takes the old one's line (its other address's stays), and it goes on.
    const again = await ask("POST", `kuvo/computers/${peer.key}/setup`);
    const asked = await until(again.body.try.id, found => found?.state === "asking");
    await ask("POST", `kuvo/computers/add/${again.body.try.id}/answer`, { answer: "trust", at: asked.question.at });
    const done = await over(again.body.try.id);
    assert.equal(done.state, "done", JSON.stringify(done.failure));
    assert.deepEqual(fs.readFileSync(link.KNOWN_HOSTS, "utf8").trim().split("\n").filter(line => line.startsWith("studio")).sort(), [`studio ${hostKey(keyTwo)}`, `studio.lan ${hostKey(keyOne)}`]);
  } finally {
    fs.writeFileSync(link.KNOWN_HOSTS, before);
    fs.writeFileSync(path.join(fake, "hostkey"), hostKey(keyOne));
  }
});

test("setting a computer up again finds its files in place and leaves its running engine alone", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const engine = read(path.join(remote, "run", "engine.json")).pid;
  const begun = await ask("POST", `kuvo/computers/${peer.key}/setup`);
  const done = await over(begun.body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.deepEqual(done.steps.map(step => [step.id, step.detail]), [["checking", "Connecting to studio"], ["files", "It has them already"], ["engine", "It has it already"], ["starting", undefined], ["seeding", "It keeps its own"], ["joining", undefined]]);
  assert.equal(read(path.join(remote, "run", "engine.json")).pid, engine, "the engine at work wasn't started over");
  // What's there is its own from then on: nothing of this box's is given again.
  assert.equal(read(path.join(there, "engine-credentials.json")).length, 2);
  assert.equal(link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine").key, peer.key, "the same computer, once");
});

test("a machine whose Kuvo is older is brought up to date by this box whatever is at work there, its running engine left alone; Try Again only once it couldn't be reached for days", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const versionFile = path.join(remote, "bin", "box-version");
  const engine = read(path.join(remote, "run", "engine.json")).pid;
  const listed = async () => (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  // As new as this box: nothing is said, nothing offered, nothing begun.
  await link.keepUpRound();
  assert.equal((await listed()).update, undefined);
  assert.equal(link.allTries().some(each => each.kind === "update"), false);
  // Older (as a beta from before boxes took releases from a client), with two sessions at work.
  fs.writeFileSync(versionFile, "0.9.0\n");
  fs.rmSync(link.KEEP_UP_FILE, { force: true });
  // Not reached over SSH (your keys refused, say): no failed update, and tried again next round.
  fs.writeFileSync(path.join(fake, "refuse"), "");
  try {
    await link.keepUpRound();
  } finally {
    fs.rmSync(path.join(fake, "refuse"), { force: true });
  }
  // (It goes on trying by itself: nothing to press yet.)
  const missed = await listed();
  assert.deepEqual([missed.update.state, missed.update.title], ["waiting", "Kuvo on studio updates once Studio can reach it"]);
  assert.equal(missed.actions.some(each => each.id === "update"), false);
  assert.equal(read(link.KEEP_UP_FILE)[peer.key].failed, undefined);
  // Not reached for three days: said, with Try Again.
  const kept = read(link.KEEP_UP_FILE);
  kept[peer.key].unreachedSince -= 3 * 86_400_000;
  fs.writeFileSync(link.KEEP_UP_FILE, JSON.stringify(kept));
  const days = await listed();
  assert.deepEqual([days.update.state, days.update.title], ["failed", "Couldn’t reach studio"]);
  assert.match(days.update.detail, /^studio didn’t accept Studio’s SSH keys\..* Kuvo also tries again by itself later\.$/);
  assert.deepEqual(days.actions.find(each => each.id === "update")?.label, "Try Again", "a way on, never a card alone");
  // Two sessions at work there: the round brings it up all the same (nothing there stops), its
  // running engine left alone.
  const busy = path.join(there, "busy");
  fs.writeFileSync(busy, "");
  atEnd(() => fs.rmSync(busy, { force: true }));
  await link.keepUpRound();
  const begun = link.allTries().find(each => each.kind === "update" && each.state === "working");
  assert.ok(begun, "begun with something at work there");
  assert.equal(begun.title, "Updating Kuvo on studio…");
  assert.deepEqual((await listed()).update, { state: "updating", title: "Updating Kuvo on studio…", detail: "" });
  const done = await over(begun.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.equal(done.title, "studio is up to date");
  assert.equal(fs.readFileSync(versionFile, "utf8").trim(), "1.0.0");
  assert.equal(read(path.join(remote, "run", "engine.json")).pid, engine, "the engine at work wasn't started over");
  fs.rmSync(busy, { force: true });
  const after = await listed();
  assert.equal(after.update, undefined);
  assert.equal(after.actions.some(each => each.id === "update"), false);
  // Update Now on one as new already: nothing is put there again.
  const again = await ask("POST", `kuvo/computers/${peer.key}/update`);
  assert.equal(again.status, 202);
  const current = await over(again.body.try.id);
  assert.deepEqual(current.steps.map(step => [step.id, step.state, step.detail]).slice(1), ["files", "engine", "starting", "joining"].map(id => [id, "done", "It’s up to date already"]));
});

test("a machine heard from that never says its version (a box from before they said it) updates in a moment, before any look over SSH, with nothing to press; a look that finds it as new clears it", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const listed = async () => (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  const told = path.join(home, "run", "peer-agents.json");
  const before = fs.existsSync(told) ? fs.readFileSync(told, "utf8") : null;
  const all = before ? JSON.parse(before) : {};
  const { computer: _computer, ...rest } = all[peer.key] ?? {};
  fs.writeFileSync(told, JSON.stringify({ ...all, [peer.key]: { ...rest, at: Date.now(), heard: Date.now() } }));
  fs.rmSync(link.KEEP_UP_FILE, { force: true });
  try {
    const it = await listed();
    assert.equal(it.reach, "connected");
    assert.equal(it.version, undefined, "it never said");
    assert.deepEqual(it.update, { state: "waiting", title: "Kuvo on studio updates in a moment", detail: "" });
    assert.equal(it.actions.some(each => each.id === "update"), false, "it's brought up by itself");
    // One this box didn't set up over SSH (its record didn't come along): it can't be brought up
    // from here until it is, which is what it offers, never nothing.
    const list = path.join(home, "machines.json");
    const saved = fs.readFileSync(list, "utf8");
    fs.writeFileSync(list, "[]");
    try {
      const apart = await listed();
      assert.deepEqual(apart.update, { state: "waiting", title: "Kuvo on studio needs an update", detail: "Studio hasn’t set it up over SSH, so it can’t update it yet. Set it up over SSH once: what runs there stays." });
      assert.deepEqual(apart.actions.find(each => each.id === "add"), { id: "add", label: "Set Up Over SSH…" });
    } finally {
      fs.writeFileSync(list, saved);
    }
    // Looked at over SSH: as new as this box, so nothing more is said.
    await link.keepUpRound();
    const after = await listed();
    assert.equal(after.update, undefined);
    assert.equal(after.actions.some(each => each.id === "update"), false);
  } finally {
    if (before === null) fs.rmSync(told, { force: true }); else fs.writeFileSync(told, before);
  }
});

test("a machine whose address doesn't answer (a home network's while you're away) is reached at another it tells its Mac, its key checked as the one known; with none, it's said in words with Try Again", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const versionFile = path.join(remote, "bin", "box-version");
  const listed = async () => (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  const told = path.join(home, "run", "peer-agents.json");
  const before = fs.existsSync(told) ? fs.readFileSync(told, "utf8") : null;
  const all = before ? JSON.parse(before) : {};
  // (None of the addresses you added it under answers: the one it was added at, and any other.)
  const own = read(path.join(home, "machines.json")).find(each => each.destination === peer.id);
  const away = [peer.id, ...(own.otherAddresses ?? [])].map(address => path.join(fake, `away-${address}`));
  fs.writeFileSync(versionFile, "0.9.0\n");
  fs.rmSync(link.KEEP_UP_FILE, { force: true });
  for (const file of away) fs.writeFileSync(file, "");
  try {
    // Nowhere else to try: in words, with Try Again, and what ssh said in the log alone.
    fs.writeFileSync(told, JSON.stringify({ ...all, [peer.key]: { ...(all[peer.key] ?? {}), at: Date.now(), heard: Date.now(), computer: { version: "0.9.0", working: 0 } } }));
    const failed = await over((await ask("POST", `kuvo/computers/${peer.key}/update`)).body.try.id);
    assert.equal(failed.state, "failed");
    assert.equal(failed.failure.message, `Couldn’t reach ${peer.name}. Make sure it’s on and connected, then Try Again.`);
    const it = await listed();
    assert.deepEqual([it.update.state, it.update.title, it.update.detail], ["failed", `Couldn’t reach ${peer.name}`, "Make sure it’s on and connected, then Try Again. Kuvo also tries again by itself later."]);
    assert.equal(it.actions.find(each => each.id === "update")?.label, "Try Again");
    // It tells its Mac its tailnet's address: Try Again reaches it there, as the user it was set
    // up as, its host key checked against the one known for its first address; nothing is asked.
    fs.writeFileSync(told, JSON.stringify({ ...all, [peer.key]: { ...(all[peer.key] ?? {}), at: Date.now(), heard: Date.now(), computer: { version: "0.9.0", working: 0, addresses: ["100.101.2.3", "bad address", peer.id] } } }));
    assert.deepEqual(link.otherAddressesOf(peer.id), [...(own.otherAddresses ?? []).map(address => ({ address, own: true })), { address: "100.101.2.3", own: false }]);
    fs.rmSync(path.join(fake, "options"), { force: true });
    const done = await over((await ask("POST", `kuvo/computers/${peer.key}/update`)).body.try.id);
    assert.equal(done.state, "done", JSON.stringify(done.failure));
    assert.equal(fs.readFileSync(versionFile, "utf8").trim(), "1.0.0");
    assert.ok(sshLog().some(line => line.startsWith("100.101.2.3 yes run")), "reached at its other address, strictly checked");
    assert.ok(fs.readFileSync(path.join(fake, "options"), "utf8").split("\n").includes(`HostKeyAlias=${peer.id}`));
    // (Started again, it tells its Mac its new version: nothing more is said.)
    fs.writeFileSync(told, JSON.stringify({ ...all, [peer.key]: { ...(all[peer.key] ?? {}), at: Date.now(), heard: Date.now(), computer: { version: "1.0.0", working: 0 } } }));
    assert.equal((await listed()).update, undefined);
  } finally {
    for (const file of away) fs.rmSync(file, { force: true });
    if (before === null) fs.rmSync(told, { force: true }); else fs.writeFileSync(told, before);
  }
});

test("a machine says where else its Macs may reach it: its tailnet's address first, then its name, never a loopback or link-local one", () => {
  const interfaces = { lo0: [{ family: "IPv4", address: "127.0.0.1", internal: true }], en0: [{ family: "IPv4", address: "192.168.1.101", internal: false }, { family: "IPv6", address: "fe80::1", internal: false }], awdl0: [{ family: "IPv4", address: "169.254.3.4", internal: false }], tailscale0: [{ family: "IPv4", address: "100.88.7.6", internal: false }] };
  assert.deepEqual(link.ownAddresses(interfaces, "lokum"), ["100.88.7.6", "lokum", "192.168.1.101"]);
  assert.deepEqual(link.ownAddresses({}, "localhost"), []);
});

test("once this Mac has moved from the old app, the old app opening again stops nothing: it's told it was replaced, and machines are still kept up to date", async () => {
  const config = link.macLink.config;
  const record = path.join(home, "updates", "rehome", "state.json");
  const appConfig = { phones: true, machines: true, relay: "ws://127.0.0.1:1", name: "Studio", app: "0.7.4" };
  const quietly = async run => { const said = console.error; console.error = () => {}; try { return await run(); } finally { console.error = said; } };
  try {
    // Before a move, the old app attached keeps this box from adding machines, and says so.
    assert.equal((await ask("PUT", "kuvo/link/config", appConfig)).status, 200);
    assert.equal(link.oldAppAttached(), true);
    assert.match(link.canAdd().message ?? "", /^Quit the old Kuvo Code app on Studio first/);
    // Moved: the old app is refused with words, takes nothing over, and stops nothing.
    write(record, JSON.stringify({ state: "done", movedAt: Date.now() }));
    link.macLink.config = config;
    const refused = await ask("PUT", "kuvo/link/config", appConfig);
    assert.deepEqual([refused.status, refused.body.message], [410, "Kuvo Code 1.0 has replaced this app on Studio. Quit it and move it to the Trash: everything it did is in Kuvo Code now."]);
    assert.equal(link.macLink.config, config, "what the old app said isn't taken");
    assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [] })).status, 410, "nor its list of machines");
    assert.equal(link.oldAppAttached(), false);
    assert.equal(link.canAdd().can, true);
    await quietly(() => link.keepUpRound());
  } finally {
    fs.rmSync(path.join(home, "updates"), { recursive: true, force: true });
    link.macLink.config = config;
    link.macLink.appSaid = undefined;
  }
});

test("a phone's Add a Machine (the old Mac app's `POST kuvo/machines`) is this box's own add, waited for; what it must ask you is asked on the Mac", async () => {
  // Its machines, as the box says them itself: by the name your computers know it by, with the
  // hosts it could add.
  const listed = await ask("GET", "kuvo/machines");
  assert.equal(listed.status, 200, JSON.stringify(listed.body));
  assert.deepEqual(listed.body.machines.map(each => [each.id, each.name, each.otherAddresses]), [["studio", "studio", ["studio.lan"]]]);
  assert.doesNotMatch(listed.body.machines[0].status, /isn.t open/);
  assert.deepEqual(listed.body.hosts, ["gpu", "github.com"]);

  // One here already: said to be known at once, as the app answered, with nothing run there.
  const reached = sshLog().length;
  const again = await ask("POST", "kuvo/machines", { destination: "studio" });
  assert.equal(again.status, 200, JSON.stringify(again.body));
  assert.deepEqual([again.body.machine.id, again.body.known], ["studio", true]);
  // By another of its addresses: the same machine.
  const other = await ask("POST", "kuvo/machines", { destination: "studio.lan" });
  assert.deepEqual([other.status, other.body.machine?.id, other.body.known], [200, "studio", true]);
  assert.equal(read(path.join(home, "machines.json")).length, 1);
  assert.equal(sshLog().length, reached, "ssh wasn't run for either");

  // A host whose key this Mac doesn't know: nothing is trusted from a phone. The add is stopped,
  // and the answer says where to add it.
  const known = fs.readFileSync(link.KNOWN_HOSTS, "utf8");
  fs.writeFileSync(link.KNOWN_HOSTS, known.split("\n").filter(line => !line.startsWith("gpu ")).join("\n"));
  const kept = fs.readFileSync(link.KNOWN_HOSTS, "utf8");
  const unknown = await ask("POST", "kuvo/machines", { destination: "gpu" });
  assert.equal(unknown.status, 409, JSON.stringify(unknown.body));
  assert.equal(unknown.body.message, "Is this gpu? Add it from Kuvo Code on Studio, which shows you what to check first.");
  assert.equal(fs.readFileSync(link.KNOWN_HOSTS, "utf8"), kept, "its key wasn't kept");
  assert.equal((await ask("GET", "kuvo/computers/add")).body.tries.some(each => each.destination === "gpu" && each.state !== "failed" && each.state !== "done"), false, "and nothing is left asking");
  assert.equal(read(path.join(home, "machines.json")).length, 1);

  // The same add begun on the Mac itself, asking there: a phone's ask doesn't stop it, and says where it's asked.
  const onMac = await ask("POST", "kuvo/computers/add", { destination: "gpu" });
  const asking = await until(onMac.body.try.id, found => found?.state === "asking");
  // (Listed meanwhile, though it's on no list yet.)
  assert.deepEqual((await ask("GET", "kuvo/machines")).body.machines.map(each => [each.id, each.connection, each.status]).filter(each => each[0] === "gpu"), [["gpu", "installing", "Adding gpu…"]]);
  assert.equal((await ask("POST", "kuvo/machines", { destination: "gpu" })).status, 409);
  const stillAsking = (await ask("GET", "kuvo/computers/add")).body.tries.find(each => each.id === onMac.body.try.id);
  assert.deepEqual([stillAsking.state, stillAsking.question.at], ["asking", asking.question.at], "the Mac's own try is as it was");
  assert.equal((await ask("DELETE", `kuvo/computers/add/${onMac.body.try.id}`)).status, 200);
  assert.equal(await until(onMac.body.try.id, found => !found), null);
  assert.equal((await ask("GET", "kuvo/machines")).body.machines.some(each => each.id === "gpu"), false);
  fs.writeFileSync(link.KNOWN_HOSTS, known);

  // One that can't be reached: why, in the box's words.
  const nowhere = await ask("POST", "kuvo/machines", { destination: "nowhere" });
  assert.deepEqual([nowhere.status, nowhere.body.message], [502, "Couldn’t reach nowhere. Make sure it’s on and connected, then Try Again."]);
});

test("uninstalling while it's being set up waits for the start under way, then nothing runs there and nothing starts it again (#603)", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const engine = read(path.join(remote, "run", "engine.json")).pid;
  // The desktop and phone have independent grants. The desktop keeps its key on this machine.
  write(path.join(remote, "desktop", "identity"), "its-device-key", 0o600);
  const appThere = k(link.publicOf(link.newKey()));
  link.updateLink(state => {
    link.pairDevice(state, { key: appThere, name: "Studio screen" });
    link.pairPhone(state, { key: k(link.publicOf(link.newKey())), name: "My iPhone", added: 1, from: "pair", pairing: "its-own" });
    return state;
  });
  const confirms = (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key).actions.find(action => action.id === "uninstall").confirm.message;
  assert.match(confirms, /The sign-ins and keys it keeps are deleted\. Its sessions stay/);
  assert.doesNotMatch(confirms, /taken off your devices/);
  // A Set Up Again, held at its start there (the script that can't be cut short).
  fs.writeFileSync(path.join(fake, "hold"), 'bin/kuvo-start" start');
  const setup = await ask("POST", `kuvo/computers/${peer.key}/setup`);
  await until(setup.body.try.id, found => found?.steps.find(step => step.id === "starting")?.state === "now");
  // Uninstall asked meanwhile: the try stops (said so), and the uninstall waits its turn.
  const listedBusy = (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key);
  assert.equal(listedBusy.actions.find(action => action.id === "setup").why, "It’s being set up now.");
  const uninstall = await ask("POST", `kuvo/computers/${peer.key}/uninstall`);
  assert.equal(uninstall.status, 202);
  const tries = (await ask("GET", "kuvo/computers/add")).body.tries;
  assert.equal(tries.find(each => each.id === setup.body.try.id).state, "stopping");
  assert.equal(tries.find(each => each.id === setup.body.try.id).title, "Stopping…");
  assert.deepEqual(steps(tries.find(each => each.id === uninstall.body.try.id)), ["checking:waiting", "removing:waiting", "forgetting:waiting"], "not begun while the start is under way");
  // Asked twice (a second click, another client): the one uninstall, and setting it up waits for it.
  assert.equal((await ask("POST", `kuvo/computers/${peer.key}/uninstall`)).body.try.id, uninstall.body.try.id);
  const meanwhile = await ask("POST", `kuvo/computers/${peer.key}/setup`);
  assert.deepEqual([meanwhile.status, meanwhile.body.message], [409, "Kuvo Code is being uninstalled from studio now. Add it again once that’s done."]);
  const offered = (await ask("GET", "kuvo/computers")).body.computers.find(each => each.key === peer.key).actions;
  assert.equal(offered.find(action => action.id === "uninstall").why, "Kuvo Code is being uninstalled from it now.");
  fs.rmSync(path.join(fake, "hold"));

  const done = await over(uninstall.body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.equal(done.title, "studio no longer has Kuvo Code running");
  assert.equal((await ask("GET", "kuvo/computers/add")).body.tries.some(each => each.id === setup.body.try.id), false, "the try that was stopped is gone");

  // Nothing of it runs, and nothing starts it again.
  assert.equal(alive(engine), false, "its engine stopped");
  assert.deepEqual(pidsThere().filter(alive), [], "and its keeper, link and bridge");
  assert.doesNotMatch(fs.readFileSync(crontabFile, "utf8"), /kuvo-code/, "its line in the crontab is gone");
  assert.equal(fs.existsSync(path.join(there, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), false);
  assert.ok(fs.existsSync(path.join(remote, "run", "off")), "marked off, until it's added again");
  // It let this computer go, and this one took it off its list. Its sessions' folder stays.
  assert.deepEqual(read(path.join(remote, "run", "link.json")).macs ?? [], []);
  assert.deepEqual(link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine"), []);
  assert.deepEqual(read(path.join(home, "machines.json")), []);
  assert.ok(fs.existsSync(path.join(remote, "data")));
  assert.equal((await ask("POST", `kuvo/computers/${peer.key}/uninstall`)).status, 404, "no longer one it set up");
  // Uninstall clears the local key. The primary keeps independent device grants until removed.
  assert.equal(fs.existsSync(path.join(remote, "desktop")), false, "the app's key there is deleted");
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name).sort(), ["My iPhone", "Studio screen"]);
  assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: appThere }] })).status, 200);
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name), ["My iPhone"]);
  assert.deepEqual(link.ownPhones(link.readLink()).removed.map(each => each.key), [appThere]);
});

// Adds `destination`, trusting its key: the try as it ended (or as `wanted` says).
const added = async (destination, wanted) => {
  const begun = await ask("POST", "kuvo/computers/add", { destination, credentials: await checkedCredentials() });
  const asking = await until(begun.body.try.id, found => found?.state === "asking");
  assert.equal(asking.question.id, "hostKey");
  await ask("POST", `kuvo/computers/add/${begun.body.try.id}/answer`, { answer: "trust", at: asking.question.at });
  return wanted ? until(begun.body.try.id, wanted) : over(begun.body.try.id);
};
const nothingOfOurs = folder => {
  const kuvo = path.join(folder, ".kuvo-code");
  let keys = [];
  try { keys = fs.readdirSync(path.join(kuvo, "box", "keys")); } catch {}
  assert.deepEqual(keys, [], "no key of this box's there");
  for (const file of ["box/opencode.json", "run/engine.json", "run/link.json"]) {
    let text = "";
    try { text = fs.readFileSync(path.join(kuvo, file), "utf8"); } catch {}
    for (const secret of secrets) assert.equal(text.includes(secret), false, `${file}: ${secret}`);
  }
};

test("a Mac with earlier Kuvo Code data must update before being added and keeps its data", async () => {
  for (const [destination, file] of [["old07", "opencode/opencode.db"], ["oldkeeper", "box/run/keeper.json"]]) {
    const folder = path.join(base, destination);
    fs.mkdirSync(folder, { recursive: true });
    write(path.join(fake, `home-${destination}`), folder);
    const oldFile = path.join(folder, "Library", "Application Support", "run.kuvo.code", file);
    write(oldFile, "kept exactly");
    const before = fs.readdirSync(folder, { recursive: true }).sort();
    const done = await added(destination);
    assert.equal(done.state, "failed");
    assert.equal(done.failure.message, `${destination} still has Kuvo Code 0.7’s sessions. Update it to Kuvo first — its sessions move over then — and add it again.`);
    assert.deepEqual(fs.readdirSync(folder, { recursive: true }).sort(), [".kuvo-code-adding.lock", ...before]);
    assert.equal(fs.statSync(path.join(folder, ".kuvo-code-adding.lock")).mode & 0o777, 0o600);
    assert.equal(fs.readFileSync(oldFile, "utf8"), "kept exactly");
    assert.equal(fs.existsSync(path.join(folder, ".kuvo-code")), false);
  }
});

test("a Mac that's a machine's box already, with earlier Kuvo Code data beside it, is added and set up again as before; that data is left as it is", async () => {
  // The installer there leaves both (`two`): only a Mac whose earlier data it would move must update first.
  const folder = path.join(base, "machine07");
  fs.mkdirSync(folder, { recursive: true });
  write(path.join(fake, "home-machine07"), folder);
  for (const tool of ["pkill", "caffeinate"]) write(path.join(folder, ".kuvo-code", "bin", tool), `#!/bin/sh\nexit 0\n`, 0o755);
  write(path.join(folder, ".kuvo-code", "bin", "kuvo-start"), "#!/bin/sh\nexit 0\n", 0o755);
  write(path.join(folder, ".kuvo-code", "box", "kind"), "machine\n");
  const oldFile = path.join(folder, "Library", "Application Support", "run.kuvo.code", "opencode", "opencode.db");
  write(oldFile, "kept exactly");
  const done = await added("machine07");
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  const again = await over((await ask("POST", `kuvo/computers/${done.computer.key}/setup`)).body.try.id);
  assert.equal(again.state, "done", JSON.stringify(again.failure));
  assert.equal(fs.readFileSync(oldFile, "utf8"), "kept exactly");
  // Taken off again, and forgotten, for the tests after.
  const uninstall = await ask("POST", `kuvo/computers/${done.computer.key}/uninstall`);
  assert.equal((await over(uninstall.body.try.id)).state, "done");
  assert.equal((await ask("DELETE", "kuvo/hosts/machine07")).status, 200);
});

test("a Mac with an existing app folder is added without copying sign-ins or keys", async () => {
  // Only the app's folder, no box of a machine's: judged free, and added with no question.
  fs.mkdirSync(path.join(homes.macbook, "Library", "Application Support", "run.kuvo.code"), { recursive: true });
  const done = await added("macbook");
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.equal(done.steps.find(step => step.id === "seeding").detail, "It keeps its own");
  nothingOfOurs(homes.macbook);
  assert.equal(fs.existsSync(path.join(homes.macbook, "engine-credentials.json")), false, "nor any account in its engine");
  assert.equal(fs.existsSync(path.join(homes.macbook, ".kuvo-code", "run", "seeded.json")), false);
  // Taken off again, for the tests after.
  const uninstall = await ask("POST", `kuvo/computers/${done.computer.key}/uninstall`);
  assert.equal((await over(uninstall.body.try.id)).state, "done");
});

test("a computer set up again with keys made anew keeps its independently paired app", async () => {
  // (Its host key is trusted already, from when it was added before: nothing is asked.)
  const done = await over((await ask("POST", "kuvo/computers/add", { destination: "macbook" })).body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  // Kuvo Code's app there has a device grant of its own.
  const appThere = k(link.publicOf(link.newKey()));
  link.updateLink(state => {
    link.pairDevice(state, { key: appThere, name: "MacBook screen" });
    return state;
  });
  // Its box's keys are lost there (stopped, its link's file gone), and it's set up again: the same
  // computer, at the same address, with keys of its own made anew.
  const kuvo = path.join(homes.macbook, ".kuvo-code");
  for (const file of ["box.pid", "link.pid"]) {
    let pid = 0;
    try { pid = Number(fs.readFileSync(path.join(kuvo, "run", file), "utf8")); } catch {}
    if (pid > 1 && alive(pid) && command(pid).includes(kuvo)) process.kill(pid);
  }
  fs.rmSync(path.join(kuvo, "run", "link.json"));
  const again = await ask("POST", `kuvo/computers/${done.computer.key}/setup`);
  const set = await over(again.body.try.id);
  assert.equal(set.state, "done", JSON.stringify(set.failure));
  const now = link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine");
  assert.deepEqual(now.map(each => [each.name, each.key === done.computer.key]), [["macbook", false]], "the one computer, by another key");
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name), ["My iPhone", "MacBook screen"], "its app is still one of your devices");
  // Removing the computer leaves the app's grant until its own removal.
  const uninstall = await ask("POST", `kuvo/computers/${now[0].key}/uninstall`);
  assert.equal((await over(uninstall.body.try.id)).state, "done");
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name), ["My iPhone", "MacBook screen"]);
  assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: appThere }] })).status, 200);
});

test("an add that fails, or is stopped, after Kuvo Code started on a computer that had nothing leaves nothing running there and no key behind", async () => {
  const kuvo = path.join(homes.mini, ".kuvo-code");
  const gone = why => {
    assert.deepEqual(pidsThere(kuvo).filter(alive), [], `${why}: nothing of it runs`);
    assert.doesNotMatch(fs.readFileSync(crontabFile, "utf8"), /kuvo-code/, `${why}: nothing starts it again`);
    assert.equal(fs.existsSync(path.join(homes.mini, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), false);
    assert.equal(fs.existsSync(path.join(kuvo, "box", "keys")), false, `${why}: the keys copied there are deleted`);
    assert.equal(fs.existsSync(path.join(kuvo, "run", "launch.env")), false);
    assert.deepEqual(link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine"), [], `${why}: not one of your computers`);
  };
  // Its join fails (its link breaks as it's signed in): it had started, with this box's keys.
  fs.writeFileSync(path.join(fake, "fail"), 'kuvo-link.js" setup');
  let failed;
  try {
    failed = await added("mini");
  } finally {
    fs.rmSync(path.join(fake, "fail"));
  }
  assert.equal(failed.state, "failed");
  assert.deepEqual(steps(failed), ["checking:done", "files:done", "engine:done", "starting:done", "seeding:done", "joining:failed"]);
  assert.equal(failed.failure.message, "Kuvo Code on mini couldn’t join your computers. it broke Kuvo Code was stopped on mini again, and the keys and sign-ins copied there were deleted.");
  assert.equal(fs.existsSync(path.join(homes.mini, "engine-credentials.json")), true, "(it had been given its accounts)");
  gone("failed");

});

test("an add stopped while its accounts are being copied, on a computer that had nothing, is taken off there again", async () => {
  const kuvo = path.join(homes.nas, ".kuvo-code");
  fs.writeFileSync(path.join(fake, "hold"), 'kuvo-link.js" box seed');
  const seeding = await added("nas", found => found?.steps.find(step => step.id === "seeding")?.state === "now");
  assert.ok(pidsThere(kuvo).filter(alive).length > 0, "it runs there by now");
  assert.equal((await ask("DELETE", `kuvo/computers/add/${seeding.id}`)).status, 200);
  fs.rmSync(path.join(fake, "hold"));
  assert.equal(await over(seeding.id), null, "stopped, and gone from the list");
  assert.deepEqual(pidsThere(kuvo).filter(alive), [], "nothing of it runs");
  assert.doesNotMatch(fs.readFileSync(crontabFile, "utf8"), /kuvo-code/, "nothing starts it again");
  assert.equal(fs.existsSync(path.join(kuvo, "box", "keys")), false, "the keys copied there are deleted");
  assert.deepEqual(link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine"), []);
});

test("a computer that had Kuvo Code before (uninstalled there, its sign-ins kept) is never seeded, and a failed add leaves what it keeps", async () => {
  // As `kuvo-link.js uninstall` leaves one: off, nothing to start it, its id, sign-ins and keys still there.
  const kuvo = path.join(homes.attic, ".kuvo-code");
  write(path.join(kuvo, "machine-id"), "0123456789abcdef0123456789abcdef", 0o600);
  write(path.join(kuvo, "run", "off"), "");
  write(path.join(kuvo, "box", "opencode.json"), JSON.stringify({ model: "its/own" }), 0o600);
  write(path.join(kuvo, "box", "keys", "KUVO_KEY_ITS_OWN"), "its-own-key", 0o600);
  write(path.join(kuvo, "agents", "codex", "auth.json"), "its-own-sign-in", 0o600);
  write(path.join(homes.attic, "engine-credentials.json"), JSON.stringify([{ id: "c", integrationID: "openai", label: "theirs", active: true, value: { type: "oauth", refresh: "its-own-refresh" } }]), 0o600);
  fs.writeFileSync(path.join(fake, "fail"), 'kuvo-link.js" setup');
  let failed;
  try {
    failed = await added("attic");
  } finally {
    fs.rmSync(path.join(fake, "fail"));
  }
  assert.equal(failed.state, "failed");
  assert.equal(failed.steps.find(step => step.id === "seeding").detail, "It keeps its own");
  assert.equal(failed.failure.message, "Kuvo Code on attic couldn’t join your computers. it broke", "nothing was deleted there, and it doesn't say so");
  // What it kept is as it was: none of it written over by this box's, none of it deleted.
  assert.deepEqual(fs.readdirSync(path.join(kuvo, "box", "keys")), ["KUVO_KEY_ITS_OWN"]);
  assert.equal(fs.readFileSync(path.join(kuvo, "box", "keys", "KUVO_KEY_ITS_OWN"), "utf8"), "its-own-key");
  assert.equal(fs.readFileSync(path.join(kuvo, "agents", "codex", "auth.json"), "utf8"), "its-own-sign-in");
  assert.equal(read(path.join(kuvo, "box", "opencode.json")).model, "its/own");
  assert.deepEqual(read(path.join(homes.attic, "engine-credentials.json")).map(each => [each.integrationID, each.label]), [["openai", "theirs"]]);
  assert.equal(fs.existsSync(path.join(kuvo, "run", "seeded.json")), false);
});

test("a phone's Remove, Uninstall and Forget (the old Mac app's `kuvo/machines/<id>` and `kuvo/hosts/<id>`) are this box's own", async () => {
  // (Its key trusted before, so nothing to ask.)
  const added = await ask("POST", "kuvo/machines", { destination: "macbook" });
  assert.equal(added.status, 200, JSON.stringify(added.body));
  assert.deepEqual([added.body.machine.id, added.body.known], ["macbook", false]);
  const peer = () => link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  assert.ok(peer(), "signed in to your computers");
  const hosts = async () => (await ask("GET", "kuvo/hosts")).body.hosts.map(each => [each.machine, each.removed]);
  assert.deepEqual(await hosts(), [["macbook", false], ["studio", true]]);

  // Remove while Set Up Again is in flight. Its old join cannot restore this peer.
  fs.writeFileSync(path.join(fake, "hold"), 'bin/kuvo-start" start');
  let setup;
  try {
    setup = await ask("POST", `kuvo/computers/${peer().key}/setup`);
    await until(setup.body.try.id, found => found?.steps.find(step => step.id === "starting")?.state === "now");
    assert.equal((await ask("DELETE", "kuvo/machines/macbook")).status, 200);
  } finally {
    fs.rmSync(path.join(fake, "hold"));
  }
  const ended = await over(setup.body.try.id);
  assert.equal(ended.state, "failed");
  assert.match(ended.failure.message, /was removed from your computers/);
  assert.deepEqual(read(path.join(home, "machines.json")), []);
  assert.equal(peer(), undefined);
  assert.ok(pidsThere(path.join(homes.macbook, ".kuvo-code")).filter(alive).length > 0, "it runs on");
  assert.deepEqual(await hosts(), [["macbook", true], ["studio", true]]);
  assert.equal((await ask("DELETE", "kuvo/machines/macbook")).status, 404);

  // Added again, then uninstalled: waited for, so the answer says how it went.
  assert.equal((await ask("POST", "kuvo/machines", { destination: "macbook" })).status, 200);
  // (Never by a one-off command, which anything on the Mac could run.)
  const once = [];
  for await (const piece of handle({ method: "POST", path: "kuvo/machines/macbook/uninstall", headers: {}, fromMac: true }, undefined)) once.push(piece);
  assert.equal(once.find(each => each.head).head.status, 409);
  assert.ok(peer(), "still one of your computers");
  fs.writeFileSync(path.join(fake, "away-macbook"), "");
  try {
    const failed = await ask("POST", "kuvo/machines/macbook/uninstall");
    assert.equal(failed.status, 502, JSON.stringify(failed.body));
    assert.ok(failed.body.message.length > 10);
    assert.equal(read(path.join(home, "machines.json")).length, 1, "still on the list, as it's still there");
  } finally {
    fs.rmSync(path.join(fake, "away-macbook"));
  }
  assert.deepEqual([(await ask("POST", "kuvo/machines/macbook/uninstall")).status, read(path.join(home, "machines.json"))], [200, []]);
  assert.deepEqual(pidsThere(path.join(homes.macbook, ".kuvo-code")).filter(alive), [], "nothing of it runs there");
  assert.equal(peer(), undefined);

  // Forgotten for your phones too, once. (The other stays kept, for the tests after.)
  assert.deepEqual([(await ask("DELETE", "kuvo/hosts/macbook")).status, (await ask("DELETE", "kuvo/hosts/macbook")).status], [200, 404]);
  assert.deepEqual(await hosts(), [["studio", true]]);
});

test("a member of a setup, and a request that can't stay, are told where to add from", async () => {
  // One request and gone (`kuvo-link.js box route`): nothing could stay to do it.
  const pieces = [];
  for await (const piece of handle({ method: "POST", path: "kuvo/computers/add", headers: {}, fromMac: true, body: Buffer.from(JSON.stringify({ destination: "gpu" })) }, undefined)) pieces.push(piece);
  assert.equal(pieces.find(each => each.head).head.status, 409);
  // (Nor by the old route's name.)
  const old = [];
  for await (const piece of handle({ method: "POST", path: "kuvo/machines", headers: {}, fromMac: true, body: Buffer.from(JSON.stringify({ destination: "gpu" })) }, undefined)) old.push(piece);
  assert.equal(old.find(each => each.head).head.status, 409);
  assert.equal((await ask("GET", "kuvo/computers/add")).body.tries.some(each => each.destination === "gpu" && each.state === "working"), false);
  // The old app attached keeps its own list of machines: adding waits for it to be quit.
  link.macLink.appSaid = Date.now();
  try {
    const said = (await ask("GET", "kuvo/computers/add")).body;
    assert.equal(said.can, false);
    assert.match(said.why, /Quit the old Kuvo Code app on Studio first/);
    assert.equal((await ask("POST", "kuvo/computers/add", { destination: "gpu" })).status, 409);
  } finally {
    link.macLink.appSaid = undefined;
  }
});

test("a missing or unreadable list of machines isn't overwritten when this box still keeps machines' links (#753)", async () => {
  const list = path.join(home, "machines.json");
  const before = fs.readFileSync(list);
  assert.ok(Object.keys(read(path.join(home, "phone-link.json")).machines).length > 0);
  try {
    const front = link.macFront(handle, { wait: 10, look: 60_000 });
    for (const contents of [null, "{ invalid JSON"]) {
      if (contents === null) fs.rmSync(list);
      else fs.writeFileSync(list, contents);
      const said = (await ask("GET", "kuvo/computers/add")).body;
      assert.deepEqual([said.can, said.why], [false, "Studio’s list of machines can’t be read right now."]);
      const refused = await ask("POST", "kuvo/computers/add", { destination: "gpu" });
      assert.deepEqual([refused.status, refused.body.message], [503, "Studio’s list of machines can’t be read right now."]);
      assert.equal(fs.existsSync(list) ? fs.readFileSync(list, "utf8") : null, contents);
      const pieces = [];
      for await (const piece of front({ method: "GET", path: "kuvo/machines", headers: {} }, undefined)) pieces.push(piece);
      assert.equal(pieces.find(each => each.head).head.status, 503);
    }
  } finally {
    fs.writeFileSync(list, before);
  }
});

test("an account enrolled after the probe keeps its data when the second setup is refused", async () => {
  const kuvo = path.join(homes.race, ".kuvo-code");
  const hold = path.join(fake, "hold");
  fs.writeFileSync(hold, 'kuvo-link.js" setup');
  let joining;
  const saved = path.join(kuvo, "run", "link.json");
  const keyFile = path.join(kuvo, "box", "keys", "KUVO_KEY_EXISTING");
  const mac = crypto.randomBytes(32).toString("base64url");
  const state = JSON.stringify({ mac, macs: [mac], phones: [] }, null, 2);
  try {
    joining = await added("race", found => found?.steps.find(step => step.id === "joining")?.state === "now");
    write(saved, state);
    write(keyFile, "existing-key", 0o600);
  } finally {
    fs.rmSync(hold);
  }
  const refused = await over(joining.id);
  assert.equal(refused.state, "failed");
  assert.match(refused.failure.message, /connect over SSH to your own OS account/);
  assert.equal(fs.readFileSync(saved, "utf8"), state);
  assert.equal(fs.readFileSync(keyFile, "utf8"), "existing-key");
});

test("a second SSH add cannot install or seed an account reserved by the first", async () => {
  const folder = homes.claim;
  const hold = path.join(fake, "hold-claim");
  write(path.join(fake, "home-claim-alias"), folder);
  write(hold, "tar xf - -C");
  let first;
  try {
    first = await added("claim", found => found?.steps.find(step => step.id === "files")?.state === "now");
    const second = await added("claim-alias");
    assert.equal(second.state, "failed");
    assert.match(second.failure.message, /change is in progress/);
    assert.equal(fs.existsSync(path.join(folder, ".kuvo-code", "box", "keys")), false);
    assert.equal(fs.existsSync(path.join(folder, "engine-credentials.json")), false);
  } finally {
    fs.rmSync(hold);
    if (first) await over(first.id);
  }
  assert.equal((await over(first.id)).state, "done");
  assert.equal(fs.lstatSync(path.join(folder, ".kuvo-code-adding"), { throwIfNoEntry: false }), undefined);
  const uninstall = await ask("POST", `kuvo/computers/${(await over(first.id)).computer.key}/uninstall`);
  assert.equal((await over(uninstall.body.try.id)).state, "done");
});

test("a saved offline computer reconnects over SSH after its Mac rotates keys", async () => {
  const original = link.macLink.overlay;
  const addedBox = await added("rotate");
  assert.equal(addedBox.state, "done");
  const remote = path.join(homes.rotate, ".kuvo-code", "run");
  const card = path.join(remote, "setup-card.json");
  const oldKey = k(link.publicOf(Buffer.from(original.key, "base64url")));
  const oldSetup = link.newSetupID();
  write(card, JSON.stringify({ ...read(card), setup: oldSetup, role: "member", primary: { key: oldKey, name: "Studio", kind: "mac" } }));
  const rotated = link.newKey();
  const newKey = k(link.publicOf(rotated));
  const beforeRotation = link.readLink();
  link.macLink.overlay = { ...original, key: k(rotated), sign: link.newSigningSeed() };
  try {
    link.updateLink(state => ({ ...state, keyNow: newKey, keysBefore: [oldKey], setup: {
      id: link.newSetupID(), epoch: 1, primary: newKey, sign: link.ownSign(state), role: "primary",
    } }));
    const reconnect = await ask("POST", "kuvo/computers/add", { destination: "rotate" });
    const done = await over(reconnect.body.try.id);
    assert.equal(done.state, "done", JSON.stringify(done.failure));
    assert.equal(done.computer.key, addedBox.computer.key);
    assert.deepEqual(read(path.join(remote, "link.json")).macs, [newKey]);
  } finally {
    link.macLink.overlay = original;
    link.writeLink(beforeRotation);
  }
});

const traceFile = path.join(fake, "trace");
const remoteCalls = () => fs.readFileSync(traceFile, "utf8").trim().split("\n");
const reservationAt = folder => path.join(folder, ".kuvo-code-adding");
const retryAdd = async destination => over((await ask("POST", "kuvo/computers/add", { destination, credentials: await checkedCredentials() })).body.try.id);
const answerRecovery = (attempt, answer) => ask("POST", `kuvo/computers/add/${attempt.id}/answer`, { answer, at: attempt.question.at });
const waitFor = async found => {
  for (let i = 0; i < 200; i++) {
    if (found()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.fail("The remote command did not reach the test hold.");
};

for (const [destination, disconnected] of [["interrupted", "SIGTERM"], ["network-drop", "255"]]) {
  test(`failure bench: a credential write interrupted by SSH ${disconnected} stops Add and recovery waits for the remote writer`, async () => {
    const folder = homes[destination];
    const alias = `${destination}-alias`;
    const hold = path.join(folder, "hold-seed");
    const drop = path.join(fake, `drop-seed-${destination}`);
    write(path.join(fake, `home-${alias}`), folder);
    write(traceFile, "");
    write(hold, "");
    write(drop, disconnected);
    try {
      const stopped = await added(destination);
      const writes = remoteCalls().filter(call => call.startsWith(`${destination} `) && / (seed|join|uninstall|release)$/.test(call));
      assert.deepEqual(writes, [`${destination} seed`], "one seed call and no join, uninstall or release");
      assert.equal(stopped.state, "failed");
      assert.match(stopped.failure.message, /lost contact.*during a change/);
      assert.equal(stopped.steps.find(step => step.id === "joining").state, "waiting");
      const original = fs.readlinkSync(reservationAt(folder));
      const whileWriting = await added(alias);
      assert.equal(whileWriting.state, "failed");
      assert.match(whileWriting.failure.message, /change is in progress/);
      assert.equal(fs.readlinkSync(reservationAt(folder)), original);

      fs.rmSync(hold);
      await waitFor(() => fs.existsSync(path.join(folder, "writer-ended")));
      fs.rmSync(drop);
      const mismatched = disconnected === "255";
      const recovery = mismatched
        ? await over((await ask("POST", "kuvo/computers/add", { destination, credentials: [] })).body.try.id)
        : await retryAdd(destination);
      assert.equal(recovery.question?.id, "recoverAccount");
      assert.equal(recovery.question.title, `Continue adding ${destination}?`);
      assert.equal(recovery.question.message, `An earlier attempt to add ${destination} didn't finish, and it's no longer running. Kuvo can clear it and start again.`);
      assert.deepEqual(recovery.question.answers.map(answer => answer.label), ["Cancel", "Clear and Continue"]);
      assert.equal(fs.readlinkSync(reservationAt(folder)), original, "recovery requires an answer");
      assert.equal((await answerRecovery(recovery, "recover")).status, 200);
      let recovered = await over(recovery.id);
      if (mismatched) {
        assert.equal(recovered.state, "failed");
        assert.match(recovered.failure.message, /different choice of sign-ins and keys/);
        assert.deepEqual(recovered.credentials, stopped.credentials);
        recovered = await retryAdd(destination);
      }
      assert.equal(recovered.state, "done", JSON.stringify(recovered.failure));
      assert.equal(fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false }), undefined);
      assert.equal(read(path.join(folder, "engine-credentials.json")).length, 2);
      assert.equal(remoteCalls().filter(call => call === `${destination} seed`).length, 2, "the resumed add completes the selected seed step");
      assert.ok(!fs.existsSync(path.join(folder, ".kuvo-code/run/.ssh-fresh-add")));
    } finally {
      fs.rmSync(hold, { force: true });
      fs.rmSync(drop, { force: true });
      fs.rmSync(traceFile);
    }
  });
}

test("a failed final release is reported and an abandoned reservation can be cleared explicitly", async () => {
  const folder = homes.release;
  const fail = path.join(fake, "fail-release-release");
  const move = path.join(tools, "mv");
  const replaced = path.join(folder, "replaced-reservation");
  write(traceFile, "");
  write(fail, "");
  try {
    const result = await added("release");
    assert.equal(result.state, "failed");
    assert.match(result.failure.message, /finished the change but couldn’t clear the account/);
    assert.ok(result.computer?.key, "the failed release reports the committed computer");
    const original = fs.readlinkSync(reservationAt(folder));
    fs.rmSync(fail);
    const question = await retryAdd("release");
    assert.equal(question.question?.id, "recoverAccount");
    assert.equal((await answerRecovery(question, "cancel")).status, 200);
    assert.equal(await over(question.id), null);
    assert.equal(fs.readlinkSync(reservationAt(folder)), original);
    const recovery = await retryAdd("release");
    assert.equal(recovery.question?.id, "recoverAccount");
    write(move, `#!/bin/sh
for last do :; done
if [ "$last" = "$HOME/.kuvo-code-adding" ]; then
  [ -L "$last" ] || exit 1
  readlink "$last" > "$HOME/replaced-reservation"
fi
exec /bin/mv "$@"
`, 0o755);
    assert.equal((await answerRecovery(recovery, "recover")).status, 200);
    const done = await over(recovery.id);
    assert.equal(done.state, "done", JSON.stringify(done.failure));
    assert.equal(done.computer.key, result.computer.key);
    assert.equal(fs.readFileSync(replaced, "utf8").trim(), original, "the old reservation still exists at replacement");
    assert.deepEqual(fs.readdirSync(folder).filter(name => name.startsWith(".kuvo-code-adding.recovery.")), []);
    assert.equal(fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false }), undefined);
  } finally {
    fs.rmSync(move, { force: true });
    fs.rmSync(fail, { force: true });
    fs.rmSync(traceFile);
  }
});

test("a recovery answer only clears the reservation shown to the user", async () => {
  const folder = homes.prepare;
  const marker = reservationAt(folder);
  const interrupted = "guarded-" + crypto.randomBytes(24).toString("hex");
  const newer = "guarded-" + crypto.randomBytes(24).toString("hex");
  fs.symlinkSync(interrupted, marker);
  try {
    const recovery = await added("prepare");
    assert.equal(recovery.question?.id, "recoverAccount");
    fs.rmSync(marker);
    fs.symlinkSync(newer, marker);
    assert.equal((await answerRecovery(recovery, "recover")).status, 200);
    assert.equal((await over(recovery.id)).state, "failed");
    assert.equal(fs.readlinkSync(marker), newer);
  } finally {
    fs.rmSync(marker);
  }
});

test("an old reservation is kept and Add explains how to clear it after a restart", async () => {
  const marker = reservationAt(homes["old-reservation"]);
  const original = "0123456789abcdef".repeat(3);
  fs.symlinkSync(original, marker);
  const result = await added("old-reservation");
  assert.equal(result.state, "failed");
  assert.equal(result.failure.message, "An older version of Kuvo Code left an unfinished change on old-reservation. Restart old-reservation to end that change, then sign in to the same OS account and run rm -f ~/.kuvo-code-adding before trying again.");
  assert.equal(fs.readlinkSync(marker), original);
});

test("SSH uninstall cannot probe ownership while Add holds the account", async () => {
  const folder = homes["add-first"];
  const installed = await added("add-first");
  assert.equal(installed.state, "done");
  const hold = path.join(fake, "hold-add-first-alias");
  const keyFile = path.join(folder, ".kuvo-code", "box", "keys", "KUVO_KEY_NOTION_TOKEN");
  write(path.join(fake, "home-add-first-alias"), folder);
  write(traceFile, "");
  write(hold, "uname -sm");
  try {
    const adding = await added("add-first-alias", found => found?.state === "working" && !found.question);
    await waitFor(() => remoteCalls().includes("add-first-alias probe"));
    const begun = await ask("POST", `kuvo/computers/${installed.computer.key}/uninstall`);
    const uninstallSecond = await over(begun.body.try.id);
    assert.equal(uninstallSecond.state, "failed");
    assert.equal(remoteCalls().includes("add-first probe"), false, "uninstall first needs the reservation");
    assert.match(uninstallSecond.failure.message, /change is in progress/);
    assert.ok(fs.existsSync(keyFile));
    fs.rmSync(hold);
    assert.equal((await over(adding.id)).state, "done");
  } finally {
    fs.rmSync(hold, { force: true });
    fs.rmSync(traceFile);
  }
});

test("SSH Add is blocked from the time uninstall probes ownership through deletion", async () => {
  const folder = homes["uninstall-first"];
  const installed = await added("uninstall-first");
  assert.equal(installed.state, "done");
  const probeHold = path.join(fake, "hold-uninstall-first");
  const deleteHold = path.join(folder, "hold-uninstall");
  const keyFile = path.join(folder, ".kuvo-code", "box", "keys", "KUVO_KEY_NOTION_TOKEN");
  write(path.join(fake, "home-uninstall-first-alias"), folder);
  write(traceFile, "");
  write(probeHold, "uname -sm");
  write(deleteHold, "");
  try {
    const begun = await ask("POST", `kuvo/computers/${installed.computer.key}/uninstall`);
    await waitFor(() => remoteCalls().includes("uninstall-first probe"));
    const beforeDelete = fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false });
    fs.rmSync(probeHold);
    await waitFor(() => fs.existsSync(path.join(folder, "deleting")));
    const addSecond = await added("uninstall-first-alias");
    assert.equal(addSecond.state, "failed");
    assert.equal(remoteCalls().includes("uninstall-first-alias probe"), false);
    assert.match(addSecond.failure.message, /change is in progress/);
    assert.ok(beforeDelete?.isSymbolicLink(), "uninstall must hold the reservation before its probe");
    assert.ok(fs.existsSync(keyFile));
    fs.rmSync(deleteHold);
    assert.equal((await over(begun.body.try.id)).state, "done");
    assert.equal(fs.existsSync(keyFile), false);
    assert.equal(fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false }), undefined);
  } finally {
    fs.rmSync(probeHold, { force: true });
    fs.rmSync(deleteHold, { force: true });
    fs.rmSync(traceFile);
  }
});

test("a reservation creation error says the account couldn't be prepared and logs the SSH detail", async () => {
  const folder = homes["prepare-error"];
  const refuse = path.join(folder, "refuse-reservation");
  write(refuse, "");
  const logged = [];
  const log = console.error;
  console.error = (...args) => logged.push(args.join(" "));
  try {
    const result = await added("prepare-error");
    assert.equal(result.state, "failed");
    assert.equal(result.failure.message, "Kuvo Code couldn’t prepare the OS account on prepare-error. Check that you can write to your home folder there, then try again.");
    assert.match(logged.join("\n"), /ln: test account can't create the reservation/);
    assert.equal(fs.lstatSync(reservationAt(folder), { throwIfNoEntry: false }), undefined);
  } finally {
    console.error = log;
    fs.rmSync(refuse);
  }
});

const macbookAlias = "macbook.lan";
const macbookAddress = destination => destination === "macbook" || destination === macbookAlias;
const macbookPeer = () => link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine" && macbookAddress(each.id));

async function holdAliasAdd(phone, during) {
  const host = path.join(fake, "hold");
  fs.writeFileSync(host, "kuvo kind");
  let id;
  try {
    const started = await ask("POST", "kuvo/computers/add", { destination: macbookAlias }, phone);
    assert.equal(started.status, 202, JSON.stringify(started.body));
    id = started.body.try.id;
    await until(id, found => found?.steps[0]?.state === "now");
    await during();
  } finally {
    fs.rmSync(host);
  }
  return over(id);
}

test("an alias Add honours only its original pairing and cannot restore a key removed during the Add", async () => {
  assert.equal((await ask("POST", "kuvo/machines", { destination: "macbook" })).status, 200);
  const original = macbookPeer();
  const otherMachines = read(path.join(home, "machines.json")).filter(each => !macbookAddress(each.destination));
  fs.appendFileSync(link.KNOWN_HOSTS, `${macbookAlias} ${fs.readFileSync(path.join(fake, "hostkey"), "utf8").trim()}\n`);
  fs.writeFileSync(path.join(fake, `home-${macbookAlias}`), homes.macbook);
  const key = link.publicOf(link.newKey());
  const phone = { key, pairing: "first" };
  link.updateLink(state => {
    link.pairPhone(state, { key: k(key), pairing: phone.pairing, name: "Add caller", from: "pair" });
    return state;
  });
  const revokedCaller = await holdAliasAdd(phone, async () => {
    assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: k(key) }] })).status, 200);
    link.updateLink(state => {
      link.pairPhone(state, { key: k(key), pairing: "second", name: "Add caller", from: "pair" });
      return state;
    });
  });
  const authorized = { key, pairing: "second" };
  const removedPeer = await holdAliasAdd(authorized, async () => {
    assert.equal((await ask("DELETE", `kuvo/computers/${original.key}`)).status, 200);
  });
  assert.deepEqual([revokedCaller.state, removedPeer.state], ["failed", "failed"]);
  assert.match(revokedCaller.failure.message, /device that started this add is no longer paired/);
  assert.match(removedPeer.failure.message, /was removed from your computers/);
  assert.equal(macbookPeer(), undefined);
  assert.deepEqual(read(path.join(home, "machines.json")), otherMachines);

  const fresh = await ask("POST", "kuvo/computers/add", { destination: macbookAlias }, authorized);
  const done = await over(fresh.body.try.id);
  assert.equal(done.state, "done", JSON.stringify(done.failure));
  assert.notEqual(done.computer.key, original.key, "a fresh Add must use a new key");
  assert.ok(link.readLink().removedHere.includes(original.key), "the old key stays revoked");
});

test("uninstall revokes the computer even when its desktop identity folder cleanup fails", async () => {
  // The preceding test leaves this box joined through the alias.
  const computer = macbookPeer();
  const appKey = link.publicOf(link.newKey());
  link.updateLink(state => {
    link.pairDevice(state, { key: k(appKey), name: "MacBook desktop" });
    return state;
  });
  const pairing = link.readLink().phones.find(each => each.key === k(appKey)).pairing;
  const kuvo = path.join(homes.macbook, ".kuvo-code");
  const identity = path.join(kuvo, "desktop", "identity");
  write(identity, "paired identity", 0o600);
  assert.equal(link.authorizePhone(appKey, {}), true);
  const rm = path.join(tools, "rm");
  write(rm, '#!/bin/sh\nfor arg do [ "$arg" != "$HOME/.kuvo-code/desktop" ] || exit 1; done\nexec /bin/rm "$@"\n', 0o755);
  let result;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    assert.equal(started.status, 202);
    result = await over(started.body.try.id);
  } finally {
    fs.rmSync(rm);
  }
  assert.equal(result.state, "failed");
  assert.equal(fs.readFileSync(identity, "utf8"), "paired identity");
  assert.deepEqual(pidsThere(kuvo).filter(alive), [], "the service stopped");
  assert.equal(macbookPeer(), undefined, "the computer membership was revoked");
  assert.equal(link.authorizePhone(appKey, {}), true, "the separate device grant is still valid");
  assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: k(appKey), pairing }] })).status, 200);
  assert.equal(link.authorizePhone(appKey, {}), false, "the device removal revokes the surviving identity");
  assert.ok(link.ownPhones(link.readLink()).removed.some(each => each.key === k(appKey) && each.pairing === pairing));
  assert.match(result.failure.message, /access to your other computers was revoked.*desktop identity folder/i);
});

test("uninstall logs a link-write failure without putting the local path in its result", async () => {
  const added = await ask("POST", "kuvo/machines", { destination: "macbook" });
  assert.equal(added.status, 200, JSON.stringify(added.body));
  const computer = macbookPeer();
  const rename = fs.renameSync;
  const linkFile = path.join(home, "run", "link.json");
  fs.renameSync = (from, to) => {
    if (to === linkFile) throw new Error(`Can't write ${linkFile}`);
    return rename(from, to);
  };
  let result;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    result = await over(started.body.try.id);
  } finally {
    fs.renameSync = rename;
  }
  assert.equal(result.state, "failed");
  assert.equal(result.failure.message, "Kuvo Code couldn’t confirm that access was revoked. Try uninstalling again.");
  assert.equal(macbookPeer().key, computer.key);
  assert.ok(read(path.join(home, "machines.json")).some(each => each.destination === "macbook"), "the saved address is still available for retry");
});

test("uninstall revokes access before SSH cleanup and reports a lost reply without its diagnostic", async () => {
  const added = await ask("POST", "kuvo/machines", { destination: "macbook" });
  assert.equal(added.status, 200, JSON.stringify(added.body));
  const computer = macbookPeer();
  const key = link.publicOf(link.newKey());
  link.updateLink(state => { link.pairDevice(state, { key: k(key), name: "MacBook desktop" }); return state; });
  const marker = path.join(fake, "lose-uninstall-reply");
  fs.writeFileSync(marker, "");
  let result;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    assert.equal(started.status, 202, JSON.stringify(started.body));
    result = await over(started.body.try.id);
  } finally {
    fs.rmSync(marker);
  }
  assert.deepEqual({
    state: result.state, message: result.failure.message,
    letsIn: link.authorizePhone(key, {}), present: Boolean(macbookPeer()),
  }, {
    state: "failed", message: "macbook’s access to your other computers was revoked. Kuvo Code couldn’t confirm whether cleanup finished on that computer. Run the local uninstall command (kuvo-link.js uninstall) on that computer to finish cleanup.",
    letsIn: true, present: false,
  });
  assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: k(key) }] })).status, 200);
  assert.deepEqual(pidsThere(path.join(homes.macbook, ".kuvo-code")).filter(alive), []);
});

for (const [destination, heldScript, step, expected] of [
  ["early-seed", "kuvo kind", "checking", ["probe"]],
  ["late-seed", 'bin/kuvo-start" start', "starting", ["probe", "start"]],
]) {
  test(`a revoked Add stops sending credentials after ${step}`, async () => {
    const folder = path.join(base, destination);
    homes[destination] = folder;
    fs.mkdirSync(folder);
    write(path.join(fake, `home-${destination}`), folder);
    for (const tool of ["pkill", "caffeinate"]) write(path.join(folder, ".kuvo-code", "bin", tool), "#!/bin/sh\nexit 0\n", 0o755);
    fs.appendFileSync(link.KNOWN_HOSTS, `${destination} ${fs.readFileSync(path.join(fake, "hostkey"), "utf8").trim()}\n`);
    const key = link.publicOf(link.newKey());
    link.updateLink(state => { link.pairPhone(state, { key: k(key), name: "Add caller", from: "pair", pairing: "caller" }); return state; });
    const track = path.join(fake, "track");
    const hold = path.join(fake, "hold");
    fs.writeFileSync(track, "");
    fs.writeFileSync(hold, heldScript);
    let id;
    try {
      const started = await ask("POST", "kuvo/computers/add", { destination, credentials: await checkedCredentials() }, { key, pairing: "caller" });
      assert.equal(started.status, 202, JSON.stringify(started.body));
      id = started.body.try.id;
      await until(id, found => found?.steps.find(each => each.id === step)?.state === "now");
      const scriptStarted = () => fs.readFileSync(track, "utf8").split("\n").includes(expected.at(-1));
      const deadline = Date.now() + 15_000;
      while (!scriptStarted() && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25));
      assert.ok(scriptStarted(), "the remote held script was reached before revoking the caller");
      assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: k(key) }] })).status, 200);
    } finally {
      fs.rmSync(hold);
    }
    const ended = await over(id);
    const ran = fs.readFileSync(track, "utf8").trim().split("\n");
    fs.rmSync(track);
    assert.equal(ended.state, "failed");
    assert.match(ended.failure.message, /device that started this add is no longer paired/);
    assert.deepEqual(ran, expected);
  });
}

test("SSH uninstall leaves membership intact when the remote pre-wipe backup fails", async () => {
  const installed = await added("backup-uninstall");
  assert.equal(installed.state, "done", JSON.stringify(installed.failure));
  const { computer } = installed;
  const kuvo = path.join(homes["backup-uninstall"], ".kuvo-code");
  const identity = path.join(kuvo, "desktop", "identity");
  write(identity, "keep until backup succeeds", 0o600);
  write(path.join(kuvo, "backup", "configuration.json"), '{"version":999}');
  const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
  assert.equal(started.status, 202, JSON.stringify(started.body));
  const ended = await over(started.body.try.id);
  assert.equal(ended.state, "failed");
  assert.match(ended.failure.message, /Unsupported backup configuration/);
  assert.ok(link.readLink().macPeers.some(peer => peer.key === computer.key));
  assert.equal(fs.readFileSync(identity, "utf8"), "keep until backup succeeds");
});

test("SSH uninstall stops if the refreshed peer list drops the target during the probe", async () => {
  const addedForProbe = await added("probe-removal");
  assert.equal(addedForProbe.state, "done", JSON.stringify(addedForProbe.failure));
  const computer = addedForProbe.computer;
  const kuvo = path.join(homes["probe-removal"], ".kuvo-code");
  const identity = path.join(kuvo, "desktop", "identity");
  write(identity, "still here", 0o600);
  const key = link.publicOf(link.newKey());
  link.updateLink(state => { link.pairDevice(state, { key: k(key), name: "MacBook desktop", by: computer.key }); return state; });
  const hold = path.join(fake, "hold");
  fs.writeFileSync(hold, "kuvo kind");
  let id;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    assert.equal(started.status, 202, JSON.stringify(started.body));
    id = started.body.try.id;
    await until(id, found => found?.steps[0]?.state === "now");
    const peers = link.readLink().macPeers.filter(peer => peer.key !== computer.key);
    assert.equal((await ask("PUT", "kuvo/link/peers", { peers })).status, 200);
  } finally {
    fs.rmSync(hold);
  }
  const ended = await over(id);
  assert.equal(ended.state, "failed");
  assert.match(ended.failure.message, /couldn’t confirm that access was revoked/);
  assert.equal(link.authorizePhone(key, {}), true, "mere absence isn't revocation");
  assert.equal(fs.readFileSync(identity, "utf8"), "still here");
  assert.equal(fs.existsSync(path.join(kuvo, "run", "off")), false);
});

test("Stop after committed revocation retains the unconfirmed SSH cleanup and local instructions", async () => {
  const addedForStop = await added("stop-uninstall");
  assert.equal(addedForStop.state, "done", JSON.stringify(addedForStop.failure));
  const computer = addedForStop.computer;
  const kuvo = path.join(homes["stop-uninstall"], ".kuvo-code");
  write(path.join(kuvo, "desktop", "identity"), "still here", 0o600);
  const marker = path.join(fake, "lose-uninstall-reply");
  const entered = path.join(fake, "uninstall-entered");
  const release = path.join(fake, "uninstall-continue");
  fs.writeFileSync(marker, "before");
  let id;
  try {
    const started = await ask("POST", `kuvo/computers/${computer.key}/uninstall`);
    assert.equal(started.status, 202, JSON.stringify(started.body));
    id = started.body.try.id;
    await until(id, found => found?.steps.find(step => step.id === "removing")?.state === "now");
    const deadline = Date.now() + 15_000;
    while (!fs.existsSync(entered) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25));
    assert.ok(fs.existsSync(entered), "the SSH command is outstanding");
    assert.equal((await ask("DELETE", `kuvo/computers/add/${id}`)).status, 200);
  } finally {
    fs.writeFileSync(release, "");
  }
  const ended = await over(id);
  fs.rmSync(marker);
  assert.equal(ended?.state, "failed");
  assert.match(ended.failure.message, /revoked.*couldn’t confirm.*Run the local uninstall command \(kuvo-link\.js uninstall\)/);
  assert.equal(link.macPeersBody(link.readLink()).boxes.some(peer => peer.key === computer.key), false);
  assert.equal(fs.readFileSync(path.join(kuvo, "desktop", "identity"), "utf8"), "still here");
  assert.equal(fs.existsSync(path.join(kuvo, "run", "off")), false);
});

function machineFixture(name, { legacy = true, workerReadyMs = 30_000 } = {}) {
  const folder = path.join(base, name);
  homes[name] = folder;
  const root = path.join(folder, ".kuvo-code");
  const run = path.join(root, "run");
  const plist = path.join(folder, "Library/LaunchAgents/run.kuvo.code.machine.plist");
  const engine = () => read(path.join(run, "engine.json"));
  const bridge = () => read(path.join(run, "bridge.json"));
  const helperWorkers = () => {
    const [, helper] = fs.readFileSync(path.join(run, "machine-keeper"), "utf8").trim().split(/\s+/).map(Number);
    const owned = (kind, pid) => fs.readFileSync(path.join(run, kind + ".launch"), "utf8").trim() === pid + " " + helper;
    return helper > 1 && bridge().helper === helper && owned("agents", bridge().pid) && owned("engine", engine().pid);
  };
  const journal = path.join(run, "machine-helper-job.json");
  const swap = path.join(run, "engine-swap.json");
  const adopted = path.join(run, "machine-helper-adopted");
  const until = async (check, message) => {
    for (let i = 0; i < 600; i++) {
      try { if (check()) return; } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    const facts = ["machine-handover.log", "engine-swap.log", "machine-helper-job.json", "fake-launchd", "fake-launchd.permissions", "box.pid", "machine-keeper", "engine.json", "bridge.json", "agents.log", "engine.log"];
    const diagnostics = facts.map(name => {
      const file = path.join(run, name);
      return fs.existsSync(file) ? `${name}: ${fs.readFileSync(file, "utf8").slice(-500)}` : `${name}: missing`;
    });
    assert.fail([message, ...diagnostics].join("\n"));
  };
  fs.cpSync(release, root, { recursive: true });
  write(path.join(root, "engine", engineVersion, "opencode"), engineBinary, 0o755);
  write(path.join(root, "box/kind"), "machine\n");
  write(path.join(root, "box/keys/KUVO_KEY_NOTION_TOKEN"), "retained");
  write(path.join(run, "launch.env"), `PATH=${process.env.PATH}\n`);
  if (legacy) {
    write(plist, '<string>exec sh "$HOME/.kuvo-code/bin/kuvo-start"</string>');
    write(path.join(run, "fake-launchd"), "legacy\n");
  }
  // Give real keeper/launcher processes a test deadline even under load. A deliberately
  // unhealthy fixture uses a shorter deadline to exercise rollback without waiting needlessly.
  const env = { ...process.env, HOME: folder, KUVO_HOME: root, KUVO_TEST_KEEP_ROUND: "1", KUVO_TEST_SWAP_EVERY: "100", KUVO_TEST_SWAP_QUIET: "700", KUVO_TEST_SWAP_START_MOST: String(workerReadyMs) };
  const start = path.join(root, "bin", "kuvo-start");
  const installHelper = () => fs.cpSync(path.join(home, helperBundle), path.join(root, helperBundle), { recursive: true });
  const nextEngine = () => {
    const next = "999.0.0";
    write(path.join(root, "engine", next, "opencode"), engineBinary, 0o755);
    write(start, fs.readFileSync(start, "utf8").replace(/^V=.*$/m, `V=${next}`), 0o700);
    write(path.join(root, "bin/.kuvo-boot"), "next");
    return next;
  };
  const stop = () => {
    spawnSync(path.join(tools, "launchctl"), ["bootout", "stub/run.kuvo.code.machine.permissions"], { env });
    spawnSync("sh", [start, "off"], { env, timeout: 40_000 });
  };
  return { folder, root, run, plist, engine, bridge, helperWorkers, journal, swap, adopted, until, env, start, installHelper, nextEngine, stop };
}

for (const reinstall of [false, true]) test(`a fresh Darwin installer rolls back when the bridge never answers${reinstall ? " during reinstall" : ""}`, {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, plist, bridge, helperWorkers, journal, adopted, until, env, installHelper, stop } = machineFixture(reinstall ? "installer-bridge-reinstall" : "installer-bridge", { legacy: false, workerReadyMs: 9_000 });
  installHelper();
  if (reinstall) write(adopted, "");
  write(path.join(folder, "fail-helper-bridge"), "");
  const installer = spawn(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], { env, stdio: "ignore" });
  const exited = new Promise(resolve => installer.once("exit", resolve));
  try {
    await until(() => helperWorkers(), "the installer should have started both workers under Helper");
    assert.equal((await fetch(`http://127.0.0.1:${bridge().port}/api/session/active`)).status, 503);
    assert.ok(fs.existsSync(journal), "the startup trial is still recoverable");
    assert.equal(fs.existsSync(adopted), false);
    assert.equal(await exited, 1);
    assert.equal(fs.existsSync(adopted), false);
    assert.equal(fs.existsSync(plist), false, "the failed launch job was removed");
  } finally {
    if (installer.exitCode === null && installer.signalCode === null) installer.kill();
    stop();
  }
});

test("a stopped Darwin machine starts through its existing launch job", {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, run, plist, bridge, adopted, env, installHelper, stop } = machineFixture("installer-stopped-legacy");
  installHelper();
  write(path.join(run, "stopped"), "");
  const installer = spawn(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], { env, stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  installer.stderr.on("data", chunk => { stderr += chunk; });
  const exited = new Promise(resolve => installer.once("close", resolve));
  try {
    assert.equal(await exited, 0, stderr);
    assert.match(fs.readFileSync(path.join(folder, "launchctl.log"), "utf8"), /^kickstart gui\/\d+\/run\.kuvo\.code\.machine$/m);
    assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
    assert.equal(bridge().helper, 0);
    assert.equal(fs.existsSync(adopted), false);
  } finally {
    if (installer.exitCode === null && installer.signalCode === null) installer.kill();
    stop();
  }
});

test("a stopped Darwin machine reports a refused legacy launch job", {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, run, plist, env, installHelper, stop } = machineFixture("installer-refused-legacy");
  installHelper();
  write(path.join(run, "stopped"), "");
  write(path.join(folder, "refuse-legacy"), "");
  const installer = spawn(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], { env, stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  installer.stderr.setEncoding("utf8").on("data", chunk => { stderr += chunk; });
  const closed = new Promise(resolve => installer.once("close", resolve));
  try {
    assert.equal(await closed, 1, "a detached keeper cannot turn the refusal into success");
    assert.match(stderr, /existing launch job couldn’t start.*legacy job refused/);
    assert.match(fs.readFileSync(path.join(folder, "launchctl.log"), "utf8"), /^kickstart gui\/\d+\/run\.kuvo\.code\.machine$/m);
    assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
    assert.equal(fs.existsSync(path.join(run, "box.pid")), false);
  } finally {
    if (installer.exitCode === null && installer.signalCode === null) installer.kill();
    stop();
  }
});

test("an adopted Darwin reinstall reports failure while the bridge returns 503", {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, plist, engine, bridge, helperWorkers, journal, adopted, env, installHelper, stop } = machineFixture("installer-already-adopted", { legacy: false });
  installHelper();
  const installer = (readyMs = env.KUVO_TEST_SWAP_START_MOST) => spawnSync(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], {
    env: { ...env, KUVO_TEST_SWAP_START_MOST: readyMs }, encoding: "utf8", timeout: 40_000,
  });
  try {
    const first = installer();
    assert.equal(first.status, 0, first.stderr);
    const registered = fs.readFileSync(plist, "utf8");
    assert.match(registered, /Kuvo Helper<\/string><string>keep-machine<\/string>/);
    assert.ok(fs.existsSync(adopted));
    assert.ok(helperWorkers(), "the clean install adopted both Helper-owned workers before the reinstall");
    const workers = [engine().pid, bridge().pid];

    write(path.join(folder, "fail-helper-bridge"), "");
    assert.equal((await fetch(`http://127.0.0.1:${bridge().port}/api/session/active`)).status, 503);
    const again = installer("9000");
    assert.equal(again.status, 1, again.stderr);
    assert.match(again.stderr, /didn’t finish starting/);
    assert.equal(fs.readFileSync(plist, "utf8"), registered);
    assert.ok(fs.existsSync(adopted), "the completed adoption stays in place");
    assert.equal(fs.existsSync(journal), false);
    assert.deepEqual([engine().pid, bridge().pid], workers);
  } finally {
    stop();
  }
});

test("a Darwin keeper finishes an interrupted fresh install when both services answer", {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, helperWorkers, journal, adopted, until, env, installHelper, stop } = machineFixture("installer-interrupted", { legacy: false });
  installHelper();
  write(path.join(folder, "fail-helper-bridge"), "");
  const installer = spawn(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], { env, stdio: "ignore" });
  const exited = new Promise(resolve => installer.once("exit", resolve));
  try {
    await until(() => read(journal).phase === "waiting" && helperWorkers(), "both Helper-owned workers start during the trial");
    assert.ok(installer.kill("SIGKILL"));
    await exited;
    fs.rmSync(path.join(folder, "fail-helper-bridge"));
    await until(() => fs.existsSync(adopted) && !fs.existsSync(journal), "the keeper verifies service and completes adoption");
  } finally {
    if (installer.exitCode === null && installer.signalCode === null) installer.kill();
    stop();
  }
});

for (const scenario of ["grant", "worker failure", "interruption", "bootstrap denied", "rollback holds", "deferred update", "deferred worker restart", "partial inbox"]) test(`legacy Darwin machine hand-over: ${scenario}`, {
  skip: process.platform !== "darwin",
}, async () => {
  const name = "legacy-" + scenario.replaceAll(" ", "-");
  const rollback = scenario === "rollback holds";
  const workerFailure = scenario === "worker failure";
  const { folder, run, plist, engine, bridge, journal, swap, adopted, until, env, start, installHelper, nextEngine, stop } = machineFixture(name, {
    workerReadyMs: rollback || workerFailure ? 9_000 : undefined,
  });
  const legacy = spawn("sh", [start, "keep"], { env, stdio: "ignore" });
  try {
    await until(() => bridge().pid && engine().pid, "legacy workers should start");
    const oldEngine = engine().pid, oldBridge = bridge().pid;
    const deferred = scenario.startsWith("deferred");
    if (deferred) write(path.join(folder, "files-access"), "false");
    else if (rollback) {
      write(path.join(folder, "files-access"), "true");
      write(path.join(folder, "refuse-helper"), "");
      write(path.join(folder, "fail-bridge-on-bootstrap"), "");
    } else write(path.join(folder, "busy"), "");
    installHelper();
    if (deferred) {
      await until(() => read(path.join(run, "permissions.json")).fullDiskAccess === false, "Helper should report a denial");
    } else if (!rollback) {
      await until(() => read(path.join(run, "permissions.json")).fullDiskAccess === null, "the signed watcher should publish while the old job is running");
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      write(path.join(folder, "files-access"), "false");
      await until(() => read(path.join(run, "permissions.json")).fullDiskAccess === false, "the watcher should report a denial");
    }

    if (scenario === "deferred worker restart") {
      write(path.join(folder, "crash-bridge"), "");
      await until(() => bridge().pid !== oldBridge, "the legacy job should restart the bridge");
      assert.equal(bridge().helper, 0, "the replacement bridge retains the old launch identity");
      assert.equal(engine().pid, oldEngine);
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      return;
    }
    if (scenario === "deferred update") {
      const next = nextEngine();
      await until(() => engine().version === next, "the ordinary swap should proceed while Files stays denied");
      assert.equal(fs.readFileSync(path.join(folder, "engine-helper"), "utf8"), "");
      assert.equal(bridge().pid, oldBridge);
      assert.equal(read(path.join(run, "permissions.json")).fullDiskAccess, false);
      assert.equal(fs.existsSync(adopted), false);
      return;
    }

    if (!rollback) {
      assert.equal(engine().pid, oldEngine);
      assert.equal(bridge().pid, oldBridge);
      for (const worker of [engine(), bridge()]) assert.ok((await fetch(`http://127.0.0.1:${worker.port}/api/session/active`)).ok);
      if (scenario === "bootstrap denied") write(path.join(folder, "refuse-helper"), "");
      if (workerFailure) write(path.join(folder, "fail-helper-bridge"), "");
      if (scenario === "interruption") write(path.join(folder, "pause-helper-bootstrap"), "");
      if (scenario === "partial inbox") write(path.join(folder, "engine-inbox.json"), JSON.stringify(["A", "B", "C"].map(text => ({
        id: `msg_${text}`, delivery: "queue", type: "user", payload: { text },
      }))));
      write(path.join(folder, "files-access"), "true");
      await until(() => read(swap).stage === "waiting", "even with a grant the hand-over should wait for work");
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      fs.rmSync(path.join(folder, "busy"));
    }

    if (scenario === "bootstrap denied" || scenario === "rollback holds") {
      if (scenario === "rollback holds") {
        await until(() => fs.existsSync(path.join(folder, "fail-legacy-bridge")) && read(journal).phase === "restoring", "the restored bridge is unavailable during rollback");
        const sent = path.join(folder, "delivered-prompts");
        const owner = read(swap).pid;
        write(path.join(run, "waiting/msg_held.json"), JSON.stringify({ id: "msg_held", text: "Keep this held", delivery: "queue" }));
        write(path.join(run, "queues.json"), JSON.stringify({ ses_held: { paused: false, held: [], waiting: [{ id: "msg_held", at: Date.now(), text: "Keep this held" }] } }));
        await until(() => spawnSync("ps", ["-p", String(owner)], { stdio: "ignore" }).status !== 0, "the first recovery finishes its readiness wait");
        assert.equal(fs.existsSync(sent), false, "a running engine on its own does not release messages");
        assert.equal(read(journal).phase, "restoring");
        assert.equal(read(swap).stage, "back");
        fs.rmSync(path.join(folder, "fail-legacy-bridge"));
        await until(() => fs.existsSync(sent) && !fs.existsSync(journal) && !fs.existsSync(swap), "verified service releases the held message");
        assert.equal(fs.readFileSync(sent, "utf8"), "Keep this held\n");
      }
      await until(() => fs.existsSync(path.join(run, "machine-helper-retry")) && !fs.existsSync(journal), "backoff after restoring the launch job");
      assert.equal(engine().pid, oldEngine);
      const next = nextEngine();
      await until(() => engine().version === next, "an ordinary engine swap can proceed during backoff");
      return;
    }

    if (scenario === "interruption") {
      await until(() => fs.existsSync(path.join(folder, "entered-helper-bootstrap")), "launch replacement should reach bootstrap");
      const backup = read(journal);
      assert.equal(backup.phase, "installing");
      assert.match(backup.plist, /exec sh/);
      assert.match(spawnSync("ps", ["-ww", "-o", "command=", "-p", String(backup.owner)], { encoding: "utf8" }).stdout, /kuvo-link\.js engine-swap --machine-helper/);
      process.kill(backup.owner, "SIGKILL");
      fs.rmSync(path.join(folder, "pause-helper-bootstrap"));
      await until(() => !fs.existsSync(journal) && !fs.existsSync(path.join(run, "machine-helper")), "keeper recovers the interrupted replacement");
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      assert.equal(engine().pid, oldEngine);
      assert.equal(bridge().pid, oldBridge);
      return;
    }

    if (workerFailure) {
      await until(() => !fs.existsSync(journal) && fs.existsSync(path.join(run, "machine-helper-retry")) && bridge().pid !== oldBridge, "the previous launch path restored working service");
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      assert.equal(fs.existsSync(adopted), false);
      assert.ok((await fetch(`http://127.0.0.1:${bridge().port}/api/session/active`)).ok);
      return;
    }
    await until(() => engine().pid !== oldEngine && bridge().pid !== oldBridge && fs.existsSync(adopted), "Helper should restart both workers and confirm adoption");
    const root = path.dirname(run);
    assert.equal(fs.readFileSync(path.join(root, "box/kind"), "utf8"), "machine\n");
    assert.equal(fs.readFileSync(path.join(root, "box/keys/KUVO_KEY_NOTION_TOKEN"), "utf8"), "retained");
    const [, helper] = fs.readFileSync(path.join(run, "machine-keeper"), "utf8").trim().split(/\s+/).map(Number);
    assert.equal(Number(fs.readFileSync(path.join(folder, "engine-helper"), "utf8")), helper);
    assert.equal(bridge().helper, helper);
    if (scenario === "partial inbox") {
      const delivered = path.join(folder, "delivered-prompts");
      await until(() => fs.readFileSync(delivered, "utf8").endsWith("C\n") && !fs.existsSync(swap), "the handover should finish delivering the inbox");
      assert.ok(fs.existsSync(path.join(folder, "inbox-delete-refused")), "one DELETE returned 503");
      assert.equal(fs.readFileSync(delivered, "utf8"), "A\nB\nC\n", "the handover sends each prompt only once");
    }
  } finally {
    stop();
    if (legacy.exitCode === null && legacy.signalCode === null) legacy.kill();
  }
});

// Every SSH installation has its own deadline below. This scenario does several in
// succession, so its outer deadline also covers the unlink and standby recovery checks.
test("failure bench: a Linux primary starts fresh, enrolls over SSH and pairs devices", { timeout: 900_000 }, async t => {
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
  const primaryBox = path.join(primaryHome, ".kuvo-code");
  const memberHome = homes["primary-member"];
  const memberBox = path.join(memberHome, ".kuvo-code");
  fs.cpSync(release, primaryBox, { recursive: true });
  // On macOS this also exercises the real Mac machine-install and attachment paths.
  if (process.platform === "darwin") fs.cpSync(path.join(home, helperBundle), path.join(primaryBox, helperBundle), { recursive: true });
  write(path.join(primaryBox, "engine", engineVersion, "opencode"), engineBinary, 0o755);
  const env = { ...process.env, HOME: primaryHome, KUVO_HOME: primaryBox, KUVO_TEST_RELAY: relay.url };
  const run = (boxEnv, ...args) => {
    const result = spawnSync(process.execPath, [script, ...args], { env: boxEnv, encoding: "utf8", timeout: 10_000 });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout;
  };
  const start = (boxEnv, ...args) => {
    const child = spawn(process.execPath, [script, ...args], { env: boxEnv, stdio: ["ignore", "ignore", "inherit"] });
    atEnd(() => new Promise(resolve => {
      if (child.exitCode !== null || child.signalCode !== null) return resolve();
      child.once("exit", resolve);
      child.kill("SIGTERM");
    }));
    return child;
  };
  const waitFor = async (description, find, timeout = 30_000) => {
    const until = Date.now() + timeout;
    do {
      const value = await find();
      if (value) return value;
      await new Promise(resolve => setTimeout(resolve, 100));
    } while (Date.now() < until);
    assert.fail(description);
  };
  const inBox = (box, file = "link.json") => read(path.join(box, "run", file));
  const localServer = async (box, boxEnv) => {
    write(path.join(box, "run", "serve.needed"), "");
    const child = start(boxEnv, "serve", "--kept");
    const record = await waitFor("the kept server starts", () => {
      try { const current = inBox(box, "serve.json"); return alive(current.pid) && current; } catch (error) { if (error.code !== "ENOENT") throw error; }
    });
    const ask = async (method, route, body) => {
      const answer = await fetch(`http://127.0.0.1:${record.port}/${route}`, {
        method, signal: AbortSignal.timeout(20_000),
        headers: { authorization: `Basic ${Buffer.from(`opencode:${record.password}`).toString("base64")}`, "x-kuvo-from": "mac" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      return { status: answer.status, body: await answer.json() };
    };
    return Object.assign(ask, {
      async stop() {
        if (record.pid === child.pid) {
          const exited = new Promise(resolve => child.once("exit", resolve));
          child.kill("SIGTERM");
          await exited;
        } else {
          // The SSH installation's own keeper can start the server first.
          const { stdout } = spawnSync("ps", ["-ww", "-o", "command=", "-p", String(record.pid)], { encoding: "utf8" });
          assert.ok(stdout.includes(path.join(box, "bin", "kuvo-link.js") + " serve --kept"), stdout);
          process.kill(record.pid, "SIGTERM");
        }
        await waitFor("the old server has removed its record", () => {
          try { return inBox(box, "serve.json").pid !== record.pid; } catch (error) { if (error.code === "ENOENT") return true; throw error; }
        });
      },
    });
  };

  assert.equal(fs.existsSync(path.join(primaryBox, "run", "link.json")), false);
  run(env, "setup-local");
  const first = inBox(primaryBox);
  const primary = first.setup;
  assert.equal(primary.role, "primary");
  assert.equal(primary.epoch, 1);
  assert.equal(primary.primary, k(link.publicOf(Buffer.from(first.key, "base64url"))));
  assert.equal(primary.sign, link.ownSign(first));
  assert.equal(first.relay, relay.url);
  run(env, "setup-local");
  assert.deepEqual(inBox(primaryBox), first, "a fresh primary keeps its identity on restart");
  const primaryApp = await localServer(primaryBox, env);
  const committed = async (answer, kind, checkReceipt) => {
    if (answer.status === 202) {
      const { setup, operation, proof, epoch, status, pending } = answer.body;
      assert.equal(status, "pending", JSON.stringify(answer.body));
      assert.equal(pending, true);
      const signed = link.readDoc(proof);
      assert.equal(signed?.body.operation, operation);
      assert.equal(signed.body.kind, kind);
      const outcome = await waitFor(`${kind} save ${operation} settles`, () => inBox(primaryBox).sharedOutcomes?.[operation]);
      assert.equal(outcome.status, "committed");
      const finished = await checkReceipt(answer.body);
      assert.equal(finished.status, 200, JSON.stringify(finished.body));
      assert.deepEqual(finished.body, { setup, operation, proof, epoch, status: "committed" });
      return;
    }
    assert.equal(answer.status, 200, JSON.stringify(answer.body));
  };
  const remove = async (key, body, wipe = false) => {
    const method = wipe ? "POST" : "DELETE";
    const route = `kuvo/computers/${key}${wipe ? "/unlink-and-wipe" : ""}`;
    await committed(await primaryApp(method, route, body), "membershipAndAccess", async save => {
      // Reusing the operation also finishes cleanup of the saved SSH destination.
      const { status, body: finished } = await primaryApp(method, route, { ...body, operation: save.operation });
      assert.equal(status, 200, JSON.stringify(finished));
      const { removal, ...receipt } = finished;
      assert.equal(removal.operation, save.operation);
      assert.equal(removal.accessRemoved, true);
      return { status, body: receipt };
    });
    assert.equal(inBox(primaryBox).pendingMachineCleanup?.[key], undefined);
  };
  assert.equal((await primaryApp("GET", "kuvo/computers/add")).body.can, true);

  const begun = await primaryApp("POST", "kuvo/computers/add", { destination: "primary-member" });
  assert.equal(begun.status, 202, JSON.stringify(begun.body));
  const route = `kuvo/computers/add/${begun.body.try.id}`;
  const tryNow = async () => {
    const listed = await primaryApp("GET", "kuvo/computers/add");
    assert.equal(listed.status, 200, JSON.stringify(listed.body));
    const attempt = listed.body.tries.find(each => each.id === begun.body.try.id);
    assert.ok(attempt);
    return attempt;
  };
  const question = await waitFor("SSH asks to trust the new host key", async () => {
    const attempt = await tryNow();
    assert.notEqual(attempt.state, "failed", JSON.stringify(attempt.failure));
    return attempt.question;
  });
  assert.equal(question.id, "hostKey");
  assert.equal((await primaryApp("POST", `${route}/answer`, { answer: "trust", at: question.at })).status, 200);
  const done = await waitFor("the SSH add finishes", async () => {
    const attempt = await tryNow();
    assert.notEqual(attempt.state, "failed", JSON.stringify(attempt.failure));
    return attempt.state === "done" && attempt;
  }, 60_000);
  const member = done.computer.key;
  assert.equal(done.save.status, "committed");
  assert.ok(inBox(primaryBox).shared.membershipAndAccess.value.boxes.some(peer => peer.key === member));
  const memberEnv = { ...env, HOME: memberHome, KUVO_HOME: memberBox };
  assert.deepEqual(inBox(memberBox).setup, { ...primary, role: "member" }, "SSH installed the signed setup");
  let memberApp = await localServer(memberBox, memberEnv);
  assert.equal((await memberApp("GET", "kuvo/computers/add")).body.can, false);
  assert.equal((await memberApp("POST", "kuvo/phones/pair")).body.primary, primary.primary);
  assert.deepEqual((await primaryApp("GET", "kuvo/setup")).body.computers.map(each => each.key), [primary.primary, member]);

  fs.copyFileSync(path.join(home, "run", "engine.json"), path.join(primaryBox, "run", "engine.json"));
  const originalPrimaryProcess = start(env, "run");
  const peers = await waitFor("the member installs the signed roster including its own entry", () => {
    const doc = link.readDoc(inBox(memberBox).docs[`${primary.sign} peers`]);
    return doc?.body.boxes.some(each => each.key === member) && doc;
  });
  assert.equal(peers.origin, primary.sign);
  assert.deepEqual(peers.body.setup, { id: primary.id, epoch: 1, primary: primary.primary });
  assert.deepEqual(peers.body.boxes.map(each => [each.key, each.kind]), [[primary.primary, "machine"], [member, "machine"]]);
  const forged = link.signDoc(link.newSigningSeed(), "peers", peers.n + 1, peers.body);
  assert.equal(link.takeDoc(inBox(memberBox), forged), null, "membership is trusted by key, not by the claimed primary");

  const desktop = { key: k(link.publicOf(link.newKey())), name: "New desktop", kind: "desktop" };
  const paired = await primaryApp("POST", "kuvo/devices/pair", desktop);
  assert.equal(paired.status, 200, JSON.stringify(paired.body));
  assert.ok(paired.body.pairing);
  await waitFor("the member takes the primary's signed pairing", () => {
    const state = inBox(memberBox);
    const phones = link.readDoc(state.docs[`${primary.sign} phones`]);
    return phones?.origin === primary.sign && phones.body.phones.some(each => each.key === desktop.key && each.pairing === paired.body.pairing)
      && state.phones.some(each => each.key === desktop.key && each.pairing === paired.body.pairing);
  });

  await t.test("a member forwards pairing to the primary", async () => {
    const device = { ...desktop, key: k(link.publicOf(link.newKey())), name: "Member desktop" };
    const made = await waitFor("the member can forward pairing once it hears from the primary", async () => {
      const answer = await memberApp("POST", "kuvo/devices/pair", device);
      assert.ok(answer.status === 200 || answer.status === 503, JSON.stringify(answer));
      return answer.status === 200 && answer.body;
    });
    assert.ok(made.pairing);
    const grant = inBox(primaryBox).phones.find(each => each.key === device.key);
    assert.equal(grant?.pairing, made.pairing);
    assert.equal(grant.by, undefined);
  });

  await t.test("a Mac member's desktop attaches to its existing box as a window", { skip: process.platform !== "darwin" }, async () => {
    const decision = JSON.parse(run({ ...memberEnv, KUVO_HOME: path.join(memberHome, "app", "box") }, "here", ".kuvo-code"));
    assert.equal(decision.use, "attach");
    assert.equal(decision.opens, "window");
    assert.equal(decision.box.primary.key, primary.primary);
    assert.equal((await memberApp("POST", "kuvo/setup/attached")).status, 200);
    assert.equal(inBox(memberBox, "setup-card.json").app, true);
    assert.equal((await memberApp("GET", "kuvo/setup")).body.you.role, "member");
  });
  const addOn = async (ask, destination, replace) => {
    const begun = await ask("POST", "kuvo/computers/add", { destination, ...(replace ? { replace } : {}), credentials: await checkedCredentials(ask) });
    assert.equal(begun.status, 202, JSON.stringify(begun.body));
    const id = begun.body.try.id;
    const current = async () => (await ask("GET", "kuvo/computers/add")).body.tries.find(attempt => attempt.id === id);
    const checked = await waitFor("SSH checks the host key", async () => {
      const attempt = await current();
      return (attempt.question || attempt.state !== "working" || attempt.steps[0].state === "done") && attempt;
    });
    if (checked.question) {
      assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "trust", at: checked.question.at })).status, 200);
    }
    return current;
  };
  const selected = await primaryApp("POST", "kuvo/setup/standby", { standby: member, epoch: 1 });
  assert.equal(selected.status, 202, JSON.stringify(selected.body));
  await waitFor("the primary has the standby's lease", () => {
    try { return inBox(primaryBox, "setup-lease.json").stamp === `${primary.id}:2:${member}`; } catch { return false; }
  });
  write(path.join(primaryBox, "box", "keys", "KUVO_KEY_NOTION_TOKEN"), "fixture-seeded-key");
  const oldAddress = link.addressOf(Buffer.from(first.secret, "base64url"));
  const standbyAddress = link.addressOf(Buffer.from(inBox(memberBox).secret, "base64url"));

  const deviceSave = await memberApp("POST", "kuvo/devices/pair", { ...desktop, key: k(link.publicOf(link.newKey())), name: "Standby desktop" });
  assert.equal(deviceSave.status, 200, JSON.stringify(deviceSave.body));
  assert.equal(deviceSave.body.status, "committed");
  assert.equal(link.readDoc(deviceSave.body.proof)?.body.operation, deviceSave.body.operation);
  let enrollmentReceipt;

  await t.test("a delayed membership receipt keeps the fresh SSH installation and the same proposal", async () => {
    const delayed = [];
    const release = relay.intercept(standbyAddress, oldAddress, packet => {
      const envelope = link.openMail(link.readFrame(packet.payload).body, inBox(primaryBox))?.payload?.docs?.give?.[0];
      if (link.readDoc(envelope)?.body.kind === "membershipAndAccess" && link.readDoc(envelope)?.body.receipt) {
        delayed.push(packet);
        return true;
      }
      return false;
    });
    try {
      const attempt = await addOn(primaryApp, "late-enrollment");
      await waitFor("the standby accepted enrollment and attempts to reply", async () => {
        const current = await attempt();
        assert.equal(current.state, "working", JSON.stringify(current));
        return delayed[0];
      }, 60_000);
      const operation = Object.keys(inBox(primaryBox).sharedPending)[0];
      const target = path.join(homes["late-enrollment"], ".kuvo-code");
      await new Promise(resolve => setTimeout(resolve, 3_300));
      const afterRestart = id => {
        const result = spawnSync(process.execPath, ["--input-type=module", "-e",
          `import { allTries } from ${JSON.stringify(script)}; console.log(JSON.stringify(allTries().find(each => each.id === ${JSON.stringify(id)})))`,
        ], { env, encoding: "utf8", timeout: 5_000 });
        assert.equal(result.status, 0, result.stderr);
        return JSON.parse(result.stdout);
      };
      const pending = await attempt();
      assert.equal(pending.state, "working", JSON.stringify(pending));
      assert.equal(pending.save?.status, "pending");
      assert.equal(pending.save.operation, operation);
      assert.equal(link.readDoc(pending.save.proof)?.body.operation, operation);
      const joined = inBox(target);
      assert.equal(joined.setup.epoch, 2);
      assert.equal(joined.leadership, undefined);
      assert.ok(link.digestOf(joined).some(entry => entry[1] === "leadership" && entry[2] === 2));
      const restarted = afterRestart(pending.id);
      assert.equal(restarted.state, "working");
      assert.deepEqual(restarted.save, pending.save);
      assert.deepEqual(Object.keys(inBox(primaryBox).sharedPending), [operation]);
      assert.equal(fs.readFileSync(path.join(target, "box", "keys", "KUVO_KEY_NOTION_TOKEN"), "utf8").trim(), "fixture-seeded-key");
      assert.ok(pidsThere(target).some(alive), "the installation is still running");
      assert.equal(fs.existsSync(path.join(target, "run", "off")), false);
      release();
      relay.replay(delayed[0]);
      const done = await waitFor("the original add commits", async () => {
        const current = await attempt();
        assert.notEqual(current.state, "failed", JSON.stringify(current.failure));
        return current.state === "done" && current;
      });
      assert.equal(inBox(primaryBox).sharedOutcomes[operation].status, "committed");
      assert.equal(afterRestart(done.id).state, "done");
      assert.equal(done.save?.status, "committed");
      assert.deepEqual(done.save.proof, pending.save.proof);
      enrollmentReceipt = done.save;
      for (const ask of [primaryApp, memberApp]) {
        await waitFor("the client's roster includes the new member", async () => {
          const roster = await ask("GET", "kuvo/computers");
          assert.equal(roster.status, 200);
          return roster.body.computers.some(each => each.key === done.computer.key);
        });
      }
      await remove(done.computer.key);
    } finally { release(); }
  });

  await t.test("unlink revokes both old keys and reinstall restores the computer’s list position", async () => {
    const firstID = "aaaaaaaa-abcd-4444-8888-aaaaaaaaaaaa";
    const otherID = "bbbbbbbb-abcd-4444-8888-bbbbbbbbbbbb";
    write(path.join(homes.rejoin, "hardware-id"), firstID);
    write(path.join(homes["rejoin-clone-name"], "hardware-id"), otherID);
    const add = async (name, replace, failure) => {
      const current = await addOn(primaryApp, name, replace);
      let last;
      const ended = await waitFor(`Add ${name} finishes`, async () => {
        last = await current();
        return (last.state === "done" || last.state === "failed") && last;
      }, 120_000).catch(error => assert.fail(`${error.message}: ${JSON.stringify(last)}`));
      if (failure) {
        assert.equal(ended.state, "failed", JSON.stringify(ended));
        assert.match(ended.failure.message, failure);
      } else assert.equal(ended.state, "done", JSON.stringify(ended.failure));
      return ended.computer;
    };
    const target = path.join(homes.rejoin, ".kuvo-code");
    const first = await add("rejoin");
    const neighbor = await add("rejoin-neighbor");
    const old = inBox(target);
    const oldPeer = { key: first.key, address: k(link.addressOf(Buffer.from(old.secret, "base64url"))), sign: link.ownSign(old) };
    const membership = box => inBox(box).shared.membershipAndAccess.value;
    const keys = async () => (await primaryApp("GET", "kuvo/setup")).body.computers.map(each => each.key);
    const before = await keys();
    assert.deepEqual(before.slice(-2), [first.key, neighbor.key]);
    const signed = link.signDoc(old.sign, "personalities", 50, { personalities: [], removed: [] });
    const targets = [primaryBox, memberBox].map(box => {
      const current = inBox(box);
      const to = { address: k(link.addressOf(Buffer.from(current.secret, "base64url"))), key: k(link.publicOf(Buffer.from(current.key, "base64url"))) };
      const frame = link.mailFor(to, { docs: { give: [signed] } }, old);
      const { body } = link.readFrame(frame.subarray(17));
      assert.equal(link.openMail(body, current)?.sender.key, first.key);
      return { box, body };
    });
    await remove(first.key, { wipe: false });
    for (const { box, body } of targets) {
      assert.ok(membership(box).left.includes(first.key));
      const current = inBox(box);
      assert.equal(link.openMail(body, current), null, "old Noise key");
      assert.equal(link.takeDoc(current, signed), null, "old document signer");
    }

    await waitFor("the removed member finishes unlinking", () => inBox(target).setup?.role === "primary");
    assert.equal(fs.existsSync(path.join(target, "run", ".ssh-fresh-add")), false);
    const stale = path.join(fake, "stale-join-rejoin");
    try {
      write(stale, JSON.stringify(oldPeer));
      await add("rejoin", undefined, /presented membership keys that cannot be used/);
      write(stale, JSON.stringify({ ...oldPeer, key: k(link.publicOf(link.newKey())) }));
      await add("rejoin", undefined, /presented membership keys that cannot be used/);
      assert.deepEqual((await keys()).slice(-1), [neighbor.key]);
    } finally { fs.rmSync(stale, { force: true }); }
    const returned = await add("rejoin");
    const rejoined = inBox(target);
    assert.notEqual(returned.key, first.key);
    assert.notEqual(link.ownSign(rejoined), oldPeer.sign);
    assert.notEqual(rejoined.secret, old.secret);
    assert.equal(inBox(target).setup.id, primary.id);
    assert.deepEqual((await keys()).slice(-2), [returned.key, neighbor.key]);

    // Put the same host name in front of genuinely different hardware.
    write(path.join(fake, "home-rejoin"), homes["rejoin-clone-name"]);
    const distinct = await add("rejoin");
    assert.equal(distinct.name, returned.name);
    assert.notEqual(distinct.key, returned.key);
    assert.deepEqual((await keys()).slice(-3), [returned.key, neighbor.key, distinct.key]);

    // Lose the live completion mail. The next SSH Add can recover the signed receipt
    // from outside the deleted installation and still replace both revoked keys.
    const returnedKey = Buffer.from(returned.key, "base64url");
    const primaryAddress = link.addressOf(Buffer.from(inBox(primaryBox).secret, "base64url"));
    const prefix = Buffer.from("kuvo-removal-v1\0");
    const operation = crypto.randomBytes(16).toString("base64url");
    const releaseDone = relay.intercept(null, primaryAddress, ({ payload }) => {
      const frame = link.readFrame(payload);
      if (frame?.kind !== link.Frame.mail || !frame.body.subarray(0, prefix.length).equals(prefix)) return false;
      const body = frame.body.subarray(prefix.length);
      if (!body.subarray(0, returnedKey.length).equals(returnedKey)) return false;
      const message = link.openOneWay(body.subarray(returnedKey.length), Buffer.from(inBox(primaryBox).key, "base64url"), returnedKey);
      return JSON.parse(message.payload).done === operation;
    });
    let installed;
    try {
      await remove(returned.key, { operation }, true);
      const proof = path.join(homes.rejoin, ".kuvo-code-removal-receipts", operation + ".ack");
      await waitFor("the old installation is erased and its receipt is recoverable", () => !fs.existsSync(target) && fs.existsSync(proof));
      assert.equal(membership(primaryBox).removals[returned.key].delivered, undefined);
      write(path.join(fake, "home-rejoin-installed"), homes.rejoin);
      installed = await add("rejoin-installed");
      assert.equal(membership(primaryBox).removals[returned.key].delivered, true);
    } finally { releaseDone(); }
    assert.notEqual(installed.key, returned.key);
    assert.equal(installed.name, "rejoin-installed");
    assert.deepEqual((await keys()).slice(-3), [installed.key, neighbor.key, distinct.key]);
    const kept = membership(primaryBox);
    assert.ok(kept.left.includes(first.key) && kept.left.includes(returned.key));
    assert.ok(kept.computerHistory.slots.some(slot => slot.key === installed.key && slot.previous.join() === [first.key, returned.key].join()));
    assert.equal(JSON.stringify(kept).includes(firstID.replaceAll("-", "")), false);
    assert.deepEqual(membership(memberBox).computerHistory, kept.computerHistory);

    // Neither installation has a usable platform ID. A deliberate choice still
    // keeps the selected computer's place and records its previous membership.
    const chosen = await add("rejoin-neighbor-return", neighbor.key);
    assert.notEqual(chosen.key, neighbor.key);
    assert.deepEqual((await keys()).slice(-3), [installed.key, chosen.key, distinct.key]);
    assert.ok(membership(primaryBox).computerHistory.slots.some(slot => slot.key === chosen.key && slot.previous.includes(neighbor.key)));
    for (const { key } of [installed, chosen, distinct]) {
      await remove(key);
    }

    if (process.platform === "linux") {
      const image = "cccccccc-abcd-4444-8888-cccccccccccc";
      write(path.join(homes["rejoin-image"], "os-image-id"), image);
      write(path.join(homes["rejoin-image-copy"], "os-image-id"), image);
      const original = await add("rejoin-image");
      await add("rejoin-image-copy", undefined, /Confirm which computer to replace/);
      assert.ok((await keys()).includes(original.key));
      const replacement = await add("rejoin-image-copy", original.key);
      assert.notEqual(replacement.key, original.key);
      assert.equal(membership(primaryBox).computerHistory.slots.find(slot => slot.key === replacement.key)?.fingerprintSource, "os-id");
      assert.ok(membership(primaryBox).left.includes(original.key));
      await remove(replacement.key);
    }
  });

  await t.test("after losing the primary, the surviving box enrolls and activates a replacement standby", async () => {
    const made = await primaryApp("POST", "kuvo/personalities", { name: "Survivor", description: "Save this across recovery." });
    await committed(made, "setupPersonalities", async save => {
      const { status, receipt } = await link.askSetupOutcome(path.join(primaryBox, "run"), save, "setupPersonalities");
      return { status, body: receipt };
    });
    assert.ok(inBox(memberBox).shared.setupPersonalities.value.personalities.some(personality => personality.name === "Survivor"));
    const saved = await memberApp("GET", "kuvo/setup");
    assert.equal(saved.body.standby, member);
    const clock = offset => {
      write(path.join(memberBox, "run", "test-setup-clock.json.new"), JSON.stringify({ offset }));
      fs.renameSync(path.join(memberBox, "run", "test-setup-clock.json.new"), path.join(memberBox, "run", "test-setup-clock.json"));
    };
    fs.writeFileSync(path.join(memberBox, "setup-takeover.json"), JSON.stringify({ alwaysOn: true, delayMinutes: 60 }));
    clock(0);
    const absent = relay.disconnect(oldAddress);
    atEnd(absent);
    originalPrimaryProcess.kill("SIGTERM");
    await new Promise(resolve => setTimeout(resolve, 2_300));
    clock(61 * 60_000);
    await waitFor("the standby promotes with no replacement", () => {
      const setup = inBox(memberBox).setup;
      return setup.role === "primary" && setup.epoch === 3 && !setup.standby;
    });
    for (const save of [deviceSave.body, enrollmentReceipt]) {
      const { setup: id, operation, proof } = save;
      const resolved = await link.askSetupOutcome(path.join(memberBox, "run"), { setup: id, operation, proof });
      assert.equal(resolved.status, 200);
      assert.equal(resolved.receipt.status, "committed");
    }
    const before = inBox(memberBox).shared;
    const target = path.join(homes["replacement-standby"], ".kuvo-code");
    const replies = [];
    const release = relay.intercept(null, standbyAddress, packet => {
      const file = path.join(target, "run", "link.json");
      if (!fs.existsSync(file)) return false;
      const address = link.addressOf(Buffer.from(read(file).secret, "base64url"));
      if (!address.equals(packet.from)) return false;
      replies.push(packet);
      return true;
    });
    atEnd(release);
    const attempt = await addOn((...args) => memberApp(...args), "replacement-standby");
    await waitFor("the replacement installs its certificate while replies are held", async () => {
      const current = await attempt();
      assert.notEqual(current.state, "failed", JSON.stringify(current.failure));
      const file = path.join(target, "run", "link.json");
      return replies.length && fs.existsSync(file) && read(file).leadership?.[4];
    }, 120_000);
    assert.ok(inBox(memberBox).leadershipPending);
    await memberApp.stop();
    memberApp = await localServer(memberBox, memberEnv);
    assert.equal((await attempt()).state, "working", "the restarted server keeps the save in flight");
    release();
    replies.forEach(relay.replay);
    const done = await waitFor("recovery adds a new standby over SSH", async () => {
      const current = await attempt();
      assert.notEqual(current.state, "failed", JSON.stringify(current.failure));
      return current.state === "done" && current;
    });
    const standby = inBox(target);
    const recovered = inBox(memberBox);
    assert.equal(recovered.setup.epoch, 4);
    assert.equal(recovered.setup.standby.key, done.computer.key);
    assert.deepEqual(standby.setup.standby, recovered.setup.standby);
    assert.equal(link.readDoc(recovered.leadership[4]).origin, recovered.setup.standby.sign);
    assert.deepEqual(standby.shared.setupPersonalities, before.setupPersonalities);
    assert.deepEqual(standby.shared.membershipAndAccess, recovered.shared.membershipAndAccess);
    assert.ok(standby.shared.queueNumberAllocation.value.next >= before.queueNumberAllocation.value.next);
  });
});
