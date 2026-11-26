// Live queue, delivery, asks and waiting on the OpenCode agent, through the pinned OpenCode with
// OpenCode Go, in a temp home with only a non-renewing OpenCode Go key (copied in for the run).
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import net from "node:net";
import { test } from "node:test";
import { stopFixtureHolders } from "./claude.mjs";
import { boxBinary, isolatedEnv, launch, localRelay, openLine, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const credential = process.env.KUVO_CONFORMANCE_OPENCODE_GO;
const model = { providerID: "opencode-go", id: "deepseek-v4.1-flash" };
const headers = { "x-kuvo-from": "app" };

test("a real model’s question holds a queue through edits, delivery, restart and Stop", {
  skip: !(pinned && credential) && "Set KUVO_CONFORMANCE_OPENCODE and KUVO_CONFORMANCE_OPENCODE_GO to isolated test inputs.",
  timeout: 300000,
}, async () => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  const work = path.join(root, "work");
  privateHome(home);
  fs.mkdirSync(work, { recursive: true });
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "box", "opencode.json"), "{}\n");
  const program = path.join(home, "engine", "2.2.6", process.platform === "win32" ? "opencode.exe" : "opencode");
  fs.mkdirSync(path.dirname(program), { recursive: true });
  // (Windows links only with a privilege: a copy there.)
  if (process.platform === "win32") fs.copyFileSync(fs.realpathSync(pinned), program);
  else fs.symlinkSync(fs.realpathSync(pinned), program);
  const env = isolatedEnv(root);
  const dirs = Object.fromEntries(["data", "config", "state", "cache"].map(sub => [`XDG_${sub.toUpperCase()}_HOME`, path.join(home, sub)]));
  for (const folder of Object.values(dirs)) fs.mkdirSync(folder, { recursive: true });
  const relay = await localRelay();
  let seeder, box;
  try {
    const given = JSON.parse(fs.readFileSync(credential, "utf8"));
    if (given.integrationID !== model.providerID || given.value?.type !== "key" || given.value.refresh) {
      throw new Error("Use only a non-renewing OpenCode Go key in the scratch credential file.");
    }
    // The key put in the home's OpenCode as a sign-in would, by a server of its own that then stops.
    const reserve = net.createServer();
    await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
    const port = reserve.address().port;
    await new Promise(resolve => reserve.close(resolve));
    const password = "scratch-seed";
    seeder = launch(program, ["serve", "--port", String(port), "--hostname", "127.0.0.1"],
      { ...env, ...dirs, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password }, work);
    const engine = { port, password };
    await until(async () => (await request(engine, "GET", "api/provider").catch(() => null))?.status === 200, "The seeding OpenCode", 30000);
    const seeded = await request(engine, "POST", "api/credential", { integrationID: model.providerID, label: "Scratch conformance", value: given.value, activate: true });
    assert.ok(seeded.status === 200 || seeded.status === 201, `Seeding returned ${seeded.status}`);
    await seeder.stop();
    seeder = undefined;

    const start = () => startBox({ binary: boxBinary(), home, env: { ...env, KUVO_SANDBOX: "off" }, relay });
    box = start();
    let record = await box.ready();
    const call = (method, route, body) => request(record, method, route, body, headers, 60000);
    const newConversation = async () => {
      const started = await call("POST", "kuvo/sessions", { folder: work, model, approval: "askFirst" });
      assert.equal(started.status, 200, started.text);
      const id = started.json.info?.id ?? started.json.id;
      // (A conversation just made may not be on the list yet: it says so, and the app asks again.)
      let where;
      await until(async () => (where = await call("GET", `kuvo/sessions/${id}/location`)).status !== 503,
        () => `Its location never answered: ${where?.text}`, 30000, 200);
      assert.ok(JSON.stringify(where.json).includes("agents"), where.text);
      return id;
    };
    let session = await newConversation();
    let base = "kuvo/sessions/" + session;
    const send = async (id, words, delivery = "queue") => {
      const reply = await call("POST", base + "/send", { id, words, delivery });
      assert.equal(reply.status, 200, reply.text);
    };
    // The queue as the app follows it: the first frame on its line.
    const queued = async () => {
      const watch = Buffer.from(JSON.stringify({ q: base + "/queue/events" })).toString("base64url");
      const line = await openLine(record, "?watch=" + watch, undefined, headers);
      try {
        for await (const frame of line.frames()) if (frame.t === "q") return (frame.d.items ?? []).map(item => item.id);
      } finally {
        line.close();
      }
    };
    const inbox = async expected => {
      let rows;
      await until(async () => (rows = await queued())?.join() === expected.join(),
        () => `Expected queue ${expected}, last read ${rows}`, 30000, 200);
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
    const transcript = async () => {
      const reply = await call("GET", `kuvo/agents/api/session/${session}/message?limit=50&order=asc`);
      return reply.json?.data ?? [];
    };
    const answer = async asked => {
      const answers = Object.fromEntries(asked.questions.map(each => {
        const value = each.options.find(option => /^(red|tea)$/i.test(option.label))?.value ?? each.options[0]?.value;
        assert.ok(value, "The model’s question has no options.");
        return [each.key, each.multiple ? [value] : value];
      }));
      const answered = await call("POST", base + "/asks/" + encodeURIComponent(asked.id), { answers });
      assert.equal(answered.status, 200, answered.text);
    };

    // The question pins active work, while the exact words or tool reasoning can vary.
    await send("msg_live1", "Use the question tool now to ask me to choose Red or Blue. Put Red and Blue in two options of one question. Wait for my selection. Do not answer in ordinary text first. After I choose, reply with just the selected color.", "steer");
    const asked = await until(question, "the real model to ask for a color", 60000, 500);
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
    assert.equal((await question())?.id, asked.id);

    await answer(asked);
    await waiting(0);
    await until(async () => (await transcript()).filter(row => row.id === draft.id && row.type === "user").length === 1,
      "the queued draft to reach the transcript once", 60000, 500);
    await inbox([]);

    // A restart: the delivery, sent again, is the same message, and still there once.
    await box.stop();
    box = start();
    record = await box.ready();
    const replay = await call("POST", base + "/delivery?computer=here", draft);
    assert.equal(replay.status, 200, replay.text);
    assert.equal(replay.json.message, draft.id);
    assert.equal((await call("GET", receipt)).json.status, "committed");
    await until(async () => (await transcript()).some(row => row.type === "idle"), "the restarted conversation to read", 30000, 500);
    assert.equal((await transcript()).filter(row => row.id === draft.id && row.type === "user").length, 1);

    session = await newConversation();
    base = "kuvo/sessions/" + session;
    await send("msg_liveStopQuestion", "Call the question tool now with one question asking me to choose tea or coffee and those two options. Wait for my choice. Do not answer in ordinary text first.", "steer");
    await until(question, "the real model to ask before Stop", 60000, 500);
    const first = "msg_liveBeforeStop";
    const second = "msg_liveWhileStopped";
    await send(first, "Reply only with FIRST.");
    await inbox([first]);
    const stopped = await call("POST", base + "/stop");
    assert.equal(stopped.status, 200, stopped.text);
    assert.equal(stopped.json.paused, true);
    await until(async () => {
      const active = await call("GET", "kuvo/agents/api/session/active");
      const running = active.json?.data ?? active.json;
      return active.status === 200 && running && (Array.isArray(running) ? !running.includes(session) : !Object.hasOwn(running, session));
    }, "the stopped turn to end", 20000);
    await send(second, "Reply only with SECOND.");
    await inbox([first, second]);

    const resumed = await call("POST", base + "/queue/resume");
    assert.equal(resumed.status, 200, resumed.text);
    const rows = await until(async () => {
      const read = await transcript();
      const at = read.findIndex(row => row.id === second && row.type === "user");
      return at >= 0 && read.slice(at).some(row => row.type === "idle") && read;
    }, "both prompts to reach the real conversation in sequence", 60000, 500);
    const firstAt = rows.findIndex(row => row.type === "user" && row.id === first);
    const secondAt = rows.findIndex(row => row.type === "user" && row.id === second);
    assert.ok(firstAt >= 0 && secondAt > firstAt, JSON.stringify(rows.map(row => [row.type, row.id])));
    assert.equal(rows.filter(row => row.id === first).length, 1);
    assert.equal(rows.filter(row => row.id === second).length, 1);
    assert.ok(rows.slice(firstAt + 1, secondAt).some(row => row.type === "idle"));
    await inbox([]);
    // OpenCode ran as the agent, never as the engine's server.
    assert.ok(!fs.existsSync(path.join(home, "run", "engine.json")), "The engine's OpenCode started.");
  } finally {
    await seeder?.stop?.();
    await box?.stop();
    await stopFixtureHolders(home);
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  }
});
