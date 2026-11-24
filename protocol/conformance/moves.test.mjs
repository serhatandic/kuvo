// Both residents read the same move journals and outcomes through the client routes.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";
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
  const { main, member, spare } = seedPeerSetup(sender.home, relay.url, 1, 3);
  const { save } = seedPeerSetup(observer.home, relay.url, 2, 3);
  save("run/peer-reach.json", { [spare.key]: Date.now() - 3600_000 });
  save("run/peer-reach-checked.json", { [spare.key]: Date.now() });
  seedPeerSetup(other.home, relay.url, 3, 3);
  const journal = path.join(sender.home, "run/moves/mov_shown.json");
  const movement = {
    id: "mov_shown", kind: "chat", thing: "ses_shown", name: "Shown", from: spare.key, to: main.key,
    role: "to", stage: "copying", sessions: ["ses_shown"], sent: 30000, size: 30000,
  };
  write(journal, JSON.stringify({ ...movement,
    formatVersion: 1, peer: spare.key, started: Date.now(), complete: true, setUpAt: Date.now(),
  }));
  const from = Buffer.from(main.address, "base64url"), to = Buffer.from(member.address, "base64url");
  const held = [];
  const release = relay.intercept(from, to, packet => { held.push(packet); return true; });
  t.after(release);
  const sending = await sender.start().ready();
  await until(() => relay.has(from), "The computer with the first snapshot did not connect");
  const resident = await observer.start().ready();
  const listing = async record => {
    const answer = await request(record, "GET", "kuvo/moves");
    assert.equal(answer.status, 200, answer.text);
    return answer.json;
  };
  const pending = await listing(resident);
  assert.deepEqual(pending.pendingComputers.map(each => each.id), [main.key]);
  release();
  for (const packet of held) relay.replay(packet);
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
async function moveRetired(computer, id, logs, wait = 20000) {
  const directory = path.join(computer.home, "run/moves");
  const outcome = path.join(directory, id + ".outcome.json");
  const journal = path.join(directory, id + ".json");
  try {
    await new Promise((resolve, reject) => {
      const check = () => {
        if (fs.existsSync(outcome) && !fs.existsSync(journal)) done(resolve);
      };
      const changes = fs.watch(directory, check);
      const deadline = setTimeout(() => done(reject, new Error("Timed out waiting for move retirement")), wait);
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
    throw new Error(`The move ${id} did not settle (${active?.stage}/${active?.step}, generation ${active?.generation})\n${logs()}`, { cause: error });
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
  let firstPart, switching, dropProof, holdPart = true;
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), key(main));
    const move = JSON.parse(payload).move;
    if (move?.op === "part" && holdPart) {
      firstPart ??= packet;
      return true;
    }
    if (move?.op === "switch") {
      switching = move;
      return true;
    }
    return false;
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
  const conversation = "kuvo/sessions/" + session;
  const stopped = await request(source, "POST", conversation + "/stop");
  assert.equal(stopped.json?.paused, true, stopped.text);
  const queue = conversation + "/queue", waiting = ["msg_firstHeld", "msg_secondHeld"];
  for (const [index, id] of waiting.entries()) {
    const added = await request(source, "POST", queue + "/add", { id, text: "Waiting prompt " + index });
    assert.equal(added.json?.waits, true, added.text);
  }
  const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  await until(() => firstPart, "The source did not prepare its copy");
  const edits = await Promise.all([
    request(source, "DELETE", queue + "/" + waiting[0]),
    request(source, "POST", queue + "/add", { id: "msg_insertHeld", text: "Undo removal", before: waiting[0] }),
    request(source, "POST", queue + "/resume", {}),
  ]);
  for (const edit of edits) {
    assert.equal(edit.status, 409, edit.text);
    assert.match(edit.json.message, /owner changes|moving to another computer/);
  }
  holdPart = false;
  relay.replay(firstPart);
  const { id, generation, hash } = await until(() => switching, "The source did not attempt to switch");
  const send = (a, b, secret, receiver, move) => {
    const payload = Buffer.from(JSON.stringify({ at: Date.now(), move }));
    const { message } = sealOneWay(payload, Buffer.alloc(32, secret), key(receiver));
    relay.replay({ from: a, to: b, payload: frame(Frame.mail, 0, message) });
  };
  send(to, from, 2, main, { op: "failed", id, generation, hash, message: "The destination ran out of disk space." });
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
  const restored = await request(source, "GET", queue);
  assert.equal(restored.json?.paused, true, restored.text);
  assert.deepEqual(restored.json.waiting.map(item => item.id), waiting);
  for (const id of waiting) assert.ok(fs.existsSync(path.join(here.home, "run/waiting", id + ".json")));
  const removed = await request(source, "DELETE", queue + "/" + waiting[0]);
  assert.equal(removed.status, 200, removed.text);
  assert.deepEqual((await request(source, "GET", queue)).json.waiting.map(item => item.id), [waiting[1]]);
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

test("a failed old offer sends only its drop and waits for the destination's receipt", {
  timeout: 95000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("failed-offer");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const session = "ses_recoverFailed", legacy = "mov_failedOffer", held = "mov_failedDrop", done = "mov_failedDone";
  for (const [id, extra] of [
    [legacy, { step: "offer", offered: true }],
    [held, { step: "drop", dropUntilAck: true, dropSince: Date.now() - 2 * 86400_000 }],
    [done, { step: "done", offered: true }],
  ]) save("run/moves/" + id + ".json", {
    id, thing: session, name: "Saved conversation", kind: "chat", from: main.key, to: member.key, peer: member.key,
    role: "from", stage: "copying", sessions: [session], failed: "It didn’t get there. Try again.",
    failedStatus: 409, started: Date.now() - 100000, ...extra,
  });
  const { sealOneWay, openOneWay, readFrame, frame, Frame } = await testMail(t, boxBinary(wireFixture));
  const secret = Buffer.alloc(32, 2), key = Buffer.from(main.key, "base64url"), address = Buffer.from(main.address, "base64url");
  const wire = relay.device(Buffer.alloc(32, 52));
  t.after(() => wire.leave());
  const sends = new Map();
  t.after(relay.intercept(address, Buffer.from(member.address, "base64url"), packet => {
    const datagram = readFrame(packet.payload);
    if (datagram?.kind !== Frame.mail) return false;
    const move = JSON.parse(openOneWay(datagram.body, secret, key).payload).move;
    if (move) sends.set(move.id, [...(sends.get(move.id) ?? []), move.op]);
    return false;
  }));
  const process = here.start();
  const resident = await process.ready();
  const journal = id => path.join(here.home, "run/moves", id + ".json");
  await until(() => fs.existsSync(path.join(here.home, "run/moves", legacy + ".outcome.json"))
    && !fs.existsSync(journal(done)), "The failed moves did not record their outcomes");
  for (const id of [legacy, held]) assert.equal(readJSON(journal(id)).step, "drop");
  await until(() => sends.has(legacy) && sends.has(held) && !fs.existsSync(journal(done)),
    () => "The restarted source did not request both failed copies' removal "
      + JSON.stringify([...sends]) + "\n" + process.log(), 78000);
  for (const id of [legacy, held]) {
    assert.deepEqual(sends.get(id), ["drop"]);
    assert.equal(readJSON(journal(id)).step, "drop");
    const outcome = await request(resident, "GET", "kuvo/moves/receipt?id=" + id);
    assert.deepEqual(outcome.json, { state: "failed", message: "It didn’t get there. Try again." });
    const message = Buffer.from(JSON.stringify({ at: Date.now(), move: { id, op: "dropped", neverSwitched: true } }));
    wire.send(address, frame(Frame.mail, 0, sealOneWay(message, secret, key).message));
    await until(() => !fs.existsSync(journal(id)), "The failed journal did not retire after the authenticated drop");
  }
  assert.equal(sends.has(done), false);
});

test("a switched move keeps the original until its backup can be written", {
  timeout: 115000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
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
  here.engine.refuseExport = undefined;
  await until(() => !fs.existsSync(path.join(here.home, "run/moves", id + ".json")),
    () => "The same resident did not retry the failed backup\n" + sourceProcess.log() + "\n" + targetProcess.log(), 85000);
  assert.equal(exports() - beforeCleanup, 2);
  assert.equal(here.engine.sessions.some(record => record.id === session), false);
  const backup = readJSON(path.join(here.home, "run/moves/left.json")).find(row => row.id === id);
  assert.equal(readJSON(path.join(backup.trash, "conversation.json"))[0].info.id, session);
  const final = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
  assert.equal(final.status, 200, final.text);
  assert.equal(final.json.state, "done", final.text);
  assert.equal(final.json.note, undefined, final.text);
});

test("retiring a source after a failed deletion keeps its complete backup", {
  timeout: 65000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("partly-retired-source"), there = await computer("partly-retired-destination");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const root = "ses_partlyRetired", child = "ses_partlyRetiredChild", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: root, title: "Recover the whole chat", directory }),
    sessionOf({ id: child, title: "Keep the child", directory, parent: root }));
  for (const [session, text] of [[root, "Preserve the root"], [child, "Preserve the child"]]) {
    here.engine.messages.set(session, [{ id: "msg_" + session.slice(4), type: "user", text }]);
  }
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const [from, to] = [member, main].map(peer => Buffer.from(peer.address, "base64url"));
  const key = Buffer.from(member.key, "base64url"), secret = Buffer.alloc(32, 1);
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const move = JSON.parse(openOneWay(packet.payload.subarray(5), secret, key).payload).move;
    if (move?.op === "switched") here.engine.refuseDeletes.add(root);
    return false;
  }));
  const source = here.start(), target = there.start();
  const [resident] = await Promise.all([source.ready(), target.ready()]);
  await readyToMove(resident, root, member);
  const begun = await request(resident, "POST", "kuvo/moves", { kind: "chat", id: root, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  const id = begun.json.move.id, route = "kuvo/moves/receipt?id=" + id;
  await until(() => fs.existsSync(path.join(here.home, "run/moves", id + ".outcome.json")),
    "The source did not save its failed cleanup");
  const cleanup = await request(resident, "GET", route);
  assert.match(cleanup.json?.note, /original is retired/i);
  assert.ok(here.engine.sessions.some(row => row.id === root));
  assert.ok(!here.engine.sessions.some(row => row.id === child));
  assert.ok(there.engine.sessions.some(row => row.id === root));
  assert.ok(there.engine.sessions.some(row => row.id === child));
  await source.stop();
  here.engine.refuseDeletes.delete(root);
  here.engine.refuseExport = root;
  const restarted = here.start();
  const after = await restarted.ready();
  await moveRetired(here, id, () => restarted.log());
  assert.deepEqual(here.engine.sessions, []);
  const backup = readJSON(path.join(here.home, "run/moves/left.json")).find(row => row.id === id);
  const contents = readJSON(path.join(backup.trash, "conversation.json"));
  assert.deepEqual(contents.map(row => [row.info.id, row.messages[0].text]), [[root, "Preserve the root"], [child, "Preserve the child"]]);
  assert.equal((await request(after, "GET", route)).json.note, undefined);
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
      return answer?.id === movement.id && (expectReply === true || answer.op === expectReply) && answer;
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
  const older = "mov_olderSwitch";
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const from = Buffer.from(main.address, "base64url"), to = Buffer.from(member.address, "base64url");
  const unknownReplies = [];
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const move = JSON.parse(openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2),
      Buffer.from(main.key, "base64url")).payload).move;
    if (move?.id === unknown) unknownReplies.push(move.op);
    return false;
  }));
  save("run/moves/ended.json", { [older]: { as: "switched", at: Date.now() - 8 * 86400_000, peer: member.key } });
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
  await ask({ op: "switch", id: unknown, generation: 1, hash, safeSwitch: true }, false);
  await ask({ op: "drop", id: unknown, generation: 1, hash }, false);
  await ask({ op: "decision?", id: unknown, generation: 1, hash }, false);
  assert.deepEqual(await ask({ op: "switch", id: older, safeSwitch: true }),
    { op: "switched", id: older });
  assert.deepEqual(unknownReplies, ["failed"]);
});

test("a drop receipt cannot release an older switch in a managed setup", {
  timeout: 35000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("legacy-switch-fence");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const session = "ses_oldSwitch", id = "mov_oldSwitch";
  const pause = path.join(here.home, "wakes", session, "paused");
  write(pause, "moving:" + id);
  here.engine.sessions.push(sessionOf({ id: session, directory: path.join(here.home, "chats") }));
  const now = Date.now();
  save("run/moves/" + id + ".json", {
    id, thing: session, name: "Held legacy move", kind: "chat", from: main.key, to: member.key,
    peer: member.key, role: "from", stage: "switching", step: "switch", sessions: [session],
    started: now, switchSince: now, sentAt: now, safeSwitch: true,
  });
  const resident = await here.start().ready();
  await ask({ op: "dropped", id, neverSwitched: true }, false);
  const ready = await ask({ op: "check", id: "mov_checkpointBarrier", kind: "chat", name: "A separate check" }, "checked");
  assert.equal(ready.op, "checked");
  const receipt = await request(resident, "GET", "kuvo/moves/receipt?id=" + id);
  assert.equal(receipt.json?.state, "moving", receipt.text);
  assert.equal(fs.readFileSync(pause, "utf8"), "moving:" + id);
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

test("a drop waits for a destination import that has already started", {
  timeout: 50000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("drop-during-import");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const id = "mov_duringImport", session = "ses_duringImport", name = "Importing chat";
  const bundle = scratch();
  t.after(() => fs.rmSync(bundle, { recursive: true, force: true }));
  write(path.join(bundle, "move.json"), JSON.stringify({
    formatVersion: 1, kind: "chat", thing: session, name,
    exports: [{ info: sessionOf({ id: session, directory: path.join(here.home, "chats") }), messages: [] }],
  }));
  const file = path.join(bundle, "copy.tgz");
  const tar = spawnSync("tar", ["-czf", file, "-C", bundle, "move.json"], { encoding: "utf8", timeout: 5000 });
  assert.equal(tar.status, 0, tar.stderr);
  const data = fs.readFileSync(file), hash = createHash("sha256").update(data).digest("base64url");
  const binding = { id, generation: 1, hash };
  let released = false, working = false, tooSoon = false;
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const a = Buffer.from(main.address, "base64url"), b = Buffer.from(member.address, "base64url");
  const secret = Buffer.alloc(32, 2), key = Buffer.from(main.key, "base64url");
  t.after(relay.intercept(a, b, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const move = JSON.parse(openOneWay(packet.payload.subarray(5), secret, key).payload).move;
    if (move?.id === id && move.op === "working") working = true;
    if (move?.id === id && move.op === "dropped" && !released) tooSoon = true;
    return false;
  }));
  const gate = Promise.withResolvers();
  t.after(() => gate.resolve());
  const accepted = Promise.withResolvers();
  here.engine.holdImport = { id: session, accepted: accepted.resolve, finish: gate.promise };
  const process = here.start();
  const resident = await process.ready();
  await until(async () => (await request(resident, "GET", "api/session")).status === 200,
    "The destination engine did not become ready");
  assert.equal((await ask({ ...binding, op: "offer", from: member.key, kind: "chat",
    thing: session, name, size: data.length, safeSwitch: true, twoComputers: true })).op, "ready");
  assert.equal((await ask({ ...binding, op: "part", i: 0, of: 1, d: data.toString("base64url") }, "got")).op, "got");
  await accepted.promise;
  assert.equal(working, true);
  const dropping = ask({ ...binding, op: "drop" }, "dropped");
  const held = await request(resident, "GET", "kuvo/moves");
  assert.ok(held.json.away.includes(session));
  released = true;
  gate.resolve();
  const proof = await dropping;
  assert.equal(tooSoon, false);
  assert.equal(proof.neverSwitched, true);
  const journal = path.join(here.home, "run/moves", id + ".json");
  await until(() => !here.engine.sessions.some(row => row.id === session) && !fs.existsSync(journal),
    () => "The destination did not remove its interrupted copy\n"
      + JSON.stringify(fs.existsSync(journal) ? readJSON(journal) : null) + "\n"
      + JSON.stringify(here.engine.asked.filter(call => call.method === "DELETE")) + "\n" + process.log(), 22000);
  assert.deepEqual(await ask({ ...binding, op: "switch", safeSwitch: true }, "dropped"), proof);
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

test("incoming bundles keep independent conversations and import a repeated part only once", {
  timeout: 50000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("owned-import");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const local = "ses_importIndependent", directory = path.join(here.home, "chats");
  const original = sessionOf({ id: local, title: "Original chat", directory });
  here.engine.sessions.push(original);
  const resident = await here.start().ready();
  await until(async () => (await request(resident, "GET", "api/session/" + local)).status === 200,
    "The original conversation did not become ready");
  const records = scratch();
  t.after(() => fs.rmSync(records, { recursive: true, force: true }));
  const cases = [
    ["mov_claimLocal", "ses_claimLocal", [{ id: "ses_claimLocal" }, { id: local, parentID: "ses_claimLocal" }]],
    ["mov_wrongRoot", "ses_wrongRoot", [{ id: "ses_notTheRoot" }]],
    ["mov_badAncestor", "ses_badAncestor", [{ id: "ses_badAncestor" }, { id: "ses_unrelatedChild", parentID: local }]],
    ["mov_badCycle", "ses_badCycle", [{ id: "ses_badCycle", parentID: "ses_cycleChild" },
      { id: "ses_cycleChild", parentID: "ses_badCycle" }]],
  ];
  async function offer(id, thing, tree, wake = false) {
    const name = "Notes";
    write(path.join(records, "move.json"), JSON.stringify({
      formatVersion: 1, kind: "chat", thing, name, ...(wake ? { folder: thing } : {}),
      exports: tree.map(({ id, parentID }) => ({ info: sessionOf({ id, directory, parent: parentID }), messages: [] })),
    }));
    if (wake) write(path.join(records, thing, "wakes.json"), "[]");
    const archive = path.join(records, id + ".tgz");
    const packed = spawnSync("tar", ["-czf", archive, "-C", records, "move.json", ...(wake ? [thing] : [])],
      { encoding: "utf8", timeout: 5000 });
    assert.equal(packed.status, 0, packed.stderr);
    const bundle = fs.readFileSync(archive);
    assert.ok(bundle.length < 30000);
    const identity = { id, hash: createHash("sha256").update(bundle).digest("base64url"), generation: 1 };
    assert.equal((await ask({ ...identity, op: "offer", from: member.key, kind: "chat", thing, name,
      size: bundle.length, safeSwitch: true, twoComputers: true })).op, "ready");
    return { identity, part: { ...identity, op: "part", i: 0, of: 1, d: bundle.toString("base64url") } };
  }
  for (const [id, thing, tree] of cases) {
    const { identity, part } = await offer(id, thing, tree);
    assert.equal((await ask(part)).op, "got");
    const failed = await ask({ ...identity, op: "arrive" }, "failed");
    assert.equal(failed.op, "failed", JSON.stringify(failed));
    assert.deepEqual(here.engine.sessions, [original]);
  }
  for (const fault of ["lookup", "conflict", "lostReply"]) {
    const root = "ses_root" + fault, child = "ses_child" + fault;
    const other = sessionOf({ id: child, title: "An independent conversation", directory });
    const folder = path.join(here.home, "wakes", root);
    let placed = false;
    const { identity, part } = await offer("mov_import" + fault, root, [{ id: root }, { id: child, parentID: root }], true);
    here.engine.beforeImport = id => {
      if (id !== root) return;
      placed = fs.existsSync(path.join(folder, "wakes.json"));
      if (fault === "conflict") here.engine.sessions.push(other);
    };
    if (fault === "lookup") {
      here.engine.sessions.push(other);
      here.engine.refuseLookup = child;
    } else if (fault === "lostReply") {
      here.engine.loseImportReply = child;
    }
    const before = here.engine.asked.length;
    assert.equal((await ask(part, "got")).op, "got");
    const failed = await ask({ ...identity, op: "arrive" }, "failed");
    assert.equal(failed.op, "failed", fault);
    await until(() => !fs.existsSync(path.join(here.home, "run/moves", identity.id + ".json")),
      "The destination did not clean up after " + fault);
    assert.deepEqual(here.engine.sessions, fault === "lostReply" ? [original] : [original, other]);
    assert.equal(placed, fault !== "lookup");
    assert.ok(!fs.existsSync(folder));
    const imports = here.engine.asked.slice(before).filter(call => call.path === "/api/experimental/session/import");
    assert.deepEqual(imports.map(call => call.json.info.id), fault === "lookup" ? [] : [root, child]);
    here.engine.sessions = [original];
    here.engine.refuseLookup = null;
    here.engine.beforeImport = null;
  }
  const moved = "ses_duplicatePart";
  const { identity, part } = await offer("mov_duplicatePart", moved, [{ id: moved }]);
  const imports = () => here.engine.asked.filter(call =>
    call.method === "POST" && call.path === "/api/experimental/session/import" && call.json.info?.id === moved).length;
  await ask(part, false);
  assert.equal((await ask(part, "got")).op, "got");
  assert.equal((await ask({ ...identity, op: "arrive" }, "arrived")).op, "arrived");
  assert.equal(imports(), 1);
  assert.ok((await request(resident, "GET", "kuvo/moves")).json.away.includes(moved));
  const switching = { ...identity, op: "switch", safeSwitch: true };
  const switched = await ask(switching, "switched");
  await ask(part, false);
  assert.deepEqual(await ask(switching, "switched"), switched);
  assert.equal(imports(), 1);
  assert.ok(!(await request(resident, "GET", "kuvo/moves")).json.away.includes(moved));
});

test("a source can replace its partial offer but must explicitly drop a complete copy", {
  timeout: 45000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("replaced-offer");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const session = "ses_replacedOffer", thing = session, name = "Offered conversation";
  const hash = "A".repeat(43), generation = 1;
  const movement = id => ({ id, hash, generation });
  const offer = (id, size = 60000, bound = hash) => ({ ...movement(id), hash: bound, op: "offer", from: member.key,
    kind: "chat", thing, name, size, safeSwitch: true, twoComputers: true });
  const resident = await here.start().ready();
  await until(async () => (await request(resident, "GET", "api/session")).status === 200,
    "The destination engine did not become ready");

  const initial = "mov_partialOriginal", newer = "mov_newOffer";
  assert.equal((await ask(offer(initial))).op, "ready");
  assert.equal((await ask({ ...movement(initial), op: "part", i: 0, of: 2,
    d: Buffer.alloc(30000).toString("base64url") }, "got")).op, "got");
  const next = await ask(offer(newer));
  assert.equal(next.op, "ready", JSON.stringify(next));
  assert.equal((await ask({ ...movement(initial), op: "drop" }, "dropped")).neverSwitched, true);
  assert.equal((await ask({ ...movement(newer), op: "drop" }, "dropped")).op, "dropped");

  const files = scratch();
  t.after(() => fs.rmSync(files, { recursive: true, force: true }));
  write(path.join(files, "move.json"), JSON.stringify({ formatVersion: 1, kind: "chat", thing, name,
    exports: [{ info: sessionOf({ id: session, directory: path.join(here.home, "chats") }), messages: [] }] }));
  const tgz = path.join(files, "copy.tgz");
  const built = spawnSync("tar", ["-czf", tgz, "-C", files, "move.json"], { encoding: "utf8", timeout: 5000 });
  assert.equal(built.status, 0, built.stderr);
  const data = fs.readFileSync(tgz), verified = createHash("sha256").update(data).digest("base64url");
  const complete = "mov_completeOriginal", replacement = "mov_completeReplacement";
  const current = { id: complete, generation, hash: verified };
  assert.equal((await ask(offer(complete, data.length, verified))).op, "ready");
  assert.equal((await ask({ ...current, op: "part", i: 0, of: 1, d: data.toString("base64url") }, "got")).op, "got");
  assert.equal((await ask({ ...current, op: "arrive" }, "arrived")).op, "arrived");
  assert.equal((await ask(offer(replacement))).op, "refused");
  assert.ok(here.engine.sessions.some(row => row.id === session));
  assert.equal((await ask({ ...current, op: "drop" }, "dropped")).neverSwitched, true);
  assert.equal((await ask(offer(replacement))).op, "ready");
  assert.ok(!here.engine.sessions.some(row => row.id === session));
});

test("incoming archive links cannot touch files outside the move", {
  timeout: 55000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("archive-links");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const resident = await here.start().ready();
  await until(async () => (await request(resident, "GET", "api/session")).status === 200,
    "The destination engine did not become ready");
  const retained = path.join(here.home, "private-chat.txt");
  write(retained, "PRIVATE-ef3e retained here");

  // Encode a small ustar archive without requiring permission to make links on the test host.
  const tar = entries => {
    const blocks = entries.flatMap(([name, type, content]) => {
      const data = Buffer.from(type === "0" ? content : "");
      const header = Buffer.alloc(512);
      header.write(name, 0, 100);
      header.write("0000700", 100);
      header.write(data.length.toString(8).padStart(11, "0"), 124);
      header.fill(0x20, 148, 156);
      header.write(type, 156);
      if (type === "2") header.write(content, 157, 100);
      header.write("ustar\0", 257);
      header.write("00", 263);
      header.write([...header].reduce((a, b) => a + b, 0).toString(8).padStart(6, "0") + "\0 ", 148);
      const padded = Buffer.alloc(Math.ceil(data.length / 512) * 512);
      data.copy(padded);
      return [header, padded];
    });
    return gzipSync(Buffer.concat([...blocks, Buffer.alloc(1024)]));
  };
  const manifest = session => JSON.stringify({
    formatVersion: 1, kind: "chat", thing: session, name: "Notes", folder: session,
    exports: [{ info: sessionOf({ id: session, directory: path.join(here.home, "chats") }), messages: [] }],
  });
  const cases = [
    ["ManifestLink", () => [["move.json", "2", retained]], /isn’t a move/],
    ["FolderLink", session => [["move.json", "0", manifest(session)], [session, "2", here.home]], /link/],
    ["OutsideLink", session => [["move.json", "0", manifest(session)], [session, "5", ""],
      [session + "/outside", "2", "../../private-chat.txt"]], /link/],
    ["ChainedLink", session => [["move.json", "0", manifest(session)], [session, "5", ""],
      [session + "/d", "5", ""], [session + "/e", "5", ""], [session + "/e/notes.txt", "0", "Notes"],
      [session + "/d/L", "2", "../e"], [session + "/A", "2", "d/L/../../move.json"]], /link/],
  ];
  for (const [label, entries, reason] of cases) {
    const session = "ses_" + label;
    const bytes = tar(entries(session));
    const identity = { id: "mov_" + label, generation: 1, hash: createHash("sha256").update(bytes).digest("base64url") };
    assert.equal((await ask({ ...identity, op: "offer", from: member.key, kind: "chat",
      thing: session, name: "Notes", size: bytes.length, safeSwitch: true, twoComputers: true })).op, "ready");
    assert.equal((await ask({ ...identity, op: "part", i: 0, of: 1, d: bytes.toString("base64url") }, "got")).op, "got");
    const failed = await ask({ ...identity, op: "arrive" }, "failed");
    assert.match(failed.message, reason, label);
    assert.doesNotMatch(failed.message, /PRIVATE-ef3e/);
    assert.equal(fs.readFileSync(retained, "utf8"), "PRIVATE-ef3e retained here");
    assert.ok(!here.engine.sessions.some(row => row.id === session));
    assert.ok(!fs.existsSync(path.join(here.home, "wakes", session)));
  }
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

test("a partial copy that never reached the engine can be dropped with the engine down", {
  timeout: 35000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("bare-copy");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const ask = await moveMail(t, relay, main);
  const identity = { id: "mov_bareCopy", generation: 1, hash: "A".repeat(43) };
  const folder = path.join(here.home, "run/moves", identity.id);
  save("run/moves/" + identity.id + ".json", {
    ...identity, thing: "ses_bareCopy", name: "Notes", kind: "chat", from: member.key, to: main.key,
    peer: member.key, role: "to", stage: "copying", sessions: [], started: Date.now(), safeSwitch: true,
    size: 20, sent: 0,
  });
  write(path.join(folder, "bundle.tgz"), "an unfinished copy");
  here.engine.refuseActive = true;
  const resident = await here.start().ready();
  const answer = await ask({ ...identity, op: "drop" });
  assert.deepEqual(answer, { ...identity, op: "dropped", neverSwitched: true });
  await until(() => !fs.existsSync(folder), "The destination kept its unfinished copy");
  const after = await request(resident, "GET", "kuvo/moves");
  assert.deepEqual(after.json.moves, []);
  assert.deepEqual(after.json.away, []);
  assert.equal(here.engine.sessions.length, 0);
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

test("a move recovers when every part in its first window is lost", {
  timeout: 75000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("lost-window-source"), there = await computer("lost-window-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_lostInitialWindow", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, title: "Long conversation", directory }));
  const bytes = Buffer.concat(Array.from({ length: 18_000 }, (_, index) => createHash("sha256").update(String(index)).digest()));
  here.engine.messages.set(session, [{ id: "msg_lostWindow", type: "user", text: bytes.toString("base64"),
    content: [{ type: "text", text: bytes.toString("base64") }] }]);
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const first = new Set(), retries = [];
  const a = Buffer.from(main.address, "base64url"), b = Buffer.from(member.address, "base64url");
  const receiver = Buffer.alloc(32, 2), sender = Buffer.from(main.key, "base64url");
  t.after(relay.intercept(a, b, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const move = JSON.parse(openOneWay(packet.payload.subarray(5), receiver, sender).payload).move;
    if (move?.op !== "part") return false;
    if (first.size < 16 && !first.has(move.i)) {
      first.add(move.i);
      return true;
    }
    if (first.has(move.i)) retries.push(move.i);
    return false;
  }));
  const source = here.start(), destination = there.start();
  const [resident] = await Promise.all([source.ready(), destination.ready()]);
  await readyToMove(resident, session, member);
  const begun = await request(resident, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(begun.status, 202, begun.text);
  await until(() => first.size === 16, "The source did not fill the first window");
  assert.equal(there.engine.sessions.length, 0);
  await until(() => retries.length >= 8, "The source did not retry its lost parts", 30000);
  assert.deepEqual(retries.slice(0, 8), [0, 1, 2, 3, 4, 5, 6, 7]);
  await moveRetired(here, begun.json.move.id, () => source.log() + "\n" + destination.log());
  const outcome = await request(resident, "GET", "kuvo/moves/receipt?id=" + begun.json.move.id);
  assert.equal(outcome.json?.state, "done", outcome.text);
  assert.equal(there.engine.messages.get(session)[0].text, bytes.toString("base64"));
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

test("handoff preserves a settled forwarding decision and rejects a conflicting one", { timeout: 60000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("handoff-source"), there = await computer("handoff-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const setup = readJSON(path.join(here.home, "run/link.json")).setup.id;
  const directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  const cases = [
    { session: "ses_keepSettledDecision", operation: "settledQueue", conflict: false },
    { session: "ses_conflictingDecision", operation: "differentQueue", conflict: true },
  ];
  for (const entry of cases) {
    const { session, operation, conflict } = entry;
    here.engine.sessions.push(sessionOf({ id: session, directory }));
    const address = createHash("sha256").update(JSON.stringify(["scoped-forward", setup, operation])).digest("base64url");
    const file = "run/owned-data/mutations/" + address + ".json";
    const record = { formatVersion: 1, digest: "A".repeat(43), at: 1, forward: {
      kind: "forwardedSessionDecisions", setup, operation, owner: main.key, submitter: { kind: "computer" },
      area: "ownership", record: session, ref: { kind: "sessionQueue", id: session },
    } };
    const settled = { ...record, at: 2, answer: { status: 200, value: { paused: true } } };
    const sending = conflict ? { ...record, answer: { status: 200, value: { paused: false } } } : record;
    write(path.join(here.home, file), JSON.stringify(sending));
    write(path.join(there.home, file), JSON.stringify(settled));
    Object.assign(entry, { file, settled });
  }
  const sourceProcess = here.start(), targetProcess = there.start();
  const [source] = await Promise.all([sourceProcess.ready(), targetProcess.ready()]);
  await until(() => {
    try { return readJSON(path.join(here.home, "run/peer-agents.json"))[member.key]?.features?.includes("data.forward-evidence"); }
    catch { return false; }
  }, "The destination did not announce support for forwarding evidence");
  for (const { session, conflict, file, settled } of cases) {
    await readyToMove(source, session, member);
    const begun = await request(source, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
    assert.equal(begun.status, 202, begun.text);
    const { id } = begun.json.move;
    await moveRetired(here, id, () => sourceProcess.log() + "\n" + targetProcess.log());
    const result = await request(source, "GET", "kuvo/moves/receipt?id=" + id);
    assert.equal(result.status, 200, result.text);
    assert.equal(result.json.state, conflict ? "failed" : "done", result.text);
    assert.deepEqual(readJSON(path.join(there.home, file)), settled);
    assert.equal(here.engine.sessions.some(row => row.id === session), conflict);
    await until(() => there.engine.sessions.some(row => row.id === session) === !conflict,
      "The destination kept the wrong conversation after handoff");
  }
});

test("a conversation that keeps changing stays with its source after three offers", {
  timeout: 110000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
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
  await moveRetired(here, id, () => "Offered generations " + [...generations].join(", ")
    + "\n" + sourceProcess.log() + "\n" + targetProcess.log(), 85000);
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

test("a computer that predates moves is listed as needing an update and refuses admission", { timeout: 35000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("new-move-source");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const id = "ses_needsMoveSupport", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id, title: "Trip plans", directory }));
  save("run/peer-agents.json", { [member.key]: { at: Date.now(), kind: "machine", list: [], features: ["sessions", "bots"] } });
  const old = relay.device(Buffer.alloc(32, 52));
  t.after(() => old.leave());
  const source = await here.start().ready();
  await until(() => relay.has(Buffer.from(main.address, "base64url")), "The source did not join the relay");
  await until(async () => (await request(source, "GET", "api/session/" + id)).status === 200,
    "The local conversation did not become ready");
  const places = await request(source, "GET", "kuvo/moves/places?kind=chat&id=" + id);
  const message = `${member.name} needs a newer Kuvo Code before things can move to or from it.`;
  assert.deepEqual(places.json.places.find(place => place.key === member.key), {
    key: member.key, name: member.name, kind: "machine", ok: false, why: message, note: "Needs a newer Kuvo Code",
  });
  const refused = await request(source, "POST", "kuvo/moves", { kind: "chat", id, to: member.key, receipt: true });
  assert.equal(refused.status, 409, refused.text);
  assert.deepEqual(refused.json, { message });
  assert.equal(here.engine.sessions[0].id, id);
});

test("move choices say which delegated work and unanswered questions stay home", { timeout: 45000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("stays-source"), there = await computer("stays-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const id = "ses_staysLead", first = "ses_staysWork", second = "ses_staysGrandchild", unrelated = "ses_staysOther";
  const directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  const work = (job, startedBy) => ({
    ...sessionOf({ id: job, title: "Delegated work", directory }),
    metadata: { kuvo: { startedBy } },
  });
  here.engine.sessions.push(sessionOf({ id, title: "Trip plans", directory }),
    work(first, id), work(second, first), sessionOf({ id: unrelated, title: "Other plans", directory }),
    { ...work("ses_archivedWork", id), metadata: { "kuvo.archived": true, kuvo: { startedBy: id } } });
  const ask = { id: "ask_stays", action: "makeProject", session: id, name: "Trip plans",
    project: "trip", folderName: "trip", folder: null, at: Date.now(), given: { name: "trip" } };
  write(path.join(here.home, "run/work/asks.json"), JSON.stringify({ asks: [ask, {
    ...ask, id: "ask_alreadyAnswered", answered: { answer: "allow", text: "Done.", isError: false, at: Date.now() },
  }] }));
  const [source] = await Promise.all([here.start().ready(), there.start().ready()]);
  await readyToMove(source, id, member);
  const pathFor = session => "kuvo/moves/places?kind=chat&id=" + session;
  const choices = await request(source, "GET", pathFor(id));
  assert.equal(choices.status, 200, choices.text);
  assert.deepEqual(choices.json.stays, { asks: 1, sessions: 2 }, choices.text);
  const message = choices.json.places.find(place => place.key === member.key).confirm.message;
  assert.ok(message.includes(`Sessions it started stay on ${main.name}. It can still reach them from ${member.name}. Its unanswered questions stay on ${main.name}.`));
  assert.equal((await request(source, "GET", pathFor(unrelated))).json.stays, undefined);
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

test("an unavailable destination or missing sign-in refuses before work is held", {
  timeout: 65000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("account-source"), there = await computer("account-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
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

  const { sealOneWay, openOneWay, frame, Frame } = await testMail(t, boxBinary(wireFixture));
  const note = `On ${place}, Account chat uses the account signed in there.`;
  const a = Buffer.from(member.address, "base64url"), b = Buffer.from(main.address, "base64url");
  t.after(relay.intercept(a, b, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const payload = JSON.parse(openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 1),
      Buffer.from(member.key, "base64url")).payload);
    if (payload.move?.op !== "checked" || !payload.move.ok) return false;
    payload.move.note = note;
    const { message } = sealOneWay(Buffer.from(JSON.stringify(payload)), Buffer.alloc(32, 2),
      Buffer.from(main.key, "base64url"));
    relay.replay({ ...packet, payload: frame(Frame.mail, 0, message) });
    return true;
  }));
  there.engine.providers.push({ id: "needs-sign-in" });
  const retry = await move();
  assert.equal(retry.status, 202, retry.text);
  await moveRetired(here, retry.json.move.id, () => "The provider check warned: " + note);
  const moved = await status(retry.json.move.id);
  assert.equal(moved.json?.state, "done", moved.text);
  assert.equal(moved.json.note, note);
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
  timeout: 90000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("wake-source"), there = await computer("wake-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_wakeMove", caller = "ses_returningLetterSender";
  const sourceWakes = path.join(path.dirname(here.home), "My wake-ups");
  const folder = path.join(sourceWakes, session);
  const target = path.join(there.home, "wakes", session);
  const sourceChats = path.join(path.dirname(here.home), "My chats");
  fs.mkdirSync(sourceChats, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, title: "Wake-up chat", directory: sourceChats }),
    sessionOf({ id: caller, title: "Reader", directory: sourceChats }));
  const savedPause = "user-paused\n", wake = "Keep the existing wake-up.\n";
  write(path.join(folder, "paused"), savedPause);
  write(path.join(folder, "directory"), sourceChats);
  write(path.join(folder, "wake.txt"), wake);
  if (process.platform !== "win32") fs.symlinkSync("wake.txt", path.join(folder, "current"));
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
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

  const noteId = "ltr_noteAtNewOwner", words = "😀".repeat(30000);
  const asked = "[The user answered] " + words, line = "You answered: " + words;
  let sentNote;
  const stopWatchingNote = relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), remote);
    const letter = JSON.parse(payload).letter;
    if (letter?.id === noteId) sentNote = letter;
    return false;
  });
  t.after(stopWatchingNote);
  write(path.join(here.home, "run/outbox/sending", noteId + ".json"), JSON.stringify({
    from: null, to: session, text: asked, at: Date.now(), peer: main.key, wait: false,
    note: { from: "Your answer", words, line, delivery: "queue" },
  }));
  const savedNote = await until(() => there.engine.messages.get(session)?.find(row => row.text === sentNote?.text),
    "The former computer's note did not reach the new owner", 15000);
  assert.equal(sentNote.note.from, "Your answer");
  assert.equal(sentNote.note.delivery, "queue");
  for (const [whole, part, prefix] of [
    [asked, sentNote.text, "[The user answered] 😀"], [words, sentNote.note.words, "😀"],
    [line, sentNote.note.line, "You answered: 😀"],
  ]) {
    assert.ok(part.startsWith(prefix) && part.endsWith("😀") && whole.startsWith(part));
    assert.ok(Buffer.byteLength(part) <= 24000);
  }
  assert.equal(savedNote.delivery, "queue");
  assert.deepEqual(savedNote.metadata.kuvo.note, { from: "Your answer", line: sentNote.note.line });
  assert.equal(here.engine.messages.has(session), false);
  stopWatchingNote();

  // The original computer must learn the imported session from its event
  // even while it cannot refresh the full engine list.
  here.engine.refuseWholeList = true;
  let returnSwitch;
  const releaseReturn = relay.intercept(to, from, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 1), Buffer.from(member.key, "base64url"));
    if (JSON.parse(payload).move?.op !== "switch") return false;
    returnSwitch = packet;
    return true;
  });
  t.after(releaseReturn);
  fs.unlinkSync(path.join(target, "paused"));
  await readyToMove(destination, session, main);
  const back = await request(destination, "POST", "kuvo/moves", { kind: "chat", id: session, to: main.key, receipt: true })
    .catch(error => { throw new Error("Starting the return move\n" + dstProcess.log(), { cause: error }); });
  assert.equal(back.status, 202, back.text);
  await until(() => returnSwitch, "The returning copy did not reach its activation");
  assert.equal(fs.readFileSync(path.join(left.trash, "wake.txt"), "utf8"), wake);
  releaseReturn();
  relay.replay(returnSwitch);
  await moveRetired(there, back.json.move.id, logs);
  const returned = await request(destination, "GET", "kuvo/moves/receipt?id=" + back.json.move.id);
  assert.equal(returned.status, 200, returned.text);
  assert.equal(returned.json.state, "done", returned.text);
  await until(() => !fs.existsSync(left.trash), "The old backup survived the conversation's return");
  assert.ok(!fs.existsSync(target));
  assert.ok(!fs.existsSync(path.join(folder, "paused")));
  assert.equal(fs.readFileSync(path.join(folder, "wake.txt"), "utf8"), wake);
  assert.equal(fs.readFileSync(path.join(folder, "directory"), "utf8"), sourceChats);

  if (binary.typescript) {
    // The TS outbox uses the local agent publication when a stale peer is
    // recorded on the letter. Wait for the returned conversation to be listed.
    const published = path.join(here.home, "run/agents.json");
    await until(() => {
      try {
        return readJSON(published).local?.includes(session);
      } catch { return false; }
    }, () => "The source did not publish its returned conversation\n"
      + (fs.existsSync(published) ? fs.readFileSync(published, "utf8") : "(no local publication)") + "\n" + logs(), 35000);
  }
  const letter = "ltr_returnedOwner", tool = path.join(here.home, "run/replies", letter);
  let mailed = 0;
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const { payload } = openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2), remote);
    if (JSON.parse(payload).letter?.id === letter) mailed++;
    return false;
  }));
  write(tool + ".alive", "");
  const sending = path.join(here.home, "run/outbox/sending");
  write(path.join(sending, letter + ".json"), JSON.stringify({
    from: caller, to: session, text: "Are the wake-ups here?", peer: member.key, wait: true, at: Date.now(),
  }));
  const conversation = await until(() => here.engine.messages.get(session)?.some(row => row.text?.includes("Are the wake-ups here?")) && here.engine.messages.get(session),
    "The old outbox address did not resolve to the local conversation", 15000);
  assert.equal(conversation.length, 2);
  const question = conversation.at(-1);
  assert.equal(question.text, `[Message from Reader · ${caller}] (They're waiting for your reply: just answer, in your words.)\n\nAre the wake-ups here?`);
  conversation.push({ id: question.id.slice(0, -1) + "z0", type: "assistant",
    content: [{ type: "text", text: "They’re here." }] });
  const reply = await until(() => {
    fs.utimesSync(tool + ".alive", new Date(), new Date());
    return fs.existsSync(tool + ".json") && readJSON(tool + ".json");
  }, "The local answer did not reach the waiting tool", 18000, 500);
  assert.deepEqual(reply, { from: session, to: caller, name: "Wake-up chat", text: "They’re here." });
  assert.equal(mailed, 0);
  const news = "[Session finished] \"Sign-in\" is done.";
  write(path.join(sending, "ltr_noteAtOldOwner.json"), JSON.stringify({
    to: session, text: news, at: Date.now(), peer: member.key, wait: false,
    note: { from: "Its sessions", words: "“Sign-in” finished", line: "“Sign-in” finished", delivery: "queue" },
  }));
  const localNote = await until(() => conversation.find(row => row.text === news), "The returned conversation did not receive its local note");
  assert.equal(localNote.delivery, "queue");
  assert.deepEqual(localNote.metadata.kuvo.note, { from: "Its sessions", line: "“Sign-in” finished" });
  assert.ok(!there.engine.sessions.some(row => row.id === session));
  assert.ok(!there.engine.messages.get(session)?.some(row => row.text === news));
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
  there.engine.refuseMessagePage = { session: recipient, cursor: "page/2" };
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

test("pending letters reach a peer in creation order", { timeout: 40000 }, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("ordered-letter-source"), there = await computer("ordered-letter-target");
  const { member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const recipient = "ses_orderedRecipient";
  there.engine.sessions.push(sessionOf({ id: recipient, title: "Reservations", directory: path.join(there.home, "chats") }));
  const target = await there.start().ready();
  await until(async () => (await request(target, "GET", "api/session/" + recipient)).status === 200,
    "The destination's engine did not become ready");
  const now = Date.now(), folder = path.join(here.home, "run/outbox/sending");
  for (const [index, name] of ["third", "second", "first"].entries()) {
    write(path.join(folder, "ltr_order_" + index + ".json"), JSON.stringify({
      to: recipient, text: name, peer: member.key, wait: false, at: now - (index + 1) * 1000,
    }));
  }
  await here.start().ready();
  const messages = await until(() => {
    const prompts = there.engine.messages.get(recipient);
    return prompts?.length === 3 && prompts;
  }, "The ordered letters did not all arrive", 18000);
  assert.deepEqual(messages.map(prompt => prompt.text.split("\n\n").at(-1)), ["first", "second", "third"]);
});

test("only its lead can address an old conversation that is no longer listed", {
  timeout: 45000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("older-letter-target");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const lead = "ses_remoteLead", unrelated = "ses_remoteOther", old = "ses_olderChat", latest = "ses_newestChat";
  const arrived = "ses_unlistedArrival", at = Date.now();
  const directory = path.join(here.home, "chats");
  here.engine.sessions.push({
    ...sessionOf({ id: old, title: "Older delegated conversation", directory }),
    metadata: { kuvo: { startedBy: lead, depth: 1 } },
  });
  here.engine.sessions.push(sessionOf({ id: arrived, title: "Recently moved conversation", directory }));
  save("run/moves/ended.json", { mov_unlistedArrival: { as: "switched", at, peer: member.key, sessions: [arrived] } });
  save("run/moves/moved.json", [{ session: arrived, key: main.key, at, arrived: true }]);
  for (let index = 2; index <= 126; index++) {
    const id = index === 126 ? latest : "ses_filler" + index;
    here.engine.sessions.push(sessionOf({ id, title: "Recent conversation " + index, directory, updated: index }));
  }
  const { sealOneWay, openOneWay, readFrame, frame, Frame } = await testMail(t, boxBinary(wireFixture));
  const secret = Buffer.alloc(32, 2), key = Buffer.from(main.key, "base64url"), address = Buffer.from(main.address, "base64url");
  const wire = relay.device(Buffer.alloc(32, 52));
  t.after(() => wire.leave());
  const send = data => {
    const bytes = Buffer.from(JSON.stringify({ ...data, at: Date.now() }));
    wire.send(address, frame(Frame.mail, 0, sealOneWay(bytes, secret, key).message));
  };
  const receive = async () => {
    const packet = readFrame((await wire.next(15000)).subarray(17));
    return packet?.kind === Frame.mail && JSON.parse(openOneWay(packet.body, secret, key).payload);
  };
  const resident = await here.start().ready();
  const list = await until(async () => {
    const mail = await receive();
    return mail?.agents?.list?.some(row => row.session === latest) && mail.agents.list;
  }, "The destination did not announce its recent conversations");
  assert.ok(!list.some(row => row.session === old || row.session === arrived));
  send({ agents: { at: Date.now(), list: [lead, unrelated].map((session, i) =>
    ({ session, name: i ? "Unrelated" : "Lead", kind: "session" })) } });
  await until(async () => {
    const location = await request(resident, "GET", "kuvo/sessions/" + lead + "/location");
    return location.json?.on?.id === member.key;
  }, "The destination did not learn the sender's address");
  const letter = async (id, from, to) => {
    send({ letter: { id, from, to, text: "How is it going?", wait: false, at: Date.now() } });
    return until(async () => {
      const receipt = (await receive())?.delivered;
      return receipt?.id === id && receipt;
    }, "The destination did not acknowledge " + id);
  };
  assert.deepEqual(await letter("ltr_deniedOld", unrelated, old), {
    id: "ltr_deniedOld", ok: false,
    why: "That conversation isn't one this computer lists, nor one you started: write to one that list_agents names.",
  });
  assert.deepEqual(await letter("ltr_allowedOld", lead, old), { id: "ltr_allowedOld", ok: true });
  assert.deepEqual(await letter("ltr_allowedRecent", unrelated, latest), { id: "ltr_allowedRecent", ok: true });
  assert.deepEqual(await letter("ltr_allowedArrival", unrelated, arrived), { id: "ltr_allowedArrival", ok: true });
  assert.deepEqual(here.engine.messages.get(old).map(row => row.text), [
    `[Message from Lead · ${lead}] (To answer, use send_message to ${lead}.)\n\nHow is it going?`,
  ]);
  assert.equal(here.engine.messages.get(latest).length, 1);
  assert.equal(here.engine.messages.get(arrived).length, 1);
});

test("an authenticated note's address survives restart and routes later notes", {
  timeout: 50000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("noted-owner");
  const { main, member, spare } = seedPeerSetup(here.home, relay.url, 1, 3);
  const local = "ses_noteReceiver", remote = "ses_unannouncedHome";
  here.engine.sessions.push(sessionOf({ id: local, title: "Reservations", directory: path.join(here.home, "chats") }));
  const { sealOneWay, openOneWay, readFrame, frame, Frame } = await testMail(t, boxBinary(wireFixture));
  const address = Buffer.from(main.address, "base64url"), localKey = Buffer.from(main.key, "base64url");
  const memberDevice = relay.device(Buffer.alloc(32, 52)), spareDevice = relay.device(Buffer.alloc(32, 53));
  t.after(() => { memberDevice.leave(); spareDevice.leave(); });
  const reply = async (device, secret, field, id) => until(async () => {
    const packet = readFrame((await device.next(12000)).subarray(17));
    if (packet?.kind !== Frame.mail) return false;
    const message = JSON.parse(openOneWay(packet.body, Buffer.alloc(32, secret), localKey).payload);
    return message[field]?.id === id && message[field];
  }, "The computer did not send " + field + " for " + id, 15000);
  const incoming = (device, secret, envelope) => {
    const text = Buffer.from(JSON.stringify({ ...envelope, at: Date.now() }));
    device.send(address, frame(Frame.mail, 0, sealOneWay(text, Buffer.alloc(32, secret), localKey).message));
  };
  const first = here.start();
  const resident = await first.ready();
  await until(() => relay.has(address), "The note receiver did not connect");
  await until(async () => (await request(resident, "GET", "api/session/" + local)).status === 200,
    "The local conversation did not become ready");
  const now = Date.now(), note = { from: "The computer", words: "Work is on this computer.", delivery: "queue", about: [remote] };
  const sendNote = async (device, secret, id, made) => {
    incoming(device, secret, { letter: { id, to: local, text: note.words, wait: false, at: Date.now(), made, note } });
    assert.deepEqual(await reply(device, secret, "delivered", id), { id, ok: true });
  };
  await sendNote(memberDevice, 2, "ltr_newAddress", now);
  await sendNote(spareDevice, 3, "ltr_olderAddress", now - 60000);
  const homes = path.join(here.home, "run/work/homes.json");
  assert.deepEqual(readJSON(homes)[remote], { key: member.key, at: now });
  const lookup = "ltr_findKnownHome";
  incoming(spareDevice, 3, { letter: { id: lookup, to: remote, text: note.words, wait: false,
    at: Date.now(), made: now, note: { ...note, about: [] } } });
  assert.deepEqual(await reply(spareDevice, 3, "delivered", lookup), {
    id: lookup, ok: false, retry: true, lives: member.key,
    why: "It lives on another computer now: it comes again there.",
  });

  await first.stop();
  await until(() => !relay.has(address), "The note receiver did not disconnect");
  const id = "ltr_followKnownHome";
  const pending = path.join(here.home, "run/outbox/sending", id + ".json");
  write(pending, JSON.stringify({ to: remote, text: "Here is the result.", peer: spare.key, wait: false, at: Date.now(),
    note: { from: "The computer", words: "Here is the result.", delivery: "queue" } }));
  const second = here.start();
  await second.ready();
  const forwarded = await reply(memberDevice, 2, "letter", id);
  assert.equal(forwarded.to, remote);
  assert.equal(forwarded.text, "Here is the result.");
  incoming(spareDevice, 3, { delivered: { id, ok: false, retry: true, lives: main.key } });
  await sendNote(spareDevice, 3, "ltr_forgedReceiptObserved", now - 50000);
  assert.equal(readJSON(homes)[remote].key, member.key);
  incoming(memberDevice, 2, { delivered: { id, ok: false, retry: true, lives: spare.key } });
  await until(() => readJSON(homes)[remote]?.key === spare.key, "The verified redirect was not recorded");
  await until(() => readJSON(pending).peer === spare.key, "The pending note did not follow the redirect");
  await second.stop();
  await until(() => !relay.has(address), "The computer receiving the redirect did not disconnect");
  await here.start().ready();
  assert.equal((await reply(spareDevice, 3, "letter", id)).to, remote);
  incoming(spareDevice, 3, { delivered: { id, ok: true } });
  await until(() => !fs.existsSync(pending), "The forwarded note's receipt was not accepted");
});

test("a letter survives a failed lookup and a restart after engine acceptance", {
  timeout: 60000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("unconfirmed-letter-source"), there = await computer("unconfirmed-letter-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const sender = "ses_unconfirmedSender", recipient = "ses_unconfirmedTarget";
  here.engine.sessions.push(sessionOf({ id: sender, title: "Reader", directory: path.join(here.home, "chats") }));
  const id = "ltr_unconfirmedAcceptance";
  const pending = path.join(here.home, "run/outbox/sending", id + ".json");
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const from = Buffer.from(main.address, "base64url"), to = Buffer.from(member.address, "base64url");
  const unpack = (packet, peer, secret) =>
    packet.payload[0] === Frame.mail && JSON.parse(openOneWay(packet.payload.subarray(5), Buffer.alloc(32, secret),
      Buffer.from(peer.key, "base64url")).payload);
  let first, retry, lastReceipt, announced = false;
  t.after(relay.intercept(from, to, packet => {
    if (unpack(packet, main, 2)?.letter?.id === id) first = packet;
    return false;
  }));
  t.after(relay.intercept(to, from, packet => {
    const mail = unpack(packet, member, 1);
    if (mail?.agents?.list?.some(agent => agent.session === recipient)) announced = true;
    const receipt = mail?.delivered;
    if (receipt?.id === id) {
      lastReceipt = receipt;
      if (receipt.retry) retry = receipt;
    }
    return false;
  }));
  const sourceProcess = here.start(), targetProcess = there.start();
  const [source, target] = await Promise.all([sourceProcess.ready(), targetProcess.ready()]);
  await Promise.all([
    until(async () => (await request(source, "GET", "api/session/" + sender)).status === 200, "The sender did not become ready"),
    until(async () => (await request(target, "GET", "api/session")).status === 200, "The destination's engine did not become ready"),
  ]);
  there.engine.refuseLookup = recipient;
  write(pending, JSON.stringify({ from: sender, to: recipient, text: "Check the live build.", wait: false, peer: member.key, at: Date.now() }));
  await until(() => retry, () => "No retry receipt after a failed lookup " + JSON.stringify(lastReceipt)
    + "\n" + sourceProcess.log() + "\n" + targetProcess.log(), 20000);
  assert.equal(retry.ok, false);
  assert.ok(fs.existsSync(pending));
  assert.ok(!there.engine.messages.has(recipient));

  there.engine.refuseLookup = null;
  const directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  const made = sessionOf({ id: recipient, title: "Reservations", directory, updated: Date.now() });
  here.engine.sessions.push(made);
  here.engine.emit({ type: "session.created", data: { sessionID: recipient, info: made } });
  await readyToMove(source, recipient, member);
  const move = await request(source, "POST", "kuvo/moves", { kind: "chat", id: recipient, to: member.key, receipt: true });
  assert.equal(move.status, 202, move.text);
  await moveRetired(here, move.json.move.id, () => sourceProcess.log() + "\n" + targetProcess.log());
  assert.ok(there.engine.sessions.some(session => session.id === recipient));
  const accepted = new Promise(resolve => { there.engine.holdPrompt = { session: recipient, accepted: resolve }; });
  relay.replay(first);
  await accepted;
  const conversation = there.engine.messages.get(recipient);
  assert.equal(conversation.length, 1);
  await targetProcess.stop();
  await until(() => !relay.has(to), "The stopped destination stayed on the relay");
  announced = false;
  const restarted = there.start();
  await restarted.ready();
  await until(() => announced, () => "The restarted destination did not announce its conversation\n" + restarted.log(), 15000);
  relay.replay(first);
  await until(() => !fs.existsSync(pending), () => "The retried letter was not acknowledged " + JSON.stringify(lastReceipt)
    + "\n" + restarted.log(), 15000);
  assert.equal(conversation.length, 1);
  const prompts = there.engine.asked.filter(call => call.method === "POST" && call.path === "/api/session/" + recipient + "/prompt");
  assert.deepEqual([...new Set(prompts.map(call => call.json.id))], [conversation[0].id]);
});

test("temporary refusals leave a peer's letter allowance intact", {
  timeout: 65000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("letter-budget");
  const { main, member, save } = seedPeerSetup(here.home, relay.url);
  const held = "ses_budgetHeld", recipient = "ses_budgetRecipient", unavailable = "ses_budgetUnavailable";
  for (const id of [held, recipient]) {
    here.engine.sessions.push(sessionOf({ id, title: "Reservations", directory: path.join(here.home, "chats") }));
  }
  const move = "mov_budgetHold";
  save("run/moves/" + move + ".json", {
    id: move, thing: held, name: "Held conversation", kind: "chat", from: member.key, to: main.key,
    role: "to", stage: "copying", peer: member.key, sessions: [held], started: Date.now(),
    complete: true, setUpAt: Date.now(), safeSwitch: true, twoComputers: true,
  });
  const { sealOneWay, openOneWay, readFrame, frame, Frame } = await testMail(t, boxBinary(wireFixture));
  const secret = Buffer.alloc(32, 2), key = Buffer.from(main.key, "base64url"), address = Buffer.from(main.address, "base64url");
  const wire = relay.device(Buffer.alloc(32, 52));
  t.after(() => wire.leave());
  const resident = await here.start().ready();
  await until(async () => (await request(resident, "GET", "api/session/" + recipient)).status === 200,
    "The letter target's engine did not become ready");
  await until(() => relay.has(address), "The target did not connect");
  const send = async (id, to) => {
    const now = Date.now();
    const bytes = Buffer.from(JSON.stringify({ at: now, letter: { id, to, at: now, text: "A message from a peer." } }));
    wire.send(address, frame(Frame.mail, 0, sealOneWay(bytes, secret, key).message));
    return until(async () => {
      const datagram = readFrame((await wire.next(12000)).subarray(17));
      if (datagram?.kind !== Frame.mail) return false;
      const receipt = JSON.parse(openOneWay(datagram.body, secret, key).payload).delivered;
      return receipt?.id === id && receipt;
    }, "The recipient did not answer " + id, 12000);
  };
  here.engine.refuseLookup = unavailable;
  for (let attempt = 0; attempt < 12; attempt++) {
    for (const [id, to] of [["ltr_budgetHeld", held], ["ltr_budgetEngine", unavailable]]) {
      const receipt = await send(id, to);
      assert.deepEqual([receipt.ok, receipt.retry], [false, true], JSON.stringify(receipt));
    }
  }
  here.engine.refuseLookup = null;
  for (let index = 0; index < 20; index++) assert.equal((await send("ltr_budgetRun" + index, recipient)).ok, true);
  const limited = await send("ltr_budgetExhausted", recipient);
  assert.deepEqual([limited.ok, limited.retry], [false, true]);
  assert.match(limited.why, /sent a lot of messages/);
  assert.equal(here.engine.messages.get(recipient).length, 20);
  assert.ok(!here.engine.messages.has(held));
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

test("a paused queue moves with its messages in order and resumes the first one", {
  timeout: 65000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("paused-queue-source"), there = await computer("paused-queue-destination");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const session = "ses_pausedQueueMove", directory = path.join(here.home, "chats");
  fs.mkdirSync(directory, { recursive: true });
  here.engine.sessions.push(sessionOf({ id: session, title: "Paused queue", directory }));
  const first = "msg_pausedQueueFirst", second = "msg_pausedQueueSecond";
  const heading = "The build is green.", next = "Then read the notes.";
  const queue = "kuvo/sessions/" + session + "/queue";
  const source = here.start(), destination = there.start();
  const [a, b] = await Promise.all([source.ready(), destination.ready()]);
  await readyToMove(a, session, member);
  const stopped = await request(a, "POST", "kuvo/sessions/" + session + "/stop");
  assert.equal(stopped.json?.paused, true, stopped.text);
  const added = await request(a, "POST", queue + "/add",
    { id: first, text: "[Letter] " + heading, from: "Ada", words: heading, kept: true });
  assert.equal(added.json?.waits, true, added.text);
  let released = false, offeredEarly = false, fenced = false;
  const delayed = [];
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const from = Buffer.from(main.address, "base64url"), to = Buffer.from(member.address, "base64url");
  t.after(relay.intercept(from, to, packet => {
    if (packet.payload[0] !== Frame.mail) return false;
    const move = JSON.parse(openOneWay(packet.payload.subarray(5), Buffer.alloc(32, 2),
      Buffer.from(main.key, "base64url")).payload).move;
    if (move?.op !== "offer") return false;
    if (!released) offeredEarly = true;
    if (fenced) return false;
    delayed.push(packet);
    return true;
  }));
  const entered = Promise.withResolvers(), resumeWrite = Promise.withResolvers();
  t.after(() => resumeWrite.resolve());
  // Both reads happen after prompt admission, in the respective queue writers.
  const kind = boxBinary().typescript ? "inbox" : "session";
  here.engine.holdQueueRead = { session, kind, entered: entered.resolve, finish: resumeWrite.promise };
  const pending = request(a, "POST", queue + "/add", {
    id: second, text: next, before: "msg_notPresent",
    files: [{ name: "notes.txt", uri: "data:text/plain;base64,RG9jcyBmaXJzdC4=" }],
  });
  await entered.promise;
  const move = await request(a, "POST", "kuvo/moves", { kind: "chat", id: session, to: member.key, receipt: true });
  assert.equal(move.status, 202, move.text);
  const journal = path.join(here.home, "run/moves", move.json.move.id + ".json");
  await until(() => {
    const row = readJSON(journal);
    return row.stage === "holding" && row.step !== "check";
  }, "The source did not hold the conversation");
  const late = request(a, "POST", queue + "/add", { id: "msg_queueTooLate", text: "Keep this on the source." });
  await new Promise(resolve => setTimeout(resolve, 300));
  released = true;
  resumeWrite.resolve();
  const [kept, rejected] = await Promise.all([pending, late]);
  assert.equal(kept.json?.waits, true, kept.text);
  assert.equal(rejected.status, 409, rejected.text);
  fenced = true;
  for (const packet of delayed) relay.replay(packet);
  await moveRetired(here, move.json.move.id, () => source.log() + "\n" + destination.log());
  assert.equal(offeredEarly, false);
  const old = await request(a, "GET", queue);
  assert.deepEqual(old.json, { paused: false, held: [] });
  for (const id of [first, second]) assert.ok(!fs.existsSync(path.join(here.home, "run/waiting", id + ".json")));
  const arrived = await request(b, "GET", queue);
  assert.equal(arrived.json?.paused, true, arrived.text);
  assert.deepEqual(arrived.json.waiting.map(item => [item.id, item.from ?? null, item.text, item.files ?? []]),
    [[first, "Ada", heading, []], [second, null, next, ["notes.txt"]]]);
  const saved = readJSON(path.join(there.home, "run/waiting", second + ".json"));
  assert.ok(saved.text.includes(path.join(there.home, "attachments")));
  assert.ok(!saved.text.includes(path.join(here.home, "attachments")));
  assert.ok(!there.engine.messages.get(session)?.length);
  there.engine.runAfterPrompt = first;
  const resumed = await request(b, "POST", queue + "/resume", {});
  assert.equal(resumed.status, 200, resumed.text);
  await until(() => there.engine.messages.get(session)?.some(message => message.id === first),
    "The first held message did not resume");
  assert.deepEqual(there.engine.messages.get(session).map(message => message.text), ["[Letter] " + heading]);
  const after = await request(b, "GET", queue);
  assert.deepEqual(after.json?.waiting?.map(item => item.id), [second]);
});

test("a missing or unreadable queued payload leaves the conversation on its source", {
  timeout: 125000, skip: !fs.existsSync(wireFixture) && "Build the reference peer mail fixture",
}, async t => {
  const { computer, relay } = await bench(t);
  const here = await computer("queue-payload-source"), there = await computer("queue-payload-target");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  seedPeerSetup(there.home, relay.url, 2);
  const directory = path.join(here.home, "chats");
  const { openOneWay, Frame } = await testMail(t, boxBinary(wireFixture));
  const a = Buffer.from(main.address, "base64url"), b = Buffer.from(member.address, "base64url");
  const key = peer => Buffer.from(peer.key, "base64url");
  const moveFrom = (packet, peer, seed) => packet.payload[0] === Frame.mail &&
    JSON.parse(openOneWay(packet.payload.subarray(5), Buffer.alloc(32, seed), key(peer)).payload).move;
  let frames, alter, selected;
  t.after(relay.intercept(a, b, packet => {
    const move = moveFrom(packet, main, 2);
    if (move) frames.push(move.op);
    return false;
  }));
  t.after(relay.intercept(b, a, packet => {
    if (alter && moveFrom(packet, member, 1)?.op === "arrived") {
      fs.writeFileSync(selected, "This is not a queued payload.");
      alter = false;
    }
    return false;
  }));
  frames = [];
  const source = here.start(), destination = there.start();
  const [resident] = await Promise.all([source.ready(), destination.ready()]);
  for (const early of [true, false]) {
    const id = early ? "ses_queuePayloadGone" : "ses_queuePayloadCorrupt";
    const session = sessionOf({ id, title: "Queued move", directory, updated: Date.now() });
    here.engine.sessions.push(session);
    here.engine.emit({ type: "session.created", data: { sessionID: id, info: session } });
    await readyToMove(resident, id, member);
    const base = "kuvo/sessions/" + id, item = early ? "msg_deletedPayload" : "msg_corruptPayload";
    const stopped = await request(resident, "POST", base + "/stop");
    assert.equal(stopped.json?.paused, true, stopped.text);
    const queued = await request(resident, "POST", base + "/queue/add", { id: item, text: "Keep this queued prompt." });
    assert.equal(queued.json?.waits, true, queued.text);
    selected = path.join(here.home, "run/waiting", item + ".json");
    const original = fs.readFileSync(selected);
    frames = [];
    if (early) fs.unlinkSync(selected);
    else alter = true;
    try {
      const begun = await request(resident, "POST", "kuvo/moves", { kind: "chat", id, to: member.key, receipt: true });
      assert.equal(begun.status, 202, begun.text);
      const outcome = await until(async () => {
        const reply = await request(resident, "GET", "kuvo/moves/receipt?id=" + begun.json.move.id);
        return reply.json?.state === "failed" && reply.json;
      }, "The conversation was allowed to move with an unreadable queued payload", 15000);
      assert.ok(outcome.message);
      assert.deepEqual([frames.includes("offer"), frames.includes("switch")], [!early, false]);
      assert.ok(here.engine.sessions.some(session => session.id === id));
      assert.deepEqual((await request(resident, "GET", base + "/queue")).json.waiting.map(item => item.id), [item]);
    } finally {
      fs.writeFileSync(selected, original);
    }
  }
  const root = "ses_childInboxRoot", child = "ses_childInboxChild";
  for (const row of [
    sessionOf({ id: root, title: "A conversation with a child", directory, updated: Date.now() }),
    sessionOf({ id: child, title: "Child with queued work", parent: root, directory, updated: Date.now() }),
  ]) {
    here.engine.sessions.push(row);
    here.engine.emit({ type: "session.created", data: { sessionID: row.id, info: row } });
  }
  await readyToMove(resident, root, member);
  const item = { id: "msg_childInbox", sessionID: child, type: "user", delivery: "queue", payload: { text: "Keep the child busy." } };
  here.engine.inboxes.set(child, [item]);
  for (const fault of ["unpaused", "unavailable"]) {
    here.engine.refuseInbox = fault === "unavailable" ? child : null;
    frames = [];
    const before = here.engine.asked.length;
    const move = await request(resident, "POST", "kuvo/moves", { kind: "chat", id: root, to: member.key, receipt: true });
    assert.equal(move.status, 202, move.text);
    const id = move.json.move.id;
    const readChild = call => call.method === "GET" && call.path === "/api/session/" + child + "/inbox";
    await until(() => here.engine.asked.slice(before).some(readChild),
      () => "The source did not try the child's inbox during " + fault + "\n"
        + JSON.stringify(here.engine.asked.slice(before).filter(call => call.path.includes("ses_"))) + "\n"
        + source.log() + "\n" + destination.log(), 35000);
    assert.equal(frames.includes("offer"), false, fault);
    here.engine.refuseInbox = child;
    await moveRetired(here, id, () => source.log() + "\n" + destination.log(), 80000);
    const canceled = await request(resident, "GET", "kuvo/moves/receipt?id=" + id);
    assert.equal(canceled.json?.state, "failed", canceled.text);
    assert.equal(frames.includes("offer"), false, fault);
    assert.ok(here.engine.sessions.some(row => row.id === root));
    assert.ok(here.engine.sessions.some(row => row.id === child));
    assert.ok(!there.engine.sessions.some(row => row.id === root || row.id === child));
    assert.deepEqual(here.engine.inboxes.get(child), [item]);
  }
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
