// The agents' tools (PROTOCOL.md, "Agents' tools"): a call that changes something, with its
// operation id, is done once, however many times it's sent.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { bench } from "./bench.mjs";
import { boxBinary } from "./adapter.mjs";
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

test("a wake-up's command runs once for its operation: sent again it says the same, and one the box was cut off running isn't run again", { timeout: 60000 }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  lab.engine.sessions.push(sessionOf({ id: "ses_lead", title: "Lead", directory: path.join(lab.home, "..", "work") }));
  // Its command claimed by a process that's gone, before it said anything.
  const key = (nonce) => crypto.createHash("sha256").update(JSON.stringify([JSON.stringify(["tool", "kuvo/agent-tools", "ses_lead"]), nonce])).digest("base64url");
  fs.mkdirSync(path.join(lab.home, "run", "tool-commands"), { recursive: true });
  fs.writeFileSync(path.join(lab.home, "run", "tool-commands", `${key("wake-cut-1")}.json`), JSON.stringify({ pid: 999999 }));
  fs.mkdirSync(path.join(lab.home, "bin"), { recursive: true });
  fs.copyFileSync(new URL("../../box/src/keeper/kuvo-wake", import.meta.url), path.join(lab.home, "bin", "kuvo-wake"));
  fs.chmodSync(path.join(lab.home, "bin", "kuvo-wake"), 0o755);
  const ask = lab.connect(await lab.start().ready());
  const tool = (name, args, operation) => ask("POST", "kuvo/agent-tools", { name, arguments: args, session: "ses_lead", ...(operation ? { operation } : {}) });
  const cut = await tool("schedule_wake_up", { when: "+2h", note: "Look again." }, "wake-cut-1");
  assert.equal(cut.status, 200, cut.text);
  assert.deepEqual(cut.json, { text: "The command worker was interrupted after accepting the command.", isError: true });
  assert.equal((await tool("list_wake_ups", {})).json.text, "No wake-ups scheduled.");
  const made = await tool("schedule_wake_up", { when: "+2h", note: "Look again." }, "wake-once-1");
  assert.equal(made.json.isError, false, made.json.text);
  assert.match(made.json.text, /^Scheduled /);
  assert.deepEqual((await tool("schedule_wake_up", { when: "+2h", note: "Look again." }, "wake-once-1")).json, made.json);
  const listed = (await tool("list_wake_ups", {})).json.text;
  assert.equal(listed.split("\n").filter(line => line.includes("Look again.")).length, 1, listed);
});

// Row 4.11 deliberately changes the reference's delivery choice.
test("a letter to a busy engine conversation is sent now, and an idle one starts a turn", { timeout: 60000, skip: boxBinary().typescript }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  const directory = path.join(lab.home, "..", "work");
  for (const id of ["lead", "busy", "idle"]) lab.engine.sessions.push(sessionOf({ id: "ses_" + id, title: id, directory }));
  lab.engine.running.add("ses_busy");
  const ask = lab.connect(await lab.start().ready());
  const send = (to, delivery) => ask("POST", "kuvo/agent-tools", { name: "send_message", arguments: { to, text: "Check this.", ...(delivery ? { delivery } : {}) }, session: "ses_lead" });
  const busy = await send("ses_busy");
  assert.equal(busy.status, 200, busy.text);
  assert.deepEqual(lab.engine.interrupted, [{ session: "ses_busy", resume: true }]);
  assert.equal(busy.json.delivery, "steered", busy.text);
  assert.equal(lab.engine.inboxes.get("ses_busy")[0].delivery, "steer");
  const idle = await send("ses_idle");
  assert.equal(idle.json.delivery, "started a turn", idle.text);
  assert.equal(lab.engine.messages.get("ses_idle").filter(row => row.text.includes("Check this.")).length, 1);
  const queued = await send("ses_busy", "queue");
  assert.equal(queued.json.delivery, "queued", queued.text);
  assert.equal(lab.engine.inboxes.get("ses_busy").at(-1).delivery, "queue");
  assert.equal(lab.engine.interrupted.length, 1);
  const step = await send("ses_busy", "steer");
  assert.equal(step.json.delivery, "queued", step.text);
  assert.equal(lab.engine.inboxes.get("ses_busy").at(-1).delivery, "steer");
  assert.equal(lab.engine.interrupted.length, 1, "An explicit next-step message does not Send Now");
});
