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
  const windowsPaths = ["C:\\work\\draft.txt", "\\\\share\\folder\\paper.pdf"];
  const windowsNote = ["Keep the attachments.", "", "<attached-files>",
    "Open these attached files with your tools if you need them:", ...windowsPaths,
    "C:relative.txt", "</attached-files>"].join("\n");
  const windows = await request(resident, "POST", "kuvo/messages/parts", { texts: [windowsNote] });
  assert.equal(windows.status, 200, windows.text);
  assert.deepEqual(windows.json.parts, [{ words: "Keep the attachments.", paths: windowsPaths }]);
  for (const texts of [[1], Array(501).fill("a")]) {
    const invalid = await request(resident, "POST", "kuvo/messages/parts", { texts });
    assert.equal(invalid.status, 400, invalid.text);
  }
});

test("send distinguishes your text from a replay and passes through an engine refusal", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-send-envelope");
  const target = "ses_send";
  const model = { id: "words", providerID: "local", capabilities: { input: ["text"] } };
  here.engine.sessions.push({ ...sessionOf({ id: target, directory: here.home }), model });
  const record = await here.start().ready();
  const desktop = { "x-kuvo-from": "app" };
  const send = body => request(record, "POST", "kuvo/sessions/" + target + "/send", body, desktop);
  const prompts = () => here.engine.asked.filter(row => row.method === "POST" && row.path.endsWith("/prompt"));
  for (const fields of [{ words: "  " }, { text: " " }]) assert.equal((await send(fields)).status, 400);
  const absent = await request(record, "POST", "kuvo/sessions/ses_missing/send", { words: "Here" }, desktop);
  assert.equal(absent.status, 404, absent.text);
  const unlisted = await send({ id: "msg_unknown", words: "Ask &Nova", named: ["ses_nova"] });
  assert.equal(unlisted.status, 409, unlisted.text);
  assert.match(unlisted.json.message, /isn’t known|isn't known/);
  assert.deepEqual(prompts(), []);

  const id = "msg_refused";
  const rejected = { message: "The engine refused this send.", reason: "example" };
  here.engine.refusePrompt = { id, body: rejected, status: 500 };
  const body = { id, words: "/review-quick main", keyword: "/plan", command: "review-quick" };
  const refused = await send(body);
  assert.equal(refused.status, 500, refused.text);
  assert.deepEqual(refused.json, rejected);
  assert.equal((await send(body)).status, 200);
  assert.equal((await send(body)).status, 200);
  assert.equal(prompts().length, 2);
  assert.deepEqual(prompts().map(row => [row.json.text, row.json.metadata.kuvo.parts]),
    Array(2).fill([body.words, { words: body.words }]));
  assert.equal(here.engine.asked.some(row => row.method === "POST" && row.path.endsWith("/command")), false);

  const letter = "[Message from Ada · ses_ada] (To answer, use send_message to ses_ada.)\n\nAsk again";
  const previousFile = { uri: "file:///x/a.png", name: "a.png" };
  assert.equal((await send({ id: "msg_asWritten", words: letter })).status, 200);
  assert.deepEqual(prompts().at(-1).json.metadata.kuvo.parts, { words: letter });
  assert.equal((await send({ id: "msg_replay", text: letter, files: [previousFile] })).status, 200);
  assert.deepEqual(prompts().at(-1).json, {
    id: "msg_replay", text: letter, files: [previousFile], delivery: "steer",
    metadata: { kuvo: { parts: { words: "Ask again", mail: { name: "Ada", session: "ses_ada" } } } },
  });

  const replay = await send({ id: "msg_replayImage", text: "What's this?",
    files: [{ uri: "data:image/png;base64,cG5n", name: "a.png" }] });
  assert.equal(replay.status, 200, replay.text);
  const written = prompts().at(-1).json;
  assert.ok(!written.files?.length);
  const [saved] = written.metadata.kuvo.parts.paths;
  assert.equal(fs.readFileSync(saved, "utf8"), "png");
  assert.ok(written.text.startsWith("What's this?\n\n<attached-files>\nOpen these attached files with your tools if you need them:\n"));
  assert.ok(written.text.includes(saved));
});

test("queue add preserves fields, replays, and saved parts", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-queue-envelope");
  const target = "ses_queue";
  here.engine.sessions.push(sessionOf({ id: target, directory: here.home }));
  const record = await here.start().ready();
  const add = body => request(record, "POST", "kuvo/sessions/" + target + "/queue/add", body, { "x-kuvo-from": "app" });
  const latest = () => here.engine.asked.findLast(row => row.method === "POST" && row.path.endsWith("/prompt")).json;
  const fresh = await add({ id: "msg_fresh", words: "After this", widget: "Plans", delivery: "queue" });
  assert.equal(fresh.status, 200, fresh.text);
  assert.equal(fresh.json.waits, false);
  assert.equal(latest().text, "After this\n\n<from-widget>\nThe user sent this from a widget shown in this conversation (show_widget), titled:\nPlans\n</from-widget>");
  assert.deepEqual(latest().metadata.kuvo.parts, { words: "After this", widget: "Plans" });

  const letter = "[Message from Ada · ses_ada] (To answer, use send_message to ses_ada.)\n\nThe build is green.";
  const again = await add({ id: "msg_letter", text: letter, again: true, words: "The build is green.", delivery: "queue" });
  assert.equal(again.status, 200, again.text);
  assert.equal(latest().text, letter);
  assert.deepEqual(latest().metadata.kuvo.parts, { words: "The build is green.", mail: { name: "Ada", session: "ses_ada" } });

  const parts = { words: "Check CI", widget: "Build" };
  const kept = await add({ id: "msg_kept", text: "Check CI", words: "Check CI", kept: true, metadata: { kuvo: { parts } } });
  assert.equal(kept.status, 200, kept.text);
  assert.deepEqual(latest().metadata.kuvo.parts, parts);
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

test("Delete with an earlier revision refuses a session that vanishes during the request", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-delete-revision");
  const session = sessionOf({ id: "ses_delete", directory: here.home, title: "A saved conversation" });
  session.metadata = { "kuvo.archived": true };
  here.engine.sessions.push(session);
  const record = await here.start().ready();
  const route = "kuvo/sessions/" + session.id;
  const actions = await request(record, "GET", route + "/actions");
  assert.equal(actions.status, 200, actions.text);
  const was = { updated: session.time.updated, archived: true,
    confirm: actions.json.actions.find(row => row.id === "delete").confirm };
  const remove = (id, was) => request(record, "DELETE", "kuvo/sessions/" + id, was === undefined ? undefined : { was });
  here.engine.delistOnRead = session.id;
  for (let attempt = 0; attempt < 2; attempt++) {
    const stale = await remove(session.id, was);
    assert.equal(stale.status, 409, stale.text);
    assert.equal(stale.json.message, "It changed since you read it.");
  }
  assert.equal((await remove(session.id)).status, 200);
  const restoring = { ...session, id: "ses_current" };
  here.engine.sessions.push(restoring);
  assert.equal((await remove(restoring.id, was)).status, 200);
  assert.equal(here.engine.sessions.length, 0);
});

test("a raw session patch waits for the conversation's resident writer", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-note-turns");
  const session = "ses_noteTurn";
  const unrelated = "ses_noteOther151";
  here.engine.sessions.push(
    { ...sessionOf({ id: session, directory: here.home }), metadata: { keep: true } },
    sessionOf({ id: unrelated, directory: here.home }),
  );
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "The session list did not connect.");
  let accepted, release;
  const atWrite = new Promise(resolve => { accepted = resolve; });
  const unheld = new Promise(resolve => { release = resolve; });
  here.engine.holdSessionPatch = { id: session, accepted, release: unheld };
  const resident = request(record, "POST", `kuvo/sessions/${session}/pin`, { pinned: true });
  await atWrite;
  const raw = request(record, "PATCH", `api/session/${session}`, { metadata: { changedByClient: true } });
  try {
    const other = await request(record, "POST", `kuvo/sessions/${unrelated}/rename`, { title: "Independent" }, {}, 2000);
    assert.equal(other.status, 200, other.text);
    assert.equal(other.json.info.title, "Independent");
  } finally {
    release();
  }
  const [pin, patch] = await Promise.all([resident, raw]);
  assert.equal(pin.status, 200, pin.text);
  assert.equal(patch.status, 200, patch.text);
  const writes = here.engine.asked.filter(call => call.method === "PATCH" && call.path === `/api/session/${session}`);
  assert.deepEqual(writes.map(call => call.json.metadata), [
    { keep: true, "kuvo.pinned": true }, { changedByClient: true },
  ]);
  assert.deepEqual(here.engine.sessions.find(row => row.id === session).metadata, { changedByClient: true });
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

test("queue reads keep the last complete inbox and the live line skips unreadable inboxes", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-queue-incomplete");
  const session = "ses_queue";
  const pathToQueue = "kuvo/sessions/" + session + "/queue";
  const inboxPath = "/api/session/" + session + "/inbox";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  const first = { id: "msg_first", type: "user", delivery: "steer", payload: { text: "Up next" } };
  const second = { id: "msg_second", type: "user", delivery: "queue", payload: { text: "Afterwards" } };
  here.engine.inboxes.set(session, [first, second]);
  const record = await here.start().ready();
  const reading = await openLine(record, "", pathToQueue + "/events");
  t.after(() => reading.close());
  const frames = reading.frames();
  assert.deepEqual((await frames.next()).value.items.map(item => item.id), [first.id, second.id]);
  await until(() => here.engine.subscribers() > 0, "the resident to follow the inbox");
  const next = frames.next();
  next.catch(() => {});
  const persisted = await request(record, "GET", pathToQueue);
  assert.deepEqual(persisted.json, { paused: false, held: [] });
  here.engine.inboxReplies = new Map();
  const reads = () => here.engine.asked.filter(call => call.path === inboxPath).length;
  for (const failure of [{ status: 200, body: { data: "not an inbox" } }, { status: 503, body: {} }, { status: 404, body: {} }]) {
    const before = reads();
    here.engine.inboxReplies.set(session, failure);
    here.engine.emit({ type: "session.inbox.delivery.changed", data: { sessionID: session } });
    await until(() => reads() > before, "the line to refresh its inbox");
    const response = await request(record, "GET", pathToQueue + "?items=1");
    assert.deepEqual(response.json.items.map(item => item.id), [first.id, second.id]);
    assert.deepEqual({ paused: response.json.paused, held: response.json.held }, persisted.json);
  }
  here.engine.inboxReplies.clear();
  here.engine.inboxes.set(session, [second]);
  here.engine.emit({ type: "session.inbox.delivered", data: { sessionID: session, inboxID: first.id } });
  const recovered = (await next).value;
  assert.deepEqual(recovered.items.map(item => item.id), [second.id]);
  reading.close();
  const reconnect = await openLine(record, "", pathToQueue + "/events");
  t.after(() => reconnect.close());
  assert.deepEqual((await reconnect.frames().next()).value.items.map(item => item.id), [second.id]);

  here.engine.inboxReplies.set("ses_missing", { status: 404, body: {} });
  const missing = await request(record, "GET", "kuvo/sessions/ses_missing/queue?items=1");
  assert.equal(missing.status, 200, missing.text);
  assert.deepEqual(missing.json, { paused: false, held: [], items: null });
});

test("legacy waiting queues read and future formats are refused before removal or Resume", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-queue-format");
  const session = "ses_queue";
  const id = "msg_saved";
  const base = "kuvo/sessions/" + session;
  const queue = base + "/queue";
  const app = { "x-kuvo-from": "app" };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const record = await here.start().ready();
  assert.equal((await request(record, "POST", base + "/stop", {}, app)).status, 200);
  const saved = await request(record, "POST", base + "/send", { id, words: "Keep the notes.", delivery: "queue" }, app);
  assert.deepEqual(saved.json, { waits: true });
  const payload = path.join(here.home, "run", "waiting", id + ".json");
  const index = path.join(here.home, "run", "queues.json");
  const prompt = readJSON(payload);
  const book = readJSON(index);
  assert.equal(prompt.formatVersion, 1);
  assert.equal(book[session].formatVersion, 1);
  delete prompt.formatVersion;
  delete book[session].formatVersion;
  write(payload, JSON.stringify(prompt));
  const older = JSON.stringify(book);
  write(index, older);
  await until(async () => (await request(record, "GET", queue + "?items=1")).json.items?.some(item => item.id === id),
    "the legacy waiting row to be read");

  const future = JSON.stringify({ ...prompt, formatVersion: 2 });
  write(payload, future);
  for (const [method, route] of [["DELETE", queue + "/" + id], ["POST", queue + "/resume"]]) {
    const refused = await request(record, method, route, undefined, app);
    assert.equal(refused.status, 409, refused.text);
    assert.equal(refused.json.code, "unsupportedFormatVersion");
    assert.equal(fs.readFileSync(payload, "utf8"), future);
    assert.equal(fs.readFileSync(index, "utf8"), older);
  }
  assert.deepEqual(here.engine.messages.get(session) ?? [], []);
  book[session].formatVersion = 2;
  const futureIndex = JSON.stringify(book);
  write(index, futureIndex);
  const refused = await request(record, "POST", queue + "/hold", {}, app);
  assert.equal(refused.status, 409, refused.text);
  assert.equal(refused.json.code, "unsupportedFormatVersion");
  assert.equal(fs.readFileSync(index, "utf8"), futureIndex);
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

test("an external Stop holds the queue unless the conversation started again", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-external-stop");
  const stopped = "ses_externalStop";
  const restarted = "ses_externalRestart";
  for (const session of [stopped, restarted]) {
    here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
    here.engine.inboxes.set(session, [{ id: "msg_afterPause", type: "user", delivery: "queue", payload: { text: "Next." } }]);
  }
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "The event watcher did not start.");
  here.engine.emit({ type: "session.execution.interrupted", data: { sessionID: restarted, reason: "user" } });
  here.engine.emit({ type: "session.execution.started", data: { sessionID: restarted } });
  here.engine.emit({ type: "session.execution.interrupted", data: { sessionID: stopped, reason: "user" } });
  const queue = session => request(record, "GET", `kuvo/sessions/${session}/queue`);
  await until(async () => (await queue(stopped)).json?.paused, "The external Stop did not pause the conversation.");
  assert.equal((await queue(restarted)).json.paused, false);
});

test("after a failure only your own message to steer opens the queue again", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-written-by-you");
  const session = "ses_afterFailure";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.inboxes.set(session, [
    { id: "msg_queue", type: "user", delivery: "queue", payload: { text: "Later." } },
    { id: "msg_steer", type: "user", delivery: "steer", payload: { text: "Next." } },
  ]);
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "The event watcher did not start.");
  here.engine.emit({ type: "session.execution.failed", data: { sessionID: session } });
  const queue = () => request(record, "GET", `kuvo/sessions/${session}/queue`);
  await until(async () => (await queue()).json?.held.includes("msg_steer"), "The failed turn did not hold its steer.");
  const written = (text, metadata) => here.engine.emit({
    type: "session.inbox.enqueued",
    data: { sessionID: session, item: { id: "msg_whatChanged", type: "user", delivery: "steer", payload: { text, metadata } } },
  });
  written("[Message from another session]");
  written("An automatic note.", { kuvo: { note: { from: "Kuvo Code", line: "An automatic note." } } });
  await new Promise(resolve => setTimeout(resolve, 200));
  assert.equal((await queue()).json.paused, true);
  written("Try with the new model.");
  await until(async () => (await queue()).json?.paused === false, "Your steer did not open the queue.");
});

// Deterministic provider errors for side-by-side TS and Rust comparison. The
// live queue scenario uses OpenCode Go separately.
test("a usage limit survives an inbox error and a manual Resume survives restart", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-limit-missed");
  const session = "ses_limitWithoutReset";
  const info = sessionOf({ id: session, directory: here.home, updated: Date.now() });
  here.engine.sessions.push({ ...info, outcome: "failed" });
  const error = { type: "limit", message: "The model reached its usage limit." };
  const latest = { type: "idle", outcome: "failed", error, time: { created: info.time.updated } };
  here.engine.messages.set(session, [latest]);
  here.engine.inboxUnavailable = new Set([session]);
  const route = `kuvo/sessions/${session}/queue`;
  const resident = here.start();
  let record = await resident.ready();
  await until(() => here.engine.subscribers() > 0, "The event watcher did not start.");
  here.engine.emit({ type: "session.execution.failed", data: { sessionID: session, error } });
  const file = path.join(here.home, "run", "queues.json");
  await until(() => {
    try { return readJSON(file)?.[session]?.limitStop?.message === error.message; }
    catch { return false; }
  }, "The stop was lost while the inbox was unavailable.", 6000, 50);
  assert.match((await request(record, "GET", route)).json.line, /provider didn't say when/);
  here.engine.inboxUnavailable.clear();
  assert.deepEqual((await request(record, "POST", route + "/resume")).json, { paused: false, held: [] });
  await resident.stop();
  const kept = readJSON(file);
  delete kept[session].resumed;
  write(file, JSON.stringify(kept));
  const scanFile = path.join(here.home, "run", "queues-scanned.json");
  fs.rmSync(scanFile, { force: true });
  record = await here.start().ready();
  await until(() => fs.existsSync(scanFile), "The restart did not scan queues.");
  assert.deepEqual((await request(record, "GET", route)).json, { paused: false, held: [] });
});

test("a timed usage limit retries sending the first waiting message at reset", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-limit-reset");
  const session = "ses_limitWithReset";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "The event watcher did not start.");
  const resetsAt = Date.now() + 3000;
  const error = {
    type: "limit", message: "The model reached its usage limit.",
    limit: { scope: "model", resetsAt },
  };
  here.engine.emit({ type: "session.execution.failed", data: { sessionID: session, error } });
  const route = `kuvo/sessions/${session}/queue`;
  await until(async () => {
    const queue = await request(record, "GET", route);
    return queue.json?.paused && queue.json?.line.includes("It resets at ");
  }, "The queue did not show its timed stop.", 2000, 50);
  const id = "msg_waitForReset";
  let firstAttempt;
  let refusedAttempt;
  const refused = new Promise(resolve => { refusedAttempt = resolve; });
  here.engine.refusePrompt = {
    id,
    seen: () => {
      firstAttempt = Date.now();
      refusedAttempt();
    },
  };
  const accepted = await request(record, "POST", route + "/add", { id, text: "After reset." });
  assert.equal(accepted.json.waits, true, accepted.text);
  assert.equal(firstAttempt, undefined);
  await refused;
  const file = path.join(here.home, "run", "queues.json");
  assert.equal(readJSON(file)[session].limitStop.releasing, true);
  const later = await request(record, "POST", route + "/add", { id: "msg_afterFirst", text: "After the first message." });
  assert.equal(later.json.waits, true, later.text);
  await until(() => here.engine.messages.get(session)?.some(row => row.id === id), "The queue did not retry after the prompt was refused.", 10000, 50);
  assert.ok(firstAttempt >= resetsAt);
  assert.equal(here.engine.messages.get(session).filter(row => row.id === id).length, 1);
  await until(async () => {
    const done = await request(record, "GET", route);
    return done.json?.paused === false &&
      done.json.waiting?.map(row => row.id).join() === "msg_afterFirst";
  }, "Only the second message should still be waiting.");
});

test("a missed usage limit is recovered from the transcript and can be resumed", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-limit-scan");
  const session = "ses_limitScan";
  const at = Date.now();
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home, updated: at }));
  here.engine.inboxes.set(session, [{ id: "msg_limitHead", type: "user", delivery: "queue", payload: { text: "Next" } }]);
  const error = { type: "provider.rate-limit", message: "Rate limit reached. Try again in 2h 30m." };
  here.engine.messages.set(session, [{ type: "idle", outcome: "failed", error, time: { created: at } }]);
  write(path.join(here.home, "box", "time-zone.json"), JSON.stringify({ zone: "Pacific/Honolulu" }));
  const record = await here.start().ready();
  const route = `kuvo/sessions/${session}/queue`;
  const resetsAt = at + 2.5 * 3_600_000;
  const time = value => new Intl.DateTimeFormat("en-US", { timeZone: "Pacific/Honolulu", hour: "numeric", minute: "2-digit" }).format(value);
  const day = value => new Intl.DateTimeFormat("en-US", { timeZone: "Pacific/Honolulu", month: "short", day: "numeric" }).format(value);
  const expected = `${error.message} It resets at ${day(at) === day(resetsAt) ? "" : day(resetsAt) + ", "}${time(resetsAt)}.`;
  await until(async () => (await request(record, "GET", route)).json?.line === expected, expected);
  assert.deepEqual((await request(record, "POST", route + "/resume")).json, { paused: false, held: [] });
  assert.equal(here.engine.inboxes.get(session)[0].delivery, "steer");
});

test("a limit reset waits for an unfinished removal before advancing the queue", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-limit-removal");
  const session = "ses_limitRemoval";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const items = ["msg_beingRemoved", "msg_nextAfterLimit"];
  here.engine.inboxes.set(session, items.map(id => ({ id, type: "user", delivery: "queue", payload: { text: id } })));
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "The event watcher did not start.");
  const resetsAt = Date.now() + 2500;
  const error = { type: "limit", message: "Usage limit.", limit: { scope: "model", resetsAt } };
  here.engine.emit({ type: "session.execution.failed", data: { sessionID: session, error } });
  const route = `kuvo/sessions/${session}/queue`;
  const file = path.join(here.home, "run", "queues.json");
  await until(() => {
    try { return readJSON(file)?.[session]?.limitStop?.resetsAt === resetsAt; }
    catch { return false; }
  }, "The queue did not record the limit reset.", 1500, 25);
  here.engine.refuseRemoval = new Set([items[0]]);
  const ticket = route + "/" + items[0] + "?receipt=msg_removalTicket";
  assert.equal((await request(record, "DELETE", ticket)).status, 502);
  await until(() => readJSON(file)[session]?.limitStop?.releasing, "The recorded limit did not reset.", 6000, 50);
  assert.deepEqual(here.engine.inboxes.get(session).map(row => row.delivery), ["queue", "queue"]);
  here.engine.refuseRemoval.clear();
  assert.equal((await request(record, "DELETE", ticket)).status, 200);
  await until(() => here.engine.inboxes.get(session)[0]?.delivery === "steer", "The queue did not advance after removal.", 6000, 50);
  assert.deepEqual(here.engine.inboxes.get(session).map(row => row.id), [items[1]]);
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

test("Stop holds a named send before its composed attachment reaches OpenCode", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("stop-slow-attachment");
  const session = "ses_late";
  const route = "kuvo/sessions/" + session;
  const theId = "msg_lateFile";
  const headers = { "x-kuvo-from": "app" };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  const resident = await here.start().ready();
  const entered = Promise.withResolvers();
  const release = Promise.withResolvers();
  let reads = 0;
  here.engine.holdSessionRead = {
    when: id => id === session && ++reads === 2,
    accepted: entered.resolve,
    release: release.promise,
  };
  t.after(() => release.resolve());
  const send = request(resident, "POST", route + "/send", {
    id: theId, words: "See the attachment.", delivery: "steer",
    files: [{ uri: "data:image/png;base64,UE5H", name: "evidence.png" }],
  }, headers);
  await entered.promise;
  const stopping = request(resident, "POST", route + "/stop", { items: [theId] }, headers);
  try {
    await until(() => {
      try { return readJSON(path.join(here.home, "run", "queues.json"))?.[session]?.arriving?.[theId]; }
      catch { return false; }
    }, "Stop records the named send");
    release.resolve();
    const stopped = await stopping;
    assert.equal(stopped.status, 200, stopped.text);
    assert.equal(stopped.json.paused, true);
  } finally {
    release.resolve();
  }
  const sent = await send;
  assert.equal(sent.status, 200, sent.text);
  assert.deepEqual(sent.json, { waits: true });
  assert.equal(here.engine.asked.some(call => call.method === "POST" && call.path.endsWith("/prompt")), false);
  const queue = await request(resident, "GET", route + "/queue?items=1");
  assert.deepEqual(queue.json.held, [theId]);
  assert.deepEqual(queue.json.waiting.map(row => row.id), [theId]);
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

test("a bot's session only shows sustained reasoning and clears it after the tool starts", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-session-thinking");
  const bot = "bot_reasoning";
  const directory = path.join(here.home, "bots", bot);
  const id = "ses_reasoning";
  write(path.join(directory, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Reader" }));
  write(path.join(directory, "session"), id);
  here.engine.sessions.push(sessionOf({ id, directory }));
  here.engine.running.add(id);
  const record = await here.start().ready();
  const facts = async () => {
    const list = await request(record, "GET", "kuvo/sessions/list");
    assert.equal(list.status, 200, list.text);
    return list.json.sessions[id];
  };
  await until(() => here.engine.subscribers() > 0, "the resident to follow reasoning events");
  assert.equal((await facts()).thinking, undefined);
  const say = type => here.engine.emit({ type, data: { sessionID: id } });
  say("session.reasoning.started");
  say("session.tool.input.started");
  await new Promise(resolve => setTimeout(resolve, 2200));
  assert.equal((await facts()).thinking, undefined);

  const began = performance.now();
  say("session.reasoning.started");
  await until(async () => (await facts()).thinking === true, "the long stretch of reasoning to show");
  assert.ok(performance.now() - began >= 1900, "Thinking appeared before two seconds of reasoning.");
  say("session.text.started");
  say("session.reasoning.started");
  assert.equal((await facts()).thinking, true);
  const stopped = performance.now();
  say("session.tool.called");
  await until(async () => (await facts()).thinking === undefined, "the tool to clear reasoning");
  assert.ok(performance.now() - stopped >= 1400, "Thinking cleared before the grace period ended.");
  assert.equal((await facts()).running, true);
});

test("the session list starts in the most recent project and orders ties by name", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-list-project-order");
  const catalog = [
    { directory: path.join(here.home, "aa"), name: "Project 10", updated: 42 },
    { directory: path.join(here.home, "zz"), name: "project 002", updated: 42 },
    { directory: path.join(here.home, "bb"), name: "First by name", updated: 1 },
  ];
  for (const [index, project] of catalog.entries()) {
    fs.mkdirSync(project.directory, { recursive: true });
    here.engine.sessions.push({ ...sessionOf({ id: "ses_project" + index, directory: project.directory }),
      time: { created: 1, updated: project.updated } });
  }
  write(path.join(here.home, "box", "projects.json"), JSON.stringify({ projects: catalog.map(project => ({
    directory: project.directory, name: project.name, folders: [],
  })) }));
  const record = await here.start().ready();
  const list = await request(record, "GET", "kuvo/sessions/list");
  assert.equal(list.status, 200, list.text);
  assert.deepEqual(list.json.projectOrder, [catalog[1].directory, catalog[0].directory, catalog[2].directory]);
  assert.equal(list.json.startProject, catalog[1].directory);
});

test("a session list saved before Needs you is read while OpenCode catches up", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-list-older-asks");
  const id = "ses_old";
  const project = path.join(here.home, "project");
  write(path.join(here.home, "run", "lists", "sessions.json"), JSON.stringify({
    epoch: "old", seq: 3, floor: 0, gone: {}, projects: { [project]: { updated: 2 } },
    sessions: { [id]: { seq: 1, facts: { kind: "project", directory: project, project, created: 1, updated: 2, asked: 5 } } },
  }));
  const gate = Promise.withResolvers();
  here.engine.holdSessionList = gate.promise;
  t.after(() => gate.resolve());
  const record = await here.start().ready();
  const read = await request(record, "GET", "kuvo/sessions/list");
  assert.equal(read.status, 200, read.text);
  assert.equal(read.json.full, true);
  assert.deepEqual(read.json.projects, { [project]: { updated: 2, asking: 0, working: 0 } });
  assert.deepEqual(read.json.needsYou, [{ session: id, asks: { kind: "other", summary: "waits on you" } }]);
  assert.equal((await request(record, "POST", "kuvo/sessions/list", {})).status, 405);
});

test("the first inventory never publishes a conversation deleted while its page was held", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-list-inventory-race");
  const gone = sessionOf({ id: "ses_vanishing", directory: here.home, title: "Old" });
  const kept = sessionOf({ id: "ses_present", directory: here.home, title: "Here" });
  here.engine.sessions.push(gone, kept);
  const gate = Promise.withResolvers();
  here.engine.holdSessionList = gate.promise;
  t.after(() => gate.resolve());
  const record = await here.start().ready();
  const reading = request(record, "GET", "kuvo/sessions/list", undefined, {}, 15000);
  await until(() => here.engine.subscribers() > 0 && here.engine.heldInventories > 0,
    "the resident to begin its first inventory and subscribe to events");
  here.engine.sessions.shift();
  here.engine.emit({ type: "session.deleted", data: { sessionID: gone.id } });
  const form = { id: "form_afterDelete", sessionID: kept.id, title: "After deletion", fields: [] };
  here.engine.forms.set(kept.id, [form]);
  here.engine.emit({ type: "form.created", data: { form } });
  const asks = path.join(here.home, "run", "glance", "asks.json");
  await until(() => fs.existsSync(asks) && readJSON(asks).asks[form.id], "the resident to hear the deletion");
  here.engine.holdSessionList = null;
  gate.resolve();
  const initial = await reading;
  assert.equal(initial.status, 200, initial.text);
  assert.equal(initial.json.full, true);
  assert.deepEqual(Object.keys(initial.json.sessions), [kept.id]);
  assert.equal(initial.json.gone, undefined);
});

test("session list cursors carry changes and tombstones, then read whole after a restart", { timeout: 60000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-list-cursors");
  const [gone, kept] = ["ses_cursorGone", "ses_cursorKept"];
  here.engine.sessions.push(
    sessionOf({ id: gone, directory: here.home, title: "To delete" }),
    sessionOf({ id: kept, directory: here.home, title: "Before" }),
  );
  const first = here.start();
  let record = await first.ready();
  const list = since => request(record, "GET", "kuvo/sessions/list" + (since ? "?since=" + encodeURIComponent(since) : ""));
  const initial = await openLine(record, "", "kuvo/sessions/list/events");
  const newest = (await initial.frames().next()).value;
  initial.close();
  assert.equal(newest.full, true);
  assert.deepEqual(Object.keys(newest.sessions).sort(), [gone, kept]);
  assert.equal(newest.pending, undefined);
  assert.equal((await request(record, "POST", `kuvo/sessions/${kept}/rename`, { title: "After" })).status, 200);
  const changed = await until(async () => {
    const answer = await list(newest.at);
    return answer.json?.sessions?.[kept]?.title === "After" && answer.json;
  }, "The recorded cursor did not receive the session change.");
  assert.equal(changed.full, undefined);
  assert.deepEqual(Object.keys(changed.sessions), [kept]);
  assert.equal((await request(record, "DELETE", `kuvo/sessions/${gone}`)).status, 200);
  const removed = await until(async () => {
    const answer = await list(changed.at);
    return answer.json?.gone?.includes(gone) && answer.json;
  }, "The recorded cursor did not receive its tombstone.");
  assert.deepEqual(removed.sessions, {});
  assert.deepEqual((await list(removed.at)).json.sessions, {});
  for (const since of ["another.1", removed.at.split(".")[0] + ".999999999", "unreadable"]) {
    const whole = (await list(since)).json;
    assert.equal(whole.full, true);
    assert.deepEqual(Object.keys(whole.sessions), [kept]);
    assert.equal(whole.gone, undefined);
  }

  await first.stop();
  const added = "ses_cursorNew";
  here.engine.sessions.push(sessionOf({ id: added, directory: here.home, title: "Since restart" }));
  let release;
  here.engine.holdSessionList = new Promise(resolve => { release = resolve; });
  t.after(release);
  record = await here.start().ready();
  const followed = await openLine(record, "", "kuvo/sessions/list/events");
  assert.equal(followed.status, 200);
  const snapshots = followed.frames();
  try {
    const saved = (await snapshots.next()).value;
    assert.equal(saved.full, true);
    assert.deepEqual(saved.pending, ["engine"]);
    assert.deepEqual(Object.keys(saved.sessions), [kept]);
    here.engine.holdSessionList = null;
    release();
    const live = (await snapshots.next()).value;
    assert.equal(live.pending, undefined);
    assert.equal(live.sessions[added].title, "Since restart");
  } finally {
    followed.close();
  }
  const current = await until(async () => {
    const answer = await list(removed.at);
    return answer.json?.full && !answer.json?.pending?.length && answer.json;
  }, "The old process's cursor did not receive a new whole list.");
  assert.deepEqual(Object.keys(current.sessions).sort(), [kept, added]);
});

test("a project query accepts a Windows key while route paths keep their separator guard", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("project-page-query");
  const record = await here.start().ready();
  const route = "kuvo/sessions/page?project=";
  const key = "C:\\Kuvo\\projects\\Example";
  const page = await request(record, "GET", route + encodeURIComponent(key));
  assert.equal(page.status, 200, page.text);
  assert.deepEqual(page.json, { project: key, sessions: [], total: 0, more: null });
  for (const badKey of ["C:\\Kuvo\\..\\Example", "/Kuvo/../Example"]) {
    assert.equal((await request(record, "GET", route + encodeURIComponent(badKey))).status, 400);
  }
  for (const segment of ["%2E%2E", "sessions%2Fpage", "sessions%5Cpage"]) {
    assert.equal((await request(record, "GET", "kuvo/" + segment + "/page")).status, 403);
  }
  assert.equal((await request(record, "GET", "api/fs/list?path=Kuvo%2F..")).status, 403);
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

test("Undo restores a message between its engine neighbors when the queue is paused", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-undo-order");
  const session = "ses_undoOrder";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.inboxes.set(session, ["msg_a", "msg_b", "msg_c"].map(id => ({
    id, type: "user", delivery: "queue", payload: { text: id },
  })));
  const record = await here.start().ready();
  const route = `kuvo/sessions/${session}/queue`;
  assert.equal((await request(record, "POST", route + "/hold")).json.paused, true);
  const { json: { removed } } = await request(record, "DELETE", route + "/msg_b?receipt=msg_undoMiddle");
  assert.equal(removed.before, "msg_c");
  const undo = await request(record, "POST", route + "/add", { ...removed, kept: true });
  assert.equal(undo.json.waits, true, undo.text);
  assert.deepEqual(undo.json.held, ["msg_a", "msg_b"]);
  let items;
  await until(async () => {
    items = (await request(record, "GET", route + "?items=1")).json?.items;
    return Array.isArray(items);
  }, "The queue did not publish its items.");
  assert.deepEqual(items.map(({ id, on }) => [id, on]), [
    ["msg_a", "engine"], ["msg_b", "box"], ["msg_c", "engine"],
  ]);
  assert.equal((await request(record, "POST", route + "/resume")).status, 200);
  assert.deepEqual(here.engine.inboxes.get(session).map(({ id, delivery }) => [id, delivery]), [
    ["msg_a", "steer"], ["msg_c", "queue"],
  ]);
  here.engine.inboxes.get(session).shift();
  here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: session } });
  await until(() => here.engine.messages.get(session)?.some(row => row.id === "msg_b"), "The restored message did not go second.");
  assert.equal(here.engine.inboxes.get(session)[0].delivery, "queue");
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
    paths: [paths[1]],
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
