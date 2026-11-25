// Stop on a command the engine runs (`POST kuvo/shell/<id>/stop`, `shell.stop`): its process group
// signalled where it has one, else the engine asked to remove it; one that ended, or isn't there,
// is none to stop. (A coding agent's is in `agents-codex.test.mjs`.)
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import path from "node:path";
import test from "node:test";
import { until } from "./adapter.mjs";
import { bench } from "./bench.mjs";

// (A process group to signal is a Unix one.)
const skip = process.platform === "win32" ? "process groups are Unix's" : false;

test("Stop on the engine's command signals its process group, else has the engine remove it; one that ended isn't stopped", { timeout: 60000, skip }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  const folder = path.join(lab.home, "..", "work");
  // A command leading a process group of its own, as the engine starts them.
  const dev = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { detached: true, stdio: "ignore" });
  const ended = new Promise(done => dev.once("exit", (code, signal) => done(signal)));
  t.after(() => { if (dev.exitCode === null && dev.signalCode === null) process.kill(dev.pid, "SIGKILL"); });
  lab.engine.shells = [
    { id: "sh_dev", status: "running", command: "npm run dev", pid: dev.pid, folder },
    { id: "sh_nopid", status: "running", command: "npm run watch", folder },
    { id: "sh_done", status: "exited", command: "npm test", folder },
  ];
  const ask = lab.connect(await lab.start().ready());
  const stop = (id) => ask("POST", `kuvo/shell/${id}/stop`, {}, { "x-opencode-directory": encodeURIComponent(folder) });

  const signalled = await stop("sh_dev");
  assert.equal(signalled.status, 200, signalled.text);
  assert.deepEqual(signalled.json, { stopped: "signal" });
  assert.equal(await until(() => ended, "Its process never ended."), "SIGTERM");

  const removed = await stop("sh_nopid");
  assert.equal(removed.status, 200, removed.text);
  assert.deepEqual(removed.json, { stopped: "removed" });
  assert.ok(!lab.engine.shells.some(job => job.id === "sh_nopid"), "the engine was asked to remove it");

  assert.equal((await stop("sh_done")).status, 404, "one that ended");
  assert.equal((await stop("sh_nothing")).status, 404, "one that isn't there");
});
