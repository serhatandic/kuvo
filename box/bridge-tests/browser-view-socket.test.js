import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
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
const { viewUpgrade, frameBudget, socketAllowed, LinkHost, IKInitiator, PROLOGUE } = await import(LINK_BUNDLE.href);

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

// (#731) A Mac's box has no browser of its own: the view isn't opened, and it says why in words.
test("a Mac's box answers the view with why it has none, never opening it", async t => {
  const server = http.createServer();
  let connections = 0;
  viewUpgrade(server, "paired-secret", events => {
    connections++;
    queueMicrotask(events.open);
    return { send() {}, close() {} };
  }, false);
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  t.after(() => server.close());
  const socket = net.createConnection(server.address().port, "127.0.0.1");
  t.after(() => socket.destroy());
  socket.write(`GET /kuvo/browser/view HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Basic ${Buffer.from("opencode:paired-secret").toString("base64")}\r\n\r\n`);
  // (Its first word, so a box that opened the socket fails here rather than waiting on it.)
  let said = String((await once(socket, "data"))[0]);
  assert.match(said, /^HTTP\/1\.1 404/);
  for await (const chunk of socket) said += chunk;
  assert.match(JSON.parse(said.slice(said.indexOf("\r\n\r\n") + 4)).message, /no browser of its own/);
  assert.equal(connections, 0);
});

test("an unpaired relay client cannot open the browser socket", async () => {
  const crypto = await import("node:crypto");
  const link = await import(LINK_BUNDLE.href);
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
  const { browserView } = await import(LINK_BUNDLE.href);
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

async function keeperView(t, relay = false, options = {}) {
  const { browserView } = await import(LINK_BUNDLE.href);
  const { Heard } = await import("./browser-host-stand-in.js");
  const run = fs.mkdtempSync(path.join(os.tmpdir(), "kv-vk-")); removeAtEnd(run);
  const commands = new Heard(), heard = new Heard();
  let keeper, opened;
  const ready = new Promise(resolve => { opened = resolve; });
  const server = net.createServer(socket => {
    keeper = socket;
    let buffer = "";
    socket.on("data", data => {
      buffer += data.toString();
      let at;
      while ((at = buffer.indexOf("\n")) >= 0) {
        const m = JSON.parse(buffer.slice(0, at)); buffer = buffer.slice(at + 1);
        commands.add(m);
        if (m.method === "Kuvo.key") socket.write(JSON.stringify({ method: "Kuvo.hello", params: {} }) + "\n");
      }
    });
  });
  server.listen(path.join(run, "browser.sock")); await once(server, "listening");
  const view = browserView({
    open: opened, text(data) { heard.add(JSON.parse(data)); },
    binary(data) { heard.add({ binary: data }); }, end(error) { heard.add({ ended: true, error }); },
  }, relay, run, () => "key", options);
  t.after(() => { view.close(); keeper?.destroy(); server.close(); });
  await ready;
  return { view, heard, commands, emit: m => keeper.write(JSON.stringify(m) + "\n") };
}
test("the view socket exposes Kuvo facts and request replies without leaking raw CDP events", async t => {
  const h = await keeperView(t);
  h.emit({ method: "Target.targetCreated", params: { targetInfo: { targetId: "secret-cdp" } } });
  h.emit({ id: 4, result: {} });
  h.emit({ method: "Kuvo.pages", params: { pages: [], active: null } });
  await h.heard.until(m => m.method === "Kuvo.pages");
  assert.equal(h.heard.all.some(m => m.method === "Target.targetCreated"), false);
  assert.ok(h.heard.all.some(m => m.id === 4));
});

test("an input-reply burst is backpressured and delivered without cutting off a healthy viewer", async t => {
  const h = await keeperView(t, true);
  for (let id = 1000; id < 1250; id++) h.emit({ id, result: {} });
  await h.heard.until(m => m.id === 1249);
  assert.equal(h.heard.all.filter(m => m.id >= 1000 && m.id < 1250).length, 250);
  assert.equal(h.heard.all.some(m => m.ended), false);
});

test("large chunked view input and split UTF-8 are joined once at the line boundary", async t => {
  const h = await keeperView(t);
  const message = { id: 88, method: "Kuvo.upload", params: { targetId: "P", files: [{ name: "café", data: "a".repeat(2 * 1024 * 1024) }] } };
  const bytes = Buffer.from(JSON.stringify(message) + "\n");
  for (let at = 0; at < bytes.length; at += 101) h.view.send(bytes.subarray(at, at + 101));
  const received = await h.commands.until(m => m.id === 88);
  assert.deepEqual(received, message);
});

test("relay pictures are skipped while host.unsent reports buffered connection traffic", async t => {
  let unsent = 1;
  const h = await keeperView(t, true, { unsent: () => unsent, budget: () => true });
  const frame = id => ({ method: "Kuvo.frame", params: { targetId: "P", frameId: id, data: "anBlZw==", width: 1, height: 1, scrollX: 0, scrollY: 0 } });
  h.emit(frame(1));
  await h.commands.until(m => m.method === "Kuvo.frameDropped" && m.params.frameId === 1);
  assert.equal(h.heard.all.some(m => m.binary), false);
  unsent = 0;
  h.emit(frame(2));
  await h.heard.until(m => m.binary);
  assert.equal(h.heard.all.filter(m => m.binary).length, 1);
});

test("Mac-kind boxes do not advertise a keeper live view before their own browser is enabled", async () => {
  const { macFeatures } = await import(LINK_BUNDLE.href);
  assert.equal(macFeatures().includes("browser.view"), false);
  assert.equal(macFeatures().includes("browser"), false);
  assert.equal(macFeatures().includes("browser.data"), true);
});

test("opening a view socket never registers it as an ask-answering legacy viewer", async t => {
  const h = await keeperView(t);
  h.view.send(Buffer.from('{"id":19,"method":"Kuvo.pages","params":{}}\n'));
  await h.commands.until(m => m.id === 19);
  assert.equal(h.commands.all.some(m => m.method === "Kuvo.viewing"), false);
});

test("external URL examples always identify the page whose viewers may handle them", () => {
  const vectors = JSON.parse(fs.readFileSync(new URL("../../protocol/vectors/browser-view.json", import.meta.url)));
  for (const fact of vectors.facts.filter(f => f.method === "Kuvo.external")) assert.equal(typeof fact.params.targetId, "string");
});

for (const reason of ["budget", "unsent"]) {
  test("re-review M3: relay " + reason + " skip is a drop, not a display acknowledgement", async t => {
    const h = await keeperView(t, true, { budget: () => reason !== "budget", unsent: () => reason === "unsent" ? 1 : 0 });
    h.emit({ method: "Kuvo.frame", params: { targetId: "P", frameId: 990, data: "anBlZw==" } });
    const reply = await h.commands.until(m => m.params?.frameId === 990);
    assert.equal(reply.method, "Kuvo.frameDropped");
    assert.equal(reply.params.reason, reason);
  });
}

test("re-review S2: the view socket permits typed dialog answers", async t => {
  const h = await keeperView(t);
  h.view.send(Buffer.from('{"id":92,"method":"Kuvo.dialogAnswer","params":{"targetId":"P","accept":true}}\n'));
  h.emit({ method: "Kuvo.pages", params: { pages: [], active: null } });
  await h.heard.until(m => m.method === "Kuvo.pages" || m.ended);
  assert.equal(h.heard.all.some(m => m.ended), false);
  await h.commands.until(m => m.id === 92 && m.method === "Kuvo.dialogAnswer");
});
