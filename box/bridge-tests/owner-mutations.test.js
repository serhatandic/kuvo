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
    const ask = async (method, route, body, headers = {}) => {
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
  assert.equal(gone.state, "neverReceived");
  assert.equal(gone.stale, false);
  assert.ok(gone.lastReceivedAt >= received.lastReceivedAt);
  status = 503;
  const offline = (await ask("GET", route + "&offset=msg_older", {})).value;
  assert.equal(offline.state, "neverReceived");
  assert.equal(offline.lastReceivedAt, gone.lastReceivedAt);
  assert.equal(offline.data, undefined);
  status = 200;
  assert.equal((await ask("GET", route, {})).value.state, "received");
  status = 503;
  assert.equal((await ask("GET", route, {})).value.state, "received");
  assert.equal((await ask("GET", route + "&offset=msg_older", {})).value.state, "neverReceived");
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
`));
