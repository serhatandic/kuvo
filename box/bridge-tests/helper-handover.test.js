// A Kuvo Helper from before beta.31 after an update (PROTOCOL.md, "This Mac's box: its helper";
// compat/helper-handover.ts): it never leaves for the new build in its place by itself, so the box's
// server it started, on the new code, hands it over once (it's told to stop as launchd tells it, and
// once it's gone, started as the app starts a stopped one), at once, what's at work running on; never
// when you stopped Kuvo on this Mac; one still old after its try is noted, not tried again; a helper
// that renews itself is never touched.
//
// The real box script (`serve`, and the hand-over it starts), in homes of the test's own laid out as
// a Mac's; Kuvo Helper and launchd are stand-ins: the helper a small program at its place in the box
// (`bin/Kuvo Helper.app`) that starts `serve` with its first line on stdin, starts it again when it
// ends, and on SIGTERM closes its stdin and exits 0, as the helper from before beta.31 does; launchd's
// `launchctl` one that starts that program for `kickstart`.
//
// Run with `node --test box/bridge-tests/helper-handover.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const built = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const AGENT = "run.kuvo.code.machine";
const mac = process.platform === "darwin";

const write = (file, text, mode = 0o600) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
};
const read = file => { try { return fs.readFileSync(file, "utf8"); } catch { return ""; } };
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };

async function until(check, what) {
  for (let i = 0; i < 600; i++) {
    if (check()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.fail(`never: ${what}`);
}

// Every process a test started, or its stand-ins did, stopped at the end by its number: the helpers
// as launchd stops them (their servers with them), then what's left.
const started = new Set(), homes = [], children = [];
atEnd(async () => {
  for (const home of homes) {
    for (const line of read(path.join(home, "helpers.log")).split("\n")) if (Number(line.split(" ")[0]) > 1) started.add(Number(line.split(" ")[0]));
  }
  for (const pid of started) { try { process.kill(pid, "SIGTERM"); } catch {} }
  for (let i = 0; i < 100 && [...started].some(alive); i++) await new Promise(resolve => setTimeout(resolve, 50));
  for (const pid of started) { try { process.kill(pid, "SIGKILL"); } catch {} }
  await Promise.all(children.map(child => child.exitCode !== null || child.signalCode !== null ? null : new Promise(resolve => child.once("exit", resolve))));
});

/** The helper stand-in: says `renews` on serve's stdin as its build does (`RENEWS`, written into it,
 * or FAKE_HELPER_RENEWS for one launchd starts still old); notes each start in `helpers.log`. */
const helperProgram = renews => `#!${process.execPath}
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const H = process.env.HOME, K = process.env.KUVO_HOME;
const renews = process.env.FAKE_HELPER_RENEWS ? process.env.FAKE_HELPER_RENEWS === "1" : ${renews};
fs.appendFileSync(path.join(H, "helpers.log"), process.pid + " " + (renews ? "renews" : "old") + "\\n");
let serve = null, finishing = false;
const startServe = () => {
  if (finishing) return;
  serve = spawn(process.execPath, [path.join(K, "bin", "kuvo-link.js"), "serve"], { stdio: ["pipe", "ignore", fs.openSync(path.join(H, "serve.log"), "a")] });
  serve.stdin.write(JSON.stringify(renews ? { password: "pw", renews: true } : { password: "pw" }) + "\\n");
  serve.once("exit", () => setTimeout(startServe, 50));
};
startServe();
process.on("SIGTERM", () => {
  finishing = true;
  if (process.env.FAKE_PAUSE_ON_TERM) fs.writeFileSync(path.join(K, "run", "paused"), "");
  serve.stdin.end();
  const end = () => process.exit(0);
  if (serve.exitCode !== null || serve.signalCode !== null) end(); else { serve.once("exit", end); setTimeout(end, 3000); }
});
setInterval(() => {}, 60000);
`;

/** A Mac's box in a home of its own, its helper's launch agent in place, and launchd's stand-in. */
function macHome() {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-helper-handover-")));
  removeAtEnd(home);
  homes.push(home);
  const K = path.join(home, ".kuvo-code"), tools = path.join(home, "tools");
  const bundle = path.join(K, "bin", "Kuvo Helper.app"), program = path.join(bundle, "Contents", "MacOS", "Kuvo Helper");
  write(path.join(K, "box", "kind"), "mac\n");
  write(path.join(K, "bin", "box-version"), "0.10.0");
  write(path.join(K, "bin", "kuvo-link.js"), fs.readFileSync(built, "utf8"));
  write(path.join(K, "bin", "kuvo-start"), "#!/bin/sh\nexit 0\n", 0o755);
  fs.mkdirSync(path.join(K, "run"), { recursive: true, mode: 0o700 });
  write(path.join(home, "Library", "LaunchAgents", `${AGENT}.plist`), `<plist><dict><key>Label</key><string>${AGENT}</string><key>ProgramArguments</key><array><string>${program}</string><string>keep</string></array><key>AssociatedBundleIdentifiers</key><array><string>run.kuvo.code</string></array></dict></plist>\n`, 0o644);
  // launchd: `print` says the job's process while the one it started runs; `kickstart` starts the
  // helper where none runs (FAKE_HELPER_RENEWS from `launchd.env`: a launchd that starts one still old).
  write(path.join(tools, "launchctl"), `#!/bin/sh
H=${JSON.stringify(home)}
echo "$*" >>"$H/launchctl.log"
p=$(cat "$H/launchd.pid" 2>/dev/null)
case "$1" in
print) if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then printf 'gui/501/${AGENT} = {\\n\\tstate = running\\n\\tpid = %s\\n}\\n' "$p"; else printf 'gui/501/${AGENT} = {\\n\\tstate = not running\\n}\\n'; fi ;;
print-disabled) printf 'disabled services = {\\n}\\n' ;;
kickstart) if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then exit 0; fi
  env $(cat "$H/launchd.env" 2>/dev/null) ${JSON.stringify(program)} keep --home ${JSON.stringify(K)} </dev/null >/dev/null 2>&1 &
  echo $! >"$H/launchd.pid" ;;
esac
exit 0
`, 0o755);
  for (const tool of ["open", "crontab"]) write(path.join(tools, tool), "#!/bin/sh\nexit 1\n", 0o755);
  // Its environment: nothing of Kuvo's but the test's own.
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KUVO_")));
  Object.assign(env, { HOME: home, KUVO_HOME: K, TMPDIR: home, KUVO_TEST_UNINSTALL_TOOLS: tools, KUVO_TEST_CODE_EVERY: "100", KUVO_TEST_NO_PEERS: "1" });
  const seal = text => write(path.join(bundle, "Contents", "_CodeSignature", "CodeResources"), text);
  return {
    home, K, env,
    /** The helper's build in place: one from before beta.31 (it doesn't renew itself) or since. */
    bundle(renews) {
      write(program, helperProgram(renews), 0o755);
      write(path.join(bundle, "Contents", "Info.plist"), `<plist><dict>${renews ? "\n\t<key>KuvoHelperRenews</key>\n\t<true/>" : ""}\n</dict></plist>\n`, 0o644);
      seal(renews ? "a build since beta.31" : "beta.30");
    },
    /** The helper launchd started at login, as its build in place then was. */
    login(more = {}) {
      const child = spawn(program, ["keep", "--home", K], { env: { ...env, ...more }, stdio: "ignore" });
      started.add(child.pid);
      children.push(child);
      fs.writeFileSync(path.join(home, "launchd.pid"), String(child.pid));
      return child.pid;
    },
    /** An update: the new build in place, and the box's files changed (its server leaves for them). */
    update() {
      this.bundle(true);
      write(path.join(K, "bin", "box-version"), "0.11.0");
    },
    helpers: () => read(path.join(home, "helpers.log")).trim().split("\n").filter(Boolean).map(line => { const [pid, how] = line.split(" "); started.add(Number(pid)); return { pid: Number(pid), how }; }),
    kickstarts: () => read(path.join(home, "launchctl.log")).split("\n").filter(line => line.startsWith("kickstart")).length,
    record: () => { try { return JSON.parse(read(path.join(K, "run", "helper-handover.json"))); } catch { return null; } },
    serving: () => { try { return JSON.parse(read(path.join(K, "run", "serve.json"))).pid; } catch { return null; } },
    serveLog: () => read(path.join(home, "serve.log")),
  };
}

/** Something at work, as the keeper starts it (in a session of its own): the engine's turn. */
function atWork(box) {
  const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 60000)"], { detached: true, stdio: "ignore", env: box.env });
  child.unref();
  started.add(child.pid);
  write(path.join(box.K, "run", "engine.json"), JSON.stringify({ pid: child.pid, port: 1, password: "x", version: "1.0.0" }));
  return child.pid;
}

test("an old helper still running after an update is handed over once, at once, whatever is at work: the new build runs, and is left as it is", { skip: !mac, timeout: 90_000 }, async () => {
  const box = macHome();
  box.bundle(false);
  const old = box.login();
  await until(() => box.serving(), "its server answering");
  const turn = atWork(box);

  box.update();
  // Its server leaves for the new code; started again by the old helper, it hands that helper over.
  await until(() => box.helpers().length === 2, `a new helper started (log: ${box.serveLog()})`);
  const [first, second] = box.helpers();
  assert.deepEqual([first.pid, first.how], [old, "old"]);
  assert.equal(second.how, "renews", "the new build, started by launchd");
  await until(() => !alive(old), "the old helper gone");
  await until(() => box.record()?.outcome === "handed", "the hand-over noted as done");
  assert.equal(box.kickstarts(), 1, "launchd asked once");
  assert.ok(alive(turn), "what was at work runs on, never waited for nor stopped");

  // The new helper's server: nothing more (it renews itself), however often it starts.
  await until(() => box.serving() && box.serving() !== null && read(path.join(box.home, "launchd.pid")).trim() === String(second.pid), "the new one serving");
  write(path.join(box.K, "bin", "box-version"), "0.11.1");
  const serving = box.serving();
  await until(() => box.serving() && box.serving() !== serving, "its server started again");
  assert.equal(box.kickstarts(), 1);
  assert.equal(box.helpers().length, 2);
  assert.ok(alive(second.pid), "the new helper left as it is");
  assert.equal(box.record().stillOld, undefined);
});

test("a helper still old after its one try is noted, never tried again", { skip: !mac, timeout: 90_000 }, async () => {
  const box = macHome();
  box.bundle(false);
  box.login();
  await until(() => box.serving(), "its server answering");
  // launchd starting one that's still old (whatever kept it so).
  fs.writeFileSync(path.join(box.home, "launchd.env"), "FAKE_HELPER_RENEWS=0");
  box.update();
  await until(() => box.helpers().length === 2, `a helper started again (log: ${box.serveLog()})`);
  const [, second] = box.helpers();
  assert.equal(second.how, "old");
  await until(() => box.record()?.stillOld, "still old, noted");
  assert.equal(box.record().stillOld.pid, second.pid);
  assert.equal(box.record().outcome, "handed");
  assert.match(box.serveLog(), /still the one from before the update/);
  // Its server started again, and again: no second try.
  for (const version of ["0.11.1", "0.11.2"]) {
    const serving = box.serving();
    write(path.join(box.K, "bin", "box-version"), version);
    await until(() => box.serving() && box.serving() !== serving, "its server started again");
  }
  assert.equal(box.kickstarts(), 1);
  assert.equal(box.helpers().length, 2);
  assert.ok(alive(second.pid), "left running");
});

test("Kuvo stopped on this Mac: its helper is never handed over, nor started, before or as it goes", { skip: !mac, timeout: 90_000 }, async () => {
  // Stopped (run/paused) while the old one still goes: nothing touches it.
  const stopped = macHome();
  stopped.bundle(false);
  const old = stopped.login();
  await until(() => stopped.serving(), "its server answering");
  write(path.join(stopped.K, "run", "paused"), "");
  const serving = stopped.serving();
  stopped.update();
  await until(() => stopped.serving() && stopped.serving() !== serving, "its server started again on the new code");
  assert.equal(stopped.record(), null, "no hand-over");
  assert.equal(stopped.kickstarts(), 0);
  assert.ok(alive(old));

  // Stop Kuvo on this Mac landing as it's handed over: it isn't started again.
  const racing = macHome();
  racing.bundle(false);
  const goes = racing.login({ FAKE_PAUSE_ON_TERM: "1" });
  await until(() => racing.serving(), "its server answering");
  racing.update();
  await until(() => racing.record()?.outcome === "stopped", `the hand-over noted as stopped (log: ${racing.serveLog()})`);
  assert.ok(!alive(goes), "the old one went, as you asked");
  assert.equal(racing.kickstarts(), 0, "and nothing started it again");
  assert.equal(racing.helpers().length, 1);
});

test("a helper that renews itself is never handed over by the box", { skip: !mac, timeout: 90_000 }, async () => {
  const box = macHome();
  box.bundle(true);
  const own = box.login();
  await until(() => box.serving(), "its server answering");
  // A newer build in place (it leaves for that by itself: exit 75); its server started again meanwhile.
  const serving = box.serving();
  write(path.join(box.K, "bin", "Kuvo Helper.app", "Contents", "_CodeSignature", "CodeResources"), "a newer build");
  write(path.join(box.K, "bin", "box-version"), "0.11.0");
  await until(() => box.serving() && box.serving() !== serving, "its server started again on the new code");
  assert.equal(box.record(), null);
  assert.equal(box.kickstarts(), 0);
  assert.ok(alive(own));
});
