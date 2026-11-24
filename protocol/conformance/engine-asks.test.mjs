// An OpenCode question comes from a child, reaches the line, and accepts the child's answer.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { openLine, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

test("ask cards use first observation order and show the subagent’s whole command", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-ask-cards");
  const parent = sessionOf({ id: "ses_parent", directory: here.home });
  const child = sessionOf({ id: "ses_child", directory: here.home, parent: parent.id });
  here.engine.sessions.push(parent, child);
  const outside = { id: "per_out", sessionID: parent.id, action: "external_directory",
    resources: [path.join(here.home, "..", "account") + "/notes/*"] };
  const file = here.home + "/src/app.txt";
  const patch = { file, patch: "@@ -1 +1 @@\n-a\n+b\n", additions: 1, deletions: 1, status: "modified" };
  const edit = { id: "per_edit", sessionID: parent.id, action: "edit", time: { created: 1 },
    resources: [file, "b.txt", "c.txt"], save: ["*"], metadata: { files: [patch] } };
  const shell = { id: "per_shell", sessionID: child.id, action: "shell", time: { created: 3 },
    resources: ["git status --short"], save: ["git status *"], source: { type: "tool", id: "call_shell" },
    message: "It wants to check the working tree." };
  const question = { id: "que_color", sessionID: parent.id, title: "Pick", metadata: { kind: "question" },
    time: { created: 2 }, fields: [{ key: "color", type: "string", title: "Color", description: "Which color?",
      required: true, options: [{ value: "blue", label: "Blue", description: "Calm" }] }] };
  const other = { id: "frm_other", sessionID: parent.id, title: "Repository",
    metadata: { kind: "mcp-elicitation", server: "example", message: "Which repository?" },
    fields: [{ key: "repo", type: "number" }] };
  here.engine.permissions.set(parent.id, [edit, outside]);
  here.engine.permissions.set(child.id, [shell]);
  here.engine.forms.set(parent.id, [question, other]);
  here.engine.messages.set(child.id, [{ id: "msg_shell", type: "assistant",
    content: [{ id: "call_shell", type: "tool", name: "shell",
      state: { status: "running", input: { command: "cd app && git status --short" } } }] }]);

  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow asks");
  const saved = path.join(here.home, "run", "glance", "asks.json");
  for (const ask of [other, question, shell, edit]) {
    here.engine.emit(ask.action ? { type: "permission.asked", data: ask } : { type: "form.created", data: { form: ask } });
    await until(() => fs.existsSync(saved) && JSON.parse(fs.readFileSync(saved, "utf8")).asks[ask.id], "the resident to record " + ask.id);
  }
  const route = `kuvo/sessions/${parent.id}/asks`;
  const listed = await request(record, "GET", route);
  assert.equal(listed.status, 200, listed.text);
  const [out, pick, command, change, dismiss] = listed.json.asks;
  assert.deepEqual(listed.json.asks.map(ask => ask.id), [outside.id, question.id, shell.id, edit.id, other.id]);
  assert.equal(out.title, "Allow working outside the project?");
  assert.deepEqual(out.resources, ["~/notes"]);
  assert.deepEqual(pick.questions, [{ key: "color", title: "Color", question: "Which color?",
    options: [{ value: "blue", label: "Blue", description: "Calm" }], multiple: false, custom: false, required: true }]);
  assert.equal(command.session, child.id);
  assert.equal(command.title, "Allow running this command?");
  assert.equal(command.message, shell.message);
  assert.equal(command.command, "cd app && git status --short");
  assert.deepEqual(command.choices[1], { answer: "always", title: "Always Allow", named: "git status", placeholder: null,
    help: "Don’t ask again before running git status commands in this project", spoken: "Always allow git status commands" });
  assert.equal(change.title, "Allow editing src/app.txt and 2 more?");
  assert.deepEqual(change.resources, ["src/app.txt", "b.txt", "c.txt"]);
  assert.deepEqual(change.files, [patch]);
  assert.deepEqual(dismiss, { id: other.id, session: parent.id, kind: "other",
    title: other.title, message: "Which repository?", dismiss: true });

  const answer = (id, body) => request(record, "POST", route + "/" + id, body, { "x-kuvo-from": "app" });
  assert.equal((await answer(other.id, { answers: { repo: 1 } })).status, 400);
  assert.equal((await answer(other.id, { dismiss: true })).status, 200);
  assert.deepEqual(here.engine.asked.filter(row => row.method !== "GET" && row.path.includes(other.id)),
    [{ method: "DELETE", path: `/api/session/${parent.id}/form/${other.id}`, json: {} }]);
});

test("asks report a missing conversation, a busy engine, and the engine’s own reply refusal", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-ask-errors");
  const parent = sessionOf({ id: "ses_parent", directory: here.home });
  here.engine.sessions.push(parent);
  const ask = { id: "per_edit", sessionID: parent.id, action: "edit", resources: ["example.txt"] };
  here.engine.permissions.set(parent.id, [ask]);
  const record = await here.start().ready();
  const read = id => request(record, "GET", `kuvo/sessions/${id}/asks`);
  assert.equal((await read("ses_missing")).status, 404);
  here.engine.unavailableSession = parent.id;
  assert.equal((await read(parent.id)).status, 503);
  here.engine.unavailableSession = null;
  here.engine.sessionListUnavailable = true;
  const busy = await read(parent.id);
  assert.equal(busy.status, 503, busy.text);
  assert.match(busy.json.message, /isn't answering/);
  here.engine.sessionListUnavailable = false;
  here.engine.refusedAsk = ask.id;
  const refused = await request(record, "POST", `kuvo/sessions/${parent.id}/asks/${ask.id}`,
    { answer: "once" }, { "x-kuvo-from": "app" });
  assert.equal(refused.status, 502, refused.text);
  assert.deepEqual(refused.json, { message: "It can't take that now." });
  assert.deepEqual((await read(parent.id)).json.asks.map(item => item.id), [ask.id]);
});

test("OpenCode permission cards preserve the shared Always button words", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-ask-words");
  const session = sessionOf({ id: "ses_words", directory: here.home });
  const account = path.join(here.home, "..", "account");
  const fixture = JSON.parse(fs.readFileSync(new URL("../../box/test/fixtures/always-words.json", import.meta.url)));
  const cases = fixture.cases.map(({ action, save, resources, home, plain, help, spoken }, index) => {
    const useAccount = words => words.map(word => home && word.startsWith(home + "/") ? account + word.slice(home.length) : word);
    return { ask: { id: "per_words_" + index, sessionID: session.id, action,
      ...(save ? { save: useAccount(save) } : {}), resources: useAccount(resources) }, plain, help, spoken };
  });
  here.engine.sessions.push(session);
  here.engine.permissions.set(session.id, cases.map(each => each.ask));
  const resident = await here.start().ready();
  const reply = await request(resident, "GET", `kuvo/sessions/${session.id}/asks`);
  assert.equal(reply.status, 200, reply.text);
  const cards = new Map(reply.json.asks.map(ask => [ask.id, ask]));
  for (const { ask, plain, help, spoken } of cases) {
    const choice = cards.get(ask.id)?.choices.find(choice => choice.answer === "always");
    assert.ok(choice, JSON.stringify(ask));
    assert.deepEqual({
      plain: choice.named === null ? choice.title : choice.title + " " + choice.named,
      help: choice.help, spoken: choice.spoken,
    }, { plain, help, spoken }, JSON.stringify(ask));
  }
});

test("a live permission read can expire an ask or discover a new local Block", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-ask-consent");
  const session = sessionOf({ id: "ses_consent", directory: here.home });
  const ask = { id: "per_edit", sessionID: session.id, action: "edit", resources: ["notes.txt"] };
  here.engine.sessions.push(session);
  here.engine.permissions.set(session.id, [ask]);
  const record = await here.start().ready();
  const raw = `api/session/${session.id}/permission/${ask.id}/reply`;
  const card = `kuvo/sessions/${session.id}/asks/${ask.id}`;
  const headers = { "x-kuvo-from": "app" };
  const grant = route => request(record, "POST", route,
    route === raw ? { decision: "once" } : { answer: "once" }, headers);
  const sent = () => here.engine.asked.filter(row => row.method === "POST" && row.path === "/" + raw);
  here.engine.beforeLivePermission = () => here.engine.permissions.set(session.id, []);
  const ended = await grant(raw);
  assert.equal(ended.status, 404, ended.text);
  assert.deepEqual(sent(), []);

  here.engine.permissions.set(session.id, [ask]);
  const config = path.join(here.home, "box", "opencode.json");
  here.engine.beforeLivePermission = () => write(config,
    JSON.stringify({ permissions: [{ action: "edit", resource: "*", effect: "deny" }] }));
  for (const route of [card, raw]) {
    const blocked = await grant(route);
    assert.equal(blocked.status, 403, blocked.text);
  }
  assert.deepEqual(sent(), []);

  here.engine.beforeLivePermission = null;
  fs.rmSync(config);
  const allowed = await grant(card);
  assert.equal(allowed.status, 200, allowed.text);
  assert.equal(sent().length, 1);
  assert.equal((await grant(card)).status, 404);
  assert.equal(sent().length, 1);
});

test("a child question changes the parent’s asks and waiting counts through the resident", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-asks");
  const parent = sessionOf({ id: "ses_parent", title: "Main", directory: here.home });
  const child = sessionOf({ id: "ses_child", title: "Child", directory: here.home, parent: parent.id });
  here.engine.sessions.push(parent, child);
  const base = "kuvo/sessions/" + parent.id + "/asks";
  const resident = await here.start().ready();
  const initial = await request(resident, "GET", base);
  assert.equal(initial.status, 200, initial.text);
  assert.deepEqual(initial.json, { asks: [] });

  const watch = Buffer.from(JSON.stringify({
    asks: base + "/events",
    counts: "kuvo/waiting/events",
  })).toString("base64url");
  const line = await openLine(resident, "?watch=" + watch);
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
  await until(() => here.engine.subscribers() > 0, () => "The resident did not follow the engine’s events.");
  const form = {
    id: "form_child", sessionID: child.id, title: "Pick next step", metadata: { kind: "question" },
    fields: [
      { key: "language", type: "string", title: "Language", required: true, options: [{ value: "rust", label: "Rust" }] },
      { key: "scope", type: "string", hidden: true, default: "local" },
    ],
  };
  here.engine.forms.set(child.id, [form]);
  here.engine.emit({ type: "form.created", data: { form } });
  const asks = await next(frame => frame.t === "asks" && frame.d?.asks?.length > 0, "the question");
  assert.deepEqual(asks.asks[0], {
    id: form.id, kind: "question", session: child.id, title: form.title,
    questions: [{
      key: "language", title: "Language", question: "Language",
      options: [{ value: "rust", label: "Rust", description: null }],
      multiple: false, custom: false, required: true,
    }],
  });
  assert.deepEqual(await next(frame => frame.t === "counts" && frame.d?.sessions === 1, "one waiting session"),
    { sessions: 1, bots: 0, queue: 0 });
  const answered = base + "/" + form.id;
  const header = { "x-kuvo-from": "app" };
  const refused = await request(resident, "POST", answered, { answers: { language: "python" } }, header);
  assert.equal(refused.status, 400, refused.text);
  const accepted = await request(resident, "POST", answered, { answers: { language: "rust" } }, header);
  assert.equal(accepted.status, 200, accepted.text);
  assert.deepEqual(here.engine.asked.find(row => row.path.endsWith("/form/form_child/reply")), {
    method: "POST", path: "/api/session/ses_child/form/form_child/reply",
    json: { answer: { language: "rust", scope: "local" } },
  });
  await next(frame => frame.t === "asks" && frame.d?.asks?.length === 0, "the answered question");
  await next(frame => frame.t === "counts" && frame.d?.sessions === 0, "zero waiting sessions");
  assert.deepEqual((await request(resident, "GET", base)).json, { asks: [] });
});

test("archiving stops a conversation and the child that asks, while an unrelated run keeps going", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-archive-waiting");
  const parent = sessionOf({ id: "ses_parent", title: "Main", directory: here.home });
  const child = sessionOf({ id: "ses_child", title: "Child", directory: here.home, parent: parent.id });
  const busy = sessionOf({ id: "ses_busy", title: "Busy", directory: here.home });
  here.engine.sessions.push(parent, child, busy);
  here.engine.forms.set(child.id, [{ id: "form_child", title: "Question", fields: [] }]);
  const resident = await here.start().ready();
  const patch = (session, archived, pinned = false) => request(resident, "PATCH", "api/session/" + session, {
    metadata: { "kuvo.archived": archived, ...(pinned ? { "kuvo.pinned": true } : {}) },
  });
  assert.equal((await patch(parent.id, true)).status, 200);
  assert.equal((await patch(busy.id, true)).status, 200);
  assert.equal((await patch(parent.id, true, true)).status, 200);
  assert.equal((await patch(parent.id, false)).status, 200);
  assert.deepEqual(here.engine.interrupted.map(row => row.session), [parent.id, child.id]);

  const typed = await request(resident, "POST", "kuvo/sessions/" + parent.id + "/archive", { archived: true });
  assert.equal(typed.status, 200, typed.text);
  assert.deepEqual(here.engine.interrupted.map(row => row.session), [parent.id, child.id, parent.id, child.id]);
});

test("an MCP form goes to the first conversation waiting on its server and is dismissed in its folder", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-shared-form");
  const parent = sessionOf({ id: "ses_main", directory: here.home });
  const child = sessionOf({ id: "ses_child", directory: here.home, parent: parent.id });
  const another = sessionOf({ id: "ses_other", directory: here.home });
  here.engine.sessions.push(parent, child, another);
  const form = { id: "frm_issue", sessionID: "global", title: "New issue",
    metadata: { kind: "mcp-elicitation", server: "issue-tracker", message: "Which repository?" } };
  here.engine.globalForms.set(here.home, [form]);
  const record = await here.start().ready();
  const asks = session => `kuvo/sessions/${session.id}/asks`;
  assert.deepEqual((await request(record, "GET", asks(parent))).json, { asks: [] });
  for (const session of [child, another]) {
    here.engine.running.add(session.id);
    here.engine.messages.set(session.id, [{ id: "msg_issue", type: "assistant",
      content: [{ type: "tool", name: "issue-tracker_create", state: { status: "running" } }] }]);
  }
  const expected = { id: form.id, kind: "other", session: parent.id, title: form.title,
    message: form.metadata.message, dismiss: true };
  assert.deepEqual((await request(record, "GET", asks(parent))).json, { asks: [expected] });
  assert.deepEqual((await request(record, "GET", asks(another))).json, { asks: [] });
  const reply = asks(parent) + "/" + form.id;
  const refused = await request(record, "POST", reply, { answers: { repo: "example" } }, { "x-kuvo-from": "app" });
  assert.equal(refused.status, 400, refused.text);
  const dismissed = await request(record, "POST", reply, { dismiss: true }, { "x-kuvo-from": "app" });
  assert.equal(dismissed.status, 200, dismissed.text);
  assert.deepEqual(here.engine.dismissedGlobalForms, [{ id: form.id, folder: here.home }]);
  assert.deepEqual((await request(record, "GET", asks(parent))).json, { asks: [] });
});

test("waiting counts include a bot’s delegated ask and open Work Queue items once each", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-waiting-work");
  const bot = "bot_scout";
  const folder = path.join(here.home, "bots", bot);
  const root = sessionOf({ id: "ses_root", directory: folder });
  const work = sessionOf({ id: "ses_work", directory: here.home });
  work.metadata = { kuvo: { startedBy: root.id, line: [root.id] } };
  here.engine.sessions.push(root, work);
  write(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Scout" }));
  write(path.join(folder, "session"), root.id);
  const queue = (owner, items) => write(path.join(here.home, "run", "work-queue", "queues", owner + ".json"),
    JSON.stringify({ formatVersion: 1, items }));
  const item = (letter, status, extra = {}) => ({ id: "wqi_" + letter.repeat(20), title: letter, status, ...extra });
  const linked = item("l", "todo", { sessions: [{ session: work.id }], updated: { at: 2 } });
  queue("usr_you", [
    item("a", "needs-you"), item("b", "needs-you", { sessions: [{ session: work.id }] }),
    item("r", "needs-you", { removed: { at: 1 } }), linked,
  ]);
  queue("ses_root", [
    item("d", "done", { sessions: [{ session: work.id }] }),
    item("c", "canceled", { sessions: [{ session: work.id }] }),
    { ...linked, updated: { at: 1 } },
  ]);
  const record = await here.start().ready();
  const counts = async () => (await request(record, "GET", "kuvo/waiting")).json;
  await until(async () => (await counts()).queue === 2, "the saved Needs you items");
  assert.deepEqual(await counts(), { sessions: 0, bots: 0, queue: 2 });
  assert.equal((await request(record, "POST", "kuvo/waiting", {})).status, 405);
  await until(() => here.engine.subscribers() > 0, "the resident to follow asks");
  const form = { id: "form_scoutextra", sessionID: work.id, title: "Choose next step", fields: [] };
  here.engine.forms.set(work.id, [form]);
  here.engine.emit({ type: "form.created", data: { form } });
  await until(async () => (await counts()).sessions === 1, "the session to ask");
  assert.deepEqual(await counts(), { sessions: 1, bots: 1, queue: 3 });
  const dismissed = await request(record, "POST", `kuvo/sessions/${work.id}/asks/${form.id}`,
    { dismiss: true }, { "x-kuvo-from": "app" });
  assert.equal(dismissed.status, 200, dismissed.text);
  await until(async () => (await counts()).sessions === 0, "the dismissed ask to stop counting");
  assert.deepEqual(await counts(), { sessions: 0, bots: 0, queue: 2 });
});

test("raw and card permission replies honor a Block saved after the ask arrived", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("engine-consent");
  const session = sessionOf({ id: "ses_consent", directory: here.home });
  const ask = { id: "per_edit", action: "edit", resources: ["testing/private.txt"] };
  here.engine.sessions.push(session);
  here.engine.permissions.set(session.id, [ask]);
  const record = await here.start().ready();
  const card = `kuvo/sessions/${session.id}/asks/${ask.id}`;
  const raw = `api/session/${session.id}/permission/${ask.id}/reply`;
  const settings = path.join(here.home, "box", "opencode.json");
  const rules = permissions => write(settings, JSON.stringify({ permissions }));
  const header = { "x-kuvo-from": "app" };
  assert.equal((await request(record, "GET", `kuvo/sessions/${session.id}/asks`)).json.asks[0].id, ask.id);
  rules([{ action: "write", resource: "testing/private.*", effect: "deny" }]);
  const rawRefused = await request(record, "POST", raw, { decision: "once" }, header);
  const cardRefused = await request(record, "POST", card, { answer: "once" }, header);
  for (const reply of [rawRefused, cardRefused]) {
    assert.equal(reply.status, 403, reply.text);
    assert.equal(reply.json.message, "This computer’s permissions no longer allow that action.");
  }
  assert.deepEqual(here.engine.permissions.get(session.id), [ask]);
  assert.equal(here.engine.asked.filter(row => row.method === "POST" && row.path === "/" + raw).length, 0);
  rules([{ action: "write", resource: "unrelated/*", effect: "deny" }]);
  const always = await request(record, "POST", card, { answer: "always" }, header);
  assert.equal(always.status, 403, always.text);
  assert.equal(always.json.message, "An Always answer could override a local restriction for this tool. Use Allow Once for this request.");
  assert.equal((await request(record, "POST", card, { answer: "once" }, header)).status, 200);
  assert.equal((await request(record, "POST", raw, { decision: "once" }, header)).status, 404);
  assert.equal(here.engine.asked.filter(row => row.method === "POST" && row.path === "/" + raw).length, 1);
});
