import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { relayStandIn } from "./relay-stand-in.js";

const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
const link = await import(script);
const ssh = Buffer.from("kuvo-ssh v1");
const line = value => JSON.stringify(value) + "\n";

// Each SSH request starts a separate route process. Keep the request and relay deadlines below
// while giving the full sequence room to exercise the stream and revocation checks.
test("failure bench: paired requests keep the same Work Queue operation over SSH and relay", { timeout: 75_000 }, async t => {
  const account = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-ssh-route-"));
  removeAtEnd(account);
  const home = path.join(account, ".kuvo-code");
  const run = path.join(home, "run");
  fs.mkdirSync(run, { recursive: true });
  const relay = await relayStandIn(link);
  atEnd(() => relay.close());
  const key = link.newKey(), device = link.newKey(), secret = link.newKey();
  const to = link.addressOf(secret);
  const target = link.publicOf(key);
  const computer = target.toString("base64url");
  const peer = link.publicOf(device).toString("base64url");
  fs.writeFileSync(path.join(run, "link.json"), JSON.stringify({ formatVersion: 1, key: key.toString("base64url"), sign: link.newSigningSeed(), secret: secret.toString("base64url"), relay: relay.url,
    phones: [{ key: peer, name: "Desktop", from: "pair", kind: "desktop", pairing: "paired-fixture" }] }));
  const env = { PATH: process.env.PATH, HOME: account, KUVO_HOME: home, TMPDIR: account };
  const prepare = spawnSync(process.execPath, [script, "setup-local"], { env, encoding: "utf8", timeout: 10_000 });
  assert.equal(prepare.status, 0, prepare.stderr);

  const launch = args => {
    const child = spawn(process.execPath, [script, ...args], { env, stdio: ["pipe", "pipe", "ignore"] });
    const ended = once(child, "exit");
    const stop = () => { if (child.exitCode === null && child.signalCode === null) child.kill("SIGTERM"); };
    t.signal.addEventListener("abort", stop, { once: true });
    atEnd(async () => { stop(); await ended; });
    return { child, ended, next: createInterface({ input: child.stdout })[Symbol.asyncIterator]() };
  };
  const start = (sender = device, app) => {
    const process = launch(["box", "route-to"]);
    const initiator = new link.IKInitiator(sender, target, ssh);
    const hello = initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, client: "desktop", app }))).toString("base64url");
    return { ...process, initiator, hello };
  };
  const open = async (request, sender = device, app) => {
    const command = start(sender, app);
    const { child, initiator, hello, next } = command;
    child.stdin.write(line({ computer, hello }));
    const first = JSON.parse((await next.next()).value);
    const secure = initiator.readMessage2(Buffer.from(first.hello, "base64url"));
    const proof = JSON.parse(secure.payload);
    const allowed = proof.allowed;
    if (allowed) {
      child.stdin.write(line({ request: secure.transport.seal(Buffer.from(JSON.stringify(request))).toString("base64url") }));
    }
    if (!request.stream || !allowed) child.stdin.end();
    return { ...command, allowed, proof };
  };

  const wrong = start();
  wrong.child.stdin.end(line({ computer: "A".repeat(43), hello: wrong.hello }));
  assert.equal(JSON.parse((await wrong.next.next()).value).status, 409);
  assert.deepEqual(await wrong.ended, [0, null]);

  const request = { method: "POST", path: "kuvo/work-queue/items", headers: { "x-kuvo-operation": "ssh-relay-same-write", "idempotency-key": "ssh-relay-same-write", "content-type": "application/json" }, body: { title: "Written once from both transports" } };
  const unpaired = await open(request, link.newKey());
  assert.equal(unpaired.allowed, false);
  assert.equal(JSON.parse((await unpaired.next.next()).value).status, 403);
  assert.deepEqual(await unpaired.ended, [0, null]);
  const legacy = spawnSync(process.execPath, [script, "box", "route"], { env, encoding: "utf8", input: line(request), timeout: 10_000 });
  assert.equal(JSON.parse(legacy.stdout).status, 403, "the unsigned local command cannot act as a paired client");

  const written = await open(request);
  assert.equal(written.allowed, true);
  const added = JSON.parse((await written.next.next()).value);
  assert.equal(added.status, 200, added.body);
  assert.equal(added.computer, computer);
  const item = JSON.parse(added.body).item;
  assert.equal(item.title, request.body.title);
  assert.deepEqual(await written.ended, [0, null]);

  const daemon = launch(["run"]);
  daemon.child.stdin.end();
  while (!relay.has(to)) {
    if (t.signal.aborted || daemon.child.exitCode !== null) assert.fail("The box did not connect to the relay.");
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const phone = relay.device();
  const session = 7;
  const initiator = new link.IKInitiator(device, target, link.PROLOGUE);
  phone.send(to, link.frame(link.Frame.handshake1, session, initiator.writeMessage1(Buffer.from('{"v":1,"client":"desktop"}'))));
  const incoming = async () => link.readFrame(link.readRelay(await phone.next(5000)).payload);
  const welcome = await incoming();
  assert.equal(welcome.kind, link.Frame.handshake2);
  const secure = initiator.readMessage2(welcome.body).transport;
  for (const part of link.pieces({ t: "req", id: 1, m: request.method, p: request.path, h: request.headers }, Buffer.from(JSON.stringify(request.body)), true)) {
    phone.send(to, link.frame(link.Frame.transport, session, secure.seal(part)));
  }
  const response = [];
  let last;
  do {
    last = link.decodeMessage(secure.open((await incoming()).body));
    response.push(last);
  } while (last.head.end !== true);
  assert.equal(response[0].head.s, 200);
  assert.deepEqual(JSON.parse(Buffer.concat(response.map(piece => piece.body))).item, item);

  const linkFile = path.join(run, "link.json");
  const beforeRelease = JSON.parse(fs.readFileSync(linkFile, "utf8"));
  const signed = Buffer.from(JSON.stringify({ manifest: Buffer.from('{"clientProtocol":99}').toString("base64") })).toString("base64");
  fs.writeFileSync(linkFile, JSON.stringify({ ...beforeRelease, setup: { ...beforeRelease.setup, checkpointed: true },
    sharedSetup: beforeRelease.setup.id, shared: {
    ...beforeRelease.shared, setupSecurityAndReleaseChoice: { sequence: 1, value: { version: "0.10.0", sha256: "a".repeat(64), signed } },
  } }));
  const older = await open({ method: "GET", path: "kuvo/info" }, device, "0.9.0");
  assert.equal(older.allowed, true);
  assert.equal(JSON.parse((await older.next.next()).value).status, 200);
  await older.ended;
  const olderRelay = new link.IKInitiator(device, target, link.PROLOGUE);
  phone.send(to, link.frame(link.Frame.handshake1, session + 1, olderRelay.writeMessage1(Buffer.from('{"v":1,"client":"desktop","app":"0.9.0"}'))));
  const relayProof = JSON.parse(olderRelay.readMessage2((await incoming()).body).payload);
  assert.equal(relayProof.v, 1);
  assert.equal(relayProof.reason, undefined);
  const newer = await open({ method: "GET", path: "kuvo/info" }, device, "0.10.0");
  assert.equal(newer.allowed, true);
  assert.equal(JSON.parse((await newer.next.next()).value).status, 200);
  await newer.ended;
  fs.writeFileSync(linkFile, JSON.stringify(beforeRelease));

  const live = await open({ method: "GET", path: "kuvo/setup/events", stream: true });
  const head = JSON.parse((await live.next.next()).value);
  assert.equal(head.status, 200);
  assert.equal(head.computer, computer);
  assert.equal(head.type, "application/x-ndjson");
  assert.equal(JSON.parse((await live.next.next()).value).welcomed, false);
  const welcomed = await open({ method: "POST", path: "kuvo/setup/welcomed", body: { welcomed: true } });
  assert.equal(JSON.parse((await welcomed.next.next()).value).status, 200);
  assert.equal(JSON.parse((await live.next.next()).value).welcomed, true);
  live.child.stdin.end();
  assert.deepEqual(await live.ended, [0, null], "disconnect stops the idle stream");

  const revoked = await open({ method: "GET", path: "kuvo/setup/events", stream: true });
  assert.equal(JSON.parse((await revoked.next.next()).value).status, 200);
  assert.equal(JSON.parse((await revoked.next.next()).value).welcomed, true);
  const removed = await open({ method: "POST", path: "kuvo/phones/remove", body: { phones: [{ key: peer, pairing: "paired-fixture" }] } });
  assert.equal(JSON.parse((await removed.next.next()).value).status, 200);
  assert.deepEqual(await revoked.ended, [0, null], "removing a paired device stops its existing SSH stream");
  const refused = await open(request);
  assert.equal(refused.allowed, false);
  await refused.ended;

  const macHome = path.join(account, "disabled-mac");
  const macRun = path.join(macHome, "run");
  fs.mkdirSync(macRun, { recursive: true });
  fs.mkdirSync(path.join(macHome, "box"), { recursive: true });
  fs.writeFileSync(path.join(macHome, "box", "kind"), "mac\n");
  fs.writeFileSync(path.join(macRun, "mac-link.json"), JSON.stringify({ phones: false, relay: relay.url }));
  const mac = spawnSync(process.execPath, [script, "box", "route-to"], { env: { ...env, KUVO_HOME: macHome },
    encoding: "utf8", input: line({ computer, hello: wrong.hello }), timeout: 5000 });
  assert.equal(mac.status, 0, mac.stderr);
  const denied = JSON.parse(mac.stdout);
  assert.equal(denied.status, 403, denied.body);
  assert.match(JSON.parse(denied.body).message, /off on this Mac/);
});
