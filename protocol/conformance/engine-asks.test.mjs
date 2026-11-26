// What a coding agent asks (the scripted Grok, `acp.mjs`): its cards, the answers that reach it, the
// Blocks they honor, and the waiting counts they change.
import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, openLine, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { askFirst } from "./claude.mjs";

const headers = { "x-kuvo-from": "app" };
const words = text => ({ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } });
const options = [{ optionId: "yes", kind: "allow_once", name: "Allow" }, { optionId: "always", kind: "allow_always", name: "Always" }, { optionId: "no", kind: "reject_once", name: "No" }];
/** Its turn asking to edit `file`. */
const editing = (id, file) => [
  { update: { sessionUpdate: "tool_call", toolCallId: id, kind: "edit", title: "Edit " + path.basename(file), status: "pending",
    locations: [{ path: file }], rawInput: { file_path: file, old_string: "a", new_string: "b" },
    content: [{ type: "diff", path: file, oldText: "a\n", newText: "b\n" }] } },
  { ask: ["session/request_permission", { toolCall: { toolCallId: id }, options }] },
];
const running = (id, command, title) => [
  { update: { sessionUpdate: "tool_call", toolCallId: id, kind: "execute", title, status: "pending", rawInput: { command } } },
  { ask: ["session/request_permission", { toolCall: { toolCallId: id }, options }] },
];
const question = (id, asked) => ({ ask: ["_x.ai/ask_user_question", { toolCallId: id, questions: [asked] }] });

/** A computer whose conversations ask first, each one's message playing its steps. */
async function asking(t, name, conversations) {
  const { computer } = await bench(t, { agents: true });
  const here = await computer(name);
  for (const [id, { steps, ...options }] of Object.entries(conversations)) {
    here.agents.seed(id, { directory: here.home, permissions: askFirst, ...options });
    if (steps) here.agents.script(`go ${id}`, steps);
  }
  const record = await here.start().ready();
  const go = async id => {
    const sent = await request(record, "POST", `kuvo/sessions/${id}/send`, { id: `msg_go${id.slice(4)}`, words: `go ${id}`, delivery: "steer" }, headers);
    assert.equal(sent.status, 200, sent.text);
  };
  const asks = async id => {
    const read = await request(record, "GET", `kuvo/sessions/${id}/asks`, undefined, headers);
    assert.equal(read.status, 200, read.text);
    return read.json.asks;
  };
  const card = (id, which, why) => until(async () => (await asks(id)).find(which), why, 20000, 50);
  const answer = (id, ask, body) => request(record, "POST", `kuvo/sessions/${id}/asks/${encodeURIComponent(ask)}`, body, headers);
  /** What the box answered the agent's asks, oldest first. */
  const answers = () => here.agents.heard().filter(line => line.answer).map(line => line.answer.result);
  return { here, record, go, asks, card, answer, answers };
}

test("ask cards show the files it edits, its whole command and its question, and take only fitting answers", { timeout: 60000 }, async t => {
  const id = "ses_cards";
  const asked = { id: "color", question: "Which color?", options: [{ label: "Blue", description: "Calm" }] };
  const { here, go, card, asks, answer, answers } = await asking(t, "engine-ask-cards", { [id]: {} });
  // (The file it edits, in its folder.)
  const file = path.join(here.home, "src", "app.txt");
  here.agents.script(`go ${id}`, [...editing("t1", file), ...running("t2", "cd app && git status --short", "Run `git status`"), question("q1", asked), words("Done.")]);
  await go(id);
  const change = await card(id, ask => ask.kind === "permission", "No card for the edit.");
  assert.equal(change.title, "Allow editing src/app.txt?");
  assert.deepEqual(change.resources, ["src/app.txt"]);
  assert.deepEqual(change.files.map(each => [each.file, each.additions, each.deletions, each.status]), [[file, 1, 1, "modified"]]);
  assert.deepEqual(change.choices.map(choice => choice.answer), ["once", "always", "reject", "instead"]);
  assert.deepEqual(change.choices[1], { answer: "always", title: "Always Allow Edits", named: null, placeholder: null,
    help: "Don’t ask again before changing files in this conversation", spoken: "Always allow edits in this conversation" });
  assert.equal((await answer(id, change.id, { answer: "once" })).status, 200);

  const command = await card(id, ask => ask.kind === "permission" && ask.id !== change.id, "No card for the command.");
  assert.equal(command.session, id);
  assert.equal(command.title, "Allow running this command?");
  assert.equal(command.message, "Run `git status`");
  assert.equal(command.command, "cd app && git status --short");
  assert.deepEqual(command.choices[1], { answer: "always", title: "Always Allow", named: "git status", placeholder: null,
    help: "Don’t ask again before running git status commands in this conversation", spoken: "Always allow git status commands" });
  assert.equal((await answer(id, command.id, { answer: "once" })).status, 200);

  const pick = await card(id, ask => ask.kind === "question", "No card for the question.");
  assert.deepEqual(pick.questions, [{ key: "color", title: "Question 1", question: "Which color?",
    options: [{ value: "Blue", label: "Blue", description: "Calm" }], multiple: false, custom: true, required: true }]);
  const wrong = await answer(id, pick.id, { answers: { color: 7 } });
  assert.equal(wrong.status, 400, wrong.text);
  assert.equal((await answer(id, pick.id, { dismiss: true })).status, 200);
  await until(() => answers().length === 3, () => "The dismissed question never reached the agent: " + JSON.stringify(answers()), 20000);
  assert.deepEqual(answers(), [
    { outcome: { outcome: "selected", optionId: "yes" } },
    { outcome: { outcome: "selected", optionId: "yes" } },
    { outcome: "cancelled" },
  ]);
  await until(async () => (await asks(id)).length === 0, "The asks stayed.", 20000);
});

test("asks report a missing conversation, and an answer to an ask no longer waiting", { timeout: 45000 }, async t => {
  const id = "ses_errors";
  const { record, go, card, answer } = await asking(t, "engine-ask-errors", { [id]: { steps: [...running("t1", "make", "Run `make`"), words("Made.")] } });
  assert.equal((await request(record, "GET", "kuvo/sessions/ses_missing/asks", undefined, headers)).status, 404);
  await go(id);
  const ask = await card(id, each => each.kind === "permission", "No card for the command.");
  assert.equal((await answer(id, ask.id, { answer: "once" })).status, 200);
  const late = await answer(id, ask.id, { answer: "once" });
  assert.equal(late.status, 404, late.text);
  assert.equal((await answer(id, "per_missing", { answer: "once" })).status, 404);
});

test("an answer on its card honors a Block saved after the ask arrived", { timeout: 60000 }, async t => {
  const id = "ses_consent";
  const { here, go, card, answer, answers } = await asking(t, "engine-consent", { [id]: {} });
  here.agents.script(`go ${id}`, [...editing("t1", path.join(here.home, "testing", "private.txt")), words("Edited.")]);
  await go(id);
  const ask = await card(id, each => each.kind === "permission", "No card for the edit.");
  const settings = path.join(here.home, "box", "opencode.json");
  const rules = permissions => write(settings, JSON.stringify({ permissions }));
  // (An agent's edit is matched by the file's whole path.)
  rules([{ action: "edit", resource: path.join(here.home, "testing", "private.*"), effect: "deny" }]);
  const refused = await answer(id, ask.id, { answer: "once" });
  assert.equal(refused.status, 403, refused.text);
  assert.equal(refused.json.message, "This computer’s permissions no longer allow that action.");
  rules([{ action: "edit", resource: path.join(here.home, "unrelated", "*"), effect: "deny" }]);
  const always = await answer(id, ask.id, { answer: "always" });
  assert.equal(always.status, 403, always.text);
  assert.equal(always.json.message, "An Always answer could override a local restriction for this tool. Use Allow Once for this request.");
  assert.deepEqual(answers(), []);
  assert.equal((await answer(id, ask.id, { answer: "once" })).status, 200);
  assert.equal((await answer(id, ask.id, { answer: "once" })).status, 404);
  await until(() => answers().length === 1, "The answer never reached the agent.", 20000);
  assert.deepEqual(answers(), [{ outcome: { outcome: "selected", optionId: "yes" } }]);
});

test("a question changes the conversation's asks and the waiting counts on the line", { timeout: 60000 }, async t => {
  const id = "ses_asking";
  const asked = { id: "language", question: "Which language?", options: [{ label: "Rust" }] };
  const { record, go, answer, answers } = await asking(t, "engine-asks", { [id]: { steps: [question("q1", asked), words("Rust it is.")] } });
  const base = `kuvo/sessions/${id}/asks`;
  assert.deepEqual((await request(record, "GET", base, undefined, headers)).json, { asks: [] });
  const watch = Buffer.from(JSON.stringify({ asks: base + "/events", counts: "kuvo/waiting/events" })).toString("base64url");
  const line = await openLine(record, "?watch=" + watch, undefined, headers);
  t.after(() => line.close());
  const frames = line.frames();
  const saved = [];
  const next = async (ready, description) => {
    const present = saved.findIndex(ready);
    if (present >= 0) return saved.splice(present, 1)[0].d;
    for (;;) {
      const { value: frame, done } = await frames.next().catch(error => {
        throw new Error("Waiting for " + description + ": " + JSON.stringify(saved), { cause: error });
      });
      assert.equal(done, false, "The line ended before the expected update.");
      if (ready(frame)) return frame.d;
      saved.push(frame);
    }
  };
  await next(frame => frame.t === "counts" && frame.d?.sessions === 0, "zero waiting sessions");
  await go(id);
  const asks = await next(frame => frame.t === "asks" && frame.d?.asks?.length > 0, "the question");
  assert.deepEqual(asks.asks.map(ask => [ask.kind, ask.session, ask.questions.map(each => each.question)]), [["question", id, ["Which language?"]]]);
  assert.deepEqual(await next(frame => frame.t === "counts" && frame.d?.sessions === 1, "one waiting session"),
    { sessions: 1, bots: 0, queue: 0 });
  const accepted = await answer(id, asks.asks[0].id, { answers: { [asks.asks[0].questions[0].key]: "Rust" } });
  assert.equal(accepted.status, 200, accepted.text);
  await next(frame => frame.t === "asks" && frame.d?.asks?.length === 0, "the answered question");
  await next(frame => frame.t === "counts" && frame.d?.sessions === 0, "zero waiting sessions");
  assert.deepEqual(answers(), [{ outcome: "accepted", answers: { "Which language?": ["Rust"] } }]);
});

test("archiving stops a conversation that asks, while an unrelated run keeps going", { timeout: 60000 }, async t => {
  const [parent, busy] = ["ses_archived", "ses_busy"];
  const asked = { id: "next", question: "Go on?", options: [{ label: "Yes" }] };
  const { here, record, go, card } = await asking(t, "engine-archive-waiting", {
    [parent]: { steps: [question("q1", asked), words("Going on.")] },
    [busy]: { steps: [words("Working"), "hang"] },
  });
  for (const id of [parent, busy]) await go(id);
  const runs = () => here.agents.sent("session/prompt").map(message => message.params.sessionId);
  await until(() => runs().length === 2, "Both turns never started.", 20000);
  const native = Object.fromEntries([parent, busy].map(id => [id, runs()[here.agents.prompts().findIndex(text => text.includes(id))]]));
  await card(parent, ask => ask.kind === "question", "It never asked.");
  const cancelled = () => here.agents.sent("session/cancel").map(message => message.params.sessionId);
  const archive = (id, archived) => request(record, "POST", `kuvo/sessions/${id}/archive`, { archived }, headers);
  for (const id of [parent, busy]) {
    const archived = await archive(id, true);
    assert.equal(archived.status, 200, archived.text);
  }
  await until(() => cancelled().includes(native[parent]), "Archiving didn't stop the conversation that asks.", 20000);
  assert.equal((await archive(parent, false)).status, 200);
  assert.equal((await archive(parent, true)).status, 200);
  assert.deepEqual(cancelled(), [native[parent]]);
  const active = await request(record, "GET", "kuvo/agents/api/session/active", undefined, headers);
  assert.ok(JSON.stringify(active.json).includes(busy), active.text);
});

test("waiting counts include a bot’s delegated ask and open Work Queue items once each", { timeout: 60000 }, async t => {
  const bot = "bot_scout";
  const { computer } = await bench(t, { agents: true });
  const here = await computer("engine-waiting-work");
  const folder = path.join(here.home, "bots", bot);
  const [root, work] = ["ses_root", "ses_work"];
  here.agents.seed(root, { directory: folder });
  here.agents.seed(work, { directory: here.home, metadata: { kuvo: { startedBy: root, line: [root] } } });
  here.agents.script("Ask about it", [question("q1", { id: "step", question: "Choose next step", options: [{ label: "Ship" }] }), words("Shipping.")]);
  write(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Scout" }));
  write(path.join(folder, "session"), root);
  const queue = (owner, items) => write(path.join(here.home, "run", "work-queue", "queues", owner + ".json"),
    JSON.stringify({ formatVersion: 1, items }));
  const item = (letter, status, extra = {}) => ({ id: "wqi_" + letter.repeat(20), title: letter, status, ...extra });
  const linked = item("l", "todo", { sessions: [{ session: work }], updated: { at: 2 } });
  queue("usr_you", [
    item("a", "needs-you"), item("b", "needs-you", { sessions: [{ session: work }] }),
    item("r", "needs-you", { removed: { at: 1 } }), linked,
  ]);
  queue(root, [
    item("d", "done", { sessions: [{ session: work }] }),
    item("c", "canceled", { sessions: [{ session: work }] }),
    { ...linked, updated: { at: 1 } },
  ]);
  const record = await here.start().ready();
  const counts = async () => (await request(record, "GET", "kuvo/waiting", undefined, headers)).json;
  await until(async () => (await counts()).queue === 2, "the saved Needs you items");
  assert.deepEqual(await counts(), { sessions: 0, bots: 0, queue: 2 });
  assert.equal((await request(record, "POST", "kuvo/waiting", {}, headers)).status, 405);
  const sent = await request(record, "POST", `kuvo/sessions/${work}/send`, { id: "msg_askAbout", words: "Ask about it", delivery: "steer" }, headers);
  assert.equal(sent.status, 200, sent.text);
  await until(async () => (await counts()).sessions === 1, "the session to ask", 20000, 100);
  assert.deepEqual(await counts(), { sessions: 1, bots: 1, queue: 3 });
  const form = (await request(record, "GET", `kuvo/sessions/${work}/asks`, undefined, headers)).json.asks[0];
  const dismissed = await request(record, "POST", `kuvo/sessions/${work}/asks/${form.id}`, { dismiss: true }, headers);
  assert.equal(dismissed.status, 200, dismissed.text);
  await until(async () => (await counts()).sessions === 0, "the dismissed ask to stop counting", 20000, 100);
  assert.deepEqual(await counts(), { sessions: 0, bots: 0, queue: 2 });
});
