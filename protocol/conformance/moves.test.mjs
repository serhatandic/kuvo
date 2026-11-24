// Both residents read the same move journals and outcomes through the client routes.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { boxBinary, isolatedEnv, openLine, readJSON, request, scratch, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { seedPeerSetup } from "./peer-fixture.mjs";
import { sessionOf } from "./engine.mjs";

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
  const other = await computer("other-party");
  const { main, spare } = seedPeerSetup(sender.home, relay.url, 1, 3);
  seedPeerSetup(observer.home, relay.url, 2, 3);
  seedPeerSetup(other.home, relay.url, 3, 3);
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
  const otherJournal = path.join(other.home, "run/moves/mov_shown.json");
  write(otherJournal, JSON.stringify({ ...movement,
    role: "from", stage: "switching", step: "switch", formatVersion: 1, peer: main.key,
    started: Date.now(), switchSince: Date.now(),
  }));
  await other.start().ready();
  async function nextWhere(matches) {
    for (;;) {
      const { value, done } = await updates.next();
      assert.equal(done, false, "The move stream ended before announcing the change");
      if (matches(value.elsewhere ?? [])) return value;
    }
  }
  const both = await nextWhere(rows => rows.length === 2);
  assert.deepEqual(both.elsewhere.find(row => row.box === main.key), seen[0]);
  const departing = both.elsewhere.find(row => row.box === spare.key);
  assert.equal(departing.moves[0].id, movement.id);
  assert.equal(departing.moves[0].role, "from");
  fs.unlinkSync(journal);
  const remaining = await nextWhere(rows => rows.length === 1 && rows[0].box === spare.key);
  assert.deepEqual(remaining.elsewhere, [departing]);
  fs.unlinkSync(otherJournal);
  await nextWhere(rows => rows.length === 0);
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

test("a newer move announcement leads to the next computer until it expires", { timeout: 45000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("previous-owner"), there = await computer("newest-owner");
  const { main, member, spare, save } = seedPeerSetup(here.home, relay.url, 1, 3);
  seedPeerSetup(there.home, relay.url, 3, 3);
  const now = Date.now();
  const recent = "ses_recentHop", expired = "ses_expiredHop", local = "ses_localHop";
  save("run/moves/moved.json", [recent, expired].map(session => ({
    session, key: member.key, at: now - 60 * 60_000, formatVersion: 0,
  })));
  save("run/peer-agents.json", { [member.key]: { at: now, kind: member.kind, list: [], moved: [
    { session: recent, key: spare.key, at: now },
    { session: expired, key: spare.key, at: now - 3 * 86400000 },
    { session: local, key: spare.key, at: now },
  ] } });
  here.engine.sessions.push(sessionOf({ id: local, title: "Trip plans", directory: path.join(here.home, "chats") }));
  there.engine.sessions.push(sessionOf({ id: recent, directory: path.join(there.home, "chats") }));
  const [resident, target] = await Promise.all([here.start().ready(), there.start().ready()]);
  const locate = id => request(resident, "GET", "kuvo/sessions/" + id + "/location");
  const own = await until(async () => {
    const result = await locate(local);
    return result.json?.place && result;
  }, "The resident did not list its own conversation");
  assert.equal(own.json.place, "engine", own.text);
  for (const [id, peer] of [[recent, spare], [expired, member]]) {
    const result = await locate(id);
    assert.equal(result.status, 200, result.text);
    assert.equal(result.json.on?.id, peer.key, result.text);
  }
  await until(async () => (await request(target, "GET", "api/session/" + recent)).status === 200 &&
    fs.existsSync(path.join(there.home, "run/peer-agents.json")) &&
    readJSON(path.join(there.home, "run/peer-agents.json"))[main.key]?.list.some(row => row.session === local),
  "The new owner did not learn the letter's sender");
  const letter = path.join(here.home, "run/outbox/sending/ltr_followSecondMove.json");
  write(letter, JSON.stringify({ from: local, to: recent, text: "What changed?", peer: member.key, wait: false, at: Date.now() }));
  const received = await until(() => there.engine.messages.get(recent), "The letter did not follow the newer announcement", 15000);
  assert.equal(received.length, 1);
  assert.match(received[0].text, /^\[Message from Trip plans · ses_localHop\][^\n]*\n\nWhat changed\?$/);
});

const mixed = process.env.KUVO_CONFORMANCE_MOVE_PEER;

async function testMail(t, binary) {
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 }));
  // The bundled crypto is the same implementation a TypeScript peer uses.
  // Its module initialization reads the home once, so give it a scratch home.
  const isolated = { ...isolatedEnv(root), KUVO_HOME: path.join(root, "box") };
  const before = Object.fromEntries(Object.keys(isolated).map(key => [key, process.env[key]]));
  Object.assign(process.env, isolated);
  try {
    return await import(pathToFileURL(binary.file).href);
  } finally {
    for (const [key, value] of Object.entries(before)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
}

function cutMoveMail(relay, opens, source, destination, main, member) {
  const next = new Map();
  const held = new Set(["check", "checked", "offer", "ready", "part", "got", "working", "arrive", "arrived", "switch", "switched"]);
  const addresses = [main, member].map(peer => Buffer.from(peer.address, "base64url"));
  const peers = [source, destination];
  const keys = [main, member].map(peer => Buffer.from(peer.key, "base64url"));
  const privateKeys = peers.map(peer => Buffer.from(readJSON(path.join(peer.home, "run/link.json")).key, "base64url"));
  const remove = addresses.map((from, i) => relay.intercept(from, addresses[1 - i], packet => {
    if (packet.payload[0] !== 5) return false;
    try {
      const opened = opens(packet.payload.subarray(5), privateKeys[1 - i], keys[i]);
      const op = JSON.parse(opened.payload.toString()).move?.op;
      if (!held.has(op)) return false;
      if (!next.has(op)) next.set(op, packet);
      return true;
    } catch { return false; }
  }));
  const ready = () => until(() => addresses.every(address => relay.has(address)), "Both computers did not reconnect to the relay", 12000);
  return {
    async resume() {
      for (const op of held) {
        await until(() => next.has(op), "The live transfer did not reach " + op, op === "arrive" ? 70000 : 20000);
        if (op === "switched") break;
        if (op === "got") {
          next.delete(op);
          await until(() => next.has(op), "The source did not resend the unacknowledged part", 30000);
        }
        held.delete(op);
        if (op === "arrive") next.delete("arrived");
        relay.replay(next.get(op));
      }
    },
    async recoverSwitch(waitForOwner) {
      await ready();
      next.delete("switched");
      const reconnect = relay.disconnect(addresses[0]);
      try {
        await until(() => !relay.has(addresses[0]), "The source relay connection did not close");
      } finally {
        reconnect();
      }
      // The original acknowledgement stays lost. A new switch request from
      // the reconnecting source must recover it from the durable receipt.
      await ready();
      await waitForOwner();
      await until(() => next.has("switched"), "The restarted destination did not answer the retried switch");
      held.delete("switched");
      relay.replay(next.get("switched"));
    },
    close() { for (const cancel of remove) cancel(); },
  };
}

test("a held move fences prompt, command, and queue writes after a restart", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("write-fence");
  const id = "ses_heldWrite";
  here.engine.sessions.push(sessionOf({ id, directory: path.join(here.home, "chats") }));
  const pathToJournal = path.join(here.home, "run/moves/mov_writeFence.json");
  const journal = { formatVersion: 1, id: "mov_writeFence", role: "to", stage: "copying", kind: "chat", thing: id,
    from: "other", to: "local", peer: "other", sessions: [id], started: Date.now(), safeSwitch: true,
    complete: true, setUpAt: Date.now() };
  write(pathToJournal, JSON.stringify(journal));
  const first = here.start();
  await first.ready();
  await first.stop();
  const restarted = await here.start().ready();
  const engine = "api/session/" + id;
  const kuvo = "kuvo/sessions/" + id;
  const moving = "This record is held while its owner changes.";
  for (const [route, body] of [[engine + "/prompt", { text: "Held" }],
    [engine + "/command", { command: "test" }], [kuvo + "/queue/resume", {}],
    [kuvo + "/send", { id: "msg_refused", words: "Do not write this while held." }]]) {
    const reply = await request(restarted, "POST", route, body);
    assert.equal(reply.status, 409, reply.text);
    assert.equal(reply.json.message, moving, route);
  }
  assert.ok(!here.engine.asked.some(call => call.method === "POST" && call.path.startsWith("/" + engine)));
  fs.unlinkSync(pathToJournal);
  const unheld = await request(restarted, "POST", engine + "/prompt", { id: "msg_afterFence", text: "Released" });
  assert.equal(unheld.status, 200, unheld.text);
  assert.equal(here.engine.messages.get(id)?.[0]?.id, "msg_afterFence");
});

const binary = boxBinary();
const wireFixture = fileURLToPath(new URL("../../box/dist/kuvo-link.js", import.meta.url));

async function readyToMove(resident, session, peer) {
  await until(async () => {
    const reply = await request(resident, "GET", "kuvo/moves/places?kind=chat&id=" + session);
    assert.equal(reply.status, 200, reply.text);
    return reply.json.places.some(place => place.key === peer.key && place.ok);
  }, () => `The move to ${peer.name} did not become available`, 15000, 100);
}

// Observe the durable outcome and the removal of the active journal. A receipt
// can already say "done" while the old computer is still retiring the folder.
async function moveRetired(computer, id, logs) {
  const directory = path.join(computer.home, "run/moves");
  const outcome = path.join(directory, id + ".outcome.json");
  const journal = path.join(directory, id + ".json");
  try {
    await new Promise((resolve, reject) => {
      const check = () => {
        if (fs.existsSync(outcome) && !fs.existsSync(journal)) done(resolve);
      };
      const changes = fs.watch(directory, check);
      const deadline = setTimeout(() => done(reject, new Error("Timed out waiting for move retirement")), 20000);
      function done(settle, result) {
        clearTimeout(deadline);
        changes.close();
        settle(result);
      }
      changes.on("error", error => done(reject, error));
      check();
    });
  } catch (error) {
    const active = fs.existsSync(journal) ? readJSON(journal) : null;
    throw new Error(`The move ${id} did not settle (${active?.stage}/${active?.step})\n${logs()}`, { cause: error });
  }
}

test("a resident keeps the saved draft pending while a recovered move holds its session", {
  timeout: 45000, skip: binary.typescript && "The standalone draft outbox is the Rust session worker",
}, async t => {
  const { computer } = await bench(t);
  const here = await computer("held-draft");
  const session = "ses_heldDraft";
  const draft = { id: "msg_heldDraft", text: "Keep it until this session can take it.", message: {} };
  here.engine.sessions.push(sessionOf({ id: session, directory: path.join(here.home, "chats") }));
  const journal = path.join(here.home, "run/moves/mov_heldDraft.json");
  write(journal, JSON.stringify({ formatVersion: 1, id: "mov_heldDraft", role: "to", stage: "copying", kind: "chat", thing: session,
    from: "other", to: "local", peer: "other", sessions: [session], started: Date.now(), safeSwitch: true,
    complete: true, setUpAt: Date.now() }));
  const first = here.start();
  const headers = { "x-kuvo-from": "app" };
  const answer = await request(await first.ready(), "POST", `kuvo/sessions/${session}/delivery?computer=here`, draft, headers);
  assert.equal(answer.status, 202, answer.text);
  assert.equal(answer.json.status, "pending");
  await first.stop();
  const restarted = await here.start().ready();
  const claim = path.join(here.home, "run/draft-receipts");
  await until(() => fs.existsSync(claim) && fs.readdirSync(claim).some(name => name.endsWith(".json")), "The worker never tried the saved draft");
  assert.ok(!here.engine.asked.some(call => call.method === "POST" && call.path === `/api/session/${session}/prompt`));
  const receipt = `kuvo/data/messages/${answer.json.operation}`;
  assert.equal((await request(restarted, "GET", receipt, undefined, headers)).json.status, "pending");
  fs.unlinkSync(journal);
  await until(async () => (await request(restarted, "GET", receipt, undefined, headers)).json?.status === "committed",
    "The saved draft did not reach the released session", 12000);
  assert.deepEqual(here.engine.messages.get(session).map(row => row.id), [draft.id]);
});

test("a source resumes the original only after the destination proves it dropped its copy", {
  timeout: 60000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("drop-proof-source"), there = await computer("drop-proof-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_dropProof", directory = path.join(here.home, "chats");
  const pause = path.join(here.home, "wakes", session, "paused");
  write(pause, "user-paused");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, title: "Trip plans", directory }));
  const from = Buffer.from(main.address, "base64url"), to = Buffer.from(member.address, "base64url");
  const { sealOneWay, openOneWay, Frame, frame } = await testMail(t, boxBinary(wireFixture));
  const key = peer => Buffer.from(peer.key, "base64url");
  let switching, dropProof;
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), key(main));
    const move = JSON.parse(payload).move;
    if (move?.op !== "switch") return false;
    switching = move;
    return true;
  }));
  const releaseProof = relay.intercept(to, from, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 1), key(member));
    if (JSON.parse(payload).move?.op !== "dropped") return false;
    dropProof = packet;
    return true;
  });
  t.after(releaseProof);
  const sourceProcess = here.start(), destinationProcess = there.start();
  const [source] = await Promise.all([sourceProcess.ready(), destinationProcess.ready()]);
  await readyToMove(source, session, member);
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const { id, generation, hash } = await until(() => switching, "The source did not attempt to switch");
  const send = (a, b, secret, receiver, move) => {
    const payload = Buffer.from(JSON.stringify({ at: Date.now(), move }));
    const { message } = sealOneWay(payload, Buffer.alloc(32, secret), key(receiver));
    relay.replay({ from: a, to: b, payload: frame(Frame.mail, 0, message) });
  };
  send(to, from, 2, main, { op: "dropped", id, generation: generation + 1, hash, neverSwitched: true });
  send(from, to, 1, member, { op: "drop", id, generation, hash });
  await until(() => dropProof, "The destination did not prove its copy was dropped");
  const route = "kuvo/moves/receipt?id=" + id;
  assert.equal((await request(source, "GET", route)).json.state, "moving");
  assert.equal(fs.readFileSync(pause, "utf8"), "moving:" + id);

  releaseProof();
  relay.replay(dropProof);
  await moveRetired(here, id, () => sourceProcess.log() + "\n" + destinationProcess.log());
  const result = await request(source, "GET", route);
  assert.deepEqual(result.json, { state: "failed", message: "“Trip plans” couldn’t be set up there, so it stayed here. Try again." });
  assert.equal(fs.readFileSync(pause, "utf8"), "user-paused");
  assert.ok(here.engine.sessions.some(row => row.id === session));
  await until(() => !there.engine.sessions.some(row => row.id === session), "The destination did not remove the dropped copy");
});

test("a destination rolls back the import that lost its engine reply during restart", { timeout: 105000 }, async t => {
  const { computer, relay } = await bench(t);
  const source = await computer("aborted-source"), destination = await computer("aborted-destination");
  const { main, member } = seedPeerSetup(source.home, relay.url);
  seedPeerSetup(destination.home, relay.url, 2);
  const root = "ses_lostImport";
  const chats = path.join(source.home, "chats");
  fs.mkdirSync(chats, { recursive: true });
  source.engine.sessions.push(sessionOf({ id: root, directory: chats }));
  let accepted = false;
  destination.engine.holdImport = { id: root, accepted: () => { accepted = true; } };
  const srcProcess = source.start(), destinationBefore = destination.start();
  const [src] = await Promise.all([srcProcess.ready(), destinationBefore.ready()]);
  await until(() => {
    try { return readJSON(path.join(source.home, "run/peer-agents.json"))[member.key]?.features?.includes("moves.two-computers"); }
    catch { return false; }
  }, "The source did not hear from the destination", 15000);
  await readyToMove(src, root, member);
  const begun = await request(src, "POST", "kuvo/moves", { kind: "chat", id: root, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const move = begun.json.move.id;
  await until(() => accepted, "The destination engine did not accept the import before interruption", 12000);
  assert.ok(destination.engine.sessions.some(row => row.id === root));
  await destinationBefore.stop();
  await destination.start().ready();
  await until(() => !destination.engine.sessions.some(row => row.id === root), "The interrupted copy is still in the destination engine", 15000);
  const proof = readJSON(path.join(destination.home, "run/moves/decisions", move + ".json"));
  assert.equal(proof.as, "dropped");
  assert.equal(proof.peer, main.key);
  await until(async () => (await request(src, "GET", "kuvo/moves/receipt?id=" + move)).json?.state === "failed",
    "The source did not receive the failed move outcome after retrying arrival", 70000);
  assert.ok(source.engine.sessions.some(row => row.id === root));
});

test("a restarted source drives a request saved before its first check", { timeout: 45000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("requested-source"), there = await computer("requested-target");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const id = "mov_recentRequest", session = "ses_recentRequest";
  const directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, directory }));
  const destination = await there.start().ready();
  assert.equal((await request(destination, "GET", "kuvo/moves")).status, 200);
  save("run/moves/" + id + ".json", {
    id, thing: session, name: "A saved request", kind: "chat", from: main.key,
    to: member.key, peer: member.key, role: "from", stage: "holding", sessions: [session],
    step: "check", started: Date.now(), wasPaused: false, tries: 0, twoComputers: true, formatVersion: 1,
  });
  const source = await here.start().ready();
  const completed = await until(async () => {
    const reply = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
    return ["done", "failed"].includes(reply.json?.state) && reply.json;
  }, "The resident did not drive the saved request", 20000);
  assert.equal(completed.state, "done", JSON.stringify(completed));
  assert.ok(there.engine.sessions.some(record => record.id === session));
  await until(() => !here.engine.sessions.some(record => record.id === session), "The retired source was not removed");
});

test("a switched move keeps the original until its backup can be written", {
  timeout: 60000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("cleanup-source"), there = await computer("cleanup-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_cleanupBackup", directory = path.join(here.home, "chats");
  const probe = "mov_cleanupRound";
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, directory }));
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const from = Buffer.from(main.address, "base64url"), to = Buffer.from(member.address, "base64url");
  const key = Buffer.from(member.key, "base64url"), secret = Buffer.alloc(32, 1);
  const exports = () => here.engine.asked.filter(call =>
    call.method === "GET" && call.path === "/api/experimental/session/" + session + "/export").length;
  let beforeCleanup, checked = false;
  const stop = relay.intercept(to, from, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), secret, key);
    if (JSON.parse(payload).move?.op === "switched") {
      beforeCleanup ??= exports();
      here.engine.refuseExport = session;
    }
    return false;
  });
  t.after(stop);
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), Buffer.from(main.key, "base64url"));
    const move = JSON.parse(payload).move;
    if (move?.op !== "check" || move.id !== probe) return false;
    checked = true;
    return true;
  }));
  const sourceProcess = here.start(), targetProcess = there.start();
  const [source] = await Promise.all([sourceProcess.ready(), targetProcess.ready()]);
  await readyToMove(source, session, member);
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const id = begun.json.move.id;
  await until(() => fs.existsSync(path.join(here.home, "run/moves", id + ".outcome.json")),
    "The source did not record its cleanup outcome", 15000);
  const outcome = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
  assert.equal(outcome.status, 200, outcome.text);
  assert.equal(outcome.json.state, "done", outcome.text);
  assert.equal(outcome.json.note, "The original is retired. Kuvo will try again to clean up its files on the source computer.");
  assert.ok(here.engine.sessions.some(record => record.id === session));
  assert.ok(!here.engine.asked.some(call => call.method === "DELETE" && call.path.endsWith("/" + session)));
  assert.ok(there.engine.sessions.some(record => record.id === session));
  const location = await request(source, "GET", `kuvo/sessions/${session}/location`);
  assert.equal(location.status, 200, location.text);
  assert.equal(location.json.on?.id, member.key, location.text);

  // A later saved request proves the same driver took another look at the
  // move journals. The failed backup should wait for its retry interval.
  const probeFile = path.join(here.home, "run/moves", probe + ".json");
  write(probeFile, JSON.stringify({
    formatVersion: 1, id: probe, thing: "ses_cleanupProbe", name: "Another request", kind: "chat",
    from: main.key, to: member.key, peer: member.key, role: "from", stage: "holding",
    sessions: ["ses_cleanupProbe"], step: "check", started: Date.now(), tries: 0, twoComputers: true,
  }));
  await until(() => checked, "The source did not reach its next move round");
  assert.equal(exports() - beforeCleanup, 1, "The next round must not immediately retry the failed export");
  fs.unlinkSync(probeFile);
  await sourceProcess.stop();
  here.engine.refuseExport = undefined;
  const restarted = here.start();
  const resident = await restarted.ready();
  await moveRetired(here, id, () => restarted.log() + "\n" + targetProcess.log());
  assert.equal(here.engine.sessions.some(record => record.id === session), false);
  const backup = readJSON(path.join(here.home, "run/moves/left.json")).find(row => row.id === id);
  assert.equal(readJSON(path.join(backup.trash, "conversation.json"))[0].info.id, session);
  const final = await request(resident, "GET", "kuvo/moves/receipt?id=" + id);
  assert.equal(final.status, 200, final.text);
  assert.equal(final.json.state, "done", final.text);
  assert.equal(final.json.note, undefined, final.text);
});

async function moveMail(t, relay, main, remote = 2) {
  const from = Buffer.alloc(32, remote), to = Buffer.from(main.key, "base64url");
  const address = Buffer.from(main.address, "base64url");
  const sender = relay.device(Buffer.alloc(32, 50 + remote));
  t.after(() => sender.leave());
  const { sealOneWay, openOneWay, readFrame, frame, Frame } = await testMail(t, boxBinary(wireFixture));
  const receive = async () => {
    const datagram = readFrame((await sender.next(20000)).subarray(17));
    return datagram?.kind === Frame.mail && JSON.parse(openOneWay(datagram.body, from, to).payload);
  };
  let welcomed = false;
  return async (movement, expectReply = true) => {
    if (!welcomed) {
      await until(async () => (await receive())?.agents, "The destination did not announce itself to the peer", 15000);
      welcomed = true;
    }
    const { message } = sealOneWay(Buffer.from(JSON.stringify({ formatVersion: 1, at: Date.now(), move: movement })), from, to);
    sender.send(address, frame(Frame.mail, 0, message));
    if (!expectReply) return;
    return until(async () => {
      const answer = (await receive())?.move;
      return answer?.id === movement.id && answer;
    }, () => "The destination never answered " + movement.id, 20000);
  };
}

test("a destination reports the same held copy and refuses an unknown arrival", {
  timeout: 45000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("arrival-query");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const id = "mov_recordedArrival", unknown = "mov_unknownArrival";
  const session = "ses_recordedArrival", hash = "A".repeat(43);
  save("run/moves/" + id + ".json", {
    formatVersion: 1, id, thing: session, name: "Notes", kind: "chat", from: member.key,
    to: main.key, peer: member.key, role: "to", stage: "copying", sessions: [session],
    started: Date.now(), safeSwitch: true, twoComputers: true, complete: true, setUpAt: Date.now(),
    generation: 1, hash, verifyPayloads: true, payloads: { sessions: [], queue: [] },
  });
  here.engine.sessions.push(sessionOf({ id: session, directory: path.join(here.home, "chats") }));
  await here.start().ready();
  assert.deepEqual(await ask({ op: "arrive", id, generation: 1, hash }),
    { op: "arrived", id, generation: 1, hash, verifiedPayloads: true });
  const refused = await ask({ op: "arrive", id: unknown, generation: 1, hash });
  assert.equal(refused.op, "failed");
  assert.equal(refused.generation, 1);
  assert.equal(refused.hash, hash);
});

test("a dropped copy stays held through an engine failure and a late switch", {
  timeout: 65000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("held-drop");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const id = "mov_droppedCopy", session = "ses_droppedCopy", hash = "A".repeat(43), generation = 1;
  const journal = "run/moves/" + id + ".json";
  const folder = path.join(here.home, "wakes", session);
  const paused = path.join(folder, "paused");
  write(paused, "moving:" + id);
  const imported = sessionOf({ id: session, directory: path.join(here.home, "chats") });
  imported.metadata = { "kuvo.moveImport": `${id}:${generation}:${hash}` };
  here.engine.sessions.push(imported);
  here.engine.refuseDeletes.add(session);
  save(journal, {
    formatVersion: 1, id, thing: session, name: "Held copy", kind: "chat", from: member.key,
    to: main.key, peer: member.key, role: "to", stage: "copying", sessions: [session],
    started: Date.now(), safeSwitch: true, twoComputers: true, complete: true, setUpAt: Date.now(),
    generation, hash, createdSessions: [session], placed: true, folder,
  });
  const process = here.start(), resident = await process.ready();
  const drop = { op: "drop", id, generation, hash };
  const proof = await ask(drop);
  assert.equal(proof.op, "dropped");
  assert.equal(proof.neverSwitched, true);
  const remove = call => call.method === "DELETE" && call.path === "/api/session/" + session;
  await until(() => here.engine.asked.some(remove), "The destination did not try to delete the held import");
  assert.ok(here.engine.sessions.some(record => record.id === session));
  const list = await request(resident, "GET", "kuvo/moves");
  assert.equal(list.status, 200, list.text);
  assert.deepEqual(list.json.moves, []);
  assert.ok(list.json.away.includes(session));
  assert.equal(fs.readFileSync(paused, "utf8"), "moving:" + id);
  assert.deepEqual(await ask({ ...drop, op: "switch", safeSwitch: true }), proof);
  const offer = { ...drop, op: "offer", kind: "chat", thing: session, name: "Held copy",
    size: 30000, safeSwitch: true, twoComputers: true };
  const repeated = await ask(offer);
  assert.equal(repeated.op, "refused");
  assert.equal(repeated.message, `An earlier copy of Held copy on ${member.name} couldn’t be let go of yet. Try again in a moment.`);
  const retry = "mov_reofferedCopy";
  const refused = await ask({ ...offer, id: retry });
  assert.equal(refused.op, "refused");
  assert.equal(refused.message, `An earlier copy of Held copy on ${main.name} couldn’t be let go of yet. Try again in a moment.`);
  assert.equal(fs.existsSync(path.join(here.home, "run/moves", retry + ".json")), false);
  const blocked = await request(resident, "POST", "api/session/" + session + "/prompt", { text: "Stay held." });
  assert.equal(blocked.status, 409, blocked.text);
  here.engine.refuseDeletes.delete(session);
  const admitted = await ask({ ...offer, id: retry });
  assert.equal(admitted.op, "ready", JSON.stringify(admitted));
  await until(() => !fs.existsSync(path.join(here.home, journal)) && !fs.existsSync(folder),
    () => "The destination did not clean up the rejected copy\n" + process.log());
  assert.equal(here.engine.sessions.some(record => record.id === session), false);
  assert.deepEqual(await ask(drop), proof);
  assert.equal((await ask({ ...drop, id: retry })).op, "dropped");
});

test("a second computer cannot take over a held incoming move", {
  timeout: 45000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("contended-copy");
  const { main, member, spare } = seedPeerSetup(here.home, relay.url, 1, 3);
  const ask = await moveMail(t, relay, main), other = await moveMail(t, relay, main, 3);
  await here.start().ready();
  const id = "mov_contendedCopy", next = "mov_contendingCopy", session = "ses_contendedCopy";
  const hash = "A".repeat(43);
  const offer = { op: "offer", id, generation: 1, hash, kind: "chat", thing: session, name: "Notes",
    size: 30000, safeSwitch: true, twoComputers: true };
  assert.equal((await ask({ ...offer, from: member.key })).op, "ready");
  const journal = path.join(here.home, "run/moves", id + ".json");
  for (const conflicting of [id, next]) {
    const response = await other({ ...offer, id: conflicting, from: spare.key });
    assert.equal(response.op, "refused");
    assert.equal(response.message, "It’s moving to another computer. Try again once it’s there.");
    assert.equal(readJSON(journal).peer, member.key);
  }
  assert.equal(fs.existsSync(path.join(here.home, "run/moves", next + ".json")), false);
  assert.equal((await ask({ op: "drop", id, generation: 1, hash })).op, "dropped");
  await until(() => !fs.existsSync(journal), "The first computer's copy did not retire");
  assert.equal((await other({ ...offer, id: next, from: spare.key })).op, "ready");
  assert.equal(readJSON(path.join(here.home, "run/moves", next + ".json")).peer, spare.key);
  assert.equal(here.engine.sessions.length, 0);

  const local = "ses_alreadyHere", overwrite = "mov_localOverwrite";
  const kept = sessionOf({ id: local, title: "Local original", directory: path.join(here.home, "chats") });
  here.engine.sessions.push(kept);
  const refused = await other({ ...offer, from: spare.key, id: overwrite, thing: local });
  assert.equal(refused.op, "refused");
  assert.equal(refused.message, `Notes is on ${main.name} already.`);
  assert.equal(fs.existsSync(path.join(here.home, "run/moves", overwrite + ".json")), false);
  assert.deepEqual(here.engine.sessions, [kept]);
});

test("the week-long cleanup warning dates from the drop and keeps the copy held", {
  timeout: 40000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("expired-drop");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const starting = Date.now() - 8 * 86400000, generation = 1, hash = "A".repeat(43);
  const stuck = "ses_stuckDrop", fresh = "ses_freshDrop";
  for (const [session, id, name, dropSince] of [
    [stuck, "mov_stuckDrop", "Week-old cleanup", starting],
    [fresh, "mov_freshDrop", "New cleanup", Date.now()],
  ]) {
    const imported = sessionOf({ id: session, directory: path.join(here.home, "chats") });
    imported.metadata = { "kuvo.moveImport": `${id}:${generation}:${hash}` };
    here.engine.sessions.push(imported);
    here.engine.refuseDeletes.add(session);
    save("run/moves/" + id + ".json", {
      formatVersion: 1, id, thing: session, name, kind: "chat", from: member.key, to: main.key,
      peer: member.key, role: "to", stage: "copying", sessions: [session], createdSessions: [session],
      started: starting, dropSince, complete: true, setUpAt: starting, dropping: true, safeSwitch: true,
      twoComputers: true, generation, hash,
    });
  }
  const process = here.start(), resident = await process.ready();
  const warning = /a copy of Week-old cleanup that came here couldn’t be let go of in a week/g;
  await until(() => process.log().match(warning)?.length, () => "The old drop did not report its warning\n" + process.log(), 12000);
  const listing = await request(resident, "GET", "kuvo/moves");
  assert.deepEqual([...listing.json.away].sort(), [fresh, stuck].sort());
  assert.deepEqual(listing.json.moves, []);
  assert.ok(!process.log().includes("a copy of New cleanup"));
  assert.equal(process.log().match(warning).length, 1);
  here.engine.refuseDeletes.clear();
  for (const id of ["mov_stuckDrop", "mov_freshDrop"]) {
    assert.equal((await ask({ op: "drop", id, generation, hash })).op, "dropped");
  }
  await until(() => here.engine.sessions.length === 0, "The held copies were not removed once the engine recovered");
  assert.equal(process.log().match(warning).length, 1);
});

test("a retired backup waits for its new owner's first run and any move back", {
  timeout: 50000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("retired-backup");
  const { main, member, save } = seedPeerSetup(here.home, relay.url, 1, 3);
  const owner = await moveMail(t, relay, main), other = await moveMail(t, relay, main, 3);
  const old = "mov_retiredBackup", session = "ses_returningBackup";
  const recent = "mov_recentBackup", recentSession = "ses_recentBackup";
  const at = Date.now() - 8 * 86_400_000, recentAt = Date.now() - 86_400_000;
  const backup = (id, thing, at) => {
    const trash = path.join(here.home, "trash", id, thing);
    write(path.join(trash, "conversation.json"), "[]");
    return { id, thing, to: member.key, kind: "chat", sessions: [thing], at, trash };
  };
  const saved = backup(old, session, at), fresh = backup(recent, recentSession, recentAt);
  const left = "run/moves/left.json";
  save(left, [saved, fresh]);
  const rows = () => readJSON(path.join(here.home, left));
  const processBox = here.start();
  const resident = await processBox.ready();
  await until(async () => (await request(resident, "GET", "api/session")).status === 200,
    "The destination's engine did not become ready");
  await other({ id: old, op: "ran", at }, false);
  assert.equal((await other({ id: "mov_unrelatedCheck", op: "arrive", generation: 1, hash: "A".repeat(43) })).op, "failed");
  assert.equal(rows().find(row => row.id === old).ranAt, undefined);
  assert.ok(fs.existsSync(saved.trash), "Another computer cannot start the backup's expiry");

  const move = { id: "mov_returningBackup", generation: 1, hash: Buffer.alloc(32, 1).toString("base64url") };
  const offer = { ...move, op: "offer", kind: "chat", thing: session, name: "Returning chat",
    size: 30000, safeSwitch: true, twoComputers: true };
  const offered = await owner(offer);
  assert.equal(offered.op, "ready", JSON.stringify(offered));
  await owner({ id: old, op: "ran", at }, false);
  await owner({ id: recent, op: "ran", at }, false);
  await until(() => rows().every(row => row.ranAt !== undefined), "The source did not learn that the chats ran");
  assert.equal(rows().find(row => row.id === recent).ranAt, recentAt, "A first run cannot predate the move");
  assert.ok(fs.existsSync(saved.trash), "A move back still needs the old backup");
  const blockDeletion = process.platform !== "win32" && process.getuid() !== 0;
  const container = path.dirname(saved.trash);
  if (blockDeletion) fs.chmodSync(container, 0o500);
  try {
    assert.equal((await owner({ ...move, op: "drop" })).op, "dropped");
    if (blockDeletion) {
      await until(() => /Permission denied|EACCES/.test(processBox.log()),
        () => "The source did not attempt to remove the expired backup\n" + processBox.log());
      assert.ok(rows().some(row => row.id === old), "A failed deletion leaves a record to retry");
    }
  } finally {
    if (blockDeletion) fs.chmodSync(container, 0o700);
  }
  await until(() => !fs.existsSync(saved.trash) && !rows().some(row => row.id === old),
    "An expired backup was not removed after the move back was dropped");
  assert.ok(fs.existsSync(fresh.trash), "The backup remains until a week after its first run");
});

test("a destination reports its first local reply after the carried transcript", {
  timeout: 45000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("first-run");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const at = Date.now() - 3600_000;
  const id = "mov_firstRun", session = "ses_importedFirstRun", gone = "mov_firstRunGone";
  save("run/moves/ended.json", {
    [id]: { as: "switched", peer: member.key, at, last: "msg_carriedLast" },
    [gone]: { as: "switched", peer: member.key, at },
  });
  here.engine.sessions.push(sessionOf({ id: session, directory: path.join(here.home, "chats") }));
  const message = (id, type, created) => ({ id, type, time: { created }, text: id });
  const history = [message("msg_carriedLast", "assistant", at + 5_000_000)];
  here.engine.messages.set(session, history);
  const resident = await here.start().ready();
  await until(async () => (await request(resident, "GET", "api/session/" + session)).status === 200,
    "The destination's engine did not become ready");
  const askRun = { op: "ran?", id, session };
  const reads = () => here.engine.asked.filter(call => call.method === "GET"
    && call.path === "/api/session/" + session + "/message").length;
  const before = reads();
  await ask(askRun, false);
  await until(() => reads() > before, "The destination did not check the imported transcript");
  history.push(message("msg_localQuestion", "user", at + 50), message("msg_localFirst", "assistant", at + 100),
    message("msg_localSecond", "assistant", at + 500));
  assert.deepEqual(await ask(askRun), { id, op: "ran", at: at + 100 });
  const checkedAt = Date.now();
  const absent = await ask({ op: "ran?", id: gone, session: "ses_absentFirstRun" });
  assert.equal(absent.op, "ran");
  assert.ok(absent.at >= checkedAt, "A conversation that no longer lives here starts the retention clock");
});

test("an expired partial copy is discarded without making a conversation", {
  timeout: 35000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("expired-copy");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const id = "mov_expiredCopy", session = "ses_expiredCopy", hash = "A".repeat(43), generation = 1;
  const journal = "run/moves/" + id + ".json";
  save(journal, {
    formatVersion: 1, id, thing: session, name: "Partial copy", kind: "chat", from: member.key,
    to: main.key, peer: member.key, role: "to", stage: "copying", sessions: [],
    started: Date.now() - 8 * 86400000, safeSwitch: true, twoComputers: true,
    generation, hash, size: 60000, sent: 30000, createdSessions: [],
  });
  const ready = "mov_waitingForSwitch";
  save("run/moves/" + ready + ".json", {
    formatVersion: 1, id: ready, thing: "ses_waitingForSwitch", name: "Ready copy", kind: "chat",
    from: member.key, to: main.key, peer: member.key, role: "to", stage: "copying", sessions: ["ses_waitingForSwitch"],
    started: Date.now() - 8 * 86400000, safeSwitch: true, twoComputers: true,
    generation, hash, size: 30000, sent: 30000, complete: true, setUpAt: Date.now() - 8 * 86400000,
  });
  const resident = here.start();
  await resident.ready();
  await until(() => !fs.existsSync(path.join(here.home, journal)),
    () => "The destination kept the expired partial copy\n" + resident.log());
  assert.equal(here.engine.sessions.length, 0);
  assert.deepEqual(await ask({ op: "decision?", id, generation, hash }),
    { op: "dropped", id, generation, hash, neverSwitched: true });
  assert.equal((await ask({ op: "arrive", id: ready, generation, hash })).op, "arrived");
});

test("a destination refuses an oversized offer before creating a copy", {
  timeout: 45000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("oversized-offer");
  const { main } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const resident = await here.start().ready();
  const id = "mov_oversizedOffer", hash = "A".repeat(43);
  const movement = { op: "offer", id, generation: 1, hash, kind: "chat", thing: "ses_bigVideos",
    name: "“Videos”", size: 1_000_000_001, safeSwitch: true, twoComputers: true };
  const refused = await ask(movement);
  assert.deepEqual(refused, { op: "refused", id, generation: 1, hash,
    message: "“Videos” is more than 1 GB, too big to move to another computer, so it stays where it is." });
  const list = await request(resident, "GET", "kuvo/moves");
  assert.equal(list.status, 200, list.text);
  assert.deepEqual(list.json.moves, []);
  assert.deepEqual(list.json.away, []);
  assert.equal(fs.existsSync(path.join(here.home, "run/moves", id + ".json")), false);
  assert.equal(here.engine.sessions.length, 0);
});

test("the source keeps a conversation whose wake-up folder is too large to move", {
  timeout: 45000, skip: process.platform === "win32" && "The fixture uses a Unix sparse file",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("large-source"), there = await computer("large-target");
  const { member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_largeWake", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, title: "Trip plans", directory }));
  const wake = path.join(here.home, "wakes", session);
  const recording = path.join(wake, "recording.mov");
  write(recording, "");
  fs.truncateSync(recording, 1_100_000_000);
  const sourceProcess = here.start(), destinationProcess = there.start();
  const [source] = await Promise.all([sourceProcess.ready(), destinationProcess.ready()]);
  await readyToMove(source, session, member);
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const id = begun.json.move.id;
  await moveRetired(here, id, () => sourceProcess.log() + "\n" + destinationProcess.log());
  const receipt = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
  assert.deepEqual(receipt.json, {
    state: "failed",
    message: "“Trip plans” is more than 1 GB, too big to move to another computer, so it stays where it is.",
  });
  assert.ok(here.engine.sessions.some(record => record.id === session));
  assert.equal(there.engine.sessions.length, 0);
  assert.equal(fs.statSync(recording).size, 1_100_000_000);
  assert.equal(fs.existsSync(path.join(wake, "paused")), false);
  assert.equal(fs.existsSync(path.join(there.home, "run/moves", id + ".json")), false);
});

test("a one-part move waits for the acknowledgement of part zero", {
  timeout: 45000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("one-part-source"), there = await computer("one-part-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_onePart", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, directory }));
  const { openOneWay, sealOneWay, frame, Frame } = await testMail(t, boxBinary(wireFixture));
  const [from, to] = [main, member].map(peer => Buffer.from(peer.address, "base64url"));
  const [sourceKey, targetKey] = [main, member].map(peer => Buffer.from(peer.key, "base64url"));
  const [sourceSecret, targetSecret] = [here, there].map(peer =>
    Buffer.from(readJSON(path.join(peer.home, "run/link.json")).key, "base64url"));
  const barrier = "mov_ackBarrier";
  let part, checked;
  const release = relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), targetSecret, sourceKey);
    const move = JSON.parse(payload).move;
    if (move?.op === "part") {
      part ??= { packet, move };
      return true;
    }
    if (move?.op === "failed" && move.id === barrier) checked = true;
    return false;
  });
  t.after(release);
  const [source] = await Promise.all([here.start().ready(), there.start().ready()]);
  await readyToMove(source, session, member);
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const { id } = begun.json.move;
  await until(() => part, "The source did not send its part");
  const { generation, hash, i, of } = part.move;
  assert.equal(i, 0);
  assert.equal(of, 1);
  const send = move => {
    const { message } = sealOneWay(Buffer.from(JSON.stringify({ formatVersion: 1, at: Date.now(), move })),
      targetSecret, sourceKey);
    relay.replay({ from: to, to: from, payload: frame(Frame.mail, 0, message) });
  };
  send({ op: "got", id, generation, hash, i: 1 });
  // The next request on the same connection proves the source handled the preceding ack.
  send({ op: "arrive", id: barrier, generation, hash });
  await until(() => checked, "The source did not answer the following arrival query");
  const before = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
  assert.equal(before.status, 200, before.text);
  assert.equal(before.json.move.sent ?? 0, 0, "An out-of-range acknowledgement must not count as copied bytes");
  release();
  relay.replay(part.packet);
  await until(async () => {
    const reply = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
    assert.notEqual(reply.json?.state, "failed", reply.text);
    return reply.json?.state === "done";
  }, "The move did not finish after the real acknowledgement");
  assert.ok(there.engine.sessions.some(record => record.id === session));
});

test("a destination resumes a partial move after restarting", {
  timeout: 50000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("partial-source"), there = await computer("partial-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_partialCopy", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, directory }));
  const bytes = Buffer.concat(Array.from({ length: 1300 }, (_, index) =>
    createHash("sha256").update(String(index)).digest()));
  const folder = path.join("wakes", session);
  write(path.join(here.home, folder, "wake.txt"), bytes);
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const [from, to] = [main, member].map(peer => Buffer.from(peer.address, "base64url"));
  const [sourceKey, targetKey] = [main, member].map(peer => Buffer.from(peer.key, "base64url"));
  let remaining, firstAck, restarting = false, announced = false;
  const release = relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const move = JSON.parse(openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), sourceKey).payload).move;
    if (move?.op !== "part" || move.i === 0) return false;
    remaining ??= { packet, move };
    return true;
  });
  t.after(release);
  t.after(relay.intercept(to, from, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { move, agents } = JSON.parse(openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 1), targetKey).payload);
    if (restarting && agents?.features?.includes("moves.two-computers")) announced = true;
    if (move?.op === "got" && move.i === 0) firstAck = move;
    return false;
  }));
  const sourceProcess = here.start(), targetProcess = there.start();
  const [source, target] = await Promise.all([sourceProcess.ready(), targetProcess.ready()]);
  await readyToMove(source, session, member);
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const { id } = begun.json.move;
  await until(() => remaining && firstAck, "The destination did not keep the first part", 15000);
  assert.equal(firstAck.id, id);
  assert.equal(remaining.move.of, 2);
  assert.equal(remaining.move.i, 1);
  assert.equal(there.engine.sessions.length, 0);
  for (const [resident, place, progress] of [
    [source, member.name, "Moving to " + member.name + "…"],
    [target, main.name, "Moving to this computer…"],
  ]) {
    const result = await request(resident, "GET", "kuvo/moves");
    assert.equal(result.status, 200, result.text);
    const move = result.json.moves.find(move => move.id === id);
    assert.equal(move?.place, place, result.text);
    assert.equal(move.progress, progress);
    assert.equal(move.stage, "copying");
    assert.ok(move.size > 0 && (move.sent ?? 0) <= move.size);
  }
  await targetProcess.stop();
  await until(() => !relay.has(to), "The stopped destination did not leave the relay");
  restarting = true;
  const restarted = there.start();
  await restarted.ready();
  await until(() => announced, () => "The restarted destination did not announce that it could receive moves\n" + restarted.log());
  release();
  relay.replay(remaining.packet);
  await moveRetired(here, id, () => sourceProcess.log() + "\n" + restarted.log());
  const receipt = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
  assert.equal(receipt.json.state, "done", receipt.text);
  assert.equal(there.engine.sessions.filter(record => record.id === session).length, 1);
  assert.deepEqual(fs.readFileSync(path.join(there.home, folder, "wake.txt")), bytes);
});

test("a move carries replay evidence saved after the first offer", {
  timeout: 75000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("evidence-source"), there = await computer("evidence-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_lateEvidence", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, directory }));
  const setup = readJSON(path.join(here.home, "run/link.json")).setup.id;
  const draft = "msg_lateEvidence";
  const address = createHash("sha256").update(JSON.stringify(["typed-draft", setup, session, draft])).digest("base64url");
  const evidence = { formatVersion: 1, digest: "scoped:" + "C".repeat(43),
    draftReplayFence: { setup, session, id: draft } };
  const file = path.join("run/owned-data/mutations", address + ".json");
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const [from, to] = [main, member].map(peer => Buffer.from(peer.address, "base64url"));
  const generations = new Set();
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), Buffer.from(main.key, "base64url"));
    const move = JSON.parse(payload).move;
    if (move?.op === "offer") {
      if (!generations.size) write(path.join(here.home, file), JSON.stringify(evidence));
      generations.add(move.generation);
    }
    return false;
  }));
  const sourceProcess = here.start(), destinationProcess = there.start();
  const [source] = await Promise.all([sourceProcess.ready(), destinationProcess.ready()]);
  await until(() => {
    try { return readJSON(path.join(here.home, "run/peer-agents.json"))[member.key]?.features?.includes("data.forward-evidence"); }
    catch { return false; }
  }, "The destination did not announce support for forwarding evidence");
  await readyToMove(source, session, member);
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const { id } = begun.json.move;
  const installed = path.join(there.home, file);
  const outcome = path.join(here.home, "run/moves", id + ".outcome.json");
  const journal = computer => {
    const file = path.join(computer.home, "run/moves", id + ".json");
    return fs.existsSync(file) ? readJSON(file) : null;
  };
  await until(() => {
    if (fs.existsSync(outcome)) assert.notEqual(readJSON(outcome).ok, false, fs.readFileSync(outcome, "utf8"));
    return fs.existsSync(installed);
  }, () => "The destination never installed the evidence\n" + JSON.stringify({
    generations: [...generations], source: journal(here), destination: journal(there),
  }) + "\n" + sourceProcess.log() + "\n" + destinationProcess.log(), 20000);
  await until(() => fs.existsSync(outcome), () => "The source did not record the move result\n"
    + sourceProcess.log() + "\n" + destinationProcess.log(), 30000);
  const receipt = await request(source, "GET", "kuvo/moves/receipt?id=" + id, undefined, {}, 20000);
  assert.equal(receipt.status, 200, receipt.text);
  assert.equal(receipt.json.state, "done", receipt.text);
  assert.equal(receipt.json.move.to, member.key, receipt.text);
  assert.deepEqual([...generations], [1, 2], "An offer prepared before the decision must be rebuilt");
  assert.deepEqual(readJSON(installed), evidence);
});

test("a conversation that keeps changing stays with its source after three offers", {
  timeout: 50000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("changing-source"), there = await computer("changing-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_changing", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, title: "A changing chat", directory }));
  const messages = [{ id: "msg_original", type: "user", content: [{ type: "text", text: "Original." }] }];
  here.engine.messages.set(session, messages);
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const [from, to] = [main, member].map(peer => Buffer.from(peer.address, "base64url"));
  const generations = new Set();
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), Buffer.from(main.key, "base64url"));
    const move = JSON.parse(payload).move;
    if (move?.op === "offer" && !generations.has(move.generation)) {
      generations.add(move.generation);
      messages.push({ id: "msg_during" + move.generation, type: "user", content: [{ type: "text", text: "And this." }] });
    }
    return false;
  }));
  const sourceProcess = here.start(), targetProcess = there.start();
  const [source] = await Promise.all([sourceProcess.ready(), targetProcess.ready()]);
  await readyToMove(source, session, member);
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const { id } = begun.json.move;
  await moveRetired(here, id, () => sourceProcess.log() + "\n" + targetProcess.log());
  const receipt = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
  assert.equal(receipt.json.state, "failed", receipt.text);
  assert.match(receipt.json.message, /kept getting messages while it moved, so it stayed where it is/);
  assert.deepEqual([...generations], [1, 2, 3]);
  assert.ok(here.engine.sessions.some(row => row.id === session));
  assert.equal(here.engine.messages.get(session).length, 4);
  assert.ok(!there.engine.sessions.some(row => row.id === session));
});

test("an older incoming copy without import proof stays held after a drop", {
  timeout: 45000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("unproven-import");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const id = "mov_UnprovenImport", session = "ses_UnprovenImport", hash = "A".repeat(43);
  here.engine.sessions.push(sessionOf({ id: session, directory: path.join(here.home, "chats") }));
  const file = "run/moves/" + id + ".json";
  save(file, { id, thing: session, name: "Existing conversation", kind: "chat", from: member.key,
    to: main.key, peer: member.key, role: "to", stage: "copying", sessions: [session],
    started: Date.now(), safeSwitch: true, complete: true, wasPaused: false, generation: 1, hash });
  const { sealOneWay, openOneWay, frame, readFrame, Frame } = await testMail(t, boxBinary(wireFixture));
  const secret = Buffer.alloc(32, 2), source = Buffer.from(main.key, "base64url");
  const peer = relay.device(Buffer.alloc(32, 52));
  t.after(() => peer.leave());
  const resident = await here.start().ready();
  const address = Buffer.from(main.address, "base64url");
  await until(() => relay.has(address), "The receiving computer did not connect");
  assert.equal((await request(resident, "GET", "kuvo/moves")).status, 200);
  const envelope = Buffer.from(JSON.stringify({ move: { op: "drop", id, generation: 1, hash }, at: Date.now(), formatVersion: 1 }));
  const { message } = sealOneWay(envelope, secret, source);
  peer.send(address, frame(Frame.mail, 0, message));
  await until(async () => {
    const delivery = await peer.next();
    const packet = readFrame(delivery.subarray(17));
    return packet?.kind === Frame.mail &&
      JSON.parse(openOneWay(packet.body, secret, source).payload).move?.op === "dropped";
  }, "The original computer was not told it could stay active");
  assert.equal(readJSON(path.join(here.home, file)).dropping, true);
  assert.ok(here.engine.sessions.some(each => each.id === session));
  assert.ok((await request(resident, "GET", "kuvo/moves")).json.away.includes(session));
});

for (const reverse of binary.typescript ? [false] : [false, true]) {
  test(`a live chat, a held queue, and letters ${reverse ? "move back" : "move to the other computer"}`, { timeout: 240000 }, async t => {
    const { computer, relay } = await bench(t);
    const source = await computer("live-source");
    const destination = await computer("live-destination");
    const { main, member } = seedPeerSetup(source.home, relay.url, reverse ? 2 : 1);
    seedPeerSetup(destination.home, relay.url, reverse ? 1 : 2);
    const [from, to] = reverse ? [member, main] : [main, member];
    const cuts = !binary.typescript && !reverse && fs.existsSync(wireFixture)
      ? cutMoveMail(relay, (await testMail(t, boxBinary(wireFixture))).openOneWay, source, destination, from, to) : null;
    if (cuts) t.after(() => cuts.close());
    const job = "ses_liveChat", child = "ses_liveChild", waiting = "msg_pausedMessage";
    const chats = path.join(source.home, "chats");
    fs.mkdirSync(chats, { recursive: true });
    source.engine.sessions.push(sessionOf({ id: job, title: "Live transfer", directory: chats }));
    source.engine.sessions.push(sessionOf({ id: child, title: "Live child", directory: chats, parent: job }));
    const siblings = Array.from({length: 100}, (_, index) => `ses_liveSibling${index}`);
    for (const id of siblings) source.engine.sessions.push(sessionOf({ id, directory: chats, parent: job }));
    const transcript = [{ id: "msg_originalMessage", type: "user", content: [{ type: "text", text: "Keep this conversation." }] }];
    const queued = { id: waiting, text: "Wait for a resume after the move.", delivery: "queue" };
    source.engine.messages.set(job, transcript);
    write(path.join(source.home, "run/queues.json"), JSON.stringify({
      [job]: { formatVersion: 1, paused: true, held: [], userPaused: true, waiting: [{ id: waiting, text: queued.text, at: 1 }] },
    }));
    write(path.join(source.home, `run/waiting/${waiting}.json`), JSON.stringify({ ...queued, formatVersion: 1 }));
    const noteFile = path.join("run/work/notes", `${job}.ordinary.json`);
    const notes = { version: 1, notes: { carried: {
      versions: [{ revision: "R".repeat(43), deleted: false, fields: { text: "Keep this ordinary note." } }],
      choices: [], receipts: [], additions: {},
    } } };
    write(path.join(source.home, noteFile), JSON.stringify(notes));
    const setup = readJSON(path.join(source.home, "run/link.json")).setup.id;
    const address = createHash("sha256").update(JSON.stringify(["typed-draft", setup, job, "msg_alreadyClaimed"])).digest("base64url");
    const fenceFile = path.join("run/owned-data/mutations", address + ".json");
    const fence = { formatVersion: 1, digest: "scoped:" + "C".repeat(43),
      draftReplayFence: { setup, session: job, id: "msg_alreadyClaimed" } };
    write(path.join(source.home, fenceFile), JSON.stringify(fence));
    const srcProcess = source.start(), destProcess = destination.start();
    const started = await Promise.all([srcProcess.ready(), destProcess.ready()]);
    const src = started[0];
    let dest = started[1];
    await until(() => {
      try {
        return readJSON(path.join(source.home, "run/peer-agents.json"))[to.key]?.features?.includes("moves.two-computers");
      } catch { return false; }
    }, () => "The source did not hear the destination's move capability\n" + destProcess.log(), 15000);
    await readyToMove(src, job, to);
    const beginMove = () => request(src, "POST", "kuvo/moves", { kind: "chat", id: job, to: to.key, receipt: true });
    const simultaneous = await Promise.all([beginMove(), beginMove()]);
    assert.deepEqual(simultaneous.map(reply => reply.status).sort(), [202, 409],
      simultaneous.map(reply => reply.text).join("\n"));
    const begin = simultaneous.find(reply => reply.status === 202);
    assert.match(simultaneous.find(reply => reply.status === 409).json.message, /moving to another computer/);
    const id = begin.json.move.id;
    if (cuts) {
      await cuts.resume();
      const proof = path.join(destination.home, "run/moves/decisions", id + ".json");
      assert.equal(readJSON(proof).as, "switched");
      const still = await request(src, "GET", "kuvo/moves/receipt?id=" + id);
      assert.equal(still.json.state, "moving");
      await destProcess.stop();
      dest = await destination.start().ready();
      const online = await request(dest, "GET", "kuvo/moves");
      assert.equal(online.status, 200, online.text);
      await cuts.recoverSwitch(async () => {
        const waiting = await until(async () => {
          const state = await request(src, "GET", "kuvo/moves/receipt?id=" + id);
          assert.equal(state.json?.state, "moving", state.text);
          return state.json.move.waiting && state.json.move;
        }, "The source never reported waiting for the switch acknowledgement", 105000, 200);
        assert.equal(waiting.progress, `Waiting for ${to.name}…`);
      });
    }
    const completed = await until(async () => {
      const answer = await request(src, "GET", "kuvo/moves/receipt?id=" + id);
      assert.equal(answer.status, 200, answer.text);
      assert.notEqual(answer.json.state, "failed", JSON.stringify(answer.json) + "\n" + srcProcess.log() + "\n" + destProcess.log());
      return answer.json.state === "done" && answer.json;
    }, () => "The transfer did not finish\n" + srcProcess.log() + "\n" + destProcess.log(), cuts ? 70000 : 22000);
    assert.equal(completed.message, `Moved to ${to.name}.`);
    const decision = readJSON(path.join(destination.home, "run/moves/decisions", id + ".json"));
    assert.equal(decision.as, "switched");
    assert.equal(decision.peer, from.key);
    assert.equal(decision.generation, 1);
    assert.match(decision.hash, /^[A-Za-z0-9_-]{43}$/);
    assert.deepEqual(destination.engine.messages.get(job), transcript);
    assert.equal(destination.engine.sessions.find(each => each.id === child)?.parentID, job);
    assert.deepEqual(destination.engine.sessions.filter(each => siblings.includes(each.id)).map(each => each.id).sort(), siblings.sort());
    await until(() => source.engine.sessions.length === 0, () => "The retired original was not removed\n" + srcProcess.log());
    assert.deepEqual(readJSON(path.join(destination.home, `run/waiting/${waiting}.json`)), { ...queued, formatVersion: 1 });
    assert.equal(readJSON(path.join(destination.home, "run/queues.json"))[job].userPaused, true);
    assert.deepEqual(readJSON(path.join(destination.home, noteFile)), {
      ...notes, move: `${id}:${decision.generation}:${decision.hash}`,
    });
    assert.deepEqual(readJSON(path.join(destination.home, fenceFile)), fence);
    assert.ok(readJSON(path.join(source.home, "run/moves/moved.json")).some(row => row.session === job && row.key === to.key));
    const resume = await request(dest, "POST", `kuvo/sessions/${job}/queue/resume`, {}).catch(error => { throw new Error("Resuming the moved chat's queue: " + destProcess.log(), { cause: error }); });
    assert.equal(resume.status, 200, resume.text);
    await until(() => destination.engine.messages.get(job).some(each => each.id === waiting), "The held message did not go after resume");

    // Its former owner is recorded in the envelope. The source's move
    // journal must resolve the new owner, and delivery uses the usual inbox.
    const text = "The sender on its earlier computer reached this moved chat.";
    write(path.join(source.home, "run/outbox/sending/liveLetter.json"), JSON.stringify({ to: job, text, at: Date.now(), peer: from.key, wait: false }));
    await until(() => destination.engine.messages.get(job).some(each => each.text?.includes(text)), "The letter never reached the moved chat", 17000);
    assert.equal(destination.engine.messages.get(job).filter(each => each.text?.includes(text)).length, 1);
    if (!binary.typescript && !reverse) {
      const left = path.join(source.home, "run/moves/left.json");
      const backup = readJSON(left).find(each => each.id === id);
      assert.ok(fs.existsSync(backup.trash));
      await readyToMove(dest, job, from);
      const returning = await request(dest, "POST", "kuvo/moves", { kind: "chat", id: job, to: from.key, receipt: true });
      assert.equal(returning.status, 202, returning.text);
      await until(async () => {
        const result = await request(dest, "GET", "kuvo/moves/receipt?id=" + returning.json.move.id);
        assert.notEqual(result.json?.state, "failed", result.text);
        return result.json?.state === "done";
      }, "The moved chat did not return to its first computer", 22000);
      assert.ok(!fs.existsSync(backup.trash));
      assert.ok(!fs.existsSync(left) || !readJSON(left).some(each => each.id === id));
      assert.ok(source.engine.messages.get(job).some(each => each.text?.includes(text)));
      assert.equal(source.engine.sessions.find(each => each.id === child)?.parentID, job);
    }
  });
}

test("move destinations show the other computer and wait for a running turn", { timeout: 45000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("places-source"), there = await computer("places-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const id = "ses_movePlaces", chats = path.join(here.home, "chats");
  fs.mkdirSync(chats, { recursive: true });
  here.engine.sessions.push(sessionOf({ id, title: "Trip plans", directory: chats }));
  const from = here.start(), to = there.start();
  let [source] = await Promise.all([from.ready(), to.ready()]);
  const places = session => request(source, "GET", "kuvo/moves/places?kind=chat&id=" + session);
  const ready = await until(async () => {
    const reply = await places(id);
    assert.equal(reply.status, 200, reply.text);
    return reply.json.places[0]?.ok && reply.json;
  }, "The destination did not become available", 15000);
  const intro = "Its conversation and wake-ups move with it.";
  const end = `New messages and wake-ups wait during the move. Vault items, integrations and browser sign-ins stay on ${main.name}. The original goes to Kuvo Code’s trash folder on ${main.name}.`;
  const destination = message => ({ places: [{ key: member.key, name: member.name, kind: "machine", ok: true,
    confirm: { title: `Move “Trip plans” to ${member.name}?`, label: "Move", message } }] });
  assert.deepEqual(ready, destination(intro + " " + end));
  here.engine.running.add(id);
  const busy = await places(id);
  assert.deepEqual(busy.json, destination(intro + " A turn is running. The move waits up to a minute for it to finish. " + end));
  const missing = await places("ses_doesNotExist");
  assert.equal(missing.status, 404);
  assert.deepEqual(missing.json, { message: "It isn’t on this computer any more." });

  await Promise.all([from.stop(), to.stop()]);
  const reach = JSON.stringify({ [member.key]: Date.now() });
  write(path.join(here.home, "run/peer-reach.json"), reach);
  write(path.join(here.home, "run/peer-reach-checked.json"), reach);
  source = await here.start().ready();
  const away = await places(id);
  assert.equal(away.status, 200, away.text);
  assert.deepEqual(away.json, { places: [{ key: member.key, name: member.name, kind: "machine", ok: false,
    why: `${member.name} isn’t connected. Connect to it, then try again.`, note: "Can’t be reached" }] });
});

test("a waiting move reports an unanswered destination as unavailable", {
  timeout: 110000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("unanswered-move-source"), there = await computer("unanswered-move-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_unansweredMove", chats = path.join(here.home, "chats");
  fs.mkdirSync(chats, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, directory: chats }));
  const pause = path.join(here.home, "wakes", session, "paused");
  write(pause, "user-paused");
  const [source] = await Promise.all([here.start().ready(), there.start().ready()]);
  await readyToMove(source, session, member);
  const { openOneWay } = await testMail(t, boxBinary(wireFixture));
  let checked = false;
  const remove = relay.intercept(Buffer.from(main.address, "base64url"), Buffer.from(member.address, "base64url"), packet => {
    if (packet.payload[0] !== 5) return false;
    try {
      const mail = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), Buffer.from(main.key, "base64url"));
      if (JSON.parse(mail.payload).move?.op !== "check") return false;
      checked = true;
      return true;
    } catch { return false; }
  });
  t.after(remove);
  const refused = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key }, {}, 80000);
  assert.equal(checked, true);
  assert.equal(refused.status, 503, refused.text);
  assert.deepEqual(refused.json, { message: `${member.name} isn’t connected. Connect to it, then try again.` });
  assert.equal(here.engine.sessions[0]?.id, session);
  assert.equal(there.engine.sessions.length, 0);
  assert.equal(fs.readFileSync(pause, "utf8"), "user-paused");
});

test("a conversation stays home while its background work is running", { timeout: 55000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("background-source"), there = await computer("background-target");
  const { member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const id = "ses_backgroundMove", child = "ses_backgroundChild", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id, title: "Trip plans", directory }), sessionOf({ id: child, directory, parent: id }));
  const [source] = await Promise.all([here.start().ready(), there.start().ready()]);
  await readyToMove(source, id, member);
  const working = path.join(here.home, "run/work/background.json");
  const running = (session, started) => ({
    engine: { sessions: { [session]: { commands: [{ id: "cmd_background", command: "npm run dev", started }],
      subagents: [], tasks: [] } }, folders: [] },
  });
  write(working, JSON.stringify(running(id, Date.now())));
  const why = "“Trip plans” has a command running. Wait for it to finish, or stop it, then move “Trip plans”.";
  const places = await request(source, "GET", "kuvo/moves/places?kind=chat&id=" + id);
  assert.equal(places.status, 200, places.text);
  const place = places.json.places.find(place => place.key === member.key);
  assert.equal(place.ok, false, places.text);
  assert.equal(place.why, why);
  assert.equal(place.note, "Running something");
  const move = () => request(source, "POST", "kuvo/moves", { kind: "chat", id, to: member.key, receipt: true });
  const refused = await move();
  assert.equal(refused.status, 409, refused.text);
  assert.equal(refused.json.message, why);
  const moves = path.join(here.home, "run/moves");
  assert.ok(!fs.existsSync(moves) || !fs.readdirSync(moves).some(name => /^mov_/.test(name)));

  const rootOld = running(id, Date.now() - 7 * 3600_000);
  Object.assign(rootOld.engine.sessions, running(child, Date.now()).engine.sessions);
  write(working, JSON.stringify(rootOld));
  const started = await move();
  assert.equal(started.status, 202, started.text);
  const file = path.join(moves, started.json.move.id + ".outcome.json");
  await until(() => fs.existsSync(file), "The source did not finish checking its child's background work");
  const outcome = await request(source, "GET", "kuvo/moves/receipt?id=" + started.json.move.id);
  assert.deepEqual(outcome.json, { state: "failed", message: why });
  assert.deepEqual(here.engine.sessions.map(row => row.id), [id, child]);
  assert.equal(there.engine.sessions.length, 0);
});

test("an unavailable destination or missing sign-in refuses before work is held", { timeout: 65000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("account-source"), there = await computer("account-target");
  const { member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const id = "ses_accountMove", chats = path.join(here.home, "chats");
  fs.mkdirSync(chats, { recursive: true });
  here.engine.sessions.push({ ...sessionOf({ id, title: "Account chat", directory: chats }),
    model: { providerID: "needs-sign-in", id: "model" } });
  there.engine.providers.push({ id: "another-account" });
  const [source] = await Promise.all([here.start().ready(), there.start().ready()]);
  await until(() => {
    try { return readJSON(path.join(here.home, "run/peer-agents.json"))[member.key]?.features?.includes("moves.check"); }
    catch { return false; }
  }, "The source did not hear the destination's move checks", 15000);
  await readyToMove(source, id, member);
  const move = () => request(source, "POST", "kuvo/moves", { kind: "chat", id, to: member.key, receipt: true });
  const status = handle => request(source, "GET", "kuvo/moves/receipt?id=" + handle);
  async function refuses(message) {
    const begun = await move();
    assert.equal(begun.status, 202, begun.text);
    const moveId = begun.json.move.id;
    const failed = await until(async () => {
      const reply = await status(moveId);
      assert.equal(reply.status, 200, reply.text);
      assert.notEqual(reply.json.state, "done", "The chat moved despite a destination refusal");
      return reply.json.state === "failed" && reply.json;
    }, "The destination did not refuse the move", 15000);
    assert.deepEqual(failed, { state: "failed", message });
    assert.ok(here.engine.sessions.some(session => session.id === id));
    assert.equal(there.engine.sessions.length, 0);
    assert.ok(!fs.existsSync(path.join(here.home, "wakes", id, "paused")));
    await until(() => !fs.existsSync(path.join(here.home, "run/moves", moveId + ".json")),
      "The refused move did not release its reservation");
  }
  const place = member.name;
  there.engine.refuseActive = true;
  await refuses(`The engine on ${place} isn’t running. Try again once it is.`);
  there.engine.refuseActive = false;
  await refuses(`“Account chat”’s model needs a sign-in on ${place}. Sign in to it in Settings › Accounts there, or choose another model, then move “Account chat”.`);

  there.engine.providers.push({ id: "needs-sign-in" });
  const retry = await move();
  assert.equal(retry.status, 202, retry.text);
  await until(async () => {
    const reply = await status(retry.json.move.id);
    assert.notEqual(reply.json?.state, "failed", reply.text);
    return reply.json?.state === "done";
  }, "The chat did not move once its provider was signed in", 20000);
  assert.equal(there.engine.sessions.find(session => session.id === id)?.id, id);
});

test("a restart restores legacy wake-up holds without releasing another move", { timeout: 45000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("legacy-wake-holds");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const held = "ses_formerWakeHold", orphan = "ses_legacyOrphan", owned = "ses_namedWakeHold";
  const paused = id => path.join(here.home, "wakes", id, "paused");
  write(paused(held), "moving");
  write(paused(orphan), "moving");
  write(paused(owned), "moving:mov_otherMove");
  const id = "mov_legacyWakeHold";
  save("run/moves/" + id + ".json", {
    formatVersion: 0, id, thing: held, name: "The previous chat", kind: "chat",
    from: main.key, to: member.key, peer: member.key, role: "from", stage: "holding",
    sessions: [held], step: "done", started: 1, failed: "It stayed here.",
    folder: path.dirname(paused(held)), wasPaused: true, pausedAs: "user-paused",
  });
  const source = here.start();
  await source.ready();
  await until(() => {
    try { return fs.readFileSync(paused(held), "utf8") === "user-paused" && !fs.existsSync(paused(orphan)); }
    catch { return false; }
  }, () => "The resident left old wake-ups held\n" + source.log(), 8000);
  assert.equal(fs.readFileSync(paused(owned), "utf8"), "moving:mov_otherMove");
});

test("a chat's wake-up folder stays held until its new computer owns it", {
  timeout: 60000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("wake-source"), there = await computer("wake-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_wakeMove";
  const sourceWakes = path.join(path.dirname(here.home), "My wake-ups");
  const folder = path.join(sourceWakes, session);
  const target = path.join(there.home, "wakes", session);
  const sourceChats = path.join(path.dirname(here.home), "My chats");
  fs.mkdirSync(sourceChats, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, directory: sourceChats }));
  const savedPause = "user-paused\n", wake = "Keep the existing wake-up.\n";
  write(path.join(folder, "paused"), savedPause);
  write(path.join(folder, "directory"), sourceChats);
  write(path.join(folder, "wake.txt"), wake);
  if (process.platform !== "win32") fs.symlinkSync("wake.txt", path.join(folder, "current"));
  const { openOneWay } = await testMail(t, boxBinary(wireFixture));
  const from = Buffer.from(main.address, "base64url"), to = Buffer.from(member.address, "base64url");
  const remote = Buffer.from(main.key, "base64url");
  let switchPacket;
  const release = relay.intercept(from, to, packet => {
    if (packet.payload[0] !== 5) return false;
    try {
      const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), remote);
      if (JSON.parse(payload).move?.op !== "switch") return false;
      switchPacket = packet;
      return true;
    } catch { return false; }
  });
  t.after(release);
  const srcProcess = here.start(binary, { KUVO_WAKES: sourceWakes, KUVO_CHATS: sourceChats });
  const dstProcess = there.start();
  const [source, destination] = await Promise.all([srcProcess.ready(), dstProcess.ready()]);
  await until(() => {
    try { return readJSON(path.join(here.home, "run/peer-agents.json"))[member.key]?.features?.includes("moves.two-computers"); }
    catch { return false; }
  }, "The source did not hear the destination's move capability", 15000);
  await readyToMove(source, session, member);
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const id = begun.json.move.id;
  await until(() => switchPacket, "The held wake-up folder did not reach its destination", 15000);
  assert.equal(fs.readFileSync(path.join(folder, "paused"), "utf8"), "moving:" + id);
  assert.equal(fs.readFileSync(path.join(target, "paused"), "utf8"), "moving:" + id);
  release();
  relay.replay(switchPacket);
  const logs = () => srcProcess.log() + "\n" + dstProcess.log();
  await moveRetired(here, id, logs);
  const first = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
  assert.equal(first.status, 200, first.text);
  assert.equal(first.json.state, "done", first.text);
  const left = readJSON(path.join(here.home, "run/moves/left.json")).find(row => row.id === id);
  assert.ok(!fs.existsSync(folder));
  assert.equal(fs.readFileSync(path.join(left.trash, "wake.txt"), "utf8"), wake);
  assert.equal(readJSON(path.join(left.trash, "conversation.json"))[0].info.id, session);
  assert.equal(fs.readFileSync(path.join(target, "wake.txt"), "utf8"), wake);
  assert.equal(fs.readFileSync(path.join(target, "directory"), "utf8"), path.join(there.home, "chats"));
  assert.equal(fs.readFileSync(path.join(target, "paused"), "utf8"), savedPause);
  if (process.platform !== "win32") assert.equal(fs.readlinkSync(path.join(target, "current")), "wake.txt");

  fs.unlinkSync(path.join(target, "paused"));
  await readyToMove(destination, session, main);
  const back = await request(destination, "POST", "kuvo/moves", { kind: "chat", id: session, to: main.key, receipt: true })
    .catch(error => { throw new Error("Starting the return move\n" + dstProcess.log(), { cause: error }); });
  assert.equal(back.status, 202, back.text);
  await moveRetired(there, back.json.move.id, logs);
  const returned = await request(destination, "GET", "kuvo/moves/receipt?id=" + back.json.move.id);
  assert.equal(returned.status, 200, returned.text);
  assert.equal(returned.json.state, "done", returned.text);
  assert.ok(!fs.existsSync(target));
  assert.ok(!fs.existsSync(path.join(folder, "paused")));
  assert.equal(fs.readFileSync(path.join(folder, "wake.txt"), "utf8"), wake);
  assert.equal(fs.readFileSync(path.join(folder, "directory"), "utf8"), sourceChats);
});

test("a transcript read returns to its tool without prompting the conversation", { timeout: 90000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("read-source"), there = await computer("read-owner");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const sender = "ses_readSender", recipient = "ses_readTarget", archivedSession = "ses_readArchived";
  here.engine.sessions.push(sessionOf({ id: sender, title: "Reader", directory: path.join(here.home, "chats") }));
  there.engine.sessions.push({
    ...sessionOf({ id: recipient, title: "Reservations", directory: path.join(there.home, "chats") }),
    revert: { messageID: "msg_undoBoundary" },
  });
  there.engine.sessions.push(sessionOf({ id: archivedSession, title: "Archived trip", archived: true,
    directory: path.join(there.home, "chats") }));
  const startedByReader = "ses_readOwnArchivedWork";
  there.engine.sessions.push({
    ...sessionOf({ id: startedByReader, title: "Archived work", directory: path.join(there.home, "chats") }),
    metadata: { "kuvo.archived": true, kuvo: { startedBy: sender } },
  });
  there.engine.messages.set(startedByReader, [
    { id: "msg_answered", type: "user", text: "[The user answered] Use the window table." },
  ]);
  there.engine.messagePages = new Map([[recipient, new Map([
    [null, { data: [{ id: "msg_undoBoundary", type: "user", text: "Undo this café detail." }],
      cursor: { next: "page/2" } }],
    ["page/2", { data: [
      { id: "msg_readAnswer", type: "assistant", content: [{ type: "text", text: "The café is booked." }] },
      { id: "msg_readQuestion", type: "user", text: "Can you book a table?" },
    ] }],
  ])]]);
  const sourceProcess = here.start(), targetProcess = there.start();
  const [source, target] = await Promise.all([sourceProcess.ready(), targetProcess.ready()]);
  await Promise.all([[source, sender], [target, recipient]].map(([resident, session]) =>
    until(async () => (await request(resident, "GET", "api/session/" + session)).status === 200,
      "The conversation's engine did not become ready")));
  await until(() => [main, member].every(peer => relay.has(Buffer.from(peer.address, "base64url"))),
    "The computers did not join the relay");
  async function read(id, to, read) {
    const tool = path.join(here.home, "run/replies", id);
    write(tool + ".alive", "");
    write(path.join(here.home, "run/outbox/sending", id + ".json"), JSON.stringify({
      from: sender, to, text: "Read the conversation", peer: member.key, wait: true, read, at: Date.now(),
    }));
    return until(() => {
      fs.utimesSync(tool + ".alive", new Date(), new Date());
      return fs.existsSync(tool + ".json") && readJSON(tool + ".json");
    }, () => "The original tool did not receive an answer for " + to + "\n"
      + sourceProcess.log() + "\n" + targetProcess.log(), 25000, 250);
  }
  const answer = await read("ltr_readConversation", recipient, { query: "cafe" });
  assert.deepEqual(answer, {
    read: true, text: "1 place mentions \"cafe\" (the rest left out as […]):\n\n**Reservations**\n\nThe café is booked.\n",
  });
  const refused = await read("ltr_readArchived", archivedSession, {});
  assert.deepEqual(refused, {
    failed: "It's archived, and only the conversation that started this work can read it.",
  });
  assert.deepEqual(await read("ltr_readItsOwnWork", startedByReader, {}), {
    read: true, text: "# Archived work\n\n**Your answer**\n\n[The user answered] Use the window table.\n",
  });
  assert.ok(!there.engine.asked.some(call => call.method === "POST" && call.path.endsWith("/prompt")));
  assert.equal(here.engine.messages.has(sender), false);
});

test("a tool that gives up leaves its letter on the way without waiting", { timeout: 35000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("gave-up-source"), there = await computer("gave-up-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const sender = "ses_gaveUpSender", recipient = "ses_gaveUpTarget", readTarget = "ses_gaveUpRead";
  here.engine.sessions.push(sessionOf({ id: sender, title: "Reader", directory: path.join(here.home, "chats") }));
  there.engine.sessions.push(sessionOf({ id: recipient, title: "Reservations", directory: path.join(there.home, "chats") }));
  there.engine.sessions.push(sessionOf({ id: readTarget, title: "Unopened transcript", directory: path.join(there.home, "chats") }));
  const [source, target] = await Promise.all([here.start().ready(), there.start().ready()]);
  await Promise.all([[source, sender], [target, recipient]].map(([resident, session]) =>
    until(async () => (await request(resident, "GET", "api/session/" + session)).status === 200,
      "The conversation's engine did not become ready")));
  await until(() => {
    try { return readJSON(path.join(there.home, "run/peer-agents.json"))[main.key]?.list.some(row => row.session === sender); }
    catch { return false; }
  }, "The destination did not learn the letter's sender");
  const id = "ltr_leaveWithoutWaiting", read = "ltr_cancelRead";
  const tool = path.join(here.home, "run/replies");
  const pending = path.join(here.home, "run/outbox/sending");
  const letter = { from: sender, to: recipient, text: "Is the table booked?", peer: member.key, wait: true, at: Date.now() };
  for (const [letterId, contents] of [[id, letter], [read, { ...letter, to: readTarget, read: {} }]]) {
    write(path.join(pending, letterId + ".json"), JSON.stringify(contents));
    write(path.join(tool, letterId + ".gaveup"), "");
  }
  const prompts = await until(() => there.engine.messages.get(recipient), "The letter never reached the destination");
  assert.equal(prompts.length, 1);
  assert.equal(prompts[0].text, `[Message from Reader · ${sender}] (To answer, use send_message to ${sender}.)\n\nIs the table booked?`);
  for (const letterId of [id, read]) {
    await until(() => !fs.existsSync(path.join(pending, letterId + ".json")), "The outbox did not retire " + letterId);
    assert.equal(fs.existsSync(path.join(tool, letterId + ".wait")), false);
    assert.equal(fs.existsSync(path.join(tool, letterId + ".gaveup")), false);
  }
  assert.ok(!there.engine.asked.some(call => call.path === "/api/session/" + readTarget + "/message"));
  // A finished tool can also leave a reply without writing a marker.
  const left = path.join(tool, "ltr_uncollectedAnswer.json");
  write(left, JSON.stringify({ to: sender, from: recipient, name: "Reservations", text: "Yes, it's booked." }));
  const recovered = await until(() => here.engine.messages.get(sender), "The late reply did not reach the local conversation");
  assert.equal(recovered.length, 1);
  assert.equal(recovered[0].text, `[Message from Reservations · ${recipient}] (To answer, use send_message to ${recipient}.)\n\nYes, it's booked.`);
  assert.ok(!fs.existsSync(left));
});

test("a letter that waits hears the authenticated answer after the sender restarts", {
  timeout: 65000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("waiting-letter-source"), there = await computer("waiting-letter-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const sender = "ses_waitingSender", recipient = "ses_waitingRecipient";
  here.engine.sessions.push(sessionOf({ id: sender, title: "Trip plans", directory: path.join(here.home, "chats") }));
  there.engine.sessions.push(sessionOf({ id: recipient, title: "Reservations", directory: path.join(there.home, "chats") }));
  const source = here.start();
  const target = there.start();
  const [src, dst] = await Promise.all([source.ready(), target.ready()]);
  const listed = (computer, peer, session) => {
    try { return readJSON(path.join(computer.home, "run/peer-agents.json"))[peer.key]?.list?.some(row => row.session === session); }
    catch { return false; }
  };
  await until(() => listed(here, member, recipient) && listed(there, main, sender),
    "The computers did not hear about the other conversations", 15000);
  // Wait until both residents can actually read their engine, as well as
  // having a saved announcement. An early retry can otherwise take a minute.
  await Promise.all([[src, sender], [dst, recipient]].map(([resident, session]) =>
    until(async () => (await request(resident, "GET", "api/session/" + session)).status === 200,
      "The conversation's engine did not become ready")));
  const letter = "ltr_waitForReservations";
  const tool = path.join(here.home, "run/replies", letter);
  const file = path.join(here.home, "run/outbox/sending", letter + ".json");
  const address = Buffer.from(main.address, "base64url");
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  let sent;
  t.after(relay.intercept(address, Buffer.from(member.address, "base64url"), packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), Buffer.from(main.key, "base64url"));
    if (JSON.parse(payload).letter?.id === letter) sent = packet;
    return false;
  }));
  write(tool + ".alive", "");
  write(file, JSON.stringify({ from: sender, to: recipient, text: "Is the train booked?", peer: member.key, wait: true, at: Date.now() }));
  const conversation = await until(() => there.engine.messages.get(recipient),
    () => "The letter never reached the other computer\n" + source.log() + "\n" + target.log(), 14000);
  assert.equal(conversation.length, 1);
  assert.match(conversation[0].text, /They're waiting for your reply/);
  await until(() => fs.existsSync(tool + ".wait"), "The sender did not save the awaited letter");
  await source.stop();
  await until(() => !relay.has(address), "The stopped sender did not leave the relay");
  const restarted = here.start();
  await restarted.ready();
  await until(() => relay.has(address), () => "The sender did not rejoin the relay\n" + restarted.log());
  fs.utimesSync(tool + ".alive", new Date(), new Date());
  conversation.push({ id: conversation[0].id.slice(0, -1) + "z0", type: "assistant",
    content: [{ type: "text", text: "The train is booked." }] });
  const answer = await until(() => {
    const now = new Date();
    fs.utimesSync(tool + ".alive", now, now);
    try { return readJSON(tool + ".json"); } catch { return false; }
  }, "The original tool never heard the answer", 15000, 1000);
  assert.deepEqual(answer, { text: "The train is booked.", to: sender, from: recipient, name: "Reservations" });
  assert.ok(!fs.existsSync(tool + ".wait"));
  assert.ok(!fs.existsSync(file));
  assert.equal(here.engine.messages.has(sender), false);

  assert.ok(sent, "The original letter crossed the relay");
  relay.replay(sent);
  const unknown = "ltr_unknownWaiter";
  const unknownReply = path.join(here.home, "run/replies", unknown + ".json");
  write(path.join(here.home, "run/outbox/sending", unknown + ".json"), JSON.stringify({
    from: "ses_unlisted", to: recipient, text: "Is lunch booked?", peer: member.key, wait: true, at: Date.now(),
  }));
  const notAnswerable = await until(() => fs.existsSync(unknownReply) && readJSON(unknownReply),
    "The unlisted sender did not hear that it couldn't receive a reply", 15000);
  assert.deepEqual(notAnswerable, { note: "Its answer can't come back to you: it doesn't know who you are." });
  assert.equal(conversation.length, 3, "Replaying the first letter does not start another turn");
  assert.equal(conversation.at(-1).text, `[Message from an agent on ${main.name} · unknown] (It can't be answered: who sent it isn't known.)\n\nIs lunch booked?`);
});

test("a refused letter becomes one queued note to the conversation that sent it", {
  timeout: 65000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("refused-letter-source"), there = await computer("refused-letter-target");
  const { main, member, spare } = seedPeerSetup(here.home, relay.url, 1, 3);
  seedPeerSetup(there.home, relay.url, 2, 3);
  const sender = "ses_letterSender", recipient = "ses_letterRecipient";
  here.engine.sessions.push(sessionOf({ id: sender, directory: path.join(here.home, "chats") }));
  there.engine.sessions.push(sessionOf({ id: recipient, archived: true, directory: path.join(there.home, "chats") }));
  const id = "ltr_refusedByOwner", sending = path.join(here.home, "run/outbox/sending");
  const file = path.join(sending, id + ".json");
  const { sealOneWay, openOneWay, Frame, frame } = await testMail(t, boxBinary(wireFixture));
  const ownKey = Buffer.from(main.key, "base64url"), address = Buffer.from(main.address, "base64url");
  const rejecting = Buffer.from(member.address, "base64url");
  let receipt;
  const release = relay.intercept(rejecting, address, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 1), Buffer.from(member.key, "base64url"));
    if (JSON.parse(payload).delivered?.id !== id) return false;
    receipt = packet;
    return true;
  });
  t.after(release);
  const other = relay.device(Buffer.alloc(32, 53));
  assert.equal(other.address.toString("base64url"), spare.address);
  t.after(() => other.leave());
  const hereProcess = here.start(), targetProcess = there.start();
  const [source, target] = await Promise.all([hereProcess.ready(), targetProcess.ready()]);
  await Promise.all([[source, sender], [target, recipient]].map(([resident, session]) =>
    until(async () => (await request(resident, "GET", "api/session/" + session)).status === 200,
      "The conversation's engine did not become ready")));
  write(file, JSON.stringify({ from: sender, to: recipient, text: "Hello", peer: member.key, wait: false, at: Date.now() }));
  await until(() => receipt, "The owner did not refuse the letter", 18000);

  const moveId = "mov_refusalHold", journal = path.join(here.home, "run/moves", moveId + ".json");
  write(journal, JSON.stringify({ id: moveId, thing: sender, name: "Held sender", kind: "chat",
    from: spare.key, to: main.key, peer: spare.key, role: "to", stage: "copying", sessions: [sender],
    started: Date.now(), safeSwitch: true, generation: 1, hash: "A".repeat(43) }));
  const forged = Buffer.from(JSON.stringify({ at: Date.now(), delivered: { id, ok: false, why: "A different computer said no." } }));
  const { message } = sealOneWay(forged, Buffer.alloc(32, 3), ownKey);
  other.send(address, frame(Frame.mail, 0, message));
  release();
  relay.replay(receipt);

  const notice = await until(() => fs.readdirSync(sending).find(name =>
    name.endsWith(".json") && name !== path.basename(file)), "No notice was saved for the held sender");
  assert.match(readJSON(path.join(sending, notice)).text, /That conversation doesn't take messages/);
  const prompts = () => here.engine.messages.get(sender) ?? [];
  assert.equal(prompts().length, 0);
  await hereProcess.stop();
  fs.unlinkSync(journal);
  await here.start().ready();
  await until(() => prompts().length, "The sender never heard the reason its letter was refused", 18000);
  const line = `Your message to that chat on ${member.name} wasn't delivered`;
  assert.equal(prompts()[0].text, `[Not delivered] ${line}: ${member.name} said "That conversation doesn't take messages."`);
  assert.deepEqual(prompts()[0].metadata.kuvo.note, { from: "Your other computers", line });
  assert.equal(prompts()[0].delivery, "queue");
  assert.ok(!fs.existsSync(file));
  assert.equal(prompts().length, 1);
  assert.equal(there.engine.messages.has(recipient), false);
});

test("an answer left by a tool reaches the new owner once", {
  timeout: 80000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("late-answer-source"), there = await computer("late-answer-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_lateAnswer", responder = "ses_lateLocalResponder", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, title: "Trip plans", directory }));
  here.engine.sessions.push(sessionOf({ id: responder, title: "Ada", directory }));
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const from = Buffer.from(main.address, "base64url"), to = Buffer.from(member.address, "base64url");
  const sender = Buffer.from(main.key, "base64url"), secret = Buffer.alloc(32, 2);
  let switching;
  const release = relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), secret, sender);
    if (JSON.parse(payload).move?.op !== "switch") return false;
    switching = packet;
    return true;
  });
  t.after(release);
  const sourceProcess = here.start(), targetProcess = there.start();
  const [source] = await Promise.all([sourceProcess.ready(), targetProcess.ready()]);
  const logs = () => sourceProcess.log() + "\n" + targetProcess.log();
  await until(() => {
    try { return readJSON(path.join(here.home, "run/peer-agents.json"))[member.key]?.features?.includes("moves.two-computers"); }
    catch { return false; }
  }, () => "The source did not hear the destination's move capability\n" + logs(), 15000);
  await readyToMove(source, session, member);
  await until(() => {
    try {
      const listed = readJSON(path.join(there.home, "run/peer-agents.json"))[main.key]?.list ?? [];
      return [session, responder].every(id => listed.some(agent => agent.session === id));
    } catch { return false; }
  }, "The source did not announce both local correspondents", 15000);
  const keptReplies = path.join(here.home, "run/replies");
  const sending = path.join(here.home, "run/outbox/sending");
  const local = "ltr_localWaitingAnswer", localTool = path.join(keptReplies, local);
  write(localTool + ".alive", "");
  write(path.join(sending, local + ".json"), JSON.stringify({
    from: session, to: responder, text: "Is the table reserved?", wait: true, at: Date.now(),
  }));
  const conversation = await until(() => here.engine.messages.get(responder), "The local letter did not reach Ada");
  assert.match(conversation[0].text, /They're waiting for your reply/);
  write(localTool + ".gaveup", "");
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  await until(() => switching, () => "The destination did not get a held copy\n" + logs(), 15000);
  const reply = path.join(here.home, "run/replies/ltr_lateAnswer");
  const abandonedRead = path.join(here.home, "run/replies/ltr_abandonedRead.json");
  const answer = "The venue is booked.";
  write(reply + ".gaveup", "");
  write(reply + ".json", JSON.stringify({ to: session, from: "ses_scout", name: "Scout", text: answer }));
  write(abandonedRead, JSON.stringify({ read: true, to: session, text: "The transcript." }));
  await until(() => !fs.existsSync(abandonedRead), "The resident did not process abandoned tool replies");
  const localAnswer = "The table is reserved.";
  conversation.push({ id: conversation[0].id.slice(0, -1) + "z0", type: "assistant",
    content: [{ type: "text", text: localAnswer }] });
  await until(() => [keptReplies, sending].some(folder => fs.readdirSync(folder).some(name => {
    if (!name.endsWith(".json")) return false;
    try { return readJSON(path.join(folder, name)).text === localAnswer; }
    catch (error) { if (error.code !== "ENOENT") throw error; return false; }
  })), () => "The local reply did not wait with the held conversation\n" + logs(), 18000);
  assert.equal(readJSON(reply + ".json").text, answer, "The reply is kept while its conversation is held");
  assert.equal(here.engine.messages.has(session), false);
  assert.deepEqual(there.engine.messages.get(session) ?? [], []);
  release();
  relay.replay(switching);
  await moveRetired(here, begun.json.move.id, logs);
  await until(() => !fs.existsSync(reply + ".json") &&
    there.engine.messages.get(session)?.some(message => message.text?.includes(answer)),
  "The late answer did not follow the moved chat", 17000);
  const received = there.engine.messages.get(session).filter(message => message.text?.includes(answer));
  assert.equal(received.length, 1);
  assert.match(received[0].text, /^\[Message from Scout · ses_scout\][^\n]*\n\nThe venue is booked\.$/);
  await until(() => there.engine.messages.get(session)?.some(message => message.text?.includes(localAnswer)),
    "Ada's answer did not follow the moved conversation", 18000);
  const replies = there.engine.messages.get(session).filter(message => message.text?.includes(localAnswer));
  assert.equal(replies.length, 1);
  assert.equal(replies[0].text, `[Message from Ada · ${responder}] (To answer, use send_message to ${responder}.)\n\n${localAnswer}`);
  assert.equal(here.engine.messages.has(session), false);
  assert.ok(!fs.existsSync(reply + ".gaveup"));
});

test("a managed attachment moves with both its typed references and its receipt", { timeout: 90000 }, async t => {
  const { computer, relay } = await bench(t);
  const source = await computer("attachment-source"), target = await computer("attachment-target");
  const { member } = seedPeerSetup(source.home, relay.url);
  seedPeerSetup(target.home, relay.url, 2);
  const session = "ses_attachmentMove", child = "ses_attachmentChild", message = "msg_attachedFile";
  const chats = path.join(source.home, "chats");
  fs.mkdirSync(chats, { recursive: true });
  source.engine.sessions.push(sessionOf({ id: session, directory: chats }));
  source.engine.sessions.push(sessionOf({ id: child, directory: chats, parent: session }));
  const original = path.join(source.home, "attachments", session, "proof space.txt");
  const outside = path.join(source.home, "export only.txt");
  const heading = "Open these attached files with your tools if you need them:";
  const block = file => `Here is the file.\n<attached-files>\n${heading}\n${file}\n</attached-files>`;
  const fileUrl = file => pathToFileURL(file).href + "?version=1#page";
  const seedReference = file => {
    source.engine.messages.set(session, [
      { id: "msg_quotedAttachment", type: "assistant", content: [{ type: "text", text: block(file) }] },
      { id: message, type: "user", content: [
      { type: "text", text: block(file) },
      { type: "file", mime: "text/plain", filename: "proof space.txt", url: fileUrl(file) },
    ] }]);
    source.engine.messages.set(child, [{ id: "msg_childAttachment", type: "user", content: [
      { type: "file", mime: "text/plain", filename: "proof space.txt", url: fileUrl(file) },
    ] }]);
  };
  write(outside, "This file stays with the source.");
  seedReference(outside);
  const sourceProcess = source.start(), targetProcess = target.start();
  const [here] = await Promise.all([sourceProcess.ready(), targetProcess.ready()]);
  await until(() => {
    try { return readJSON(path.join(source.home, "run/peer-agents.json"))[member.key]?.features?.includes("moves.two-computers"); }
    catch { return false; }
  }, "The source did not hear the destination's move capability", 15000);
  await readyToMove(here, session, member);
  const begin = async description => {
    const reply = await request(here, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true }, {}, 25000)
      .catch(error => { throw new Error(`Starting the ${description} move: ${sourceProcess.log()}`, { cause: error }); });
    assert.equal(reply.status, 202, reply.text);
    return reply.json.move.id;
  };
  const outcome = id => until(async () => {
    const reply = await request(here, "GET", "kuvo/moves/receipt?id=" + id, undefined, {}, 25000);
    if (reply.json?.state === "done" || reply.json?.state === "failed") return reply.json;
  }, "The move never published an outcome " + id, 30000, 100);
  const exportOnly = await outcome(await begin("export-only"));
  assert.equal(exportOnly.state, "failed");
  assert.match(exportOnly.message, /outside the managed roots/);
  const bytes = "These are the managed bytes.\n";
  write(original, bytes);
  fs.unlinkSync(original);
  seedReference(original);
  const refused = await outcome(await begin("expired attachment"));
  assert.equal(refused.state, "failed");
  assert.match(refused.message, /attachment.*missing|attachment.*unreadable/i);
  assert.ok(source.engine.sessions.some(each => each.id === session));
  assert.equal(target.engine.sessions.length, 0);

  write(original, bytes);
  const waiting = "msg_attachmentWaiting";
  write(path.join(source.home, "run/queues.json"), JSON.stringify({
    [session]: { formatVersion: 1, paused: true, held: [], userPaused: true, waiting: [{ id: waiting, text: "Here is the file.", at: 1 }] },
  }));
  write(path.join(source.home, "run/waiting", waiting + ".json"), JSON.stringify({
    formatVersion: 1, id: waiting, delivery: "queue", text: block(original),
    metadata: { kuvo: { parts: { words: "Here is the file.", paths: [original] } } },
    files: [{ name: "proof space.txt", uri: fileUrl(original) }],
  }));
  const move = await begin("available attachment");
  const completed = await outcome(move);
  assert.equal(completed.state, "done", JSON.stringify(completed));
  const receipt = readJSON(path.join(target.home, "run/moves/decisions", move + ".json"));
  const slot = createHash("sha256").update(original).digest("hex");
  const relative = `${session}/move-${move}/${slot}/proof space.txt`;
  const installed = path.join(target.home, "attachments", ...relative.split("/"));
  assert.deepEqual(receipt.payloads.sessions, [{
    from: original, relative, size: Buffer.byteLength(bytes), sha256: createHash("sha256").update(bytes).digest("base64url"),
  }]);
  await sourceProcess.stop();
  fs.unlinkSync(original);
  await targetProcess.stop();
  const reopened = await target.start().ready();
  const file = await request(reopened, "GET", `kuvo/file?withMode=1&uri=${encodeURIComponent(fileUrl(installed))}`);
  assert.equal(file.status, 200, file.text);
  assert.equal(file.text, bytes);
  const copied = target.engine.messages.get(session).find(entry => entry.id === message);
  assert.equal(copied.content[0].text, block(installed));
  assert.equal(copied.content[1].url, fileUrl(installed));
  assert.equal(target.engine.messages.get(session)[0].content[0].text, block(original));
  assert.equal(target.engine.messages.get(child)[0].content[0].url, fileUrl(installed));
  const saved = readJSON(path.join(target.home, "run/waiting", waiting + ".json"));
  assert.equal(saved.text, block(installed));
  assert.equal(saved.files[0].uri, "data:text/plain;base64," + Buffer.from(bytes).toString("base64"));
  assert.deepEqual(saved.metadata.kuvo.parts.paths, [installed]);
  assert.ok(target.engine.asked.some(call => call.method === "PUT"
    && call.path.endsWith("/instructions/entries/kuvo.moved")
    && call.json?.value.includes(`The attachment at ${original} is at ${installed} now.`)));
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
