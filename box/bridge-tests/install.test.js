// A computer's first box, installed from the release an app carries (PROTOCOL.md, "Installing this
// computer's box"; #713 piece 3): `kuvo-install`, as scripts/box-release.sh writes it beside
// `boxes.json` and the pack, run as the app runs it, in a home of the test's own. The release is
// packed here by the real script from this checkout's link as built; only its engine is a stand-in
// (it says its version, and runs the link's script as the engine's Bun does), its keeper's start
// script (it notes what it's asked, and starts nothing), and on a Mac Kuvo Helper (a program signed
// ad hoc) with stand-ins for `launchctl`, `crontab` and `open`: never this computer's launchd or
// crontab, nor anything fetched from the network (the engine's "download" is a file here).
// Run with `node --test box/bridge-tests/install.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { removeAtEnd } from "./at-end.js";

const mac = process.platform === "darwin";
const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const scratch = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-install-")));
removeAtEnd(scratch);

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
}
const read = file => fs.readFileSync(file, "utf8");
const exists = file => fs.existsSync(file);
const modeOf = file => (fs.statSync(file).mode & 0o777).toString(8);

// The engine's binary: its version, and the link's script run as the engine's Bun runs it (here
// this Node). Each run of it is noted, to tell a download from one that's there already.
const ENGINE = "1.1.0", VERSION = "0.10.0";
const engineBinary = `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo ${ENGINE}; exit 0 ;;
*.js) exec '${process.execPath}' "$@" ;;
esac
exit 64
`;
const engineTar = path.join(scratch, `engine-${ENGINE}.tgz`);
write(path.join(scratch, "engine", "package", "bin", "opencode"), engineBinary, 0o755);
assert.equal(spawnSync("tar", ["czf", engineTar, "-C", path.join(scratch, "engine"), "package"], { env: { ...process.env, COPYFILE_DISABLE: "1" } }).status, 0);
const engineBytes = fs.readFileSync(engineTar);

// The release's sources: this checkout's own, with the engine above pinned (fetched from a file)
// and a start script that only notes what it's asked.
const sources = path.join(scratch, "sources");
for (const file of ["box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js", "box/dist/kuvo-agents.js"]) {
  write(path.join(sources, file), fs.readFileSync(path.join(checkout, file)));
}
write(path.join(sources, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"), fs.readFileSync(script));
write(path.join(sources, "box/src/keeper/kuvo-start"), `#!/bin/sh\n# A stand-in for the keeper, for engine @ENGINE@: it notes what it's asked.\necho "$*" >>"$HOME/keeper-asked"\nif [ -f "$HOME/keeper-fails" ]; then mkdir -p "$HOME/.kuvo-code/box/keys" "$HOME/.kuvo-code/run"; echo '{"mac":"${"A".repeat(43)}","macs":["${"A".repeat(43)}"],"phones":[]}' >"$HOME/.kuvo-code/run/link.json"; echo "its engine didn't come up" >"$HOME/.kuvo-code/run/engine.log"; exit 3; fi\n`);
const build = { url: pathToFileURL(engineTar).href, integrity: "sha512-" + crypto.createHash("sha512").update(engineBytes).digest("base64") };
write(path.join(sources, "box/src/shared/engine-pin.json"), JSON.stringify({ version: ENGINE, linux: { x86_64: build, aarch64: build }, darwin: { x86_64: build, aarch64: build } }));

// Kuvo Helper as its build leaves it: on a Mac a program signed ad hoc (packing checks it).
const helper = path.join(scratch, "helper", "Kuvo Helper.app");
if (mac) {
  write(path.join(helper, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleExecutable</key><string>Kuvo Helper</string><key>CFBundleIdentifier</key><string>run.kuvo.code.tests.box</string></dict></plist>\n`);
  fs.mkdirSync(path.join(helper, "Contents", "MacOS"), { recursive: true });
  fs.copyFileSync("/usr/bin/true", path.join(helper, "Contents", "MacOS", "Kuvo Helper"));
  fs.chmodSync(path.join(helper, "Contents", "MacOS", "Kuvo Helper"), 0o755);
  const signed = spawnSync("codesign", ["--force", "--sign", "-", helper], { encoding: "utf8" });
  assert.equal(signed.status, 0, signed.stderr);
}

// Packed once, as a release is, into the folder an app carries.
const release = path.join(scratch, "box-release");
{
  const { privateKey } = crypto.generateKeyPairSync("ed25519");
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), VERSION, `https://example.invalid/releases/download/v${VERSION}/box-${VERSION}.tar.gz`, release, ...(mac ? [helper] : [])], {
    env: { ...process.env, BUN: process.env.BUN || process.env.KUVO_TEST_BUN || (process.versions.bun ? process.execPath : "bun"), NODE: process.execPath, BOX_SOURCES: sources, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }) }, encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);
}

const APP_ID = "run.kuvo.code.trial713";
let count = 0;
/** A computer of the test's own: its home folder, the app's bundle, and stand-ins for what starts
 * a box from outside its folder, each noting what it's asked. */
function computer({ app = APP_ID } = {}) {
  const home = path.join(scratch, `home-${++count}`);
  const tools = path.join(home, "tools");
  write(path.join(tools, "launchctl"), `#!/bin/sh\necho "$*" >>"${home}/launchctl-asked"\ncase "$1" in print) [ -f "${home}/loaded" ] ;; bootstrap) if [ -f "${home}/launchd-refuses" ]; then echo "Bootstrap failed: 5: Input/output error" >&2; exit 5; fi; touch "${home}/loaded" ;; bootout) rm -f "${home}/loaded" ;; print-disabled) exit 0 ;; esac\n`, 0o755);
  write(path.join(tools, "crontab"), `#!/bin/sh\nif [ "$1" = -l ]; then [ -f "${home}/crontab" ] && cat "${home}/crontab" || { echo "no crontab for you" >&2; exit 1; }; else cat >"${home}/crontab"; fi\n`, 0o755);
  write(path.join(tools, "open"), `#!/bin/sh\necho "$*" >>"${home}/open-asked"\n`, 0o755);
  const bundle = path.join(home, "Applications", "Kuvo Code.app");
  write(path.join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${app}</string></dict></plist>\n`);
  return { home, tools, bundle, K: path.join(home, ".kuvo-code"), S: path.join(home, "Library", "Application Support", app) };
}
/** `sh kuvo-install <verb>`, as the app runs it from the release it carries: its lines, the last
 * one its answer. A KUVO_ variable of the app's own never reaches the box's commands. */
function install(where, verb, { from = release, args = mac ? ["--app", where.bundle] : [], env = {} } = {}) {
  const ran = spawnSync("sh", [path.join(from, "kuvo-install"), verb, ...args], {
    env: { PATH: process.env.PATH, HOME: where.home, TMPDIR: process.env.TMPDIR ?? os.tmpdir(), KUVO_TEST_UNINSTALL_TOOLS: where.tools, KUVO_TEST_INSTALL_FILE_ENGINE: "1", KUVO_BOX: "mac", KUVO_HOME: "/nowhere", ...env },
    encoding: "utf8", timeout: 240_000,
  });
  const lines = ran.stdout.trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
  return { status: ran.status, lines, steps: lines.filter(line => line.step).map(line => line.step), said: lines[lines.length - 1], stderr: ran.stderr };
}
const staging = where => path.join(where.K, "updates", "install");
const everything = folder => (exists(folder) ? fs.readdirSync(folder, { recursive: true, encoding: "utf8" }).sort() : []);

test("status on a new computer: the pack checked and unpacked, the release's engine fetched once and checked, and the verdict `fresh`", () => {
  const where = computer();
  const first = install(where, "status");
  assert.equal(first.status, 0, first.stderr + JSON.stringify(first.lines));
  assert.deepEqual(first.steps, ["checking", "engine"]);
  // (Each in words an app shows as they are.)
  assert.deepEqual(first.lines.filter(line => line.step).map(line => line.title), ["Getting ready", "Downloading the engine"]);
  assert.equal(first.said.verdict, "fresh");
  assert.equal(first.said.verb, "new");
  assert.equal(first.said.status.state, null, "no move here");
  // What it staged, in the box's home alone: the release whole, and the engine by its version.
  assert.equal(read(path.join(staging(where), "release", "bin", "box-version")), VERSION);
  assert.equal(read(path.join(staging(where), "engine", ENGINE, "opencode")), engineBinary);
  assert.equal(modeOf(where.K), "700");
  assert.deepEqual(fs.readdirSync(where.K), ["updates"], "no box made by asking");
  assert.deepEqual(fs.readdirSync(where.home).sort(), [".kuvo-code", "Applications", "tools"], "nothing else in the home folder");
  // Asked again (each time the app opens): nothing unpacked or fetched again.
  const again = install(where, "status");
  assert.equal(again.status, 0, again.stderr);
  assert.deepEqual(again.steps, []);
  assert.equal(again.said.verdict, "fresh");
});

test("two installers at once on a new computer each get their answer: neither takes the other's unpacking or download away", async () => {
  const where = computer();
  const run = () => new Promise(resolve => {
    const child = spawn("sh", [path.join(release, "kuvo-install"), "status", ...(mac ? ["--app", where.bundle] : [])], {
      env: { PATH: process.env.PATH, HOME: where.home, TMPDIR: process.env.TMPDIR ?? os.tmpdir(), KUVO_TEST_UNINSTALL_TOOLS: where.tools, KUVO_TEST_INSTALL_FILE_ENGINE: "1" }, stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    child.stdout.on("data", chunk => { out += chunk; });
    child.once("exit", status => resolve({ status, said: JSON.parse(out.trim().split("\n").pop() || "{}") }));
  });
  const all = await Promise.all([run(), run(), run(), run()]);
  for (const each of all) assert.deepEqual({ status: each.status, verdict: each.said.verdict }, { status: 0, verdict: "fresh" }, JSON.stringify(each.said));
  assert.deepEqual(fs.readdirSync(staging(where)).sort(), ["engine", "release"], "no scratch copy left");
  assert.equal(fs.readdirSync(path.join(staging(where), "release")).some(name => name.startsWith("release")), false, "nor one inside the other");
  assert.deepEqual(fs.readdirSync(path.join(staging(where), "engine")), [ENGINE]);
  assert.equal(read(path.join(staging(where), "release", "bin", "box-version")), VERSION);
});

test("an engine named as a file is only ever a test's: without a test's say-so the release is refused as damaged, and no engine is taken", () => {
  const where = computer();
  const refused = install(where, "status", { env: { KUVO_TEST_INSTALL_FILE_ENGINE: "" } });
  assert.equal(refused.status, 1);
  assert.deepEqual(refused.said, { result: "failed", message: "This copy of Kuvo Code is damaged. Download it again, then try again." });
  assert.deepEqual(everything(path.join(staging(where), "engine")), []);
});

test("a damaged copy is refused in words before anything of it runs: a pack that isn't what boxes.json says, an engine that isn't what the release says", () => {
  // The pack, changed after it was signed.
  const damaged = path.join(scratch, "damaged-release");
  fs.cpSync(release, damaged, { recursive: true });
  fs.appendFileSync(path.join(damaged, `box-${VERSION}.tar.gz`), "x");
  const one = computer();
  const refused = install(one, "status", { from: damaged });
  assert.equal(refused.status, 1);
  assert.deepEqual(refused.said, { result: "failed", message: "This copy of Kuvo Code is damaged. Download it again, then try again." });
  assert.deepEqual(everything(staging(one)), [], "nothing unpacked");

  // The engine, other bytes than the release pinned.
  const two = computer();
  fs.writeFileSync(engineTar, Buffer.concat([engineBytes, Buffer.from("x")]));
  try {
    const wrong = install(two, "status");
    assert.equal(wrong.status, 1);
    assert.deepEqual(wrong.said, { result: "failed", message: "The engine that was downloaded isn't what this Kuvo Code's release says. Try again." });
    assert.deepEqual(everything(path.join(staging(two), "engine")), [], "no engine kept, nor what was fetched");
    // And one that can't be had (offline): said so, to try again.
    fs.rmSync(engineTar);
    const offline = install(two, "status");
    assert.equal(offline.status, 1);
    assert.match(offline.said.message, /^The engine couldn't be downloaded\. Check that this (Mac|computer) is online, then try again\.$/);
  } finally {
    fs.writeFileSync(engineTar, engineBytes);
  }
  // Tried again once it can be: it goes through.
  assert.equal(install(two, "status").said.verdict, "fresh");
});

test("beside a box that's here already, nothing is made, and the engine the installer fetched goes once that box has it itself", () => {
  const where = computer();
  assert.equal(install(where, "status").said.verdict, "fresh");
  // A machine's box set up here meanwhile, as another computer leaves it, with the same engine.
  write(path.join(where.K, "bin", "kuvo-start"), "#!/bin/sh\n", 0o700);
  write(path.join(where.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }));
  write(path.join(where.K, "engine", ENGINE, "opencode"), engineBinary, 0o755);
  const status = install(where, "status");
  assert.equal(status.status, 0, status.stderr);
  assert.equal(status.said.verdict, "machine");
  assert.equal(status.said.verb, null);
  assert.equal(exists(path.join(staging(where), "engine")), false, "one copy of the engine, the box's own");
  // Asked again, on the box's own engine: nothing fetched.
  const again = install(where, "status");
  assert.deepEqual(again.steps, []);
  assert.equal(again.said.verdict, "machine");
  assert.equal(exists(path.join(staging(where), "engine")), false);
  const refused = install(where, "new");
  assert.equal(refused.status, 1);
  assert.equal(refused.said.result, "refused");
  assert.equal(refused.said.message, "This computer is set up as one of your machines already: Kuvo Code uses what's here.");
  assert.equal(read(path.join(where.K, "bin", "kuvo-start")), "#!/bin/sh\n", "its files as they were");
});

test("a computer whose box has another engine than the release's is still told how it stands, offline: asking never waits on a download", () => {
  const where = computer();
  write(path.join(where.K, "bin", "kuvo-start"), "#!/bin/sh\n", 0o700);
  write(path.join(where.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }));
  write(path.join(where.K, "engine", "1.0.0", "opencode"), engineBinary, 0o755);
  fs.rmSync(engineTar);
  try {
    const status = install(where, "status");
    assert.equal(status.status, 0, status.stderr + JSON.stringify(status.lines));
    assert.deepEqual(status.steps, ["checking"], "nothing fetched");
    assert.equal(status.said.verdict, "machine");
    assert.equal(exists(path.join(staging(where), "engine")), false);
  } finally {
    fs.writeFileSync(engineTar, engineBytes);
  }
});

test("new on a Mac: this Mac's own box made in its home from the release and its engine, started by Kuvo Helper, the installer's staging gone", { skip: !mac && "a Mac's box" }, () => {
  const where = computer();
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.equal(made.said.result, "done");
  assert.equal(made.said.verdict, "ours");
  assert.equal(made.said.started, true);
  // Its files from the release, its helper whole, the release's engine, and its kind: every file of
  // the release, as its box.json says, the engine's plugin outside `bin` too (which gives the
  // commands agents run your own folders back).
  const box = JSON.parse(read(path.join(scratch, "unpacked-box.json")));
  for (const [name, sum] of Object.entries(box.files)) {
    assert.equal(crypto.createHash("sha256").update(fs.readFileSync(path.join(where.K, name))).digest("hex"), sum, name);
  }
  assert.equal(read(path.join(where.K, "bin", "box-version")), VERSION);
  assert.deepEqual(fs.readFileSync(path.join(where.K, "bin", "kuvo-link.js")), fs.readFileSync(script));
  assert.equal(spawnSync("codesign", ["--verify", "--strict", path.join(where.K, "bin", "Kuvo Helper.app")]).status, 0, "the helper, signed as it was");
  assert.equal(read(path.join(where.K, "engine", ENGINE, "opencode")), engineBinary);
  assert.equal(read(path.join(where.K, "box", "kind")), "mac\n");
  // Started at login and now: its launch agent names the helper in this home, under the app.
  const plist = read(path.join(where.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist"));
  assert.ok(plist.includes(`<string>${path.join(where.K, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper")}</string><string>keep</string><string>--home</string><string>${where.K}</string>`), plist);
  assert.ok(plist.includes(`<key>AssociatedBundleIdentifiers</key><array><string>${APP_ID}</string></array>`), plist);
  assert.match(read(path.join(where.home, "launchctl-asked")), /^bootstrap gui\/\d+ .*run\.kuvo\.code\.machine\.plist$/m);
  // Nothing of the installer's left, and the old app's folder never made.
  assert.equal(exists(staging(where)), false);
  assert.equal(exists(where.S), false);
  // From now on: ours, with nothing to run; asked to make it again, nothing.
  const status = install(where, "status");
  assert.equal(status.said.verdict, "ours");
  assert.equal(status.said.verb, null);
  const again = install(where, "new");
  assert.equal(again.status, 0, again.stderr);
  assert.equal(again.said.result, "nothing");
  assert.equal(read(path.join(where.K, "box", "kind")), "mac\n");
});

test("a Mac's install cut short is made again: what a making left, and a box made whose helper never got its launch agent", { skip: !mac && "a Mac's box" }, () => {
  // Cut short while its files went in: the mark of a making, half a `bin`, the scratch copies,
  // and an unpacking and a download that never finished.
  const cut = computer();
  write(path.join(cut.K, "updates", "rehome", "new"), "1\n");
  write(path.join(cut.K, "bin", "kuvo-link.js"), "// half of it");
  write(path.join(cut.K, "bin.new", "kuvo-start"), "half");
  write(path.join(staging(cut), "release.new", "bin", "kuvo-link.js"), "// half of it");
  write(path.join(staging(cut), "engine", ".fetch.1", "engine.tgz"), "half");
  const status = install(cut, "status");
  assert.equal(status.said.verdict, "fresh");
  const made = install(cut, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.equal(made.said.result, "done");
  // What the making cut short left is set aside, and the answer says where.
  assert.match(made.said.setAside, new RegExp(`^${cut.K}/trash/earlier-box-`));
  assert.equal(read(path.join(made.said.setAside, "bin", "kuvo-link.js")), "// half of it");
  assert.deepEqual(fs.readFileSync(path.join(cut.K, "bin", "kuvo-link.js")), fs.readFileSync(script), "the release's, whole");
  assert.equal(exists(path.join(cut.K, "bin.new")), false);
  assert.equal(read(path.join(cut.K, "box", "kind")), "mac\n");

  // Cut short between the box and its helper: made, with nothing to start it. Status says to
  // run `new`, which only puts the helper in place.
  const unstarted = computer();
  assert.equal(install(unstarted, "new").said.result, "done");
  fs.rmSync(path.join(unstarted.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist"));
  fs.rmSync(path.join(unstarted.home, "loaded"));
  write(path.join(unstarted.K, "chats", "c1", "notes.md"), "kept");
  const asked = install(unstarted, "status");
  assert.equal(asked.said.verdict, "ours");
  assert.equal(asked.said.verb, "new");
  const finished = install(unstarted, "new");
  assert.equal(finished.status, 0, finished.stderr + JSON.stringify(finished.lines));
  assert.equal(finished.said.result, "nothing", "the box as it was");
  assert.equal(finished.said.started, true);
  assert.equal(exists(path.join(unstarted.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), true);
  assert.equal(read(path.join(unstarted.K, "chats", "c1", "notes.md")), "kept");
  assert.equal(install(unstarted, "status").said.verb, null);

  // Its launch agent written, but launchd wouldn't take it: said as failed, never as started, and
  // `new` is still what to run. Tried again once launchd takes it, it's started.
  const refused = computer();
  write(path.join(refused.home, "launchd-refuses"), "");
  const failed = install(refused, "new");
  assert.equal(failed.status, 1);
  assert.equal(failed.said.result, "failed");
  assert.equal(failed.said.started, false);
  assert.match(failed.said.message, /launchd didn't start it now: Bootstrap failed/);
  assert.equal(read(path.join(refused.K, "box", "kind")), "mac\n", "the box is made");
  assert.equal(exists(path.join(refused.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist")), true);
  assert.deepEqual({ verdict: install(refused, "status").said.verdict, verb: install(refused, "status").said.verb }, { verdict: "ours", verb: "new" });
  fs.rmSync(path.join(refused.home, "launchd-refuses"));
  const retried = install(refused, "new");
  assert.equal(retried.status, 0, retried.stderr + JSON.stringify(retried.lines));
  assert.deepEqual({ result: retried.said.result, started: retried.said.started }, { result: "nothing", started: true });
  assert.equal(exists(path.join(refused.home, "loaded")), true);
  assert.equal(install(refused, "status").said.verb, null);
});

test("a Mac that holds an earlier Kuvo Code's sessions gets no new box beside them: they move instead", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.S, "opencode", "data", "opencode", "opencode.db"), "sessions");
  const status = install(where, "status");
  assert.equal(status.said.verdict, "app-07");
  assert.equal(status.said.verb, "move");
  const refused = install(where, "new");
  assert.equal(refused.status, 1);
  assert.equal(refused.said.result, "refused");
  assert.equal(refused.said.message, "Kuvo Code has your sessions and settings on this Mac already: they move into their new place instead.");
  assert.deepEqual(fs.readdirSync(where.K), ["updates"], "no box made");
  assert.equal(exists(path.join(where.home, "Library", "LaunchAgents")), false);
  // (Sessions only Kuvo's own engine can read: here the engine is a stand-in, which says so and changes nothing.)
  const move = install(where, "move");
  assert.equal(move.status, 1);
  assert.deepEqual({ result: move.said.result, verdict: move.said.verdict, message: move.said.message }, { result: "refused", verdict: "app-07", message: "Your sessions can only be moved by Kuvo's own engine." });
  assert.equal(read(path.join(where.S, "opencode", "data", "opencode", "opencode.db")), "sessions");
  assert.equal(exists(path.join(where.K, "box")), false, "no box made");
});

// `kuvo-keys`, as a stand-in the app carries: it notes what it's asked, and like the real one does
// nothing unless the move's lock names the very process that started it.
const keysStandIn = `#!/bin/sh
verb=$1; shift; H=""
while [ $# -gt 0 ]; do case "$1" in --home) H=$2; shift 2 ;; *) shift ;; esac; done
[ "$(cat "$H/updates/rehome/lock/pid" 2>/dev/null)" = "$PPID" ] || { echo "not the move" >&2; exit 2; }
echo "$verb" >>"$HOME/keys-asked"
case "$verb" in copy) echo '{"copied":0}' ;; passwords) echo '{"v":1,"logins":[]}' ;; *) echo '{}' ;; esac
`;

test("an earlier Kuvo Code's chats and settings move with `kuvo-install move`: each step said in the box's own words, its helper started, and the app's own folder as it was", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.bundle, "Contents", "MacOS", "kuvo-keys"), keysStandIn, 0o755);
  write(path.join(where.S, "Chats", "c1", "notes.md"), "a chat's notes");
  write(path.join(where.S, "bots.json"), JSON.stringify([{ id: "bot_a1", name: "Scout", job: "Watch", instructions: "Watch.", look: {}, folder: path.join(where.S, "bots", "bot_a1"), sessionID: "ses_b", created: "2026-10-01T10:00:00Z" }]));
  write(path.join(where.S, "bots", "bot_a1", "MEMORY.md"), "what Scout remembers");
  write(path.join(where.S, "Browser", "history.json"), "[]");
  const before = everything(where.S).map(name => [name, fs.statSync(path.join(where.S, name)).isFile() ? read(path.join(where.S, name)) : ""]);
  assert.deepEqual({ verdict: install(where, "status").said.verdict, verb: install(where, "status").said.verb }, { verdict: "app-07", verb: "move" });

  const moved = install(where, "move");
  assert.equal(moved.status, 0, moved.stderr + JSON.stringify(moved.lines));
  assert.deepEqual({ result: moved.said.result, verdict: moved.said.verdict, started: moved.said.started }, { result: "done", verdict: "ours", started: true });
  assert.equal(moved.said.message, undefined, "started: nothing more to say");
  assert.equal(/\bbox\b|Kuvo Core/i.test(JSON.stringify(moved.lines.map(line => [line.title, line.message, line.status?.dropped]))), false, "no word of the move's own workings");
  // Its steps, as the box says them: which, in words to show, where it is and of how many.
  const steps = moved.lines.filter(line => line.title);
  assert.deepEqual(steps[0], { step: "plan", title: "Checking this Mac", index: 1, of: 11 });
  assert.ok(steps.length >= 2 && steps.every(step => typeof step.title === "string" && step.of === 11 && step.index >= 1 && step.index <= 11), JSON.stringify(steps));
  assert.deepEqual(steps.map(step => step.index), [...steps.map(step => step.index)].sort((a, b) => a - b), "in order");
  // Moved: a Mac's own box, with the chat and the bot (described as the box reads one), its helper
  // loaded, this Mac's keys asked of `kuvo-keys` by the move itself.
  assert.equal(read(path.join(where.K, "box", "kind")), "mac\n");
  assert.equal(read(path.join(where.K, "chats", "c1", "notes.md")), "a chat's notes");
  assert.equal(JSON.parse(read(path.join(where.K, "bots", "bot_a1", "bot.json"))).name, "Scout");
  assert.equal(read(path.join(where.K, "bots", "bot_a1", "directory")), path.join(where.K, "bots", "bot_a1", "workspace"));
  assert.equal(read(path.join(where.K, "bin", "box-version")), VERSION);
  assert.equal(read(path.join(where.K, "engine", ENGINE, "opencode")), engineBinary);
  assert.equal(exists(path.join(where.home, "loaded")), true);
  assert.deepEqual(read(path.join(where.home, "keys-asked")).trim().split("\n"), ["copy", "delete-originals", "passwords"]);
  assert.deepEqual(moved.said.status.owed, []);
  assert.deepEqual(moved.said.status.dropped, ["Websites you were signed in to in Kuvo Code's browser need signing in again. Your history and saved passwords come along."]);
  // The earlier Kuvo Code's folder: every file as it was. Nothing the installer staged is left.
  assert.deepEqual(everything(where.S).map(name => [name, fs.statSync(path.join(where.S, name)).isFile() ? read(path.join(where.S, name)) : ""]), before);
  assert.equal(exists(staging(where)), false);
  // Asked again as the app next opens: this Mac's own box, nothing to run, nothing moved twice.
  const after = install(where, "status");
  assert.deepEqual({ verdict: after.said.verdict, verb: after.said.verb }, { verdict: "ours", verb: null });
  const again = install(where, "move");
  assert.deepEqual({ status: again.status, result: again.said.result }, { status: 0, result: "nothing" });
});

test("a move launchd wouldn't start is finished by running it again: until then the installer says to", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.bundle, "Contents", "MacOS", "kuvo-keys"), keysStandIn, 0o755);
  write(path.join(where.S, "Chats", "c1", "notes.md"), "a chat's notes");
  write(path.join(where.home, "launchd-refuses"), "");
  const moved = install(where, "move");
  assert.equal(moved.status, 0, moved.stderr + JSON.stringify(moved.lines));
  assert.deepEqual({ result: moved.said.result, started: moved.said.started, owed: moved.said.status.owed }, { result: "done", started: false, owed: ["start", "keychain", "passwords"] });
  // (In words to act on: never the helper's own, nor what launchd answered.)
  assert.equal(moved.said.message, "Your sessions are moved, but Kuvo couldn't be started on this Mac. Try again.");
  assert.deepEqual({ verdict: install(where, "status").said.verdict, verb: install(where, "status").said.verb }, { verdict: "ours", verb: "move" });
  fs.rmSync(path.join(where.home, "launchd-refuses"));
  const again = install(where, "move");
  assert.equal(again.status, 0, again.stderr + JSON.stringify(again.lines));
  assert.deepEqual({ result: again.said.result, started: again.said.started, owed: again.said.status.owed }, { result: "done", started: true, owed: [] });
  assert.equal(install(where, "status").said.verb, null);
});

test("a Mac that's one of your machines too is told so in words by the installer, and nothing is moved or stopped", { skip: !mac && "a Mac's app folder" }, () => {
  const where = computer();
  write(path.join(where.S, "Chats", "c1", "notes.md"), "a chat's notes");
  // A machine's box in its home, as another Mac set it up.
  write(path.join(where.K, "bin", "kuvo-start"), "#!/bin/sh\n", 0o700);
  write(path.join(where.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }));
  // (Nothing to run, and said in words: its earlier sessions stay where they are, unseen.)
  assert.deepEqual({ verdict: install(where, "status").said.verdict, verb: install(where, "status").said.verb, message: install(where, "status").said.message }, { verdict: "two", verb: null, message: "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet." });
  const refused = install(where, "move");
  assert.equal(refused.status, 1);
  assert.deepEqual({ result: refused.said.result, verdict: refused.said.verdict, message: refused.said.message }, { result: "refused", verdict: "two", message: "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet." });
  assert.equal(exists(path.join(where.K, "box", "kind")), false);
  assert.equal(exists(path.join(where.K, "chats")), false);
  assert.equal(exists(path.join(where.home, "launchctl-asked")), false);
});

test("on Linux there's nothing to move: said, and nothing made", { skip: mac && "Linux has no app's folder" }, () => {
  const where = computer();
  const refused = install(where, "move");
  assert.equal(refused.status, 1);
  assert.deepEqual({ result: refused.said.result, message: refused.said.message }, { result: "refused", message: "There's nothing to move on this computer: Kuvo is where it belongs already." });
  assert.equal(exists(path.join(where.K, "bin")), false);
});

test("a copy of the app never judges or installs for the real one, and is refused before anything is written: Kuvo Code's own ID goes with ~/.kuvo-code alone", { skip: !mac && "a Mac's app" }, () => {
  for (const verb of ["status", "new", "move"]) {
    const where = computer({ app: "run.kuvo.code" });
    const refused = install(where, verb);
    assert.equal(refused.status, 1);
    assert.deepEqual(refused.said, { result: "refused", message: "Kuvo Code itself sets up ~/.kuvo-code only." });
    assert.equal(exists(where.K), false, "nothing written, not even its folder");
    assert.deepEqual(fs.readdirSync(where.home).sort(), ["Applications", "tools"]);
  }
  // And with no app named, or one that isn't an app, a Mac's installer says what it needs.
  const unnamed = computer();
  for (const args of [[], ["--app", path.join(unnamed.home, "No Such.app")]]) {
    const asked = install(unnamed, "status", { args });
    assert.equal(asked.status, 1);
    assert.deepEqual(asked.said, { result: "refused", message: "Kuvo couldn't tell which app started it. Open Kuvo Code again." });
    assert.equal(exists(unnamed.K), false);
  }
});

test("the real home is told however it's written: on a Mac's disk a name in other letters' case is the same folder, there or not yet", { skip: !mac && "a Mac's disk" }, () => {
  // The installer's own rule, asked of folders of the test's own (never of the real home).
  const text = read(path.join(release, "kuvo-install"));
  const rule = text.slice(text.indexOf("same_place() {"), text.indexOf("\n}\n", text.indexOf("same_place() {")) + 3);
  const same = (a, b) => spawnSync("sh", ["-c", `${rule}\nsame_place "$1" "$2"`, "sh", a, b]).status === 0;
  const home = path.join(scratch, "case");
  fs.mkdirSync(path.join(home, ".kuvo-code"), { recursive: true });
  fs.mkdirSync(path.join(home, "other"));
  const insensitive = exists(path.join(home, ".KUVO-CODE"));
  assert.equal(same(path.join(home, ".kuvo-code"), path.join(home, ".kuvo-code")), true);
  assert.equal(same(path.join(home, ".Kuvo-Code"), path.join(home, ".kuvo-code")), insensitive, "the same folder, as the disk has it");
  assert.equal(same(path.join(home, "other"), path.join(home, ".kuvo-code")), false);
  // Neither there yet: by name, without regard to case.
  assert.equal(same(path.join(home, "none", ".Kuvo-Code"), path.join(home, "none", ".kuvo-code")), true);
  assert.equal(same(path.join(home, "none", ".kuvo-demo"), path.join(home, "none", ".kuvo-code")), false);
});

test("new on Linux: a machine's box in ~/.kuvo-code, its files and engine in place and started as a computer that's added is; one cut short is made again", { skip: mac && "a Linux computer's box" }, () => {
  const where = computer();
  assert.deepEqual(install(where, "status").said.verdict, "fresh");
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.equal(made.said.result, "done");
  assert.equal(made.said.verdict, "machine");
  assert.equal(made.said.started, true);
  // Every file of the release, as its box.json says, with who may read and run each.
  const box = JSON.parse(read(path.join(scratch, "unpacked-box.json")));
  for (const [name, sum] of Object.entries(box.files)) {
    assert.equal(crypto.createHash("sha256").update(fs.readFileSync(path.join(where.K, name))).digest("hex"), sum, name);
  }
  assert.equal(modeOf(path.join(where.K, "bin", "kuvo-link.js")), "600");
  assert.equal(modeOf(path.join(where.K, "bin", "kuvo-start")), "700");
  assert.equal(read(path.join(where.K, "engine", ENGINE, "opencode")), engineBinary);
  assert.equal(exists(path.join(where.K, "bin", "Kuvo Helper.app")), false, "no helper on Linux");
  // Started as a machine is: how its engine starts noted, its id made, its line in the crontab
  // (the test's stand-in), and its own keeper asked to start.
  assert.match(read(path.join(where.K, "run", "launch.env")), /^PATH=/m);
  assert.match(read(path.join(where.K, "machine-id")), /^[0-9a-f]{32}$/);
  assert.match(read(path.join(where.home, "crontab")), /^@reboot sh "\$HOME\/\.kuvo-code\/bin\/kuvo-start" .*# kuvo-code: starts its engine again after a restart$/m);
  assert.equal(read(path.join(where.home, "keeper-asked")), "start\n");
  assert.equal(exists(staging(where)), false);
  // From now on a machine's box is here: nothing made over it.
  assert.equal(install(where, "status").said.verdict, "machine");
  const again = install(where, "new");
  assert.equal(again.status, 1);
  assert.equal(again.said.result, "refused");
  assert.equal(read(path.join(where.home, "keeper-asked")), "start\n", "nothing started twice");

  // Cut short after its files went in, before it started: fresh still, and made again whole.
  const cut = computer();
  install(cut, "status");
  write(path.join(staging(cut), "making"), "1\n");
  write(path.join(cut.K, "bin", "kuvo-start"), "half", 0o700);
  assert.equal(install(cut, "status").said.verdict, "fresh");
  const remade = install(cut, "new");
  assert.equal(remade.status, 0, remade.stderr + JSON.stringify(remade.lines));
  assert.equal(read(path.join(cut.home, "keeper-asked")), "start\n");
  assert.notEqual(read(path.join(cut.K, "bin", "kuvo-start")), "half");

  // A box the computer that set it up removed (turned off, its sessions left in its folder) is no
  // box: a new one is made, and what was there is set aside in its trash, nothing deleted. The
  // desktop app's own folder there stays where it is.
  const removed = computer();
  write(path.join(removed.K, "run", "off"), "");
  write(path.join(removed.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }));
  write(path.join(removed.K, "chats", "c1", "notes.md"), "kept");
  write(path.join(removed.K, "desktop", "identity"), "the app's own");
  assert.equal(install(removed, "status").said.verdict, "fresh");
  const anew = install(removed, "new");
  assert.equal(anew.status, 0, anew.stderr + JSON.stringify(anew.lines));
  assert.equal(anew.said.verdict, "machine");
  const aside = fs.readdirSync(path.join(removed.K, "trash")).filter(name => name.startsWith("earlier-box-"));
  assert.equal(aside.length, 1);
  assert.equal(anew.said.setAside, path.join(removed.K, "trash", aside[0]), "the answer says where it went");
  assert.equal(made.said.setAside, undefined, "and says nothing where nothing was there");
  assert.equal(read(path.join(removed.K, "trash", aside[0], "chats", "c1", "notes.md")), "kept");
  assert.equal(exists(path.join(removed.K, "trash", aside[0], "run", "link.json")), true, "its old link with it, never the new box's");
  assert.equal(exists(path.join(removed.K, "run", "off")), false);
  assert.equal(exists(path.join(removed.K, "chats")), false);
  assert.equal(read(path.join(removed.K, "desktop", "identity")), "the app's own");
});

test("on Linux nothing is made or set aside under something that still runs in the folder", { skip: mac && "a Linux computer's box" }, async () => {
  // A box turned off whose keeper still runs (it's ending): by its record and its command line.
  const where = computer();
  const keeper = spawn(process.execPath, ["-e", "process.stdin.resume()", "kuvo-start"], { stdio: ["pipe", "ignore", "ignore"] });
  try {
    write(path.join(where.K, "run", "off"), "");
    write(path.join(where.K, "run", "box.pid"), `${keeper.pid}\n`);
    write(path.join(where.K, "chats", "c1", "notes.md"), "kept");
    const refused = install(where, "new");
    assert.equal(refused.status, 1);
    assert.equal(refused.said.result, "refused");
    assert.equal(refused.said.message, "Something is running in this computer's ~/.kuvo-code: Kuvo Code is installed there once that's stopped.");
    assert.equal(read(path.join(where.K, "chats", "c1", "notes.md")), "kept");
    assert.equal(exists(path.join(where.K, "trash")), false);
    assert.equal(exists(path.join(where.K, "bin")), false);
  } finally {
    const gone = new Promise(resolve => keeper.once("exit", resolve));
    keeper.kill();
    await gone;
  }
  // Once it's stopped, it's made.
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.equal(read(path.join(made.said.setAside, "chats", "c1", "notes.md")), "kept");
});

test("on Linux a start that fails after its keeper wrote keys and a link is still not a box: `new` is what to run, and it's made again whole", { skip: mac && "a Linux computer's box" }, () => {
  const where = computer();
  write(path.join(where.home, "keeper-fails"), "");
  const failed = install(where, "new");
  assert.equal(failed.status, 1);
  assert.equal(failed.said.result, "failed");
  assert.equal(failed.said.started, false);
  assert.match(failed.said.message, /couldn't be started: its engine didn't come up\. Try again\.$/);
  assert.equal(exists(path.join(where.K, "run", "link.json")), true, "what its start left");
  const status = install(where, "status");
  assert.deepEqual({ verdict: status.said.verdict, verb: status.said.verb }, { verdict: "fresh", verb: "new" });
  fs.rmSync(path.join(where.home, "keeper-fails"));
  const made = install(where, "new");
  assert.equal(made.status, 0, made.stderr + JSON.stringify(made.lines));
  assert.deepEqual({ result: made.said.result, verdict: made.said.verdict, started: made.said.started }, { result: "done", verdict: "machine", started: true });
  assert.equal(exists(path.join(where.K, "run", "link.json")), false, "the half-made one's link set aside with it");
  assert.equal(exists(path.join(made.said.setAside, "run", "link.json")), true);
  assert.equal(install(where, "status").said.verdict, "machine");
});

// (What the pack's box.json names, for the Linux test: read from the pack itself.)
{
  const unpacked = path.join(scratch, "unpacked");
  fs.mkdirSync(unpacked);
  assert.equal(spawnSync("tar", ["xzf", path.join(release, `box-${VERSION}.tar.gz`), "-C", unpacked]).status, 0);
  fs.copyFileSync(path.join(unpacked, "box.json"), path.join(scratch, "unpacked-box.json"));
}
