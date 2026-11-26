// Session and message behavior through the selected resident and its engine API.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { DatabaseSync } from "node:sqlite";
import { boxBinary, isolatedEnv, openLine, readJSON, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { git, sessionOf } from "./engine.mjs";

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

/** A computer whose conversations, `ids` (or `[id, options]`), are its Grok's (`acp.mjs`). */
async function onAgents(t, name, ids = [], { start = true } = {}) {
  const { computer } = await bench(t, { agents: true });
  const here = await computer(name);
  for (const each of ids) {
    const [id, options] = Array.isArray(each) ? each : [each, {}];
    here.agents.seed(id, { directory: here.home, ...options });
  }
  const record = start ? await here.start().ready() : undefined;
  return { here, record };
}
const desktop = { "x-kuvo-from": "app" };
const said = text => ({ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } });
/** A conversation's queue as the app follows it: its first frame on the line. */
async function queueItems(record, id) {
  const watch = Buffer.from(JSON.stringify({ q: `kuvo/sessions/${id}/queue/events` })).toString("base64url");
  const line = await openLine(record, "?watch=" + watch, undefined, desktop);
  try {
    for await (const frame of line.frames()) if (frame.t === "q") return frame.d.items;
  } finally {
    line.close();
  }
}
/** Its rows from the user, once there are `count`. */
async function userRows(here, record, id, count) {
  let rows = [];
  await until(async () => (rows = (await here.agents.rows(record, id)).filter(row => row.type === "user")).length >= count,
    () => `Only ${rows.length} of its messages arrived: ${JSON.stringify(rows).slice(-800)}`, 20000, 50);
  return rows;
}

test("send distinguishes your text from a replay", { timeout: 45000 }, async t => {
  const target = "ses_send";
  const { here, record } = await onAgents(t, "engine-send-envelope", [target]);
  const send = body => request(record, "POST", "kuvo/sessions/" + target + "/send", body, desktop);
  for (const fields of [{ words: "  " }, { text: " " }]) assert.equal((await send(fields)).status, 400);
  const absent = await request(record, "POST", "kuvo/sessions/ses_missing/send", { words: "Here" }, desktop);
  assert.equal(absent.status, 404, absent.text);
  const unlisted = await send({ id: "msg_unknown", words: "Ask &Nova", named: ["ses_nova"] });
  assert.equal(unlisted.status, 409, unlisted.text);
  assert.match(unlisted.json.message, /isn’t known|isn't known/);
  assert.deepEqual(here.agents.prompts(), []);

  const body = { id: "msg_command", words: "/review-quick main", keyword: "/plan", command: "review-quick" };
  assert.equal((await send(body)).status, 200);
  assert.equal((await send(body)).status, 200);
  const [command] = await userRows(here, record, target, 1);
  assert.deepEqual([command.id, command.text, command.metadata.kuvo.parts], [body.id, body.words, { words: body.words }]);

  const letter = "[Message from Ada · ses_ada] (To answer, use send_message to ses_ada.)\n\nAsk again";
  assert.equal((await send({ id: "msg_asWritten", words: letter })).status, 200);
  assert.deepEqual((await userRows(here, record, target, 2))[1].metadata.kuvo.parts, { words: letter });
  const replay = await send({ id: "msg_replay", text: letter, files: [{ uri: "data:image/png;base64,cG5n", name: "a.png" }] });
  assert.equal(replay.status, 200, replay.text);
  const written = (await userRows(here, record, target, 3))[2];
  assert.ok(written.text.startsWith(letter + "\n\n<attached-files>\n"), written.text);
  const { parts } = written.metadata.kuvo;
  assert.deepEqual(parts.mail, { name: "Ada", session: "ses_ada" });
  assert.ok(parts.words.startsWith("Ask again\n\n<attached-files>"), parts.words);
  const saved = written.text.split("\n").find(line => line.endsWith(path.sep + "a.png"));
  assert.equal(fs.readFileSync(saved, "utf8"), "png");
  await until(() => here.agents.prompts().length === 3, "Its messages never all reached the agent.", 20000);
  assert.deepEqual(here.agents.prompts().map(text => text.split("\n")[0]), [body.words, letter.split("\n")[0], letter.split("\n")[0]]);
});

test("queue add preserves fields, replays, and saved parts", { timeout: 45000 }, async t => {
  const target = "ses_queue";
  const { here, record } = await onAgents(t, "engine-queue-envelope", [target]);
  const add = body => request(record, "POST", "kuvo/sessions/" + target + "/queue/add", body, desktop);
  const draft = { id: "msg_fresh", words: "After this", widget: "Plans", delivery: "queue" };
  const fresh = await add(draft);
  assert.equal(fresh.status, 200, fresh.text);
  assert.equal(fresh.json.waits, false);
  const [first] = await userRows(here, record, target, 1);
  assert.equal(first.text, "After this\n\n<from-widget>\nThe user sent this from a widget shown in this conversation (show_widget), titled:\nPlans\n</from-widget>");
  assert.deepEqual(first.metadata.kuvo.parts, { words: "After this", widget: "Plans" });
  const retry = await add(draft);
  assert.equal(retry.status, 200, retry.text);
  assert.equal(retry.json.waits, false);

  const letter = "[Message from Ada · ses_ada] (To answer, use send_message to ses_ada.)\n\nThe build is green.";
  const again = await add({ id: "msg_letter", text: letter, again: true, words: "The build is green.", delivery: "queue" });
  assert.equal(again.status, 200, again.text);
  const parts = { words: "Check CI", widget: "Build" };
  const kept = await add({ id: "msg_kept", text: "Check CI", words: "Check CI", kept: true, metadata: { kuvo: { parts } } });
  assert.equal(kept.status, 200, kept.text);
  const rows = await userRows(here, record, target, 3);
  assert.deepEqual(rows.map(row => row.id), [draft.id, "msg_letter", "msg_kept"]);
  assert.equal(rows[1].text, letter);
  assert.deepEqual(rows[1].metadata.kuvo.parts, { words: "The build is green.", mail: { name: "Ada", session: "ses_ada" } });
  assert.deepEqual(rows[2].metadata.kuvo.parts, parts);
});

test("queue Add says when there's no such conversation", { timeout: 45000 }, async t => {
  const { here, record } = await onAgents(t, "queue-missing");
  const added = await request(record, "POST", "kuvo/sessions/ses_missing/queue/add", { id: "msg_nowhere", text: "Check the notes." }, desktop);
  assert.equal(added.status, 404, added.text);
  assert.deepEqual(here.agents.prompts(), []);
});

test("a stopped queue refuses Add when the payload cannot be saved and accepts a clean retry", { timeout: 45000 }, async t => {
  const session = "ses_saveQueue";
  const { here, record: resident } = await onAgents(t, "queue-save-failure", [session]);
  const id = "msg_saveQueue";
  const route = `kuvo/sessions/${session}`;
  const queue = route + "/queue";
  assert.equal((await request(resident, "POST", route + "/stop", undefined, desktop)).status, 200);
  const file = path.join(here.home, "run", "waiting", id + ".json");
  fs.mkdirSync(file, { recursive: true });
  const add = () => request(resident, "POST", queue + "/add", { id, words: "Save it." }, desktop);
  const failed = await add();
  assert.equal(failed.status, 500, failed.text);
  assert.match(failed.json.message, /couldn.t save the message/);
  const saved = path.join(here.home, "run", "queues.json");
  assert.deepEqual(readJSON(saved)[session].waiting ?? [], []);
  fs.rmdirSync(file);
  if (process.platform !== "win32" && process.getuid?.() !== 0) {
    const run = path.dirname(saved);
    const before = fs.statSync(run).mode & 0o777;
    fs.chmodSync(run, 0o500);
    try {
      const refused = await add();
      assert.equal(refused.status, 500, refused.text);
      assert.deepEqual(readJSON(saved)[session].waiting ?? [], []);
      if (fs.existsSync(file)) assert.match(readJSON(file).text, /Save it/);
    } finally {
      fs.chmodSync(run, before);
    }
  }
  const retried = await add();
  assert.equal(retried.status, 200, retried.text);
  assert.equal(retried.json.waits, true);
  assert.deepEqual(readJSON(saved)[session].waiting.map(row => row.id), [id]);
  assert.equal(readJSON(file).text, "Save it.");
  assert.deepEqual(here.agents.prompts(), []);
});

test("rename and archive keep session notes, and a subagent goes with its conversation", { timeout: 45000 }, async t => {
  const notes = { kuvo: { startedBy: "ses_starter", archived: true }, another: { kept: true } };
  const { here, record: resident } = await onAgents(t, "engine-actions", [
    ["ses_project", { title: "Before", metadata: notes }],
    ["ses_child", { title: "Child", parent: "ses_project" }],
    ["ses_seat", { title: "Seat", metadata: { kuvo: { seat: "grp_test" } } }],
    ["ses_hidden", { title: "Internal", metadata: { kuvo: { engineFor: "ses_project" } } }],
  ]);
  const route = "kuvo/sessions/ses_project";
  const call = (method, at, body) => request(resident, method, at, body, desktop);
  assert.equal((await call("GET", "kuvo/sessions/ses%E0/actions")).status, 403);
  const rename = await call("POST", route + "/rename", { title: "  After  " });
  assert.equal(rename.status, 200, rename.text);
  assert.equal(rename.json.info.title, "After");
  assert.deepEqual(rename.json.info.metadata, notes);
  assert.equal((await call("POST", route + "/rename", { title: "  " })).status, 400);

  for (const pinned of [true, false]) {
    const changed = await call("POST", route + "/pin", { pinned });
    assert.equal(changed.status, 200, changed.text);
    assert.deepEqual(changed.json.info.metadata, { ...notes, "kuvo.pinned": pinned });
  }

  const stale = await call("POST", route + "/archive", { archived: false, was: { updated: 0, archived: true } });
  assert.equal(stale.status, 409);
  assert.equal(stale.json.message, "It changed since you read it.");
  const updated = (await call("GET", "kuvo/agents/api/session/ses_project")).json.data.time.updated;
  const unarchive = await call("POST", route + "/archive", { archived: false, was: { updated, archived: true } });
  assert.equal(unarchive.status, 200, unarchive.text);
  assert.deepEqual(unarchive.json.info.metadata, {
    kuvo: { startedBy: "ses_starter" }, another: { kept: true }, "kuvo.archived": false, "kuvo.pinned": false,
  });
  const child = await call("DELETE", "kuvo/sessions/ses_child");
  assert.equal(child.status, 409);
  assert.match(child.json.message, /subagent/);
  for (const [id, message] of [["ses_seat", /seat/], ["ses_hidden", /another conversation/]]) {
    const refused = await call("DELETE", "kuvo/sessions/" + id);
    assert.equal(refused.status, 409, refused.text);
    assert.match(refused.json.message, message);
  }
  assert.equal((await call("POST", route + "/stop", {})).status, 200);
  const queued = { id: "msg_deletedWithSession", words: "Saved for later", delivery: "queue" };
  const kept = await call("POST", route + "/send", queued);
  assert.deepEqual(kept.json, { waits: true });
  const waiting = path.join(here.home, "run/waiting", queued.id + ".json");
  assert.equal(fs.existsSync(waiting), true);
  const deleted = await call("DELETE", route);
  assert.equal(deleted.status, 200, deleted.text);
  assert.deepEqual(deleted.json, { session: "ses_project" });
  assert.deepEqual((await call("DELETE", route)).json, deleted.json);
  assert.equal((await call("DELETE", route, { was: { archived: false, updated: 1 } })).status, 409);
  assert.equal((await call("GET", "kuvo/agents/api/session/ses_project")).status, 404);
  await until(() => !fs.existsSync(waiting), "The deleted session's waiting payload was retained.");
  assert.deepEqual(here.agents.prompts(), []);
});

test("concurrent session note writes keep each other's notes", { timeout: 45000 }, async t => {
  const id = "ses_notes";
  const initial = { pinned: "another owner’s note", kuvo: { approval: "auto", startedBy: "ses_first" } };
  const { record: resident } = await onAgents(t, "session-note-turns", [[id, { metadata: initial }], "ses_otherNotes"]);
  const edit = (id, field, value) => request(resident, "POST",
    `kuvo/sessions/${id}/${field === "pinned" ? "pin" : "archive"}`, { [field]: value }, desktop);
  const notes = async () => (await request(resident, "GET", `kuvo/agents/api/session/${id}`, undefined, desktop)).json.data.metadata;
  const both = await Promise.all([edit(id, "pinned", true), edit(id, "archived", true), edit("ses_otherNotes", "pinned", true)]);
  assert.deepEqual(both.map(answer => answer.status), [200, 200, 200]);
  assert.deepEqual(await notes(), { ...initial, "kuvo.archived": true, "kuvo.pinned": true });
  const changed = await Promise.all([edit(id, "pinned", false), edit(id, "archived", false)]);
  assert.deepEqual(changed.map(answer => answer.status), [200, 200]);
  assert.deepEqual(await notes(), { ...initial, "kuvo.archived": false, "kuvo.pinned": false });
  assert.equal((await edit("ses_absent", "pinned", true)).status, 404);
});

test("session note writers share older lock paths and remove only stale locks", { timeout: 45000 }, async t => {
  const session = "ses_noteLock";
  const other = "ses_otherNoteLock";
  const { here } = await onAgents(t, "compatible-session-note-locks", [session, other], { start: false });
  const notes = path.join(here.home, "run", "notes");
  const lock = id => path.join(notes, createHash("sha256").update(id).digest("hex").slice(0, 32) + ".lock");
  const live = lock(session);
  const stale = lock("ses_staleLock");
  const old = path.join(here.home, "run", "work", ".notes.lock");
  for (const file of [live, stale]) fs.mkdirSync(file, { recursive: true });
  const then = new Date(Date.now() - 90000);
  fs.utimesSync(stale, then, then);
  const record = await here.start().ready();
  await until(() => !fs.existsSync(stale), "the startup check to clear abandoned session locks", 4000);
  assert.equal(fs.existsSync(live), true);
  const rename = (id, title) => request(record, "POST", `kuvo/sessions/${id}/rename`, { title }, desktop);
  const title = async () => (await request(record, "GET", `kuvo/agents/api/session/${session}`, undefined, desktop)).json.data.title;
  const first = rename(session, "After the other writer");
  first.catch(() => {});
  try {
    assert.equal((await rename(other, "Independent writer")).status, 200);
    await new Promise(resolve => setTimeout(resolve, 150));
    assert.equal(await title(), "ACP fixture");
  } finally {
    fs.rmdirSync(live);
  }
  assert.equal((await first).status, 200);
  fs.mkdirSync(old, { recursive: true });
  const second = rename(session, "After the older writer");
  second.catch(() => {});
  try {
    await new Promise(resolve => setTimeout(resolve, 150));
    assert.equal(await title(), "After the other writer");
    assert.equal(fs.existsSync(old), true);
  } finally {
    fs.rmdirSync(old);
  }
  assert.equal((await second).status, 200);
  assert.equal(await title(), "After the older writer");
  assert.equal(fs.existsSync(live), false);
});

test("Delete with an earlier revision refuses a session changed since", { timeout: 45000 }, async t => {
  const session = "ses_delete";
  const { record } = await onAgents(t, "engine-delete-revision", [[session, { title: "A saved conversation", metadata: { "kuvo.archived": true } }]]);
  const route = "kuvo/sessions/" + session;
  const actions = await request(record, "GET", route + "/actions", undefined, desktop);
  assert.equal(actions.status, 200, actions.text);
  const confirm = actions.json.actions.find(row => row.id === "delete").confirm;
  const remove = was => request(record, "DELETE", route, was === undefined ? undefined : { was }, desktop);
  for (const was of [{ updated: 1, archived: false, confirm }, { updated: 0, archived: true, confirm }]) {
    const stale = await remove(was);
    assert.equal(stale.status, 409, stale.text);
    assert.equal(stale.json.message, "It changed since you read it.");
  }
  assert.equal((await remove({ updated: 1, archived: true, confirm })).status, 200);
  assert.equal((await request(record, "GET", "kuvo/agents/api/session/" + session, undefined, desktop)).status, 404);
});

test("a busy conversation can steer, edit and remove messages in its queue", { timeout: 45000 }, async t => {
  const session = "ses_queue";
  const { here, record: resident } = await onAgents(t, "engine-queue", [[session, { title: "Busy" }]]);
  here.agents.script("Keep busy", [said("Working"), "hang"]);
  const at = "kuvo/sessions/" + session;
  const send = (id, words, delivery = "queue") => request(resident, "POST", at + "/send", { id, words, delivery }, desktop);
  assert.equal((await send("msg_busy", "Keep busy", "steer")).status, 200);
  await until(() => here.agents.prompts().length === 1, "Its turn never started.", 20000);
  assert.equal((await send("msg_first", "first")).status, 200);
  assert.equal((await send("msg_edit", "edit me")).status, 200);
  assert.equal((await send("msg_drop", "take back")).status, 200);
  const line = () => queueItems(resident, session);

  const steer = await request(resident, "POST", at + "/queue/msg_first/steer", undefined, desktop);
  assert.equal(steer.status, 200, steer.text);
  assert.deepEqual(here.agents.sent("session/cancel"), []);
  assert.deepEqual((await line()).map(row => [row.id, row.delivery]),
    [["msg_first", "steer"], ["msg_edit", "queue"], ["msg_drop", "queue"]]);
  const removed = await request(resident, "DELETE", at + "/queue/msg_edit?receipt=msg_undo", undefined, desktop);
  assert.equal(removed.status, 200, removed.text);
  assert.equal(removed.json.removed.words, "edit me");
  assert.equal(removed.json.removed.delivery, "queue");
  assert.equal(removed.json.removed.id, "msg_edit");
  assert.equal(removed.json.removed.before, "msg_drop");
  assert.equal(removed.json.removed.receipt, "msg_undo");
  assert.equal((await request(resident, "DELETE", at + "/queue/msg_drop", undefined, desktop)).status, 200);
  assert.equal((await request(resident, "DELETE", at + "/queue/msg_missing", undefined, desktop)).status, 404);
  assert.equal((await send("msg_changed", "edited words")).status, 200);
  assert.deepEqual((await line()).map(row => [row.id, row.words]), [["msg_first", "first"], ["msg_changed", "edited words"]]);
});

/** A conversation on its Grok, at work on a turn that doesn't end by itself. */
async function busyOnAgents(t, name, id, options) {
  const { here, record } = await onAgents(t, name, [[id, options ?? {}]]);
  here.agents.script("Keep busy", [said("Working"), "hang"]);
  const sent = await request(record, "POST", `kuvo/sessions/${id}/send`, { id: "msg_keepBusy", words: "Keep busy", delivery: "steer" }, desktop);
  assert.equal(sent.status, 200, sent.text);
  await until(() => here.agents.prompts().length === 1, "Its turn never started.", 20000);
  return { here, record };
}

// (The reference doesn't mark a held steer in an agent's line, nor name its rows' files.)
const heldUnmarked = boxBinary().typescript && "The reference neither marks a held steer in a coding agent's line nor names the files on its queue rows.";
test("a stopped queue shows messages in order and Edit returns carried and retained files", { timeout: 45000, skip: heldUnmarked }, async t => {
  const session = "ses_editFiles";
  const { record: resident } = await busyOnAgents(t, "queue-edit-files", session);
  const id = "msg_editFiles";
  const route = `kuvo/sessions/${session}`;
  const queue = route + "/queue";
  const words = "Look at both attachments.";
  const picture = { name: "shot.png", uri: "data:image/png;base64,YQ==" };
  const zip = { name: "logs.zip", uri: "data:application/zip;base64,UEsDBA==" };
  const attached = { name: "held.png", uri: "data:image/png;base64,Yg==" };
  const call = (method, at, body) => request(resident, method, at, body, desktop);
  // What waits in its line: a steer with a file, and a letter.
  assert.equal((await call("POST", route + "/send", { id: "msg_heldFile", words: "First.", files: [attached], delivery: "steer" })).status, 200);
  const letter = "[Message from Mira · ses_mira] (To answer, use send_message to ses_mira.)\n\nCheck this.";
  assert.equal((await call("POST", route + "/send", { id: "msg_mail", text: letter, delivery: "queue" })).status, 200);
  assert.equal((await call("POST", queue + "/hold")).json.paused, true);
  for (const invalid of [route + "/send", queue + "/add"]) {
    const refusal = await call("POST", invalid, { id: "msg_bad_id", text: "Keep it.", kept: true, delivery: "queue" });
    assert.equal(refusal.status, 400, refusal.text);
    assert.match(refusal.json.message, /Send it again as a new message/);
  }
  const send = await call("POST", queue + "/add", { id, words, files: [picture, zip] });
  assert.equal(send.status, 200, send.text);
  assert.equal(send.json.waits, true);
  const found = await until(async () => {
    const response = await call("GET", queue + "?items=1");
    const items = await queueItems(resident, session);
    return items?.length === 3 && { ...response.json, items };
  }, "the queue to show the two waiting messages and the saved message");
  assert.deepEqual(found.held, ["msg_heldFile"]);
  assert.deepEqual(found.items.map(item => [item.id, item.words, item.files, item.from, item.held]), [
    ["msg_heldFile", "First.", [attached.name], null, true],
    ["msg_mail", "Check this.", [], "Mira", false],
    [id, words, [picture.name], null, false],
  ]);
  assert.equal(found.items[2].on, "box");
  const editHeld = await call("DELETE", queue + "/msg_heldFile?receipt=msg_editHeld");
  assert.equal(editHeld.status, 200, editHeld.text);
  assert.deepEqual(editHeld.json.removed.attached.map(file => file.name), [attached.name]);
  assert.equal(editHeld.json.removed.before, "msg_mail");
  assert.deepEqual(editHeld.json.held, []);
  const edit = await call("DELETE", queue + "/" + id + "?receipt=msg_editBoth");
  assert.equal(edit.status, 200, edit.text);
  assert.equal(edit.json.removed.draft, words);
  assert.deepEqual(edit.json.removed.files, [picture]);
  assert.deepEqual(edit.json.removed.attached.map(file => file.name), ["shot.png", "logs.zip"]);
  assert.equal(edit.json.removed.attached[0].uri, picture.uri);
  assert.deepEqual(fs.readFileSync(new URL(edit.json.removed.attached[1].uri)), Buffer.from("PK\x03\x04"));
  for (const [item, receipt] of [["msg_heldFile", "msg_editHeld"], [id, "msg_editBoth"]]) {
    assert.equal((await call("POST", queue + "/" + item + "/ack", { receipt })).status, 200);
    assert.equal((await call("DELETE", queue + "/" + item + "?receipt=" + receipt)).status, 404);
  }
});

test("wake-up rows show the same saved summary in the agent's line or the resident queue", { timeout: 45000 }, async t => {
  const session = "ses_wakeSummary";
  const { record } = await busyOnAgents(t, "queue-wake-summary", session);
  const words = "Check everything. More instructions here.";
  const text = "[Wake-up] " + words;
  const summary = "Check the release";
  const metadata = { kuvo: { wake: { summary } } };
  const route = `kuvo/sessions/${session}/queue`;
  const call = (method, at, body) => request(record, method, at, body, desktop);
  const line = await call("POST", route + "/add", { id: "msg_lineWake", text, metadata, words, from: "Wake-up", kept: true, delivery: "queue" });
  assert.equal(line.json.waits, false, line.text);
  assert.equal((await call("POST", route + "/hold")).json.paused, true);
  const add = await call("POST", route + "/add", { id: "msg_boxWake", text, metadata, words, from: "Wake-up", kept: true });
  assert.equal(add.json.waits, true);
  const items = await until(async () => {
    const items = await queueItems(record, session);
    return items?.length === 2 && items;
  }, "both wake-up rows to be listed");
  assert.deepEqual(items.map(row => [row.id, row.from, row.words]), [["msg_lineWake", "Wake-up", summary], ["msg_boxWake", "Wake-up", summary]]);
  assert.equal(items[1].on, "box");
  const taken = await call("DELETE", route + "/msg_boxWake");
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

test("Edit says a message already delivered isn't waiting", { timeout: 45000 }, async t => {
  const session = "ses_editRefused";
  const { here, record: resident } = await busyOnAgents(t, "queue-edit-refused", session);
  const queue = `kuvo/sessions/${session}/queue`;
  const gone = await request(resident, "DELETE", queue + "/msg_keepBusy", undefined, desktop);
  assert.equal(gone.status, 404, gone.text);
  assert.equal(gone.json.removed, undefined);
  assert.deepEqual(await here.agents.users(resident, session), ["msg_keepBusy"]);
});

test("Send Now on a stopped queue hands its agent the saved message and retires its waiting copy", { timeout: 45000 }, async t => {
  const session = "ses_unconfirmed";
  const id = "msg_unconfirmed";
  const { here, record: resident } = await onAgents(t, "queue-unconfirmed", [session]);
  const queue = `kuvo/sessions/${session}/queue`;
  assert.equal((await request(resident, "POST", `kuvo/sessions/${session}/stop`, undefined, desktop)).status, 200);
  assert.equal((await request(resident, "POST", queue + "/add", { id, words: "Wait for proof.", delivery: "queue" }, desktop)).json.waits, true);
  const file = path.join(here.home, "run", "waiting", id + ".json");
  assert.equal(fs.existsSync(file), true);
  assert.deepEqual((await request(resident, "GET", queue, undefined, desktop)).json.waiting.map(row => row.id), [id]);
  const now = await request(resident, "POST", queue + "/" + id + "/now", undefined, desktop);
  assert.equal(now.status, 200, now.text);
  await until(() => !fs.existsSync(file), "The sent message's waiting copy stayed.", 20000);
  assert.deepEqual(await userRows(here, resident, session, 1).then(rows => rows.map(row => row.id)), [id]);
  await until(() => here.agents.prompts().includes("Wait for proof."), "It never reached the agent.", 20000);
});

test("an old unconfirmed handoff does not become safe to retry", { timeout: 45000 }, async t => {
  const session = "ses_oldHandoff";
  const id = "msg_oldHandoff";
  const { here } = await onAgents(t, "queue-legacy-handoff", [session], { start: false });
  const item = `kuvo/sessions/${session}/queue/${id}`;
  write(path.join(here.home, "run", "queues.json"), JSON.stringify({ [session]: { paused: true, held: [], handoff: [id], waiting: [{ id, text: "Pending.", at: 1 }] } }));
  const file = path.join(here.home, "run", "waiting", id + ".json");
  write(file, JSON.stringify({ id, text: "Pending.", delivery: "queue" }));
  const resident = await here.start().ready();
  assert.equal((await request(resident, "POST", item + "/now", undefined, desktop)).status, 502);
  assert.equal((await request(resident, "DELETE", item, undefined, desktop)).status, 502);
  assert.equal(fs.existsSync(file), true);
  assert.deepEqual(here.agents.prompts(), []);
});

test("the queue line follows its agent's line, and a conversation that isn't here has none", { timeout: 45000 }, async t => {
  const session = "ses_queue";
  const { record } = await busyOnAgents(t, "engine-queue-line", session);
  const route = "kuvo/sessions/" + session;
  const send = (id, words, delivery) => request(record, "POST", route + "/send", { id, words, delivery }, desktop);
  assert.equal((await send("msg_first", "Up next", "queue")).status, 200);
  assert.equal((await send("msg_second", "Afterwards", "queue")).status, 200);
  const reading = await openLine(record, "", route + "/queue/events", desktop);
  t.after(() => reading.close());
  const frames = reading.frames();
  assert.deepEqual((await frames.next()).value.items.map(item => item.id), ["msg_first", "msg_second"]);
  assert.deepEqual((await request(record, "GET", route + "/queue", undefined, desktop)).json, { paused: false, held: [] });
  assert.equal((await send("msg_third", "One more thing.", "queue")).status, 200);
  for (;;) {
    const { value } = await frames.next();
    if (value.items?.length === 3) {
      assert.deepEqual(value.items.map(({ id, words }) => [id, words]), [
        ["msg_first", "Up next"], ["msg_second", "Afterwards"], ["msg_third", "One more thing."],
      ]);
      break;
    }
  }
  const missing = await request(record, "GET", "kuvo/sessions/ses_missing/queue?items=1", undefined, desktop);
  assert.equal(missing.status, 200, missing.text);
  assert.deepEqual(missing.json, { paused: false, held: [], items: null });
});

test("a reordered stopped queue drains in its new order once resumed", { timeout: 60000 }, async t => {
  const session = "ses_reorderInterleave";
  const { here, record } = await onAgents(t, "queue-slow-reorder", [session]);
  const base = `kuvo/sessions/${session}`;
  const queue = base + "/queue";
  const drafts = ["msg_a", "msg_b", "msg_c", "msg_d"];
  assert.equal((await request(record, "POST", base + "/stop", undefined, desktop)).status, 200);
  const add = id => request(record, "POST", queue + "/add", { id, words: "Draft " + id }, desktop);
  for (const id of drafts.slice(0, 3)) assert.equal((await add(id)).json.waits, true);
  const ordered = await request(record, "POST", queue + "/reorder", { item: drafts[2], anchor: drafts[1], before: true }, desktop);
  assert.equal(ordered.status, 200, ordered.text);
  assert.deepEqual(ordered.json.items.map(item => item.id), [drafts[0], drafts[2], drafts[1]]);
  assert.equal((await add(drafts[3])).json.waits, true);
  assert.equal((await request(record, "POST", queue + "/" + drafts[0] + "/now", undefined, desktop)).status, 200);
  assert.equal((await request(record, "POST", queue + "/resume", undefined, desktop)).status, 200);
  const expected = [drafts[0], drafts[2], drafts[1], drafts[3]];
  await until(() => here.agents.prompts().length === 4, () => "Not every draft reached the agent: " + JSON.stringify(here.agents.prompts()), 30000);
  assert.deepEqual(here.agents.prompts(), expected.map(id => "Draft " + id));
  assert.deepEqual(await here.agents.users(record, session), expected);
});

test("legacy waiting queues read and future formats are refused before removal or Resume", { timeout: 45000 }, async t => {
  const session = "ses_queue";
  const id = "msg_saved";
  const { here, record } = await onAgents(t, "engine-queue-format", [session]);
  const base = "kuvo/sessions/" + session;
  const queue = base + "/queue";
  assert.equal((await request(record, "POST", base + "/stop", {}, desktop)).status, 200);
  const saved = await request(record, "POST", base + "/send", { id, words: "Keep the notes.", delivery: "queue" }, desktop);
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
  await until(async () => (await queueItems(record, session))?.some(item => item.id === id), "the legacy waiting row to be read");
  const future = JSON.stringify({ ...prompt, formatVersion: 2 });
  write(payload, future);
  for (const [method, route] of [["DELETE", queue + "/" + id], ["POST", queue + "/resume"]]) {
    const refused = await request(record, method, route, undefined, desktop);
    assert.equal(refused.status, 409, refused.text);
    assert.equal(refused.json.code, "unsupportedFormatVersion");
    assert.equal(fs.readFileSync(payload, "utf8"), future);
    assert.equal(fs.readFileSync(index, "utf8"), older);
  }
  assert.deepEqual(here.agents.prompts(), []);
  book[session].formatVersion = 2;
  const futureIndex = JSON.stringify(book);
  write(index, futureIndex);
  const refused = await request(record, "POST", queue + "/hold", {}, desktop);
  assert.equal(refused.status, 409, refused.text);
  assert.equal(refused.json.code, "unsupportedFormatVersion");
  assert.equal(fs.readFileSync(index, "utf8"), futureIndex);
});

test("the next queued message waits for the agent's turn to end", { timeout: 45000 }, async t => {
  const session = "ses_laterDrain";
  const { here, record } = await onAgents(t, "drain-after-turn", [session]);
  here.agents.script("Long task", [said("Working"), { until: "long-done" }]);
  const send = (id, words, delivery) => request(record, "POST", `kuvo/sessions/${session}/send`, { id, words, delivery }, desktop);
  assert.equal((await send("msg_long", "Long task", "steer")).status, 200);
  await until(() => here.agents.prompts().length === 1, "Its turn never started.", 20000);
  for (const [id, words] of [["msg_first", "First."], ["msg_second", "Second."]]) assert.equal((await send(id, words, "queue")).status, 200);
  await new Promise(resolve => setTimeout(resolve, 1000));
  assert.deepEqual(here.agents.prompts(), ["Long task"]);
  here.agents.go("long-done");
  await until(() => here.agents.prompts().length === 3, () => "The queued messages never went: " + JSON.stringify(here.agents.prompts()), 20000);
  assert.deepEqual(here.agents.prompts(), ["Long task", "First.", "Second."]);
  assert.deepEqual(here.agents.sent("session/cancel"), []);
});

test("after a failure only your own message to steer opens the queue again", { timeout: 45000 }, async t => {
  const session = "ses_afterFailure";
  const { here, record } = await onAgents(t, "queue-written-by-you", [session, "ses_writer"]);
  here.agents.script("Fails now", [said("Trying"), { until: "fail-now" }, { error: { code: -32603, message: "It failed." } }]);
  const route = `kuvo/sessions/${session}`;
  const send = (id, body) => request(record, "POST", route + "/send", { id, ...body }, desktop);
  assert.equal((await send("msg_fails", { words: "Fails now", delivery: "steer" })).status, 200);
  await until(() => here.agents.prompts().length === 1, "Its turn never started.", 20000);
  assert.equal((await send("msg_queue", { words: "Later.", delivery: "queue" })).status, 200);
  here.agents.go("fail-now");
  const queue = () => request(record, "GET", route + "/queue", undefined, desktop);
  await until(async () => (await queue()).json?.paused, "The failed turn did not hold its queue.", 20000, 100);
  // Another agent's letter isn't yours: it waits.
  const letter = await request(record, "POST", "kuvo/agent-tools",
    { name: "send_message", session: "ses_writer", arguments: { to: session, text: "A letter about it." } }, desktop);
  assert.equal(letter.status, 200, letter.text);
  await new Promise(resolve => setTimeout(resolve, 500));
  assert.equal((await queue()).json.paused, true);
  assert.deepEqual(here.agents.prompts(), ["Fails now"]);
  assert.equal((await send("msg_yours", { words: "Try with the new model.", delivery: "steer" })).status, 200);
  await until(async () => (await queue()).json?.paused === false, "Your steer did not open the queue.", 20000, 100);
  await until(() => here.agents.prompts().some(text => text.includes("Try with the new model.")), "Your steer never reached the agent.", 20000);
});

test("Stop holds a steer ahead of the queue until Resume", { timeout: 45000, skip: heldUnmarked }, async t => {
  const session = "ses_stopOrder";
  const { here, record } = await busyOnAgents(t, "stop-inbox-order", session);
  const base = `kuvo/sessions/${session}`;
  const send = (id, words, delivery) => request(record, "POST", base + "/send", { id, words, delivery }, desktop);
  assert.equal((await send("msg_later", "Later.", "queue")).status, 200);
  assert.equal((await send("msg_next", "Next.", "steer")).status, 200);
  const stopped = await request(record, "POST", base + "/stop", undefined, desktop);
  assert.equal(stopped.status, 200, stopped.text);
  assert.deepEqual(stopped.json, { paused: true, held: ["msg_next"] });
  await until(() => here.agents.sent("session/cancel").length === 1, "Stop never stopped its turn.", 20000);
  const held = await until(async () => {
    const items = await queueItems(record, session);
    return items?.length === 2 && items;
  }, "the queue to show both waiting messages");
  assert.deepEqual(held.map(row => [row.id, row.held, row.delivery]), [["msg_next", true, "queue"], ["msg_later", false, "queue"]]);
  assert.deepEqual(here.agents.prompts(), ["Keep busy"]);
  const resumed = await request(record, "POST", base + "/queue/resume", undefined, desktop);
  assert.equal(resumed.json.paused, false, resumed.text);
  await until(() => here.agents.prompts().includes("Next."), "The held steer never went once resumed.", 20000);
  await until(async () => (await request(record, "GET", base + "/queue", undefined, desktop)).json?.held.length === 0,
    "the queue to clear its held item after delivery");
  assert.equal(here.agents.prompts()[1], "Next.");
});

test("the global queue line starts with a snapshot and reports holds and resumes", { timeout: 45000 }, async t => {
  const session = "ses_queueLine";
  // (A Hold holds what waits for a conversation at work.)
  const { record } = await busyOnAgents(t, "global-queue-line", session);
  const later = await request(record, "POST", `kuvo/sessions/${session}/send`, { id: "msg_later", words: "Later.", delivery: "queue" }, desktop);
  assert.equal(later.status, 200, later.text);
  const line = await openLine(record, "", "kuvo/sessions/queues/events", desktop);
  t.after(() => line.close());
  assert.equal(line.status, 200);
  const frames = line.frames();
  assert.deepEqual((await frames.next()).value, { queues: {} });
  const route = `kuvo/sessions/${session}/queue`;
  assert.equal((await request(record, "POST", route + "/hold", undefined, desktop)).status, 200);
  assert.deepEqual((await frames.next()).value, { queues: { [session]: { paused: true, held: [] } } });
  assert.equal((await request(record, "POST", route + "/resume", undefined, desktop)).status, 200);
  assert.deepEqual((await frames.next()).value, { queues: {} });
});

test("a paused queue stays paused when its last message is taken back", { timeout: 45000, skip: heldUnmarked }, async t => {
  const session = "ses_pausedEmpty";
  const item = "msg_theLast";
  const { here, record } = await busyOnAgents(t, "paused-empty-queue", session);
  const route = `kuvo/sessions/${session}/queue`;
  const call = (method, at, body) => request(record, method, at, body, desktop);
  assert.equal((await call("POST", `kuvo/sessions/${session}/send`, { id: item, words: "Last.", delivery: "steer" })).status, 200);
  assert.deepEqual((await call("POST", `kuvo/sessions/${session}/stop`)).json, { paused: true, held: [item] });
  assert.equal((await call("DELETE", route + "/" + item)).status, 200);
  const empty = await until(async () => {
    const found = await call("GET", route);
    return found.json?.held.length === 0 && found.json;
  }, "the held item to leave the queue");
  assert.deepEqual(empty, { paused: true, held: [] });
  const late = { id: "msg_late", text: "One more thing." };
  const saved = await call("POST", route + "/add", late);
  assert.equal(saved.status, 200, saved.text);
  assert.equal(saved.json.waits, true);
  assert.deepEqual(saved.json.waiting.map(row => row.id), [late.id]);
  const repeated = await call("POST", route + "/add", late);
  assert.equal(repeated.status, 200, repeated.text);
  assert.deepEqual(repeated.json.waiting.map(row => row.id), [late.id]);
  const next = { id: "msg_next", text: "Use the new notes." };
  assert.equal((await call("POST", route + "/add", next)).json.waits, true);
  const removed = await call("DELETE", route + "/" + late.id);
  assert.equal(removed.status, 200, removed.text);
  assert.deepEqual(removed.json.waiting.map(row => row.id), [next.id]);
  assert.deepEqual((await call("DELETE", route + "/" + late.id)).json, removed.json);
  const ack = await call("POST", route + "/" + late.id + "/ack", { receipt: removed.json.removed.receipt });
  assert.equal(ack.status, 200, ack.text);
  assert.equal((await call("DELETE", route + "/" + late.id)).status, 404);
  assert.equal(fs.existsSync(path.join(here.home, "run", "waiting", late.id + ".json")), false);
  assert.deepEqual(here.agents.prompts(), ["Keep busy"]);
  const sent = await call("POST", route + "/" + next.id + "/now");
  assert.deepEqual(sent.json, { paused: false, held: [] });
  await until(() => here.agents.prompts().includes(next.text), "Send Now never reached the agent.", 20000);
  assert.deepEqual(here.agents.prompts(), ["Keep busy", next.text]);
});

test("Stop on an empty queue keeps two later messages waiting and releases them one turn at a time", { timeout: 45000 }, async t => {
  const session = "ses_stopEmpty";
  const { here, record } = await busyOnAgents(t, "stop-while-empty", session);
  here.agents.script("msg_firstLate", [said("First"), { until: "first-done" }]);
  const base = `kuvo/sessions/${session}`;
  const queue = base + "/queue";
  const stop = await request(record, "POST", base + "/stop", undefined, desktop);
  assert.deepEqual(stop.json, { paused: true, held: [] });
  for (const id of ["msg_firstLate", "msg_secondLate"]) {
    const add = await request(record, "POST", base + "/send", { id, words: id, delivery: "queue" }, desktop);
    assert.deepEqual(add.json, { waits: true });
  }
  const items = await until(async () => {
    const items = await queueItems(record, session);
    return items?.length === 2 && items;
  }, "both saved messages to be listed");
  assert.deepEqual(items.map(row => [row.id, row.on]), [["msg_firstLate", "box"], ["msg_secondLate", "box"]]);
  assert.deepEqual(here.agents.prompts(), ["Keep busy"]);
  const resume = await request(record, "POST", queue + "/resume", undefined, desktop);
  assert.equal(resume.status, 200, resume.text);
  await until(() => here.agents.prompts().length === 2, "The first saved message never went.", 20000);
  await new Promise(resolve => setTimeout(resolve, 500));
  assert.deepEqual(here.agents.prompts(), ["Keep busy", "msg_firstLate"]);
  here.agents.go("first-done");
  await until(() => here.agents.prompts().length === 3, "the second message to follow the completed turn", 20000);
  assert.deepEqual(here.agents.prompts(), ["Keep busy", "msg_firstLate", "msg_secondLate"]);
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

test("the recorded sidebar order follows the resident's session and project facts", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const fixture = readJSON(new URL("../vectors/session-lists.json", import.meta.url)).cases.find(row => row.expect.sidebar);
  const here = await computer("sidebar-session-vector");
  const origin = "/home/me/.kuvo-code";
  const rebase = folder => {
    if (folder.startsWith(origin + "/")) return path.join(here.home, folder.slice(origin.length + 1));
    if (folder.startsWith("/home/me/")) return path.join(here.home, "workspace", folder.slice("/home/me/".length));
    return folder;
  };
  const rebased = value => JSON.parse(JSON.stringify(value, (_key, field) => typeof field === "string" ? rebase(field) : field));
  // The bot roster supplies the one explicit bot assignment outside the bot folder.
  const native = fixture.sessions.filter(session => !fixture.box.botSessions.includes(session.id));
  here.engine.sessions.push(...rebased(native));
  for (const id of fixture.box.running) here.engine.running.add(id);
  write(path.join(here.home, "box", "projects.json"), JSON.stringify({ projects: rebased(fixture.box.projects) }));
  write(path.join(here.home, "box", "worktrees.json"), JSON.stringify({
    handedOver: Object.fromEntries(Object.entries(fixture.box.filed).map(([folder, project]) => [rebase(folder), rebase(project)])),
  }));
  const record = await here.start().ready();
  const reply = await request(record, "GET", "kuvo/sessions/list");
  assert.equal(reply.status, 200, reply.text);
  const rows = Object.entries(reply.json.sessions).filter(([, facts]) => !facts.archived && ["project", "chat"].includes(facts.kind));
  const newest = predicate => rows.filter(([, facts]) => predicate(facts))
    .sort(([, a], [, b]) => b.updated - a.updated).map(([id]) => id);
  const visible = {
    pinned: newest(facts => facts.pinned),
    chats: newest(facts => facts.kind === "chat"),
    projects: reply.json.projectOrder.map(directory => ({
      directory, sessions: newest(facts => facts.kind === "project" && facts.project === directory),
    })),
  };
  assert.deepEqual(visible, rebased(fixture.expect.sidebar));
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
  if (boxBinary().typescript) {
    // The TypeScript reader can filter the deletion from its in-flight page and
    // re-read just the affected ID; another whole read may already be requested.
    assert.ok([1, 2].includes(here.engine.inventoryReads), "at most one additional inventory");
  } else {
    assert.equal(here.engine.inventoryReads, 2, "the change requests one new inventory");
  }
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
  const streamed = (await changes.next()).value;
  try {
    assert.equal(changed.full, undefined);
    assert.deepEqual(Object.keys(changed.sessions), [kept]);
    assert.equal(streamed.full, undefined);
    assert.deepEqual(streamed.sessions, changed.sessions);
  } catch (error) {
    // (Seen once on Windows under load: what each answer and the kept list held.)
    t.diagnostic(`listed since ${newest.at}: ${JSON.stringify(changed)}`);
    t.diagnostic(`streamed: ${JSON.stringify(streamed)}`);
    t.diagnostic(`sessions.json: ${fs.existsSync(savedList) ? fs.readFileSync(savedList, "utf8") : "(none)"}`);
    throw error;
  }
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
  const observed = sessionOf({ id: session, directory: tree });
  here.engine.sessions.push(observed);
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow the engine");
  // Establish the baseline after the initial passive adoption and a complete event-driven read.
  observed.title = "Ready to refile";
  here.engine.emit({ type: "session.renamed", data: { sessionID: session, info: observed } });
  const before = await until(async () => {
    const reply = await request(record, "GET", "kuvo/sessions/list");
    const found = reply.json?.sessions?.[session];
    return found?.project === tree && found.title === observed.title && reply.json;
  }, "the initial project filing");
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
  assert.equal(calls(), readBefore, JSON.stringify({ readBefore, reads: calls(), asked: here.engine.asked.filter(call => call.method === "GET" && call.path.includes("session")), latest }));

  const formerProject = path.join(here.home, "former-project");
  write(path.join(here.home, "box", "worktrees.json"), JSON.stringify({ handedOver: { [tree]: formerProject } }));
  const handedOver = await until(async () => {
    const reply = await request(record, "GET", "kuvo/sessions/list?since=" + encodeURIComponent(changed.at));
    return reply.json?.sessions?.[session]?.project === formerProject && reply.json;
  }, "the old Mac worktree record to refile its session", 8000);
  assert.deepEqual(handedOver.projects[formerProject].shown, [session]);
  assert.equal(calls(), readBefore);
});

test("linked and removed worktrees keep their project and last message time", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-worktree-history");
  const project = path.join(here.home, "repository");
  const linked = path.join(here.home, "linked-tree");
  const removed = path.join(here.home, "removed-tree");
  const nested = path.join(linked, "nested");
  const env = isolatedEnv(path.dirname(here.home));
  write(path.join(project, "nested", "note.txt"), "Tracked.");
  git(env, project, "init", "-q");
  git(env, project, "add", "nested/note.txt");
  git(env, project, "commit", "-qm", "Fixture");
  git(env, project, "worktree", "add", "--quiet", "--detach", linked);
  write(path.join(here.home, "box", "projects.json"),
    JSON.stringify({ projects: [{ directory: project, name: "Repository" }, { directory: linked }] }));
  const database = path.join(here.home, "data", "opencode", "opencode-lists.db");
  fs.mkdirSync(path.dirname(database), { recursive: true });
  const db = new DatabaseSync(database);
  db.exec(`PRAGMA journal_mode=WAL;
    CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT, vcs TEXT);
    CREATE TABLE worktree (project_id TEXT, directory TEXT);
    CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time_created INTEGER);
    CREATE TABLE session_message (session_id TEXT, seq INTEGER, time_created INTEGER);
    INSERT INTO session_v2 VALUES ('ses_removedTree', 1);
    INSERT INTO session_message VALUES ('ses_removedTree', 1, 300);`);
  db.prepare("INSERT INTO project VALUES ('project', ?, 'git')").run(project);
  db.prepare("INSERT INTO worktree VALUES ('project', ?)").run(removed);
  db.close();
  const bytes = fs.readFileSync(database);
  here.engine.sessions.push(
    sessionOf({ id: "ses_linkedTree", directory: linked, updated: 500 }),
    sessionOf({ id: "ses_nestedTree", directory: nested, updated: 400 }),
    sessionOf({ id: "ses_removedTree", directory: removed, updated: 900 }),
  );
  const record = await here.start().ready();
  const list = async () => (await request(record, "GET", "kuvo/sessions/list")).json;
  const initial = await list();
  assert.equal(initial.sessions.ses_linkedTree.project, project);
  assert.equal(initial.sessions.ses_nestedTree.project, project);
  assert.equal(initial.sessions.ses_removedTree.project, project);
  assert.equal(initial.sessions.ses_removedTree.updated, 300);
  assert.deepEqual(initial.projects[project].shown, ["ses_linkedTree", "ses_nestedTree", "ses_removedTree"]);
  assert.equal(initial.projects[linked], undefined);

  await until(() => here.engine.subscribers() > 0, "the resident to follow title changes");
  const row = here.engine.sessions[2];
  row.title = "Renamed";
  row.time.updated = 1000;
  here.engine.emit({ type: "session.renamed", data: { sessionID: row.id, info: row } });
  const visible = await until(async () => {
    const each = (await list())?.sessions?.ses_removedTree;
    return each?.title === "Renamed" && each;
  }, "the renamed session");
  assert.equal(visible.updated, 300);
  assert.equal(visible.project, project);
  assert.deepEqual(fs.readFileSync(database), bytes);
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
