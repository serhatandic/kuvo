// A local draft is acknowledged from disk, then confirmed after a broken handoff. The
// conversations are a coding agent's (the scripted Grok, `acp.mjs`), on each computer.
import assert from "node:assert/strict";
import { test } from "node:test";
import { openLine, request, until } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { parkedTimers } from "./idle-counter.mjs";

const headers = { "x-kuvo-from": "app" };
const words = text => ({ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } });
/** How many of the agent's prompts had `text`. */
const heard = (agents, text) => agents.prompts().filter(prompt => prompt.includes(text)).length;
const reached = (agents, text, why) => until(() => heard(agents, text) > 0, why, 20000);

/**
 * Two computers in one setup, the conversation on the owner's agent (seeded once it joined: a
 * computer with conversations of its own can't join). Drafts sent from the listing computer while
 * the owner is away wait there.
 */
async function pair(t, session, steps) {
  const { computer, leases } = await bench(t, { agents: true });
  const listing = await computer("Listing");
  const owner = await computer("Owner");
  let listingBox = listing.start();
  let primary = listing.connect(await listingBox.ready());
  const setup = await primary("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  const invitation = await primary("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invitation.status, 201, invitation.text);
  await until(() => leases.leaseFor(setup.json.setup), "The listing computer did not claim its setup.");
  const joined = await owner.join(invitation.json.code);
  assert.equal(joined.code, 0, joined.log);
  owner.agents.seed(session, { directory: owner.home });
  for (const [text, script] of Object.entries(steps ?? {})) owner.agents.script(text, script);
  let ownerBox = owner.start();
  let ownerRecord = await ownerBox.ready();
  const target = owner.connect(ownerRecord);
  const membership = await target("GET", "kuvo/setup");
  assert.equal(membership.status, 200, membership.text);
  const key = membership.json.you.key;
  const local = await target("GET", "kuvo/sessions/list");
  assert.equal(local.status, 200, local.text);
  await until(async () => {
    const scope = await primary("GET", `kuvo/sessions/${session}/submissions?scope=1`, undefined, headers);
    return scope.json?.currentOwner === key && scope.json.canSubmit;
  }, "The listing computer did not learn the conversation's owner.", 30000, 50);
  return {
    listing, owner, key, setup: setup.json.setup,
    get primary() { return primary; },
    get ownerRecord() { return ownerRecord; },
    stopOwner: () => ownerBox.stop(),
    async startOwner() { ownerBox = owner.start(); ownerRecord = await ownerBox.ready(); return ownerRecord; },
    async restartListing() {
      listingBox.child.kill("SIGKILL");
      await listingBox.exited;
      listingBox = listing.start();
      primary = listing.connect(await listingBox.ready());
      return primary;
    },
  };
}

test("delivery recovery parks with no saved drafts", { timeout: 30000 }, async t => {
  await parkedTimers(t, ["delivery.recovery"], 6500);
});

test("two computers deliver addressed drafts and recover the original scoped receipt", { timeout: 90000 }, async t => {
  const session = "ses_addressed";
  const base = "kuvo/sessions/" + session;
  const two = await pair(t, session);
  const { primary, listing, owner, key, setup } = two;
  const scopedId = "msg_remoteScope";
  const submission = [setup, key, scopedId].join(".");
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
  // (A receipt settles once its conversation's line has it; its agent takes it in its turn.)
  for (const said of [scoped.text, text.words, draft.text]) await reached(owner.agents, said, `"${said}" never reached the agent.`);
  for (const said of [scoped.text, text.words, draft.text]) assert.equal(heard(owner.agents, said), 1, said);
  assert.deepEqual(await owner.agents.users(two.ownerRecord, session), [scopedId, text.id, draft.id]);
  assert.deepEqual(listing.agents.prompts(), []);
});

test("a saved draft replies before the agent and settles its original receipt after restart", { timeout: 60000 }, async t => {
  const { computer } = await bench(t, { agents: true });
  const here = await computer("engine-delivery");
  const session = "ses_delivery";
  const base = "kuvo/sessions/" + session;
  here.agents.seed(session, { directory: here.home });
  // (The agent takes it and is still on it when the box stops.)
  here.agents.script("Saved before", [words("Working"), "hang"]);
  const first = here.start();
  const original = await first.ready();
  const draft = { id: "msg_savedDraft", text: "Saved before the agent replies.", message: {} };
  const saved = await request(original, "POST", base + "/delivery?computer=here", draft, headers);
  assert.equal(saved.status, 202, saved.text);
  assert.equal(saved.json.status, "pending");
  assert.equal(saved.json.owner, "here");
  const receiptRoute = "kuvo/data/messages/" + saved.json.operation;
  const pending = await request(original, "GET", receiptRoute, undefined, headers);
  assert.equal(pending.json.operation, saved.json.operation);
  await reached(here.agents, draft.text, "The draft never reached the agent.");
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
  assert.deepEqual(await here.agents.users(next, session), [draft.id]);
  assert.equal(heard(here.agents, draft.text), 1);
});

test("taking back a draft waiting for its away computer keeps its recovery copy until the composer acknowledges it", { timeout: 90000 }, async t => {
  const session = "ses_deliveryEdit";
  const route = `kuvo/sessions/${session}/delivery`;
  const two = await pair(t, session, { "Already handed": [words("On it"), "hang"] });
  const first = { id: "msg_handoff", text: "Already handed off", message: {} };
  const sent = await two.primary("POST", `${route}?computer=${two.key}`, first, headers);
  assert.equal(sent.status, 202, sent.text);
  await reached(two.owner.agents, first.text, "The first message never reached the agent.");
  await two.stopOwner();
  const files = [{ name: "notes.txt", uri: "data:text/plain;base64,eA==" }];
  const draft = { id: "msg_waiting", text: "Visible text", quotedText: "Full draft", message: { files } };
  const saved = await two.primary("POST", `${route}?computer=${two.key}`, draft, headers);
  assert.equal(saved.status, 202, saved.text);
  const waiting = await two.primary("GET", `${route}?computer=${two.key}`, undefined, headers);
  assert.deepEqual(waiting.json.items.map(item => item.id), [draft.id]);
  const editing = `${route}/${draft.id}`;
  const removed = { removed: { id: draft.id, words: draft.quotedText, draft: draft.quotedText, files, receipt: "edit-1" } };
  const takeBack = () => two.primary("DELETE", `${editing}?computer=${two.key}&receipt=edit-1`, undefined, headers);
  const result = await takeBack();
  assert.equal(result.status, 200, result.text);
  assert.deepEqual(result.json, removed);
  await two.restartListing();

  const repeated = await takeBack();
  assert.equal(repeated.status, 200, repeated.text);
  assert.deepEqual(repeated.json, removed);
  const receipt = await two.primary("GET", "kuvo/data/messages/" + saved.json.operation, undefined, headers);
  assert.deepEqual(receipt.json, {
    operation: saved.json.operation, status: "interrupted", owner: two.key, message: "The message was removed.",
  });
  const ack = await two.primary("POST", `${editing}/ack?computer=${two.key}&receipt=edit-1`, undefined, headers);
  assert.equal(ack.status, 200, ack.text);
  assert.deepEqual((await takeBack()).json, { removed: null });
  assert.equal((await two.primary("POST", `${route}?computer=${two.key}`, draft, headers)).status, 409);
  const back = await two.startOwner();
  assert.deepEqual(await two.owner.agents.users(back, session), [first.id]);
  assert.equal(heard(two.owner.agents, draft.text), 0);
});

test("Steer and Send Now changed on drafts waiting for their away computer reach its agent after a restart", { timeout: 90000 }, async t => {
  const session = "ses_deliveryMode";
  const route = `kuvo/sessions/${session}/delivery`;
  const two = await pair(t, session, { "Already delivered": [words("On it"), "hang"] });
  const first = { id: "msg_beforeMode", text: "Already delivered", message: {} };
  assert.equal((await two.primary("POST", `${route}?computer=${two.key}`, first, headers)).status, 202);
  await reached(two.owner.agents, first.text, "The first message never reached the agent.");
  await two.stopOwner();
  const queued = [];
  for (const mode of ["steer", "now"]) {
    const id = "msg_" + mode;
    const draft = { id, text: `Changed to ${mode}`, message: { delivery: "queue" } };
    const saved = await two.primary("POST", `${route}?computer=${two.key}`, draft, headers);
    assert.equal(saved.status, 202, saved.text);
    const edited = await two.primary("POST", `${route}/${id}/${mode}?computer=${two.key}`, undefined, headers);
    assert.equal(edited.status, 200, edited.text);
    queued.push({ id, operation: saved.json.operation });
  }
  await two.restartListing();
  const back = await two.startOwner();
  for (const { operation } of queued) {
    await until(async () => (await two.primary("GET", "kuvo/data/messages/" + operation, undefined, headers)).json?.status === "committed",
      "the changed draft to be accepted", 30000, 150);
  }
  for (const mode of ["steer", "now"]) await reached(two.owner.agents, `Changed to ${mode}`, `The ${mode} draft never reached the agent.`);
  for (const mode of ["steer", "now"]) assert.equal(heard(two.owner.agents, `Changed to ${mode}`), 1, mode);
  // (Send Now can go ahead of a Steer, which waits for the agent's next step.)
  const users = await two.owner.agents.users(back, session);
  assert.deepEqual([users[0], ...users.slice(1).sort()], [first.id, "msg_now", "msg_steer"]);
});
