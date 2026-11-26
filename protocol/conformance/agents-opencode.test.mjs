// OpenCode conversations as the app reaches them (`kuvo/sessions`, its asks, and the messages under
// the prefix `location` names), on the pinned OpenCode with a labelled stand-in model. The reference
// box runs them on its engine; the Rust box on the OpenCode agent (`opencode acp`). Both must give
// the app the same: these cases assert only what it reads.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { after, before, beforeEach, describe, test } from "node:test";
import { stopFixtureHolders } from "./claude.mjs";
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
    // (Windows links only with a privilege: a copy there.)
    if (process.platform === "win32") fs.copyFileSync(fs.realpathSync(pinned), program);
    else fs.symlinkSync(fs.realpathSync(pinned), program);
    model = await standInModel(script);
    write(path.join(home, "config", "opencode", "opencode.json"), JSON.stringify(standInSettings(model.url)));
    write(path.join(home, "box", "opencode.json"), "{}\n");
    if (process.platform !== "win32") {
      fs.mkdirSync(path.join(home, "bin"), { recursive: true });
      fs.copyFileSync(new URL("../../box/src/keeper/kuvo-wake", import.meta.url), path.join(home, "bin", "kuvo-wake"));
      fs.chmodSync(path.join(home, "bin", "kuvo-wake"), 0o755);
    }
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
    if (home) await stopFixtureHolders(home);
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

  const offered = async () => {
    const read = await call("GET", "kuvo/models/offered");
    assert.equal(read.status, 200, read.text);
    return read.json;
  };
  const engineStarted = () => fs.existsSync(path.join(home, "run", "engine.json")) && !binary.typescript;

  test("where OpenCode never ran, the picker offers it as an agent not yet listed, and nothing starts it", {
    timeout: 60000, skip: binary.typescript && "The reference starts OpenCode to list its models.",
  }, async () => {
    const answer = await offered();
    const own = answer.models.filter(model => model.providerID === "opencode-agent");
    assert.deepEqual(own.map(model => [model.id, model.name]), [["default", "OpenCode (its default model)"]]);
    assert.ok(!engineStarted(), "Listing started OpenCode's server.");
    assert.ok(!fs.existsSync(path.join(home, "run", "engine", "catalog.json")), "A list was made from nothing.");
  });

  test("a new conversation's turn streams its words", { timeout: 90000 }, async () => {
    script.push({ text: "Hello from the stand-in." });
    const id = await begin();
    await send(id, "Say hello.");
    const rows = await answered(id, "Hello from the stand-in.");
    const user = rows.find(row => role(row) === "user");
    assert.ok(user, "Its message isn't there.");
  });

  test("once a conversation ran, the picker and its accounts list what OpenCode offered it, and still don't start it", {
    timeout: 60000, skip: binary.typescript && "The reference starts OpenCode to list its models.",
  }, async () => {
    const answer = await offered();
    const listed = answer.models.filter(model => model.side !== "agents").map(model => `${model.providerID}/${model.id}`);
    assert.ok(listed.includes("standin/scripted") && listed.includes("standin/other"), JSON.stringify(listed));
    assert.ok(!answer.models.some(model => model.providerID === "opencode-agent"), "Still offered as not yet listed.");
    // Its accounts too, as an older client lists them.
    const accounts = await call("GET", "api/integration");
    assert.equal(accounts.status, 200, accounts.text);
    assert.ok(accounts.json.data.some(account => account.id === "standin"), accounts.text);
    assert.ok(!engineStarted(), "Listing started OpenCode's server.");
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
    // (Sent once the turn has ended, as the app shows it stopped: one sent while it stops waits.)
    const at = await prefix(id);
    await until(async () => !((await call("GET", `${at}api/session/active`)).json?.data ?? {})[id], "The turn never ended.", 30000, 100);
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

  test("its wake-ups are its own: set by its tools or from the app, and Wake Up Now reaches it", { timeout: 90000 }, async () => {
    script.push({ text: "Ready." });
    const id = await begin();
    await send(id, "Get ready.");
    await answered(id, "Ready.");
    const made = await call("POST", "kuvo/agent-tools", { name: "schedule_wake_up", arguments: { when: "+2h", note: "Look again." }, session: id });
    assert.equal(made.status, 200, made.text);
    assert.equal(made.json.isError, false, made.json.text);
    assert.match(made.json.text, /^Scheduled /);
    const added = await call("POST", `kuvo/wakes/${id}`, { due: Math.floor(Date.now() / 1000) + 3 * 3600, note: "From the app." });
    assert.equal(added.status, 200, added.text);
    const listed = (await call("GET", `kuvo/wakes/${id}`)).json;
    assert.deepEqual(listed.wakes.map(wake => wake.note), ["Look again.", "From the app."]);
    // (Kept as a coding agent's, so its round never starts OpenCode's server.)
    if (!binary.typescript) {
      assert.equal(fs.readFileSync(path.join(home, "wakes", id, "via"), "utf8").trim(), "bridge");
    }
    script.push({ text: "Woke up." });
    const now = await call("POST", `kuvo/wakes/${id}/now`, { wake: listed.wakes[0].id });
    assert.equal(now.status, 200, now.text);
    await answered(id, "Woke up.");
    assert.ok(JSON.stringify(model.requests.at(-1).messages).includes("Look again."), "It didn't hear its note.");
    assert.ok(!engineStarted(), "Its wake-ups started OpenCode's server.");
  });

  test("its usage is counted once", { timeout: 120000 }, async () => {
    // (Insights' totals over the last week, once it read what changed.)
    const totals = async () => {
      await call("POST", "kuvo/agents/api/kuvo/insights/refresh", {});
      const read = await until(async () => {
        const answer = await call("GET", "kuvo/agents/api/kuvo/insights?period=7");
        return answer.status === 200 && answer.json?.data && !answer.json.data.scanning && answer.json.data;
      }, "Insights never finished reading.", 60000, 500);
      return read.totals;
    };
    // (Once what earlier cases ran is all counted: two readings alike.)
    let before = await totals();
    await until(async () => {
      await new Promise(resolve => setTimeout(resolve, 1500));
      const again = await totals();
      const same = JSON.stringify(again) === JSON.stringify(before);
      before = again;
      return same;
    }, "Insights kept changing.", 30000, 0);
    script.push({ text: "Counted." });
    const id = await begin();
    await send(id, "Count me.");
    await answered(id, "Counted.");
    // (OpenCode keeps its turns; the agent's own record of them isn't counted again.)
    const grew = field => (after[field] ?? 0) - (before[field] ?? 0);
    const after = await until(async () => {
      const read = await totals();
      return read.calls > (before.calls ?? 0) && read;
    }, "Its turn was never counted.", 30000, 1000);
    assert.deepEqual({ sessions: grew("sessions"), calls: grew("calls"), input: grew("input"), output: grew("output") },
      { sessions: 1, calls: 1, input: 100, output: 10 }, JSON.stringify({ before, after }));
  });

  test("it's titled as OpenCode titles it, and its turn's tokens are what the model said", { timeout: 90000 }, async () => {
    script.push({ text: "Titled." });
    const id = await begin();
    await send(id, "Name this.");
    const rows = await answered(id, "Titled.");
    const reply = rows.filter(row => role(row) === "assistant").at(-1);
    const tokens = reply.info?.tokens ?? reply.tokens;
    assert.deepEqual({ input: tokens?.input, output: tokens?.output }, { input: 100, output: 10 }, JSON.stringify(reply).slice(0, 800));
    const at = await prefix(id);
    const titled = await until(async () => {
      const read = await call("GET", `${at}api/session/${id}`);
      const session = read.json?.data ?? read.json;
      return session?.title === "Scripted title" && session;
    }, async () => `Never titled: ${(await call("GET", `${at}api/session/${id}`)).text.slice(0, 600)}`, 30000, 500);
    assert.equal(titled.title, "Scripted title");
  });
});
