// The deployed, pre-marker updater taking a signed release with the real keeper. Engines and
// the agents bridge are stand-ins, but both versions' Box CLI entry points are the built files.
// Run with `node --test box/bridge-tests/update-real-keeper.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { layout0Build } from "./failure-bench-legacy.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const script = fileURLToPath(LINK_BUNDLE);
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const baseline = layout0Build();
test("an app-owned box probes the saved baseline as a disposable machine", () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-layout-probe-app-"));
  removeAtEnd(base);
  const home = path.join(base, "app", "box");
  const run = path.join(home, "run");
  const saved = path.join(home, "updates", "rollback", "files", "bin");
  fs.mkdirSync(run, { recursive: true });
  fs.mkdirSync(saved, { recursive: true });
  fs.writeFileSync(path.join(run, "update-trial"), "0.10.0");
  const reader = path.join(saved, "kuvo-link.js");
  const marker = path.join(home, "folder-version.json");
  const upgrade = () => spawnSync(process.execPath, [script, "folder-upgrade"], {
    env: { ...process.env, HOME: base, KUVO_HOME: home, KUVO_BOX: "mac", KUVO_ENGINE_CONFIG: path.join(base, "app", "config") },
    // The folder runner can spend up to 15 seconds on each saved-reader probe.
    encoding: "utf8", timeout: 45_000,
  });
  fs.writeFileSync(reader, baseline);
  const probed = upgrade();
  assert.equal(probed.status, 0, probed.error?.message ?? probed.stderr);
  assert.equal(fs.existsSync(marker), false);

  const rollback = path.join(run, ".rolling-back");
  fs.mkdirSync(rollback);
  fs.writeFileSync(path.join(rollback, "pid"), String(process.pid));
  const rolling = upgrade();
  assert.equal(rolling.status, 1, rolling.error?.message ?? rolling.stderr);
  assert.match(rolling.stderr, /rolling back this Box/);
  assert.equal(fs.existsSync(marker), false);
  fs.rmSync(rollback, { recursive: true });

  const additive = '{"version":1}\n';
  fs.writeFileSync(marker, additive);
  const deferred = upgrade();
  assert.equal(deferred.status, 0, deferred.error?.message ?? deferred.stderr);
  assert.equal(fs.readFileSync(marker, "utf8"), additive);

  fs.writeFileSync(reader, "// An empty script cannot prove it will recover the folder.\n");
  const refused = upgrade();
  assert.equal(refused.status, 1, refused.error?.message ?? refused.stderr);
  assert.match(refused.stderr, /The saved Kuvo Code can't recover folder layout 23/);
  assert.equal(fs.readFileSync(marker, "utf8"), additive);
});

const runUpdate = (name, interrupted) => test(name, async () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-real-keeper-"));
  const home = path.join(base, ".kuvo-code");
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-real-keeper-release-"));
  const env = { ...process.env, HOME: base, KUVO_HOME: home };
  removeAtEnd(base);
  removeAtEnd(scratch);

  function write(file, text, mode = 0o644) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text, { mode });
    fs.chmodSync(file, mode);
  }
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
  const realKeeper = (...args) => spawnSync("sh", [path.join(home, "bin", "kuvo-start"), ...args], { env, encoding: "utf8", timeout: 120_000 });

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
  env.PATH = `${tools}${path.delimiter}${env.PATH}`;
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
  const oldEngine = path.join(home, "engine", "1.0.0", "old-engine.mjs");
  write(oldEngine, `
import fs from "node:fs"; import http from "node:http";
const server = http.createServer((request, response) => { response.writeHead(200, { "content-type": "application/json" }); response.end(JSON.stringify({ data: request.url.startsWith("/api/session/active") ? {} : [] })); });
const noted = ${JSON.stringify(engineFile)};
server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(noted + ".new", JSON.stringify({ pid: process.pid, port: server.address().port, password: "pw", version: "1.0.0", notes: { mac: "mac-1" } }));
  fs.renameSync(noted + ".new", noted);
  console.log("ready");
});
setInterval(() => { if (!fs.existsSync(${JSON.stringify(home)})) process.exit(0); }, 300);
`);
  const oldKeeper = `#!/bin/sh
V=1.0.0
K="$KUVO_HOME"; R="$K/run"
noted() { sed -n 's/.*"pid":\\([0-9]*\\).*/\\1/p' "$R/engine.json" 2>/dev/null; }
old_engine() { [ -n "$1" ] && ps -ww -o command= -p "$1" | grep -Fq -- '${oldEngine}'; }
case "$1" in
stop) touch "$R/stopped"; pid=$(noted); if old_engine "$pid"; then kill "$pid" 2>/dev/null; while old_engine "$pid"; do sleep 0.1; done; fi; rm -f "$R/engine.json" ;;
start) rm -f "$R/stopped"; old_engine "$(noted)" && exit 0; '${process.execPath}' '${oldEngine}' >/dev/null 2>&1 & pid=$!; until [ "$(noted)" = "$pid" ]; do kill -0 "$pid" 2>/dev/null || exit 3; sleep 0.1; done ;;
esac
`;

  // What the test started, stopped by its own process: the engines by the pids noted, the keeper's
  // stand-in, and (through the real keeper's stop, which ends its group) the bridge.
  const started = new Set();
  let keeping;
  let recovering;
  atEnd(async () => {
    // Off first (a box removed): from here no start, the update's own included, starts an engine
    // or a keeper, even once the keeper's stand-in below is gone.
    try { write(path.join(home, "run", "off"), ""); } catch {}
    if (recovering?.exitCode === null && recovering.signalCode === null) {
      recovering.kill();
      await new Promise(resolve => recovering.once("exit", resolve));
    }
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

  const marker = path.join(home, "folder-version.json");
  const trial = path.join(home, "run", "update-trial");
  const state = path.join(home, "run", "update.json");
  const installed = path.join(home, "bin", "kuvo-link.js");
  const saved = path.join(home, "updates", "rollback", "files", "bin", "kuvo-link.js");
  const ledger = path.join(home, "run", "work-queue", "numbers.json");
  const data = '{"next":301,"numbers":{},"since":1234}\n';
  const linkFile = path.join(home, "run", "link.json");
  const device = { key: Buffer.alloc(32, 1).toString("base64url"), from: "pair", kind: "desktop", pairing: "existing-pairing" };
  const voucher = Buffer.alloc(32, 2).toString("base64url");
  const grants = { phones: [{ ...device, by: voucher }], vouchBlocked: [{ key: voucher, device: "Another Mac", at: 1234 }] };
  write(ledger, data);
  write(linkFile, JSON.stringify(grants));
  write(path.join(home, "bin", "box-version"), "0.9.0");
  write(path.join(home, "bin", "kuvo-start"), oldKeeper, 0o700);
  write(installed, baseline, 0o600);
  write(path.join(home, "engine", "1.0.0", "opencode"), engineBinary("1.0.0"), 0o755);
  write(path.join(home, "run", "launch.env"), "LANG=C\n", 0o600);
  write(path.join(home, "run", "launch.notes"), '{"mac":"mac-1"}', 0o600);
  standInTools(path.join(home, "bin"));
  keeping = spawn("sh", ["-c", "while :; do sleep 1; done", "kuvo-start"], { stdio: "ignore" });
  write(path.join(home, "run", "box.pid"), `${keeping.pid}\n`);
  // Have Node reap the original worker as soon as it stops. The updater checks that its PID
  // is gone, which can race the OS reaping a worker launched by an already-exited shell.
  const beforeWorker = spawn(process.execPath, [oldEngine], { env, stdio: ["ignore", "pipe", "ignore"] });
  const beforeExit = once(beforeWorker, "exit");
  atEnd(async () => {
    if (beforeWorker.exitCode === null && beforeWorker.signalCode === null) beforeWorker.kill();
    await beforeExit;
  });
  await once(beforeWorker.stdout, "data", { signal: AbortSignal.timeout(10_000) });
  const before = read(engineFile).pid;
  assert.equal(before, beforeWorker.pid, "the old engine runs");
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
  write(path.join(root, "box/dist/kuvo-link.js"), fs.readFileSync(script));
  write(path.join(root, "box/dist/kuvo-agents.js"), bridgeStandIn);
  write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: "1.1.0", linux: builds, darwin: builds }));
  const out = path.join(scratch, "box-release");
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), "0.10.0", "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz", out], {
    env: { ...env, BUN: env.BUN || process.env.KUVO_TEST_BUN || (process.versions.bun ? process.execPath : "bun"), NODE: process.execPath, BOX_SOURCES: root, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }) }, encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);

  // The real baseline CLI saves its own bridge before swapping in the signed release.
  const waitFor = async (ready, reason, timeout = 30_000, stopped = () => false) => {
    const deadline = Date.now() + timeout;
    while (!stopped() && Date.now() < deadline) {
      if (ready()) return;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.fail(`${reason}\n${output}\n${fs.existsSync(state) ? fs.readFileSync(state, "utf8") : ""}`);
  };
  const cut = path.join(scratch, "cut.mjs");
  if (interrupted) write(cut, `
import fs from "node:fs";
const rename = fs.renameSync;
fs.renameSync = (from, to) => {
  rename(from, to);
  if (to === ${JSON.stringify(path.join(home, "bin", "kuvo-start"))}) process.kill(process.pid, "SIGKILL");
};
`);
  const updater = spawn(process.execPath, [
    ...(interrupted ? ["--import", pathToFileURL(cut).href] : []),
    installed, "update", "--now", "--from", out, "--key", pinned,
  ], { env: { ...env, KUVO_TEST_HEALTH_WINDOW: "3000", KUVO_TEST_HEALTH_EVERY: "200" }, stdio: ["ignore", "pipe", "pipe"] });
  let output = "";
  updater.stdout.on("data", chunk => { output += chunk; });
  updater.stderr.on("data", chunk => { output += chunk; });
  const ended = new Promise((resolve, reject) => {
    updater.once("error", reject);
    updater.once("exit", (code, signal) => resolve({ code, signal }));
  });
  atEnd(async () => {
    if (updater.exitCode === null && updater.signalCode === null) {
      updater.kill();
      await ended;
    }
  });
  if (interrupted) {
    assert.deepEqual(await ended, { code: null, signal: "SIGKILL" }, output);
    assert.equal(read(state).trial.swapped, undefined, "one or more files are still being installed");
  } else {
    // The baseline writes this marker after swapping the files, just before calling the keeper.
    // Start the keeper's 120-second command budget there, with room to publish the result.
    const updaterStopped = () => updater.exitCode !== null || updater.signalCode !== null;
    const engineStarted = path.join(home, "updates", "rollback", "engine-started");
    await waitFor(() => fs.existsSync(engineStarted),
      "the baseline updater should finish installing the release before starting the engine", 120_000, updaterStopped);
    await waitFor(() => fs.existsSync(state) && !!read(state).trial?.swapped,
      "the baseline updater should finish the swap and start the keeper", 125_000, updaterStopped);
    await waitFor(() => fs.existsSync(state) && !!read(state).trial?.judged?.well,
      "the baseline judge should see a healthy trial", 30_000, updaterStopped);
  }
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.10.0");
  assert.equal(fs.readFileSync(trial, "utf8"), "0.10.0");
  assert.deepEqual(fs.readFileSync(saved), baseline);
  assert.deepEqual(fs.readFileSync(installed), fs.readFileSync(script));
  assert.equal(fs.existsSync(marker), false, "the saved pre-marker bridge still has an unmarked folder");
  assert.equal(fs.readFileSync(ledger, "utf8"), data);
  assert.equal(realKeeper("check-rollback-layout").status, 0);
  assert.deepEqual(read(linkFile), grants);

  if (interrupted) {
    // The keeper can start the incoming resident before it detects the interrupted trial.
    const resident = spawn(process.execPath, [installed, "serve", "--resident"], {
      env, stdio: ["ignore", "ignore", "pipe"],
    });
    resident.stderr.on("data", chunk => { output += chunk; });
    const residentExit = once(resident, "exit");
    const stopResident = async () => {
      if (resident.exitCode === null && resident.signalCode === null) resident.kill("SIGTERM");
      await residentExit;
    };
    atEnd(stopResident);
    const serveFile = path.join(home, "run", "serve.json");
    await waitFor(() => fs.existsSync(serveFile) && read(serveFile).pid === resident.pid,
      "the resident should answer during the trial", 20_000, () => resident.exitCode !== null || resident.signalCode !== null);
    assert.deepEqual(read(linkFile), grants, "the trial resident leaves the legacy link state as it found it");
    await stopResident();

    // A repair can have loaded the trial's link before rollback, but reach the
    // folder preflight only after the saved link is in place and the trial is gone.
    const latch = path.join(scratch, "delay-repair.mjs");
    const loaded = path.join(scratch, "repair-loaded");
    const resume = path.join(scratch, "repair-resume");
    write(latch, `
import fs from "node:fs";
import { registerHooks } from "node:module";
const nap = new Int32Array(new SharedArrayBuffer(4));
registerHooks({ load(url, context, next) {
  const code = next(url, context);
  if (url === ${JSON.stringify(pathToFileURL(fs.realpathSync(installed)).href)}) {
    fs.writeFileSync(${JSON.stringify(loaded)}, "");
    while (!fs.existsSync(${JSON.stringify(resume)})) Atomics.wait(nap, 0, 0, 10);
  }
  return code;
} });
`);
    const delayed = spawn(process.execPath, ["--import", pathToFileURL(latch).href, installed, "repair"], {
      env, stdio: ["ignore", "ignore", "pipe"],
    });
    let repairError = "";
    delayed.stderr.on("data", chunk => { repairError += chunk; });
    const repairExit = new Promise(resolve => delayed.once("close", resolve));
    atEnd(async () => {
      if (delayed.exitCode === null && delayed.signalCode === null) delayed.kill();
      await repairExit;
    });
    await waitFor(() => fs.existsSync(loaded), "the repair must have loaded the trial's link");

    // The real keeper launches the replacement judge, which sees the incomplete swap and
    // calls the keeper's rollback. Test code never calls rollback or rewrites the marker.
    recovering = spawn("sh", [path.join(home, "bin", "kuvo-start"), "keep"], {
      env: { ...env, KUVO_TEST_KEEP_ROUND: "1" }, stdio: "ignore",
    });
    await waitFor(() => !fs.existsSync(trial) && !fs.existsSync(path.join(home, "run", ".rolling-back")) && fs.existsSync(engineFile),
      "the keeper should recover the interrupted update and release its rollback fence");
    const restarted = read(engineFile);
    started.add(restarted.pid);
    assert.equal(restarted.version, "1.0.0");
    assert.ok(alive(restarted.pid), "the original engine is running again");
    assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.9.0");
    assert.deepEqual(fs.readFileSync(installed), baseline);
    assert.equal(fs.existsSync(marker), false);
    write(resume, "");
    assert.equal(await repairExit, 1, repairError);
    assert.match(repairError, /files changed before it could upgrade this Box/);
    assert.equal(fs.existsSync(marker), false, "the stale repair must leave the restored folder unmarked");
    assert.equal(fs.readFileSync(ledger, "utf8"), data);
    assert.deepEqual(read(linkFile), grants);
    const restored = spawnSync(process.execPath, [installed, "box", "route"], {
      env, input: '{"method":"GET","path":"kuvo/info"}', encoding: "utf8", timeout: 10_000,
    });
    assert.equal(restored.status, 0, restored.stderr);
    assert.equal(JSON.parse(restored.stdout).status, 200);
    return;
  }

  assert.deepEqual(await ended, { code: 0, signal: null }, output);
  assert.equal(read(state).last.result, "updated", output);
  assert.equal(fs.existsSync(trial), false);
  assert.equal(fs.existsSync(path.dirname(path.dirname(path.dirname(saved)))), false);
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

  // Stop: the engine and the bridge go, and it stays stopped; Start: a new engine, noted anew.
  const stopped = realKeeper("stop");
  assert.equal(stopped.status, 0, stopped.stderr);
  assert.equal(alive(noted.pid), false, "the engine, stopped");
  assert.equal(alive(bridge), false, "the bridge, with it");
  assert.equal(fs.existsSync(engineFile), false);
  assert.equal(fs.existsSync(path.join(home, "run", "stopped")), true);
  assert.equal(realKeeper("engine").status, 0);
  assert.deepEqual(read(marker), { version: 23 }, "the keeper commits the layout once the baseline judge confirms");
  assert.deepEqual(read(linkFile).phones, [device], "the migration preserves the existing device pairing");
  assert.equal(Object.hasOwn(read(linkFile), "vouchBlocked"), false);
  const probe = spawnSync(process.execPath, [installed, "box", "route"], {
    env, input: '{"method":"GET","path":"kuvo/info"}', encoding: "utf8", timeout: 10_000,
  });
  assert.equal(probe.status, 0, probe.stderr);
  const info = JSON.parse(probe.stdout);
  assert.equal(info.status, 200);
  assert.deepEqual(JSON.parse(info.body).features.filter(feature => feature === "goals" || feature.startsWith("work.goals")
    || feature === "messages.commands" || feature === "queue.removed.keyword"), [], "the retired features disappear when the trial ends");
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
runUpdate("the pre-marker updater confirms a release using the real keeper, then the folder upgrades", false);
runUpdate("the real keeper rolls an interrupted pre-marker updater back to the saved baseline", true);
