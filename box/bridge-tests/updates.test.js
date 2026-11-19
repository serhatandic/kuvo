// A box updating itself (PROTOCOL.md, "Updates"): only what a pinned key signed, only newer, and
// back to what it had when the new version doesn't come up. Run here, in a folder of the test's
// own, against a release served on localhost, with a stand-in engine and keeper (the real ones
// need Linux); the new link's self-test is the real one.
// Run with `node --test box/bridge-tests/*.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { layout4Build } from "./failure-bench-legacy.js";
import { runningAs } from "./running-as.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const boxHome = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-updates-"));
const home = path.join(boxHome, ".kuvo-code");
process.env.KUVO_HOME = home;
const link = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-release-"));
const engines = new Set();
removeAtEnd(boxHome);
removeAtEnd(scratch);
atEnd(() => {
  for (const pid of engines) try { process.kill(pid); } catch {}
  stopEngine();
});

// The engine: answers as one does, notes itself as the keeper's start does, runs until stopped.
const fakeEngine = path.join(scratch, "engine.mjs");
fs.writeFileSync(fakeEngine, `
import fs from "node:fs"; import http from "node:http"; import path from "node:path";
const [version, root] = process.argv.slice(2);
const server = http.createServer((request, response) => {
  response.writeHead(200, { "content-type": "application/json" });
  response.end(JSON.stringify({ data: request.url.startsWith("/api/session/active") ? {} : [] }));
});
// Noted whole or not at all: a reader never finds it half written.
const noted = path.join(root, "run", "engine.json");
server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(noted + ".new", JSON.stringify({ pid: process.pid, port: server.address().port, password: "pw", version, notes: { mac: "mac-1" } }));
  fs.renameSync(noted + ".new", noted);
});
// Gone with the test's folder, however late an update in the background started it.
setInterval(() => { if (!fs.existsSync(root)) process.exit(0); }, 300);
`);
process.env.FAKE_ENGINE = fakeEngine;
// This node itself, not a version manager's shim that might start it as a child: the engine is the
// keeper's own process (its pid, $!), as the keeper's start waits to see it noted.
process.env.FAKE_NODE = process.execPath;
// One that `breaks` runs and fails before it answers: "silent", writing nothing; "writes", after
// writing its journal beside the data (as an engine opening it does).
const engineBinary = (version, breaks = null) => `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in --version) echo ${version}; exit 0 ;; *.js) exec "$FAKE_NODE" "$@" ;; esac
${breaks === "writes" ? `mkdir -p "$KUVO_HOME/data/opencode"; echo "the new engine's journal" > "$KUVO_HOME/data/opencode/opencode.db-wal"; exit 1` : breaks ? "exit 1" : `exec "$FAKE_NODE" "$FAKE_ENGINE" ${version} "$KUVO_HOME" "$0"`}
`;
// The keeper: stops the engine (waiting until it's gone, as the real one does), or starts this
// version's. As the real one's (RemoteEngine.swift's bootScript), a start leaves an engine that
// runs as it is, and returns once the one it started is noted in run/engine.json: never a second
// engine, nor a note written after it returned. One that `breaks` starts none, and leaves the
// journal a new engine would (WAL) beside the database.
const keeper = (version, breaks = false) => `#!/bin/sh
K="$KUVO_HOME"; R="$K/run"
V=${version}
noted() { sed -n 's/.*"pid":\\([0-9]*\\).*/\\1/p' "$R/engine.json" 2>/dev/null; }
case "$1" in
stop) touch "$R/stopped"; pid=$(noted); if [ -n "$pid" ]; then kill "$pid" 2>/dev/null; while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done; fi; rm -f "$R/engine.json" ;;
start) rm -f "$R/stopped"; old=$(noted); if [ -n "$old" ] && kill -0 "$old" 2>/dev/null; then exit 0; fi; ${breaks ? `mkdir -p "$K/data/opencode"; echo "the new engine's journal" > "$K/data/opencode/opencode.db-wal"` : `"$K/engine/$V/opencode" serve >/dev/null 2>&1 & pid=$!; until [ "$(noted)" = "$pid" ]; do kill -0 "$pid" 2>/dev/null || exit 3; sleep 0.1; done`} ;;
rollback) [ -f "$R/update-trial" ] || exit 0; sh "$K/updates/rollback/restore.sh" ;;
esac
`;

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
}
const sha256 = data => crypto.createHash("sha256").update(data).digest("hex");
const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const engineFile = path.join(home, "run", "engine.json");

function stopEngine() {
  try { process.kill(read(engineFile).pid); } catch {}
  fs.rmSync(engineFile, { force: true });
}

async function waitFor(check, what, tries = 100) {
  for (let i = 0; i < tries; i++) {
    if (check()) return;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.fail(`never: ${what}`);
}

// A machine on 0.9.0, its engine 1.0.0 running.
function machine() {
  stopEngine();
  fs.rmSync(home, { recursive: true, force: true });
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  // The stand-in release changes code within the same layout. Layout changes use real builds in folder-version.test.js.
  write(path.join(home, "folder-version.json"), '{"version":6}\n');
  write(path.join(home, "bin", "box-version"), "0.9.0");
  write(path.join(home, "bin", "kuvo-start"), keeper("1.0.0"), 0o700);
  write(path.join(home, "bin", "kuvo-link.js"), "// the old link\n", 0o600);
  write(path.join(home, "bin", "kuvo-wake"), "# the old wake-ups\n", 0o755);
  write(path.join(home, "bin", ".own", "kuvo-wake"), "# the old wake-ups\n", 0o755);
  write(path.join(home, "engine", "1.0.0", "opencode"), engineBinary("1.0.0"), 0o755);
  write(path.join(home, "data", "opencode", "opencode.db"), "the old engine's data");
  // A start that never sees its engine noted fails here (status null) rather than holding up the
  // file for good; one takes a second or two under load.
  assert.equal(spawnSync("sh", [path.join(home, "bin", "kuvo-start"), "start"], { env: process.env, timeout: 60_000 }).status, 0, "the old engine runs");
  engines.add(read(engineFile).pid);
}

// A release of 0.10.0 (engine 1.1.0), signed with `key`, served here, and kept in a folder as Kuvo
// Code carries it (`folder`: boxes.json and the pack beside it). `tamper` changes what's packed;
// `bundle` adds a Mac bundle by its files (`{name: text}`, in `bin/Kuvo Helper.app`, named in
// box.json's `bundles`); `extra` adds files to the pack that box.json doesn't name, `link` a link
// in the bundle.
const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
const pinned = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
let served = {};
// Each answer on a connection of its own. An update holds this process's event loop through its
// self-test, stop and start (each a spawnSync; 3 to 5 seconds in all at a load of 120, more at
// times), and past the server's five-second keep-alive, the next update's request went out on the
// connection kept from the last one as the server closed it ("fetch failed").
// What breaks off once before it's served (`drop`: the connection cut, else that status).
let breaksOnce = {};
// How many times each was asked for.
const fetched = {};
const server = http.createServer((request, response) => {
  fetched[request.url] = (fetched[request.url] ?? 0) + 1;
  const breaking = breaksOnce[request.url];
  if (breaking) {
    delete breaksOnce[request.url];
    if (breaking === "drop") return request.socket.destroy();
    response.writeHead(breaking, { connection: "close" });
    return response.end("");
  }
  const body = served[request.url];
  response.writeHead(body ? 200 : 404, { connection: "close" });
  response.end(body ?? "");
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
atEnd(() => server.close());
const base = `http://127.0.0.1:${server.address().port}`;

function release({ version = "0.10.0", engine: engineVersion = "1.1.0", released = new Date(Date.now() - 10 * 86_400_000), breaks = false, engineBreaks = null, linkBreaks = false, tamper = false, key = privateKey, bundle = null, extra = null, link = false, keeperScript = null } = {}) {
  const files = path.join(scratch, `files-${crypto.randomUUID()}`);
  write(path.join(files, "bin", "box-version"), version);
  write(path.join(files, "bin", "kuvo-start"), keeperScript ?? keeper(engineVersion, breaks), 0o700);
  // (One that `linkBreaks` passes its self-test, and answers nothing else.)
  write(path.join(files, "bin", "kuvo-link.js"), linkBreaks ? 'if (process.argv.includes("selftest")) console.log("ok"); else process.exit(1);\n' : fs.readFileSync(script, "utf8"), 0o600);
  write(path.join(files, "bin", "kuvo-wake"), "# the new wake-ups\n", 0o755);
  const box = { version, engine: engineVersion, files: {} };
  for (const file of ["bin/box-version", "bin/kuvo-start", "bin/kuvo-link.js", "bin/kuvo-wake"]) box.files[file] = sha256(fs.readFileSync(path.join(files, file)));
  if (bundle) {
    const listed = box.bundles = { "bin/Kuvo Helper.app": {} };
    for (const [name, text] of Object.entries(bundle)) {
      write(path.join(files, "bin", "Kuvo Helper.app", name), text, name.includes("MacOS/") ? 0o755 : 0o644);
      listed["bin/Kuvo Helper.app"][name] = sha256(Buffer.from(text));
    }
  }
  for (const [name, text] of Object.entries(extra ?? {})) write(path.join(files, name), text);
  if (link) fs.symlinkSync("/etc/hosts", path.join(files, "bin", "Kuvo Helper.app", "Contents", "hosts"));
  write(path.join(files, "box.json"), JSON.stringify(box));
  const pack = path.join(scratch, `pack-${crypto.randomUUID()}.tgz`);
  spawnSync("tar", ["czf", pack, "-C", files, "."], { env: { ...process.env, COPYFILE_DISABLE: "1" } });
  const engine = path.join(scratch, `engine-${crypto.randomUUID()}`);
  write(path.join(engine, "package", "bin", "opencode"), engineBinary(engineVersion, engineBreaks), 0o755);
  const engineTar = path.join(scratch, `engine-${crypto.randomUUID()}.tgz`);
  spawnSync("tar", ["czf", engineTar, "-C", engine, "package"], { env: { ...process.env, COPYFILE_DISABLE: "1" } });
  const packData = fs.readFileSync(pack), engineData = fs.readFileSync(engineTar);
  const integrity = "sha512-" + crypto.createHash("sha512").update(engineData).digest("base64");
  const manifest = Buffer.from(JSON.stringify({
    kuvo: "box/1", version, released: released.toISOString(), engine: engineVersion,
    // Each system's own build, and the other's isn't there: a box takes the one for its system.
    ...Object.fromEntries(["linux", "darwin"].map(system => {
      const url = (system === "darwin") === (process.platform === "darwin") ? `${base}/engine.tgz` : `${base}/another-systems-engine.tgz`;
      return [system, { x86_64: { url, integrity }, aarch64: { url, integrity } }];
    })),
    pack: { url: `${base}/pack.tgz`, sha256: sha256(packData), size: packData.length },
  }));
  const signature = crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), key);
  served = {
    "/boxes.json": JSON.stringify({ manifest: manifest.toString("base64"), signature: signature.toString("base64"), key: crypto.createPublicKey(key).export({ type: "spki", format: "der" }).subarray(-32).toString("base64") }),
    "/pack.tgz": tamper ? Buffer.concat([packData, Buffer.from("x")]) : packData,
    "/engine.tgz": engineData,
  };
  const folder = path.join(scratch, `release-${crypto.randomUUID()}`);
  write(path.join(folder, "boxes.json"), served["/boxes.json"]);
  write(path.join(folder, "pack.tgz"), served["/pack.tgz"]);
  return { folder };
}

const update = (options = {}) => link.update({ now: true, keys: [pinned], url: `${base}/boxes.json`, blocked: null, ...options });
// The update on trial judged by the one health rule (updates/health.ts), its looks the real ones,
// its clock the test's: a look a second, all holding three seconds of it.
let ticks = 0, looks = 0;
const settle = (options = {}) => link.settleUpdate({ clock: () => ticks, every: 1000, window: 3000, failingFor: 2000, wait: async () => { ticks += 1000; }, ...options, look: async (...args) => { looks++; return (options.look ?? link.lookAtPieces)(...args); } });
// A judge of its own (\`update --settle\`), as the keeper starts it, and the update command's: quick here.
process.env.KUVO_TEST_HEALTH_WINDOW = "600";
process.env.KUVO_TEST_HEALTH_EVERY = "150";
// A release from a folder here (`kuvo/box/update {from}`): what Kuvo Code carries, offered as it
// updates, checked as a downloaded one is, and taken at a quiet moment.
const offer = (folder, options = {}) => link.offerUpdate(folder, { keys: [pinned], blocked: null, ...options });
const take = (options = {}) => link.takeOffer({ keys: [pinned], blocked: null, ...options });
const offered = path.join(home, "updates", "offered");
const boxVersion = () => fs.readFileSync(path.join(home, "bin", "box-version"), "utf8");

test("versions order by number, a beta before its release", () => {
  assert.equal(link.newerVersion("0.10.0", "0.9.3"), true);
  assert.equal(link.newerVersion("0.9.3", "0.9.3"), false);
  assert.equal(link.newerVersion("0.8.0", "0.8.0-beta.2"), true);
  assert.equal(link.newerVersion("0.8.0-beta.3", "0.8.0-beta.2"), true);
  assert.equal(link.newerVersion("0.8.0-beta.1", "0.8.0"), false);
  assert.equal(link.newerVersion("0.8.0", null), true, "a box nothing marked takes any");
  assert.equal(link.newerVersion("not a version", "0.1.0"), false);
});

test("a release counts only signed with a pinned key, over what it says", () => {
  release();
  const manifest = link.verifyBoxes(served["/boxes.json"], [pinned]);
  assert.equal(manifest.version, "0.10.0");
  assert.throws(() => link.verifyBoxes(served["/boxes.json"], []), /isn't signed with a key/);
  assert.throws(() => link.verifyBoxes(served["/boxes.json"], [crypto.generateKeyPairSync("ed25519").publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64")]), /isn't signed with a key/);
  const given = JSON.parse(served["/boxes.json"]);
  const changed = Buffer.from(given.manifest, "base64").toString("utf8").replace("0.10.0", "0.99.0");
  assert.throws(() => link.verifyBoxes(JSON.stringify({ ...given, manifest: Buffer.from(changed).toString("base64") }), [pinned]), /signature doesn't match/);
  // The same key's signature on the bytes alone (as for something else it signs) doesn't count.
  const bare = crypto.sign(null, Buffer.from(given.manifest, "base64"), privateKey).toString("base64");
  assert.throws(() => link.verifyBoxes(JSON.stringify({ ...given, signature: bare }), [pinned]), /signature doesn't match/);
});

test("a box updates itself only while every Mac that started its engine lately leaves a newer box as it is, and only in its own folder", () => {
  const own = path.join(os.homedir(), ".kuvo-code");
  assert.match(link.updatesBlocked({}, Date.now(), home), /own Kuvo Code folder/);
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  link.noteLauncher(() => ({ notes: { mac: "mac-old" } }));
  assert.match(link.updatesBlocked({ tolerantIDs: { "mac-new": true } }, Date.now(), own), /older Kuvo Code/);
  assert.match(link.updatesBlocked({ tolerantIDs: { "mac-old": false } }, Date.now(), own), /older Kuvo Code/);
  assert.equal(link.updatesBlocked({ tolerantIDs: { "mac-old": true } }, Date.now(), own), null);
  assert.equal(link.updatesBlocked({}, Date.now() + 31 * 86_400_000, own), null, "one not seen in 30 days doesn't count");
  fs.rmSync(path.join(home, "run", "launchers.json"), { force: true });
});

test("a box updates to a newer release: its engine and files swapped, started again, kept until the new link says it's well", async () => {
  machine();
  release();
  const outcome = await update();
  assert.deepEqual(outcome, { result: "updated", version: "0.10.0" });
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.10.0");
  assert.match(fs.readFileSync(path.join(home, "bin", "kuvo-start"), "utf8"), /^V=1\.1\.0$/m);
  assert.equal((fs.statSync(path.join(home, "bin", "kuvo-link.js")).mode & 0o777), 0o600, "with its own mode");
  // Its tools kept as the box's own too, for its keeper to put back after an older Mac's visit.
  assert.equal(fs.readFileSync(path.join(home, "bin", ".own", "kuvo-wake"), "utf8"), "# the new wake-ups\n");
  assert.equal((fs.statSync(path.join(home, "bin", ".own", "kuvo-wake")).mode & 0o777), 0o755);
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
  assert.ok(fs.existsSync(path.join(home, "updates", "rollback", "restore.sh")));
  assert.ok(fs.existsSync(path.join(home, "run", "update-trial")));
  assert.equal(fs.readdirSync(path.join(home, "updates")).sort().join(), "backups,installed.json,release,rollback", "what was downloaded goes; what it put there is noted; its engine changed, a copy of the data from before is kept aside; the signed release kept, to hand to your other computers");

  // Judged well: what was kept, and the old engine, go.
  assert.equal(await settle(), "well");
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false);
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false);
  assert.deepEqual(fs.readdirSync(path.join(home, "engine")), ["1.1.0"]);
  assert.deepEqual(read(path.join(home, "run", "update.json")).last.to, "0.10.0");
  assert.equal(await settle(), "none", "once");

  // The same release again: nothing to do.
  assert.equal((await update()).result, "current");
});

test("a well update, with no link running and no relay, is judged well by a judge of its own (as the keeper starts it), on any box", async () => {
  machine();
  release();
  assert.equal((await update()).result, "updated");
  engines.add(read(engineFile).pid);
  assert.equal(fs.existsSync(path.join(home, "run", "link.needed")), false, "nothing for a link to answer");
  const judged = spawnSync(process.execPath, [script, "update", "--settle"], { env: process.env, encoding: "utf8", timeout: 60_000 });
  assert.deepEqual(JSON.parse(judged.stdout.trim().split("\n").at(-1)), { settled: "well" }, judged.stderr);
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false);
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false);
  assert.equal(read(path.join(home, "run", "update.json")).last.to, "0.10.0");
  assert.equal(fs.existsSync(path.join(home, "run", "update-judge.pid")), false, "its mark goes with it");
});

test("failure bench: a judge cut off midway is taken up again where it was; time the computer slept doesn't count", async () => {
  machine();
  release();
  assert.equal((await update()).result, "updated");
  engines.add(read(engineFile).pid);
  // Cut off after its second look (the machine went down).
  let waits = 0;
  ticks = 0;
  await assert.rejects(settle({ wait: async () => { ticks += 1000; if (++waits === 2) throw new Error("cut off"); } }));
  assert.equal(read(path.join(home, "run", "update.json")).trial.judged.well, 1000, "what held so far is kept");
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), true);
  // The next judge needs only the rest: three looks, where a fresh one takes four.
  looks = 0;
  assert.equal(await settle(), "well");
  assert.equal(looks, 3);

  // Asleep an hour between looks: two looks' time at most counts each time.
  machine();
  release();
  assert.equal((await update()).result, "updated");
  engines.add(read(engineFile).pid);
  looks = 0;
  assert.equal(await settle({ wait: async () => { ticks += 3_600_000; } }), "well");
  assert.equal(looks, 3, "not well at its second look, an hour on");
});

test("a new server that starts again and again is unwell: the code from before is put back, never the data", async () => {
  machine();
  release();
  write(path.join(home, "run", "serve.needed"), "");
  assert.equal((await update()).result, "updated");
  engines.add(read(engineFile).pid);
  write(path.join(home, "data", "opencode", "opencode.db"), "a conversation after the update");
  // Its server for clients, ending each time it starts (the keeper starting it again each round).
  let serving = null;
  const restart = () => {
    if (serving) serving.kill("SIGKILL");
    serving = spawn(process.execPath, ["-e", "setInterval(() => {}, 1e6)", path.join(home, "bin", "kuvo-link.js"), "serve", "--kept"], { stdio: "ignore" });
    write(path.join(home, "run", "serve.json"), JSON.stringify({ pid: serving.pid, port: 1, password: "pw", kept: true }));
  };
  atEnd(() => serving?.kill("SIGKILL"));
  looks = 0;
  ticks = 0;
  const settled = await settle({ look: async (...args) => { restart(); await waitFor(() => serving.pid, "it runs"); return link.lookAtPieces(...args); }, window: 60_000 });
  assert.equal(settled, "unwell");
  assert.ok(looks <= 5, `seen as starting again and again by its fifth look (${looks})`);
  assert.equal(boxVersion(), "0.9.0", "the code from before");
  assert.equal(fs.readFileSync(path.join(home, "bin", "kuvo-link.js"), "utf8"), "// the old link\n");
  assert.equal(fs.readFileSync(path.join(home, "data", "opencode", "opencode.db"), "utf8"), "a conversation after the update", "never the data");
  const state = read(path.join(home, "run", "update.json"));
  assert.equal(state.trial, undefined);
  assert.equal(state.last.result, "failed");
  assert.match(state.last.message, /0\.10\.0 didn't run well here \(its server for your devices didn't run\), so it went back to 0\.9\.0/);
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false);
  await waitFor(() => fs.existsSync(engineFile), "an engine runs again");
  engines.add(read(engineFile).pid);
  fs.rmSync(path.join(home, "run", "serve.needed"));
});

test("a rollback begun that didn't get it all back is carried on by the next judge, not judged again; nothing is judged while Kuvo is stopped until it next starts", async () => {
  machine();
  release();
  assert.equal((await update()).result, "updated");
  engines.add(read(engineFile).pid);
  // Stopped until it next starts (Stop Kuvo on this Mac): no judging meanwhile, the trial kept.
  write(path.join(home, "run", "paused"), "");
  looks = 0;
  assert.equal(await settle(), "none");
  assert.equal(looks, 0);
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), true);
  fs.rmSync(path.join(home, "run", "paused"));
  // A rollback begun before (its first try failed): carried on, at once, without a look.
  write(path.join(home, "run", "update-restore"), "1");
  let putBack = 0;
  assert.equal(await settle({ rollBack: () => { putBack++; return link.rollBackCode(); } }), "unwell");
  assert.equal(putBack, 1);
  assert.equal(looks, 0);
  assert.equal(boxVersion(), "0.9.0");
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false);
  await waitFor(() => fs.existsSync(engineFile), "an engine runs again");
  engines.add(read(engineFile).pid);
});

test("failure bench: an updater killed during a file swap rolls back and preserves the engine data", { timeout: 90_000 }, async () => {
  machine();
  release();
  const cut = path.join(home, "bin", "kuvo-link.js");
  const ledger = path.join(home, "run", "work-queue", "numbers.json");
  const stored = { primary: "self", next: 47, leases: [], since: 1_000 };
  write(ledger, JSON.stringify(stored));
  const boot = path.join(scratch, "update-until-first-rename.mjs");
  // Let the updater finish the first atomic rename. Kill its process there, before the second
  // file or the new engine can run. This leaves the actual on-disk trial and rollback journal.
  write(boot, `
import fs from "node:fs";
const rename = fs.renameSync;
fs.renameSync = (from, to) => {
  rename(from, to);
  if (to === ${JSON.stringify(path.join(home, "bin", "box-version"))}) process.kill(process.pid, "SIGKILL");
};
const link = await import(${JSON.stringify(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url).href)});
const result = await link.update({ now: true, keys: [${JSON.stringify(pinned)}], url: ${JSON.stringify(`${base}/boxes.json`)}, blocked: null });
throw new Error("The updater did not reach the cut: " + JSON.stringify(result));
`);
  const child = spawn(process.execPath, [boot], { stdio: ["ignore", "ignore", "inherit"] });
  const ended = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", (code, signal) => resolve({ code, signal }));
  });
  atEnd(() => {
    if (child.exitCode === null && child.signalCode === null) child.kill();
  });
  assert.deepEqual(await ended, { code: null, signal: "SIGKILL" });
  assert.equal(boxVersion(), "0.10.0", "one file is already new");
  assert.equal(fs.readFileSync(cut, "utf8"), "// the old link\n", "the remaining files are still old");
  assert.equal(fs.existsSync(engineFile), false, "neither engine is running");

  looks = 0;
  assert.equal(await settle(), "unwell");
  assert.equal(looks, 0, "the partial update never runs for health checks");
  assert.equal(boxVersion(), "0.9.0");
  assert.equal(fs.readFileSync(cut, "utf8"), "// the old link\n");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.0.0", "the old engine runs again");
  engines.add(read(engineFile).pid);
  assert.equal(fs.readFileSync(path.join(home, "data", "opencode", "opencode.db"), "utf8"), "the old engine's data");
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false);
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false);
  assert.deepEqual(read(ledger), stored);

  assert.equal((await update()).result, "updated", "retry takes the same signed release");
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.10.0");
  const resumed = spawnSync(process.execPath, ["--input-type=module", "-e",
    `import { askToSend } from ${JSON.stringify(pathToFileURL(cut).href)}; askToSend();`,
  ], { encoding: "utf8", timeout: 10_000 });
  assert.equal(resumed.status, 0, resumed.stderr);
  assert.deepEqual(read(ledger), { ...stored, formatVersion: 1 });
  assert.equal(fs.readFileSync(path.join(home, "data", "opencode", "opencode.db"), "utf8"), "the old engine's data");
});

function layoutTrial() {
  machine();
  const previous = layout4Build();
  const current = path.join(home, "bin", "kuvo-link.js");
  write(current, previous);
  write(path.join(home, "bin", "kuvo-start"), keeper("1.0.0").replace("stop)", "bridge) : ;;\nstop)"), 0o700);
  const keeperScript = fs.readFileSync(new URL("../src/keeper/kuvo-start", import.meta.url), "utf8").replaceAll("@ENGINE@", "1.0.0");
  release({ engine: "1.0.0", keeperScript });
  const marker = path.join(home, "folder-version.json");
  const data = path.join(home, "data", "opencode", "opencode.db");
  write(marker, '{"version":4}\n');
  const off = path.join(home, "run", "off");
  // Keep the box entry point from detaching a keeper. The test starts the recovery keeper itself.
  write(off, "");
  write(path.join(home, "bin", "kuvo-setsid"), '#!/bin/sh\nexec "$@"\n', 0o700);
  write(path.join(home, "run", "engine-ran"), "1.0.0\n");
  const trace = path.join(home, "run", "keeper-test.log");
  write(path.join(home, "engine", "1.0.0", "opencode"), engineBinary("1.0.0").replace('*.js) exec "$FAKE_NODE" "$@"',
    `*.js) if [ "$2" = update ] && [ "$3" = --settle ]; then exec "$FAKE_NODE" "$@" >>${JSON.stringify(trace)} 2>&1; fi
    exec "$FAKE_NODE" "$@"`), 0o755);
  const env = { ...process.env, HOME: boxHome, KUVO_TEST_KEEP_ROUND: "1" };
  return { current, data, env, marker, off, previous, trace, keeper: path.join(home, "bin", "kuvo-start") };
}

async function installedUpdater(current, env, cut) {
  const boot = path.join(scratch, "kuvo-link-layout-update.mjs");
  write(boot, `
import fs from "node:fs";
const rename = fs.renameSync;
const cut = ${JSON.stringify(cut ?? null)};
fs.renameSync = (from, to) => {
  rename(from, to);
  if (to === cut) process.kill(process.pid, "SIGKILL");
};
const link = await import(${JSON.stringify(pathToFileURL(current).href)});
const result = await link.update({ now: true, keys: [${JSON.stringify(pinned)}], url: ${JSON.stringify(`${base}/boxes.json`)}, blocked: null });
console.log(JSON.stringify(result));
`);
  const child = spawn(process.execPath, [boot], { env, stdio: ["ignore", "pipe", "inherit"] });
  atEnd(() => { if (child.exitCode === null && child.signalCode === null) child.kill(); });
  let output = "";
  child.stdout.on("data", chunk => { output += chunk; });
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code, signal) => resolve({ code, signal, output }));
  });
}

test("failure bench: a layout 4 update runs the real keeper preflight and confirms layout 6", async () => {
  const { current, data, env, marker, off, keeper } = layoutTrial();
  const run = await installedUpdater(current, env);
  assert.equal(run.code, 0, run.output);
  assert.equal(JSON.parse(run.output.trim().split("\n").at(-1)).result, "updated");
  assert.deepEqual(read(marker), { version: 4 });
  const preflight = spawnSync("sh", [keeper, "box"], { env, encoding: "utf8", timeout: 15_000 });
  assert.equal(preflight.status, 0, preflight.stderr);
  assert.deepEqual(read(marker), { version: 4 }, "the saved bridge can still open the trial folder");
  fs.unlinkSync(off);
  assert.equal(await settle(), "well");
  assert.deepEqual(read(marker), { version: 6 });
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false);
  assert.equal(fs.readFileSync(data, "utf8"), "the old engine's data");
  assert.deepEqual(fs.readFileSync(current), fs.readFileSync(script));
});

test("failure bench: a layout 4 keeper recovers after the updater loses the file swap", { timeout: 45_000 }, async t => {
  const { current, data, env, marker, off, previous, trace, keeper } = layoutTrial();
  const cut = await installedUpdater(current, env, keeper);
  assert.equal(cut.code, null, cut.output);
  assert.equal(cut.signal, "SIGKILL");
  assert.deepEqual(fs.readFileSync(current), fs.readFileSync(script));
  assert.deepEqual(read(marker), { version: 4 });
  fs.unlinkSync(off);
  const orphan = spawn("sh", [keeper, "keep"], { env, stdio: ["ignore", "ignore", "inherit"] });
  t.after(async () => {
    if (orphan.exitCode !== null || orphan.signalCode !== null) return;
    const exited = new Promise(resolve => orphan.once("exit", resolve));
    orphan.kill();
    await exited;
  });
  const diagnostics = () => ["update.json", "update-settle.tries", "update-judge.pid", "update-restore", ".rolling-back/pid", path.basename(trace)]
    .flatMap(file => {
      const at = path.join(home, "run", file);
      return fs.existsSync(at) ? [`${file}: ${fs.readFileSync(at, "utf8")}`] : [];
    }).join("\n");
  try {
    await waitFor(() => !fs.existsSync(path.join(home, "run", "update-trial"))
      || orphan.exitCode !== null, "the keeper settles the orphaned trial", 250);
  } catch (error) {
    assert.fail(`${error.message}\n${diagnostics()}`);
  }
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false, "the keeper recovered before exiting");
  assert.deepEqual(read(marker), { version: 4 });
  assert.deepEqual(fs.readFileSync(current), previous);
  assert.equal(boxVersion(), "0.9.0");
  assert.equal(fs.readFileSync(data, "utf8"), "the old engine's data");
});

test("a bridge that won't take the new build (it doesn't run here) isn't well", async t => {
  machine();
  release();
  assert.equal((await update()).result, "updated");
  engines.add(read(engineFile).pid);
  // The engine's launch started a bridge beside it, which answers.
  const bridge = http.createServer((request, response) => { response.writeHead(200, { "content-type": "application/json" }); response.end("{}"); });
  await new Promise(resolve => bridge.listen(0, "127.0.0.1", resolve));
  atEnd(() => bridge.close());
  write(engineFile, JSON.stringify({ ...read(engineFile), agentsPort: bridge.address().port }));
  write(path.join(home, "bin", "kuvo-agents.js"), "// the bridge\n");
  const pid = await runningAs("kuvo-agents", t);
  const record = { pid, port: bridge.address().port, password: "pw", build: "a".repeat(64) };
  write(path.join(home, "run", "bridge.json"), JSON.stringify(record));
  const trial = read(path.join(home, "run", "update.json")).trial;
  assert.equal((await link.lookAtPieces(trial, trial.swapped)).bridge.ok, true, "making way, or quiet: well");
  write(path.join(home, "run", "bridge.json"), JSON.stringify({ ...record, renewal: { since: Date.now(), holding: false, waits: "restart" } }));
  assert.equal((await link.lookAtPieces(trial, trial.swapped)).bridge.ok, false, "never taking it: not well");
  assert.equal(await settle(), "unwell");
  assert.equal(boxVersion(), "0.9.0");
  await waitFor(() => fs.existsSync(engineFile), "an engine runs again");
  engines.add(read(engineFile).pid);
});

test("a rollback that gave up with some of each version's files left says so, once, wherever it's looked at", () => {
  machine();
  write(path.join(home, "run", "update.json"), JSON.stringify({ trial: { from: "0.9.0", to: "0.10.0", at: Date.now() } }));
  fs.mkdirSync(path.join(home, "updates", "rollback", "files"), { recursive: true });
  link.updateStatus();
  const state = read(path.join(home, "run", "update.json"));
  assert.equal(state.trial, undefined);
  assert.equal(state.last.result, "failed");
  assert.equal(state.last.message, "0.10.0 didn't run well here, and not all of it could go back to 0.9.0: Kuvo updates again later. Your conversations stay as they are.");
  assert.equal(state.offerFailed.takenBack, false);
  fs.rmSync(path.join(home, "updates", "rollback"), { recursive: true });
});

test("an agent's browser waiting on you (a sign-in) holds an update the box begins itself: nothing is stopped or swapped", async t => {
  machine();
  release();
  write(path.join(home, "run", "browser.pid"), String(await runningAs("kuvo-browser", t)));
  write(path.join(home, "run", "browser-asks.json"), JSON.stringify({ asks: [{ id: "ask_1", kind: "signIn", host: "github.com", session: "ses_1", username: true, password: true }] }));
  // (Update Now, on your word, goes ahead: a client asks first. Not by night.)
  assert.deepEqual(await update({ now: false }), { result: "busy", message: "Something is running there now." });
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.9.0");
  assert.equal(read(engineFile).version, "1.0.0", "its engine still runs");
});

test("a new engine that never answers, and wrote nothing, goes back with the rest: the old keeper starts the old engine, on the data as it was", async () => {
  machine();
  release({ engineBreaks: "silent" });
  assert.equal((await update()).result, "updated");
  assert.equal(await settle(), "unwell", "its engine never runs");
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.9.0");
  assert.equal(fs.readFileSync(path.join(home, "bin", "kuvo-link.js"), "utf8"), "// the old link\n");
  assert.equal(fs.readFileSync(path.join(home, "bin", ".own", "kuvo-wake"), "utf8"), "# the old wake-ups\n", "the box's own tools as they were");
  assert.match(fs.readFileSync(path.join(home, "bin", "kuvo-start"), "utf8"), /^V=1\.0\.0$/m);
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.0.0", "the old engine runs again");
  engines.add(read(engineFile).pid);
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false);
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false);
  assert.equal(read(path.join(home, "run", "update.json")).last.result, "failed");
  assert.equal(fs.readFileSync(path.join(home, "data", "opencode", "opencode.db"), "utf8"), "the old engine's data");
});

test("a new engine that wrote to the data before it failed stays: the rollback puts back code only, never the data", async () => {
  machine();
  release({ engineBreaks: "writes" });
  assert.equal((await update()).result, "updated");
  assert.equal(await settle(), "unwell");
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.9.0");
  assert.equal(fs.readFileSync(path.join(home, "bin", "kuvo-link.js"), "utf8"), "// the old link\n");
  // The keeper put back starts the new engine: the old one may not read what the new one wrote.
  assert.match(fs.readFileSync(path.join(home, "bin", "kuvo-start"), "utf8"), /^V=1\.1\.0$/m);
  assert.doesNotMatch(fs.readFileSync(path.join(home, "bin", "kuvo-start"), "utf8"), /breaks|journal/, "the old keeper's code");
  assert.equal(fs.readFileSync(path.join(home, "data", "opencode", "opencode.db-wal"), "utf8"), "the new engine's journal\n", "the data as the new engine left it");
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false);
});

test("a rollback after an engine change keeps what was written since: the copy from before is only kept aside, the latest one", async () => {
  machine();
  release();
  assert.deepEqual(await update(), { result: "updated", version: "0.10.0" });
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
  // A conversation after the update, as the new engine writes it.
  write(path.join(home, "data", "opencode", "opencode.db"), "a conversation after the update");
  write(path.join(home, "data", "opencode", "opencode.db-wal"), "its last message");
  // Not said to be well: the keeper takes it back, as its trial runs out.
  assert.equal(spawnSync("sh", [path.join(home, "updates", "rollback", "restore.sh")], { env: process.env, timeout: 60_000 }).status, 0);
  assert.equal(boxVersion(), "0.9.0", "its code is back");
  assert.equal(fs.readFileSync(path.join(home, "bin", "kuvo-link.js"), "utf8"), "// the old link\n");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs again");
  engines.add(read(engineFile).pid);
  assert.equal(fs.readFileSync(path.join(home, "data", "opencode", "opencode.db"), "utf8"), "a conversation after the update");
  assert.equal(fs.readFileSync(path.join(home, "data", "opencode", "opencode.db-wal"), "utf8"), "its last message");
  // The data from before the engine changed, aside (never put back by anything): the latest copy only.
  const backups = path.join(home, "updates", "backups");
  const kept = fs.readdirSync(backups);
  assert.equal(kept.length, 1);
  assert.match(kept[0], /^\d{4}-\d{2}-\d{2}-1\.0\.0-to-1\.1\.0$/);
  assert.equal(fs.readFileSync(path.join(backups, kept[0], "opencode.db"), "utf8"), "the old engine's data");
});

test("no update goes over one still on trial, whichever way it's asked for: what it kept to go back to stays", async () => {
  machine();
  release();
  assert.deepEqual(await update(), { result: "updated", version: "0.10.0" });
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
  const rollback = path.join(home, "updates", "rollback");
  const keptThen = () => fs.readdirSync(rollback, { recursive: true }).sort().map(name => [name, fs.statSync(path.join(rollback, name)).isFile() ? fs.readFileSync(path.join(rollback, name), "utf8") : ""]);
  const before = keptThen();
  const { folder } = release({ version: "0.11.0" });
  const onTrial = { result: "blocked", message: "Its last update is still being checked: try again in a few minutes." };
  assert.deepEqual(await update(), onTrial, "Update Now");
  assert.deepEqual(await update({ now: false }), onTrial, "the release check's");
  assert.deepEqual(await update({ from: folder }), onTrial, "from a folder (the installer's)");
  assert.equal(offer(folder).status, 202);
  assert.equal(await take({ now: true }), null, "the offered one, on your word");
  assert.deepEqual(keptThen(), before, "what it kept to go back to, untouched");
  assert.equal(boxVersion(), "0.10.0");
  assert.equal(await settle(), "well");
  fs.rmSync(offered, { recursive: true, force: true });
});

test("the release check's update while the last is on trial: tried again in that round, and how the last went stays as it was", async () => {
  machine();
  release();
  assert.deepEqual(await update(), { result: "updated", version: "0.10.0" });
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
  const state = path.join(home, "run", "update.json");
  const before = read(state);
  release({ version: "0.11.0" });
  // As the bridge starts its release check, in a process of its own.
  const round = spawnSync(process.execPath, [script, "update", "--round", link.releaseRound(), "--url", `${base}/boxes.json`, "--key", pinned], { env: process.env, encoding: "utf8", timeout: 60_000 });
  assert.deepEqual(JSON.parse(round.stdout.trim().split("\n").at(-1)), { result: "blocked", message: "Its last update is still being checked: try again in a few minutes." });
  const after = read(state);
  assert.equal(after.round, undefined, "not the round's look: it goes again in that round");
  assert.deepEqual(after.last, before.last, "how the last went, as it was");
  assert.deepEqual(after.trial, before.trial);
  assert.equal(boxVersion(), "0.10.0");
  assert.equal(await settle(), "well");
});

test("never an older engine than the one running: refused, from GitHub or a folder here", async () => {
  machine();
  // The engine here is newer than the release's (1.1.0): it may have written what that can't read.
  write(engineFile, JSON.stringify({ ...read(engineFile), version: "1.2.0" }));
  const { folder } = release();
  const outcome = await update();
  assert.equal(outcome.result, "blocked");
  assert.match(outcome.message, /engine \(1\.2\.0\) is newer than the release's \(1\.1\.0\)/);
  assert.equal((await update({ from: folder })).result, "blocked");
  const offered = offer(folder);
  assert.equal(offered.status, 409);
  assert.match(offered.message, /never goes back to an older engine/);
  assert.equal(boxVersion(), "0.9.0");
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false, "nothing kept, nothing stopped");
});

test("a rollback cut before the new engine started goes back whole: the old keeper starts the old engine", async () => {
  machine();
  release();
  assert.deepEqual(await update(), { result: "updated", version: "0.10.0" });
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
  // As if the machine went down after the files were swapped, before the new engine was started.
  fs.rmSync(path.join(home, "updates", "rollback", "engine-started"));
  assert.equal(spawnSync("sh", [path.join(home, "updates", "rollback", "restore.sh")], { env: process.env, timeout: 60_000 }).status, 0);
  assert.match(fs.readFileSync(path.join(home, "bin", "kuvo-start"), "utf8"), /^V=1\.0\.0$/m);
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.0.0", "the old engine runs again");
  engines.add(read(engineFile).pid);
});

test("the copy of the data from before an engine change is kept 30 days, then goes", async () => {
  machine();
  release();
  assert.deepEqual(await update(), { result: "updated", version: "0.10.0" });
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
  const backups = path.join(home, "updates", "backups");
  assert.equal(await settle(), "well");
  assert.equal(fs.readdirSync(backups).length, 1, "kept as the update's said to be well");
  link.dropOldBackups(Date.now() + 29 * 86_400_000);
  assert.equal(fs.readdirSync(backups).length, 1, "within 30 days");
  link.dropOldBackups(Date.now() + 31 * 86_400_000);
  assert.deepEqual(fs.readdirSync(backups), [], "past 30 days");
});

test("Update Now runs in a process of its own: the one that asked can go meanwhile, and the update goes through", async () => {
  machine();
  release();
  // The server that asked (a Mac's `serve`, a machine's bridge), killed just after it asked.
  const asker = path.join(scratch, "asker.mjs");
  fs.writeFileSync(asker, `import fs from "node:fs";
const link = await import(process.argv[2]);
fs.writeSync(1, JSON.stringify(link.startUpdate({ now: true, keys: [process.argv[3]], url: process.argv[4] })) + "\\n");
process.kill(process.pid, "SIGKILL");
`);
  const asked = spawnSync(process.execPath, [asker, script, pinned, `${base}/boxes.json`], { env: process.env, encoding: "utf8", timeout: 60_000 });
  assert.equal(asked.signal, "SIGKILL");
  assert.equal(asked.stdout.trim(), "true", "started");
  // (Its own process says it went well, and with no bridge to start again, confirms it itself.)
  await waitFor(() => fs.existsSync(path.join(home, "run", "update.json")) && read(path.join(home, "run", "update.json")).last?.to === "0.10.0", "the update went through", 600);
  assert.equal(boxVersion(), "0.10.0");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
});

test("a stopped box, or one whose version isn't known, doesn't update; a lock whose process went doesn't hold one", async () => {
  machine();
  release();
  fs.writeFileSync(path.join(home, "run", "stopped"), "");
  assert.match((await update()).message, /stopped/);
  fs.rmSync(path.join(home, "run", "stopped"));
  fs.rmSync(path.join(home, "bin", "box-version"));
  assert.match((await update()).message, /version isn't known/);
  write(path.join(home, "bin", "box-version"), "0.9.0");
  fs.mkdirSync(path.join(home, "run", ".updating"));
  fs.writeFileSync(path.join(home, "run", ".updating", "pid"), "999999");
  assert.equal(link.isUpdating(), false);
  assert.equal((await update()).result, "updated");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
  assert.equal(await settle(), "well");

  // An older Mac puts its own link back: seen twice (not a Mac's install under way), the box
  // doesn't update itself for a while, and says why; its version mark stays.
  const own = path.join(os.homedir(), ".kuvo-code");
  assert.equal(link.checkDrift(), false);
  // Not while an update tries what it put there (its version changes then).
  fs.writeFileSync(path.join(home, "run", "update-trial"), "0.11.0");
  fs.writeFileSync(path.join(home, "bin", "box-version"), "0.11.0");
  assert.equal(link.checkDrift(), false);
  assert.ok(fs.existsSync(path.join(home, "updates", "installed.json")), "what it installed is still noted");
  fs.writeFileSync(path.join(home, "bin", "box-version"), "0.10.0");
  fs.rmSync(path.join(home, "run", "update-trial"));
  fs.writeFileSync(path.join(home, "bin", "kuvo-link.js"), "// an older Mac's link\n");
  fs.writeFileSync(path.join(home, "run", "mac-installing"), "");
  assert.equal(link.checkDrift(), false, "not while a Mac says it puts its files in place");
  fs.rmSync(path.join(home, "run", "mac-installing"));
  assert.equal(link.checkDrift(), false, "seen once");
  assert.equal(link.checkDrift(), true, "seen again");
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.10.0");
  assert.match(link.updatesBlocked({}, Date.now(), own), /older Kuvo Code/);
  assert.match(read(path.join(home, "run", "update.json")).last.message, /older Kuvo Code/);
  fs.rmSync(path.join(home, "run", "launchers.json"), { force: true });

  // An older Mac connecting (it touches run/attached; this version never does) holds updates back.
  // One from before this version put its files there doesn't: that Mac, before it updated.
  const attached = path.join(home, "run", "attached"), mark = path.join(home, "bin", "box-version");
  assert.equal(link.noteOlderMacs(), false);
  fs.writeFileSync(attached, "");
  fs.utimesSync(mark, new Date(), new Date(Date.now() + 1000));
  assert.equal(link.noteOlderMacs(), false, "touched before the box was marked");
  assert.equal(link.updatesBlocked({}, Date.now(), own), null);
  fs.utimesSync(mark, new Date(), new Date(Date.now() - 60_000));
  assert.equal(link.noteOlderMacs(), true);
  assert.match(link.updatesBlocked({}, Date.now(), own), /older Kuvo Code/);
  fs.rmSync(path.join(home, "run", "attached"));
  // One a Mac's mark saw (and hid, marking the box), noted all the same.
  fs.rmSync(path.join(home, "run", "launchers.json"), { force: true });
  fs.writeFileSync(path.join(home, "run", "older-mac"), "");
  assert.equal(link.noteOlderMacs(), true);
  assert.match(link.updatesBlocked({}, Date.now(), own), /older Kuvo Code/);
  fs.rmSync(path.join(home, "run", "older-mac"));
  fs.rmSync(path.join(home, "run", "launchers.json"), { force: true });
});

test("an engine that won't stop changes nothing; no release for machines yet is nothing newer", async () => {
  machine();
  release();
  const running = read(engineFile).pid;
  const keeperFile = path.join(home, "bin", "kuvo-start");
  write(keeperFile, keeper("1.0.0").replace(/^stop\) .*$/m, "stop) exit 4 ;;"), 0o700);
  const outcome = await update();
  assert.match(outcome.message, /couldn't be stopped/);
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.9.0");
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false);
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false);
  assert.equal(read(engineFile).version, "1.0.0");
  assert.equal(read(engineFile).pid, running, "the engine it had, still running");
  served = {};
  assert.deepEqual(await update(), { result: "current", version: "0.9.0" });
});

test("an older keeper with an engine Kuvo Helper started (HOME written with a slash at its end): run as that engine was started, it stops it, and a stop that fails starts no second engine", async t => {
  // Kuvo Helper gave its keeper HOME "/Users/you/", so the engine runs as "…//.kuvo-code/engine/…".
  // An older keeper names its folder from HOME and knows its engine only by that very spelling:
  // run from Kuvo Code (HOME "/Users/you"), its stop stopped nothing (the update failed: "Its engine
  // couldn't be stopped to update it."), and its start started a second engine beside the first.
  machine();
  release();
  const above = path.dirname(home), name = path.basename(home);
  const realHome = process.env.HOME;
  process.env.HOME = above;
  t.after(() => { process.env.HOME = realHome; });
  const older = `#!/bin/sh
K="$KUVO_HOME"; R="$K/run"; O="$HOME/${name}"
noted() { sed -n 's/.*"pid":\\([0-9]*\\).*/\\1/p' "$R/engine.json" 2>/dev/null; }
mine() { [ -n "$1" ] && ps -ww -o command= -p "$1" 2>/dev/null | grep -q -- "$O/engine/"; }
case "$1" in
stop) [ -f "$K/once" ] || { touch "$K/once"; echo "A start of the engine is under way there." >&2; exit 4; }
  touch "$R/stopped"; pid=$(noted); if mine "$pid"; then kill "$pid"; while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done; echo "$pid" >>"$K/stopped-by-keeper"; fi; rm -f "$R/engine.json" ;;
start) rm -f "$R/stopped"; mine "$(noted)" && exit 0; echo started >>"$K/started"; "$O/engine/1.0.0/opencode" serve >/dev/null 2>&1 </dev/null & pid=$!; until [ "$(noted)" = "$pid" ]; do kill -0 "$pid" 2>/dev/null || exit 3; sleep 0.1; done ;;
esac
`;
  write(path.join(home, "bin", "kuvo-start"), older, 0o700);
  stopEngine();
  // (Started in the background of a shell that ends, as a keeper's launch does: not this test's child.)
  spawnSync("sh", ["-c", `"$1//${name}/engine/1.0.0/opencode" serve >/dev/null 2>&1 </dev/null &`, "sh", above], { env: process.env });
  await waitFor(() => fs.existsSync(engineFile), "the engine runs as the helper started it");
  const running = read(engineFile).pid;
  engines.add(running);
  assert.match(spawnSync("ps", ["-ww", "-o", "command=", "-p", String(running)], { encoding: "utf8" }).stdout, /\/\/[^/]+\/engine\/1\.0\.0\/opencode/);

  // Its stop fails (a start under way): nothing changed, and the engine it had is the one running.
  const failed = await update();
  assert.equal(failed.result, "failed");
  assert.match(failed.message, /couldn't be stopped for the update, so nothing changed/);
  assert.equal(fs.existsSync(path.join(home, "started")), false, "no second engine started");
  assert.equal(read(engineFile).pid, running);
  process.kill(running, 0);

  // Then its own stop stops it, and the update goes through.
  const outcome = await update();
  assert.deepEqual(outcome, { result: "updated", version: "0.10.0" });
  assert.equal(fs.readFileSync(path.join(home, "stopped-by-keeper"), "utf8").trim(), String(running), "the keeper's own stop");
  assert.throws(() => process.kill(running, 0), "the engine it had is stopped");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
});

test("a release that isn't what it says, an older one, or one too new for the night changes nothing", async () => {
  machine();
  release({ tamper: true });
  assert.match((await update()).message, /aren't what it says/);
  release({ key: crypto.generateKeyPairSync("ed25519").privateKey });
  assert.match((await update()).message, /isn't signed with a key/);
  release({ version: "0.8.0" });
  assert.equal((await update()).result, "current");
  release({ released: new Date() });
  assert.equal((await update({ now: false })).result, "waiting");
  assert.equal((await update({ keys: [] })).result, "off");
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8"), "0.9.0");
  assert.equal(read(engineFile).version, "1.0.0");
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false);
});

test("kuvo/update says how the box stands, and only reads and starts one", async () => {
  machine();
  const handler = link.engineHandler(() => read(engineFile), {}, () => ({}));
  const ask = async (method, body) => {
    const parts = [];
    for await (const part of handler({ method, path: "kuvo/update", headers: {}, body: Buffer.from(body ? JSON.stringify(body) : "") }, null, null)) parts.push(part);
    return { status: parts[0].head.status, json: JSON.parse(Buffer.concat(parts.slice(1).map(part => part.body)).toString("utf8")) };
  };
  const status = await ask("GET");
  assert.equal(status.status, 200);
  assert.deepEqual([status.json.setUp, status.json.version, status.json.engine], [true, "0.9.0", "1.0.0"]);
  assert.equal((await ask("PATCH", { auto: false })).status, 405, "there's nothing to change: a machine always updates by itself");
  const now = await ask("POST", { now: true });
  assert.equal(now.status, 409, "a test's folder, not a machine's own");
  assert.match(now.json.message, /own Kuvo Code folder/);
});

test("a machine with no bridge, updated by its `update` command: confirmed, and its integrations brought up to date", async () => {
  machine();
  release();
  const key = "KUVO_KEY_FIGMA_FIGMA_API_KEY";
  const catalog = fileURLToPath(new URL("../../protocol/vectors/routes/box/integrations.json", import.meta.url));
  write(path.join(home, "bin", "integrations.json"), fs.readFileSync(catalog));
  write(path.join(home, "box", "keys", key), "tok");
  write(path.join(home, "box", "opencode.json"), JSON.stringify({ mcp: { servers: {
    figma: { type: "local", command: ["kuvo-npx", "figma-developer-mcp@0.6.0", "--stdio"], environment: { FIGMA_API_KEY: `{file:${home}/box/keys/${key}}` } },
  } } }));
  const updating = spawn(process.execPath, [script, "update", "--now", "--url", `${base}/boxes.json`, "--key", pinned], { env: process.env, stdio: ["ignore", "pipe", "inherit"] });
  let said = "";
  updating.stdout.setEncoding("utf8");
  updating.stdout.on("data", chunk => { said += chunk; });
  // (Once its output has all come.)
  await new Promise(resolve => updating.once("close", resolve));
  assert.equal(JSON.parse(said).result, "updated", said);
  engines.add(read(engineFile).pid);
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false, "confirmed");
  const figma = read(catalog).integrations.find(entry => entry.id === "figma").settings;
  assert.deepEqual(read(path.join(home, "box", "opencode.json")).mcp.servers.figma, figma);
  assert.equal(fs.existsSync(path.join(home, "box", "keys", key)), false);
});

test("a release offered from a folder here is checked, kept, and taken at the next quiet moment, whatever its age", async () => {
  machine();
  // Out today: a downloaded one would wait three days for the night; one Kuvo Code here carries
  // came with the version it runs.
  const { folder } = release({ released: new Date() });
  assert.deepEqual(offer(folder), { status: 202, version: "0.10.0" });
  assert.equal(link.offeredVersion(), "0.10.0");
  assert.equal(link.updateStatus().offered, "0.10.0", "kuvo/update says it waits");
  // Kept: the app it came in may be gone (updated again, moved to the Trash) by the time it's taken.
  fs.rmSync(folder, { recursive: true });
  assert.equal(boxVersion(), "0.9.0", "nothing taken yet");

  assert.deepEqual(await take(), { result: "updated", version: "0.10.0" });
  assert.equal(boxVersion(), "0.10.0");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs");
  engines.add(read(engineFile).pid);
  assert.equal(fs.existsSync(offered), false, "taken, it goes");
  assert.equal(link.updateStatus().offered, undefined);
  assert.equal(await settle(), "well");
  assert.equal(read(path.join(home, "run", "update.json")).latest, undefined, "only GitHub's says what the newest release is");
  assert.equal(await take(), null, "nothing more to take");
});

test("an offered release is refused, with nothing kept or changed, when it isn't signed by a trusted key, isn't newer, or can't be read", async t => {
  machine();
  const refused = (given, status, words) => {
    const answer = offer(given);
    assert.equal(answer.status, status, JSON.stringify(answer));
    assert.match(answer.message, words);
    assert.equal(fs.existsSync(offered), false);
  };
  refused(release({ key: crypto.generateKeyPairSync("ed25519").privateKey }).folder, 422, /isn't signed with a key/);
  // A manifest changed after it was signed.
  const { folder: changed } = release();
  const given = read(path.join(changed, "boxes.json"));
  const manifest = Buffer.from(given.manifest, "base64").toString("utf8").replace("0.10.0", "0.99.0");
  write(path.join(changed, "boxes.json"), JSON.stringify({ ...given, manifest: Buffer.from(manifest).toString("base64") }));
  refused(changed, 422, /signature doesn't match/);
  refused(release({ version: "0.8.0" }).folder, 409, /has 0\.9\.0 already, and takes only a newer version/);
  // Its pack not what the signed manifest says, missing, or no release there at all.
  refused(release({ tamper: true }).folder, 422, /aren't what it says/);
  const { folder: packless } = release();
  fs.rmSync(path.join(packless, "pack.tgz"));
  refused(packless, 422, /can't be read/);
  refused(path.join(scratch, "nothing-here"), 422, /can't be read/);
  const { folder: notAFile } = release();
  fs.rmSync(path.join(notAFile, "boxes.json"));
  fs.mkdirSync(path.join(notAFile, "boxes.json"));
  refused(notAFile, 422, /can't be read/);
  write(path.join(scratch, "junk", "boxes.json"), "not a release");
  refused(path.join(scratch, "junk"), 422, /isn't a release/);
  refused("relative/folder", 400, /full path/);
  refused(undefined, 400, /full path/);
  // A box that doesn't update itself (not its own folder), or isn't set up to, takes none either.
  const { folder } = release();
  assert.equal(offer(folder, { blocked: "Only a machine's own Kuvo Code folder updates itself." }).status, 409);
  assert.equal(offer(folder, { keys: [] }).status, 409);
  // Nor while it updates: what's offered is what an update would be reading.
  fs.mkdirSync(path.join(home, "run", ".updating"));
  fs.writeFileSync(path.join(home, "run", ".updating", "pid"), String(await runningAs("kuvo-link", t)));
  refused(folder, 409, /updating already/);
  fs.rmSync(path.join(home, "run", ".updating"), { recursive: true });
  // An older one than what's offered already doesn't replace it.
  assert.equal(offer(release({ version: "0.11.0" }).folder).status, 202);
  const older = offer(folder);
  assert.equal(older.status, 409);
  assert.match(older.message, /newer release \(0\.11\.0\) waits/);
  assert.equal(link.offeredVersion(), "0.11.0");
  fs.rmSync(offered, { recursive: true });
  assert.equal(boxVersion(), "0.9.0");
  assert.equal(read(engineFile).version, "1.0.0", "its engine still runs");
});

test("a release of the box's own version from a folder here is taken when its files differ (an app rebuilt under its version), and is current when they're the same; an older one never", async () => {
  machine();
  // The app rebuilt under the version the box has: other files than those in place.
  const { folder } = release({ version: "0.9.0", released: new Date() });
  assert.deepEqual(offer(folder), { status: 202, version: "0.9.0" });
  assert.equal(link.offerWaits(), true, "it waits to be taken");
  assert.deepEqual(await take(), { result: "updated", version: "0.9.0" });
  engines.add(read(engineFile).pid);
  assert.equal(read(engineFile).version, "1.1.0", "the release's engine runs");
  assert.match(fs.readFileSync(path.join(home, "bin", "kuvo-start"), "utf8"), /^V=1\.1\.0$/m, "with the release's files");
  assert.equal(fs.existsSync(offered), false, "taken, it goes");
  assert.equal(await settle(), "well");

  // The same release offered again (the app reconnects), or taken from its folder: the files in
  // place are its own, so there's nothing to take.
  const again = offer(folder);
  assert.equal(again.status, 409);
  assert.match(again.message, /has 0\.9\.0 already, with these same files/);
  assert.equal(fs.existsSync(offered), false, "nothing kept");
  assert.equal(link.offerWaits(), false);
  assert.deepEqual(await update({ from: folder }), { result: "current", version: "0.9.0" });
  assert.equal(read(engineFile).version, "1.1.0", "its engine runs on, untouched");

  // An older one, never: offered, or from a folder.
  const { folder: older } = release({ version: "0.8.0" });
  assert.match(offer(older).message, /has 0\.9\.0 already, and takes only a newer version/);
  assert.deepEqual(await update({ from: older }), { result: "current", version: "0.9.0" });
  assert.equal(boxVersion(), "0.9.0");
});

test("an offered release waits while something runs, and is taken once nothing does", async t => {
  machine();
  const { folder } = release();
  assert.equal(offer(folder).status, 202);
  write(path.join(home, "run", "browser.pid"), String(await runningAs("kuvo-browser", t)));
  write(path.join(home, "run", "browser-asks.json"), JSON.stringify({ asks: [{ id: "ask_1", kind: "signIn", host: "github.com", session: "ses_1", username: true, password: true }] }));
  assert.equal(await take(), null, "not now");
  assert.equal(link.offeredVersion(), "0.10.0", "still offered");
  assert.equal(boxVersion(), "0.9.0");
  assert.equal(read(engineFile).version, "1.0.0", "its engine still runs");
  fs.rmSync(path.join(home, "run", "browser-asks.json"));
  // Nor while it's blocked, its engine stopped on purpose, or an update before is on trial (its
  // rollback would be lost): it waits, untouched, and says nothing of it.
  const last = fs.existsSync(path.join(home, "run", "update.json")) ? read(path.join(home, "run", "update.json")).last : undefined;
  assert.equal(await take({ blocked: "A Mac that connects to this machine has an older Kuvo Code" }), null);
  for (const mark of ["update-trial", "stopped"]) {
    fs.writeFileSync(path.join(home, "run", mark), "");
    assert.equal(await take(), null, mark);
    fs.rmSync(path.join(home, "run", mark));
  }
  assert.equal(link.offeredVersion(), "0.10.0");
  assert.deepEqual(fs.existsSync(path.join(home, "run", "update.json")) ? read(path.join(home, "run", "update.json")).last : undefined, last);
  assert.equal((await take()).result, "updated");
  engines.add(read(engineFile).pid);
  assert.equal(await settle(), "well");
});

test("Update Now takes the offered release whatever is at work, and this box's own row says how its update stands until then, with nothing to press while it takes it by itself", async t => {
  machine();
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 0, null), { actions: [] }, "nothing offered: nothing said");
  assert.equal(offer(release().folder).status, 202);
  assert.equal(link.offerWaits(), true);
  // Two sessions at work: it waits for them, by itself.
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 2, null), {
    update: { state: "waiting", title: "Kuvo on Studio Mac updates once your sessions there finish.", detail: "" },
    actions: [],
  });
  // No session counted at work: as soon as nothing is (a browser ask may wait).
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 0, null), {
    update: { state: "waiting", title: "Kuvo on Studio Mac updates as soon as nothing’s at work there.", detail: "" },
    actions: [],
  });
  // A box that can't update (an older Mac connects), or not even on your word now (its engine
  // stopped on purpose): nothing offered that couldn't be done.
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 0, "A Mac that connects to this machine has an older Kuvo Code"), { actions: [] });
  fs.writeFileSync(path.join(home, "run", "stopped"), "");
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 0, null), { actions: [] });
  assert.equal(link.offerHeld(), "Its engine is stopped: start it to update it.");
  fs.rmSync(path.join(home, "run", "stopped"));
  write(path.join(home, "run", "browser.pid"), String(await runningAs("kuvo-browser", t)));
  write(path.join(home, "run", "browser-asks.json"), JSON.stringify({ asks: [{ id: "ask_1", kind: "signIn", host: "github.com", session: "ses_1", username: true, password: true }] }));
  assert.equal(await take(), null, "by itself, not while something's at work");
  assert.deepEqual(await take({ now: true }), { result: "updated", version: "0.10.0" }, "on your word, now");
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.10.0");
  assert.equal(fs.existsSync(offered), false, "taken, it goes");
  assert.equal(link.offerWaits(), false);
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 2, null), { actions: [] });
  assert.equal(await settle(), "well");

  // One that keeps the engine running stops nothing: it's taken in a moment, whatever is at work.
  assert.equal(offer(release({ version: "0.11.0", engine: "1.1.0" }).folder).status, 202);
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 2, null), {
    update: { state: "waiting", title: "Kuvo on Studio Mac updates in a moment.", detail: "" },
    actions: [],
  });
  assert.deepEqual(await take(), { result: "updated", version: "0.11.0" }, "by itself, with something at work");
  assert.equal(await settle(), "well");
});

test("`kuvo-link.js update --installer` (the installer Kuvo Code carries) waits for nothing to be at work, saying so once, then updates; with --now it doesn't wait", async t => {
  machine();
  const run = (...args) => {
    const updating = spawn(process.execPath, [script, "update", "--installer", ...args, "--key", pinned], { env: { ...process.env, KUVO_TEST_UNTIL_FREE_EVERY: "100" }, stdio: ["ignore", "pipe", "inherit"] });
    const lines = [];
    let partial = "";
    const listeners = new Set();
    updating.stdout.setEncoding("utf8");
    updating.stdout.on("data", chunk => {
      partial += chunk;
      const whole = partial.split("\n");
      partial = whole.pop();
      for (const line of whole) if (line.trim()) { lines.push(JSON.parse(line)); for (const listener of listeners) listener(); }
    });
    const closed = new Promise(resolve => updating.once("close", code => resolve(code)));
    const said = step => new Promise(resolve => { const check = () => lines.some(line => line.step === step) && resolve(); listeners.add(check); check(); });
    return { lines, closed, said };
  };
  const asks = path.join(home, "run", "browser-asks.json");
  write(path.join(home, "run", "browser.pid"), String(await runningAs("kuvo-browser", t)));
  write(asks, JSON.stringify({ asks: [{ id: "ask_1", kind: "signIn", host: "github.com", session: "ses_1", username: true, password: true }] }));
  const { folder } = release({ released: new Date() });
  const waiting = run("--from", folder);
  await waiting.said("waiting");
  assert.equal(boxVersion(), "0.9.0", "nothing taken while something's at work");
  assert.equal(read(engineFile).version, "1.0.0", "its engine still runs");
  fs.rmSync(asks);
  assert.equal(await waiting.closed, 0);
  assert.deepEqual(waiting.lines, [
    { step: "waiting", title: "Kuvo on this Mac updates once your sessions finish.".replace("this Mac", process.platform === "darwin" ? "this Mac" : "this computer"), confirm: { title: `Update Kuvo on ${process.platform === "darwin" ? "this Mac" : "this computer"} now?`, message: `What your sessions on ${process.platform === "darwin" ? "this Mac" : "this computer"} are doing stops as Kuvo restarts. It’s back in a moment.`, label: "Update Now" } },
    { step: "updating", title: `Updating Kuvo on ${process.platform === "darwin" ? "this Mac" : "this computer"}…` },
    { result: "updated", version: "0.10.0" },
  ]);
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.10.0");
  // (Judged by a judge of its own, the installer's process gone: well, once it's said so.)
  await waitFor(() => !fs.existsSync(path.join(home, "run", "update-trial")), "the update judged well", 300);
  assert.equal(read(path.join(home, "run", "update.json")).last.to, "0.10.0");

  // A newer one, with something at work: --now takes it at once; and once the box has it, a run
  // that would wait has nothing to wait for.
  write(asks, JSON.stringify({ asks: [{ id: "ask_2", kind: "signIn", host: "github.com", session: "ses_1", username: true, password: true }] }));
  const newer = release({ version: "0.11.0" }).folder;
  const now = run("--from", newer, "--now");
  assert.equal(await now.closed, 0);
  assert.deepEqual(now.lines.map(line => line.step ?? line.result), ["updating", "updated"]);
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.11.0");
  await waitFor(() => !fs.existsSync(path.join(home, "run", "update-trial")), "the update judged well", 300);
  const again = run("--from", newer);
  assert.equal(await again.closed, 0);
  assert.deepEqual(again.lines, [{ result: "current", version: "0.11.0" }]);
  fs.rmSync(asks);
});

test("`kuvo-link.js update --installer` tries once more by itself, a little later, where a try failed before anything changed; only a second failure is said", async () => {
  machine();
  const { folder } = release();
  // (Not spawnSync: the release is served from this process.)
  const run = async () => {
    const ran = spawn(process.execPath, [script, "update", "--installer", "--now", "--from", folder, "--key", pinned], { env: { ...process.env, KUVO_TEST_FETCH_WAIT: "20" }, stdio: ["ignore", "pipe", "inherit"] });
    let said = "";
    ran.stdout.setEncoding("utf8");
    ran.stdout.on("data", chunk => { said += chunk; });
    await new Promise(resolve => ran.once("close", resolve));
    return said.split("\n").filter(line => line.trim()).map(line => JSON.parse(line));
  };
  // Its engine's download refused once (as GitHub might for a moment): the second try takes it.
  breaksOnce = { "/engine.tgz": 404 };
  assert.deepEqual((await run()).at(-1), { result: "updated", version: "0.10.0" });
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.10.0");

  // Its engine not stopped the first time (a start of it under way holds its keeper's lock): the
  // second try stops it, and nothing was said of the first.
  machine();
  const { folder: again } = release();
  const keeperFile = path.join(home, "bin", "kuvo-start");
  const enginesBefore = fetched["/engine.tgz"] ?? 0;
  write(keeperFile, keeper("1.0.0").replace(/^stop\) /m, `stop) [ -f "$R/once" ] || { touch "$R/once"; echo "A start of the engine is under way there." >&2; exit 4; }; `), 0o700);
  const stopping = await (async () => {
    const ran = spawn(process.execPath, [script, "update", "--installer", "--now", "--from", again, "--key", pinned], { env: { ...process.env, KUVO_TEST_FETCH_WAIT: "20" }, stdio: ["ignore", "pipe", "inherit"] });
    let said = "";
    ran.stdout.setEncoding("utf8");
    ran.stdout.on("data", chunk => { said += chunk; });
    await new Promise(resolve => ran.once("close", resolve));
    return said.split("\n").filter(line => line.trim()).map(line => JSON.parse(line));
  })();
  assert.deepEqual(stopping.filter(line => line.result), [{ result: "updated", version: "0.10.0" }], "only the try that went through is said");
  assert.equal(fetched["/engine.tgz"] - enginesBefore, 1, "its engine downloaded once: kept from the try that failed");
  engines.add(read(engineFile).pid);

  // A check that would fail again (what it got isn't what it says): said at once, not tried again.
  machine();
  const tampered = release({ version: "0.10.0", tamper: true }).folder;
  const checked = await (async () => {
    const ran = spawn(process.execPath, [script, "update", "--installer", "--now", "--from", tampered, "--key", pinned], { env: { ...process.env, KUVO_TEST_FETCH_WAIT: "20" }, stdio: ["ignore", "pipe", "inherit"] });
    let said = "";
    ran.stdout.setEncoding("utf8");
    ran.stdout.on("data", chunk => { said += chunk; });
    await new Promise(resolve => ran.once("close", resolve));
    return said.split("\n").filter(line => line.trim()).map(line => JSON.parse(line));
  })();
  assert.equal(checked.at(-1).result, "failed");
  assert.equal(checked.filter(line => line.step === "updating").length, 1, "tried once");

  // Refused every time: said once both tries failed, and nothing changed.
  machine();
  release({ version: "0.11.0" });
  delete served["/engine.tgz"];
  const failed = (await run()).at(-1);
  assert.equal(failed.result, "failed");
  assert.equal(boxVersion(), "0.9.0");
});

test("an offered release that doesn't come up is taken back and goes; one the box has caught up with goes untaken", async () => {
  machine();
  assert.equal(offer(release({ breaks: true }).folder).status, 202);
  assert.equal((await take()).result, "updated");
  assert.equal(await settle(), "unwell", "its keeper starts no engine");
  assert.equal(boxVersion(), "0.9.0");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs again, under the old code (a rollback puts back code only)");
  engines.add(read(engineFile).pid);
  assert.equal(fs.existsSync(offered), false, "not tried again and again");
  // Offered again as Kuvo Code next opens: not that version for a day, so a release that never
  // comes up here isn't tried at every open.
  const { folder: again } = release({ breaks: true });
  const rest = offer(again);
  assert.equal(rest.status, 409);
  assert.match(rest.message, /tried 0\.10\.0 lately/);
  assert.equal(offer(again, { clock: () => Date.now() + 86_400_000 + 1000 }).status, 202, "a day on");
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 0, null).actions, [], "once: nothing to press, it tries again by itself");
  fs.rmSync(offered, { recursive: true });

  // One whose new bridge never says it's well, taken back by the keeper twenty minutes on (the
  // trial on its way to it, the box back where it was): it rests a day too.
  write(path.join(home, "run", "update.json"), JSON.stringify({ trial: { from: "0.9.0", to: "0.12.0", at: Date.now(), healthy: true } }));
  assert.match(offer(release({ version: "0.12.0" }).folder).message, /tried 0\.12\.0 lately/);
  fs.rmSync(path.join(home, "run", "update.json"));

  // Offered, then the box got there another way (the night's update): nothing to take.
  assert.equal(offer(release({ version: "0.11.0" }).folder).status, 202);
  fs.writeFileSync(path.join(home, "bin", "box-version"), "0.11.0");
  assert.equal(await take(), null);
  assert.equal(fs.existsSync(offered), false);
});

test("a release put back after its trial twice isn't taken by itself again: this box's row says so with Update Now, though no offer waits", async () => {
  machine();
  // Taken, and put back after its trial (its keeper starts no engine).
  assert.equal(offer(release({ breaks: true }).folder).status, 202);
  assert.equal((await take()).result, "updated");
  assert.equal(await settle(), "unwell");
  await waitFor(() => fs.existsSync(engineFile), "an engine runs again");
  engines.add(read(engineFile).pid);
  assert.equal(read(path.join(home, "run", "update.json")).offerFailed.tries, 1);
  // A day on, the same version again (with a newer engine, so its broken keeper is what starts it),
  // taken and put back again: twice.
  const dayOn = { clock: () => Date.now() + 86_400_000 + 1000 };
  assert.equal(offer(release({ engine: "1.2.0", breaks: true }).folder, dayOn).status, 202);
  assert.equal((await take(dayOn)).result, "updated");
  assert.equal(await settle(), "unwell");
  assert.equal(boxVersion(), "0.9.0");
  try { engines.add(read(engineFile).pid); } catch {}
  assert.equal(read(path.join(home, "run", "update.json")).offerFailed.tries, 2);
  assert.equal(link.offerWaits(), false, "the offer went with the take");
  const stuck = link.keepingSelfUp("Studio Mac", 0, null);
  assert.deepEqual([stuck.update.state, stuck.update.title], ["failed", "Kuvo on Studio Mac couldn’t be updated to 0.10.0 after 2 tries."]);
  assert.deepEqual(stuck.actions.map(action => [action.id, action.label]), [["update-here", "Update Now"]]);
  // Offered again days later: kept, but not taken by itself; on your word, it is.
  const later = { clock: () => Date.now() + 3 * 86_400_000 };
  assert.equal(offer(release({ engine: "1.2.0", breaks: true }).folder, later).status, 202);
  assert.equal(await take(later), null, "not by itself after failing twice");
  assert.equal(await link.offerDue({ blocked: null, clock: later.clock, now: true }), true, "Update Now takes it");
  fs.rmSync(offered, { recursive: true });
});

const helper = { "Contents/Info.plist": "<plist>the new helper</plist>\n", "Contents/MacOS/Kuvo Helper": "#!/bin/sh\necho the new helper\n", "Contents/_CodeSignature/CodeResources": "the new seal\n" };
function oldHelper() {
  const bundle = path.join(home, "bin", "Kuvo Helper.app");
  write(path.join(bundle, "Contents", "Info.plist"), "<plist>the old helper</plist>\n");
  write(path.join(bundle, "Contents", "MacOS", "Kuvo Helper"), "#!/bin/sh\necho the old helper\n", 0o755);
  write(path.join(bundle, "Contents", "Resources", "only-the-old-one-had.txt"), "old\n");
  return bundle;
}

test("an offered release that fails before anything changes (its engine's download, offline) stays, and is tried again an hour on", async () => {
  machine();
  const { folder } = release();
  const engine = served["/engine.tgz"];
  delete served["/engine.tgz"];
  assert.equal(offer(folder).status, 202);
  const outcome = await take();
  assert.equal(outcome.result, "failed");
  assert.equal(read(engineFile).version, "1.0.0", "its engine never stopped");
  assert.equal(link.offeredVersion(), "0.10.0", "still offered");
  // kuvo/update says how the offered release went, in its own words, apart from the night's
  // (Kuvo Code here goes by it, #679).
  const said = link.updateStatus();
  assert.equal(said.offered, "0.10.0");
  assert.equal(said.offerFailed.version, "0.10.0");
  assert.equal(said.offerFailed.takenBack, false);
  assert.equal(said.offerFailed.message, outcome.message);
  // One failure that changed nothing isn't said as a failure: it's tried again by itself.
  assert.equal(link.keepingSelfUp("Studio Mac", 0, null).update.state, "waiting");
  assert.equal(link.keepingSelfUp("Studio Mac", 0, null).update.title, "Kuvo on Studio Mac tries its update again in a while.");
  assert.equal(await take(), null, "not again at once");
  assert.deepEqual(offer(folder), { status: 202, version: "0.10.0" }, "offered again meanwhile: it's there already");
  // Failing again an hour on (as one whose own checks fail would, every time), it rests a day.
  const at = hours => ({ clock: () => Date.now() + hours * 3_600_000 + 1000 });
  assert.equal((await take(at(1))).result, "failed");
  assert.equal(read(path.join(home, "run", "update.json")).offerFailed.tries, 2);
  // Failing again: what it can't get past by itself, said, with Update Now.
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 0, null).update, { state: "failed", title: "Kuvo on Studio Mac couldn’t be updated to 0.10.0 after 2 tries.", detail: "" });
  assert.deepEqual(link.keepingSelfUp("Studio Mac", 0, null).actions.map(action => action.label), ["Update Now"]);
  assert.equal(await take(at(2)), null, "not an hour on");
  assert.equal(link.offeredVersion(), "0.10.0");
  served["/engine.tgz"] = engine;
  // (Failed twice: not taken by itself again, only on your word.)
  assert.equal(await take(at(26)), null);
  assert.equal((await take({ ...at(26), now: true })).result, "updated");
  engines.add(read(engineFile).pid);
  assert.equal(fs.existsSync(offered), false);
  assert.equal(read(path.join(home, "run", "update.json")).offerFailed?.tries, 2, "kept while it's on trial: put back, it counts");
  assert.equal(await settle(), "well");
  assert.equal(read(path.join(home, "run", "update.json")).offerFailed, undefined, "forgotten once it's judged well");
});

test("a download that breaks off on the way (a dropped connection, a server failing) is tried again a moment later, and the update goes through", async t => {
  process.env.KUVO_TEST_FETCH_WAIT = "20";
  t.after(() => { delete process.env.KUVO_TEST_FETCH_WAIT; });
  machine();
  release();
  breaksOnce = { "/boxes.json": 503, "/pack.tgz": "drop", "/engine.tgz": "drop" };
  const outcome = await update();
  assert.equal(outcome.result, "updated", outcome.message);
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.10.0");
  assert.deepEqual(breaksOnce, {}, "each broke off once");
  assert.equal(await settle(), "well");

  // What's answered for good (it isn't there) isn't asked again: it fails as it did.
  machine();
  release({ version: "0.11.0" });
  delete served["/engine.tgz"];
  const missing = await update();
  assert.equal(missing.result, "failed");
  assert.equal(read(engineFile).version, "1.0.0", "its engine never stopped");
});

test("a release's bundle (the box's helper) is swapped whole on a Mac, and left out on Linux", async () => {
  machine();
  const bundle = oldHelper();
  assert.equal(offer(release({ bundle: helper }).folder).status, 202);
  assert.equal((await take()).result, "updated");
  engines.add(read(engineFile).pid);
  const installed = read(path.join(home, "updates", "installed.json")).files;
  // Never noted as installed: the helper's own keeper may put its own back, which isn't drift.
  assert.equal(Object.keys(installed).some(file => file.includes(".app")), false);
  if (process.platform === "darwin") {
    assert.equal(fs.readFileSync(path.join(bundle, "Contents", "MacOS", "Kuvo Helper"), "utf8"), helper["Contents/MacOS/Kuvo Helper"]);
    assert.equal(fs.statSync(path.join(bundle, "Contents", "MacOS", "Kuvo Helper")).mode & 0o777, 0o755, "with its own mode");
    assert.equal(fs.readFileSync(path.join(bundle, "Contents", "_CodeSignature", "CodeResources"), "utf8"), "the new seal\n");
    assert.equal(fs.existsSync(path.join(bundle, "Contents", "Resources")), false, "whole: nothing of the old one left in it");
  } else {
    assert.equal(fs.readFileSync(path.join(bundle, "Contents", "Info.plist"), "utf8"), "<plist>the old helper</plist>\n", "a Mac's alone");
  }
  assert.deepEqual(fs.readdirSync(path.join(home, "bin")).filter(name => name.includes(".app")), ["Kuvo Helper.app"], "no copy left beside it");
  assert.equal(await settle(), "well");
  assert.equal(link.checkDrift(), false, "what's there is what it installed");
});

test("a release whose bundle doesn't come up puts the bundle back as it was; one with a file box.json doesn't name, or a link, isn't taken", async () => {
  machine();
  const bundle = oldHelper();
  assert.equal(offer(release({ breaks: true, bundle: helper }).folder).status, 202);
  assert.equal((await take()).result, "updated");
  assert.equal(await settle(), "unwell");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs again, under the old code (a rollback puts back code only)");
  engines.add(read(engineFile).pid);
  assert.equal(fs.readFileSync(path.join(bundle, "Contents", "Info.plist"), "utf8"), "<plist>the old helper</plist>\n");
  assert.equal(fs.readFileSync(path.join(bundle, "Contents", "Resources", "only-the-old-one-had.txt"), "utf8"), "old\n");
  assert.equal(fs.statSync(path.join(bundle, "Contents", "MacOS", "Kuvo Helper")).mode & 0o777, 0o755);
  assert.deepEqual(fs.readdirSync(path.join(home, "bin")).filter(name => name.includes(".app")), ["Kuvo Helper.app"]);

  // A new bundle where there was none goes again, as it came. (Each offered again a day after the
  // one before didn't come up. Its engine the one running now, which nothing restarts: its new link
  // that answers nothing is what's unwell.)
  let day = 0;
  const later = () => ({ clock: (at => () => at)(Date.now() + ++day * 86_400_001) });
  fs.rmSync(bundle, { recursive: true });
  let when = later();
  assert.equal(offer(release({ linkBreaks: true, bundle: helper }).folder, when).status, 202);
  assert.equal((await take(when)).result, "updated");
  assert.equal(await settle(), "unwell");
  await waitFor(() => fs.existsSync(engineFile) && read(engineFile).version === "1.1.0", "the new engine runs again, under the old code (a rollback puts back code only)");
  engines.add(read(engineFile).pid);
  assert.equal(fs.existsSync(bundle), false);

  const odds = [
    [{ extra: { "bin/Kuvo Helper.app/Contents/MacOS/unnamed": "not in box.json" } }, /aren't what it says/],
    [{ link: true }, /aren't what it says/],
    [{ bundle: { ".hidden/x": "y" } }, /outside Kuvo’s folder/],
  ];
  for (const [odd, words] of odds) {
    when = later();
    // (Each a release of its own: what failed before doesn't hold this one back.)
    const state = read(path.join(home, "run", "update.json"));
    delete state.offerFailed;
    write(path.join(home, "run", "update.json"), JSON.stringify(state));
    assert.equal(offer(release({ bundle: helper, ...odd }).folder, when).status, 202, "the pack is as its signed manifest says");
    const running = read(engineFile).pid;
    const outcome = await take(when);
    assert.equal(outcome.result, "failed", JSON.stringify(odd));
    assert.match(outcome.message, words, JSON.stringify(odd));
    assert.equal(boxVersion(), "0.9.0");
    assert.equal(fs.existsSync(bundle), false);
    assert.equal(read(engineFile).pid, running, "its engine never stopped");
    // (Nothing changed: it stays, for an hour on. Each case here its own.)
    assert.equal(link.offeredVersion(), "0.10.0");
    fs.rmSync(offered, { recursive: true });
  }
});

test("kuvo/box/update takes an offer from Kuvo Code only, as a POST with a full path", async () => {
  machine();
  const handler = link.engineHandler(() => read(engineFile), {}, () => ({}));
  const ask = async (method, body, from = {}) => {
    const parts = [];
    for await (const part of handler({ method, path: "kuvo/box/update", headers: {}, body: Buffer.from(body ? JSON.stringify(body) : ""), ...from }, null, null)) parts.push(part);
    return { status: parts[0].head.status, json: JSON.parse(Buffer.concat(parts.slice(1).map(part => part.body)).toString("utf8") || "{}") };
  };
  const { folder } = release();
  assert.equal((await ask("GET")).status, 405);
  assert.equal((await ask("POST", { from: folder })).status, 403, "never a phone");
  assert.equal((await ask("POST", {}, { fromApp: true })).status, 400);
  // Kuvo Code here, or on another computer (`box route`), as far as the box's own rules let it.
  for (const from of [{ fromApp: true }, { fromMac: true }]) {
    const answer = await ask("POST", { from: folder }, from);
    assert.equal(answer.status, 409, "a test's folder, not a machine's own");
    assert.match(answer.json.message, /own Kuvo Code folder/);
  }
  assert.equal(fs.existsSync(offered), false);
});

test("`kuvo-link.js update --from` takes a release from a folder there", async () => {
  machine();
  const { folder } = release({ released: new Date(), bundle: helper });
  const updating = spawn(process.execPath, [script, "update", "--from", folder, "--key", pinned], { env: process.env, stdio: ["ignore", "pipe", "inherit"] });
  let said = "";
  updating.stdout.setEncoding("utf8");
  updating.stdout.on("data", chunk => { said += chunk; });
  await new Promise(resolve => updating.once("close", resolve));
  assert.equal(JSON.parse(said).result, "updated", said);
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.10.0");
  assert.equal(fs.existsSync(path.join(home, "bin", "Kuvo Helper.app")), process.platform === "darwin");
});

test("`kuvo-link.js update --offered` takes the offered release in a process of its own, and with no bridge to start again confirms it itself", async () => {
  machine();
  const cli = async (...args) => {
    const updating = spawn(process.execPath, [script, "update", ...args, "--key", pinned], { env: process.env, stdio: ["ignore", "pipe", "inherit"] });
    let said = "";
    updating.stdout.setEncoding("utf8");
    updating.stdout.on("data", chunk => { said += chunk; });
    await new Promise(resolve => updating.once("close", resolve));
    return JSON.parse(said);
  };
  assert.equal((await cli("--offered")).result, "current", "nothing offered: nothing to do");
  assert.equal(offer(release().folder).status, 202);
  assert.equal((await cli("--offered")).result, "updated");
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.10.0");
  assert.equal(fs.existsSync(offered), false);
  assert.equal(fs.existsSync(path.join(home, "run", "update-trial")), false, "confirmed, by itself");
  assert.equal(read(path.join(home, "run", "update.json")).last.to, "0.10.0");
});

// The box's sources as a checkout has them (scripts/box-release.sh, `BOX_SOURCES`), the keeper's
// start and the engine it names stand-ins as everywhere here (the real ones would start a real box):
// everything else the real files, written by the real builder.
const checkout = fileURLToPath(new URL("../..", import.meta.url));
// The browser as its build leaves it: a first line naming the rest.
const browserBody = "// kuvo-browser.js\n";
const builtBrowser = `// kuvo-browser ${sha256(browserBody).slice(0, 16)}\n${browserBody}`;
function sources() {
  const root = path.join(scratch, `sources-${crypto.randomUUID()}`);
  for (const file of ["box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json"]) {
    write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
  }
  write(path.join(root, "box/src/keeper/kuvo-start"), keeper("@ENGINE@"));
  write(path.join(root, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"), fs.readFileSync(script));
  write(path.join(root, "box/dist/kuvo-agents.js"), "// kuvo-agents.js\n");
  write(path.join(root, "box/dist/kuvo-browser.js"), builtBrowser);
  const integrity = "sha512-" + crypto.createHash("sha512").update(served["/engine.tgz"]).digest("base64");
  served["/engine-1.1.0.tgz"] = served["/engine.tgz"];
  const builds = { x86_64: { url: `${base}/engine-1.1.0.tgz`, integrity }, aarch64: { url: `${base}/engine-1.1.0.tgz`, integrity } };
  write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: "1.1.0", linux: builds, darwin: builds }));
  return root;
}
// Kuvo Helper as its build leaves it (scripts/build-helper.sh): on a Mac a program signed ad hoc,
// as packing checks the signature where codesign is.
function builtHelper() {
  const bundle = path.join(scratch, `helper-${crypto.randomUUID()}`, "Kuvo Helper.app");
  const program = path.join(bundle, "Contents", "MacOS", "Kuvo Helper");
  if (process.platform === "darwin") {
    write(path.join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Kuvo Helper</string><key>CFBundleIdentifier</key><string>run.kuvo.code.tests.box</string></dict></plist>\n`);
    fs.mkdirSync(path.dirname(program), { recursive: true });
    fs.copyFileSync("/usr/bin/true", program);
    fs.chmodSync(program, 0o755);
    const signed = spawnSync("codesign", ["--force", "--sign", "-", bundle], { encoding: "utf8" });
    assert.equal(signed.status, 0, signed.stderr);
  } else {
    for (const [name, text] of Object.entries(helper)) write(path.join(bundle, name), text, name.includes("MacOS/") ? 0o755 : 0o644);
  }
  return { bundle, program };
}
// The Bun the release's files are written with: the one named, the one on the PATH, or (run by
// Bun) this one.
const bun = process.env.BUN || process.env.KUVO_TEST_BUN || (process.versions.bun ? process.execPath : "bun");
const boxRelease = (env, ...args) => spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), ...args], {
  env: { ...process.env, BUN: bun, NODE: process.execPath, ...env }, encoding: "utf8",
});
const signingKey = privateKey.export({ type: "pkcs8", format: "pem" });

test("scripts/box-release.sh packs the box's release from its sources with its helper, signed, and a box takes it from a folder", async () => {
  machine();
  release();
  const root = sources();
  const { bundle, program } = builtHelper();
  // As an app carries it (`Contents/Resources/box-release`), or any folder.
  const out = path.join(scratch, `box-release-${crypto.randomUUID()}`);
  const pack = (...args) => boxRelease({ KUVO_BOX_SIGNING_KEY: signingKey, BOX_SOURCES: root }, ...args);
  const wrongURL = pack("0.10.0", "https://example.invalid/pack.tgz", out, bundle);
  assert.equal(wrongURL.status, 2, "a box finds the pack by the name its URL ends in");
  assert.equal(boxRelease({ BOX_SOURCES: root }, "0.10.0", "https://example.invalid/box-0.10.0.tar.gz", out, bundle).status, 2, "never unsigned");
  assert.equal(pack("ten", "https://example.invalid/box-ten.tar.gz", out, bundle).status, 1, "a version, or none");
  assert.equal(pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", out, path.join(scratch, "no-such.app")).status, 2, "each bundle named is there");
  assert.deepEqual(fs.readdirSync(out), [], "nothing left there by those");
  const packed = pack("0.10.0", "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz", out, bundle);
  assert.equal(packed.status, 0, packed.stderr);
  // (And the installer beside them, for an app that carries the release: install.test.js.)
  assert.deepEqual(fs.readdirSync(out).sort(), ["box-0.10.0.tar.gz", "boxes.json", "kuvo-install"]);
  assert.deepEqual(fs.readFileSync(path.join(out, "kuvo-install")), fs.readFileSync(path.join(checkout, "box/src/install/kuvo-install")));
  const manifest = link.verifyBoxes(fs.readFileSync(path.join(out, "boxes.json"), "utf8"), [pinned]);
  assert.equal(manifest.version, "0.10.0");
  assert.equal(manifest.engine, "1.1.0");
  assert.equal(manifest.pack.url, "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz");
  // What's packed: the helper under `bundles`, so a box from before bundles, which refuses a file
  // in `files` outside its own rule, still takes the release (leaving the helper out).
  const unpacked = path.join(scratch, `unpacked-${crypto.randomUUID()}`);
  fs.mkdirSync(unpacked);
  assert.equal(spawnSync("tar", ["xzf", path.join(out, "box-0.10.0.tar.gz"), "-C", unpacked]).status, 0);
  const packedBox = read(path.join(unpacked, "box.json"));
  const before = /^(bin\/[A-Za-z0-9._-]+|bin\/runtime\/[A-Za-z0-9._-]+|config\/opencode\/plugins\/[A-Za-z0-9._-]+)$/;
  assert.deepEqual(Object.keys(packedBox.files).filter(file => !before.test(file)), []);
  assert.deepEqual(Object.keys(packedBox.bundles["bin/Kuvo Helper.app"]).sort(), process.platform === "darwin" ? ["Contents/Info.plist", "Contents/MacOS/Kuvo Helper", "Contents/_CodeSignature/CodeResources"] : Object.keys(helper).sort());

  assert.deepEqual(offer(out), { status: 202, version: "0.10.0" });
  assert.equal((await take()).result, "updated");
  engines.add(read(engineFile).pid);
  assert.equal(boxVersion(), "0.10.0");
  assert.equal(read(engineFile).version, "1.1.0", "the engine its sources pin, started by the keeper they hold");
  assert.equal(fs.readFileSync(path.join(home, "bin", "kuvo-browser.js"), "utf8"), builtBrowser);
  // Everything a release brings, each as its sources have it and with its own mode.
  const modes = Object.fromEntries(Object.keys(packedBox.files).map(file => [file, (fs.statSync(path.join(home, file)).mode & 0o777).toString(8)]));
  assert.deepEqual(modes, {
    "bin/.kuvo-boot": "644", "bin/bot-words.json": "644", "bin/box-version": "644", "bin/integrations-mac.json": "644", "bin/integrations.json": "644",
    "bin/kuvo-agents.js": "644", "bin/kuvo-browser.js": "644", "bin/kuvo-launch": "700", "bin/kuvo-link.js": "600", "bin/kuvo-npx": "755", "bin/kuvo-setsid": "700",
    "bin/kuvo-start": "700", "bin/kuvo-wake": "755", "bin/mcp-catalog.json": "644", "bin/runtime/node": "755", "config/opencode/plugins/kuvo-code.js": "644",
  });
  assert.ok(fs.readFileSync(path.join(home, "bin", "kuvo-wake")).equals(fs.readFileSync(path.join(checkout, "box/src/keeper/kuvo-wake"))));
  assert.match(fs.readFileSync(path.join(home, "bin", "kuvo-npx"), "utf8"), /engine\/1\.1\.0\/opencode/, "the engine's version written into what names it");
  if (process.platform === "darwin") {
    assert.ok(fs.readFileSync(path.join(home, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper")).equals(fs.readFileSync(program)));
    const verified = spawnSync("codesign", ["--verify", "--strict", "--deep", path.join(home, "bin", "Kuvo Helper.app")], { encoding: "utf8" });
    assert.equal(verified.status, 0, `signed as it was, in the box: ${verified.stderr}`);
  }
  assert.equal(await settle(), "well");

  // A bundle a box couldn't take (a link in it) isn't packed.
  fs.symlinkSync("/etc/hosts", path.join(bundle, "Contents", "hosts"));
  const linked = pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", path.join(scratch, "never"), bundle);
  assert.equal(linked.status, 1);
  assert.match(linked.stderr, /can't take .*hosts/);
  fs.rmSync(path.join(bundle, "Contents", "hosts"));

  // One whose signature doesn't hold once packed (here, changed after it was signed) isn't packed.
  if (process.platform === "darwin") {
    fs.appendFileSync(path.join(bundle, "Contents", "Info.plist"), "\n");
    const broken = pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", path.join(scratch, "never"), bundle);
    assert.equal(broken.status, 1);
    assert.match(broken.stderr, /Kuvo Helper\.app isn't signed as it was/);
  }
  assert.deepEqual(fs.readdirSync(path.join(scratch, "never")), [], "nothing left there");

  // The same files pack to the same bytes, wherever your temporary folder is: one named with a
  // last slash (a Mac's own is) stays as private as it was, and nothing above the pack is touched.
  // (The helper as it was signed again, where the line added above changed it.)
  if (process.platform === "darwin") fs.writeFileSync(path.join(bundle, "Contents", "Info.plist"), fs.readFileSync(path.join(bundle, "Contents", "Info.plist"), "utf8").replace(/\n$/, ""));
  const again = path.join(scratch, `again-${crypto.randomUUID()}`), temporary = path.join(scratch, `tmp-${crypto.randomUUID()}`);
  fs.mkdirSync(temporary, { mode: 0o700 });
  const repacked = boxRelease({ KUVO_BOX_SIGNING_KEY: signingKey, BOX_SOURCES: root, TMPDIR: `${temporary}/` }, "0.10.0", "https://example.invalid/releases/download/v0.10.0/box-0.10.0.tar.gz", again, bundle);
  assert.equal(repacked.status, 0, repacked.stderr);
  assert.equal((fs.statSync(temporary).mode & 0o777).toString(8), "700", "your temporary folder as it was");
  assert.deepEqual(fs.readdirSync(temporary), [], "and nothing left in it");
  assert.equal(sha256(fs.readFileSync(path.join(again, "box-0.10.0.tar.gz"))), sha256(fs.readFileSync(path.join(out, "box-0.10.0.tar.gz"))), "the same pack, byte for byte");

  // A bundle with a file whose name is too long to pack isn't signed short of it.
  const long = path.join(bundle, "Contents", "Resources", `${"n".repeat(120)}.txt`);
  write(long, "x\n");
  const short = pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", path.join(scratch, "never"), bundle);
  assert.notEqual(short.status, 0, "a pack missing a file box.json names is never made");
  fs.rmSync(path.dirname(long), { recursive: true });

  // A test's sources never make a pack for a place something is published at.
  for (const url of ["https://github.com/example/example/releases/download/v0.10.0/box-0.10.0.tar.gz", "https://github.com/example/example.invalid/box-0.10.0.tar.gz"]) {
    const published = pack("0.10.0", url, path.join(scratch, "never"), bundle);
    assert.equal(published.status, 2, url);
    assert.match(published.stderr, /BOX_SOURCES is for tests/);
  }
  // A browser that isn't what its first line says (changed by hand, cut short) isn't packed.
  fs.appendFileSync(path.join(root, "box/dist/kuvo-browser.js"), "// changed\n");
  const edited = pack("0.10.0", "https://example.invalid/box-0.10.0.tar.gz", path.join(scratch, "never"), bundle);
  assert.equal(edited.status, 1);
  assert.match(edited.stderr, /kuvo-browser\.js isn't what its first line says/);
  write(path.join(root, "box/dist/kuvo-browser.js"), builtBrowser);
  assert.deepEqual(fs.readdirSync(path.join(scratch, "never")), [], "nothing left there");

  // A signing that fails leaves the release that was there whole: never a new pack beside the
  // old boxes.json, nor one with none.
  const kept = Object.fromEntries(fs.readdirSync(out).map(name => [name, sha256(fs.readFileSync(path.join(out, name)))]));
  const unsigned = boxRelease({ KUVO_BOX_SIGNING_KEY: "not a key", BOX_SOURCES: root }, "0.10.0", "https://example.invalid/box-0.10.0.tar.gz", out);
  assert.notEqual(unsigned.status, 0);
  assert.deepEqual(Object.fromEntries(fs.readdirSync(out).map(name => [name, sha256(fs.readFileSync(path.join(out, name)))])), kept);
});

test("the release built from this checkout's own sources is whole: every file a box runs, as box.json names it, for the pinned engine", () => {
  const folder = path.join(scratch, `files-${crypto.randomUUID()}`);
  const written = boxRelease({}, "--files", "1.2.3-beta.4", folder);
  assert.equal(written.status, 0, written.stderr);
  const box = read(path.join(folder, "box.json"));
  const pin = read(path.join(checkout, "box/src/shared/engine-pin.json"));
  assert.deepEqual([box.version, box.engine, box.linux, box.darwin], ["1.2.3-beta.4", pin.version, pin.linux, pin.darwin]);
  // Named: each file there but box.json itself, by what it holds, and nothing a box wouldn't take.
  const there = fs.readdirSync(folder, { recursive: true, encoding: "utf8" }).filter(file => fs.statSync(path.join(folder, file)).isFile() && file !== "box.json").sort();
  assert.deepEqual(Object.keys(box.files), there);
  for (const file of there) assert.equal(box.files[file], sha256(fs.readFileSync(path.join(folder, file))), file);
  assert.deepEqual(there, [
    "bin/.kuvo-boot", "bin/bot-words.json", "bin/box-version", "bin/integrations-mac.json", "bin/integrations.json", "bin/kuvo-agents.js", "bin/kuvo-browser.js",
    "bin/kuvo-launch", "bin/kuvo-link.js", "bin/kuvo-npx", "bin/kuvo-setsid", "bin/kuvo-start", "bin/kuvo-wake", "bin/mcp-catalog.json", "bin/runtime/node",
    "config/opencode/plugins/kuvo-code.js",
  ]);
  const text = file => fs.readFileSync(path.join(folder, file), "utf8");
  assert.equal(text("bin/box-version"), "1.2.3-beta.4");
  // The keeper's files read, and run this engine: its version where each names it, never the mark it's written over.
  for (const file of ["bin/kuvo-start", "bin/kuvo-launch", "bin/kuvo-setsid", "bin/kuvo-wake", "bin/kuvo-npx", "bin/runtime/node"]) {
    assert.equal(spawnSync("sh", ["-n", path.join(folder, file)]).status, 0, `${file} reads`);
    assert.doesNotMatch(text(file), /@ENGINE@/, file);
  }
  assert.ok(text("bin/kuvo-start").includes(`\nV=${pin.version}\n`));
  assert.ok(text("bin/kuvo-launch").includes(`V=\${KUVO_VERSION:-${pin.version}}`));
  for (const file of ["bin/kuvo-npx", "bin/runtime/node"]) assert.ok(text(file).includes(`/.kuvo-code/engine/${pin.version}/opencode"`), file);
  // What says which keeper files a box has: their names and bytes, in the order a Mac wrote them.
  assert.equal(text("bin/.kuvo-boot"), sha256(["kuvo-launch", "kuvo-start", "kuvo-setsid"].map(name => `${name}\n${text(`bin/${name}`)}`).join("\n")));
  // The bridge, as built, then the line naming it.
  const bridge = fs.readFileSync(path.join(checkout, "box/dist/kuvo-agents.js"));
  assert.equal(text("bin/kuvo-agents.js"), `${bridge}\n// kuvo-agents ${sha256(bridge).slice(0, 16)}\n`);
  assert.ok(fs.readFileSync(path.join(folder, "bin/kuvo-link.js")).equals(fs.readFileSync(script)));
  // The catalogs: a machine's without what uses a Mac's own apps, a Mac's box's with it, each key
  // named where a box keeps it.
  const machines = read(path.join(folder, "bin/integrations.json")).integrations, macs = read(path.join(folder, "bin/integrations-mac.json")).integrations;
  const catalog = read(path.join(folder, "bin/mcp-catalog.json"));
  assert.deepEqual(macs.filter(each => each.macOnly).map(each => each.id), ["apple-calendar"]);
  assert.deepEqual(machines, macs.filter(each => !each.macOnly));
  assert.deepEqual(catalog.macOnly, ["apple-calendar", "chrome-devtools"]);
  const github = machines.find(each => each.id === "github");
  assert.deepEqual([github.variables, github.settings.headers], [{ TOKEN: "KUVO_KEY_GITHUB_TOKEN" }, { Authorization: "Bearer {file:~/.kuvo-code/box/keys/KUVO_KEY_GITHUB_TOKEN}" }]);
  for (const each of macs) assert.equal(each.settings === undefined, each.way === "browser", `${each.id}: a server unless it's used on its site`);
  assert.deepEqual(catalog.servers.filter(each => each.integration).map(each => each.id), ["linear", "notion", "atlassian"]);
  assert.equal(read(path.join(folder, "bin/bot-words.json")).kuvo, 1);
});
