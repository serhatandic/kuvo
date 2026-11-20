import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { atEnd, removeAtEnd } from "./at-end.js";
import { relayStandIn } from "./relay-stand-in.js";

const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const link = await import(script);
const temp = fs.realpathSync(fs.mkdtempSync("/tmp/kuvo-enroll-"));
removeAtEnd(temp);

const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const keyOf = value => link.publicOf(Buffer.from(value.key, "base64url")).toString("base64url");
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
  const launch = (args, input) => spawnSync(process.execPath, [script, ...args], { input, encoding: "utf8", timeout: 12_000, env });
  const initialized = launch(["setup-local"]);
  assert.equal(initialized.status, 0, initialized.stderr);
  const start = (args, input) => {
    const child = spawn(process.execPath, [script, ...args], { env, stdio: [input ? "pipe" : "ignore", "pipe", "pipe"] });
    let out = "", err = "";
    child.stdout.on("data", data => { out += data; });
    child.stderr.on("data", data => { err += data; });
    const done = new Promise(resolve => child.once("close", (status, signal) => resolve({ status, signal, out, err })));
    atEnd(async () => {
      if (child.exitCode === null && child.signalCode === null) child.kill();
      await done;
    });
    if (input) child.stdin.end(input);
    return { child, done };
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
  const startBridge = async () => {
    const bridge = start(["run"]);
    const address = link.addressOf(Buffer.from(state().secret, "base64url"));
    await until(`${name}'s test-owned bridge connects`, () => fs.existsSync(file("link.pid"))
      && Number(fs.readFileSync(file("link.pid"), "utf8")) === bridge.child.pid && relay.has(address));
    return bridge;
  };
  const release = () => {
    for (const [name] of link.RELEASE_FILES) {
      const target = path.join(folder, name);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.writeFileSync(target, name === "bin/box-version" ? "0.8.0-beta.35\n" : "test release file");
    }
  };
  return { home, folder, file, start, state, request, startBridge, startServer, release };
}

async function emptyEngine(...boxes) {
  const engine = http.createServer((_request, response) => response.writeHead(200, { "content-type": "application/json" }).end('{"data":[]}'));
  await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
  atEnd(() => { engine.closeAllConnections(); engine.close(); });
  for (const box of boxes) fs.writeFileSync(box.file("engine.json"), JSON.stringify({ port: engine.address().port, password: "fake-engine" }));
}

test("failure bench: a lost merge answer keeps one accepted roster and resumes with both old Work Queues", { timeout: 50_000 }, async () => {
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
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
  fs.writeFileSync(primary.file("link.json"), JSON.stringify({ ...first, phones: [here, revoked] }));
  fs.writeFileSync(source.file("link.json"), JSON.stringify({ ...other, phones: [there], removed: [{ key: revoked.key, pairing: revoked.pairing }] }));
  primary.startServer();
  const initialServer = source.startServer();
  const primaryDaemon = await primary.startBridge();
  await source.startBridge();
  await until("both primary boxes connect", () => relay.has(targetAddress) && relay.has(sourceAddress));

  const make = async (box, title, operation) => {
    const result = await box.request("POST", "kuvo/work-queue/items", { title, body: "See #1 for the original." }, { "x-kuvo-operation": operation, "x-kuvo-from": "app" });
    assert.equal(result.status, 200, JSON.stringify(result.value));
    return result.value.item;
  };
  const oldA = await make(primary, "Work from the retained setup", "merge-primary-work");
  const oldB = await make(source, "Work from the joining setup", "merge-source-work");
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
  assert.equal(primary.state().shared.queueNumberAllocation.value.next, 3, "the same save has reserved both Work Queue numbers");
  initialServer.child.kill("SIGKILL");
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
  assert.equal(item.title, oldB.title);
  assert.equal(item.number, 2);
  assert.equal(item.body, "See #2 for the original.");
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
  assert.equal(unchangedAlias.value.item.title, oldB.title);
  const next = await make(primary, "Work after the merge", "merge-next-work");
  assert.ok(next.number >= 3);
  assert.equal(primary.state().shared.membershipAndAccess.value.boxes.filter(peer => peer.key === sourceKey).length, 1);
});

test("failure bench: ordinary enrollment leaves a used independent setup alone", { timeout: 35_000 }, async () => {
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
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
  const holder = "ssh-test-holder";
  fs.symlinkSync(holder, path.join(other.home, ".kuvo-code-adding"));
  const ssh = await other.start(["setup", holder], JSON.stringify({ mac: keep.state().setup.primary, macSign: keep.state().setup.sign, relay: relay.url })).done;
  assert.notEqual(ssh.status, 0);
  assert.match(ssh.err, /already has a Kuvo setup/);
  assert.equal(other.state().setup.id, before.setup.id);
  assert.equal(keyOf(other.state()), keyOf(before));
  assert.equal(fs.readFileSync(session, "utf8"), '{"id":"ses_here"}\n');
});

test("failure bench: a whole setup comes over after its standby accepts the departure", { timeout: 80_000 }, async () => {
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
  const keep = box("multi-keep", relay);
  const leaving = box("multi-leaving", relay);
  const member = box("multi-member", relay);
  const keptSetup = keep.state().setup;
  const oldSetup = leaving.state().setup;
  const memberKey = keyOf(member.state());
  await emptyEngine(keep, leaving, member);
  for (const box of [leaving, member]) fs.writeFileSync(box.file("test-setup-clock.json"), "{}");
  const labels = (box, word, count) => {
    const file = box.file("work-queue/labels.json");
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify({ formatVersion: 1, labels: Array.from({ length: count }, (_, index) =>
      ({ id: `lbl_${word}-${index}`, name: `${word} ${index}`, color: "graphite" })) }));
  };
  labels(leaving, "joining", 31);
  for (const box of [keep, leaving]) {
    fs.writeFileSync(box.file("link.json"), JSON.stringify({ ...box.state(), pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
    await box.startBridge();
  }
  keep.startServer();
  const initialServer = leaving.startServer();
  fs.writeFileSync(member.file("link.json"), JSON.stringify({ ...member.state(), pairing: { token: "fixture", expires: Date.now() + 180_000 } }));
  const originalBridge = await member.startBridge();
  const join = await leaving.request("POST", "kuvo/computers/add/code", {});
  assert.equal(join.status, 201, JSON.stringify(join.value));
  const enrolled = await member.start(["join"], join.value.code + "\n").done;
  assert.equal(enrolled.status, 0, enrolled.err);
  await until("the joining setup knows the member supports merge", () => fs.existsSync(leaving.file("peer-agents.json"))
    && read(leaving.file("peer-agents.json"))[memberKey]?.features?.includes("setup.merge"));
  await until("the old standby accepted its assignment and granted the primary a lease", () => {
    const setup = leaving.state().setup;
    return setup.epoch === oldSetup.epoch + 1 && setup.standby?.key === memberKey
      && member.state().setup.epoch === setup.epoch && fs.existsSync(leaving.file("setup-lease.json"))
      && read(leaving.file("setup-lease.json")).stamp === `${setup.id}:${setup.epoch}:${memberKey}`;
  });
  labels(keep, "retained", 31);
  const rejected = await keep.request("POST", "kuvo/computers/add/merge-code", { source: oldSetup.id, computer: oldSetup.primary });
  assert.equal(rejected.status, 201, JSON.stringify(rejected.value));
  const refused = await leaving.request("POST", "kuvo/setup/merge", { code: rejected.value.code });
  assert.equal(refused.status, 409, JSON.stringify(refused.value));
  assert.match(refused.value.message, /label capacity/);
  assert.equal(leaving.state().setup.id, oldSetup.id);
  assert.equal(member.state().setup.id, oldSetup.id);
  assert.equal(leaving.state().setupMerge, undefined, "the accepted refusal releases the old primary");
  assert.equal(member.state().shared.setupDeparture.value, null, "the old standby accepted the release");
  assert.equal(keep.state().shared.membershipAndAccess.value.mergedComputers?.[oldSetup.primary], undefined);
  labels(keep, "retained", 0);
  const next = await keep.request("POST", "kuvo/computers/add/merge-code", { source: oldSetup.id, computer: oldSetup.primary });
  assert.equal(next.status, 201, JSON.stringify(next.value));
  const paused = relay.pause(link.addressOf(Buffer.from(leaving.state().secret, "base64url")),
    link.addressOf(Buffer.from(member.state().secret, "base64url")));
  const interrupted = leaving.request("POST", "kuvo/setup/merge", { code: next.value.code }).catch(error => error);
  const outgoing = await until("the old primary recorded its departure intent while the standby was unreachable", () => leaving.state().setupMerge?.authority && leaving.state().setupMerge);
  assert.equal(outgoing.snapshot, undefined, "the source cannot seal its snapshot until the standby agrees");
  assert.equal(keep.state().shared?.membershipAndAccess?.value?.mergedComputers?.[oldSetup.primary], undefined);
  initialServer.child.kill("SIGKILL");
  await initialServer.done;
  assert.ok(await interrupted instanceof Error);
  paused();
  const readyAgain = leaving.startServer();
  await until("the joining server restarted", () => fs.existsSync(leaving.file("serve.json")) && read(leaving.file("serve.json")).pid === readyAgain.child.pid);
  const resumed = await keep.request("POST", "kuvo/computers/add/merge-code", { source: oldSetup.id, computer: oldSetup.primary, resume: outgoing.operation });
  assert.equal(resumed.status, 201, JSON.stringify(resumed.value));
  const merging = await leaving.request("POST", "kuvo/setup/merge", { code: resumed.value.code });
  assert.equal(merging.status, 200, JSON.stringify(merging.value));
  assert.deepEqual(keep.state().shared.membershipAndAccess.value.boxes.map(peer => peer.key).sort(),
    [keptSetup.primary, oldSetup.primary, memberKey].sort());
  assert.equal(keep.state().setup.role, "primary");
  assert.equal(leaving.state().setup.role, "member");
  const came = await until("the old standby adopts the new setup without asking a client", () => {
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
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
  const keep = box("retry-keep", relay);
  const leaving = box("retry-leaving", relay);
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
  child.child.kill("SIGKILL");
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
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
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
  const key = keyOf(sleeping.state());
  const address = link.addressOf(Buffer.from(sleeping.state().secret, "base64url"));
  const invite = await from.request("POST", "kuvo/computers/add/code", {});
  assert.equal(invite.status, 201, JSON.stringify(invite.value));
  const joined = await sleeping.start(["join"], invite.value.code + "\n").done;
  assert.equal(joined.status, 0, joined.err);
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

test("failure bench: a merge waits for a signed extension to the retained number block", { timeout: 60_000 }, async () => {
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
  const primary = box("range-retained", relay);
  const standby = box("range-standby", relay);
  const source = box("range-source", relay);
  await emptyEngine(primary, standby, source);
  const primaryBefore = primary.state().setup;
  const standbyKey = keyOf(standby.state());
  const ledger = source.file("work-queue/numbers.json");
  fs.mkdirSync(path.dirname(ledger), { recursive: true });
  fs.writeFileSync(ledger, JSON.stringify({ formatVersion: 1, primary: "self", next: 3001 }));
  for (const box of [primary, standby]) fs.writeFileSync(box.file("test-setup-clock.json"), "{}");
  for (const box of [primary, standby, source]) {
    fs.writeFileSync(box.file("link.json"), JSON.stringify({ ...box.state(), pairing: { token: "fixture", expires: Date.now() + 120_000 } }));
    await box.startBridge();
  }
  for (const box of [primary, source]) box.startServer();
  const invite = await primary.request("POST", "kuvo/computers/add/code", {});
  assert.equal(invite.status, 201, JSON.stringify(invite.value));
  const enrolled = await standby.start(["join"], invite.value.code + "\n").done;
  assert.equal(enrolled.status, 0, enrolled.err);
  const assigned = await until("the retained primary and standby agree on a first number ceiling", () => {
    const setup = primary.state().setup;
    return setup.epoch === primaryBefore.epoch + 1 && setup.standby?.key === standbyKey
      && standby.state().setup.epoch === setup.epoch && fs.existsSync(primary.file("setup-lease.json"))
      && read(primary.file("setup-lease.json")).stamp === `${setup.id}:${setup.epoch}:${standbyKey}` && setup;
  });
  assert.ok(assigned.numberCeiling < 3000);
  const outgoing = source.state().setup;
  const code = await primary.request("POST", "kuvo/computers/add/merge-code", { source: outgoing.id, computer: outgoing.primary });
  assert.equal(code.status, 201, JSON.stringify(code.value));
  const merged = await source.request("POST", "kuvo/setup/merge", { code: code.value.code });
  assert.equal(merged.status, 200, JSON.stringify(merged.value));
  assert.equal(primary.state().setup.primary, primaryBefore.primary);
  assert.ok(primary.state().setup.epoch > assigned.epoch);
  assert.ok(standby.state().setup.numberCeiling >= 3000);
  assert.ok(standby.state().shared.queueNumberAllocation.value.next >= 3001);
  assert.equal(standby.state().shared.membershipAndAccess.value.mergedComputers[outgoing.primary].operation, merged.value.operation);
});

test("failure bench: installer retries a saved join after a lost answer and rejects code reuse by a different computer", { timeout: 55_000 }, async () => {
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
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
  atEnd(() => { engine.closeAllConnections(); engine.close(); });
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
  assert.ok(!JSON.stringify(primary.state()).includes(token), "the primary stores the hash of the invitation");
  const joining = box("joining", relay);
  attachEngine(joining, "destination");
  const identity = joining.state();
  const key = keyOf(identity);
  const committed = () => primary.state().shared?.membershipAndAccess?.value?.boxes?.some(peer => peer.key === key);
  let release;
  const watch = relay.intercept(null, address, ({ from, payload }) => {
    if (!release && payload.includes(Buffer.from("kuvo installer v1"))) {
      release = relay.intercept(address, from, ({ payload }) => committed() && payload.includes(Buffer.from("kuvo installer v1")));
    }
    return false;
  });
  const initial = joining.start(["join"], code + "\n");
  await until("the primary saved accepted membership", committed);
  assert.equal(joining.state().setup.role, "primary", "the installer has not heard the result");
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
  failInstall = false;
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
  assert.ok(link.readDoc(joining.state().docs[`${first.setup.sign} peers`]));
  assert.equal(primary.state().shared.membershipAndAccess.value.boxes.filter(peer => peer.key === key).length, 1);

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
      assert.match(refused.value.message, /Update your primary/);
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

  // A saved SSH address also uses this owner when a one-shot client asks to set it up again.
  fs.writeFileSync(path.join(primary.folder, "machines.json"), JSON.stringify([{ destination: "from-member.invalid" }]));
  fs.writeFileSync(path.join(primary.folder, "phone-link.json"), JSON.stringify({ machines: { "from-member.invalid": { key } } }));
  const rerun = await primary.start(["box", "route"], JSON.stringify({ method: "POST", path: `kuvo/computers/${key}/setup` })).done;
  assert.equal(rerun.status, 0, rerun.err);
  const reply = JSON.parse(rerun.out);
  assert.equal(reply.status, 202, reply.body);
  const operation = JSON.parse(reply.body).try;
  assert.equal(operation.kind, "setup");
  const seen = await primary.request("GET", "kuvo/computers/add");
  assert.ok(seen.value.tries.some(attempt => attempt.id === operation.id));
});
