// The selected resident answers the desktop routes with a real backup directory.
// The only external components are the common bench's engine and relay.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { readJSON, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { at, id, key, oldPoint, source } from "./backup-fixture.mjs";

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
  write(path.join(original.home, "bots/bot_a/notes.md"), notes);
  write(path.join(original.home, "attachments/ses_a/evidence.bin"), payload);
  await until(async () => {
    const status = await on(old.ask, "GET", prefix);
    return status.computers.some(computer => computer.key === primaryKey && computer.relay);
  }, "The receiving computer did not advertise Relay backups", 20_000);
  const kept = await on(old.ask, "POST", prefix + "/destination", { confirm: true, destination });
  const { point } = await on(old.ask, "POST", prefix + "/now", {});
  const input = { source: kept.source, destination, id: point.id, recoveryKey: kept.recoveryKey, confirm: true };
  assert.deepEqual(await on(old.ask, "POST", prefix + "/points", input), { points: [point.id] });
  const account = path.join(path.dirname(receiver.home), "account");
  const archive = path.join(account, ".kuvo-backups", "relay", setup.setup, old.key, kept.source, point.id + ".kuvo");
  const ciphertext = fs.readFileSync(archive);
  assert.equal(ciphertext.subarray(0, Buffer.byteLength("KUVO backup 1\n")).toString("utf8"), "KUVO backup 1\n");
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
  assert.deepEqual(fs.readFileSync(path.join(replacement.home, "attachments/ses_a/evidence.bin")), payload);
  assert.equal(fs.readFileSync(path.join(replacement.home, "bots/bot_a/notes.md"), "utf8"), notes);
});

test("a project recovers its executable, empty directory and safe link", {
  skip: process.platform === "win32" && "POSIX project modes and link creation",
}, async t => {
  const { computer } = await bench(t);
  const owner = await computer("Project source");
  const replacement = await computer("Project replacement");
  const base = path.dirname(owner.home);
  const disk = path.join(base, "disk");
  const project = path.join(base, "project");
  fs.mkdirSync(disk);
  write(path.join(project, "bin", "check"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  fs.mkdirSync(path.join(project, "empty"), { mode: 0o750 });
  fs.symlinkSync("./bin/check", path.join(project, "check"));
  fs.symlinkSync("../elsewhere", path.join(project, "escape"));
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
  assert.ok(recovered.json.unavailable.some(({ entry, reason }) =>
    entry === "projects/0/escape" && reason === "unsafe symbolic link"));
  const tree = path.join(replacement.home, "backup", "recovered", backup.json.point.id, "projects", "0");
  assert.equal(fs.readFileSync(path.join(tree, "bin", "check"), "utf8"), "#!/bin/sh\nexit 0\n");
  assert.equal(fs.statSync(path.join(tree, "bin", "check")).mode & 0o777, 0o755);
  assert.deepEqual(fs.readdirSync(path.join(tree, "empty")), []);
  assert.equal(fs.statSync(path.join(tree, "empty")).mode & 0o777, 0o750);
  assert.equal(fs.readlinkSync(path.join(tree, "check")), "./bin/check");
  assert.equal(fs.lstatSync(path.join(tree, "escape"), { throwIfNoEntry: false }), undefined);
});
