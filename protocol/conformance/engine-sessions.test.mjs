// Session and message behavior through the selected resident and its engine API.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { openLine, request, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

const vectors = JSON.parse(fs.readFileSync(new URL("../vectors/message-parts.json", import.meta.url)));

test("messages saved before parts were kept still show their parts", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("message-parts");
  const resident = await here.start().ready();
  const response = await request(resident, "POST", "kuvo/messages/parts", {
    texts: vectors.read.map(row => row.text),
  });
  assert.equal(response.status, 200);
  for (const [index, row] of vectors.read.entries()) {
    assert.deepEqual(response.json.parts[index], row.parts, row.about);
  }
});

test("a session reports the latest measured step until a summary supersedes it", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-context");
  const model = { id: "wide", providerID: "test" };
  const session = {
    ...sessionOf({ id: "ses_context" }),
    model,
    tokens: { input: 3_000_000 },
  };
  here.engine.sessions.push(session);
  here.engine.models = [{ ...model, limit: { context: 1_000_000 } }];
  const resident = await here.start().ready();
  const read = async () => {
    const response = await request(resident, "GET", "api/session/ses_context");
    assert.equal(response.status, 200);
    return response.json.data;
  };
  assert.deepEqual(await read(), { ...session, contextWindow: 1_000_000 });

  here.engine.models = [];
  const step = { type: "assistant", model, tokens: { input: 10, output: 8, reasoning: 2, cache: { read: 400_000, write: 500 } } };
  here.engine.messages.set(session.id, [step]);
  assert.deepEqual(await read(), {
    ...session,
    contextWindow: 1_000_000,
    context: { used: 400_520, window: 1_000_000 },
  });

  here.engine.messages.set(session.id, [{ type: "compaction", status: "completed" }, step]);
  assert.deepEqual(await read(), { ...session, contextWindow: 1_000_000 });
});

test("rename and archive keep session notes, and a subagent goes with its conversation", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-actions");
  const notes = { kuvo: { startedBy: "ses_starter", archived: true }, another: { kept: true } };
  here.engine.sessions.push(
    { ...sessionOf({ id: "ses_project", title: "Before", directory: here.home }), metadata: notes },
    sessionOf({ id: "ses_child", title: "Child", parent: "ses_project" }),
  );
  const resident = await here.start().ready();
  const path = "kuvo/sessions/ses_project";
  const rename = await request(resident, "POST", path + "/rename", { title: "  After  " });
  assert.equal(rename.status, 200);
  assert.equal(rename.json.info.title, "After");
  assert.deepEqual(rename.json.info.metadata, notes);

  const stale = await request(resident, "POST", path + "/archive", { archived: false, was: { updated: 0, archived: true } });
  assert.equal(stale.status, 409);
  assert.equal(stale.json.message, "It changed since you read it.");

  const unarchive = await request(resident, "POST", path + "/archive", { archived: false, was: { updated: 1, archived: true } });
  assert.equal(unarchive.status, 200);
  assert.deepEqual(unarchive.json.info.metadata, {
    kuvo: { startedBy: "ses_starter" }, another: { kept: true }, "kuvo.archived": false,
  });
  const child = await request(resident, "DELETE", "kuvo/sessions/ses_child");
  assert.equal(child.status, 409);
  assert.match(child.json.message, /subagent/);
  const deleted = await request(resident, "DELETE", path);
  assert.equal(deleted.status, 200);
  assert.deepEqual(deleted.json, { session: "ses_project" });
  assert.equal((await request(resident, "GET", "api/session/ses_project")).status, 404);
});

test("a busy conversation can steer, edit and remove messages in its queue", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-queue");
  const session = "ses_queue";
  here.engine.sessions.push(sessionOf({ id: session, title: "Busy", directory: here.home }));
  here.engine.running.add(session);
  const resident = await here.start().ready();
  const path = "kuvo/sessions/" + session;
  const send = (id, words) => request(resident, "POST", path + "/send", { id, words, delivery: "queue" }, { "x-kuvo-from": "app" });
  assert.equal((await send("msg_first", "first")).status, 200);
  assert.equal((await send("msg_edit", "edit me")).status, 200);
  assert.equal((await send("msg_drop", "take back")).status, 200);

  const steer = await request(resident, "POST", path + "/queue/msg_first/steer");
  assert.equal(steer.status, 200);
  assert.deepEqual(here.engine.interrupted, []);
  assert.deepEqual(here.engine.inboxes.get(session).map(row => [row.id, row.delivery]),
    [["msg_first", "steer"], ["msg_edit", "queue"], ["msg_drop", "queue"]]);
  const removed = await request(resident, "DELETE", path + "/queue/msg_edit?receipt=msg_undo");
  assert.equal(removed.status, 200);
  assert.equal(removed.json.removed.words, "edit me");
  assert.equal(removed.json.removed.before, "msg_drop");
  assert.equal(removed.json.removed.receipt, "msg_undo");
  assert.equal((await request(resident, "DELETE", path + "/queue/msg_drop")).status, 200);
  assert.equal((await send("msg_changed", "edited words")).status, 200);
  assert.deepEqual(here.engine.inboxes.get(session).map(row => [row.id, row.payload.metadata.kuvo.parts.words]),
    [["msg_first", "first"], ["msg_changed", "edited words"]]);
});

test("repeating the same send after the resident loses its reply leaves one message", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-retry");
  const session = "ses_recovery";
  here.engine.sessions.push(sessionOf({ id: session, title: "Restart", directory: here.home }));
  const first = here.start();
  const initial = await first.ready();
  const path = "kuvo/sessions/" + session + "/send";
  const message = { id: "msg_lostReply", words: "One copy" };
  const accepted = new Promise(resolve => { here.engine.holdPrompt = { id: message.id, accepted: resolve }; });
  const original = request(initial, "POST", path, message).catch(() => null);
  await accepted;
  first.child.kill("SIGKILL");
  await first.exited;
  await original;

  const resumed = await here.start().ready();
  const sent = await request(resumed, "POST", path, message);
  assert.equal(sent.status, 200);
  assert.deepEqual(sent.json, {});
  assert.deepEqual(here.engine.messages.get(session).map(row => row.id), [message.id]);
  assert.deepEqual((await request(resumed, "POST", path, message)).json, sent.json);
});

test("a new chat keeps its session marks and appears in the local session list", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-start");
  const resident = await here.start().ready();
  const started = await request(resident, "POST", "kuvo/sessions", { title: "New conversation", chat: true, approval: "askFirst" });
  assert.equal(started.status, 200);
  assert.equal(started.json.title, "New conversation");
  assert.equal(started.json.project, null);
  assert.equal(started.json.folder, path.join(here.home, "chats"));
  assert.equal(started.json.info.metadata.kuvo.chat, started.json.folder);
  assert.equal(started.json.info.metadata.kuvo.approval, "askFirst");

  const watch = Buffer.from(JSON.stringify({ sessions: "kuvo/sessions/list/events" })).toString("base64url");
  const line = await openLine(resident, "?watch=" + watch);
  t.after(() => line.close());
  for await (const frame of line.frames()) {
    if (!frame.d?.sessions?.[started.json.session]) continue;
    assert.equal(frame.d.sessions[started.json.session].kind, "chat");
    assert.equal(frame.d.sessions[started.json.session].title, "New conversation");
    return;
  }
  assert.fail("the new chat did not reach the line");
});

test("a stopped queue replays its send and protects an undone message from an old removal", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-undo");
  const id = "ses_undo";
  here.engine.sessions.push(sessionOf({ id, title: "Paused", directory: here.home }));
  const first = here.start();
  let resident = await first.ready();
  const path = "kuvo/sessions/" + id;
  assert.equal((await request(resident, "POST", path + "/stop")).status, 200);
  const message = { id: "msg_undo", words: "Keep me", delivery: "queue" };
  const send = await request(resident, "POST", path + "/send", message);
  assert.equal(send.status, 200);
  assert.deepEqual(send.json, { waits: true });
  await first.stop();
  resident = await here.start().ready();

  const replay = await request(resident, "POST", path + "/send", message);
  assert.equal(replay.status, 200);
  assert.deepEqual(replay.json, send.json);
  const remove = path + "/queue/msg_undo?receipt=msg_ticket";
  const removed = await request(resident, "DELETE", remove);
  assert.equal(removed.status, 200);
  const undo = await request(resident, "POST", path + "/queue/add", { ...removed.json.removed, kept: true });
  assert.equal(undo.status, 200, undo.text);
  assert.equal(undo.json.waits, true);
  assert.equal((await request(resident, "DELETE", remove)).status, 404);
  const queue = await request(resident, "GET", path + "/queue");
  assert.equal(queue.status, 200);
  assert.deepEqual(queue.json.waiting.map(row => [row.id, row.text]), [[message.id, message.words]]);
});

test("a message removed while on its way can be taken back when it arrives", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-arriving");
  const id = "ses_arriving";
  here.engine.sessions.push(sessionOf({ id, title: "Arriving", directory: here.home }));
  const resident = await here.start().ready();
  const path = "kuvo/sessions/" + id;
  assert.equal((await request(resident, "POST", path + "/stop")).status, 200);
  const route = path + "/queue/msg_arriving?arriving=1";
  const before = await request(resident, "DELETE", route);
  assert.equal(before.status, 409);
  assert.equal(before.json.arriving, true);
  const send = await request(resident, "POST", path + "/send", { id: "msg_arriving", words: "Keep it here first" });
  assert.equal(send.status, 200);
  assert.deepEqual(send.json, { waits: true });
  const after = await request(resident, "DELETE", route);
  assert.equal(after.status, 200);
  assert.equal(after.json.removed.words, "Keep it here first");
  assert.deepEqual(here.engine.messages.get(id) ?? [], []);
});

test("summary titles agree between a conversation's message page and its live line", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-summary");
  const session = "ses_summary";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.messages.set(session, [
    { id: "msg_summary", type: "compaction", status: "completed", text: "Earlier steps" },
    { id: "msg_text", type: "user", text: "Continue" },
  ]);
  const resident = await here.start().ready();
  const page = await request(resident, "GET", "api/session/" + session + "/message?limit=2");
  assert.equal(page.status, 200);
  assert.equal(page.json.data[0].title, "Conversation summarized");
  assert.deepEqual(page.json.data[1], here.engine.messages.get(session)[1]);

  const watch = Buffer.from(JSON.stringify({ conversation: "api/event" })).toString("base64url");
  const line = await openLine(resident, "?watch=" + watch);
  t.after(() => line.close());
  for await (const frame of line.frames()) {
    if (frame.d?.type === "server.connected") {
      here.engine.emit({ type: "session.compaction.ended", data: { sessionID: session } });
    }
    if (frame.d?.type === "session.compaction.ended") {
      assert.deepEqual(frame.d.data, { sessionID: session, title: page.json.data[0].title });
      return;
    }
  }
  assert.fail("the summary did not reach the line");
});

test("a send keeps what the user wrote and the parts that went with it", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-parts");
  const id = "ses_sendParts";
  here.engine.sessions.push(sessionOf({ id, directory: here.home }));
  const record = await here.start().ready();
  const supported = vectors.compose.filter(row => !row.kept);
  const sessions = new Set(supported.map(row => row.excluding ?? id));
  for (const session of sessions) {
    if (session !== id) here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  }
  for (const [index, row] of supported.entries()) {
    const session = row.excluding ?? id;
    const given = { ...row.fields, id: "msg_parts" + index };
    write(path.join(here.home, "run", "agents.json"), JSON.stringify({ machine: row.here ?? null, agents: vectors.agents }));
    const response = await request(record, "POST", "kuvo/sessions/" + session + "/send", given,
      row.phone ? {} : { "x-kuvo-from": "app" });
    assert.equal(response.status, 200, row.about);
    const stored = here.engine.messages.get(session).find(message => message.id === given.id);
    assert.equal(stored.text, row.text, row.about);
    assert.deepEqual(stored.metadata.kuvo.parts, row.parts, row.about);
  }
});

test("the line shows a reordered queue and Send Now interrupts the running step", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-reorder");
  const id = "ses_reorder";
  here.engine.sessions.push(sessionOf({ id, title: "Reorder", directory: here.home }));
  here.engine.running.add(id);
  const record = await here.start().ready();
  const path = "kuvo/sessions/" + id;
  for (const item of ["msg_A", "msg_B", "msg_C"]) {
    const send = await request(record, "POST", path + "/send", { id: item, words: item, delivery: "queue" });
    assert.equal(send.status, 200);
  }
  const watch = Buffer.from(JSON.stringify({ queue: path + "/queue/events" })).toString("base64url");
  const line = await openLine(record, "?watch=" + watch);
  try {
    const frames = line.frames();
    const nextItems = async () => {
      for (;;) {
        const { done, value } = await frames.next();
        assert.ok(!done, "the queue line closed");
        if (value.d?.items) return value.d.items.map(item => item.id);
      }
    };
    assert.deepEqual(await nextItems(), ["msg_A", "msg_B", "msg_C"]);
    const reorder = await request(record, "POST", path + "/queue/reorder", {
      item: "msg_C", anchor: "msg_A", before: true,
    });
    assert.equal(reorder.status, 200);
    assert.deepEqual(reorder.json.items.map(item => item.id), ["msg_C", "msg_A", "msg_B"]);
    assert.deepEqual(await nextItems(), ["msg_C", "msg_A", "msg_B"]);
    const now = await request(record, "POST", path + "/queue/msg_C/now");
    assert.equal(now.status, 200);
    assert.deepEqual(here.engine.interrupted, [{ session: id, resume: true }]);
    assert.equal(here.engine.inboxes.get(id).find(item => item.id === "msg_C").delivery, "steer");
    assert.equal(here.engine.inboxes.get(id).find(item => item.id === "msg_A").delivery, "queue");
  } finally {
    line.close();
  }
});
