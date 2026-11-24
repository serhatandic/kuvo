// The main and departing computer use real residents. The private relay lets
// the test hold the removed computer offline while the main commits.
import assert from "node:assert/strict";
import { createHmac, createPublicKey, verify } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, launch, localRelay, readJSON, relayAddress, request, scratch, startBox, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { seedPeerSetup } from "./peer-fixture.mjs";

const fromMac = { "x-kuvo-from": "mac" };
const applicant = process.env.KUVO_CONFORMANCE_JOIN_BIN
  ? boxBinary(process.env.KUVO_CONFORMANCE_JOIN_BIN) : boxBinary();

test("the computers list shows signed pending wipes and omits resolved removals", { timeout: 30000 }, async t => {
  const { computer, relay } = await bench(t);
  const main = await computer("Studio");
  const operation = Buffer.alloc(16, 90).toString("base64url");
  const error = "This computer’s data layout cannot be wiped from here. Cancel the wipe to leave its data in place.";
  const initial = seedPeerSetup(main.home, relay.url, 1, 3, roster => {
    const [here, pending, lost] = roster.boxes;
    roster.boxes = [here];
    roster.left = [pending.key, lost.key];
    roster.removals = {
      [pending.key]: { peer: pending, operation, wipe: true, error, unresolved: ["ses_retained"] },
      [lost.key]: { peer: lost, operation: Buffer.alloc(16, 91).toString("base64url"), wipe: true, resolution: "lost" },
    };
  });
  const current = main.connect(await main.start().ready());
  const listed = await current("GET", "kuvo/computers");
  assert.equal(listed.status, 200, listed.text);
  assert.deepEqual(listed.json.wipePending, [{
    key: initial.member.key, name: initial.member.name, operation, error,
    nextAction: "cancel-wipe", unresolved: ["ses_retained"],
  }]);
  assert.deepEqual(listed.json.computers.map(peer => peer.key), [initial.main.key]);
});

test("an offline wipe stays pending across a restart until a lost disk is recorded", { timeout: 100000 }, async t => {
  const fromApp = { "x-kuvo-from": "app" };
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio");
  const lab = await computer("Lab");
  const workshop = await computer("Workshop");
  const running = studio.start();
  const primary = studio.connect(await running.ready());
  const setup = (await primary("GET", "kuvo/setup")).json.setup;
  await until(() => leases.leaseFor(setup), "Studio did not obtain the relay lease.");
  for (const member of [lab, workshop]) {
    const offer = await primary("POST", "kuvo/computers/add/code", {}, fromApp);
    assert.equal(offer.status, 201, offer.text);
    const joined = await member.join(offer.json.code);
    assert.equal(joined.code, 0, joined.log);
  }
  const copy = workshop.connect(await workshop.start().ready());
  const state = () => readJSON(path.join(studio.home, "run", "link.json"));
  const roster = () => state().shared.membershipAndAccess.value;
  const source = roster().boxes.find(peer => peer.name === "Lab");
  const key = source.key;
  await until(async () => (await copy("GET", "kuvo/setup")).json.computers.some(peer => peer.key === key),
    "Workshop did not see Lab as a member before removal.");
  const work = path.join(lab.home, "agents", "sessions", "ses_retained", "messages.json");
  write(work, "Lab remains offline");
  const operation = Buffer.alloc(16, 77).toString("base64url");
  const route = `kuvo/computers/${key}/unlink-and-wipe`;
  if (boxBinary().typescript) {
    await until(async () => (await copy("GET", "kuvo/computers")).json.computers
      .some(peer => peer.key === state().setup.primary && peer.features?.includes("computers.removal")),
    "Workshop did not learn the main's removal capability.", 20000);
  }
  const queued = await copy("POST", route, { operation }, fromApp);
  assert.equal(queued.status, 200, queued.text);
  assert.deepEqual(queued.json.removal, { operation, state: "wipe-pending", accessRemoved: true });
  const row = { key, name: "Lab", operation };
  const pending = async from => (await from("GET", "kuvo/computers")).json.wipePending;
  await until(async () => (await pending(copy))?.some(each => each.operation === operation), "Workshop did not see the signed pending wipe.");
  assert.deepEqual(await pending(copy), [row]);
  await running.stop();
  const next = studio.connect(await studio.start().ready());
  assert.deepEqual(await pending(next), [row]);
  assert.equal(roster().boxes.some(peer => peer.key === key), false);
  await until(async () => !(await next("GET", "kuvo/setup")).json.leadershipWarning?.includes("resume when it reconnects"), "The primary did not regain the write lease.");
  const lost = await next("POST", route, { operation, resolve: "lost" }, fromApp);
  assert.equal(lost.status, 200, lost.text);
  assert.deepEqual(lost.json.removal, { operation, state: "lost", accessRemoved: true });
  assert.deepEqual((await next("POST", route, { operation }, fromApp)).json, lost.json);
  assert.equal(await pending(next), undefined);
  await until(async () => (await pending(copy)) === undefined, "Workshop did not receive the resolution.");
  assert.deepEqual(roster().left, [key]);
  assert.equal(roster().removals[key].peer.sign, source.sign);
  assert.equal(fs.readFileSync(work, "utf8"), "Lab remains offline");
});

test("a removed Unix member wipes on reconnect and retries the sealed completion from a fresh reply identity", {
  skip: process.platform === "win32", timeout: 60000,
}, async t => {
  const { computer, relay, leases } = await bench(t);
  const studio = await computer("Studio");
  const lab = await computer("Lab", undefined, undefined, { canonical: true });
  const primary = studio.connect(await studio.start().ready());
  const accepted = (await primary("GET", "kuvo/setup")).json;
  await until(() => leases.leaseFor(accepted.setup), "Studio did not obtain the relay lease.");
  const offer = await primary("POST", "kuvo/computers/add/code", {}, fromMac);
  assert.equal(offer.status, 201, offer.text);
  const joined = await lab.join(offer.json.code);
  assert.equal(joined.code, 0, joined.log);
  fs.rmSync(path.join(lab.home, "run", "engine.json"), { force: true });
  write(path.join(lab.home, "data", "old-session"), "retire only this work");
  const outside = path.join(path.dirname(lab.home), "keep-outside.txt");
  write(outside, "keep this");
  const state = () => readJSON(path.join(studio.home, "run", "link.json"));
  const roster = () => state().shared.membershipAndAccess.value;
  const studioAddress = Buffer.from(roster().boxes.find(peer => peer.key === accepted.you.key).address, "base64url");
  const target = roster().boxes.find(peer => peer.name === "Lab");
  const original = readJSON(path.join(lab.home, "run", "link.json"));
  const oldAddress = Buffer.from(target.address, "base64url");
  const operation = Buffer.alloc(16, 78).toString("base64url");
  const route = `kuvo/computers/${target.key}/unlink-and-wipe`;
  const result = await primary("POST", route, { operation }, fromMac);
  assert.equal(result.status, 200, result.text);
  assert.equal(result.json.removal.state, "wipe-pending");
  assert.ok(fs.existsSync(lab.home));
  const receipts = path.join(path.dirname(lab.home), ".kuvo-code-removal-receipts");
  const file = path.join(receipts, `${operation}.json`);
  const proof = path.join(receipts, `${operation}.ack`);
  const packets = [];
  const stopIntercepting = relay.intercept(null, studioAddress, packet => {
    if (packet.from.equals(oldAddress) || packet.payload[0] !== 5 || fs.existsSync(lab.home)) return false;
    if (!packet.payload.subarray(5).subarray(0, 16).equals(Buffer.from("kuvo-removal-v1\0"))) return false;
    packets.push(packet);
    return true;
  });
  t.after(stopIntercepting);
  // The source removed at the primary still starts from its prior, signed roster.
  lab.start();
  await until(() => packets.length >= 2, "The erased source did not retry its completion from a fresh relay address.", 30000);
  assert.equal(fs.existsSync(lab.home), false);
  assert.equal(fs.readFileSync(outside, "utf8"), "keep this");
  const kept = readJSON(file);
  assert.equal(kept.deleted, true);
  assert.equal(JSON.stringify(kept).includes(original.key), false);
  assert.equal(JSON.stringify(kept).includes(original.sign), false);
  const signed = readJSON(proof);
  const document = Buffer.from(signed.d, "base64url");
  const body = JSON.parse(document).body;
  assert.equal(body.removal, "completed");
  assert.equal(body.source, target.key);
  const signer = createPublicKey({ format: "der", type: "spki", key: Buffer.concat([
    Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(target.sign, "base64url"),
  ]) });
  assert.equal(verify(null, Buffer.concat([Buffer.from("kuvo-link doc v1\0"), document]), signer,
    Buffer.from(signed.s, "base64url")), true);
  assert.ok(!packets[0].from.equals(oldAddress));
  assert.deepEqual(packets[0].from, relayAddress(Buffer.from(kept.secret, "base64url")));
  assert.deepEqual(packets[1].payload, packets[0].payload);
  assert.deepEqual(Buffer.concat([Buffer.from([2]), studioAddress, packets[0].payload]), Buffer.from(kept.done, "base64url"));
  assert.deepEqual(packets[0].payload.subarray(0, 5), Buffer.from([5, 0, 0, 0, 0]));
  assert.deepEqual(packets[0].payload.subarray(21, 53), Buffer.from(target.key, "base64url"));
  const reconnect = relay.disconnect(packets[0].from);
  t.after(reconnect);
  assert.notEqual(roster().removals[target.key].delivered, true);
  stopIntercepting();
  reconnect();
  await until(() => roster().removals[target.key]?.delivered, "The main did not take the authenticated completion.");
  await until(() => !fs.existsSync(file) && !fs.existsSync(proof), "The fresh identity did not receive the matching acknowledgment.", 12000);
  const finished = await primary("POST", route, { operation }, fromMac);
  assert.equal(finished.json.removal.state, "wiped", finished.text);
  assert.equal((await primary("GET", "kuvo/computers")).json.wipePending, undefined);
});

test("an unsupported wipe can be cancelled after the source holds its work for review", { timeout: 80000 }, async t => {
  const fromApp = { "x-kuvo-from": "app" };
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio");
  const lab = await computer("Lab");
  const ask = studio.connect(await studio.start().ready());
  const setup = (await ask("GET", "kuvo/setup")).json.setup;
  await until(() => leases.leaseFor(setup), "Studio did not obtain the relay lease.");
  const offer = await ask("POST", "kuvo/computers/add/code", {}, fromApp);
  assert.equal(offer.status, 201, offer.text);
  const joined = await lab.join(offer.json.code);
  assert.equal(joined.code, 0, joined.log);
  const read = box => readJSON(path.join(box.home, "run", "link.json"));
  const session = path.join(lab.home, "agents/sessions/ses_retained/messages.json");
  write(session, "saved work");
  const there = lab.connect(await lab.start().ready());
  const key = (await there("GET", "kuvo/setup")).json.you.key;
  const route = `kuvo/computers/${key}/unlink-and-wipe`;
  const operation = Buffer.alloc(16, 43).toString("base64url");
  const take = body => ask("POST", route, { operation, ...body }, fromApp);
  const begun = await take({});
  assert.equal(begun.status, 200, begun.text);
  assert.equal(begun.json.removal.state, "wipe-pending");
  const error = "This computer’s data layout cannot be wiped from here. Cancel the wipe to leave its data in place.";
  const roster = () => read(studio).shared.membershipAndAccess.value;
  await until(() => roster().removals[key]?.error === error, "The main did not record the source review.");
  assert.equal(read(lab).removalSource.operation, operation);
  assert.equal(read(lab).removalSource.review, "unsupported-layout");
  assert.equal(fs.readFileSync(session, "utf8"), "saved work");
  const held = await take({});
  assert.deepEqual(held.json.removal, { operation, state: "review", accessRemoved: true, nextAction: "cancel-wipe" });
  assert.equal(held.json.message, error);
  const cancelled = await take({ resolve: "cancel" });
  assert.equal(cancelled.status, 200, cancelled.text);
  assert.deepEqual(cancelled.json.removal, { operation, state: "cancelled", accessRemoved: true });
  await until(async () => {
    const view = (await there("GET", "kuvo/setup")).json;
    return view.setup !== setup && view.you.role === "primary" && roster().removals[key]?.delivered;
  }, "The source did not keep its standalone setup after cancellation.");
  assert.equal(fs.readFileSync(session, "utf8"), "saved work");
  assert.equal((await ask("GET", "kuvo/computers")).json.wipePending, undefined);
  assert.equal(roster().boxes.some(peer => peer.key === key), false);
});

test("a member set aside by its main computer cannot pair a desktop", { timeout: 20000 }, async t => {
  const { computer, relay } = await bench(t);
  const lab = await computer("Lab");
  seedPeerSetup(lab.home, relay.url, 2);
  const link = path.join(lab.home, "run", "link.json");
  const before = readJSON(link);
  before.setup.aside = true;
  before.macNames = { [before.setup.primary]: "Studio" };
  write(link, JSON.stringify(before));
  const ask = lab.connect(await lab.start().ready());
  const pair = await ask("POST", "kuvo/devices/pair", {
    key: Buffer.alloc(32, 91).toString("base64url"), name: "Laptop", kind: "desktop",
  }, { "x-kuvo-from": "app" });
  assert.equal(pair.status, 409, pair.text);
  assert.deepEqual(pair.json, {
    message: "Studio doesn't list this computer in its setup now. Re-add this computer from Studio to pair this app.",
  });
  assert.deepEqual(readJSON(link).phones, []);
  assert.deepEqual((await ask("GET", "kuvo/phones")).json.phones, []);
  const forwards = path.join(lab.home, "run", "work-queue", "forward");
  assert.deepEqual(fs.existsSync(forwards) ? fs.readdirSync(forwards) : [], []);
});

test("uninstalling a stopped machine clears its desktop identity and sign-ins but retains work", async t => {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const home = path.join(root, "box");
  const env = isolatedEnv(root);
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

test("uninstall reports the desktop identity failure first and can be retried", {
  skip: process.platform === "win32" || process.getuid?.() === 0,
}, async t => {
  const { computer } = await bench(t);
  const local = await computer("Studio");
  fs.unlinkSync(path.join(local.home, "run", "engine.json"));
  const desktop = path.join(local.home, "desktop");
  const identity = path.join(desktop, "identity");
  const launch = path.join(local.home, "run", "launch.env");
  const work = path.join(local.home, "data", "retained.txt");
  write(identity, "local desktop identity");
  write(work, "saved work");
  fs.mkdirSync(launch);
  fs.chmodSync(desktop, 0o500);
  try {
    const result = await local.command(["uninstall"]);
    assert.equal(result.code, 1, result.log);
    assert.deepEqual(JSON.parse(result.text), {
      result: "unfinished", stopped: true, launchAgent: false, crontab: false,
      startsAgain: [], left: [local.home],
      message: "Kuvo Code is stopped on this computer and won't start again. Its folder stays, with your sessions and settings. Kuvo Code couldn’t delete this desktop’s local identity.",
    });
    assert.equal(fs.readFileSync(identity, "utf8"), "local desktop identity");
    assert.equal(fs.existsSync(path.join(local.home, "run", "off")), true);
  } finally {
    fs.chmodSync(desktop, 0o700);
    fs.rmdirSync(launch);
  }
  const retried = await local.command(["uninstall"]);
  assert.equal(retried.code, 0, retried.log);
  assert.equal(JSON.parse(retried.text).result, "uninstalled");
  assert.equal(fs.existsSync(identity), false);
  assert.equal(fs.readFileSync(work, "utf8"), "saved work");
});

test("local uninstall refuses a running main computer before clearing its identity", { timeout: 45000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const member = await computer("Lab");
  const active = main.start();
  const ask = main.connect(await active.ready());
  const original = (await ask("GET", "kuvo/setup")).json;
  await until(() => leases.leaseFor(original.setup), "The main computer did not claim its lease.");
  const invitation = await ask("POST", "kuvo/computers/add/code", {}, fromMac);
  assert.equal(invitation.status, 201, invitation.text);
  const enrolled = await member.join(invitation.json.code);
  assert.equal(enrolled.code, 0, enrolled.log);
  const other = member.connect(await member.start().ready());
  const followed = await other("GET", "kuvo/setup");
  assert.equal(followed.json.you.role, "member", followed.text);
  assert.equal(followed.json.setup, original.setup);
  const device = Buffer.alloc(32, 47).toString("base64url");
  const paired = await ask("POST", "kuvo/devices/pair", { key: device, kind: "desktop", name: "Studio desktop" },
    { "x-kuvo-from": "app" });
  assert.equal(paired.status, 200, paired.text);
  const listed = async at => (await at("GET", "kuvo/phones")).json.phones
    .find(phone => phone.key === device);
  const onMember = await until(() => listed(other), "Lab did not see the paired desktop.");
  assert.equal(onMember.pairing, paired.json.pairing);
  const before = (await ask("GET", "kuvo/setup")).json;
  const identity = path.join(main.home, "desktop", "identity");
  write(identity, "keep this identity");

  const refused = await main.command(["uninstall"]);
  assert.equal(refused.code, 1, refused.log);
  assert.deepEqual(JSON.parse(refused.text), {
    result: "refused", message: "Choose another main computer first.",
  });
  assert.equal(fs.readFileSync(identity, "utf8"), "keep this identity");
  assert.equal(fs.existsSync(path.join(main.home, "run", "off")), false);
  assert.equal(active.child.exitCode, null);
  assert.deepEqual((await ask("GET", "kuvo/setup")).json, before);
  assert.equal((await other("GET", "kuvo/setup")).json.setup, followed.json.setup);
  assert.equal((await listed(ask)).pairing, paired.json.pairing);
  assert.deepEqual(await listed(other), onMember);
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
    await until(() => leases.leaseFor(nextSetup), "The replacement setup did not claim a relay lease.");
    const invite = await ask("POST", "kuvo/computers/add/code", {}, fromMac);
    assert.equal(invite.status, 201, invite.text);
    fs.mkdirSync(home);
    const installer = boxCommand(binary, home, env, newRelay, ["install", "--join"]);
    children.push(installer);
    installer.child.stdin.end(invite.json.code + "\n");
    assert.equal((await installer.exited).code, 0, installer.log());
    assert.equal(fs.existsSync(marker), false, "successful reinstall consumes the former setup's offer");
    const next = startBox({ binary, home, env, relay: newRelay });
    children.push(next);
    const adopted = await request(await next.ready(), "GET", "kuvo/setup");
    assert.equal(adopted.json.setup, nextSetup);
    assert.equal(adopted.json.you.role, "member");
    assert.notEqual(adopted.json.you.key, key);
    assert.equal(fs.existsSync(local), false);
  }
});

test("the last app-owned Mac stops its keeper and clears its identity before erasing its directory", {
  skip: process.platform !== "darwin", timeout: 45000,
}, t => appOwnedWipe(t, false));

test("an app-owned Mac member clears its identity after detach and acknowledges the wipe", {
  skip: process.platform !== "darwin", timeout: 55000,
}, t => appOwnedWipe(t, true));

async function appOwnedWipe(t, member) {
  const root = fs.realpathSync(scratch());
  const env = isolatedEnv(root);
  const binary = boxBinary();
  const { computer, relay, leases } = await bench(t);
  const id = "run.kuvo.code.wipe-fixture";
  const support = path.join(env.HOME, "Library", "Application Support");
  const app = path.join(support, id);
  const home = path.join(app, "box");
  const operation = Buffer.alloc(16, 71).toString("base64url");
  const receipts = path.join(support, `.${id}-removal-receipts`);
  const marker = path.join(receipts, member ? "app-member-wipe" : "last-setup-wipe");
  const work = path.join(app, "Chats", "chat");
  const outside = path.join(env.HOME, "unrelated.txt");
  const steps = path.join(root, "steps");
  write(work, "old app data");
  write(outside, "keep this");
  write(path.join(home, "folder-version.json"), '{"version":23}');
  write(path.join(home, "box", "kind"), "mac");
  const password = "fixture-helper";
  const keys = new Set(["KUVO_KEY_SAMPLE"]);
  const seed = Buffer.alloc(32, 94).toString("base64url");
  let identity = { key: seed, sign: seed, secret: seed };
  let studio;
  if (member) {
    const setMac = roster => { roster.boxes[1].kind = "mac"; };
    studio = await computer("Studio");
    seedPeerSetup(studio.home, relay.url, 1, 2, setMac);
    seedPeerSetup(home, relay.url, 2, 2, setMac);
    const file = path.join(home, "run", "link.json");
    const { key, secret, sign, ...link } = readJSON(file);
    identity = { key, secret, sign };
    write(file, JSON.stringify(link), { mode: 0o600 });
  }
  const helper = http.createServer(async (req, res) => {
    const bytes = [];
    for await (const chunk of req) bytes.push(chunk);
    const input = JSON.parse(Buffer.concat(bytes).toString() || "{}");
    const send = (status, value) => res.writeHead(status, { "content-type": "application/json", "x-kuvo-host-version": "1" }).end(JSON.stringify(value));
    if (req.url === "/hello") {
      const proof = createHmac("sha256", password).update(`kuvo-host-hello\0${input.nonce}\0${req.headers.host}`).digest("hex");
      return send(200, { proof });
    }
    if (req.headers.authorization !== `Bearer ${password}`) return send(401, {});
    if (req.url === "/link/identity") return send(200, identity);
    if (req.url === "/keys/names") return send(200, { names: [...keys] });
    if (req.url === "/keys/delete") {
      if (!keys.delete(input.name) || fs.existsSync(marker)) return send(409, {});
      fs.appendFileSync(steps, "helper\n");
      return send(200, {});
    }
    send(404, {});
  });
  await new Promise(resolve => helper.listen(0, "127.0.0.1", resolve));
  const keeperFile = path.join(home, "run", "keeper.json");
  const ownedResident = path.join(root, "fixture-resident-pid");
  const keeperScript = path.join(root, "keeper.mjs");
  write(keeperScript, `import fs from "node:fs";
import http from "node:http";
const [record, marker, work, steps, ownedResident] = process.argv.slice(3);
const keeper = http.createServer((req, res) => {
  if (req.method !== "POST" || req.url !== "/stop" || req.headers.authorization !== "Bearer fixture-keeper") {
    return res.writeHead(403).end();
  }
  if (JSON.parse(fs.readFileSync(marker)).phase !== "stopping" || !fs.existsSync(work)) {
    return res.writeHead(409).end();
  }
  fs.appendFileSync(steps, "keeper\\n");
  if (fs.existsSync(ownedResident)) {
    const pid = Number(fs.readFileSync(ownedResident, "utf8"));
    try { process.kill(pid, "SIGTERM"); } catch (error) { if (error.code !== "ESRCH") throw error; }
  }
  res.writeHead(204, { Connection: "close" }).end(() => keeper.close());
});
keeper.listen(0, "127.0.0.1", () => fs.writeFileSync(record,
  JSON.stringify({ pid: process.pid, url: "http://127.0.0.1:" + keeper.address().port, password: "fixture-keeper" }), { mode: 0o600 }));
`);
  const keeper = launch(process.execPath, [keeperScript, "--box", keeperFile, marker, work, steps, ownedResident], env, root);
  const tools = path.join(env.HOME, "tools");
  // Use a fixture tool on this test account. It cannot reach the account Keychain.
  const checkIdentityTiming = member
    ? `[ ! -e '${app}' ] && [ -f '${path.join(receipts, `erase-${operation}`, "Chats", "chat")}' ] && /usr/bin/grep -q '"phase":"detached"' '${marker}'`
    : `[ -f '${work}' ] && /usr/bin/grep -q '"phase":"stopping"' '${marker}'`;
  write(path.join(tools, "security"), `#!/bin/sh
set -eu
[ "$1" = delete-generic-password ] && [ "$3" = '${id}.phone-link' ]
${checkIdentityTiming}
printf 'keychain %s\\n' "$5" >> '${steps}'
`, { mode: 0o700 });
  write(path.join(home, "run", "host.json"), JSON.stringify({
    formatVersion: 1, password, url: `http://127.0.0.1:${helper.address().port}`,
  }), { mode: 0o600 });
  write(path.join(home, "run", "mac-link.json"), JSON.stringify({ phones: true, own: true, relay: relay.url }));
  const residentEnv = {
    ...env, KUVO_BOX: "mac", KUVO_KEYS_ON_HOST: "1", KUVO_ENGINE_CONFIG: path.join(app, "engine-config"),
    KUVO_TEST_UNINSTALL_TOOLS: tools, OPENCODE_PASSWORD: "fixture-serve",
  };
  const resident = binary.typescript
    ? boxCommand(binary, home, residentEnv, relay, ["serve"])
    : startBox({ binary, home, relay, env: residentEnv });
  if (binary.typescript) write(ownedResident, String(resident.child.pid), { mode: 0o600 });
  t.after(async () => {
    await Promise.all([resident.stop(), keeper.stop()]);
    helper.closeAllConnections();
    await new Promise(resolve => helper.close(resolve));
    fs.rmSync(root, { recursive: true, force: true });
  });
  await until(() => fs.existsSync(keeperFile), "The test app's keeper did not start.");
  const toolHttp = path.join(home, "run", "tool-http.json");
  const record = binary.typescript
    ? await until(() => fs.existsSync(toolHttp) && readJSON(toolHttp), resident.log, 10000)
    : await resident.ready();
  const before = await request(record, "GET", "kuvo/setup");
  const { setup, you } = before.json;
  assert.equal(you.role, member ? "member" : "primary", before.text);
  let ask = (...args) => request(record, ...args);
  if (member) {
    ask = studio.connect(await studio.start().ready());
    await until(() => leases.leaseFor(setup), "The main did not acquire the relay lease.");
  }
  const route = `kuvo/computers/${you.key}/${member ? "unlink-and-wipe" : "unlink"}`;
  const requested = await ask("POST", route, { operation }, fromMac);
  assert.equal(requested.status, 200, requested.text);
  assert.equal(requested.json.removal?.state, "wipe-pending", requested.text);
  assert.equal(requested.json.offer, undefined, requested.text);
  await until(() => fs.existsSync(marker) && !readJSON(marker).phase,
    () => `The app-owned wipe did not finish. ${resident.log()}`, 20000);
  assert.equal((await keeper.exited).code, 0, keeper.log());
  assert.deepEqual(readJSON(marker), { setup, operation, formatVersion: 1 });
  assert.equal(fs.existsSync(app), false);
  assert.equal(fs.readFileSync(outside, "utf8"), "keep this");
  assert.equal(fs.readFileSync(steps, "utf8"), "helper\nkeeper\nkeychain key\nkeychain secret\nkeychain sign\n");
  if (member) {
    const primaryLink = path.join(studio.home, "run", "link.json");
    await until(() => readJSON(primaryLink).shared.membershipAndAccess.value.removals[you.key]?.delivered,
      "The main did not accept the app member's completion.");
    await until(() => !fs.existsSync(path.join(receipts, `${operation}.json`)), "The app member did not retire its acknowledgment.");
  }
  const bundle = path.join(root, "Fixture.app");
  write(path.join(bundle, "Contents", "Info.plist"),
    `<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${id}</string></dict></plist>`);
  const installer = boxCommand(binary, root, env, relay, ["install", "--status", "--root", ".kuvo-code", "--app", bundle],
    { KUVO_HOME: path.join(env.HOME, ".kuvo-code") }, "pipe");
  let stdout = "";
  installer.child.stdout.setEncoding("utf8").on("data", text => { stdout += text; });
  installer.child.stdin.end();
  assert.equal((await installer.exited).code, 0, stdout + installer.log());
  const status = JSON.parse(stdout);
  assert.deepEqual([status.verdict, status.verb, status.offer, status.wipeReceipt], member
    ? ["fresh", "new", undefined, undefined]
    : ["fresh", null, "reinstall-local", { setup, operation }]);
}

test("the installer waits for an authorized wipe before offering reinstall", {
  skip: process.platform === "win32" && "Windows wipe startup still belongs to 8.1.",
  timeout: 45000,
}, async t => {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const receipts = path.join(env.HOME, ".kuvo-code-removal-receipts");
  const marker = path.join(receipts, "last-setup-wipe");
  const setup = "set_" + Buffer.alloc(16, 38).toString("base64url");
  const operation = Buffer.alloc(16, 39).toString("base64url");
  const args = ["install", "--status", "--root", ".kuvo-code"];
  if (process.platform === "darwin") {
    const bundle = path.join(root, "Fixture.app");
    write(path.join(bundle, "Contents", "Info.plist"),
      '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.code.wipe-test</string></dict></plist>');
    args.push("--app", bundle);
  }
  const status = async () => {
    const child = boxCommand(boxBinary(), root, env, { url: "ws://127.0.0.1:1" }, args, { KUVO_HOME: home }, "pipe");
    let stdout = "";
    child.child.stdout.setEncoding("utf8").on("data", text => { stdout += text; });
    child.child.stdin.end();
    assert.equal((await child.exited).code, 0, stdout + child.log());
    return JSON.parse(stdout);
  };
  const link = path.join(home, "run", "link.json");
  const pending = JSON.stringify({ setup: { id: setup, role: "primary" }, localWipe: { setup, operation } });
  write(link, pending);
  write(path.join(home, "run", "off"), "");
  write(path.join(home, "data", "old-session"), "old work");
  const before = await status();
  assert.deepEqual([before.verdict, before.verb, before.offer], ["waits", null, undefined]);
  assert.equal(fs.readFileSync(link, "utf8"), pending);
  assert.equal(fs.existsSync(marker), false);

  const { dev, ino } = fs.lstatSync(home, { bigint: true });
  write(marker, JSON.stringify({ setup, operation, source: `${dev}:${ino}`, phase: "erasing" }), { mode: 0o600 });
  fs.chmodSync(receipts, 0o700);
  const old = path.join(receipts, `erase-${operation}`);
  fs.renameSync(home, old);
  const replacement = path.join(home, "desktop", "fixture");
  write(replacement, "new work");
  const lock = home + "-wipe.lock";
  write(path.join(lock, `owner-${process.pid}-${"a".repeat(16)}`), "");
  try {
    const held = await status();
    assert.deepEqual([held.verdict, held.verb, held.offer], ["waits", null, undefined]);
    assert.equal(fs.readFileSync(path.join(old, "data", "old-session"), "utf8"), "old work");
  } finally {
    fs.rmSync(lock, { recursive: true });
  }
  const replacementStamp = fs.lstatSync(home, { bigint: true });
  write(marker, JSON.stringify({ setup, operation, source: `${replacementStamp.dev}:${replacementStamp.ino}`, phase: "erasing" }));
  const mismatched = await status();
  assert.deepEqual([mismatched.verdict, mismatched.verb, mismatched.offer], ["waits", null, undefined]);
  assert.equal(fs.readFileSync(path.join(old, "data", "old-session"), "utf8"), "old work");
  write(marker, JSON.stringify({ setup, operation, source: `${dev}:${ino}`, phase: "erasing" }));
  for (let attempt = 0; attempt < 2; attempt++) {
    const after = await status();
    assert.deepEqual([after.verdict, after.verb, after.offer, after.wipeReceipt],
      ["fresh", null, "reinstall-local", { setup, operation }]);
    assert.equal(fs.existsSync(old), false);
    assert.equal(fs.readFileSync(replacement, "utf8"), "new work");
    assert.equal(fs.existsSync(link), false);
    assert.deepEqual(readJSON(marker), { setup, operation, formatVersion: 1 });
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

test("a member forwards an unlink and receives the main’s saved decision", { timeout: 90000 }, async t => {
  const { computer, leases, relay } = await bench(t);
  const main = await computer("Studio");
  const member = await computer("Workshop");
  const other = await computer("Lab");
  const primary = main.connect(await main.start().ready());
  const setup = (await primary("GET", "kuvo/setup")).json.setup;
  await until(() => leases.leaseFor(setup), "The main computer did not claim a relay lease.");
  for (const computer of [member, other]) {
    const offer = await primary("POST", "kuvo/computers/add/code", {}, fromMac);
    assert.equal(offer.status, 201, offer.text);
    const joined = await computer.join(offer.json.code);
    assert.equal(joined.code, 0, joined.log);
  }
  const ask = member.connect(await member.start().ready());
  const work = ["bots/bot_retained/notes.md", "agents/sessions/ses_retained/messages.json"]
    .map(relative => path.join(other.home, relative));
  for (const file of work) write(file, "local work");
  const target = other.connect(await other.start().ready());
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
  await until(() => mainState().value.removals[lab.key].delivered
    && load(member.home).shared?.membershipAndAccess?.value.removals?.[lab.key]?.delivered,
    "Lab's acknowledgment did not reach both remaining computers.");
  const released = (await target("GET", "kuvo/setup")).json;
  assert.notEqual(released.setup, setup);
  assert.equal(released.you.role, "primary");
  assert.deepEqual(released.computers.map(peer => peer.key), [released.you.key]);
  assert.deepEqual(load(other.home).phones, []);
  assert.deepEqual(work.map(file => fs.readFileSync(file, "utf8")), ["local work", "local work"]);
  assert.deepEqual((await ask("GET", "kuvo/setup")).json.computers.map(peer => peer.key), [studio.key, workshop.key]);
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
  if (applicant.typescript) {
    await until(() => usageScope() === setup, "Usage settings did not follow the initial setup.");
  }
  const old = await local("GET", "kuvo/setup");
  const key = old.json.you.key;
  const priorSign = readLink(main.home).shared.membershipAndAccess.value.boxes
    .find(peer => peer.key === key).sign;
  const desktop = Buffer.alloc(32, 67).toString("base64url");
  const paired = await ask("POST", "kuvo/devices/pair", { key: desktop, kind: "desktop", name: "Retired desktop" },
    { "x-kuvo-from": "app" });
  assert.equal(paired.status, 200, paired.text);
  const revoked = { key: desktop, pairing: paired.json.pairing };
  const revoke = await ask("POST", "kuvo/phones/remove", { phones: [revoked] }, fromMac);
  assert.equal(revoke.status, 200, revoke.text);
  await until(() => readLink(other.home).shared?.membershipAndAccess?.value.devices?.removed
    ?.some(grant => grant.key === desktop && grant.pairing === revoked.pairing),
    "The departing computer did not receive the device revocation.");
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
  assert.deepEqual(standalone.formerDeviceRevocations[setup], [revoked]);
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
  assert.deepEqual(accepted.devices.removed, [revoked]);
  assert.ok(accepted.computerHistory.retiredSigns.includes(priorSign));
  assert.deepEqual(accepted.boxes.map(peer => peer.name), ["Studio", "Lab"]);
  assert.notEqual(accepted.boxes[1].key, key);
  const repeated = await other.join(next.json.code, { resident: joinWith.typescript, joinWith });
  assert.equal(repeated.code, 0, repeated.log);
  assert.equal(readLink(other.home).key, member.key);
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
