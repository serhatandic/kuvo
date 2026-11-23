import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";

test("duplicate tool workers run one command and keep only its masked result", async () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-tool-command-"));
  removeAtEnd(home);
  const script = fileURLToPath(LINK_BUNDLE);
  const id = "C".repeat(43);
  const effect = path.join(home, "effect");
  const secret = "fixture-only-vault-value";
  const launch = () => {
    const child = spawn(process.execPath, [script, "box", "tool-command-worker", id], {
      env: { HOME: home, KUVO_HOME: home, PATH: process.env.PATH, TMPDIR: home },
      stdio: ["pipe", "ignore", "inherit"],
    });
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify({
      command: 'printf once >> "$EFFECT"; sleep 1; printf "%s" "$TEST_VALUE"',
      folder: home,
      env: { PATH: process.env.PATH, HOME: home, EFFECT: effect, TEST_VALUE: secret },
      hidden: [{ item: "Fixture", value: secret }],
      timeout: 10000,
    }));
    return once(child, "exit");
  };
  const exits = await Promise.all([launch(), launch()]);
  assert.deepEqual(exits, [[0, null], [0, null]]);
  assert.equal(fs.readFileSync(effect, "utf8"), "once");
  const kept = fs.readFileSync(path.join(home, "run", "tool-commands", id + ".json"), "utf8");
  assert.ok(!kept.includes(secret));
  assert.deepEqual(JSON.parse(kept).result, { code: 0, output: "[vault: Fixture]", timedOut: false });
});
