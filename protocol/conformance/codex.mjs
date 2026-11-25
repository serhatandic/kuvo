// A scripted `codex app-server` (JSON-RPC on its pipes, as Codex 0.17x speaks it) that both
// residents run, and the harness its cases share.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {
  boxBinary, isolatedEnv, localRelay, privateHome, readJSON, request, scratch, startBox, until, write,
} from "./adapter.mjs";
import { headers, settingsEngine, startClaudeBridge, stopFixtureHolders, transcript } from "./claude.mjs";

export const provider = "codex-cli";

/**
 * The stand-in's program. Its turns play `codex-script.json` (by a line their words contain): each
 * step a notification `[method, params]` (its thread and turn filled in), `{ask: [method, params]}`
 * (a request that waits for its answer), `{until: file}` (waits for a file in its home), `"stop"`
 * (works until stopped) or `{end: {status, error}}`. What it reads it keeps in `codex-heard.jsonl`.
 */
const PROGRAM = String.raw`
const fs = require("node:fs");
const path = require("node:path");
const root = process.env.KUVO_CODEX_ROOT;
const home = process.env.CODEX_HOME || path.join(process.env.HOME || process.env.USERPROFILE, ".codex");
const auth = path.join(home, "fixture-auth.json");
const heard = (line) => fs.appendFileSync(path.join(root, "codex-heard.jsonl"), JSON.stringify({ pid: process.pid, ...line }) + "\n");
const args = process.argv.slice(2);
if (args[0] === "logout") {
  fs.rmSync(auth, { force: true });
  heard({ logout: true, home });
  process.exit(0);
}
if (args[0] !== "app-server") process.exit(2);
heard({ argv: args, env: { CODEX_HOME: process.env.CODEX_HOME ?? null, originator: process.env.CODEX_INTERNAL_ORIGINATOR_OVERRIDE ?? null } });
const read = (file, fallback) => { try { return JSON.parse(fs.readFileSync(path.join(root, file), "utf8")); } catch { return fallback; } };
const send = (message) => process.stdout.write(JSON.stringify(message) + "\n");
const waiting = new Map();
const threads = new Map();
/** What each thread left running in background terminals, as a script step set it. */
const terminals = new Map();
let asked = 0, turns = 0, made = 0;
let running;
const wait = (file) => new Promise((resolve) => {
  const look = () => fs.existsSync(path.join(root, file)) ? resolve() : setTimeout(look, 20);
  look();
});
async function play(threadId, turnId, steps) {
  const fill = (params) => ({ threadId, turnId, ...params });
  send({ method: "turn/started", params: { threadId, turn: { id: turnId } } });
  let end = { status: "completed" };
  for (const step of steps) {
    if (running?.turnId !== turnId) return;
    if (step === "stop") { await new Promise((resolve) => (running.stop = resolve)); return; }
    if (step.until) { await wait(step.until); continue; }
    if (step.end) { end = step.end; break; }
    // {terminals: [...], failsUntil?: file, endsWith?: file}: listed while the turn goes on and after.
    if (step.terminals) { terminals.set(threadId, step); continue; }
    if (step.ask) {
      const id = "ask-" + ++asked;
      send({ id, method: step.ask[0], params: fill(step.ask[1]) });
      heard({ asked: { id, method: step.ask[0], params: fill(step.ask[1]) } });
      await new Promise((resolve) => waiting.set(id, resolve));
      continue;
    }
    send({ method: step[0], params: fill(step[1]) });
  }
  if (running?.turnId !== turnId) return;
  running = undefined;
  send({ method: "turn/completed", params: { threadId, turn: { id: turnId, status: end.status, ...(end.error ? { error: end.error } : {}) } } });
}
let buffer = "";
process.stdin.on("end", () => process.exit(0));
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
    const line = buffer.slice(0, at);
    buffer = buffer.slice(at + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    heard({ message });
    const { id, method, params = {} } = message;
    const answer = (result) => send({ id, result });
    const refuse = (code, words) => send({ id, error: { code, message: words } });
    if (method === undefined) { waiting.get(id)?.(); waiting.delete(id); continue; }
    if (method === "initialize") {
      // (An older or newer Codex turns it down; one stuck never answers.)
      if (fs.existsSync(path.join(root, "codex-refuses-initialize"))) refuse(-32601, "Unknown method");
      else if (!fs.existsSync(path.join(root, "codex-never-initializes"))) answer({ userAgent: "codex-stand-in/1" });
    }
    else if (method === "initialized") {}
    else if (method === "account/read") {
      const saved = fs.existsSync(auth) ? JSON.parse(fs.readFileSync(auth, "utf8")) : null;
      answer({ account: saved, requiresOpenaiAuth: true });
    } else if (method === "account/login/start") {
      fs.mkdirSync(home, { recursive: true });
      if (params.type === "apiKey") {
        fs.writeFileSync(auth, JSON.stringify({ type: "apiKey" }));
        answer({ type: "apiKey" });
        continue;
      }
      const loginId = "login-" + Date.now();
      if (params.type === "chatgptDeviceCode") answer({ type: "chatgptDeviceCode", loginId, verificationUrl: "https://auth.example.test/device", userCode: "ABCD-1234" });
      else answer({ type: "chatgpt", loginId, authUrl: "https://auth.example.test/codex" });
      wait("codex-login-complete").then(() => {
        fs.writeFileSync(auth, JSON.stringify({ type: "chatgpt", email: "you@example.test", planType: "plus" }));
        send({ method: "account/login/completed", params: { loginId, success: true, error: null } });
      });
    } else if (method === "model/list") {
      const pages = read("codex-models.json", [[
        { id: "gpt-test", model: "gpt-test", displayName: "GPT Test", isDefault: true, hidden: false,
          supportedReasoningEfforts: [{ reasoningEffort: "low" }, { reasoningEffort: "high" }],
          serviceTiers: [{ id: "priority", name: "Fast", description: "1.5x speed" }] },
        { id: "gpt-mini", model: "gpt-mini", displayName: "GPT Mini", hidden: true, supportedReasoningEfforts: [] },
      ]]);
      const page = Number(params.cursor ?? 0);
      const data = pages[page].filter((model) => params.includeHidden || !model.hidden);
      answer({ data, nextCursor: page + 1 < pages.length ? String(page + 1) : null });
    } else if (method === "thread/start") {
      const thread = "0199a000-0000-7000-8000-" + String(process.pid % 100000).padStart(6, "0") + String(++made).padStart(6, "0");
      threads.set(thread, []);
      answer({ thread: { id: thread } });
    } else if (method === "thread/resume") {
      if (read("codex-gone.json", []).includes(params.threadId)) refuse(-32600, "no rollout found for thread id " + params.threadId);
      else { if (!threads.has(params.threadId)) threads.set(params.threadId, []); answer({ thread: { id: params.threadId } }); }
    } else if (method === "turn/start") {
      const turnId = "u" + ++turns + "-" + process.pid;
      const text = params.input?.[0]?.text ?? "";
      threads.get(params.threadId)?.push({ id: turnId, status: "inProgress", items: [{ type: "userMessage", id: "m" + turnId, clientId: params.clientUserMessageId, content: params.input }] });
      running = { threadId: params.threadId, turnId };
      answer({ turn: { id: turnId, status: "inProgress" } });
      const script = read("codex-script.json", {});
      const key = Object.keys(script).find((each) => text.includes(each));
      void play(params.threadId, turnId, key ? script[key] : [["item/completed", { item: { type: "agentMessage", id: "a" + turnId, text: "Done." } }]]);
    } else if (method === "turn/steer") {
      if (!running || running.turnId !== params.expectedTurnId || fs.existsSync(path.join(root, "codex-refuses-steers"))) { refuse(-32600, "no active turn to steer"); continue; }
      threads.get(params.threadId)?.find((turn) => turn.id === running.turnId)?.items.push({ type: "userMessage", id: "s" + Date.now(), clientId: params.clientUserMessageId, content: params.input });
      answer({ turnId: running.turnId });
      send({ method: "item/started", params: { threadId: params.threadId, turnId: running.turnId, item: { type: "userMessage", id: "s" + Date.now(), content: params.input } } });
    } else if (method === "turn/interrupt") {
      answer({});
      if (running && running.turnId === params.turnId) {
        const stopped = running;
        running = undefined;
        stopped.stop?.();
        send({ method: "turn/completed", params: { threadId: params.threadId, turn: { id: params.turnId, status: "interrupted" } } });
      }
    } else if (method === "thread/compact/start") {
      answer({});
      const thread = params.threadId;
      send({ method: "turn/started", params: { threadId: thread, turn: { id: "c1" } } });
      send({ method: "item/started", params: { threadId: thread, turnId: "c1", item: { type: "contextCompaction", id: "cc1" } } });
      send({ method: "item/completed", params: { threadId: thread, turnId: "c1", item: { type: "contextCompaction", id: "cc1" } } });
      send({ method: "turn/completed", params: { threadId: thread, turn: { id: "c1", status: "completed" } } });
    } else if (method === "thread/backgroundTerminals/list") {
      const kept = terminals.get(params.threadId);
      const there = (file) => file && fs.existsSync(path.join(root, file));
      if (kept?.failsUntil && !there(kept.failsUntil)) refuse(-32603, "background terminals unavailable");
      else answer({ data: kept && !there(kept.endsWith) ? kept.terminals : [], nextCursor: null });
    } else if (method === "thread/read") {
      const kept = threads.get(params.threadId);
      if (!kept) refuse(-32603, "history unavailable");
      else answer({ thread: { id: params.threadId, turns: kept.map((turn) => ({ ...turn, status: running?.turnId === turn.id ? "inProgress" : "completed" })) } });
    } else if (id !== undefined) answer({});
  }
});
`;

/** Puts the stand-in `codex` in `root`'s `.local/bin`, signed in as `signedIn` says. */
export function standInCodex(root, { signedIn = "chatgpt" } = {}) {
  const bin = path.join(root, ".local", "bin");
  fs.mkdirSync(bin, { recursive: true });
  if (process.platform === "win32") {
    fs.writeFileSync(path.join(bin, "codex.cjs"), PROGRAM);
    fs.writeFileSync(path.join(bin, "codex.cmd"), `@"${process.execPath}" "%~dp0codex.cjs" %*\r\n`);
  } else {
    fs.writeFileSync(path.join(bin, "codex"), `#!${process.execPath}\n${PROGRAM}`, { mode: 0o755 });
  }
  if (signedIn) {
    const account = signedIn === "apiKey" ? { type: "apiKey" } : { type: "chatgpt", email: "you@example.test", planType: "pro" };
    write(path.join(root, ".codex", "fixture-auth.json"), JSON.stringify(account));
  }
  return bin;
}

export function seedCodexSession(home, id, { directory = home, model = "gpt-test", permissions, agent = "build", state } = {}) {
  write(path.join(home, "agents/sessions", id, "session.json"), JSON.stringify({
    id, title: "Codex fixture", projectID: "fixture", agent, ...(permissions && { permissions }),
    location: { directory }, model: { providerID: provider, id: model },
    time: { created: 1, updated: 1 }, cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
  }));
  if (state) write(path.join(home, "agents/sessions", id, "agent.json"), JSON.stringify({ threads: { [provider]: { state } } }));
}

/** One resident (and, for the reference, its bridge) on the scripted Codex. */
export async function scriptedCodex(fixture = {}) {
  const binary = boxBinary();
  const root = scratch();
  const home = path.join(root, "box");
  const relay = await localRelay();
  privateHome(home);
  const base = isolatedEnv(root);
  const bin = standInCodex(base.HOME, fixture);
  const system = process.platform === "win32" ? path.join(base.SystemRoot, "System32") : "/usr/bin:/bin";
  const env = { ...base, KUVO_TEST_PATH_ONLY: "1", KUVO_SANDBOX: "off", KUVO_CODEX_ROOT: base.HOME, PATH: [bin, system].join(path.delimiter) };
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "box", "opencode.json"), "{}\n");
  const engine = binary.typescript ? await settingsEngine(home) : undefined;
  const heardFile = path.join(base.HOME, "codex-heard.jsonl");
  const scripts = {};
  let bridge, resident, listener;
  const codex = {
    binary, home, env, root: base.HOME,
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
      await codex.stop();
      await stopFixtureHolders(home);
      await relay.close();
      await engine?.close();
      fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
    },
    log: () => [resident?.log(), bridge?.log()].filter(Boolean).join("\n"),
    script(text, steps) {
      scripts[text] = steps;
      write(path.join(base.HOME, "codex-script.json"), JSON.stringify(scripts));
    },
    go: (name) => write(path.join(base.HOME, name), ""),
    heard: () => fs.existsSync(heardFile)
      ? fs.readFileSync(heardFile, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)) : [],
    /** What Kuvo sent the stand-in, by method. */
    sent: (method) => codex.heard().filter((line) => line.message?.method === method).map((line) => line.message),
    seed: (id, options) => seedCodexSession(home, id, options),
    route: (id) => `kuvo/agents/api/session/${id}`,
    async prompt(id, text, more = {}) {
      const answer = await request(listener, "POST", codex.route(id) + "/prompt", {
        id: `msg_${id.replace(/^ses_/, "")}_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
        text, ...more,
      }, headers, 45000);
      assert.equal(answer.status, 200, answer.text);
    },
    rows: (id) => transcript(listener, codex.route(id)),
    async idles(id, count = 1, why = "The Codex turn never ended.") {
      let rows = [];
      return until(async () => {
        rows = await codex.rows(id);
        return rows.filter((row) => row.type === "idle").length >= count && rows;
      }, () => [why, JSON.stringify(rows.slice(-4)), JSON.stringify(codex.heard().slice(-6)), codex.log()].join("\n"), 20000);
    },
    async compact(id) {
      return request(listener, "POST", codex.route(id) + "/compact", {}, headers, 20000);
    },
    async interrupt(id) {
      const answer = await request(listener, "POST", codex.route(id) + "/interrupt", {}, headers, 20000);
      assert.equal(answer.status, 200, answer.text);
    },
    async permissions(id) {
      const answer = await request(listener, "GET", codex.route(id) + "/permission", undefined, headers);
      assert.equal(answer.status, 200, answer.text);
      return answer.json.data ?? answer.json;
    },
    async replyPermission(id, ask, decision) {
      const answer = await request(listener, "POST", `${codex.route(id)}/permission/${ask}/reply`, { decision }, headers);
      assert.ok(answer.status < 300, answer.text);
    },
    async forms(id) {
      const answer = await request(listener, "GET", codex.route(id) + "/form", undefined, headers);
      assert.equal(answer.status, 200, answer.text);
      return answer.json.data ?? answer.json;
    },
    async dismissForm(id, form) {
      return request(listener, "DELETE", `${codex.route(id)}/form/${form}`, undefined, headers);
    },
    saved: (id) => readJSON(path.join(home, "agents/sessions", id, "session.json")),
    async replyForm(id, form, answers) {
      return request(listener, "POST", `${codex.route(id)}/form/${form}/reply`, { answer: answers }, headers);
    },
    /** The box's answer to the stand-in's ask about item `item`. */
    answeredFor: (item) => until(() => {
      const asked = codex.heard().find((line) => line.asked?.params.itemId === item)?.asked;
      return asked && codex.heard().find((line) => line.message?.id === asked.id && line.message.method === undefined)?.message;
    }, `The box never answered the ask about ${item}.`),
    /** The box's answer to the stand-in's ask `ask`. */
    answered: (ask) => until(() => codex.heard().find((line) => line.message?.id === ask && line.message.method === undefined)?.message,
      `The box never answered ${ask}.`),
    /** What the box says runs besides the session's turn (`kuvo/work/background…`). */
    async work(suffix = "") {
      const answer = await request(listener, "GET", `kuvo/work/background${suffix}`, undefined, headers, 20000);
      assert.equal(answer.status, 200, answer.text);
      return answer.json;
    },
    agentState: (id) => readJSON(path.join(home, "agents/sessions", id, "agent.json")).threads?.[provider]?.state,
  };
  return codex;
}
