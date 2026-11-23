// An OpenCode question comes from a child, reaches the line, and accepts the child's answer.
import assert from "node:assert/strict";
import { test } from "node:test";
import { openLine, request, until } from "./adapter.mjs";
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
