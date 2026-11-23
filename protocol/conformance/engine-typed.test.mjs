// Composer decisions and local draft recovery through both resident binaries.
import assert from "node:assert/strict";
import { test } from "node:test";
import { request } from "./adapter.mjs";
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
