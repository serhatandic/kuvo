// A Kuvo Helper from before beta.31 after an update (PROTOCOL.md, "This Mac's box: its helper";
// compat/helper-handover.ts): it never leaves for the new build in its place by itself, so the box's
// server it started, on the new code, hands it over once (it's told to stop as launchd tells it, and
// once it's gone, started as the app starts a stopped one), at once, what's at work running on; never
// when you stopped Kuvo on this Mac; one still old after its try is noted, not tried again; a helper
// that renews itself is never touched.
//
// The real box script (`serve`, and the hand-over it starts), in homes of the test's own laid out as
// a Mac's; Kuvo Helper, its keeper and launchd are stand-ins: the helper a small program at its place
// in the box (`bin/Kuvo Helper.app`) that starts `kuvo-start keep` in a session of its own and
// `kuvo-start serve` (which `exec`s the box's server, as the real one does) with its first line on
// stdin, starts serve again when it ends, and on SIGTERM closes serve's stdin, sends its keeper
// SIGTERM by its number and exits 0, as the helper from before beta.31 does
// (mac/helper/Sources/KuvoHelperKit/BoxHelper.swift at 5d9b7b8db, `finish`); the keeper starts
// the engine's turn in a session of its own, as `kuvo-launch` does; launchd's `launchctl` and
// `open` start that program for `kickstart` and `open -n`.
//
// Run with `node --test box/bridge-tests/helper-handover.test.js`.
import { removeTestHomeAtExit } from "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd } from "./at-end.js";

const built = fileURLToPath(LINK_BUNDLE);
const AGENT = "run.kuvo.code.machine";
const mac = process.platform === "darwin";
const root = mac ? fs.realpathSync(fs.mkdtempSync("/tmp/k")) : null;
if (root) removeTestHomeAtExit(root);

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
const start = path.join(K, "bin", "kuvo-start");
const keeper = spawn("/bin/sh", [start, "keep"], { detached: true, stdio: "ignore" });
const startServe = () => {
  if (finishing) return;
  serve = spawn("/bin/sh", [start, "serve"], { stdio: ["pipe", "ignore", fs.openSync(path.join(H, "serve.log"), "a")] });
  serve.stdin.write(JSON.stringify(renews ? { password: "pw", renews: true } : { password: "pw" }) + "\\n");
  serve.once("exit", () => setTimeout(startServe, 50));
};
startServe();
process.on("SIGTERM", () => {
  finishing = true;
  if (process.env.FAKE_PAUSE_ON_TERM) fs.writeFileSync(path.join(K, "run", "paused"), "");
  serve.stdin.end();
  try { process.kill(keeper.pid, "SIGTERM"); } catch {}
  const end = () => process.exit(0);
  if (serve.exitCode !== null || serve.signalCode !== null) end(); else { serve.once("exit", end); setTimeout(end, 3000); }
});
setInterval(() => {}, 60000);
`;

/** A Mac's box in a home of its own, its helper's launch agent in place, and launchd's stand-in. */
function macHome() {
  const home = fs.mkdtempSync(path.join(root, "kuvo-helper-handover-"));
  const K = path.join(home, ".kuvo-code"), tools = path.join(home, "tools");
  const bundle = path.join(K, "bin", "Kuvo Helper.app"), program = path.join(bundle, "Contents", "MacOS", "Kuvo Helper");
  write(path.join(K, "box", "kind"), "mac\n");
  write(path.join(K, "bin", "box-version"), "0.10.0");
  write(path.join(K, "bin", "kuvo-link.js"), fs.readFileSync(built, "utf8"));
  // The keeper's script: `serve` execs the box's server (so the helper is its parent); `keep` notes
  // itself, starts the engine's turn in a session of its own once (as kuvo-launch does), and runs on.
  write(path.join(K, "bin", "kuvo-start"), `#!/bin/sh
K="$HOME/.kuvo-code"
case "$1" in
serve) exec ${JSON.stringify(process.execPath)} "$K/bin/kuvo-link.js" serve ;;
keep) echo $$ >>"$K/run/keepers.log"
  [ -f "$K/run/turn.pid" ] || ${JSON.stringify(process.execPath)} -e 'const c = require("node:child_process").spawn(process.execPath, ["-e", "setInterval(() => {}, 60000)", process.argv[1]], { detached: true, stdio: "ignore" }); require("node:fs").writeFileSync(process.argv[1], String(c.pid)); c.unref()' "$K/run/turn.pid"
  exec ${JSON.stringify(process.execPath)} -e 'setInterval(() => {}, 60000)' "$K" ;;
esac
exit 3
`, 0o755);
  fs.mkdirSync(path.join(K, "run"), { recursive: true, mode: 0o700 });
  write(path.join(home, "Library", "LaunchAgents", `${AGENT}.plist`), `<plist><dict><key>Label</key><string>${AGENT}</string><key>ProgramArguments</key><array><string>${program}</string><string>keep</string></array><key>AssociatedBundleIdentifiers</key><array><string>run.kuvo.code</string></array></dict></plist>\n`, 0o644);
  // launchd: `print` says the job's process while the one it started runs; `kickstart` starts the
  // helper where none runs (FAKE_HELPER_RENEWS from `launchd.env`: a launchd that starts one still old).
  write(path.join(tools, "launchctl"), `#!/bin/sh
H=${JSON.stringify(home)}
[ -d "${root}" ] && [ ! -e "${root}/.closing" ] || exit 1
echo "$*" >>"$H/launchctl.log"
p=$(cat "$H/launchd.pid" 2>/dev/null)
case "$1" in
print) if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then printf 'gui/501/${AGENT} = {\\n\\tstate = running\\n\\tpid = %s\\n}\\n' "$p"; else printf 'gui/501/${AGENT} = {\\n\\tstate = not running\\n}\\n'; fi ;;
print-disabled) printf 'disabled services = {\\n}\\n' ;;
kickstart) [ -f "$H/kickstart.refused" ] && { echo "Could not kickstart service: 5: Input/output error" >&2; exit 5; }
  if [ -n "$p" ] && kill -0 "$p" 2>/dev/null; then exit 0; fi
  env -u FAKE_HELPER_RENEWS -u FAKE_PAUSE_ON_TERM $(cat "$H/launchd.env" 2>/dev/null) ${JSON.stringify(program)} keep --home ${JSON.stringify(K)} </dev/null >/dev/null 2>&1 &
  echo $! >"$H/launchd.pid" ;;
esac
exit 0
`, 0o755);
  // \`open -g -j -n <bundle> --args …\`: its program started, as Launch Services would.
  write(path.join(tools, "open"), `#!/bin/sh
[ -d "${root}" ] && [ ! -e "${root}/.closing" ] || exit 1
echo "$*" >>${JSON.stringify(path.join(home, "open.log"))}
[ "$1 $2 $3" = "-g -j -n" ] && [ "$4" = ${JSON.stringify(bundle)} ] && [ "$5" = --args ] || exit 1
b=$4; shift 5
"$b/Contents/MacOS/Kuvo Helper" "$@" </dev/null >/dev/null 2>&1 &
exit 0
`, 0o755);
  write(path.join(tools, "crontab"), "#!/bin/sh\nexit 1\n", 0o755);
  // Its environment: nothing of Kuvo's but the test's own.
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KUVO_")));
  Object.assign(env, { HOME: home, KUVO_HOME: K, TMPDIR: home, KUVO_TEST_UNINSTALL_TOOLS: tools, KUVO_TEST_CODE_EVERY: "100", KUVO_TEST_NO_PEERS: "1" });
  return {
    home, K, env,
    /** The helper's build in place: one from before beta.31 (it doesn't renew itself) or since. */
    bundle(renews, sealed = renews ? "a build since beta.31" : "beta.30") {
      // (A copy of its own, swapped into place, as an update puts one: \`swapBundle\`.)
      const fresh = `${bundle}.new`;
      fs.rmSync(fresh, { recursive: true, force: true });
      write(path.join(fresh, "Contents", "MacOS", "Kuvo Helper"), helperProgram(renews), 0o755);
      write(path.join(fresh, "Contents", "Info.plist"), `<plist><dict>${renews ? "\n\t<key>KuvoHelperRenews</key>\n\t<true/>" : ""}\n</dict></plist>\n`, 0o644);
      write(path.join(fresh, "Contents", "_CodeSignature", "CodeResources"), sealed);
      fs.rmSync(bundle, { recursive: true, force: true });
      fs.renameSync(fresh, bundle);
    },
    /** The helper launchd started at login, as its build in place then was. */
    login(more = {}) {
      const child = spawn(program, ["keep", "--home", K], { env: { ...env, ...more }, stdio: "ignore" });
      fs.writeFileSync(path.join(home, "launchd.pid"), String(child.pid));
      return child.pid;
    },
    /** An update: the new build in place, and the box's files changed (its server leaves for them). */
    update() {
      this.bundle(true);
      write(path.join(K, "bin", "box-version"), "0.11.0");
    },
    helpers: () => read(path.join(home, "helpers.log")).trim().split("\n").filter(Boolean).map(line => { const [pid, how] = line.split(" "); return { pid: Number(pid), how }; }),
    kickstarts: () => read(path.join(home, "launchctl.log")).split("\n").filter(line => line.startsWith("kickstart")).length,
    record: () => { try { return JSON.parse(read(path.join(K, "run", "helper-handover.json"))); } catch { return null; } },
    serving: () => { try { return JSON.parse(read(path.join(K, "run", "serve.json"))).pid; } catch { return null; } },
    serveLog: () => read(path.join(home, "serve.log")),
    keepers: () => read(path.join(K, "run", "keepers.log")).trim().split("\n").filter(Boolean).map(Number),
    turn: () => Number(read(path.join(K, "run", "turn.pid"))) || null,
  };
}

test("an old helper still running after an update is handed over once, at once, whatever is at work: the new build runs, and is left as it is", { skip: !mac, timeout: 90_000 }, async () => {
  const box = macHome();
  box.bundle(false);
  const old = box.login();
  await until(() => box.serving() && box.turn(), "its server answering, and its keeper's turn at work");
  const turn = box.turn(), [oldKeeper] = box.keepers();

  box.update();
  // Its server leaves for the new code; started again by the old helper, it hands that helper over.
  await until(() => box.helpers().length === 2, `a new helper started (log: ${box.serveLog()})`);
  const [first, second] = box.helpers();
  assert.deepEqual([first.pid, first.how], [old, "old"]);
  assert.equal(second.how, "renews", "the new build, started by launchd");
  await until(() => !alive(old), "the old helper gone");
  await until(() => box.record()?.outcome === "handed", "the hand-over noted as done");
  assert.equal(box.kickstarts(), 1, "launchd asked once");
  // What was at work, started by the old helper's keeper in a session of its own, runs on: the
  // keeper went with its helper, and the new helper's keeper took over.
  await until(() => box.keepers().length === 2, "the new helper's keeper");
  assert.ok(!alive(oldKeeper), "the old keeper went with its helper");
  assert.ok(alive(turn), "the turn at work was never stopped, nor waited for");

  // The new helper's server: nothing more (it renews itself), however often it starts.
  await until(() => read(path.join(box.home, "launchd.pid")).trim() === String(second.pid) && box.serving(), "the new one serving");
  write(path.join(box.K, "bin", "box-version"), "0.11.1");
  const serving = box.serving();
  await until(() => box.serving() && box.serving() !== serving, "its server started again");
  assert.equal(box.kickstarts(), 1);
  assert.equal(box.helpers().length, 2);
  assert.ok(alive(second.pid), "the new helper left as it is");
  assert.equal(box.record().stillOld, undefined);
});

test("not while an update is under way: the old helper is handed over once it's done", { skip: !mac, timeout: 90_000 }, async () => {
  const box = macHome();
  box.bundle(false);
  const old = box.login();
  await until(() => box.serving(), "its server answering");
  // An update under way (its process holds run/.updating), its files in place already.
  const updater = spawn(process.execPath, ["-e", "setInterval(() => {}, 60000)", "kuvo-link-updating"], { env: box.env, stdio: "ignore" });
  write(path.join(box.K, "run", ".updating", "pid"), String(updater.pid));
  box.update();
  // Its server started again meanwhile (a crash, say): it waits.
  const serving = box.serving();
  process.kill(serving, "SIGTERM");
  await until(() => box.serving() && box.serving() !== serving, "its server started again");
  assert.equal(box.record(), null, "nothing while the update runs");
  assert.equal(box.kickstarts(), 0);
  assert.ok(alive(old));
  // The update done: handed over at the server's next look.
  fs.rmSync(path.join(box.K, "run", ".updating"), { recursive: true });
  updater.kill();
  await until(() => box.record()?.outcome === "handed", `handed over (log: ${box.serveLog()})`);
  await until(() => box.helpers().length === 2, "the new one started");
  assert.equal(box.kickstarts(), 1);
  assert.deepEqual(box.helpers().map(each => each.how), ["old", "renews"]);
});

test("one launchd refuses to start is opened instead, so this Mac is never left without its helper", { skip: !mac, timeout: 90_000 }, async () => {
  const box = macHome();
  box.bundle(false);
  const old = box.login();
  await until(() => box.serving(), "its server answering");
  fs.writeFileSync(path.join(box.home, "kickstart.refused"), "");
  box.update();
  await until(() => box.helpers().length === 2, `a new helper started (log: ${box.serveLog()})`);
  assert.equal(box.helpers()[1].how, "renews");
  assert.ok(!alive(old));
  await until(() => box.record()?.outcome === "handed", "the hand-over noted as done");
  assert.match(box.record().message, /opened until you log out: launchd didn't start it/);
  assert.match(read(path.join(box.home, "open.log")), /^-g -j -n .*Kuvo Helper\.app --args keep --home /);
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

  // The same release taken again after a rollback (a copy of its own in place): tried once more.
  fs.rmSync(path.join(box.home, "launchd.env"));
  box.bundle(true);
  write(path.join(box.K, "bin", "box-version"), "0.11.3");
  await until(() => box.helpers().length === 3, `handed over again (log: ${box.serveLog()})`);
  assert.equal(box.helpers()[2].how, "renews");
  assert.equal(box.kickstarts(), 2);
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
  box.bundle(true, "a newer build");
  write(path.join(box.K, "bin", "box-version"), "0.11.0");
  await until(() => box.serving() && box.serving() !== serving, "its server started again on the new code");
  assert.equal(box.record(), null);
  assert.equal(box.kickstarts(), 0);
  assert.ok(alive(own));
});
