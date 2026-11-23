import "./test-safety.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { test } from "node:test";
import {
  base, engineBinary, engineVersion, helperBundle, home, homes, read, release, tools, write,
} from "./computers-add-fixture.js";

function machineFixture(name, { legacy = true, workerReadyMs = 30_000 } = {}) {
  const folder = path.join(base, name);
  homes[name] = folder;
  const root = path.join(folder, ".kuvo-code");
  const run = path.join(root, "run");
  const plist = path.join(folder, "Library/LaunchAgents/run.kuvo.code.machine.plist");
  const engine = () => read(path.join(run, "engine.json"));
  const bridge = () => read(path.join(run, "bridge.json"));
  const helperWorkers = () => {
    const [, helper] = fs.readFileSync(path.join(run, "machine-keeper"), "utf8").trim().split(/\s+/).map(Number);
    const owned = (kind, pid) => fs.readFileSync(path.join(run, kind + ".launch"), "utf8").trim() === pid + " " + helper;
    return helper > 1 && bridge().helper === helper && owned("agents", bridge().pid) && owned("engine", engine().pid);
  };
  const journal = path.join(run, "machine-helper-job.json");
  const swap = path.join(run, "engine-swap.json");
  const adopted = path.join(run, "machine-helper-adopted");
  const until = async (check, message) => {
    for (let i = 0; i < 600; i++) {
      try { if (check()) return; } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    const facts = ["machine-handover.log", "engine-swap.log", "machine-helper-job.json", "fake-launchd", "fake-launchd.permissions", "box.pid", "machine-keeper", "engine.json", "bridge.json", "agents.log", "engine.log"];
    const diagnostics = facts.map(name => {
      const file = path.join(run, name);
      return fs.existsSync(file) ? `${name}: ${fs.readFileSync(file, "utf8").slice(-500)}` : `${name}: missing`;
    });
    assert.fail([message, ...diagnostics].join("\n"));
  };
  fs.cpSync(release, root, { recursive: true });
  write(path.join(root, "engine", engineVersion, "opencode"), engineBinary, 0o755);
  write(path.join(root, "box/kind"), "machine\n");
  write(path.join(root, "box/keys/KUVO_KEY_NOTION_TOKEN"), "retained");
  write(path.join(run, "launch.env"), `PATH=${process.env.PATH}\n`);
  if (legacy) {
    write(plist, '<string>exec sh "$HOME/.kuvo-code/bin/kuvo-start"</string>');
    write(path.join(run, "fake-launchd"), "legacy\n");
  }
  // Give real keeper/launcher processes a test deadline even under load. A deliberately
  // unhealthy fixture uses a shorter deadline to exercise rollback without waiting needlessly.
  const env = { ...process.env, HOME: folder, KUVO_HOME: root, KUVO_TEST_KEEP_ROUND: "1", KUVO_TEST_SWAP_EVERY: "100", KUVO_TEST_SWAP_QUIET: "700", KUVO_TEST_SWAP_START_MOST: String(workerReadyMs) };
  const start = path.join(root, "bin", "kuvo-start");
  const installHelper = () => fs.cpSync(path.join(home, helperBundle), path.join(root, helperBundle), { recursive: true });
  const nextEngine = () => {
    const next = "999.0.0";
    write(path.join(root, "engine", next, "opencode"), engineBinary, 0o755);
    write(start, fs.readFileSync(start, "utf8").replace(/^V=.*$/m, `V=${next}`), 0o700);
    write(path.join(root, "bin/.kuvo-boot"), "next");
    return next;
  };
  const stop = () => {
    spawnSync(path.join(tools, "launchctl"), ["bootout", "stub/run.kuvo.code.machine.permissions"], { env });
    spawnSync("sh", [start, "off"], { env, timeout: 40_000 });
  };
  return { folder, root, run, plist, engine, bridge, helperWorkers, journal, swap, adopted, until, env, start, installHelper, nextEngine, stop };
}

for (const reinstall of [false, true]) test(`a fresh Darwin installer rolls back when the bridge never answers${reinstall ? " during reinstall" : ""}`, {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, plist, bridge, helperWorkers, journal, adopted, until, env, installHelper, stop } = machineFixture(reinstall ? "installer-bridge-reinstall" : "installer-bridge", { legacy: false, workerReadyMs: 9_000 });
  installHelper();
  if (reinstall) write(adopted, "");
  write(path.join(folder, "fail-helper-bridge"), "");
  const installer = spawn(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], { env, stdio: "ignore" });
  const exited = new Promise(resolve => installer.once("exit", resolve));
  try {
    await until(() => helperWorkers(), "the installer should have started both workers under Helper");
    assert.equal((await fetch(`http://127.0.0.1:${bridge().port}/api/session/active`)).status, 503);
    assert.ok(fs.existsSync(journal), "the startup trial is still recoverable");
    assert.equal(fs.existsSync(adopted), false);
    assert.equal(await exited, 1);
    assert.equal(fs.existsSync(adopted), false);
    assert.equal(fs.existsSync(plist), false, "the failed launch job was removed");
  } finally {
    if (installer.exitCode === null && installer.signalCode === null) installer.kill();
    stop();
  }
});

test("a stopped Darwin machine starts through its existing launch job", {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, run, plist, bridge, adopted, env, installHelper, stop } = machineFixture("installer-stopped-legacy");
  installHelper();
  write(path.join(run, "stopped"), "");
  const installer = spawn(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], { env, stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  installer.stderr.on("data", chunk => { stderr += chunk; });
  const exited = new Promise(resolve => installer.once("close", resolve));
  try {
    assert.equal(await exited, 0, stderr);
    assert.match(fs.readFileSync(path.join(folder, "launchctl.log"), "utf8"), /^kickstart gui\/\d+\/run\.kuvo\.code\.machine$/m);
    assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
    assert.equal(bridge().helper, 0);
    assert.equal(fs.existsSync(adopted), false);
  } finally {
    if (installer.exitCode === null && installer.signalCode === null) installer.kill();
    stop();
  }
});

test("a stopped Darwin machine reports a refused legacy launch job", {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, run, plist, env, installHelper, stop } = machineFixture("installer-refused-legacy");
  installHelper();
  write(path.join(run, "stopped"), "");
  write(path.join(folder, "refuse-legacy"), "");
  const installer = spawn(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], { env, stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  installer.stderr.setEncoding("utf8").on("data", chunk => { stderr += chunk; });
  const closed = new Promise(resolve => installer.once("close", resolve));
  try {
    assert.equal(await closed, 1, "a detached keeper cannot turn the refusal into success");
    assert.match(stderr, /existing launch job couldn’t start.*legacy job refused/);
    assert.match(fs.readFileSync(path.join(folder, "launchctl.log"), "utf8"), /^kickstart gui\/\d+\/run\.kuvo\.code\.machine$/m);
    assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
    assert.equal(fs.existsSync(path.join(run, "box.pid")), false);
  } finally {
    if (installer.exitCode === null && installer.signalCode === null) installer.kill();
    stop();
  }
});

test("an adopted Darwin reinstall reports failure while the bridge returns 503", {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, plist, engine, bridge, helperWorkers, journal, adopted, env, installHelper, stop } = machineFixture("installer-already-adopted", { legacy: false });
  installHelper();
  const installer = (readyMs = env.KUVO_TEST_SWAP_START_MOST) => spawnSync(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], {
    env: { ...env, KUVO_TEST_SWAP_START_MOST: readyMs }, encoding: "utf8", timeout: 40_000,
  });
  try {
    const first = installer();
    assert.equal(first.status, 0, first.stderr);
    const registered = fs.readFileSync(plist, "utf8");
    assert.match(registered, /Kuvo Helper<\/string><string>keep-machine<\/string>/);
    assert.ok(fs.existsSync(adopted));
    assert.ok(helperWorkers(), "the clean install adopted both Helper-owned workers before the reinstall");
    const workers = [engine().pid, bridge().pid];

    write(path.join(folder, "fail-helper-bridge"), "");
    assert.equal((await fetch(`http://127.0.0.1:${bridge().port}/api/session/active`)).status, 503);
    const again = installer("9000");
    assert.equal(again.status, 1, again.stderr);
    assert.match(again.stderr, /didn’t finish starting/);
    assert.equal(fs.readFileSync(plist, "utf8"), registered);
    assert.ok(fs.existsSync(adopted), "the completed adoption stays in place");
    assert.equal(fs.existsSync(journal), false);
    assert.deepEqual([engine().pid, bridge().pid], workers);
  } finally {
    stop();
  }
});

test("a Darwin keeper finishes an interrupted fresh install when both services answer", {
  skip: process.platform !== "darwin",
}, async () => {
  const { folder, root, helperWorkers, journal, adopted, until, env, installHelper, stop } = machineFixture("installer-interrupted", { legacy: false });
  installHelper();
  write(path.join(folder, "fail-helper-bridge"), "");
  const installer = spawn(process.execPath, [path.join(root, "bin/kuvo-link.js"), "helper", "machine-start"], { env, stdio: "ignore" });
  const exited = new Promise(resolve => installer.once("exit", resolve));
  try {
    await until(() => read(journal).phase === "waiting" && helperWorkers(), "both Helper-owned workers start during the trial");
    assert.ok(installer.kill("SIGKILL"));
    await exited;
    fs.rmSync(path.join(folder, "fail-helper-bridge"));
    await until(() => fs.existsSync(adopted) && !fs.existsSync(journal), "the keeper verifies service and completes adoption");
  } finally {
    if (installer.exitCode === null && installer.signalCode === null) installer.kill();
    stop();
  }
});

for (const scenario of ["grant", "worker failure", "interruption", "bootstrap denied", "rollback holds", "deferred update", "deferred worker restart", "partial inbox"]) test(`legacy Darwin machine hand-over: ${scenario}`, {
  skip: process.platform !== "darwin",
}, async () => {
  const name = "legacy-" + scenario.replaceAll(" ", "-");
  const rollback = scenario === "rollback holds";
  const workerFailure = scenario === "worker failure";
  const { folder, run, plist, engine, bridge, journal, swap, adopted, until, env, start, installHelper, nextEngine, stop } = machineFixture(name, {
    workerReadyMs: rollback || workerFailure ? 9_000 : undefined,
  });
  // The previous job already has work when Helper is installed.
  write(path.join(run, "engine.now"), "");
  const legacy = spawn("sh", [start, "keep"], { env, stdio: "ignore" });
  try {
    await until(() => bridge().pid && engine().pid, "legacy workers should start");
    const oldEngine = engine().pid, oldBridge = bridge().pid;
    const deferred = scenario.startsWith("deferred");
    if (deferred) write(path.join(folder, "files-access"), "false");
    else if (rollback) {
      write(path.join(folder, "files-access"), "true");
      write(path.join(folder, "refuse-helper"), "");
      write(path.join(folder, "fail-bridge-on-bootstrap"), "");
    } else write(path.join(folder, "busy"), "");
    installHelper();
    if (deferred) {
      await until(() => read(path.join(run, "permissions.json")).fullDiskAccess === false, "Helper should report a denial");
    } else if (!rollback) {
      await until(() => read(path.join(run, "permissions.json")).fullDiskAccess === null, "the signed watcher should publish while the old job is running");
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      write(path.join(folder, "files-access"), "false");
      await until(() => read(path.join(run, "permissions.json")).fullDiskAccess === false, "the watcher should report a denial");
    }

    if (scenario === "deferred worker restart") {
      write(path.join(folder, "crash-bridge"), "");
      await until(() => bridge().pid !== oldBridge, "the legacy job should restart the bridge");
      assert.equal(bridge().helper, 0, "the replacement bridge retains the old launch identity");
      assert.equal(engine().pid, oldEngine);
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      return;
    }
    if (scenario === "deferred update") {
      const next = nextEngine();
      await until(() => engine().version === next, "the ordinary swap should proceed while Files stays denied");
      assert.equal(fs.readFileSync(path.join(folder, "engine-helper"), "utf8"), "");
      assert.equal(bridge().pid, oldBridge);
      assert.equal(read(path.join(run, "permissions.json")).fullDiskAccess, false);
      assert.equal(fs.existsSync(adopted), false);
      return;
    }

    if (!rollback) {
      assert.equal(engine().pid, oldEngine);
      assert.equal(bridge().pid, oldBridge);
      for (const worker of [engine(), bridge()]) assert.ok((await fetch(`http://127.0.0.1:${worker.port}/api/session/active`)).ok);
      if (scenario === "bootstrap denied") write(path.join(folder, "refuse-helper"), "");
      if (workerFailure) write(path.join(folder, "fail-helper-bridge"), "");
      if (scenario === "interruption") write(path.join(folder, "pause-helper-bootstrap"), "");
      if (scenario === "partial inbox") write(path.join(folder, "engine-inbox.json"), JSON.stringify(["A", "B", "C"].map(text => ({
        id: `msg_${text}`, delivery: "queue", type: "user", payload: { text },
      }))));
      write(path.join(folder, "files-access"), "true");
      await until(() => read(swap).stage === "waiting", "even with a grant the hand-over should wait for work");
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      fs.rmSync(path.join(folder, "busy"));
    }

    if (scenario === "bootstrap denied" || scenario === "rollback holds") {
      if (scenario === "rollback holds") {
        await until(() => fs.existsSync(path.join(folder, "fail-legacy-bridge")) && read(journal).phase === "restoring", "the restored bridge is unavailable during rollback");
        const sent = path.join(folder, "delivered-prompts");
        const owner = read(swap).pid;
        write(path.join(run, "waiting/msg_held.json"), JSON.stringify({ id: "msg_held", text: "Keep this held", delivery: "queue" }));
        write(path.join(run, "queues.json"), JSON.stringify({ ses_held: { paused: false, held: [], waiting: [{ id: "msg_held", at: Date.now(), text: "Keep this held" }] } }));
        await until(() => spawnSync("ps", ["-p", String(owner)], { stdio: "ignore" }).status !== 0, "the first recovery finishes its readiness wait");
        assert.equal(fs.existsSync(sent), false, "a running engine on its own does not release messages");
        assert.equal(read(journal).phase, "restoring");
        assert.equal(read(swap).stage, "back");
        fs.rmSync(path.join(folder, "fail-legacy-bridge"));
        await until(() => fs.existsSync(sent) && !fs.existsSync(journal) && !fs.existsSync(swap), "verified service releases the held message");
        assert.equal(fs.readFileSync(sent, "utf8"), "Keep this held\n");
      }
      await until(() => fs.existsSync(path.join(run, "machine-helper-retry")) && !fs.existsSync(journal), "backoff after restoring the launch job");
      assert.equal(engine().pid, oldEngine);
      const next = nextEngine();
      await until(() => engine().version === next, "an ordinary engine swap can proceed during backoff");
      return;
    }

    if (scenario === "interruption") {
      await until(() => fs.existsSync(path.join(folder, "entered-helper-bootstrap")), "launch replacement should reach bootstrap");
      const backup = read(journal);
      assert.equal(backup.phase, "installing");
      assert.match(backup.plist, /exec sh/);
      assert.match(spawnSync("ps", ["-ww", "-o", "command=", "-p", String(backup.owner)], { encoding: "utf8" }).stdout, /kuvo-link\.js engine-swap --machine-helper/);
      process.kill(backup.owner, "SIGKILL");
      fs.rmSync(path.join(folder, "pause-helper-bootstrap"));
      await until(() => !fs.existsSync(journal) && !fs.existsSync(path.join(run, "machine-helper")), "keeper recovers the interrupted replacement");
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      assert.equal(engine().pid, oldEngine);
      assert.equal(bridge().pid, oldBridge);
      return;
    }

    if (workerFailure) {
      await until(() => !fs.existsSync(journal) && fs.existsSync(path.join(run, "machine-helper-retry")) && bridge().pid !== oldBridge, "the previous launch path restored working service");
      assert.match(fs.readFileSync(plist, "utf8"), /exec sh/);
      assert.equal(fs.existsSync(adopted), false);
      assert.ok((await fetch(`http://127.0.0.1:${bridge().port}/api/session/active`)).ok);
      return;
    }
    await until(() => engine().pid !== oldEngine && bridge().pid !== oldBridge && fs.existsSync(adopted), "Helper should restart both workers and confirm adoption");
    const root = path.dirname(run);
    assert.equal(fs.readFileSync(path.join(root, "box/kind"), "utf8"), "machine\n");
    assert.equal(fs.readFileSync(path.join(root, "box/keys/KUVO_KEY_NOTION_TOKEN"), "utf8"), "retained");
    const [, helper] = fs.readFileSync(path.join(run, "machine-keeper"), "utf8").trim().split(/\s+/).map(Number);
    assert.equal(Number(fs.readFileSync(path.join(folder, "engine-helper"), "utf8")), helper);
    assert.equal(bridge().helper, helper);
    if (scenario === "partial inbox") {
      const delivered = path.join(folder, "delivered-prompts");
      await until(() => fs.readFileSync(delivered, "utf8").endsWith("C\n") && !fs.existsSync(swap), "the handover should finish delivering the inbox");
      assert.ok(fs.existsSync(path.join(folder, "inbox-delete-refused")), "one DELETE returned 503");
      assert.equal(fs.readFileSync(delivered, "utf8"), "A\nB\nC\n", "the handover sends each prompt only once");
    }
  } finally {
    stop();
    if (legacy.exitCode === null && legacy.signalCode === null) legacy.kill();
  }
});
