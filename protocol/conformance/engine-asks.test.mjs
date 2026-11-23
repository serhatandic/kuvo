// An OpenCode question comes from a child, reaches the line, and accepts the child's answer.
import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { openLine, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

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
  queue("usr_you", [item("a", "needs-you"), item("r", "needs-you", { removed: { at: 1 } }), linked]);
  queue("ses_root", [item("d", "done", { sessions: [{ session: work.id }] }), { ...linked, updated: { at: 1 } }]);
  const record = await here.start().ready();
  const counts = async () => (await request(record, "GET", "kuvo/waiting")).json;
  await until(async () => (await counts()).queue === 1, "the saved Needs you item");
  assert.deepEqual(await counts(), { sessions: 0, bots: 0, queue: 1 });
  await until(() => here.engine.subscribers() > 0, "the resident to follow asks");
  const form = { id: "form_scoutextra", sessionID: work.id, title: "Choose next step", fields: [] };
  here.engine.forms.set(work.id, [form]);
  here.engine.emit({ type: "form.created", data: { form } });
  await until(async () => (await counts()).sessions === 1, "the session to ask");
  assert.deepEqual(await counts(), { sessions: 1, bots: 1, queue: 2 });
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
