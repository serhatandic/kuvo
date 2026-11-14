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
process.env.KUVO_HOME = fs.mkdtempSync(path.join(os.tmpdir(), "kv-ws-"));
removeAtEnd(process.env.KUVO_HOME);
const { viewUpgrade } = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
async function server(t) {
  const server = http.createServer();
  const calls = [];
  let events;
  viewUpgrade(server, "secret", e => {
    events = e; queueMicrotask(e.open);
    return { send(data) { calls.push(data); }, close() {} };
  });
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  t.after(() => server.close());
  async function open(password = "secret") {
    const socket = net.createConnection(server.address().port, "127.0.0.1");
    socket.on("error", () => {});
    t.after(() => socket.destroy());
    socket.write(`GET /kuvo/browser/view HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Basic ${Buffer.from("opencode:" + password).toString("base64")}\r\n\r\n`);
    const [response] = await once(socket, "data");
    return { socket, response: String(response) };
  }
  return { open, calls, events: () => events };
}
function masked(opcode, data = Buffer.alloc(0), fin = true) {
  const header = Buffer.from([opcode | (fin ? 128 : 0), 128 | data.length, 1, 2, 3, 4]);
  return Buffer.concat([header, Buffer.from(data.map((n, i) => n ^ header[2 + i % 4]))]);
}
test("resetting an upgrade refused with 401 cannot raise an unhandled socket error", async t => {
  const h = await server(t);
  const { socket, response } = await h.open("wrong");
  assert.match(response, /401/);
  socket.resetAndDestroy();
  // A fresh authenticated connection proves the listener and its process survived the reset.
  assert.match((await h.open()).response, /101/);
  assert.equal(h.calls.length, 0);
});

test("empty continuation fragments have a count limit even when their byte size is zero", async t => {
  const h = await server(t), { socket } = await h.open();
  const closed = once(socket, "close");
  socket.write(Buffer.concat([masked(1, Buffer.alloc(0), false), ...Array.from({ length: 4096 }, () => masked(0, Buffer.alloc(0), false))]));
  await closed;
  assert.equal(h.calls.length, 0);
});

test("a slow serve viewer skips and credits pictures while retaining text and its connection", async () => {
  const { EventEmitter } = await import("node:events");
  const server = new EventEmitter(), socket = new EventEmitter();
  const writes = [], commands = [];
  Object.assign(socket, { writableLength: 0, destroyed: false, write(data) { writes.push(data); }, end() {}, destroy() { this.destroyed = true; } });
  let events;
  viewUpgrade(server, "secret", e => { events = e; return { send(data) { commands.push(JSON.parse(data)); }, close() {} }; });
  server.emit("upgrade", { method: "GET", url: "/kuvo/browser/view", headers: {
    authorization: "Basic " + Buffer.from("opencode:secret").toString("base64"),
    "sec-websocket-version": "13", "sec-websocket-key": "dGhlIHNhbXBsZSBub25jZQ==", upgrade: "websocket", connection: "Upgrade",
  } }, socket, Buffer.alloc(0));
  events.open();
  socket.writableLength = 300000;
  const json = Buffer.from(JSON.stringify({ method: "Kuvo.frame", params: { frameId: 12 } }));
  const length = Buffer.alloc(4); length.writeUInt32BE(json.length);
  events.binary(Buffer.concat([length, json, Buffer.from("jpeg")]));
  events.text(Buffer.from('{"id":7,"result":{}}\n'));
  assert.equal(socket.destroyed, false);
  assert.equal(writes.length, 2);
  assert.match(writes[1].toString(), /"id":7/);
  assert.deepEqual(commands, [{ method: "Kuvo.frameAck", params: { frameId: 12 } }]);
});
