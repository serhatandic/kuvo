// A Mac's box in a home of its own has its link's settings and keys with no app telling it them
// (#713 piece 3; PROTOCOL.md, "This Mac's box: the relay"): the desktop app says none, and a Mac
// that never ran the retired Mac app has none kept. So the box takes its own: the relay, this Mac's
// name, your devices let in, and its keys from its helper. Then Kuvo Code on this Mac can pair a
// device and add a computer. What an app said before is kept as it was. Each box here is the real
// link in a process and a home of its own; its helper's host interface is a stand-in that keeps the
// keys in memory, and the relay is an address nothing answers at (nothing here goes online).
// Run with `node --test box/bridge-tests/own-link.test.js`.
import "./test-safety.js";
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { script } from "./box-stand-ins.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-own-link-")));
removeAtEnd(root);
const RELAY = "ws://127.0.0.1:1";

// This Mac's keys as its helper keeps them (made the first time they're asked for, in your Keychain).
const identity = { key: crypto.randomBytes(32).toString("base64url"), secret: crypto.randomBytes(32).toString("base64url"), sign: crypto.randomBytes(32).toString("base64url") };
const PASSWORD = "host-password-not-a-key";
const host = { asked: [] };
const server = http.createServer(async (request, response) => {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  const answer = (status, value) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(value));
  if (request.url === "/hello") {
    const { nonce } = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    return answer(200, { proof: crypto.createHmac("sha256", PASSWORD).update(`kuvo-host-hello\0${nonce}\0${host.url.replace("http://", "")}`).digest("hex") });
  }
  if (request.headers.authorization !== `Bearer ${PASSWORD}`) return answer(401, {});
  host.asked.push(request.url);
  return request.url === "/link/identity" ? answer(200, identity) : answer(404, {});
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
atEnd(() => { server.closeAllConnections(); server.close(); });
host.url = `http://127.0.0.1:${server.address().port}`;
const X_PKCS8 = Buffer.from("302e020100300506032b656e04220420", "hex");
const publicKey = crypto.createPublicKey(crypto.createPrivateKey({ key: Buffer.concat([X_PKCS8, Buffer.from(identity.key, "base64url")]), format: "der", type: "pkcs8" }))
  .export({ format: "der", type: "spki" }).subarray(-32).toString("base64url");

// One box, as it starts: its link begun (`runMacLink`, as `serve` begins it), then asked, as Kuvo
// Code on this Mac asks, for what a computer being added is given (its keys) and for a pairing link.
const asker = path.join(root, "ask.mjs");
fs.writeFileSync(asker, `
const link = await import(${JSON.stringify(script)});
link.runMacLink().catch(() => {});
const handle = link.engineHandler(() => null, { name: "this Mac" }, () => null);
async function ask(method, route) {
  let status = 0, text = "";
  for await (const piece of handle({ method, path: route, headers: {}, body: Buffer.alloc(0), fromApp: true })) {
    if (piece.head) status = piece.head.status;
    if (piece.body) text += piece.body.toString("utf8");
  }
  return { status, value: text ? JSON.parse(text) : {} };
}
const setup = await ask("GET", "kuvo/link/setup");
const pair = await ask("POST", "kuvo/phones/pair");
if (pair.status === 200) await ask("DELETE", "kuvo/phones/pair");
link.stopMacLink();
console.log(JSON.stringify({ setup, pair, config: link.macLink.config }));
process.exit(0);
`);
let count = 0;
/** A Mac's box in a home of its own (`kept`: what an app said of its link before, if any), asked. */
async function box({ kept = null, relay = RELAY } = {}) {
  const home = path.join(root, `home-${++count}`);
  const K = path.join(home, ".kuvo-code");
  fs.mkdirSync(path.join(K, "box"), { recursive: true });
  fs.mkdirSync(path.join(K, "run"), { recursive: true });
  fs.writeFileSync(path.join(K, "box", "kind"), "mac\n");
  if (kept) fs.writeFileSync(path.join(K, "run", "mac-link.json"), JSON.stringify(kept));
  const child = spawn(process.execPath, [asker], {
    env: { PATH: process.env.PATH, HOME: home, TMPDIR: process.env.TMPDIR ?? os.tmpdir(), KUVO_HOME: K, KUVO_KEYS_ON_HOST: "1", KUVO_HOST_URL: host.url, KUVO_HOST_PASSWORD: PASSWORD, ...(relay ? { KUVO_TEST_RELAY: relay } : {}) },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let out = "", err = "";
  child.stdout.on("data", chunk => { out += chunk; });
  child.stderr.on("data", chunk => { err += chunk; });
  const status = await new Promise(resolve => child.once("exit", resolve));
  assert.equal(status, 0, err);
  return { ...JSON.parse(out.trim().split("\n").pop()), K };
}

test("a Mac's box no app ever told its link's settings takes its own: it has its keys for a computer it adds, and pairs a device through the relay", { skip: process.platform !== "darwin" && "a Mac's own box" }, async () => {
  host.asked.length = 0;
  const { setup, pair, config, K } = await box();
  // What a computer being added is signed in with: this Mac's own keys, from its helper.
  assert.equal(setup.status, 200, JSON.stringify(setup));
  assert.equal(setup.value.mac, publicKey);
  assert.ok(host.asked.includes("/link/identity"), "asked of its helper, never made by the box");
  // A device's pairing link: through the relay, to this Mac by its key and its name.
  assert.equal(pair.status, 200, JSON.stringify(pair));
  const link = new URL(pair.value.link);
  assert.equal(link.searchParams.get("relay"), RELAY);
  assert.equal(link.searchParams.get("key"), publicKey);
  assert.ok(link.searchParams.get("name"), "this Mac's name");
  assert.deepEqual({ phones: config.phones, relay: config.relay }, { phones: true, relay: RELAY });
  // Its keys never in its files, and its own settings not kept as if an app had said them.
  assert.equal(fs.existsSync(path.join(K, "run", "mac-link.json")), false);
  const kept = fs.existsSync(path.join(K, "run", "link.json")) ? fs.readFileSync(path.join(K, "run", "link.json"), "utf8") : "";
  for (const secret of [identity.key, identity.secret, identity.sign]) assert.ok(!kept.includes(secret));
});

test("what an app said before stays as it was: its relay, and your devices kept out where you turned that off; still it has its keys for a computer it adds", { skip: process.platform !== "darwin" && "a Mac's own box" }, async () => {
  const { setup, pair, config } = await box({ kept: { phones: false, relay: "ws://127.0.0.1:2", name: "Studio", app: "0.8.0" } });
  assert.deepEqual({ phones: config.phones, relay: config.relay, name: config.name }, { phones: false, relay: "ws://127.0.0.1:2", name: "Studio" });
  assert.equal(pair.status, 409);
  assert.equal(pair.value.message, "“Let my devices connect” is off on this Mac.");
  assert.equal(setup.status, 200, "adding a computer asks for the keys whether or not anything else wants the link yet");
  assert.equal(setup.value.mac, publicKey);
});

test("a test's or a demo copy's box never takes the real relay by itself: with none given it has none, as before", { skip: process.platform !== "darwin" && "a Mac's own box" }, async () => {
  const { setup, pair, config } = await box({ relay: null });
  assert.equal(config, null);
  assert.equal(pair.status, 409);
  assert.equal(setup.status, 503);
});
