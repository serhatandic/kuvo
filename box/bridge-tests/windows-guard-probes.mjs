// Exercise the actual guard import in children using disposable directories. On Unix, a
// stand-in for the OS credential check reports the same exit codes as the Windows helper.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const guard = new URL("./test-safety.js", import.meta.url).href;
const native = process.platform === "win32";
const folders = ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP"];
const optInName = "KUVO_TEST_DISPOSABLE_ACCOUNT";

function loadGuard(t, { platform = "win32", outside, optIn = os.userInfo().username, credentialStatus = 0 } = {}) {
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-guard-"));
  t.after(() => fs.rmSync(fixture, { recursive: true, force: true }));
  const testHome = path.join(fixture, "home");
  const sibling = path.join(fixture, "home-outside");
  fs.mkdirSync(testHome);
  fs.mkdirSync(sibling);
  const env = {
    ...process.env,
    ...Object.fromEntries(folders.map(name => [name, testHome])),
    WINDOWS_TEST_HOME: testHome,
    TMPDIR: testHome,
  };
  delete env.KUVO_TEST_REAL_HOME;
  if (optIn === null) delete env[optInName];
  else env[optInName] = optIn;
  if (outside) env[outside] = sibling;
  if (!native && platform === "win32") {
    env.SystemRoot = path.join(fixture, "windows");
    const shell = path.join(env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
    fs.mkdirSync(path.dirname(shell), { recursive: true });
    fs.writeFileSync(shell, `#!/bin/sh\nexit ${credentialStatus}\n`, { mode: 0o700 });
  }
  const run = spawnSync(process.execPath, ["--input-type=module", "-e", `
    if (process.platform !== ${JSON.stringify(platform)}) {
      Object.defineProperty(process, "platform", { value: ${JSON.stringify(platform)} });
    }
    await import(${JSON.stringify(guard)});
    console.log(process.env.HOME);
  `], { env, encoding: "utf8" });
  assert.ifError(run.error);
  return { run, testHome };
}

for (const name of folders) {
  test(`the Windows guard refuses ${name} outside the test home`, t => {
    const { run } = loadGuard(t, { outside: name });
    assert.equal(run.status, 1, run.stderr);
    assert.match(run.stderr, new RegExp(`test-safety.js: ${name} must be inside WINDOWS_TEST_HOME`));
    assert.equal(run.stdout, "");
  });
}

for (const [scenario, optIn] of [["missing", null], ["mismatched", os.userInfo().username + "-other"]]) {
  test(`a Windows test stops when the disposable-account opt-in is ${scenario}`, t => {
    const { run } = loadGuard(t, { optIn });
    assert.equal(run.status, 1, run.stderr);
    assert.match(run.stderr, /test-safety.js: KUVO_TEST_DISPOSABLE_ACCOUNT must match the current Windows logon/);
    assert.equal(run.stdout, "");
  });
}

function assertOwnHome({ run, testHome }) {
  assert.equal(run.status, 0, run.stderr);
  const home = run.stdout.trim();
  assert.equal(path.dirname(home), path.join(fs.realpathSync(testHome), ".kuvo-code-test"));
  assert.equal(fs.existsSync(home), false);
}

test("the Windows guard allows the opted-in disposable account with an empty credential store", t => {
  assertOwnHome(loadGuard(t));
});

test("the Windows guard refuses a pre-existing credential on the opted-in account", t => {
  if (native) {
    assert.equal(process.env[optInName]?.toLowerCase(), os.userInfo().username.toLowerCase());
    const target = `kuvo-test-${randomUUID()}`;
    const system = path.join(process.env.SystemRoot, "System32");
    const script = fileURLToPath(new URL("./windows-test-credentials.ps1", import.meta.url));
    t.after(() => {
      const removed = spawnSync(path.join(system, "WindowsPowerShell", "v1.0", "powershell.exe"), [
        "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File",
        script, "-Action", "remove-fixture", "-Target", target,
      ], { encoding: "utf8" });
      assert.equal(removed.status, 0, removed.stderr || removed.error?.message);
    });
    const fixture = spawnSync(path.join(system, "cmdkey.exe"), [
      `/generic:${target}`, "/user:kuvo-test", "/pass:disposable-fixture",
    ], { encoding: "utf8" });
    assert.equal(fixture.status, 0, fixture.stderr || fixture.error?.message);
  }
  const { run } = loadGuard(t, { credentialStatus: 42 });
  assert.equal(run.status, 1, run.stderr);
  assert.match(run.stderr, /test-safety.js: Windows Credential Manager already holds credentials/);
  assert.equal(run.stdout, "");
});

test("the Mac and Linux guard still makes and removes its own home", {
  skip: native && "runs on macOS and Linux",
}, t => {
  assertOwnHome(loadGuard(t, { platform: process.platform, outside: "APPDATA" }));
});
