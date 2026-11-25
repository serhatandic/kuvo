// The agents' tools (PROTOCOL.md, "Agents' tools"): a call that changes something, with its
// operation id, is done once, however many times it's sent.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

test("a start cut short and sent again makes one session, given its task once; sent again after, it says the same", { timeout: 60000 }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  lab.engine.sessions.push(sessionOf({ id: "ses_lead", title: "Lead", directory: path.join(lab.home, "..", "work") }));
  const ask = lab.connect(await lab.start().ready());
  const args = { chat: true, title: "One", task: "Do it." };
  const start = (operation, given = args) => ask("POST", "kuvo/agent-tools", { name: "start_session", arguments: given, session: "ses_lead", operation });
  // Its task doesn't reach it the first time: the call is to be sent again.
  lab.engine.refusePrompt = { any: true, status: 503 };
  const cut = await start("start-cut-1");
  assert.equal(cut.status, 503, cut.text);
  assert.equal(cut.json.retryToolCall, true);
  const made = lab.engine.sessions.filter(each => each.title === "One");
  assert.equal(made.length, 1, "made before the cut");
  const again = await start("start-cut-1");
  assert.equal(again.status, 200, again.text);
  assert.equal(again.json.isError, false, again.json.text);
  assert.match(again.json.text, new RegExp(`^Started "One" \\(address ${made[0].id}\\)`));
  assert.equal(lab.engine.sessions.filter(each => each.title === "One").length, 1, "not made twice");
  const prompts = lab.engine.asked.filter(each => each.method === "POST" && each.path === `/api/session/${made[0].id}/prompt`);
  assert.ok(prompts.length >= 1, "its task reached it");
  assert.equal(new Set(prompts.map(each => each.json.id)).size, 1, "as one message");
  // Its answer lost on the way: sent again, the same answer, nothing done again.
  assert.deepEqual((await start("start-cut-1")).json, again.json);
  assert.equal(lab.engine.sessions.filter(each => each.title === "One").length, 1);
  // The same id for another call is refused.
  assert.equal((await start("start-cut-1", { ...args, title: "Two" })).status, 409);
});

test("a message cut short and sent again reaches its conversation once; sent again after, it says the same", { timeout: 60000 }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  const directory = path.join(lab.home, "..", "work");
  lab.engine.sessions.push(sessionOf({ id: "ses_lead", title: "Lead", directory }), sessionOf({ id: "ses_target", title: "Target", directory }));
  const ask = lab.connect(await lab.start().ready());
  const args = { to: "ses_target", text: "Look at this." };
  const send = (operation, given = args) => ask("POST", "kuvo/agent-tools", { name: "send_message", arguments: given, session: "ses_lead", operation });
  // Its delivery isn't confirmed the first time: the call is to be sent again.
  lab.engine.refusePrompt = { any: true, status: 503 };
  const cut = await send("message-cut-1");
  assert.equal(cut.status, 503, cut.text);
  assert.equal(cut.json.retryToolCall, true);
  lab.engine.refusePrompt = undefined;
  const again = await send("message-cut-1");
  assert.equal(again.status, 200, again.text);
  assert.equal(again.json.isError, false, again.json.text);
  assert.match(again.json.text, /^Sent to Target\./);
  const prompts = lab.engine.asked.filter(each => each.method === "POST" && each.path === "/api/session/ses_target/prompt");
  assert.ok(prompts.length >= 1, "it reached its conversation");
  assert.equal(new Set(prompts.map(each => each.json.id)).size, 1, "as one message");
  // Its answer lost on the way: sent again, the same answer, nothing sent again.
  assert.deepEqual((await send("message-cut-1")).json, again.json);
  assert.equal(lab.engine.asked.filter(each => each.method === "POST" && each.path === "/api/session/ses_target/prompt").length, prompts.length);
});
