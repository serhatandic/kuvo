import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import { test } from "node:test";
import { setTimeout } from "node:timers/promises";
import { PROCESS_SCOPE, watchTestHome } from "./test-home-processes.js";

const makeHome = () => fs.realpathSync(fs.mkdtempSync("/tmp/k"));
const live = pid => {
  const result = spawnSync("/bin/ps", ["-p", String(pid), "-o", "stat="], { encoding: "utf8" });
  if (result.error) throw result.error;
  return Boolean(result.stdout.trim()) && !result.stdout.trim().startsWith("Z");
};

test("test-home teardown waits for detached processes and leaves an unrelated observer alone", { timeout: 15_000 }, async () => {
  const root = makeHome();
  const { scope, stop } = await watchTestHome(root);
  const own = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
    cwd: root, detached: true, stdio: "ignore", env: { ...process.env, [PROCESS_SCOPE]: scope },
  });
  // This process is inspecting the test home. It did not come from the fixture.
  const decoy = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)", root + "/file"], {
    cwd: root, detached: true, stdio: "ignore",
  });
  const script = root + "/keeper";
  fs.writeFileSync(script, "while :; do /bin/sleep 1; done\n");
  const shell = spawn("/bin/sh", [script], { detached: true, stdio: "ignore" });
  const ownDone = once(own, "exit"), decoyDone = once(decoy, "exit"), shellDone = once(shell, "exit");
  try {
    await Promise.all([once(own, "spawn"), once(decoy, "spawn"), once(shell, "spawn")]);
    assert.equal(live(own.pid), true);
    assert.equal(live(shell.pid), true);
    await stop();
    assert.equal(live(own.pid), false);
    assert.equal(live(shell.pid), false);
    assert.equal(fs.existsSync(root), false);
    assert.equal(live(decoy.pid), true);
  } finally {
    own.kill("SIGKILL");
    decoy.kill("SIGKILL");
    shell.kill("SIGKILL");
    await Promise.all([ownDone, decoyDone, shellDone]);
    fs.rmSync(root, { recursive: true, force: true });
  }
});

for (const outcome of ["SIGTERM", "SIGKILL", "assertion failure", "test timeout"]) {
  test(`test-home teardown survives ${outcome}`, { timeout: 15_000 }, async () => {
    const root = makeHome();
    const file = new URL("./test-home-processes.js", import.meta.url).href;
    const signal = outcome.startsWith("SIG") ? outcome : null;
    const script = `
      import { PROCESS_SCOPE, watchTestHome } from ${JSON.stringify(file)};
      import { spawn } from "node:child_process";
      import { once } from "node:events";
      import { after, test } from "node:test";
      import assert from "node:assert/strict";
      const root = process.argv[1];
      const { scope, stop } = await watchTestHome(root);
      const worker = spawn(process.execPath, ["-e", "setTimeout(() => {}, 8000)"], {
        cwd: root, detached: true, stdio: "ignore", env: { ...process.env, [PROCESS_SCOPE]: scope },
      });
      await once(worker, "spawn");
      console.error("worker PID " + worker.pid);
      ${signal ? `process.kill(process.pid, ${JSON.stringify(signal)});` : `
      after(stop);
      test("fixture", { timeout: 50 }, async () => {
        ${outcome === "test timeout" ? "await new Promise(() => {});" : 'assert.fail("intentional fixture failure");'}
      });
      `}
    `;
    const { NODE_TEST_CONTEXT: _, ...env } = process.env;
    const result = spawnSync(process.execPath, ["--input-type=module", "-e", script, root], { env, encoding: "utf8", timeout: 10_000 });
    try {
      assert.ifError(result.error);
      assert.equal(result.signal, signal, result.stderr);
      if (!signal) {
        assert.equal(result.status, 1, result.stdout);
        assert.match(result.stdout, outcome === "test timeout" ? /test timed out/ : /intentional fixture failure/);
      }
      const worker = Number(result.stderr.match(/worker PID (\d+)/)?.[1]);
      assert.ok(Number.isInteger(worker) && worker > 1, result.stderr);
      for (let round = 0; round < 30 && fs.existsSync(root); round++) await setTimeout(100);
      assert.equal(live(worker), false, `worker ${worker} survived ${outcome}`);
      assert.equal(fs.existsSync(root), false);
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });
}
