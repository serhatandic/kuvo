// A machine's coding agents' bridge killed (SIGKILL: a crash) while a coding agent's turn waits on
// your approval: the agent, under its holder, lives on; the bridge its launch starts again takes the
// turn up, shows the approval again, once; you answer it once, and the turn ends as it would have,
// one turn, with what the agent said before and after the crash. Then the box is stopped
// (`kuvo-start stop`), and nothing it held runs on. The real bridge (box/dist/kuvo-agents.js, on
// Bun), its real holders and keeper, and stand-ins for Codex's app-server and Claude Code (they
// speak their protocols; the real ones were checked the same way in spikes S1 and S2).
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
const installed = () => { try { return fs.readFileSync(path.join(folder, "codex-version"), "utf8"); } catch { return "0.169.0"; } };
const startedWith = installed();
if (process.argv[3] === "update") {
  fs.writeFileSync(path.join(folder, "installer-started"), "");
  const timer = setInterval(() => {
    if (!fs.existsSync(path.join(folder, "finish-install"))) return;
    clearInterval(timer);
    fs.writeFileSync(path.join(folder, "codex-version"), "0.170.0");
    process.exit(0);
  }, 20);
} else {
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
      log({ answered: message.result?.decision, startedWith, files: installed() });
      send({ method: "item/started", params: { threadId, turnId, item: { type: "agentMessage", id: "m" + turnId, text: "" } } });
      send({ method: "item/agentMessage/delta", params: { threadId, turnId, itemId: "m" + turnId, delta: "Ran it." } });
      send({ method: "item/completed", params: { threadId, turnId, item: { type: "agentMessage", id: "m" + turnId, text: "Ran it." } } });
      send({ method: "turn/completed", params: { threadId, turn: { id: turnId, status: "completed" } } });
    } else if (message.method === "thread/start") send({ id: message.id, result: { thread: { id: "t" + process.pid + "_" + ++threads } } });
    else if (message.method === "thread/resume") send({ id: message.id, result: { thread: { id: message.params.threadId } } });
    else if (message.method === "thread/backgroundTerminals/list") send({ id: message.id, result: { data: [] } });
    else if (message.method === "turn/start") {
      const threadId = message.params.threadId, turnId = "u" + ++turns, id = asks++;
      send({ id: message.id, result: { turn: { id: turnId, status: "inProgress" } } });
      send({ method: "turn/started", params: { threadId, turn: { id: turnId } } });
      send({ method: "turn/plan/updated", params: { threadId, turnId, plan: [{ step: "Keep the same row", status: "inProgress" }] } });
      send({ method: "item/started", params: { threadId, turnId, item: { type: "agentMessage", id: "a" + turnId, text: "" } } });
      send({ method: "item/completed", params: { threadId, turnId, item: { type: "agentMessage", id: "a" + turnId, text: "Asking first." } } });
      open.set(id, { threadId, turnId });
      send({ id, method: "item/commandExecution/requestApproval", params: { threadId, turnId, itemId: "c" + turnId, command: "echo held" } });
      log({ asked: id, parent: process.ppid, startedWith });
    } else if (message.method === "account/read") send({ id: message.id, result: { account: { type: "apiKey" } } });
    else if (message.method === "model/list") send({ id: message.id, result: { data: [{ id: "gpt-test", model: "gpt-test", displayName: "GPT Test", isDefault: true }] } });
    else if (message.id !== undefined) send({ id: message.id, result: {} });
  }
});
}
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
      if (message.request.subtype === "initialize") {
        log({ initialized: true });
        if (fs.existsSync(path.join(folder, "fail-rejoin"))) {
          fs.unlinkSync(path.join(folder, "fail-rejoin"));
          log({ rejoinFailed: true });
          setTimeout(() => send({ type: "control_response", response: { subtype: "error", request_id: message.request_id, error: "Cannot rejoin this client" } }), 750);
          continue;
        }
      }
      const response = message.request.subtype === "initialize" ? { commands: [], models, agents: [], output_style: "default", available_output_styles: ["default"], account: {} } : {};
      send({ type: "control_response", response: { subtype: "success", request_id: message.request_id, response, ...(message.request.subtype === "initialize" ? { pending_permission_requests: open ? [ask] : [] } : {}) } });
    } else if (message.type === "user") {
      send({ type: "system", subtype: "init", session_id: session, claude_code_version: process.env.STAND_IN_VERSION, cwd: process.cwd(), tools: ["Bash"], model: "claude-opus-5-5", permissionMode: "default", uuid: "00000000-0000-4000-8000-000000000000" });
      send({ type: "system", subtype: "memory_recall", session_id: session, uuid: "00000000-0000-4000-8000-000000000006", paths: ["MEMORY.md"] });
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
      update(sessionId, { sessionUpdate: "current_mode_update", currentModeId: "review" });
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

// The real Cursor worker loads this local SDK stand-in. Its in-memory run is the only thing
// that can finish the first message, so a bridge cannot pass this by resending it.
const CURSOR_SDK = String.raw`
import fs from "node:fs";
import path from "node:path";
export class JsonlLocalAgentStore { constructor(root) { this.root = root } }
export class AuthenticationError extends Error {}
export const Cursor = { me: async () => ({}), models: { list: async () => [{ id: "cursor-test" }] } };
const make = agentId => ({
  agentId, close() {},
  async send(text, { onDelta }) {
    const folder = process.env.STAND_IN_FOLDER;
    fs.appendFileSync(path.join(folder, "cursor.log"), JSON.stringify({ text, pid: process.pid, parent: process.ppid }) + "\n");
    const say = text => onDelta({ update: { type: "text-delta", text } });
    await say("Before.");
    if (fs.existsSync(path.join(folder, "cursor-task"))) {
      const callId = "child-1";
      const toolCall = { type: "task", args: { description: "Read the project", prompt: "Find the entry point" } };
      await onDelta({ update: { type: "tool-call-started", callId, toolCall } });
      await onDelta({ update: { type: "tool-call-delta", callId, taskUpdate: { type: "text-delta", text: "Found entry.ts." } } });
      await onDelta({ update: { type: "tool-call-completed", callId, toolCall: { ...toolCall, result: { status: "success", value: {} } } } });
      await say("Child finished.");
    }
    const ending = (async () => {
      const key = text.match(/turn-liveness-\d+$/)?.[0] ?? "cursor-release";
      while (!fs.existsSync(path.join(folder, key))) await new Promise(resolve => setTimeout(resolve, 20));
      await say(" After.");
      return { status: "finished" };
    })();
    return { wait: () => ending, cancel: async () => {} };
  },
});
export const Agent = { create: async () => make("cursor-live"), resume: async id => make(id) };
`;

// It uses the same ACP request IDs as the Grok stand-in, with Antigravity's authenticate-first
// and config surface. Its permission and prompt finishes are sent only to the original login.
const ANTIGRAVITY = GROK
  .replace('if (!args.includes("stdio")) process.exit(1);', '')
  .replace('"grok.log"', '"antigravity.log"')
  .replace('send({ id: prompt, result: { stopReason: "end_turn" } });', 'if (!fs.existsSync(path.join(folder, "antigravity-disk-end"))) send({ id: prompt, result: { stopReason: "end_turn" } });')
  .replace('id: "xai.api_key"', 'id: "oauth-personal"')
  .replace('modelId: "grok-test", name: "Grok Test"', 'modelId: "antigravity-test", name: "Antigravity Test"');

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
async function machine({ claudeVersion = "2.1.325", liveTransfer = false, cursor = false, programUpdate = false, single = false, livenessEngine = false } = {}) {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kh-")));
  removeAtEnd(root);
  const home = path.join(root, ".kuvo-code");
  for (const folder of ["tmp", "work", "agents", "run", "bin", "home", "codex-home", "fake"]) fs.mkdirSync(path.join(home, folder), { recursive: true });
  fs.copyFileSync(built, path.join(home, "bin", "kuvo-agents.js"));
  fs.writeFileSync(path.join(home, "run", "agents.log"), "");
  const fake = path.join(home, "fake");
  fs.writeFileSync(path.join(fake, "app-server.js"), CODEX);
  fs.writeFileSync(path.join(fake, "codex-version"), "0.169.0");
  if (programUpdate) fs.writeFileSync(path.join(home, "agents", "accounts.json"), JSON.stringify([{ kind: "codex", id: "work" }]));
  fs.writeFileSync(path.join(fake, "codex"), `#!/bin/sh\nexport BUN_BE_BUN=1\n[ "$1" = "--version" ] && { printf "codex-cli "; cat "${fake}/codex-version"; exit 0; }\nexec "${bun}" "${path.join(fake, "app-server.js")}" "${fake}" "$@"\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(fake, "claude.js"), CLAUDE);
  fs.writeFileSync(path.join(fake, "claude"), `#!/bin/sh\nexport BUN_BE_BUN=1 STAND_IN_FOLDER="${fake}" STAND_IN_VERSION="${claudeVersion}"\nexec "${bun}" "${path.join(fake, "claude.js")}" "$@"\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(fake, "grok.js"), GROK);
  fs.writeFileSync(path.join(fake, "grok"), `#!/bin/sh\nexport BUN_BE_BUN=1 STAND_IN_FOLDER="${fake}"\nexec "${bun}" "${path.join(fake, "grok.js")}" "$@"\n`, { mode: 0o755 });
  const agy = path.join(home, "agents", "antigravity");
  fs.mkdirSync(path.join(agy, "1.3.0"), { recursive: true });
  fs.mkdirSync(path.join(agy, "profile", "antigravity-acp"), { recursive: true });
  fs.writeFileSync(path.join(agy, "profile", "antigravity-acp", "acp_token.json"), "{}", { mode: 0o600 });
  fs.writeFileSync(path.join(agy, "1.3.0", ".complete"), "");
  fs.writeFileSync(path.join(fake, "antigravity.js"), ANTIGRAVITY);
  fs.writeFileSync(path.join(agy, "1.3.0", "agy_acp_server.par"), `#!/bin/sh\nexport BUN_BE_BUN=1 STAND_IN_FOLDER="${fake}"\nexec "${bun}" "${path.join(fake, "antigravity.js")}" "$@"\n`, { mode: 0o755 });
  if (cursor) {
    const sdk = path.join(home, "agents", "runtime", "cursor", "node_modules", "@cursor", "sdk");
    fs.mkdirSync(sdk, { recursive: true });
    fs.writeFileSync(path.join(sdk, "package.json"), '{"name":"@cursor/sdk","version":"1.0.35","type":"module","main":"index.js"}');
    fs.writeFileSync(path.join(sdk, "index.js"), CURSOR_SDK);
  }
  if (cursor || programUpdate) {
    // The fixture's auth is local. Block any account/limits probes outside this scratch box.
    const bundle = path.join(home, "bin", "kuvo-agents.js");
    const network = 'const localFetch = globalThis.fetch; globalThis.fetch = (url, ...args) => { const address = new URL(typeof url === "string" || url instanceof URL ? url : url.url); return address.hostname === "127.0.0.1" ? localFetch(url, ...args) : Promise.reject(new Error("Outside this test")); };\n';
    fs.writeFileSync(bundle, network + fs.readFileSync(bundle, "utf8"));
  }
  // (Claude's SDK, as the box's own tests have it: nothing is downloaded.)
  fs.mkdirSync(path.join(home, "agents", "runtime", "claude"), { recursive: true });
  fs.symlinkSync(boxModules, path.join(home, "agents", "runtime", "claude", "node_modules"));
  const holders = path.join(home, "run", "agents", "hold");
  // (Holders run in sessions of their own: any this left are stopped, by their own pid.)
  atEnd(() => {
    if (programUpdate) fs.writeFileSync(path.join(fake, "finish-install"), "");
    for (const name of fs.existsSync(holders) ? fs.readdirSync(holders) : []) {
      if (!name.endsWith(".json")) continue;
      try { process.kill(JSON.parse(fs.readFileSync(path.join(holders, name), "utf8")).pid, "SIGTERM"); } catch {}
    }
  });

  const engine = livenessEngine ? await engineFixture(home, bun) : http.createServer((request, response) => {
    if (request.url.startsWith("/api/event") || request.url.startsWith("/event")) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write(": open\n\n");
      return;
    }
    // An Allow reply checks the engine's current Blocks, including after a bridge restart.
    const config = request.method === "GET" && request.url === "/api/config";
    response.writeHead(config ? 200 : 404, { "content-type": "application/json" });
    response.end(config ? '{"data":[{"info":{"policies":[]}}]}' : "{}");
  });
  if (!livenessEngine) {
    await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
    atEnd(() => new Promise(resolve => { engine.closeAllConnections(); engine.close(resolve); }));
  }

  const port = await freePort();
  const env = {
    PATH: `${fake}:/usr/bin:/bin`, HOME: path.join(home, "home"), TMPDIR: `${home}/tmp/`, CODEX_HOME: path.join(home, "codex-home"), BUN_BE_BUN: "1",
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_AGENTS_REMOTE: "1", KUVO_HOME: home, KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_AGENTS_PORT: String(port),
    KUVO_AGENTS_PASSWORD: "pw", KUVO_ENGINE_PORT: String(engine.address().port), KUVO_ENGINE_PID: String(process.pid), OPENCODE_PASSWORD: "engine", KUVO_SANDBOX: "off",
  };
  // Its launch, as it keeps a machine's bridge: started again a moment after it ends, its pid noted
  // where the keeper looks (`run/agents.pid`).
  if (liveTransfer) env.KUVO_AGENTS_KEPT = "1";
  if (cursor) Object.assign(env, { CURSOR_API_KEY: "fixture-only", STAND_IN_FOLDER: fake });
  const direct = 'echo $$ >"$5"; exec "$2" --config=/dev/null --no-env-file "$3" >>"$4" 2>&1';
  const loop = 'while kill -0 "$1" 2>/dev/null; do "$2" --config=/dev/null --no-env-file "$3" >>"$4" 2>&1 </dev/null & echo $! >"$5"; wait $!; sleep 1; done';
  const start = () => spawn("sh", ["-c", liveTransfer || single ? direct : loop, "kuvo-agents", String(process.pid), bun, path.join(home, "bin", "kuvo-agents.js"), path.join(home, "run", "agents.log"), path.join(home, "run", "agents.pid")], { cwd: path.join(home, "work"), env, stdio: "ignore", detached: true });
  let launch = start();
  const shutdown = () => new Promise(resolve => {
    if (launch.exitCode !== null || launch.signalCode !== null) return resolve();
    launch.on("exit", () => resolve());
    try { process.kill(-launch.pid, "SIGTERM"); } catch { resolve(); }
  });
  atEnd(shutdown);

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
  // The desktop chooses Ask First when opening these sessions. The engine’s Blocks are separate.
  const createSession = body => api("POST", "api/session", {
    permissions: [
      { action: "shell", resource: "*", effect: "ask" },
      { action: "edit", resource: "*", effect: "ask" },
    ],
    ...body,
  });
  const up = besides => until(async () => {
    const said = record();
    if (!said?.pid || said.pid === besides) return null;
    const info = await api("GET", "api/info");
    return info.status === 200 && info.json.pid === said.pid ? said.pid : null;
  }, besides ? "the bridge started again to answer" : "the bridge to answer");
  const log = (name = "log") => { try { return fs.readFileSync(path.join(fake, name), "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); } catch { return []; } };
  const held = () => (fs.existsSync(holders) ? fs.readdirSync(holders) : []).filter(name => name.endsWith(".json")).map(name => JSON.parse(fs.readFileSync(path.join(holders, name), "utf8")));
  const recover = () => {
    const run = path.join(home, "run");
    const debug = fs.openSync(path.join(run, "agents.log"), "a");
    const recovered = spawn("sh", ["-c", 'echo $$ >"$1"; shift; exec "$@"', "recover", path.join(run, "agents.pid"), bun,
      "--config=/dev/null", "--no-env-file", path.join(run, "agents", "previous", "kuvo-agents.js"), "handover-recovery"],
    { cwd: path.join(home, "work"), env, stdio: ["ignore", debug, debug] });
    fs.closeSync(debug);
    atEnd(() => { if (recovered.exitCode === null) recovered.kill(); });
    return recovered;
  };
  return { root, home, api, createSession, up, log, held, holders, recover, shutdown, engine, restart: () => { launch = start(); } };
}

const textOf = messages => messages.filter(each => each.type === "assistant").flatMap(each => each.content ?? []).filter(block => block.type === "text").map(block => block.text);

function changeBridge(box) {
  const bundle = path.join(box.home, "bin", "kuvo-agents.js");
  const changed = fs.readFileSync(bundle, "utf8").replace(/(Built from Packages\/Box\/src\/agents )[0-9a-f]{16,64}/, "$1" + "d".repeat(64));
  fs.writeFileSync(bundle + ".new", changed);
  fs.renameSync(bundle + ".new", bundle);
}

/**
 * The crash and the stop, for one agent: its session, a message whose turn asks to run a command,
 * the bridge killed while it asks, the next bridge asking once more, your answer, the turn's end,
 * and the box stopped.
 */
async function continueWhileAsking({ provider, model, log, liveTransfer = false, releaseSwitch = false }) {
  const box = await machine({ liveTransfer, single: releaseSwitch });
  const first = await box.up();
  const made = await box.createSession({ title: "Held", location: { directory: path.join(box.home, "work") }, model: { providerID: provider, id: model } });
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

  const beforeCut = (await box.api("GET", `api/session/${session}/message?limit=100&order=asc`)).json.data;
  const notices = page => page.filter(each => each.type === "synthetic" && each.text.endsWith("switched to its review mode.")).map(each => each.id);
  if (releaseSwitch) {
    const extra = await box.createSession({ title: "Another session", location: { directory: path.join(box.home, "work") }, model: { providerID: "claude-code", id: "default" } });
    assert.equal(extra.status, 200);
    const extraID = extra.json.data.id;
    assert.equal((await box.api("POST", `api/session/${extraID}/prompt`, { id: "msg_heldslow000000001", delivery: "steer", text: "run it" })).status, 200);
    const slow = await until(() => box.held().find(each => each.meta.session === extraID && each.meta.version && Object.keys(each.meta.turns).length), "another held session");
    // Hold one peer's acknowledgement back so the outgoing bridge has time to observe the
    // first socket closing while it's still alive. This is the multi-holder release path.
    process.kill(slow.pid, "SIGSTOP");
    try {
      changeBridge(box);
      process.kill(first, "SIGUSR2");
      await until(() => !alive(first), "the outgoing bridge to exit");
    } finally {
      process.kill(slow.pid, "SIGCONT");
    }
    box.restart();
  } else if (liveTransfer) changeBridge(box);
  else process.kill(first, "SIGKILL");
  const second = await box.up(first);
  assert.notEqual(second, first);
  if (releaseSwitch) {
    const page = (await box.api("GET", `api/session/${session}/message?limit=100&order=asc`)).json.data;
    assert.deepEqual(page.filter(each => each.type === "idle").map(each => each.outcome), [], "detaching the old bridge leaves the running turn open");
    const adopted = box.held().find(each => each.id === holder.id);
    assert.equal(adopted?.child, asked.pid, "the replacement bridge takes up the same agent process");
    assert.equal(adopted.generation, holder.generation + 1);
  }
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
  const syntheticTool = provider === "codex-cli" ? "todowrite" : provider === "claude-code" ? "memory" : undefined;
  if (syntheticTool && liveTransfer) {
    const ids = page => page.flatMap(each => Array.isArray(each.content) ? each.content : []).filter(part => part.type === "tool" && part.name === syntheticTool).map(part => part.id);
    assert.equal(ids(beforeCut).length, 1);
    assert.deepEqual(ids(messages), ids(beforeCut), "the synthetic tool keeps its original ID");
  }
  if (provider === "grok-cli" && liveTransfer) {
    assert.equal(notices(beforeCut).length, 1);
    assert.deepEqual(notices(messages), notices(beforeCut), "ACP's mode notice keeps its original ID and appears once");
  }
  assert.equal(box.log(log).filter(each => "answered" in each).length, 1, "answered once");
  assert.deepEqual((await box.api("GET", `api/session/${session}/permission`)).json?.data, [], "and nothing is left to ask, with the turn over");
  assert.equal(box.log(log).filter(each => "asked" in each).length, 1, "asked about once: the turn wasn't started again");
  if (releaseSwitch) return;

  // Stopping the box (Stop, and uninstall's `off`): the held agent stops too, its holder with it.
  const stopped = spawnSync("sh", [keeper, "stop"], { env: { PATH: "/usr/bin:/bin", HOME: box.root }, encoding: "utf8", timeout: 60_000 });
  assert.equal(stopped.status, 0, stopped.stderr);
  assert.equal(alive(asked.pid), false, "nothing held runs after a stop");
  assert.equal(alive(holder.pid), false);
  assert.deepEqual(box.held(), []);
}

test("Codex: a crash while it waits on your approval, then a stop", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "codex-cli", model: "gpt-test", log: "log" }));

for (const [provider, model, log] of [
  ["codex-cli", "gpt-test", "log"],
  ["antigravity", "antigravity-test", "antigravity.log"],
]) test(`release switch keeps the running ${provider} turn open`, { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider, model, log, releaseSwitch: true }));

for (const [provider, model, log] of [
  ["claude-code", "default", "claude.log"],
  ["codex-cli", "gpt-test", "log"],
]) test(`a legacy ${provider} holder keeps running with the default compaction setting`, { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ single: true });
  const first = await box.up();
  const made = await box.createSession({ title: "Legacy compaction", location: { directory: path.join(box.home, "work") }, model: { providerID: provider, id: model } });
  assert.equal(made.status, 200);
  const route = `api/session/${made.json.data.id}`;
  const send = async (id) => assert.equal((await box.api("POST", `${route}/prompt`, { id, delivery: "steer", text: "run it" })).status, 200);
  const ask = async () => until(async () => (await box.api("GET", `${route}/permission`)).json?.data?.[0], "the agent's approval");
  const finish = async () => {
    assert.equal((await box.api("POST", `${route}/permission/${(await ask()).id}/reply`, { decision: "reject" })).status, 200);
    await until(async () => (await box.api("GET", "api/session/active")).json?.data?.[made.json.data.id] === undefined, "the session to finish");
  };
  await send("msg_legacycompact00001");
  await ask();
  const original = box.log(log).find(row => "asked" in row);
  const holder = box.held().find(row => row.child === original.pid);
  assert.ok(holder);
  if (provider === "claude-code") await finish();
  process.kill(first, "SIGUSR2");
  await until(() => !alive(first), "the outgoing bridge to exit");
  const file = path.join(box.holders, holder.id + ".json");
  const legacy = JSON.parse(fs.readFileSync(file, "utf8"));
  if (provider === "claude-code") delete legacy.meta.opened.compaction;
  else delete legacy.meta.compaction;
  // The persisted holder record as releases before compaction settings wrote it.
  fs.writeFileSync(file, JSON.stringify(legacy));
  box.restart();
  await box.up(first);
  if (provider === "codex-cli") await finish();
  await send("msg_legacycompact00002");
  await ask();
  assert.deepEqual(box.log(log).filter(row => "asked" in row).map(row => row.pid), [original.pid, original.pid]);
  await finish();
});

test("Claude Code: a crash while it waits on your approval, then a stop", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "claude-code", model: "default", log: "claude.log" }));

test("Grok (ACP): a crash while it waits on your approval, then a stop", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "grok-cli", model: "grok-test", log: "grok.log" }));

test("Codex keeps the visible plan tool ID through a live bridge update", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "codex-cli", model: "gpt-test", log: "log", liveTransfer: true }));

test("a native program update waits for its held turn and queues the next prompt", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ programUpdate: true });
  await box.up();
  const make = async (title, providerID = "codex-cli", id = "gpt-test") => {
    const response = await box.createSession({
      title, location: { directory: path.join(box.home, "work") }, model: { providerID, id },
    });
    assert.equal(response.status, 200);
    return response.json.data.id;
  };
  const first = await make("On the old program", "codex-cli@work");
  const next = await make("On the new program");
  const grok = await make("A different program", "grok-cli", "grok-test");
  const send = (session, id) => box.api("POST", `api/session/${session}/prompt`, { id, text: "run it", delivery: "steer" });
  const pending = async session => (await box.api("GET", `api/session/${session}/permission`)).json?.data?.[0];
  assert.equal((await send(first, "msg_programupdate0001")).status, 200);
  const ask = await until(() => pending(first), "the active turn's approval");
  const old = box.log().find(each => "asked" in each);
  assert.equal(box.held().find(holder => holder.child === old.pid)?.pid, old.parent);
  assert.equal((await box.api("POST", "api/kuvo/updates/codex-cli", {})).status, 200);
  const installer = path.join(box.home, "fake", "installer-started");
  assert.equal((await send(next, "msg_programupdate0002")).status, 200);
  assert.equal((await send(grok, "msg_programupdate0003")).status, 200);
  const unrelated = await until(() => pending(grok), "the other program to keep admitting work");
  await new Promise(resolve => setTimeout(resolve, 500));
  assert.equal(fs.existsSync(installer), false, "the in-place installer must wait for the running turn");
  assert.equal(box.log().filter(each => "asked" in each).length, 1, "the next turn stays in the inbox");
  const reply = (session, id) => box.api("POST", `api/session/${session}/permission/${id}/reply`, { decision: "once" });
  assert.equal((await reply(first, ask.id)).status, 200);
  assert.equal((await reply(grok, unrelated.id)).status, 200);
  await until(() => fs.existsSync(installer), "the installer to start once the active turn finishes");
  assert.deepEqual(box.log().filter(each => "answered" in each).map(each => [each.pid, each.startedWith, each.files]), [[old.pid, "0.169.0", "0.169.0"]]);
  assert.equal(box.log().filter(each => "asked" in each).length, 1, "the prompt is still queued during installation");
  assert.deepEqual((await box.api("GET", `api/session/${next}/inbox`)).json.data.map(each => each.id), ["msg_programupdate0002"]);
  fs.writeFileSync(path.join(box.home, "fake", "finish-install"), "");
  const later = await until(() => pending(next), "the queued prompt to run on the new program");
  const seen = box.log().filter(each => "asked" in each);
  assert.equal(seen.length, 2);
  assert.equal(seen[1].startedWith, "0.170.0");
  assert.notEqual(seen[1].pid, old.pid);
  assert.equal((await reply(next, later.id)).status, 200);
});

test("a bridge crash during the program drain preserves the pending update and hold", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ programUpdate: true });
  const first = await box.up();
  const make = async title => {
    const made = await box.createSession({
      title, location: { directory: path.join(box.home, "work") },
      model: { providerID: "codex-cli", id: "gpt-test" },
    });
    assert.equal(made.status, 200);
    return made.json.data.id;
  };
  const active = await make("Drain before the crash");
  const waiting = await make("Hold after the crash");
  const pending = async session => (await box.api("GET", `api/session/${session}/permission`)).json?.data?.[0];
  const send = (session, id) => box.api("POST", `api/session/${session}/prompt`, { id, text: "run it", delivery: "steer" });
  const reply = (session, id) => box.api("POST", `api/session/${session}/permission/${id}/reply`, { decision: "once" });
  const inbox = async session => (await box.api("GET", `api/session/${session}/inbox`)).json?.data.map(item => item.id);
  assert.equal((await send(active, "msg_programdrain0001")).status, 200);
  await until(() => pending(active), "the old turn to wait for approval");
  const native = box.log().find(item => "asked" in item).pid;
  assert.equal((await box.api("POST", "api/kuvo/updates/codex-cli", {})).status, 200);
  const installer = path.join(box.home, "fake", "installer-started");
  assert.equal((await send(waiting, "msg_programdrain0002")).status, 200);
  assert.deepEqual(await inbox(waiting), ["msg_programdrain0002"]);
  assert.equal(fs.existsSync(installer), false);

  const journal = path.join(box.home, "agents", "program-draining", "codex.json");
  const originalDrain = JSON.parse(fs.readFileSync(journal, "utf8"));
  process.kill(first, "SIGKILL");
  await box.up(first);
  assert.deepEqual(JSON.parse(fs.readFileSync(journal, "utf8")), originalDrain, "the restart preserves the attempt and original deadline");
  const ask = await until(() => pending(active), "the original turn to resume");
  assert.ok(alive(native));
  assert.equal((await send(waiting, "msg_programdrain0003")).status, 200);
  assert.deepEqual(await inbox(waiting), ["msg_programdrain0002", "msg_programdrain0003"]);
  assert.equal(fs.existsSync(installer), false, "the installer is still waiting on the original turn");
  assert.equal((await reply(active, ask.id)).status, 200);
  await until(() => fs.existsSync(installer), "the recovered update to start its installer");
  assert.deepEqual(await inbox(waiting), ["msg_programdrain0002", "msg_programdrain0003"]);
  fs.writeFileSync(path.join(box.home, "fake", "finish-install"), "");
  const next = await until(() => pending(waiting), "the first queued prompt to start");
  assert.equal((await reply(waiting, next.id)).status, 200);
  const last = await until(async () => {
    const found = await pending(waiting);
    return found && found.id !== next.id ? found : null;
  }, "the second queued prompt to start");
  assert.equal((await reply(waiting, last.id)).status, 200);
  assert.equal(fs.existsSync(journal), false, "a finished update retires its drain");
  assert.deepEqual(box.log().filter(item => "asked" in item).map(item => item.startedWith), ["0.169.0", "0.170.0", "0.170.0"]);
});

test("a replacement bridge keeps prompts queued until a detached program installer exits", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ programUpdate: true });
  const first = await box.up();
  const made = await box.createSession({
    title: "Wait across restart", location: { directory: path.join(box.home, "work") },
    model: { providerID: "codex-cli", id: "gpt-test" },
  });
  assert.equal(made.status, 200);
  const session = made.json.data.id;
  assert.equal((await box.api("POST", "api/kuvo/updates/codex-cli", {})).status, 200);
  await until(() => fs.existsSync(path.join(box.home, "fake", "installer-started")), "the detached installer to start");
  process.kill(first, "SIGKILL");
  await box.up(first);
  const id = "msg_programrestart0001";
  assert.equal((await box.api("POST", `api/session/${session}/prompt`, { id, text: "run after restart", delivery: "steer" })).status, 200);
  assert.deepEqual((await box.api("GET", `api/session/${session}/inbox`)).json.data.map(each => each.id), [id]);
  assert.deepEqual(box.log().filter(each => "asked" in each), []);
  fs.writeFileSync(path.join(box.home, "fake", "finish-install"), "");
  const ask = await until(async () => (await box.api("GET", `api/session/${session}/permission`)).json?.data?.[0], "the queued turn to start after installation");
  assert.deepEqual(box.log().filter(each => "asked" in each).map(each => each.startedWith), ["0.170.0"]);
  assert.equal((await box.api("POST", `api/session/${session}/permission/${ask.id}/reply`, { decision: "once" })).status, 200);
});

test("Claude Code keeps its pending approval through a live bridge update", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "claude-code", model: "default", log: "claude.log", liveTransfer: true }));

test("Grok (ACP) keeps its pending approval through a live bridge update", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "grok-cli", model: "grok-test", log: "grok.log", liveTransfer: true }));

test("Antigravity ACP keeps its original session, prompt and approval through a live bridge update", { skip: bun ? false : NO_BUN }, () =>
  continueWhileAsking({ provider: "antigravity", model: "antigravity-test", log: "antigravity.log", liveTransfer: true }));

test("Cursor's live worker keeps streaming the same SDK run through a bridge update", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ liveTransfer: true, cursor: true });
  const first = await box.up();
  const made = await box.createSession({
    title: "Cursor held", location: { directory: path.join(box.home, "work") },
    model: { providerID: "cursor", id: "cursor-test" },
  });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  const message = "msg_heldcursor00000001";
  assert.equal((await box.api("POST", `api/session/${session}/prompt`, { id: message, delivery: "steer", text: "continue once" })).status, 200);
  const page = async () => (await box.api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data;
  const visible = await until(async () => (await page())?.find(item => textOf([item]).includes("Before.")), "Cursor's first delta", 20_000, async () => `\n${JSON.stringify(await page())}`);
  const initial = box.log("cursor.log");
  assert.equal(initial.length, 1);
  const original = box.held().find(record => record.child === initial[0].pid);
  assert.equal(original.pid, initial[0].parent);
  assert.equal(original.meta.turns["cursor-live"].session, session);

  changeBridge(box);
  await box.up(first);
  const current = box.held().find(record => record.id === original.id);
  assert.equal(current.child, original.child);
  assert.ok(current.generation > original.generation);
  fs.writeFileSync(path.join(box.home, "fake", "cursor-release"), "");
  const messages = await until(async () => {
    const found = await page();
    return found?.at(-1)?.type === "idle" ? found : null;
  }, "the original Cursor SDK run to end");
  assert.deepEqual(messages.filter(item => item.type === "user").map(item => item.id), [message]);
  assert.deepEqual(messages.filter(item => item.type === "assistant").map(item => item.id), [visible.id]);
  assert.deepEqual(textOf(messages), ["Before. After."]);
  assert.deepEqual(messages.filter(item => item.type === "idle").map(item => item.outcome), ["succeeded"]);
  assert.deepEqual(box.log("cursor.log"), initial, "the native process received exactly one prompt");
});

test("a rejected Claude rejoin keeps the checkpoint for the previous bridge", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ liveTransfer: true });
  const first = await box.up();
  const made = await box.createSession({
    title: "Rejoin failure", location: { directory: path.join(box.home, "work") },
    model: { providerID: "claude-code", id: "default" },
  });
  assert.equal(made.status, 200);
  const session = made.json.data.id;
  assert.equal((await box.api("POST", `api/session/${session}/prompt`, { id: "msg_rejoinfailure00001", delivery: "steer", text: "run it" })).status, 200);
  const pending = async () => (await box.api("GET", `api/session/${session}/permission`)).json?.data?.[0];
  const before = await until(pending, "the original approval");
  const native = box.log("claude.log").find(each => "asked" in each).pid;
  const run = path.join(box.home, "run");
  const checkpoint = path.join(run, "agents", "handover.json");
  const previous = path.join(run, "agents", "previous", "kuvo-agents.js");
  fs.writeFileSync(path.join(box.home, "fake", "fail-rejoin"), "");
  changeBridge(box);
  await until(() => box.log("claude.log").some(each => each.rejoinFailed), "the provisional Claude SDK initialization");
  assert.ok(fs.existsSync(checkpoint), "recovery is still available while rejoin is pending");
  assert.ok(fs.existsSync(previous), "the previous build is still available");
  assert.equal(JSON.parse(fs.readFileSync(path.join(run, "bridge.json"), "utf8")).pid, first, "admission is still closed");
  const plan = JSON.parse(fs.readFileSync(checkpoint, "utf8"));
  await until(() => !alive(plan.next), "the rejected candidate to exit", 12_000);
  assert.ok(alive(native), "the candidate detached without killing Claude");
  assert.ok(fs.existsSync(checkpoint), "a rejoin failure leaves the checkpoint intact");
  const recovered = box.recover();
  assert.equal(await box.up(first), recovered.pid);
  assert.equal((await pending())?.id, before.id);
  assert.equal((await box.api("POST", `api/session/${session}/permission/${before.id}/reply`, { decision: "once" })).status, 200);
  await until(async () => (await box.api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data?.at(-1)?.outcome === "succeeded", "the original turn to finish");
  assert.equal(box.log("claude.log").filter(each => "answered" in each).length, 1);
});

test("Cursor's completed child keeps its session and tool link through cutover", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ liveTransfer: true, cursor: true });
  const first = await box.up();
  const made = await box.createSession({
    title: "Cursor parent", location: { directory: path.join(box.home, "work") },
    model: { providerID: "cursor", id: "cursor-test" },
  });
  assert.equal(made.status, 200);
  const session = made.json.data.id;
  fs.writeFileSync(path.join(box.home, "fake", "cursor-task"), "");
  assert.equal((await box.api("POST", `api/session/${session}/prompt`, { id: "msg_heldcursortask0001", delivery: "steer", text: "run a task" })).status, 200);
  const page = async id => (await box.api("GET", `api/session/${id}/message?limit=100&order=asc`)).json?.data ?? [];
  const link = messages => messages.flatMap(each => each.content ?? []).find(each => each.id === "child-1" && each.state?.status === "completed")?.state.metadata?.sessionID;
  const child = await until(async () => {
    const messages = await page(session);
    return textOf(messages).includes("Child finished.") && link(messages);
  }, "the completed child and later parent output");
  const childMessages = await page(child);
  assert.deepEqual(textOf(childMessages), ["Found entry.ts."]);
  changeBridge(box);
  await box.up(first);
  fs.writeFileSync(path.join(box.home, "fake", "cursor-release"), "");
  const completed = await until(async () => {
    const messages = await page(session);
    return messages.at(-1)?.outcome === "succeeded" ? messages : null;
  }, "the parent to finish");
  assert.equal(link(completed), child, "the rebuilt tool still links to the original child");
  assert.deepEqual(await page(child), childMessages, "the child transcript is unchanged");
  assert.deepEqual((await box.api("GET", `api/session?parentID=${session}`)).json.data.map(each => each.id), [child]);
});

test("Claude Code older than 2.1.268 (it doesn't ask again what's open, as a new client joins): a crash ends its turn as before, and it's stopped, not left waiting", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine({ claudeVersion: "2.1.200" });
  const first = await box.up();
  const made = await box.createSession({ title: "Older", location: { directory: path.join(box.home, "work") }, model: { providerID: "claude-code", id: "default" } });
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

// A real process with the engine's HTTP and event surface for these turns. It sends an inbox
// receipt and stays at work until the test releases the turn or kills this process.
async function engineFixture(home, bun) {
  const folder = path.join(home, "fake");
  const script = path.join(folder, "engine.js");
  const port = await freePort();
  fs.writeFileSync(script, String.raw`
import { appendFileSync } from "node:fs";
const [port, log] = process.argv.slice(2);
const id = "ses_engine_fixture";
const base = "/api/session/" + id;
const clients = new Set();
let running, idle, sequence = 0;
const event = (type, data = {}) => {
  const frame = new TextEncoder().encode("data: " + JSON.stringify({ id: ++sequence, type, data: { sessionID: id, ...data } }) + "\n\n");
  for (const client of clients) client.enqueue(frame);
};
const reply = data => Response.json({ data });
Bun.serve({ hostname: "127.0.0.1", port: Number(port), idleTimeout: 0, async fetch(request) {
  const url = new URL(request.url).pathname;
  if (url === "/api/event") {
    let client;
    return new Response(new ReadableStream({
      start(controller) { clients.add(client = controller); controller.enqueue(new TextEncoder().encode(": open\n\n")); },
      cancel() { clients.delete(client); },
    }), { headers: { "content-type": "text/event-stream" } });
  }
  if (url === "/api/provider") return reply([{ id: "fixture-engine", name: "Fixture Engine" }]);
  if (url === "/api/model") return reply([{ id: "fixture-test", providerID: "fixture-engine", limit: { context: 128000, output: 16000 } }]);
  if (url === "/api/session" && request.method === "POST") return reply({ id });
  if (url === "/api/session/active") return reply(running ? { [id]: { type: "busy" } } : {});
  if (url === base + "/prompt") {
    const prompt = await request.json();
    running = true;
    appendFileSync(log, JSON.stringify({ pid: process.pid, text: prompt.text }) + "\n");
    event("session.inbox.delivered", { inboxID: prompt.id });
    event("session.execution.started");
    event("session.text.delta", { ordinal: 0, delta: "Working." });
  }
  if (url === "/release" && running) {
    running = false;
    idle = Date.now();
    event("session.execution.succeeded");
    event("session.idle");
  }
  if (url === base && request.method === "GET") return reply({ id, outcome: idle ? "succeeded" : undefined, time: { idle } });
  return reply({});
} });
`);
  let child;
  const kill = async (signal = "SIGTERM") => {
    if (!child || child.exitCode !== null || child.signalCode !== null) return;
    const exited = new Promise(resolve => child.once("exit", resolve));
    child.kill(signal);
    await exited;
  };
  const start = async () => {
    child = spawn(bun, [script, String(port), path.join(folder, "engine.log")], {
      cwd: folder, env: { HOME: path.join(home, "home"), TMPDIR: path.join(home, "tmp"), BUN_BE_BUN: "1", PATH: "/usr/bin:/bin" },
      stdio: "ignore",
    });
    const record = path.join(home, "run", "engine.json");
    fs.writeFileSync(record + ".new", JSON.stringify({ port, password: "engine", pid: child.pid }));
    fs.renameSync(record + ".new", record);
    await until(() => fetch(`http://127.0.0.1:${port}/api/provider`).then(r => r.ok, () => false), "the fixture engine to listen", 6000);
  };
  atEnd(kill);
  await start();
  return { address: () => ({ port }), start, kill, release: () => fetch(`http://127.0.0.1:${port}/release`) };
}

for (const [provider, model, log] of [
  ["claude-code", "claude-opus-5-5", "claude.log"],
  ["codex-cli", "gpt-test", "log"],
  ["antigravity", "antigravity-test", "antigravity.log"],
  ["grok-cli", "grok-test", "grok.log"],
  ["cursor", "cursor-test", "cursor.log"],
  ["fixture-engine", "fixture-test", "engine.log"],
]) test(`turn liveness for ${provider}: completion and process exit release the queue once`, { skip: bun ? false : NO_BUN }, async t => {
  const engine = provider === "fixture-engine";
  const cursor = provider === "cursor";
  const antigravity = provider === "antigravity";
  const box = await machine({ single: true, cursor, livenessEngine: engine });
  t.after(async () => {
    await box.shutdown();
    if (engine) await box.engine.kill();
  });
  await box.up();
  const made = await box.createSession({
    title: "Turn liveness",
    location: { directory: path.join(box.home, "work") },
    model: { providerID: provider, id: model },
  });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  const route = `api/session/${session}`;
  const id = i => `msg_turnliveness0000000${i}`;
  const send = async (i, delivery) => {
    const answer = await box.api("POST", `${route}/prompt`, { id: id(i), text: `turn-liveness-${i}`, delivery });
    assert.equal(answer.status, 200, JSON.stringify(answer.json));
  };
  const apiData = async name => (await box.api("GET", `${route}/${name}`)).json?.data;
  const messages = () => apiData("message?limit=100&order=asc");
  const diagnostic = async () => `\n${JSON.stringify(await messages())?.slice(-2400)}\n${fs.readFileSync(path.join(box.home, "run", "agents.log"), "utf8").slice(-2400)}`;
  const began = i => until(() => box.log(log).filter(row => engine || cursor || "asked" in row)[i - 1], `turn ${i} to reach its process`, 9000, diagnostic);
  const release = async i => {
    if (engine) assert.ok((await box.engine.release()).ok);
    else if (cursor) fs.writeFileSync(path.join(box.home, "fake", `turn-liveness-${i}`), "");
    else {
      const ask = await until(async () => (await apiData("permission"))?.[0], "the turn's permission", 5000, diagnostic);
      const answer = await box.api("POST", `${route}/permission/${ask.id}/reply`, { decision: "reject" });
      assert.equal(answer.status, 200, JSON.stringify(answer.json));
    }
  };

  // Antigravity's regression: the final text and durable transcript arrive but ACP never returns
  // its prompt response. Other agents finish normally while their process remains available.
  const marker = path.join(box.home, "fake", "antigravity-disk-end");
  if (antigravity) fs.writeFileSync(marker, "");
  await send(1, "steer");
  const first = await began(1);
  await send(2, "queue");
  assert.deepEqual((await apiData("inbox"))?.map(item => item.id), [id(2)]);
  await release(1);
  if (antigravity) {
    await until(() => box.log(log).some(row => "answered" in row), "Antigravity to stream its reply", 5000, diagnostic);
    const logs = path.join(box.home, "agents", "antigravity", "profile", "antigravity-acp", "brain", "g1", ".system_generated", "logs");
    fs.mkdirSync(logs, { recursive: true });
    fs.writeFileSync(path.join(logs, "transcript.jsonl"), [
      { step_index: 1, type: "USER_INPUT", status: "DONE" },
      { step_index: 2, type: "PLANNER_RESPONSE", status: "DONE", content: "Ran it." },
    ].map(row => JSON.stringify(row)).join("\n") + "\n");
  }
  const second = await began(2);
  if (antigravity) fs.unlinkSync(marker);
  else assert.equal(second.pid, first.pid, "the next turn can use the live process");
  await send(3, "queue");
  await send(4, "queue");
  assert.deepEqual((await apiData("inbox"))?.map(item => item.id), [id(3), id(4)]);
  if (engine) {
    await box.engine.kill("SIGKILL");
    await until(async () => {
      const page = await messages();
      const active = (await box.api("GET", "api/session/active")).json?.data;
      return page?.at(-1)?.reason === "agent-exit" && !active?.[session];
    }, "the dead engine's turn to end before its replacement starts", 6000, diagnostic);
    assert.deepEqual((await apiData("inbox"))?.map(item => item.id), [id(3), id(4)]);
    await box.engine.start();
  } else {
    assert.ok(box.held().some(holder => holder.child === second.pid), "kill only this fixture's agent");
    process.kill(second.pid, "SIGKILL");
  }
  const third = await began(3);
  assert.notEqual(third.pid, second.pid);
  await release(3);
  await began(4);
  await release(4);

  const page = await until(async () => {
    const page = await messages();
    return page?.filter(item => item.type === "idle").length === 4 ? page : undefined;
  }, "all four turns to end", 9000, diagnostic);
  assert.deepEqual(page.filter(item => item.type === "user").map(item => item.id), [1, 2, 3, 4].map(id));
  assert.deepEqual(page.filter(item => item.type === "idle").map(({ outcome, reason }) => [outcome, reason]), [
    ["succeeded", undefined], ["interrupted", "agent-exit"], ["succeeded", undefined], ["succeeded", undefined],
  ]);
  assert.deepEqual(await apiData("inbox"), []);
});
