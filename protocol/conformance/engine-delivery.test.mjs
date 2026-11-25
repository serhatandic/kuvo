// A local draft is acknowledged from disk, then confirmed after a broken engine handoff.
import assert from "node:assert/strict";
import { test } from "node:test";
import { openLine, request, until } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

test("two computers deliver addressed drafts and recover the original scoped receipt", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const listing = await computer("Listing");
  const owner = await computer("Owner");
  const session = "ses_addressed";
  const base = "kuvo/sessions/" + session;
  const headers = { "x-kuvo-from": "app" };
  owner.engine.sessions.push(sessionOf({ id: session, title: "Addressed conversation", directory: owner.home }));
  const primary = listing.connect(await listing.start().ready());
  const setup = await primary("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  const invitation = await primary("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invitation.status, 201, invitation.text);
  await until(() => leases.leaseFor(setup.json.setup), "The listing computer did not claim its setup.");
  const joined = await owner.join(invitation.json.code);
  assert.equal(joined.code, 0, joined.log);
  const target = owner.connect(await owner.start().ready());
  const membership = await target("GET", "kuvo/setup");
  assert.equal(membership.status, 200, membership.text);
  const key = membership.json.you.key;
  const local = await target("GET", "kuvo/sessions/list");
  assert.equal(local.status, 200, local.text);
  await until(async () => {
    const scope = await primary("GET", base + "/submissions?scope=1", undefined, headers);
    return scope.json?.currentOwner === key && scope.json.canSubmit;
  }, "The listing computer did not learn the conversation's owner.", 30000, 50);

  const scopedId = "msg_remoteScope";
  const submission = [setup.json.setup, key, scopedId].join(".");
  const scoped = { id: scopedId, submission, text: "Keep my scoped receipt", message: {} };
  const save = await primary("POST", base + "/submit", scoped, headers);
  assert.equal(save.status, 202, save.text);
  const received = await until(async () => {
    const response = await primary("GET", base + "/submissions/" + submission, undefined, headers);
    return response.json?.outcomes?.find(row => row.status === "committed" && row.evidence === "owner");
  }, "The authenticated owner receipt did not arrive.", 22000, 150);
  assert.equal(received.effect.message, scopedId);
  const receipt = await primary("GET", "kuvo/data/messages/" + save.json.operation, undefined, headers);
  assert.equal(receipt.status, 200, receipt.text);
  assert.equal(receipt.json.submission, submission);
  assert.equal(receipt.json.status, "committed");
  const replay = await primary("POST", base + "/submit", scoped, headers);
  assert.equal(replay.status, 200, replay.text);
  assert.equal(replay.json.message, scopedId);

  const text = { id: "msg_oldAddressed", words: "An addressed message" };
  const legacy = await primary("POST", `kuvo/data/messages?session=${session}&computer=${key}`, text, headers);
  assert.equal(legacy.status, 202, legacy.text);
  const draft = { id: "msg_addressedDraft", text: "With an attachment", message: { files: [
    { name: "notes.txt", uri: "data:text/plain;base64," + Buffer.alloc(160000, 88).toString("base64") },
  ] } };
  const saved = await primary("POST", base + "/delivery?computer=" + key, draft, headers);
  assert.equal(saved.status, 202, saved.text);
  for (const operation of [legacy.json.operation, saved.json.operation]) {
    await until(async () => {
      const receipt = await primary("GET", "kuvo/data/messages/" + operation, undefined, headers);
      return receipt.json?.status === "committed";
    }, "An addressed delivery did not settle its receipt.", 22000, 150);
  }
  assert.deepEqual(owner.engine.messages.get(session).map(row => row.id), [scopedId, text.id, draft.id]);
  assert.deepEqual(listing.engine.messages.get(session) ?? [], []);
});

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
  const line = await openLine(next, "?watch=" + watch, undefined, headers);
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

test("taking back a waiting draft keeps its recovery copy until the composer acknowledges it", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-delivery-edit");
  const session = "ses_deliveryEdit";
  const route = `kuvo/sessions/${session}/delivery`;
  const headers = { "x-kuvo-from": "app" };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const resident = here.start();
  const record = await resident.ready();
  const first = { id: "msg_handoff", text: "Already being handed off", message: {} };
  const accepted = new Promise(resolve => { here.engine.holdPrompt = { id: first.id, accepted: resolve }; });
  assert.equal((await request(record, "POST", route, first, headers)).status, 202);
  await accepted;
  const files = [{ name: "notes.txt", uri: "data:text/plain;base64,eA==" }];
  const draft = { id: "msg_waiting", text: "Visible text", quotedText: "Full draft", message: { files } };
  const saved = await request(record, "POST", route, draft, headers);
  assert.equal(saved.status, 202, saved.text);
  const editing = `${route}/${draft.id}`;
  const active = await request(record, "DELETE", `${route}/${first.id}`, undefined, headers);
  assert.equal(active.status, 409, active.text);
  const removed = { removed: { id: draft.id, words: draft.quotedText, draft: draft.quotedText, files, receipt: "edit-1" } };
  const takeBack = computer => request(computer, "DELETE", editing + "?receipt=edit-1", undefined, headers);
  const result = await takeBack(record);
  assert.equal(result.status, 200, result.text);
  assert.deepEqual(result.json, removed);
  resident.child.kill("SIGKILL");
  await resident.exited;

  const restarted = await here.start().ready();
  const repeated = await takeBack(restarted);
  assert.equal(repeated.status, 200, repeated.text);
  assert.deepEqual(repeated.json, removed);
  const receipt = await request(restarted, "GET", "kuvo/data/messages/" + saved.json.operation, undefined, headers);
  assert.deepEqual(receipt.json, {
    operation: saved.json.operation, status: "interrupted", owner: "here", message: "The message was removed.",
  });
  const ack = await request(restarted, "POST", editing + "/ack?receipt=edit-1", undefined, headers);
  assert.equal(ack.status, 200, ack.text);
  assert.deepEqual((await takeBack(restarted)).json, { removed: null });
  assert.equal((await request(restarted, "POST", route, draft, headers)).status, 409);
  assert.deepEqual(here.engine.messages.get(session).map(row => row.id), [first.id]);
});

test("Steer and Send Now changed on waiting drafts reach the engine after restart", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-delivery-mode");
  const session = "ses_deliveryMode";
  const route = `kuvo/sessions/${session}/delivery`;
  const headers = { "x-kuvo-from": "app" };
  here.engine.sessions.push(sessionOf({ id: session, directory: here.home }));
  const resident = here.start();
  const record = await resident.ready();
  const first = { id: "msg_beforeMode", text: "Already delivered", message: {} };
  const accepted = new Promise(resolve => { here.engine.holdPrompt = { id: first.id, accepted: resolve }; });
  assert.equal((await request(record, "POST", route, first, headers)).status, 202);
  await accepted;
  const queued = [];
  for (const mode of ["steer", "now"]) {
    const id = "msg_" + mode;
    const draft = { id, text: mode, message: { delivery: "queue" } };
    const saved = await request(record, "POST", route, draft, headers);
    assert.equal(saved.status, 202, saved.text);
    const edited = await request(record, "POST", route + "/" + id + "/" + mode, undefined, headers);
    assert.equal(edited.status, 200, edited.text);
    queued.push({ id, operation: saved.json.operation });
  }
  resident.child.kill("SIGKILL");
  await resident.exited;
  here.engine.running.add(session);
  const next = await here.start().ready();
  for (const { operation } of queued) {
    await until(async () => (await request(next, "GET", "kuvo/data/messages/" + operation, undefined, headers)).json?.status === "committed",
      "the changed draft to be accepted", 20000);
  }
  assert.deepEqual(here.engine.inboxes.get(session).map(({ id, delivery }) => [id, delivery]), [
    ["msg_steer", "steer"],
    ["msg_now", "steer"],
  ]);
  assert.deepEqual(here.engine.interrupted, [{ session, resume: true }]);
});
