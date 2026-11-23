// The main and departing computer use real residents. The private relay lets
// the test hold the removed computer offline while the main commits.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { boxBinary, readJSON, until } from "./adapter.mjs";
import { bench } from "./bench.mjs";

const fromMac = { "x-kuvo-from": "mac" };
const applicant = process.env.KUVO_CONFORMANCE_JOIN_BIN
  ? boxBinary(process.env.KUVO_CONFORMANCE_JOIN_BIN) : boxBinary();

test("the main revokes an offline computer once and keeps the unlink receipt after restart", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const other = await computer("Lab", applicant);
  const mainProcess = main.start();
  const ask = main.connect(await mainProcess.ready());
  const original = await ask("GET", "kuvo/setup");
  assert.equal(original.status, 200, original.text);
  const { setup, you } = original.json;
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
  const code = await ask("POST", "kuvo/computers/add/code", {}, fromMac);
  assert.equal(code.status, 201, code.text);
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

test("a released computer enrolls again after usage settings follow its standalone setup", {
  timeout: 120000,
  skip: !applicant.typescript,
}, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const other = await computer("Lab", applicant);
  const ask = main.connect(await main.start().ready());
  const initial = await ask("GET", "kuvo/setup");
  const setup = initial.json.setup;
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
  const invite = () => ask("POST", "kuvo/computers/add/code", {}, fromMac);
  const firstCode = await invite();
  assert.equal(firstCode.status, 201, firstCode.text);
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
  await until(() => usageScope() === setup, "Usage settings did not follow the initial setup.");
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
  await until(() => usageScope() === kept.json.setup, "Usage settings did not follow the standalone setup.");
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
  await until(() => leases.leaseFor(initial.setup), "The main computer did not claim a relay lease.");
  const offered = await ask("POST", "kuvo/computers/add/code", {}, fromMac);
  assert.equal(offered.status, 201, offered.text);
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
