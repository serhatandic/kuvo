// A Mac that has only Kuvo Code's desktop app (#713 piece 8; PROTOCOL.md, "Machines", In a home
// of its own): its box lives in a home of its own, and no app answers for it (the desktop app is a
// client). The box answers browser asks, While Agents Work and engine restart itself. The real box
// script, in a home of the test's own laid out as a Mac's; its
// helper, `launchctl`, its keeper's `kuvo-start` and the relay are stand-ins.
//
// Run with `node --test box/bridge-tests/own-home-mac-routes.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { before, test } from "node:test";
import { relayStandIn } from "./relay-stand-in.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-own-home-routes-")));
removeAtEnd(home);
const K = path.join(home, ".kuvo-code");
const write = (file, text, mode = 0o600) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
};
const readJSON = file => JSON.parse(fs.readFileSync(file, "utf8"));
write(path.join(K, "box", "kind"), "mac\n");
fs.mkdirSync(path.join(K, "run"), { recursive: true });

// Its keeper's tool, noting what it's asked; Kuvo Helper, its launch agent, and `launchctl`, which
// says whether you switched the helper off in Login Items.
const asked = path.join(home, "kuvo-start-asked");
write(path.join(K, "bin", "kuvo-start"), `#!/bin/sh\necho "$*" >>'${asked}'\n`, 0o755);
const helperProgram = path.join(K, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper");
write(helperProgram, "#!/bin/sh\nexit 0\n", 0o755);
write(path.join(home, "Library", "LaunchAgents", "run.kuvo.code.machine.plist"), `<plist><array><string>${helperProgram}</string></array></plist>`);
const tools = path.join(home, "tools");
const switchedOff = path.join(home, "switched-off");
const launchctlRuns = path.join(home, "launchctl-runs");
write(path.join(tools, "launchctl"), `#!/bin/sh\necho "$1" >>'${launchctlRuns}'\ncase "$1" in print-disabled) if [ -f '${switchedOff}' ]; then echo '"run.kuvo.code.machine" => disabled'; else echo '"run.kuvo.code.machine" => enabled'; fi ;; esac\n`, 0o755);

const listen = async server => {
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  atEnd(() => { server.closeAllConnections(); server.close(); });
  return server.address().port;
};
const body = request => new Promise(resolve => {
  const chunks = [];
  request.on("data", chunk => chunks.push(chunk));
  request.on("end", () => resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}")));
});

// This Mac's keys, as its helper keeps them in your Keychain. It answers nothing else.
const helper = { asked: [], keys: null };
const HELPER_PASSWORD = crypto.randomBytes(16).toString("hex");
const helperHost = http.createServer(async (request, response) => {
  const given = await body(request);
  if (request.url === "/hello") {
    const proof = crypto.createHmac("sha256", HELPER_PASSWORD).update(`kuvo-host-hello\u0000${given.nonce}\u0000127.0.0.1:${helperHost.address().port}`).digest("hex");
    return response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ proof }));
  }
  if (request.headers.authorization !== `Bearer ${HELPER_PASSWORD}`) return response.writeHead(401).end();
  helper.asked.push(request.url);
  if (request.url === "/link/identity") return response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(helper.keys));
  response.writeHead(503, { "content-type": "application/json" }).end(JSON.stringify({ message: "Kuvo Code isn’t open on this Mac." }));
});
const helperPort = await listen(helperHost);
write(path.join(K, "run", "host.json"), JSON.stringify({ url: `http://127.0.0.1:${helperPort}`, password: HELPER_PASSWORD }));

Object.assign(process.env, { HOME: home, KUVO_HOME: K, TMPDIR: home, KUVO_TEST_UNINSTALL_TOOLS: tools });
delete process.env.KUVO_BOX;
const link = await import(LINK_BUNDLE.href);
const relay = await relayStandIn(link, { request: relayLeaseStandIn().request });
atEnd(() => relay.close());

const macKey = link.newKey();
const macSecret = crypto.randomBytes(32);
const macAddress = link.addressOf(macSecret);
helper.keys = { key: macKey.toString("base64url"), secret: macSecret.toString("base64url"), sign: link.newSigningSeed() };

// Kuvo Code's desktop app, a client of this box over its own server.
const handle = link.engineHandler(link.readEngineFile, { name: "Studio Mac" }, link.readBridgeFile);
async function ask(method, p, given) {
  const parts = [];
  for await (const part of handle({ method, path: p, headers: {}, body: given ? Buffer.from(JSON.stringify(given)) : Buffer.alloc(0), fromApp: true }, {}, undefined)) parts.push(part);
  const text = Buffer.concat(parts.filter(part => part.body).map(part => part.body)).toString();
  let json;
  try { json = JSON.parse(text); } catch {}
  return { status: parts[0].head.status, json };
}

async function untilConnected() {
  const stop = new AbortController();
  try {
    for await (const part of handle({ method: "GET", path: "kuvo/link/status/events", headers: {}, body: Buffer.alloc(0), fromApp: true }, {}, stop.signal)) {
      if (part.body && part.body.toString().split("\n").filter(Boolean).some(line => JSON.parse(line).state === "connected")) return;
    }
  } finally {
    stop.abort();
  }
}

// A phone, paired with this Mac, asking it things over the relay.
async function pairedPhone() {
  const pairing = await ask("POST", "kuvo/phones/pair");
  assert.equal(pairing.status, 200, JSON.stringify(pairing.json));
  await untilConnected();
  const token = new URL(pairing.json.link).searchParams.get("token");
  const key = link.newKey();
  const device = relay.device();
  const session = crypto.randomInt(1, 2 ** 31);
  const frames = async () => {
    const message = link.readRelay(await device.next());
    return message?.kind === "deliver" ? link.readFrame(message.payload) : { kind: "unreachable" };
  };
  const initiator = new link.IKInitiator(key, link.publicOf(macKey), link.PROLOGUE);
  device.send(macAddress, link.frame(link.Frame.handshake1, session, initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, name: "Test iPhone", app: "0", pair: token })))));
  const answer = await frames();
  assert.equal(answer.kind, link.Frame.handshake2, "paired");
  const { transport } = initiator.readMessage2(answer.body);
  return async (method, p, given) => {
    const sent = given ? Buffer.from(JSON.stringify(given)) : Buffer.alloc(0);
    for (const piece of link.pieces({ t: "req", id: crypto.randomInt(1, 2 ** 20), m: method, p, h: {} }, sent, true)) device.send(macAddress, link.frame(link.Frame.transport, session, transport.seal(piece)));
    const pieces = [];
    for (;;) {
      const next = await frames();
      if (next.kind !== link.Frame.transport) break;
      const message = link.decodeMessage(transport.open(next.body));
      pieces.push(message);
      if (message.head.end === true) break;
    }
    return { status: pieces[0].head.s, json: JSON.parse(Buffer.concat(pieces.map(piece => piece.body)).toString() || "null") };
  };
}

let phone;
before(async () => {
  // (No app tells this box its link's settings, as the old one did: they're its own, with the
  // relay a test's folder is given.)
  process.env.KUVO_TEST_RELAY = relay.url;
  link.runMacLink();
  // (Its keys from its helper first, as the link reads them before anything pairs.)
  assert.equal(await link.macKeys(), true);
  // Settle leadership before requesting a token so the pairing route can acquire its lease.
  link.noteSetupNow(Date.now(), { soon: 0 });
  phone = await pairedPhone();
});
atEnd(() => link.stopMacLink());

test("what its browser asks you: a phone reads it from the box, as from a machine's, with no app to ask", async () => {
  const asks = await phone("GET", "kuvo/asks");
  assert.deepEqual([asks.status, asks.json], [200, { asks: [] }]);
  // (One that isn't asked: the box's own answer, not that an app must be open.)
  assert.equal((await phone("POST", "kuvo/asks/none", { answer: "declined" })).status, 404);
  assert.deepEqual(helper.asked.filter(call => call !== "/link/identity"), [], "its helper was asked for nothing but its keys");
});

test("While Agents Work: a phone reads and changes it on the box, which keeps the Mac awake itself and says whether Kuvo starts at login", async () => {
  const darwin = process.platform === "darwin";
  const read = await phone("GET", "kuvo/mac");
  assert.equal(read.status, 200, JSON.stringify(read.json));
  // (Only a Mac has Login Items: anywhere else no helper keeps this box, which is `off`.)
  assert.deepEqual(read.json, { backgroundWork: darwin ? "on" : "off", openAtLogin: darwin, keepsAwake: true, awakeNow: false });

  // Keep This Mac Awake, turned off from the phone: the box's own setting, which it holds by.
  const off = await phone("PATCH", "kuvo/mac", { keepsAwake: false });
  assert.deepEqual([off.status, off.json.keepsAwake], [200, false]);
  assert.equal(readJSON(path.join(K, "box", "work.json")).keepsAwake, false);
  // (Only on a Mac does the box hold anything awake, so only there do its work settings name it.)
  if (darwin) assert.equal((await ask("GET", "kuvo/work/settings")).json.keepsAwake, false, "the desktop app's Settings reads the same");
  assert.equal((await phone("PATCH", "kuvo/mac", { keepsAwake: true })).json.keepsAwake, true);
  // What isn't a yes or a no changes nothing.
  assert.equal((await phone("PATCH", "kuvo/mac", { keepsAwake: "yes" })).status, 400);
  assert.equal((await phone("GET", "kuvo/mac")).json.keepsAwake, true);
  assert.equal((await phone("DELETE", "kuvo/mac")).status, 405);

  if (!darwin) {
    const refused = await phone("PATCH", "kuvo/mac", { backgroundWork: true });
    assert.equal(refused.status, 409);
    assert.match(refused.json.message, /Kuvo Helper isn’t set up/);
    return;
  }
  // On already: asking for it again is the same; turning it off is yours, on the Mac.
  assert.equal((await phone("PATCH", "kuvo/mac", { backgroundWork: true })).json.backgroundWork, "on");
  const refused = await phone("PATCH", "kuvo/mac", { backgroundWork: false, keepsAwake: false });
  assert.deepEqual([refused.status, refused.json], [400, { message: "Background work is turned off in Login Items & Extensions, on the Mac." }]);
  assert.equal((await phone("GET", "kuvo/mac")).json.keepsAwake, true, "refused, so nothing of it was kept");
  // However often a phone asks, Login Items was asked of macOS once: what it said stands a while,
  // and several asking at once share the one ask.
  const runs = () => fs.readFileSync(launchctlRuns, "utf8").split("\n").filter(Boolean).length;
  assert.equal(runs(), 1);
  link.forgetLoginItems();
  const many = await Promise.all(Array.from({ length: 6 }, () => ask("GET", "kuvo/mac")));
  assert.deepEqual(many.map(each => each.json.backgroundWork), Array(6).fill("on"));
  assert.equal(runs(), 2);
  // You switched Kuvo Helper off in Login Items: said once it's asked again. It waits for you
  // there, and a phone can't turn it on.
  fs.writeFileSync(switchedOff, "");
  assert.equal((await phone("GET", "kuvo/mac")).json.backgroundWork, "on", "what it said a moment ago stands");
  link.forgetLoginItems();
  try {
    assert.deepEqual((await phone("GET", "kuvo/mac")).json, { backgroundWork: "needsApproval", openAtLogin: false, keepsAwake: true, awakeNow: false });
    const waits = await phone("PATCH", "kuvo/mac", { openAtLogin: true });
    assert.deepEqual([waits.status, waits.json.backgroundWork], [200, "needsApproval"]);
  } finally {
    fs.rmSync(switchedOff);
    link.forgetLoginItems();
  }
});

test("Restart Engine from a phone: the box asks its own keeper, with no app", async () => {
  const restart = await phone("POST", "kuvo/engine/restart");
  assert.deepEqual([restart.status, restart.json], [202, {}]);
  await link.toolRun();
  assert.equal(fs.readFileSync(asked, "utf8"), "restart\n");
  // While it updates, it says so by this Mac's name, and nothing is asked of its keeper.
  // (An update as it's just begun: its lock made, its files being swapped.)
  fs.mkdirSync(path.join(K, "run", ".updating"));
  write(path.join(K, "run", "update.json"), JSON.stringify({ running: "installing" }));
  try {
    const updating = await phone("POST", "kuvo/engine/restart");
    assert.deepEqual([updating.status, updating.json], [409, { message: `${link.macLink.config.name} is updating. Its engine starts again once that's done.` }]);
  } finally {
    fs.rmSync(path.join(K, "run", "update.json"));
    fs.rmSync(path.join(K, "run", ".updating"), { recursive: true });
  }
  // (Start is a machine's alone: a Mac never had a Stop to undo.)
  assert.equal((await phone("POST", "kuvo/engine/start")).status, 404);
  assert.equal((await phone("POST", "kuvo/engine/stop")).status, 404);
  await link.toolRun();
  assert.equal(fs.readFileSync(asked, "utf8"), "restart\n", "nothing more was asked of its keeper");
});
