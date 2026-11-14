import "./test-safety.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import http from "node:http";
import net from "node:net";
import { once } from "node:events";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";
process.env.KUVO_HOME = fs.mkdtempSync(path.join(os.tmpdir(), "kv-view-"));
removeAtEnd(process.env.KUVO_HOME);
const { viewUpgrade, frameBudget, socketAllowed, LinkHost, IKInitiator, PROLOGUE } = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");

test("the shared frame message budget drops pictures without spending the next window", () => {
  for (let i = 0; i < 100; i++) assert.equal(frameBudget(1, 100000), true);
  assert.equal(frameBudget(1, 100000), false);
  assert.equal(frameBudget(1, 101000), true);
});

test("serve upgrades only the view path with its password", async t => {
  const server = http.createServer();
  let connections = 0;
  viewUpgrade(server, "paired-secret", events => {
    connections++;
    queueMicrotask(events.open);
    return { send() {}, close() {} };
  });
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  t.after(() => server.close());
  async function open(route, password) {
    const socket = net.createConnection(server.address().port, "127.0.0.1");
    t.after(() => socket.destroy());
    socket.write(`GET /${route} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Basic ${Buffer.from("opencode:" + password).toString("base64")}\r\n\r\n`);
    return String((await once(socket, "data"))[0]);
  }
  assert.match(await open("kuvo/browser/view", "wrong"), /401/);
  assert.match(await open("kuvo/browser/elsewhere", "paired-secret"), /403/);
  assert.equal(connections, 0);
  assert.match(await open("kuvo/browser/view", "paired-secret"), /101 Switching Protocols/);
  assert.equal(connections, 1);
  assert.equal(socketAllowed("kuvo/browser/view"), true);
  assert.equal(socketAllowed("kuvo/browser/view/other"), false);
  assert.equal(socketAllowed("kuvo/browser/%2e%2e/view"), false);
});

test("an unpaired relay client cannot open the browser socket", async () => {
  const crypto = await import("node:crypto");
  const link = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
  const key = crypto.randomBytes(32), phone = crypto.randomBytes(32), address = crypto.randomBytes(16);
  let connected = 0, paired = false, sent = [];
  const host = new LinkHost({
    key, hello: { v: 1, kind: "machine", name: "Test", app: "0" },
    authorize: () => paired, handle: async function* () {},
    connect: () => { connected++; return { send() {}, close() {} }; },
    send: bytes => sent.push(bytes),
  });
  async function hello(number) {
    const initiator = new IKInitiator(phone, link.publicOf(key), PROLOGUE);
    sent = [];
    await host.receive(address, link.frame(link.Frame.handshake1, number, initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, name: "Test" })))));
    return { initiator, frame: link.readFrame(link.readRelay(sent[0]).payload) };
  }
  assert.equal((await hello(1)).frame.kind, link.Frame.close);
  assert.equal(connected, 0);
  paired = true;
  const opened = await hello(2);
  const { transport } = opened.initiator.readMessage2(opened.frame.body);
  await host.receive(address, link.frame(link.Frame.transport, 2, transport.seal(link.encodeMessage({
    t: "req", id: 1, m: "GET", p: "kuvo/browser/view", h: { upgrade: "websocket" },
  }))));
  assert.equal(connected, 1);
  host.reset();
});

test("relay pictures are binary and excess or oversized frames are dropped at the socket", async t => {
  const { browserView } = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
  const run = fs.mkdtempSync(path.join(os.tmpdir(), "kv-frames-"));
  removeAtEnd(run);
  const frames = [], acknowledgements = [];
  let complete;
  const done = new Promise(resolve => { complete = resolve; });
  const server = net.createServer(socket => {
    let input = "";
    socket.setEncoding("utf8");
    socket.on("data", chunk => {
      input += chunk;
      let at;
      while ((at = input.indexOf("\n")) >= 0) {
        const message = JSON.parse(input.slice(0, at)); input = input.slice(at + 1);
        if (message.method === "Kuvo.key") {
          assert.equal(message.params.key, "private");
          const lines = [{ method: "Kuvo.hello", params: {} }];
          for (let frameId = 1; frameId <= 103; frameId++) lines.push({
            method: "Kuvo.frame", params: { targetId: "P", frameId, width: 393, height: 246,
              scrollX: 0, scrollY: 0, data: Buffer.alloc(frameId === 103 ? 61000 : 100, 7).toString("base64") },
          });
          socket.write(lines.map(m => JSON.stringify(m) + "\n").join(""));
        }
        if (["Kuvo.frameAck", "Kuvo.frameDropped"].includes(message.method)) {
          acknowledgements.push(message.params.frameId);
          if (message.params.frameId === 103) complete();
        }
      }
    });
  });
  server.listen(path.join(run, "browser.sock")); await once(server, "listening");
  const view = browserView({ open() {}, text() {}, binary(data) { frames.push(data); }, end() {} }, true, run, () => "private");
  t.after(() => { view.close(); server.close(); });
  await done;
  assert.ok(frames.length > 0 && frames.length <= 80);
  assert.equal(acknowledgements.length, 103 - frames.length);
  const first = frames[0], length = first.readUInt32BE(0);
  assert.equal(JSON.parse(first.subarray(4, 4 + length)).method, "Kuvo.frame");
  assert.deepEqual(first.subarray(4 + length), Buffer.alloc(100, 7));
});

test("box-only browser socket paths follow their own vectors without changing kept-up Swift vectors", () => {
  const vectors = JSON.parse(fs.readFileSync(new URL("../../protocol/vectors/browser-view.json", import.meta.url)));
  for (const v of vectors.sockets) assert.equal(socketAllowed(v.path), v.allowed);
  const shared = JSON.parse(fs.readFileSync(new URL("../../protocol/vectors/link.json", import.meta.url)));
  assert.ok(shared.sockets.every(v => !v.path.startsWith("kuvo/browser/")));
});
