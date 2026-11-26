// OpenCode conversations as the app reaches them (`kuvo/sessions`, its asks, and the messages under
// the prefix `location` names), on the pinned OpenCode with a labelled stand-in model. The reference
// box runs them on its engine; the Rust box on the OpenCode agent (`opencode acp`). Both must give
// the app the same: these cases assert only what it reads.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { after, before, beforeEach, describe, test } from "node:test";
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
      for (const folder of [...Object.values(dirs), path.join(home, "workspace")]) fs.mkdirSync(folder, { recursive: true });
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

  // Each case plays its own steps only.
  beforeEach(() => { script.length = 0; });

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

  // (The engine's rows are `{ info, parts }`, the agents' `{ type, content }`: the app reads both.)
  const role = row => row.info?.role ?? row.role ?? row.type;
  const parts = row => row.parts ?? row.content ?? [];
  const said = rows => rows.filter(row => role(row) === "assistant")
    .flatMap(parts).filter(part => part.type === "text").map(part => part.text).join("");
  const tools = rows => rows.flatMap(parts).filter(part => part.type === "tool");

  async function answered(id, words) {
    let rows = [], asks;
    await until(async () => {
      rows = await messages(id);
      asks = (await call("GET", `kuvo/sessions/${id}/asks`)).json?.asks;
      return said(rows).includes(words);
    }, () => `It never said "${words}": ${JSON.stringify(rows).slice(-1500)}\nWaiting on: ${JSON.stringify(asks)}\nThe model was asked ${model.requests.length} times.\n${box.log?.() ?? ""}`, 60000, 250);
    return rows;
  }

  test("a new conversation's turn streams its words", { timeout: 90000 }, async () => {
    script.push({ text: "Hello from the stand-in." });
    const id = await begin();
    await send(id, "Say hello.");
    const rows = await answered(id, "Hello from the stand-in.");
    const user = rows.find(row => role(row) === "user");
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

  /** The stand-in's last request that came from the conversation (not a title's). */
  const lastTurn = () => model.requests.filter(asked => asked.tools?.length).at(-1);
  const lastUser = asked => asked.messages.filter(message => message.role === "user").at(-1);
  const words = message => typeof message.content === "string" ? message.content
    : (message.content ?? []).map(part => part.text ?? "").join("");

  test("its question is the app's card; the answer chosen there is what it hears", { timeout: 90000 }, async () => {
    script.push(
      { tools: [{ name: "question", args: { questions: [{ question: "Which size?", header: "Size", options: [{ label: "Small", description: "A little" }, { label: "Large", description: "A lot" }] }] } }] },
      messages => ({ text: `You chose ${JSON.stringify(messages.at(-1)?.content ?? "").includes("Large") ? "Large" : "nothing"}.` }),
    );
    const id = await begin();
    await send(id, "Ask me a size.");
    const ask = await until(async () => (await call("GET", `kuvo/sessions/${id}/asks`)).json?.asks?.find(each => each.kind === "question"),
      "No card for its question.", 60000, 250);
    assert.equal(ask.questions[0].question, "Which size?");
    assert.deepEqual(ask.questions[0].options.map(option => option.label), ["Small", "Large"]);
    const reply = await call("POST", `kuvo/sessions/${id}/asks/${encodeURIComponent(ask.id)}`, { answers: { [ask.questions[0].key]: "Large" } });
    assert.ok(reply.status < 300, reply.text);
    await answered(id, "You chose Large.");
  });

  test("Stop ends the turn where it is, and the next message is answered", { timeout: 90000 }, async () => {
    script.push({ hang: true }, { text: "Back again." });
    const id = await begin();
    const stopped = model.stopped;
    await send(id, "Take your time.");
    await until(() => lastTurn() && words(lastUser(lastTurn())).includes("Take your time."), "It never asked the model.", 60000, 100);
    const stop = await call("POST", `kuvo/sessions/${id}/stop`, {});
    assert.ok(stop.status < 300, stop.text);
    await until(() => model.stopped > stopped, "The model's request was never stopped.", 30000, 100);
    await send(id, "Carry on.");
    await answered(id, "Back again.");
  });

  test("a message sent while it works reaches it before it's done", { timeout: 90000 }, async () => {
    script.push({ wait: "first", text: "First done." }, messages => ({
      text: JSON.stringify(messages).includes("Also this.") ? "Heard both." : "Heard one.",
    }));
    const id = await begin();
    await send(id, "Start.");
    await until(() => lastTurn() && words(lastUser(lastTurn())).includes("Start."), "It never asked the model.", 60000, 100);
    await send(id, "Also this.");
    model.open("first");
    await answered(id, "Heard both.");
  });

  test("another model picked goes with its next turn, with its effort", { timeout: 90000 }, async () => {
    script.push({ text: "On the first." }, { text: "On the other." });
    const id = await begin();
    await send(id, "One.");
    await answered(id, "On the first.");
    const at = await prefix(id);
    const picked = await call("POST", `${at}api/session/${id}/model`, { model: { providerID: STAND_IN.providerID, id: "other" } });
    assert.ok(picked.status < 300, picked.text);
    await send(id, "Two.");
    await answered(id, "On the other.");
    assert.equal(lastTurn().model, "other");
  });

  test("a picture sent goes to the model as a picture", { timeout: 90000 }, async () => {
    script.push({ text: "Nice picture." });
    const id = await begin();
    const pixel = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    const sent = await call("POST", `kuvo/sessions/${id}/send`, {
      id: `msg_${Date.now().toString(36)}pic`, words: "Look.", delivery: "steer",
      files: [{ uri: `data:image/png;base64,${pixel}`, name: "pixel.png" }],
    });
    assert.equal(sent.status, 200, sent.text);
    await answered(id, "Nice picture.");
    const parts = lastUser(lastTurn()).content;
    assert.ok(Array.isArray(parts) && parts.some(part => part.type === "image_url"), JSON.stringify(parts).slice(0, 400));
  });

  test("/compact summarizes the conversation with its model, and it goes on after", { timeout: 90000 }, async () => {
    // (Its summary requests, however many it makes, answered as such: the reply to "Go on." is "After.")
    const answer = messages => ({ text: JSON.stringify(messages.at(-1) ?? "").includes("Go on.") ? "After." : "The summary." });
    script.push({ text: "Before." }, answer, answer, answer, answer);
    const id = await begin();
    await send(id, "Before.");
    await answered(id, "Before.");
    const asked = model.requests.length;
    await send(id, "/compact");
    await until(() => model.requests.slice(asked).some(each => JSON.stringify(each.messages).toLowerCase().includes("summar")),
      "Its model was never asked for a summary.", 60000, 200);
    await send(id, "Go on.");
    await answered(id, "After.");
  });

  test("a subagent it starts runs, and its work shows on its step", { timeout: 90000 }, async () => {
    script.push(
      { tools: [{ name: "subagent", args: { agent: "general", description: "Look around", prompt: "Say what you see." } }] },
      { text: "I see a folder." },
      { text: "The subagent saw a folder." },
    );
    const id = await begin();
    await send(id, "Use a subagent.");
    const rows = await answered(id, "The subagent saw a folder.");
    const step = tools(rows).find(part => /subagent|task/.test(part.tool ?? part.name ?? ""));
    assert.ok(step, JSON.stringify(tools(rows)).slice(0, 800));
    assert.equal(step.state?.status, "completed");
  });
});
