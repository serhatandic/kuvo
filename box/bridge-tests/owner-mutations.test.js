import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

const url = name => JSON.stringify(new URL("../src/link/" + name + ".ts", import.meta.url).href);
// The main renews its relay lease every 10 s; a test of what the main saves starts with a fresh one.
const leaseHelper = JSON.stringify(new URL("../test/relay-lease.ts", import.meta.url).href);
const bun = process.env.KUVO_TEST_BUN || "bun";

function isolated(code) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-owner-test-"));
  removeAtEnd(home);
  const kuvo = path.join(home, ".kuvo-code");
  fs.mkdirSync(kuvo);
  fs.writeFileSync(path.join(kuvo, "folder-version.json"), '{"version":7}');
  const ran = spawnSync(bun, ["--config=/dev/null", "--no-env-file", "-e", `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { publicOf } from ${url("crypto/keys")};
    import { newSigningSeed, signingKeyOf } from ${url("documents/format")};
    import { readLink, writeLink } from ${url("state")};
    import { engineHandler } from ${url("server/router")};
    import { collected } from ${url("server/sent-once")};
    import { ownKey } from ${url("peers/known")};
    const seed = Buffer.alloc(32, 42);
    const keyOf = n => publicOf(Buffer.alloc(32, n)).toString("base64url");
    const own = keyOf(42), from = keyOf(43), other = keyOf(44);
    const peer = key => ({ key, kind: "machine", name: key, address: "A".repeat(22) });
    const sign = newSigningSeed();
    const id = "set_" + "A".repeat(22);
    const setup = { id, primary: own, sign: signingKeyOf(sign), role: "primary", managed: true, checkpointed: true, epoch: 1 };
    writeLink({ key: seed.toString("base64url"), sign, setup, sharedSetup: id,
      shared: { membershipAndAccess: { sequence: 1, value: { left: [], boxes: [own, from, other].map(peer) } } } });
    assert.equal(ownKey(readLink()), own);
    const handle = engineHandler(undefined, { name: "Owner" });
    const ask = async (method, route, body = null, headers = {}) => {
      const reply = await collected(handle({ fromApp: true, method, path: route, headers, body: Buffer.from(JSON.stringify(body)) }));
      return { status: reply.status, value: JSON.parse(reply.body) };
    };
    ${code}
  `], { encoding: "utf8", timeout: 45_000, env: { ...process.env, HOME: home, KUVO_HOME: kuvo } });
  assert.equal(ran.status, 0, ran.stderr || String(ran.error ?? ran.signal));
  return ran.stdout;
}

test("one queue creation receipt covers direct requests and fresh forwarding attempts", () => isolated(`
  import { takeAct, forgetActs } from ${url("work-queue/forward")};
  import { writeForPeer } from ${url("ownership/write")};
  import { allItems } from ${url("work-queue/store")};
  const route = "kuvo/work-queue/items", operation = "client-create-once";
  const value = { owner: "usr_you", title: "Build once" };
  const attempt = async (suffix, sent = value, nonce = operation) => {
    let answer;
    await takeAct(peer(from), { id: "wqa_" + suffix.repeat(22), at: Date.now(), area: "ownership",
      item: route, method: "POST", ...(nonce ? { operation: nonce } : {}), body: { value: sent } }, reply => { answer = reply.acted; },
      act => writeForPeer(peer(from), act, { targets: [] }));
    const { id: _delivery, ...result } = answer;
    return result;
  };
  assert.equal((await ask("POST", route, value)).status, 400);
  assert.equal((await attempt("D", value, "")).status, 400);
  const first = await attempt("A");
  assert.equal(first.status, 200);
  forgetActs();
  assert.deepEqual(await attempt("B"), first);
  assert.deepEqual(await ask("POST", route, { ...value, operation }), first);
  assert.deepEqual(await ask("POST", route, value, { "x-kuvo-operation": operation }), first);
  assert.equal((await attempt("C", { ...value, title: "Different intent" })).status, 409);
  assert.equal(allItems().length, 1);
`));

test("a forwarded action never runs when its durable claim cannot be written", () => isolated(`
  import { takeAct } from ${url("work-queue/forward")};
  import { WORK_QUEUE } from ${url("work-queue/store")};
  const claim = path.join(WORK_QUEUE, "acted.json");
  fs.mkdirSync(WORK_QUEUE, { recursive: true });
  // A real directory at the receipt's address prevents reading or replacing the claim.
  fs.mkdirSync(claim);
  let calls = 0, answer;
  await takeAct(peer(from), { id: "wqa_" + "A".repeat(22), at: Date.now(), item: "anything", method: "PATCH" },
    reply => { answer = reply.acted; }, async () => { calls++; return { status: 200, value: {} }; });
  assert.equal(calls, 0);
  assert.equal(answer.status, 503);
`));

test("ordinary revision-aware queue edits and the offline route replay the same decision", () => isolated(`
  const item = "wqi_" + "A".repeat(20);
  const ref = { kind: "queueItems", id: item, queue: "usr_you" };
  const edit = { ref, operation: "create-with-revision", baseRevision: null, create: true, fields: { title: "Original" } };
  const direct = await ask("POST", "kuvo/work-queue/items", { id: item, owner: ref.queue, baseRevision: null, fields: edit.fields }, { "x-kuvo-operation": edit.operation });
  assert.equal(direct.value.status, "committed");
  assert.deepEqual(await ask("POST", "kuvo/data/edits", edit), direct);
  const changed = await ask("PATCH", "kuvo/work-queue/items/" + item,
    { owner: ref.queue, operation: "change-with-revision", baseRevision: direct.value.revision, fields: { title: "Updated" } });
  assert.equal(changed.value.status, "committed");
  assert.deepEqual(await ask("POST", "kuvo/data/edits", { ref, operation: "change-with-revision", baseRevision: direct.value.revision, fields: { title: "Updated" } }), changed);
`));

test("only the current main saves a setup-wide value", () => isolated(`
  import { stageShared } from ${url("setup/shared")};
  import { grantRelayLease } from ${leaseHelper};
  grantRelayLease(readLink());
  const display = { units: {}, imported: true };
  const state = readLink();
  const former = { ...state, setup: { ...state.setup, role: "member", primary: other } };
  assert.equal(stageShared(former, "setupUsageDisplay", display), null);
  assert.equal(former.shared.setupUsageDisplay, undefined);
  const proposal = stageShared(state, "setupUsageDisplay", display);
  assert.equal(proposal.status, "committed");
  assert.deepEqual(state.shared.setupUsageDisplay.value, display);
`));

test("a holder retry returns its original setup receipt after a later save", () => isolated(`
  import { stageShared } from ${url("setup/shared")};
  import { saveLocalSetup } from ${url("areas/setup")};
  import { grantRelayLease } from ${leaseHelper};
  grantRelayLease(readLink());
  const json = (status, value) => [{ head: { status, headers: {} } }, { body: Buffer.from(JSON.stringify(value)) }];
  const body = { id, epoch: 1, sequence: 0, operation: "A".repeat(22), kind: "setupUsageDisplay", value: { units: {}, imported: true } };
  const save = async data => {
    const answer = await collected(saveLocalSetup({ json, request: { fromHolder: true, method: "POST", body: Buffer.from(JSON.stringify(data)) } }));
    return { status: answer.status, value: JSON.parse(answer.body) };
  };
  const current = readLink();
  const first = stageShared(current, body.kind, body.value, undefined, body.operation);
  assert.equal(first.status, "committed");
  assert.equal(stageShared(current, body.kind, { ...body.value, imported: false }).status, "committed");
  writeLink(current);
  assert.deepEqual(await save(body), { status: 200, value: first });
  assert.equal((await save({ ...body, value: { ...body.value, imported: false } })).status, 409);
`));

test("a computer on its own saves setup-wide settings without the relay", () => isolated(`
  const save = async (operation, zone) => {
    const revision = (await ask("GET", "kuvo/data/edits?kind=setupTimeZone", {})).value.revision;
    return ask("POST", "kuvo/data/edits", { ref: { kind: "setupTimeZone" }, operation, baseRevision: revision, fields: { zone } });
  };
  // No relay lease was ever heard. Alone in its setup, nothing else could take over.
  const together = readLink();
  writeLink({ ...together, shared: { ...together.shared, membershipAndAccess: { sequence: 2, value: { left: [], boxes: [peer(own)] } } } });
  const alone = await save("alone-zone", "Europe/Istanbul");
  assert.equal(alone.status, 200);
  assert.equal(alone.value.status, "committed");
  // With another computer, it waits for the relay as before.
  writeLink(together);
  const waiting = await save("together-zone", "Europe/Paris");
  assert.equal(waiting.value.status, "pending");
`));

test("an older conversation on another computer is found there, not run here", () => isolated(`
  import { takeAgents } from ${url("peers/directory")};
  import { LIST_FILE } from ${url("lists/sessions")};
  import { ownedDigests, ownerOf } from ${url("ownership/resolve")};
  // The other computer has 60 conversations. It names its 40 newest, and says it has every one.
  const sessions = Object.fromEntries(Array.from({ length: 60 }, (_, ix) =>
    ["ses_older" + ix, { seq: ix + 1, facts: { kind: "project", directory: "/", created: ix, updated: ix } }]));
  fs.mkdirSync(path.dirname(LIST_FILE), { recursive: true });
  fs.writeFileSync(LIST_FILE, JSON.stringify({ epoch: "b", seq: 60, floor: 0, gone: {}, projects: {}, sessions }));
  const owns = ownedDigests();
  fs.rmSync(LIST_FILE);
  const newest = Array.from({ length: 40 }, (_, ix) => ({ session: "ses_older" + (59 - ix), name: "Newer", kind: "session" }));
  assert.equal(takeAgents(peer(from), { at: Date.now(), list: newest, owns, features: ["moves.two-computers"] }), true);
  assert.equal(ownerOf({ kind: "conversations", id: "ses_older0" })?.key, from);
  assert.equal(ownerOf({ kind: "conversations", id: "ses_nowhere" }), null);
  // A client still pointed at this computer renames it: told where it is.
  const renamed = await ask("POST", "kuvo/sessions/ses_older0/rename", { title: "Renamed" });
  assert.equal(renamed.status, 409);
  assert.equal(renamed.value.computer, from);
`));

test("a conversation this computer has is here, whatever another computer last said", () => isolated(`
  import { takeAgents } from ${url("peers/directory")};
  import { LIST_FILE } from ${url("lists/sessions")};
  // The computer it came back from named it, and hasn't said otherwise since.
  takeAgents(peer(from), { at: Date.now(), list: [{ session: "ses_back", name: "Back", kind: "session" }], features: ["moves.two-computers"] });
  fs.mkdirSync(path.dirname(LIST_FILE), { recursive: true });
  fs.writeFileSync(LIST_FILE, JSON.stringify({ epoch: "a", seq: 1, floor: 0, gone: {}, projects: {},
    sessions: { ses_back: { seq: 1, facts: { kind: "project", directory: "/", created: 1, updated: 1 } } } }));
  const { value } = await ask("GET", "kuvo/data/owner?kind=conversations&id=ses_back");
  assert.equal(value.owner.local, true);
  assert.equal(value.owner.key, own);
`));

test("authoritative absence replaces a remote record in the snapshot cache", () => isolated(`
  import { FORWARDS, forwardsSoon, takeActed } from ${url("work-queue/forward")};
  const run = path.join(process.env.KUVO_HOME, "run");
  fs.writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify({ [from]: { features: ["data.ownership"], heard: Date.now(), list: [{ session: "ses_a", name: "A", kind: "session" }] } }));
  const route = "kuvo/data/owner?kind=conversations&id=ses_a&computer=" + from;
  let status = 200;
  forwardsSoon.send = () => {
    const [file] = fs.readdirSync(FORWARDS);
    const { act } = JSON.parse(fs.readFileSync(path.join(FORWARDS, file), "utf8"));
    takeActed(peer(from), { id: act.id, status, value: status === 200 ? { name: "Before deletion" } : null });
  };
  const received = (await ask("GET", route + "&offset=msg_older", {})).value;
  assert.equal(received.data.name, "Before deletion");
  status = 404;
  const gone = (await ask("GET", route, {})).value;
  assert.equal(gone.state, "absent");
  assert.equal(gone.refresh, "live");
  assert.equal(gone.stale, false);
  assert.ok(gone.lastReceivedAt >= received.lastReceivedAt);
  status = 503;
  const offline = (await ask("GET", route + "&offset=msg_older", {})).value;
  assert.equal(offline.state, "absent");
  assert.equal(offline.refresh, "failed");
  assert.equal(offline.lastReceivedAt, gone.lastReceivedAt);
  assert.equal(offline.data, undefined);
  status = 200;
  assert.equal((await ask("GET", route, {})).value.state, "received");
  status = 503;
  assert.equal((await ask("GET", route, {})).value.state, "received");
  assert.equal((await ask("GET", route + "&offset=msg_older", {})).value.state, "neverReceived");
`));

test("owner read observations survive out-of-order replies without resurrecting a page", () => isolated(`
  import { FORWARDS, forwardsSoon, takeActed } from ${url("work-queue/forward")};
  const run = path.join(process.env.KUVO_HOME, "run");
  fs.writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify({ [from]: { features: ["data.ownership"], heard: Date.now(), list: [{ session: "ses_a", name: "A", kind: "session" }] } }));
  const route = "kuvo/data/owner?kind=conversations&id=ses_a&computer=" + from;
  const acts = new Set();
  forwardsSoon.send = () => {
    for (const file of fs.readdirSync(FORWARDS).filter(name => !name.endsWith(".answer.json"))) {
      acts.add(JSON.parse(fs.readFileSync(path.join(FORWARDS, file), "utf8")).act.id);
    }
  };
  const start = async () => {
    const seen = new Set(acts);
    const answer = ask("GET", route, {});
    let act;
    while (!(act = [...acts].find(id => !seen.has(id)))) await new Promise(resolve => setTimeout(resolve, 5));
    await new Promise(resolve => setTimeout(resolve, 5));
    return { act, answer };
  };
  const finish = async (read, status, value) => {
    takeActed(peer(from), { id: read.act, status, value });
    return (await read.answer).value;
  };
  const oldDeletion = await start();
  const newerPage = await start();
  const saved = await finish(newerPage, 200, { messages: [{ id: "newer" }] });
  assert.equal(saved.refresh, "live");
  const superseded = await finish(oldDeletion, 404, null);
  assert.deepEqual(superseded.data, saved.data);
  assert.equal(superseded.lastReceivedAt, saved.lastReceivedAt);

  const oldPage = await start();
  const newerDeletion = await start();
  const gone = await finish(newerDeletion, 404, null);
  const stalePage = await finish(oldPage, 200, { messages: [{ id: "older" }] });
  assert.equal(stalePage.state, "absent");
  assert.equal(stalePage.data, undefined);
  assert.equal(stalePage.lastReceivedAt, gone.lastReceivedAt);
`));

test("a setup draft above the owner capacity reaches a terminal receipt and stays recoverable", () => isolated(`
  import { MAX_SHARED_BYTES } from ${url("setup/shared")};
  import { readRecord } from ${url("ownership/store")};
  import { grantRelayLease } from ${leaseHelper};
  grantRelayLease(readLink());
  const link = readLink();
  link.shared.ownershipAssignments = { sequence: 1, value: "x".repeat(MAX_SHARED_BYTES) };
  writeLink(link);
  const ref = { kind: "setupTimeZone" };
  const revision = (await ask("GET", "kuvo/data/edits?kind=setupTimeZone", {})).value.revision;
  const edit = { ref, operation: "capacity-test-draft", baseRevision: revision, fields: { zone: "Europe/Istanbul" } };
  const terminal = await ask("POST", "kuvo/data/edits", edit);
  assert.equal(terminal.status, 413);
  assert.equal(terminal.value.status, "capacity");
  assert.deepEqual(await ask("POST", "kuvo/data/edits", edit), terminal);
  assert.equal(readRecord("edit-outbox", terminal.value.operation).envelope, undefined);
  const receipt = await ask("GET", "kuvo/data/edits/" + terminal.value.operation, {});
  assert.deepEqual(receipt.value.recovery, edit.fields);
`));

test("an attachment-only edit rejected by the owner stays in the outbox with its original attachment id", () => isolated(`
  const item = "wqi_" + "B".repeat(20);
  const ref = { kind: "queueItems", id: item, queue: "usr_you" };
  const initial = await ask("POST", "kuvo/data/edits", { ref, operation: "create-attachment-fixture", baseRevision: null, create: true, fields: { title: "Original" } });
  assert.equal(initial.value.status, "committed");
  const attachments = [{ id: "draft-attachment-id", file: { name: "draft.txt", uri: "data:text/plain;base64,ZHJhZnQ=" } }];
  const refused = await ask("POST", "kuvo/data/edits", { ref, operation: "refused-attachment-draft", baseRevision: "A".repeat(43), attachments });
  assert.equal(refused.value.status, "refresh");
  const outbox = (await ask("GET", "kuvo/data/edits/outbox", {})).value.entries;
  assert.ok(outbox.some(entry => entry.receipt.operation === refused.value.operation));
  assert.deepEqual((await ask("GET", "kuvo/data/edits/" + refused.value.operation, {})).value.attachments, attachments);
`));

test("a scoped forwarded write keeps its operation across deliveries and rechecks the exact pairing", () => isolated(`
  import { takeScopedAct } from ${url("work-queue/forward")};
  import { recordKey, readRecord } from ${url("ownership/store")};
  import { forgetMovedAway, noteMovedAway } from ${url("moves/records")};
  const key = keyOf(50), pairing = "original";
  const state = readLink();
  state.phones = [{ key, pairing, from: "pair" }];
  writeLink(state);
  const item = "wqi_" + "A".repeat(20);
  const created = await ask("POST", "kuvo/data/edits", {
    ref: { kind: "queueItems", id: item, queue: "usr_you" }, operation: "seed", baseRevision: null,
    create: true, fields: { title: "Original" },
  });
  assert.equal(created.value.status, "committed");
  let effects = 0;
  async function attempt(delivery, operation, title = "Edit", replayOnly = false) {
    const act = { id: "wqa_" + delivery.repeat(22), method: "PATCH", item, body: { title }, at: Date.now() };
    const grant = { setup: id, operation, submitter: { kind: "paired", key, pairing } };
    let result;
    await takeScopedAct(peer(from), { owner: own, act, grant, replayOnly }, reply => { result = reply.acted; }, async () => {
      effects++;
      return { status: 200, value: { title } };
    });
    return result;
  }

  assert.equal((await attempt("A", "accepted")).status, 200);
  assert.equal(readRecord("mutations", recordKey("scoped-forward", id, "accepted")).answer.status, 200);
  assert.equal((await attempt("B", "accepted", "Different")).status, 409);
  noteMovedAway([item], other, Date.now(), { queue: "usr_you" });
  assert.equal(await attempt("F", "wrong-owner"), undefined);
  assert.equal(effects, 1);
  forgetMovedAway([item]);
  const revoked = readLink();
  revoked.phones = [{ key, pairing: "replacement", from: "pair" }];
  revoked.removed = [{ key, pairing }];
  writeLink(revoked);
  assert.equal((await attempt("C", "accepted")).status, 200);
  const refused = await attempt("D", "never-accepted");
  assert.equal(refused.status, 409);
  assert.match(refused.value.message, /submitting device no longer has access/);
  assert.equal((await attempt("E", "never-accepted", "Edit", true)).status, 409);
  assert.equal(effects, 1);
`));

test("an interrupted native draft remains ambiguous after its durable claim", () => isolated(`
  import { runMutation } from ${url("ownership/mutation")};
  import { ownerOf } from ${url("ownership/resolve")};
  import { recordKey } from ${url("ownership/store")};
  import { noteMovedAway } from ${url("moves/records")};
  const given = { id: "msg_heldshell", text: "!test", message: {} };
  const payload = { given, submitter: { kind: "computer" } };
  await assert.rejects(runMutation(recordKey("typed-draft", id, "ses_native", given.id), payload, async () => {
    throw new Error("Interrupted before recording the native outcome");
  }));
  const response = await ask("POST", "kuvo/sessions/ses_native/draft", given);
  assert.equal(response.status, 409);
  assert.equal(response.value.ambiguous, true);
  assert.equal(response.value.restore, undefined);
  noteMovedAway(["ses_native"], from);
  assert.equal(ownerOf({ kind: "conversations", id: "ses_native" })?.key, from);
  assert.deepEqual(await ask("POST", "kuvo/sessions/ses_native/draft", given), response);
`));

test("submission receipts survive either handoff order and compact without releasing an identity", () => isolated(`
  import { sendOwnerMessages, takeMessageReceipt } from ${url("ownership/messages")};
  import { keepRecord, readRecord, records } from ${url("ownership/store")};
  import { submissionIntent, submissionKey } from ${url("ownership/submissions")};
  import { fingerprint } from ${url("ownership/merge")};
  import { forgetMovedAway, noteMovedAway } from ${url("moves/records")};
  const session = "ses_handoff";
  const state = readLink();
  const moved = () => noteMovedAway([session], other);
  const host = { welcomed: true, send() {} };
  const setupID = id;
  const submitter = { kind: "computer" };
  for (let ix = 0; ix < 205; ix++) {
    forgetMovedAway([session]);
    const submission = setupID + "." + from + ".msg_handoff" + ix;
    const operation = submissionKey(setupID, session, submission);
    const body = { id: "msg_handoff" + ix, submission, text: "Draft " + ix, message: { files: [{ uri: "data:text/plain;base64,QQ==", name: "A.txt" }] } };
    const queued = { setup: setupID, record: session, operation, owner: from, body, submitter, draft: true, scoped: true };
    const pending = { setup: setupID, record: session, submission, digest: fingerprint(submissionIntent(body, submitter, own)), at: 1, submitter, scoped: true, draft: true, queuedSubmission: queued,
      receipt: { owner: from, status: "pending", operation, message: "Waiting" } };
    const answer = { operation, owner: from, status: "committed", acceptedAt: 100 + ix, digest: pending.digest };
    keepRecord("outbox", operation, pending);
    if (ix % 2 === 0) {
      moved();
      await sendOwnerMessages(host, readLink(), Date.now());
      assert.deepEqual(readRecord("message-receipts", operation).parked.body.message.files, body.message.files);
      await takeMessageReceipt(peer(from), { ...answer, digest: "wrong" }, true, true);
      assert.equal(readRecord("message-receipts", operation).receipt.status, "interrupted");
    }
    await takeMessageReceipt(peer(from), answer, true, true);
    moved();
    await sendOwnerMessages(host, readLink(), Date.now());
    await takeMessageReceipt(peer(from), { ...answer, acceptedAt: 123456 }, true, true);
    assert.equal((await ask("GET", "kuvo/data/messages/" + operation)).value.acceptedAt, answer.acceptedAt);
    assert.equal(readRecord("message-receipts", operation).parked, undefined);
    assert.equal((await ask("POST", "kuvo/sessions/" + session + "/submit", body)).value.message, body.id);
    assert.equal((await ask("POST", "kuvo/sessions/" + session + "/submit", { ...body, text: "Changed" })).status, 409);
    if (ix === 0) {
      keepRecord("outbox", operation, pending); // Cut after archive publication, before unlink.
      await sendOwnerMessages(host, readLink(), Date.now());
      assert.equal(readRecord("outbox", operation), undefined);
    }
  }
  assert.equal(records("outbox").length, 0);
  assert.equal(records("message-receipts").length, 205);
  const alone = readLink();
  writeLink({ ...alone, shared: { ...alone.shared, membershipAndAccess: { sequence: 2, value: { left: [], boxes: [peer(own)] } } } });
  const page = await ask("GET", "kuvo/sessions/" + session + "/submissions");
  assert.equal(page.value.currentOwner, other);
  assert.equal(page.value.outcomes.length, 40, JSON.stringify(page.value));
  assert.ok(page.value.next);
  const next = await ask("GET", "kuvo/sessions/" + session + "/submissions?after=" + page.value.next);
  assert.equal(next.value.outcomes.length, 40);
  assert.ok(next.value.outcomes.every(outcome => outcome.submission > page.value.next));
`));

test("submission lookup distinguishes an owner claim, committed effect and explicit non-admission after handoff", () => isolated(`
  import { runMutation } from ${url("ownership/mutation")};
  import { submissionIntent, submissionKey } from ${url("ownership/submissions")};
  import { ownerOf, mayActivate } from ${url("ownership/resolve")};
  import { captureBackup } from ${url("backup/engine")};
  import { archiveContext, newRecoveryKey, openArchive } from ${url("backup/archive")};
  import { receiptReference } from ${url("backup/locations")};
  import { keepRecord, recordKey } from ${url("ownership/store")};
  import { noteMovedAway } from ${url("moves/records")};
  import { LIST_FILE } from ${url("lists/sessions")};
  const session = "ses_evidence";
  // The conversation is this computer's: its session list has it.
  fs.mkdirSync(path.dirname(LIST_FILE), { recursive: true });
  fs.writeFileSync(LIST_FILE, JSON.stringify({ epoch: "a", seq: 1, floor: 0, gone: {}, projects: {},
    sessions: { [session]: { seq: 1, facts: { kind: "project", directory: "/", created: 1, updated: 1 } } } }));
  let effects = 0;
  const submitter = { kind: "computer" };
  const prepare = (suffix) => {
    const body = { id: "msg_" + suffix, submission: id + "." + own + ".msg_" + suffix, text: "!example",
      message: { files: [{ uri: "data:text/plain;base64,QQ==", name: "saved.txt" }] } };
    return { body, key: submissionKey(id, session, body.submission), change: submissionIntent(body, submitter, own),
      metadata: { id: body.submission, setup: id, session, from: own, origin: own, submittedThrough: own, submitter, draft: body } };
  };
  const accepted = prepare("accepted");
  const unknown = prepare("unknown");
  const blocked = prepare("blocked");
  const route = "kuvo/sessions/" + session;
  const legacyKey = rawID => recordKey("typed-draft", id, session, rawID);
  await runMutation(accepted.key, accepted.change, async () => { effects++; return { status: 200, value: { answer: { shell: "example" } } }; },
    undefined, undefined, accepted.metadata, legacyKey(accepted.body.id));
  await assert.rejects(runMutation(unknown.key, unknown.change, async () => { effects++; throw new Error("cut"); },
    undefined, undefined, unknown.metadata));
  await runMutation(blocked.key, blocked.change, async () => { throw new Error("ran ruled-out effect"); },
    () => ({ status: 503, value: { message: "Unavailable" }, notAdmitted: true }), undefined, blocked.metadata);
  const { submission: _pin, ...oldBody } = accepted.body;
  assert.equal((await ask("POST", route + "/draft", oldBody)).status, 409);
  const legacy = prepare("legacy");
  const { submission: _otherPin, ...legacyBody } = legacy.body;
  await runMutation(legacyKey(legacy.body.id), { given: legacyBody, submitter },
    async () => ({ status: 200, value: { answer: { shell: "example" } } }));
  assert.equal((await ask("POST", route + "/submit", legacy.body)).value.ambiguous, true);
  const kept = (family, key) => {
    const name = "home/run/owned-data/" + family + "/" + key + ".json";
    return receiptReference(name, path.join(process.env.KUVO_HOME, name.slice(5)));
  };
  const evidence = kept("submissions", accepted.key);
  const fence = kept("mutations", legacyKey(accepted.body.id));
  assert.equal(evidence.kind, "draftSubmissionEvidence");
  assert.equal(ownerOf(evidence).key, own);
  assert.equal(fence.kind, "draftSubmissionReplayFences");
  assert.equal(ownerOf(fence).key, own);
  assert.equal(kept("mutations", legacyKey(legacy.body.id)), undefined, "opaque old draft claims carry no domain authority");
  const archive = point => {
    const scratch = fs.mkdtempSync(path.join(process.env.HOME, "capture-"));
    const file = path.join(scratch, "sealed");
    const recovery = newRecoveryKey();
    captureBackup(file, recovery, "fixture", point, Date.now(), [], scratch);
    return openArchive(file, recovery, archiveContext("fixture", point), scratch).map(entry => entry.name);
  };
  const evidenceFile = "home/run/owned-data/submissions/" + accepted.key + ".json";
  const fenceFile = "home/run/owned-data/mutations/" + legacyKey(accepted.body.id) + ".json";
  const before = archive("before");
  assert.ok(before.includes(evidenceFile) && before.includes(fenceFile));
  const state = readLink();
  noteMovedAway([session], from);
  assert.equal(ownerOf(evidence).key, own);
  assert.equal(mayActivate(evidence), true);
  assert.equal(mayActivate(evidence, { activating: "move" }), false);
  assert.equal(ownerOf(fence).key, from);
  assert.equal(mayActivate(fence), false);
  const after = archive("after");
  assert.ok(after.includes(evidenceFile));
  assert.equal(after.includes(fenceFile), false);
  const opaque = prepare("wrongslot");
  const slot = recordKey("opaque-slot");
  keepRecord("submissions", slot, { submission: opaque.metadata, digest: "opaque" });
  assert.equal(kept("submissions", slot), undefined);
  assert.ok(archive("opaque").includes("home/run/owned-data/submissions/" + slot + ".json"));
  const page = await ask("GET", route + "/submissions");
  assert.equal(page.value.outcomes.some(outcome => outcome.submission === opaque.body.submission), false);
  for (const [fixture, status] of [[accepted, "committed"], [unknown, "unknown"], [blocked, "notAdmitted"]]) {
    const { value } = await ask("GET", route + "/submissions/" + fixture.body.submission);
    assert.equal(value.currentOwner, from);
    const [outcome] = value.outcomes;
    assert.equal(outcome.status, status);
    assert.equal(outcome.originalOwner, own);
    if (status === "committed") {
      assert.equal(outcome.effect.answer.shell, "example");
      assert.ok(outcome.acceptedAt > 0);
      assert.equal(outcome.draft, undefined);
    } else {
      assert.deepEqual(outcome.draft.message.files, fixture.body.message.files);
    }
  }
  assert.equal((await ask("POST", route + "/submit", unknown.body)).value.ambiguous, true);
  assert.equal((await ask("POST", route + "/submit", { ...accepted.body, text: "Changed" })).status, 409);
  const device = keyOf(46);
  const paired = { kind: "paired", key: device, pairing: "old" };
  const historical = prepare("repaired");
  historical.metadata.submitter = paired;
  const intent = submissionIntent(historical.body, paired, own);
  await runMutation(historical.key, intent,
    async () => ({ status: 200, value: { answer: { shell: "example" } } }),
    undefined, undefined, historical.metadata);
  writeLink({ ...readLink(), phones: [{ key: device, pairing: "new" }] });
  const asDevice = async (method, query, pairing, body) => {
    const reply = await collected(handle({ method, path: route + query, headers: {}, body: Buffer.from(JSON.stringify(body ?? null)) },
      { key: Buffer.from(device, "base64url"), pairing }));
    return { status: reply.status, value: JSON.parse(reply.body) };
  };
  const lookup = "/submissions/" + historical.body.submission + "?digest=" + (await import(${url("ownership/merge")})).fingerprint(intent);
  assert.equal((await asDevice("GET", lookup, "old")).status, 403);
  assert.equal((await asDevice("GET", lookup, "new")).value.outcomes[0].status, "committed");
  const recoveredPage = (await asDevice("GET", "/submissions", "new")).value;
  assert.deepEqual(recoveredPage.outcomes.map(item => [item.submission, item.submitter.pairing]), [[historical.body.submission, "old"]]);
  assert.equal((await asDevice("POST", "/submit", "new", { ...historical.body, text: "!changed" })).status, 409);
  const replacementState = readLink();
  replacementState.key = Buffer.alloc(32, 44).toString("base64url");
  replacementState.setup.role = "member";
  replacementState.shared.membershipAndAccess.value.left = [own];
  replacementState.shared.membershipAndAccess.value.boxes = [from, other].map(peer);
  replacementState.shared.ownershipRecovery = { sequence: 1, value: { ["replacement:" + own]: { computer: other, source: "approved" } } };
  assert.equal(ownerOf(evidence, { link: replacementState }).key, other);
  assert.equal(ownerOf(fence, { link: replacementState }).key, from);
  const changedSetup = "set_" + "B".repeat(22);
  writeLink({ ...state, setup: { ...state.setup, id: changedSetup }, sharedSetup: changedSetup });
  assert.equal((await ask("POST", route + "/submit", unknown.body)).value.ambiguous, true);
  assert.equal(effects, 2);
`));

test("receipt page and move preparation stay bounded with unrelated history", () => {
  const counts = JSON.parse(isolated(`
    import { spawnSync } from "node:child_process";
    import { packForwardEvidence, forwardDecisionKey, draftKey } from ${url("ownership/forward-evidence")};
    import { refreshReceiptIndex, RECEIPT_INDEX_BATCH } from ${url("ownership/receipt-index")};
    import { submissionKey } from ${url("ownership/submissions")};
    import { keepRecord, recordKey } from ${url("ownership/store")};
    const session = "ses_index";
    const author = { kind: "computer" };
    const directory = path.join(process.env.KUVO_HOME, "run", "owned-data");
    const relevant = new Set();
    const seedRecord = (family, key, value) => {
      const folder = path.join(directory, family);
      fs.mkdirSync(folder, { recursive: true });
      fs.writeFileSync(path.join(folder, key + ".json"), JSON.stringify(value));
    };
    const save = (session, ix) => {
      const submission = id + "." + own + ".msg_" + String(ix).padStart(4, "0");
      const key = submissionKey(id, session, submission);
      if (session === "ses_index") relevant.add(key).add(draftKey(id, session, submission.split(".").at(-1)));
      seedRecord("submissions", key, { digest: "d".repeat(43), at: ix + 1, answer: { status: 200, value: { message: submission } },
        submission: { setup: id, id: submission, session, origin: own, submittedThrough: own, submitter: author } });
      seedRecord("message-receipts", key, { scoped: true, setup: id, submission, record: session, submitter: author,
        digest: "d".repeat(43), receipt: { status: "committed", owner: own, operation: key, acceptedAt: ix + 1 } });
    };
    for (let ix = 0; ix < 480; ix++) {
      save("ses_unrelated" + ix, ix);
      seedRecord("mutations", recordKey("unrelated", String(ix)), { digest: "r".repeat(43) });
    }
    for (let ix = 0; ix < 85; ix++) save(session, ix);
    const operation = "indexed-move";
    relevant.add(forwardDecisionKey(id, operation));
    seedRecord("mutations", forwardDecisionKey(id, operation), { digest: "e".repeat(43), forward: { setup: id, operation,
      kind: "forwardedSessionDecisions", owner: own, submitter: author, area: "ownership", record: session, ref: { kind: "sessionQueue", id: session } } });
    const visits = {};
    const original = fs.readFileSync;
    fs.readFileSync = function(file, ...options) {
      const family = typeof file === "string" && file.startsWith(directory) ? path.dirname(file).split(path.sep).at(-1) : null;
      if (family) {
        visits[family] = (visits[family] || 0) + 1;
        if (!relevant.has(path.basename(file, ".json"))) visits.unrelated = (visits.unrelated || 0) + 1;
      }
      return original.call(this, file, ...options);
    };
    const take = () => {
      const result = { ...visits };
      for (const family of Object.keys(visits)) delete visits[family];
      return result;
    };
    const route = "kuvo/sessions/" + session + "/submissions";
    const fleet = readLink();
    writeLink({ ...fleet, shared: { ...fleet.shared, membershipAndAccess: { sequence: 2, value: { left: [], boxes: [peer(own)] } } } });
    const cold = await ask("GET", route);
    assert.equal(cold.value.complete, false);
    assert.equal(cold.value.next, undefined);
    const coldPage = take();
    assert.ok((coldPage.unrelated ?? 0) <= RECEIPT_INDEX_BATCH);
    const scope = { sessions: [session], items: new Set() };
    assert.throws(() => packForwardEvidence(id, scope), /still being indexed/);
    const coldMove = take();
    assert.ok((coldMove.unrelated ?? 0) <= RECEIPT_INDEX_BATCH);
    await refreshReceiptIndex();
    const interruptedBuild = take();
    assert.ok((interruptedBuild.unrelated ?? 0) <= RECEIPT_INDEX_BATCH);
    // Reopen the same store in a new process and continue its interrupted scan.
    const indexModule = JSON.stringify(${url("ownership/receipt-index")});
    const restarted = spawnSync(process.execPath, ["--config=/dev/null", "--no-env-file", "-e",
      "import { refreshReceiptIndex } from " + indexModule + "; for (let i=0; i<20; i++) await refreshReceiptIndex();"],
      { cwd: process.cwd(), env: process.env, encoding: "utf8", timeout: 15_000 });
    assert.equal(restarted.status, 0, restarted.stderr);
    const first = await ask("GET", route);
    assert.equal(first.value.complete, true);
    assert.equal(first.value.outcomes.length, 40);
    const initialPage = take();
    const next = await ask("GET", route + "?after=" + first.value.next);
    assert.equal(next.value.outcomes.length, 40);
    const secondPage = take();
    const third = await ask("GET", route + "?after=" + next.value.next);
    assert.equal(third.value.outcomes.length, 5);
    assert.equal(third.value.next, undefined);
    assert.equal(new Set([...first.value.outcomes, ...next.value.outcomes, ...third.value.outcomes].map(row => row.submission)).size, 85);
    take();
    const moved = packForwardEvidence(id, scope);
    assert.deepEqual(Object.keys(moved), [forwardDecisionKey(id, operation)]);
    const move = take();
    assert.deepEqual(initialPage, { submissions: 41, "message-receipts": 41 });
    assert.deepEqual(secondPage, initialPage);
    assert.deepEqual(move, { mutations: 86 });
    // A raw install and a process cut after file publication invalidate the index. Exact
    // reads still come from the canonical file and reopening reclaims the dead owner's lock.
    const imported = id + "." + own + ".msg_0085";
    const importedKey = submissionKey(id, session, imported);
    const importedFile = path.join(directory, "submissions", importedKey + ".json");
    const claim = { digest: "d".repeat(43), at: 86, answer: { status: 200, value: { message: imported } },
      submission: { setup: id, id: imported, session, origin: own, submittedThrough: own, submitter: author } };
    const cut = spawnSync(process.execPath, ["--config=/dev/null", "--no-env-file", "-e",
      "import fs from 'node:fs'; import { changeReceiptFile } from " + indexModule +
      "; const [file, key, json] = process.argv.slice(-3); changeReceiptFile('submissions', key, JSON.parse(json), () => {" +
      "fs.writeFileSync(file + '.new', json); fs.renameSync(file + '.new', file); process.exit(77); });",
      importedFile, importedKey, JSON.stringify(claim)],
      { cwd: process.cwd(), env: process.env, encoding: "utf8", timeout: 10_000 });
    assert.equal(cut.status, 77, cut.stderr);
    const rebuilding = await ask("GET", route);
    assert.equal(rebuilding.value.complete, false);
    assert.equal(rebuilding.value.next, undefined);
    const exact = await ask("GET", route + "/" + imported + "?digest=" + "d".repeat(43));
    assert.equal(exact.value.outcomes[0].acceptedAt, 86);
    for (let i = 0; i < 20; i++) await refreshReceiptIndex();
    const afterRestore = await ask("GET", route + "?after=" + next.value.next);
    assert.equal(afterRestore.value.outcomes.length, 6);
    keepRecord("submissions", submissionKey(id, session, imported), {
      digest: "d".repeat(43), at: 86, answer: { status: 200, value: { message: imported } },
      submission: { setup: id, id: imported, session, origin: own, submittedThrough: own, submitter: author },
    });
    assert.equal((await ask("GET", route)).value.complete, true);
    writeLink(fleet);
    fs.writeFileSync(path.join(process.env.KUVO_HOME, "run", "peer-agents.json"), JSON.stringify({
      [from]: { features: ["typed.submission-recovery", "data.ownership"], list: [], heard: Date.now() } }));
    const started = performance.now();
    const late = await ask("GET", route);
    assert.equal(late.value.complete, false);
    assert.equal(late.value.next, undefined);
    const unavailableMs = Math.round(performance.now() - started);
    assert.ok(unavailableMs < 5_000, unavailableMs);
    console.log(JSON.stringify({ coldPage, coldMove, interruptedBuild, initialPage, secondPage, move, unavailableMs, unavailableRows: late.value.outcomes.length, nextWhileIncomplete: Boolean(late.value.next) }));
  `));
  console.log("Receipt storage measurements", JSON.stringify(counts));
});

test("mixed peer pages retry the same cursor and hold it across an issuer change", () => isolated(`
  import { FORWARDS, forwardsSoon, takeActed } from ${url("work-queue/forward")};
  const device = keyOf(46), retired = keyOf(45);
  const pairing = "current";
  const viewer = { kind: "paired", key: device, pairing };
  const historical = { ...viewer, pairing: "earlier" };
  const session = "ses_mergepages";
  const route = "kuvo/sessions/" + session + "/submissions";
  const state = readLink();
  const accepted = { ...state.shared.membershipAndAccess.value, left: [retired] };
  const replace = computer => writeLink({ ...state, phones: [{ key: device, name: "Device", pairing }],
    shared: { membershipAndAccess: { sequence: 2, value: accepted },
      ownershipAssignments: { sequence: computer === from ? 1 : 2, value: { ["replacement:" + retired]: { computer, source: "approved" } } } } });
  replace(from);
  fs.writeFileSync(path.join(process.env.KUVO_HOME, "run", "peer-agents.json"), JSON.stringify(Object.fromEntries([from, other].map(key =>
    [key, { features: ["typed.submission-recovery", "data.ownership"], list: [], heard: Date.now() }]))));
  forwardsSoon.send = () => { throw new Error("read triggered a global send scan"); };
  const list = (issuer, evidence, n) => {
    const submission = id + "." + issuer + ".msg_" + String(n).padStart(3, "0");
    return { submission, originalOwner: issuer, evidence, status: "committed", digest: "x".repeat(43),
      submitter: historical, acceptedAt: n + 1, effect: { message: submission } };
  };
  const collection = Array.from({ length: 95 }, (_, n) => list(own, "listing", n));
  let mode = "silent";
  const replied = new Set();
  const watches = setInterval(() => {
    if (!fs.existsSync(FORWARDS)) return;
    for (const name of fs.readdirSync(FORWARDS).filter(name => /^wqa_[A-Za-z0-9_-]+\\.json$/.test(name))) {
      if (replied.has(name)) continue;
      const file = path.join(FORWARDS, name);
      if (!fs.existsSync(file)) continue;
      const sent = JSON.parse(fs.readFileSync(file, "utf8"));
      if (sent.act?.item !== "draft-submissions" || (mode === "silent" && sent.to === other)) continue;
      replied.add(name);
      assert.equal(sent.act.method, "GET");
      assert.equal(sent.act.body.viewer.pairing, pairing);
      const current = sent.to === from;
      const after = sent.act.body.after || "";
      const ready = current ? collection.filter((_, n) => n % 2 === 0 || n === 39 || n === 41)
        : collection.filter((_, n) => n % 2 === 1);
      const outcomes = mode === "swap" ? current ? [list(retired, "owner", 500)] : []
        : ready.filter(row => row.submission > after).slice(0, current ? 41 : 40);
      if (mode === "swap") replace(other);
      takeActed(peer(sent.to), { id: sent.act.id, status: 200, value: { outcomes, ...(current ? { complete: true } : {}) } });
    }
  }, 20);
  const query = async (after = "") => {
    const answer = await collected(handle({ method: "GET", path: route + (after ? "?after=" + after : ""), headers: {} },
      { key: Buffer.from(device, "base64url"), pairing }));
    assert.equal(answer.status, 200);
    return JSON.parse(answer.body);
  };
  try {
    const incomplete = await query();
    assert.equal(incomplete.complete, false);
    assert.equal(incomplete.next, undefined);
    assert.ok(incomplete.outcomes.length);
    mode = "ready";
    const first = await query();
    const second = await query(first.next);
    const third = await query(second.next);
    assert.equal(first.complete, true);
    assert.equal(second.complete, true);
    assert.equal(third.complete, true);
    assert.equal(third.next, undefined);
    assert.deepEqual([...first.outcomes, ...second.outcomes, ...third.outcomes].map(row => row.submission), collection.map(row => row.submission));
    mode = "swap";
    const changed = await query();
    assert.equal(changed.complete, false);
    assert.equal(changed.next, undefined);
    assert.deepEqual(changed.outcomes, []);
  } finally { clearInterval(watches); }
`));
