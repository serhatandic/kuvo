import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";

const runner = new URL("../src/shared/folder-version.ts", import.meta.url).href;
const link = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const agents = fileURLToPath(new URL("../dist/kuvo-agents.js", import.meta.url));
const bun = process.env.KUVO_TEST_BUN || "bun";

function scratch() {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-layout-"));
  removeAtEnd(home);
  return home;
}

function launch(home, program, args, options = {}) {
  return spawnSync(program, args, {
    ...options,
    env: { ...process.env, HOME: home, KUVO_HOME: path.join(home, ".kuvo-code"), ...options.env },
    encoding: "utf8",
    timeout: 15_000,
  });
}

test("folder layout 2 adopts existing data in place and the current layout opens read-only", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  const raw = '{"old":"shape"}\n';
  fs.writeFileSync(path.join(folder, "machines.json"), raw);
  const marker = path.join(folder, "folder-version.json");
  const result = launch(home, process.execPath, [link, "folder-upgrade"]);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 2 });
  assert.equal(fs.readFileSync(path.join(folder, "machines.json"), "utf8"), raw);
  assert.deepEqual(fs.readdirSync(folder).sort(), ["folder-version.json", "machines.json"]);
  const inode = fs.statSync(marker).ino;
  const reopened = launch(home, process.execPath, [link, "folder-upgrade"]);
  assert.equal(reopened.status, 0, reopened.stderr);
  assert.equal(fs.statSync(marker).ino, inode);
});

test("failure bench: a folder upgrade killed after step 1 resumes from step 2", () => {
  const folder = scratch();
  const run = kill => spawnSync(bun, ["--config=/dev/null", "--no-env-file", "-e", `
    import { appendFileSync } from "node:fs";
    import { upgradeFolder } from ${JSON.stringify(runner)};
    upgradeFolder(${JSON.stringify(folder)}, [
      folder => appendFileSync(folder + "/applied", "1"),
      folder => {
        if (${kill}) process.kill(process.pid, "SIGKILL");
        appendFileSync(folder + "/applied", "2");
      },
    ]);
  `], { encoding: "utf8", timeout: 15_000 });
  const marker = path.join(folder, "folder-version.json");
  const read = () => JSON.parse(fs.readFileSync(marker, "utf8"));
  const killed = run(true);
  assert.equal(killed.signal, "SIGKILL", killed.stderr);
  assert.deepEqual(read(), { version: 1, upgradingTo: 2 });
  assert.equal(fs.readFileSync(path.join(folder, "applied"), "utf8"), "1");
  const restarted = run(false);
  assert.equal(restarted.status, 0, restarted.stderr);
  assert.deepEqual(read(), { version: 2 });
  assert.equal(fs.readFileSync(path.join(folder, "applied"), "utf8"), "12");
  assert.deepEqual(fs.readdirSync(folder).sort(), ["applied", "folder-version.json"]);
});

test("failure bench: startup refuses a newer folder without changing it", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  const marker = path.join(folder, "folder-version.json");
  for (const raw of ['{"version":3}\n', '{"version":2,"upgradingTo":3}\n']) {
    fs.writeFileSync(marker, raw);
    const result = launch(home, process.execPath, [link, "serve"]);
    assert.equal(result.status, 1, result.stderr);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /Kuvo folder at .* format version 3 is unsupported \(this build writes 2\)/);
    assert.equal(fs.readFileSync(marker, "utf8"), raw);
    assert.deepEqual(fs.readdirSync(folder), ["folder-version.json"]);
  }
});

test("startup refuses an unreadable marker and preserves its bytes", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  const marker = path.join(folder, "folder-version.json");
  fs.writeFileSync(marker, "{");
  const result = launch(home, process.execPath, [link, "folder-upgrade"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Kuvo folder at .* format version invalid is unsupported/);
  assert.equal(fs.readFileSync(marker, "utf8"), "{");
  assert.deepEqual(fs.readdirSync(folder), ["folder-version.json"]);
});


test("the agents bridge refuses the newer layout before initializing its store", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  fs.writeFileSync(path.join(folder, "folder-version.json"), '{"version":3}');
  const result = launch(home, bun, [agents], {
    env: { KUVO_AGENTS_HOME: path.join(folder, "agents"), OPENCODE_PASSWORD: "test" },
  });
  assert.equal(result.status, 1, result.stderr);
  assert.equal(result.stdout, "");
  assert.match(result.stderr, /Kuvo folder at .* format version 3 is unsupported/);
  assert.deepEqual(fs.readdirSync(folder), ["folder-version.json"]);
});

test("keeper and engine launcher refuse the newer layout before making runtime folders", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  const engine = path.join(folder, "engine", "test");
  fs.mkdirSync(engine, { recursive: true });
  fs.mkdirSync(path.join(folder, "bin"));
  fs.copyFileSync(link, path.join(folder, "bin", "kuvo-link.js"));
  // Only run the preflight. Even if the guard regresses, the stand-in cannot start a real engine.
  fs.writeFileSync(path.join(engine, "opencode"), `#!/bin/sh
[ "$4" = folder-upgrade ] || exit 77
exec ${JSON.stringify(process.execPath)} "$3" "$4"
`, { mode: 0o700 });
  fs.writeFileSync(path.join(folder, "folder-version.json"), '{"version":3}');
  for (const script of ["kuvo-start", "kuvo-launch"]) {
    const source = fs.readFileSync(new URL(`../src/keeper/${script}`, import.meta.url), "utf8");
    const staged = path.join(home, script);
    fs.writeFileSync(staged, source.replaceAll("@ENGINE@", "test"));
    // A failure to guard a startup must still fail fast and leave no detached test process.
    const result = launch(home, "/bin/sh", [staged, "status"], { input: "\n" });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /Kuvo folder at .* format version 3 is unsupported/);
    assert.deepEqual(fs.readdirSync(folder).sort(), ["bin", "engine", "folder-version.json"]);
  }
});
