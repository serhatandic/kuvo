// Run real test subprocesses and verify their boxes and keepers disappear when the runner ends.
import { ownPath } from "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { setTimeout } from "node:timers/promises";

function live(pid) {
  const result = spawnSync("/bin/ps", ["-p", String(pid), "-o", "stat="], { encoding: "utf8" });
  if (result.error) throw result.error;
  const state = result.stdout.trim();
  return Boolean(state) && !state.startsWith("Z");
}

const safety = new URL("./test-safety.js", import.meta.url).href;
const bundle = LINK_BUNDLE.href;
const keeperSource = new URL("../src/keeper/kuvo-start", import.meta.url);

test("killing a runner leaves no keeper or worker in its test home", { timeout: 15_000 }, async () => {
  const root = fs.realpathSync(fs.mkdtempSync("/tmp/kuvo-keeper-watch-"));
  const install = path.join(root, ".kuvo-code");
  const bin = path.join(install, "bin");
  const run = path.join(install, "run");
  const ready = path.join(root, "ready");
  const started = path.join(run, "started");
  const recordedPids = () => [started, ready].map(file => {
    try { return Number(fs.readFileSync(file, "utf8")); }
    catch (error) { if (error.code !== "ENOENT") throw error; return 0; }
  });
  fs.mkdirSync(bin, { recursive: true });
  fs.mkdirSync(run);
  fs.mkdirSync(path.join(install, "box"));
  fs.writeFileSync(path.join(install, "box", "kind"), "mac\n");
  fs.copyFileSync(keeperSource, path.join(bin, "kuvo-start"));
  fs.writeFileSync(path.join(bin, "caffeinate"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  // Park the real keeper in its first round and record the PID of its sleep.
  fs.writeFileSync(path.join(bin, "sleep"), '#!/bin/sh\nprintf "%s\\n" "$$" > "$HOME/ready"\nexec /bin/sleep 60\n', { mode: 0o700 });
  const runner = spawn(process.execPath, ["--input-type=module", "-e", `
    import { ownPath, removeTestHomeAtExit } from ${JSON.stringify(safety)};
    import { spawn } from "node:child_process";
    import fs from "node:fs";
    const [root, bin, run] = process.argv.slice(1);
    removeTestHomeAtExit(ownPath(root));
    const keeper = spawn("/usr/bin/env", ["-i", "HOME=" + root, "PATH=" + bin + ":/usr/bin:/bin",
      "/bin/sh", bin + "/kuvo-start", "keep"], { detached: true, stdio: ["ignore", "ignore", "inherit"] });
    fs.writeFileSync(run + "/started", String(keeper.pid));
  `, root, bin, run], { stdio: ["ignore", "ignore", "inherit"] });
  const exited = once(runner, "exit");
  let pids = [];
  try {
    for (let i = 0; i < 250 && runner.exitCode === null; i++) {
      pids = recordedPids();
      if (pids.every(pid => pid > 1)) break;
      await setTimeout(20);
    }
    assert.ok(pids.length === 2 && pids.every(pid => pid > 1 && live(pid)), "the keeper reached its first round");
    assert.ok(runner.kill("SIGKILL"));
    assert.deepEqual(await exited, [null, "SIGKILL"]);
    for (let i = 0; i < 50 && (fs.existsSync(root) || pids.some(live)); i++) await setTimeout(100);
    assert.deepEqual(pids.filter(live), [], "processes survived from the keeper's home");
    assert.equal(fs.existsSync(root), false);
  } finally {
    const startedPids = new Set([...pids, ...recordedPids()]);
    if (runner.exitCode === null && runner.signalCode === null) runner.kill("SIGKILL");
    await exited;
    for (const pid of startedPids) {
      if (pid > 1 && live(pid)) try { process.kill(pid, "SIGKILL"); } catch (error) { if (error.code !== "ESRCH") throw error; }
    }
    fs.rmSync(root, { recursive: true, force: true });
  }
});

const fixtureSource = `
import { removeTestHomeAtExit } from ${JSON.stringify(safety)};
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { setTimeout } from "node:timers/promises";

const scenario = process.env.FIXTURE_OUTCOME;
const root = fs.realpathSync(fs.mkdtempSync("/tmp/k"));
removeTestHomeAtExit(root);
const home = path.join(root, ".kuvo-code");
fs.mkdirSync(path.join(home, "run"), { recursive: true });
process.env.KUVO_HOME = home;
const resident = path.join(home, "bin", "kuvo-link.js");
fs.mkdirSync(path.dirname(resident), { recursive: true });
fs.copyFileSync(fileURLToPath(${JSON.stringify(bundle)}), resident);
const child = spawn(process.execPath, [resident, "serve", "--resident"], {
  cwd: root, env: { PATH: process.env.PATH, HOME: root, KUVO_HOME: home, TMPDIR: root, KUVO_TEST_NO_PEERS: "1" },
  detached: true, stdio: ["ignore", "ignore", "inherit"],
});
// A keeper that drops its environment still has its script in the registered fixture root.
const keeper = path.join(root, "keeper");
fs.writeFileSync(keeper, "while :; do /bin/sleep 1; done\\n");
const shell = spawn("/usr/bin/env", ["-i", "/bin/sh", keeper], { detached: true, stdio: "ignore" });
for (let tries = 0; !fs.existsSync(path.join(home, "run", "serve.json")); tries++) {
  if (tries === 450 || child.exitCode !== null) throw new Error("The resident didn't publish its address");
  await setTimeout(20);
}
fs.writeFileSync(process.env.FIXTURE_READY, JSON.stringify({ root, box: child.pid, keeper: shell.pid, worker: process.pid }));
if (!scenario.startsWith("SIG")) {
  child.unref();
  shell.unref();
}
test("fixture", { timeout: 100 }, async () => {
  if (scenario === "test timeout") await setTimeout(500);
  if (scenario === "assertion failure") throw new Error("intentional fixture failure");
});
if (scenario.startsWith("SIG")) setInterval(() => {}, 1000);
`;

for (const outcome of ["SIGKILL", "SIGTERM", "pass", "assertion failure", "test timeout"]) {
  test(`test run leaves no box or keeper after ${outcome}`, { timeout: 20_000 }, async () => {
    const folder = fs.mkdtempSync("/tmp/kuvo-lifetime-test-");
    const fixture = path.join(folder, "fixture.mjs");
    const ready = path.join(folder, "ready");
    fs.writeFileSync(fixture, fixtureSource);
    const { NODE_TEST_CONTEXT: _, ...env } = process.env;
    const runner = spawn(process.execPath, ["--test", fixture], {
      env: { ...env, FIXTURE_READY: ready, FIXTURE_OUTCOME: outcome }, stdio: ["ignore", "pipe", "pipe"],
    });
    const exited = once(runner, "exit");
    let output = "";
    runner.stdout.on("data", chunk => { output += chunk; });
    runner.stderr.on("data", chunk => { output += chunk; });
    let fixtureInfo;
    let observer;
    let observerDone;
    try {
      for (let round = 0; round < 500 && !fs.existsSync(ready) && runner.exitCode === null; round++) await setTimeout(20);
      assert.ok(fs.existsSync(ready), output);
      fixtureInfo = JSON.parse(fs.readFileSync(ready, "utf8"));
      const { box, keeper, root, worker } = fixtureInfo;
      if (outcome.startsWith("SIG")) {
        assert.ok(live(box));
        observer = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)", root + "/keeper"], {
          cwd: root, detached: true, stdio: "ignore",
        });
        observerDone = once(observer, "exit");
        await once(observer, "spawn");
        // SIGKILL targets the CLI, and SIGTERM targets the worker to exercise both ownership paths.
        if (outcome === "SIGKILL") runner.kill("SIGKILL");
        else process.kill(worker, "SIGTERM");
      }
      const [code, signal] = await exited;
      if (outcome === "SIGKILL") assert.equal(signal, "SIGKILL");
      else assert.equal(code, outcome === "pass" ? 0 : 1, output);
      if (outcome === "test timeout") assert.match(output, /test timed out/);
      if (outcome === "assertion failure") assert.match(output, /intentional fixture failure/);
      for (let round = 0; round < 65 && (fs.existsSync(root) || live(box) || live(keeper)); round++) await setTimeout(100);
      assert.equal(live(box), false, `box ${box} survived ${outcome}`);
      assert.equal(live(keeper), false, `keeper ${keeper} survived ${outcome}`);
      assert.equal(fs.existsSync(root), false);
      if (observer) assert.equal(live(observer.pid), true, "unrelated observer");
    } finally {
      if (observer) {
        observer.kill("SIGKILL");
        await observerDone;
      }
      if (runner.exitCode === null && runner.signalCode === null) runner.kill("SIGKILL");
      await exited;
      if (fixtureInfo) {
        for (const pid of [fixtureInfo.box, fixtureInfo.keeper, fixtureInfo.worker]) {
          if (live(pid)) try { process.kill(pid, "SIGKILL"); } catch (error) { if (error.code !== "ESRCH") throw error; }
        }
        fs.rmSync(ownPath(fixtureInfo.root), { recursive: true, force: true });
      }
      fs.rmSync(folder, { recursive: true, force: true });
    }
  });
}
