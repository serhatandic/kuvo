import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

const url = name => JSON.stringify(new URL("../src/link/" + name + ".ts", import.meta.url).href);
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
  `], { encoding: "utf8", timeout: 30_000, env: { ...process.env, HOME: home, KUVO_HOME: kuvo } });
  assert.equal(ran.status, 0, ran.stderr || String(ran.error ?? ran.signal));
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

test("a holder retry returns its original setup receipt after a later save", () => isolated(`
  import { stageShared } from ${url("setup/shared")};
  import { saveLocalSetup } from ${url("areas/setup")};
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

test("authoritative absence replaces a remote record in the snapshot cache", () => isolated(`
  import { FORWARDS, forwardsSoon, takeActed } from ${url("work-queue/forward")};
  const run = path.join(process.env.KUVO_HOME, "run");
  fs.writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify({ [from]: { features: ["data.ownership"], heard: Date.now(), list: [] } }));
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
  fs.writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify({ [from]: { features: ["data.ownership"], heard: Date.now(), list: [] } }));
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
  import { writeLink, readLink } from ${url("state")};
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
  const key = keyOf(50), pairing = "original";
  const state = readLink();
  state.phones = [{ key, pairing, from: "pair" }];
  writeLink(state);
  let effects = 0;
  async function attempt(delivery, operation, title = "Edit", replayOnly = false) {
    const act = { id: "wqa_" + delivery.repeat(22), method: "PATCH", item: "wqi_" + "A".repeat(20), body: { title }, at: Date.now() };
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
  const given = { id: "msg_heldshell", text: "!test", message: {} };
  const payload = { given, submitter: { kind: "computer" } };
  await assert.rejects(runMutation(recordKey("typed-draft", id, "ses_native", given.id), payload, async () => {
    throw new Error("Interrupted before recording the native outcome");
  }));
  const response = await ask("POST", "kuvo/sessions/ses_native/draft", given);
  assert.equal(response.status, 409);
  assert.equal(response.value.ambiguous, true);
  assert.equal(response.value.restore, undefined);
  const state = readLink();
  const ownershipAssignments = { sequence: 1, value: { "session computer:ses_native": { computer: from } } };
  writeLink({ ...state, shared: { ...state.shared, ownershipAssignments } });
  assert.equal(ownerOf({ kind: "conversations", id: "ses_native" })?.key, from);
  assert.deepEqual(await ask("POST", "kuvo/sessions/ses_native/draft", given), response);
`));

test("submission receipts survive either handoff order and compact without releasing an identity", () => isolated(`
  import { sendOwnerMessages, takeMessageReceipt } from ${url("ownership/messages")};
  import { keepRecord, readRecord, records } from ${url("ownership/store")};
  import { submissionIntent, submissionKey } from ${url("ownership/submissions")};
  import { fingerprint } from ${url("ownership/merge")};
  const session = "ses_handoff";
  const state = readLink();
  const moved = () => writeLink({ ...state, shared: { ...state.shared, ownershipAssignments: { sequence: 1,
    value: { ["session computer:" + session]: { computer: other, from } } } } });
  const host = { welcomed: true, send() {} };
  const setupID = id;
  const submitter = { kind: "computer" };
  for (let ix = 0; ix < 205; ix++) {
    writeLink(state);
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
      sendOwnerMessages(host, readLink(), Date.now());
      assert.deepEqual(readRecord("message-receipts", operation).parked.body.message.files, body.message.files);
      takeMessageReceipt(peer(from), { ...answer, digest: "wrong" }, true, true);
      assert.equal(readRecord("message-receipts", operation).receipt.status, "interrupted");
    }
    takeMessageReceipt(peer(from), answer, true, true);
    moved();
    sendOwnerMessages(host, readLink(), Date.now());
    takeMessageReceipt(peer(from), { ...answer, acceptedAt: 123456 }, true, true);
    assert.equal((await ask("GET", "kuvo/data/messages/" + operation)).value.acceptedAt, answer.acceptedAt);
    assert.equal(readRecord("message-receipts", operation).parked, undefined);
    assert.equal((await ask("POST", "kuvo/sessions/" + session + "/submit", body)).value.message, body.id);
    assert.equal((await ask("POST", "kuvo/sessions/" + session + "/submit", { ...body, text: "Changed" })).status, 409);
    if (ix === 0) {
      keepRecord("outbox", operation, pending); // Cut after archive publication, before unlink.
      sendOwnerMessages(host, readLink(), Date.now());
      assert.equal(readRecord("outbox", operation), undefined);
    }
  }
  assert.equal(records("outbox").length, 0);
  assert.equal(records("message-receipts").length, 205);
  const page = await ask("GET", "kuvo/sessions/" + session + "/submissions");
  assert.equal(page.value.currentOwner, other);
  assert.equal(page.value.outcomes.length, 40);
  assert.ok(page.value.next);
  const next = await ask("GET", "kuvo/sessions/" + session + "/submissions?after=" + page.value.next);
  assert.equal(next.value.outcomes.length, 40);
  assert.ok(next.value.outcomes.every(outcome => outcome.submission > page.value.next));
`));

test("submission lookup distinguishes an owner claim, committed effect and explicit non-admission after handoff", () => isolated(`
  import { runMutation } from ${url("ownership/mutation")};
  import { submissionIntent, submissionKey } from ${url("ownership/submissions")};
  import { recordKey } from ${url("ownership/store")};
  const session = "ses_evidence";
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
  const state = readLink();
  const ownershipAssignments = { sequence: 1, value: { ["session computer:" + session]: { computer: from, from: own } } };
  writeLink({ ...state, shared: { ...state.shared, ownershipAssignments } });
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
  const changedSetup = "set_" + "B".repeat(22);
  writeLink({ ...state, setup: { ...state.setup, id: changedSetup }, sharedSetup: changedSetup });
  assert.equal((await ask("POST", route + "/submit", unknown.body)).value.ambiguous, true);
  assert.equal(effects, 2);
`));
