// Real Rust residents and their HTTP routes, with the controllable loopback relay.
// The initial roster fixture represents the accepted state written by enrollment.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { createHmac, createPublicKey, verify } from "node:crypto";
import http from "node:http";
import {
  boxBinary, isolatedEnv, localRelay, readJSON, request, scratch, startBox, until, write,
} from "../../../../protocol/conformance/adapter.mjs";
import { relayLeaseStandIn } from "../../../bridge-tests/relay-lease-stand-in.js";

const binary = boxBinary();
const app = { "x-kuvo-from": "app" };
const linkPath = box => path.join(box.home, "run", "link.json");
const entry = (box, kind) => readJSON(linkPath(box)).shared?.[kind];
const get = (box, route = "kuvo/setup") => box.ask("GET", route);
const post = (box, route, body) => box.ask("POST", route, body);

async function bench(t, { handover = false } = {}) {
  const root = fs.realpathSync(scratch());
  const leases = relayLeaseStandIn({ transferStartsNow: handover });
  const relay = await localRelay({ request: leases.request });
  const children = [];
  t.after(async () => {
    for (const child of children.reverse()) await child.stop();
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  });
  function computer(name) {
    const folder = path.join(root, name);
    const home = path.join(folder, "box");
    const env = isolatedEnv(folder);
    write(path.join(home, "run", "link.json"), JSON.stringify({ name, phones: [] }));
    return {
      home,
      async start({ expectFailure = false } = {}) {
        const proc = startBox({ binary, home, env, relay });
        children.push(proc);
        this.stop = proc.stop;
        this.log = proc.log;
        if (expectFailure) {
          const result = await proc.exited;
          assert.equal(result.code, 1, proc.log());
          return proc.log();
        }
        const record = await proc.ready();
        this.ask = (method, route, body, headers = app) => request(record, method, route, body, headers);
        assert.equal((await get(this)).status, 200);
      },
    };
  }
  return { computer, leases, relay };
}

async function paired(t, options) {
  const { computer, leases, relay } = await bench(t, options);
  const a = computer("Studio");
  const b = computer("Lab");
  await a.start();
  const first = readJSON(linkPath(a));
  await until(() => leases.leaseFor(first.setup.id), "Studio should claim the initial lease");
  const id = first.setup.id;
  const aKey = first.setup.primary;
  await b.start();
  const joined = readJSON(linkPath(b));
  const bKey = joined.setup.primary;
  await a.stop();
  await b.stop();
  const roster = first.shared.membershipAndAccess;
  roster.value.boxes.push(joined.shared.membershipAndAccess.value.boxes[0]);
  roster.sequence += 1;
  const second = {
    ...joined, setup: { ...first.setup, role: "member" }, sharedSetup: id, leadership: first.leadership,
    shared: first.shared, mac: aKey, macs: [aKey], macSigns: { [aKey]: first.setup.sign },
  };
  const checkpoints = "run/setup-checkpoints";
  for (const file of fs.readdirSync(path.join(a.home, checkpoints))) {
    write(path.join(b.home, checkpoints, file), fs.readFileSync(path.join(a.home, checkpoints, file)));
  }
  write(linkPath(a), JSON.stringify(first));
  write(linkPath(b), JSON.stringify(second));
  write(path.join(b.home, "setup-takeover.json"), JSON.stringify({ alwaysOn: true }));
  await a.start();
  await b.start();
  await until(() => leases.leaseFor(id)?.roster?.[bKey] === joined.setup.sign, "the relay should learn the accepted roster");
  return { a, b, aKey, bKey, id, leases, relay };
}

const preference = "setupUsageDisplay";
const preferenceRoute = `kuvo/data/edits?kind=${preference}`;
async function baseline(box) {
  const answer = await get(box, preferenceRoute);
  assert.equal(answer.status, 200, answer.text);
  return answer.json.revision;
}

async function edit(box, operation, baseRevision, fields) {
  const answer = await post(box, "kuvo/data/edits", { ref: { kind: preference }, operation, baseRevision, fields });
  assert.ok([200, 202].includes(answer.status), answer.text);
  return answer.json.operation;
}

async function decision(box, operation) {
  return until(async () => {
    const reply = await get(box, `kuvo/data/edits/${operation}`);
    assert.equal(reply.status, 200, reply.text);
    return reply.json.receipt.status === "pending" ? undefined : reply.json.receipt;
  }, "accepted preference receipt");
}

function proof(envelope, origin) {
  const bytes = Buffer.from(envelope.d, "base64url");
  const publicKey = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: origin }, format: "jwk" });
  assert.equal(verify(null, Buffer.concat([Buffer.from("kuvo-link doc v1\0"), bytes]), publicKey, Buffer.from(envelope.s, "base64url")), true);
  return JSON.parse(bytes.toString());
}

test("a sole computer starts with a signed epoch and saves after its relay goes away", { timeout: 25000 }, async t => {
  const { computer, leases } = await bench(t);
  const box = computer("Studio");
  await box.start();
  const initial = readJSON(linkPath(box));
  assert.equal(initial.formatVersion, 1);
  assert.equal(initial.setup.role, "primary");
  assert.equal(initial.setup.epoch, 1);
  const certificate = proof(initial.leadership["1"], initial.setup.sign);
  assert.equal(certificate.kind, "leadership");
  assert.equal(certificate.body.relayTerm, 1);
  const manifest = proof(readJSON(path.join(box.home, "run", "setup-checkpoints", certificate.body.checkpoint + ".json")), initial.setup.sign);
  assert.deepEqual(manifest.body.shared.membershipAndAccess.value.boxes.map(box => box.key), [initial.setup.primary]);
  leases.disconnect(initial.setup.primary);
  const made = await post(box, "kuvo/personalities", { name: "Night  Owl", description: "  Quiet.  " });
  assert.equal(made.status, 200, made.text);
  assert.equal(made.json.name, "Night Owl");
  assert.equal(made.json.description, "Quiet.");
  assert.equal(made.json.status, "committed");
  assert.equal(proof(made.json.proof, initial.setup.sign).body.operation, made.json.operation);
  await box.stop();
  await box.start();
  assert.equal((await get(box, "kuvo/personalities")).json.personalities[0].id, made.json.id);
});

test("a member's independent edits and a conflicting choice survive takeover by that member", { timeout: 40000 }, async t => {
  const { a, b, aKey, bKey, id, leases } = await paired(t);
  const aView = (await get(a)).json;
  const bView = (await get(b)).json;
  assert.equal(aView.setup, bView.setup);
  assert.deepEqual(aView.computers, bView.computers);
  assert.equal(aView.primary.key, aKey);
  assert.equal(bView.you.role, "member");

  const before = await baseline(b);
  assert.equal((await decision(b, await edit(b, "usage-card", before, { totalSpend: { show: true } }))).status, "committed");
  assert.equal((await decision(b, await edit(b, "usage-model", before, { codexFallback: "gpt-5.4" }))).status, "committed");
  const clash = await decision(b, await edit(b, "usage-other-model", before, { codexFallback: "gpt-5.3-codex" }));
  assert.equal(clash.status, "choice");
  assert.deepEqual((await get(a, preferenceRoute)).json.fields, { totalSpend: { show: true }, codexFallback: "gpt-5.4" });
  await until(() => entry(b, preference)?.offlineEdits.choices.length === 1, "the member should receive the accepted choice");
  assert.deepEqual(entry(b, preference), entry(a, preference));

  await a.stop();
  leases.advance(125000);
  const next = await until(async () => {
    const answer = await get(b);
    return answer.json?.you?.role === "primary" && answer.json;
  }, () => `Lab should take over its setup, ${b.log()}`, 12000);
  assert.equal(next.setup, id);
  assert.equal(next.epoch, 2);
  assert.equal(next.primary.key, bKey);
  assert.equal(leases.leaseFor(id).term, 2);
  const retained = (await get(b, preferenceRoute)).json;
  assert.deepEqual(retained.fields, { totalSpend: { show: true }, codexFallback: "gpt-5.4" });
  assert.equal(retained.choices[0].id, clash.choices[0]);
  const saved = readJSON(linkPath(b));
  const cert = proof(saved.leadership["2"], saved.setup.sign);
  const checkpoint = readJSON(path.join(b.home, "run", "setup-checkpoints", cert.body.checkpoint + ".json"));
  const page = proof(checkpoint, saved.setup.sign).body.archives[0];
  const outcomes = proof(readJSON(path.join(b.home, "run", "setup-checkpoints", page + ".json")), saved.setup.sign);
  assert.equal(outcomes.body.outcomes[clash.operation].offlineEdit.decision.status, "choice");

  await a.start();
  const former = await until(async () => {
    const result = (await get(a)).json;
    return result?.epoch === 2 && result;
  }, () => `the former main should follow the new certificate, ${a.log()}`, 12000);
  assert.equal(former.you.role, "member");
  assert.equal(former.primary.key, bKey);
  assert.deepEqual((await get(a, preferenceRoute)).json.choices, retained.choices);
});

test("handover checks the successor before transferring the lease", { timeout: 30000 }, async t => {
  const { b, bKey, id, leases } = await paired(t, { handover: true });
  const answer = await post(b, "kuvo/setup/handover", { primary: bKey, epoch: 1 });
  assert.equal(answer.status, 202, answer.text);
  assert.deepEqual(answer.json, { primary: bKey, epoch: 2 });
  const promoted = await until(async () => {
    const result = (await get(b)).json;
    return result?.you?.role === "primary" && result;
  }, () => `Lab should use the transferred lease, ${b.log()}`);
  assert.equal(promoted.epoch, 2);
  assert.equal(leases.leaseFor(id).holder, bKey);
});

test("clock changes are persisted and stale settings revisions are refused", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  await box.start();
  const route = "kuvo/settings/clock";
  const read = (await get(box, route)).json.groups[0].rows[0];
  assert.equal(read.value, "own-clock");
  const given = { row: "time-zone", value: "Europe/Istanbul", was: read.revision };
  const answer = await box.ask("PATCH", route, given);
  assert.equal(answer.status, 200, answer.text);
  assert.equal(answer.json.groups[0].rows[0].value, "Europe/Istanbul");
  assert.equal(answer.json.status, "committed");
  assert.equal(fs.readFileSync(path.join(box.home, "wakes", "timezone"), "utf8"), "Europe/Istanbul");
  const stale = await box.ask("PATCH", route, { ...given, value: "Asia/Tokyo" });
  assert.equal(stale.status, 409, stale.text);
  assert.equal(entry(box, "setupTimeZone").value.zone, "Europe/Istanbul");
});

test("a removed personality cannot be brought back by a more recent copy", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  await box.start();
  const route = "kuvo/personalities";
  const made = await post(box, route, { name: "Scout", description: "Finds details." });
  assert.equal(made.status, 200, made.text);
  const { id, name, description, symbol, updated } = made.json;
  const removed = await box.ask("DELETE", `${route}/${id}`);
  assert.equal(removed.status, 200, removed.text);
  const take = await post(box, `${route}/take`, { document: { personalities: [{ id, name, description, symbol, updated: updated + 100000 }] } });
  assert.equal(take.status, 200, take.text);
  assert.deepEqual(take.json.changed, []);
  const kept = (await get(box, `${route}/document`)).json;
  assert.equal(kept.kuvo, 1);
  assert.deepEqual(kept.personalities, []);
  assert.equal(kept.removed[0].id, id);
});

test("a main that loses its grant refuses writes and follows the winning member", { timeout: 35000 }, async t => {
  const { a, b, aKey, bKey, id, leases } = await paired(t);
  const reconnect = leases.disconnect(aKey);
  write(path.join(a.home, "run", "test-setup-clock.json"), JSON.stringify({ offset: 125000 }));
  const blocked = await post(a, "kuvo/personalities", { name: "Fenced", description: "Changes once there is a lease." });
  assert.equal(blocked.status, 503, blocked.text);
  assert.equal(entry(a, "setupPersonalities"), undefined);
  leases.advance(125000);
  await until(() => leases.leaseFor(id)?.holder === bKey, "Lab should claim the expired grant");
  // The old main has no authority until it can confirm the current term.
  const again = await post(a, "kuvo/personalities", { name: "Fenced", description: "Still no grant." });
  assert.equal(again.status, 503, again.text);
  reconnect();
  fs.unlinkSync(path.join(a.home, "run", "test-setup-clock.json"));
  const followed = await until(async () => {
    const result = (await get(a)).json;
    return result?.you?.role === "member" && result;
  }, "the old main should follow the winning member", 15000);
  assert.equal(followed.primary.key, bKey);
});

test("a stopped member gets saves on restart and a repeated edit returns its first decision", { timeout: 30000 }, async t => {
  const { a, b } = await paired(t);
  await b.stop();
  const baselineAtMain = await baseline(a);
  const submit = { ref: { kind: preference }, operation: "restart-copy", baseRevision: baselineAtMain, fields: { codexFallback: "gpt-5.4" } };
  const saved = await post(a, "kuvo/data/edits", submit);
  assert.equal(saved.status, 200, saved.text);
  await b.start();
  await until(() => entry(b, preference)?.value.units.codexFallback.value === "gpt-5.4", "the restarted member should catch up");
  const retry = await post(a, "kuvo/data/edits", submit);
  assert.equal(retry.status, 200, retry.text);
  assert.deepEqual(retry.json, saved.json);
  const changed = await post(a, "kuvo/data/edits", { ...submit, fields: { codexFallback: "different" } });
  assert.equal(changed.status, 409, changed.text);
  assert.equal(entry(a, preference).sequence, 1);
});

test("a corrupt signed checkpoint cannot reopen the setup's authority", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  await box.start();
  const link = readJSON(linkPath(box));
  const cert = proof(link.leadership["1"], link.setup.sign);
  const checkpoint = path.join(box.home, "run", "setup-checkpoints", cert.body.checkpoint + ".json");
  await box.stop();
  const original = readJSON(checkpoint);
  const signature = Buffer.from(original.s, "base64url");
  signature[0] ^= 1;
  const corrupt = JSON.stringify({ ...original, s: signature.toString("base64url") });
  write(checkpoint, corrupt);
  const log = await box.start({ expectFailure: true });
  assert.match(log, /signed setup document: invalid document signature/);
  assert.equal(fs.readFileSync(checkpoint, "utf8"), corrupt);
  assert.deepEqual(readJSON(linkPath(box)).leadership, link.leadership);
});

test("an unknown link format leaves the setup files unchanged", { timeout: 12000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  const future = JSON.stringify({ formatVersion: 2, untouched: ["opaque"] });
  write(linkPath(box), future);
  const log = await box.start({ expectFailure: true });
  assert.match(log, /unsupported link state format/);
  assert.equal(fs.readFileSync(linkPath(box), "utf8"), future);
  assert.equal(fs.existsSync(path.join(box.home, "run", "setup-checkpoints")), false);
});

test("an internal holder retry returns the original receipt after a later save", { timeout: 20000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  await box.start();
  const setup = (await get(box)).json;
  const route = "kuvo/setup/save-local";
  const headers = { ...app, "x-kuvo-internal": "1" };
  const save = {
    id: setup.setup, epoch: setup.epoch, kind: preference, operation: "A".repeat(22), sequence: 0,
    value: { units: {}, imported: true },
  };
  assert.equal((await post(box, route, save)).status, 403);
  const first = await box.ask("POST", route, save, headers);
  assert.equal(first.status, 200, first.text);
  const later = await box.ask("POST", route, { ...save, sequence: 1, operation: "B".repeat(22), value: { ...save.value, imported: false } }, headers);
  assert.equal(later.status, 200, later.text);
  const retry = await box.ask("POST", route, save, headers);
  assert.equal(retry.status, 200, retry.text);
  assert.deepEqual(retry.json, first.json);
  const mismatched = await box.ask("POST", route, { ...save, value: { ...save.value, imported: false } }, headers);
  assert.equal(mismatched.status, 409, mismatched.text);
  assert.equal(entry(box, preference).sequence, 2);
});

test("a draft denied by the setup capacity remains recoverable", { timeout: 20000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  await box.start();
  await box.stop();
  const link = readJSON(linkPath(box));
  link.shared.ownershipAssignments = { sequence: 1, value: "x".repeat(Math.floor(512 * 1024 / 3)) };
  write(linkPath(box), JSON.stringify(link));
  await box.start();
  const kind = "setupTimeZone";
  const before = await get(box, `kuvo/data/edits?kind=${kind}`);
  const edit = { ref: { kind }, operation: "capacity", baseRevision: before.json.revision, fields: { zone: "Europe/Istanbul" } };
  const rejected = await post(box, "kuvo/data/edits", edit);
  assert.equal(rejected.status, 413, rejected.text);
  assert.equal(rejected.json.status, "capacity");
  assert.deepEqual((await post(box, "kuvo/data/edits", edit)).json, rejected.json);
  assert.deepEqual((await get(box, `kuvo/data/edits/${rejected.json.operation}`)).json.recovery, edit.fields);
  assert.equal(entry(box, kind), undefined);
});

test("a Mac starts its setup with the authenticated helper and leaves the private keys there", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  const password = "a-disposable-test-password";
  const secret = Buffer.alloc(32, 7).toString("base64url");
  let proofIsValid = false;
  let identityRequests = 0;
  const host = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const { nonce } = JSON.parse(Buffer.concat(chunks));
    const answer = (status, value) => res.writeHead(status, { "content-type": "application/json", "x-kuvo-host-version": "1" }).end(JSON.stringify(value));
    if (req.url === "/hello" && !req.headers.authorization) {
      const proof = createHmac("sha256", password).update(`kuvo-host-hello\0${nonce}\0${req.headers.host}`).digest("hex");
      return answer(200, { proof: proofIsValid ? proof : "00".repeat(32) });
    }
    if (req.url === "/link/identity" && req.headers.authorization === `Bearer ${password}`) {
      identityRequests++;
      return answer(200, { key: secret, sign: secret, secret });
    }
    answer(403, {});
  });
  await new Promise(resolve => host.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise(resolve => host.close(resolve)));
  write(path.join(box.home, "box", "kind"), "mac");
  write(path.join(box.home, "run", "host.json"), JSON.stringify({
    formatVersion: 1, password, url: `http://127.0.0.1:${host.address().port}`,
  }));
  assert.match(await box.start({ expectFailure: true }), /invalid Mac helper proof/);
  assert.equal(identityRequests, 0);

  proofIsValid = true;
  await box.start();
  const setup = (await get(box)).json;
  assert.equal(setup.you.role, "primary");
  assert.equal(setup.primary.kind, "mac");
  const link = readJSON(linkPath(box));
  for (const field of ["key", "secret", "sign", "name", "relay"]) assert.equal(link[field], undefined, field);
  assert.equal(identityRequests, 1);
  await box.stop();
  await box.start();
  assert.equal((await get(box)).json.setup, setup.setup);
});

test("relay restart grace lets the same main renew its expired grant", { timeout: 25000 }, async t => {
  const { a, b, aKey, id, leases } = await paired(t);
  await a.stop();
  const until = leases.leaseFor(id).expires;
  leases.restart();
  leases.advance(65000);
  await a.start();
  await waitForRenewal(leases, id, until);
  assert.equal(leases.leaseFor(id).holder, aKey);
  assert.equal(leases.leaseFor(id).term, 1);
  assert.equal((await get(a)).json.you.role, "primary");
  assert.equal((await get(b)).json.you.role, "member");
});

function waitForRenewal(leases, setup, expires) {
  return until(() => leases.leaseFor(setup)?.expires > expires, "the main should renew in restart grace", 12000);
}
