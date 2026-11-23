// The browser's live view on a Mac's box (PROTOCOL.md, "The browser's live view", #731): a Mac's
// box has no browser of its own, so it opens no view, through the relay or locally, and says why in
// words. The real box code, as a Mac's box.
//
// Run with `node --test box/bridge-tests/browser-view-mac.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { once } from "node:events";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-view-mac-")));
removeAtEnd(home);
process.env.KUVO_HOME = home;
process.env.KUVO_BOX = "mac";
process.env.KUVO_ENGINE_CONFIG = path.join(home, "engine");
const { engineConnect, viewUpgrade } = await import(LINK_BUNDLE.href);

test("through the relay, a Mac's box ends a view socket with why, opening nothing", async () => {
  let opened = false;
  const ended = new Promise(done => {
    engineConnect(() => { throw new Error("the engine isn't read for a view"); })(
      { path: "kuvo/browser/view", headers: { upgrade: "websocket" } },
      {},
      { open() { opened = true; }, text() {}, binary() {}, end: done },
    );
  });
  const error = await ended;
  assert.match(String(error?.message), /no browser of its own/);
  assert.equal(opened, false);
});

test("locally, a Mac's box answers the view's upgrade 404 with why", async t => {
  const server = http.createServer();
  let connections = 0;
  // (As serve sets it up: which box this is decides, never an argument.)
  viewUpgrade(server, "paired-secret", events => { connections++; queueMicrotask(events.open); return { send() {}, close() {} }; });
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  t.after(() => server.close());
  const socket = net.createConnection(server.address().port, "127.0.0.1");
  t.after(() => socket.destroy());
  socket.write(`GET /kuvo/browser/view HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: Basic ${Buffer.from("opencode:paired-secret").toString("base64")}\r\n\r\n`);
  const said = String((await once(socket, "data"))[0]);
  assert.match(said, /^HTTP\/1\.1 404/);
  assert.match(said, /no browser of its own/);
  assert.equal(connections, 0);
});
