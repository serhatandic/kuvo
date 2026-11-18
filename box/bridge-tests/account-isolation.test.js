import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import path from "node:path";
import { createInterface } from "node:readline";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { script } from "./box-stand-ins.js";
import { removeAtEnd } from "./at-end.js";

const keeper = fileURLToPath(new URL("../src/keeper/kuvo-npx", import.meta.url));

test("two account homes have independent box listeners, keys, SSH socket paths and keeper caches", { timeout: 20_000 }, async () => {
  const root = fs.realpathSync(fs.mkdtempSync("/tmp/k-acct-"));
  removeAtEnd(root);
  const work = path.join(root, "outside-homes");
  fs.mkdirSync(work);
  const bin = path.join(root, "bin");
  fs.mkdirSync(bin);
  // Keeps this test offline. kuvo-npx should run from a cache directory inside each HOME.
  fs.writeFileSync(path.join(bin, "npx"), '#!/bin/sh\npwd -P\n', { mode: 0o700 });
  const envFor = home => ({
    HOME: home, PATH: `${bin}:/usr/bin:/bin`, TMPDIR: work, BUN_BE_BUN: process.env.BUN_BE_BUN,
  });
  const homes = ["a", "b"].map(name => path.join(root, name));
  const children = [];
  try {
    for (const home of homes) {
      const run = path.join(home, ".kuvo-code", "run");
      fs.mkdirSync(run, { recursive: true });
      fs.writeFileSync(path.join(run, "serve.needed"), "");
      // Verify the socket layout, including the short path used for a deeply nested macOS box.
      const deep = path.join(home, "Library", "Application Support", "run.kuvo.code.demo", "box");
      const layout = spawnSync(process.execPath, ["--input-type=module", "-e", `import { boxPlaces } from ${JSON.stringify(pathToFileURL(script).href)}; console.log(boxPlaces().sshAgents)`], {
        env: { ...envFor(home), KUVO_HOME: deep }, encoding: "utf8",
      });
      assert.equal(layout.status, 0, layout.stderr);
      assert.ok(layout.stdout.trim().startsWith(home + "/"), layout.stdout);
      const apart = spawnSync("sh", [keeper, "example"], {
        env: { ...envFor(home), KUVO_NPX_APART: "1" }, cwd: work, encoding: "utf8",
      });
      assert.equal(apart.status, 0, apart.stderr);
      assert.equal(apart.stdout.trim(), path.join(home, ".kuvo-code", "cache", "kuvo-npx"));
      const child = spawn(process.execPath, [script, "serve", "--kept"], {
        env: envFor(home), stdio: ["ignore", "pipe", "inherit"],
      });
      children.push(child);
      const lines = createInterface({ input: child.stdout });
      const [line] = await once(lines, "line", { signal: AbortSignal.timeout(5000) });
      lines.close();
      const record = JSON.parse(fs.readFileSync(path.join(run, "serve.json"), "utf8"));
      assert.equal(JSON.parse(line).url, `http://127.0.0.1:${record.port}`);
      assert.equal(record.pid, child.pid);
    }
    const [a, b] = homes.map(home => {
      const run = path.join(home, ".kuvo-code", "run");
      return { ...JSON.parse(fs.readFileSync(path.join(run, "serve.json"), "utf8")),
        key: fs.readFileSync(path.join(run, "socket.key"), "utf8") };
    });
    assert.notEqual(a.port, b.port);
    assert.notEqual(a.key, b.key);
    for (const [own, other] of [[a, b], [b, a]]) {
      const ask = password => fetch(`http://127.0.0.1:${own.port}/kuvo/info`, {
        headers: { authorization: `Basic ${Buffer.from(`opencode:${password}`).toString("base64")}` },
      });
      assert.equal((await ask(own.password)).status, 200);
      assert.equal((await ask(other.password)).status, 401);
    }
    assert.deepEqual(fs.readdirSync(work), []);
  } finally {
    await Promise.all(children.map(async child => {
      if (child.exitCode !== null || child.signalCode !== null) return;
      const ended = once(child, "exit");
      child.kill("SIGTERM");
      await ended;
    }));
  }
});

test("SSH setup rejects a second setup on the same account and preserves its saved data", () => {
  const home = fs.realpathSync(fs.mkdtempSync("/tmp/k-own-"));
  removeAtEnd(home);
  const run = path.join(home, ".kuvo-code", "run");
  fs.mkdirSync(run, { recursive: true });
  const link = path.join(run, "link.json");
  const kept = JSON.stringify({
    mac: "a".repeat(43), macs: ["a".repeat(43)],
    phones: [{ key: "b".repeat(43), pairing: "existing-device", by: "c".repeat(43) }],
    vouched: { legacy: true },
  });
  fs.writeFileSync(link, kept);
  const result = spawnSync(process.execPath, [script, "setup"], {
    env: { HOME: home, PATH: "/usr/bin:/bin", BUN_BE_BUN: process.env.BUN_BE_BUN },
    input: JSON.stringify({ mac: "d".repeat(43), relay: "wss://unused.invalid" }), encoding: "utf8",
  });
  assert.equal(result.status, 4);
  assert.match(result.stderr, /Connect to your own OS account/);
  assert.equal(fs.readFileSync(link, "utf8"), kept);
});
