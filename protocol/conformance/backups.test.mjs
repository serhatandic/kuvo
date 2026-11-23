// The selected resident answers the desktop routes with a real backup directory.
// The only external components are the common bench's engine and relay.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { readJSON, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { at, id, key, oldPoint, source, unpackPoint } from "./backup-fixture.mjs";

const app = { "x-kuvo-from": "app" };
const prefix = "kuvo/box/backups";

test("backup health and private backup operations require the local app", async t => {
  const { computer } = await bench(t);
  const box = await computer("Backups");
  const ask = box.connect(await box.start().ready());
  const health = await ask("GET", prefix + "/status");
  assert.equal(health.status, 403, health.text);
  assert.deepEqual(health.json, { message: "Open backup status from a paired Kuvo Code client." });
  const privateStatus = await ask("GET", prefix);
  assert.equal(privateStatus.status, 403, privateStatus.text);

  const local = await ask("GET", prefix + "/status", undefined, app);
  assert.equal(local.status, 200, local.text);
  assert.deepEqual(local.json, {
    enabled: false, destinationKind: null, destinationAvailable: null, lastVerifiedAt: null, attention: false,
  });

  const pathOnSameDisk = path.join(path.dirname(box.home), "disk");
  fs.mkdirSync(pathOnSameDisk);
  const destination = { kind: "disk", path: pathOnSameDisk };
  const preview = await ask("POST", prefix + "/destination", { destination }, app);
  assert.equal(preview.status, 200, preview.text);
  assert.deepEqual(preview.json.destination, destination);
  assert.deepEqual(preview.json.projects, []);
  assert.equal(preview.json.current.restorePointStatus, "no separate backup");
  const choose = await ask("POST", prefix + "/destination", { destination, confirm: true }, app);
  assert.equal(choose.status, 409, choose.text);
  assert.deepEqual(choose.json, { message: "Choose a folder on a separate disk." });
  assert.equal(fs.existsSync(path.join(box.home, "backup", "configuration.json")), false);

  for (const action of ["/vault", "/vault/import"]) {
    const privateVault = await ask("POST", prefix + action, { destination, confirm: true });
    assert.equal(privateVault.status, 403, privateVault.text);
    const unconfirmed = await ask("POST", prefix + action, { destination }, app);
    assert.equal(unconfirmed.status, 400, unconfirmed.text);
  }

  write(path.join(box.home, "backup/configuration.json"), JSON.stringify({
    version: 1, enabled: true, source: "private-source", key: "private-key",
    dest: destination, projects: [path.join(pathOnSameDisk, "private-project")],
    last: { id: "private-id", at: 1234, version: 1 }, error: "private error",
  }));
  const projected = await ask("GET", prefix + "/status", undefined, app);
  assert.equal(projected.status, 200, projected.text);
  assert.deepEqual(projected.json, {
    enabled: true, destinationKind: "disk", destinationAvailable: false, lastVerifiedAt: 1234, attention: true,
  });
});

test("a replacement verifies an old disk archive before placing portable data or pending work", async t => {
  const { computer } = await bench(t);
  const box = await computer("Replacement");
  const disk = path.join(path.dirname(box.home), "disk");
  oldPoint(disk);
  const ask = box.connect(await box.start().ready());
  const existingApproval = '{"last":"auto"}';
  write(path.join(box.home, "box", "approval.json"), existingApproval);
  const input = { destination: { kind: "disk", path: disk }, source, id, confirm: true };
  const list = await ask("POST", prefix + "/points", input, app);
  assert.equal(list.status, 200, list.text);
  assert.deepEqual(list.json, { points: [id] });

  const wrong = await ask("POST", prefix + "/restore", { ...input, recoveryKey: Buffer.alloc(32).toString("base64url") }, app);
  assert.equal(wrong.status, 409, wrong.text);
  assert.deepEqual(wrong.json, { message: "Backup verification failed. Check the recovery key and archive." });
  assert.equal(fs.existsSync(path.join(box.home, "bots", "bot_a", "notes.md")), false);

  const restored = await ask("POST", prefix + "/restore", { ...input, recoveryKey: key.toString("base64url") }, app);
  assert.equal(restored.status, 200, restored.text);
  assert.deepEqual(restored.json, {
    id, at, installed: 4, unavailable: [{
      entry: "home/box/approval.json", reason: "existing local record; recovered copy held for review",
    }], held: ["home/run/queues.json", "home/run/waiting/msg_due.json"],
    savedAfterPoint: "unavailable unless verified elsewhere", credentials: "sign-in required",
  });
  assert.equal(fs.readFileSync(path.join(box.home, "bots", "bot_a", "notes.md"), "utf8"), "History from the old disk.");
  assert.equal(fs.readFileSync(path.join(box.home, "box", "approval.json"), "utf8"), existingApproval);
  assert.equal(fs.readFileSync(path.join(box.home, "backup", "recovered", id, "home", "box", "approval.json"), "utf8"),
    '{"last":"askFirst","since":10}');
  assert.equal(fs.readFileSync(path.join(box.home, "wakes", "ses_a", "paused"), "utf8"), "restore-review");
  assert.equal(fs.existsSync(path.join(box.home, "run", "waiting", "msg_due.json")), false);
  assert.equal(fs.readFileSync(path.join(box.home, "backup", "recovered", id, "home", "run", "waiting", "msg_due.json"), "utf8"), '{"id":"msg_due","text":"Review the old send"}');

  const pending = await ask("GET", prefix + "/pending", undefined, app);
  assert.equal(pending.status, 200, pending.text);
  assert.deepEqual(pending.json, {
    waiting: [{ session: "ses_a", id: "msg_due", text: "Review the old send", at: 0, paused: true, review: true, available: true }],
    queueItems: [],
  });

  const replay = await ask("POST", prefix + "/restore", { ...input, recoveryKey: key.toString("base64url") }, app);
  assert.equal(replay.status, 200, replay.text);
  assert.deepEqual(replay.json, restored.json);
});

test("a legacy point reports incomplete attachments and lists the held queue item", async t => {
  const { computer } = await bench(t);
  const box = await computer("Queue replacement");
  const disk = path.join(path.dirname(box.home), "disk");
  const current = "wqi_" + "A".repeat(20);
  const deleted = "wqi_" + "B".repeat(20);
  const held = "wqi_" + "C".repeat(20);
  const queue = "home/run/work-queue/queues/usr_you.json";
  const file = (item, name) => `home/run/work-queue/files/${item}/${name}`;
  const undo = Buffer.from([0, 255, 7]);
  oldPoint(disk, [
    [queue, JSON.stringify({ owner: "usr_you", items: [
      { id: current, title: "Old item", files: [{ name: "missing.db", size: 4 }, { name: "short.txt", size: 2 }] },
      { id: deleted, title: "Deleted item", removed: { at, by: { user: true } }, files: [{ name: "proof.tmp", size: 3 }] },
      { id: held, title: "Held item", body: "Review the handoff", files: [{ name: "note.txt", size: 4 }],
        handing: { id: "wqh_restore", to: "ses_other", key: "A".repeat(43), at, by: { user: true } } },
    ] })],
    [file(current, "short.txt"), Buffer.from([0])],
    [file(deleted, "proof.tmp"), undo],
    [file(held, "note.txt"), "note"],
  ]);
  const ask = box.connect(await box.start().ready());
  const restore = await ask("POST", prefix + "/restore", {
    destination: { kind: "disk", path: disk }, source, id, confirm: true, recoveryKey: key.toString("base64url"),
  }, app);
  assert.equal(restore.status, 200, restore.text);
  assert.deepEqual(restore.json.unavailable.filter(({ entry }) => entry.startsWith(file(current, ""))), [
    { entry: file(current, "missing.db"), reason: "attachment missing from restore point" },
    { entry: file(current, "short.txt"), reason: "attachment size differs from its record" },
  ]);
  assert.deepEqual(fs.readFileSync(path.join(box.home, file(deleted, "proof.tmp").slice(5))), undo);
  assert.ok(readJSON(path.join(box.home, queue.slice(5))).items.find(item => item.id === deleted)?.removed);
  const review = await ask("GET", prefix + "/pending", undefined, app);
  assert.equal(review.status, 200, review.text);
  assert.deepEqual(review.json.queueItems, [{
    id: held, queue: "usr_you", title: "Held item", body: "Review the handoff",
    canKeep: true, files: [{ name: "note.txt", available: true }],
  }]);
});

test("a populated replacement database stays live and the old snapshot is held", async t => {
  const { computer } = await bench(t);
  const box = await computer("Database replacement");
  const disk = path.join(path.dirname(box.home), "disk");
  const old = path.join(path.dirname(box.home), "archived.db");
  const live = path.join(box.home, "data", "opencode", "opencode.db");
  fs.mkdirSync(path.dirname(live), { recursive: true });
  for (const [file, session] of [[old, "ses_archived"], [live, "ses_current"]]) {
    const db = new DatabaseSync(file);
    db.exec("PRAGMA journal_mode=WAL; CREATE TABLE session_v2(id TEXT PRIMARY KEY)");
    db.prepare("INSERT INTO session_v2 VALUES (?)").run(session);
    db.close();
  }
  oldPoint(disk, [["engine/opencode.db", fs.readFileSync(old)]]);
  const ask = box.connect(await box.start().ready());
  const restore = await ask("POST", prefix + "/restore", {
    destination: { kind: "disk", path: disk }, source, id, confirm: true, recoveryKey: key.toString("base64url"),
  }, app);
  assert.equal(restore.status, 200, restore.text);
  assert.ok(restore.json.unavailable.some(({ entry }) => entry === "engine/opencode.db"), restore.text);
  const held = path.join(box.home, "backup", "recovered", id, "engine", "opencode.db");
  for (const [file, session] of [[live, "ses_current"], [held, "ses_archived"]]) {
    const db = new DatabaseSync(file, { readOnly: true });
    try {
      assert.deepEqual(db.prepare("SELECT id FROM session_v2").all().map(row => row.id), [session]);
    } finally {
      db.close();
    }
  }
});

test("a restore holds journal records addressed to a different setup", async t => {
  const { computer } = await bench(t);
  const box = await computer("Journal replacement");
  const disk = path.join(path.dirname(box.home), "disk");
  const setup = "set_" + "A".repeat(22);
  const session = "ses_old";
  const issuer = "B".repeat(43);
  const operation = "old-request";
  const digest = (...parts) => createHash("sha256").update(JSON.stringify(parts)).digest("base64url");
  const submission = `${setup}.${issuer}.${operation}`;
  const issuerKey = digest("draft-submission", setup, session, submission);
  const fenceKey = digest("typed-draft", setup, session, operation);
  const forwardKey = digest("scoped-forward", setup, operation);
  const mutations = "home/run/owned-data/mutations/";
  const expected = [
    [mutations + fenceKey + ".json", { formatVersion: 1, digest: "scoped:" + issuerKey,
      draftReplayFence: { setup, session, id: operation } }],
    [mutations + forwardKey + ".json", { formatVersion: 1, digest: "C".repeat(43),
      forward: { kind: "forwardedSessionDecisions", setup, operation, owner: issuer, submitter: { kind: "computer" },
        area: "session", record: session, ref: { kind: "sessionQueue", id: session } }, answer: { status: 200, value: {} } }],
    [`home/run/owned-data/submissions/${issuerKey}.json`, { formatVersion: 1, submission: { setup, origin: issuer, session, id: submission } }],
  ];
  const opaque = mutations + "D".repeat(43) + ".json";
  const oldClaim = '{"digest":"historical","at":1}';
  oldPoint(disk, [[opaque, oldClaim], ...expected.map(([name, value]) => [name, JSON.stringify(value)])]);
  const ask = box.connect(await box.start().ready());
  const restore = await ask("POST", prefix + "/restore", {
    destination: { kind: "disk", path: disk }, source, id, confirm: true, recoveryKey: key.toString("base64url"),
  }, app);
  assert.equal(restore.status, 200, restore.text);
  assert.deepEqual(restore.json.unavailable.filter(({ entry }) => entry.includes("/owned-data/")), []);
  assert.deepEqual(restore.json.held.filter(name => name.includes("/owned-data/")).sort(), expected.map(([name]) => name).sort());
  for (const [name, value] of expected) {
    assert.equal(fs.existsSync(path.join(box.home, name.slice(5))), false);
    assert.deepEqual(readJSON(path.join(box.home, "backup", "recovered", id, name)), value);
  }
  assert.equal(fs.readFileSync(path.join(box.home, opaque.slice(5)), "utf8"), oldClaim);
});

test("a relay point survives the source and restores through its replacement membership", { timeout: 100_000 }, async t => {
  const { computer, leases } = await bench(t);
  const receiver = await computer("Receiving");
  const original = await computer("Original");
  const replacement = await computer("Replacement");
  const receiverProc = receiver.start();
  const primary = receiver.connect(await receiverProc.ready());
  const on = async (ask, method, route, body, expected = 200) => {
    const reply = await ask(method, route, body, app);
    assert.equal(reply.status, expected, reply.text);
    return reply.json;
  };
  const setup = await on(primary, "GET", "kuvo/setup");
  await until(() => leases.leaseFor(setup.setup), "Main computer did not claim its lease");
  const add = async box => {
    const invite = await on(primary, "POST", "kuvo/computers/add/code", {}, 201);
    const joined = await box.join(invite.code);
    assert.equal(joined.code, 0, joined.log);
    const proc = box.start();
    const ask = box.connect(await proc.ready());
    return { ask, proc, key: (await on(ask, "GET", "kuvo/setup")).you.key };
  };
  const old = await add(original);
  const { key: primaryKey } = setup.you;
  const destination = { kind: "computer", computer: primaryKey };
  const notes = "A backup point made before the source disk was lost.\n";
  const payload = Buffer.alloc(280_001, 19);
  const data = [
    ["bots/bot_a/notes.md", notes],
    ["bots/bot_a/MEMORY.md", "Durable bot memory"],
    ["bots/bot_a/groups/transcript.md", "Private group transcript"],
    ["groups/grp_a/messages.json", '["Saved group history"]'],
    ["attachments/ses_a/evidence.bin", payload],
    ["run/moves/decisions/mov_fixture.json", '{"decision":"switched"}'],
    ["run/work-queue/seen.json", '{"formatVersion":1}'],
    ["artifacts/copies/aa/aabb", Buffer.from([20, 255])],
  ];
  for (const [name, bytes] of data) write(path.join(original.home, name), bytes);
  const project = seedProject(path.dirname(original.home));
  await until(async () => {
    const status = await on(old.ask, "GET", prefix);
    return status.computers.some(computer => computer.key === primaryKey && computer.relay);
  }, "The receiving computer did not advertise Relay backups", 20_000);
  const kept = await on(old.ask, "POST", prefix + "/destination", { confirm: true, destination, projects: [project] });
  const { point } = await on(old.ask, "POST", prefix + "/now", {});
  const input = { source: kept.source, destination, id: point.id, recoveryKey: kept.recoveryKey, confirm: true };
  assert.deepEqual(await on(old.ask, "POST", prefix + "/points", input), { points: [point.id] });
  const account = path.join(path.dirname(receiver.home), "account");
  const archive = path.join(account, ".kuvo-backups", "relay", setup.setup, old.key, kept.source, point.id + ".kuvo");
  const ciphertext = fs.readFileSync(archive);
  const frames = unpackPoint(ciphertext, kept.source, point.id, Buffer.from(kept.recoveryKey, "base64url"));
  const manifest = JSON.parse(frames.get("meta/manifest.json"));
  assert.equal(manifest.format, 1);
  assert.equal(manifest.source, kept.source);
  assert.equal(manifest.id, point.id);
  assert.equal(manifest.origin, old.key);
  assert.deepEqual(frames.get("attachments/ses_a/evidence.bin"), payload);
  assert.equal(ciphertext.includes(notes), false);
  const ownerFolder = path.join(account, ".kuvo-backups", "relay", setup.setup, old.key);
  assert.deepEqual(fs.readdirSync(ownerFolder).sort(), [kept.source]);

  await old.proc.stop();
  fs.renameSync(original.home, original.home + "-gone");
  const gone = await primary("DELETE", "kuvo/computers/" + old.key, undefined, app);
  assert.ok([200, 202].includes(gone.status), gone.text);
  const fresh = await add(replacement);
  await on(primary, "POST", prefix + "/match", { source: kept.source, previous: old.key, replacement: fresh.key });
  await until(() => {
    const current = readJSON(path.join(replacement.home, "run", "link.json"));
    const assigned = { ...current.shared?.ownershipAssignments?.value, ...current.shared?.ownershipRecovery?.value };
    return assigned["replacement:" + old.key]?.computer === fresh.key;
  }, "The replacement did not save the main computer’s backup match", 20_000);
  await until(async () => {
    const points = await fresh.ask("POST", prefix + "/points", input, app);
    return points.status === 200 && points.json.points.includes(point.id);
  }, "The replacement did not receive its accepted backup match", 20_000);
  const result = await on(fresh.ask, "POST", prefix + "/restore", input);
  assert.equal(result.id, point.id);
  for (const [name] of data) {
    assert.deepEqual(fs.readFileSync(path.join(replacement.home, name)),
      fs.readFileSync(path.join(original.home + "-gone", name)), name);
  }
  checkProject(replacement.home, point.id, result);
});

const projectScript = "#!/bin/sh\nexit 0\n";

function seedProject(base) {
  const project = path.join(base, "project");
  write(path.join(project, "bin", "check"), projectScript, { mode: 0o755 });
  fs.mkdirSync(path.join(project, "empty"), { mode: 0o750 });
  fs.symlinkSync("check", path.join(project, "bin", "check-link"), "file");
  fs.symlinkSync("../elsewhere", path.join(project, "escape"), "file");
  return project;
}

function checkProject(home, point, recovered) {
  assert.ok(recovered.unavailable.some(({ entry, reason }) =>
    entry === "projects/0/escape" && reason === "unsafe symbolic link"));
  const tree = path.join(home, "backup", "recovered", point, "projects", "0");
  assert.equal(fs.readFileSync(path.join(tree, "bin", "check"), "utf8"), projectScript);
  assert.deepEqual(fs.readdirSync(path.join(tree, "empty")), []);
  if (process.platform !== "win32") {
    assert.equal(fs.statSync(path.join(tree, "bin", "check")).mode & 0o777, 0o755);
    assert.equal(fs.statSync(path.join(tree, "empty")).mode & 0o777, 0o750);
  }
  assert.equal(fs.readlinkSync(path.join(tree, "bin", "check-link")), "check");
  assert.equal(fs.readFileSync(path.join(tree, "bin", "check-link"), "utf8"), projectScript);
  assert.equal(fs.lstatSync(path.join(tree, "escape"), { throwIfNoEntry: false }), undefined);
}

test("a disk point recovers project data using the configured POSIX device", {
  skip: process.platform === "win32" && "The Relay scenario verifies project files and symbolic links on Windows",
}, async t => {
  const { computer } = await bench(t);
  const owner = await computer("Project source");
  const replacement = await computer("Project replacement");
  const base = path.dirname(owner.home);
  const disk = path.join(base, "disk");
  const project = seedProject(base);
  fs.mkdirSync(disk);
  const destination = { kind: "disk", path: disk };
  const recoveryKey = key.toString("base64url");
  // Start from an already-configured, currently mounted destination.
  write(path.join(owner.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source, key: recoveryKey, dest: destination,
    disk: fs.statSync(disk).dev, projects: [project],
  }));
  const askOwner = owner.connect(await owner.start().ready());
  const backup = await askOwner("POST", prefix + "/now", {}, app);
  assert.equal(backup.status, 200, backup.text);
  const askReplacement = replacement.connect(await replacement.start().ready());
  const recovered = await askReplacement("POST", prefix + "/restore", {
    destination, source, id: backup.json.point.id, recoveryKey, confirm: true,
  }, app);
  assert.equal(recovered.status, 200, recovered.text);
  checkProject(replacement.home, backup.json.point.id, recovered.json);
});
