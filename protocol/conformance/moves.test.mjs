// Both residents read the same move journals and outcomes through the client routes.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, openLine, readJSON, request, scratch, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { seedPeerSetup } from "./peer-fixture.mjs";

test("a restarted computer shows the held copy and answers durable move receipts", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("move-receipts");
  const save = (name, value) => write(path.join(here.home, "run/moves", name + ".json"), JSON.stringify(value));
  const from = "source", to = "destination";
  const move = (id, role, stage, sessions) => ({
    id, kind: "chat", thing: sessions[0], name: "Conversation", from, to, role, stage, sessions,
  });
  const done = move("mov_done", "from", "finishing", ["ses_done"]);
  save("mov_done.outcome", { formatVersion: 1, ok: true, move: done, note: "The original is retired." });
  const held = move("mov_held", "to", "copying", ["ses_held", "ses_child"]);
  save("mov_held", { ...held, formatVersion: 0, peer: from, started: 1, complete: true, setUpAt: Date.now() });
  save("mov_failed.outcome", { ok: false, status: 409, message: "The copy stayed here." });
  save("moved", [{ session: "ses_gone", key: to, at: 1, formatVersion: 0 }]);

  const resident = await here.start().ready();
  const receipt = id => request(resident, "GET", "kuvo/moves/receipt?id=" + id);
  const unavailable = { message: "The move’s result isn’t available. Check where it is before trying again." };
  for (const id of ["mov_missing", held.id]) {
    const reply = await receipt(id);
    assert.equal(reply.status, 404);
    assert.deepEqual(reply.json, unavailable);
  }
  const failed = await receipt("mov_failed");
  assert.equal(failed.status, 200);
  assert.deepEqual(failed.json, { state: "failed", message: "The copy stayed here." });
  const completed = await receipt(done.id);
  assert.equal(completed.status, 200);
  assert.deepEqual(completed.json, {
    state: "done", message: "Moved to the other computer.", note: "The original is retired.",
    move: { ...done, place: "the other computer", progress: "Finishing the move to the other computer…" },
  });
  const listing = await request(resident, "GET", "kuvo/moves");
  assert.equal(listing.status, 200);
  assert.deepEqual(listing.json, {
    pendingComputers: [], away: held.sessions,
    moves: [{ ...held, place: "the other computer", progress: "Moving to this computer…" }],
  });
});

test("a third computer sees a move until the other computer announces it is over", { timeout: 45000 }, async t => {
  const { computer, relay } = await bench(t);
  const observer = await computer("observer");
  const sender = await computer("sender");
  const { main, spare } = seedPeerSetup(sender.home, relay.url, 1, 3);
  seedPeerSetup(observer.home, relay.url, 2, 3);
  const journal = path.join(sender.home, "run/moves/mov_shown.json");
  const movement = {
    id: "mov_shown", kind: "chat", thing: "ses_shown", name: "Shown", from: spare.key, to: main.key,
    role: "to", stage: "copying", sessions: ["ses_shown"], sent: 30000, size: 30000,
  };
  write(journal, JSON.stringify({ ...movement,
    formatVersion: 1, peer: spare.key, started: Date.now(), complete: true, setUpAt: Date.now(),
  }));
  const [resident, sending] = await Promise.all([observer.start().ready(), sender.start().ready()]);
  const listing = async record => {
    const answer = await request(record, "GET", "kuvo/moves");
    assert.equal(answer.status, 200, answer.text);
    return answer.json;
  };
  const seen = await until(async () => (await listing(resident)).elsewhere, "The other computer did not announce the move", 18000);
  assert.deepEqual(seen, [{ box: main.key, moves: [{
    ...movement, place: spare.name, progress: "Moving to Studio…",
  }] }]);
  assert.equal((await listing(sending)).elsewhere, undefined);
  assert.deepEqual((await listing(resident)).away, []);
  assert.ok(!(await listing(resident)).pendingComputers?.some(computer => computer.id === main.key));

  const line = await openLine(resident, "", "kuvo/moves/events");
  t.after(() => line.close());
  assert.equal(line.status, 200);
  const updates = line.frames();
  assert.deepEqual((await updates.next()).value.elsewhere, seen);
  fs.unlinkSync(journal);
  let cleared = false;
  for await (const state of updates) {
    if (!state.elsewhere) {
      cleared = true;
      break;
    }
  }
  assert.equal(cleared, true, "A subscribed client hears that the move ended");
  assert.equal((await listing(resident)).elsewhere, undefined);
});

test("an old departure still routes the conversation to its owner", { timeout: 45000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("previous-owner");
  const { member, save } = seedPeerSetup(here.home, relay.url);
  const at = Date.now() - 3 * 86400000;
  save("run/moves/moved.json", [{ session: "ses_retired", key: member.key, at, formatVersion: 0 }]);
  save("run/peer-reach.json", { [member.key]: at });
  const resident = await here.start().ready();
  const answer = await request(resident, "GET", "kuvo/sessions/ses_retired/location");
  assert.equal(answer.status, 200, answer.text);
  assert.deepEqual(answer.json, { on: { id: member.key, name: member.name, away: true, actions: [] } });
  const listing = await request(resident, "GET", "kuvo/moves");
  assert.equal(listing.status, 200);
  assert.equal(listing.json.moved, undefined, "The old departure is no longer advertised");
});

const mixed = process.env.KUVO_CONFORMANCE_MOVE_PEER;
test("a TypeScript source recovers a lost switch acknowledgement from a restarted destination", {
  timeout: 45000, skip: !mixed && "Set KUVO_CONFORMANCE_MOVE_PEER to the Rust destination binary",
}, async t => {
  const { computer, relay } = await bench(t);
  const source = await computer("source");
  const destination = await computer("destination", boxBinary(mixed));
  const { main, member } = seedPeerSetup(source.home, relay.url);
  seedPeerSetup(destination.home, relay.url, 2);
  const old = Date.now() - 120000;
  const id = "mov_lostReply", session = "ses_lostReply";
  const bundle = { generation: 2, hash: "A".repeat(43) };
  const journal = {
    id, kind: "chat", thing: session, name: "A moved chat", from: main.key, to: member.key,
    role: "from", stage: "switching", step: "switch", peer: member.key, sessions: [session],
    wasPaused: false, tries: 0, started: old, sentAt: old, switchSince: old,
    twoComputers: true, safeSwitch: true, verifyPayloads: true, ...bundle, formatVersion: 1,
  };
  const decision = {
    as: "switched", at: old, peer: main.key, ...bundle, payloads: { sessions: [], queue: [] },
    formatVersion: 1,
  };
  const sourceJournal = path.join(source.home, "run/moves", id + ".json");
  const destinationReceipt = path.join(destination.home, "run/moves/decisions", id + ".json");
  write(sourceJournal, JSON.stringify(journal));
  write(destinationReceipt, JSON.stringify(decision));

  const before = destination.start();
  await before.ready();
  await before.stop();
  await destination.start().ready();
  const sourceProcess = source.start();
  const resident = await sourceProcess.ready();
  const receipt = () => request(resident, "GET", "kuvo/moves/receipt?id=" + id);
  const moved = await until(async () => {
    const answer = await receipt();
    assert.equal(answer.status, 200, answer.text);
    return answer.json.state === "done" && answer.json;
  }, () => "The source did not recover the switch acknowledgement\n" + sourceProcess.log(), 22000);
  assert.equal(moved.message, "Moved to Lab.");
  assert.equal(moved.move.stage, "finishing");
  assert.equal(readJSON(destinationReceipt).as, "switched");
  assert.deepEqual(readJSON(destinationReceipt), decision);
  assert.ok(readJSON(path.join(source.home, "run/moves/moved.json")).some(entry => (
    entry.session === session && entry.key === member.key
  )));
});

test("both residents create and clear the same peer reach files", {
  timeout: 90000, skip: !mixed && "Select both binaries to compare their reach files",
}, async t => {
  const { compare } = await import("./differential/compare.mjs");
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 }));
  const here = path.join(root, "here"), peer = path.join(root, "peer");
  const empty = path.join(root, "empty");
  const relay = "ws://127.0.0.1:1";
  const { member, save } = seedPeerSetup(here, relay);
  seedPeerSetup(peer, relay, 2);
  write(path.join(empty, "folder-version.json"), '{"version":23}');
  write(path.join(empty, "run/link.json"), '{"name":"Studio","phones":[]}');
  const binaries = [boxBinary(), boxBinary(mixed)];
  const step = state => ({ method: "GET", path: "kuvo/moves/receipt?id=mov_absent", peerReach: { key: member.key, state } });
  const check = async (home, sequence) => {
    const differences = await compare(home, sequence, ...binaries, { peerSeed: peer });
    assert.deepEqual(differences.filter(line => /^(request |home\/run\/peer-reach)/.test(line)), []);
  };
  await check(empty, [step("absent")]);
  await check(here, [step("away"), step("answered")]);
  // Older readers allow any numeric timestamp. Changing Lab must preserve another entry.
  save("run/peer-reach.json", { kept: 1000.5 });
  save("run/peer-reach-checked.json", { kept: 1100.5 });
  await check(here, [step("away"), step("answered")]);
});
