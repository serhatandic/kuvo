// Kuvo Code's desktop app reaching every box of a setup (#742; PROTOCOL.md, "Paired phones",
// `kuvo/devices/pair`): the app on a member's computer asks the box beside it, which vouches for
// it to the setup's main computer; that one pairs it, and from its phones document every box lets
// the device in, as what it is. Removed from another device, it's out everywhere, for good.
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
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { relayStandIn } from "./relay-stand-in.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
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
const link = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const relay = await relayStandIn(link, { changed: () => {} });

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
const machineEnv = { PATH: process.env.PATH, HOME: support, KUVO_HOME: machineHome, TMPDIR: support };
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
write(path.join(machineHome, "run", "work-queue", "numbers.json"), { primary: macPub, leases: [[41, 41]], since: 2_000 });

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
  const setup = await ask("GET", "kuvo/link/setup");
  const result = spawnSync(process.execPath, [script, "setup"], {
    input: JSON.stringify({ relay: relay.url, name: "lab", mac: macPub, macSign: setup.json.macSign, docs: setup.json.docs, id: "lab.example" }),
    env: machineEnv,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  lab = JSON.parse(result.stdout.trim().split("\n").at(-1));
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [{ key: lab.key, sign: lab.sign, address: lab.address, name: "lab", id: "lab.example" }], moved: [] })).status, 200);
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
function client(hello) {
  const key = link.newKey();
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
      transport = opened.transport;
      return JSON.parse(opened.payload);
    },
    async request(method, p, body) {
      const id = ++request;
      const h = body === undefined ? {} : { "content-type": "application/json" };
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
const vouch = (item, id = `wqa_${crypto.randomBytes(16).toString("base64url")}`) => ({ id, area: "devices", method: "POST", item, body: { name: "Someone's laptop", kind: "desktop" }, at: Date.now() });

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

  // A phone paired with the main computer, as phones are: with its pairing link.
  const phone = client({ name: "My iPhone" });
  const token = new URL((await ask("POST", "kuvo/phones/pair")).json.link).searchParams.get("token");
  assert.equal((await phone.open(main, { pair: token })).kind, "mac");
  await eventually(() => (labLink()?.phones ?? []).some(each => each.key === phone.public));

  // (4) Only the app beside a box asks: a phone's session, the desktop app's own once it's in
  // (below), and the box's CLI are refused, on the main computer and on the member.
  const desk = client({ name: "Lab screen", client: "desktop" });
  const given = { key: desk.public, name: "Lab screen", kind: "desktop" };
  const onlyTheApp = { status: 403, json: { message: "Only Kuvo Code on this computer adds itself to your devices." } };
  assert.deepEqual(await phone.request("POST", "kuvo/devices/pair", given), onlyTheApp);
  assert.equal((await phone.open(member)).kind, "machine");
  assert.deepEqual(await phone.request("POST", "kuvo/devices/pair", given), onlyTheApp);
  assert.deepEqual(labRoute("POST", "kuvo/devices/pair", given), onlyTheApp);
  assert.deepEqual(await ask("POST", "kuvo/devices/pair", given, { fromMac: true }), onlyTheApp);
  // Not paired yet: turned away by both.
  assert.deepEqual([await desk.open(main), await desk.open(member)], [{ refused: true }, { refused: true }]);

  // (1) The app on the member's computer asks the box beside it, which vouches for it to the main
  // computer: paired there, with a pairing of its own, and nothing for you to paste.
  const paired = await labApp("POST", "kuvo/devices/pair", given);
  assert.equal(paired.status, 200, JSON.stringify(paired.json));
  assert.deepEqual(paired.json, { paired: true, pairing: paired.json.pairing });
  const ours = (await ask("GET", "kuvo/phones")).json.phones.find(each => each.key === desk.public);
  assert.deepEqual({ ...ours, added: null }, { key: desk.public, name: "Lab screen", added: null, pairing: paired.json.pairing, kind: "desktop", you: false });
  // The pairing link a phone could be shown on the main computer isn't touched by it.
  const shown = (await ask("POST", "kuvo/phones/pair")).json.link;
  // Asked again (the member may not have heard of it yet): the same pairing, no second device.
  assert.deepEqual((await labApp("POST", "kuvo/devices/pair", given)).json, paired.json);
  assert.equal((await ask("GET", "kuvo/phones")).json.phones.filter(each => each.key === desk.public).length, 1);
  assert.equal(new URL(shown).searchParams.get("token"), link.readLink().appPairing?.token, "the link Settings shows still waits");

  // It opens a session with the main computer, and, once the main computer's phones document
  // reached it, with the member: each answers it.
  assert.equal((await desk.open(main)).kind, "mac");
  assert.equal((await desk.request("GET", "kuvo/info")).status, 200);
  // (2) Listed as what it is, on both.
  const listed = async () => (await desk.request("GET", "kuvo/phones")).json.phones.map(each => [each.name, each.kind, each.you]).sort();
  assert.deepEqual(await listed(), [["Lab screen", "desktop", true], ["My iPhone", "phone", false]]);
  assert.deepEqual(await desk.request("POST", "kuvo/devices/pair", given), onlyTheApp);
  await eventually(() => (labLink()?.phones ?? []).some(each => each.key === desk.public && each.kind === "desktop"));
  const hello = await desk.open(member);
  assert.deepEqual([hello.kind, hello.name], ["machine", "lab"]);
  assert.equal((await desk.request("GET", "kuvo/info")).json.name, "lab");
  assert.deepEqual(await listed(), [["Lab screen", "desktop", true], ["My iPhone", "phone", false]]);
  // Asked once more, now that the member lets it in: yes at once, as it has it.
  assert.deepEqual((await labApp("POST", "kuvo/devices/pair", given)).json, paired.json);

  // (3) What it writes on the member reaches the agent as written; a phone's says it's a phone's.
  assert.equal((await desk.request("POST", "api/session/ses_far/prompt", { id: "msg_desk1", text: "Ship it" })).status, 200);
  assert.deepEqual(labEngine.prompted, ["Ship it"]);
  assert.equal((await phone.request("POST", "api/session/ses_far/prompt", { id: "msg_phone1", text: "Ship it" })).status, 200);
  assert.match(labEngine.prompted[1], /^Ship it\n\n<from-phone>\n/);

  // (5) A vouch from a box that isn't in the setup pairs no one: its mail isn't even opened.
  const stray = link.publicOf(link.newKey()).toString("base64url");
  mailToMac(link.newKey(), { workQueue: { act: vouch(stray) }, at: Date.now() });

  // (6) Removed from the other device (the phone, on the member): its session there ends, both
  // boxes turn it away, and its app is never paired again by asking.
  assert.deepEqual(await phone.request("POST", "kuvo/phones/remove", { phones: [{ key: desk.public }] }), { status: 200, json: {} });
  assert.deepEqual(await desk.open(member), { refused: true });
  await eventually(async () => !(await ask("GET", "kuvo/phones")).json.phones.some(each => each.key === desk.public));
  assert.deepEqual(await desk.open(main), { refused: true });
  const removed = { status: 409, json: { message: "This app was removed from your devices.", reason: "removed" } };
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", given), removed);
  assert.deepEqual(await ask("POST", "kuvo/devices/pair", given, { fromApp: true, served: true }), removed);
  assert.deepEqual([await desk.open(main), await desk.open(member)], [{ refused: true }, { refused: true }]);
  // The phone is still in, on both.
  assert.equal((await phone.open(main)).kind, "mac");
  assert.equal((await phone.open(member)).kind, "machine");

  // The app beside the main computer's own box is paired there at once, and the member lets it in too.
  const studio = client({ name: "Studio screen", client: "desktop" });
  const here = await ask("POST", "kuvo/devices/pair", { key: studio.public, name: "Studio screen", kind: "desktop" }, { fromApp: true, served: true });
  assert.equal(here.status, 200, JSON.stringify(here.json));
  assert.equal((await studio.open(main)).kind, "mac");
  await eventually(() => (labLink()?.phones ?? []).some(each => each.key === studio.public));
  assert.equal((await studio.open(member)).kind, "machine");

  // (5) A member that says it's shared with another setup vouches for no one: mailed with the
  // member's own key, as its box would, the main computer answers no and pairs nothing.
  const act = vouch(link.publicOf(link.newKey()).toString("base64url"));
  const labKey = Buffer.from(labLink().key, "base64url");
  mailToMac(labKey, { agents: { at: Date.now() + 600_000, list: [], shared: true }, workQueue: { act }, at: Date.now() });
  const answered = await eventually(() => {
    for (const each of relay.delivered) {
      if (!each.to.equals(member.address)) continue;
      try {
        const acted = JSON.parse(link.openOneWay(link.readFrame(each.payload).body, labKey, link.publicOf(macKey)).payload.toString("utf8")).workQueue?.acted;
        if (acted?.id === act.id) return acted;
      } catch {}
    }
    return null;
  });
  assert.deepEqual(answered, { id: act.id, status: 409, value: { message: "Only computers in Studio Mac's own setup can add your devices." } });
  const now = (await ask("GET", "kuvo/phones")).json.phones.map(each => each.key);
  assert.ok(!now.includes(act.item) && !now.includes(stray), "neither vouch paired anyone");
  // A Mac that lets no device in ("Let my devices connect" off) vouches for none either, were it
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
  // (7) And one that isn't yours says no way in: listed apart, with no address or relay.
  const theirs = (await ask("GET", "kuvo/computers")).json.computers.find(each => each.key === lab.key);
  assert.deepEqual([theirs.apart, theirs.address, theirs.relay], [true, undefined, undefined]);
});
