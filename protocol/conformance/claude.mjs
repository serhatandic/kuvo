// Keep the reference bridge and both resident binaries on the same private, local CLI fixture.
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { standInClaude } from "../../box/bridge-tests/claude-stand-in.js";
import assert from "node:assert/strict";
import {
  boxBinary, boxCommand, isolatedEnv, launch, localRelay, privateHome, readJSON, request, scratch,
  startBox, typescriptRuntime, until, write,
} from "./adapter.mjs";

export const headers = { "x-kuvo-from": "app" };
export const provider = "claude-code";

export function claudeEnvironment(binary, home, env, fixture) {
  const bin = standInClaude({
    root: env.HOME,
    agents: binary.typescript ? path.join(home, "agents") : undefined,
    ...fixture,
  });
  const system = process.platform === "win32" ? path.join(env.SystemRoot, "System32") : "/usr/bin:/bin";
  return { ...env, KUVO_TEST_PATH_ONLY: "1", PATH: [bin, system].join(path.delimiter) };
}

export async function startClaudeBridge(binary, home, env, { engine } = {}) {
  if (!binary.typescript) return undefined;
  fs.rmSync(path.join(home, "run", "bridge.json"), { force: true });
  const bundle = fileURLToPath(new URL("../../box/dist/kuvo-agents.js", import.meta.url));
  const bridge = launch(typescriptRuntime(), ["--config=/dev/null", "--no-env-file", bundle], {
    ...env, KUVO_HOME: home, KUVO_AGENTS_REMOTE: "1", KUVO_AGENTS_PORT: String(await freePort()),
    KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_AGENTS_PASSWORD: "fixture",
    KUVO_SANDBOX: "off", BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
    ...(engine && { KUVO_ENGINE_PORT: String(engine.port), OPENCODE_PASSWORD: "fixture-engine" }),
  }, home);
  try {
    await until(() => fs.existsSync(path.join(home, "run", "bridge.json")),
      () => "The TypeScript bridge never published its fixture listener. " + bridge.log());
    return bridge;
  } catch (error) {
    await bridge.stop();
    throw error;
  }
}

export async function keptListener(home, keeper, previous) {
  return until(async () => {
    const file = path.join(home, "run", "serve.json");
    if (!fs.existsSync(file)) return;
    const listener = readJSON(file);
    if (listener.pid === previous) return;
    try {
      const info = await request(listener, "GET", "kuvo/info", undefined, headers, 1000);
      return info.status === 200 && listener;
    } catch {
      return;
    }
  }, () => "The keeper did not publish a ready resident. " + keeper.log(), 25000);
}

export async function transcript(listener, route) {
  const answer = await request(listener, "GET", route + "/message?order=asc", undefined, headers);
  assert.equal(answer.status, 200, answer.text);
  return answer.json.data;
}

function holderRecords(home) {
  const folder = path.join(home, "run", "agents", "hold");
  if (!fs.existsSync(folder)) return [];
  // A holder removes its own record as it exits, so one listed may be gone when read.
  return fs.readdirSync(folder).filter(name => /^[a-f\d]{12}\.json$/.test(name)).flatMap(name => {
    try { return [readJSON(path.join(folder, name))]; }
    catch (error) { if (error.code === "ENOENT") return []; throw error; }
  });
}

export function heldClaude(home, session) {
  return holderRecords(home).find(record => record.meta?.provider === provider && record.meta?.session === session);
}

// Stops the holders this case started and waits for them to exit, so the scratch home can go.
export async function stopFixtureHolders(home) {
  await stopPids(holderRecords(home).map(record => record.pid));
}

export async function stopPids(pids) {
  pids = pids.filter(pid => Number.isInteger(pid) && pid > 1);
  const alive = pid => {
    try { process.kill(pid, 0); return true; }
    catch (error) { if (error.code === "ESRCH") return false; throw error; }
  };
  for (const pid of pids) {
    try { process.kill(pid, "SIGTERM"); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  }
  const deadline = Date.now() + 10000;
  while (pids.some(alive) && Date.now() < deadline) await new Promise(done => setTimeout(done, 50));
}

export function seedSession(home, id, { directory = home, model = "opus", permissions, agent = "build" } = {}) {
  write(path.join(home, "agents/sessions", id, "session.json"), JSON.stringify({
    id, title: "Claude fixture", projectID: "fixture", agent, ...(permissions && { permissions }),
    location: { directory }, model: { providerID: provider, id: model },
    time: { created: 1, updated: 1 }, cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
  }));
}

// Two Rust releases of the same binary in a scratch home, and the keeper command that picks one.
export function twoReleases(binary, home, env, relay) {
  const old = "versions/0.8.0-beta.1-123456789abc/bin";
  const next = "versions/0.8.0-beta.2-123456789abc/bin";
  for (const [folder, version] of [[old, "0.8.0-beta.1"], [next, "0.8.0-beta.2"]]) {
    const installed = path.join(home, folder, path.basename(binary.file));
    write(path.join(home, folder, "..", "box.json"), JSON.stringify({ kuvo: "box/1", launch: "rust", version }));
    fs.mkdirSync(path.dirname(installed), { recursive: true });
    try { fs.linkSync(binary.file, installed); }
    catch { fs.copyFileSync(binary.file, installed); }
  }
  const control = async args => {
    const command = boxCommand(binary, home, env, relay, ["keep", ...args]);
    assert.equal((await command.exited).code, 0, command.log());
  };
  return { old, next, control };
}

/**
 * The engine as far as the reference bridge asks it here: the computer's settings (`/api/config`),
 * with the policies (Settings › Permissions' Blocks) the Rust resident reads from the same
 * `box/opencode.json`. Nothing else.
 */
export async function settingsEngine(home) {
  const server = http.createServer((incoming, reply) => {
    const url = new URL(incoming.url, "http://engine");
    if (incoming.method === "GET" && url.pathname === "/api/config") {
      let settings = {};
      try { settings = readJSON(path.join(home, "box", "opencode.json")); } catch {}
      reply.writeHead(200, { "content-type": "application/json" });
      reply.end(JSON.stringify({ data: settings }));
      return;
    }
    reply.writeHead(404, { "content-type": "application/json" });
    reply.end("{}");
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { port: server.address().port, close: () => new Promise(resolve => server.close(resolve)) };
}

async function freePort() {
  const server = net.createServer();
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

// One resident (and, for the reference, its bridge) on a scripted Claude stand-in, shared by a
// file's cases: each case seeds its own session and scripts its own prompts.
export async function scriptedClaude(fixture = {}) {
  const binary = boxBinary();
  const root = scratch();
  const home = path.join(root, "box");
  const relay = await localRelay();
  privateHome(home);
  const env = { ...claudeEnvironment(binary, home, isolatedEnv(root), { turns: true, ...fixture }), KUVO_SANDBOX: "off" };
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "box", "opencode.json"), "{}\n");
  const engine = binary.typescript ? await settingsEngine(home) : undefined;
  let bridge, resident, listener;
  const scripts = {};
  const heardFile = path.join(env.HOME, "claude-heard.jsonl");
  const claude = {
    binary, home, env,
    get listener() { return listener; },
    async start() {
      bridge = await startClaudeBridge(binary, home, env, { engine });
      resident = startBox({ binary, home, env, relay });
      listener = await resident.ready();
      return listener;
    },
    async stop() {
      await resident?.stop();
      await bridge?.stop();
      resident = bridge = undefined;
    },
    async close() {
      await claude.stop();
      await stopFixtureHolders(home);
      await relay.close();
      await engine?.close();
      fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
    },
    log: () => [resident?.log(), bridge?.log()].filter(Boolean).join("\n"),
    /** What the prompt whose first line is `text` makes the stand-in do. */
    script(text, steps) {
      scripts[text] = steps;
      write(path.join(env.HOME, "claude-script.json"), JSON.stringify(scripts));
    },
    /** The next Claude Code started for a conversation says `stderr` and stops with `exit`, before it
     * reads anything (once `until` is there, leaving `linger` running: see the stand-in). */
    stopsAsItStarts(stderr, more = {}) {
      write(path.join(env.HOME, "claude-start.json"), JSON.stringify({ stderr, exit: 1, ...more }));
    },
    /** What the stand-in read: its arguments and every line, from each Claude Code it ran. */
    heard: () => fs.existsSync(heardFile)
      ? fs.readFileSync(heardFile, "utf8").split("\n").filter(Boolean).map(line => JSON.parse(line)) : [],
    seed: (id, options) => seedSession(home, id, options),
    /** What a command Claude Code left running (its task `task`) has printed so far. */
    printed(task, text) {
      const file = path.join(env.HOME, "claude-task-output.json");
      const outputs = fs.existsSync(file) ? readJSON(file) : {};
      if (text === undefined) delete outputs[task];
      else outputs[task] = text;
      write(file, JSON.stringify(outputs));
    },
    /** What the box says runs besides the session's turn (`kuvo/work/background…`). */
    async work(suffix = "") {
      const answer = await request(listener, "GET", `kuvo/work/background${suffix}`, undefined, headers, 20000);
      assert.equal(answer.status, 200, answer.text);
      return answer.json;
    },
    /** Lets a script waiting `{until: name}` go on. */
    go: name => write(path.join(env.HOME, name), ""),
    /** The session as it's saved (its totals, once a turn ends). */
    saved: id => readJSON(path.join(home, "agents/sessions", id, "session.json")),
    route: id => `kuvo/agents/api/session/${id}`,
    async prompt(id, text, more = {}) {
      const answer = await request(listener, "POST", claude.route(id) + "/prompt", {
        id: `msg_${id.replace(/^ses_/, "")}_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
        text, ...more,
      }, headers, 45000);
      assert.equal(answer.status, 200, answer.text);
    },
    rows: id => transcript(listener, claude.route(id)),
    async idles(id, count = 1, why = "The Claude turn never ended.") {
      return until(async () => {
        const rows = await claude.rows(id);
        return rows.filter(row => row.type === "idle").length >= count && rows;
      }, () => why + "\n" + claude.log(), 20000);
    },
    /** Asks for a summary now: its state, or the refusal. */
    async compact(id) {
      return request(listener, "POST", claude.route(id) + "/compact", {}, headers, 20000);
    },
    async interrupt(id) {
      const answer = await request(listener, "POST", claude.route(id) + "/interrupt", {}, headers, 20000);
      assert.equal(answer.status, 200, answer.text);
      return answer.json;
    },
    /** The box's answer to `request`, from the Claude Code that took the session's first message. */
    answered: (session, request_) => until(() => {
      const heard = claude.heard();
      const pid = heard.find(line => line.type === "user" && line.message?.content?.some?.(part =>
        part.type === "text" && part.text.startsWith(`go ${session}`)))?.pid;
      return heard.find(line => line.pid === pid && line.type === "control_response"
        && line.response?.request_id === request_)?.response;
    }, `The box never answered ${request_} in ${session}.`),
    async permissions(id) {
      const answer = await request(listener, "GET", claude.route(id) + "/permission", undefined, headers);
      assert.equal(answer.status, 200, answer.text);
      return answer.json.data ?? answer.json;
    },
    async replyPermission(id, request_, decision) {
      const answer = await request(listener, "POST", `${claude.route(id)}/permission/${request_}/reply`, { decision }, headers);
      assert.ok(answer.status < 300, answer.text);
    },
    async forms(id) {
      const answer = await request(listener, "GET", claude.route(id) + "/form", undefined, headers);
      assert.equal(answer.status, 200, answer.text);
      return answer.json.data ?? answer.json;
    },
    async replyForm(id, form, answers) {
      return request(listener, "POST", `${claude.route(id)}/form/${form}/reply`, { answer: answers }, headers);
    },
    async dismissForm(id, form, message) {
      const words = message === undefined ? "" : `?message=${encodeURIComponent(message)}`;
      return request(listener, "DELETE", `${claude.route(id)}/form/${form}${words}`, undefined, headers);
    },
    async selectAgent(id, agent) {
      return request(listener, "POST", `${claude.route(id)}/agent`, { agent }, headers);
    },
    async session(id) {
      const answer = await request(listener, "GET", `kuvo/agents/api/session/${id}`, undefined, headers);
      assert.equal(answer.status, 200, answer.text);
      return answer.json.data;
    },
  };
  return claude;
}

/** A request's reply as Claude Code streams it: each block whole as it ends, with its usage. */
export function streams(uuid, blocks, usage = { input: 10, read: 0, write: 0, output: 5 }) {
  const id = `msg_${Math.random().toString(36).slice(2)}`;
  const event = event => ({ type: "stream_event", event, parent_tool_use_id: null, session_id: "$SESSION" });
  const frames = [event({ type: "message_start", message: { id, usage: { input_tokens: usage.input, cache_read_input_tokens: usage.read, cache_creation_input_tokens: usage.write, output_tokens: 1 } } })];
  blocks.forEach((block, index) => {
    if (block.type === "text") {
      frames.push(event({ type: "content_block_start", index, content_block: { type: "text", text: "" } }));
      frames.push(event({ type: "content_block_delta", index, delta: { type: "text_delta", text: block.text } }));
    } else {
      frames.push(event({ type: "content_block_start", index, content_block: { ...block, input: {} } }));
      frames.push(event({ type: "content_block_delta", index, delta: { type: "input_json_delta", partial_json: JSON.stringify(block.input) } }));
    }
    frames.push(event({ type: "content_block_stop", index }));
    frames.push({ type: "assistant", message: { id, role: "assistant", content: [block] }, parent_tool_use_id: null, session_id: "$SESSION", user_message_uuids: [uuid] });
  });
  frames.push(event({ type: "message_delta", delta: { stop_reason: blocks.some(block => block.type === "tool_use") ? "tool_use" : "end_turn" }, usage: { output_tokens: usage.output } }));
  frames.push(event({ type: "message_stop" }));
  return frames;
}

/** What a tool came back with, and Claude Code's own account of it. */
export function toolResult(id, content, { error, structured } = {}) {
  return { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content, is_error: error }] }, parent_tool_use_id: null, session_id: "$SESSION", tool_use_result: structured };
}

export function result(uuids, subtype = "success", more = {}) {
  return { type: "result", subtype, is_error: subtype !== "success", result: "", session_id: "$SESSION", user_message_uuids: uuids, ...more };
}

/** Claude Code asking whether it may use a tool, as `request` (the box answers it). */
export function canUse(request_id, tool_name, input, tool_use_id, more = {}) {
  return { type: "control_request", request_id, request: { subtype: "can_use_tool", tool_name, input, tool_use_id, ...more } };
}

/** Claude Code running the box's PreToolUse hook before a tool, as `request` (the box answers it). */
export function hook(request_id, tool_name, tool_input, tool_use_id) {
  return { type: "control_request", request_id, request: { subtype: "hook_callback", callback_id: "$HOOK", tool_use_id, input: { hook_event_name: "PreToolUse", tool_name, tool_input, tool_use_id } } };
}

/** Like Bun's `toMatchObject`: objects in part, lists whole. */
export function assertMatch(actual, expected, at = "value") {
  if (expected === null || typeof expected !== "object") {
    assert.deepEqual(actual, expected, `${at}: ${JSON.stringify(actual)}`);
    return;
  }
  if (Array.isArray(expected)) {
    assert.ok(Array.isArray(actual), `${at} should be a list: ${JSON.stringify(actual)}`);
    assert.equal(actual.length, expected.length, `${at} should have ${expected.length}: ${JSON.stringify(actual)}`);
    expected.forEach((each, index) => assertMatch(actual[index], each, `${at}[${index}]`));
    return;
  }
  assert.ok(actual && typeof actual === "object", `${at} should be an object: ${JSON.stringify(actual)}`);
  for (const [key, each] of Object.entries(expected)) assertMatch(actual[key], each, `${at}.${key}`);
}

export const askFirst = ["edit", "write", "patch", "shell", "webfetch", "external_directory"]
  .map(action => ({ action, resource: "*", effect: "ask" }));
export const steps = rows => rows.filter(row => row.type === "assistant");
export const texts = rows => steps(rows).flatMap(row => row.content.flatMap(part => part.type === "text" ? [part.text] : []));
export const toolCalls = rows => steps(rows).flatMap(row => row.content.filter(part => part.type === "tool"));
export const idles = rows => rows.filter(row => row.type === "idle");
