// The resident HTTP routes and installed keeper are real. The stand-in for
// opencode's CLI and HTTP API lets the test control active turns and advance time.
import "./safety";
import { expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";

test("a new conversation starts a cold engine and the next conversation wakes it after idle", async () => {
  const home = fs.mkdtempSync(path.join(process.env.HOME!, "engine-lifetime-"));
  try {
    const env = { ...process.env, HOME: home, KUVO_HOME: path.join(home, ".kuvo-code"), BUN_BE_BUN: "1" };
    const child = Bun.spawn([process.execPath, "--config=/dev/null", "--no-env-file", path.join(import.meta.dir, "fixtures", "engine-lifetime.ts")], {
      env, stdout: "ignore", stderr: "pipe",
    });
    try {
      const [status, error] = await Promise.all([child.exited, new Response(child.stderr).text()]);
      expect(status, error).toBe(0);
    } finally {
      if (child.exitCode === null) child.kill();
      await child.exited;
    }
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
}, 60_000);
