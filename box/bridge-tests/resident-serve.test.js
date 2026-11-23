import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { relayStandIn } from "./relay-stand-in.js";

const script = fileURLToPath(LINK_BUNDLE);
const link = await import(script);
const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
const keyOf = key => link.publicOf(key).toString("base64url");

test("resident serve owns HTTP, relay, vault and the main lease across pairing and restart", { timeout: 120_000 }, async t => {
  const account = fs.mkdtempSync(path.join(os.tmpdir(), "ku-resident-"));
  removeAtEnd(account);
  const home = path.join(account, "box");
  const run = path.join(home, "run");
  fs.mkdirSync(run, { recursive: true });
  const record = path.join(run, "serve.json");
  const linkFile = path.join(run, "link.json");
  const holder = path.join(run, "vault.sock");
  const linkPID = path.join(run, "link.pid");
  const demand = path.join(run, "serve.needed");
  const leases = relayLeaseStandIn();
  const relay = await relayStandIn(link, { request: leases.request });
  atEnd(() => relay.close());
  const env = { PATH: process.env.PATH, HOME: account, KUVO_HOME: home, TMPDIR: account, KUVO_TEST_RELAY: relay.url };

  const launch = (...args) => {
    const child = spawn(process.execPath, [script, ...args], { env, stdio: ["ignore", "ignore", "pipe"] });
    let log = "";
    child.stderr.on("data", chunk => { log += chunk; });
    const ended = once(child, "exit");
    const stop = async () => {
      if (child.exitCode === null && child.signalCode === null) child.kill();
      return ended;
    };
    atEnd(stop);
    return { child, ended, stop, log: () => log };
  };
  const until = async (check, message) => {
    const deadline = Date.now() + 25_000;
    while (!check()) {
      if (Date.now() >= deadline) assert.fail(message());
      await sleep(25, undefined, { signal: t.signal });
    }
  };
  const ready = process => until(() => fs.existsSync(record) && read(record).pid === process.child.pid, process.log);
  const ask = async (path, method = "GET", body) => {
    const { port, password } = read(record);
    const headers = {
      authorization: "Basic " + Buffer.from("opencode:" + password).toString("base64"),
      "x-kuvo-from": "mac",
      ...(body ? { "content-type": "application/json" } : {}),
    };
    const reply = await fetch(`http://127.0.0.1:${port}/${path}`, {
      method, headers, body: body && JSON.stringify(body), signal: AbortSignal.any([t.signal, AbortSignal.timeout(10_000)]),
    });
    return { status: reply.status, value: await reply.json() };
  };
  const clockRow = async () => (await ask("kuvo/settings/clock")).value.groups
    .flatMap(group => group.rows).find(row => row.id === "time-zone");

  const starters = [launch("serve", "--resident"), launch("serve", "--resident")];
  await until(() => fs.existsSync(record), () => `Run files: ${fs.readdirSync(run).join(", ")}\n${starters.map(entry => entry.log()).join("\n")}`);
  const resident = starters.find(entry => entry.child.pid === read(record).pid);
  assert.ok(resident);
  const other = starters.find(entry => entry !== resident);
  assert.deepEqual(await other.ended, [1, null], other.log());
  assert.match(other.log(), /resident box already runs|Stop the existing box client processes/);
  assert.equal(Number(fs.readFileSync(linkPID, "utf8")), resident.child.pid);
  assert.equal(read(record).kept, false);
  assert.equal(fs.existsSync(demand), false);
  const socket = fs.statSync(holder);
  assert.ok(socket.isSocket());
  assert.equal(socket.mode & 0o077, 0);

  fs.writeFileSync(demand, "");
  for (const legacy of [launch("run"), launch("serve", "--kept")]) {
    assert.deepEqual(await legacy.ended, [0, null], legacy.log());
  }
  assert.equal(read(record).pid, resident.child.pid);
  assert.equal(fs.statSync(holder).ino, socket.ino);
  assert.equal((await ask("kuvo/box/serve")).value.serving, true);

  const identity = read(linkFile);
  const boxKey = link.publicOf(Buffer.from(identity.key, "base64url"));
  const address = link.addressOf(Buffer.from(identity.secret, "base64url"));
  const leaseFile = path.join(run, "setup-relay-lease.json");
  await until(() => fs.existsSync(leaseFile) && read(linkFile).setup.checkpointed, resident.log);
  assert.equal(read(leaseFile).holder, boxKey.toString("base64url"));
  assert.equal(relay.has(address), false, "the main lease does not require a device connection");
  assert.deepEqual((await ask("kuvo/phones")).value.phones, []);
  assert.equal((await ask("kuvo/info")).status, 200);

  const attention = await ask("kuvo/attention", "POST", { for: 30, sessions: [] });
  assert.equal(attention.status, 200);
  const pairedKey = link.newKey();
  const pair = async key => {
    const result = await ask("kuvo/devices/pair", "POST", { key: keyOf(key), name: "Parity desktop", kind: "desktop" });
    assert.equal(result.status, 200, JSON.stringify(result.value));
    return result.value.pairing;
  };
  const pairing = await pair(pairedKey);
  const secondKey = link.newKey();
  const secondPairing = await pair(secondKey);
  await until(() => relay.has(address), resident.log);

  let session = 0;
  const open = async (key, pair) => {
    const client = relay.device();
    const number = ++session;
    const initiator = new link.IKInitiator(key, boxKey, link.PROLOGUE);
    const send = (kind, body) => client.send(address, link.frame(kind, number, body));
    const receive = async () => {
      const given = link.readFrame(link.readRelay(await client.next(12_000)).payload);
      assert.equal(given.session, number);
      return given;
    };
    send(link.Frame.handshake1, initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, client: "desktop", pair }))));
    const welcome = await receive();
    assert.equal(welcome.kind, link.Frame.handshake2);
    const { transport } = initiator.readMessage2(welcome.body);
    const route = async path => {
      send(link.Frame.transport, transport.seal(link.encodeMessage({ t: "req", id: 1, m: "GET", p: path })));
      const first = link.decodeMessage(transport.open((await receive()).body));
      const chunks = [first.body];
      for (let last = first; !last.head.end;) {
        last = link.decodeMessage(transport.open((await receive()).body));
        chunks.push(last.body);
      }
      return { status: first.head.s, value: JSON.parse(Buffer.concat(chunks)) };
    };
    return { route, receive };
  };

  const before = await open(pairedKey);
  assert.equal((await before.route("kuvo/attention")).value.inUse, true, "HTTP and relay see the same live hold");
  assert.deepEqual(await before.route("kuvo/computers/add"), await ask("kuvo/computers/add"));
  const removed = await ask("kuvo/phones/remove", "POST", { phones: [{ key: keyOf(pairedKey), pairing }] });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  assert.equal((await before.receive()).kind, link.Frame.close);
  const token = await ask("kuvo/phones/pair", "POST");
  assert.equal(token.status, 200);
  await open(pairedKey, new URL(token.value.link).searchParams.get("token"));
  const grant = (await ask("kuvo/phones")).value.phones.find(phone => phone.key === keyOf(pairedKey));
  assert.ok(grant?.pairing);
  assert.notEqual(grant.pairing, pairing);

  // The main can commit during a WebSocket outage when its relay lease is still valid.
  const restoreRelay = relay.disconnect(address);
  const row = await clockRow();
  const saved = await ask("kuvo/settings/clock", "PATCH", { row: row.id, was: row.revision, value: "America/New_York" });
  assert.equal(saved.status, 200, JSON.stringify(saved.value));
  assert.equal(saved.value.status, "committed");
  const operation = saved.value.operation;
  assert.ok(operation);
  assert.equal(read(linkFile).sharedOutcomes[operation].status, "committed");

  assert.deepEqual(await resident.stop(), [0, null]);
  assert.equal(fs.existsSync(record), false);
  assert.equal(fs.existsSync(linkPID), false);
  const restarted = launch("serve", "--resident");
  await ready(restarted);
  assert.equal(relay.has(address), false);
  assert.equal((await ask("kuvo/info")).status, 200);
  assert.equal(read(linkFile).sharedOutcomes[operation].status, "committed");
  assert.equal((await clockRow()).value, "America/New_York");
  assert.equal(fs.statSync(holder).mode & 0o077, 0);
  restoreRelay();
  await until(() => relay.has(address), restarted.log);
  assert.equal((await (await open(pairedKey)).route("kuvo/setup")).value.you.role, "primary");

  const finalRemoval = await ask("kuvo/phones/remove", "POST", { phones: [
    { key: keyOf(pairedKey), pairing: grant.pairing },
    { key: keyOf(secondKey), pairing: secondPairing },
  ] });
  assert.equal(finalRemoval.status, 200, JSON.stringify(finalRemoval.value));
  await until(() => !relay.has(address), restarted.log);
  assert.equal(read(record).pid, restarted.child.pid);
  assert.equal((await ask("kuvo/info")).status, 200);

  assert.deepEqual(await restarted.stop(), [0, null]);
  const kept = launch("serve", "--kept");
  await ready(kept);
  const blocked = launch("serve", "--resident");
  assert.deepEqual(await blocked.ended, [1, null]);
  assert.match(blocked.log(), /Stop the existing box client processes/);
  assert.equal(read(record).pid, kept.child.pid);
});
