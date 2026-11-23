import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const repo = fileURLToPath(new URL("../../", import.meta.url));
const built = path.join(repo, "box/dist/kuvo-link.js");
const script = fs.readFileSync(path.join(repo, "box/src/keeper/kuvo-start"), "utf8");
const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";

test("the keeper launches one resident, probes its generation, and stops it on off", { timeout: 100_000 }, async t => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "ku-owner-"));
  removeAtEnd(home);
  const root = path.join(home, ".kuvo-code");
  const run = path.join(root, "run");
  const bin = path.join(root, "versions", "0.8.0-beta.42-123456789abc", "bin");
  const put = (file, content, mode = 0o600) => {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, content, { mode });
  };
  put(path.join(run, "stopped"), "");
  put(path.join(root, "box", "kind"), "machine\n");
  put(path.join(bin, "box-version"), "0.8.0-beta.42\n");
  put(path.join(bin, "kuvo-start"), script.replace("@ENGINE@", "1.0.0"), 0o700);
  put(path.join(bin, "kuvo-link.js"), fs.readFileSync(built));
  put(path.join(bin, "kuvo-setsid"), '#!/bin/sh\nexec "$@"\n', 0o700);
  fs.symlinkSync(path.relative(root, bin), path.join(root, "bin"));
  put(path.join(root, "engine", "1.0.0", "opencode"), `#!/bin/sh
while [ "$1" = --config=/dev/null ] || [ "$1" = --no-env-file ]; do shift; done
[ "$1" != --version ] || { echo 1.0.0; exit 0; }
exec ${quote(process.execPath)} "$@"
`, 0o700);

  const env = { HOME: home, PATH: process.env.PATH, TMPDIR: home, KUVO_TEST_KEEP_ROUND: "1" };
  const keeperPath = path.join(root, "bin", "kuvo-start");
  const keeper = spawn("sh", [keeperPath, "keep"], { env, cwd: root, stdio: "ignore" });
  const ended = once(keeper, "exit");
  const off = async () => {
    const child = spawn("sh", [keeperPath, "off"], { env, cwd: root, stdio: "ignore" });
    return once(child, "exit");
  };
  atEnd(off);
  const record = path.join(run, "serve.json");
  const until = async (read, description) => {
    const deadline = Date.now() + 40_000;
    while (Date.now() < deadline) {
      const value = read();
      if (value) return value;
      await sleep(50, undefined, { signal: t.signal });
    }
    assert.fail(description + "\n" + (fs.existsSync(path.join(run, "serve.log")) ? fs.readFileSync(path.join(run, "serve.log"), "utf8") : ""));
  };
  const ready = await until(() => fs.existsSync(record) && JSON.parse(fs.readFileSync(record, "utf8")), "receiver never published");
  assert.ok(ready.generation);
  assert.equal(ready.pid, Number(fs.readFileSync(path.join(run, "link.pid"), "utf8")));
  assert.equal(fs.existsSync(path.join(run, "serve.needed")), false);
  assert.equal(fs.existsSync(path.join(run, "link.needed")), false);
  const command = async (...args) => {
    const child = spawn(process.execPath, [path.join(root, "bin", "kuvo-link.js"), "box", ...args],
      { env, cwd: root, stdio: ["ignore", "pipe", "inherit"] });
    const ended = once(child, "exit");
    const chunks = [];
    for await (const chunk of child.stdout) chunks.push(chunk);
    const out = Buffer.concat(chunks).toString("utf8");
    return { status: (await ended)[0], value: out ? JSON.parse(out) : null };
  };
  const probed = await command("resident-info");
  assert.equal(probed.status, 0);
  assert.equal(probed.value.serverGeneration, ready.generation);
  const ssh = await command("route", "--internal", "kuvo/attention");
  assert.equal(ssh.value.status, 200, "the resident pipe answers even with the engine stopped");

  put(record, JSON.stringify({ ...ready, generation: "stale" }));
  assert.equal((await command("resident-info")).status, 1);
  put(record, JSON.stringify(ready));

  const stopped = await off();
  assert.equal(stopped[0], 0);
  await ended;
  assert.equal(fs.existsSync(record), false);
  assert.equal(fs.existsSync(path.join(run, ".client-owner")), false);
  assert.equal((await command("resident-info")).status, 1);
});
