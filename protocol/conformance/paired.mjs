// A desktop paired with one computer's box and talking to it over the relay, as the app does with
// every computer but its own: paired by the box on that computer, then a Noise session through the
// relay stand-in. The wire format comes from the TypeScript bundle.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { readJSON, relayAddress, until } from "./adapter.mjs";

export const wireFixture = fileURLToPath(new URL("../../box/dist/kuvo-link.js", import.meta.url));

export const hasWire = () => fs.existsSync(wireFixture);

export async function loadWire(home, env) {
  const fixtureEnv = { ...env, KUVO_HOME: home };
  const previous = Object.fromEntries(Object.keys(fixtureEnv).map(key => [key, process.env[key]]));
  Object.assign(process.env, fixtureEnv);
  try {
    return await import(pathToFileURL(wireFixture).href);
  } finally {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
}

/** Pairs a desktop with `computer` (its local `ask`) under `secret`, and opens a session `session`
 * to it through `relay`: `ask(method, route, body, headers)` answers `{ status, json }`. */
export async function relayDesktop(t, relay, computer, ask, secret, session, name, { relaySecret } = {}) {
  const wire = await loadWire(computer.home, computer.env);
  const key = wire.publicOf(secret).toString("base64url");
  const paired = await ask("POST", "kuvo/devices/pair", { key, kind: "desktop", name }, { "x-kuvo-from": "app" });
  assert.equal(paired.status, 200, paired.text);
  const link = readJSON(path.join(computer.home, "run", "link.json"));
  const address = relayAddress(relaySecret ?? Buffer.from(link.secret, "base64url"));
  if (relaySecret) await until(() => relay.has(address), "The Mac did not connect to the fixture relay", 20000);
  const target = Buffer.from((await ask("GET", "kuvo/setup")).json.you.key, "base64url");
  const device = relay.device();
  t.after(() => device.leave());
  const send = (kind, body) => device.send(address, wire.frame(kind, session, body));
  // (A change forwarded to its keeper is answered within 20 seconds.)
  const receive = async kind => {
    const message = wire.readRelay(await device.next(25000));
    assert.equal(message.kind, "deliver");
    const frame = wire.readFrame(message.payload);
    assert.equal(frame.session, session);
    assert.equal(frame.kind, kind);
    return frame.body;
  };
  const initiator = new wire.IKInitiator(secret, target, wire.PROLOGUE);
  const hello = { v: 1, app: "1.0.0", client: "desktop", pairing: paired.json.pairing };
  send(wire.Frame.handshake1, initiator.writeMessage1(Buffer.from(JSON.stringify(hello))));
  const { transport } = initiator.readMessage2(await receive(wire.Frame.handshake2));
  let requestId = 0;
  const request = async (method, route, body, headers = {}, raw = false) => {
    const id = ++requestId;
    const head = { t: "req", id, m: method, p: route, h: { "content-type": "application/json", ...headers } };
    const bytes = body === undefined ? Buffer.alloc(0) : Buffer.from(JSON.stringify(body));
    for (const part of wire.pieces(head, bytes, true)) {
      send(wire.Frame.transport, transport.seal(part));
    }
    const chunks = [];
    let status;
    for (;;) {
      const answer = wire.decodeMessage(transport.open(await receive(wire.Frame.transport)));
      assert.equal(answer.head.id, id);
      status ??= answer.head.s;
      chunks.push(answer.body);
      if (answer.head.end === true) {
        const text = Buffer.concat(chunks).toString();
        return raw ? { status, text } : { status, json: text ? JSON.parse(text) : undefined };
      }
    }
  };
  return { key, pairing: paired.json.pairing, ask: request, refusedSocket: (route) => request("GET", route, undefined, { upgrade: "websocket" }, true), post: (route, body, headers) => request("POST", route, body, headers) };
}
