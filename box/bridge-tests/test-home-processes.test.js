import { ownPath } from "./test-safety.js";
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
const bundle = new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url).href;
const standIn = new URL("./relay-stand-in.js", import.meta.url).href;
const fixtureSource = `
import { removeTestHomeAtExit } from ${JSON.stringify(safety)};
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { setTimeout } from "node:timers/promises";
import { relayStandIn } from ${JSON.stringify(standIn)};

const scenario = process.env.FIXTURE_OUTCOME;
const root = fs.realpathSync(fs.mkdtempSync("/tmp/k"));
removeTestHomeAtExit(root);
const home = path.join(root, ".kuvo-code");
fs.mkdirSync(path.join(home, "run"), { recursive: true });
process.env.KUVO_HOME = home;
const link = await import(${JSON.stringify(bundle)});
const relay = await relayStandIn(link);
const secret = crypto.randomBytes(32);
fs.writeFileSync(path.join(home, "run", "link.json"), JSON.stringify({
  relay: relay.url, secret: secret.toString("base64url"), key: link.newKey().toString("base64url"),
  sign: link.newSigningSeed(), name: "leftover-fixture", phones: [],
  pairing: { token: crypto.randomBytes(16).toString("base64url"), expires: Date.now() + 60_000, issuer: null },
}));
const child = spawn(process.execPath, [fileURLToPath(${JSON.stringify(bundle)}), "run"], {
  cwd: root, env: { PATH: process.env.PATH, HOME: root, KUVO_HOME: home }, detached: true, stdio: ["ignore", "ignore", "inherit"],
});
// A keeper that drops its environment still has its script in the registered fixture root.
const keeper = path.join(root, "keeper");
fs.writeFileSync(keeper, "while :; do /bin/sleep 1; done\\n");
const shell = spawn("/usr/bin/env", ["-i", "/bin/sh", keeper], { detached: true, stdio: "ignore" });
for (let tries = 0; !relay.has(link.addressOf(secret)); tries++) {
  if (tries === 500 || child.exitCode !== null) throw new Error("The test box didn't connect");
  await setTimeout(20);
}
fs.writeFileSync(process.env.FIXTURE_READY, JSON.stringify({ root, box: child.pid, keeper: shell.pid, worker: process.pid }));
if (!scenario.startsWith("SIG")) {
  child.unref();
  shell.unref();
  relay.close();
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
      for (let round = 0; round < 40 && fs.existsSync(root); round++) await setTimeout(100);
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
