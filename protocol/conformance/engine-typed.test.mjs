// Composer decisions and local draft recovery through both resident binaries, on a coding agent's
// conversation (the scripted Grok, `acp.mjs`).
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { request, until, write } from "./adapter.mjs";
import { seedGrokAuth } from "./acp.mjs";
import { bench } from "./bench.mjs";

const desktop = { "x-kuvo-from": "app" };
const conversation = "ses_typed";
const base = "kuvo/sessions/" + conversation;
const words = text => ({ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } });
/** A computer with the conversation on its Grok. */
async function typedOn(t, name, options) {
  const { computer } = await bench(t, { agents: true });
  const here = await computer(name);
  here.agents.seed(conversation, { directory: here.home, ...options });
  return here;
}

test("a typed decision is answered once across restart and sends no message itself", { timeout: 45000 }, async t => {
  const here = await typedOn(t, "engine-typed-decision");
  const first = here.start();
  const record = await first.ready();
  const offers = await request(record, "GET", base + "/typed");
  assert.equal(offers.status, 200, offers.text);
  assert.deepEqual(offers.json, { keywords: [], commands: [] });
  for (const text of ["fix the tests", "/goal ship it", "! ls", "/plan", "/compact", "/review-quick main", "/init now"]) {
    const decision = await request(record, "POST", base + "/typed", { text }, desktop);
    assert.deepEqual(decision.json, { send: { text, quoted: true } });
  }
  const headers = { ...desktop, "idempotency-key": "typed-stable" };
  const original = await request(record, "POST", base + "/typed", { text: "/goal ship it" }, headers);
  assert.equal(original.status, 200, original.text);
  assert.deepEqual(original.json, { send: { text: "/goal ship it", quoted: true } });
  await first.stop();
  const again = await here.start().ready();
  const retry = await request(again, "POST", base + "/typed", { text: "Different words" }, headers);
  assert.equal(retry.status, 200, retry.text);
  assert.deepEqual(retry.json, original.json);
  assert.deepEqual(here.agents.prompts(), []);
  assert.deepEqual(await here.agents.users(again, conversation), []);
});

test("typed and draft sends restore the words while its agent needs a sign-in", { timeout: 60000 }, async t => {
  const here = await typedOn(t, "engine-draft-model-refusal");
  const grok = path.join(here.agents.root, ".grok");
  fs.rmSync(path.join(grok, "auth.json"));
  const record = await here.start().ready();
  const text = "!npm test";
  const refuse = {
    restore: { text }, error: "acp-test needs a sign-in to Grok: sign in, or choose another model.",
    action: { do: "signIn", label: "Sign In", help: "Sign in to Grok" },
  };
  const typed = value => request(record, "POST", base + "/typed", value, desktop);
  assert.deepEqual((await typed({ text, refusal: "The client's old refusal." })).json, refuse);
  const draft = (id, said) => request(record, "POST", base + "/draft", { id, text: said, message: {} }, desktop);
  const original = await draft("msg_modelRefused", text);
  assert.equal(original.status, 200, original.text);
  assert.deepEqual(original.json, { answer: refuse });
  assert.deepEqual(here.agents.prompts(), []);

  // Signed in since: what it's told goes.
  seedGrokAuth(grok);
  const offered = await request(record, "GET", "kuvo/models/offered?refresh=1", undefined, desktop, 45000);
  assert.equal(offered.status, 200, offered.text);
  await until(async () => {
    const current = await typed({ text, refusal: "The client's old refusal." });
    return current.json?.send?.text === text && current.json.send.quoted === true;
  }, "Signing in did not carry the draft.", 20000, 200);
  assert.deepEqual((await draft("msg_modelRefused", text)).json, original.json);
  for (const [id, said] of [["msg_slash", "/goal ship it"], ["msg_bang", "! ls"]]) {
    const sent = await draft(id, said);
    assert.equal(sent.status, 200, sent.text);
    assert.equal(sent.json.message, id);
    assert.deepEqual(sent.json.answer, { send: { text: said, quoted: true } });
    assert.deepEqual((await draft(id, said)).json, sent.json);
    assert.equal((await draft(id, said + " again")).status, 409);
  }
  await until(() => here.agents.prompts().length === 2, "The drafts never reached the agent.", 20000);
  assert.deepEqual(here.agents.prompts(), ["/goal ship it", "! ls"]);
  assert.deepEqual(await here.agents.users(record, conversation), ["msg_slash", "msg_bang"]);
});

test("a missing conversation keeps the words, and a decision needs them", { timeout: 45000 }, async t => {
  const here = await typedOn(t, "engine-draft-unknown");
  const record = await here.start().ready();
  const text = "Keep the original words.";
  const absent = await request(record, "POST", "kuvo/sessions/ses_missing/typed", { text }, desktop);
  assert.equal(absent.status, 404, absent.text);
  assert.deepEqual(absent.json, { message: "There's no such conversation here.", restore: { text } });
  assert.equal((await request(record, "POST", base + "/typed", {}, desktop)).status, 400);
  const draft = await request(record, "POST", "kuvo/sessions/ses_missing/draft", { id: "msg_nowhere", text, message: {} }, desktop);
  assert.equal(draft.status, 404, draft.text);
  assert.deepEqual(draft.json.restore, { text });
});

test("a draft lost after its agent took it confirms the original message once", { timeout: 60000 }, async t => {
  const here = await typedOn(t, "engine-typed-draft");
  here.agents.script("! ls", [words("Listing"), "hang"]);
  const first = here.start();
  const record = await first.ready();
  const draft = { id: "msg_lostDraft", text: "! ls", quotedText: "Quoted: ! ls", message: {} };
  const original = request(record, "POST", base + "/draft", draft, desktop).catch(() => null);
  await until(() => here.agents.prompts().length > 0, "The draft never reached the agent.", 20000);
  first.child.kill("SIGKILL");
  await first.exited;
  await original;
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
  const rows = (await here.agents.rows(again, conversation)).filter(row => row.type === "user");
  assert.deepEqual(rows.map(row => [row.id, row.text]), [[draft.id, draft.quotedText]]);
  assert.equal(here.agents.prompts().length, 1);
});

test("a scoped draft keeps its receipt and preserves the earlier send across restart", { timeout: 45000 }, async t => {
  const here = await typedOn(t, "engine-scoped-draft");
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
  assert.deepEqual(await here.agents.users(again, conversation), [id]);
});

// (Autopilot is gone from the app: a conversation it marked is an ordinary one.)
test("a conversation kept with an Autopilot mark opens and takes a message as any other", { timeout: 45000 }, async t => {
  const here = await typedOn(t, "engine-autopilot-mark", { metadata: { kuvo: { autopilot: true } }, messages: [
    { type: "user", text: "Earlier, on Autopilot." },
    { type: "assistant", content: [{ type: "text", text: "Done before." }] },
  ] });
  here.agents.script("And now this", [words("Done now.")]);
  const record = await here.start().ready();
  const header = await request(record, "GET", `kuvo/agents/api/session/${conversation}`, undefined, desktop);
  assert.equal(header.status, 200, header.text);
  assert.deepEqual((await request(record, "POST", base + "/typed", { text: "And now this" }, desktop)).json, { send: { text: "And now this", quoted: true } });
  const sent = await request(record, "POST", base + "/send", { id: "msg_afterAutopilot", words: "And now this", delivery: "steer" }, desktop);
  assert.equal(sent.status, 200, sent.text);
  const said = await until(async () => {
    const rows = await here.agents.rows(record, conversation);
    return rows.some(row => row.type === "assistant" && JSON.stringify(row.content).includes("Done now.")) && rows;
  }, "The conversation never answered.", 20000, 100);
  assert.deepEqual(said.filter(row => row.type === "user").map(row => row.text), ["Earlier, on Autopilot.", "And now this"]);
  // (Once, with what came before it: the agent hasn't this conversation of its own yet.)
  const prompts = here.agents.prompts();
  assert.equal(prompts.length, 1);
  assert.ok(prompts[0].endsWith("And now this"), prompts[0]);
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
