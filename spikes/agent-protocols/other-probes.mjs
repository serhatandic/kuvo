import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";

const scratch = process.env.SCRATCH;
assert(scratch && process.env.HOME === join(scratch, "home"));
const agy = join(scratch, "bin/agy");
const cursor = join(scratch, "cursor/cursor-agent");
const work = process.cwd();
const deadline = 20_000;

function run(program, args, env = process.env) {
  const child = spawn(program, args, { env, stdio: ["pipe", "pipe", "pipe"] });
  const completion = once(child, "close");
  let stdout = "", stderr = "";
  child.stdout.on("data", chunk => stdout += chunk);
  child.stderr.on("data", chunk => stderr += chunk);
  return {
    child,
    async done(timeout = 15_000) {
      const timer = setTimeout(() => child.kill(), timeout);
      try { return { status: (await completion)[0], stdout, stderr }; }
      finally { clearTimeout(timer); }
    },
  };
}

function rpc(child, onNotification = () => {}) {
  const awaiting = new Map();
  let id = 2 ** 30;
  createInterface({ input: child.stdout }).on("line", line => {
    let response;
    try { response = JSON.parse(line); } catch { return; }
    if (response.id === undefined) onNotification(response);
    else awaiting.get(response.id)?.(response);
  });
  child.once("close", () => {
    for (const answer of awaiting.values()) answer({ error: "process exited" });
  });
  return (method, params, jsonrpc) => new Promise(resolve => {
    const request = ++id;
    const timer = setTimeout(() => awaiting.get(request)?.({ error: "timeout" }), deadline);
    awaiting.set(request, response => {
      clearTimeout(timer);
      awaiting.delete(request);
      resolve(response);
    });
    child.stdin.write(JSON.stringify({ ...jsonrpc && { jsonrpc: "2.0" }, method, params, id: request }) + "\n");
  });
}

async function antigravity(legacy = false) {
  const profile = process.env.GEMINI_HOME + (legacy ? "-legacy" : "");
  if (legacy) {
    mkdirSync(join(profile, "antigravity-acp"), { recursive: true });
    writeFileSync(join(profile, "antigravity-acp/acp_token.json"), JSON.stringify({
      access_token: "spike-legacy-placeholder", refresh_token: "spike-legacy-placeholder",
      expiry: "2099-01-01T00:00:00Z", token_type: "Bearer",
    }));
  }
  mkdirSync(join(profile, "antigravity-acp/tmp"), { recursive: true });
  writeFileSync(join(profile, "antigravity-acp/settings.json"), '{"auth":{"type":"oauth-personal"}}');
  const env = { ...process.env, GEMINI_HOME: profile, KUVO_AGY_PROGRAM: agy };
  delete env.AGY_ACP_FORCE_FILE_STORAGE; // Match the agy branch of antigravity.ts.
  const args = ["acp", "--gemini-dir", profile, "--app-data-dir", "antigravity-acp"];
  const launched = run(agy, args, env);
  const request = rpc(launched.child);
  try {
    const init = await request("initialize", {
      protocolVersion: 1, clientCapabilities: { fs: { readTextFile: true, writeTextFile: true }, terminal: false },
      clientInfo: { name: "kuvo-spike", version: "1" },
    }, true);
    const session = await request("session/new", { cwd: work, mcpServers: [] }, true);
    const auth = await request("authenticate", { methodId: "oauth-personal" }, true);
    assert.equal(init.result?.protocolVersion, 1);
    assert.deepEqual(init.result?.authMethods.map(method => method.id).sort(), [
      "agent-platform", "gemini-api-key", "oauth-business", "oauth-personal",
    ]);
    assert.equal(session.error?.code, -32000);
    assert.match(auth.error?.message ?? "", /agy acp --login/);
    return { init, session, auth };
  } finally {
    launched.child.stdin.end();
    await launched.done(2_000);
  }
}

async function antigravityLogin() {
  const profile = process.env.GEMINI_HOME;
  const launched = run(agy, ["acp", "--gemini-dir", profile, "--app-data-dir", "antigravity-acp", "--login"]);
  launched.child.stdin.end();
  const output = await launched.done();
  const said = output.stdout + output.stderr;
  // Avoid recording the browser URL or OAuth state.
  return { status: output.status, browserUrlPrinted: /https:\/\/accounts\.google\.com\//.test(said),
    codePrompt: /authorization code/i.test(said), stderr: output.stderr.replaceAll(/https?:\/\/\S+/g, "[URL]") };
}

async function cursorWorker() {
  // This is the worker that actually ships, materialized next to the pinned SDK as in cursor-worker.ts.
  const source = readFileSync(join(scratch, "cursor-worker.ts"), "utf8");
  const template = source.match(/const WORKER = (`[\s\S]*?\n`)\n/);
  assert(template);
  const worker = join(scratch, "sdk/kuvo-cursor-worker.mjs");
  writeFileSync(worker, Function("return " + template[1])());
  const launched = run(join(scratch, "bin/opencode"), [worker], {
    ...process.env,
    BUN_BE_BUN: "1", CURSOR_DATA_DIR: join(scratch, "cursor-sdk-data"),
    CUA_APP_SUPPORT_DIR: join(scratch, "cursor-cua"),
    CURSOR_SANDBOX_POLICY_DIR: join(scratch, "cursor-policies"),
  });
  const notifications = [];
  const request = rpc(launched.child, frame => notifications.push(frame));
  try {
    const options = {
      apiKey: "cursor-loopback-spike-placeholder",
      local: { cwd: work, settingSources: [], store: { root: join(scratch, "cursor-store") } },
    };
    const resume = await request("create", { options, resume: "missing-spike-agent" });
    const create = await request("create", { options });
    const missingSend = await request("send", { agent: "missing-spike-agent", run: 123, message: "spike", options: {} });
    const agent = create.result?.agentId;
    assert(agent, JSON.stringify(create));
    const send = await request("send", { agent, run: 124, message: "spike", options: { model: { id: "default" }, mode: "ask" } });
    const cancel = await request("cancel", { run: 124 });
    const close = await request("close", { agent });
    const resumed = await request("create", { options, resume: agent });
    assert.equal(resume.result?.agentId, null);
    assert.equal(resumed.result?.agentId, agent);
    assert.equal(missingSend.error?.code, -32000);
    assert.equal(send.error?.data?.name, "NetworkError");
    assert.deepEqual(cancel.result, {});
    assert.deepEqual(close.result, {});
    await request("close", { agent });
    return { resume, create, missingSend, send, cancel, close, resumed, notifications };
  } finally {
    launched.child.stdin.end();
    const end = await launched.done(2_000);
    if (end.stderr.trim()) console.error(end.stderr.trim());
  }
}

switch (process.argv[2]) {
  case "agy":
    console.log(JSON.stringify(await antigravity(), null, 2));
    break;
  case "agy-legacy":
    console.log(JSON.stringify(await antigravity(true), null, 2));
    break;
  case "agy-login":
    console.log(JSON.stringify(await antigravityLogin(), null, 2));
    break;
  case "cursor": {
    const output = await run(cursor, ["-p", "--output-format", "stream-json", "--stream-partial-output", "spike"]).done();
    assert.equal(output.status, 1);
    assert.equal(output.stdout, "");
    assert.match(output.stderr, /authentication required/i);
    console.log(JSON.stringify(output, null, 2));
    break;
  }
  case "worker":
    console.log(JSON.stringify(await cursorWorker(), null, 2));
    break;
  default:
    throw new Error("expected agy, agy-legacy, agy-login, cursor, or worker");
}
