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
