// The main and departing computer use real residents. The private relay lets
// the test hold the removed computer offline while the main commits.
import assert from "node:assert/strict";
import path from "node:path";
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

test("a TypeScript computer completes the signed unlink handshake", {
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
  const resident = other.start();
  const local = other.connect(await resident.ready());
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
});
