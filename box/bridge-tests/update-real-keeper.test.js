// A box taking a release whose keeper is the real one (PROTOCOL.md, "Updates"): the pack
// scripts/box-release.sh makes, with `kuvo-start`, `kuvo-launch` and `kuvo-setsid` as
// box/src/keeper has them, installed by the real link and then run: the new keeper starts the new
// engine, notes it, starts the agents' bridge, and stops and starts them again when asked. Only the
// engine's binary is a stand-in (it answers as one, and runs the link's script as the engine's Bun
// does), and so are the bridge and the old version's keeper. In a home of the test's own
// (`$HOME/.kuvo-code`, as the real keeper names its folder), with a keeper already noted as
// running there, as on a box that updates itself, so the real one starts no second keeper.
// Run with `node --test box/bridge-tests/update-real-keeper.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const base = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-real-keeper-"));
const home = path.join(base, ".kuvo-code");
process.env.HOME = base;
process.env.KUVO_HOME = home;
const link = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-real-keeper-release-"));
removeAtEnd(base);
removeAtEnd(scratch);

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
}
const sha256 = data => crypto.createHash("sha256").update(data).digest("hex");
const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const engineFile = path.join(home, "run", "engine.json");
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const command = pid => spawnSync("ps", ["-ww", "-o", "command=", "-p", String(pid)], { encoding: "utf8" }).stdout.trim();
// The coding agents' bridge, a stand-in: it answers where its keeper said, notes itself as the real
// one does (run/bridge.json), and says where it answers, as the launcher waits to read.
const bridgeStandIn = `import fs from "node:fs"; import http from "node:http"; import path from "node:path";
const port = Number(process.env.KUVO_AGENTS_PORT), password = process.env.KUVO_AGENTS_PASSWORD;
http.createServer((request, response) => { response.writeHead(200, { "content-type": "application/json" }); response.end(JSON.stringify({ pid: process.pid })); }).listen(port, "127.0.0.1", () => {
  fs.writeFileSync(path.join(process.env.KUVO_HOME, "run", "bridge.json"), JSON.stringify({ pid: process.pid, port, password }));
  console.log(JSON.stringify({ url: "http://127.0.0.1:" + port }));
});
setInterval(() => { if (!fs.existsSync(process.env.KUVO_HOME)) process.exit(0); }, 300);
`;
const realKeeper = (...args) => spawnSync("sh", [path.join(home, "bin", "kuvo-start"), ...args], { env: process.env, encoding: "utf8", timeout: 120_000 });

// The engine, as the real launcher starts it (`opencode serve --port <port> --hostname 127.0.0.1`):
// it says it's listening, as the launcher waits to read, and answers as one does.
const fakeEngine = path.join(scratch, "engine.mjs");
fs.writeFileSync(fakeEngine, `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
const server = http.createServer((request, response) => {
  response.writeHead(200, { "content-type": "application/json" });
  response.end(JSON.stringify({ data: request.url.startsWith("/api/session/active") ? {} : [] }));
});
server.listen(port, "127.0.0.1", () => console.log("server listening on http://127.0.0.1:" + port));
setInterval(() => { if (!fs.existsSync(${JSON.stringify(home)})) process.exit(0); }, 300);
`);
// What only the keeper's own loop does (`kuvo-start keep`: ending an older Mac's watchdogs by
// pattern, keeping a Mac awake, setting up the box's browser), which this test never means to
// reach: stand-ins first on the PATH, in the box's own `bin` (where the launcher's PATH starts)
// and before the system's, each only noting it was asked.
const reached = path.join(scratch, "reached");
const tools = path.join(scratch, "tools");
function standInTools(folder) {
  for (const tool of ["pkill", "caffeinate", "crontab", "launchctl"]) write(path.join(folder, tool), `#!/bin/sh\necho "${tool} $*" >>'${reached}'\n`, 0o755);
}
standInTools(tools);
process.env.PATH = `${tools}${path.delimiter}${process.env.PATH}`;
// Its binary: its version, the link's script run as the engine's Bun runs it (here this Node), and
// the engine itself as its own child, so the process the keeper started and looks for by its
// path (`<home>/engine/<version>/opencode`) is this one, and goes with its engine when it's told to.
const engineBinary = version => `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo ${version}; exit 0 ;;
*.js) case "$2" in browser) echo "$*" >>'${reached}'; exit 0 ;; esac; BUN_BE_BUN=1 exec '${process.execPath}' "$@" ;;
serve) trap 'kill "$c" 2>/dev/null' TERM INT HUP; BUN_BE_BUN=1 '${process.execPath}' '${fakeEngine}' "$@" & c=$!; wait "$c" ;;
esac
`;
// The version before's keeper and engine, stand-ins as in updates.test.js: only its stop is asked.
const oldEngine = path.join(scratch, "old-engine.mjs");
fs.writeFileSync(oldEngine, `
import fs from "node:fs"; import http from "node:http";
const server = http.createServer((request, response) => { response.writeHead(200, { "content-type": "application/json" }); response.end(JSON.stringify({ data: request.url.startsWith("/api/session/active") ? {} : [] })); });
const noted = ${JSON.stringify(engineFile)};
server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(noted + ".new", JSON.stringify({ pid: process.pid, port: server.address().port, password: "pw", version: "1.0.0", notes: { mac: "mac-1" } }));
  fs.renameSync(noted + ".new", noted);
});
setInterval(() => { if (!fs.existsSync(${JSON.stringify(home)})) process.exit(0); }, 300);
`);
const oldKeeper = `#!/bin/sh
K="$KUVO_HOME"; R="$K/run"
noted() { sed -n 's/.*"pid":\\([0-9]*\\).*/\\1/p' "$R/engine.json" 2>/dev/null; }
case "$1" in
stop) touch "$R/stopped"; pid=$(noted); if [ -n "$pid" ]; then kill "$pid" 2>/dev/null; while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done; fi; rm -f "$R/engine.json" ;;
start) rm -f "$R/stopped"; '${process.execPath}' '${oldEngine}' >/dev/null 2>&1 & pid=$!; until [ "$(noted)" = "$pid" ]; do kill -0 "$pid" 2>/dev/null || exit 3; sleep 0.1; done ;;
esac
`;

// What the test started, stopped by its own process: the engines by the pids noted, the keeper's
// stand-in, and (through the real keeper's stop, which ends its group) the bridge.
const started = new Set();
let keeping;
atEnd(async () => {
  // Off first (a box removed): from here no start, the update's own included, starts an engine
  // or a keeper, even once the keeper's stand-in below is gone.
  try { write(path.join(home, "run", "off"), ""); } catch {}
  if (fs.existsSync(path.join(home, "bin", "kuvo-start"))) realKeeper("stop");
  // Only what's still the test's own: a pid given to another process since is left alone.
  for (const pid of started) if (command(pid).includes(base) || command(pid).includes(scratch)) try { process.kill(pid); } catch {}
  if (keeping && keeping.exitCode === null && keeping.signalCode === null) {
    keeping.kill();
    await new Promise(resolve => keeping.once("exit", resolve));
  }
});

const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
const pinned = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
let served = {};
const server = http.createServer((request, response) => {
  const body = served[request.url];
  response.writeHead(body ? 200 : 404, { connection: "close" });
  response.end(body ?? "");
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
atEnd(() => server.close());
const address = `http://127.0.0.1:${server.address().port}`;

test("a box takes a release with the real keeper: it starts the release's engine and the bridge, notes them, and stops and starts them when asked", async () => {
  // A machine on 0.9.0, its engine 1.0.0 running, as a Mac last started it (`run/launch.env`),
  // its keeper noted as running.
  // This update exercises the keeper and engine swap on an already upgraded folder.
  write(path.join(home, "folder-version.json"), '{"version":7}\n');
  write(path.join(home, "bin", "box-version"), "0.9.0");
  write(path.join(home, "bin", "kuvo-start"), oldKeeper, 0o700);
  write(path.join(home, "bin", "kuvo-link.js"), "// the old link\n", 0o600);
  write(path.join(home, "engine", "1.0.0", "opencode"), engineBinary("1.0.0"), 0o755);
  write(path.join(home, "run", "launch.env"), "LANG=C\n", 0o600);
  write(path.join(home, "run", "launch.notes"), '{"mac":"mac-1"}', 0o600);
  standInTools(path.join(home, "bin"));
  keeping = spawn("sh", ["-c", "while :; do sleep 1; done", "kuvo-start"], { stdio: "ignore" });
  write(path.join(home, "run", "box.pid"), `${keeping.pid}\n`);
  assert.equal(spawnSync("sh", [path.join(home, "bin", "kuvo-start"), "start"], { env: process.env, timeout: 60_000 }).status, 0, "the old engine runs");
  const before = read(engineFile).pid;
  started.add(before);

  // The release: this checkout's own keeper, launcher, tools and catalogs, its link as built, for
  // an engine of the test's (served here), and a bridge that says where it answers, as the
  // launcher waits to read.
  const engine = path.join(scratch, "engine");
  write(path.join(engine, "package", "bin", "opencode"), engineBinary("1.1.0"), 0o755);
  const engineTar = path.join(scratch, "engine.tgz");
  assert.equal(spawnSync("tar", ["czf", engineTar, "-C", engine, "package"], { env: { ...process.env, COPYFILE_DISABLE: "1" } }).status, 0);
  served["/engine-1.1.0.tgz"] = fs.readFileSync(engineTar);
  const integrity = "sha512-" + crypto.createHash("sha512").update(served["/engine-1.1.0.tgz"]).digest("base64");
  const builds = { x86_64: { url: `${address}/engine-1.1.0.tgz`, integrity }, aarch64: { url: `${address}/engine-1.1.0.tgz`, integrity } };
  const root = path.join(scratch, "sources");
  for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js"]) {
    write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
  }
  write(path.join(root, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"), fs.readFileSync(script));
  write(path.join(root, "box/dist/kuvo-agents.js"), bridgeStandIn);
  write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: "1.1.0", linux: builds, darwin: builds }));
  const out = path.join(scratch, "box-release");
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), "0.10.0", "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz", out], {
    env: { ...process.env, BUN: process.env.BUN || process.env.KUVO_TEST_BUN || (process.versions.bun ? process.execPath : "bun"), NODE: process.execPath, BOX_SOURCES: root, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }) }, encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);

  // Taken from the folder, as a box takes the release an app on its computer carries.
  assert.deepEqual(link.offerUpdate(out, { keys: [pinned], blocked: null }), { status: 202, version: "0.10.0" });
  const taken = await link.takeOffer({ keys: [pinned], blocked: null });
  assert.equal(taken.result, "updated", JSON.stringify(taken));
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.10.0");
  assert.equal(alive(before), false, "the engine before, stopped");

  // The real keeper's files in place, for the release's engine, and what they started.
  const keeper = fs.readFileSync(path.join(home, "bin", "kuvo-start"), "utf8");
  assert.equal(keeper, fs.readFileSync(path.join(checkout, "box/src/keeper/kuvo-start"), "utf8").replace("@ENGINE@", "1.1.0"));
  assert.equal(fs.readFileSync(path.join(home, "bin", "kuvo-launch"), "utf8"), fs.readFileSync(path.join(checkout, "box/src/keeper/kuvo-launch"), "utf8").replace("@ENGINE@", "1.1.0"));
  const noted = read(engineFile);
  started.add(noted.pid);
  assert.equal(noted.version, "1.1.0");
  assert.deepEqual(noted.notes, { mac: "mac-1" }, "as the Mac that started it last left it noted");
  assert.ok(Number.isInteger(noted.port) && noted.password, "a port and password of its own");
  assert.ok(command(noted.pid).includes(path.join(home, "engine", "1.1.0", "opencode")), `the release's engine, from the box's own folder: ${command(noted.pid)}`);
  const answered = await fetch(`http://127.0.0.1:${noted.port}/api/session`);
  assert.equal(answered.status, 200, "and it answers where it's noted");
  // The bridge, by the engine's own runtime, kept by the keeper apart from the engine: noted, and
  // saying where it answers.
  const bridge = Number(fs.readFileSync(path.join(home, "run", "agents.pid"), "utf8"));
  assert.ok(alive(bridge), "the agents' bridge runs");
  assert.ok(command(bridge).includes(path.join(home, "bin", "kuvo-agents.js")), command(bridge));
  const bridgeNoted = read(path.join(home, "run", "bridge.json"));
  assert.match(fs.readFileSync(path.join(home, "run", "agents.log"), "utf8"), new RegExp(`"url":"http://127\\.0\\.0\\.1:${bridgeNoted.port}"`));
  // Only your own to read, as the launcher leaves them.
  assert.equal((fs.statSync(engineFile).mode & 0o777).toString(8), "600");
  assert.equal(fs.existsSync(path.join(home, "run", ".launching")), false, "the start's lock let go");
  // Judged well by the one health rule (its clock the test's: a look a second, three seconds in all).
  let ticks = 0;
  assert.equal(await link.settleUpdate({ clock: () => ticks, every: 1000, window: 3000, wait: async () => { ticks += 1000; } }), "well");

  // Stop: the engine and the bridge go, and it stays stopped; Start: a new engine, noted anew.
  const stopped = realKeeper("stop");
  assert.equal(stopped.status, 0, stopped.stderr);
  assert.equal(alive(noted.pid), false, "the engine, stopped");
  assert.equal(alive(bridge), false, "the bridge, with it");
  assert.equal(fs.existsSync(engineFile), false);
  assert.equal(fs.existsSync(path.join(home, "run", "stopped")), true);
  assert.equal(realKeeper("engine").status, 0);
  assert.equal(fs.existsSync(engineFile), false, "stopped on purpose, its keeper starts none");
  const again = realKeeper("start");
  assert.equal(again.status, 0, again.stderr);
  const second = read(engineFile);
  started.add(second.pid);
  assert.notEqual(second.pid, noted.pid);
  assert.equal(second.version, "1.1.0");
  assert.ok(alive(second.pid));
  const secondBridge = Number(fs.readFileSync(path.join(home, "run", "agents.pid"), "utf8"));
  assert.ok(alive(secondBridge), "Start starts the bridge too");
  assert.equal(realKeeper("stop").status, 0);
  assert.equal(alive(secondBridge), false);
  assert.equal(alive(second.pid), false);
  // And no keeper but the one noted before: the real one found it running, and started none.
  assert.equal(Number(fs.readFileSync(path.join(home, "run", "box.pid"), "utf8")), keeping.pid);
  assert.equal(fs.existsSync(reached), false, `nothing of the keeper's own loop ran: ${fs.existsSync(reached) ? fs.readFileSync(reached, "utf8") : ""}`);
});
