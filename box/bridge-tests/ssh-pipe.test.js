import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { relayStandIn } from "./relay-stand-in.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";

const script = fileURLToPath(LINK_BUNDLE);
const link = await import(script);
const prologue = Buffer.from("kuvo-ssh v1");
const line = value => JSON.stringify(value) + "\n";

function output(stream) {
  const chunks = stream[Symbol.asyncIterator]();
  let pending = Buffer.alloc(0);
  return {
    async line() {
      for (;;) {
        const end = pending.indexOf(10);
        if (end >= 0) {
          const value = JSON.parse(pending.subarray(0, end).toString("utf8"));
          pending = pending.subarray(end + 1);
          return value;
        }
        const next = await chunks.next();
        assert.ok(!next.done, "the command closed before the next JSON line");
        pending = Buffer.concat([pending, next.value]);
      }
    },
    async rest() {
      const body = [pending];
      for (let next; !(next = await chunks.next()).done;) body.push(next.value);
      return Buffer.concat(body);
    },
  };
}

test("the resident answers both SSH verbs and settles a tool call across a restart", { timeout: 140_000 }, async t => {
  const account = fs.mkdtempSync(path.join(os.tmpdir(), "ku-pipe-"));
  removeAtEnd(account);
  const home = path.join(account, "box");
  const run = path.join(home, "run");
  fs.mkdirSync(run, { recursive: true });
  const record = path.join(run, "serve.json");
  const pipe = path.join(run, "route", "ssh");
  const key = link.newKey(), device = link.newKey(), writer = link.newKey(), secret = link.newKey();
  const target = link.publicOf(key);
  const computer = target.toString("base64url");
  const peer = link.publicOf(device).toString("base64url");
  const pairing = "pipe-test-grant";
  const relay = await relayStandIn(link, { request: relayLeaseStandIn().request });
  atEnd(() => relay.close());
  fs.writeFileSync(path.join(run, "link.json"), JSON.stringify({ formatVersion: 1, key: key.toString("base64url"),
    sign: link.newSigningSeed(), secret: secret.toString("base64url"), relay: relay.url,
    phones: [
      { key: peer, name: "Fixture", from: "pair", kind: "desktop", pairing },
      { key: link.publicOf(writer).toString("base64url"), name: "Writer", from: "pair", kind: "desktop", pairing: "writer-grant" },
    ] }));
  const env = { PATH: process.env.PATH, HOME: account, KUVO_HOME: home, TMPDIR: account };
  const launch = (...args) => {
    const child = spawn(process.execPath, [script, ...args], { env, stdio: ["pipe", "pipe", "pipe"] });
    let log = "";
    child.stderr.on("data", chunk => { log += chunk; });
    const ended = once(child, "exit");
    const stop = (signal = "SIGTERM") => {
      if (child.exitCode === null && child.signalCode === null) child.kill(signal);
      return ended;
    };
    atEnd(() => stop());
    return { child, ended, stop, log: () => log, reply: output(child.stdout) };
  };
  const legacy = (request, ...flags) => {
    const command = launch("box", "route", ...flags);
    command.child.stdin.end(line(request));
    return command;
  };
  const one = async command => {
    const answer = await command.reply.line();
    assert.deepEqual(await command.ended, [0, null], command.log());
    assert.equal((await command.reply.rest()).length, 0, "one JSON line");
    return answer;
  };
  const open = async (request, sender = device, addressed = computer) => {
    const command = launch("box", "route-to");
    const initiator = new link.IKInitiator(sender, target, prologue);
    const hello = initiator.writeMessage1(Buffer.from('{"v":1,"client":"desktop"}')).toString("base64url");
    command.child.stdin.write(line({ computer: addressed, hello }));
    const greeting = await command.reply.line();
    if (greeting.hello) {
      const secure = initiator.readMessage2(Buffer.from(greeting.hello, "base64url"));
      const { allowed } = JSON.parse(secure.payload);
      if (allowed) command.child.stdin.write(line({ request: secure.transport.seal(Buffer.from(JSON.stringify(request))).toString("base64url") }));
      if (!allowed || !request.stream) command.child.stdin.end();
      return { ...command, allowed };
    }
    command.child.stdin.end();
    return { ...command, greeting };
  };
  const resident = async () => {
    const owner = launch("serve", "--resident");
    const ready = await owner.reply.line();
    assert.match(ready.url, /^http:\/\/127\.0\.0\.1:\d+$/);
    assert.equal(JSON.parse(fs.readFileSync(record, "utf8")).pid, owner.child.pid);
    return owner;
  };
  const local = async (route, body) => {
    const { port, password } = JSON.parse(fs.readFileSync(record, "utf8"));
    const reply = await fetch(`http://127.0.0.1:${port}/${route}`, { method: body ? "POST" : "GET",
      headers: { authorization: "Basic " + Buffer.from("opencode:" + password).toString("base64"),
        "x-kuvo-from": "mac", "content-type": "application/json" }, body: body && JSON.stringify(body), signal: t.signal });
    return { status: reply.status, value: await reply.json() };
  };

  const attention = { path: "kuvo/attention" };
  assert.equal((await one(legacy(attention))).status, 503, "unselected machines still use the cold command");
  assert.equal(fs.existsSync(pipe), false);
  let owner = await resident();
  const outer = fs.statSync(path.dirname(pipe));
  const socket = fs.lstatSync(pipe);
  assert.ok(socket.isSocket());
  assert.equal(outer.mode & 0o077, 0);
  assert.equal(socket.mode & 0o077, 0);
  const denied = net.createConnection(pipe);
  const deniedReply = output(denied);
  await once(denied, "connect");
  const bogus = Buffer.from(JSON.stringify({ addressed: false, internal: true, internalPath: "kuvo/info", key: "wrong" }));
  const size = Buffer.alloc(4);
  size.writeUInt32BE(bogus.length);
  denied.write(Buffer.concat([size, bogus]));
  assert.equal((await deniedReply.rest()).length, 0);

  fs.chmodSync(pipe, 0o666);
  assert.equal((await one(legacy(attention))).status, 503);
  fs.chmodSync(pipe, 0o600);
  const alias = await one(legacy({}, "--internal", "kuvo/attention"));
  assert.equal(alias.status, 200);
  assert.equal(alias.internal, true);
  assert.deepEqual(JSON.parse(alias.body), (await local(attention.path)).value);
  const addressed = await open(attention);
  assert.equal(addressed.allowed, true);
  assert.equal((await one(addressed)).status, 200, "the paired command sees the resident hold");
  const unpaired = await open(attention, link.newKey());
  assert.equal(unpaired.allowed, false);
  assert.equal((await one(unpaired)).status, 403);
  const wrong = await open(attention, device, "A".repeat(43));
  assert.equal(wrong.greeting.status, 409);
  assert.deepEqual(await wrong.ended, [0, null]);

  const pairRoute = { method: "POST", path: "kuvo/devices/pair", headers: { "x-kuvo-from": "app" }, fromApp: true, served: true };
  assert.equal((await one(legacy(pairRoute))).status, 403, "legacy SSH keeps OS-account route authority");

  const feed = { path: "kuvo/setup/events" };
  assert.equal(JSON.parse((await one(legacy(feed))).body).welcomed, false);
  const stream = await open({ ...feed, stream: true });
  assert.equal((await stream.reply.line()).type, "application/x-ndjson");
  assert.equal((await stream.reply.line()).welcomed, false);
  const welcomed = await one(legacy({ method: "POST", path: "kuvo/setup/welcomed", body: { welcomed: true } }));
  assert.equal(welcomed.status, 200);
  assert.equal((await stream.reply.line()).welcomed, true);

  // The stand-in commits once, then holds its reply through the crash.
  let arrived, closed;
  const waiting = () => {
    const request = new Promise(resolve => { arrived = resolve; });
    const disconnect = new Promise(resolve => { closed = resolve; });
    return { request, disconnect };
  };
  let writes = 0;
  const received = new Map();
  let prompts = 0;
  const receiver = { id: "ses_receiver", title: "Receiver", location: { directory: home } };
  const engine = http.createServer(async (req, res) => {
    const parts = [];
    for await (const part of req) parts.push(part);
    const url = new URL(req.url, "http://localhost");
    const reply = (data, status = 200) => res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify({ data }));
    if (url.pathname === "/api/session") return reply([receiver]);
    if (url.pathname === "/api/session/ses_receiver") return reply(receiver);
    if (url.pathname === "/api/session/ses_receiver/inbox") return reply([]);
    if (url.pathname.startsWith("/api/session/ses_receiver/message/")) {
      const message = received.get(url.pathname.split("/").at(-1));
      return reply(message ?? null, message ? 200 : 404);
    }
    if (url.pathname === "/api/session/ses_receiver/prompt" && req.method === "POST") {
      const message = JSON.parse(Buffer.concat(parts));
      prompts++;
      received.set(message.id, message);
      res.once("close", closed);
      arrived();
      return;
    }
    if (!["/api/pipe-binary", "/api/pipe-wait", "/api/pipe-write"].includes(url.pathname)) return reply(null, 404);
    if (req.url === "/api/pipe-binary") return res.writeHead(200, { "content-type": "application/octet-stream" }).end(Buffer.concat(parts));
    if (req.url === "/api/pipe-write") writes++;
    res.once("close", closed);
    arrived();
  });
  await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
  atEnd(() => { engine.closeAllConnections(); engine.close(); });
  fs.writeFileSync(path.join(run, "engine.json"), JSON.stringify({ pid: process.pid, port: engine.address().port, password: "fixture" }));

  const binary = Buffer.alloc(150_000);
  for (let i = 0; i < binary.length; i++) binary[i] = i % 256;
  const bytes = await open({ method: "POST", path: "api/pipe-binary", stream: true, bodyBase64: binary.toString("base64") });
  assert.equal((await bytes.reply.line()).type, "application/octet-stream");
  assert.deepEqual(await bytes.reply.rest(), binary);
  assert.deepEqual(await bytes.ended, [0, null]);

  const held = waiting();
  const finite = legacy({ path: "api/pipe-wait" });
  await held.request;
  await finite.stop();
  await held.disconnect;

  const streaming = waiting();
  const waitingStream = await open({ path: "api/pipe-wait", stream: true });
  await streaming.request;
  waitingStream.child.stdin.end();
  assert.deepEqual(await waitingStream.ended, [0, null]);
  await streaming.disconnect;

  const removed = await local("kuvo/phones/remove", { phones: [{ key: peer, pairing }] });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  assert.deepEqual(await stream.ended, [0, null], "the resident closes a revoked SSH stream");
  const revoked = await open(attention);
  assert.equal(revoked.allowed, false);
  assert.equal((await one(revoked)).status, 403);

  const write = waiting();
  const changed = await open({ method: "POST", path: "api/pipe-write", body: { value: "saved" } }, writer);
  assert.equal(changed.allowed, true);
  await write.request;
  assert.deepEqual(await owner.stop("SIGKILL"), [null, "SIGKILL"]);
  assert.deepEqual(await changed.ended, [1, null]);
  assert.equal((await changed.reply.rest()).length, 0);
  assert.equal(writes, 1);

  const request = { method: "POST", path: "kuvo/agent-tools", body: {
    name: "send_message", arguments: { to: receiver.id, text: "Please check the build." }, operation: "test_send_once",
  } };
  const delivering = waiting();
  const call = legacy(request, "--internal");
  await sleep(750); // There is still no listening socket.
  assert.equal(call.child.exitCode, null, "a call made during startup waits");
  owner = await resident();
  await Promise.race([delivering.request, call.ended.then(async () => assert.fail((await call.reply.line()).body))]);
  assert.deepEqual(await owner.stop("SIGKILL"), [null, "SIGKILL"]);
  await delivering.disconnect;
  owner = await resident();
  const first = await one(call);
  assert.equal(first.status, 200);
  assert.match(JSON.parse(first.body).text, /^Sent to Receiver/);
  assert.equal(prompts, 1, "recovery finds the committed prompt instead of posting it again");
  assert.deepEqual(await one(legacy(request, "--internal")), first);
  assert.equal(prompts, 1);
  assert.equal((await one(await open(attention, writer))).status, 200);
  assert.equal(writes, 1);
  assert.deepEqual(await owner.stop(), [0, null]);
});
