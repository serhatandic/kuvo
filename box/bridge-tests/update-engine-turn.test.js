// The real updater, keeper and bridge with a stand-in OpenCode. Turns in the engine
// belong to its original process, and the bridge follows its own turn to completion.
import "./test-safety.js";
import assert from "node:assert/strict";
import { execFile, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { NO_BUN, bunRunsAs } from "./bun-for-tests.js";
import { bun, builtLink, liveUpdateFixture, readJSON } from "./live-update-fixture.js";

async function runningBox(name) {
  const fixture = liveUpdateFixture();
  const { base, scratch, RUN, env, box, keep, pack, bridgeUp, serveUp, until } = fixture;
  Object.assign(process.env, { HOME: env.HOME, KUVO_HOME: env.KUVO_HOME, TMPDIR: env.TMPDIR });
  const link = await import(pathToFileURL(builtLink).href + "#" + name);
  const { project } = box();
  keep();
  await serveUp();
  const bridge = await bridgeUp();
  const engine = await until(() => readJSON(path.join(RUN, "engine.json")), "the engine to run", 15_000);
  await until(() => fs.existsSync(path.join(scratch, `feed-${engine.port}`)), "the bridge to follow the engine", 15_000);
  const native = (method, route, body) => fetch(`http://127.0.0.1:${engine.port}/api/session/${route}`, {
    method, ...(body ? { body: JSON.stringify(body) } : {}),
  }).then(response => response.json());
  async function prepare() {
    const { out, pinned } = pack();
    const manifest = link.verifyBoxes(fs.readFileSync(path.join(out, "boxes.json"), "utf8"), [pinned]);
    const expected = { version: manifest.version, sha256: manifest.pack.sha256 };
    assert.equal(link.receiveRelease(out, [pinned]).status, 202);
    const phase = mode => new Promise((resolve, reject) => {
      const args = ["--config=/dev/null", "--no-env-file", builtLink, "update", "--switch",
        "--phase", mode, "--version", expected.version, "--sha256", expected.sha256, "--key", pinned];
      if (mode === "prepare") args.push("--from", out);
      execFile(bunRunsAs(bun), args, { env: { HOME: base, PATH: process.env.PATH, BUN_BE_BUN: "1" },
        timeout: 120_000, encoding: "utf8" }, (error, stdout, stderr) => {
        if (error) return reject(new Error(stderr || error.message, { cause: error }));
        try { resolve(JSON.parse(stdout.trim().split("\n").at(-1))); } catch (error) { reject(error); }
      });
    });
    const prepared = await phase("prepare");
    assert.equal(prepared.result, "waiting", prepared.message);
    link.fleetWith({ keys: [pinned] });
    assert.equal(link.activationAllowed(expected), true);
    const activate = async () => {
      const switched = await phase("activate");
      assert.equal(switched.result, "updated", switched.message);
      assert.equal(link.boxVersion(), expected.version);
    };
    return { expected, pinned, activate };
  }
  return { ...fixture, bridge, engine, link, native, prepare, project };
}

test("an OpenCode conversation finishes on the same engine after a release switch", { skip: bun ? false : NO_BUN }, async () => {
  const { bridge, bridgeUp, engine, native, prepare, RUN } = await runningBox("native");
  await native("POST", "ses_engine/prompt", { id: "msg_updateengine0000", text: "original turn" });
  assert.equal((await native("GET", "active")).data.ses_engine.type, "busy");

  await (await prepare()).activate();
  await bridgeUp(bridge.pid);
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid, "the original process owns this turn");
  assert.deepEqual(await native("GET", "active"), { data: { ses_engine: { type: "busy" } } });
  assert.deepEqual(await native("POST", "ses_engine/fixture-finish"), { data: { outcome: "succeeded", count: 1 } });
});

test("a release that needs to restart OpenCode waits for its background shell", { skip: bun ? false : NO_BUN }, async () => {
  const { engine, native, env, scratch, RUN } = await runningBox("shell");
  const marker = path.join(scratch, "engine-shell");
  fs.writeFileSync(marker, "");
  assert.deepEqual(await native("GET", "active"), { data: {} });
  const probe = () => spawnSync(bunRunsAs(bun), ["--config=/dev/null", "--no-env-file", builtLink, "release-switch-quiet", "restart"], {
    env: { HOME: env.HOME, KUVO_HOME: env.KUVO_HOME, TMPDIR: env.TMPDIR, PATH: env.PATH, BUN_BE_BUN: "1" },
    encoding: "utf8",
  });
  const busy = probe();
  assert.equal(busy.status, 1, busy.stderr);
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  fs.unlinkSync(marker);
  const quiet = probe();
  assert.equal(quiet.status, 0, quiet.stderr);
});

test("a bridge session on an engine model finishes once before the replacement bridge takes over", { skip: bun ? false : NO_BUN }, async () => {
  const { bridge, bridgeAPI, bridgeUp, engine, link, native, prepare, project, RUN, until } = await runningBox("bridge");
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Engine model", location: { directory: project }, model: { providerID: "fixture", id: "test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  const sent = await api("POST", `api/session/${session}/prompt`, { id: "msg_updateengine0001", delivery: "steer", text: "finish my turn" });
  assert.equal(sent.status, 200, JSON.stringify(sent.json));
  await until(async () => (await native("GET", "active")).data.ses_hidden, "the hidden engine turn to start", 10_000);

  const { expected, pinned, activate } = await prepare();
  let elapsed = 0;
  const deferred = await link.update({ phase: "activate", expected, keys: [pinned], blocked: null,
    drainClock: () => (elapsed += 5 * 60_000) });
  assert.equal(deferred.result, "waiting", "the release defers when the engine turn outlasts the wait");
  assert.equal(readJSON(path.join(RUN, "bridge.json")).pid, bridge.pid);
  assert.equal(link.boxVersion(), "0.9.0");
  const switching = activate();
  await until(() => fs.existsSync(path.join(RUN, "update-engine-hold.json")), "the release to hold new engine messages", 10_000);
  const queued = await api("POST", `api/session/${session}/prompt`, { id: "msg_updateengine0002", delivery: "queue", text: "go after the update" });
  assert.equal(queued.status, 200, JSON.stringify(queued.json));
  assert.deepEqual(await native("POST", "ses_hidden/fixture-finish"), { data: { outcome: "succeeded", count: 1 } });

  await switching;
  await bridgeUp(bridge.pid);
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  await until(async () => (await native("GET", "active")).data.ses_hidden, "the new bridge to send the queued turn");
  assert.deepEqual(await native("POST", "ses_hidden/fixture-finish"), { data: { outcome: "succeeded", count: 2 } });
  const replacement = bridgeAPI().api;
  const messages = await until(async () => {
    const reply = await replacement("GET", `api/session/${session}/message?limit=100&order=asc`);
    return reply.json?.data?.filter(message => message.type === "idle").length === 2 ? reply.json.data : null;
  }, "both turns to finish on the replacement bridge", 10_000);
  assert.deepEqual(messages.filter(message => message.type === "idle").map(message => message.outcome), ["succeeded", "succeeded"]);
  assert.deepEqual(messages.filter(message => message.type === "assistant").flatMap(message => message.content)
    .filter(block => block.type === "text").map(block => block.text), ["Done: finish my turn", "Done: go after the update"]);
});
