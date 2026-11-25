// OpenCode conversations as the app reaches them (`kuvo/sessions`, its asks, and the messages under
// the prefix `location` names), on the pinned OpenCode with a labelled stand-in model. The reference
// box runs them on its engine; the Rust box on the OpenCode agent (`opencode acp`). Both must give
// the app the same: these cases assert only what it reads.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { STAND_IN, standInModel, standInSettings } from "./opencode-model.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const version = "2.2.6";
const headers = { "x-kuvo-from": "app" };
const skip = !pinned && "Set KUVO_CONFORMANCE_OPENCODE to the pinned OpenCode (used in place).";

describe("OpenCode conversations", { skip }, () => {
  const binary = boxBinary();
  let root, home, work, model, relay, box, listener, engine, worker;
  const script = [];

  before(async () => {
    if (skip) return;
    root = fs.realpathSync(scratch());
    home = path.join(root, "box");
    work = path.join(root, "work");
    privateHome(home);
    fs.mkdirSync(work, { recursive: true });
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    const program = path.join(home, "engine", version, process.platform === "win32" ? "opencode.exe" : "opencode");
    fs.mkdirSync(path.dirname(program), { recursive: true });
    fs.symlinkSync(fs.realpathSync(pinned), program);
    model = await standInModel(script);
    write(path.join(home, "config", "opencode", "opencode.json"), JSON.stringify(standInSettings(model.url)));
    write(path.join(home, "box", "opencode.json"), "{}\n");
    const env = { ...isolatedEnv(root), KUVO_SANDBOX: "off", OPENCODE_DISABLE_MODELS_FETCH: "1" };
    relay = await localRelay();
    if (binary.typescript) {
      // The reference's engine, as its box runs it: the same program, settings and data.
      const reserve = net.createServer();
      await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
      const port = reserve.address().port;
      await new Promise(resolve => reserve.close(resolve));
      const password = "scratch-engine";
      const dirs = Object.fromEntries(["data", "config", "state", "cache"].map(sub => [`XDG_${sub.toUpperCase()}_HOME`, path.join(home, sub)]));
      worker = launch(program, ["serve", "--port", String(port), "--hostname", "127.0.0.1"],
        { ...env, ...dirs, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password }, path.join(home, "workspace"));
      engine = { port, password };
      await until(async () => (await request(engine, "GET", "api/session?limit=1").catch(() => null))?.status === 200, "The reference's engine", 30000);
      write(path.join(home, "run", "engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version }));
    }
    box = startBox({ binary, home, env, relay });
    listener = await box.ready();
  });

  after(async () => {
    await box?.stop();
    await worker?.stop?.();
    await relay?.close();
    await model?.close();
    if (root) fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  });

  const call = (method, route, body, timeout = 30000) => request(listener, method, route, body, headers, timeout);

  /** A new conversation in the work folder on the stand-in, as the app starts one. */
  async function begin(fields = {}) {
    const started = await call("POST", "kuvo/sessions", { folder: work, model: STAND_IN, ...fields });
    assert.equal(started.status, 200, started.text);
    const id = started.json.info?.id ?? started.json.id;
    assert.match(id ?? "", /^ses_/, started.text);
    return id;
  }

  /** The prefix the app reads the conversation's messages under. */
  async function prefix(id) {
    const found = await call("GET", `kuvo/sessions/${id}/location`);
    assert.equal(found.status, 200, found.text);
    const where = found.json.where ?? found.json.place ?? found.json;
    return JSON.stringify(where).includes("agents") ? "kuvo/agents/" : "";
  }

  async function send(id, words, message = `msg_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`) {
    const sent = await call("POST", `kuvo/sessions/${id}/send`, { id: message, words, delivery: "steer" });
    assert.equal(sent.status, 200, sent.text);
  }

  async function messages(id) {
    const at = await prefix(id);
    const read = await call("GET", `${at}api/session/${id}/message?limit=50&order=asc`);
    assert.equal(read.status, 200, read.text);
    return read.json.data ?? read.json;
  }

  const said = rows => rows.filter(row => (row.info ?? row).role === "assistant")
    .flatMap(row => row.parts ?? []).filter(part => part.type === "text").map(part => part.text).join("");
  const tools = rows => rows.flatMap(row => row.parts ?? []).filter(part => part.type === "tool");

  async function answered(id, words) {
    let rows = [];
    await until(async () => {
      rows = await messages(id);
      return said(rows).includes(words);
    }, () => `It never said "${words}": ${JSON.stringify(rows).slice(-1500)}\n${box.log?.() ?? ""}`, 60000, 250);
    return rows;
  }

  test("a new conversation's turn streams its words", { timeout: 90000 }, async () => {
    script.push({ text: "Hello from the stand-in." });
    const id = await begin();
    await send(id, "Say hello.");
    const rows = await answered(id, "Hello from the stand-in.");
    const user = rows.find(row => (row.info ?? row).role === "user");
    assert.ok(user, "Its message isn't there.");
  });

  test("a command it asks to run waits on the app's asks; Allow Once runs it", { timeout: 90000 }, async () => {
    script.push({ tools: [{ name: "shell", args: { command: "echo made > made.txt", description: "Make a file" } }] }, { text: "Made it." });
    const id = await begin({ approval: "askFirst" });
    await send(id, "Make a file.");
    const ask = await until(async () => {
      const reply = await call("GET", `kuvo/sessions/${id}/asks`);
      assert.equal(reply.status, 200, reply.text);
      return reply.json.asks?.find(each => each.kind === "permission");
    }, "No permission ask for the command.", 60000, 250);
    const reply = await call("POST", `kuvo/sessions/${id}/asks/${encodeURIComponent(ask.id)}`, { answer: "once" });
    assert.ok(reply.status < 300, reply.text);
    const rows = await answered(id, "Made it.");
    assert.ok(fs.existsSync(path.join(work, "made.txt")), "The command didn't run.");
    assert.ok(tools(rows).some(part => part.state?.status === "completed"), JSON.stringify(tools(rows)));
  });
});
