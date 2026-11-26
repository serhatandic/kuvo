// A busy conversation hears its next message before its old turn finishes on its own.
// The stand-ins gate a real tool completion or hold the current turn open. The Rust
// resident, its queue, and all its agent adapters run for real.
import assert from "node:assert/strict";
import test from "node:test";
import path from "node:path";
import { boxBinary, request, until, write } from "./adapter.mjs";
import { headers, result, scriptedClaude, streams, texts } from "./claude.mjs";
import { scriptedCodex } from "./codex.mjs";
import { providers, scriptedAcp } from "./acp.mjs";

const skip = boxBinary().typescript && "The new delivery semantics deliberately differ from the reference.";
const lead = "ses_steerLead";
const target = "ses_steerTarget";
const sendLetter = (fixture, text, delivery) => request(fixture.listener, "POST", "kuvo/agent-tools",
  { name: "send_message", session: lead, arguments: { to: target, text, ...(delivery ? { delivery } : {}) } }, headers);
const send = (fixture, id, words, delivery) => request(fixture.listener, "POST", `kuvo/sessions/${target}/send`, { id, words, delivery }, headers);
const seed = (fixture, seedOne) => {
  for (const id of [lead, target]) {
    const directory = path.join(fixture.home, "projects", id);
    write(path.join(directory, "notes.txt"), "");
    seedOne(id, { directory });
  }
};
const acpWords = text => ({ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } });

for (const [name, provider] of Object.entries(providers)) {
  async function fixture(t) {
    const acp = await scriptedAcp({ names: [name], grok: name === "grok", agy: name === "agy" });
    t.after(() => acp.close());
    seed(acp, (id, options) => acp.seed(id, provider, options));
    await acp.start();
    return acp;
  }
  const heard = (acp, text) => acp.sent("session/prompt").filter(message => JSON.stringify(message.params.prompt).includes(text));
  test(`${name}: a letter reaches an active prompt immediately via Send Now`, { skip, timeout: 90000 }, async t => {
    const acp = await fixture(t);
    acp.script("First task", [acpWords("Working first"), "hang"]);
    acp.script("Now letter", [acpWords("Heard the letter")]);
    await acp.prompt(target, "First task");
    await until(async () => texts(await acp.rows(target)).join("").includes("Working first"), "First turn didn't start");
    const answer = await sendLetter(acp, "Now letter");
    assert.equal(answer.status, 200, answer.text);
    assert.equal(answer.json.delivery, "steered", answer.text);
    await until(() => heard(acp, "Now letter").length === 1, () => "The letter never reached the agent: " + acp.log());
    assert.equal(acp.sent("session/cancel").length, 1);
    await acp.idles(target, 2);
    acp.script("Idle letter", [acpWords("Heard the idle letter")]);
    const idle = await sendLetter(acp, "Idle letter");
    assert.equal(idle.status, 200, idle.text);
    assert.equal(idle.json.delivery, "started a turn", idle.text);
    await until(() => heard(acp, "Idle letter").length === 1, "The idle letter did not begin a turn");
    assert.equal(acp.sent("session/cancel").length, 1);
    await acp.idles(target, 3);
  });
  test(`${name}: a next-step message waits for the tool to end, not the turn`, { skip, timeout: 90000 }, async t => {
    const acp = await fixture(t);
    acp.script("Second task", [
      acpWords("Working second"),
      { update: { sessionUpdate: "tool_call", toolCallId: "tool1", kind: "execute", title: "Run make", status: "in_progress", rawInput: { command: "make" } } },
      { until: "tool-ended" },
      { update: { sessionUpdate: "tool_call_update", toolCallId: "tool1", status: "completed", rawOutput: { stdout: "done", exit_code: 0 } } },
      "hang",
    ]);
    acp.script("Next step message", [acpWords("Heard next step")]);
    await acp.prompt(target, "Second task");
    await until(async () => texts(await acp.rows(target)).join("").includes("Working second"), "Second turn didn't start");
    const next = name === "grok"
      ? await sendLetter(acp, "Next step message", "steer")
      : await send(acp, "msg_nextStep", "Next step message", "steer");
    assert.equal(next.status, 200, next.text);
    if (name === "grok") assert.equal(next.json.delivery, "queued", next.text);
    assert.equal(heard(acp, "Next step message").length, 0, "Sent before the step ended");
    assert.equal(acp.sent("session/cancel").length, 0);
    acp.go("tool-ended");
    await until(() => heard(acp, "Next step message").length === 1, () => "The tool ended but the message never went: " + acp.log(), 15000);
    assert.equal(acp.sent("session/cancel").length, 1);
    await acp.idles(target, 2);
  });
  test(`${name}: a letter explicitly queued for after the turn doesn't cancel it at a tool boundary`, { skip, timeout: 90000 }, async t => {
    const acp = await fixture(t);
    acp.script("First task", [
      acpWords("Working"),
      { update: { sessionUpdate: "tool_call", toolCallId: "tool1", kind: "execute", title: "Run make", status: "in_progress", rawInput: { command: "make" } } },
      { until: "tool-ended" },
      { update: { sessionUpdate: "tool_call_update", toolCallId: "tool1", status: "completed", rawOutput: { stdout: "done", exit_code: 0 } } },
      acpWords("Still working"), { until: "turn-ended" }, acpWords("Done."),
    ]);
    acp.script("After turn message", [acpWords("Heard after turn")]);
    await acp.prompt(target, "First task");
    await until(async () => texts(await acp.rows(target)).join("").includes("Working"), "Turn didn't start");
    const queued = await sendLetter(acp, "After turn message", "queue");
    assert.equal(queued.status, 200, queued.text);
    assert.equal(queued.json.delivery, "queued", queued.text);
    acp.go("tool-ended");
    await until(async () => texts(await acp.rows(target)).join("").includes("Still working"), "Tool didn't finish");
    assert.equal(heard(acp, "After turn message").length, 0);
    assert.equal(acp.sent("session/cancel").length, 0);
    acp.go("turn-ended");
    await until(() => heard(acp, "After turn message").length === 1, "The queued letter didn't reach the next turn");
    await acp.idles(target, 2);
    assert.equal(acp.sent("session/cancel").length, 0);
  });
}

test("Claude Code: letters and messages for the next step reach the same running turn", { skip, timeout: 90000 }, async t => {
  const claude = await scriptedClaude();
  t.after(() => claude.close());
  seed(claude, (id, options) => claude.seed(id, { ...options, model: "default" }));
  claude.script("First task", [
    ...streams("$UUID", [{ type: "text", text: "Working" }]),
    { wait: "user" }, { wait: "user" }, { until: "end" },
    result(["$UUID", "$UUID2", "$UUID3"]),
  ]);
  await claude.start();
  await claude.prompt(target, "First task");
  await until(async () => texts(await claude.rows(target)).join("").includes("Working"), "Turn didn't start");
  const answer = await sendLetter(claude, "Now letter");
  assert.equal(answer.status, 200, answer.text);
  assert.equal(answer.json.delivery, "steered", answer.text);
  const next = await send(claude, "msg_nextStep", "Next step message", "steer");
  assert.equal(next.status, 200, next.text);
  const heard = text => claude.heard().filter(row => row.type === "user" && JSON.stringify(row.message.content).includes(text));
  await until(() => heard("Now letter").length === 1 && heard("Next step message").length === 1, "Claude didn't receive both messages into its turn");
  assert.equal(heard("Now letter")[0].pid, heard("First task")[0].pid);
  assert.equal(heard("Next step message")[0].pid, heard("First task")[0].pid);
  assert.equal((await claude.rows(target)).filter(row => row.type === "idle").length, 0);
  claude.go("end");
  await claude.idles(target);
});

test("Codex: letters and messages for the next step reach the same running turn", { skip, timeout: 90000 }, async t => {
  const codex = await scriptedCodex();
  t.after(() => codex.close());
  seed(codex, (id, options) => codex.seed(id, options));
  codex.script("First task", [
    ["item/completed", { item: { type: "agentMessage", id: "working", text: "Working" } }],
    { until: "end" },
  ]);
  await codex.start();
  await codex.prompt(target, "First task");
  await until(async () => texts(await codex.rows(target)).join("").includes("Working"), "Turn didn't start");
  const answer = await sendLetter(codex, "Now letter");
  assert.equal(answer.status, 200, answer.text);
  assert.equal(answer.json.delivery, "steered", answer.text);
  const next = await send(codex, "msg_nextStep", "Next step message", "steer");
  assert.equal(next.status, 200, next.text);
  const steers = await until(() => {
    const messages = codex.sent("turn/steer");
    return messages.length === 2 && messages;
  }, "Codex didn't receive both messages into its turn");
  assert.ok(JSON.stringify(steers[0]).includes("Now letter"));
  assert.ok(JSON.stringify(steers[1]).includes("Next step message"));
  assert.equal(steers[0].params.expectedTurnId, steers[1].params.expectedTurnId);
  assert.equal(codex.sent("turn/start").length, 1);
  codex.go("end");
  await codex.idles(target);
});

test("Codex: a confirmed native refusal uses the same Send Now fallback", { skip, timeout: 90000 }, async t => {
  const codex = await scriptedCodex();
  t.after(() => codex.close());
  seed(codex, (id, options) => codex.seed(id, options));
  codex.script("First task", [
    ["item/completed", { item: { type: "agentMessage", id: "working", text: "Working" } }],
    "stop",
  ]);
  codex.script("Now letter", [["item/completed", { item: { type: "agentMessage", id: "heard", text: "Heard the letter" } }]]);
  await codex.start();
  await codex.prompt(target, "First task");
  await until(async () => texts(await codex.rows(target)).join("").includes("Working"), "Turn didn't start");
  write(path.join(codex.root, "codex-refuses-steers"), "");
  const answer = await sendLetter(codex, "Now letter");
  assert.equal(answer.status, 200, answer.text);
  assert.equal(answer.json.delivery, "steered", answer.text);
  await until(() => codex.sent("turn/start").length === 2, "A refused steer never went as Send Now");
  assert.equal(codex.sent("turn/steer").length, 1);
  assert.equal(codex.sent("turn/interrupt").length, 1);
  assert.ok(JSON.stringify(codex.sent("turn/start")[1]).includes("Now letter"));
  await codex.idles(target, 2);
});

test("Antigravity: a completed hidden tool is still a next-step boundary", { skip, timeout: 90000 }, async t => {
  const acp = await scriptedAcp({ names: ["agy"], agy: true, grok: false });
  t.after(() => acp.close());
  seed(acp, (id, options) => acp.seed(id, providers.agy, options));
  acp.script("Hidden task", [
    acpWords("Working"),
    { update: { sessionUpdate: "tool_call", toolCallId: "hidden", kind: "other", title: "start_subagent", status: "in_progress", rawInput: { name: "start_subagent" } } },
    { until: "hidden-ended" },
    { update: { sessionUpdate: "tool_call_update", toolCallId: "hidden", status: "completed" } },
    "hang",
  ]);
  await acp.start();
  await acp.prompt(target, "Hidden task");
  await until(async () => texts(await acp.rows(target)).join("").includes("Working"), "Turn didn't start");
  const next = await send(acp, "msg_hiddenStep", "Next step message", "steer");
  assert.equal(next.status, 200, next.text);
  assert.equal(acp.sent("session/cancel").length, 0);
  acp.go("hidden-ended");
  await until(() => acp.sent("session/prompt").some(message => JSON.stringify(message.params.prompt).includes("Next step message")),
    "A hidden tool completed without delivering the next-step message");
  assert.equal(acp.sent("session/cancel").length, 1);
  await acp.idles(target, 2);
});

test("Antigravity: closing a failed turn isn't a next-step boundary", { skip, timeout: 90000 }, async t => {
  const acp = await scriptedAcp({ names: ["agy"], agy: true, grok: false });
  t.after(() => acp.close());
  seed(acp, (id, options) => acp.seed(id, providers.agy, options));
  acp.script("Failing task", [
    acpWords("Working"),
    { until: "fail" },
    { error: { code: -32000, message: "Fixture turn failed." } },
  ]);
  await acp.start();
  await acp.prompt(target, "Failing task");
  await until(async () => texts(await acp.rows(target)).join("").includes("Working"), "Turn didn't start");
  const next = await send(acp, "msg_failedStep", "Next step message", "steer");
  assert.equal(next.status, 200, next.text);
  acp.go("fail");
  const rows = await acp.idles(target);
  assert.equal(rows.filter(row => row.type === "idle").at(-1).outcome, "failed");
  let queue;
  await until(async () => {
    queue = await request(acp.listener, "GET", `kuvo/sessions/${target}/queue?items=1`, undefined, headers);
    assert.equal(queue.status, 200, queue.text);
    return queue.json.paused === true && queue.json.items?.filter(item => item.id === "msg_failedStep").length === 1;
  }, () => "The failed turn did not hold the message: " + queue?.text);
  assert.equal(acp.sent("session/prompt").length, 1);
  assert.equal(acp.sent("session/cancel").length, 0);
});
