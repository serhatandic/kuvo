// A scripted ACP agent that both residents run, as Grok's CLI (`grok agent stdio`) and as
// Google's `agy` (`agy acp`), and the harness their cases share.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import {
  boxBinary, isolatedEnv, localRelay, privateHome, readJSON, request, scratch, startBox, until, write,
} from "./adapter.mjs";
import { headers, settingsEngine, startClaudeBridge, stopFixtureHolders, transcript } from "./claude.mjs";

export const providers = { grok: "grok-cli", agy: "antigravity" };

/**
 * The stand-in's program, by the name it runs as. A prompt plays `acp-script.json` (by a line its
 * words contain): each step a session update `{update}`, a notification of its own `{ext: [method,
 * params]}`, a request that waits for its answer `{ask: [method, params]}`, `{until: file}` (waits
 * for a file in its root), `"hang"` (until it's told to cancel), `{end: stopReason}`, `{error:
 * {code, message}}`, `{own: {until, steps}}` (what it says by itself once its answer went),
 * `{opens: url}` (a command of its own opens a page: through `$BROWSER` where it's set, and
 * writing it to `$TMPDIR/url`), `{say: line}` (a line of its own on stdout, not ACP) or `{wait:
 * ms}`. As `agy`, its models have a `<model>-<level>` entry for each thinking level, as agy's do.
 * What it reads it keeps in `acp-heard.jsonl`.
 */
const PROGRAM = String.raw`
const fs = require("node:fs");
const path = require("node:path");
const root = process.env.KUVO_ACP_ROOT;
const name = path.basename(process.argv[1]).replace(/\.c?js$/, "");
const args = process.argv.slice(2);
const own = (file) => path.join(root, file);
const heard = (line) => fs.appendFileSync(own("acp-heard.jsonl"), JSON.stringify({ pid: process.pid, name, ...line }) + "\n");
const grokHome = process.env.GROK_HOME || path.join(process.env.HOME || process.env.USERPROFILE, ".grok");
const models = [{ modelId: "acp-test", name: "ACP Test", _meta: { reasoningEfforts: [{ value: "low" }, { value: "high", default: true }] } }, { modelId: "acp-fast", name: "ACP Fast" }];
const thinking = [{ value: "low", name: "Low" }, { value: "high", name: "High" }, { value: "xhigh", name: "Extra" }];
const agyModels = [...thinking.map((level) => ({ modelId: "acp-test-" + level.value, name: "ACP Test (" + level.name + ")" })), { modelId: "acp-fast", name: "ACP Fast" }];
const versionFile = own(name + "-version");
const currentVersion = () => fs.existsSync(versionFile) ? fs.readFileSync(versionFile, "utf8").trim() : "1.0.0";
if (args[0] === "--version") { process.stdout.write(name + " " + currentVersion() + "\n"); process.exit(0); }
if (args[0] === "update") {
  heard({ update: true });
  const toFile = own(name + "-update-to");
  if (fs.existsSync(toFile)) {
    fs.writeFileSync(versionFile, fs.readFileSync(toFile, "utf8").trim());
    const exe = path.join(path.dirname(process.argv[1]), process.platform === "win32" ? name + ".cmd" : name);
    fs.appendFileSync(exe, process.platform === "win32" ? "\r\n" : "\n");
  }
  process.exit(0);
}
if (name === "grok" && args[0] === "logout") { fs.rmSync(path.join(grokHome, "auth.json"), { force: true }); heard({ logout: true }); process.exit(0); }
if (name === "grok" && args[0] === "models") {
  heard({ argv: args });
  const signed = fs.existsSync(path.join(grokHome, "auth.json"));
  process.stdout.write(signed ? "You are logged in.\n" + models.map((m, i) => "* " + m.modelId + (i ? "" : " (default)")).join("\n") + "\n" : "Not logged in: run grok login\n");
  process.exit(0);
}
if (name === "agy" && args.includes("--login")) {
  heard({ login: true });
  process.stdout.write("Open this page to sign in: https://accounts.example.test/o/oauth2?client=agy\nPaste the authorization code here and press Enter: ");
  let typed = "";
  process.stdin.on("data", (chunk) => {
    typed += chunk;
    if (!typed.includes("\n")) return;
    const good = typed.trim() === "good-code";
    if (good) fs.writeFileSync(path.join(process.env.GEMINI_HOME, "agy-signed-in"), "yes");
    process.exit(good ? 0 : 1);
  });
  return;
}
heard({ argv: args, script: process.argv[1], startedVersion: currentVersion(), env: { GROK_HOME: process.env.GROK_HOME ?? null, GEMINI_HOME: process.env.GEMINI_HOME ?? null, threshold: process.env.GROK_AUTO_COMPACT_THRESHOLD_PERCENT ?? null } });
// agy is signed in by its own sign-in, or takes an earlier agent's that says it may (a stand-in
// token; whether the real agy takes a real one is checked in the update rehearsal).
const legacy = () => { try { return fs.readFileSync(path.join(process.env.GEMINI_HOME, "antigravity-acp", "acp_token.json"), "utf8").includes("accepted-legacy"); } catch { return false; } };
const signedIn = () => name === "grok" ? fs.existsSync(path.join(grokHome, "auth.json")) || Boolean(process.env.XAI_API_KEY) : fs.existsSync(path.join(process.env.GEMINI_HOME, "agy-signed-in")) || legacy();
const read = (file, fallback) => { try { return JSON.parse(fs.readFileSync(own(file), "utf8")); } catch { return fallback; } };
const send = (message) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n");
const waiting = new Map();
let asked = 0, made = 0;
const running = new Map();
const wait = (file) => new Promise((resolve) => { const look = () => fs.existsSync(own(file)) ? resolve() : setTimeout(look, 20); look(); });
const setup = (sessionId) => ({
  sessionId,
  models: name === "agy" ? { availableModels: agyModels, currentModelId: "acp-test-high" } : { availableModels: models, currentModelId: "acp-test" },
  configOptions: name === "agy"
    ? [{ id: "model", category: "model", currentValue: "acp-test", options: models.map((m) => ({ value: m.modelId, name: m.name })) },
       { id: "thought_level", category: "thought_level", type: "select", currentValue: "high", options: thinking },
       { id: "mode", category: "mode", currentValue: "default", options: [{ value: "default" }, { value: "yolo" }, { value: "plan" }] }]
    : [{ id: "reasoning_effort", category: "thought_level", currentValue: "high", options: [] }],
});
async function play(sessionId, id, steps) {
  const state = { stop: undefined };
  running.set(sessionId, state);
  let stopReason = "end_turn";
  let own;
  for (const step of steps) {
    // {own: {until, steps}}: once this prompt is answered and the file is there, it goes on by itself.
    if (step.own) { own = step.own; continue; }
    if (state.cancelled) break;
    if (step === "hang") { await new Promise((resolve) => (state.stop = resolve)); break; }
    if (step.until) { await wait(step.until); continue; }
    if (step.wait) { await new Promise((resolve) => setTimeout(resolve, step.wait)); continue; }
    if (step.say) { process.stdout.write(step.say + "\n"); continue; }
    if (step.opens) {
      // (As a command it runs would: in a process of its own, with its environment.)
      const command = 'const fs = require("node:fs"), path = require("node:path"), os = require("node:os"); const url = process.argv[1];' +
        'if (process.env.BROWSER) { try { require("node:child_process").execFileSync(process.env.BROWSER, [url], { stdio: "ignore", shell: process.platform === "win32" }); } catch {} }' +
        'fs.writeFileSync(path.join(process.env.TMPDIR || os.tmpdir(), "url"), url + "\\n");';
      require("node:child_process").spawnSync(process.execPath, ["-e", command, step.opens], { stdio: "ignore", env: process.env });
      heard({ opened: { browser: Boolean(process.env.BROWSER) } });
      continue;
    }
    if (step.end) { stopReason = step.end; break; }
    if (step.error) { running.delete(sessionId); send({ id, error: step.error }); return; }
    if (step.update) { send({ method: "session/update", params: { sessionId: step.session ?? sessionId, update: step.update } }); continue; }
    if (step.ext) { send({ method: step.ext[0], params: { sessionId, ...step.ext[1] } }); continue; }
    if (step.ask) {
      const ask = "ask-" + ++asked;
      heard({ asked: { id: ask, method: step.ask[0] } });
      const answer = await new Promise((resolve) => { waiting.set(ask, resolve); send({ id: ask, method: step.ask[0], params: { sessionId, ...step.ask[1] } }); });
      heard({ answer: { id: ask, ...answer } });
    }
  }
  running.delete(sessionId);
  send({ id, result: { stopReason: state.cancelled ? "cancelled" : stopReason } });
  if (own) {
    await wait(own.until);
    for (const step of own.steps) {
      if (step.update) send({ method: "session/update", params: { sessionId, update: step.update } });
      if (step.ext) send({ method: step.ext[0], params: { sessionId, ...step.ext[1] } });
    }
  }
}
let buffer = "";
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let at;
  while ((at = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, at);
    buffer = buffer.slice(at + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    heard({ message });
    const { id, method, params } = message;
    if (method === undefined) { waiting.get(id)?.(message); waiting.delete(id); continue; }
    const answer = (result) => send({ id, result });
    const refuse = (code, words) => send({ id, error: { code, message: words } });
    if (method === "initialize") answer({ protocolVersion: 1, authMethods: name === "grok" ? [{ id: "cached_token" }, { id: "xai.api_key" }] : [{ id: "oauth-personal" }], agentCapabilities: { loadSession: true, sessionCapabilities: { resume: {} }, mcpCapabilities: { http: true }, auth: { logout: name === "agy" } } });
    else if (method === "authenticate") signedIn() ? answer({}) : refuse(-32000, "Authentication required");
    else if (method === "logout") { fs.rmSync(path.join(process.env.GEMINI_HOME, "agy-signed-in"), { force: true }); answer({}); }
    else if (method === "session/new") signedIn() ? answer(setup("acp-" + process.pid + "-" + ++made)) : refuse(-32000, "Authentication required");
    else if (method === "session/load" || method === "session/resume") {
      for (const update of read("acp-replay.json", [])) send({ method: "session/update", params: { sessionId: params.sessionId, update, _meta: { isReplay: true } } });
      answer(setup(params.sessionId));
    }
    else if (method === "session/set_model" || method === "session/set_config_option" || method === "session/close") answer({});
    else if (method === "session/cancel") { const state = running.get(params.sessionId); if (state) { state.cancelled = true; state.stop?.(); } }
    else if (method === "session/prompt") {
      const text = params.prompt.filter((part) => part.type === "text").map((part) => part.text).join("\n");
      const scripts = read("acp-script.json", {});
      const steps = Object.entries(scripts).find(([key]) => text.includes(key))?.[1] ?? [{ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Done." } } }];
      play(params.sessionId, id, steps);
    }
    else if (id !== undefined) answer({});
  }
});
`;

/** Puts the stand-in `grok` and `agy` in `root`'s `.local/bin` (or `binDir`). */
export function standInAcp(root, names = ["grok", "agy"], binDir = path.join(root, ".local", "bin")) {
  fs.mkdirSync(binDir, { recursive: true });
  for (const name of names) {
    if (process.platform === "win32") {
      fs.writeFileSync(path.join(binDir, `${name}.cjs`), `(async () => {${PROGRAM}})()`);
      fs.writeFileSync(path.join(binDir, `${name}.cmd`), `@"${process.execPath}" "%~dp0${name}.cjs" %*\r\n`);
    } else {
      fs.writeFileSync(path.join(binDir, name), `#!${process.execPath}\n(async () => {${PROGRAM}})()`, { mode: 0o755 });
    }
  }
  return binDir;
}

/** Grok signed in, as `grok login` keeps it. */
export function seedGrokAuth(grokHome) {
  write(path.join(grokHome, "auth.json"), JSON.stringify({ "https://grok.example.test": { key: "grok-fixture-key" } }));
}

/** Antigravity's home in the box signed in with `agy`, with the models it listed. */
export function seedAgyAuth(home, { listed = true } = {}) {
  const profile = path.join(home, "agents", "antigravity", "profile");
  write(path.join(profile, "agy-signed-in"), "yes");
  write(path.join(profile, "antigravity-acp", "kuvo-sign-in.json"), JSON.stringify({ id: "agy_fixture", at: 1 }));
  if (listed) {
    // (The sign-in's fingerprint, as both boxes keep it.)
    const credential = createHash("sha256").update("agy_fixture").digest("hex").slice(0, 16);
    write(path.join(profile, "kuvo-models.json"), JSON.stringify({ credential, current: "acp-test", models: [{ value: "acp-test", name: "ACP Test" }, { value: "acp-claude", name: "Claude Fixture" }] }));
  }
}

/**
 * A conversation on `provider`'s agent, as the box keeps it: its header and, with `messages`, its
 * rows (`{ type: "user", text }`, `{ type: "assistant", content }`; IDs are given where missing).
 */
export function seedAcpSession(home, id, provider, { directory = home, model = "acp-test", variant, permissions, state, title = "ACP fixture", parent, metadata, updated = 1, messages, agent = "build" } = {}) {
  const folder = path.join(home, "agents/sessions", id);
  write(path.join(folder, "session.json"), JSON.stringify({
    id, title, projectID: "fixture", agent, ...(permissions && { permissions }),
    ...(parent && { parentID: parent }), ...(metadata && { metadata }),
    location: { directory }, model: { providerID: provider, id: model, ...(variant && { variant }) },
    time: { created: 1, updated }, cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
  }));
  if (messages) {
    // (An agent's reply names the model and agent it ran on.)
    const rows = messages.map((row, at) => ({
      id: `msg_${id.replace(/^ses_/, "")}${String(at).padStart(4, "0")}`, time: { created: at + 1 },
      ...(row.type === "assistant" && { agent: "build", model: { providerID: provider, id: model } }), ...row,
    }));
    write(path.join(folder, "messages.json"), JSON.stringify(rows));
  }
  if (state) write(path.join(home, "agents/sessions", id, "agent.json"), JSON.stringify({ threads: { [provider]: { state } } }));
}

/**
 * The scripted Grok on a computer whose `env` and `home` a harness of its own makes (`bench.mjs`):
 * what to add to its environment, and its script, gates and what it heard.
 */
export function agentsOn(home, env) {
  const root = env.HOME;
  const bin = standInAcp(root, ["grok"]);
  const grokHome = path.join(root, ".grok");
  seedGrokAuth(grokHome);
  const system = process.platform === "win32" ? path.join(env.SystemRoot, "System32") : "/usr/bin:/bin";
  const heardFile = path.join(root, "acp-heard.jsonl");
  const scripts = {};
  const agents = {
    root,
    env: { GROK_HOME: grokHome, KUVO_ACP_ROOT: root, KUVO_TEST_PATH_ONLY: "1", KUVO_TEST_PRICE_FEEDS: "off", PATH: [bin, system].join(path.delimiter) },
    /** A conversation on Grok, in `directory`. */
    seed: (id, options) => seedAcpSession(home, id, providers.grok, options),
    script(text, steps) {
      scripts[text] = steps;
      write(path.join(root, "acp-script.json"), JSON.stringify(scripts));
    },
    go: (name) => write(path.join(root, name), ""),
    heard: () => fs.existsSync(heardFile)
      ? fs.readFileSync(heardFile, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)) : [],
    sent: (method) => agents.heard().filter((line) => line.message?.method === method).map((line) => line.message),
    /** The words of each prompt the agent was handed, in order. */
    prompts: () => agents.sent("session/prompt").map((message) => message.params.prompt.filter((part) => part.type === "text").map((part) => part.text).join("\n")),
    started: () => agents.heard().filter((line) => line.argv && line.argv[0] !== "models"),
    /** The conversation's rows as `listener` shows them, oldest first. */
    rows: (listener, id) => transcript(listener, `kuvo/agents/api/session/${id}`),
    /** The IDs of its messages from the user, in order. */
    async users(listener, id) {
      return (await agents.rows(listener, id)).filter((row) => row.type === "user").map((row) => row.id);
    },
  };
  return agents;
}

/** One resident (and, for the reference, its bridge) on the scripted ACP agents. */
export async function scriptedAcp({ names = ["grok", "agy"], grok = true, agy = true, grokNative = false } = {}) {
  const binary = boxBinary();
  const root = scratch();
  const home = path.join(root, "box");
  const relay = await localRelay();
  privateHome(home);
  const base = isolatedEnv(root);
  const bin = grokNative
    ? [
        ...(names.includes("grok") ? [standInAcp(base.HOME, ["grok"], path.join(base.HOME, ".grok", "bin"))] : []),
        ...(names.filter((each) => each !== "grok").length ? [standInAcp(base.HOME, names.filter((each) => each !== "grok"))] : []),
      ].join(path.delimiter)
    : standInAcp(base.HOME, names);
  const grokHome = path.join(base.HOME, ".grok");
  if (grok) seedGrokAuth(grokHome);
  if (agy) seedAgyAuth(home);
  const system = process.platform === "win32" ? path.join(base.SystemRoot, "System32") : "/usr/bin:/bin";
  const env = { ...base, CODEX_HOME: path.join(base.HOME, ".codex"), GROK_HOME: grokHome, CLAUDE_CONFIG_DIR: path.join(base.HOME, ".claude"), KUVO_TEST_PRICE_FEEDS: "off", KUVO_TEST_PATH_ONLY: "1", KUVO_SANDBOX: "off", KUVO_ACP_ROOT: base.HOME, PATH: [bin, system].join(path.delimiter) };
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "box", "opencode.json"), "{}\n");
  const engine = binary.typescript ? await settingsEngine(home) : undefined;
  const heardFile = path.join(base.HOME, "acp-heard.jsonl");
  const scripts = {};
  let bridge, resident, listener;
  const acp = {
    binary, home, env, root: base.HOME, grokHome,
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
      await acp.stop();
      await stopFixtureHolders(home);
      await relay.close();
      await engine?.close();
      const alive = (pid) => {
        try { process.kill(pid, 0); return true; } catch { return false; }
      };
      const started = [...new Set(acp.heard().filter((line) => line.argv).map((line) => line.pid))];
      const deadline = Date.now() + 10000;
      while (started.some(alive) && Date.now() < deadline) await new Promise((done) => setTimeout(done, 50));
      const left = started.filter(alive);
      if (left.length) throw new Error(`An agent the box started was left running after it stopped: ${left.join(", ")}`);
      fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
    },
    log: () => [resident?.log(), bridge?.log()].filter(Boolean).join("\n"),
    script(text, steps) {
      scripts[text] = steps;
      write(path.join(base.HOME, "acp-script.json"), JSON.stringify(scripts));
    },
    go: (name) => write(path.join(base.HOME, name), ""),
    heard: () => fs.existsSync(heardFile)
      ? fs.readFileSync(heardFile, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)) : [],
    /** The agent processes started, by the name they ran as. */
    started: (name) => acp.heard().filter((line) => line.name === name && line.argv && line.argv[0] !== "models"),
    sent: (method) => acp.heard().filter((line) => line.message?.method === method).map((line) => line.message),
    seed: (id, provider, options) => seedAcpSession(home, id, provider, options),
    route: (id) => `kuvo/agents/api/session/${id}`,
    async prompt(id, text) {
      const answer = await request(listener, "POST", acp.route(id) + "/prompt", {
        id: `msg_${id.replace(/^ses_/, "")}_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, text,
      }, headers, 45000);
      assert.equal(answer.status, 200, answer.text);
    },
    rows: (id) => transcript(listener, acp.route(id)),
    async idles(id, count = 1, why = "The turn never ended.") {
      let rows = [];
      return until(async () => {
        rows = await acp.rows(id);
        return rows.filter((row) => row.type === "idle").length >= count && rows;
      }, () => [why, JSON.stringify(rows.slice(-4)), JSON.stringify(acp.heard().slice(-6)), acp.log()].join("\n"), 20000);
    },
    async interrupt(id) {
      const answer = await request(listener, "POST", acp.route(id) + "/interrupt", {}, headers, 20000);
      assert.equal(answer.status, 200, answer.text);
    },
    async permissions(id) {
      const answer = await request(listener, "GET", acp.route(id) + "/permission", undefined, headers);
      assert.equal(answer.status, 200, answer.text);
      return answer.json.data ?? answer.json;
    },
    async replyPermission(id, ask, decision) {
      const answer = await request(listener, "POST", `${acp.route(id)}/permission/${ask}/reply`, { decision }, headers);
      assert.ok(answer.status < 300, answer.text);
    },
    async forms(id) {
      const answer = await request(listener, "GET", acp.route(id) + "/form", undefined, headers);
      assert.equal(answer.status, 200, answer.text);
      return answer.json.data ?? answer.json;
    },
    async replyForm(id, form, answers) {
      return request(listener, "POST", `${acp.route(id)}/form/${form}/reply`, { answer: answers }, headers);
    },
    /** The box's answer to the stand-in's ask, by its method. */
    answered: (method) => until(() => {
      const heard = acp.heard();
      const asked = heard.filter((line) => line.asked?.method === method).at(-1);
      return asked && heard.find((line) => line.pid === asked.pid && line.answer?.id === asked.asked.id)?.answer;
    }, `The box never answered ${method}.`),
    async work() {
      const answer = await request(listener, "GET", "kuvo/work/background", undefined, headers, 20000);
      assert.equal(answer.status, 200, answer.text);
      return answer.json;
    },
    saved: (id) => readJSON(path.join(home, "agents/sessions", id, "session.json")),
  };
  return acp;
}
