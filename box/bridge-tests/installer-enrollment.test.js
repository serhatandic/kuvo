import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash, createHmac, randomBytes } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { test as nodeTest } from "node:test";
import { atEnd, removeAtEnd } from "./at-end.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { relayStandIn } from "./relay-stand-in.js";

const script = fileURLToPath(LINK_BUNDLE);
const link = await import(script);
const temp = fs.realpathSync(fs.mkdtempSync("/tmp/kuvo-enroll-"));
removeAtEnd(temp);

const caseCleanup = [];
const afterCase = step => caseCleanup.push(step);
async function finishCase() {
  const errors = [];
  while (caseCleanup.length) {
    try { await caseCleanup.pop()(); }
    catch (error) { errors.push(error); }
  }
  if (errors.length) throw new AggregateError(errors, "Could not clean up enrollment fixtures");
}
atEnd(finishCase);
function test(name, options, run) {
  nodeTest(name, options, t => {
    t.after(finishCase);
    return run(t);
  });
}

const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const keyOf = value => link.publicOf(Buffer.from(value.key, "base64url")).toString("base64url");
const recoveries = current => ({
  ...current.shared?.membershipAndAccess?.value?.installerRecoveries,
  ...current.shared?.ownershipAssignments?.value?.["installer recovery"],
  ...current.shared?.ownershipRecovery?.value?.["installer recovery"],
});
const reservationToken = () => "guarded-" + randomBytes(24).toString("hex");
async function installerRelay() {
  const leases = relayLeaseStandIn();
  const relay = await relayStandIn(link, { request: leases.request });
  afterCase(() => relay.close());
  return Object.assign(relay, { leases });
}
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(description, read, timeout = 20_000) {
  const deadline = Date.now() + timeout;
  do {
    const value = await read();
    if (value) return value;
    await wait(50);
  } while (Date.now() < deadline);
  assert.fail(description);
}

function box(name, relay) {
  const home = path.join(temp, name);
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(home, { recursive: true });
  const ssh = path.join(home, "fake-ssh");
  fs.writeFileSync(ssh, '#!/bin/sh\nprintf "asked\\n" >> "$HOME/ssh-called"\nprintf "ssh: connect to host from-member.invalid port 22: Connection refused\\n" >&2\nexit 255\n', { mode: 0o700 });
  const env = { ...process.env, HOME: home, KUVO_HOME: folder, KUVO_TEST_RELAY: relay.url, KUVO_TEST_SSH: ssh };
  const file = name => path.join(folder, "run", name);
  const launch = (args, input) => spawnSync(process.execPath, [script, ...args], { input, encoding: "utf8", timeout: 25_000, env });
  const initialized = launch(["setup-local"]);
  assert.equal(initialized.status, 0, initialized.stderr);
  const start = (args, input, build = script) => {
    const child = spawn(process.execPath, [build, ...args], { env, stdio: [input ? "pipe" : "ignore", "pipe", "pipe"] });
    let out = "", err = "";
    child.stdout.on("data", data => { out += data; });
    child.stderr.on("data", data => { err += data; });
    const done = new Promise(resolve => child.once("close", (status, signal) => resolve({ status, signal, out, err })));
    afterCase(async () => {
      if (child.exitCode === null && child.signalCode === null) child.kill();
      await done;
    });
    if (input) child.stdin.end(input);
    return { child, done, output: () => ({ out, err }) };
  };
  const state = () => read(file("link.json"));
  const request = async (method, name, body, headers = {}) => {
    const serve = await until("local server did not start", () => fs.existsSync(file("serve.json")) && read(file("serve.json")));
    const reply = await fetch(`http://127.0.0.1:${serve.port}/${name}`, {
      method, headers: { authorization: "Basic " + Buffer.from(`opencode:${serve.password}`).toString("base64"), "x-kuvo-from": "mac", ...headers },
      signal: AbortSignal.timeout(25_000), body: body === undefined ? undefined : JSON.stringify(body),
    });
    return { status: reply.status, value: await reply.json() };
  };
  const startServer = () => {
    fs.writeFileSync(file("serve.needed"), "");
    return start(["serve", "--kept"]);
  };
  const startBridge = async build => {
    if (!fs.existsSync(file("test-setup-clock.json"))) fs.writeFileSync(file("test-setup-clock.json"), "{}");
    const bridge = start(["run"], undefined, build);
    const address = link.addressOf(Buffer.from(state().secret, "base64url"));
    try {
      await until(`${name}'s test-owned bridge connects`, () => fs.existsSync(file("link.pid"))
        && Number(fs.readFileSync(file("link.pid"), "utf8")) === bridge.child.pid && relay.has(address));
    } catch (error) {
      assert.fail(`${error.message}: ${JSON.stringify(bridge.output())}`);
    }
    return bridge;
  };
  const release = () => {
    for (const [name] of link.RELEASE_FILES) {
      const target = path.join(folder, name);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.writeFileSync(target, name === "bin/box-version" ? "0.8.0-beta.35\n" : "test release file");
    }
  };
  const evaluate = (source, input = "{}") => {
    const result = spawnSync(process.execPath, ["--input-type=module", "-e", `
      import * as box from ${JSON.stringify(pathToFileURL(script).href)};
      ${source}
    `], { input, encoding: "utf8", timeout: 25_000, env });
    assert.equal(result.status, 0, result.stderr);
    return JSON.parse(result.stdout);
  };
  return { home, folder, file, launch, start, state, request, startBridge, startServer, release, evaluate };
}

async function emptyEngine(...boxes) {
  const engine = http.createServer((_request, response) => response.writeHead(200, { "content-type": "application/json" }).end('{"data":[]}'));
  await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
  afterCase(() => { engine.closeAllConnections(); engine.close(); });
  for (const box of boxes) fs.writeFileSync(box.file("engine.json"), JSON.stringify({ port: engine.address().port, password: "fake-engine" }));
}

// These recovery flows start multiple independent Box processes. Their individual wait limits
// stay in place while the case deadline allows each recovery and refusal check to run.
const installerRecoveryTimeout = 90_000;

test("failure bench: a lost merge answer keeps one accepted roster and resumes with both old Work Queues", { timeout: installerRecoveryTimeout }, async () => {
  const relay = await installerRelay();
  const primary = box("merge-primary", relay);
  const source = box("merge-source", relay);
  await emptyEngine(primary, source);
  const first = primary.state();
  const other = source.state();
  const sourceKey = keyOf(other);
  const targetAddress = link.addressOf(Buffer.from(first.secret, "base64url"));
  const sourceAddress = link.addressOf(Buffer.from(other.secret, "base64url"));
  const device = name => ({ key: link.publicOf(link.newKey()).toString("base64url"), name, pairing: `${name}-fixture`, kind: "desktop", from: "pair" });
  const here = device("retained");
  const there = device("joining");
  const revoked = device("revoked");
  const retiredSeed = link.newSigningSeed();
  const retiredSign = link.ownSign({ sign: retiredSeed });
  const seedMembership = (box, state, phones, removed, computerHistory) => {
    const roster = state.shared.membershipAndAccess.value;
    roster.devices = { phones, removed };
    if (computerHistory) roster.computerHistory = computerHistory;
    fs.writeFileSync(box.file("link.json"), JSON.stringify({ ...state, phones,
      pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
  };
  seedMembership(primary, first, [here, revoked], []);
  seedMembership(source, other, [there], [{ key: revoked.key, pairing: revoked.pairing }],
    { setup: other.setup.id, slots: [], retiredSigns: [retiredSign] });
  primary.startServer();
  const initialServer = source.startServer();
  const primaryDaemon = await primary.startBridge();
  await source.startBridge();
  await until("both main computers can allocate Work Queue numbers", () =>
    [first, other].every(state => relay.leases.leaseFor(state.setup.id)?.high > 0));

  const make = async (box, title, operation) => {
    const result = await box.request("POST", "kuvo/work-queue/items", { title, body: "See #1 for the original." }, { "x-kuvo-operation": operation, "x-kuvo-from": "app" });
    assert.equal(result.status, 200, JSON.stringify(result.value));
    return result.value.item;
  };
  const oldA = await make(primary, "Work from the retained setup", "merge-primary-work");
  const oldB = await make(source, "Work from the joining setup #1", "merge-source-work");
  assert.equal(oldA.number, 1);
  assert.equal(oldB.number, 1);
  const legacy = '{"id":"ses_keep","title":"Local work keeps its ID"}\n';
  const session = path.join(source.folder, "agents", "sessions", "ses_keep", "session.json");
  fs.mkdirSync(path.dirname(session), { recursive: true });
  fs.writeFileSync(session, legacy);
  const bot = path.join(source.folder, "bots", "example", "bot.json");
  fs.mkdirSync(path.dirname(bot), { recursive: true });
  fs.writeFileSync(bot, '{"name":"Example","instructions":"Keep these"}\n');
  const personalities = path.join(source.folder, "bots", "personalities.json");
  fs.writeFileSync(personalities, JSON.stringify({ personalities: [{ id: "custom.source", name: "Source", description: "Original personality", updated: 1 }] }));
  const invented = path.join(source.folder, "box", "keys", "KUVO_KEY_UNRELATED");
  fs.mkdirSync(path.dirname(invented), { recursive: true });
  fs.writeFileSync(invented, "held on the joining computer");

  const created = await primary.request("POST", "kuvo/computers/add/merge-code", { source: other.setup.id, computer: sourceKey });
  assert.equal(created.status, 201, JSON.stringify(created.value));
  const { code } = created.value;
  assert.equal((await source.start(["join"], code + "\n").done).status, 1, "ordinary enrollment cannot redeem a merge");
  const committed = () => primary.state().shared?.membershipAndAccess?.value?.mergedComputers?.[sourceKey];
  let release;
  const pauseOffers = relay.pause(targetAddress, sourceAddress);
  const watch = relay.intercept(null, targetAddress, ({ from, payload }) => {
    if (!release && payload.includes(Buffer.from("kuvo installer v1"))) {
      release = relay.intercept(targetAddress, from, ({ payload }) => !!committed() && payload.includes(Buffer.from("kuvo installer v1")));
    }
    return false;
  });
  const interrupted = source.request("POST", "kuvo/setup/merge", { code }).catch(error => error);
  await until("the destination has accepted the whole merge", committed);
  assert.equal(source.state().setup.id, other.setup.id);
  assert.equal(source.state().setupMerge.target, first.setup.id, "the original primary stays fenced until it receives its acceptance");
  const plan = committed();
  const lastMergedNumber = plan.offset + plan.sourceHigh;
  const mergedNumber = plan.offset + oldB.number;
  assert.ok(mergedNumber > oldA.number);
  assert.ok(relay.leases.leaseFor(first.setup.id).high >= lastMergedNumber, "the lease covers the entire merged number range");
  initialServer.child.kill("SIGTERM");
  await initialServer.done;
  assert.ok(await interrupted instanceof Error);
  watch();
  release();
  primaryDaemon.child.kill();
  await primaryDaemon.done;
  const saved = primary.state();
  delete saved.installerInvites[committed().operation];
  fs.writeFileSync(primary.file("link.json"), JSON.stringify(saved), { mode: 0o600 });
  await primary.startBridge();
  await until("primary returns with accepted membership and no invitation", () => relay.has(targetAddress));
  const resumedServer = source.startServer();
  await until("the joining box’s new server replaces the lost one", () => fs.existsSync(source.file("serve.json"))
    && read(source.file("serve.json")).pid === resumedServer.child.pid);
  const done = await source.request("POST", "kuvo/setup/merge", { code });
  pauseOffers();
  // The dropped offer also interrupted the setup's normal peer announcements.
  relay.disconnect(targetAddress)();
  await until("the retained primary is connected again", () => relay.has(targetAddress));
  assert.equal(done.status, 200, JSON.stringify(done.value));
  assert.equal(done.value.setup, first.setup.id);
  assert.equal(done.value.primary, first.setup.primary);
  assert.equal(source.state().setup.role, "member");
  assert.equal(source.state().setupMerge, undefined);
  assert.equal(keyOf(source.state()), sourceKey);
  assert.deepEqual((await source.request("POST", "kuvo/setup/merge", { code })).value, done.value);
  assert.deepEqual(primary.state().phones.map(phone => phone.key).sort(), [here.key, there.key].sort());
  assert.deepEqual(source.state().phones.map(phone => phone.key).sort(), [here.key, there.key].sort());
  assert.ok(primary.state().removed.some(phone => phone.key === revoked.key));
  assert.ok(primary.state().shared.membershipAndAccess.value.computerHistory.retiredSigns.includes(retiredSign));
  assert.equal(primary.state().shared.setupPersonalities.value.personalities[0].description, "Original personality");
  assert.equal(fs.readFileSync(session, "utf8"), legacy);
  assert.equal(fs.readFileSync(bot, "utf8"), '{"name":"Example","instructions":"Keep these"}\n');
  assert.equal(fs.readFileSync(invented, "utf8"), "held on the joining computer");
  const ledger = read(source.file("work-queue/numbers.json"));
  assert.equal(ledger.primary, first.setup.primary);
  const lost = read(source.file(`setup-merges/${done.value.operation}.json`));
  assert.equal(lost.numbers.primary, "self");
  const files = fs.readdirSync(source.file("work-queue/queues"));
  const items = files.flatMap(file => read(source.file(`work-queue/queues/${file}`)).items);
  const item = items.find(each => each.id === oldB.id);
  assert.equal(item.title, `Work from the joining setup #${mergedNumber}`);
  assert.equal(item.number, mergedNumber);
  assert.equal(item.body, `See #${mergedNumber} for the original.`);
  assert.deepEqual(item.numberAliases, [{ setup: other.setup.id, name: other.name, number: 1 }]);
  const alias = await source.request("GET", `kuvo/work-queue/items/${encodeURIComponent(other.setup.id + ":#1")}`);
  assert.equal(alias.value.item.id, oldB.id);
  await until("the retained primary advertises its Work Queue routes", () => fs.existsSync(source.file("peer-agents.json"))
    && read(source.file("peer-agents.json"))[first.setup.primary]?.features?.includes("work-queue.forward"));
  const canonical = await until("the joined box finds the retained #1", async () => {
    const found = await source.request("GET", "kuvo/work-queue/items/1", undefined, { "x-kuvo-from": "app" });
    return found.status === 200 && found.value.item;
  });
  assert.equal(canonical.id, oldA.id);
  const changedNumber = await source.request("PATCH", "kuvo/work-queue/items/1", { title: "Retained #1 edited from the joined box" }, { "x-kuvo-from": "app" });
  assert.equal(changedNumber.status, 200, JSON.stringify(changedNumber.value));
  assert.equal(changedNumber.value.item.id, oldA.id);
  const refs = await until("the joined box receives the retained Work Queue summaries", async () => {
    const reply = await source.request("GET", "kuvo/work-queue/refs?refs=1");
    return reply.status === 200 && reply.value.refs["1"] && reply.value;
  });
  assert.equal(refs.refs["1"].id, oldA.id);
  assert.deepEqual(refs.aliases["1"].map(item => item.id), [oldB.id]);
  const unchangedAlias = await source.request("GET", `kuvo/work-queue/items/${oldB.id}`);
  assert.equal(unchangedAlias.value.item.title, item.title);
  const next = await make(primary, "Work after the merge", "merge-next-work");
  assert.ok(next.number > oldA.number && (next.number <= plan.offset || next.number > lastMergedNumber));
  assert.equal(primary.state().shared.membershipAndAccess.value.boxes.filter(peer => peer.key === sourceKey).length, 1);
  const reusingSigner = box("merge-retired-signer", relay);
  const identity = reusingSigner.state();
  const reusedCode = await primary.request("POST", "kuvo/computers/add/code", {});
  assert.equal(reusedCode.status, 201, JSON.stringify(reusedCode.value));
  const { token } = JSON.parse(Buffer.from(reusedCode.value.code.split(".")[1], "base64url"));
  const codeIdentity = createHash("sha256").update(token).digest("base64url");
  // Simulate retrying an installer identity that already chose the retired signing key.
  identity.shared.membershipAndAccess.value.boxes[0].sign = retiredSign;
  fs.writeFileSync(reusingSigner.file("link.json"), JSON.stringify({ ...identity, sign: retiredSeed,
    setup: { ...identity.setup, sign: retiredSign }, codeIdentity }));
  const refusedSigner = await reusingSigner.start(["join"], reusedCode.value.code + "\n").done;
  assert.equal(refusedSigner.status, 1, refusedSigner.err);
  assert.match(refusedSigner.err, /membership keys|signing key/);
  assert.equal(reusingSigner.state().setup.id, identity.setup.id);
});

test("failure bench: ordinary enrollment leaves a used independent setup alone", { timeout: 105_000 }, async () => {
  const relay = await installerRelay();
  const keep = box("add-refuses-used", relay);
  const other = box("has-session", relay);
  const before = other.state();
  const session = path.join(other.folder, "agents", "sessions", "ses_here", "session.json");
  fs.mkdirSync(path.dirname(session), { recursive: true });
  fs.writeFileSync(session, '{"id":"ses_here"}\n');
  fs.writeFileSync(keep.file("link.json"), JSON.stringify({ ...keep.state(), pairing: { token: "fixture", expires: Date.now() + 90_000 } }));
  keep.startServer();
  await keep.startBridge();
  const invitation = await keep.request("POST", "kuvo/computers/add/code", {});
  assert.equal(invitation.status, 201, JSON.stringify(invitation.value));
  const joined = await other.start(["join"], invitation.value.code + "\n").done;
  assert.equal(joined.status, 1);
  assert.match(joined.err, /setup merge/);
  const holder = reservationToken();
  fs.symlinkSync(holder, path.join(other.home, ".kuvo-code-adding"));
  const ssh = await other.start(["setup", holder], JSON.stringify({ mac: keep.state().setup.primary, macSign: keep.state().setup.sign, relay: relay.url })).done;
  assert.notEqual(ssh.status, 0);
  assert.match(ssh.err, /already has a Kuvo setup/);
  assert.equal(other.state().setup.id, before.setup.id);
  assert.equal(keyOf(other.state()), keyOf(before));
  assert.equal(fs.readFileSync(session, "utf8"), '{"id":"ses_here"}\n');
});

test("failure bench: a whole setup comes over while one member is unreachable", { timeout: 80_000 }, async () => {
  const relay = await installerRelay();
  const keep = box("multi-keep", relay);
  const leaving = box("multi-leaving", relay);
  const member = box("multi-member", relay);
  const keptSetup = keep.state().setup;
  const oldSetup = leaving.state().setup;
  await emptyEngine(keep, leaving, member);
  for (const box of [leaving, member]) fs.writeFileSync(box.file("test-setup-clock.json"), "{}");
  const definitions = word => Array.from({ length: 31 }, (_, index) =>
    ({ id: `lbl_${word}-${index}`, name: `${word} ${index}`, color: "graphite" }));
  const labels = (box, word) => {
    const file = box.file("work-queue/labels.json");
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify({ formatVersion: 1, labels: definitions(word) }));
  };
  labels(leaving, "joining");
  for (const box of [keep, leaving]) {
    fs.writeFileSync(box.file("link.json"), JSON.stringify({ ...box.state(), pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
    await box.startBridge();
  }
  keep.startServer();
  leaving.startServer();
  fs.writeFileSync(member.file("link.json"), JSON.stringify({ ...member.state(), pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
  const originalBridge = await member.startBridge();
  const join = await leaving.request("POST", "kuvo/computers/add/code", {});
  assert.equal(join.status, 201, JSON.stringify(join.value));
  const enrolled = await member.start(["join"], join.value.code + "\n").done;
  assert.equal(enrolled.status, 0, enrolled.err);
  const memberKey = keyOf(member.state());
  await until("the joining setup knows the member supports merge", () => fs.existsSync(leaving.file("peer-agents.json"))
    && read(leaving.file("peer-agents.json"))[memberKey]?.features?.includes("setup.merge"));
  labels(keep, "retained");
  const app = { "x-kuvo-from": "app" };
  const rejected = await keep.request("POST", "kuvo/computers/add/merge-code", { source: oldSetup.id, computer: oldSetup.primary });
  assert.equal(rejected.status, 201, JSON.stringify(rejected.value));
  const refused = await leaving.request("POST", "kuvo/setup/merge", { code: rejected.value.code });
  assert.equal(refused.status, 409, JSON.stringify(refused.value));
  assert.match(refused.value.message, /label capacity/);
  assert.equal(leaving.state().setup.id, oldSetup.id);
  assert.equal(member.state().setup.id, oldSetup.id);
  assert.equal(leaving.state().setupMerge, undefined, "the accepted refusal releases the old primary");
  assert.equal(leaving.state().shared.setupDeparture.value, null, "the main released the departure");
  assert.equal(keep.state().shared.membershipAndAccess.value.mergedComputers?.[oldSetup.primary], undefined);
  // Clear space with committed edits to the source library.
  const removedLabels = definitions("joining").slice(0, 2).map(label => label.id);
  for (const id of removedLabels) {
    const removed = await leaving.request("POST", "kuvo/work-queue/labels", { remove: id }, app);
    assert.equal(removed.status, 200, JSON.stringify(removed.value));
  }
  const sourceLabels = { labels: definitions("joining").slice(2), removed: removedLabels };
  assert.deepEqual(leaving.state().shared.queueLabels.value, sourceLabels);
  await until("the member receives the updated labels", () =>
    member.state().shared?.queueLabels?.value?.removed?.length === removedLabels.length);
  assert.deepEqual(member.state().shared.queueLabels.value, sourceLabels);
  const next = await keep.request("POST", "kuvo/computers/add/merge-code", { source: oldSetup.id, computer: oldSetup.primary });
  assert.equal(next.status, 201, JSON.stringify(next.value));
  const paused = relay.pause(link.addressOf(Buffer.from(leaving.state().secret, "base64url")),
    link.addressOf(Buffer.from(member.state().secret, "base64url")));
  const merging = await leaving.request("POST", "kuvo/setup/merge", { code: next.value.code });
  assert.equal(member.state().setup.id, oldSetup.id, "the unreachable member has yet to learn of the merge");
  paused();
  assert.equal(merging.status, 200, JSON.stringify(merging.value));
  assert.deepEqual(keep.state().shared.queueLabels.value, {
    labels: [...definitions("retained"), ...sourceLabels.labels],
    removed: removedLabels,
  });
  assert.deepEqual(keep.state().shared.membershipAndAccess.value.boxes.map(peer => peer.key).sort(),
    [keptSetup.primary, oldSetup.primary, memberKey].sort());
  assert.equal(keep.state().setup.role, "primary");
  assert.equal(leaving.state().setup.role, "member");
  const came = await until("the member adopts the new setup without asking a client", () => {
    const state = member.state();
    return state.setup.id === keptSetup.id && state.lastSetupMerge && state;
  }, 25_000);
  assert.equal(came.setup.role, "member");
  assert.deepEqual(came.macs, [keptSetup.primary]);
  assert.equal(came.shared.setupDeparture, undefined);
  assert.equal(came.lastSetupMerge.operation, merging.value.operation);
  // A duplicate would replace the running child after it boots, after the membership write.
  await wait(2_000);
  assert.equal(Number(fs.readFileSync(member.file("link.pid"), "utf8")), originalBridge.child.pid);
  assert.equal(originalBridge.child.exitCode, null);
});

test("failure bench: a prepared merge can resume with a new code before destination acceptance", { timeout: 45_000 }, async () => {
  const relay = await installerRelay();
  const keep = box("retry-keep", relay);
  const leaving = box("retry-leaving", relay);
  await emptyEngine(keep, leaving);
  const old = leaving.state().setup;
  for (const box of [keep, leaving]) {
    fs.writeFileSync(box.file("link.json"), JSON.stringify({ ...box.state(), pairing: { token: "fixture", expires: Date.now() + 90_000 } }));
    await box.startBridge();
  }
  keep.startServer();
  const child = leaving.startServer();
  const address = link.addressOf(Buffer.from(keep.state().secret, "base64url"));
  const suggested = { source: old.id, computer: old.primary };
  const initial = await keep.request("POST", "kuvo/computers/add/merge-code", suggested);
  assert.equal(initial.status, 201, JSON.stringify(initial.value));
  const pause = relay.intercept(null, address, ({ payload }) => !!leaving.state().setupMerge?.snapshot && payload.includes(Buffer.from("kuvo installer v1")));
  const cut = leaving.request("POST", "kuvo/setup/merge", { code: initial.value.code }).catch(error => error);
  await until("source fenced and saved its snapshot", () => leaving.state().setupMerge?.snapshot);
  const pending = leaving.state().setupMerge;
  assert.equal(keep.state().shared?.membershipAndAccess?.value?.mergedComputers?.[old.primary], undefined);
  child.child.kill("SIGTERM");
  await child.done;
  assert.ok(await cut instanceof Error);
  pause();
  const next = leaving.startServer();
  await until("the joining server restarted", () => fs.existsSync(leaving.file("serve.json")) && read(leaving.file("serve.json")).pid === next.child.pid);
  const status = await leaving.request("GET", "kuvo/setup/merge");
  assert.equal(status.value.pending.operation, pending.operation);
  const retry = await keep.request("POST", "kuvo/computers/add/merge-code", { ...suggested, resume: pending.operation });
  assert.equal(retry.status, 201, JSON.stringify(retry.value));
  const accepted = await leaving.request("POST", "kuvo/setup/merge", { code: retry.value.code });
  assert.equal(accepted.status, 200, JSON.stringify(accepted.value));
  assert.equal(accepted.value.operation, pending.operation);
  assert.equal(keep.state().shared.membershipAndAccess.value.mergedComputers[old.primary].operation, pending.operation);
});

test("failure bench: a sleeping computer follows both accepted mergers when it returns", { timeout: 70_000 }, async () => {
  const relay = await installerRelay();
  const from = box("chain-original", relay);
  const midway = box("chain-middle", relay);
  const last = box("chain-retained", relay);
  const sleeping = box("chain-sleeping", relay);
  await emptyEngine(from, midway, last, sleeping);
  for (const box of [from, midway, last, sleeping]) {
    fs.writeFileSync(box.file("link.json"), JSON.stringify({ ...box.state(), pairing: { token: "fixture", expires: Date.now() + 120_000 } }));
    await box.startBridge();
  }
  for (const box of [from, midway, last]) box.startServer();
  const invite = await from.request("POST", "kuvo/computers/add/code", {});
  assert.equal(invite.status, 201, JSON.stringify(invite.value));
  const joined = await sleeping.start(["join"], invite.value.code + "\n").done;
  assert.equal(joined.status, 0, joined.err);
  const key = keyOf(sleeping.state());
  const address = link.addressOf(Buffer.from(sleeping.state().secret, "base64url"));
  await until("the first primary knows its member supports merging", () => fs.existsSync(from.file("peer-agents.json"))
    && read(from.file("peer-agents.json"))[key]?.features?.includes("setup.merge"));
  const wake = relay.disconnect(address);
  const transfer = async (joining, retained) => {
    const setup = joining.state().setup;
    const issued = await retained.request("POST", "kuvo/computers/add/merge-code", { source: setup.id, computer: setup.primary });
    assert.equal(issued.status, 201, JSON.stringify(issued.value));
    const merged = await joining.request("POST", "kuvo/setup/merge", { code: issued.value.code });
    assert.equal(merged.status, 200, JSON.stringify(merged.value));
    return merged.value;
  };
  const first = await transfer(from, midway);
  const second = await transfer(midway, last);
  assert.notEqual(sleeping.state().setup.id, second.setup);
  wake();
  const settled = await until("the sleeping computer follows both signed offers", () => {
    const state = sleeping.state();
    return state.setup.id === second.setup && state.lastSetupMerge?.operation === second.operation && state;
  }, 28_000);
  assert.deepEqual(settled.macs, [second.primary]);
  assert.ok(fs.existsSync(sleeping.file(`setup-merges/${first.operation}.json`)));
  assert.ok(fs.existsSync(sleeping.file(`setup-merges/${second.operation}.json`)));
});

test("failure bench: a merge reserves Work Queue numbers through the relay lease", { timeout: 60_000 }, async () => {
  const relay = await installerRelay();
  const primary = box("range-retained", relay);
  const member = box("range-member", relay);
  const source = box("range-source", relay);
  await emptyEngine(primary, member, source);
  const primaryBefore = primary.state().setup;
  const ledger = source.file("work-queue/numbers.json");
  fs.mkdirSync(path.dirname(ledger), { recursive: true });
  fs.writeFileSync(ledger, JSON.stringify({ formatVersion: 1, primary: "self", next: 3001 }));
  for (const box of [primary, member]) fs.writeFileSync(box.file("test-setup-clock.json"), "{}");
  for (const box of [primary, member, source]) {
    fs.writeFileSync(box.file("link.json"), JSON.stringify({ ...box.state(), pairing: { token: "fixture", expires: Date.now() + 120_000 } }));
    await box.startBridge();
  }
  for (const box of [primary, source]) box.startServer();
  const invite = await primary.request("POST", "kuvo/computers/add/code", {});
  assert.equal(invite.status, 201, JSON.stringify(invite.value));
  const enrolled = await member.start(["join"], invite.value.code + "\n").done;
  assert.equal(enrolled.status, 0, enrolled.err);
  const assigned = await until("the retained main holds a relay lease", () => relay.leases.leaseFor(primaryBefore.id));
  assert.equal(assigned.holder, primaryBefore.primary);
  assert.ok(assigned.high < 3000);
  const memberKey = keyOf(member.state());
  await until("the retained main knows its member supports merging", () => fs.existsSync(primary.file("peer-agents.json"))
    && read(primary.file("peer-agents.json"))[memberKey]?.features?.includes("setup.merge"));
  const outgoing = source.state().setup;
  const code = await primary.request("POST", "kuvo/computers/add/merge-code", { source: outgoing.id, computer: outgoing.primary });
  assert.equal(code.status, 201, JSON.stringify(code.value));
  const merged = await source.request("POST", "kuvo/setup/merge", { code: code.value.code });
  assert.equal(merged.status, 200, JSON.stringify(merged.value));
  assert.equal(primary.state().setup.primary, primaryBefore.primary);
  const plan = primary.state().shared.membershipAndAccess.value.mergedComputers[outgoing.primary];
  assert.equal(plan.operation, merged.value.operation);
  assert.ok(plan.sourceHigh >= 3000);
  assert.ok(relay.leases.leaseFor(primaryBefore.id).high >= plan.offset + plan.sourceHigh);
  await until("the member receives the main’s accepted number allocation", () =>
    member.state().shared?.membershipAndAccess?.value?.mergedComputers?.[outgoing.primary]?.operation === merged.value.operation);
  const created = await primary.request("POST", "kuvo/work-queue/items", { title: "Work after merging" },
    { "x-kuvo-operation": "range-after-merge", "x-kuvo-from": "app" });
  assert.equal(created.status, 200, JSON.stringify(created.value));
  assert.ok(created.value.item.number > plan.offset + plan.sourceHigh);
});

test("failure bench: installer retries a saved join after a lost answer and rejects code reuse by a different computer", { timeout: installerRecoveryTimeout }, async () => {
  const relay = await installerRelay();
  const sourceSecret = "fake-source-account-secret";
  const notSelected = "fake-source-unchecked-secret";
  const accounts = [{ id: "one", integrationID: "demo", label: "Work", active: true, value: { type: "api", key: sourceSecret } }];
  const installed = [];
  let failInstall = true;
  const engine = http.createServer(async (request, response) => {
    const source = request.headers.authorization === "Basic " + Buffer.from("opencode:source").toString("base64");
    const send = (data, status = 200) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify({ data }));
    if (request.url === "/api/integration") return send(source ? [{ id: "demo" }] : []);
    if (request.url === "/api/credential" && request.method === "GET") return send(source ? accounts : installed);
    if (request.url === "/api/credential" && request.method === "POST" && !source) {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      if (failInstall) return send({}, 503);
      installed.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      return send({});
    }
    return send([]);
  });
  await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
  afterCase(() => { engine.closeAllConnections(); engine.close(); });
  const attachEngine = (target, password) => fs.writeFileSync(target.file("engine.json"), JSON.stringify({ port: engine.address().port, password }));
  const primary = box("primary", relay);
  const first = primary.state();
  // A pending pairing keeps the normal keeper bridge online before the first computer joins.
  fs.writeFileSync(primary.file("link.json"), JSON.stringify({ ...first, pairing: { token: "test", expires: Date.now() + 120_000 } }));
  attachEngine(primary, "source");
  const keys = path.join(primary.folder, "box/keys");
  fs.mkdirSync(keys, { recursive: true });
  fs.writeFileSync(path.join(keys, "KUVO_KEY_DEMO_SELECTED"), "fake-source-selected-key");
  fs.writeFileSync(path.join(keys, "KUVO_KEY_DEMO_UNCHECKED"), notSelected);
  primary.release();
  primary.startServer();
  await primary.startBridge();
  const address = link.addressOf(Buffer.from(first.secret, "base64url"));
  await until("primary is on the relay", () => relay.has(address));

  const checklist = await primary.request("GET", "kuvo/computers/add/credentials");
  assert.equal(checklist.status, 200);
  assert.ok(checklist.value.items.every(item => item.checked === true));
  assert.deepEqual(checklist.value.items.map(item => item.label).sort(), ["KUVO_KEY_DEMO_SELECTED", "KUVO_KEY_DEMO_UNCHECKED", "Work"]);
  assert.doesNotMatch(JSON.stringify(checklist.value), /fake-source-/);
  const selected = checklist.value.items.filter(item => item.label !== "KUVO_KEY_DEMO_UNCHECKED").map(item => item.id);
  const created = await primary.request("POST", "kuvo/computers/add/code", { credentials: selected });
  assert.equal(created.status, 201, JSON.stringify(created.value));
  const { code } = created.value;
  assert.ok(created.value.expires > Date.now());
  const token = JSON.parse(Buffer.from(code.slice("kuvo-add-1.".length), "base64url")).token;
  const inviteID = createHash("sha256").update(token).digest("base64url");
  assert.ok(!JSON.stringify(primary.state()).includes(token), "the primary stores the hash of the invitation");
  assert.equal(primary.state().installerInvites[inviteID].credentials, undefined, "older issuers cannot re-gather by checklist ID");
  assert.ok(primary.state().installerInvites[inviteID].credentialSeed);
  const joining = box("joining", relay);
  joining.release();
  attachEngine(joining, "destination");
  const committed = () => primary.state().shared?.membershipAndAccess?.value?.boxes?.some(peer => peer.key === keyOf(joining.state()));
  let release;
  const watch = relay.intercept(null, address, ({ from, payload }) => {
    if (!release && payload.includes(Buffer.from("kuvo installer v1"))) {
      release = relay.intercept(address, from, ({ payload }) => committed() && payload.includes(Buffer.from("kuvo installer v1")));
    }
    return false;
  });
  const initial = joining.start(["join"], code + "\n");
  await until("the primary saved accepted membership", committed);
  const identity = joining.state();
  const key = keyOf(identity);
  assert.equal(identity.setup.role, "primary", "the installer has not heard the result");
  initial.child.kill();
  const cut = await initial.done;
  assert.equal(cut.signal, "SIGTERM");
  watch();
  release();

  // Keep the applicant's bridge as a test-owned child even after the installer returns.
  fs.writeFileSync(joining.file("link.json"), JSON.stringify({ ...joining.state(), pairing: { token: "test", expires: Date.now() + 120_000 } }));
  await joining.startBridge();
  await until("the applicant's bridge is running", () => relay.has(link.addressOf(Buffer.from(identity.secret, "base64url"))));
  const incomplete = await joining.start(["join"], code + "\n").done;
  assert.equal(incomplete.status, 1, incomplete.err);
  assert.match(incomplete.err, /complete the selected sign-ins and keys/);
  assert.equal(joining.state().setup.role, "member");
  assert.equal(joining.state().installerClaim.credentialsPending, true);
  assert.equal(installed.length, 0);
  accounts[0].value.key = "fake-source-rotated-account-secret";
  fs.writeFileSync(path.join(keys, "KUVO_KEY_DEMO_SELECTED"), "fake-source-rotated-key");
  failInstall = false;
  const acknowledged = () => primary.state().installerInvites[inviteID]?.credentialsCompleted;
  const releaseReceipts = new Map();
  const loseAck = relay.intercept(null, address, ({ from, payload }) => {
    if (!payload.includes(Buffer.from("kuvo installer v1"))) return false;
    // Once the issuer retires the consent, the next response to this installer is its receipt.
    const sender = from.toString("base64url");
    if (!releaseReceipts.has(sender)) {
      releaseReceipts.set(sender, relay.intercept(address, from, ({ payload: answer }) => acknowledged() && answer.includes(Buffer.from("kuvo installer v1"))));
    }
    return false;
  });
  const cutting = joining.start(["join"], code + "\n");
  await until("the issuer saved completion before the receipt was delivered", acknowledged);
  cutting.child.kill("SIGTERM");
  assert.equal((await cutting.done).signal, "SIGTERM");
  assert.equal(joining.state().installerClaim.credentialsAckPending, true);
  loseAck();
  for (const release of releaseReceipts.values()) release();
  const retry = await joining.start(["join"], code + "\n").done;
  assert.equal(retry.status, 0, retry.err);
  assert.deepEqual(JSON.parse(retry.out), { setup: first.setup.id, primary: first.setup.primary });
  assert.equal(joining.state().setup.role, "member");
  assert.equal(joining.state().setup.id, first.setup.id);
  assert.equal(keyOf(joining.state()), key);
  assert.equal(joining.state().installerClaim.credentialsPending, undefined);
  assert.equal(JSON.stringify(joining.state()).includes(sourceSecret), false);
  const gotKeys = path.join(joining.folder, "box/keys");
  assert.deepEqual(fs.readdirSync(gotKeys), ["KUVO_KEY_DEMO_SELECTED"]);
  assert.equal(fs.readFileSync(path.join(gotKeys, "KUVO_KEY_DEMO_SELECTED"), "utf8"), "fake-source-selected-key");
  assert.equal(installed.length, 1);
  assert.equal(installed[0].value.key, sourceSecret);
  const done = primary.state().installerInvites[inviteID];
  assert.equal(done.credentialsCompleted, true);
  assert.equal(done.credentialSeed, undefined);
  assert.equal(done.credentialSelectionDigest, undefined);
  assert.equal(done.credentials, undefined);
  assert.equal(joining.state().installerClaim.credentialsAckPending, undefined);
  assert.ok(link.readDoc(joining.state().docs[`${first.setup.sign} peers`]));
  assert.equal(primary.state().shared.membershipAndAccess.value.boxes.filter(peer => peer.key === key).length, 1);
  assert.equal(recoveries(primary.state())?.[key], undefined, "activation is portable");
  assert.equal(joining.state().installerClaim.recovery, undefined);
  const oldClaim = identity.installerClaim;
  const restoredReceipt = "kuvo-cancel-1." + Buffer.from(JSON.stringify({ setup: oldClaim.setup, invite: oldClaim.invite, ...oldClaim.recovery })).toString("base64url");
  const refusedRecovery = await primary.request("POST", "kuvo/setup/installer-recovery", { receipt: restoredReceipt });
  assert.equal(refusedRecovery.status, 409);
  assert.match(refusedRecovery.value.message, /no matching installer recovery entry/);

  const second = box("second", relay);
  const prefix = "kuvo-add-1.";
  const expired = JSON.parse(Buffer.from(code.slice(prefix.length), "base64url"));
  expired.expires = Date.now() - 1;
  const stale = await second.start(["join"], prefix + Buffer.from(JSON.stringify(expired)).toString("base64url") + "\n").done;
  assert.equal(stale.status, 1, stale.err);
  assert.match(stale.err, /installer code has expired/);
  assert.equal(second.state().installerClaim, undefined);
  const used = await second.start(["join"], code + "\n").done;
  assert.equal(used.status, 1, used.err);
  assert.match(used.err, /installer code is no longer available/);
  assert.equal(second.state().setup.role, "primary");
  assert.equal(second.state().installerClaim, undefined);
  assert.equal(committed(), true);

  const withData = second.state();
  withData.sharedSetup = withData.setup.id;
  withData.shared = { setupTimeZone: { sequence: 1, value: { zone: "Etc/UTC" } } };
  fs.writeFileSync(second.file("link.json"), JSON.stringify(withData));
  const refused = await second.start(["join"], code + "\n").done;
  assert.equal(refused.status, 1, refused.err);
  assert.match(refused.err, /This computer has setup data/);
  assert.deepEqual(second.state(), withData);

  // The same request reaches the primary through an SSH one-shot on the member.
  joining.startServer();
  await until("the member hears the primary's add feature", async () => {
    const result = await joining.request("GET", "kuvo/computers/add");
    return result.value.can;
  });
  const peerFile = joining.file("peer-agents.json");
  const primaryFeatures = fs.readFileSync(peerFile, "utf8");
  const older = JSON.parse(primaryFeatures);
  older[first.setup.primary].features = older[first.setup.primary].features.filter(feature => feature !== "computers.add.credentials");
  const resumeUpdates = relay.pause(address, link.addressOf(Buffer.from(identity.secret, "base64url")));
  try {
    fs.writeFileSync(peerFile, JSON.stringify(older));
    for (const credentials of [undefined, []]) {
      const refused = await joining.request("POST", "kuvo/computers/add", { destination: "from-member.invalid", credentials });
      assert.equal(refused.status, 409, JSON.stringify(refused.value));
      assert.match(refused.value.message, /Update your main computer/);
    }
    assert.equal(fs.existsSync(path.join(primary.home, "ssh-called")), false);
  } finally {
    fs.writeFileSync(peerFile, primaryFeatures);
    resumeUpdates();
  }
  const forwardedCode = await joining.request("POST", "kuvo/computers/add/code");
  assert.equal(forwardedCode.status, 201, JSON.stringify(forwardedCode.value));
  const privateCode = forwardedCode.value.code;
  const privateToken = JSON.parse(Buffer.from(privateCode.slice(prefix.length), "base64url")).token;
  const receipts = fs.readFileSync(primary.file("work-queue/acted.json"), "utf8");
  assert.ok(!receipts.includes(privateCode) && !receipts.includes(privateToken) && !receipts.includes(sourceSecret), "portable receipts do not contain a bearer code");
  const command = await joining.start(["box", "route"], JSON.stringify({ method: "POST", path: "kuvo/computers/add", body: { destination: "--non-host" } })).done;
  assert.equal(command.status, 0, command.err);
  const forwarded = JSON.parse(command.out);
  assert.equal(forwarded.status, 400, forwarded.body);
  assert.match(JSON.parse(forwarded.body).message, /Give a host name/);
  const added = await joining.request("POST", "kuvo/computers/add", { destination: "from-member.invalid" });
  assert.equal(added.status, 202, JSON.stringify(added.value));
  const attempt = await until("only the primary runs the forwarded SSH attempt", async () => {
    const primaryList = await primary.request("GET", "kuvo/computers/add");
    return primaryList.value.tries.find(attempt => attempt.id === added.value.try.id && attempt.state === "failed");
  });
  assert.equal(attempt.destination, "from-member.invalid");
  const memberList = await joining.request("GET", "kuvo/computers/add");
  assert.deepEqual(memberList.value.tries.find(attempt => attempt.id === added.value.try.id), attempt);
  assert.ok(fs.existsSync(path.join(primary.home, "ssh-called")), JSON.stringify(attempt.failure));
  assert.equal(fs.existsSync(path.join(joining.home, "ssh-called")), false);

  // A current computer follows fleet updates even when it has a saved SSH address.
  await until("the main sees the member’s fleet version", () => {
    const file = primary.file("peer-agents.json");
    return fs.existsSync(file) && read(file)[key]?.computer?.kuvo;
  });
  fs.writeFileSync(path.join(primary.folder, "machines.json"), JSON.stringify([{ destination: "from-member.invalid" }]));
  fs.writeFileSync(path.join(primary.folder, "phone-link.json"), JSON.stringify({ machines: { "from-member.invalid": { key } } }));
  const setupAgain = () => primary.start(["box", "route"], JSON.stringify({ method: "POST", path: `kuvo/computers/${key}/setup` })).done;
  const fleet = await setupAgain();
  assert.equal(fleet.status, 0, fleet.err);
  assert.equal(JSON.parse(fleet.out).status, 409);
  assert.match(JSON.parse(JSON.parse(fleet.out).body).message, /updates with its setup/);
  // A computer that predates fleet updates still uses the saved SSH destination.
  const memberRecord = primary.file("peer-agents.json");
  const peers = read(memberRecord);
  delete peers[key].computer.kuvo;
  const resumeMemberUpdates = relay.pause(link.addressOf(Buffer.from(identity.secret, "base64url")), address);
  fs.writeFileSync(memberRecord, JSON.stringify(peers));
  const rerun = await setupAgain();
  assert.equal(rerun.status, 0, rerun.err);
  const reply = JSON.parse(rerun.out);
  assert.equal(reply.status, 202, reply.body);
  const operation = JSON.parse(reply.body).try;
  assert.equal(operation.kind, "setup");
  const seen = await primary.request("GET", "kuvo/computers/add");
  assert.ok(seen.value.tries.some(attempt => attempt.id === operation.id));
  resumeMemberUpdates();
});

function deviceFixture() {
  const privateKey = link.newKey();
  return { privateKey, key: link.publicOf(privateKey).toString("base64url"), name: "Test device" };
}

function legacyMembership(primary, peer, revision, left = []) {
  const key = keyOf(primary);
  const entry = state => ({
    key: keyOf(state), sign: link.ownSign(state), name: state.name, kind: "machine",
    address: link.addressOf(Buffer.from(state.secret, "base64url")).toString("base64url"),
  });
  return link.signDoc(primary.sign, "peers", revision, {
    setup: { id: primary.setup.id, epoch: 1, primary: key },
    boxes: [entry(primary), ...(peer ? [entry(peer)] : [])], left, moved: [],
  });
}

const applyDocs = (target, primary, give) => target.evaluate(`
  import fs from "node:fs";
  const { sender, give } = JSON.parse(fs.readFileSync(0, "utf8"));
  const link = box.updateLink(link => {
    box.docsWork({ link, library: { personalities: [], removed: [] } }, sender, { give });
    return link;
  });
  box.writeCard();
  console.log(JSON.stringify(link));
`, JSON.stringify({ sender: { key: keyOf(primary), sign: link.ownSign(primary) }, give }));

const sshInput = (primary, docs, options) => JSON.stringify({
  relay: primary.relay, name: "Returning computer", mac: keyOf(primary), macSign: link.ownSign(primary), docs, ...options,
});

function reapFixtureBridge(target) {
  afterCase(async () => {
    let pid;
    try { pid = Number(fs.readFileSync(target.file("link.pid"), "utf8")); }
    catch (error) { if (error.code === "ENOENT") return; throw error; }
    const ps = spawnSync("ps", ["-ww", "-o", "command=", "-p", String(pid)], { encoding: "utf8" });
    if (ps.status !== 0) return;
    assert.ok(ps.stdout.includes(script + " run"), ps.stdout);
    process.kill(pid, "SIGTERM");
    await until("fixture bridge stopped", () => !fs.existsSync(target.file("link.pid")), 4_000);
  });
}

for (const [suffix, description] of Object.entries({
  reserved: "unlink under the same reservation",
  restart: "a restart between reservations",
  unreserved: "unlink with no reservation",
})) test(`failure bench: legacy SSH re-enrolls after ${description}`, { timeout: 105_000 }, async () => {
  const relay = await installerRelay();
  const former = box("legacy-primary-" + suffix, relay).state();
  const target = box("legacy-member-" + suffix, relay);
  const independent = box("legacy-independent-" + suffix, relay).state();
  await emptyEngine(target);
  const before = target.state();
  const retiredKey = keyOf(before);
  const revoked = deviceFixture();
  const live = deviceFixture();
  const oldPairing = { key: revoked.key, pairing: "old" };
  const phoneDoc = (n, phones, removed, setup) => link.signDoc(former.sign, "phones", n, {
    ...(setup ? { setup } : {}), phones, removed,
  });
  const legacy = { ...before, setup: undefined, mac: keyOf(former), macSigns: { [keyOf(former)]: link.ownSign(former) } };
  fs.writeFileSync(target.file("link.json"), JSON.stringify(legacy));
  const joined = applyDocs(target, former, [
    legacyMembership(former, before, 1), phoneDoc(1, [oldPairing], []),
  ]);
  assert.equal(joined.setup.role, "member");
  assert.equal(joined.phones[0].pairing, "old");
  const lock = path.join(target.home, ".kuvo-code-adding");
  const firstReservation = reservationToken();
  if (suffix !== "unreserved") fs.symlinkSync(firstReservation, lock);
  const left = applyDocs(target, former, [
    legacyMembership(former, null, 2, [retiredKey]), phoneDoc(2, [], [oldPairing]),
  ]);
  assert.equal(left.setup, undefined);
  assert.deepEqual(left.formerDeviceRevocations[former.setup.id], [oldPairing]);
  assert.deepEqual(left.phones, []);
  if (suffix === "restart") fs.unlinkSync(lock);
  const boot = target.launch(["setup-local"]);
  assert.equal(boot.status, 0, boot.stderr);
  assert.equal(target.state().setup?.role, suffix === "reserved" ? undefined : "primary");
  if (suffix === "unreserved") {
    const forwards = target.file("work-queue/forward");
    fs.mkdirSync(forwards, { recursive: true });
    const id = "wqa_" + "x".repeat(20);
    fs.writeFileSync(path.join(forwards, id + ".json"), JSON.stringify({ act: { id, method: "POST", action: "restore", item: "fixture", at: Date.now() - 180_000 } }));
    fs.writeFileSync(path.join(forwards, "wqa_" + "y".repeat(20) + ".answer.json"), "{}");
  }
  const card = target.evaluate("console.log(JSON.stringify(box.cardNow()));");
  assert.equal(card.used, false);
  const judgement = link.judgeComputer([{ folder: target.folder, app: false, card }], link.oursNow(independent));
  assert.equal(judgement.joins || judgement.verdict === "free", true);

  const token = suffix === "restart" ? reservationToken() : firstReservation;
  if (suffix === "restart") fs.symlinkSync(token, lock);
  const grant = { key: live.key, pairing: "fresh-live" };
  // The gift comes from a legacy client that only sends signed documents. A stale
  // grant in that gift cannot undo the final revocation retained before restart.
  reapFixtureBridge(target);
  const added = target.launch(suffix === "unreserved" ? ["setup"] : ["setup", token], sshInput(former, [
    legacyMembership(former, null, 3, [retiredKey]), phoneDoc(3, [oldPairing, grant], []),
  ]));
  assert.equal(added.status, 0, added.stderr);
  const membership = JSON.parse(added.stdout);
  assert.notEqual(membership.key, retiredKey);
  assert.notEqual(membership.sign, link.ownSign(before));
  if (suffix !== "unreserved") fs.unlinkSync(lock);
  const current = target.state();
  assert.equal(current.setup.id, former.setup.id);
  assert.deepEqual(current.phones.map(phone => phone.key), [live.key]);
  const accepted = applyDocs(target, former, [
    legacyMembership(former, current, 4, [retiredKey]),
    phoneDoc(4, [oldPairing, grant], []),
    phoneDoc(999, [oldPairing], [], independent.setup.id),
  ]);
  assert.deepEqual(accepted.formerDeviceRevocations[former.setup.id], [oldPairing]);
  assert.deepEqual(accepted.phones.map(phone => phone.pairing), ["fresh-live"]);
  const authorize = device => target.evaluate(`
    console.log(JSON.stringify(box.authorizePhone(Buffer.from(${JSON.stringify(device.key)}, "base64url"))));
  `);
  assert.equal(authorize(revoked), false);
  assert.equal(authorize(live), true);
  assert.equal(link.readDoc(accepted.docs[`${link.ownSign(former)} peers`]).body.boxes.filter(peer => peer.key === membership.key).length, 1);
  const newGrant = { key: revoked.key, pairing: "fresh-after-revocation" };
  const repaired = applyDocs(target, former, [phoneDoc(5, [newGrant, grant], [oldPairing], former.setup.id)]);
  assert.ok(repaired.phones.some(phone => phone.pairing === newGrant.pairing));
  assert.ok(!repaired.phones.some(phone => phone.pairing === "old"));
  assert.equal(authorize(revoked), true);

  if (suffix === "unreserved") {
    // This signer has since established a different setup with the same keys.
    // An untagged document for the old setup must neither grant access nor occupy
    // the version slot needed by an explicitly scoped document.
    const other = { ...former, setup: { ...former.setup, id: link.newSetupID() } };
    const moved = applyDocs(target, other, [
      legacyMembership(other, current, 6), phoneDoc(9999, [grant], []),
    ]);
    assert.equal(moved.setup.id, other.setup.id);
    assert.equal(authorize(live), false);
    applyDocs(target, other, [phoneDoc(6, [newGrant], [], other.setup.id)]);
    assert.equal(authorize(revoked), true);
  }
});

test("a computer enrolled by installer code receives a verified backup over Relay", { timeout: 60_000 }, async () => {
  const relay = await installerRelay();
  const source = box("backup-source", relay);
  const receiver = box("backup-receiver", relay);
  await emptyEngine(source, receiver);
  const sourceState = source.state();
  for (const computer of [source, receiver]) {
    const state = computer.state();
    fs.writeFileSync(computer.file("link.json"), JSON.stringify({ ...state,
      pairing: { token: "fixture", expires: Date.now() + 120_000 } }));
    await computer.startBridge();
  }
  source.startServer();
  const code = await source.request("POST", "kuvo/computers/add/code", { credentials: [] });
  assert.equal(code.status, 201, JSON.stringify(code.value));
  const joined = await receiver.start(["join"], code.value.code + "\n").done;
  assert.equal(joined.status, 0, joined.err);
  const member = receiver.state();
  assert.equal(member.setup.id, sourceState.setup.id);
  assert.equal(member.setup.role, "member");

  const destination = { kind: "computer", computer: keyOf(member) };
  const backups = "kuvo/box/backups";
  const available = await until("the enrolled backup receiver advertised its Relay route", async () => {
    const reply = await source.request("GET", backups);
    assert.equal(reply.status, 200, JSON.stringify(reply.value));
    return reply.value.computers.some(computer => computer.key === destination.computer && computer.relay) && reply.value;
  });
  assert.equal(available.defaultDestination, "computer");
  const configured = await source.request("POST", backups + "/destination", { destination, confirm: true });
  assert.equal(configured.status, 200, JSON.stringify(configured.value));
  const { recoveryKey, source: namespace } = configured.value;
  assert.match(recoveryKey, /^[A-Za-z0-9_-]{43}$/);
  const backedUp = await source.request("POST", backups + "/now");
  assert.equal(backedUp.status, 200, JSON.stringify(backedUp.value));
  const { point } = backedUp.value;
  assert.ok(point?.id);
  const status = await source.request("GET", backups);
  assert.equal(status.status, 200, JSON.stringify(status.value));
  assert.deepEqual(status.value.lastVerifiedRestorePoint, point);
  assert.equal(status.value.recoveryKey, undefined);
  const archive = path.join(receiver.home, ".kuvo-backups", "relay", member.setup.id, keyOf(sourceState), namespace, point.id + ".kuvo");
  assert.equal(fs.readFileSync(archive).includes(recoveryKey), false);
  for (const computer of [source, receiver]) assert.equal(fs.existsSync(path.join(computer.home, "ssh-called")), false);
});

test("failure bench: plain installer code re-enrolls an explicitly released computer with fresh keys", { timeout: 140_000 }, async () => {
  const relay = await installerRelay();
  const main = box("released-code-primary", relay);
  const target = box("released-code-target", relay);
  await emptyEngine(main, target);
  const first = main.state();
  fs.writeFileSync(main.file("link.json"), JSON.stringify({ ...first, pairing: { token: "fixture", expires: Date.now() + 120_000 } }));
  main.startServer();
  await main.startBridge();
  reapFixtureBridge(target);
  const issue = async () => {
    const made = await main.request("POST", "kuvo/computers/add/code", { credentials: [] });
    assert.equal(made.status, 201, JSON.stringify(made.value));
    return made.value.code + "\n";
  };
  const join = async code => {
    const result = await target.start(["join"], code).done;
    assert.equal(result.status, 0, result.err);
    assert.equal(target.state().setup.id, first.setup.id);
    return target.state();
  };
  const unlink = async state => {
    const key = keyOf(state);
    const result = await main.request("DELETE", `kuvo/computers/${key}`);
    assert.equal(result.status, 200, JSON.stringify(result.value));
    await until("unlink completed on both computers", () => target.state().setup.role === "primary" && !target.state().localUnlink
      && main.state().shared.membershipAndAccess.value.removals[key]?.delivered);
    assert.ok(target.state().releasedWork);
  };
  const card = () => target.evaluate("console.log(JSON.stringify(box.cardNow()));");

  const bootstrap = target.state();
  assert.equal(card().used, false, "a fresh signed self-only setup is available to join");
  const old = await join(await issue());
  assert.notEqual(keyOf(old), keyOf(bootstrap), "plain enrollment rekeys a fresh standalone computer");
  const session = path.join(target.folder, "agents", "sessions", "ses_retained", "session.json");
  fs.mkdirSync(path.dirname(session), { recursive: true });
  fs.writeFileSync(session, JSON.stringify({ id: "ses_retained", title: "Kept on unlink" }));
  const engineFile = path.join(target.folder, "data", "opencode", "opencode.db");
  fs.mkdirSync(path.dirname(engineFile), { recursive: true });
  let engine = new DatabaseSync(engineFile);
  afterCase(() => engine?.close());
  engine.exec("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT); INSERT INTO session VALUES ('kept', 'Before unlink');");
  engine.close();
  engine = undefined;

  const device = deviceFixture();
  const paired = await main.request("POST", "kuvo/devices/pair", { key: device.key, name: device.name, kind: "desktop" });
  assert.equal(paired.status, 200, JSON.stringify(paired.value));
  const pairing = { key: device.key, pairing: paired.value.pairing };
  const stale = link.signDoc(first.sign, "phones", 9999, { setup: first.setup.id, epoch: 1, phones: [pairing], removed: [] });
  const retired = await main.request("POST", "kuvo/phones/remove", { phones: [pairing] });
  assert.equal(retired.status, 200, JSON.stringify(retired.value));
  await until("the old pairing revocation reached the member", () => target.state().removed?.some(each => each.key === device.key));
  await unlink(old);
  assert.equal(card().used, false, "the intentionally retained session is unchanged");
  assert.ok(target.state().releasedDatabaseWork, "closed WAL-mode databases can be fingerprinted");
  engine = new DatabaseSync(engineFile);
  engine.exec("PRAGMA wal_autocheckpoint=0");
  engine.exec("BEGIN; UPDATE session SET title = 'Temporary'; UPDATE session SET title = 'Before unlink'; COMMIT;");
  assert.ok(fs.statSync(engineFile + "-wal").size > 0);
  engine.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  assert.equal(fs.statSync(engineFile + "-wal").size, 0);
  assert.equal(card().used, false, "checkpointing committed rows leaves the retained work unchanged");
  engine.exec("ANALYZE; PRAGMA wal_checkpoint(TRUNCATE)");
  assert.equal(card().used, false, "SQLite planner statistics are derived from unchanged work");
  assert.deepEqual(target.state().formerDeviceRevocations[first.setup.id], [pairing]);
  const secondCode = await issue();
  const current = await join(secondCode);
  assert.notEqual(keyOf(current), keyOf(old));
  assert.notEqual(link.ownSign(current), link.ownSign(old));
  assert.notEqual(current.secret, old.secret);
  const saved = main.state().shared.membershipAndAccess.value;
  assert.equal(saved.boxes.filter(box => box.key !== keyOf(first)).length, 1);
  assert.ok(saved.boxes.some(box => box.key === keyOf(current)));
  assert.ok(saved.left.includes(keyOf(old)));
  assert.ok(saved.computerHistory.retiredSigns.includes(link.ownSign(old)));
  assert.deepEqual(saved.devices.removed, [pairing]);
  const oldMail = link.mailFor(saved.boxes.find(box => box.key === keyOf(first)), { fixture: true }, old);
  assert.equal(link.openMail(link.readFrame(oldMail.subarray(17)).body, main.state()), null);
  const replayed = applyDocs(target, first, [stale]);
  assert.ok(!replayed.phones.some(each => each.key === device.key));
  const repeated = await join(secondCode);
  assert.equal(keyOf(repeated), keyOf(current), "retry keeps the current enrollment");
  assert.equal(target.launch(["installer-recovery", "issue"]).status, 1, "adoption consumed the cancellation secret");

  await unlink(current);
  engine.exec("UPDATE session SET title = 'After unlink' WHERE id = 'kept'; PRAGMA wal_checkpoint(TRUNCATE)");
  assert.equal(card().used, true, "editing a retained database row is independent work even after checkpointing");
  engine.exec("UPDATE session SET title = 'Before unlink' WHERE id = 'kept'");
  assert.equal(card().used, false, "restoring the committed rows restores the baseline");
  fs.writeFileSync(session, JSON.stringify({ id: "ses_retained", title: "Edited as a standalone" }));
  assert.equal(card().used, true, "an edit to an existing session is independent work");
  const refused = await target.start(["join"], await issue()).done;
  assert.equal(refused.status, 1);
  assert.match(refused.err, /setup merge/);
  const ssh = target.launch(["setup"], sshInput(first, [legacyMembership(first, null, 20, [keyOf(current)])]));
  assert.equal(ssh.status, 4, ssh.stderr);
  assert.equal(target.state().setup.role, "primary");
  assert.match(fs.readFileSync(session, "utf8"), /Edited as a standalone/);
});

test("failure bench: probe, SSH and installer code protect independent work and grants", { timeout: 175_000 }, async t => {
  const relay = await installerRelay();
  const primary = box("protected-code-primary", relay);
  await emptyEngine(primary);
  const initial = primary.state();
  primary.startServer();
  reapFixtureBridge(primary);
  const { status, value } = await primary.request("POST", "kuvo/computers/add/code", { credentials: [] });
  assert.equal(status, 201, JSON.stringify(value));
  const put = (target, file, content) => {
    const absolute = path.join(target.folder, file);
    fs.mkdirSync(path.dirname(absolute), { recursive: true });
    fs.writeFileSync(absolute, JSON.stringify(content));
    return absolute;
  };
  const scenarios = {
    "active pairing": target => {
      const device = deviceFixture();
      fs.writeFileSync(target.file("link.json"), JSON.stringify({ ...target.state(),
        phones: [{ key: device.key, name: device.name, pairing: "independent", from: "pair" }] }));
    },
    "engine session": target => {
      const folder = path.join(target.folder, "data", "opencode");
      fs.mkdirSync(folder, { recursive: true });
      const db = new DatabaseSync(path.join(folder, "opencode.db"));
      db.exec("CREATE TABLE session (id TEXT PRIMARY KEY); INSERT INTO session VALUES ('ses_independent')");
      db.close();
    },
    "local bot": target => put(target, "bots/bot_independent/bot.json", { id: "bot_independent", name: "Independent bot" }),
    "allocated number block": target => put(target, "run/work-queue/numbers.json", { leases: [[42, 46]] }),
    "old pending delivery": target => put(target, "run/owned-data/outbox/" + "A".repeat(43) + ".json",
      { setup: link.newSetupID(), receipt: { status: "pending" } }),
    "undelivered legacy removal": target => {
      const state = target.state();
      fs.writeFileSync(target.file("link.json"), JSON.stringify({ ...state, untold: [{
        key: deviceFixture().key, address: randomBytes(16).toString("base64url"), kind: "machine", name: "Offline computer",
      }] }));
    },
    "saved setup time zone": target => {
      const state = target.state();
      state.shared.setupTimeZone = { sequence: 1, value: { zone: "Etc/UTC" } };
      fs.writeFileSync(target.file("link.json"), JSON.stringify(state));
    },
    "unreadable store": target => {
      fs.mkdirSync(path.join(target.folder, "bots", "personalities.json"), { recursive: true });
    },
  };
  for (const [name, seed] of Object.entries(scenarios)) await t.test(name, async () => {
    const target = box("guard-" + name.replaceAll(" ", "-"), relay);
    seed(target);
    const card = target.evaluate("console.log(JSON.stringify(box.cardNow()));");
    assert.equal(card.used, true);
    assert.equal(link.judgeComputer([{ folder: target.folder, app: false, card }], link.oursNow(initial)).joins, false);
    const before = target.state();
    const token = reservationToken();
    fs.symlinkSync(token, path.join(target.home, ".kuvo-code-adding"));
    const ssh = target.launch(["setup", token], sshInput(initial, [legacyMembership(initial, null, 1)], { discardLocalPairings: true }));
    assert.equal(ssh.status, 4, ssh.stderr);
    fs.unlinkSync(path.join(target.home, ".kuvo-code-adding"));
    const installer = await target.start(["join"], value.code + "\n").done;
    assert.equal(installer.status, 1);
    assert.match(installer.err, /setup merge/);
    assert.deepEqual(target.state(), before);
  });
  assert.deepEqual(link.peersOf(primary.state()), []);
});

test("failure bench: a post-claim edit cancels only the provisional installer roster entry", { timeout: 110_000 }, async () => {
  const relay = await installerRelay();
  const primary = box("cancel-enrollment-primary", relay);
  const target = box("cancel-enrollment-applicant", relay);
  await emptyEngine(primary, target);
  const main = primary.state();
  const mainAddress = link.addressOf(Buffer.from(main.secret, "base64url"));
  const priorDenial = { key: deviceFixture().key, pairing: "formerly-revoked" };
  main.shared.membershipAndAccess.value.devices.removed = [priorDenial];
  fs.writeFileSync(primary.file("link.json"), JSON.stringify({ ...main, pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
  primary.startServer();
  await primary.startBridge();
  const issued = await primary.request("POST", "kuvo/computers/add/code", { credentials: [] });
  assert.equal(issued.status, 201, JSON.stringify(issued.value));
  const code = issued.value.code + "\n";
  const roster = () => primary.state().shared?.membershipAndAccess?.value;
  const isInstaller = payload => payload.includes(Buffer.from("kuvo installer v1"));
  const releases = new Map();
  const cutoff = relay.intercept(null, mainAddress, ({ from, payload }) => {
    const source = from.toString("base64url");
    if (isInstaller(payload) && !releases.has(source)) {
      releases.set(source, relay.intercept(mainAddress, from, ({ payload }) => isInstaller(payload) && !!roster()?.boxes?.some(peer => peer.key === keyOf(target.state()))));
    }
    return false;
  });
  const joining = target.start(["join"], code);
  await until("the primary accepted the provisional member", () => roster()?.boxes?.some(peer => peer.key === keyOf(target.state())));
  joining.child.kill();
  assert.equal((await joining.done).signal, "SIGTERM");
  for (const release of releases.values()) release();
  cutoff();
  const provisional = target.state();
  assert.equal(provisional.setup.role, "primary");
  assert.ok(provisional.installerClaim);
  const session = path.join(target.folder, "agents", "sessions", "ses_postclaim", "session.json");
  fs.mkdirSync(path.dirname(session), { recursive: true });
  const words = '{"id":"ses_postclaim","title":"Kept after rejected enrollment"}';
  fs.writeFileSync(session, words);

  const KEY_PREFIX = Buffer.from("kuvo installer v1\0");
  const dropAck = new Map();
  const intercept = relay.intercept(null, mainAddress, ({ from, payload }) => {
    const source = from.toString("base64url");
    if (isInstaller(payload) && !dropAck.has(source)) {
      dropAck.set(source, relay.intercept(mainAddress, from, ({ payload }) => {
        const frame = link.readFrame(payload);
        if (!frame?.body.subarray(0, KEY_PREFIX.length).equals(KEY_PREFIX)) return false;
        const sender = frame.body.subarray(KEY_PREFIX.length, KEY_PREFIX.length + 32);
        const opened = link.openOneWay(frame.body.subarray(KEY_PREFIX.length + 32), Buffer.from(provisional.key, "base64url"), sender, KEY_PREFIX);
        return JSON.parse(opened.payload).cancelled === true;
      }));
    }
    return false;
  });
  const cancelling = target.start(["join"], code);
  const retired = keyOf(provisional);
  await until("the primary retired the rejected enrollment", () => roster()?.left.includes(retired));
  assert.ok(target.state().installerClaim?.cancelPending);
  cancelling.child.kill();
  assert.equal((await cancelling.done).signal, "SIGTERM");
  for (const release of dropAck.values()) release();
  intercept();
  const retry = await target.start(["join"], code).done;
  assert.equal(retry.status, 1);
  assert.match(retry.err, /enrollment was cancelled/);
  assert.equal(target.state().installerClaim, undefined);
  assert.equal(target.state().setup.id, provisional.setup.id);
  assert.equal(fs.readFileSync(session, "utf8"), words);
  assert.deepEqual(roster().boxes.map(peer => peer.key), [keyOf(main)]);
  assert.equal(roster().left.filter(key => key === retired).length, 1);
  assert.ok(roster().computerHistory.retiredSigns.includes(link.ownSign(provisional)));
  assert.deepEqual(roster().devices.removed, [priorDenial]);
});

test("failure bench: an expired installer claim can retire its exact entry after a member takes over the issuer’s relay lease", { timeout: 210_000 }, async () => {
  const relay = await installerRelay();
  const issuer = box("lost-issuer", relay);
  const successor = box("recovery-successor", relay);
  const observer = box("recovery-observer", relay);
  const applicant = box("recovery-applicant", relay);
  const clean = box("recovery-applicant-clean-copy", relay);
  await emptyEngine(issuer, successor, observer, applicant, clean);
  reapFixtureBridge(clean);
  const first = issuer.state();
  const denial = { key: deviceFixture().key, pairing: "old-denial" };
  first.shared.membershipAndAccess.value.devices.removed = [denial];
  fs.writeFileSync(issuer.file("link.json"), JSON.stringify({ ...first,
    pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
  const clock = (box, offset = 0) => fs.writeFileSync(box.file("test-setup-clock.json"), JSON.stringify({ offset }));
  for (const participant of [issuer, successor, observer]) {
    clock(participant);
    fs.writeFileSync(path.join(participant.folder, "setup-takeover.json"), JSON.stringify({ alwaysOn: participant === successor }));
  }
  const issuerBridge = await issuer.startBridge();
  issuer.startServer();
  const code = async (primary = issuer) => {
    const issued = await primary.request("POST", "kuvo/computers/add/code", { credentials: [] });
    assert.equal(issued.status, 201, JSON.stringify(issued.value));
    return issued.value.code + "\n";
  };
  const join = async participant => {
    const state = participant.state();
    fs.writeFileSync(participant.file("link.json"), JSON.stringify({ ...state, pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
    await participant.startBridge();
    const result = await participant.start(["join"], await code()).done;
    assert.equal(result.status, 0, result.err);
    participant.startServer();
    return keyOf(participant.state());
  };
  const successorKey = await join(successor);
  const observerKey = await join(observer);
  await until("the issuer’s relay lease names both enrolled computers", () => {
    const grant = relay.leases.leaseFor(first.setup.id);
    return grant?.holder === keyOf(first) && grant.roster?.[successorKey] && grant.roster?.[observerKey];
  });
  const applicantCode = await code();
  const issuerAddress = link.addressOf(Buffer.from(first.secret, "base64url"));
  const prefix = Buffer.from("kuvo installer v1\0");
  const isInstaller = payload => payload.includes(prefix);
  const openInstaller = (payload, state) => {
    const { body } = link.readFrame(payload);
    assert.ok(body.subarray(0, prefix.length).equals(prefix));
    const offset = prefix.length + 32;
    return JSON.parse(link.openOneWay(body.subarray(offset), Buffer.from(state.key, "base64url"),
      body.subarray(prefix.length, offset), prefix).payload);
  };
  const roster = box => box.state().shared?.membershipAndAccess?.value;
  const blockedReplies = new Map();
  const claimPackets = [];
  const intercept = relay.intercept(null, issuerAddress, ({ from, payload }) => {
    if (isInstaller(payload)) claimPackets.push({ from, to: issuerAddress, payload });
    if (isInstaller(payload) && !blockedReplies.has(from.toString("base64url"))) {
      blockedReplies.set(from.toString("base64url"), relay.intercept(issuerAddress, from, ({ payload }) =>
        isInstaller(payload) && Boolean(recoveries(issuer.state())?.[keyOf(applicant.state())])));
    }
    return false;
  });
  const joining = applicant.start(["join"], applicantCode);
  const entry = await until("the issuer saved the commitment and the successor received it", () => {
    const key = keyOf(applicant.state());
    const expected = recoveries(issuer.state())?.[key];
    return expected && recoveries(successor.state())?.[key]?.commitment === expected.commitment && expected;
  });
  assert.equal(joining.child.kill(), true);
  assert.equal((await joining.done).signal, "SIGTERM");
  intercept();
  for (const release of blockedReplies.values()) release();
  const left = applicant.state();
  const applicantKey = keyOf(left);
  assert.equal(left.setup.role, "primary");
  assert.equal(createHash("sha256").update(left.installerClaim.recovery.secret).digest("base64url"), entry.commitment);
  assert.equal(applicant.launch(["installer-recovery", "issue"]).status, 1, "a live claim cannot issue a receipt before refusing the join");
  const localSession = path.join(applicant.folder, "agents", "sessions", "local.json");
  fs.mkdirSync(path.dirname(localSession), { recursive: true });
  fs.writeFileSync(localSession, '{"title":"Saved after the claim"}');

  assert.equal(issuerBridge.child.kill(), true);
  await issuerBridge.done;
  const beforeExpiry = applicant.launch(["installer-recovery", "issue"]);
  assert.equal(beforeExpiry.status, 0, beforeExpiry.stderr);
  // Expire the applicant's actual saved deadline. The lease test clock drives native Box
  // takeover code independently of this installer deadline.
  left.installerClaim.until = Date.now() - 1;
  fs.writeFileSync(applicant.file("link.json"), JSON.stringify(left));
  const expired = await applicant.start(["join"], applicantCode).done;
  assert.equal(expired.status, 1);
  assert.match(expired.err, /claim expired.*installer-recovery issue/);
  const receipt = applicant.launch(["installer-recovery", "issue"]);
  assert.equal(receipt.status, 0, receipt.stderr);
  assert.match(receipt.stdout, /^kuvo-cancel-1\./);
  assert.equal(receipt.stdout, beforeExpiry.stdout, "expiry leaves the exact earlier cancellation receipt usable");
  const allowLeaseRead = relay.leases.disconnect(keyOf(first));
  const resumeObserver = relay.leases.disconnect(observerKey);
  relay.leases.advance(125_000);
  for (const participant of [issuer, successor, observer]) clock(participant, 125_000);
  await until("the successor took over with the issuer offline", () => successor.state().setup.role === "primary", 30_000);
  resumeObserver();
  await until("the other member follows the new main", () => observer.state().setup.primary === successorKey);
  assert.deepEqual(recoveries(successor.state())?.[applicantKey], entry, "the new main carries the exact commitment");
  assert.equal(successor.state().installerInvites, undefined, "the successor has no original installer code journal");
  assert.ok(roster(successor).boxes.some(peer => peer.key === applicantKey), "takeover and expiration alone never retire an entry");
  assert.equal(relay.leases.leaseFor(first.setup.id).holder, successorKey);

  const forged = JSON.parse(Buffer.from(receipt.stdout.trim().split(".")[1], "base64url").toString("utf8"));
  const tampered = "kuvo-cancel-1." + Buffer.from(JSON.stringify({ ...forged, key: observerKey })).toString("base64url") + "\n";
  const denied = await successor.start(["installer-recovery", "apply"], tampered).done;
  assert.equal(denied.status, 1);
  assert.match(denied.err, /no matching installer recovery entry/);
  const priorSlots = roster(successor).computerHistory.slots;
  const applied = await successor.start(["installer-recovery", "apply"], receipt.stdout).done;
  assert.equal(applied.status, 0, applied.err);
  assert.match(applied.out, /entry was retired/);
  await until("the new observer accepted the exact retirement", () => roster(observer).left.includes(applicantKey));
  const after = roster(successor);
  const knownSlots = new Set(priorSlots.map(slot => slot.key));
  assert.deepEqual(after.computerHistory.slots.filter(slot => knownSlots.has(slot.key)), priorSlots);
  assert.ok(after.computerHistory.slots.some(slot => slot.key === applicantKey), "retirement keeps the original applicant slot");
  assert.deepEqual(after.devices.removed, [denial]);
  assert.equal(after.left.filter(key => key === applicantKey).length, 1);
  assert.ok(after.computerHistory.retiredSigns.includes(entry.sign));
  assert.equal(recoveries(successor.state())?.[applicantKey], undefined);
  assert.ok(after.boxes.some(peer => peer.key === observerKey));
  const repeated = await successor.start(["installer-recovery", "apply"], receipt.stdout).done;
  assert.equal(repeated.status, 0, repeated.err);
  assert.match(repeated.out, /entry was already retired/);
  // The independent edit prevents this disk from joining. A clean copy of its
  // claim may re-enroll, but it must receive an entirely new link identity.
  const refused = await applicant.start(["join"], await code(successor)).done;
  assert.equal(refused.status, 1);
  assert.match(refused.err, /setup merge/);
  fs.writeFileSync(clean.file("link.json"), JSON.stringify(left));
  const readded = await clean.start(["join"], await code(successor)).done;
  assert.equal(readded.status, 0, readded.err);
  const rejoined = clean.state();
  assert.equal(rejoined.setup.id, first.setup.id);
  assert.notEqual(keyOf(rejoined), applicantKey);
  assert.notEqual(link.ownSign(rejoined), entry.sign);
  assert.notEqual(rejoined.secret, left.secret);
  assert.deepEqual(roster(successor).devices.removed, [denial]);
  assert.equal(clean.evaluate(`console.log(JSON.stringify(box.authorizePhone(Buffer.from(${JSON.stringify(denial.key)}, "base64url"))));`), false);
  assert.equal(clean.launch(["installer-recovery", "issue"]).stdout, receipt.stdout, "the new enrollment keeps the exact abandoned receipt");
  assert.ok(claimPackets.length >= 2);
  allowLeaseRead();
  await issuer.startBridge();
  await until("the former issuer sees the retirement as a follower", () => issuer.state().setup.primary === successorKey && roster(issuer)?.left.includes(applicantKey));
  const earlierClaim = claimPackets.find(packet => openInstaller(packet.payload, first).confirmed === true);
  assert.ok(earlierClaim, "captured the earlier confirmation");
  let replayReply;
  const watchReplay = relay.intercept(issuerAddress, earlierClaim.from, ({ payload }) => {
    if (isInstaller(payload)) replayReply = openInstaller(payload, left);
    return true;
  });
  relay.replay(earlierClaim);
  const answer = await until("the former issuer handled the replay", () => replayReply);
  watchReplay();
  assert.equal(answer.retrySameSetup, true);
  assert.match(answer.message, /main computer changed/);
  const replay = await successor.start(["installer-recovery", "apply"], receipt.stdout).done;
  assert.equal(replay.status, 0, replay.err);
  assert.match(replay.out, /entry was already retired/);
  for (const participant of [successor, issuer]) {
    const membership = roster(participant);
    assert.equal(membership.boxes.some(peer => peer.key === applicantKey), false);
    assert.ok(membership.boxes.some(peer => peer.key === keyOf(rejoined)));
    assert.deepEqual(membership.boxes.find(peer => peer.key === observerKey), after.boxes.find(peer => peer.key === observerKey));
    assert.deepEqual(membership.devices.removed, [denial]);
  }
  assert.equal(applicant.state().setup.id, left.setup.id);
  assert.equal(fs.readFileSync(localSession, "utf8"), '{"title":"Saved after the claim"}');
});

test("failure bench: cancelling an unconfirmed installer claim fences its delayed confirmation", { timeout: 80_000 }, async () => {
  const relay = await installerRelay();
  const primary = box("cancel-unconfirmed-primary", relay);
  const applicant = box("cancel-unconfirmed-applicant", relay);
  await emptyEngine(primary, applicant);
  const main = primary.state();
  fs.writeFileSync(primary.file("link.json"), JSON.stringify({ ...main, pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
  primary.startServer();
  await primary.startBridge();
  const issued = await primary.request("POST", "kuvo/computers/add/code", { credentials: [] });
  assert.equal(issued.status, 201, JSON.stringify(issued.value));
  const code = issued.value.code + "\n";
  const prefix = Buffer.from("kuvo installer v1\0");
  let confirmation;
  const intercept = relay.intercept(null, link.addressOf(Buffer.from(main.secret, "base64url")), packet => {
    const frame = link.readFrame(packet.payload);
    if (!frame?.body.subarray(0, prefix.length).equals(prefix)) return false;
    const sender = frame.body.subarray(prefix.length, prefix.length + 32);
    const opened = link.openOneWay(frame.body.subarray(prefix.length + 32), Buffer.from(main.key, "base64url"), sender, prefix);
    if (!JSON.parse(opened.payload).confirmed) return false;
    confirmation = packet;
    return true;
  });
  const joining = applicant.start(["join"], code);
  await until("confirmation was cut after the local claim was saved", () => confirmation);
  joining.child.kill();
  assert.equal((await joining.done).signal, "SIGTERM");
  const claim = applicant.state().installerClaim;
  assert.ok(claim);
  assert.equal(primary.state().installerInvites[claim.invite].claim, undefined);
  const bot = path.join(applicant.folder, "bots", "bot_independent", "bot.json");
  fs.mkdirSync(path.dirname(bot), { recursive: true });
  fs.writeFileSync(bot, '{"id":"bot_independent","name":"Kept"}');
  const rejected = await applicant.start(["join"], code).done;
  assert.equal(rejected.status, 1);
  assert.match(rejected.err, /enrollment was cancelled/);
  assert.equal(applicant.state().installerClaim, undefined);
  assert.ok(primary.state().installerInvites[claim.invite].cancelled);
  intercept();
  relay.replay(confirmation);
  await wait(500);
  assert.deepEqual(link.peersOf(primary.state()), []);
  assert.match(fs.readFileSync(bot, "utf8"), /Kept/);
});

test("failure bench: Mac code re-enrollment retries a lost host key rotation reply", { timeout: 90_000 }, async () => {
  const relay = await installerRelay();
  const primary = box("mac-rotation-primary", relay);
  const target = box("mac-rotation-applicant", relay);
  await emptyEngine(primary, target);
  const old = target.state();
  const oldKey = keyOf(old);
  const main = primary.state();
  const seededRoster = main.shared.membershipAndAccess.value;
  seededRoster.left = [oldKey];
  seededRoster.computerHistory = { setup: main.setup.id, slots: [], retiredSigns: [link.ownSign(old)] };
  fs.writeFileSync(primary.file("link.json"), JSON.stringify({ ...main, pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
  primary.startServer();
  await primary.startBridge();
  const issued = await primary.request("POST", "kuvo/computers/add/code", { credentials: [] });
  assert.equal(issued.status, 201, JSON.stringify(issued.value));
  const code = issued.value.code + "\n";
  const kind = path.join(target.folder, "box", "kind");
  fs.mkdirSync(path.dirname(kind), { recursive: true });
  fs.writeFileSync(kind, "mac\n");
  const { key, sign, secret, relay: url, name, ...kept } = old;
  fs.writeFileSync(target.file("link.json"), JSON.stringify(kept));
  fs.writeFileSync(target.file("mac-link.json"), JSON.stringify({ relay: url, name, phones: true }));
  const password = randomBytes(16).toString("hex");
  let identity = { key, sign, secret }, savedAttempt, calls = 0;
  const host = http.createServer(async (request, response) => {
    const data = [];
    for await (const chunk of request) data.push(chunk);
    const body = JSON.parse(Buffer.concat(data));
    const send = value => response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(value));
    if (request.headers.authorization !== `Bearer ${password}` && request.url !== "/hello") return response.writeHead(403).end();
    if (request.url === "/hello") {
      return send({ proof: createHmac("sha256", password).update(`kuvo-host-hello\0${body.nonce}\x00127.0.0.1:${host.address().port}`).digest("hex") });
    }
    if (request.url === "/link/identity") return send({ ...identity, rotation: true });
    if (request.url !== "/link/rotate" || body.previous !== oldKey) return response.writeHead(400).end();
    calls++;
    if (savedAttempt === body.attempt) return send(identity);
    if (savedAttempt) return response.writeHead(409).end();
    savedAttempt = body.attempt;
    identity = { key: link.newKey().toString("base64url"), sign: link.newSigningSeed(), secret: randomBytes(32).toString("base64url") };
    response.destroy();
  });
  await new Promise(resolve => host.listen(0, "127.0.0.1", resolve));
  afterCase(() => { host.closeAllConnections(); host.close(); });
  fs.writeFileSync(target.file("host.json"), JSON.stringify({ url: `http://127.0.0.1:${host.address().port}`, password }));
  const initial = await target.start(["join"], code).done;
  assert.equal(initial.status, 1);
  assert.match(initial.err, /rotate this Mac/);
  assert.equal(target.state().macRotation.attempt, savedAttempt);
  const retry = await target.start(["join"], code).done;
  assert.equal(retry.status, 0, retry.err);
  assert.equal(calls, 2);
  assert.equal(primary.state().setup.epoch, main.setup.epoch, "the first relay lease keeps the code’s setup epoch");
  assert.ok(Object.values(primary.state().installerInvites).some(invite => invite.activated));
  assert.equal(target.state().setup.id, main.setup.id);
  assert.equal(target.state().macIdentityVersion, savedAttempt);
  assert.equal(target.state().macRotation, undefined);
  for (const privateField of ["key", "secret", "sign"]) assert.equal(target.state()[privateField], undefined);
  const roster = primary.state().shared.membershipAndAccess.value;
  assert.ok(roster.left.includes(oldKey));
  assert.ok(roster.computerHistory.retiredSigns.includes(link.ownSign(old)));
  assert.ok(roster.boxes.some(peer => peer.key === keyOf(identity) && peer.sign === link.ownSign(identity) && peer.kind === "mac"));
});
