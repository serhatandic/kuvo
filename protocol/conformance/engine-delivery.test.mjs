// A local draft is acknowledged from disk, then confirmed after a broken engine handoff.
import assert from "node:assert/strict";
import { test } from "node:test";
import { openLine, request } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

test("a saved draft replies before OpenCode and settles its original receipt after restart", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-delivery");
  const session = "ses_delivery";
  const base = "kuvo/sessions/" + session;
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const first = here.start();
  const original = await first.ready();
  const draft = { id: "msg_savedDraft", text: "Saved before OpenCode replies.", message: {} };
  const headers = { "x-kuvo-from": "app" };
  const accepted = new Promise(resolve => { here.engine.holdPrompt = { id: draft.id, accepted: resolve }; });
  const saved = await request(original, "POST", base + "/delivery?computer=here", draft, headers);
  assert.equal(saved.status, 202, saved.text);
  assert.equal(saved.json.status, "pending");
  assert.equal(saved.json.owner, "here");
  const receiptRoute = "kuvo/data/messages/" + saved.json.operation;
  const waiting = await request(original, "GET", base + "/delivery", undefined, headers);
  assert.equal(waiting.status, 200, waiting.text);
  assert.deepEqual(waiting.json.items.map(item => item.id), [draft.id]);
  const pending = await request(original, "GET", receiptRoute, undefined, headers);
  assert.deepEqual(pending.json, saved.json);
  await accepted;
  first.child.kill("SIGKILL");
  await first.exited;

  const next = await here.start().ready();
  const watch = Buffer.from(JSON.stringify({ delivery: receiptRoute + "/events" })).toString("base64url");
  const line = await openLine(next, "?watch=" + watch, headers);
  t.after(() => line.close());
  let receipt;
  for await (const frame of line.frames()) {
    if (frame.t === "delivery" && frame.d?.status === "committed") { receipt = frame.d; break; }
  }
  assert.ok(receipt, "The delivery receipt did not settle.");
  assert.equal(receipt.operation, saved.json.operation);
  const replay = await request(next, "POST", base + "/delivery", draft, headers);
  assert.equal(replay.status, 200, replay.text);
  assert.deepEqual(replay.json, { message: draft.id, acceptedAt: receipt.acceptedAt, answer: { send: { text: draft.text, quoted: true } } });
  assert.deepEqual(here.engine.messages.get(session).map(row => [row.id, row.text]), [[draft.id, draft.text]]);
  assert.equal(here.engine.asked.filter(call => call.method === "POST" && call.path.endsWith("/prompt")).length, 1);
});
