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
  for (const text of ["fix the tests", "/goal ship it", "! ls", "/plan", "/compact", "/review-quick main", "/init now"]) {
    const decision = await request(record, "POST", base + "/typed", { text }, desktop);
    assert.deepEqual(decision.json, { send: { text, quoted: true } });
  }
  assert.equal(here.engine.asked.some(row => row.path === "/api/command"), false);
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

test("typed and draft sends restore the words when the selected model needs an account", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-draft-model-refusal");
  const model = { providerID: "anthropic", id: "claude" };
  const ready = { providerID: "deepseek", id: "v4" };
  here.engine.providers.push({ id: ready.providerID });
  here.engine.models.push({ ...ready, name: "DeepSeek V4" }, { ...model, name: "Claude" });
  here.engine.integrations.push({
    id: "anthropic", name: "Anthropic", connections: [], methods: [{ type: "oauth", id: "browser" }, { type: "key" }],
  });
  const session = { ...sessionOf({ id: conversation, directory: here.home }), model };
  here.engine.sessions.push(session);
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "The engine did not publish account changes.");
  const text = "!npm test";
  const refuse = {
    restore: { text }, error: "Claude needs a sign-in to Anthropic: sign in, or choose another model.",
    action: { do: "signIn", label: "Sign In", help: "Sign in to Anthropic" },
  };
  const typed = words => request(record, "POST", base + "/typed", words, desktop);
  assert.deepEqual((await typed({ text, refusal: "The client's old refusal." })).json, refuse);
  const draft = (id, words) => request(record, "POST", base + "/draft", { id, text: words, message: {} }, desktop);
  const original = await draft("msg_modelRefused", text);
  assert.equal(original.status, 200, original.text);
  assert.deepEqual(original.json, { answer: refuse });
  assert.deepEqual(here.engine.messages.get(conversation) ?? [], []);

  session.model = ready;
  here.engine.emit({ type: "session.updated", data: { sessionID: conversation, info: session } });
  await until(async () => {
    const current = await typed({ text, refusal: "The client's old refusal." });
    return current.json?.send?.text === text && current.json.send.quoted === true;
  }, "The changed model did not carry the draft.");
  assert.deepEqual((await draft("msg_modelRefused", text)).json, original.json);
  for (const [id, words] of [["msg_slash", "/goal ship it"], ["msg_bang", "! ls"]]) {
    const sent = await draft(id, words);
    assert.equal(sent.status, 200, sent.text);
    assert.equal(sent.json.message, id);
    assert.deepEqual(sent.json.answer, { send: { text: words, quoted: true } });
    assert.deepEqual((await draft(id, words)).json, sent.json);
    assert.equal((await draft(id, words + " again")).status, 409);
  }
  assert.deepEqual(here.engine.messages.get(conversation).map(row => [row.id, row.text]), [
    ["msg_slash", "/goal ship it"], ["msg_bang", "! ls"],
  ]);
  here.engine.refusePrompt = { id: "msg_busyDraft", seen() {} };
  const busy = await draft("msg_busyDraft", "The engine was busy.");
  assert.equal(busy.status, 200, busy.text);
  assert.equal(busy.json.answer.ambiguous, true);
  assert.equal(typeof busy.json.answer.error, "string");
  assert.equal(here.engine.messages.get(conversation).length, 2);
});

test("an explicit empty default asks for a model and an unavailable catalog leaves a draft alone", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-draft-unknown-model");
  const model = { providerID: "deepseek", id: "v4", name: "DeepSeek V4" };
  here.engine.providers.push({ id: model.providerID });
  here.engine.models.push(model);
  here.engine.sessions.push(sessionOf({ id: conversation, directory: here.home }));
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "The engine event reader did not connect.");
  const text = "Keep the original words.";
  const typed = value => request(record, "POST", base + "/typed", value, desktop);
  const choose = {
    restore: { text }, error: "Choose a model to send it.",
    action: { do: "choose", label: "Choose a Model", help: "Choose a model" },
  };
  assert.deepEqual((await typed({ text })).json, choose);
  const none = await request(record, "POST", base + "/draft", { id: "msg_chooseModel", text, message: {} }, desktop);
  assert.equal(none.status, 200, none.text);
  assert.deepEqual(none.json, { answer: choose });
  assert.deepEqual(here.engine.messages.get(conversation) ?? [], []);
  here.engine.providers.length = 0;
  here.engine.models.length = 0;
  here.engine.emit({ type: "model.updated", data: {} });
  await until(async () => (await typed({ text })).json?.send?.text === text, "The unavailable model catalog refused the user's words.");
  const absent = await request(record, "POST", "kuvo/sessions/ses_missing/typed", { text }, desktop);
  assert.equal(absent.status, 404, absent.text);
  assert.deepEqual(absent.json, { message: "There's no such conversation here.", restore: { text } });
  assert.equal((await typed({})).status, 400);
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

test("two retries recover a draft that never reached the engine before restart", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-draft-unaccepted");
  here.engine.sessions.push(sessionOf({ id: conversation, directory: here.home }));
  const first = here.start();
  const record = await first.ready();
  const draft = { id: "msg_orphanDraft", text: "Send the same draft once.", message: {} };
  const atEngine = new Promise(resolve => {
    here.engine.holdPrompt = { id: draft.id, beforeAcceptance: true, accepted: resolve };
  });
  const post = host => request(host, "POST", base + "/draft", draft, desktop);
  const lost = post(record).catch(() => null);
  await atEngine;
  first.child.kill("SIGKILL");
  await first.exited;
  assert.equal(await lost, null);
  assert.deepEqual(here.engine.messages.get(conversation) ?? [], []);
  const again = await here.start().ready();
  here.engine.inboxUnavailable = new Set([conversation]);
  const unknown = await post(again);
  assert.equal(unknown.status, 409, unknown.text);
  assert.equal(unknown.json.ambiguous, true);
  assert.deepEqual(here.engine.messages.get(conversation) ?? [], []);
  here.engine.inboxUnavailable.clear();
  const firstTwo = await Promise.all([post(again), post(again)]);
  assert.ok(firstTwo.some(answer => answer.status === 200 && answer.json.message === draft.id));
  const confirmed = await post(again);
  assert.equal(confirmed.status, 200, confirmed.text);
  assert.equal(confirmed.json.message, draft.id);
  assert.deepEqual(here.engine.messages.get(conversation).map(row => [row.id, row.text]), [[draft.id, draft.text]]);
  assert.equal(here.engine.asked.filter(row => row.method === "POST" && row.path.endsWith("/prompt")).length, 2);
});

test("a retry waiting for the original draft leaves other requests responsive", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-draft-contended");
  here.engine.sessions.push(sessionOf({ id: conversation, directory: here.home }));
  const record = await here.start().ready();
  const draft = { id: "msg_contended", text: "Only once.", message: {} };
  const entered = Promise.withResolvers();
  const release = Promise.withResolvers();
  t.after(() => release.resolve());
  here.engine.holdPrompt = { id: draft.id, beforeAcceptance: true, accepted: entered.resolve, release: release.promise };
  const submit = () => request(record, "POST", base + "/draft", draft, desktop);
  const first = submit();
  await entered.promise;
  let retryFinished = false;
  const retry = submit().finally(() => { retryFinished = true; });
  try {
    const probe = await request(record, "GET", "kuvo/nonexistent");
    assert.equal(probe.status, 404, probe.text);
    assert.equal(retryFinished, false, "The probe should finish while the draft is still waiting.");
  } finally {
    release.resolve();
    await Promise.allSettled([first, retry]);
  }
  const [original, repeated] = await Promise.all([first, retry]);
  assert.equal(original.status, 200, original.text);
  assert.equal(repeated.status, 200, repeated.text);
  assert.equal(repeated.json.message, draft.id);
  assert.equal(here.engine.messages.get(conversation).filter(row => row.id === draft.id).length, 1);
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
