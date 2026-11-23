import "./test-safety.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { testBun } from "./bun-for-tests.js";
import { assertReceipt, link, network, submit, until } from "./setup-handover-fixture.js";
import {
  operationID, preparation, removalMessage, removalOf, seedLocalWork, separateExportRoot, uploadQueueFiles,
  uploadSessionFiles, waitForPreparedRemoval, waitForWipe, wipeRoute,
} from "./setup-removal-fixture.js";

test("failure bench: move all verifies accepted ownership before wiping the source", { timeout: 60_000 }, async t => {
  const group = { id: "grp_beforewipe", name: "Before wipe", members: ["bot_one", "bot_two"], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, c, groupCopies } = await network(t, false, undefined, false, group, true, false, undefined);
  const data = seedLocalWork(c, { botWork: false });
  const planned = await a.route("POST", `kuvo/computers/${c.key}/removal-plan`, {}, true);
  assert.equal(planned.status, 200, JSON.stringify(planned.value));
  const plan = planned.value.plan;
  assert.equal(plan.source.key, c.key);
  assert.equal(plan.complete, true);
  assert.deepEqual(plan.movable.map(({ id, destinations }) => [id, destinations.map(place => place.key)]), [[group.id, [a.key]]]);
  assert.ok(plan.stays.includes("native-agent-histories"));
  assert.equal(Object.hasOwn(plan, "projects"), false);
  const temp = c.env.TMPDIR;
  const project = fs.realpathSync(fs.mkdtempSync(path.join(temp, "chosen-project-")));
  c.env.TMPDIR = fs.mkdtempSync(path.join(temp, "source-scratch-"));
  try {
    const selected = await c.route("POST", `kuvo/computers/${c.key}/removal-plan`, { projects: [project] }, true);
    assert.equal(selected.status, 200, JSON.stringify(selected.value));
    assert.deepEqual(selected.value.plan.projects, [project]);
  } finally { c.env.TMPDIR = temp; }
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(c), { operation, prepare: { moveTo: a.key } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  assert.equal(start.value.removal.state, "moving");
  assert.equal(removalOf(a, c).operation, operation);
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
  await waitForPreparedRemoval(a, c, operation, AbortSignal.timeout(30_000)).catch(error => {
    const moves = box => {
      const folder = path.join(box.run, "moves");
      return fs.existsSync(folder) ? fs.readdirSync(folder).filter(file => file.endsWith(".json")).map(file => JSON.parse(fs.readFileSync(path.join(folder, file), "utf8"))) : [];
    };
    throw new Error(`${error.message}. Source moves ${JSON.stringify(moves(c))}. Destination moves ${JSON.stringify(moves(a))}.`);
  });
  await waitForWipe(c, t.signal);
  assert.ok(data.every(file => !fs.existsSync(file)));
  assert.equal(a.read().shared?.ownershipAssignments?.value?.["group host:" + group.id], undefined);
  assert.equal(groupCopies.get("Studio")?.group.id, group.id);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.equal(groupCopies.has("Lab"), false);
});

test("failure bench: an offline Relay source completes its accepted moves and export on reconnect", { timeout: 150_000 }, async t => {
  const group = { id: "grp_offlinemove", name: "Offline move", members: [], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, b, c, relay, groupCopies } = await network(t, false, undefined, false, group, true, false, undefined);
  const bridge = JSON.parse(fs.readFileSync(path.join(a.run, "bridge.json"), "utf8"));
  fs.writeFileSync(path.join(b.run, "bridge.json"), JSON.stringify({ ...bridge, password: b.name }));
  await until("destination supports accepted moves", () => a.heard(b.key) && c.heard(b.key), t.signal);
  await c.stop("SIGTERM");
  await until("move source offline", () => !relay.has(c.address), t.signal);
  fs.mkdirSync(path.join(c.home, "backup"), { recursive: true });
  fs.writeFileSync(path.join(c.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source: "b_" + operationID(), key: crypto.randomBytes(32).toString("base64url"),
    dest: { kind: "computer", computer: b.key }, projects: [],
  }));
  fs.writeFileSync(path.join(c.run, "peer-reach.json"), JSON.stringify({ [b.key]: Date.now() }));
  // After access is revoked, the destination can't clear the source's stale reachability by ordinary mail.
  const scoped = [];
  const seeRemoval = (packet, target, sender) => {
    const message = removalMessage(packet, target, sender);
    if (message) scoped.push(message.scopedMove?.op ?? (message.moveProbe ? "probe" : message.moveReady ? "ready for removal" : "other"));
    return message;
  };
  t.after(relay.intercept(b.address, c.address, packet => !seeRemoval(packet, c, b)));
  t.after(relay.intercept(c.address, b.address, packet => { seeRemoval(packet, b, c); return false; }));
  const first = operationID();
  const start = await a.route("POST", wipeRoute(c), { operation: first, prepare: { moveTo: b.key } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(c.key));
  c.start();
  await until("move planning requires a separate export", () => removalOf(a, c)?.error,
    AbortSignal.any([t.signal, AbortSignal.timeout(30_000)]));
  const review = await a.route("POST", wipeRoute(c), { operation: first }, true);
  assert.equal(review.status, 200, JSON.stringify(review.value));
  assert.equal(review.value.removal.nextAction, "revise-export");
  assert.equal(groupCopies.get("Lab")?.status, "active");
  const operation = operationID();
  const destination = { kind: "disk", path: separateExportRoot(t, c) };
  const newPlan = await a.route("POST", wipeRoute(c), { operation, supersedes: first,
    prepare: { moveTo: b.key, export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } } }, true);
  assert.equal(newPlan.status, 200, JSON.stringify(newPlan.value));
  await waitForPreparedRemoval(a, c, operation, AbortSignal.any([t.signal, AbortSignal.timeout(55_000)])).catch(error => {
    const state = { source: preparation(c, "source", operation), accepted: removalOf(a, c),
      destination: removalOf(b, c), ownership: b.read().shared?.ownershipAssignments, scoped, groups: [...groupCopies] };
    throw new Error(`${error.message}: ${JSON.stringify(state)}`, { cause: error });
  });
  await waitForWipe(c, t.signal);
  const point = removalOf(a, c).export;
  assert.ok(fs.existsSync(path.join(destination.path, "kuvo-backups", point.source, point.id + ".kuvo")));
  assert.ok(scoped.includes("probe"));
  assert.ok(scoped.includes("ready for removal"));
  assert.equal(a.read().shared?.ownershipAssignments?.value?.["group host:" + group.id], undefined);
  assert.equal(groupCopies.get("Desk")?.status, "active");
  assert.equal(groupCopies.has("Lab"), false);
});

test("failure bench: an encrypted export survives wipe and imports through restore", { timeout: 120_000 }, async t => {
  // The outer deadline covers setup, real uploads, prepare, detach and a fresh restore.
  // Each destructive handoff below still has its own tighter deadline.
  const startedAt = performance.now();
  const { a, b: replacement, c: owner, prompts, phase } = await network(t, false, undefined, true, undefined, true, false, undefined,
    { phaseDiagnostics: true });
  const milestone = description => phase(`${description} (+${Math.round(performance.now() - startedAt)} ms)`);
  const data = seedLocalWork(owner);
  const sessionBytes = {
    "readme.txt": Buffer.from("read me"), "yarn.lock": Buffer.of(0, 1, 255), "design.new": Buffer.of(255, 3, 0),
    ".keep": Buffer.alloc(0), "report.tmp": Buffer.of(0, 128, 255), "report.db": Buffer.of(7, 0, 129),
  };
  const queueBytes = {
    "queue.bin": Buffer.of(1, 0), "queue.lock": Buffer.of(255, 1), "queue.new": Buffer.of(128, 1),
    "queue.tmp": Buffer.of(0, 1, 2, 255), "queue.db": Buffer.of(7, 6, 5), "queue.db-wal": Buffer.of(0, 128),
  };
  const sessionFiles = await uploadSessionFiles(owner, "msg_backupfiles", sessionBytes);
  const { id: queueItem, paths: queueFiles } = await uploadQueueFiles(owner, queueBytes);
  assert.equal(prompts.at(-1)?.id, "msg_backupfiles");
  assert.deepEqual(sessionFiles.map(file => path.basename(file)).sort(), Object.keys(sessionBytes).sort());
  const promised = [
    ...sessionFiles.map(file => [file, sessionBytes[path.basename(file)]]),
    ...queueFiles.map(file => [file, queueBytes[path.basename(file)]]),
  ];
  for (const [file, bytes] of promised) {
    assert.deepEqual(fs.readFileSync(path.join(owner.home, file)), bytes);
    assert.equal(fs.existsSync(path.join(replacement.home, file)), false);
  }
  milestone("session and queue uploads saved and their source bytes verified");
  const destination = { kind: "disk", path: path.join(owner.env.TMPDIR, "manual-export") };
  fs.mkdirSync(destination.path);
  const recoveryKey = crypto.randomBytes(32).toString("base64url");
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(owner), { operation, prepare: { export: { destination, recoveryKey } } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  assert.equal(start.value.removal.state, "exporting");
  milestone("removal plan saved with export requested");
  await waitForPreparedRemoval(a, owner, operation, AbortSignal.any([t.signal, AbortSignal.timeout(25_000)]));
  milestone("prepared export acknowledged by the primary");
  // Allow the next Relay offer, export readback and the worker's writer-stop window.
  await waitForWipe(owner, t.signal, 30_000);
  milestone("source installation detached");
  assert.ok(data.every(file => !fs.existsSync(file)));
  await until("verified wipe accepted", () => removalOf(a, owner)?.delivered, AbortSignal.any([t.signal, AbortSignal.timeout(10_000)]));
  milestone("source wipe completion recorded by the primary");
  const retry = await a.route("POST", wipeRoute(owner), { operation });
  assert.equal(retry.status, 200, JSON.stringify(retry.value));
  const poll = await a.route("POST", wipeRoute(owner), { operation: start.value.operation });
  assert.deepEqual(poll, retry);
  const { source, id } = retry.value.export;
  const archive = fs.readFileSync(path.join(destination.path, "kuvo-backups", source, id + ".kuvo"));
  assert.ok(archive.subarray(0, 14).equals(Buffer.from("KUVO backup 1\n")));
  assert.equal(archive.includes("local work"), false);
  const matched = await a.route("POST", "kuvo/box/backups/match", { source, previous: owner.key, replacement: replacement.key }, true);
  assert.equal(matched.status, 200, JSON.stringify(matched.value));
  milestone("primary committed the replacement match");
  await until("replacement ownership replica", () => replacement.read().shared?.ownershipRecovery?.value?.["replacement:" + owner.key],
    AbortSignal.any([t.signal, AbortSignal.timeout(8_000)]));
  milestone("approved replacement mapping replicated");
  const restored = await replacement.route("POST", "kuvo/box/backups/restore", { destination, source, id, recoveryKey, confirm: true }, true);
  assert.equal(restored.status, 200, JSON.stringify(restored.value));
  assert.equal(fs.readFileSync(path.join(replacement.home, "bots", "bot_retained", "notes.md"), "utf8"), "local work");
  assert.deepEqual(restored.value.unavailable.filter(({ entry }) => entry === queueItem ||
    entry.startsWith("attachments/") || entry.startsWith("home/run/work-queue/files/")), []);
  for (const [file, bytes] of promised) assert.deepEqual(fs.readFileSync(path.join(replacement.home, file)), bytes, file);
  milestone("replacement restored and all promised bytes verified");
  const queue = JSON.parse(fs.readFileSync(path.join(replacement.run, "work-queue/queues/ses_owned.json"), "utf8"));
  assert.deepEqual(queue.items.find(item => item.id === queueItem)?.files.map(file => file.name), Object.keys(queueBytes));
});

test("failure bench: a missing listed attachment blocks a requested export and wipe", { timeout: 40_000 }, async t => {
  const { a, c: source } = await network(t, false, undefined, true, undefined, true);
  const data = seedLocalWork(source);
  const payloads = { "missing.tmp": Buffer.of(0, 7, 255), "keep.bin": Buffer.of(8, 0, 129) };
  const { paths: [missing, retained] } = await uploadQueueFiles(source, payloads);
  fs.unlinkSync(path.join(source.home, missing));
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(source.env.TMPDIR, "incomplete-export-")) };
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(source), { operation, prepare: {
    export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") },
  } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assert.equal(start.value.removal.state, "exporting");
  const failure = await until("missing attachment reported", () => removalOf(a, source)?.error, t.signal);
  assert.match(failure, /missing\.tmp/);
  assert.equal(removalOf(a, source).generation, undefined);
  assert.equal(removalOf(a, source).export, undefined);
  assert.equal(source.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));
  assert.deepEqual(fs.readFileSync(path.join(source.home, retained)), payloads["keep.bin"]);
  assert.equal(fs.readdirSync(destination.path, { recursive: true }).some(file => file.endsWith(".kuvo")), false);
  const bypass = await a.route("POST", wipeRoute(source), { operation: operationID() });
  assert.equal(bypass.status, 409, JSON.stringify(bypass.value));
  const cancelled = await a.route("POST", wipeRoute(source), { operation, resolve: "cancel" }, true);
  assert.equal(cancelled.status, 200, JSON.stringify(cancelled.value));
  assert.equal(cancelled.value.removal.state, "cancelled");
  assert.ok(data.every(file => fs.existsSync(file)));
});

test("failure bench: legacy export readiness cannot authorize a wipe", { timeout: 35_000 }, async t => {
  const { a, b: source, relay } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(source);
  const operation = operationID();
  const destination = { kind: "disk", path: fs.mkdtempSync(path.join(source.env.TMPDIR, "legacy-export-")) };
  let ready;
  t.after(relay.intercept(source.address, a.address, packet => {
    const message = removalMessage(packet, a, source);
    if (message?.operation !== operation || !message.progress?.generation) return false;
    ready ??= { packet, message };
    return true;
  }));
  const start = await a.route("POST", wipeRoute(source), { operation, prepare: {
    export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") },
  } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  const { packet, message } = await until("new readiness message", () => ready, t.signal);
  assert.ok(message.progress.export?.id);
  assert.equal(typeof message.progress.inventoryVersion, "number");
  // Model the same source on an older build, which could authenticate its archive but sent no inventory version.
  delete message.progress.inventoryVersion;
  const legacy = link.readRelay(link.mailFor(a.entry, message, source.read(), true));
  assert.equal(legacy.kind, "deliver");
  relay.replay({ ...packet, payload: legacy.payload });
  const failure = await until("legacy export readiness refused", () => removalOf(a, source)?.error, t.signal);
  assert.match(failure, /Update the source computer to verify all attachments/);
  assert.equal(removalOf(a, source).generation, undefined);
  assert.equal(removalOf(a, source).export, undefined);
  assert.equal(source.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));

  // Model a primary upgrade after a grant was accepted under the old rules.
  const saved = await a.evaluate(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
    import { updateAccess } from ${JSON.stringify(new URL("../src/link/setup/access.ts", import.meta.url).href)};
    const receipt = updateAccess(link => {
      const record = link.memberRemovals[${JSON.stringify(source.key)}];
      record.generation = ${JSON.stringify(message.progress.generation)};
      record.export = ${JSON.stringify(message.progress.export)};
      delete record.error;
      return link;
    }, { checkpoint: true }).save;
    // The main saves alone: the receipt is final.
    console.log(JSON.stringify(receipt));
  `]);
  assert.equal(saved.status, 0, saved.err);
  assert.equal(saved.value?.status, "committed");
  const withdrawn = await until("saved legacy grant withdrawn", () => {
    const record = removalOf(a, source);
    return record?.withdrawnGeneration && record;
  }, t.signal);
  assert.equal(withdrawn.generation, undefined);
  assert.equal(withdrawn.export, undefined);
  assert.equal(withdrawn.withdrawnGeneration, message.progress.generation);
  assert.equal(source.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));
  assert.equal((await a.route("POST", wipeRoute(source), { operation, resolve: "cancel" }, true)).status, 409);
  const revision = await a.route("POST", wipeRoute(source), { operation: operationID(), supersedes: operation,
    prepare: { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } },
  }, true);
  assert.equal(revision.status, 409);

  // A completion for the same operation can have been in flight before upgrade.
  const done = link.readRelay(link.mailFor(a.entry, {
    setup: message.setup, done: operation, generation: message.progress.generation,
  }, source.read(), true));
  assert.equal(done.kind, "deliver");
  relay.replay({ ...packet, payload: done.payload });
  await until("previously issued wipe completion recorded", () => removalOf(a, source)?.delivered, t.signal);
  assert.equal(removalOf(a, source).error, undefined);
});

test("failure bench: a failed requested export revokes access and keeps source data", { timeout: 50_000 }, async t => {
  const { a, b } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(b);
  const destination = { kind: "disk", path: path.join(b.env.TMPDIR, "missing-export-folder") };
  const recoveryKey = crypto.randomBytes(32).toString("base64url");
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(b), { operation, prepare: { export: { destination, recoveryKey } } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  const failure = await until("export error", () => removalOf(a, b)?.error, t.signal);
  assert.match(failure, /source or export path is unavailable/);
  const status = await a.route("GET", "kuvo/computers");
  assert.equal(status.value.wipePending?.find(pending => pending.operation === operation)?.error, failure);
  const bypass = await a.route("POST", wipeRoute(b), { operation: operationID() });
  assert.equal(bypass.status, 409, JSON.stringify(bypass.value));
  assert.equal(removalOf(a, b).operation, operation);
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(b.key));
  assert.equal(b.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));
  fs.mkdirSync(destination.path);
  const retry = await a.route("POST", wipeRoute(b), { operation, retryPreparation: true }, true);
  assert.equal(retry.status, 200, JSON.stringify(retry.value));
  await until("retry reaches source", () => !preparation(b, "source", operation)?.error, t.signal);
  await waitForPreparedRemoval(a, b, operation, t.signal);
  await waitForWipe(b, t.signal);
});

test("failure bench: an unreachable source keeps a requested export and wipe pending until it returns", { timeout: 50_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(b);
  const destination = { kind: "disk", path: path.join(b.env.TMPDIR, "pending-export") };
  fs.mkdirSync(destination.path);
  await b.stop();
  await until("target offline", () => !relay.has(b.address), t.signal);
  const operation = operationID();
  const start = await a.route("POST", wipeRoute(b), { operation, prepare: {
    export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") },
  } }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  assert.equal(start.value.removal.state, "exporting");
  assert.equal(removalOf(a, b).operation, operation);
  assert.ok(a.read().shared.membershipAndAccess.value.left.includes(b.key));
  assert.ok(data.every(file => fs.existsSync(file)));
  await a.stop("SIGTERM");
  a.start();
  await until("primary restarted", () => relay.has(a.address), t.signal);
  const listed = await a.route("GET", "kuvo/computers");
  assert.deepEqual(listed.value.wipePending, [{ key: b.key, name: b.name, operation }]);
  b.start();
  await waitForPreparedRemoval(a, b, operation, t.signal);
  await waitForWipe(b, t.signal);
  assert.ok(data.every(file => !fs.existsSync(file)));
  assert.ok(fs.readdirSync(path.join(destination.path, "kuvo-backups")).length);
});

test("failure bench: an offline source’s accepted export preparation survives primary handover", { timeout: 60_000 }, async t => {
  const { a, b, c, relay, see } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(b);
  const destination = { kind: "disk", path: path.join(b.env.TMPDIR, "handover-export") };
  fs.mkdirSync(destination.path);
  const operation = operationID();
  const prepare = { export: { destination, recoveryKey: crypto.randomBytes(32).toString("base64url") } };
  await b.stop();
  await until("source offline", () => !relay.has(b.address), t.signal);
  const start = await a.route("POST", wipeRoute(b), { operation, prepare }, true);
  assert.equal(start.status, 200, JSON.stringify(start.value));
  assertReceipt(a, "membershipAndAccess", "committed", start.value);
  const membership = a.read().shared.membershipAndAccess.value;
  assert.equal(membership.removals[b.key].operation, operation);
  assert.equal(JSON.stringify(membership).includes(prepare.export.recoveryKey), false);
  await until("candidate learns the export preparation before taking over", () => removalOf(c, b)?.operation === operation, t.signal);
  const transfer = await submit(a, c, 2);
  assert.equal(transfer.status, 202, JSON.stringify(transfer.value));
  await see(c, 3, c);
  assert.equal(c.read().shared.membershipAndAccess.value.removals[b.key].operation, operation);
  const bypass = await c.route("POST", wipeRoute(b), { operation: operationID() });
  assert.equal(bypass.status, 409, JSON.stringify(bypass.value));
  assert.equal(removalOf(c, b).operation, operation);
  assert.ok(data.every(file => fs.existsSync(file)));
  const resumed = await c.route("POST", wipeRoute(b), { operation, prepare }, true);
  assert.equal(resumed.status, 200, JSON.stringify(resumed.value));
  b.start();
  await waitForPreparedRemoval(c, b, operation, t.signal);
  await waitForWipe(b, t.signal);
});
