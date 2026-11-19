// A machine's coding agents' bridge killed (SIGKILL: a crash) while a coding agent's turn waits on
// your approval: the agent, under its holder, lives on; the bridge its launch starts again takes the
// turn up, shows the approval again, once; you answer it once, and the turn ends as it would have,
// one turn, with what the agent said before and after the crash. Then the box is stopped
// (`kuvo-start stop`), and nothing it held runs on. The real bridge (box/dist/kuvo-agents.js, on
// Bun), its real holders and keeper, and stand-ins for Codex's app-server and Claude Code (they
// speak their protocols; the real ones were checked the same way in spikes S1 and S2,
// .claude/notes/update-handover.md).
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { NO_BUN, testBun } from "./bun-for-tests.js";

const bun = testBun();
const built = fileURLToPath(new URL("../dist/kuvo-agents.js", import.meta.url));
const keeper = fileURLToPath(new URL("../src/keeper/kuvo-start", import.meta.url));

/** Codex as far as this asks it: a turn asks to run a command, and once that's answered, says it's done. */
const CODEX = String.raw`
const fs = require("node:fs");
const path = require("node:path");
const folder = process.argv[2];
const log = what => fs.appendFileSync(path.join(folder, "log"), JSON.stringify({ ...what, pid: process.pid }) + "\n");
const send = message => process.stdout.write(JSON.stringify(message) + "\n");
let buffer = "", threads = 0, turns = 0, asks = 100;
const open = new Map();
process.stdin.on("end", () => { log({ ended: true }); process.exit(0); });
process.stdin.on("data", chunk => {
  buffer += chunk;
  for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
    const line = buffer.slice(0, at);
    buffer = buffer.slice(at + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    if (message.method === undefined && open.has(message.id)) {
      const { threadId, turnId } = open.get(message.id);
      open.delete(message.id);
      log({ answered: message.result?.decision });
      send({ method: "item/started", params: { threadId, turnId, item: { type: "agentMessage", id: "m" + turnId, text: "" } } });
      send({ method: "item/agentMessage/delta", params: { threadId, turnId, itemId: "m" + turnId, delta: "Ran it." } });
      send({ method: "item/completed", params: { threadId, turnId, item: { type: "agentMessage", id: "m" + turnId, text: "Ran it." } } });
      send({ method: "turn/completed", params: { threadId, turn: { id: turnId, status: "completed" } } });
    } else if (message.method === "thread/start") send({ id: message.id, result: { thread: { id: "t" + ++threads } } });
    else if (message.method === "thread/resume") send({ id: message.id, result: { thread: { id: message.params.threadId } } });
    else if (message.method === "turn/start") {
      const threadId = message.params.threadId, turnId = "u" + ++turns, id = asks++;
      send({ id: message.id, result: { turn: { id: turnId, status: "inProgress" } } });
      send({ method: "turn/started", params: { threadId, turn: { id: turnId } } });
      send({ method: "item/started", params: { threadId, turnId, item: { type: "agentMessage", id: "a" + turnId, text: "" } } });
      send({ method: "item/completed", params: { threadId, turnId, item: { type: "agentMessage", id: "a" + turnId, text: "Asking first." } } });
      open.set(id, { threadId, turnId });
      send({ id, method: "item/commandExecution/requestApproval", params: { threadId, turnId, itemId: "c" + turnId, command: "echo held" } });
      log({ asked: id, parent: process.ppid });
    } else if (message.method === "account/read") send({ id: message.id, result: { account: { type: "apiKey" } } });
    else if (message.method === "model/list") send({ id: message.id, result: { data: [{ id: "gpt-test", model: "gpt-test", displayName: "GPT Test", isDefault: true }] } });
    else if (message.id !== undefined) send({ id: message.id, result: {} });
  }
});
`;

/** Claude Code as far as this asks it: signed in; a message asks to run a command, and once that's
 * answered, it's done. A second SDK client's `initialize` is answered with the ask still open, as
 * Claude Code 2.1.325 answered it in spike S1. */
const CLAUDE = String.raw`
const fs = require("node:fs");
const path = require("node:path");
const folder = process.env.STAND_IN_FOLDER;
const args = process.argv.slice(2);
if (args[0] === "auth") { console.log('{"loggedIn":true,"authMethod":"console"}'); process.exit(0); }
if (args.includes("--version")) { console.log("2.1.325 (Claude Code)"); process.exit(0); }
if (!args.includes("stream-json")) process.exit(1);
const named = flag => (args.indexOf(flag) >= 0 ? args[args.indexOf(flag) + 1] : undefined);
const session = named("--session-id") ?? named("--resume") ?? "s1";
const log = what => fs.appendFileSync(path.join(folder, "claude.log"), JSON.stringify({ ...what, pid: process.pid }) + "\n");
const send = message => process.stdout.write(JSON.stringify(message) + "\n");
const models = [{ value: "default", resolvedModel: "claude-opus-5-5", displayName: "Default (recommended)", description: "Opus 5.5" }];
const ask = { type: "control_request", request_id: "p1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "touch held.txt" }, tool_use_id: "toolu_1" } };
const assistant = (id, content) => send({ type: "assistant", message: { id, role: "assistant", type: "message", model: "claude-opus-5-5", content, stop_reason: null, usage: { input_tokens: 1, output_tokens: 1 } }, parent_tool_use_id: null, session_id: session, uuid: "00000000-0000-4000-8000-00000000000" + id.slice(-1) });
let open = false, buffer = "";
process.stdin.on("end", () => { log({ ended: true }); process.exit(0); });
process.stdin.on("data", chunk => {
  buffer += chunk;
  for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
    const line = buffer.slice(0, at);
    buffer = buffer.slice(at + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    if (message.type === "control_request") {
      if (message.request.subtype === "initialize") log({ initialized: true });
      const response = message.request.subtype === "initialize" ? { commands: [], models, agents: [], output_style: "default", available_output_styles: ["default"], account: {} } : {};
      send({ type: "control_response", response: { subtype: "success", request_id: message.request_id, response, ...(message.request.subtype === "initialize" ? { pending_permission_requests: open ? [ask] : [] } : {}) } });
    } else if (message.type === "user") {
      send({ type: "system", subtype: "init", session_id: session, claude_code_version: process.env.STAND_IN_VERSION, cwd: process.cwd(), tools: ["Bash"], model: "claude-opus-5-5", permissionMode: "default", uuid: "00000000-0000-4000-8000-000000000000" });
      assistant("m1", [{ type: "text", text: "Asking first." }, { type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "touch held.txt" } }]);
      open = true;
      send(ask);
      log({ asked: "p1", parent: process.ppid });
    } else if (message.type === "control_response" && message.response?.request_id === "p1" && open) {
      open = false;
      log({ answered: message.response.response?.behavior });
      send({ type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_1", content: "" }] }, parent_tool_use_id: null, session_id: session, uuid: "00000000-0000-4000-8000-000000000003" });
      assistant("m4", [{ type: "text", text: "Ran it." }]);
      send({ type: "result", subtype: "success", is_error: false, result: "Ran it.", num_turns: 2, duration_ms: 1, duration_api_ms: 1, total_cost_usd: 0, usage: { input_tokens: 1, output_tokens: 1 }, session_id: session, uuid: "00000000-0000-4000-8000-000000000005" });
    }
  }
});
`;

/** Grok's CLI as far as this asks it: signed in, its models, and `agent stdio` speaking ACP: a
 * prompt asks to run a command, and once that's answered, it's done (Grok 1.0.77 was checked the
 * same way in spike S2's ACP run). */
const GROK = String.raw`
const fs = require("node:fs");
const path = require("node:path");
const folder = process.env.STAND_IN_FOLDER;
const args = process.argv.slice(2);
if (args[0] === "models") { console.log("You are logged in.\n  * grok-test (default)"); process.exit(0); }
if (args.includes("--version")) { console.log("grok 1.0.77"); process.exit(0); }
if (!args.includes("stdio")) process.exit(1);
const log = what => fs.appendFileSync(path.join(folder, "grok.log"), JSON.stringify({ ...what, pid: process.pid }) + "\n");
const send = message => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n");
const update = (sessionId, update) => send({ method: "session/update", params: { sessionId, update } });
let buffer = "", prompt;
process.stdin.on("end", () => { log({ ended: true }); process.exit(0); });
process.stdin.on("data", chunk => {
  buffer += chunk;
  for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
    const line = buffer.slice(0, at);
    buffer = buffer.slice(at + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    if (message.method === "initialize") send({ id: message.id, result: { protocolVersion: 1, agentCapabilities: { loadSession: true, sessionCapabilities: { resume: {} } }, authMethods: [{ id: "xai.api_key" }] } });
    else if (message.method === "session/new") send({ id: message.id, result: { sessionId: "g1", models: { currentModelId: "grok-test", availableModels: [{ modelId: "grok-test", name: "Grok Test" }] } } });
    else if (message.method === "session/prompt") {
      prompt = message.id;
      const sessionId = message.params.sessionId;
      update(sessionId, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Asking first." } });
      update(sessionId, { sessionUpdate: "tool_call", toolCallId: "t1", title: "touch held.txt", kind: "execute", status: "pending", rawInput: { command: "touch held.txt" } });
      send({ id: 0, method: "session/request_permission", params: { sessionId, toolCall: { toolCallId: "t1", title: "touch held.txt", kind: "execute", rawInput: { command: "touch held.txt" } }, options: [{ optionId: "allow-once", name: "Allow", kind: "allow_once" }, { optionId: "reject-once", name: "Reject", kind: "reject_once" }] } });
      log({ asked: 0, parent: process.ppid });
    } else if (message.id === 0 && message.method === undefined) {
      log({ answered: message.result?.outcome?.optionId ?? "none" });
      update("g1", { sessionUpdate: "tool_call_update", toolCallId: "t1", status: "completed" });
      update("g1", { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Ran it." } });
      send({ id: prompt, result: { stopReason: "end_turn" } });
    } else if (message.id !== undefined && message.method !== undefined) send({ id: message.id, result: {} });
  }
});
`;

const boxModules = fileURLToPath(new URL("../node_modules", import.meta.url));

const freePort = () => new Promise((resolve, reject) => {
  const server = net.createServer();
  server.on("error", reject);
  server.listen(0, "127.0.0.1", () => {
    const { port } = server.address();
    server.close(() => resolve(port));
  });
});

async function until(wanted, what, within = 60_000, said = async () => "") {
  for (let waited = 0; waited < within; waited += 20) {
    const found = await wanted();
    if (found) return found;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert.fail(`Waited too long for ${what}${await said()}`);
}

const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };

/** A machine's box in folders of the test's own (`~/.kuvo-code` in a home of its own, for its
 * keeper), its bridge kept running as its launch keeps it. */
async function machine({ claudeVersion = "2.1.325", liveTransfer = false } = {}) {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kh-")));
  removeAtEnd(root);
  const home = path.join(root, ".kuvo-code");
  for (const folder of ["tmp", "work", "agents", "run", "bin", "home", "codex-home", "fake"]) fs.mkdirSync(path.join(home, folder), { recursive: true });
  fs.copyFileSync(built, path.join(home, "bin", "kuvo-agents.js"));
  fs.writeFileSync(path.join(home, "run", "agents.log"), "");
  const fake = path.join(home, "fake");
  fs.writeFileSync(path.join(fake, "app-server.js"), CODEX);
  fs.writeFileSync(path.join(fake, "codex"), `#!/bin/sh\nexport BUN_BE_BUN=1\n[ "$1" = "--version" ] && { echo "codex-cli 0.169.0"; exit 0; }\nexec "${bun}" "${path.join(fake, "app-server.js")}" "${fake}"\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(fake, "claude.js"), CLAUDE);
  fs.writeFileSync(path.join(fake, "claude"), `#!/bin/sh\nexport BUN_BE_BUN=1 STAND_IN_FOLDER="${fake}" STAND_IN_VERSION="${claudeVersion}"\nexec "${bun}" "${path.join(fake, "claude.js")}" "$@"\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(fake, "grok.js"), GROK);
  fs.writeFileSync(path.join(fake, "grok"), `#!/bin/sh\nexport BUN_BE_BUN=1 STAND_IN_FOLDER="${fake}"\nexec "${bun}" "${path.join(fake, "grok.js")}" "$@"\n`, { mode: 0o755 });
  // (Claude's SDK, as the box's own tests have it: nothing is downloaded.)
  fs.mkdirSync(path.join(home, "agents", "runtime", "claude"), { recursive: true });
  fs.symlinkSync(boxModules, path.join(home, "agents", "runtime", "claude", "node_modules"));
  const holders = path.join(home, "run", "agents", "hold");
  // (Holders run in sessions of their own: any this left are stopped, by their own pid.)
  atEnd(() => {
    for (const name of fs.existsSync(holders) ? fs.readdirSync(holders) : []) {
      if (!name.endsWith(".json")) continue;
      try { process.kill(JSON.parse(fs.readFileSync(path.join(holders, name), "utf8")).pid, "SIGTERM"); } catch {}
    }
  });

  const engine = http.createServer((request, response) => {
    if (request.url.startsWith("/api/event") || request.url.startsWith("/event")) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write(": open\n\n");
      return;
    }
    response.writeHead(404, { "content-type": "application/json" });
    response.end("{}");
  });
  await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
  atEnd(() => new Promise(resolve => { engine.closeAllConnections(); engine.close(resolve); }));

  const port = await freePort();
  const env = {
    PATH: `${fake}:/usr/bin:/bin`, HOME: path.join(home, "home"), TMPDIR: `${home}/tmp/`, CODEX_HOME: path.join(home, "codex-home"), BUN_BE_BUN: "1",
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_AGENTS_REMOTE: "1", KUVO_HOME: home, KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_AGENTS_PORT: String(port),
    KUVO_AGENTS_PASSWORD: "pw", KUVO_ENGINE_PORT: String(engine.address().port), KUVO_ENGINE_PID: String(process.pid), OPENCODE_PASSWORD: "engine", KUVO_SANDBOX: "off",
  };
  // Its launch, as it keeps a machine's bridge: started again a moment after it ends, its pid noted
  // where the keeper looks (`run/agents.pid`).
  if (liveTransfer) env.KUVO_AGENTS_KEPT = "1";
  const direct = 'echo $$ >"$5"; exec "$2" --config=/dev/null --no-env-file "$3" >>"$4" 2>&1';
  const loop = 'while kill -0 "$1" 2>/dev/null; do "$2" --config=/dev/null --no-env-file "$3" >>"$4" 2>&1 </dev/null & echo $! >"$5"; wait $!; sleep 1; done';
  const launch = spawn("sh", ["-c", liveTransfer ? direct : loop, "kuvo-agents", String(process.pid), bun, path.join(home, "bin", "kuvo-agents.js"), path.join(home, "run", "agents.log"), path.join(home, "run", "agents.pid")], { cwd: path.join(home, "work"), env, stdio: "ignore", detached: true });
  atEnd(() => new Promise(resolve => {
    if (launch.exitCode !== null) return resolve();
    launch.on("exit", () => resolve());
    try { process.kill(-launch.pid, "SIGTERM"); } catch { resolve(); }
  }));

  if (liveTransfer) atEnd(() => spawnSync("sh", [keeper, "stop"], { env: { PATH: "/usr/bin:/bin", HOME: root }, timeout: 30_000 }));
  const record = () => { try { return JSON.parse(fs.readFileSync(path.join(home, "run", "bridge.json"), "utf8")); } catch { return null; } };
  const auth = { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json", "x-opencode-directory": encodeURIComponent(path.join(home, "work")) };
  const api = async (method, route, body) => {
    try {
      const response = await fetch(`http://127.0.0.1:${record()?.port ?? port}/${route}`, { method, headers: auth, body: body ? JSON.stringify(body) : undefined });
      const text = await response.text();
      return { status: response.status, json: text ? JSON.parse(text) : null };
    } catch {
      return { status: 0, json: null };
    }
  };
  const up = besides => until(async () => {
    const said = record();
    if (!said?.pid || said.pid === besides) return null;
    const info = await api("GET", "api/info");
    return info.status === 200 && info.json.pid === said.pid ? said.pid : null;
  }, besides ? "the bridge started again to answer" : "the bridge to answer");
  const log = (name = "log") => { try { return fs.readFileSync(path.join(fake, name), "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); } catch { return []; } };
  const held = () => (fs.existsSync(holders) ? fs.readdirSync(holders) : []).filter(name => name.endsWith(".json")).map(name => JSON.parse(fs.readFileSync(path.join(holders, name), "utf8")));
  return { root, home, api, up, log, held, holders };
}

const textOf = messages => messages.filter(each => each.type === "assistant").flatMap(each => each.content ?? []).filter(block => block.type === "text").map(block => block.text);

/**
 * The crash and the stop, for one agent: its session, a message whose turn asks to run a command,
 * the bridge killed while it asks, the next bridge asking once more, your answer, the turn's end,
 * and the box stopped.
 */
async function continueWhileAsking({ provider, model, log, liveTransfer = false }) {
  const box = await machine({ liveTransfer });
  const first = await box.up();
  const made = await box.api("POST", "api/session", { title: "Held", location: { directory: path.join(box.home, "work") }, model: { providerID: provider, id: model } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  assert.equal((await box.api("POST", `api/session/${session}/prompt`, { id: "msg_heldagent00000001", delivery: "steer", text: "run it" })).status, 200);
  const asked = await until(() => box.log(log).find(each => "asked" in each), "the agent to ask", 60_000, async () =>
    `\n${fs.readFileSync(path.join(box.home, "run", "agents.log"), "utf8").slice(-3000)}\n${JSON.stringify((await box.api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? []).slice(-2000)}\n${JSON.stringify(box.log(log))}`);
  const priorAsk = await until(async () => (await box.api("GET", `api/session/${session}/permission`)).json?.data?.[0], "the approval to show");
  const holder = box.held().find(each => each.child === asked.pid);
  assert.ok(holder, `the agent runs under a holder: ${JSON.stringify(box.held().map(each => [each.pid, each.child, each.meta]))}`);
  assert.equal(asked.parent, holder.pid);
  await until(() => Object.values(box.held().find(each => each.pid === holder.pid)?.meta.turns ?? {})[0]?.session === session, "the holder's record to say which session's turn is under way");
  assert.equal(fs.statSync(box.holders).mode & 0o777, 0o700, "only you reach the holders");

  if (liveTransfer) {
    const bundle = path.join(box.home, "bin", "kuvo-agents.js");
    const changed = fs.readFileSync(bundle, "utf8").replace(/(Built from Packages\/Box\/src\/agents )[0-9a-f]{16,64}/, "$1" + "d".repeat(64));
    fs.writeFileSync(bundle + ".new", changed);
    fs.renameSync(bundle + ".new", bundle);
  } else process.kill(first, "SIGKILL");
  const second = await box.up(first);
  assert.notEqual(second, first);
  assert.ok(alive(asked.pid), "the original agent process lived through the bridge change");
  const asks = await until(async () => {
    const data = (await box.api("GET", `api/session/${session}/permission`)).json?.data;
    return data?.length ? data : null;
  }, "the approval to show again");
  assert.equal(asks.length, 1, "asked once");
  if (liveTransfer) assert.equal(asks[0].id, priorAsk.id, "the pending approval kept its identity");
  assert.equal((await box.api("POST", `api/session/${session}/permission/${asks[0].id}/reply`, { decision: "once" })).status, 200);
  const messages = await until(async () => {
    const page = (await box.api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the turn to end");
  assert.deepEqual(messages.filter(each => each.type === "user").map(each => each.text), ["run it"]);
  assert.deepEqual(textOf(messages), ["Asking first.", "Ran it."], "what the agent said before the crash and after, once each");
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"], "one turn, not cut");
  assert.equal(box.log(log).filter(each => "answered" in each).length, 1, "answered once");
  assert.deepEqual((await box.api("GET", `api/session/${session}/permission`)).json?.data, [], "and nothing is left to ask, with the turn over");
  assert.equal(box.log(log).filter(each => "asked" in each).length, 1, "asked about once: the turn wasn't started again");

  // Stopping the box (Stop, and uninstall's `off`): the held agent stops too, its holder with it.
  const stopped = spawnSync("sh", [keeper, "stop"], { env: { PATH: "/usr/bin:/bin", HOME: box.root }, encoding: "utf8", timeout: 60_000 });
  assert.equal(stopped.status, 0, stopped.stderr);
  assert.equal(alive(asked.pid), false, "nothing held runs after a stop");
  assert.equal(alive(holder.pid), false);
  assert.deepEqual(box.held(), []);
}

test("Codex: a crash while it waits on your approval, then a stop", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "codex-cli", model: "gpt-test", log: "log" }));

test("Claude Code: a crash while it waits on your approval, then a stop", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "claude-code", model: "default", log: "claude.log" }));

test("Grok (ACP): a crash while it waits on your approval, then a stop", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "grok-cli", model: "grok-test", log: "grok.log" }));

test("Claude Code keeps its pending approval through a live bridge update", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "claude-code", model: "default", log: "claude.log", liveTransfer: true }));

test("Grok (ACP) keeps its pending approval through a live bridge update", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "grok-cli", model: "grok-test", log: "grok.log", liveTransfer: true }));

test("Claude Code older than 2.1.268 (it doesn't ask again what's open, as a new client joins): a crash ends its turn as before, and it's stopped, not left waiting", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ claudeVersion: "2.1.200" });
  const first = await box.up();
  const made = await box.api("POST", "api/session", { title: "Older", location: { directory: path.join(box.home, "work") }, model: { providerID: "claude-code", id: "default" } });
  const session = made.json.data.id;
  assert.equal((await box.api("POST", `api/session/${session}/prompt`, { id: "msg_heldolder000000001", delivery: "steer", text: "run it" })).status, 200);
  const asked = await until(() => box.log("claude.log").find(each => "asked" in each), "Claude Code to ask");
  await until(() => box.held().find(each => each.child === asked.pid)?.meta.version === "2.1.200", "its version in its holder's record");
  process.kill(first, "SIGKILL");
  await box.up(first);
  const messages = await until(async () => {
    const page = (await box.api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "its turn to end");
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["interrupted"], "as a crash ended it before");
  await until(() => !alive(asked.pid), "it to be stopped");
  assert.equal(box.log("claude.log").filter(each => "answered" in each).length, 0);
});
