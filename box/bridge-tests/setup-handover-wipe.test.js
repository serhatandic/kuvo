import "./test-safety.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { put } from "./recovery-bench.js";
import { link, network, until } from "./setup-handover-fixture.js";
import {
  inlineFiles, operationID, preparation, removalMessage, removalOf, seedLocalWork,
  uploadQueueFiles, uploadSessionFiles, waitForPreparedRemoval, waitForWipe, wipeRoute,
} from "./setup-removal-fixture.js";

test("failure bench: move-only wipe waits for an explicitly revised, byte-verified queue plan", { timeout: 120_000 }, async t => {
  // Setup and route subprocesses consume part of the outer budget on slower hosts.
  const startedAt = performance.now();
  const group = { id: "grp_badfiles", name: "Files for removal", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, relay, phase } = await network(t, false, undefined, false, group, true, false, undefined,
    { phaseDiagnostics: true });
  const milestone = description => phase(`${description} (+${Math.round(performance.now() - startedAt)} ms)`);
  milestone("setup connected");
  const bytes = Buffer.of(0, 255, 1, 7);
  const { id: item, paths: [attachment] } = await uploadQueueFiles(c, { "payload.lock": bytes }, group.id);
  milestone("queue attachment uploaded");
  const source = path.join(c.home, attachment);
  const planRoute = `kuvo/computers/${c.key}/removal-plan`;
  for (const corrupted of [bytes.subarray(0, 1), Buffer.of(0, 255, 1, 8)]) {
    fs.writeFileSync(source, corrupted);
    const plan = await a.route("POST", planRoute, {}, true);
    assert.equal(plan.status, 200, JSON.stringify(plan.value));
    assert.equal(plan.value.plan.complete, false);
    assert.deepEqual(plan.value.plan.files.filter(file => file.status === "unavailable"), [{ item: group.id, name: "payload.lock", status: "unavailable" }]);
  }
  milestone("both corrupt previews marked the attachment unavailable");

  const operation = operationID();
  const prepare = { moveTo: a.key };
  assert.equal((await a.route("POST", wipeRoute(c), { operation, prepare }, true)).status, 200);
  const refused = await until("unavailable attachment blocks removal", () => {
    const kept = removalOf(a, c);
    return kept?.error && kept;
  }, AbortSignal.any([t.signal, AbortSignal.timeout(30_000)]));
  assert.deepEqual(refused.unresolved, [group.id]);
  assert.equal(refused.generation, undefined);
  assert.equal(c.read().localWipe, undefined);
  assert.equal(fs.existsSync(source), true);
  milestone("first removal refused with the source intact");

  fs.writeFileSync(source, bytes);
  const revised = operationID();
  const readinessChecks = new Set();
  let checkedBeforeErase = false;
  const stopWatching = relay.intercept(c.address, a.address, packet => {
    const check = removalMessage(packet, a, c);
    const nonce = check?.moveFilesCheck;
    if (checkedBeforeErase || check?.operation !== revised || typeof nonce !== "string" || readinessChecks.has(nonce)) return false;
    // Separate preparation retries from the check made by the approved wipe worker.
    const wipe = c.read().localWipe;
    if (!wipe) readinessChecks.add(nonce);
    else if (wipe.operation === revised && wipe.prepared) checkedBeforeErase = true;
    return false;
  });
  t.after(stopWatching);
  const accepted = await a.route("POST", wipeRoute(c), { operation: revised, supersedes: operation, prepare }, true);
  assert.equal(accepted.status, 200, JSON.stringify(accepted.value));
  milestone("revised removal accepted for preparation");
  await waitForPreparedRemoval(a, c, revised, AbortSignal.any([t.signal, AbortSignal.timeout(30_000)]));
  assert.ok(readinessChecks.size > 0, "readiness checked the destination bytes");
  milestone("revised removal prepared after the destination check");
  await waitForWipe(c, t.signal, 30_000);
  assert.equal(fs.existsSync(source), false);
  assert.ok(checkedBeforeErase, "the wipe worker checked the destination using a fresh nonce");
  milestone("source erased after a fresh destination check");

  const opened = await a.command(["box", "route"], JSON.stringify({ method: "GET", path: `kuvo/work-queue/items/${item}/files/payload.lock` }));
  assert.equal(opened.status, 0, opened.err);
  assert.equal(opened.value.status, 200, JSON.stringify(opened.value));
  assert.deepEqual(Buffer.from(opened.value.bodyBase64, "base64"), bytes);
  milestone("original attachment bytes reopened from the destination");
});

test("removal preview discloses unselected payloads and missing retained artifacts", { timeout: 35_000 }, async t => {
  const group = { id: "grp_filepreview", name: "Preview", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c } = await network(t, false, undefined, false, group, true);
  const bytes = Buffer.of(0, 255, 9);
  const local = await c.route("POST", "kuvo/work-queue/items", { operation: "local-attachment-preview", title: "Local item",
    files: inlineFiles({ "local.lock": bytes }) }, true);
  assert.equal(local.status, 200, JSON.stringify(local.value));
  assert.equal(local.value.item.owner, "usr_you");
  put(path.join(c.home, "attachments", "legacy", "orphan.lock"), bytes);
  const hex = crypto.createHash("sha256").update(bytes).digest("hex");
  put(path.join(c.home, "artifacts", "log", "2026-11.jsonl"),
    JSON.stringify({ a: { id: "art_preview" }, v: { n: 1, kept: true, size: bytes.length, hash: `sha256:${hex}` } }) + "\n");
  const route = `kuvo/computers/${c.key}/removal-plan`;
  const preview = await a.route("POST", route, {}, true);
  assert.equal(preview.status, 200, JSON.stringify(preview.value));
  assert.equal(preview.value.plan.complete, false);
  assert.deepEqual(preview.value.plan.files.filter(file => file.status === "unavailable"),
    [{ item: "artifacts", name: `Retained artifact version ${hex.slice(0, 12)}`, status: "unavailable" }]);
  assert.ok(preview.value.plan.files.some(file => file.item === "usr_you" && file.status === "export-only"));
  assert.ok(preview.value.plan.files.some(file => file.name === "orphan.lock" && file.status === "export-only"));
  const copy = path.join(c.home, "artifacts", "copies", hex.slice(0, 2), hex);
  put(copy, Buffer.of(0, 255, 8));
  const changed = await a.route("POST", route, {}, true);
  assert.equal(changed.value.plan.complete, false);
  put(copy, bytes);
  const present = await a.route("POST", route, {}, true);
  assert.equal(present.value.plan.complete, true, JSON.stringify(present.value));
  assert.ok(present.value.plan.files.some(file => file.item === "artifacts" && file.status === "export-only"));
});

test("failure bench: an unreadable bot remains unresolved and blocks move-all deletion", { timeout: 35_000 }, async t => {
  const group = { id: "grp_unreadable", name: "Unresolved", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, groupCopies } = await network(t, false, undefined, false, group, true);
  const data = seedLocalWork(c);
  const planned = await a.route("POST", `kuvo/computers/${c.key}/removal-plan`, {}, true);
  assert.equal(planned.status, 200);
  assert.equal(planned.value.plan.complete, false);
  assert.deepEqual(planned.value.plan.unresolved, ["bot_retained"]);
  const operation = operationID();
  assert.equal((await a.route("POST", wipeRoute(c), { operation, prepare: { moveTo: a.key } }, true)).status, 200);
  const failure = await until("accepted unresolved work", () => removalOf(a, c)?.error && removalOf(a, c), t.signal);
  assert.equal(failure.generation, undefined);
  assert.deepEqual(failure.unresolved, ["bot_retained"]);
  assert.equal(c.read().localWipe, undefined);
  assert.equal(groupCopies.get("Lab")?.status, "active");
  assert.ok(data.every(file => fs.existsSync(file)));
  const cancelled = await a.route("POST", wipeRoute(c), { operation, resolve: "cancel" }, true);
  assert.equal(cancelled.status, 200);
  assert.deepEqual(cancelled.value.removal, { operation, state: "cancelled", accessRemoved: true });
  await until("source became standalone", () => c.read().setup?.role === "primary", t.signal);
  assert.ok(data.every(file => fs.existsSync(file)));
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
});

test("failure bench: a revised removal keeps completed moves and replaces a failed export", { timeout: 50_000 }, async t => {
  const group = { id: "grp_revised", name: "Keep this move", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, groupCopies } = await network(t, false, undefined, false, group, true, false, undefined);
  const data = seedLocalWork(c, { botWork: false });
  const operation = operationID();
  const original = path.join(c.env.TMPDIR, "missing-disk");
  const replacement = fs.mkdtempSync(path.join(c.env.TMPDIR, "replacement-disk-"));
  const prepare = folder => ({ export: { destination: { kind: "disk", path: folder }, recoveryKey: crypto.randomBytes(32).toString("base64url") } });
  const first = await a.route("POST", wipeRoute(c), { operation, prepare: { moveTo: a.key, ...prepare(original) } }, true);
  assert.equal(first.status, 200);
  await until("first recipe stalls after the transfer", () => removalOf(a, c)?.error, t.signal);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.deepEqual(preparation(c, "source", operation).moves, [{ id: group.id, kind: "group", completed: true, verified: true, sessions: [] }]);
  const revised = operationID();
  const second = await a.route("POST", wipeRoute(c), { operation: revised, supersedes: operation, prepare: prepare(replacement) }, true);
  assert.equal(second.status, 200, JSON.stringify(second.value));
  const poll = await a.route("POST", wipeRoute(c), { operation });
  assert.equal(poll.value.removal.state, "cancelled");
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
  await waitForWipe(c, t.signal);
  assert.ok(data.every(file => !fs.existsSync(file)));
  assert.equal(a.read().shared?.ownershipAssignments?.value?.["group host:" + group.id], undefined);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.equal(fs.existsSync(original), false);
  assert.ok(fs.readdirSync(path.join(replacement, "kuvo-backups")).length);
});

test("failure bench: a WAL checkpoint preserves an accepted wipe generation", { timeout: 40_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  const database = path.join(b.home, "data", "opencode", "opencode.db");
  fs.mkdirSync(path.dirname(database), { recursive: true });
  const db = new DatabaseSync(database);
  let closed = false;
  t.after(() => { if (!closed) db.close(); });
  db.exec("PRAGMA journal_mode=WAL; CREATE TABLE session (id TEXT); INSERT INTO session VALUES ('session');");
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(b.env.TMPDIR, "checkpoint-")) };
  const prepare = { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } };
  let grant;
  const release = relay.intercept(a.address, b.address, packet => {
    const instruction = link.readDoc(removalMessage(packet, b, a)?.instruction);
    if (!instruction?.body.generation) return false;
    grant = packet;
    return true;
  });
  t.after(release);
  assert.equal((await a.route("POST", wipeRoute(b), { operation, prepare }, true)).status, 200);
  await until("wipe grant issued", () => grant, t.signal);
  const generation = removalOf(a, b).generation;
  assert.ok(generation);
  assert.equal(db.prepare("PRAGMA wal_checkpoint(TRUNCATE)").get().busy, 0);
  db.close();
  closed = true;
  assert.equal(fs.existsSync(database + "-wal"), false);
  release();
  await waitForWipe(b, t.signal);
  assert.equal(removalOf(a, b).generation, generation);
});

test("failure bench: source work added after readiness invalidates the wipe grant", { timeout: 45_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  seedLocalWork(b);
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(b.env.TMPDIR, "generation-")) };
  const prepare = { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } };
  let grant;
  let hold = true;
  const release = relay.intercept(a.address, b.address, packet => {
    const instruction = link.readDoc(removalMessage(packet, b, a)?.instruction);
    if (!hold || !instruction?.body.generation) return false;
    grant = packet;
    return true;
  });
  t.after(release);
  assert.equal((await a.route("POST", wipeRoute(b), { operation, prepare }, true)).status, 200);
  await until("wipe grant issued", () => grant, t.signal);
  const originalGeneration = removalOf(a, b).generation;
  assert.ok(originalGeneration);
  const added = path.join(b.home, "bots", "bot_retained", "after-ready.md");
  fs.writeFileSync(added, "new work");
  hold = false;
  relay.replay(grant);
  await until("generation invalidated", () => !removalOf(a, b)?.generation && removalOf(a, b)?.error, t.signal);
  assert.equal(b.read().localWipe, undefined);
  assert.equal(fs.readFileSync(added, "utf8"), "new work");
  const retry = await a.route("POST", wipeRoute(b), { operation, retryPreparation: true }, true);
  assert.equal(retry.status, 200);
  // This accepts the retry request. The source must prepare again before the wipe can start.
  await until("the replacement wipe generation is accepted", () => {
    const generation = removalOf(a, b)?.generation;
    return generation && generation !== originalGeneration;
  }, t.signal);
  await waitForWipe(b, t.signal);
});

for (const change of ["added", "changed"]) test(`failure bench: a session attachment ${change} after readiness invalidates the wipe grant`, { timeout: 45_000 }, async t => {
  const { a, c: owner, relay } = await network(t, false, undefined, true, undefined, true);
  const file = "yarn.lock";
  const [original] = await uploadSessionFiles(owner, "msg_originalfile", { [file]: Buffer.of(0, 255) });
  assert.ok(original);
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(owner.env.TMPDIR, "attachment-generation-")) };
  const prepare = { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } };
  let grant;
  const release = relay.intercept(a.address, owner.address, packet => {
    const instruction = link.readDoc(removalMessage(packet, owner, a)?.instruction);
    if (!instruction?.body.generation) return false;
    grant = packet;
    return true;
  });
  t.after(release);
  const started = await a.route("POST", wipeRoute(owner), { operation, prepare }, true);
  assert.equal(started.status, 200, JSON.stringify(started.value));
  await until("wipe grant issued", () => grant, t.signal);
  assert.ok(removalOf(a, owner).generation);
  const bytes = Buffer.of(255, 0);
  let changed = original;
  if (change === "added") {
    [changed] = await uploadSessionFiles(owner, "msg_latefile", { "later.tmp": bytes });
  } else {
    fs.writeFileSync(path.join(owner.home, original), bytes);
  }
  relay.replay(grant);
  await until("attachment change invalidated generation", () => !removalOf(a, owner)?.generation && removalOf(a, owner)?.error, t.signal);
  assert.equal(owner.read().localWipe, undefined);
  assert.deepEqual(fs.readFileSync(path.join(owner.home, changed)), bytes);
});

test("failure bench: a deleted member retries a lost completion from outside its folder", { timeout: 40_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  const operation = operationID();
  let dropped = 0;
  const release = relay.intercept(null, a.address, packet => {
    if (removalMessage(packet, a, b)?.done !== operation) return false;
    dropped++;
    return true;
  });
  t.after(release);
  assert.equal((await a.route("POST", wipeRoute(b), { operation })).status, 200);
  await waitForWipe(b, t.signal);
  await until("completion dropped", () => dropped, t.signal);
  await delay(8_300);
  assert.equal(removalOf(a, b).delivered, undefined);
  const receipt = path.join(b.env.HOME, ".kuvo-code-removal-receipts", operation + ".json");
  const saved = JSON.parse(fs.readFileSync(receipt, "utf8"));
  assert.equal(saved.deleted, true);
  assert.equal(JSON.stringify(saved).includes(b.identity.key), false);
  assert.equal(JSON.stringify(saved).includes(b.identity.sign), false);
  release();
  await until("completion accepted", () => removalOf(a, b)?.delivered, t.signal);
  await until("completion acknowledged", () => !fs.existsSync(receipt), t.signal);
  assert.equal((await a.route("POST", wipeRoute(b), { operation })).value.removal.state, "wiped");
});
