// The link owns usage, artifacts, and Work Queue sign-in. The old bridge URLs forward there,
// and the link can serve these concerns when the agents bridge has no process at all.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { NO_BUN, testBun } from "./bun-for-tests.js";

const bun = testBun();
const linkScript = fileURLToPath(LINK_BUNDLE);
const bridgeScript = fileURLToPath(new URL("../dist/kuvo-agents.js", import.meta.url));
const sleep = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

test("the existing usage routes share one reader across an agents bridge restart", { skip: bun ? false : NO_BUN, timeout: 45_000 }, async () => {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-box-services-"));
  removeAtEnd(scratch);
  const home = path.join(scratch, ".kuvo-code");
  fs.mkdirSync(home);
  const run = path.join(home, "run");
  const bin = path.join(home, "bin");
  fs.mkdirSync(run);
  fs.mkdirSync(bin);
  fs.writeFileSync(path.join(bin, "gh"), "#!/bin/sh\n[ \"$1/$2\" = auth/status ]\n", { mode: 0o700 });

  const codexStarted = path.join(home, "codex-app-server-started");
  fs.writeFileSync(path.join(bin, "codex"), "#!/bin/sh\ncase \"$1\" in --version) echo 'codex-cli 1.0';; *) touch \"$FIXTURE_CODEX_STARTED\";; esac\n", { mode: 0o700 });
  const fixtureLink = path.join(bin, "kuvo-link.js");
  fs.copyFileSync(linkScript, fixtureLink);

  const start = (script, args, options = {}) => {
    const child = spawn(bun, ["--config=/dev/null", "--no-env-file", script, ...args], {
      cwd: home,
      stdio: ["ignore", "ignore", "pipe"],
      env: { ...process.env, HOME: scratch, KUVO_HOME: home, KUVO_TEST_PRICE_FEEDS: "off", FIXTURE_CODEX_STARTED: codexStarted, PATH: `${bin}${path.delimiter}${process.env.PATH}`, BUN_BE_BUN: "1", ...options },
    });
    let output = "";
    child.stderr.on("data", chunk => { output += chunk; });
    const done = once(child, "exit");
    const stop = async () => {
      if (child.exitCode === null && child.signalCode === null) child.kill();
      await done;
    };
    atEnd(stop);
    return { child, stop, output: () => output };
  };

  const ready = async (file, { child, output }) => {
    for (let tries = 0; tries < 200; tries++) {
      if (child.exitCode !== null || child.signalCode !== null) throw new Error(output());
      try {
        const record = JSON.parse(fs.readFileSync(path.join(run, file), "utf8"));
        if (record.pid === child.pid) return record;
      } catch {}
      await sleep(50);
    }
    assert.fail(`No ${file}: ${output()}`);
  };

  const auth = ({ password }) => "Basic " + Buffer.from(`opencode:${password}`).toString("base64");
  const ask = (record, route, request = {}) => fetch(`http://127.0.0.1:${record.port}/${route}`, {
    ...request, headers: { ...request.headers, authorization: auth(record) }, signal: AbortSignal.timeout(10_000),
  });
  const ok = async (record, route, request) => {
    const response = await ask(record, route, request);
    assert.equal(response.status, 200, await response.clone().text());
    return response.json();
  };

  const services = start(fixtureLink, ["box-services"]);
  const original = await ready("box-services.json", services);
  const route = "api/kuvo/limits";
  const before = await ok(original, route);
  assert.deepEqual(before.data.rows, []);
  assert(before.data.services.includes("antigravity"));
  const blocked = await fetch(`http://127.0.0.1:${original.port}/${route}`, { signal: AbortSignal.timeout(5_000) });
  assert.equal(blocked.status, 401);
  const github = await ok(original, "api/integration/github-work-queue");
  assert.equal(github.data.connections[0].status.status, "connected");
  const artifacts = await ok(original, "api/kuvo/link/artifacts?bot=bot_ada");
  assert.deepEqual(artifacts.artifacts, []);
  const insights = await ok(original, "api/kuvo/insights?period=7");
  assert(insights.data);

  const startBridge = async () => {
    const reservation = net.createServer();
    await new Promise(resolve => reservation.listen(0, "127.0.0.1", resolve));
    const port = reservation.address().port;
    await new Promise(resolve => reservation.close(resolve));
    const launched = start(bridgeScript, [], {
      KUVO_AGENTS_REMOTE: "1", KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_AGENTS_PORT: String(port),
      KUVO_AGENTS_PASSWORD: "fixture", KUVO_SANDBOX: "off",
    });
    return { launched, record: await ready("bridge.json", launched) };
  };

  const first = await startBridge();
  await sleep(1200); // the artifact reader has attached to the bridge
  assert.equal(fs.existsSync(codexStarted), false);
  assert.deepEqual(await ok(first.record, route), before);
  await first.launched.stop();
  assert.deepEqual(await ok(original, route), before);
  const second = await startBridge();
  assert.notEqual(first.record.pid, second.record.pid);
  assert.deepEqual(await ok(second.record, route), before);
  assert.equal((await ready("box-services.json", services)).pid, original.pid);
  assert.equal(fs.existsSync(codexStarted), false);

  // A legacy rollback fences out services before placing the legacy bridge. quiesce-services
  // waits for the original writer to go, and another launch cannot start during the rollback.
  const keeper = fileURLToPath(new URL("../src/keeper/kuvo-start", import.meta.url));
  fs.mkdirSync(path.join(run, ".rolling-back"));
  const quiesced = spawnSync("sh", [keeper, "quiesce-services"], { env: { ...process.env, HOME: scratch }, encoding: "utf8" });
  assert.equal(quiesced.status, 0, quiesced.stderr);
  assert.equal(fs.existsSync(path.join(run, "box-services.json")), false);
  const fenced = start(fixtureLink, ["box-services"]);
  assert.equal((await once(fenced.child, "exit"))[0], 0);
  assert.equal(fs.existsSync(path.join(run, "box-services.json")), false);
  fs.rmdirSync(path.join(run, ".rolling-back"));
  await second.launched.stop();

  // A previous bridge without the ownership marker may still recover the handover.
  const previous = path.join(run, "agents", "previous", "kuvo-agents.js");
  const checkpoint = path.join(run, "agents", "handover.json");
  fs.mkdirSync(path.dirname(previous), { recursive: true });
  fs.writeFileSync(previous, "// Legacy fixture\n");
  fs.writeFileSync(checkpoint, "{}");
  const recovering = start(fixtureLink, ["box-services"]);
  assert.equal((await once(recovering.child, "exit"))[0], 0);
  assert.equal(fs.existsSync(path.join(run, "box-services.json")), false);
  fs.unlinkSync(checkpoint);
  fs.unlinkSync(previous);

  // With no live bridge or leftover record, installed ownership can start a new reader.
  const replacement = start(fixtureLink, ["box-services"]);
  const next = await ready("box-services.json", replacement);
  assert.notEqual(next.pid, original.pid);
  assert.deepEqual(await ok(next, route), before);
});
