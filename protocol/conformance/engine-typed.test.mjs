// Composer decisions and local draft recovery through both resident binaries.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import path from "node:path";
import { test } from "node:test";
import { request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

const desktop = { "x-kuvo-from": "app" };
const conversation = "ses_typed";
const base = "kuvo/sessions/" + conversation;

test("a typed decision is answered once across restart and sends no message itself", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-typed-decision");
  here.engine.sessions.push(sessionOf({ id: conversation, directory: here.home }));
  const first = here.start();
  const record = await first.ready();
  const offers = await request(record, "GET", base + "/typed");
  assert.equal(offers.status, 200, offers.text);
  assert.deepEqual(offers.json, { keywords: [], commands: [] });
  const headers = { ...desktop, "idempotency-key": "typed-stable" };
  const original = await request(record, "POST", base + "/typed", { text: "/goal ship it" }, headers);
  assert.equal(original.status, 200, original.text);
  assert.deepEqual(original.json, { send: { text: "/goal ship it", quoted: true } });
  await first.stop();
  const retry = await request(await here.start().ready(), "POST", base + "/typed", { text: "Different words" }, headers);
  assert.equal(retry.status, 200, retry.text);
  assert.deepEqual(retry.json, original.json);
  assert.deepEqual(here.engine.messages.get(conversation) ?? [], []);
});

test("a draft lost after its engine accepted it confirms the original message once", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-typed-draft");
  here.engine.sessions.push(sessionOf({ id: conversation, directory: here.home }));
  const first = here.start();
  const record = await first.ready();
  const draft = { id: "msg_lostDraft", text: "! ls", quotedText: "Quoted: ! ls", message: {} };
  const accepted = new Promise(resolve => { here.engine.holdPrompt = { id: draft.id, accepted: resolve }; });
  const original = request(record, "POST", base + "/draft", draft, desktop).catch(() => null);
  await accepted;
  first.child.kill("SIGKILL");
  await first.exited;
  assert.equal(await original, null);
  const again = await here.start().ready();
  const result = await request(again, "POST", base + "/draft", draft, desktop);
  assert.equal(result.status, 200, result.text);
  assert.deepEqual(result.json.answer, { send: { text: "! ls", quoted: true } });
  assert.equal(result.json.message, draft.id);
  assert.ok(Number.isSafeInteger(result.json.acceptedAt));
  const replay = await request(again, "POST", base + "/draft", draft, desktop);
  assert.deepEqual(replay.json, result.json);
  const different = await request(again, "POST", base + "/draft", { ...draft, text: "Another" }, desktop);
  assert.equal(different.status, 409, different.text);
  assert.deepEqual(here.engine.messages.get(conversation).map(row => [row.id, row.text]), [[draft.id, draft.quotedText]]);
  assert.equal(here.engine.asked.filter(call => call.method === "POST" && call.path.endsWith("/prompt")).length, 1);
});

test("a scoped draft keeps its receipt and preserves the earlier send across restart", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-scoped-draft");
  here.engine.sessions.push(sessionOf({ id: conversation, directory: here.home }));
  const first = here.start();
  const record = await first.ready();
  const scope = await request(record, "GET", base + "/submissions?scope=1", undefined, desktop);
  assert.equal(scope.status, 200, scope.text);
  assert.equal(scope.json.canSubmit, true, scope.text);
  const id = "msg_scoped";
  const submission = [scope.json.setup, scope.json.currentOwner, id].join(".");
  const draft = { id, submission, text: "The scoped draft", message: {} };
  const sent = await request(record, "POST", base + "/submit", draft, desktop);
  assert.equal(sent.status, 200, sent.text);
  assert.equal(sent.json.message, id);
  await first.stop();

  const again = await here.start().ready();
  const recovered = await request(again, "GET", base + "/submissions/" + submission, undefined, desktop);
  assert.equal(recovered.status, 200, recovered.text);
  assert.equal(recovered.json.complete, true, recovered.text);
  assert.equal(recovered.json.outcomes[0]?.status, "committed");
  assert.equal(recovered.json.outcomes[0].effect.message, id);
  const list = await request(again, "GET", base + "/submissions", undefined, desktop);
  assert.equal(list.status, 200, list.text);
  assert.deepEqual(list.json.outcomes.map(row => row.submission), [submission]);
  const replay = await request(again, "POST", base + "/submit", draft, desktop);
  assert.deepEqual(replay.json, sent.json);
  const conflicting = await request(again, "POST", base + "/submit", { ...draft, text: "New words" }, desktop);
  assert.equal(conflicting.status, 409, conflicting.text);
  const { submission: _, ...unscoped } = draft;
  const old = await request(again, "POST", base + "/draft", unscoped, desktop);
  assert.equal(old.status, 409, old.text);
  assert.deepEqual(here.engine.messages.get(conversation).map(row => row.id), [id]);
});

test("submission history pages across saved receipts from two record versions", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("receipt-index");
  const record = await here.start().ready();
  const scope = await request(record, "GET", base + "/submissions?scope", undefined, desktop);
  assert.equal(scope.status, 200, scope.text);
  const { setup, currentOwner } = scope.json;
  const submitter = { kind: "computer" };
  const seed = (session, number, current) => {
    const id = "msg_fixture" + String(number).padStart(3, "0");
    const submission = [setup, currentOwner, id].join(".");
    const key = createHash("sha256").update(JSON.stringify(["draft-submission", setup, session, submission])).digest("base64url");
    const stored = {
      ...(current ? { formatVersion: 1 } : {}),
      digest: "x".repeat(43), at: number + 1,
      answer: { status: 200, value: { message: id, answer: { send: { text: id, quoted: true } } } },
      submission: { setup, session, id: submission, origin: currentOwner, submittedThrough: currentOwner, submitter },
    };
    write(path.join(here.home, "run/owned-data/submissions", key + ".json"), JSON.stringify(stored));
    return submission;
  };
  const wanted = Array.from({ length: 42 }, (_, number) => seed(conversation, number, number % 2 === 0));
  seed("ses_unrelated", 420, true);
  const read = after => request(record, "GET", base + "/submissions" + (after ? "?after=" + after : ""), undefined, desktop);
  const first = await until(async () => {
    const result = await read();
    assert.equal(result.status, 200, result.text);
    return result.json?.complete && result.json;
  }, "The earlier receipt index did not settle.", 15000, 100);
  assert.equal(first.outcomes.length, 40);
  assert.equal(first.next, wanted[39]);
  const second = await read(first.next);
  assert.equal(second.status, 200, second.text);
  assert.equal(second.json.complete, true);
  assert.equal(second.json.next, undefined);
  assert.deepEqual([...first.outcomes, ...second.json.outcomes].map(row => row.submission), wanted);
  const later = seed(conversation, 42, false);
  const changed = await until(async () => {
    const result = await read(wanted.at(-1));
    return result.json?.complete && result.json;
  }, "The receipt written after the first page did not enter the index.", 15000, 100);
  assert.deepEqual(changed.outcomes.map(row => row.submission), [later]);
});
