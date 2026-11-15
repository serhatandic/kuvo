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
import { spawnSync } from "node:child_process";
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
write(path.join(sources, "box/src/keeper/kuvo-start"), `#!/bin/sh\n# A stand-in for the keeper, for engine @ENGINE@: it notes what it's asked.\necho "$*" >>"$HOME/keeper-asked"\n`);
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
    env: { PATH: process.env.PATH, HOME: where.home, TMPDIR: process.env.TMPDIR ?? os.tmpdir(), KUVO_TEST_UNINSTALL_TOOLS: where.tools, KUVO_BOX: "mac", KUVO_HOME: "/nowhere", ...env },
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
  assert.equal(refused.said.message, "This computer is set up as one of your machines already: Kuvo Code uses the box that's here.");
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
  // Its files from the release, its helper whole, the release's engine, and its kind.
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

test("a Mac that holds an earlier Kuvo Code's sessions gets no new box beside them, and moving them isn't here yet, said plainly", { skip: !mac && "a Mac's app folder" }, () => {
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
  const move = install(where, "move");
  assert.equal(move.status, 1);
  assert.equal(move.said.result, "refused");
  assert.equal(move.said.message, "Moving an earlier Kuvo Code's sessions into their new place isn't in this version yet.");
  assert.equal(read(path.join(where.S, "opencode", "data", "opencode", "opencode.db")), "sessions");
});

test("a copy of the app never judges or installs for the real one: Kuvo Code's own ID goes with ~/.kuvo-code alone", { skip: !mac && "a Mac's app" }, () => {
  const where = computer({ app: "run.kuvo.code" });
  const refused = install(where, "new");
  assert.equal(refused.status, 1);
  assert.equal(refused.said.result, "refused");
  assert.equal(refused.said.message, "Kuvo Code itself installs its box in ~/.kuvo-code only.");
  assert.deepEqual(fs.readdirSync(where.K), ["updates"]);
  // And with no app named, a Mac's installer says what it needs.
  const unnamed = install(computer(), "status", { args: [] });
  assert.equal(unnamed.status, 1);
  assert.match(unnamed.said.message, /needs the app it's run from/);
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
  assert.equal(read(path.join(removed.K, "trash", aside[0], "chats", "c1", "notes.md")), "kept");
  assert.equal(exists(path.join(removed.K, "trash", aside[0], "run", "link.json")), true, "its old link with it, never the new box's");
  assert.equal(exists(path.join(removed.K, "run", "off")), false);
  assert.equal(exists(path.join(removed.K, "chats")), false);
  assert.equal(read(path.join(removed.K, "desktop", "identity")), "the app's own");
});

// (What the pack's box.json names, for the Linux test: read from the pack itself.)
{
  const unpacked = path.join(scratch, "unpacked");
  fs.mkdirSync(unpacked);
  assert.equal(spawnSync("tar", ["xzf", path.join(release, `box-${VERSION}.tar.gz`), "-C", unpacked]).status, 0);
  fs.copyFileSync(path.join(unpacked, "box.json"), path.join(scratch, "unpacked-box.json"));
}
