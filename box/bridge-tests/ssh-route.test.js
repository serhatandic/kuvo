import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { relayStandIn } from "./relay-stand-in.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";

const script = fileURLToPath(LINK_BUNDLE);
const link = await import(script);
const ssh = Buffer.from("kuvo-ssh v1");
const line = value => JSON.stringify(value) + "\n";

// Each SSH request starts a separate route process. Keep the request and relay deadlines below
// while giving the full sequence room to exercise the stream and revocation checks.
test("local, relay and SSH preserve their route contracts and paired operation identity", { timeout: 90_000 }, async t => {
  const account = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-ssh-route-"));
  removeAtEnd(account);
  const home = path.join(account, ".kuvo-code");
  const run = path.join(home, "run");
  fs.mkdirSync(run, { recursive: true });
  const relay = await relayStandIn(link, { request: relayLeaseStandIn().request });
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
  const version = "x-kuvo-route-version";
  let nextRelayID = 0;
  const sendRelay = part => phone.send(to, link.frame(link.Frame.transport, session, secure.seal(part)));
  const startRelay = ({ method, path, headers }, body) => {
    const id = ++nextRelayID;
    for (const part of link.pieces({ t: "req", id, m: method, p: path, h: headers }, body, true)) sendRelay(part);
    return id;
  };
  const nextRelay = async id => {
    const piece = link.decodeMessage(secure.open((await incoming()).body));
    assert.equal(piece.head.id, id);
    return piece;
  };
  const overRelay = async (request, body = Buffer.alloc(0)) => {
    const id = startRelay(request, body);
    const first = await nextRelay(id);
    const chunks = [first.body];
    for (let last = first; last.head.end !== true;) {
      last = await nextRelay(id);
      chunks.push(last.body);
    }
    return { status: first.head.s, version: first.head.h?.[version], type: first.head.h?.["content-type"], body: Buffer.concat(chunks) };
  };
  const response = await overRelay(request, Buffer.from(JSON.stringify(request.body)));
  assert.equal(response.status, 200);
  assert.deepEqual(JSON.parse(response.body).item, item);

  const engineReached = [];
  const engine = http.createServer(async (req, res) => {
    if (req.url !== "/api/parity") return res.writeHead(404).end();
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    engineReached.push({ req, res, body: Buffer.concat(chunks) });
  });
  await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
  atEnd(() => { engine.closeAllConnections(); engine.close(); });
  fs.writeFileSync(path.join(run, "engine.json"), JSON.stringify({ pid: process.pid, port: engine.address().port, password: "test-engine" }));
  fs.writeFileSync(path.join(run, "serve.needed"), "");
  const local = launch(["serve", "--kept"]);
  const record = path.join(run, "serve.json");
  const until = async (ready, message) => {
    const deadline = Date.now() + 10_000;
    while (!ready()) {
      if (t.signal.aborted || Date.now() >= deadline) assert.fail(message);
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  };
  await until(() => fs.existsSync(record) || local.child.exitCode !== null, "Local server was never ready");
  const { pid, port, password } = JSON.parse(fs.readFileSync(record, "utf8"));
  assert.equal(pid, local.child.pid);
  assert.equal(Number(fs.readFileSync(path.join(run, "link.pid"), "utf8")), daemon.child.pid);
  assert.notEqual(local.child.pid, daemon.child.pid);
  const authorization = "Basic " + Buffer.from("opencode:" + password).toString("base64");
  const fetchLocal = ({ method, path, headers }, body, auth = authorization) =>
    fetch(`http://127.0.0.1:${port}/${path}`, { method, headers: { authorization: auth, "x-kuvo-from": "mac", ...headers }, body, signal: t.signal });
  const overLocal = async (request, body) => {
    const reply = await fetchLocal(request, body);
    return { status: reply.status, version: reply.headers.get(version), type: reply.headers.get("content-type"), body: Buffer.from(await reply.arrayBuffer()) };
  };
  const overSsh = async (request, body) => {
    const routed = await open(body ? { ...request, bodyBase64: body.toString("base64") } : request);
    assert.equal(routed.allowed, true);
    const answer = JSON.parse((await routed.next.next()).value);
    assert.deepEqual(await routed.ended, [0, null]);
    return { status: answer.status, version: answer.routeVersion, type: answer.type,
      body: answer.bodyBase64 ? Buffer.from(answer.bodyBase64, "base64") : Buffer.from(answer.body) };
  };

  const missing = { method: "GET", path: "kuvo/parity-missing", headers: { "X-Kuvo-Route-Version": "1" } };
  assert.equal((await fetchLocal(missing, undefined, "Basic wrong")).status, 401);
  for (const [request, status, expectedBody] of [
    [missing, 404, {}],
    [{ ...missing, headers: { "X-Kuvo-Route-Version": "99" } }, 400, { code: "unsupportedFormatVersion" }],
  ]) {
    const replies = await Promise.all([overLocal(request), overRelay(request), overSsh(request)]);
    for (const reply of replies) {
      assert.equal(reply.status, status);
      assert.equal(reply.version, "1");
      assert.equal(reply.type, "application/json");
      const body = JSON.parse(reply.body);
      assert.deepEqual(expectedBody.code ? { code: body.code } : body, expectedBody);
      assert.deepEqual(reply.body, replies[0].body);
    }
  }
  const pair = { method: "POST", path: "kuvo/devices/pair" };
  assert.equal((await overLocal(pair)).status, 400, "the nearby desktop reaches pairing input validation");
  for (const reply of await Promise.all([overRelay(pair), overSsh(pair)])) {
    assert.equal(reply.status, 403);
    assert.match(JSON.parse(reply.body).message, /on this computer/);
  }
  const attention = { method: "GET", path: "kuvo/attention" };
  assert.equal((await overRelay(attention)).status, 200, "the machine relay has the live notice hold");
  assert.equal((await overLocal(attention)).status, 503);
  assert.equal((await overSsh(attention)).status, 503);

  // The engine withholds the response until all transports have passed the same binary request.
  const binary = Buffer.alloc(140_000);
  for (let i = 0; i < binary.length; i++) binary[i] = i % 256;
  const echo = { method: "POST", path: "api/parity", headers: { "Content-Type": "application/octet-stream", "X-Parity-Private": "discard" } };
  const replies = Promise.allSettled([overLocal(echo, binary), overRelay(echo, binary), overSsh(echo, binary)]);
  await until(() => engineReached.length === 3, "All three held requests must reach the engine");
  for (const { req, res, body } of engineReached) {
    assert.deepEqual(body, binary);
    assert.equal(req.headers["content-type"], "application/octet-stream");
    assert.equal(req.headers["x-parity-private"], undefined);
    res.writeHead(200, { "content-type": "application/octet-stream", "x-parity-private": "discard" }).end(body);
  }
  for (const reply of await replies) {
    if (reply.status === "rejected") throw reply.reason;
    assert.deepEqual(reply.value, { status: 200, version: "1", type: "application/octet-stream", body: binary });
  }

  const events = { method: "GET", path: "kuvo/setup/events" };
  const relayFeedID = startRelay(events, Buffer.alloc(0));
  const localFeed = await fetchLocal(events);
  const localReader = localFeed.body.getReader();
  const relayFirst = await nextRelay(relayFeedID);
  const sshFirst = await overSsh(events);
  const legacyFeed = spawnSync(process.execPath, [script, "box", "route"], { env, encoding: "utf8", input: line(events), timeout: 10_000 });
  assert.equal(legacyFeed.status, 0, legacyFeed.stderr);
  assert.equal(localFeed.headers.get("content-type"), "application/x-ndjson");
  assert.equal(relayFirst.head.h["content-type"], "application/x-ndjson");
  assert.equal(sshFirst.type, "application/x-ndjson");
  assert.equal(relayFirst.head.end, false);
  for (const body of [Buffer.from((await localReader.read()).value), relayFirst.body, sshFirst.body, JSON.parse(legacyFeed.stdout).body]) {
    assert.equal(JSON.parse(body.toString().split("\n")[0]).welcomed, false);
  }
  await localReader.cancel();
  sendRelay(link.encodeMessage({ t: "cancel", id: relayFeedID }));
  sendRelay(link.encodeMessage({ t: "ping" }));
  assert.equal(link.decodeMessage(secure.open((await incoming()).body)).head.t, "pong");

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
  const removal = JSON.parse((await removed.next.next()).value);
  assert.equal(removal.status, 200, removal.body);
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
