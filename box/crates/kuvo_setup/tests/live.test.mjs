// Real Rust residents and their HTTP routes, with the controllable loopback relay.
// The initial roster fixture represents the accepted state written by enrollment.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { createHash, createHmac, createPrivateKey, createPublicKey, sign, verify } from "node:crypto";
import http from "node:http";
import {
  boxBinary, isolatedEnv, localRelay, readJSON, relayAddress, request, scratch, startBox, until, write,
} from "../../../../protocol/conformance/adapter.mjs";
import { relayLeaseStandIn } from "../../../bridge-tests/relay-lease-stand-in.js";
import { seedPeerSetup } from "../../../../protocol/conformance/peer-fixture.mjs";

const binary = boxBinary();
const app = { "x-kuvo-from": "app" };
const linkPath = box => path.join(box.home, "run", "link.json");
const entry = (box, kind) => readJSON(linkPath(box)).shared?.[kind];
const get = (box, route = "kuvo/setup") => box.ask("GET", route);
const post = (box, route, body) => box.ask("POST", route, body);
const address = box => relayAddress(Buffer.from(readJSON(linkPath(box)).secret, "base64url"));

async function bench(t, { handover = false, transferred, holdLeaseReply, startupTimeout } = {}) {
  const root = fs.realpathSync(scratch());
  const leases = relayLeaseStandIn({ transferStartsNow: handover });
  const relay = await localRelay({
    request: transferred || holdLeaseReply ? (request, response) => {
      const prior = leases.lease;
      return leases.request(request, {
        writeHead(...args) {
          response.writeHead(...args);
          return {
            end(...args) {
              const next = leases.lease;
              if (prior && next && prior.holder !== next.holder) transferred?.(relay);
              if (!holdLeaseReply?.(prior, next)) response.end(...args);
            },
          };
        },
      });
    } : leases.request,
  });
  const children = [];
  t.after(async () => {
    for (const child of children.reverse()) await child.stop();
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  });
  function computer(name, { reserved = false } = {}) {
    const folder = path.join(root, name);
    const home = path.join(folder, "box");
    const env = isolatedEnv(folder);
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    write(path.join(home, "run", "link.json"), JSON.stringify({ name, phones: [] }));
    if (reserved) write(path.join(folder, ".kuvo-code-adding"), "");
    return {
      home,
      async start({ expectFailure = false, spawned } = {}) {
        const proc = startBox({ binary, home, env, relay });
        children.push(proc);
        this.stop = proc.stop;
        this.log = proc.log;
        spawned?.(proc);
        if (expectFailure) {
          const result = await proc.exited;
          assert.equal(result.code, 1, proc.log());
          return proc.log();
        }
        const record = await proc.ready(startupTimeout);
        this.record = record;
        this.ask = (method, route, body, headers = app, timeout) => request(record, method, route, body, headers, timeout);
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
  const c = options?.third ? computer("Third") : undefined;
  await a.start();
  const first = readJSON(linkPath(a));
  await until(() => leases.leaseFor(first.setup.id), "Studio should claim the initial lease");
  const id = first.setup.id;
  const aKey = first.setup.primary;
  await b.start();
  const joined = readJSON(linkPath(b));
  const bKey = joined.setup.primary;
  if (c) await c.start();
  const third = c ? readJSON(linkPath(c)) : undefined;
  await a.stop();
  await b.stop();
  await c?.stop();
  const roster = first.shared.membershipAndAccess;
  roster.value.boxes.push(joined.shared.membershipAndAccess.value.boxes[0]);
  if (third) roster.value.boxes.push(third.shared.membershipAndAccess.value.boxes[0]);
  roster.sequence += 1;
  const second = {
    ...joined, setup: { ...first.setup, role: "member" }, sharedSetup: id, leadership: first.leadership,
    shared: first.shared, mac: aKey, macs: [aKey], macSigns: { [aKey]: first.setup.sign },
  };
  const checkpoints = "run/setup-checkpoints";
  for (const file of fs.readdirSync(path.join(a.home, checkpoints))) {
    write(path.join(b.home, checkpoints, file), fs.readFileSync(path.join(a.home, checkpoints, file)));
    if (c) write(path.join(c.home, checkpoints, file), fs.readFileSync(path.join(a.home, checkpoints, file)));
  }
  write(linkPath(a), JSON.stringify(first));
  write(linkPath(b), JSON.stringify(second));
  if (c) write(linkPath(c), JSON.stringify({
    ...third, setup: second.setup, sharedSetup: id, leadership: first.leadership,
    shared: first.shared, mac: aKey, macs: [aKey], macSigns: second.macSigns,
  }));
  write(path.join(b.home, "setup-takeover.json"), JSON.stringify({ alwaysOn: true }));
  await a.start();
  await b.start();
  await until(() => leases.leaseFor(id)?.roster?.[bKey] === joined.setup.sign, "the relay should learn the accepted roster");
  return { a, b, c, aKey, bKey, id, leases, relay };
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
  assert.equal(read.revision, createHash("sha256").update(JSON.stringify({
    can: true, choices: read.choices.map(choice => choice.id), kind: "choice", value: read.value,
  })).digest("base64url").slice(0, 16));
  const given = { row: "time-zone", value: "Europe/Istanbul", was: read.revision };
  const answer = await box.ask("PATCH", route, given);
  assert.equal(answer.status, 200, answer.text);
  const accepted = answer.json.groups[0].rows[0];
  assert.equal(accepted.value, "Europe/Istanbul");
  assert.deepEqual(accepted.choices.find(choice => choice.id === accepted.value),
    { id: "Europe/Istanbul", title: "Europe › Istanbul" });
  if (fs.existsSync("/usr/share/zoneinfo/Europe/Istanbul")) assert.match(accepted.line, /GMT\+3 now\./);
  assert.equal(answer.json.status, "committed");
  assert.equal(fs.readFileSync(path.join(box.home, "wakes", "timezone"), "utf8"), "Europe/Istanbul");
  const stale = await box.ask("PATCH", route, { ...given, value: "Asia/Tokyo" });
  assert.equal(stale.status, 409, stale.text);
  assert.equal(stale.json.message, "It changed since you read it.");
  assert.equal(entry(box, "setupTimeZone").value.zone, "Europe/Istanbul");
});

test("the current setup endpoint never advertises the old standby seat", { timeout: 25000 }, async t => {
  const { a, b, bKey } = await paired(t);
  await b.stop();
  const link = readJSON(linkPath(b));
  link.setup.standby = bKey;
  write(linkPath(b), JSON.stringify(link));
  await b.start();
  for (const box of [a, b]) {
    const { json: setup } = await get(box);
    assert.ok(!Object.hasOwn(setup, "standby"));
    assert.deepEqual(setup.computers.map(each => each.role), ["primary", "member"]);
  }
});

test("machine startup and authenticated activity preserve the legacy marker formats", { timeout: 15000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  await box.start();
  const build = (await get(box, "kuvo/info")).json.serverBuild;
  assert.equal(fs.readFileSync(path.join(box.home, "run", "link.version"), "utf8"), build);
  const seen = path.join(box.home, "run", "link.seen");
  assert.equal(fs.existsSync(seen), false);
  assert.equal((await box.ask("GET", "kuvo/setup", undefined, {})).status, 200);
  assert.equal(fs.readFileSync(seen).length, 0);
  const before = fs.statSync(seen).mtimeMs;
  assert.equal((await box.ask("GET", "kuvo/setup", undefined, {})).status, 200);
  assert.equal(fs.statSync(seen).mtimeMs, before);
});

test("a stale preference choice refreshes before a current choice can commit", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  await box.start();
  const old = await baseline(box);
  await edit(box, "first-model", old, { codexFallback: "gpt-5.4" });
  const conflict = await decision(box, await edit(box, "second-model", old, { codexFallback: "gpt-5.3-codex" }));
  assert.equal(conflict.status, "choice");
  const id = conflict.choices[0];
  const read = await baseline(box);
  await edit(box, "independent", read, { totalSpend: { show: true } });
  const current = await baseline(box);
  const resolve = (operation, baseRevision) => post(box, "kuvo/data/edits", {
    ref: { kind: preference }, operation, baseRevision, resolve: { id, use: "proposed" },
  });
  const stale = await resolve("stale-resolution", read);
  assert.equal(stale.status, 409, stale.text);
  assert.equal(stale.json.status, "refresh");
  assert.equal(stale.json.revision, current);
  const applied = await resolve("fresh-resolution", current);
  assert.equal(applied.status, 200, applied.text);
  const actual = await decision(box, applied.json.operation);
  assert.equal(actual.status, "committed");
  const { json: view } = await get(box, preferenceRoute);
  assert.deepEqual(view.fields, { totalSpend: { show: true }, codexFallback: "gpt-5.3-codex" });
  assert.deepEqual(view.choices, []);
});

test("a member misses two handovers and sees the former primary unlinked", { timeout: 65000 }, async t => {
  const { a, b, c, aKey, bKey } = await paired(t, { handover: true, third: true, startupTimeout: 40000 });
  await b.stop();
  const link = readJSON(linkPath(b));
  link.sharedOutcomes = Object.fromEntries(Array.from({ length: 257 }, (_, index) => {
    const operation = createHash("sha256").update(`archive ${index}`).digest("base64url");
    return [operation, {
      kind: "setupUsageDisplay", epoch: 1, sequence: index + 1, baseRevision: index,
      proposal: operation, status: "committed",
    }];
  }));
  write(linkPath(b), JSON.stringify(link));
  await b.start();
  const handover = async (box, target, epoch) => {
    const reply = await post(box, "kuvo/setup/handover", { primary: target, epoch });
    assert.equal(reply.status, 202, reply.text);
  };
  const atEpoch = (box, epoch) => until(async () => {
    const { json } = await get(box);
    return json.epoch === epoch && json;
  }, () => `the computer should follow epoch ${epoch}: ${box.log()}`, 13000);
  await handover(b, bKey, 1);
  await atEpoch(b, 2);
  await atEpoch(a, 2);
  await handover(a, aKey, 2);
  await atEpoch(a, 3);
  await c.start();
  const caught = await atEpoch(c, 3);
  assert.equal(caught.primary.key, aKey);
  const after = readJSON(linkPath(c));
  assert.equal(after.leadership["2"], undefined);
  const certificate = proof(after.leadership["3"], after.setup.sign);
  const readCheckpoint = hash => readJSON(path.join(c.home, "run", "setup-checkpoints", hash + ".json"));
  const manifest = proof(readCheckpoint(certificate.body.checkpoint), after.setup.sign);
  const pages = manifest.body.archives.map(hash => JSON.parse(Buffer.from(readCheckpoint(hash).d, "base64url")));
  assert.deepEqual(pages.map(page => Object.keys(page.body.outcomes).length), [256, 1]);
  for (const [operation, outcome] of Object.entries(link.sharedOutcomes)) {
    assert.deepEqual(after.sharedOutcomes[operation], outcome);
  }

  const removed = await post(a, `kuvo/computers/${bKey}/unlink`, {});
  assert.equal(removed.status, 200, removed.text);
  assert.deepEqual(removed.json.removal, {
    operation: removed.json.operation, state: "unlinked", accessRemoved: true,
  });
  const receipt = proof(removed.json.proof, after.setup.sign);
  assert.equal(receipt.body.epoch, 3);
  await until(() => {
    const local = readJSON(linkPath(b));
    const settled = [a, c].every(box => entry(box, "membershipAndAccess").value.removals?.[bKey]?.delivered);
    return local.setup?.id !== caught.setup && local.setup?.role === "primary" && settled;
  }, "The former primary did not leave and acknowledge its unlink.", 13000);
  const carried = (await get(c)).json;
  assert.equal(carried.setup, caught.setup);
  assert.equal(carried.epoch, 3);
  assert.equal(carried.primary.key, aKey);
  assert.equal(carried.computers.some(peer => peer.key === bKey), false);
});

test("a member forwards the main's signed documents to a third computer while the main is away", { timeout: 25000 }, async t => {
  const { a, b, c } = await paired(t, { third: true });
  const readDocs = box => readJSON(linkPath(box)).docs;
  const signedByMain = await until(() => {
    const docs = readDocs(b);
    return Object.keys(docs ?? {}).length === 2 && docs;
  }, "the first member should keep both signed documents");
  await a.stop();
  await c.start();
  await until(() => Object.keys(readDocs(c) ?? {}).length === 2,
    () => `the new member should learn both documents: ${c.log()}\n${b.log()}`, 15000);
  assert.deepEqual(readDocs(c), signedByMain);
  assert.equal((await get(c)).json.you.role, "member");
});

test("a saved preference reaches a witness only after the former main exits", { timeout: 40000 }, async t => {
  const { a, b, c, bKey, id, leases, relay } = await paired(t, { third: true });
  const resume = [relay.pause(address(a), address(c)), relay.pause(address(b), address(c))];
  await c.start();
  const before = await baseline(a);
  assert.equal((await decision(a, await edit(a, "saved-before-exit", before, { codexFallback: "gpt-5.4" }))).status, "committed");
  await until(() => entry(b, preference)?.value.units?.codexFallback?.value === "gpt-5.4", "the successor has a durable copy");
  assert.equal(entry(c, preference), undefined);
  await a.stop();
  await b.stop();
  const saved = readJSON(linkPath(b));
  delete saved.docs;
  write(linkPath(b), JSON.stringify(saved));
  const allowWitnessClaim = leases.disconnect(readJSON(linkPath(c)).shared.membershipAndAccess.value.boxes[2].key);
  await b.start();
  leases.advance(125000);
  await until(() => leases.leaseFor(id)?.holder === bKey, "the successor claims the lease");
  allowWitnessClaim();
  for (const unpause of resume) unpause();
  await until(async () => entry(c, preference)?.value.units?.codexFallback?.value === "gpt-5.4"
    && (await get(c)).json.primary.key === bKey, "the witness receives the successor's checkpoint", 14000);
});

test("a connected member catches up after the main's publication is lost", { timeout: 25000 }, async t => {
  const { a, b, relay } = await paired(t);
  const resume = relay.pause(address(a), address(b));
  const before = await baseline(a);
  const saved = await edit(a, "lost-publication", before, { codexFallback: "gpt-5.4" });
  assert.equal((await decision(a, saved)).status, "committed");
  assert.equal(entry(b, preference), undefined);
  resume();
  await until(async () => (await get(b, preferenceRoute)).json.fields?.codexFallback === "gpt-5.4",
    "the resident should resend the accepted preference without a restart");
});

test("independent preference fields merge against a common base in either order", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const fields = [{ codexFallback: "gpt-5.4" }, { totalSpend: { show: true } }];
  for (const [name, order] of [["First", [0, 1]], ["Second", [1, 0]]]) {
    const box = computer(name);
    await box.start();
    const before = await baseline(box);
    for (const index of order) {
      const saved = await edit(box, `independent-field-${index}`, before, fields[index]);
      assert.equal((await decision(box, saved)).status, "committed");
    }
    assert.deepEqual((await get(box, preferenceRoute)).json.fields, { ...fields[0], ...fields[1] });
    await box.stop();
  }
});

test("a successor unavailable before readiness leaves the original lease in place", { timeout: 42000 }, async t => {
  const { a, b, aKey, bKey, id, leases, relay } = await paired(t, { handover: true });
  const reconnect = relay.disconnect(address(b));
  try {
    const result = await a.ask("POST", "kuvo/setup/handover", { primary: bKey, epoch: 1 }, app, 30000);
    assert.equal(result.status, 503, result.text);
    assert.equal(leases.leaseFor(id).holder, aKey);
    assert.equal((await get(a)).json.epoch, 1);
    assert.deepEqual(Object.keys(readJSON(linkPath(a)).leadership), ["1"]);
  } finally {
    reconnect();
  }
});

test("a successor that disconnects after the transfer publishes its epoch on return", { timeout: 35000 }, async t => {
  let target, reconnect;
  const { a, b, bKey, id, leases } = await paired(t, {
    handover: true,
    transferred: relay => {
      if (target && !reconnect) reconnect = relay.disconnect(target);
    },
  });
  target = address(b);
  const result = await post(a, "kuvo/setup/handover", { primary: bKey, epoch: 1 });
  assert.equal(result.status, 202, result.text);
  assert.equal(leases.leaseFor(id).holder, bKey);
  assert.equal(typeof reconnect, "function");
  reconnect();
  await until(async () => (await get(a)).json.epoch === 2 && (await get(b)).json.you.role === "primary",
    "both computers should follow the transferred lease");
});

test("a former main stopped immediately after transfer restarts as a member", { timeout: 35000 }, async t => {
  let cut, resume;
  const { a, b, bKey } = await paired(t, {
    handover: true,
    transferred: relay => {
      if (cut && !resume) resume = relay.pause(cut.from, cut.to);
    },
  });
  cut = { from: address(b), to: address(a) };
  const result = await post(a, "kuvo/setup/handover", { primary: bKey, epoch: 1 });
  assert.equal(result.status, 202, result.text);
  assert.equal(typeof resume, "function");
  await a.stop();
  assert.equal(readJSON(linkPath(a)).setup.epoch, 1);
  await until(async () => (await get(b)).json.you.role === "primary", "the successor becomes main");
  resume();
  await a.start();
  await until(async () => (await get(a)).json.you.role === "member", "the former main follows the new epoch");
  assert.equal((await get(a)).json.primary.key, bKey);
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

test("a standalone library meets oversized copies and keeps 500 valid removal IDs", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio", { reserved: true });
  await box.start();
  const big = Array.from({ length: 300 }, (_, i) => ({
    id: `custom.big${i}`, name: `Big ${i}`, description: "Words. ".repeat(285), updated: 1,
  }));
  const removed = Array.from({ length: 510 }, (_, i) => ({ id: `custom.gone${i}`, removed: i + 2 }));
  const met = await post(box, "kuvo/personalities/take", { document: {
    personalities: [...big, { ...big[0], id: ["custom.invalid"] }],
    removed: [...removed, { id: ["custom.bad"], removed: 999 }],
  } });
  assert.equal(met.status, 200, met.text);
  assert.equal(met.json.changed.length, 800);
  const stored = (await get(box, "kuvo/personalities/document")).json;
  assert.equal(stored.personalities.length, 300);
  assert.deepEqual(stored.removed, removed.slice(10).reverse());
  const remove = await post(box, "kuvo/personalities/take", { document: {
    personalities: [], removed: [{ id: big[0].id, removed: 999 }],
  } });
  assert.deepEqual(remove.json, { changed: [big[0].id] });
  assert.equal((await get(box, "kuvo/personalities/document")).json.personalities.length, 299);
});

test("the library stream follows another writer and a future file stays intact", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio", { reserved: true });
  await box.start();
  const controller = new AbortController();
  t.after(() => controller.abort());
  const response = await fetch(`http://127.0.0.1:${box.record.port}/kuvo/personalities/events`, {
    headers: { authorization: "Basic " + Buffer.from(`opencode:${box.record.password}`).toString("base64"), "x-kuvo-route-version": "1" },
    signal: controller.signal,
  });
  assert.equal(response.status, 200);
  const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
  let pending = "";
  const line = async () => {
    while (!pending.includes("\n")) {
      const next = await reader.read();
      assert.equal(next.done, false, "the personality stream closed early");
      pending += next.value;
    }
    const end = pending.indexOf("\n");
    const read = JSON.parse(pending.slice(0, end));
    pending = pending.slice(end + 1);
    return read;
  };
  assert.deepEqual(await line(), { personalities: [] });
  const library = path.join(box.home, "bots", "personalities.json");
  const personality = { id: "custom.ssh", name: "From SSH", description: "Written outside the resident.", updated: 4 };
  write(library, JSON.stringify({ kuvo: 1, personalities: [personality], removed: [] }));
  const changed = await line();
  assert.equal(changed.personalities[0].id, personality.id);
  controller.abort();
  const future = JSON.stringify({ kuvo: 2, personalities: [personality], removed: [] });
  write(library, future);
  const refused = await post(box, "kuvo/personalities/take", { document: { personalities: [], removed: [] } });
  assert.equal(refused.status, 409, refused.text);
  assert.equal(refused.json.code, "unsupportedFormatVersion");
  assert.equal(fs.readFileSync(library, "utf8"), future);
  await box.stop();
  await box.start();
  assert.equal((await get(box, "kuvo/personalities/document")).status, 409);
});

test("machines carry kit, wake or launch zones and follow a changing kit only until a pick", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const kit = computer("Kit");
  const wake = computer("Wake");
  const launch = computer("Launch");
  const kitFile = path.join(kit.home, "bots", "kit.json");
  const setting = box => readJSON(path.join(box.home, "box", "time-zone.json"));
  const route = "kuvo/settings/clock";
  const row = async box => (await get(box, route)).json.groups[0].rows[0];
  write(kitFile, JSON.stringify({ kuvo: 1, timeZone: "Europe/Istanbul" }));
  write(path.join(kit.home, "wakes", "timezone"), "Asia/Tokyo");
  write(path.join(wake.home, "wakes", "timezone"), "Asia/Tokyo");
  write(path.join(wake.home, "run", "launch.env"), "KUVO_TZ=Europe/London\n");
  write(path.join(launch.home, "run", "launch.env"), "KUVO_TZ=Europe/London\n");
  for (const [box, zone] of [[kit, "Europe/Istanbul"], [wake, "Asia/Tokyo"], [launch, "Europe/London"]]) {
    await box.start();
    assert.equal((await row(box)).value, zone);
    assert.equal(setting(box).n, 0);
    assert.equal(setting(box).carried, true);
    assert.equal(entry(box, "setupTimeZone"), undefined);
  }
  write(kitFile, JSON.stringify({ kuvo: 1, timeZone: "America/New_York" }));
  const current = await until(async () => {
    const got = await row(kit);
    return got.value === "America/New_York" && got;
  }, "the carried clock follows the kit");
  assert.equal(fs.readFileSync(path.join(kit.home, "wakes", "timezone"), "utf8"), current.value);
  const save = await kit.ask("PATCH", route, { row: "time-zone", value: current.value, was: current.revision });
  assert.equal(save.status, 200, save.text);
  assert.equal(entry(kit, "setupTimeZone").value.carried, undefined);
  write(kitFile, JSON.stringify({ kuvo: 1, timeZone: "Europe/London" }));
  await kit.stop();
  await kit.start();
  assert.equal((await row(kit)).value, "America/New_York");
});

test("a member without zone data keeps the main's rules and warns if a later save has none", { timeout: 30000 }, async t => {
  const { a, b } = await paired(t);
  // A minimal TZif v1 file with one UTC type and no transitions.
  const rules = Buffer.alloc(54);
  rules.write("TZif");
  rules.writeUInt32BE(1, 36);
  rules.writeUInt32BE(4, 40);
  rules.write("UTC\0", 50);
  const check = async (sequence, zoneinfo) => {
    await a.stop();
    const link = readJSON(linkPath(a));
    link.shared.setupTimeZone = {
      sequence, value: { kuvo: 1, zone: "Fixture/Unavailable", n: sequence, ...(zoneinfo && { zoneinfo }) },
    };
    write(linkPath(a), JSON.stringify(link));
    await a.start();
    await until(() => entry(b, "setupTimeZone")?.sequence === sequence, "the zone save should replicate");
    const row = await until(async () => {
      const { json } = await get(b, "kuvo/settings/clock");
      const [current] = json.groups[0].rows;
      return current.value === "Fixture/Unavailable" && current.tone === (zoneinfo ? undefined : "warning") && current;
    }, "the replica should reach the clock endpoint");
    assert.deepEqual(row.choices[0], { id: row.value, title: "Fixture › Unavailable" });
    const file = path.join(b.home, "wakes", "zoneinfo");
    if (zoneinfo) {
      assert.deepEqual(fs.readFileSync(file), rules);
      assert.equal(row.tone, undefined);
    } else {
      assert.equal(fs.existsSync(file), false);
      assert.equal(row.tone, "warning");
    }
  };
  await check(1, rules.toString("base64"));
  await check(2);
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

async function macHost(t, box, keys, { proofIsValid = true } = {}) {
  const password = "a-disposable-test-password";
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
      return answer(200, keys);
    }
    answer(403, {});
  });
  await new Promise(resolve => host.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise(resolve => host.close(resolve)));
  write(path.join(box.home, "box", "kind"), "mac");
  write(path.join(box.home, "run", "host.json"), JSON.stringify({
    formatVersion: 1, password, url: `http://127.0.0.1:${host.address().port}`,
  }));
  return { allow() { proofIsValid = true }, requests() { return identityRequests } };
}

test("a Mac starts its setup with the authenticated helper and leaves the private keys there", { timeout: 25000 }, async t => {
  const { computer } = await bench(t);
  const box = computer("Studio");
  const seed = Buffer.alloc(32, 7).toString("base64url");
  const host = await macHost(t, box, { key: seed, sign: seed, secret: seed }, { proofIsValid: false });
  assert.match(await box.start({ expectFailure: true }), /invalid Mac helper proof/);
  assert.equal(host.requests(), 0);

  host.allow();
  await box.start();
  const setup = (await get(box)).json;
  assert.equal(setup.you.role, "primary");
  assert.equal(setup.primary.kind, "mac");
  const link = readJSON(linkPath(box));
  for (const field of ["key", "secret", "sign", "name", "relay"]) assert.equal(link[field], undefined, field);
  assert.equal(host.requests(), 1);
  await box.stop();
  await box.start();
  assert.equal((await get(box)).json.setup, setup.setup);
});

test("a main with incoming devices off still revokes a device and delivers a machine unlink", { timeout: 50000 }, async t => {
  const { computer, relay, leases } = await bench(t);
  const studio = computer("Studio");
  const lab = computer("Lab");
  const { main, member } = seedPeerSetup(studio.home, relay.url);
  seedPeerSetup(lab.home, relay.url, 2);
  const original = readJSON(linkPath(studio));
  await macHost(t, studio, { key: original.key, sign: original.sign, secret: original.secret });
  const configuration = phones => write(path.join(studio.home, "run", "mac-link.json"),
    JSON.stringify({ phones, machines: true, relay: relay.url, name: "Studio", own: false }));
  configuration(true);
  await studio.start();
  await lab.start();
  await until(() => leases.leaseFor(original.setup.id)?.holder === main.key, "Studio did not hold the relay lease.");
  const desktop = Buffer.alloc(32, 79).toString("base64url");
  const pairing = await post(studio, "kuvo/devices/pair", { key: desktop, kind: "desktop", name: "Test desktop" });
  assert.equal(pairing.status, 200, pairing.text);
  const labDevices = async () => (await get(lab, "kuvo/phones")).json.phones;
  await until(async () => (await labDevices()).some(device => device.key === desktop), "Lab did not see the desktop grant.");
  await studio.stop();
  configuration(false);
  await studio.start();
  await until(async () => (await get(studio)).json.leadershipWarning
    === "The main computer also runs Kuvo's relay. If this computer goes offline, another computer can't take over.",
    "Studio did not resume its relay lease.");
  const refused = await post(studio, "kuvo/devices/pair", { key: Buffer.alloc(32, 80).toString("base64url"), kind: "desktop", name: "Another desktop" });
  assert.equal(refused.status, 409, refused.text);
  const release = relay.pause(Buffer.from(main.address, "base64url"), Buffer.from(member.address, "base64url"));
  try {
    const revoked = await post(studio, "kuvo/phones/remove", { phones: [{ key: desktop, pairing: pairing.json.pairing }] });
    assert.equal(revoked.status, 200, revoked.text);
    assert.ok((await labDevices()).some(device => device.key === desktop));
  } finally { release(); }
  await until(async () => !(await labDevices()).some(device => device.key === desktop), "Lab missed the revocation while incoming devices were off.");
  const unlink = await post(studio, `kuvo/computers/${member.key}/unlink`, {});
  assert.equal(unlink.status, 200, unlink.text);
  await until(() => entry(studio, "membershipAndAccess").value.removals[member.key].delivered,
    "Lab did not acknowledge the unlink while incoming devices were off.", 15000);
  assert.equal((await get(lab)).json.you.role, "primary");
});

test("an older Mac main converts its signed checkpoint once under the relay lease", { timeout: 25000 }, async t => {
  let cut;
  const held = new Promise(resolve => { cut = resolve; });
  let interrupt = true;
  const { computer, leases, relay } = await bench(t, {
    holdLeaseReply(prior, next) {
      if (!interrupt || prior || !next) return false;
      cut();
      return true;
    },
  });
  const box = computer("Studio");
  const { main, member } = seedPeerSetup(box.home, relay.url);
  const old = readJSON(linkPath(box));
  await macHost(t, box, { key: old.key, sign: old.sign, secret: old.secret });
  const seed = Buffer.from(old.sign, "base64url");
  const signer = createPrivateKey({
    key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), seed]),
    format: "der", type: "pkcs8",
  });
  for (const key of ["key", "sign", "secret", "relay"]) delete old[key];
  old.shared.membershipAndAccess.value.boxes[0].kind = "mac";
  old.shared.membershipAndAccess.value.successorOrder = [member.key, main.key];
  old.shared.membershipAndAccess.value.successorPin = member.key;
  old.shared.setupNames = { sequence: 2, value: { [member.key]: "Lab upstairs" } };
  const { id, epoch, primary } = old.setup;
  const tip = Buffer.from(JSON.stringify({
    v: 1, kind: "leadership", origin: main.sign, n: epoch,
    body: { setup: { id, epoch, primary }, standby: member.key, successorChain: true },
  }));
  old.leadership = { [epoch]: {
    d: tip.toString("base64url"),
    s: sign(null, Buffer.concat([Buffer.from("kuvo-link doc v1\0"), tip]), signer).toString("base64url"),
  } };
  write(linkPath(box), JSON.stringify(old));

  let process;
  const abandoned = box.start({ spawned: proc => { process = proc; } })
    .then(() => ({ ready: true }), error => ({ error }));
  const stoppedAtLease = await Promise.race([held.then(() => true), abandoned.then(() => false)]);
  assert.equal(stoppedAtLease, true, box.log());
  process.child.kill("SIGKILL");
  await process.exited;
  assert.ok((await abandoned).error);
  assert.deepEqual(readJSON(linkPath(box)), old);
  interrupt = false;

  await box.start();
  const current = readJSON(linkPath(box));
  assert.equal(current.setup.id, id);
  assert.equal(current.setup.epoch, epoch + 1);
  assert.equal(current.setup.primary, main.key);
  assert.equal(leases.leaseFor(id).holder, main.key);
  assert.equal(current.shared.membershipAndAccess.value.successorOrder, undefined);
  assert.equal(current.shared.membershipAndAccess.value.successorPin, undefined);
  assert.deepEqual(current.shared.membershipAndAccess.value.boxes.map(({ key }) => key), [main.key, member.key]);
  assert.deepEqual(current.shared.setupNames, old.shared.setupNames);
  assert.deepEqual(current.leadership[epoch], old.leadership[epoch]);
  for (const key of ["key", "sign", "secret"]) assert.equal(current[key], undefined);
  await box.stop();
  await box.start();
  assert.deepEqual(readJSON(linkPath(box)).setup, current.setup);
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
