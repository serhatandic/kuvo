// Adding a computer over SSH from a box (PROTOCOL.md, "Computers", Adding one; #636): a client
// only gives the address and answers what the box asks; the box checks the host's key, puts its
// own files and engine there, starts the computer's own keeper, and signs it into your setup. The
// real box code as a Mac's box, the real keeper, launcher and link on the computer added, in a home
// of the test's own. Stand-ins only for what's outside: `ssh` (a script that runs what it's given
// in that home, with a host key and refusals of its own), the engine's binary, the agents' bridge,
// and `crontab`/`launchctl`.
//
import { addSshStandIn } from "./add-ssh-stand-in.js";
import { removeTestHomeAtExit } from "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { atEnd } from "./at-end.js";
import { NO_BUN, testBun } from "./bun-for-tests.js";

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
const bun = testBun();
assert.ok(bun, NO_BUN);
const engineBinary = `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo 9.9.9; exit 0 ;;
-e) BUN_BE_BUN=1 exec true ;;
*.js)
  case "$2:$3" in
    browser:*) exit 0 ;;
    version-layout:*)
      # Studio's real layout conversion uses Bun's atomic filesystem exchange.
      if [ "$HOME" = '${there}' ]; then BUN_BE_BUN=1 exec '${bun}' --config=/dev/null --no-env-file "$@"; fi ;;
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
    // even there, and pass the stand-ins to the workers the keeper starts later.
    const keeper = source.toString();
    assert.match(keeper, /^  ctl=\/bin\/launchctl$/m);
    source = keeper.replace(/^  ctl=\/bin\/launchctl$/m, '  ctl="$KUVO_TEST_UNINSTALL_TOOLS/launchctl"')
      .replace(/^#![^\n]+\n/, `$&export KUVO_TEST_UNINSTALL_TOOLS='${tools}'\nexport KUVO_TEST_SSH='${path.join(fake, "ssh")}' FAKE_SSH='${fake}' FAKE_SSH_HOME='${there}'\n`);
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
write(path.join(fake, "ssh"), addSshStandIn, 0o755);

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

async function checkedCredentials(request = ask) {
  const result = await request("GET", "kuvo/computers/add/credentials");
  assert.equal(result.status, 200, JSON.stringify(result.body));
  return result.body.items.filter(item => item.checked).map(item => item.id);
}
// What a client follows (`kuvo/computers/add/events`), until the try `id` is as `wanted` says: its
// line then. (Gone from the list: `null` is what `wanted` gets.)
const until = async (id, wanted) => {
  const stop = new AbortController();
  const deadline = AbortSignal.any([stop.signal, AbortSignal.timeout(90_000)]);
  let held = "";
  let last = null;
  try {
    for await (const piece of handle({ method: "GET", path: "kuvo/computers/add/events", headers: {}, fromApp: true }, undefined, deadline)) {
      if (!piece.body) continue;
      held += piece.body.toString();
      const lines = held.split("\n");
      held = lines.pop();
      for (const line of lines) {
        const found = last = JSON.parse(line).tries.find(each => each.id === id) ?? null;
        if (wanted(found)) return found;
        // (Never waited on for good: one that failed, or asks what the test didn't mean to answer.)
        if (found?.state === "failed" || found?.state === "asking") throw new Error(`The try stands at ${found.state}: ${JSON.stringify(found.failure ?? found.question)}`);
      }
    }
  } finally {
    stop.abort();
  }
  throw new Error(`The try did not reach the expected state: ${JSON.stringify(last)}`);
};
// (Until it's over; one that asks something instead is handed back as it is, for the test to
// fail on rather than wait for an answer no one gives.)
const over = id => until(id, found => !found || found.state === "done" || found.state === "failed" || found.state === "asking");
const steps = found => found.steps.map(step => `${step.id}:${step.state}`);
const sshLog = () => (fs.existsSync(path.join(fake, "log")) ? fs.readFileSync(path.join(fake, "log"), "utf8").trim().split("\n") : []);
// (Two more computers, each a home of its own: a Mac that has Kuvo Code's app, and one with nothing.)
const homes = Object.fromEntries(["macbook", "mini", "nas", "attic", "race", "claim", "interrupted", "network-drop", "release", "add-first", "uninstall-first", "prepare", "old-reservation", "prepare-error", "probe-removal", "stop-uninstall", "primary-member", "backup-uninstall", "rejoin", "rejoin-neighbor", "rejoin-neighbor-return", "rejoin-clone-name", "rejoin-image", "rejoin-image-copy"].map(name => [name, path.join(base, name)]));
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

// Adds `destination`, trusting its key: the try as it ended (or as `wanted` says).
const added = async (destination, wanted) => {
  const begun = await ask("POST", "kuvo/computers/add", { destination, credentials: await checkedCredentials() });
  const asking = await until(begun.body.try.id, found => found?.state === "asking");
  assert.equal(asking.question.id, "hostKey");
  await ask("POST", `kuvo/computers/add/${begun.body.try.id}/answer`, { answer: "trust", at: asking.question.at });
  return wanted ? until(begun.body.try.id, wanted) : over(begun.body.try.id);
};

export {
  added, alive, ask, base, checkedCredentials, command, crontabFile, engineBinary, engineVersion,
  fake, handle, helperBundle, home, homes, hostKey, k, keyOne, keyTwo, link, mac, over, pidsThere,
  primaryHome, read, release, remote, script, secrets, sha256, sshLog, steps, there, tools, until,
  write,
};
