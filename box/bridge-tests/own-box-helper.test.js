// This Mac's box's helper, Kuvo Helper (#333 piece 4; PROTOCOL.md, "This Mac's box: its helper"):
// the launch agent that starts it at login (`kuvo-link.js helper install [--load]`, `helper status`),
// and a Mac's box in a home of its own saying where its server answers (`run/serve.json`), as its
// helper starts it; `helper start` opening the helper itself where Login Items is off for it; and
// what macOS allows the helper, as the box says it (`kuvo/box/permissions`). The real box script, in homes of the test's own, with a stand-in for `launchctl`
// and a stand-in helper: never this computer's launchd, which a test's home leaves alone without them.
//
// Run with `node --test box/bridge-tests/own-box-helper.test.js`.
import { removeTestHomeAtExit } from "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
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

const script = fileURLToPath(LINK_BUNDLE);
const write = (file, text, mode) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
  if (mode) fs.chmodSync(file, mode);
};
const read = file => fs.readFileSync(file, "utf8");
const exists = file => fs.existsSync(file);

/** A home of the test's own, its box of `kind`, with stand-ins for the helper and `launchctl`. */
function home(kind, { helper = true, parent } = {}) {
  const folder = fs.realpathSync(fs.mkdtempSync(path.join(parent ?? os.tmpdir(), "kuvo-own-box-helper-")));
  if (!parent) removeAtEnd(folder);
  const K = path.join(folder, ".kuvo-code");
  if (kind) write(path.join(K, "box", "kind"), `${kind}\n`);
  fs.mkdirSync(path.join(K, "run"), { recursive: true });
  if (helper) write(path.join(K, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper"), "#!/bin/sh\nexit 0\n", 0o755);
  const tools = path.join(folder, "tools");
  // launchctl: notes what it's asked; `print` says it's loaded once it was bootstrapped.
  write(path.join(tools, "launchctl"), `#!/bin/sh\necho "$*" >>"${folder}/launchctl"\ncase "$1" in print) [ -f "${folder}/loaded" ] ;; bootstrap) touch "${folder}/loaded" ;; bootout) rm -f "${folder}/loaded" ;; esac\n`, 0o755);
  return { home: folder, K, run: path.join(K, "run"), tools };
}
const envOf = (where, more = {}) => ({ ...process.env, HOME: where.home, KUVO_HOME: where.K, TMPDIR: where.home, KUVO_TEST_UNINSTALL_TOOLS: where.tools, ...more });
const helper = (where, args, more) => {
  const done = spawnSync(process.execPath, [script, "helper", ...args], { env: envOf(where, more), encoding: "utf8" });
  return { status: done.status, said: done.stdout.trim() ? JSON.parse(done.stdout.trim()) : null, stderr: done.stderr };
};
const plistOf = where => path.join(where.home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist");

test("helper install: the launch agent starts Kuvo Helper at login, with this box's folder, under Kuvo Code in Login Items", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, () => {
  const where = home("mac");
  const done = helper(where, ["install", "--app", "run.kuvo.code.trial4"]);
  assert.equal(done.status, 0, done.stderr);
  assert.deepEqual(done.said, { written: true, loaded: false, message: "Kuvo Helper starts Kuvo on this Mac at login." });
  const plist = read(plistOf(where));
  const program = path.join(where.K, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper");
  assert.match(plist, /<key>Label<\/key><string>run\.kuvo\.code\.machine<\/string>/);
  assert.ok(plist.includes(`<array><string>${program}</string><string>keep</string><string>--home</string><string>${where.K}</string></array>`), plist);
  assert.match(plist, /<key>RunAtLoad<\/key><true\/>/);
  assert.match(plist, /<key>KeepAlive<\/key><dict><key>SuccessfulExit<\/key><false\/><\/dict>/);
  assert.match(plist, /<key>AbandonProcessGroup<\/key><true\/>/);
  assert.match(plist, /<key>AssociatedBundleIdentifiers<\/key><array><string>run\.kuvo\.code\.trial4<\/string><\/array>/);
  // A real property list, as launchd reads it.
  assert.equal(spawnSync("plutil", ["-lint", plistOf(where)]).status, 0);
  assert.equal(exists(path.join(where.home, "launchctl")), false, "written, not loaded");
  assert.deepEqual(helper(where, ["status"]).said, { helper: true, agent: true, loaded: false, running: false, handOver: null }, "written, not loaded: not running either");

  // Loaded now: whatever had the label goes first.
  fs.rmSync(path.join(where.home, "launchctl"));
  const loaded = helper(where, ["install", "--load"]);
  assert.equal(loaded.status, 0, loaded.stderr);
  assert.equal(loaded.said.loaded, true);
  const uid = process.getuid();
  assert.deepEqual(read(path.join(where.home, "launchctl")).trim().split("\n"), [
    `bootout gui/${uid}/run.kuvo.code.machine`,
    // (Gone before it's started again.)
    `print gui/${uid}/run.kuvo.code.machine`,
    `bootstrap gui/${uid} ${plistOf(where)}`,
  ]);
  assert.match(read(plistOf(where)), /<string>run\.kuvo\.code<\/string>/, "the app's ID by default");
  assert.deepEqual(helper(where, ["status"]).said, { helper: true, agent: true, loaded: true, running: null, handOver: null }, "loaded, launchd saying no state: whether it runs isn't known");
});

test("helper install --load waits for launchd to let go of the one before, and starts it again once it has", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, () => {
  const where = home("mac");
  // launchd still stopping the helper before (two looks), and refusing the first start meanwhile.
  write(path.join(where.tools, "launchctl"), `#!/bin/sh
echo "$*" >>"${where.home}/launchctl"
n=$(cat "${where.home}/looks" 2>/dev/null || echo 0)
case "$1" in
print) echo $((n + 1)) >"${where.home}/looks"; [ "$n" -lt 2 ] ;;
bootstrap) if [ -f "${where.home}/refused" ]; then exit 0; fi; touch "${where.home}/refused"; echo "5: Input/output error" >&2; exit 5 ;;
esac
`, 0o755);
  const done = helper(where, ["install", "--load"]);
  assert.equal(done.status, 0, done.stderr);
  assert.equal(done.said.loaded, true);
  const asked = read(path.join(where.home, "launchctl")).trim().split("\n").map(line => line.split(" ")[0]);
  assert.deepEqual(asked, ["bootout", "print", "print", "print", "bootstrap", "bootstrap"]);
});

test("helper install: a path with characters XML reserves is written as launchd reads it", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, () => {
  const folder = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-own-box-helper-&<\"-")));
  removeAtEnd(folder);
  const where = { home: folder, K: path.join(folder, ".kuvo-code"), run: path.join(folder, ".kuvo-code", "run"), tools: path.join(folder, "tools") };
  write(path.join(where.K, "box", "kind"), "mac\n");
  write(path.join(where.K, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper"), "#!/bin/sh\n", 0o755);
  write(path.join(where.tools, "launchctl"), "#!/bin/sh\n", 0o755);
  assert.equal(helper(where, ["install"]).status, 0);
  const listed = spawnSync("plutil", ["-extract", "ProgramArguments.3", "raw", plistOf(where)], { encoding: "utf8" });
  assert.equal(listed.stdout.trim(), where.K);
});

test("helper start: the whole start through the helper, its launch agent put in place, loaded and started as each needs, one running left as it is", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, () => {
  const where = home("mac");
  const uid = process.getuid();
  const asked = () => { const said = exists(path.join(where.home, "launchctl")) ? read(path.join(where.home, "launchctl")).trim().split("\n") : []; fs.rmSync(path.join(where.home, "launchctl"), { force: true }); return said; };
  // Not in place: written and loaded.
  const first = helper(where, ["start", "--app", "run.kuvo.code.trial5"]);
  assert.equal(first.status, 0, first.stderr);
  assert.deepEqual(first.said, { started: true, mac: true, message: "Kuvo Helper runs Kuvo on this Mac now, and at login." });
  assert.match(read(plistOf(where)), /<string>run\.kuvo\.code\.trial5<\/string>/);
  assert.equal(asked().filter(line => line.startsWith("bootstrap")).length, 1);
  // Loaded: started if it isn't running (kickstart leaves one running as it is).
  assert.deepEqual(helper(where, ["start"]).said, { started: true, mac: true, message: "Kuvo Helper runs Kuvo on this Mac." });
  assert.deepEqual(asked(), [`print gui/${uid}/run.kuvo.code.machine`, `kickstart gui/${uid}/run.kuvo.code.machine`]);
  // In place but not loaded: loaded, then started.
  fs.rmSync(path.join(where.home, "loaded"));
  assert.equal(helper(where, ["start"]).said.started, true);
  assert.deepEqual(asked().map(line => line.split(" ")[0]), ["print", "bootstrap", "kickstart"]);
  // --if-missing: one in place is left as it is (switched off in Login Items, it stays off).
  fs.rmSync(path.join(where.home, "loaded"));
  const left = helper(where, ["start", "--if-missing"]);
  assert.equal(left.status, 0);
  assert.equal(left.said.left, true);
  assert.deepEqual(asked().filter(line => !line.startsWith("print")), []);
  // launchd won't load it, for a reason that isn't Login Items: said, and nothing opened.
  fs.rmSync(path.join(where.home, "loaded"), { force: true });
  write(path.join(where.tools, "open"), `#!/bin/sh\nprintf '%s\\n' "$@" >"${where.home}/opened"\n[ ! -f "${where.home}/open-fails" ] || { echo "The application cannot be opened." >&2; exit 1; }\n`, 0o755);
  write(path.join(where.tools, "launchctl"), `#!/bin/sh\necho "$*" >>"${where.home}/launchctl"\ncase "$1" in print) exit 113 ;; bootstrap) echo "Bootstrap failed: 5: Input/output error" >&2; exit 5 ;; print-disabled) printf '\\t"run.kuvo.code.machine" => enabled\\n' ;; esac\n`, 0o755);
  const refusedByLaunchd = helper(where, ["start"]);
  assert.equal(refusedByLaunchd.status, 1);
  assert.deepEqual(refusedByLaunchd.said, { started: false, mac: true, message: "launchd didn't load Kuvo Helper: Bootstrap failed: 5: Input/output error." });
  assert.equal(exists(path.join(where.home, "opened")), false);
  // Switched off in Login Items: launchd won't, so the box opens the helper itself, as its own
  // program (in the background, a new one, keeping this box), and says it's only for now.
  write(path.join(where.tools, "launchctl"), `#!/bin/sh\necho "$*" >>"${where.home}/launchctl"\ncase "$1" in print) exit 113 ;; bootstrap) echo "Bootstrap failed: 5: Input/output error" >&2; exit 5 ;; print-disabled) printf '\\t"run.kuvo.code.machine" => disabled\\n' ;; esac\n`, 0o755);
  const off = helper(where, ["start"]);
  assert.equal(off.status, 0, off.stderr);
  assert.deepEqual(off.said, { started: true, mac: true, disabled: true, opened: true, message: "Kuvo Helper is turned off in System Settings › General › Login Items, so it was opened for now, to run Kuvo on this Mac until you log out. Turn it on there to start it at login." });
  assert.deepEqual(read(path.join(where.home, "opened")).trim().split("\n"), ["-g", "-j", "-n", path.join(where.K, "bin", "Kuvo Helper.app"), "--args", "keep", "--home", where.K]);
  // --if-missing leaves one switched off as it is: not opened.
  fs.rmSync(path.join(where.home, "opened"));
  assert.equal(helper(where, ["start", "--if-missing"]).said.left, true);
  assert.equal(exists(path.join(where.home, "opened")), false);
  // macOS won't open it either: said, with what to do.
  write(path.join(where.home, "open-fails"), "");
  const unopened = helper(where, ["start"]);
  assert.equal(unopened.status, 1);
  assert.deepEqual(unopened.said, { started: false, mac: true, disabled: true, message: "Kuvo Helper is turned off in System Settings › General › Login Items, and macOS didn't open it: The application cannot be opened.. Turn it on there, then try again." });
  // Its launch agent's file gone too (taken away by hand) while it's switched off: put back, and opened.
  fs.rmSync(path.join(where.home, "open-fails"));
  fs.rmSync(plistOf(where));
  const again = helper(where, ["start"]);
  assert.equal(again.status, 0, again.stderr);
  assert.equal(again.said.opened, true);
  assert.equal(exists(plistOf(where)), true);
  // A Mac's box with no helper in its bin: says why, which the app shows.
  const bare = home("mac", { helper: false });
  const refused = helper(bare, ["start"]);
  assert.equal(refused.status, 1);
  assert.equal(refused.said.mac, true);
  assert.match(refused.said.message, /isn't in Kuvo's bin folder/);
  // Not a Mac's own box: nothing asked, and it says so.
  const machine = home("machine");
  const other = helper(machine, ["start"]);
  assert.equal(other.status, 1);
  assert.equal(other.said.mac, false, "not a Mac's own box: its own start, not the helper's");
  assert.equal(exists(path.join(machine.home, "launchctl")), false);
});

test("helper install refuses what isn't a Mac's own box in a home of its own, and writes nothing", () => {
  // A machine's box (its launch agent is the machines' own, /bin/sh).
  const machine = home("machine");
  const refused = helper(machine, ["install"]);
  assert.equal(refused.status, 1);
  assert.equal(refused.said.written, false);
  assert.match(refused.said.message, process.platform === "darwin" ? /Only Kuvo on a Mac in its own folder runs under Kuvo Helper/ : /Only Kuvo on a Mac runs under Kuvo Helper/);
  assert.equal(exists(plistOf(machine)), false);
  if (process.platform !== "darwin") return;
  // No helper in its bin yet.
  const bare = home("mac", { helper: false });
  assert.match(helper(bare, ["install"]).said.message, /isn't in Kuvo's bin folder/);
  assert.equal(exists(plistOf(bare)), false);
  // A test's home without stand-ins: this computer's launchd and LaunchAgents are left alone.
  const alone = home("mac");
  assert.match(helper(alone, ["install"], { KUVO_TEST_UNINSTALL_TOOLS: "" }).said.message, /Only Kuvo in ~\/\.kuvo-code starts at login/);
  assert.equal(exists(plistOf(alone)), false);
});

test("helper says what it doesn't know", () => {
  const where = home("mac");
  for (const args of [[], ["remove"], ["install", "--now"], ["install", "--app"], ["install", "--app", "not an id"], ["status", "--load"], ["start", "--load"]]) {
    const done = helper(where, args);
    assert.equal(done.status, 2, args.join(" "));
    assert.match(done.stderr, /Usage: kuvo-link\.js helper install/);
  }
});

test("a Mac's box in a home of its own says where its server answers, as its helper starts it, and takes it away as it goes", async () => {
  const where = home("mac", { helper: false });
  const password = "p".repeat(48);
  const child = spawn(process.execPath, [script, "serve"], { env: envOf(where, { KUVO_TEST_NO_PEERS: "1" }), stdio: ["pipe", "pipe", "pipe"] });
  atEnd(() => child.kill());
  const ended = new Promise(resolve => child.on("exit", code => resolve(code)));
  // As its helper gives them: its password and the vault's secret, on stdin, the pipe left open.
  child.stdin.write(JSON.stringify({ password, hostVaultSecret: "s".repeat(64) }) + "\n");
  const { url } = await new Promise((resolve, reject) => {
    let out = "";
    child.stdout.on("data", chunk => {
      out += chunk;
      if (out.includes("\n")) resolve(JSON.parse(out.split("\n")[0]));
    });
    ended.then(code => reject(new Error(`serve ended (${code})`)));
  });
  const recordFile = path.join(where.run, "serve.json");
  const record = JSON.parse(read(recordFile));
  assert.deepEqual(record, { pid: child.pid, port: Number(new URL(url).port), password, kept: false, formatVersion: 1 });
  assert.equal(fs.statSync(recordFile).mode & 0o777, 0o600);
  const info = await fetch(`${url}/kuvo/info`, { headers: { authorization: "Basic " + Buffer.from(`opencode:${password}`).toString("base64") } });
  assert.equal(info.status, 200);
  await info.body?.cancel();
  // Its helper gone (its stdin closed): so is it, and its record.
  child.stdin.end();
  assert.equal(await ended, 0);
  assert.equal(exists(recordFile), false);
});

/** `serve` for the box at `where`, as its helper starts it: where it answers, and its end. */
async function served(where, extraEnv = {}) {
  const password = "p".repeat(48);
  const child = spawn(process.execPath, [script, "serve"], { env: envOf(where, { KUVO_TEST_NO_PEERS: "1", ...extraEnv }), stdio: ["pipe", "pipe", "pipe"] });
  atEnd(() => child.kill());
  const ended = new Promise(resolve => child.on("exit", code => resolve(code)));
  child.stdin.write(JSON.stringify({ password, hostVaultSecret: "s".repeat(64) }) + "\n");
  const { url } = await new Promise((resolve, reject) => {
    let out = "";
    child.stdout.on("data", chunk => {
      out += chunk;
      if (out.includes("\n")) resolve(JSON.parse(out.split("\n")[0]));
    });
    ended.then(code => reject(new Error(`serve ended (${code})`)));
  });
  const ask = async (route, method = "GET", body, headers = {}) => {
    const answer = await fetch(`${url}/${route}`, { method, headers: { authorization: "Basic " + Buffer.from(`opencode:${password}`).toString("base64"), ...(body ? { "content-type": "application/json" } : {}), ...headers }, ...(body ? { body: JSON.stringify(body) } : {}) });
    return { status: answer.status, body: await answer.json().catch(() => null) };
  };
  return { ask, stop: async () => { child.stdin.end(); await ended; } };
}

const filesPermission = (allowed, asks = allowed === false) => ({
  id: "fullDiskAccess", name: "Files", line: "For agents that work with your files anywhere on this Mac.",
  settings: "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles", byHand: true, byHandLine: "Drag Kuvo Helper into the list in System Settings, then switch it on.", unknownLine: "Kuvo Helper hasn’t said yet.", allowed, asks,
});

test("kuvo/box/permissions: what macOS allows Kuvo Helper, as the helper running this box noted it", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, async () => {
  const where = home("mac");
  const box = await served(where);
  const facts = path.join(where.run, "permissions.json");
  // (When this test's process started, as the helper notes its own.)
  const started = Math.floor(Date.now() / 1000 - process.uptime());
  const bundle = path.join(where.K, "bin", "Kuvo Helper.app");
  // Each permission in words: what System Settings calls it, the address of its list there, that
  // you add the helper to it by hand, and whether a client brings it up now.
  const listed = (screenRecording, accessibility, asks = {}, files = null) => [
    { id: "screenRecording", name: "Screen Recording", line: "For agents that look at the screen.", settings: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture", byHand: true, byHandLine: "Drag Kuvo Helper into the list in System Settings, then switch it on.", unknownLine: "Kuvo Helper hasn’t said yet.", allowed: screenRecording, asks: asks.screenRecording ?? screenRecording === false },
    { id: "accessibility", name: "Accessibility", line: "For agents that click and type in other apps.", settings: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility", byHand: true, byHandLine: "Drag Kuvo Helper into the list in System Settings, then switch it on.", unknownLine: "Kuvo Helper hasn’t said yet.", allowed: accessibility, asks: asks.accessibility ?? accessibility === false },
    filesPermission(files, asks.fullDiskAccess ?? files === false),
  ];
  const unknown = { applies: true, name: "Kuvo Helper", known: false, screenRecording: null, accessibility: null, bundle, list: listed(null, null) };
  // No helper has said: it applies here, and nothing's known (so nothing's brought up).
  assert.deepEqual(await box.ask("kuvo/box/permissions"), { status: 200, body: unknown });
  // The helper running this box (a stand-in: this test's own process) noted what it's allowed.
  write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: true, accessibility: false }));
  assert.deepEqual((await box.ask("kuvo/box/permissions")).body, { applies: true, name: "Kuvo Helper", known: true, screenRecording: true, accessibility: false, bundle, list: listed(true, false) });
  // Older helpers only know about the first two permissions.
  write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: true, accessibility: false, fullDiskAccess: false }));
  assert.deepEqual((await box.ask("kuvo/box/permissions")).body.list, listed(true, false, {}, false));
  assert.deepEqual((await box.ask("kuvo/box/permissions", "POST", { later: "fullDiskAccess" })).body.list.at(-1), filesPermission(false, false));
  write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: true, accessibility: false, fullDiskAccess: true }));
  assert.deepEqual((await box.ask("kuvo/box/permissions")).body.list.at(-1), filesPermission(true));
  write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: true, accessibility: false }));
  // Not Now: put off in the box, so no client brings it up again, at this open or the next.
  assert.deepEqual((await box.ask("kuvo/box/permissions", "POST", { later: "accessibility" })).body.list, listed(true, false, { accessibility: false }));
  assert.deepEqual((await box.ask("kuvo/box/permissions")).body.list, listed(true, false, { accessibility: false }));
  assert.equal((await box.ask("kuvo/box/permissions", "POST", { later: "the moon" })).status, 400);
  assert.equal((await box.ask("kuvo/box/permissions", "POST", {})).status, 400);
  // Granted since: the helper notes it again, and the box says it as it's asked.
  write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: true, accessibility: true }));
  assert.equal((await box.ask("kuvo/box/permissions")).body.accessibility, true);
  // Taken away again later: it was allowed in between, so it's brought up again.
  write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: true, accessibility: false }));
  assert.deepEqual((await box.ask("kuvo/box/permissions")).body.list, listed(true, false));
  write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: true, accessibility: true }));
  // A helper killed where it stood left its note, and its number went to another process of yours
  // (here this test's, which started at another time than the note says its writer did).
  for (const other of [started - 3600, started + 600]) {
    write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started: other, screenRecording: true, accessibility: true }));
    assert.deepEqual((await box.ask("kuvo/box/permissions")).body, unknown, String(other));
  }
  // A note left by a helper that's gone (it ended without taking it away) says nothing of now.
  const gone = spawnSync(process.execPath, ["-e", "console.log(process.pid)"], { encoding: "utf8" });
  write(facts, JSON.stringify({ v: 1, pid: Number(gone.stdout.trim()), started, screenRecording: true, accessibility: true }));
  assert.deepEqual((await box.ask("kuvo/box/permissions")).body, unknown);
  // Nor one whose number is someone else's process now (launchd's here), not yours to signal.
  write(facts, JSON.stringify({ v: 1, pid: 1, started, screenRecording: true, accessibility: true }));
  assert.deepEqual((await box.ask("kuvo/box/permissions")).body, unknown);
  // Nor one that isn't the helper's (half written, another version's, not facts).
  for (const text of ["{", "[]", JSON.stringify({ v: 2, pid: process.pid, observedAt: Date.now(), started, screenRecording: true, accessibility: true }), JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: "yes", accessibility: true }), JSON.stringify({ v: 1, pid: 0, started, screenRecording: true, accessibility: true }), JSON.stringify({ v: 1, pid: process.pid, screenRecording: true, accessibility: true })]) {
    write(facts, text);
    assert.deepEqual((await box.ask("kuvo/box/permissions")).body, unknown, text);
  }
  // Not Now that can't be kept (its file's place taken by a folder) is said, never thrown.
  fs.rmSync(path.join(where.K, "box", "permissions-later.json"), { force: true });
  fs.mkdirSync(path.join(where.K, "box", "permissions-later.json", "in the way"), { recursive: true });
  write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: false, accessibility: true }));
  const unkept = await box.ask("kuvo/box/permissions", "POST", { later: "screenRecording" });
  assert.equal(unkept.status, 500);
  assert.equal(typeof unkept.body.message, "string");
  fs.rmSync(path.join(where.K, "box", "permissions-later.json"), { recursive: true });
  // Nothing here grants anything, whatever it's sent.
  assert.equal((await box.ask("kuvo/box/permissions", "PUT")).status, 405);
  await box.stop();
});

test("kuvo/box/permissions: a permission switched on in System Settings is said as a client asks, the helper asked to look again first", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, async () => {
  const where = home("mac");
  const facts = path.join(where.run, "permissions.json");
  const started = Math.floor(Date.now() / 1000 - process.uptime());
  // The helper's host interface (a stand-in: this test's own process, as the note's writer): what
  // macOS allows it now, noted as it's asked to look (`permissions`), as the helper does; it proves
  // it's what wrote host.json first, as the box asks.
  const password = "h".repeat(48);
  let allowed = false;
  let knows = true;
  const asked = [];
  const note = () => write(facts, JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started, screenRecording: allowed, accessibility: false }));
  const host = http.createServer((request, response) => {
    let body = "";
    request.on("data", chunk => { body += chunk; });
    request.on("end", () => {
      asked.push(request.url);
      const answer = (status, value) => { response.writeHead(status, { "content-type": "application/json", connection: "close" }); response.end(JSON.stringify(value)); };
      if (request.url === "/hello") {
        const { nonce } = JSON.parse(body);
        return answer(200, { proof: crypto.createHmac("sha256", password).update(["kuvo-host-hello", nonce, `127.0.0.1:${host.address().port}`].join("\0")).digest("hex") });
      }
      if (request.headers.authorization !== `Bearer ${password}`) return answer(401, {});
      if (request.url === "/permissions" && knows) { note(); return answer(200, {}); }
      answer(404, {});
    });
  });
  await new Promise(resolve => host.listen(0, "127.0.0.1", resolve));
  atEnd(() => new Promise(resolve => host.close(resolve)));
  write(path.join(where.run, "host.json"), JSON.stringify({ url: `http://127.0.0.1:${host.address().port}`, password }), 0o600);
  note();
  const box = await served(where);
  assert.equal((await box.ask("kuvo/box/permissions")).body.screenRecording, false);
  // Switched on while the tile's up: said at the next ask, not at the helper's next look.
  allowed = true;
  assert.equal((await box.ask("kuvo/box/permissions")).body.screenRecording, true);
  assert.ok(asked.includes("/permissions"));
  // A helper from before it could be asked so: the note as it last wrote it.
  knows = false;
  allowed = false;
  assert.equal((await box.ask("kuvo/box/permissions")).body.screenRecording, true);
  // None answering at all: the same.
  fs.rmSync(path.join(where.run, "host.json"));
  assert.equal((await box.ask("kuvo/box/permissions")).body.screenRecording, true);
  await box.stop();
});

test("kuvo/box/permissions: a machine's box runs under no such program, and says so", async () => {
  const where = home("machine", { helper: false });
  // (Whatever a file there says: nothing here is the helper's word.)
  write(path.join(where.run, "permissions.json"), JSON.stringify({ v: 1, pid: process.pid, observedAt: Date.now(), started: Math.floor(Date.now() / 1000 - process.uptime()), screenRecording: true, accessibility: true }));
  const box = await served(where);
  assert.deepEqual(await box.ask("kuvo/box/permissions"), { status: 200, body: { applies: false, name: null, known: false, screenRecording: null, accessibility: null, bundle: null, list: [] } });
  // Nothing to put off where nothing's asked.
  assert.equal((await box.ask("kuvo/box/permissions", "POST", { later: "accessibility" })).status, 409);
  await box.stop();
});

test("Kuvo's desktop app says where it is as it opens: kept for Open Kuvo, never taken for the app the browser opens", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, async () => {
  const where = home("mac");
  const box = await served(where);
  const app = { "x-kuvo-from": "app" };
  const desktop = "/Applications/Kuvo.app";
  const said = await box.ask("kuvo/box/app", "POST", { desktop }, app);
  assert.equal(said.status, 200, JSON.stringify(said.body));
  assert.deepEqual([said.body.desktop, said.body.bundle, said.body.open], [desktop, null, false]);
  assert.equal((await box.ask("kuvo/box/app")).body.desktop, desktop);
  // The browser's app registering (or quitting) keeps where the desktop app is.
  assert.equal((await box.ask("kuvo/box/app", "POST", { tools: null }, app)).body.desktop, desktop);
  // Only from the app itself, and a whole path to an app only.
  assert.equal((await box.ask("kuvo/box/app", "POST", { desktop: "/Users/someone/Kuvo.app" }, { "x-kuvo-from": "mac" })).status, 403);
  assert.equal((await box.ask("kuvo/box/app", "POST", { desktop: "Kuvo.app" }, app)).status, 400);
  assert.equal((await box.ask("kuvo/box/app", "POST", { desktop: "/Applications/../tmp/Kuvo.app" }, app)).status, 400);
  await box.stop();
});

test("kuvo/box/at-work and kuvo/box/stop: what Kuvo Helper draws in the menu bar, and Stop Kuvo on this Mac, which the box's keeper does", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, async () => {
  const where = home("mac");
  // Its keeper, a stand-in: notes what it's asked, as `kuvo-start pause` would be.
  write(path.join(where.K, "bin", "kuvo-start"), `#!/bin/sh\necho "$1 $HOME" >>"${where.home}/keeper-asked"\n`, 0o755);
  const box = await served(where);
  const said = await box.ask("kuvo/box/at-work");
  assert.equal(said.status, 200);
  // A Mac's own box is in its menu bar; with no engine read, nothing is known to work.
  const { computers, ...facts } = said.body;
  assert.deepEqual(facts, { applies: true, working: 0, sessions: [], asking: [], waiting: { sessions: 0, bots: 0, queue: 0 }, background: 0, busy: false, rings: [] });
  // Your computers, this Mac alone so far.
  assert.deepEqual(computers.map(({ kind, here, reach, working, waiting }) => ({ kind, here, reach, working, waiting })), [{ kind: "mac", here: true, reach: "connected", working: 0, waiting: 0 }]);
  const stopping = await box.ask("kuvo/box/stop", "POST", {});
  assert.deepEqual(stopping, { status: 202, body: { stopping: true } });
  // In a process of its own (it outlasts the server, which goes with the helper), its HOME the
  // folder around the box.
  for (let waited = 0; waited < 200 && !exists(path.join(where.home, "keeper-asked")); waited++) await new Promise(done => setTimeout(done, 25));
  assert.equal(read(path.join(where.home, "keeper-asked")), `pause ${where.home}\n`);
  await box.stop();
});

test("kuvo/box/stop: a machine's box isn't stopped so (nothing would start it again at login)", async () => {
  const where = home("machine");
  write(path.join(where.K, "bin", "kuvo-start"), `#!/bin/sh\necho "$1" >>"${where.home}/keeper-asked"\n`, 0o755);
  const box = await served(where);
  assert.equal((await box.ask("kuvo/box/stop", "POST", {})).status, 409);
  assert.equal((await box.ask("kuvo/box/at-work")).body.applies, false);
  assert.equal(exists(path.join(where.home, "keeper-asked")), false);
  await box.stop();
});

test("helper start after Stop Kuvo on this Mac waits for the stopped helper to go before starting it again", { skip: process.platform !== "darwin" && "only a Mac's box has the helper" }, () => {
  const where = home("mac");
  write(path.join(where.run, "paused"), "");
  // launchd still has the stopped helper running for three looks, then it's gone.
  write(path.join(where.tools, "launchctl"), `#!/bin/sh
echo "$*" >>"${where.home}/launchctl"
case "$1" in
  print) n=$(cat "${where.home}/looks" 2>/dev/null || echo 0); n=$((n + 1)); echo $n >"${where.home}/looks"; [ $n -le 3 ] && echo "	pid = 4242"; exit 0 ;;
esac
`, 0o755);
  helper(where, ["install", "--app", "run.kuvo.code.trial4"]);
  fs.rmSync(path.join(where.home, "launchctl"), { force: true });
  const started = helper(where, ["start"]);
  assert.equal(started.status, 0, started.stderr);
  const asked = read(path.join(where.home, "launchctl")).trim().split("\n");
  const kick = asked.findIndex(line => line.startsWith("kickstart"));
  assert.ok(kick > 0, asked.join("\n"));
  // (Its own look for whether it's loaded, then the waits: kickstarted only once it's gone.)
  assert.ok(asked.slice(0, kick).filter(line => line.startsWith("print gui")).length >= 4, asked.join("\n"));
  assert.equal(Number(read(path.join(where.home, "looks")).trim()) >= 4, true);
});


test("a Darwin machine under Kuvo Helper reports Files from the Helper's note", { skip: process.platform !== "darwin" }, async () => {
  const parent = fs.realpathSync(fs.mkdtempSync("/tmp/k"));
  removeTestHomeAtExit(parent);
  const where = home("machine", { parent });
  write(path.join(where.run, "machine-helper"), "");
  const facts = path.join(where.run, "permissions.json");
  const program = path.join(where.K, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper");
  const shell = path.join(where.K, "bin", "kuvo-start");
  const standIn = (...args) => spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)", ...args], {
    env: envOf(where), stdio: "ignore",
  });
  const helper = standIn(program, "keep-machine");
  const keeper = standIn(shell, "keep");
  const started = Math.floor(Date.now() / 1000);
  write(path.join(where.run, "box.pid"), String(keeper.pid));
  write(path.join(where.run, "machine-keeper"), `${keeper.pid} ${helper.pid}\n`);
  const note = (fullDiskAccess, pid = helper.pid) => write(facts, JSON.stringify({ v: 1, pid, observedAt: Date.now(), started, screenRecording: true, accessibility: true, fullDiskAccess }));
  const box = await served(where);
  try {
    note(false);
    const denied = (await box.ask("kuvo/box/permissions")).body;
    assert.equal(denied.applies, true);
    assert.equal(denied.known, true);
    assert.deepEqual(denied.list, [filesPermission(false)]);
    note(true);
    assert.deepEqual((await box.ask("kuvo/box/permissions")).body.list, [filesPermission(true)]);
    note(true, keeper.pid);
    assert.deepEqual((await box.ask("kuvo/box/permissions")).body.list, [filesPermission(null)]);
    note(true);
    const stale = JSON.parse(fs.readFileSync(facts, "utf8"));
    write(facts, JSON.stringify({ ...stale, observedAt: Date.now() - 60_000 }));
    assert.deepEqual((await box.ask("kuvo/box/permissions")).body.list, [filesPermission(null)]);
  } finally {
    helper.kill();
    keeper.kill();
    await box.stop();
  }
});
