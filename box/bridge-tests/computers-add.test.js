// Adding a computer over SSH from a box (PROTOCOL.md, "Computers", Adding one; #636): a client
// only gives the address and answers what the box asks; the box checks the host's key, puts its
// own files and engine there, starts the computer's own keeper, and signs it into your setup. The
// real box code as a Mac's box, the real keeper, launcher and link on the computer added, in a home
// of the test's own. Stand-ins only for what's outside: `ssh` (a script that runs what it's given
// in that home, with a host key and refusals of its own), the engine's binary, the agents' bridge,
// and `crontab`/`launchctl`.
//
// Run with `node --test box/bridge-tests/computers-add.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

// (Short: a shared connection's socket goes under the box's folder on Linux.)
const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-add-")));
removeAtEnd(base);
const home = path.join(base, "here", ".kuvo-code");
const there = path.join(base, "there");
const remote = path.join(there, ".kuvo-code");
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
const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
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
for (const tool of ["pkill", "caffeinate", "launchctl", "open"]) write(path.join(tools, tool), `#!/bin/sh\necho "${tool} $*" >>'${path.join(fake, "reached")}'\n`, 0o755);

// The engine: it says it's listening, as the launcher waits to read, and goes when its home does.
const fakeEngine = path.join(fake, "engine.mjs");
fs.writeFileSync(fakeEngine, `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
// (Its sign-ins, kept as an engine keeps them: listed, and stored as they're given.)
const kept = ${JSON.stringify(path.join(there, "engine-credentials.json"))};
const credentials = () => { try { return JSON.parse(fs.readFileSync(kept, "utf8")); } catch { return []; } };
const server = http.createServer((request, response) => {
  const chunks = [];
  request.on("data", chunk => chunks.push(chunk));
  request.on("end", () => {
    response.writeHead(200, { "content-type": "application/json" });
    if (request.url === "/api/credential" && request.method === "POST") {
      const given = JSON.parse(Buffer.concat(chunks).toString());
      fs.writeFileSync(kept, JSON.stringify([...credentials(), { id: "cred_" + credentials().length, integrationID: given.integrationID, label: given.label, value: given.value, active: given.activate }]), { mode: 0o600 });
      return response.end("{}");
    }
    response.end(JSON.stringify({ data: request.url === "/api/credential" ? credentials() : request.url.startsWith("/api/session/active") ? {} : [] }));
  });
});
server.listen(port, "127.0.0.1", () => console.log("server listening on http://127.0.0.1:" + port));
setInterval(() => { if (!fs.existsSync(${JSON.stringify(there)})) process.exit(0); }, 300);
`);
// Its binary, the same on both computers (they're the same kind: this box gives its own): its
// version, the link's script run as the engine's Bun runs it (here this Node), and the engine.
const engineBinary = `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo 9.9.9; exit 0 ;;
-e) BUN_BE_BUN=1 exec true ;;
*.js) case "$2" in browser) exit 0 ;; esac; BUN_BE_BUN=1 exec '${process.execPath}' "$@" ;;
serve) trap 'kill "$c" 2>/dev/null' TERM INT HUP; BUN_BE_BUN=1 '${process.execPath}' '${fakeEngine}' "$@" & c=$!; wait "$c" ;;
esac
`;

// This box's own release files, in its folder, as an installed box has them: this checkout's own
// keeper, launcher, tools and link as built, with a bridge that only says where it answers.
const root = path.join(base, "sources");
for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/src/shared/engine-pin.json", "box/dist/kuvo-browser.js"]) {
  write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
}
write(path.join(root, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"), fs.readFileSync(script));
write(path.join(root, "box/dist/kuvo-agents.js"), `import fs from "node:fs";\nconsole.log(JSON.stringify({ url: "http://127.0.0.1:" + process.env.KUVO_AGENTS_PORT }));\nsetInterval(() => { if (!fs.existsSync(${JSON.stringify(there)})) process.exit(0); }, 300);\n`);
const release = path.join(base, "release");
const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), "--files", "1.0.0", release], {
  env: { ...process.env, BUN: process.env.BUN || process.env.KUVO_TEST_BUN || (process.versions.bun ? process.execPath : "bun"), BOX_SOURCES: root }, encoding: "utf8",
});
assert.equal(packed.status, 0, packed.stderr);
fs.cpSync(release, home, { recursive: true });
fs.rmSync(path.join(home, "box.json"));
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
strict=; known=; pubkey=yes; control=
while [ $# -gt 0 ]; do
  case "$1" in
    -o) case "$2" in
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
echo "$dest $strict \${control:-run}" >>"$F/log"
[ -n "$control" ] && exit 0
case "$dest" in nowhere) echo "ssh: Could not resolve hostname nowhere: Name or service not known" >&2; exit 255 ;; esac
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
if [ -f "$F/hold" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in *"$(cat "$F/hold")"*) while [ -f "$F/hold" ]; do sleep 0.05; done ;; esac
fi
HOME="$FAKE_SSH_HOME" exec sh -c "$cmd"
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

const link = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const k = key => key.toString("base64url");
const mac = link.newKey();
link.macLink.overlay = { key: k(mac), secret: k(crypto.randomBytes(32)), sign: link.newSigningSeed(), relay: "ws://127.0.0.1:1", name: "Studio" };
link.writeLink({ phones: [] });
link.macLink.config = { phones: true, machines: false, relay: "ws://127.0.0.1:1", name: "Studio", app: "1.0.0" };

const handle = link.engineHandler(() => ({ port: 1, password: "pw" }), { name: "Studio" });
const ask = async (method, route, body) => {
  const pieces = [];
  for await (const piece of handle({ method, path: route, headers: {}, fromApp: true, ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) }, undefined)) pieces.push(piece);
  return { status: pieces.find(each => each.head).head.status, body: JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString() || "{}") };
};
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
const pidsThere = () => {
  const pids = [];
  try { pids.push(read(path.join(remote, "run", "engine.json")).pid); } catch {}
  for (const file of ["box.pid", "link.pid", "agents.pid"]) try { pids.push(Number(fs.readFileSync(path.join(remote, "run", file), "utf8"))); } catch {}
  return pids.filter(pid => Number.isInteger(pid) && pid > 1);
};

atEnd(async () => {
  await link.forgetTries();
  fs.rmSync(path.join(fake, "hold"), { force: true });
  // Off (as a box removed), by its own keeper, then only what's still the test's own by its command.
  const pids = pidsThere();
  if (fs.existsSync(path.join(remote, "bin", "kuvo-start"))) spawnSync("sh", [path.join(remote, "bin", "kuvo-start"), "off"], { env: { ...process.env, HOME: there }, timeout: 120_000 });
  for (const pid of pids) if (alive(pid) && command(pid).includes(base)) try { process.kill(pid); } catch {}
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
  const begun = await ask("POST", "kuvo/computers/add", { destination: "studio" });
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
  assert.equal((await ask("POST", `kuvo/computers/add/${id}/answer`, { answer: "sure" })).status, 409, "only one of its answers");

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
  assert.match(fs.readFileSync(crontabFile, "utf8"), /^@reboot sh "\$HOME\/\.kuvo-code\/bin\/kuvo-start" >\/dev\/null 2>&1 # kuvo-code: starts its engine again after a restart$/m);
  if (process.platform === "darwin") assert.ok(fs.existsSync(path.join(there, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), "a Mac's launch agent");

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
  // One connection shared by its scripts, and let go of at the end.
  // (Where a socket's path fits: a Mac's short folder in /tmp; on Linux the box's own `run/ssh`,
  // which a test's deep home can be too long for, and then each script connects by itself.)
  const shares = process.platform === "darwin" || Buffer.byteLength(path.join(home, "run", "ssh")) <= 55;
  assert.equal(sshLog().at(-1), shares ? "studio  exit" : "studio yes run");
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
  assert.equal(nowhere.failure.message, "Couldn’t reach nowhere. ssh: Could not resolve hostname nowhere: Name or service not known.");
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
    await ask("POST", `kuvo/computers/add/${begun.body.try.id}/answer`, { answer: "trust" });
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
    await ask("POST", `kuvo/computers/add/${begun.body.try.id}/answer`, { answer: "cancel" });
    assert.equal(await over(begun.body.try.id), null, "stopped, and gone from the list");
    assert.equal(fs.readFileSync(link.KNOWN_HOSTS, "utf8"), before);
    // Trust New Key: the new key takes the old one's line (its other address's stays), and it goes on.
    const again = await ask("POST", `kuvo/computers/${peer.key}/setup`);
    await until(again.body.try.id, found => found?.state === "asking");
    await ask("POST", `kuvo/computers/add/${again.body.try.id}/answer`, { answer: "trust" });
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

test("uninstalling while it's being set up waits for the start under way, then nothing runs there and nothing starts it again (#603)", async () => {
  const peer = link.macPeersBody(link.readLink()).boxes.find(each => each.kind === "machine");
  const engine = read(path.join(remote, "run", "engine.json")).pid;
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
});

test("a member of a setup, and a request that can't stay, are told where to add from", async () => {
  // One request and gone (`kuvo-link.js box route`): nothing could stay to do it.
  const pieces = [];
  for await (const piece of handle({ method: "POST", path: "kuvo/computers/add", headers: {}, fromMac: true, body: Buffer.from(JSON.stringify({ destination: "gpu" })) }, undefined)) pieces.push(piece);
  assert.equal(pieces.find(each => each.head).head.status, 409);
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
