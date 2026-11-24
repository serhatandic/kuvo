// Live queue, delivery, asks and waiting through an isolated, pinned OpenCode with OpenCode Go.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import net from "node:net";
import { test } from "node:test";
import { boxBinary, isolatedEnv, launch, localRelay, request, scratch, startBox, until, write } from "./adapter.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const credential = process.env.KUVO_CONFORMANCE_OPENCODE_GO;
const model = { providerID: "opencode-go", id: "deepseek-v4.1-flash" };
test("a real model’s question holds a queue through edits, delivery, restart and Stop", {
  skip: !(pinned && credential) && "Set KUVO_CONFORMANCE_OPENCODE and KUVO_CONFORMANCE_OPENCODE_GO to isolated test inputs.",
  timeout: 240000,
}, async () => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  fs.mkdirSync(home);
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  const env = isolatedEnv(root);
  const reserve = net.createServer();
  await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
  const port = reserve.address().port;
  await new Promise(resolve => reserve.close(resolve));
  const password = "scratch-engine";
  const pin = fs.realpathSync(pinned);
  const engine = { port, password };
  const relay = await localRelay();
  let worker, box;
  try {
    const given = JSON.parse(fs.readFileSync(credential, "utf8"));
    if (given.integrationID !== model.providerID || given.value?.type !== "key" || given.value.refresh) {
      throw new Error("Use only a non-renewing OpenCode Go key in the scratch credential file.");
    }
    worker = launch(pin, ["serve", "--port", String(port), "--hostname", "127.0.0.1"], {
      ...env, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password,
      XDG_DATA_HOME: path.join(root, "opencode-data"), XDG_CONFIG_HOME: path.join(root, "opencode-config"),
      XDG_CACHE_HOME: path.join(root, "opencode-cache"), XDG_STATE_HOME: path.join(root, "opencode-state"),
    }, home);
    await until(async () => (await request(engine, "GET", "api/session?limit=1").catch(() => null))?.status === 200,
      "Pinned OpenCode readiness", 20000);
    await until(async () => (await request(engine, "GET", "api/provider")).status === 200,
      "OpenCode’s provider plugins to be ready", 25000);
    const seeded = await request(engine, "POST", "api/credential", {
      integrationID: model.providerID, label: "Scratch conformance", value: given.value, activate: true,
    });
    assert.ok(seeded.status === 200 || seeded.status === 201, `Scratch credential registration returned ${seeded.status}`);
    await until(async () => {
      const read = await request(engine, "GET", "api/model");
      return read.json?.data?.some(row => row.providerID === model.providerID && row.id === model.id && row.enabled !== false);
    }, "OpenCode Go DeepSeek V4.1 Flash in the scratch catalog", 20000);

    write(path.join(home, "run", "engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version: "2.2.6" }));
    const binary = boxBinary();
    const start = () => startBox({ binary, home, env: { ...env, KUVO_SANDBOX: "off" }, relay });
    box = start();
    let record = await box.ready();
    const newSession = async title => {
      const started = await request(engine, "POST", "api/session", { title, location: { directory: home }, model });
      assert.equal(started.status, 200, started.text);
      return started.json.data.id;
    };
    let session = await newSession("Scratch live queue");
    let base = "kuvo/sessions/" + session;
    const headers = { "x-kuvo-from": "app" };
    const call = (method, route, body) => request(record, method, route, body, headers);
    const send = async (id, words, delivery = "queue") => {
      const reply = await call("POST", base + "/send", { id, words, delivery });
      assert.equal(reply.status, 200, reply.text);
    };
    const inbox = async expected => {
      let rows;
      await until(async () => {
        const reply = await call("GET", base + "/queue?items=1");
        assert.equal(reply.status, 200, reply.text);
        rows = reply.json.items?.map(row => row.id);
        return rows?.join() === expected.join();
      }, () => `Expected queue ${expected}, last read ${rows}`);
    };
    const question = async () => {
      const reply = await call("GET", base + "/asks");
      assert.equal(reply.status, 200, reply.text);
      return reply.json.asks.find(ask => ask.kind === "question");
    };
    const waiting = async expected => until(async () => {
      const reply = await call("GET", "kuvo/waiting");
      assert.equal(reply.status, 200, reply.text);
      return reply.json.sessions === expected;
    }, `waiting count ${expected}`, 15000);

    // The question pins active work, while the exact words or tool reasoning can vary.
    await send("msg_live1", "Use the question tool now to ask me to choose Red or Blue. Put Red and Blue in two options of one question. Wait for my selection. Do not answer in ordinary text first. After I choose, reply with just the selected color.", "steer");
    const asked = await until(question, "the real model to ask for a color", 45000);
    await waiting(1);

    await send("msg_live2", "Wrong second message");
    await send("msg_live3", "Unneeded third message");
    await inbox(["msg_live2", "msg_live3"]);
    assert.equal((await call("POST", base + "/queue/msg_live2/steer")).status, 200);
    assert.equal((await call("DELETE", base + "/queue/msg_live2?receipt=msg_edit")).status, 200);
    assert.equal((await call("DELETE", base + "/queue/msg_live3?receipt=msg_remove")).status, 200);

    const draft = { id: "msg_live4", text: "Reply only with Done.", message: { delivery: "queue" } };
    const saved = await call("POST", base + "/delivery?computer=here", draft);
    assert.equal(saved.status, 202, saved.text);
    const receipt = "kuvo/data/messages/" + saved.json.operation;
    await until(async () => (await call("GET", receipt)).json?.status === "committed", "the queued draft’s receipt to settle", 20000);
    await inbox([draft.id]);

    await box.stop();
    box = start();
    record = await box.ready();
    await inbox([draft.id]);
    assert.equal((await question())?.id, asked.id);
    const replay = await call("POST", base + "/delivery?computer=here", draft);
    assert.equal(replay.status, 200, replay.text);
    assert.equal(replay.json.message, draft.id);
    assert.equal((await call("GET", receipt)).json.status, "committed");
    const answers = Object.fromEntries(asked.questions.map(question => {
      const value = question.options.find(option => option.label.toLowerCase() === "red")?.value ?? question.options[0]?.value;
      assert.ok(value, "The model’s question has no options.");
      return [question.key, question.multiple ? [value] : value];
    }));
    const answered = await call("POST", base + "/asks/" + encodeURIComponent(asked.id), { answers });
    assert.equal(answered.status, 200, answered.text);
    await waiting(0);
    await until(async () => {
      const actual = await request(engine, "GET", `api/session/${session}/message?limit=50&order=desc`);
      return actual.json?.data?.filter(row => row.id === draft.id && row.type === "user").length === 1;
    }, "the queued draft to reach the transcript once", 30000);

    session = await newSession("Scratch Stop and Resume");
    base = "kuvo/sessions/" + session;
    await send("msg_liveStopQuestion", "Call the question tool now with one question asking me to choose tea or coffee and those two options. Wait for my choice. Do not answer in ordinary text first.", "steer");
    await until(question, "the real model to ask before Stop", 45000);
    const first = "msg_liveBeforeStop";
    const second = "msg_liveWhileStopped";
    await send(first, "Reply only with FIRST.");
    await inbox([first]);
    const stopped = await call("POST", base + "/stop");
    assert.equal(stopped.status, 200, stopped.text);
    assert.equal(stopped.json.paused, true);
    await until(async () => {
      const active = await request(engine, "GET", "api/session/active");
      const running = active.json?.data;
      return active.status === 200 && running && (Array.isArray(running) ? !running.includes(session) : !Object.hasOwn(running, session));
    }, "the stopped turn to end", 20000);
    await send(second, "Reply only with SECOND.");
    await inbox([first, second]);
    const waitingBeforeResume = await call("GET", base + "/queue");
    assert.deepEqual(waitingBeforeResume.json.waiting.map(row => row.id), [second]);
    const engineInbox = await request(engine, "GET", `api/session/${session}/inbox`);
    assert.deepEqual(engineInbox.json.data.map(row => row.id), [first]);

    const resumed = await call("POST", base + "/queue/resume");
    assert.equal(resumed.status, 200, resumed.text);
    const transcript = await until(async () => {
      const reply = await request(engine, "GET", `api/session/${session}/message?limit=50&order=desc`);
      const rows = reply.json?.data;
      return rows?.some(row => row.id === second && row.type === "user") && rows.toReversed();
    }, "both prompts to reach the real conversation in sequence", 50000);
    const firstAt = transcript.findIndex(row => row.type === "user" && row.id === first);
    const secondAt = transcript.findIndex(row => row.type === "user" && row.id === second);
    assert.ok(firstAt >= 0 && secondAt > firstAt);
    assert.equal(transcript.filter(row => row.id === first).length, 1);
    assert.equal(transcript.filter(row => row.id === second).length, 1);
    assert.ok(transcript.slice(firstAt + 1, secondAt).some(row => row.type === "idle" && row.outcome === "succeeded"));
    await inbox([]);
  } finally {
    await box?.stop();
    await worker?.stop();
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  }
});
