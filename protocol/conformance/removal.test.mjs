// The main and departing computer use real residents. The private relay lets
// the test hold the removed computer offline while the main commits.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, localRelay, privateHome, readJSON, request, scratch, startBox, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

const fromMac = { "x-kuvo-from": "mac" };
const applicant = process.env.KUVO_CONFORMANCE_JOIN_BIN
  ? boxBinary(process.env.KUVO_CONFORMANCE_JOIN_BIN) : boxBinary();

test("uninstalling a stopped machine clears its desktop identity and sign-ins but retains work", async t => {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const home = path.join(root, "box");
  const env = isolatedEnv(root);
  privateHome(home);
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "desktop", "identity"), "fixture identity");
  write(path.join(home, "box", "keys", "KUVO_KEY_SAMPLE"), "fixture key");
  write(path.join(home, "run", "launch.notes"), "fixture launch notes");
  write(path.join(home, "data", "retained.txt"), "saved work");
  write(path.join(home, "bots", "bot_test", "bot.json"), '{"kuvo":1,"id":"bot_test","name":"Test"}');
  write(path.join(home, "bots", "bot_test", "session"), "ses_bot");
  write(path.join(home, "bots", "bot_test", "watches", "watch", "check"), "a check");
  write(path.join(home, "bots", "bot_old", "session"), "ses_old");
  write(path.join(home, "wakes", "ses_local", "session"), "ses_local");
  write(path.join(home, "wakes", "ses_local", "wakes", "1700000000.2-test"), "a wake");
  write(path.join(home, "wakes", "ses_local", "wakes", "unrecognized"), "ignore");
  write(path.join(home, "wakes", "ses_away", "session"), "ses_away");
  write(path.join(home, "wakes", "ses_away", "wakes", "1700000001-test"), "a moved wake");
  write(path.join(home, "run", "moves", "mov_test.json"),
    '{"formatVersion":1,"id":"mov_test","thing":"ses_away","peer":"peer","role":"from","stage":"switching","sessions":["ses_away"]}');
  const run = async (...argv) => {
    const proc = boxCommand(boxBinary(), home, env, { url: "ws://127.0.0.1:1" }, ["uninstall", ...argv], {}, "pipe");
    t.after(() => proc.stop());
    let stdout = "";
    proc.child.stdout.setEncoding("utf8").on("data", text => { stdout += text; });
    proc.child.stdin.end();
    assert.equal((await proc.exited).code, 0, proc.log());
    return JSON.parse(stdout.trim());
  };
  const before = await run("--check");
  assert.equal(before.folder, home);
  assert.deepEqual(before.running, []);
  assert.equal(before.bots, 1);
  assert.equal(before.wakes, 2);
  assert.equal(fs.existsSync(path.join(home, "desktop", "identity")), true);
  await run();
  assert.ok(fs.existsSync(path.join(home, "run", "off")));
  for (const file of ["desktop", "box/keys", "run/launch.notes"]) {
    assert.equal(fs.existsSync(path.join(home, file)), false, file);
  }
  assert.equal(fs.readFileSync(path.join(home, "data", "retained.txt"), "utf8"), "saved work");
  assert.equal((await run()).result, "uninstalled");
  assert.equal(fs.existsSync(path.join(home, "run", "link.json")), false);
});

test("the only computer commits a wipe and erases only its old installation", {
  timeout: 35000, skip: process.platform === "win32" && "Native startup and shutdown belong to the Windows keeper.",
}, async t => {
  const root = fs.realpathSync(scratch());
  const binary = boxBinary();
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const relay = await localRelay();
  const local = path.join(home, "data", "fixture.txt");
  const outside = path.join(env.HOME, "untouched.txt");
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "run", "link.json"), '{"name":"Last computer","phones":[]}');
  write(local, "erase this work");
  write(outside, "outside the installation");
  fs.symlinkSync(outside, path.join(home, "data", "outside-link"));
  const resident = startBox({ binary, home, env, relay });
  const children = [resident];
  t.after(async () => {
    for (const child of children.reverse()) await child.stop();
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true });
  });
  const record = await resident.ready();
  const before = await request(record, "GET", "kuvo/setup");
  assert.equal(before.status, 200, before.text);
  const setup = before.json.setup;
  const key = before.json.you.key;
  const operation = Buffer.alloc(16, 90).toString("base64url");
  const answer = await request(record, "POST", `kuvo/computers/${key}/unlink`, { operation }, fromMac);
  assert.equal(answer.status, 200, answer.text);
  assert.deepEqual(answer.json.removal, { operation, state: "wipe-pending", accessRemoved: true });
  assert.equal(answer.json.offer, "reinstall-local");
  assert.equal(answer.json.status, "committed");
  const marker = path.join(env.HOME, ".kuvo-code-removal-receipts", "last-setup-wipe");
  await until(() => fs.existsSync(marker) && !readJSON(marker).phase,
    () => `The old installation was not erased. ${resident.log()}`);
  assert.deepEqual(readJSON(marker), { setup, operation, formatVersion: 1 });
  assert.equal(fs.existsSync(home), false);
  assert.equal(fs.readFileSync(outside, "utf8"), "outside the installation");

  if (!binary.typescript) {
    const { computer, leases, relay: newRelay } = await bench(t);
    const main = await computer("New setup");
    const ask = main.connect(await main.start().ready());
    const nextSetup = (await ask("GET", "kuvo/setup")).json.setup;
    const invite = await ask("POST", "kuvo/computers/add/code", {}, fromMac);
    assert.equal(invite.status, 201, invite.text);
    await until(() => leases.leaseFor(nextSetup), "The replacement setup did not claim a relay lease.");
    fs.mkdirSync(home);
    const installer = boxCommand(binary, home, env, newRelay, ["install", "--join"]);
    children.push(installer);
    installer.child.stdin.end(invite.json.code + "\n");
    assert.equal((await installer.exited).code, 0, installer.log());
    const next = startBox({ binary, home, env, relay: newRelay });
    children.push(next);
    const adopted = await request(await next.ready(), "GET", "kuvo/setup");
    assert.equal(adopted.json.setup, nextSetup);
    assert.equal(adopted.json.you.role, "member");
    assert.notEqual(adopted.json.you.key, key);
    assert.equal(fs.existsSync(local), false);
  }
});

test("a Rust wipe resumes against its detached directory and leaves a replacement alone", {
  skip: (process.platform === "win32" || boxBinary().typescript) && "The Rust stopped-machine recovery command.",
}, async t => {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const receipts = path.join(env.HOME, ".kuvo-code-removal-receipts");
  const operation = Buffer.alloc(16, 91).toString("base64url");
  const setup = "set_" + Buffer.alloc(16, 15).toString("base64url");
  const marker = path.join(receipts, "last-setup-wipe");
  const detached = path.join(receipts, `erase-${operation}`);
  write(path.join(detached, "old.txt"), "old installation");
  write(path.join(home, "new.txt"), "new installation");
  fs.chmodSync(receipts, 0o700);
  const journal = source => {
    write(marker, JSON.stringify({ setup, operation, source, phase: "erasing", formatVersion: 1 }));
    fs.chmodSync(marker, 0o600);
  };
  const run = async () => {
    const child = boxCommand(boxBinary(), home, env, { url: "ws://127.0.0.1:1" }, ["wipe-member"]);
    t.after(() => child.stop());
    child.child.stdin.end();
    return { ...await child.exited, log: child.log() };
  };
  const identity = p => { const { dev, ino } = fs.lstatSync(p, { bigint: true }); return `${dev}:${ino}`; };
  journal(identity(home));
  const refused = await run();
  assert.equal(refused.code, 1, refused.log);
  assert.match(refused.log, /The folder to erase has changed\./);
  assert.equal(fs.existsSync(detached), true);
  journal(identity(detached));
  const finished = await run();
  assert.equal(finished.code, 0, finished.log);
  assert.equal(fs.existsSync(detached), false);
  assert.equal(fs.readFileSync(path.join(home, "new.txt"), "utf8"), "new installation");
  assert.deepEqual(readJSON(marker), { setup, operation, formatVersion: 1 });
});

test("the main revokes an offline computer once and keeps the unlink receipt after restart", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const other = await computer("Lab", applicant);
  const mainProcess = main.start();
  const ask = main.connect(await mainProcess.ready());
  const original = await ask("GET", "kuvo/setup");
  assert.equal(original.status, 200, original.text);
  const { setup, you } = original.json;
  const code = await ask("POST", "kuvo/computers/add/code", {}, fromMac);
  assert.equal(code.status, 201, code.text);
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
  const joined = await other.join(code.json.code);
  assert.equal(joined.code, 0, joined.log);
  // The resident on Lab has never started, so it cannot acknowledge the operation yet.
  const link = () => readJSON(path.join(main.home, "run", "link.json"));
  const view = await ask("GET", "kuvo/setup");
  const key = view.json.computers.find(computer => computer.name === "Lab").key;
  const operation = Buffer.alloc(16, 55).toString("base64url");
  const method = "POST";
  const route = `kuvo/computers/${key}/unlink`;
  const removed = await ask(method, route, { operation }, fromMac);
  assert.equal(removed.status, 200, removed.text);
  assert.deepEqual(removed.json.removal, { operation, state: "unlinked", accessRemoved: true });
  assert.equal(removed.json.setup, setup);
  assert.equal(removed.json.status, "committed");
  assert.equal(typeof removed.json.proof.d, "string");
  assert.equal(typeof removed.json.proof.s, "string");
  const accepted = link().shared.membershipAndAccess;
  assert.deepEqual(accepted.value.boxes.map(peer => peer.key), [you.key]);
  assert.deepEqual(accepted.value.left, [key]);
  assert.equal(accepted.value.removalRevisions[key], 1);
  assert.equal(accepted.value.removals[key].wipe, false);
  assert.equal(accepted.value.removals[key].delivered, undefined);

  await mainProcess.stop();
  const after = main.connect(await main.start().ready());
  const replay = await after(method, route, { operation }, fromMac);
  assert.equal(replay.status, 200, replay.text);
  assert.deepEqual(replay.json, removed.json);
  assert.equal(link().shared.membershipAndAccess.sequence, accepted.sequence);
  const conflict = await after(method, route, { operation: Buffer.alloc(16, 99).toString("base64url") }, fromMac);
  assert.equal(conflict.status, 409, conflict.text);
});

test("a member forwards an unlink and receives the main’s saved decision", { timeout: 90000 }, async t => {
  const { computer, leases, relay } = await bench(t);
  const main = await computer("Studio");
  const member = await computer("Workshop");
  const other = await computer("Lab");
  const primary = main.connect(await main.start().ready());
  const setup = (await primary("GET", "kuvo/setup")).json.setup;
  for (const computer of [member, other]) {
    const offer = await primary("POST", "kuvo/computers/add/code", {}, fromMac);
    assert.equal(offer.status, 201, offer.text);
    await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
    const joined = await computer.join(offer.json.code);
    assert.equal(joined.code, 0, joined.log);
  }
  const ask = member.connect(await member.start().ready());
  const load = home => readJSON(path.join(home, "run", "link.json"));
  const mainState = () => load(main.home).shared.membershipAndAccess;
  const [studio, workshop, lab] = mainState().value.boxes;
  assert.deepEqual([studio.name, workshop.name, lab.name], ["Studio", "Workshop", "Lab"]);
  await until(() => load(member.home).shared?.membershipAndAccess?.value.boxes.some(peer => peer.key === lab.key),
    "The member did not see Lab before unlinking it.");
  const route = `kuvo/computers/${lab.key}/unlink`;
  const operation = Buffer.alloc(16, 151).toString("base64url");
  let dropped = false;
  const resume = relay.intercept(Buffer.from(studio.address, "base64url"), Buffer.from(workshop.address, "base64url"), () => {
    dropped ||= mainState().value.removals?.[lab.key]?.operation === operation;
    return true;
  });
  const pending = ask("POST", route, { operation }, fromMac);
  let early;
  pending.then(answer => { early = answer; });
  try {
    await until(() => {
      assert.ok(!early, early?.text);
      return dropped;
    }, "The main did not publish the forwarded unlink.");
  } finally {
    resume();
  }
  const answered = await pending;
  assert.equal(answered.status, 200, answered.text);
  assert.deepEqual(answered.json.removal, { operation, state: "unlinked", accessRemoved: true });
  const accepted = mainState();
  const replay = await primary("POST", route, { operation }, fromMac);
  assert.equal(replay.status, 200, replay.text);
  assert.deepEqual(replay.json, answered.json);
  assert.equal(mainState().sequence, accepted.sequence);
  assert.deepEqual(accepted.value.left, [lab.key]);
});

test("a released computer enrolls again when its retained work is unchanged", {
  timeout: 120000,
}, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const other = await computer("Lab", applicant);
  const ask = main.connect(await main.start().ready());
  const initial = await ask("GET", "kuvo/setup");
  const setup = initial.json.setup;
  const invite = () => ask("POST", "kuvo/computers/add/code", {}, fromMac);
  const firstCode = await invite();
  assert.equal(firstCode.status, 201, firstCode.text);
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
  const first = await other.join(firstCode.json.code);
  assert.equal(first.code, 0, first.log);
  const readLink = home => readJSON(path.join(home, "run", "link.json"));
  const database = path.join(other.home, "data", "opencode", "opencode.db");
  fs.mkdirSync(path.dirname(database), { recursive: true });
  const initialize = new DatabaseSync(database);
  initialize.exec("PRAGMA journal_mode=WAL; CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT); INSERT INTO session VALUES ('kept', 'Before unlink');");
  initialize.close();
  const resident = other.start();
  const local = other.connect(await resident.ready());
  const usageFile = path.join(other.home, "agents", "usage", "settings.json");
  const usageScope = () => fs.existsSync(usageFile) && readJSON(usageFile).setup;
  if (applicant.typescript) {
    await until(() => usageScope() === setup, "Usage settings did not follow the initial setup.");
  }
  const old = await local("GET", "kuvo/setup");
  const key = old.json.you.key;
  const operation = Buffer.alloc(16, 21).toString("base64url");
  const removed = await ask("POST", `kuvo/computers/${key}/unlink`, { operation }, fromMac);
  assert.equal(removed.status, 200, removed.text);
  await until(() => {
    const state = readLink(other.home);
    return state.setup?.role === "primary" && state.setup.id !== setup
      && state.departedKey === key && !state.localUnlink
      && readLink(main.home).shared.membershipAndAccess.value.removals[key].delivered;
  }, () => `The unlink handshake did not complete.\n${resident.log()}`, 30000);

  const kept = await local("GET", "kuvo/setup");
  assert.equal(kept.status, 200, kept.text);
  assert.notEqual(kept.json.setup, setup);
  assert.deepEqual(kept.json.computers.map(row => [row.key, row.role]), [[key, "primary"]]);
  if (applicant.typescript) {
    await until(() => usageScope() === kept.json.setup, "Usage settings did not follow the standalone setup.");
  }
  const next = await invite();
  assert.equal(next.status, 201, next.text);
  const joinWith = process.env.KUVO_CONFORMANCE_REJOIN_BIN
    ? boxBinary(process.env.KUVO_CONFORMANCE_REJOIN_BIN) : applicant;
  const standalone = readLink(other.home);
  assert.match(standalone.releasedScopedWork?.logical ?? "", /^[A-Za-z0-9_-]{43}$/,
    "unlink records the logical snapshot of a closed WAL database");
  if (!joinWith.typescript) {
    await resident.stop();
    const db = new DatabaseSync(database);
    db.exec("UPDATE session SET title = 'After unlink'");
    db.close();
    const changed = await other.join(next.json.code, { joinWith });
    assert.equal(changed.code, 1, changed.log);
    assert.match(changed.log, /This computer has setup data/);
    assert.equal(readLink(other.home).key, standalone.key);
    const restored = new DatabaseSync(database);
    restored.exec("UPDATE session SET title = 'Before unlink'; ANALYZE; PRAGMA wal_checkpoint(TRUNCATE)");
    restored.close();
    // A write after unlink must still be protected by the Rust applicant.
    const payload = path.join(other.home, "artifacts", "retained-work.json");
    fs.mkdirSync(path.dirname(payload), { recursive: true });
    fs.writeFileSync(payload, '{"title":"Written after unlink"}', { flag: "wx" });
    try {
      const blocked = await other.join(next.json.code, { joinWith });
      assert.equal(blocked.code, 1, blocked.log);
      assert.match(blocked.log, /This computer has setup data/);
      assert.equal(readLink(other.home).key, standalone.key);
      assert.equal(fs.readFileSync(payload, "utf8"), '{"title":"Written after unlink"}');
    } finally {
      fs.unlinkSync(payload);
    }
  }
  const rejoined = await other.join(next.json.code, { resident: joinWith.typescript, joinWith });
  assert.equal(rejoined.code, 0, rejoined.log);
  const member = readLink(other.home);
  assert.equal(member.setup.id, setup);
  assert.equal(member.setup.role, "member");
  for (const field of ["key", "sign", "secret"]) {
    assert.notEqual(member[field], standalone[field], field);
  }
  const accepted = readLink(main.home).shared.membershipAndAccess.value;
  assert.deepEqual(accepted.left, [key]);
  assert.deepEqual(accepted.boxes.map(peer => peer.name), ["Studio", "Lab"]);
  assert.notEqual(accepted.boxes[1].key, key);
  if (!joinWith.typescript) {
    const connect = other.connect(await other.start(joinWith).ready());
    const view = await connect("GET", "kuvo/setup");
    assert.equal(view.status, 200, view.text);
    assert.equal(view.json.setup, setup);
    assert.equal(view.json.you.role, "member");
    assert.equal(view.json.you.key, accepted.boxes[1].key);
  }
});

test("a departing member clears credentials and keeps its work before acknowledging unlink", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const member = await computer("Desk", applicant);
  const ask = main.connect(await main.start().ready());
  const initial = (await ask("GET", "kuvo/setup")).json;
  const offered = await ask("POST", "kuvo/computers/add/code", {}, fromMac);
  assert.equal(offered.status, 201, offered.text);
  await until(() => leases.leaseFor(initial.setup), "The main computer did not claim a relay lease.");
  const joined = await member.join(offered.json.code);
  assert.equal(joined.code, 0, joined.log);

  const data = path.join(member.home, "data", "opencode");
  const database = path.join(data, "opencode.db");
  fs.mkdirSync(data, { recursive: true });
  const db = new DatabaseSync(database);
  db.exec("PRAGMA journal_mode=WAL; CREATE TABLE credential (secret TEXT); INSERT INTO credential VALUES ('test-secret'); CREATE TABLE session (title TEXT); INSERT INTO session VALUES ('Keep this session');");
  db.close();
  fs.writeFileSync(path.join(data, "auth.json"), "{}");
  const keys = path.join(member.home, "box", "keys");
  fs.mkdirSync(keys, { recursive: true });
  fs.writeFileSync(path.join(keys, "TEST_KEY"), "test-key");

  let resident = member.start();
  const local = member.connect(await resident.ready());
  const key = (await local("GET", "kuvo/setup")).json.you.key;
  // An invalid journal blocks cleanup. The departing Rust computer must keep
  // its removal fence across a resident restart and resume once it can scrub.
  const journal = database + "-journal";
  if (!applicant.typescript) fs.mkdirSync(journal);
  const removed = await ask("POST", `kuvo/computers/${key}/unlink`, {}, fromMac);
  assert.equal(removed.status, 200, removed.text);
  if (!applicant.typescript) {
    await until(() => readJSON(path.join(member.home, "run", "link.json")).localUnlink,
      () => `The member did not save the unlink fence.\n${resident.log()}`);
    assert.equal(readJSON(path.join(main.home, "run", "link.json"))
      .shared.membershipAndAccess.value.removals[key].delivered, undefined);
    await resident.stop();
    fs.rmdirSync(journal);
    resident = member.start();
    await resident.ready();
  }
  await until(() => {
    const mainState = readJSON(path.join(main.home, "run", "link.json"));
    const state = readJSON(path.join(member.home, "run", "link.json"));
    return state.setup.role === "primary" && state.setup.id !== initial.setup
      && !state.localUnlink && mainState.shared.membershipAndAccess.value.removals[key].delivered;
  }, () => `Unlink did not finish.\n${resident.log()}`, 30000);
  assert.equal(fs.existsSync(keys), false);
  assert.equal(fs.existsSync(path.join(data, "auth.json")), false);
  const kept = new DatabaseSync(database, { readOnly: true });
  try {
    assert.deepEqual(kept.prepare("SELECT secret FROM credential").all(), []);
    assert.equal(kept.prepare("SELECT title FROM session").get().title, "Keep this session");
  } finally {
    kept.close();
  }
  // A departed computer that starts again must leave its released work untouched,
  // or it could no longer rejoin without a merge.
  await resident.stop();
  resident = member.start();
  const restarted = member.connect(await resident.ready());
  const outputs = await restarted("GET", "kuvo/artifacts/storage");
  assert.equal(outputs.status, 200, outputs.text);
  const next = await ask("POST", "kuvo/computers/add/code", {}, fromMac);
  assert.equal(next.status, 201, next.text);
  const joinWith = process.env.KUVO_CONFORMANCE_REJOIN_BIN
    ? boxBinary(process.env.KUVO_CONFORMANCE_REJOIN_BIN) : applicant;
  if (!joinWith.typescript) await resident.stop();
  const rejoined = await member.join(next.json.code, { resident: joinWith.typescript, joinWith });
  assert.equal(rejoined.code, 0, rejoined.log);
  const returned = readJSON(path.join(member.home, "run", "link.json"));
  assert.equal(returned.setup.id, initial.setup);
  assert.equal(returned.setup.role, "member");
  assert.notEqual(returned.key, key);
});
