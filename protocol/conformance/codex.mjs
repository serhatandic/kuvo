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
const nativeAuth = path.join(home, "auth.json");
function signIn(account) {
  fs.writeFileSync(auth, JSON.stringify(account));
  const claims = Buffer.from(JSON.stringify({ email: account.email, "https://api.openai.com/auth": { chatgpt_plan_type: account.planType, chatgpt_account_id: account.email } })).toString("base64url");
  const token = "e30." + claims + ".fixture";
  fs.writeFileSync(nativeAuth, JSON.stringify(account.type === "apiKey" ? {auth_mode:"apikey",OPENAI_API_KEY:"sk-fixture"} : {auth_mode:"chatgpt",tokens:{access_token:token,id_token:token,account_id:account.email}}));
}
const heard = (line) => fs.appendFileSync(path.join(root, "codex-heard.jsonl"), JSON.stringify({ pid: process.pid, ...line }) + "\n");
const args = process.argv.slice(2);
if (args[0] === "logout") {
  fs.rmSync(auth, { force: true });
  fs.rmSync(nativeAuth, { force: true });
  heard({ logout: true, home });
  process.exit(0);
}
// Its version (\`codex-version\` in its root), and its own updater: \`update\` waits while
// \`codex-update-waits\` is there until \`codex-update-go\` is, then fails (\`codex-update-fails\`) or
// moves its version to \`codex-update-to\`.
const own = (name) => path.join(root, name);
if (args[0] === "--version") {
  heard({ version: true });
  process.stdout.write("codex-cli " + (fs.existsSync(own("codex-version")) ? fs.readFileSync(own("codex-version"), "utf8").trim() : "0.1.0") + "\n");
  process.exit(0);
}
if (args[0] === "update") {
  heard({ update: true });
  const pause = new Int32Array(new SharedArrayBuffer(4));
  while (fs.existsSync(own("codex-update-waits")) && !fs.existsSync(own("codex-update-go"))) Atomics.wait(pause, 0, 0, 50);
  if (fs.existsSync(own("codex-update-fails"))) {
    process.stderr.write("no network\n");
    process.exit(1);
  }
  if (fs.existsSync(own("codex-update-to"))) {
    fs.writeFileSync(own("codex-version"), fs.readFileSync(own("codex-update-to"), "utf8").trim());
    const executable=path.join(root,".local","bin",process.platform==="win32"?"codex.cmd":"codex");
    fs.appendFileSync(executable, process.platform==="win32"?"\r\n":"\n");
  }
  process.exit(0);
}
if (args[0] !== "app-server") process.exit(2);
const startedVersion = fs.existsSync(own("codex-version")) ? fs.readFileSync(own("codex-version"), "utf8").trim() : "0.1.0";
heard({ argv: args, script: process.argv[1], startedVersion, env: { CODEX_HOME: process.env.CODEX_HOME ?? null, originator: process.env.CODEX_INTERNAL_ORIGINATOR_OVERRIDE ?? null } });
const read = (file, fallback) => { try { return JSON.parse(fs.readFileSync(path.join(root, file), "utf8")); } catch { return fallback; } };
const send = (message) => process.stdout.write(JSON.stringify(message) + "\n");
const waiting = new Map();
const threads = new Map();
/** What each thread left running in background terminals, as a script step set it. */
const terminals = new Map();
let asked = 0, turns = 0, made = 0, sleeps = 0;
let running;
/** How each thread's turn takes what's steered into it, as its script's steps set it. */
const quirks = new Map();
const quirk = (thread) => {
  if (!quirks.has(thread)) quirks.set(thread, { steers: 0 });
  return quirks.get(thread);
};
const sleepItem = () => ({ type: "sleep", id: "sleep" + ++sleeps, durationMs: 43200000 });
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
    const q = quirk(threadId);
    // {refuses}: what's steered is turned down from here on; {refusesNext}: the next one only;
    // {refusesAsItSleeps}: the next one, saying a sleep's started in the same breath.
    if (step.refuses) { q.refusing = true; continue; }
    // {losesSteerAck}: takes the next thing steered without saying so (it's in its history).
    if (step.losesSteerAck) { q.losingAck = true; continue; }
    if (step.refusesNext) { q.refusingNext = true; continue; }
    if (step.refusesAsItSleeps) { q.refusingAsItSleeps = true; continue; }
    // {endsAsTold}: takes what's steered and says its turn is over in the same breath;
    // {takesAsItSleeps}: takes it as a sleep starts, which ends, and so does its turn.
    if (step.endsAsTold) { q.endsAsTold = true; return; }
    if (step.takesAsItSleeps) { q.takesAsItSleeps = true; return; }
    // {untilTold}: works until something's steered in.
    if (step.untilTold) {
      if (!q.steers) await new Promise((resolve) => (q.wake = resolve));
      q.steers = 0;
      continue;
    }
    // {sleeps}: sleeps for hours until something new is steered in (one before it doesn't count).
    if (step.sleeps) {
      const sleep = q.sleptInBreath ?? sleepItem();
      if (!q.sleptInBreath) send({ method: "item/started", params: fill({ item: sleep }) });
      q.sleptInBreath = undefined;
      await new Promise((resolve) => (q.asleep = resolve));
      if (running?.turnId !== turnId) return;
      send({ method: "item/completed", params: fill({ item: sleep }) });
      continue;
    }
    // {sleepsAgainOnceRefused}: sleeps until it turns one down; a moment later that sleep ends and
    // another starts in the same breath, until told.
    if (step.sleepsAgainOnceRefused) {
      const first = q.sleptInBreath ?? sleepItem();
      if (!q.sleptInBreath) send({ method: "item/started", params: fill({ item: first }) });
      q.sleptInBreath = undefined;
      await new Promise((resolve) => (q.refusedOne = resolve));
      await new Promise((resolve) => setTimeout(resolve, 100));
      const next = sleepItem();
      process.stdout.write(JSON.stringify({ method: "item/completed", params: fill({ item: first }) }) + "\n" + JSON.stringify({ method: "item/started", params: fill({ item: next }) }) + "\n");
      await new Promise((resolve) => (q.asleep = resolve));
      if (running?.turnId !== turnId) return;
      send({ method: "item/completed", params: fill({ item: next }) });
      continue;
    }
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
    else if (method === "account/rateLimits/read") {
      const own = path.join(home, "codex-limits.json");
      if (fs.existsSync(path.join(home, "codex-limits-refused"))) refuse(-32000, "Fixture maker unavailable");
      else answer(fs.existsSync(own) ? JSON.parse(fs.readFileSync(own,"utf8")) : read("codex-limits.json", {}));
    }
    else if (method === "account/rateLimitResetCredit/consume") {
      const journal=read("../box/agents/limits-resets.json", {});
      const kept=journal.spends?.find(s=>s.request===params.idempotencyKey && s.target===params.creditId && s.state==="sending");
      if (!kept) {refuse(-32000,"Reset request was not journaled."); continue;}
      const prior=read("codex-consumed.json", []);
      const outcome=read("codex-reset-answer.json", {outcome:"reset"});
      if (prior.includes(params.idempotencyKey)) answer({outcome:"alreadyRedeemed"});
      else {
        prior.push(params.idempotencyKey);
        fs.writeFileSync(path.join(root,"codex-consumed.json"), JSON.stringify(prior));
        answer(outcome);
      }
    }
    else if (method === "account/read") {
      const saved = fs.existsSync(auth) ? JSON.parse(fs.readFileSync(auth, "utf8")) : null;
      answer({ account: saved, requiresOpenaiAuth: true });
    } else if (method === "account/login/start") {
      fs.mkdirSync(home, { recursive: true });
      if (params.type === "apiKey") {
        signIn({ type: "apiKey" });
        answer({ type: "apiKey" });
        continue;
      }
      const loginId = "login-" + Date.now();
      if (params.type === "chatgptDeviceCode") answer({ type: "chatgptDeviceCode", loginId, verificationUrl: "https://auth.example.test/device", userCode: "ABCD-1234" });
      else answer({ type: "chatgpt", loginId, authUrl: "https://auth.example.test/codex" });
      wait("codex-login-complete").then(() => {
        signIn({ type: "chatgpt", email: "you@example.test", planType: "plus" });
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
      running = { threadId: params.threadId, turnId, input: params.input, client: params.clientUserMessageId };
      quirk(params.threadId).steers = 0;
      answer({ turn: { id: turnId, status: "inProgress" } });
      const script = read("codex-script.json", {});
      const key = Object.keys(script).find((each) => text.includes(each));
      void play(params.threadId, turnId, key ? script[key] : [["item/completed", { item: { type: "agentMessage", id: "a" + turnId, text: "Done." } }]]);
    } else if (method === "turn/steer") {
      const q = quirk(params.threadId);
      const inTurn = (more) => ({ threadId: params.threadId, turnId: running.turnId, ...more });
      const took = () => threads.get(params.threadId)?.find((turn) => turn.id === running.turnId)?.items.push({ type: "userMessage", id: "s" + Date.now(), clientId: params.clientUserMessageId, content: params.input });
      if (!running || running.turnId !== params.expectedTurnId || fs.existsSync(path.join(root, "codex-refuses-steers"))) {
        refuse(-32600, "no active turn to steer");
        q.refusedOne?.();
        q.refusedOne = undefined;
        continue;
      }
      if (q.losingAck) {
        q.losingAck = false;
        // (Kept on disk: a next app-server's history has it too.)
        fs.writeFileSync(path.join(root, "codex-steered-" + params.threadId + ".json"), JSON.stringify({
          id: running.turnId, status: "completed",
          items: [{ type: "userMessage", id: "first", clientId: running.client, content: running.input },
            { type: "userMessage", id: "steer", clientId: params.clientUserMessageId, content: params.input }],
        }));
        q.steers++;
        q.wake?.();
        q.wake = undefined;
        continue;
      }
      if (q.endsAsTold) {
        q.endsAsTold = false;
        took();
        const ended = running;
        running = undefined;
        process.stdout.write(JSON.stringify({ id, result: { turnId: ended.turnId } }) + "\n" + JSON.stringify({ method: "turn/completed", params: { threadId: params.threadId, turn: { id: ended.turnId, status: "completed" } } }) + "\n");
        continue;
      }
      if (q.takesAsItSleeps) {
        q.takesAsItSleeps = false;
        took();
        const ended = running;
        running = undefined;
        const fill = (more) => ({ threadId: params.threadId, turnId: ended.turnId, ...more });
        const sleep = sleepItem();
        process.stdout.write([
          { method: "item/started", params: fill({ item: sleep }) },
          { id, result: { turnId: ended.turnId } },
          { method: "item/started", params: fill({ item: { type: "userMessage", id: "steer" + q.steers++, content: params.input } }) },
          { method: "item/completed", params: fill({ item: sleep }) },
          { method: "turn/completed", params: { threadId: params.threadId, turn: { id: ended.turnId, status: "completed" } } },
        ].map((each) => JSON.stringify(each) + "\n").join(""));
        continue;
      }
      if (q.refusingAsItSleeps) {
        q.refusingAsItSleeps = false;
        q.sleptInBreath = sleepItem();
        process.stdout.write(JSON.stringify({ id, error: { code: -32600, message: "not now" } }) + "\n" + JSON.stringify({ method: "item/started", params: inTurn({ item: q.sleptInBreath }) }) + "\n");
        q.steers++;
        q.wake?.();
        q.wake = undefined;
        continue;
      }
      const refused = q.refusing || q.refusingNext;
      q.refusingNext = false;
      if (refused) {
        refuse(-32600, "no active turn to steer");
        q.refusedOne?.();
        q.refusedOne = undefined;
      } else {
        took();
        answer({ turnId: running.turnId });
        send({ method: "item/started", params: inTurn({ item: { type: "userMessage", id: "s" + Date.now(), content: params.input } }) });
        q.asleep?.();
        q.asleep = undefined;
      }
      q.steers++;
      q.wake?.();
      q.wake = undefined;
      continue;
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
      // (Held until stopped, for a thread the test names.)
      if (fs.existsSync(path.join(root, "codex-compact-holds-" + thread))) { running = { threadId: thread, turnId: "c1" }; continue; }
      send({ method: "item/completed", params: { threadId: thread, turnId: "c1", item: { type: "contextCompaction", id: "cc1" } } });
      send({ method: "turn/completed", params: { threadId: thread, turn: { id: "c1", status: "completed" } } });
    } else if (method === "thread/backgroundTerminals/list") {
      const kept = terminals.get(params.threadId);
      const there = (file) => file && fs.existsSync(path.join(root, file));
      if (kept?.failsUntil && !there(kept.failsUntil)) refuse(-32603, "background terminals unavailable");
      else answer({ data: kept && !there(kept.endsWith) ? kept.terminals : [], nextCursor: null });
    } else if (method === "thread/backgroundTerminals/terminate" || method === "thread/backgroundTerminals/clean") {
      // Ended: one (by its process), or all its thread left running.
      const kept = terminals.get(params.threadId);
      if (kept) kept.terminals = method.endsWith("clean") ? [] : kept.terminals.filter((terminal) => terminal.processId !== params.processId);
      answer({});
    } else if (method === "thread/read") {
      const kept = threads.get(params.threadId);
      const steered = path.join(root, "codex-steered-" + params.threadId + ".json");
      if (fs.existsSync(path.join(root, "codex-withhold-history"))) refuse(-32603, "history unavailable");
      else if (fs.existsSync(steered)) answer({ thread: { id: params.threadId, turns: [JSON.parse(fs.readFileSync(steered, "utf8"))] } });
      else if (!kept) refuse(-32603, "history unavailable");
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
    seedCodexAuth(path.join(root, ".codex"), account);
  }
  return bin;
}

// Seed both the native credential file and the scripted RPC's account answer, without real tokens.
export function seedCodexAuth(home, account) {
  write(path.join(home, "fixture-auth.json"), JSON.stringify(account));
  const claims = Buffer.from(JSON.stringify({ email: account.email, "https://api.openai.com/auth": { chatgpt_plan_type: account.planType, chatgpt_account_id: account.email } })).toString("base64url");
  const token = `e30.${claims}.fixture`;
  write(path.join(home, "auth.json"), JSON.stringify(account.type === "apiKey" ? {auth_mode:"apikey",OPENAI_API_KEY:"sk-fixture"} : {auth_mode:"chatgpt",tokens:{access_token:token,id_token:token,account_id:account.email}}));
}

export function seedCodexSession(home, id, { directory = home, model = "gpt-test", permissions, agent = "build", state, metadata } = {}) {
  write(path.join(home, "agents/sessions", id, "session.json"), JSON.stringify({
    id, title: "Codex fixture", projectID: "fixture", agent, ...(permissions && { permissions }), ...(metadata && { metadata }),
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
  const env = { ...base, CODEX_HOME: path.join(base.HOME, ".codex"), GROK_HOME: path.join(base.HOME, ".grok"), CLAUDE_CONFIG_DIR: path.join(base.HOME, ".claude"), KUVO_TEST_PRICE_FEEDS: "off", KUVO_TEST_PATH_ONLY: "1", KUVO_SANDBOX: "off", KUVO_CODEX_ROOT: base.HOME, PATH: [bin, system].join(path.delimiter) };
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
      // Every Codex the boxes started has ended (each is gone with its holder): a folder one still
      // runs in can't be deleted on Windows.
      const alive = (pid) => {
        try { process.kill(pid, 0); return true; } catch { return false; }
      };
      const started = [...new Set(codex.heard().map((line) => line.pid))];
      const deadline = Date.now() + 10000;
      while (started.some(alive) && Date.now() < deadline) await new Promise((done) => setTimeout(done, 50));
      const left = started.filter(alive);
      if (left.length) {
        const records = codex.heard().filter((line) => left.includes(line.pid));
        throw new Error(`A Codex the box started was left running after it stopped: ${JSON.stringify(records.slice(0, 6))}`);
      }
      const hold = path.join(home, "run", "agents", "hold");
      const lifecycle = fs.existsSync(path.join(hold, "lifecycle.log")) ? fs.readFileSync(path.join(hold, "lifecycle.log"), "utf8") : "";
      const records = fs.existsSync(hold) ? fs.readdirSync(hold) : [];
      try {
        fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
      } catch (error) {
        throw new Error(`${error.message}\nheld: ${records.join(" ")}\nlifecycle:\n${lifecycle}\nleft: ${fs.existsSync(hold) ? fs.readdirSync(hold).join(" ") : "-"}`, { cause: error });
      }
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
    /** Stop on what the session left running besides its turn. */
    async stopBackground(id) {
      const answer = await request(listener, "DELETE", codex.route(id) + "/background", undefined, headers, 20000);
      assert.equal(answer.status, 200, answer.text);
    },
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
