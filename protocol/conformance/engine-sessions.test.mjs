// Session and message behavior through the selected resident and its engine API.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { openLine, readJSON, request, until, write } from "./adapter.mjs";
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
    { ...sessionOf({ id: "ses_seat", title: "Seat" }), metadata: { kuvo: { seat: "grp_test" } } },
    { ...sessionOf({ id: "ses_hidden", title: "Internal" }), metadata: { kuvo: { engineFor: "ses_project" } } },
    { ...sessionOf({ id: "ses_engineArchived", title: "Archived by OpenCode" }), time: { created: 1, updated: 2, archived: 2 } },
  );
  const resident = await here.start().ready();
  const route = "kuvo/sessions/ses_project";
  const rename = await request(resident, "POST", route + "/rename", { title: "  After  " });
  assert.equal(rename.status, 200);
  assert.equal(rename.json.info.title, "After");
  assert.deepEqual(rename.json.info.metadata, notes);
  assert.equal((await request(resident, "POST", route + "/rename", { title: "  " })).status, 400);

  for (const pinned of [true, false]) {
    const changed = await request(resident, "POST", route + "/pin", { pinned });
    assert.equal(changed.status, 200, changed.text);
    assert.deepEqual(changed.json.info.metadata, { ...notes, "kuvo.pinned": pinned });
  }

  const stale = await request(resident, "POST", route + "/archive", { archived: false, was: { updated: 0, archived: true } });
  assert.equal(stale.status, 409);
  assert.equal(stale.json.message, "It changed since you read it.");

  const unarchive = await request(resident, "POST", route + "/archive", { archived: false, was: { updated: 1, archived: true } });
  assert.equal(unarchive.status, 200);
  assert.deepEqual(unarchive.json.info.metadata, {
    kuvo: { startedBy: "ses_starter" }, another: { kept: true }, "kuvo.archived": false, "kuvo.pinned": false,
  });
  const child = await request(resident, "DELETE", "kuvo/sessions/ses_child");
  assert.equal(child.status, 409);
  assert.match(child.json.message, /subagent/);
  for (const [id, message] of [["ses_seat", /seat/], ["ses_hidden", /another conversation/]]) {
    const refused = await request(resident, "DELETE", "kuvo/sessions/" + id);
    assert.equal(refused.status, 409, refused.text);
    assert.match(refused.json.message, message);
  }
  const archived = "kuvo/sessions/ses_engineArchived";
  const menu = await request(resident, "GET", archived + "/actions");
  assert.equal(menu.status, 200, menu.text);
  assert.ok(!menu.json.actions.some(action => action.id === "unarchive"));
  const engineArchived = await request(resident, "POST", archived + "/archive", { archived: false });
  assert.equal(engineArchived.status, 409, engineArchived.text);
  assert.match(engineArchived.json.message, /archived by the engine/);
  assert.equal((await request(resident, "POST", route + "/stop", {}, { "x-kuvo-from": "app" })).status, 200);
  const queued = { id: "msg_deletedWithSession", words: "Saved for later", delivery: "queue" };
  const kept = await request(resident, "POST", route + "/send", queued, { "x-kuvo-from": "app" });
  assert.deepEqual(kept.json, { waits: true });
  const waiting = path.join(here.home, "run/waiting", queued.id + ".json");
  assert.equal(fs.existsSync(waiting), true);
  const deleted = await request(resident, "DELETE", route);
  assert.equal(deleted.status, 200);
  assert.deepEqual(deleted.json, { session: "ses_project" });
  assert.deepEqual((await request(resident, "DELETE", route)).json, deleted.json);
  assert.equal((await request(resident, "DELETE", route, { was: { archived: false, updated: 1 } })).status, 409);
  assert.equal((await request(resident, "GET", "api/session/ses_project")).status, 404);
  await until(() => !fs.existsSync(waiting), "The deleted session's waiting payload was retained.");
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

test("a restart beside the same engine scans its queues and preserves an explicit stop", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-scan");
  const resumed = "ses_knownInbox";
  const stopped = "ses_stoppedInbox";
  for (const session of [resumed, stopped]) {
    const info = sessionOf({ id: session, directory: here.home });
    info.time.updated = Date.now();
    here.engine.sessions.push(info);
    here.engine.inboxes.set(session, [{ id: "msg_waitingAfterRestart", type: "user", delivery: "queue", payload: { text: "Saved while offline" } }]);
  }
  here.engine.messages.set(stopped, [{ type: "idle", outcome: "interrupted", reason: "stop", time: { created: Date.now() } }]);
  const scanned = path.join(here.home, "run", "queues-scanned.json");
  write(scanned, JSON.stringify({ formatVersion: 1, [here.engine.record.port]: here.engine.record.pid }));
  const record = await here.start().ready();
  await until(async () => {
    const queue = await request(record, "GET", `kuvo/sessions/${stopped}/queue`);
    return queue.json?.paused === true && here.engine.inboxes.get(resumed)[0]?.delivery === "steer";
  }, "Known engine work did not resume or the explicit stop was forgotten.", 20000, 50);
  assert.equal(here.engine.inboxes.get(stopped)[0].delivery, "queue");
  assert.deepEqual(readJSON(scanned), { formatVersion: 1, [here.engine.record.port]: here.engine.record.pid });
});

test("Stop records messages on their way while it waits for a send in flight", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("stop-in-flight");
  const session = "ses_inFlight";
  const route = "kuvo/sessions/" + session;
  const native = "/api/session/" + session;
  const headers = { "x-kuvo-from": "app" };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  const record = await here.start().ready();
  const draft = { id: "msg_inFlight", words: "Send before stopping.", delivery: "steer" };
  const open = Promise.withResolvers();
  const arrived = Promise.withResolvers();
  here.engine.holdPrompt = { id: draft.id, beforeAcceptance: true, accepted: arrived.resolve, release: open.promise };
  t.after(() => open.resolve());
  const sending = request(record, "POST", route + "/send", draft, headers);
  await arrived.promise;
  const next = "msg_namedByStop";
  const stopping = request(record, "POST", route + "/stop", { items: [next] }, headers);
  await until(() => {
    try { return readJSON(path.join(here.home, "run/queues.json"))?.[session]?.arriving?.[next]; }
    catch { return false; }
  }, "Stop waited for the prompt before it recorded the next message.", 5000, 25);
  open.resolve();
  const [stop, send] = await Promise.all([stopping, sending]);
  assert.equal(send.status, 200, send.text);
  assert.equal(stop.status, 200, stop.text);
  assert.equal(stop.json.paused, true);
  assert.deepEqual(stop.json.held, [draft.id]);
  const changed = here.engine.asked.findIndex(call => call.method === "PATCH" && call.path === native + "/inbox/" + draft.id);
  const interrupted = here.engine.asked.findIndex(call => call.path === native + "/interrupt");
  assert.ok(changed >= 0 && interrupted > changed);
});

test("Stop keeps named late messages on disk and Resume forgets the ones still missing", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("stop-arrivals");
  const session = "ses_stopArrivals";
  const route = "kuvo/sessions/" + session;
  const headers = { "x-kuvo-from": "app" };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  const child = here.start();
  const before = await child.ready();
  const send = (resident, id) => request(resident, "POST", route + "/send", { id, words: id, delivery: "steer" }, headers);
  const [first, second, untilResumed, free] = ["msg_arriveFirst", "msg_arriveSecond", "msg_afterResume", "msg_unnamed"];
  const stop = await request(before, "POST", route + "/stop", { items: [first, second, untilResumed] }, headers);
  assert.equal(stop.status, 200, stop.text);
  assert.deepEqual((await send(before, first)).json, { waits: true });
  await child.stop();
  const after = await here.start().ready();
  assert.deepEqual((await send(after, second)).json, { waits: true });

  const waiting = await request(after, "GET", route + "/queue?items=1");
  assert.equal(waiting.status, 200, waiting.text);
  assert.deepEqual(waiting.json.held, [first, second]);
  assert.deepEqual(waiting.json.waiting.map(row => row.id), [first, second]);
  assert.deepEqual(here.engine.inboxes.get(session) ?? [], []);
  const notNamed = await send(after, free);
  assert.equal(notNamed.status, 200, notNamed.text);
  assert.ok(here.engine.inboxes.get(session).some(row => row.id === free));
  const resume = await request(after, "POST", route + "/queue/resume", {}, headers);
  assert.equal(resume.status, 200, resume.text);
  const late = await send(after, untilResumed);
  assert.equal(late.status, 200, late.text);
  assert.ok(here.engine.inboxes.get(session).some(row => row.id === untilResumed));
});

for (const stage of ["before acceptance", "in the inbox", "in the transcript"]) {
  test(`the same desktop send recovers a reply lost ${stage} with one message`, { timeout: 45000 }, async t => {
    const { computer } = await bench(t);
    const here = await computer("engine-retry");
    const session = "ses_recovery";
    here.engine.sessions.push(sessionOf({ id: session, title: "Restart", directory: here.home }));
    if (stage === "in the inbox") here.engine.running.add(session);
    const first = here.start();
    const initial = await first.ready();
    const path = "kuvo/sessions/" + session + "/send";
    const message = { id: "msg_lostReply", words: "One copy", delivery: "queue" };
    const headers = { "x-kuvo-from": "app", "idempotency-key": message.id };
    const beforeAcceptance = stage === "before acceptance";
    const reached = new Promise(resolve => {
      here.engine.holdPrompt = { id: message.id, beforeAcceptance, accepted: resolve };
    });
    const original = request(initial, "POST", path, message, headers).catch(() => null);
    await reached;
    first.child.kill("SIGKILL");
    await first.exited;
    assert.equal(await original, null);

    const resumed = await here.start().ready();
    const sent = await request(resumed, "POST", path, message, headers);
    assert.equal(sent.status, 200, sent.text);
    assert.deepEqual(sent.json, {});
    assert.deepEqual((await request(resumed, "POST", path, message, headers)).json, sent.json);
    const delivered = [...here.engine.messages.get(session) ?? [], ...here.engine.inboxes.get(session) ?? []];
    assert.deepEqual(delivered.map(row => row.id), [message.id]);
    const offers = here.engine.asked.filter(call => call.method === "POST" && call.path.endsWith("/prompt"));
    assert.equal(offers.length, beforeAcceptance ? 2 : 1);
  });
}

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

  const watch = Buffer.from(JSON.stringify({
    sessions: "kuvo/sessions/list/events",
    setup: "kuvo/setup/sessions/events",
  })).toString("base64url");
  const line = await openLine(resident, "?watch=" + watch);
  t.after(() => line.close());
  let local = false;
  let published = false;
  for await (const frame of line.frames()) {
    const facts = frame.t === "sessions" && frame.d?.sessions?.[started.json.session];
    if (facts) {
      assert.equal(facts.kind, "chat");
      assert.equal(facts.title, "New conversation");
      local = true;
    }
    const row = frame.t === "setup" && frame.d?.sessions?.find(row => row.id === started.json.session);
    if (row) {
      assert.equal(row.facts.title, "New conversation");
      assert.equal(row.here, true);
      published = true;
    }
    if (local && published) return;
  }
  assert.fail("the new chat did not reach both lists on the line");
});

test("a project page grows in steps and keeps its running conversations first", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("project-page");
  const folder = path.join(here.home, "project");
  fs.mkdirSync(folder, { recursive: true });
  write(path.join(here.home, "box/projects.json"), JSON.stringify({ projects: [{ directory: folder, name: "Project" }] }));
  const day = number => "ses_page" + String(number).padStart(2, "0");
  for (let number = 1; number <= 80; number++) {
    const row = sessionOf({ id: day(number), title: "Conversation " + number, directory: folder });
    row.time.updated = number;
    here.engine.sessions.push(row);
  }
  const child = sessionOf({ id: "ses_pageChild", title: "Subagent", directory: folder, parent: day(80) });
  const anotherChild = sessionOf({ id: "ses_quietChild", title: "Subagent", directory: folder, parent: day(80) });
  child.time.updated = 5;
  anotherChild.time.updated = 20;
  here.engine.sessions.push(child, anotherChild);
  here.engine.running.add(day(1));
  here.engine.running.add(child.id);
  const record = await here.start().ready();
  const read = await until(async () => {
    const result = await request(record, "GET", "kuvo/sessions/list");
    return result.status === 200 && result.json.projects?.[folder]?.total === 80 && result.json;
  }, "The resident did not file the project conversations.", 12000, 80);
  const project = read.projects[folder];
  assert.deepEqual(project.shown, [day(80), day(1), day(79), day(78), day(77), day(76)]);
  assert.deepEqual(read.sessions[day(80)].subagents, [child.id, anotherChild.id]);
  assert.deepEqual(read.sessions[child.id].messages, { via: "direct" });
  const page = cursor => request(record, "GET", "kuvo/sessions/page?" +
    new URLSearchParams({ project: folder, ...(cursor ? { cursor } : {}) }));
  assert.equal((await request(record, "GET", "kuvo/sessions/page")).status, 400);
  assert.equal((await page("s7.zz")).status, 400);
  assert.equal((await page()).json.sessions.length, 6);
  let more = project.more;
  for (const [count, next] of [[16, 20], [36, 40], [76, 4], [80, null]]) {
    const response = await page(more.cursor);
    assert.equal(response.status, 200, response.text);
    assert.equal(response.json.project, folder);
    assert.equal(response.json.sessions[0], day(80));
    assert.equal(new Set(response.json.sessions).size, count);
    more = response.json.more;
    assert.equal(more?.count ?? null, next);
  }
  here.engine.running.delete(day(1));
  here.engine.running.delete(child.id);
  here.engine.running.add(day(30));
  here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: day(1) } });
  here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: child.id } });
  here.engine.emit({ type: "session.execution.started", data: { sessionID: day(30) } });
  const changed = await until(async () => {
    const response = await page(project.more.cursor);
    return response.json?.sessions?.[0] === day(30) && response.json;
  }, "The old project cursor did not follow the current running work.", 12000, 80);
  assert.equal(changed.sessions.length, 16);
  assert.ok(!changed.sessions.includes(day(1)));
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
  const sessions = new Set(vectors.compose.map(row => row.excluding ?? id));
  for (const session of sessions) {
    if (session !== id) here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  }
  for (const [index, row] of vectors.compose.entries()) {
    const session = row.excluding ?? id;
    const given = { ...row.fields, id: "msg_parts" + index };
    const included = row.kept?.included ?? [];
    const paths = row.kept?.paths ?? [];
    const attachments = [...included, ...paths];
    if (attachments.length) {
      given.files = attachments.map(kept => {
        const name = path.posix.basename(kept);
        const mime = { ".png": "image/png", ".jpg": "image/jpeg", ".zip": "application/zip" }[path.extname(name)];
        return { name, uri: `data:${mime};base64,${Buffer.from("fixture " + name).toString("base64")}` };
      });
    }
    write(path.join(here.home, "run", "agents.json"), JSON.stringify({ machine: row.here ?? null, agents: vectors.agents }));
    const response = await request(record, "POST", "kuvo/sessions/" + session + "/send", given,
      row.phone ? {} : { "x-kuvo-from": "app" });
    assert.equal(response.status, 200, row.about);
    const stored = here.engine.messages.get(session).find(message => message.id === given.id);
    const renamed = new Map();
    if (attachments.length) {
      const folder = path.join(here.home, "attachments", session);
      assert.ok(fs.existsSync(folder), row.about);
      for (const child of fs.readdirSync(folder)) {
        const file = path.join(folder, child);
        for (const name of fs.readdirSync(file)) {
          const kept = attachments.find(which => path.posix.basename(which) === name);
          if (kept) {
            const storedFile = path.join(file, name);
            assert.equal(fs.readFileSync(storedFile, "utf8"), "fixture " + name);
            renamed.set(kept, storedFile);
          }
        }
      }
      assert.equal(renamed.size, attachments.length, row.about);
      assert.deepEqual(stored.files.map(file => file.name), included.map(file => path.posix.basename(file)));
    }
    const move = text => [...renamed].reduce((next, [from, to]) => next.replaceAll(from, to), text);
    assert.equal(stored.text, move(row.text), row.about);
    const parts = structuredClone(row.parts);
    if (parts.paths) parts.paths = parts.paths.map(move);
    assert.deepEqual(stored.metadata.kuvo.parts, parts, row.about);
  }
});

test("a raw desktop prompt keeps inline attachments and sends only what the model reads", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-raw-attachments");
  const session = "ses_rawAttachments";
  const model = { providerID: "test", id: "text-only" };
  here.engine.sessions.push({ ...sessionOf({ id: session, directory: here.home }), model });
  here.engine.models.push({ ...model, capabilities: { input: ["text"] } });
  const files = [
    { name: "data.json", uri: "data:application/json;base64,e30=" },
    { name: "diagram.png", uri: "data:image/png;base64,cGljdHVyZQ==" },
  ];
  const record = await here.start().ready();
  const result = await request(record, "POST", `api/session/${session}/prompt`,
    { id: "msg_rawAttachments", text: "Check these", files }, { "x-kuvo-from": "mac" });
  assert.equal(result.status, 200, result.text);
  const [message] = here.engine.messages.get(session);
  assert.deepEqual(message.files, [{ ...files[0], uri: "data:text/plain;base64,e30=" }]);
  const paths = message.text.split("\n").filter(line => path.isAbsolute(line));
  assert.deepEqual(paths.map(file => path.basename(file)), ["data.json", "diagram.png"]);
  for (const [file, bytes] of paths.map((file, index) => [file, ["{}", "picture"][index]])) {
    assert.equal(path.dirname(path.dirname(file)), path.join(here.home, "attachments", session));
    assert.equal(fs.readFileSync(file, "utf8"), bytes);
  }
  assert.deepEqual(message.metadata.kuvo.parts, {
    words: "Check these",
    ...(path.sep === "/" ? { paths: [paths[1]] } : {}),
  });
  const retry = await request(record, "POST", `api/session/${session}/prompt`,
    { id: "msg_rawAttachments", text: "Check these", files }, { "x-kuvo-from": "mac" });
  assert.deepEqual([retry.status, retry.json], [result.status, result.json]);
  assert.equal(here.engine.asked.filter(row => row.path === `/api/session/${session}/prompt`).length, 1);
  assert.equal(fs.readdirSync(path.join(here.home, "attachments", session)).length, 2);
});

test("attachment expiry leaves fresh files and files retained for moves and transcripts", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-attachment-expiry");
  const session = "ses_now";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const attachments = path.join(here.home, "attachments");
  const old = new Date(Date.now() - 15 * 86400000);
  for (const folder of ["oldflat", "ses_old/copies", "ses_move/move-mov_kept", "ses_transcript/transcript-pictures"]) {
    const directory = path.join(attachments, folder);
    write(path.join(directory, "notes.txt"), "stored");
    fs.utimesSync(directory, old, old);
  }
  const record = await here.start().ready();
  const result = await request(record, "POST", `api/session/${session}/prompt`, {
    id: "msg_expiry", text: "Keep this.", files: [{ name: "notes.txt", uri: "data:text/plain;base64,eA==" }],
  }, { "x-kuvo-from": "mac" });
  assert.equal(result.status, 200, result.text);
  await until(() => !fs.existsSync(path.join(attachments, "oldflat")) && !fs.existsSync(path.join(attachments, "ses_old")),
    "old attachments to expire");
  assert.deepEqual(fs.readdirSync(attachments).sort(), ["ses_move", "ses_now", "ses_transcript"]);
  assert.ok(fs.existsSync(path.join(attachments, "ses_move", "move-mov_kept", "notes.txt")));
  assert.ok(fs.existsSync(path.join(attachments, "ses_transcript", "transcript-pictures", "notes.txt")));
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

test("a removal requested before a send reaches a busy engine holds that message", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-arrival-busy");
  const id = "ses_busyArrival";
  here.engine.sessions.push(sessionOf({ id, title: "Busy", directory: here.home }));
  here.engine.running.add(id);
  const record = await here.start().ready();
  const route = "kuvo/sessions/" + id;
  const remove = route + "/queue/msg_onTheWay?arriving=1";
  assert.equal((await request(record, "DELETE", remove)).status, 409);
  const send = await request(record, "POST", route + "/send", { id: "msg_onTheWay", words: "Change this" });
  assert.equal(send.status, 200);
  assert.deepEqual(here.engine.inboxes.get(id).map(row => [row.id, row.delivery]), [["msg_onTheWay", "queue"]]);
  const removed = await request(record, "DELETE", remove);
  assert.equal(removed.status, 200);
  assert.equal(removed.json.removed.words, "Change this");
  assert.deepEqual(here.engine.inboxes.get(id), []);
  assert.deepEqual(here.engine.interrupted, []);
});
