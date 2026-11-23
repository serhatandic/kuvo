// Devices paired through the setup primary, using real box and relay connections.
//
// Two real boxes over the relay: this Mac's (`serve`'s link, here, its app a stand-in), which
// leads the setup, and one of its machines, a member (`kuvo-link.js run`, a process of its own,
// set up as the app sets one up over SSH), with its server kept for the app beside it
// (`kuvo-link.js serve --kept`). The desktop app and a phone are devices on the relay, speaking
// the link's protocol with the box's own Noise. Only the relay, the app's host and the engines
// are stand-ins.
//
// Run with `node --test box/bridge-tests/devices-two-boxes.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { before, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { legacyBuild } from "./failure-bench-legacy.js";
import { relayStandIn } from "./relay-stand-in.js";
import { recordRelayLease, relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const script = fileURLToPath(LINK_BUNDLE);
const support = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-devices-two-")));
removeAtEnd(support);
const macHome = path.join(support, "mac");
const machineHome = path.join(support, "machine");
for (const home of [macHome, machineHome]) fs.mkdirSync(path.join(home, "run"), { recursive: true });
const write = (file, value) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, typeof value === "string" ? value : JSON.stringify(value), { mode: 0o600 });
};
const read = file => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; } };

// What a box does in another process, or a moment after it answers, looked at every few
// milliseconds until it's there, or until the test runs out of time (`signal`).
let signal;
function eventually(found) {
  return new Promise((resolve, reject) => {
    let looking = false;
    const look = async () => {
      if (looking) return;
      looking = true;
      try {
        const value = await found();
        if (!value) return;
        clearInterval(timer);
        resolve(value);
      } catch (error) {
        clearInterval(timer);
        reject(error);
      } finally {
        looking = false;
      }
    };
    const timer = setInterval(look, 50);
    signal?.addEventListener("abort", () => {
      clearInterval(timer);
      reject(new Error(`it never got there: ${found}`));
    }, { once: true });
    look();
  });
}
const listening = server => new Promise(resolve => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

// An engine with one conversation, keeping each message sent to it as the agent would read it.
async function engineWith(session) {
  const prompted = [];
  const server = http.createServer((request, response) => {
    let body = "";
    request.on("data", chunk => { body += chunk; });
    request.on("end", () => {
      const url = new URL(request.url, "http://x");
      const send = (status, data) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(status < 400 ? { data } : { message: data }));
      if (request.headers.authorization !== `Basic ${Buffer.from("opencode:pw").toString("base64")}`) return response.writeHead(401).end();
      if (url.pathname === "/api/session") return send(200, [{ id: session, title: "Work", directory: "/work", time: { created: 1, updated: 1 } }]);
      if (url.pathname === "/api/session/active") return send(200, {});
      if (url.pathname === "/api/model/default") return send(404, "None.");
      if (url.pathname === "/api/model") return send(200, [{ id: "words", providerID: "p", capabilities: { input: ["text"] } }]);
      if (url.pathname === `/api/session/${session}`) return send(200, { id: session, model: { id: "words", providerID: "p" } });
      if (url.pathname === `/api/session/${session}/prompt` && request.method === "POST") {
        prompted.push(JSON.parse(body).text);
        return send(200, {});
      }
      return send(200, []);
    });
  });
  const port = await listening(server);
  atEnd(() => { server.closeAllConnections(); server.close(); });
  return { port, password: "pw", prompted };
}

// The app's host, as it answers this Mac's box: its keys, from your Keychain.
const app = { keys: null };
const appHost = http.createServer((request, response) => {
  const chunks = [];
  request.on("data", chunk => chunks.push(chunk));
  request.on("end", () => {
    const given = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
    if (request.url === "/hello") {
      const proof = crypto.createHmac("sha256", "host-password").update(`kuvo-host-hello\u0000${given.nonce}\u0000127.0.0.1:${appHost.address().port}`).digest("hex");
      return response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ proof }));
    }
    if (request.headers.authorization !== "Bearer host-password") return response.writeHead(401).end();
    if (request.url === "/link/identity") return response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(app.keys));
    if (request.url === "/link/route") return response.writeHead(404, { "content-type": "application/json", "x-kuvo-answer": "1" }).end("{}");
    response.writeHead(404).end();
  });
});
await listening(appHost);
atEnd(() => appHost.close());

Object.assign(process.env, {
  KUVO_HOME: macHome,
  KUVO_BOTS: path.join(support, "bots"),
  KUVO_WAKES: path.join(support, "wakes"),
  KUVO_BOX: "mac",
  KUVO_ENGINE_CONFIG: path.join(support, "opencode"),
  KUVO_KEYS: path.join(support, "keys"),
  KUVO_HOST_URL: `http://127.0.0.1:${appHost.address().port}`,
  KUVO_HOST_PASSWORD: "host-password",
});
const link = await import(LINK_BUNDLE.href);
const leases = relayLeaseStandIn({ transferStartsNow: true });
const relay = await relayStandIn(link, { request: leases.request, changed: () => {} });

function renewStudioLease(peers) {
  const setup = link.readLink().setup;
  assert.equal(setup.role, "primary");
  const roster = Object.fromEntries([[setup.primary, setup.sign], ...peers.map(peer => [peer.key, peer.sign])]);
  const grant = leases.seed(setup, roster, 500);
  recordRelayLease(path.join(macHome, "run"), setup, { relay: grant });
}

const macEngine = await engineWith("ses_lead");
const labEngine = await engineWith("ses_far");
write(path.join(macHome, "run", "engine.json"), { port: macEngine.port, password: "pw" });
write(path.join(machineHome, "run", "engine.json"), { port: labEngine.port, password: "pw" });

// This Mac's keys, as your Keychain keeps them.
const macKey = link.newKey();
const macSecret = crypto.randomBytes(32);
const macAddress = link.addressOf(macSecret);
const macPub = link.publicOf(macKey).toString("base64url");
app.keys = { key: macKey.toString("base64url"), secret: macSecret.toString("base64url"), sign: link.newSigningSeed() };

link.macLink.hold = link.macNoticeHold();
const handle = link.engineHandler(link.readEngineFile, { name: "Mac" }, link.readBridgeFile, () => {}, link.macLink.hold);
// What this Mac's box answers its own app (`from`: how the request reached it).
async function ask(method, p, given, from = { fromApp: true }) {
  const parts = [];
  for await (const part of handle({ method, path: p, headers: {}, body: given ? Buffer.from(JSON.stringify(given)) : Buffer.alloc(0), ...from }, {}, undefined)) parts.push(part);
  const text = Buffer.concat(parts.filter(part => part.body).map(part => part.body)).toString();
  let json;
  try { json = JSON.parse(text); } catch {}
  return { status: parts[0].head.status, json };
}
const machineEnv = { PATH: process.env.PATH, HOME: machineHome, KUVO_HOME: machineHome, TMPDIR: support };
const labLink = () => read(path.join(machineHome, "run", "link.json"));
const labPid = () => { try { return Number(fs.readFileSync(path.join(machineHome, "run", "link.pid"), "utf8")); } catch { return 0; } };
// What the machine's box answers its CLI (`box route`: a Mac over SSH, or an agent's command there).
const labRoute = (method, p, body) => {
  const result = spawnSync(process.execPath, [script, "box", "route"], { input: JSON.stringify({ method, path: p, body }), env: machineEnv, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  const answer = JSON.parse(result.stdout.trim().split("\n").at(-1));
  return { status: answer.status, json: JSON.parse(answer.body) };
};

// This Mac numbers the Work Queue on its own, so it leads a setup (PROTOCOL.md, "Your setup").
write(path.join(macHome, "run", "work-queue", "numbers.json"), { primary: "self", next: 70, leases: [], since: 1_000 });

let lab = null;
// The machine's server kept for the app beside it, and how that app asks it: over HTTP on this
// computer, with the server's password, as Kuvo Code attached to a box in place (`x-kuvo-from: mac`).
let kept = null;
async function labApp(method, p, body) {
  const record = read(path.join(machineHome, "run", "serve.json"));
  const response = await fetch(`${kept.url}/${p}`, {
    method,
    headers: { authorization: `Basic ${Buffer.from(`opencode:${record.password}`).toString("base64")}`, "x-kuvo-from": "mac", "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return { status: response.status, json: await response.json() };
}

before(async () => {
  assert.notEqual((await ask("PUT", "kuvo/link/config", { phones: true, machines: true, relay: relay.url, name: "Studio Mac", app: "0.9.0" })).json?.state, "failed");
  // The SSH install already has its identity when the main publishes the membership.
  const key = link.newKey(), secret = crypto.randomBytes(32), seed = link.newSigningSeed();
  const peer = { key: link.publicOf(key).toString("base64url"), sign: link.signingKeyOf(seed),
    address: link.addressOf(secret).toString("base64url"), name: "lab", id: "lab.example", kind: "machine" };
  write(path.join(machineHome, "run", "link.json"), {
    key: key.toString("base64url"), secret: secret.toString("base64url"), sign: seed, phones: [],
    mac: macPub, macs: [macPub],
  });
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [peer], moved: [] })).status, 200);
  link.noteSetupNow(Date.now(), { soon: 0 });
  renewStudioLease([peer]);
  const setup = await ask("GET", "kuvo/link/setup");
  assert.equal(setup.status, 200);
  const result = spawnSync(process.execPath, [script, "setup"], {
    input: JSON.stringify({ ...setup.json, relay: relay.url, name: "lab", id: "lab.example" }),
    env: machineEnv,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  lab = JSON.parse(result.stdout.trim().split("\n").at(-1));
  assert.equal(lab.key, peer.key, result.stdout);
  // A member can hold a number lease after it has enrolled in the setup that grants it.
  write(path.join(machineHome, "run", "work-queue", "numbers.json"), { primary: macPub, leases: [[41, 41]], since: 2_000 });
  link.runMacLink();
  link.runMacPeers();
  // The app there wants the machine's server: started as its keeper starts it.
  write(path.join(machineHome, "run", "serve.needed"), "");
  const child = spawn(process.execPath, [script, "serve", "--kept"], { env: machineEnv, stdio: ["ignore", "pipe", "inherit"] });
  atEnd(() => new Promise(resolve => {
    if (child.exitCode !== null) return resolve();
    child.once("exit", resolve);
    child.kill("SIGTERM");
  }));
  kept = await new Promise((resolve, reject) => {
    let out = "";
    child.stdout.on("data", chunk => {
      out += chunk;
      if (out.includes("\n")) resolve(JSON.parse(out.split("\n")[0]));
    });
    child.once("exit", code => reject(new Error(`the machine's server ended (${code})`)));
  });
});

atEnd(async () => {
  link.stopMacPeers();
  link.stopMacLink();
  const pid = labPid();
  if (pid > 0) {
    try { process.kill(pid, "SIGTERM"); } catch {}
    for (let tries = 0; tries < 200; tries++) {
      try { process.kill(pid, 0); } catch { break; }
      await new Promise(resolve => setTimeout(resolve, 25));
    }
  }
  await relay.close();
});

// A device on the relay (the desktop app, or a phone), opening sessions with a box (`to`: its
// address and key) and asking it things, as KuvoLink's client and the desktop app's do.
function client(hello, key = link.newKey()) {
  const device = relay.device();
  let session = crypto.randomInt(1, 2 ** 30);
  let transport = null, at = null, request = 0;
  // What the box it's talking to sends it (another's, a moment late from a session before, isn't).
  const frames = async () => {
    for (;;) {
      const message = link.readRelay(await device.next(20_000));
      if (message && at && message.address && !message.address.equals(at.address)) continue;
      if (message?.kind !== "deliver") return { kind: "unreachable" };
      const frame = link.readFrame(message.payload);
      if (frame && frame.session !== session) continue;
      return frame;
    }
  };
  return {
    public: link.publicOf(key).toString("base64url"),
    // Opens a session: what the box says back, or that it turned this device away.
    async open(to, more = {}) {
      at = to;
      session += 1;
      const initiator = new link.IKInitiator(key, to.key, link.PROLOGUE);
      device.send(to.address, link.frame(link.Frame.handshake1, session, initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, app: "1.0.0", ...hello, ...more })))));
      const answer = await frames();
      if (answer.kind !== link.Frame.handshake2) return { refused: true };
      const opened = initiator.readMessage2(answer.body);
      const response = JSON.parse(opened.payload);
      if (response.waiting) return { waiting: response.waiting };
      transport = opened.transport;
      return response;
    },
    async request(method, p, body, headers = {}) {
      const id = ++request;
      const h = { ...(body === undefined ? {} : { "content-type": "application/json" }), ...headers };
      const bytes = body === undefined ? Buffer.alloc(0) : Buffer.from(JSON.stringify(body));
      for (const piece of link.pieces({ t: "req", id, m: method, p, h }, bytes, true)) device.send(at.address, link.frame(link.Frame.transport, session, transport.seal(piece)));
      const pieces = [];
      for (;;) {
        const next = await frames();
        if (next.kind !== link.Frame.transport) return { closed: true };
        const message = link.decodeMessage(transport.open(next.body));
        if (message.head.id !== id) continue;
        pieces.push(message);
        if (message.head.end === true) break;
      }
      const text = Buffer.concat(pieces.map(piece => piece.body)).toString();
      let json;
      try { json = JSON.parse(text); } catch {}
      return { status: pieces[0].head.s, json };
    },
  };
}

// Mail to this Mac's box sealed with `key`, as a box holding that key mails another (PROTOCOL.md, "Mail").
const mailToMac = (key, payload) => relay.device().send(macAddress, link.frame(link.Frame.mail, 0, link.sealOneWay(Buffer.from(JSON.stringify(payload)), key, link.publicOf(macKey)).message));
const pairingAct = (item, id = `wqa_${crypto.randomBytes(16).toString("base64url")}`) => ({ id, area: "devices", method: "POST", item, body: { name: "Someone's laptop", kind: "desktop" }, at: Date.now() });
// The member has installed the main computer's removal of this device.
const memberHasRemoval = key => {
  const there = labLink();
  return !!there && (there.removed ?? []).some(each => each.key === key) && !(there.phones ?? []).some(each => each.key === key);
};
const memberHeardRemoved = key => eventually(() => memberHasRemoval(key));
const openOnMember = (client, member) => eventually(async () => {
  const hello = await client.open(member);
  return hello.kind === "machine" ? hello : null;
});
// What this Mac's box answered the ask `id` of the box holding `key`, at `address` on the relay.

test("the desktop app on a member's computer is paired through its own box and reaches every box of the setup, until another device removes it", { timeout: 180_000 }, async t => {
  signal = t.signal;
  const main = { key: link.publicOf(macKey), address: macAddress };
  const member = { key: Buffer.from(lab.key, "base64url"), address: Buffer.from(lab.address, "base64url") };
  // The setup: this Mac leads it, the machine is a member and has heard what this Mac's box takes.
  await eventually(async () => (await ask("GET", "kuvo/setup")).json?.you?.role === "primary");
  await eventually(() => labLink()?.setup?.role === "member");
  await eventually(() => (read(path.join(machineHome, "run", "peer-agents.json"))?.[macPub]?.features ?? []).includes("devices.pair"));
  await eventually(() => relay.has(member.address) && relay.has(macAddress));

  // (7) Each box says where the other's is reached, for a client that connects to each itself.
  const where = computers => Object.fromEntries(computers.map(each => [each.here ? "here" : each.name, [each.address, each.relay]]));
  assert.deepEqual(where((await ask("GET", "kuvo/computers")).json.computers), { here: [macAddress.toString("base64url"), relay.url], lab: [lab.address, relay.url] });
  assert.deepEqual(where(labRoute("GET", "kuvo/computers").json.computers), { here: [lab.address, relay.url], "Studio Mac": [macAddress.toString("base64url"), relay.url] });

  const manager = client({ name: "Studio desktop", client: "desktop" });
  assert.equal((await ask("POST", "kuvo/devices/pair", { key: manager.public, name: "Studio desktop", kind: "desktop" }, { served: true, fromApp: true })).status, 200);
  assert.equal((await manager.open(main)).kind, "mac");

  // (4) Only the app beside a box asks. A remote desktop and the box's CLI are refused.
  const desk = client({ name: "Lab screen", client: "desktop" });
  const given = { key: desk.public, name: "Lab screen", kind: "desktop" };
  const onlyTheApp = { status: 403, json: { message: "Only Kuvo Code on this computer adds itself to your devices." } };
  assert.deepEqual(await manager.request("POST", "kuvo/devices/pair", given), onlyTheApp);
  await openOnMember(manager, member);
  assert.deepEqual(await manager.request("POST", "kuvo/devices/pair", given), onlyTheApp);
  assert.deepEqual(labRoute("POST", "kuvo/devices/pair", given), onlyTheApp);
  assert.deepEqual(await ask("POST", "kuvo/devices/pair", given, { fromMac: true }), onlyTheApp);
  // Not paired yet: turned away by both.
  assert.deepEqual([await desk.open(main), await desk.open(member)], [{ refused: true }, { refused: true }]);

  // (1) The app on the member's computer asks the box beside it, which forwards its request to the main
  // computer: paired there, with a pairing of its own, and nothing for you to paste.
  const paired = await labApp("POST", "kuvo/devices/pair", given);
  assert.equal(paired.status, 200, JSON.stringify(paired.json));
  assert.equal(paired.json.paired, true);
  assert.ok(paired.json.pairing);
  assert.equal(paired.json.status, "committed");
  const proof = link.readDoc(paired.json.proof);
  assert.deepEqual([proof.kind, proof.body.kind, proof.body.operation, proof.body.id], [
    "setup-shared", "membershipAndAccess", paired.json.operation, link.readLink().setup.id,
  ]);
  const ours = (await ask("GET", "kuvo/phones")).json.phones.find(each => each.key === desk.public);
  assert.deepEqual({ ...ours, added: null }, { key: desk.public, name: "Lab screen", added: null, pairing: paired.json.pairing, kind: "desktop", you: false });
  // Asked again (the member may not have heard of it yet): the same pairing, no second device.
  const repeated = await labApp("POST", "kuvo/devices/pair", given);
  if (repeated.status === 503) assert.equal(repeated.json.message, "Checking this app with your other computers…");
  else assert.deepEqual(repeated, { status: 200, json: { paired: true, pairing: paired.json.pairing } });
  assert.equal((await ask("GET", "kuvo/phones")).json.phones.filter(each => each.key === desk.public).length, 1);

  // It opens a session with the main computer, and, once the main computer's phones document
  // reached it, with the member: each answers it.
  assert.equal((await desk.open(main)).kind, "mac");
  assert.equal((await desk.request("GET", "kuvo/info")).status, 200);
  // (2) Listed as what it is, on both.
  const listed = async () => (await desk.request("GET", "kuvo/phones")).json.phones.map(each => [each.name, each.kind, each.you]).sort();
  assert.deepEqual(await listed(), [["Lab screen", "desktop", true], ["Studio desktop", "desktop", false]]);
  assert.deepEqual(await desk.request("POST", "kuvo/devices/pair", given), onlyTheApp);
  await eventually(() => (labLink()?.phones ?? []).some(each => each.key === desk.public && each.kind === "desktop"));
  const hello = await openOnMember(desk, member);
  assert.deepEqual([hello.kind, hello.name], ["machine", "lab"]);
  assert.equal((await desk.request("GET", "kuvo/info")).json.name, "lab");
  assert.deepEqual(await listed(), [["Lab screen", "desktop", true], ["Studio desktop", "desktop", false]]);
  // Asked once more, now that the member lets it in: yes at once, as it has it.
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", given), { status: 200, json: { paired: true, pairing: paired.json.pairing } });

  // (3) What it writes on the member reaches the agent as written.
  assert.equal((await desk.request("POST", "api/session/ses_far/prompt", { id: "msg_desk1", text: "Ship it" })).status, 200);
  assert.deepEqual(labEngine.prompted, ["Ship it"]);

  // (5) A pairing request from a box that isn't in the setup pairs no one: its mail isn't even opened.
  const stray = link.publicOf(link.newKey()).toString("base64url");
  mailToMac(link.newKey(), { workQueue: { act: pairingAct(stray) }, at: Date.now() });

  const twin = client({ name: "Lab twin", client: "desktop" });
  const twinGiven = { key: twin.public, name: "Lab twin", kind: "desktop" };
  assert.equal((await labApp("POST", "kuvo/devices/pair", twinGiven)).status, 200);
  await eventually(() => (labLink()?.phones ?? []).some(each => each.key === twin.public));
  assert.deepEqual([(await twin.open(main)).kind, (await openOnMember(twin, member)).kind], ["mac", "machine"]);

  assert.equal((await manager.open(main)).kind, "mac");
  assert.equal((await manager.request("POST", "kuvo/phones/remove", { phones: [{ key: desk.public, pairing: paired.json.pairing }] })).status, 200);
  await memberHeardRemoved(desk.public);
  assert.deepEqual(await desk.request("GET", "kuvo/info"), { closed: true });
  assert.deepEqual([await desk.open(main), await desk.open(member)], [{ refused: true }, { refused: true }]);
  const removed = { status: 409, json: { message: "This app was removed from your devices.", reason: "removed" } };
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", given), removed);
  assert.deepEqual(await ask("POST", "kuvo/devices/pair", given, { fromApp: true, served: true }), removed);
  assert.equal((await twin.request("GET", "kuvo/info")).status, 200, "another device paired through the same member keeps its session");
  assert.equal((await twin.open(main)).kind, "mac");

  const staleRemoval = await labApp("POST", "kuvo/phones/remove", { phones: [{ key: twin.public, pairing: "older" }] });
  assert.equal(staleRemoval.status, 200, JSON.stringify(staleRemoval.json));
  assert.equal(staleRemoval.json.status, "committed");
  assert.equal(link.readDoc(staleRemoval.json.proof).body.operation, staleRemoval.json.operation);
  assert.equal((await twin.open(main)).kind, "mac", "an old pairing ID cannot remove the current pairing");
  const dropTwin = await labApp("POST", "kuvo/phones/remove", { phones: [{ key: twin.public }] });
  assert.equal(dropTwin.status, 200, JSON.stringify(dropTwin.json));
  assert.equal(dropTwin.json.status, "committed");
  assert.equal(link.readDoc(dropTwin.json.proof).body.operation, dropTwin.json.operation);
  await memberHeardRemoved(twin.public);
  assert.deepEqual([await twin.open(main), await twin.open(member)], [{ refused: true }, { refused: true }]);
  assert.equal((await manager.open(main)).kind, "mac");
  await openOnMember(manager, member);
  const now = (await ask("GET", "kuvo/phones")).json.phones.map(each => each.key);
  assert.ok(!now.includes(stray), "forwarding from outside the setup paired no one");
  // A Mac that lets no device in ("Let my devices connect" off) forwards no pairing requests either, were it
  // a member: its own switch counts, not only its main computer's.
  const wasConfig = link.macLink.config;
  const wasSetup = link.readLink().setup;
  link.macLink.config = { ...wasConfig, phones: false };
  link.updateLink(state => ({ ...state, setup: { ...state.setup, role: "member", primary: lab.key } }));
  const off = await ask("POST", "kuvo/devices/pair", { key: link.publicOf(link.newKey()).toString("base64url"), name: "Laptop", kind: "desktop" }, { served: true, fromApp: true });
  link.updateLink(state => ({ ...state, setup: wasSetup }));
  link.macLink.config = wasConfig;
  assert.equal(off.status, 409);
  assert.match(off.json.message, /^“Let my devices connect” is off on /);
});

test("relay refuses mixed-case future and conflicting route versions before sending a prompt", { timeout: 18_000 }, async t => {
  signal = t.signal;
  const desktop = client({ name: "Version check", client: "desktop" });
  assert.equal((await ask("POST", "kuvo/devices/pair", { key: desktop.public, name: "Version check", kind: "desktop" }, { served: true, fromApp: true })).status, 200);
  try {
    await eventually(() => relay.has(macAddress));
    assert.equal((await desktop.open({ key: link.publicOf(macKey), address: macAddress })).kind, "mac");
    const prompted = macEngine.prompted.length;
    for (const headers of [
      { "X-Kuvo-Route-Version": "2" },
      { "x-kuvo-route-version": "1", "X-Kuvo-Route-Version": "2" },
      { "X-Kuvo-Route-Version": "2", "x-kuvo-route-version": "1" },
    ]) {
      const refused = await desktop.request("POST", "api/session/ses_lead/prompt", { text: "Unsupported prompt" }, headers);
      assert.equal(refused.status, 400);
      assert.equal(refused.json.code, "unsupportedFormatVersion");
      assert.equal(macEngine.prompted.length, prompted);
    }
  } finally {
    await ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.public }] });
  }
});

test("a device revocation reaches a member even while the main's inbound sessions are switched off", { timeout: 25_000 }, async t => {
  signal = t.signal;
  await eventually(() => labLink()?.setup?.role === "member" && relay.has(macAddress));
  const member = { key: Buffer.from(lab.key, "base64url"), address: Buffer.from(lab.address, "base64url") };
  const desktop = client({ name: "Cellar desktop", client: "desktop" });
  const added = await ask("POST", "kuvo/devices/pair", { key: desktop.public, name: "Cellar desktop", kind: "desktop" }, { served: true, fromApp: true });
  assert.equal(added.status, 200, JSON.stringify(added.json));
  await openOnMember(desktop, member);
  assert.equal((await desktop.request("GET", "kuvo/computers")).status, 200);

  const config = link.macLink.config;
  const appSaid = link.macLink.appSaid;
  const release = relay.pause(macAddress, member.address);
  t.after(release);
  try {
    assert.equal((await ask("PUT", "kuvo/link/config", { ...config, phones: false })).status, 200);
    link.macLink.appSaid = undefined;
    assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.public, pairing: added.json.pairing }] })).status, 200);
    assert.equal(memberHasRemoval(desktop.public), false, "the member missed the change");

    const reconnect = relay.disconnect(macAddress);
    t.after(reconnect);
    await eventually(() => !relay.has(macAddress));
    reconnect();
    release();
    await eventually(() => relay.has(macAddress) && memberHasRemoval(desktop.public));
    assert.deepEqual(await desktop.request("GET", "kuvo/computers"), { closed: true });
    assert.deepEqual(await desktop.open(member), { refused: true });
    assert.equal(link.macLink.config.phones, false);
  } finally {
    await ask("PUT", "kuvo/link/config", config);
    link.macLink.appSaid = appSaid;
  }
});

test("the reconnect reply carries the current signed membership as well as phones", { timeout: 15_000 }, async t => {
  signal = t.signal;
  await eventually(() => labLink()?.key);
  const replies = [];
  const id = crypto.randomBytes(16).toString("base64url");
  const key = Buffer.from(labLink().key, "base64url");
  const origin = link.signingKeyOf(app.keys.sign);
  link.peerMail({ key: lab.key, address: lab.address, kind: "machine", name: "lab" }, { phoneSync: { id, origin } }, {
    welcomed: true,
    send: bytes => {
      const frame = link.readFrame(bytes.subarray(17));
      replies.push(JSON.parse(link.openOneWay(frame.body, key, link.publicOf(macKey)).payload.toString()));
    },
  });
  const docs = replies.flatMap(each => each.docs?.give ?? []).map(link.readDoc);
  const peers = docs.find(each => each?.kind === "peers");
  assert.equal(peers?.origin, origin);
  assert.deepEqual(peers.body.boxes.map(each => each.key).sort(), [macPub, ...link.readLink().macPeers.map(each => each.key)].sort());
  const phones = docs.find(each => each?.kind === "phones");
  assert.equal(phones?.origin, origin);
  assert.deepEqual(replies.at(-1).phoneSync, { id, list: `${phones.n}:${phones.h}`, peers: `${peers.n}:${peers.h}`, ack: true });
});

test("failure bench: an older peer's signed copies cannot establish current access without the main", { timeout: 100_000 }, async t => {
  signal = t.signal;
  const previous = legacyBuild();
  assert.ok(!previous.equals(fs.readFileSync(script)), "the two builds differ");
  const member = { key: Buffer.from(lab.key, "base64url"), address: Buffer.from(lab.address, "base64url") };
  const extraHome = path.join(support, "workshop");
  const extraFile = path.join(extraHome, "run", "link.json");
  const extraKey = link.newKey(), extraSign = link.newSigningSeed(), extraSecret = crypto.randomBytes(32);
  const extraAddress = link.addressOf(extraSecret);
  const extra = { key: link.publicOf(extraKey).toString("base64url"), sign: link.signingKeyOf(extraSign),
    address: extraAddress.toString("base64url"), name: "Workshop", id: "workshop.example" };
  const before = link.readLink().shared.membershipAndAccess.value.boxes;
  const docKey = `${link.signingKeyOf(app.keys.sign)} phones`;
  const countIn = state => link.readDoc(state?.docs?.[docKey])?.n ?? 0;
  async function reached(label, found, ms = 15_000) {
    const until = Date.now() + ms;
    const result = await eventually(async () => (await found()) ? "ready" : Date.now() >= until && "timed out");
    assert.equal(result, "ready", label);
  }
  async function opens(device, label) {
    await reached(label, async () => (await device.open(member)).kind === "machine", 9_000);
  }
  const restart = new Set();
  const disconnect = address => {
    const resume = relay.disconnect(address);
    restart.add(resume);
    return async () => {
      resume();
      restart.delete(resume);
      if (address.equals(macAddress)) link.macLink.wake?.();
      await reached("the box returned to the relay", () => relay.has(address), 30_000);
    };
  };
  await reached("Lab follows the main Mac's setup", () => labLink()?.setup?.role === "member" && labLink().setup.primary === macPub);
  await reached("Studio leads the setup", async () => (await ask("GET", "kuvo/setup")).json?.you?.role === "primary");
  function commitRoster(boxes) {
    renewStudioLease(boxes);
    let saved;
    link.updateLink(current => {
      const membership = current.shared.membershipAndAccess.value;
      saved = link.stageShared(current, "membershipAndAccess", { ...membership, boxes });
      return saved ? current : null;
    }, { durable: true });
    assert.ok(saved, "the main committed the roster");
  }
  commitRoster([...before, { ...extra, kind: "machine" }]);
  await reached("Lab received Workshop in the signed roster", () => labLink()?.shared?.membershipAndAccess?.value.boxes.some(each => each.key === extra.key));
  const seed = labLink();
  write(extraFile, { key: extraKey.toString("base64url"), secret: extraSecret.toString("base64url"), sign: extraSign,
    name: extra.name, relay: relay.url, mac: macPub, macs: [macPub], macSigns: seed.macSigns,
    docs: seed.docs, setup: seed.setup, sharedSetup: seed.sharedSetup, shared: seed.shared, leadership: seed.leadership });
  const ledger = path.join(extraHome, "run", "work-queue", "numbers.json");
  write(ledger, { primary: macPub, leases: [[121, 160]], since: 2_000 });
  const workshopScript = path.join(extraHome, "bin", "kuvo-link.js");
  write(workshopScript, previous.toString("utf8"));
  const workshopEnv = { ...machineEnv, HOME: extraHome, KUVO_HOME: extraHome };
  const startWorkshop = build => spawn(process.execPath, [build, "run"], { env: workshopEnv, stdio: "ignore" });
  let workshop = startWorkshop(workshopScript);
  async function stopWorkshop() {
    if (workshop.exitCode !== null || workshop.signalCode !== null) return;
    const stopped = new Promise(resolve => workshop.once("close", resolve));
    workshop.kill("SIGTERM");
    await stopped;
  }
  async function pair(name) {
    const device = client({ name, client: "desktop" });
    assert.equal((await ask("POST", "kuvo/devices/pair", { key: device.public, name, kind: "desktop" }, { served: true, fromApp: true })).status, 200);
    await reached(`Lab installed ${name}'s pairing`, () => labLink()?.phones?.some(each => each.key === device.public));
    return device;
  }
  async function remove(device) {
    const old = countIn(labLink());
    assert.equal((await ask("POST", "kuvo/phones/remove", { phones: [{ key: device.public }] })).status, 200);
    await reached("Workshop installed the later removal", () => countIn(read(extraFile)) > old && read(extraFile)?.removed?.some(each => each.key === device.public));
  }
  try {
    await eventually(() => relay.has(extraAddress));
    const yours = await pair("Current desktop");
    const first = await pair("Revoked before reconnect");
    const second = await pair("Revoked while isolated");
    let resumeMember = disconnect(member.address);
    await eventually(() => !relay.has(member.address));
    const promptedBefore = labEngine.prompted.length;
    const text = "Keep working while off the relay";
    assert.equal((await labApp("POST", "api/session/ses_far/prompt", { id: "msg_offline", text })).status, 200);
    assert.deepEqual(labEngine.prompted.slice(promptedBefore), [text]);
    await remove(first);
    let resumeMain = disconnect(macAddress);
    await eventually(() => !relay.has(macAddress));
    await resumeMember();
    // A reconnecting member waits for the main to confirm its current grants.
    assert.equal(labLink().setup?.primary, macPub);
    assert.equal((await first.open(member)).waiting, "Checking this app with your other computers…");
    assert.equal((await yours.open(member)).waiting, "Checking this app with your other computers…");
    assert.equal(relay.has(macAddress), false);

    resumeMember = disconnect(member.address);
    await eventually(() => !relay.has(member.address));
    await resumeMain();
    await resumeMember();
    await reached("Lab installed the first removal from the main", () => memberHasRemoval(first.public));
    assert.deepEqual(await first.open(member), { refused: true });
    resumeMember = disconnect(member.address);
    await eventually(() => !relay.has(member.address));
    await remove(second);
    // Change the Workshop's stored format while the main box stays live. The previous process
    // has really used this ledger. Cut the new process before the atomic replacement, exercise
    // the previous reader on rollback, then retry the same upgrade and start the new link.
    await stopWorkshop();
    await eventually(() => !relay.has(extraAddress));
    const before = fs.readFileSync(ledger, "utf8");
    const fields = JSON.parse(before);
    assert.equal(fields.formatVersion, undefined);
    assert.deepEqual(fields.leases, [[121, 160]]);
    const boot = path.join(extraHome, "upgrade.mjs");
    write(boot, `
import fs from "node:fs";
const rename = fs.renameSync;
if (process.argv[2] === "cut") fs.renameSync = (from, to) => {
  if (to === ${JSON.stringify(ledger)}) process.kill(process.pid, "SIGKILL");
  rename(from, to);
};
const link = await import(${JSON.stringify(LINK_BUNDLE.href)});
link.askToSend();
`);
    const cut = spawnSync(process.execPath, [boot, "cut"], { env: workshopEnv, encoding: "utf8", timeout: 10_000 });
    assert.equal(cut.signal, "SIGKILL", cut.stderr);
    assert.equal(fs.readFileSync(ledger, "utf8"), before);
    const readWithPreviousBuild = () => {
      const previousRead = spawnSync(process.execPath, ["--input-type=module", "-e",
        `import { readNumbers } from ${JSON.stringify(pathToFileURL(workshopScript).href)}; console.log(JSON.stringify(readNumbers()));`,
      ], { env: workshopEnv, encoding: "utf8", timeout: 10_000 });
      assert.equal(previousRead.status, 0, previousRead.stderr);
      return JSON.parse(previousRead.stdout);
    };
    const rollback = readWithPreviousBuild();
    assert.deepEqual(rollback.leases, fields.leases);
    // The lock of the interrupted process is reclaimed; it cannot leave a partially upgraded ledger.
    const resumed = spawnSync(process.execPath, [boot], { env: workshopEnv, encoding: "utf8", timeout: 10_000 });
    assert.equal(resumed.status, 0, resumed.stderr);
    assert.deepEqual(read(ledger), { ...fields, formatVersion: 1 });
    assert.deepEqual(readWithPreviousBuild(), rollback);
    workshop = startWorkshop(script);
    await reached("the upgraded Workshop returned to the relay", () => relay.has(extraAddress));
    resumeMain = disconnect(macAddress);
    const resumeExtra = disconnect(extraAddress);
    await eventually(() => !relay.has(macAddress) && !relay.has(extraAddress));
    await resumeMember();
    // In isolation the member still has the old grant and waits to confirm its freshness.
    assert.equal(labLink().setup?.primary, macPub);
    assert.equal((await yours.open(member)).waiting, "Checking this app with your other computers…");
    assert.equal((await second.open(member)).waiting, "Checking this app with your other computers…");
    assert.equal(memberHasRemoval(second.public), false);

    // Even the upgraded Workshop cannot confirm current access on behalf of the main.
    await resumeExtra();
    assert.equal((await second.open(member)).waiting, "Checking this app with your other computers…");
    assert.equal(relay.has(macAddress), false);
    await resumeMain();
    await reached("Lab installed the second removal from the main", () => memberHasRemoval(second.public));
    assert.deepEqual(await second.open(member), { refused: true });
    await opens(yours, "Lab accepted its current desktop after the main confirmed the grant");
    assert.equal((await yours.request("GET", "kuvo/info")).status, 200);
  } finally {
    for (const resume of restart) resume();
    await stopWorkshop();
    commitRoster(before);
    link.macLink.wake?.();
  }
});

test("local uninstall refuses to clear a primary while a member holds an open session", { timeout: 25_000 }, async t => {
  signal = t.signal;
  renewStudioLease(link.readLink().shared.membershipAndAccess.value.boxes);
  const member = { key: Buffer.from(lab.key, "base64url"), address: Buffer.from(lab.address, "base64url") };
  const key = link.newKey();
  const desktop = client({ name: "Studio uninstall desktop", client: "desktop" }, key);
  assert.equal((await ask("POST", "kuvo/devices/pair", { key: desktop.public, name: "Studio uninstall desktop", kind: "desktop" }, { served: true, fromApp: true })).status, 200);
  await eventually(() => labLink()?.phones?.some(each => each.key === desktop.public));
  await openOnMember(desktop, member);
  assert.equal((await desktop.request("GET", "kuvo/computers")).status, 200);
  const server = link.serveBox("local-uninstall", handle);
  const port = await listening(server);
  write(path.join(macHome, "run", "serve.json"), { port, password: "local-uninstall" });
  write(path.join(macHome, "box", "kind"), "mac\n");
  const identity = path.join(macHome, "desktop", "identity");
  const identityBefore = `${key.toString("base64url")}.${crypto.randomBytes(32).toString("base64url")}`;
  write(identity, identityBefore);
  try {
    const result = await new Promise(resolve => {
      const child = spawn(process.execPath, [script, "uninstall"], { env: { ...machineEnv, KUVO_HOME: macHome }, stdio: ["ignore", "pipe", "pipe"] });
      let stdout = "", stderr = "";
      child.stdout.on("data", bytes => { stdout += bytes; });
      child.stderr.on("data", bytes => { stderr += bytes; });
      child.on("close", status => resolve({ status, stdout, stderr }));
    });
    assert.equal(result.status, 1, `${result.stdout}\n${result.stderr}`);
    assert.deepEqual(JSON.parse(result.stdout), { result: "refused", message: "Choose another main computer first." });
    assert.equal((await desktop.request("GET", "kuvo/computers")).status, 200);
    assert.equal(labLink()?.removed?.some(each => each.key === desktop.public) ?? false, false);
    assert.equal(fs.readFileSync(identity, "utf8"), identityBefore);
  } finally {
    server.closeAllConnections();
    server.close();
    fs.rmSync(identity, { force: true });
    fs.rmSync(path.join(macHome, "run", "off"), { force: true });
    fs.rmSync(path.join(macHome, "run", "serve.json"), { force: true });
  }
});

test("local uninstall revokes its desktop before stopping the box, including a session open to its primary", { timeout: 60_000 }, async t => {
  signal = t.signal;
  renewStudioLease(link.readLink().shared.membershipAndAccess.value.boxes);
  const main = { key: link.publicOf(macKey), address: macAddress };
  const key = link.newKey();
  const desktop = client({ name: "Lab desktop", client: "desktop" }, key);
  assert.equal((await labApp("POST", "kuvo/devices/pair", { key: desktop.public, name: "Lab desktop", kind: "desktop" })).status, 200);
  const identity = path.join(machineHome, "desktop", "identity");
  write(identity, `${key.toString("base64url")}.${crypto.randomBytes(32).toString("base64url")}`);
  assert.equal((await desktop.open(main)).kind, "mac");
  assert.equal((await desktop.request("GET", "kuvo/computers")).status, 200);
  const result = await new Promise(resolve => {
    const child = spawn(process.execPath, [script, "uninstall"], { env: machineEnv, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    child.stdout.on("data", bytes => { stdout += bytes; });
    child.stderr.resume();
    child.on("close", status => resolve({ status, stdout }));
  });
  assert.equal(result.status, 0, result.stdout);
  assert.equal(JSON.parse(result.stdout).result, "uninstalled");
  assert.deepEqual(await desktop.request("GET", "kuvo/computers"), { closed: true });
  assert.deepEqual(await desktop.open(main), { refused: true });
  assert.equal(fs.existsSync(identity), false);
});
