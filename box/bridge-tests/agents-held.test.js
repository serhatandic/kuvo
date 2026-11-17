// A machine's coding agents' bridge killed (SIGKILL: a crash) while a Codex turn waits on your
// approval: Codex, under its holder, lives on; the bridge its launch starts again takes the turn up,
// shows the approval again, once; you answer it once, and the turn ends as it would have, one turn,
// with what Codex said before and after the crash. Then the box is stopped (`kuvo-start stop`), and
// nothing it held runs on. The real bridge (box/dist/kuvo-agents.js, on Bun), its real holders and
// keeper, and a stand-in `codex app-server` (it speaks Codex's JSON-RPC; Codex itself was checked
// the same way in spike S2, .claude/notes/update-handover.md).
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

const freePort = () => new Promise((resolve, reject) => {
  const server = net.createServer();
  server.on("error", reject);
  server.listen(0, "127.0.0.1", () => {
    const { port } = server.address();
    server.close(() => resolve(port));
  });
});

async function until(wanted, what, within = 60_000) {
  for (let waited = 0; waited < within; waited += 20) {
    const found = await wanted();
    if (found) return found;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert.fail(`Waited too long for ${what}`);
}

const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };

/** A machine's box in folders of the test's own (`~/.kuvo-code` in a home of its own, for its
 * keeper), its bridge kept running as its launch keeps it. */
async function machine() {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kh-")));
  removeAtEnd(root);
  const home = path.join(root, ".kuvo-code");
  for (const folder of ["tmp", "work", "agents", "run", "bin", "home", "codex-home", "fake"]) fs.mkdirSync(path.join(home, folder), { recursive: true });
  fs.copyFileSync(built, path.join(home, "bin", "kuvo-agents.js"));
  fs.writeFileSync(path.join(home, "run", "agents.log"), "");
  const fake = path.join(home, "fake");
  fs.writeFileSync(path.join(fake, "app-server.js"), CODEX);
  fs.writeFileSync(path.join(fake, "codex"), `#!/bin/sh\nexport BUN_BE_BUN=1\n[ "$1" = "--version" ] && { echo "codex-cli 0.169.0"; exit 0; }\nexec "${bun}" "${path.join(fake, "app-server.js")}" "${fake}"\n`, { mode: 0o755 });
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
  const loop = 'while kill -0 "$1" 2>/dev/null; do "$2" --config=/dev/null --no-env-file "$3" >>"$4" 2>&1 </dev/null & echo $! >"$5"; wait $!; sleep 1; done';
  const launch = spawn("sh", ["-c", loop, "kuvo-agents", String(process.pid), bun, path.join(home, "bin", "kuvo-agents.js"), path.join(home, "run", "agents.log"), path.join(home, "run", "agents.pid")], { cwd: path.join(home, "work"), env, stdio: "ignore", detached: true });
  atEnd(() => new Promise(resolve => {
    if (launch.exitCode !== null) return resolve();
    launch.on("exit", () => resolve());
    try { process.kill(-launch.pid, "SIGTERM"); } catch { resolve(); }
  }));

  const auth = { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json", "x-opencode-directory": encodeURIComponent(path.join(home, "work")) };
  const api = async (method, route, body) => {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/${route}`, { method, headers: auth, body: body ? JSON.stringify(body) : undefined });
      const text = await response.text();
      return { status: response.status, json: text ? JSON.parse(text) : null };
    } catch {
      return { status: 0, json: null };
    }
  };
  const record = () => { try { return JSON.parse(fs.readFileSync(path.join(home, "run", "bridge.json"), "utf8")); } catch { return null; } };
  const up = besides => until(async () => {
    const said = record();
    if (!said?.pid || said.pid === besides) return null;
    const info = await api("GET", "api/info");
    return info.status === 200 && info.json.pid === said.pid ? said.pid : null;
  }, besides ? "the bridge started again to answer" : "the bridge to answer");
  const log = () => { try { return fs.readFileSync(path.join(fake, "log"), "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); } catch { return []; } };
  const held = () => (fs.existsSync(holders) ? fs.readdirSync(holders) : []).filter(name => name.endsWith(".json")).map(name => JSON.parse(fs.readFileSync(path.join(holders, name), "utf8")));
  return { root, home, api, up, log, held, holders };
}

const textOf = messages => messages.filter(each => each.type === "assistant").flatMap(each => each.content ?? []).filter(block => block.type === "text").map(block => block.text);

test("a crash while Codex waits on your approval: Codex lives on, the next bridge asks once, and the turn ends as one; stopping the box stops it", { skip: bun ? false : NO_BUN }, async () => {
  const box = await machine();
  const first = await box.up();
  const made = await box.api("POST", "api/session", { title: "Held", location: { directory: path.join(box.home, "work") }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  assert.equal((await box.api("POST", `api/session/${session}/prompt`, { id: "msg_heldcodex00000001", delivery: "steer", text: "run it" })).status, 200);
  const asked = await until(() => box.log().find(each => each.asked), "Codex to ask");
  await until(async () => (await box.api("GET", `api/session/${session}/permission`)).json?.data?.length === 1, "the approval to show");
  const [holder] = box.held();
  assert.equal(holder.child, asked.pid, "Codex runs under a holder");
  assert.equal(asked.parent, holder.pid);
  assert.equal(holder.meta.turns[Object.keys(holder.meta.turns)[0]].session, session, "the holder's record says which session's turn is under way");
  assert.equal(fs.statSync(box.holders).mode & 0o777, 0o700, "only you reach the holders");

  process.kill(first, "SIGKILL");
  const second = await box.up(first);
  assert.ok(alive(asked.pid), "Codex lived on through the crash");
  const asks = await until(async () => {
    const data = (await box.api("GET", `api/session/${session}/permission`)).json?.data;
    return data?.length ? data : null;
  }, "the approval to show again");
  assert.equal(asks.length, 1, "asked once");
  assert.equal((await box.api("POST", `api/session/${session}/permission/${asks[0].id}/reply`, { decision: "once" })).status, 200);
  const messages = await until(async () => {
    const page = (await box.api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the turn to end");
  assert.deepEqual(messages.filter(each => each.type === "user").map(each => each.text), ["run it"]);
  assert.deepEqual(textOf(messages), ["Asking first.", "Ran it."], "what Codex said before the crash and after, once each");
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"], "one turn, not cut");
  assert.deepEqual(box.log().filter(each => each.answered).map(each => each.answered), ["accept"], "answered once");
  assert.equal(box.log().filter(each => each.asked).length, 1, "the command was asked about once: the turn wasn't started again");
  assert.notEqual(second, first);

  // Stopping the box (Stop, and uninstall's `off`): the held Codex stops too, its holder with it.
  const stopped = spawnSync("sh", [keeper, "stop"], { env: { PATH: "/usr/bin:/bin", HOME: box.root }, encoding: "utf8", timeout: 60_000 });
  assert.equal(stopped.status, 0, stopped.stderr);
  assert.equal(alive(asked.pid), false, "nothing held runs after a stop");
  assert.equal(alive(holder.pid), false);
  assert.deepEqual(box.held(), []);
});
