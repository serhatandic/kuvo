// Session and message behavior through the selected resident and its engine API.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, openLine, readJSON, request, until, write } from "./adapter.mjs";
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
  const draft = { id: "msg_fresh", words: "After this", widget: "Plans", delivery: "queue" };
  const fresh = await add(draft);
  assert.equal(fresh.status, 200, fresh.text);
  assert.equal(fresh.json.waits, false);
  assert.equal(latest().delivery, "queue");
  assert.equal(latest().text, "After this\n\n<from-widget>\nThe user sent this from a widget shown in this conversation (show_widget), titled:\nPlans\n</from-widget>");
  assert.deepEqual(latest().metadata.kuvo.parts, { words: "After this", widget: "Plans" });
  const retry = await add(draft);
  assert.equal(retry.status, 200, retry.text);
  assert.equal(retry.json.waits, false);
  assert.equal(here.engine.asked.filter(row => row.method === "POST" && row.path.endsWith("/prompt") && row.json?.id === draft.id).length, 1);

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

test("queue Add distinguishes a slow OpenCode lookup from a missing conversation", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-slow-engine");
  const id = "ses_slowLookup";
  const resident = await here.start().ready();
  const queued = { id: "msg_slowLookup", text: "Check the notes." };
  const add = session => request(resident, "POST", `kuvo/sessions/${session}/queue/add`, queued);
  assert.equal((await add("ses_missing")).status, 404);
  here.engine.unavailableSession = id;
  const busy = await add(id);
  assert.equal(busy.status, 504, busy.text);
  assert.equal(busy.json.busy, true);
  assert.match(busy.json.message, /engine didn't answer in time/);
  assert.equal(here.engine.asked.filter(call => call.method === "POST" && call.path.endsWith("/prompt")).length, 0);
  here.engine.unavailableSession = null;
  here.engine.sessions.push(sessionOf({ id, directory: here.home }));
  const seen = Promise.withResolvers();
  const release = Promise.withResolvers();
  t.after(release.resolve);
  here.engine.holdSessionRead = { when: session => session === id, accepted: seen.resolve, release: release.promise };
  const adding = add(id);
  adding.catch(() => {});
  await seen.promise;
  release.resolve();
  const accepted = await adding;
  assert.equal(accepted.status, 200, accepted.text);
  assert.equal(accepted.json.waits, false);
  assert.deepEqual(here.engine.messages.get(id).map(row => row.id), [queued.id]);
  await here.engine.close();
  const gone = await add("ses_offlineEngine");
  assert.equal(gone.status, 503, gone.text);
  assert.deepEqual(gone.json, { message: "The engine isn't running." });
});

test("a stopped queue refuses Add when the payload cannot be saved and accepts a clean retry", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-save-failure");
  const session = "ses_saveQueue";
  const id = "msg_saveQueue";
  const route = `kuvo/sessions/${session}`;
  const queue = route + "/queue";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const resident = await here.start().ready();
  assert.equal((await request(resident, "POST", route + "/stop")).status, 200);
  const file = path.join(here.home, "run", "waiting", id + ".json");
  fs.mkdirSync(file, { recursive: true });
  const add = () => request(resident, "POST", queue + "/add", { id, words: "Save it." }, { "x-kuvo-from": "app" });
  const failed = await add();
  assert.equal(failed.status, 500, failed.text);
  assert.match(failed.json.message, /couldn.t save the message/);
  const saved = path.join(here.home, "run", "queues.json");
  assert.deepEqual(readJSON(saved)[session].waiting ?? [], []);
  assert.equal(here.engine.asked.some(call => call.method === "POST" && call.path.endsWith("/prompt")), false);
  fs.rmdirSync(file);
  const retried = await add();
  assert.equal(retried.status, 200, retried.text);
  assert.equal(retried.json.waits, true);
  assert.deepEqual(readJSON(saved)[session].waiting.map(row => row.id), [id]);
  assert.equal(readJSON(file).text, "Save it.");
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

test("concurrent session note writes keep earlier notes and a failed writer releases its turn", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("session-note-turns");
  const id = "ses_notes";
  const other = "ses_otherNotes";
  const initial = { pinned: "another owner’s note", kuvo: { approval: "auto", startedBy: "ses_first" } };
  const note = { ...sessionOf({ id, directory: here.home }), metadata: initial };
  here.engine.sessions.push(note, sessionOf({ id: other, directory: here.home }));
  const resident = await here.start().ready();
  const edit = (id, field, value) => request(resident, "POST",
    `kuvo/sessions/${id}/${field === "pinned" ? "pin" : "archive"}`, { [field]: value });
  const first = Promise.withResolvers();
  const release = Promise.withResolvers();
  t.after(release.resolve);
  here.engine.holdSessionPatch = { id, accepted: first.resolve, release: release.promise, status: 500 };
  const fails = edit(id, "pinned", true);
  fails.catch(() => {});
  await first.promise;
  const succeeds = edit(id, "archived", true);
  succeeds.catch(() => {});
  assert.equal((await edit(other, "pinned", true)).status, 200);
  assert.deepEqual(note.metadata, initial);
  release.resolve();
  assert.equal((await fails).status, 502);
  assert.equal((await succeeds).status, 200);
  assert.deepEqual(note.metadata, { ...initial, "kuvo.archived": true });
  const next = await edit(id, "pinned", true);
  assert.equal(next.status, 200, next.text);
  assert.deepEqual(note.metadata, { ...initial, "kuvo.archived": true, "kuvo.pinned": true });
  const changed = await Promise.all([edit(id, "pinned", false), edit(id, "archived", false)]);
  assert.deepEqual(changed.map(answer => answer.status), [200, 200]);
  assert.deepEqual(note.metadata, { ...initial, "kuvo.archived": false, "kuvo.pinned": false });
  const writes = () => here.engine.asked.filter(row => row.method === "PATCH" && row.path.endsWith("/" + id)).length;
  assert.equal(writes(), 5);
  assert.equal((await edit(id, "pinned", false)).status, 200);
  assert.equal((await edit("ses_absent", "pinned", true)).status, 404);
  assert.equal(writes(), 5);
});

test("session note writers share older lock paths and remove only stale locks", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("compatible-session-note-locks");
  const session = "ses_noteLock";
  const other = "ses_otherNoteLock";
  const notes = path.join(here.home, "run", "notes");
  const lock = id => path.join(notes, createHash("sha256").update(id).digest("hex").slice(0, 32) + ".lock");
  const live = lock(session);
  const stale = lock("ses_staleLock");
  const old = path.join(here.home, "run", "work", ".notes.lock");
  for (const file of [live, stale]) fs.mkdirSync(file, { recursive: true });
  const then = new Date(Date.now() - 90000);
  fs.utimesSync(stale, then, then);
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }),
    sessionOf({ id: other, directory: here.home }));
  const record = await here.start().ready();
  await until(() => !fs.existsSync(stale), "the startup check to clear abandoned session locks", 4000);
  assert.equal(fs.existsSync(live), true);
  const rename = (id, title) => request(record, "POST", `kuvo/sessions/${id}/rename`, { title });
  const touches = () => here.engine.asked.filter(row => row.path === "/api/session/" + session).length;
  const before = touches();
  const first = rename(session, "After the other writer");
  first.catch(() => {});
  try {
    assert.equal((await rename(other, "Independent writer")).status, 200);
    await new Promise(resolve => setTimeout(resolve, 150));
    // TypeScript's route can look up the session before entering the current writer's turn.
    assert.equal(here.engine.asked.some(row => row.method === "PATCH" && row.path.endsWith("/" + session)), false);
    assert.ok(touches() <= before + 1);
  } finally {
    fs.rmdirSync(live);
  }
  assert.equal((await first).status, 200);
  fs.mkdirSync(old, { recursive: true });
  const second = rename(session, "After the older writer");
  second.catch(() => {});
  try {
    await new Promise(resolve => setTimeout(resolve, 150));
    assert.equal(here.engine.sessions.find(row => row.id === session).title, "After the other writer");
    assert.equal(fs.existsSync(old), true);
  } finally {
    fs.rmdirSync(old);
  }
  assert.equal((await second).status, 200);
  assert.equal(here.engine.sessions.find(row => row.id === session).title, "After the older writer");
  assert.equal(fs.existsSync(live), false);
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
  assert.equal(removed.json.removed.delivery, "queue");
  assert.equal(removed.json.removed.id, "msg_edit");
  assert.equal(removed.json.removed.before, "msg_drop");
  assert.equal(removed.json.removed.receipt, "msg_undo");
  assert.equal((await request(resident, "DELETE", path + "/queue/msg_drop")).status, 200);
  assert.equal((await request(resident, "DELETE", path + "/queue/msg_missing")).status, 404);
  assert.equal((await send("msg_changed", "edited words")).status, 200);
  assert.deepEqual(here.engine.inboxes.get(session).map(row => [row.id, row.payload.metadata.kuvo.parts.words]),
    [["msg_first", "first"], ["msg_changed", "edited words"]]);
});

test("a stopped queue shows messages in order and Edit returns carried and retained files", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-edit-files");
  const session = "ses_editFiles";
  const id = "msg_editFiles";
  const route = `kuvo/sessions/${session}`;
  const queue = route + "/queue";
  const directory = here.home;
  const words = "Look at both attachments.";
  const picture = { name: "shot.png", uri: "data:image/png;base64,YQ==" };
  const zip = { name: "logs.zip", uri: "data:application/zip;base64,UEsDBA==" };
  const attached = { uri: "file:///team/held.png", name: "held.png" };
  const held = { id: "msg_heldFile", type: "user", delivery: "steer", payload: { text: "First.", files: [attached] } };
  const mail = { id: "msg_mail", type: "user", delivery: "queue", payload: {
    text: "Check this.", metadata: { kuvo: { parts: { words: "Check this.", mail: { name: "Mira", session: "ses_mira" } } } },
  } };
  const model = { id: "picture-reader", providerID: "local", capabilities: { input: ["text", "image"] } };
  here.engine.sessions.push({ ...sessionOf({ id: session, directory }), model });
  here.engine.inboxes.set(session, [held, mail]);
  here.engine.running.add(session);
  const resident = await here.start().ready();
  assert.equal((await request(resident, "POST", queue + "/hold")).json.paused, true);
  for (const invalid of [route + "/send", queue + "/add"]) {
    const refusal = await request(resident, "POST", invalid, { id: "msg_bad_id", text: "Keep it.", kept: true, delivery: "queue" });
    assert.equal(refusal.status, 400, refusal.text);
    assert.match(refusal.json.message, /Send it again as a new message/);
  }
  assert.equal(fs.existsSync(path.join(here.home, "run", "waiting", "msg_bad_id.json")), false);
  const send = await request(resident, "POST", queue + "/add", { id, words, files: [picture, zip] }, { "x-kuvo-from": "app" });
  assert.equal(send.status, 200, send.text);
  assert.equal(send.json.waits, true);
  const found = await until(async () => {
    const response = await request(resident, "GET", queue + "?items=1");
    return response.json?.items?.length === 3 && response.json;
  }, "the queue to show the two engine messages and the saved message");
  assert.deepEqual(found.held, [held.id]);
  assert.deepEqual(found.items.slice(0, 2), [
    { id: held.id, words: "First.", files: [attached.name], from: null, on: "engine", delivery: "queue", held: true },
    { id: mail.id, words: "Check this.", files: [], from: "Mira", on: "engine", delivery: "queue", held: false },
  ]);
  assert.deepEqual(found.items[2], { id, words, files: [picture.name], from: null, on: "box", delivery: "queue", held: false });
  const editHeld = await request(resident, "DELETE", queue + "/" + held.id + "?receipt=msg_editHeld");
  assert.equal(editHeld.status, 200, editHeld.text);
  assert.deepEqual(editHeld.json.removed.attached, [attached]);
  assert.equal(editHeld.json.removed.before, mail.id);
  assert.deepEqual(editHeld.json.held, []);
  const edit = await request(resident, "DELETE", queue + "/" + id + "?receipt=msg_editBoth");
  assert.equal(edit.status, 200, edit.text);
  assert.equal(edit.json.removed.draft, words);
  assert.deepEqual(edit.json.removed.files, [picture]);
  assert.deepEqual(edit.json.removed.attached.map(file => file.name), ["shot.png", "logs.zip"]);
  assert.equal(edit.json.removed.attached[0].uri, picture.uri);
  assert.deepEqual(fs.readFileSync(new URL(edit.json.removed.attached[1].uri)), Buffer.from("PK\x03\x04"));
  for (const [item, receipt] of [[held.id, "msg_editHeld"], [id, "msg_editBoth"]]) {
    assert.equal((await request(resident, "POST", queue + "/" + item + "/ack", { receipt })).status, 200);
    assert.equal((await request(resident, "DELETE", queue + "/" + item + "?receipt=" + receipt)).status, 404);
  }
});

test("wake-up rows show the same saved summary in the engine or the resident queue", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-wake-summary");
  const session = "ses_wakeSummary";
  const words = "Check everything. More instructions here.";
  const text = "[Wake-up] " + words;
  const summary = "Check the release";
  const metadata = { kuvo: { wake: { summary } } };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.inboxes.set(session, [{ id: "msg_engineWake", type: "user", delivery: "queue", payload: { text, metadata } }]);
  const record = await here.start().ready();
  const route = `kuvo/sessions/${session}/queue`;
  assert.equal((await request(record, "POST", route + "/hold")).json.paused, true);
  const add = await request(record, "POST", route + "/add", { id: "msg_boxWake", text, metadata, words, from: "Wake-up", kept: true });
  assert.equal(add.json.waits, true);
  const items = await until(async () => {
    const reply = await request(record, "GET", route + "?items=1");
    return reply.json?.items?.length === 2 && reply.json.items;
  }, "both wake-up rows to be listed");
  assert.deepEqual(items.map(row => [row.from, row.on, row.words]), [["Wake-up", "engine", summary], ["Wake-up", "box", summary]]);
  const taken = await request(record, "DELETE", route + "/msg_boxWake");
  assert.equal(taken.json.removed.text, text);
});

test("a standalone summary keeps a read conversation read while actual replies stay unread", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("summary-read");
  const idle = created => ({ type: "idle", time: { created }, outcome: "succeeded" });
  const compact = { type: "compaction", status: "completed", reason: "manual" };
  const reply = { type: "assistant", content: [] };
  const cases = [
    ["ses_read", 2000, [idle(3000), compact, idle(1000)]],
    ["ses_unread", 500, [idle(3000), compact, idle(1000)]],
    ["ses_laterReply", 2000, [idle(3000), reply, compact, idle(1000)]],
    ["ses_sameTurn", 2000, [idle(3000), compact, reply, { type: "user", text: "Go." }, idle(1000)]],
  ];
  for (const [id, viewed, messages] of cases) {
    const session = sessionOf({ id, directory: here.home });
    Object.assign(session.time, { viewed, idle: 3000 });
    here.engine.sessions.push(session);
    here.engine.messages.set(id, messages);
  }
  await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow completed summaries");
  const measured = new Set();
  here.engine.messagePageRead = (id, query) => {
    if (query.get("limit") === "6") measured.add(id);
  };
  for (const [sessionID] of cases) {
    here.engine.emit({ type: "session.execution.succeeded", data: { sessionID } });
  }
  await until(() => measured.size === cases.length && here.engine.sessions[0].time.viewed === 3000,
    "the resident to preserve the read marker through the standalone summary");
  assert.deepEqual(here.engine.sessions.map(session => session.time.viewed), [3000, 500, 2000, 2000]);
  assert.deepEqual(here.engine.asked.filter(call => call.path.endsWith("/view")).map(call => [call.path, call.json]), [
    ["/api/session/ses_read/view", { idle: 3000 }],
  ]);
});

for (const unreadable of ["inbox", "transcript"]) {
  test(`Edit recovers an inbox draft after the ${unreadable} read fails`, { timeout: 45000 }, async t => {
    const { computer } = await bench(t);
    const here = await computer("queue-edit-recovery");
    const session = "ses_editRecovery";
    const id = "msg_editRecovery";
    const words = "Keep these notes.";
    const file = { uri: "file:///team/notes.png", name: "notes.png" };
    const metadata = { kuvo: { parts: { words } } };
    const queue = `kuvo/sessions/${session}/queue`;
    const edit = queue + "/" + id + "?receipt=msg_recoverEdit";
    const payload = { text: words + "\n\n<attached-files>\n- /team/notes.png\n</attached-files>", files: [file], metadata };
    const after = { id: "msg_afterEdit", type: "user", delivery: "queue", payload: { text: "Next." } };
    here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
    here.engine.running.add(session);
    here.engine.inboxes.set(session, [{ id, type: "user", delivery: "steer", payload }, after]);
    here.engine.inboxUnavailable = new Set();
    here.engine.messageUnavailable = new Set();
    here.engine.afterInboxRemoval = () => {
      if (unreadable === "inbox") here.engine.inboxUnavailable.add(session);
      else here.engine.messageUnavailable.add(id);
    };
    const first = here.start();
    const record = await first.ready();
    const blocked = await request(record, "DELETE", edit);
    assert.equal(blocked.status, 502, blocked.text);
    assert.equal(blocked.json.removed, undefined);
    assert.deepEqual(here.engine.inboxes.get(session), [after]);
    await first.stop();

    const next = await here.start().ready();
    const checking = await request(next, "DELETE", edit);
    assert.equal(checking.status, 502, checking.text);
    here.engine.inboxUnavailable.clear();
    here.engine.messageUnavailable.clear();
    const recovered = await request(next, "DELETE", edit);
    assert.equal(recovered.status, 200, recovered.text);
    assert.deepEqual(recovered.json.removed, {
      id, text: payload.text, words, draft: words, delivery: "steer",
      files: [file], attached: [file], metadata, before: after.id, receipt: "msg_recoverEdit",
    });
    assert.deepEqual((await request(next, "DELETE", edit)).json, recovered.json);
    assert.equal(here.engine.asked.filter(row => row.method === "DELETE" && row.path.endsWith("/" + id)).length, 1);
    const ack = await request(next, "POST", queue + "/" + id + "/ack", { receipt: "msg_recoverEdit" });
    assert.equal(ack.status, 200, ack.text);
    const spent = path.join(here.home, "run", "queue-removals", "spent");
    const [receipt] = fs.readdirSync(spent);
    assert.equal(fs.readFileSync(path.join(spent, receipt)).length, 0);
    assert.equal((await request(next, "DELETE", edit)).status, 404);
  });
}

test("Edit refuses an inbox draft when delivery wins and the exact lookup fails", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-edit-delivered");
  const session = "ses_editDelivered";
  const id = "msg_editDelivered";
  const edit = `kuvo/sessions/${session}/queue/${id}?receipt=msg_deliveredEdit`;
  const payload = { text: "Already delivered." };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  here.engine.inboxes.set(session, [{ id, type: "user", delivery: "queue", payload }]);
  here.engine.afterInboxRemoval = () => {
    here.engine.messages.set(session, [{ id, type: "user", ...payload }]);
    here.engine.messageUnavailable = new Set([id]);
  };
  const resident = await here.start().ready();
  const blocked = await request(resident, "DELETE", edit);
  assert.equal(blocked.status, 502, blocked.text);
  assert.equal(blocked.json.removed, undefined);
  here.engine.messageUnavailable.clear();
  for (const expected of [404, 404]) {
    const delivered = await request(resident, "DELETE", edit);
    assert.equal(delivered.status, expected, delivered.text);
    assert.equal(delivered.json.removed, undefined);
  }
  assert.deepEqual(here.engine.messages.get(session), [{ id, type: "user", ...payload }]);
});

test("Edit distinguishes an already delivered message from an inbox that refuses removal", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-edit-refused");
  const session = "ses_editRefused";
  const queue = `kuvo/sessions/${session}/queue`;
  const delivered = { id: "msg_gone", type: "user", text: "Already delivered." };
  const waiting = { id: "msg_waits", type: "user", delivery: "queue", payload: { text: "Still waiting." } };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  here.engine.messages.set(session, [delivered]);
  here.engine.inboxes.set(session, [waiting]);
  here.engine.refuseRemoval = new Set([waiting.id]);
  const resident = await here.start().ready();
  const gone = await request(resident, "DELETE", queue + "/" + delivered.id);
  assert.equal(gone.status, 404, gone.text);
  assert.equal(gone.json.removed, undefined);
  const refused = await request(resident, "DELETE", queue + "/" + waiting.id);
  assert.equal(refused.status, 502, refused.text);
  assert.equal(refused.json.removed, undefined);
  assert.deepEqual(here.engine.messages.get(session), [delivered]);
  assert.deepEqual(here.engine.inboxes.get(session), [waiting]);
});

test("Send Now keeps the waiting payload until OpenCode confirms the exact message", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-unconfirmed");
  const session = "ses_unconfirmed";
  const id = "msg_unconfirmed";
  const queue = `kuvo/sessions/${session}/queue`;
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const resident = await here.start().ready();
  assert.equal((await request(resident, "POST", `kuvo/sessions/${session}/stop`)).status, 200);
  assert.equal((await request(resident, "POST", queue + "/add", { id, words: "Wait for proof.", delivery: "queue" })).json.waits, true);
  const file = path.join(here.home, "run", "waiting", id + ".json");
  here.engine.unconfirmedPrompt = id;
  const response = await request(resident, "POST", queue + "/" + id + "/now");
  assert.equal(response.status, 502, response.text);
  assert.equal(fs.existsSync(file), true);
  assert.deepEqual((await request(resident, "GET", queue)).json.waiting.map(row => row.id), [id]);
  here.engine.unconfirmedPrompt = null;
  assert.equal((await request(resident, "POST", queue + "/" + id + "/now")).status, 200);
  assert.equal(fs.existsSync(file), false);
  assert.deepEqual(here.engine.messages.get(session).map(row => row.id), [id]);
});

test("Edit protects a queued handoff until OpenCode replies and confirms the inbox", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-inflight");
  const session = "ses_inflight";
  const id = "msg_inflight";
  const queue = `kuvo/sessions/${session}/queue`;
  const item = queue + "/" + id;
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  const resident = await here.start().ready();
  assert.equal((await request(resident, "POST", `kuvo/sessions/${session}/stop`)).status, 200);
  assert.equal((await request(resident, "POST", queue + "/add", { id, words: "Keep my backup.", delivery: "queue" })).json.waits, true);
  const arrived = Promise.withResolvers();
  const release = Promise.withResolvers();
  here.engine.holdPrompt = { id, accepted: arrived.resolve, release: release.promise };
  const sending = request(resident, "POST", item + "/now");
  sending.catch(() => {});
  try {
    await arrived.promise;
    const editing = await request(resident, "DELETE", item, undefined, {}, 3000);
    assert.equal(editing.status, 502, editing.text);
    assert.deepEqual(here.engine.inboxes.get(session).map(row => row.id), [id]);
    assert.equal(fs.existsSync(path.join(here.home, "run", "waiting", id + ".json")), true);
  } finally {
    release.resolve();
  }
  assert.equal((await sending).status, 200);
  assert.equal(fs.existsSync(path.join(here.home, "run", "waiting", id + ".json")), false);
});

test("Edit requires the exact message lookup after a refused queued handoff", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-refused");
  const session = "ses_refused";
  const id = "msg_refused";
  const queue = `kuvo/sessions/${session}/queue`;
  const item = queue + "/" + id;
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const resident = await here.start().ready();
  assert.equal((await request(resident, "POST", `kuvo/sessions/${session}/stop`)).status, 200);
  assert.equal((await request(resident, "POST", queue + "/add", { id, words: "Keep my draft.", delivery: "queue" })).json.waits, true);
  const file = path.join(here.home, "run", "waiting", id + ".json");
  here.engine.refusePrompt = { id };
  assert.equal((await request(resident, "POST", item + "/now")).status, 502);
  here.engine.messageUnavailable = new Set([id]);
  const blocked = await request(resident, "DELETE", item);
  assert.equal(blocked.status, 502, blocked.text);
  assert.equal(fs.existsSync(file), true);
  here.engine.messageUnavailable.clear();
  const removed = await request(resident, "DELETE", item);
  assert.equal(removed.status, 200, removed.text);
  assert.equal(removed.json.removed.draft, "Keep my draft.");
  assert.equal(removed.json.paused, true);
  assert.equal(fs.existsSync(file), false);
  assert.equal(here.engine.asked.filter(row => row.method === "POST" && row.path.endsWith("/prompt")).length, 1);
});

test("a native Send Now keeps settling the handoff after the client waits thirty seconds", { timeout: 60000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-now-deadline");
  const [accept, reject] = ["accepted", "refused"].map(outcome => {
    const session = "ses_" + outcome;
    here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
    return { session, id: "msg_" + outcome, queue: `kuvo/sessions/${session}/queue` };
  });
  const record = await here.start().ready();
  for (const row of [accept, reject]) {
    assert.equal((await request(record, "POST", `kuvo/sessions/${row.session}/stop`)).status, 200);
    assert.equal((await request(record, "POST", row.queue + "/add", { id: row.id, words: "Keep this message." })).json.waits, true);
  }
  const answers = Promise.withResolvers();
  const sending = async (row, status) => {
    const seen = Promise.withResolvers();
    here.engine.holdPrompt = { id: row.id, accepted: seen.resolve, release: answers.promise, beforeAcceptance: true, status };
    const edit = row.queue + "/" + row.id;
    const response = request(record, "POST", edit + "/now", undefined, {}, 38000);
    response.catch(() => {});
    await seen.promise;
    return { edit, response, file: path.join(here.home, "run", "waiting", row.id + ".json") };
  };
  let one, two;
  try {
    one = await sending(accept);
    two = await sending(reject, 503);
    for (const send of [one, two]) {
      const response = await send.response;
      assert.equal(response.status, 502, response.text);
      assert.match(response.json.message, /still checking whether the message was sent/);
      assert.equal((await request(record, "DELETE", send.edit)).status, 502);
      assert.equal(fs.existsSync(send.file), true);
    }
  } finally {
    answers.resolve();
  }
  await until(() => !fs.existsSync(one.file), "the accepted handoff to retire its waiting file");
  assert.equal((await request(record, "DELETE", one.edit)).status, 404);
  const recovered = await until(async () => {
    const response = await request(record, "DELETE", two.edit);
    return response.status !== 502 && response;
  }, "the refused handoff to allow its draft to be edited");
  assert.equal(recovered.status, 200, recovered.text);
  assert.equal(recovered.json.removed.draft, "Keep this message.");
  assert.deepEqual(here.engine.messages.get(accept.session).map(row => row.id), [accept.id]);
  assert.equal(here.engine.messages.has(reject.session), false);
});

test("a delivered message leaves the displayed queue while its handoff backup survives", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-handoff-backup");
  const session = "ses_handoffBackup";
  const id = "msg_handoffBackup";
  const queue = `kuvo/sessions/${session}/queue`;
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const resident = await here.start().ready();
  assert.equal((await request(resident, "POST", `kuvo/sessions/${session}/stop`)).status, 200);
  const queued = await request(resident, "POST", queue + "/add", { id, words: "Keep my backup." });
  assert.equal(queued.json.waits, true);
  const line = await openLine(resident, "", queue + "/events");
  t.after(() => line.close());
  const frames = line.frames();
  assert.deepEqual((await frames.next()).value.items.map(item => item.id), [id]);
  await until(() => here.engine.subscribers() > 0, "the queue to follow engine events");
  const release = Promise.withResolvers();
  here.engine.holdPrompt = { id, accepted: release.resolve, release: release.promise, status: 503 };
  const now = await request(resident, "POST", queue + "/" + id + "/now");
  assert.equal(now.status, 502, now.text);
  here.engine.emit({ type: "session.inbox.delivered", data: { sessionID: session, inboxID: id } });
  for await (const update of frames) {
    if (update.items?.length === 0) break;
  }
  const listing = await request(resident, "GET", queue + "?items=1");
  assert.deepEqual(listing.json.items, []);
  assert.equal((await request(resident, "DELETE", queue + "/" + id)).status, 404);
  assert.deepEqual((await request(resident, "GET", queue)).json.waiting.map(row => row.id), [id]);
  assert.equal(fs.existsSync(path.join(here.home, "run", "waiting", id + ".json")), true);
  assert.deepEqual(here.engine.messages.get(session).map(row => row.id), [id]);
});

test("an old unconfirmed native handoff does not become safe to retry", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-legacy-handoff");
  const session = "ses_oldHandoff";
  const id = "msg_oldHandoff";
  const queue = `kuvo/sessions/${session}/queue`;
  const item = queue + "/" + id;
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const index = path.join(here.home, "run", "queues.json");
  write(index, JSON.stringify({ [session]: { paused: true, held: [], handoff: [id], waiting: [{ id, text: "Pending.", at: 1 }] } }));
  const file = path.join(here.home, "run", "waiting", id + ".json");
  write(file, JSON.stringify({ id, text: "Pending.", delivery: "queue" }));
  const resident = await here.start().ready();
  assert.equal((await request(resident, "POST", item + "/now")).status, 502);
  assert.equal((await request(resident, "DELETE", item)).status, 502);
  assert.equal(fs.existsSync(file), true);
  assert.equal(here.engine.asked.some(row => row.method === "POST" && row.path.endsWith("/prompt")), false);
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
  const changes = reconnect.frames();
  assert.deepEqual((await changes.next()).value.items.map(item => item.id), [second.id]);
  const third = { id: "msg_third", type: "user", delivery: "queue", payload: { text: "One more thing." } };
  here.engine.inboxes.set(session, [second, third]);
  here.engine.emit({ type: "session.inbox.enqueued", data: { sessionID: session, inboxID: third.id, item: third } });
  assert.deepEqual((await changes.next()).value.items.map(({ id, words, on }) => [id, words, on]), [
    [second.id, second.payload.text, "engine"], [third.id, third.payload.text, "engine"],
  ]);

  here.engine.inboxReplies.set("ses_missing", { status: 404, body: {} });
  const missing = await request(record, "GET", "kuvo/sessions/ses_missing/queue?items=1");
  assert.equal(missing.status, 200, missing.text);
  assert.deepEqual(missing.json, { paused: false, held: [], items: null });

  const saved = "ses_savedDuringOutage";
  const savedRoute = `kuvo/sessions/${saved}/queue`;
  here.engine.sessions.push(sessionOf({ id: saved, directory: here.home }));
  assert.equal((await request(record, "POST", `kuvo/sessions/${saved}/stop`)).json.paused, true);
  const added = await request(record, "POST", savedRoute + "/add", { id: "msg_savedDuringOutage", words: "Keep this." },
    { "x-kuvo-from": "app" });
  assert.equal(added.json.waits, true);
  here.engine.inboxReplies.set(saved, { status: 503, body: {} });
  const unreadable = await request(record, "GET", savedRoute + "?items=1");
  assert.equal(unreadable.status, 200, unreadable.text);
  assert.equal(unreadable.json.items, null);
  assert.deepEqual(unreadable.json.waiting.map(row => [row.id, row.text]), [["msg_savedDuringOutage", "Keep this."]]);
});

test("a slow reorder reads the queue after a concurrent Add and Send Now finish", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-slow-reorder");
  const session = "ses_reorderInterleave";
  const base = `kuvo/sessions/${session}`;
  const queue = base + "/queue";
  const drafts = ["msg_a", "msg_b", "msg_c", "msg_d"];
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const record = await here.start().ready();
  assert.equal((await request(record, "POST", base + "/stop")).status, 200);
  const add = id => request(record, "POST", queue + "/add", { id, words: id }, { "x-kuvo-from": "app" }, 3000);
  for (const id of drafts.slice(0, 3)) assert.equal((await add(id)).json.waits, true);
  const read = Promise.withResolvers();
  const release = Promise.withResolvers();
  here.engine.holdInbox = { session, accepted: read.resolve, release: release.promise };
  const ordering = request(record, "POST", queue + "/reorder", { item: drafts[2], anchor: drafts[1], before: true });
  ordering.catch(() => {});
  try {
    await read.promise;
    assert.equal((await add(drafts[3])).json.waits, true);
    assert.equal((await request(record, "POST", queue + "/" + drafts[0] + "/now", undefined, {}, 3000)).status, 200);
  } finally {
    release.resolve();
  }
  const ordered = await ordering;
  assert.equal(ordered.status, 200, ordered.text);
  assert.deepEqual(ordered.json.items.map(item => item.id), [drafts[2], drafts[1], drafts[3]]);
  assert.equal((await request(record, "POST", queue + "/resume")).status, 200);
  const delivered = () => here.engine.messages.get(session).map(row => row.id);
  assert.deepEqual(delivered(), [drafts[0], drafts[2]]);
  for (const count of [3, 4]) {
    here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: session } });
    await until(() => delivered().length === count, "the next saved draft to drain in its new order");
  }
  assert.deepEqual(delivered(), [drafts[0], drafts[2], drafts[1], drafts[3]]);
});

test("Send Now recovers a confirmed message after the queue index fails to save", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-index-after-send");
  const session = "ses_indexFailure";
  const base = `kuvo/sessions/${session}`;
  const queue = base + "/queue";
  const draft = "msg_cannotSeal";
  const later = "msg_afterSealing";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const record = await here.start().ready();
  assert.equal((await request(record, "POST", base + "/stop")).json.paused, true);
  for (const id of [draft, later]) {
    assert.equal((await request(record, "POST", queue + "/add", { id, words: id }, { "x-kuvo-from": "app" })).json.waits, true);
  }
  const index = path.join(here.home, "run", "queues.json");
  const backup = index + ".fixture";
  const send = () => request(record, "POST", `${queue}/${draft}/now`);
  const calls = () => here.engine.asked.filter(row => row.method === "POST" && row.path.endsWith("/prompt")).map(row => row.json.id);
  here.engine.afterPromptAccepted = (_session, id) => {
    if (id !== draft) return;
    fs.renameSync(index, backup);
    fs.mkdirSync(index);
  };
  try {
    const refused = await send();
    assert.equal(refused.status, 502, refused.text);
    assert.ok(fs.existsSync(backup));
    assert.deepEqual(calls(), [draft]);
    assert.ok(fs.existsSync(path.join(here.home, "run", "waiting", draft + ".json")));
  } finally {
    here.engine.afterPromptAccepted = null;
    if (fs.existsSync(backup)) {
      fs.rmdirSync(index);
      fs.renameSync(backup, index);
    }
  }
  const recovered = await send();
  assert.equal(recovered.status, 200, recovered.text);
  assert.deepEqual(calls(), [draft]);
  assert.deepEqual(recovered.json.waiting.map(row => row.id), [later]);
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

test("the next queued prompt waits for the engine's previous turn to settle", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("drain-after-turn");
  const session = "ses_laterDrain";
  const first = { id: "msg_first", type: "user", delivery: "queue", payload: { text: "First." } };
  const second = { id: "msg_second", type: "user", delivery: "queue", payload: { text: "Second." } };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.inboxes.set(session, [first, second]);
  here.engine.running.add(session);
  const record = await here.start().ready();
  assert.equal((await request(record, "GET", "api/session/" + session)).status, 200);
  await until(() => here.engine.subscribers() > 0, "the resident to follow the running session");
  const end = () => here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: session } });
  here.engine.running.delete(session);
  end();
  await new Promise(resolve => setTimeout(resolve, 1000));
  assert.deepEqual([first.delivery, second.delivery], ["queue", "queue"]);
  end();
  await new Promise(resolve => setTimeout(resolve, 1000));
  assert.deepEqual([first.delivery, second.delivery], ["queue", "queue"]);
  await until(() => first.delivery === "steer", "the first queued item to take its turn");
  assert.equal(second.delivery, "queue");
  assert.deepEqual(here.engine.interrupted, []);
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

test("a process exit releases its queue live and again after a resident restart", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-after-process-exit");
  const session = "ses_processExit";
  const item = id => ({ id, type: "user", delivery: "queue", payload: { text: id } });
  const first = item("msg_beforeRestart");
  const second = item("msg_afterRestart");
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home, updated: Date.now() }));
  here.engine.inboxes.set(session, [first]);
  here.engine.running.add(session);
  const child = here.start();
  const record = await child.ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow process exits");
  const exit = { outcome: "interrupted", reason: "agent-exit", time: { created: Date.now() } };
  here.engine.messages.set(session, [{ type: "idle", ...exit }]);
  here.engine.emit({ type: "session.execution.interrupted", data: { sessionID: session, ...exit } });
  await new Promise(resolve => setTimeout(resolve, 1700));
  assert.equal(first.delivery, "queue", "the engine has yet to report that the turn is free");
  here.engine.running.delete(session);
  here.engine.emit({ type: "session.idle", data: { sessionID: session } });
  await until(() => first.delivery === "steer", "the queue head to advance after the process exit");
  const read = active => request(active, "GET", `kuvo/sessions/${session}/queue`);
  assert.equal((await read(record)).json.paused, false);
  await child.stop();
  here.engine.inboxes.set(session, [second]);
  const resumed = await here.start().ready();
  await until(() => second.delivery === "steer", "the next queue head to advance after reconnect");
  assert.deepEqual((await read(resumed)).json, { paused: false, held: [] });
});

test("queue recovery leaves an unknown turn alone and resumes an open turn on a replacement engine", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("queue-engine-unknown");
  const replacement = await computer("queue-engine-replacement");
  const session = "ses_engineReplacement";
  const original = { id: "msg_original", type: "user", delivery: "steer", payload: { text: "Maybe in use." } };
  const waiting = { id: "msg_recovered", type: "user", delivery: "queue", payload: { text: "Carry on." } };
  const conversation = sessionOf({ id: session, directory: here.home, updated: Date.now() });
  here.engine.sessions.push(conversation);
  here.engine.inboxes.set(session, [original]);
  here.engine.activeUnavailable = true;
  const child = here.start();
  let record = await child.ready();
  await until(() => here.engine.asked.some(row => row.method === "GET" && row.path.endsWith(`/${session}/inbox`)),
    "queue recovery to inspect the old inbox");
  assert.equal(original.delivery, "steer");
  const read = () => request(record, "GET", `kuvo/sessions/${session}/queue`);
  assert.deepEqual((await read()).json, { paused: false, held: [] });
  await child.stop();

  replacement.engine.sessions.push(conversation);
  replacement.engine.inboxes.set(session, [waiting]);
  replacement.engine.messages.set(session, [{ id: "msg_openTurn", type: "assistant", time: { created: Date.now() } }]);
  write(path.join(here.home, "run", "engine.json"), JSON.stringify(replacement.engine.record));
  record = await here.start().ready();
  await until(() => waiting.delivery === "steer", "the replacement engine to resume its queued prompt");
  assert.deepEqual((await read()).json, { paused: false, held: [] });
  assert.deepEqual(replacement.engine.interrupted, []);
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
  written("[Wake-up] Check this again.");
  written("An automatic note.", { kuvo: { note: { from: "Kuvo Code", line: "An automatic note." } } });
  await new Promise(resolve => setTimeout(resolve, 200));
  assert.equal((await queue()).json.paused, true);
  written("Try with the new model.");
  await until(async () => (await queue()).json?.paused === false, "Your steer did not open the queue.");
});

test("Stop holds an inbox steer ahead of the queue until Resume", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("stop-inbox-order");
  const session = "ses_stopOrder";
  const later = { id: "msg_later", type: "user", delivery: "queue", payload: { text: "Later." } };
  const next = { id: "msg_next", type: "user", delivery: "steer", payload: { text: "Next." } };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow the session");
  here.engine.inboxes.set(session, [later, next]);
  const base = `kuvo/sessions/${session}`;
  const stopped = await request(record, "POST", base + "/stop");
  assert.equal(stopped.status, 200, stopped.text);
  assert.deepEqual(stopped.json, { paused: true, held: [next.id] });
  assert.equal(next.delivery, "queue");
  here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: session } });
  await new Promise(resolve => setTimeout(resolve, 1700));
  const held = await until(async () => {
    const reply = await request(record, "GET", base + "/queue?items=1");
    return reply.json?.items?.length === 2 && reply.json;
  }, "the queue to show both inbox items");
  assert.deepEqual(held.items.map(row => [row.id, row.held, row.delivery]),
    [[next.id, true, "queue"], [later.id, false, "queue"]]);
  assert.equal(held.paused, true);
  const resumed = await request(record, "POST", base + "/queue/resume");
  assert.deepEqual(resumed.json, { paused: false, held: [next.id] });
  assert.deepEqual(here.engine.interrupted, [{ session, resume: false }]);
  assert.equal(next.delivery, "queue");
  here.engine.running.delete(session);
  here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: session } });
  await until(() => next.delivery === "steer", "the held steer to advance once the engine is free");
  await until(async () => (await request(record, "GET", base + "/queue")).json?.held.length === 0,
    "the queue to clear its held item after delivery");
  assert.equal(next.delivery, "steer");
  assert.equal(later.delivery, "queue");
});

test("a failed turn drops its waiting summary and a failed summary lets the queue advance", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("failed-summary");
  const failedTurn = "ses_failedTurn";
  const failedSummary = "ses_failedSummary";
  for (const id of [failedTurn, failedSummary]) {
    here.engine.sessions.push(sessionOf({ id, directory: here.home }));
    here.engine.running.add(id);
  }
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow the sessions");
  const queued = id => ({ id, type: "user", delivery: "queue", payload: { text: "Next." } });
  const afterTurn = queued("msg_afterTurn");
  const afterSummary = queued("msg_afterSummary");
  here.engine.inboxes.set(failedTurn, [afterTurn, { id: "msg_summary", type: "compaction", delivery: "steer" }]);
  here.engine.inboxes.set(failedSummary, [afterSummary]);
  here.engine.messages.set(failedSummary, [
    { type: "idle", outcome: "failed" }, { type: "compaction", status: "failed", reason: "manual" },
  ]);
  for (const id of [failedTurn, failedSummary]) {
    here.engine.running.delete(id);
    here.engine.emit({ type: "session.execution.failed", data: { sessionID: id, error: { message: "It failed." } } });
  }
  await until(() => here.engine.inboxes.get(failedTurn).length === 1, "the failed turn to remove its summary");
  await until(() => afterSummary.delivery === "steer", "the failed summary to leave the queue moving");
  assert.equal(afterTurn.delivery, "queue");
  const queue = id => request(record, "GET", `kuvo/sessions/${id}/queue`);
  assert.deepEqual((await queue(failedTurn)).json, { paused: true, held: [] });
  assert.deepEqual((await queue(failedSummary)).json, { paused: false, held: [] });
});

test("Stop only holds the messages still in the inbox after changing delivery", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("stop-with-delivery");
  const session = "ses_deliveryDuringStop";
  const message = (id, delivery) => ({ id, type: "user", delivery, payload: { text: id } });
  const taken = message("msg_taken", "steer");
  const held = message("msg_stillHeld", "steer");
  const later = message("msg_later", "queue");
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  here.engine.inboxes.set(session, [taken, held, later]);
  here.engine.afterInboxPatch = (id, item) => {
    if (item.id !== taken.id) return;
    here.engine.inboxes.set(id, [held, later]);
    here.engine.messages.set(id, [{ id: item.id, type: "user", text: item.payload.text }]);
  };
  const record = await here.start().ready();
  const stopped = await request(record, "POST", `kuvo/sessions/${session}/stop`);
  assert.equal(stopped.status, 200, stopped.text);
  assert.deepEqual(stopped.json, { paused: true, held: [held.id] });
  assert.deepEqual(here.engine.inboxes.get(session).map(row => [row.id, row.delivery]),
    [[held.id, "queue"], [later.id, "queue"]]);
});

test("an idle inbox arrival advances after the grace period unless paused", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("idle-inbox-arrival");
  const idle = "ses_idleArrival";
  const paused = "ses_pausedArrival";
  for (const id of [idle, paused]) here.engine.sessions.push(sessionOf({ id, directory: here.home }));
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow inbox events");
  assert.deepEqual((await request(record, "POST", `kuvo/sessions/${paused}/stop`)).json,
    { paused: true, held: [] });
  const arrive = sessionID => {
    const item = { id: "msg_arrived", type: "user", delivery: "queue", payload: { text: "Now I'm waiting." } };
    here.engine.inboxes.set(sessionID, [item]);
    here.engine.emit({ type: "session.inbox.enqueued", data: { sessionID, inboxID: item.id, item } });
    return item;
  };
  const held = arrive(paused);
  const next = arrive(idle);
  await new Promise(resolve => setTimeout(resolve, 1000));
  assert.deepEqual([held.delivery, next.delivery], ["queue", "queue"]);
  await until(() => next.delivery === "steer", "the idle inbox item to advance");
  assert.equal(held.delivery, "queue");
  assert.deepEqual(here.engine.interrupted, [{ session: paused, resume: false }]);
});

test("the global queue line starts with a snapshot and reports holds and resumes", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("global-queue-line");
  const session = "ses_queueLine";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  here.engine.inboxes.set(session, [{ id: "msg_later", type: "user", delivery: "queue", payload: { text: "Later." } }]);
  const record = await here.start().ready();
  const line = await openLine(record, "", "kuvo/sessions/queues/events");
  t.after(() => line.close());
  assert.equal(line.status, 200);
  const frames = line.frames();
  assert.deepEqual((await frames.next()).value, { queues: {} });
  const route = `kuvo/sessions/${session}/queue`;
  assert.equal((await request(record, "POST", route + "/hold")).status, 200);
  assert.deepEqual((await frames.next()).value, { queues: { [session]: { paused: true, held: [] } } });
  assert.equal((await request(record, "POST", route + "/resume")).status, 200);
  assert.deepEqual((await frames.next()).value, { queues: {} });
});

test("a quick Resume supersedes the external Stop before its delayed hold", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("resume-after-external-stop");
  const session = "ses_resumeAtStop";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.inboxes.set(session, [
    { id: "msg_first", type: "user", delivery: "queue", payload: { text: "First." } },
    { id: "msg_second", type: "user", delivery: "queue", payload: { text: "Second." } },
  ]);
  here.engine.running.add(session);
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow the Stop");
  here.engine.running.delete(session);
  here.engine.emit({ type: "session.execution.interrupted", data: { sessionID: session, reason: "user" } });
  await new Promise(resolve => setTimeout(resolve, 100));
  const route = `kuvo/sessions/${session}/queue`;
  const resumed = await request(record, "POST", route + "/resume");
  assert.deepEqual(resumed.json, { paused: false, held: [] });
  await new Promise(resolve => setTimeout(resolve, 1700));
  assert.deepEqual((await request(record, "GET", route)).json, resumed.json);
  assert.deepEqual(here.engine.inboxes.get(session).map(row => row.delivery), ["steer", "queue"]);
  assert.equal(here.engine.asked.some(row => row.method === "PATCH" && row.json?.delivery === "queue"), false);
});

test("a paused queue stays paused when its last engine message is taken back", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("paused-empty-queue");
  const session = "ses_pausedEmpty";
  const item = "msg_theLast";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.inboxes.set(session, [{ id: item, type: "user", delivery: "steer", payload: { text: "Last." } }]);
  here.engine.running.add(session);
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow inbox removal");
  assert.deepEqual((await request(record, "POST", `kuvo/sessions/${session}/stop`)).json,
    { paused: true, held: [item] });
  here.engine.inboxes.set(session, []);
  here.engine.running.delete(session);
  here.engine.emit({ type: "session.inbox.cancelled", data: { sessionID: session, inboxID: item } });
  const route = `kuvo/sessions/${session}/queue`;
  const empty = await until(async () => {
    const found = await request(record, "GET", route);
    return found.json?.held.length === 0 && found.json;
  }, "the held item to leave the queue");
  assert.deepEqual(empty, { paused: true, held: [] });
  const late = { id: "msg_late", text: "One more thing." };
  const saved = await request(record, "POST", route + "/add", late);
  assert.equal(saved.status, 200, saved.text);
  assert.equal(saved.json.waits, true);
  assert.deepEqual(saved.json.waiting.map(row => row.id), [late.id]);
  const repeated = await request(record, "POST", route + "/add", late);
  assert.equal(repeated.status, 200, repeated.text);
  assert.equal(repeated.json.waits, true);
  assert.deepEqual(repeated.json.waiting.map(row => row.id), [late.id]);
  assert.deepEqual(here.engine.messages.get(session) ?? [], []);
  const next = { id: "msg_next", text: "Use the new notes." };
  assert.equal((await request(record, "POST", route + "/add", next)).json.waits, true);
  const removed = await request(record, "DELETE", route + "/" + late.id);
  assert.equal(removed.status, 200, removed.text);
  assert.deepEqual(removed.json.waiting.map(row => row.id), [next.id]);
  assert.deepEqual((await request(record, "DELETE", route + "/" + late.id)).json, removed.json);
  const ack = await request(record, "POST", route + "/" + late.id + "/ack", { receipt: removed.json.removed.receipt });
  assert.equal(ack.status, 200, ack.text);
  assert.equal((await request(record, "DELETE", route + "/" + late.id)).status, 404);
  assert.equal(fs.existsSync(path.join(here.home, "run", "waiting", late.id + ".json")), false);
  here.engine.running.add(session);
  const sent = await request(record, "POST", route + "/" + next.id + "/now");
  assert.deepEqual(sent.json, { paused: false, held: [] });
  assert.deepEqual(here.engine.inboxes.get(session).map(item => [item.id, item.delivery]), [[next.id, "steer"]]);
  assert.deepEqual(here.engine.interrupted.at(-1), { session, resume: true });
});

test("Stop on an empty queue keeps two later messages waiting and releases them one turn at a time", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("stop-while-empty");
  const session = "ses_stopEmpty";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  const record = await here.start().ready();
  const base = `kuvo/sessions/${session}`;
  const queue = base + "/queue";
  const stop = await request(record, "POST", base + "/stop");
  assert.deepEqual(stop.json, { paused: true, held: [] });
  for (const id of ["msg_firstLate", "msg_secondLate"]) {
    const add = await request(record, "POST", base + "/send",
      { id, words: id, delivery: "queue" }, { "x-kuvo-from": "app" });
    assert.deepEqual(add.json, { waits: true });
  }
  const items = await until(async () => {
    const reply = await request(record, "GET", queue + "?items=1");
    return reply.json?.items?.length === 2 && reply.json.items;
  }, "both saved messages to be listed");
  assert.deepEqual(items.map(row => [row.id, row.on]), [["msg_firstLate", "box"], ["msg_secondLate", "box"]]);
  const sent = () => here.engine.asked.filter(row => row.method === "POST" && row.path.endsWith("/prompt")).map(row => row.json.id);
  assert.deepEqual(sent(), []);

  here.engine.running.delete(session);
  const resume = await request(record, "POST", queue + "/resume");
  assert.equal(resume.status, 200, resume.text);
  here.engine.running.add(session);
  assert.deepEqual(sent(), ["msg_firstLate"]);
  assert.deepEqual((await request(record, "GET", queue)).json.waiting.map(row => row.id), ["msg_secondLate"]);
  here.engine.running.delete(session);
  here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: session } });
  await until(() => sent().length === 2, "the second message to follow the completed turn");
  assert.deepEqual(sent(), ["msg_firstLate", "msg_secondLate"]);
});

test("Steer releases only one held engine message and starts a saved draft when selected", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("steer-held-queue");
  const session = "ses_steerHeld";
  const item = id => ({ id, type: "user", delivery: "steer", payload: { text: id } });
  const one = item("msg_one");
  const two = item("msg_two");
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.inboxes.set(session, [one, two]);
  here.engine.running.add(session);
  const record = await here.start().ready();
  const base = `kuvo/sessions/${session}`;
  const queue = base + "/queue";
  assert.deepEqual((await request(record, "POST", base + "/stop")).json, { paused: true, held: [one.id, two.id] });
  const steer = id => request(record, "POST", `${queue}/${id}/steer`);
  const steered = await steer(two.id);
  assert.equal(steered.status, 200, steered.text);
  assert.deepEqual(steered.json, { paused: true, held: [one.id] });
  assert.deepEqual([one.delivery, two.delivery], ["queue", "steer"]);
  assert.deepEqual(here.engine.interrupted, [{ session, resume: false }]);
  assert.equal((await steer("msg_missing")).status, 404);

  const taken = item("msg_taken");
  taken.delivery = "queue";
  here.engine.inboxes.get(session).push(taken);
  const reading = Promise.withResolvers();
  const handoff = Promise.withResolvers();
  here.engine.holdInbox = { session, accepted: reading.resolve, release: handoff.promise };
  t.after(handoff.resolve);
  const racing = steer(taken.id);
  await reading.promise;
  const inbox = here.engine.inboxes.get(session);
  inbox.splice(inbox.indexOf(taken), 1);
  handoff.resolve();
  assert.equal((await racing).status, 404);
  assert.equal(here.engine.interrupted.length, 1);

  const draft = { id: "msg_saved", text: "Take this next." };
  const saved = await request(record, "POST", queue + "/add", draft);
  assert.equal(saved.json.waits, true);
  const advance = await steer(draft.id);
  assert.equal(advance.status, 200, advance.text);
  assert.deepEqual(advance.json, { paused: false, held: [one.id] });
  assert.equal(here.engine.inboxes.get(session).find(row => row.id === draft.id)?.delivery, "steer");
  assert.deepEqual(here.engine.interrupted.at(-1), { session, resume: true });

  here.engine.running.delete(session);
  const idle = await steer(one.id);
  assert.deepEqual(idle.json, { paused: false, held: [] });
  assert.equal(one.delivery, "steer");
  assert.equal(here.engine.interrupted.length, 2);
});

test("the composer saves a queued prompt while stopped and Send Now interrupts for its accepted prompt", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("composer-send-now");
  const session = "ses_composerNow";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.running.add(session);
  const record = await here.start().ready();
  const base = `kuvo/sessions/${session}`;
  const send = (id, words, delivery) => request(record, "POST", base + "/send", { id, words, delivery }, { "x-kuvo-from": "app" });
  assert.equal((await request(record, "POST", base + "/stop")).status, 200);
  const wait = await send("msg_waits", "After the other turn.", "queue");
  assert.deepEqual(wait.json, { waits: true });
  assert.deepEqual(here.engine.inboxes.get(session) ?? [], []);
  const now = await send("msg_goNow", "Stop and use the new copy.", "now");
  assert.equal(now.status, 200, now.text);
  assert.deepEqual(here.engine.inboxes.get(session).map(row => [row.id, row.delivery]), [["msg_goNow", "steer"]]);
  const effects = here.engine.asked.filter(row => row.method === "POST" && /\/(prompt|interrupt)/.test(row.path));
  assert.deepEqual(effects.map(row => row.path.split("/").at(-1)), ["interrupt", "prompt", "interrupt"]);
  assert.deepEqual(here.engine.interrupted, [{ session, resume: false }, { session, resume: true }]);
  const left = await request(record, "GET", base + "/queue");
  assert.equal(left.json.paused, false);
  assert.deepEqual(left.json.waiting.map(row => row.id), ["msg_waits"]);
});

test("a restart resumes the first saved draft and failure holds the next", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("waiting-after-restart");
  const session = "ses_restartWaiting";
  const one = "msg_first";
  const two = "msg_second";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const child = here.start();
  let record = await child.ready();
  const base = `kuvo/sessions/${session}`;
  const route = base + "/queue";
  assert.deepEqual((await request(record, "POST", base + "/stop")).json, { paused: true, held: [] });
  for (const id of [one, two]) {
    assert.equal((await request(record, "POST", route + "/add", { id, text: id })).json.waits, true);
  }
  await child.stop();
  const index = path.join(here.home, "run", "queues.json");
  const saved = readJSON(index);
  saved[session].paused = false;
  delete saved[session].userPaused;
  write(index, JSON.stringify(saved));
  const arrived = Promise.withResolvers();
  const reply = Promise.withResolvers();
  t.after(reply.resolve);
  here.engine.holdPrompt = { id: one, accepted: arrived.resolve, release: reply.promise };
  record = await here.start().ready();
  await arrived.promise;
  await until(() => here.engine.subscribers() > 0, "the restarted resident to follow the engine");
  assert.deepEqual(here.engine.messages.get(session).map(row => row.id), [one]);
  here.engine.emit({ type: "session.execution.started", data: { sessionID: session } });
  here.engine.emit({ type: "session.execution.failed", data: { sessionID: session, error: { message: "The run failed." } } });
  await new Promise(resolve => setTimeout(resolve, 200));
  reply.resolve();
  const held = await until(async () => {
    const result = await request(record, "GET", route);
    return result.json?.paused && result.json.waiting?.length === 1 && result.json;
  }, "the failed turn to pause the saved queue");
  assert.deepEqual(held.waiting.map(row => row.id), [two]);
  assert.deepEqual(here.engine.messages.get(session).map(row => row.id), [one]);
  assert.equal((await request(record, "POST", route + "/resume")).status, 200);
  assert.deepEqual(here.engine.messages.get(session).map(row => row.id), [one, two]);
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
  }, headers, 25000);
  await entered.promise;
  const stopping = request(resident, "POST", route + "/stop", { items: [theId] }, headers, 25000);
  try {
    await until(() => {
      try { return readJSON(path.join(here.home, "run", "queues.json"))?.[session]?.arriving?.[theId]; }
      catch { return false; }
    }, "Stop records the named send");
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

test("archived groups keep Chats and the names of removed projects", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("archive-titles");
  const project = path.join(here.home, "project-folder");
  const chats = path.join(here.home, "chats");
  write(path.join(here.home, "box/projects.json"), JSON.stringify({
    projects: [{ directory: project, folders: [], name: "The original name", removed: true }],
  }));
  for (const [id, directory, notes] of [
    ["ses_oldProject", project, {}], ["ses_oldChat", chats, {}],
    ["ses_oldSeat", chats, { seat: "grp_example" }], ["ses_oldRun", project, { engineFor: "ses_oldProject" }],
  ]) {
    here.engine.sessions.push({ ...sessionOf({ id, directory }), metadata: { "kuvo.archived": true, kuvo: notes } });
  }
  const resident = await here.start().ready();
  const listing = await until(async () => {
    const response = await request(resident, "GET", "kuvo/sessions/list");
    return response.json?.archivedGroups?.length && response.json;
  }, "the list to publish the archived groups");
  assert.deepEqual(listing.archivedGroups, [
    { title: "Chats", kind: "chat", directory: chats, sessions: ["ses_oldChat"] },
    { title: "The original name", kind: "project", directory: project, sessions: ["ses_oldProject"] },
  ]);
  assert.equal(listing.projects[project], undefined);
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

test("a subagent changes only its own row and the parent whose list was reordered", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("subagent-delta");
  const parent = sessionOf({ id: "ses_parentDelta", directory: here.home, updated: 110 });
  const first = sessionOf({ id: "ses_childDelta", directory: here.home, updated: 120, parent: parent.id });
  const next = sessionOf({ id: "ses_nextDelta", directory: here.home, updated: 130, parent: parent.id });
  const other = sessionOf({ id: "ses_otherDelta", directory: here.home, updated: 140 });
  first.time.created = 120;
  next.time.created = 130;
  here.engine.sessions.push(parent, first, next, other);
  const record = await here.start().ready();
  const list = at => request(record, "GET", "kuvo/sessions/list" + (at ? "?since=" + encodeURIComponent(at) : ""));
  const initial = (await list()).json;
  assert.deepEqual(initial.sessions[parent.id].subagents, [next.id, first.id]);
  await until(() => here.engine.subscribers() > 0, "the resident to follow subagent events");
  other.time.updated = 200;
  here.engine.emit({ type: "session.updated", data: { sessionID: other.id, info: other } });
  const changed = await until(async () => {
    const page = (await list(initial.at)).json;
    return page?.sessions?.[other.id]?.updated === 200 && page;
  }, "the unrelated conversation delta");
  assert.deepEqual(Object.keys(changed.sessions), [other.id]);

  here.engine.running.add(first.id);
  here.engine.emit({ type: "session.execution.started", data: { sessionID: first.id } });
  const started = await until(async () => {
    const page = (await list(changed.at)).json;
    return page?.sessions?.[first.id]?.running === true && page;
  }, "the active subagent delta");
  assert.deepEqual(Object.keys(started.sessions).sort(), [first.id, parent.id]);
  assert.deepEqual(started.sessions[parent.id].subagents, [first.id, next.id]);
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
  assert.equal(fs.existsSync(path.join(here.home, "run", "lists", "sessions.json")), false);
  const asks = path.join(here.home, "run", "glance", "asks.json");
  await until(() => fs.existsSync(asks) && readJSON(asks).asks[form.id], "the resident to hear the deletion");
  assert.equal(here.engine.heldInventories, 1, "only one complete inventory may be in flight");
  here.engine.holdSessionList = null;
  gate.resolve();
  const initial = await reading;
  assert.equal(initial.status, 200, initial.text);
  assert.equal(initial.json.full, true);
  assert.deepEqual(Object.keys(initial.json.sessions), [kept.id]);
  assert.equal(initial.json.gone, undefined);
  assert.equal(here.engine.inventoryReads, 2, "the change requests one new inventory");
});

test("an earlier session read cannot bring back an old name or undo a pin or deletion", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-list-stale-event");
  const before = sessionOf({ id: "ses_changes", directory: here.home, title: "Original" });
  const safe = sessionOf({ id: "ses_safe", directory: here.home, title: "Keep this one" });
  here.engine.sessions.push(before, safe);
  const resident = await here.start().ready();
  const listing = async () => {
    const response = await request(resident, "GET", "kuvo/sessions/list");
    assert.equal(response.status, 200, response.text);
    return response.json;
  };
  await until(async () => (await listing()).sessions[before.id]?.title === "Original" && here.engine.subscribers() > 0,
    "the original inventory and event stream");

  const hold = async (change, event = "session.updated") => {
    const gate = Promise.withResolvers();
    const exact = Promise.withResolvers();
    const inventories = here.engine.heldInventories ?? 0;
    let exactRead = false;
    void exact.promise.then(() => { exactRead = true; });
    here.engine.holdSessionRead = { when: id => id === before.id, accepted: exact.resolve, release: gate.promise };
    here.engine.holdSessionList = gate.promise;
    here.engine.emit({ type: event, data: { sessionID: before.id } });
    try {
      await until(() => exactRead || (here.engine.heldInventories ?? 0) > inventories, "the earlier engine read");
      change();
    } finally {
      here.engine.holdSessionRead = null;
      here.engine.holdSessionList = null;
      gate.resolve();
    }
  };
  here.engine.sessions[0] = { ...before, title: "The earlier name" };
  await hold(() => {
    here.engine.sessions[0] = { ...before, title: "Newest name" };
    here.engine.emit({ type: "session.renamed", data: { sessionID: before.id } });
  });
  await until(async () => (await listing()).sessions[before.id]?.title === "Newest name",
    "the newest title to replace the earlier read");
  here.engine.sessions[0].metadata = { "kuvo.pinned": true };
  await hold(() => {
    for (let i = 0; i < 12; i++) here.engine.emit({ type: "session.text.delta", data: { sessionID: before.id, delta: "x" } });
  }, "session.metadata.updated");
  await until(async () => (await listing()).sessions[before.id]?.pinned === true,
    "streaming text to leave the pinned session update intact");
  await hold(() => {
    here.engine.sessions.shift();
    here.engine.emit({ type: "session.deleted", data: { sessionID: before.id } });
  });
  const gone = await until(async () => {
    const list = await listing();
    return !list.sessions[before.id] && list;
  }, "the deleted session to stay gone");
  assert.equal(gone.sessions[safe.id].title, safe.title);
  assert.ok(!(await listing()).sessions[before.id]);
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
  const savedList = path.join(here.home, "run", "lists", "sessions.json");
  await until(() => fs.existsSync(savedList), "the resident to retain its inventory");
  const keptBytes = fs.readFileSync(savedList, "utf8");
  const routeArgs = boxBinary().typescript ? ["box", "route"] : ["route"];
  for (let reads = 0; reads < 2; reads++) {
    const requestPath = `kuvo/sessions/list?since=${encodeURIComponent(newest.at)}`;
    const routed = await here.command(routeArgs, JSON.stringify({ method: "GET", path: requestPath }));
    assert.equal(routed.code, 0, routed.log);
    const reply = JSON.parse(routed.text);
    assert.equal(reply.status, 200, routed.text);
    const local = JSON.parse(reply.body);
    assert.equal(local.at, newest.at);
    assert.deepEqual(local.sessions, {});
  }
  assert.equal(fs.readFileSync(savedList, "utf8"), keptBytes);
  const followedChanges = await openLine(record, `?since=${encodeURIComponent(newest.at)}`, "kuvo/sessions/list/events");
  t.after(() => followedChanges.close());
  const changes = followedChanges.frames();
  assert.deepEqual((await changes.next()).value.sessions, {});
  assert.equal((await request(record, "POST", `kuvo/sessions/${kept}/rename`, { title: "After" })).status, 200);
  const changed = await until(async () => {
    const answer = await list(newest.at);
    return answer.json?.sessions?.[kept]?.title === "After" && answer.json;
  }, "The recorded cursor did not receive the session change.");
  assert.equal(changed.full, undefined);
  assert.deepEqual(Object.keys(changed.sessions), [kept]);
  const streamed = (await changes.next()).value;
  assert.equal(streamed.full, undefined);
  assert.deepEqual(streamed.sessions, changed.sessions);
  assert.equal((await request(record, "DELETE", `kuvo/sessions/${gone}`)).status, 200);
  const removed = await until(async () => {
    const answer = await list(changed.at);
    return answer.json?.gone?.includes(gone) && answer.json;
  }, "The recorded cursor did not receive its tombstone.");
  assert.deepEqual(removed.sessions, {});
  assert.deepEqual((await changes.next()).value.gone, [gone]);
  followedChanges.close();
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

test("a project file change refiles the saved inventory without another engine read", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("project-filing-from-disk");
  const project = path.join(here.home, "work");
  const tree = path.join(here.home, "side-tree");
  const session = "ses_rehomeProject";
  for (const folder of [project, tree]) fs.mkdirSync(folder, { recursive: true });
  const projects = path.join(here.home, "box", "projects.json");
  const list = folders => JSON.stringify({ version: 1, projects: [{ directory: project, name: "Work", folders }] });
  write(projects, list([]));
  here.engine.sessions.push(sessionOf({ id: session, directory: tree }));
  const record = await here.start().ready();
  const before = await until(async () => {
    const reply = await request(record, "GET", "kuvo/sessions/list");
    return reply.json?.sessions?.[session]?.project === tree && reply.json;
  }, "the initial project filing");
  await until(() => here.engine.subscribers() > 0, "the resident to follow the engine");
  const calls = () => here.engine.inventoryReads ?? 0;
  const readBefore = calls();
  const release = Promise.withResolvers();
  t.after(release.resolve);
  here.engine.holdSessionList = release.promise;
  write(projects + ".next", list([tree]));
  fs.renameSync(projects + ".next", projects);
  let latest;
  const changed = await until(async () => {
    const reply = await request(record, "GET", "kuvo/sessions/list?since=" + encodeURIComponent(before.at));
    latest = reply.json;
    return latest?.sessions?.[session]?.project === project && latest;
  }, () => `The project change did not refile the saved engine inventory after ${calls() - readBefore} engine reads: ${JSON.stringify(latest)}`, 8000);
  assert.deepEqual(Object.keys(changed.sessions), [session]);
  assert.deepEqual(changed.projects[project].shown, [session]);
  assert.equal(changed.projects[tree], undefined);
  assert.equal(calls(), readBefore);

  const formerProject = path.join(here.home, "former-project");
  write(path.join(here.home, "box", "worktrees.json"), JSON.stringify({ handedOver: { [tree]: formerProject } }));
  const handedOver = await until(async () => {
    const reply = await request(record, "GET", "kuvo/sessions/list?since=" + encodeURIComponent(changed.at));
    return reply.json?.sessions?.[session]?.project === formerProject && reply.json;
  }, "the old Mac worktree record to refile its session", 8000);
  assert.deepEqual(handedOver.projects[formerProject].shown, [session]);
  assert.equal(calls(), readBefore);
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

test("Undo keeps a held draft first with its full text, author and display words", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("undo-first-held");
  const session = "ses_undoFirstHeld";
  const id = "msg_mine";
  const text = "The build is green.\n\n[Attached: /tmp/report.png]";
  const words = "The build is green.";
  const later = { id: "msg_later", type: "user", delivery: "queue", payload: { text: "Later." } };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  here.engine.inboxes.set(session, [{ id, type: "user", delivery: "steer", payload: { text,
    metadata: { kuvo: { parts: { words, mail: { name: "Ada", session: "ses_ada" } } } },
  } }, later]);
  here.engine.running.add(session);
  const record = await here.start().ready();
  const base = `kuvo/sessions/${session}`;
  const route = base + "/queue";
  assert.deepEqual((await request(record, "POST", base + "/stop")).json, { paused: true, held: [id] });
  here.engine.running.delete(session);
  const taken = await request(record, "DELETE", route + "/" + id + "?receipt=msg_undoHeld");
  assert.equal(taken.status, 200, taken.text);
  const { removed } = taken.json;
  assert.deepEqual([removed.id, removed.before, removed.text, removed.from], [id, later.id, text, "Ada"]);
  const back = await request(record, "POST", route + "/add", { ...removed, words, kept: true });
  assert.equal(back.status, 200, back.text);
  assert.deepEqual(back.json.held, [id]);
  assert.deepEqual(back.json.waiting.map(row => [row.id, row.text, row.from]), [[id, words, "Ada"]]);
  assert.equal((await request(record, "POST", route + "/resume")).status, 200);
  const prompt = here.engine.asked.find(row => row.method === "POST" && row.path.endsWith("/prompt")).json;
  assert.deepEqual([prompt.id, prompt.text, prompt.delivery], [id, text, "steer"]);
  assert.equal(later.delivery, "queue");
  here.engine.emit({ type: "session.execution.succeeded", data: { sessionID: session } });
  await until(() => later.delivery === "steer", "the second message to follow the restored draft");
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

test("assistant tool steps carry the same words on message pages and single reads", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("conversation-tool-words");
  const session = "ses_toolWords";
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const fixture = JSON.parse(fs.readFileSync(new URL("../../box/test/fixtures/message-words.json", import.meta.url)));
  write(path.join(here.home, "box", "opencode.json"), JSON.stringify({ mcp: { servers: Object.fromEntries(
    [...fixture.servers, "my-server-two"].map(id => [id, {}]),
  ) } }));
  const cases = fixture.cases.filter(entry => entry.message.type === "assistant").map((entry, index) => {
    const id = `msg_words${index}`;
    return { ...entry, message: { ...entry.message, id }, read: { ...entry.read, id } };
  });
  const call = (name, input) => ({ type: "tool", name, state: { status: "running", input } });
  const extra = [call("my-server-two_getIssue", {}), call("kuvo-browser_some_new_tool", {}),
    call("kuvo-browser_read_conversation", { of: "Mira" }), call("kuvo-browser_queue_add", { title: "Fix the sign-in" }),
    call("kuvo-browser_vault_run", {}), call("kuvo-browser_sandbox_ask", {}), call("atlassian_list_issues", {})];
  const expected = [
    ["My server two · Get issue", "My server two · Get issue"], ["Some new tool", "Some new tool"],
    ["Read Mira’s conversation", "Reading Mira’s conversation"],
    ["Added “Fix the sign-in” to the Work Queue", "Adding “Fix the sign-in” to the Work Queue"],
    ["Ran a command with vault values", "Running a command with vault values"],
    ["Asked to run a command outside the sandbox", "Asking to run a command outside the sandbox"],
    ["Jira and Confluence · List issues", "Jira and Confluence · List issues"],
  ];
  cases.push({ name: "tool catalogs and the longest configured prefix", message: { type: "assistant", id: "msg_extra", content: extra },
    read: { type: "assistant", id: "msg_extra", content: extra.map((step, index) => ({ ...step, title: expected[index][0], doing: expected[index][1] })) } });
  const failures = [
    ["edit", { error: { type: "tool", message: "The tool use was rejected." } }, "Edited 1 file", "declined", "The tool use was rejected."],
    ["shell", { error: { type: "aborted", message: "Not running." } }, "Ran 1 command", "stopped", "Not running."],
    ["read", { error: "Tool error: <tool_use_error>File does not exist. /tmp/a</tool_use_error>" }, "Read 1 file", "not found", "File does not exist. /tmp/a"],
    ["shell", { error: { message: "Exit code 1\nnpm ERR! missing script: lint" } }, "Ran 1 command", "exit code 1", "Exit code 1\nnpm ERR! missing script: lint"],
    ["grep", { error: "Error: ripgrep failed to start" }, "Searched once", "failed", "ripgrep failed to start"],
    ["shell", { content: [{ type: "text", text: "Permission denied: /tmp/a" }] }, "Ran 1 command", "blocked", "Permission denied: /tmp/a"],
    ["webfetch", {}, "Read 1 web page", "failed", "It failed without saying why."],
  ];
  const failed = failures.map(([name, state]) => ({ type: "tool", name, state: { status: "failed", ...state } }));
  cases.push({ name: "failure reasons and the original step", message: { type: "assistant", id: "msg_failed", content: failed },
    read: { type: "assistant", id: "msg_failed", content: failed.map((step, index) => {
      const [, , said, reason, error] = failures[index];
      return { ...step, failure: { line: `${said} · ${reason}`, reason, error } };
    }) } });
  here.engine.messages.set(session, cases.map(entry => entry.message));
  const record = await here.start().ready();
  const route = `api/session/${session}/message`;
  const page = await request(record, "GET", route);
  assert.equal(page.status, 200, page.text);
  for (const [index, entry] of cases.entries()) {
    assert.deepEqual(page.json.data[index], entry.read, entry.name);
    const single = await request(record, "GET", route + "/" + entry.message.id);
    assert.equal(single.status, 200, single.text);
    assert.deepEqual(single.json.data, entry.read, entry.name);
  }
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
    assert.equal((await request(record, "POST", path + "/queue/hold")).json.paused, true);
    const letter = { id: "msg_followUp", from: "Ada", words: "The build is green.", text: "[Letter] The build is green.", kept: true };
    assert.equal((await request(record, "POST", path + "/queue/add", letter)).json.waits, true);
    const now = await request(record, "POST", path + "/queue/msg_C/now");
    assert.equal(now.status, 200);
    assert.equal(now.json.paused, false);
    assert.deepEqual(now.json.waiting.map(row => [row.id, row.from, row.text]), [[letter.id, "Ada", letter.words]]);
    assert.deepEqual(here.engine.interrupted, [{ session: id, resume: true }]);
    assert.equal(here.engine.inboxes.get(id).find(item => item.id === "msg_C").delivery, "steer");
    assert.equal(here.engine.inboxes.get(id).find(item => item.id === "msg_A").delivery, "queue");
    assert.equal((await request(record, "POST", path + "/queue/msg_missing/now")).status, 404);
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
