// An update stops nothing (PROTOCOL.md, "Updates"): a box takes a release while a coding agent's turn
// and an engine's turn both run, and both finish; the new code is live afterwards in its client
// server and coding agents' bridge. The bridge switches while the Codex turn still has an open
// approval. The new bridge shows the same approval, continues that turn once, and accepts a new
// turn while the engine is still busy. Then the update is judged well.
//
// Everything is the real thing: the keeper (`kuvo-start keep`, `kuvo-launch`), the link and the
// bridge as built, the release packed by scripts/box-release.sh and taken by the link. Stand-ins
// only for the engine (an HTTP server whose own turn runs while the test says) and for `codex
// app-server` (its turn holds until let go). In a home of the test's own; the same on macOS and
// Linux. Run with `node --test box/bridge-tests/update-stops-nothing.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { NO_BUN, bunRunsAs } from "./bun-for-tests.js";
import { bun, builtLink, builtAgents, write, readJSON, runs, build, OLD_BUILD, turns, liveUpdateFixture } from "./live-update-fixture.js";

const { base, home, scratch, RUN, env, box, keep, pack, bridgeAPI, bridgeUp, serveUp, linkUp, holderFor, until } = liveUpdateFixture();
Object.assign(process.env, { HOME: env.HOME, KUVO_HOME: env.KUVO_HOME, TMPDIR: env.TMPDIR });
const link = await import(builtLink);
const folderLayout = Number(/^\/\/ Kuvo folder layout (\d+) /m.exec(fs.readFileSync(builtLink, "utf8"))[1]);

test("standby capability is advertised only to a bridge with the same recovery layout", { skip: bun ? false : NO_BUN }, () => {
  const expected = `ok ${build(fs.readFileSync(builtAgents, "utf8"))}`;
  for (const [layout, capability] of [
    [[], ""],
    [["folder-layout=1"], ""],
    [[`folder-layout=${folderLayout - 1}`], ""],
    [[`folder-layout=${folderLayout}`], ` kuvo-live-handover/1 folder-layout=${folderLayout}`],
  ]) {
    const result = spawnSync(bunRunsAs(bun), ["--config=/dev/null", "--no-env-file", builtAgents, "selftest", "adopt-check", ...layout], {
      env: { ...process.env, BUN_BE_BUN: "1" }, encoding: "utf8", timeout: 15_000,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), expected + capability);
  }
});

test("the atomic fleet switch preserves a held agent turn and starts the release’s workers", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const { fake } = box();
  keep();
  const engine = await until(() => { const said = readJSON(path.join(RUN, "engine.json")); return said?.pid && runs(said.pid) ? said : null; }, "the engine to run");
  const firstBridge = await bridgeUp();
  assert.equal(firstBridge.build, OLD_BUILD);
  const firstServe = await serveUp();
  const firstLink = await linkUp();

  // A coding agent's turn under way, and the engine's own.
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Long one", permissions: [{ action: "shell", resource: "*", effect: "ask" }], location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  assert.equal((await api("POST", `api/session/${session}/prompt`, { id: "msg_stopsnothing00001", delivery: "steer", text: "hold one" })).status, 200);
  await until(() => turns(fake).length === 1, "the agent's turn to start");
  const pending = async () => (await bridgeAPI().api("GET", `api/session/${session}/permission`)).json?.data;
  await until(async () => (await pending())?.[0], "the permission to appear");
  const firstHolder = holderFor(session);
  assert.ok(firstHolder, "the active turn has a holder");
  assert.ok(runs(firstHolder.child));
  write(path.join(scratch, "engine-busy"), "");

  const { out, pinned } = pack();
  const manifest = link.verifyBoxes(fs.readFileSync(path.join(out, "boxes.json"), "utf8"), [pinned]);
  assert.equal(link.receiveRelease(out, [pinned]).status, 202);
  function phase(mode) {
    const args = ["--config=/dev/null", "--no-env-file", builtLink, "update", "--switch",
      "--phase", mode, "--version", manifest.version, "--sha256", manifest.pack.sha256, "--key", pinned];
    if (mode === "prepare") args.push("--from", out);
    const ran = spawnSync(bunRunsAs(bun), args, { env: { HOME: base, PATH: process.env.PATH, BUN_BE_BUN: "1" },
      timeout: 120_000, encoding: "utf8" });
    assert.equal(ran.status, 0, ran.stderr);
    return JSON.parse(ran.stdout.trim().split("\n").at(-1));
  }
  const prepared = phase("prepare");
  assert.equal(prepared.result, "waiting", prepared.message);
  assert.equal(link.boxVersion(), "0.9.0");
  const switched = phase("activate");
  assert.equal(switched.result, "updated", switched.message);
  assert.equal(link.boxVersion(), manifest.version);
  // The holder stays up as the published bridge changes. Neither turn was given its completion signal.
  const secondBridge = await bridgeUp(firstBridge.pid);
  assert.equal(secondBridge.build, build(fs.readFileSync(builtAgents, "utf8")), "the new bridge runs the release's build");
  assert.equal(fs.existsSync(path.join(fake, "release-one")), false);
  assert.equal(holderFor(session).child, firstHolder.child);
  assert.ok(holderFor(session).generation > firstHolder.generation);
  assert.ok(runs(firstHolder.child), "the same agent process continued");
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  const again = await pending();
  assert.equal(again.length, 1);
  assert.equal((await bridgeAPI().api("POST", `api/session/${session}/permission/${again[0].id}/reply`, { decision: "once" })).status, 200);
  write(path.join(fake, "release-one"), "");
  const messages = await until(async () => {
    const page = (await bridgeAPI().api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the agent's turn to end");
  assert.deepEqual(messages.filter(each => each.type === "assistant").flatMap(each => each.content ?? []).filter(block => block.type === "text").map(block => block.text), ["Before the update.", "Done: hold one"]);
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"], "never cut");
  assert.deepEqual(turns(fake), [{ text: "hold one", bridge: firstBridge.pid }]);

  assert.equal(fs.readFileSync(path.join(fake, "approvals.log"), "utf8").trim().split("\n").length, 1, "the original agent accepted one answer");
  const second = await bridgeAPI().api("POST", "api/session", { title: "New work", location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(second.status, 200);
  const another = second.json.data.id;
  assert.equal((await bridgeAPI().api("POST", `api/session/${another}/prompt`, { id: "msg_stopsnothing00003", delivery: "steer", text: "new work" })).status, 200);
  await until(() => turns(fake).some(turn => turn.text === "new work"), "the adopted server to accept new work");
  await until(async () => (await bridgeAPI().api("GET", `api/session/${another}/message?limit=100&order=asc`)).json?.data?.at(-1)?.type === "idle", "the new work to finish");

  // The original engine finishes the turn it started.
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  fs.rmSync(path.join(scratch, "engine-busy"));
  assert.equal(await link.agentsWorking(), false, "both finished");

  // Live afterwards: each answers on the new code, and the update is judged well (by the judge the
  // keeper started for it, the updater here being a test's).
  // (A server or link started again within a second of the new link's arrival may be ended once
  // more by the keeper, which can't tell it from one on the code before, and started again.)
  await serveUp(firstServe);
  await linkUp(firstLink);
  assert.ok(runs(secondBridge.pid));
  await until(() => !fs.existsSync(path.join(RUN, "previous")), "the start guard confirms the update");
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8").trim(), "0.10.0", "kept");
});

test("the engine started again while a coding agent's turn runs: the turn completes on the same bridge, which follows the new engine", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  // (The box the test before left running, on 0.10.0.)
  const fake = path.join(scratch, "fake");
  const engine = readJSON(path.join(RUN, "engine.json"));
  const bridge = await bridgeUp();
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Through a restart", location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  const before = turns(fake).length;
  assert.equal((await api("POST", `api/session/${session}/prompt`, { id: "msg_stopsnothing00002", delivery: "steer", text: "hold two" })).status, 200);
  await until(() => turns(fake).length === before + 1, "the agent's turn to start");
  const holder = await until(() => holderFor(session), "the holder to record the new turn");

  // The engine ends (a crash, or a hung one ended): its keeper starts another, on a port and
  // password of its own.
  process.kill(engine.pid, "SIGTERM");
  const restarted = await until(() => { const said = readJSON(path.join(RUN, "engine.json")); return said?.pid && said.pid !== engine.pid && runs(said.pid) ? said : null; }, "the keeper to start the engine again");
  assert.notEqual(restarted.port, engine.port);
  // The bridge never went, and follows the new engine where its record says.
  assert.equal(readJSON(path.join(RUN, "bridge.json")).pid, bridge.pid);
  assert.ok(runs(bridge.pid), "the bridge runs on");
  await until(() => fs.existsSync(path.join(scratch, `feed-${restarted.port}`)), "the bridge to follow the new engine");
  const stillHeld = holderFor(session);
  assert.equal(stillHeld.child, holder.child, "the native app-server never changed");
  assert.equal(stillHeld.generation, holder.generation, "the same controller kept the holder");

  // The turn ends as it would have: on that bridge, once.
  write(path.join(fake, "release-two"), "");
  const messages = await until(async () => {
    const page = (await bridgeAPI().api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the agent's turn to end");
  assert.deepEqual(messages.filter(each => each.type === "assistant").flatMap(each => each.content ?? []).filter(block => block.type === "text").map(block => block.text), ["Done: hold two"]);
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"], "never cut");
  // The fixture logs the process that launched the app-server, which a handover can predate.
  assert.deepEqual(turns(fake).slice(before), [{ text: "hold two", bridge: holder.bridge }]);
  assert.equal(readJSON(path.join(RUN, "bridge.json")).pid, bridge.pid);
});

test("if the new bridge exits at the cutover, the keeper resumes the turn on the previous build", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async t => {
  const first = await bridgeUp();
  const engine = readJSON(path.join(RUN, "engine.json")).pid;
  const fake = path.join(scratch, "fake");
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Failed take-over", permissions: [{ action: "shell", resource: "*", effect: "ask" }], location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  assert.equal((await api("POST", `api/session/${session}/prompt`, { id: "msg_stopsnothing00004", delivery: "steer", text: "hold recovery" })).status, 200);
  const pending = async () => (await bridgeAPI().api("GET", `api/session/${session}/permission`)).json?.data;
  const ask = await until(async () => (await pending())?.[0], "the permission to appear");
  const holder = await until(() => holderFor(session), "the holder to record the turn before recovery");

  const bundle = path.join(home, "bin", "kuvo-agents.js");
  const broken = "b".repeat(64);
  const failsOnCutover = 'if (process.argv.includes("handover-standby")) process.prependOnceListener("disconnect", () => process.exit(6));\n';
  write(bundle + ".new", failsOnCutover + fs.readFileSync(builtAgents, "utf8").replace(first.build, broken));
  fs.renameSync(bundle + ".new", bundle);
  const recovered = await bridgeUp(first.pid);
  assert.equal(recovered.build, first.build, "the previous executable answered after the candidate failed");
  const resumed = holderFor(session);
  assert.equal(resumed.pid, holder.pid, "the original holder is still running");
  assert.equal(resumed.child, holder.child, "the previous build resumed the original native process");
  assert.ok(resumed.generation > holder.generation, "the recovery bridge took ownership of the holder");
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine);
  assert.equal((await bridgeAPI().api("GET", `api/session/${session}/prompt-receipt/msg_stopsnothing00004`)).status, 200);
  t.diagnostic(`bridge ${first.pid} → ${recovered.pid}; engine ${engine}, holder ${holder.pid}, provider ${holder.child}`);
  assert.equal((await until(async () => (await pending())?.[0], "the same pending permission")).id, ask.id);
  assert.equal((await bridgeAPI().api("POST", `api/session/${session}/permission/${ask.id}/reply`, { decision: "once" })).status, 200);
  write(path.join(fake, "release-recovery"), "");
  const messages = await until(async () => {
    const page = (await bridgeAPI().api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the recovered turn to finish");
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"]);
  assert.deepEqual(turns(fake).filter(turn => turn.text === "hold recovery"), [{ text: "hold recovery", bridge: holder.bridge }]);
});


test("the one-time pre-holder update restarts its bridge cleanly and leaves an engine turn running", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  if (!fs.existsSync(path.join(RUN, "launch.env"))) {
    box();
    keep();
  }
  const first = await bridgeUp();
  const bundle = path.join(home, "bin", "kuvo-agents.js");
  const installed = fs.readFileSync(builtAgents, "utf8");
  const legacyBuild = "2b3fded6f49f21e8b2434216207d63dd8996b579aba9f11bc63a4bd84ae49176";
  // Run the real bridge's normal shutdown path, with pre-holder process ownership and no
  // handover capability or marker as on a pre-holder installation.
  const legacy = installed.replace(build(installed), legacyBuild)
    .replaceAll("kuvo-live-handover/1", "legacy-test-bridge")
    .replaceAll("agents-live-handover", "agents-legacy-fixture")
    .replace(/function agentHoldFolder\(kind\) \{\n/, "$&  return undefined;\n");
  assert.notEqual(legacy, installed);
  const crossing = path.join(RUN, "agents-live-handover");
  const replace = contents => {
    write(bundle + ".new", contents);
    fs.renameSync(bundle + ".new", bundle);
  };
  const unknownBuild = "e".repeat(64);
  replace(legacy.replace(legacyBuild, unknownBuild));
  process.kill(first.pid, "SIGTERM");
  const old = await bridgeUp(first.pid);
  assert.equal(old.build, unknownBuild);
  fs.rmSync(crossing);
  const holderRecords = () => {
    try { return fs.readdirSync(path.join(RUN, "agents", "hold")).filter(name => name.endsWith(".json")); }
    catch { return []; }
  };
  await until(() => holderRecords().length === 0, "the old fixture to have no holders");

  const engine = readJSON(path.join(RUN, "engine.json"));
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Pre-holder turn", location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200);
  const session = made.json.data.id;
  const fake = path.join(scratch, "fake");
  assert.equal((await api("POST", `api/session/${session}/prompt`, { id: "msg_stopsnothing00005", delivery: "steer", text: "hold legacy" })).status, 200);
  await until(() => turns(fake).some(turn => turn.text === "hold legacy" && turn.bridge === old.pid), "the unheld turn to start");
  assert.equal(holderRecords().length, 0, "the pre-holder bridge owns this agent directly");

  replace(installed);
  // With no live holders this still has an unidentified generation and a live unheld turn.
  // Wait for a keeper round before checking the original turn and the exclusive marker.
  await new Promise(resolve => setTimeout(resolve, 3500));
  assert.equal(readJSON(path.join(RUN, "bridge.json")).pid, old.pid);
  assert.ok(!fs.existsSync(crossing));

  // Model a bridge whose running build is known to precede holders.
  const oldRecord = path.join(RUN, "bridge.json");
  write(oldRecord, JSON.stringify({ ...readJSON(oldRecord), build: legacyBuild }));
  const after = await bridgeUp(old.pid);
  assert.equal(after.build, build(installed));
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  assert.equal(fs.readFileSync(crossing, "utf8"), "kuvo-live-handover/1");
  const page = (await bridgeAPI().api("GET", `api/session/${session}/message?limit=100&order=asc`)).json.data;
  assert.deepEqual(page.filter(each => each.type === "idle").map(each => each.outcome), ["interrupted"]);
  assert.deepEqual(page.filter(each => each.type === "user").map(each => each.text), ["hold legacy"]);
  assert.equal(turns(fake).filter(turn => turn.text === "hold legacy").length, 1);
});
