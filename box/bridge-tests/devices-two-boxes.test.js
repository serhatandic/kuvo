// Kuvo Code's desktop app reaching every box of a setup (#742; PROTOCOL.md, "Paired phones",
// `kuvo/devices/pair`): the app on a member's computer asks the box beside it, which vouches for
// it to the setup's main computer; that one pairs it, and from its phones document every box lets
// the device in, as what it is. Removed from another device, it's out everywhere, for good, and
// the computer that vouched for it adds no other until you allow it again on the main computer.
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
const actedFor = (id, key, address) => eventually(() => {
  for (const each of relay.delivered) {
    if (!each.to.equals(address)) continue;
    try {
      const acted = JSON.parse(link.openOneWay(link.readFrame(each.payload).body, key, link.publicOf(macKey)).payload.toString("utf8")).workQueue?.acted;
      if (acted?.id === id) return acted;
    } catch {}
  }
  return null;
});

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
  const repeated = await labApp("POST", "kuvo/devices/pair", given);
  if (repeated.status === 503) assert.equal(repeated.json.message, "Checking this app with your other computers…");
  else assert.deepEqual(repeated, paired);
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
  const hello = await openOnMember(desk, member);
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

  // Someone at the member's computer has its box vouch for a second key meanwhile: in, on both.
  const twin = client({ name: "Lab twin", client: "desktop" });
  const twinGiven = { key: twin.public, name: "Lab twin", kind: "desktop" };
  assert.equal((await labApp("POST", "kuvo/devices/pair", twinGiven)).status, 200);
  await eventually(() => (labLink()?.phones ?? []).some(each => each.key === twin.public));
  assert.deepEqual([(await twin.open(main)).kind, (await twin.open(member)).kind], ["mac", "machine"]);

  // (6) Removed from the other device (the phone, on the member): its session there ends, both
  // boxes turn it away, and its app is never paired again by asking.
  assert.deepEqual(await phone.request("POST", "kuvo/phones/remove", { phones: [{ key: desk.public }] }), { status: 200, json: {} });
  assert.deepEqual(await desk.open(member), { refused: true });
  await eventually(async () => !(await ask("GET", "kuvo/phones")).json.phones.some(each => each.key === desk.public));
  assert.deepEqual(await desk.open(main), { refused: true });
  const removed = { status: 409, json: { message: "This app was removed from your devices.", reason: "removed" } };
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", given), removed);
  // The other key that computer vouched for went with it, removed as any device is: out on both
  // boxes, and never paired again by asking, so nobody there is left with a way back in.
  assert.ok(!(await ask("GET", "kuvo/phones")).json.phones.some(each => each.key === twin.public));
  assert.deepEqual(await twin.open(main), { refused: true });
  await eventually(() => !(labLink()?.phones ?? []).some(each => each.key === twin.public));
  assert.deepEqual(await twin.open(member), { refused: true });
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", twinGiven), removed);
  assert.deepEqual(await ask("POST", "kuvo/devices/pair", given, { fromApp: true, served: true }), removed);
  assert.deepEqual([await desk.open(main), await desk.open(member)], [{ refused: true }, { refused: true }]);
  // The phone is still in, on both.
  assert.equal((await phone.open(main)).kind, "mac");
  assert.equal((await phone.open(member)).kind, "machine");

  // (8) The computer that vouched for it adds no other now (the removal came in the member's
  // phones document): whoever sits there can't have its app make a new key and come back.
  const allow = `kuvo/computers/${lab.key}/allow-devices`;
  const labKey = Buffer.from(labLink().key, "base64url");
  const second = client({ name: "Lab screen", client: "desktop" });
  const again = { key: second.public, name: "Second screen", kind: "desktop" };
  const keptOut = removed => ({ status: 409, json: { message: `${removed} on lab was removed from your devices, so lab can't add Kuvo Code's app by itself now. Allow it again from Kuvo Code on Studio Mac or another computer's app: Computers, lab.`, reason: "blocked" } });
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", again), keptOut("Lab screen"));
  assert.deepEqual(await second.open(main), { refused: true });
  // The main computer says so on that computer, with what allows it again, in its own words; the
  // member says nothing of it.
  const labOn = async () => (await ask("GET", "kuvo/computers")).json.computers.find(each => each.key === lab.key);
  const saidOf = async () => { const { vouching, actions } = await labOn(); return [vouching, actions.find(each => each.id === "allow-devices")]; };
  const offered = { id: "allow-devices", label: "Allow Its App Again", confirm: { title: "Let lab add Kuvo Code's app again?", message: "Kuvo Code on lab can add itself to your devices again, and reach every computer of yours. What you removed stays removed.", label: "Allow" } };
  assert.deepEqual(await saidOf(), [{ blocked: true, words: "Lab screen was removed from your devices. lab can't add Kuvo Code's app again until you allow it." }, offered]);
  assert.deepEqual((await labOn()).actions.at(-1), offered, "after what else can be done with it");
  for (const each of labRoute("GET", "kuvo/computers").json.computers) assert.deepEqual([each.vouching, each.actions.some(action => action.id === "allow-devices")], [undefined, false]);
  // Another computer of the setup still adds its app: only the one that vouched is kept from it.
  const atticKey = link.newKey(), atticSecret = crypto.randomBytes(32);
  const attic = { pub: link.publicOf(atticKey).toString("base64url"), address: link.addressOf(atticSecret), on: relay.device(atticSecret) };
  const mailAs = (from, key, payload) => from.send(macAddress, link.frame(link.Frame.mail, 0, link.sealOneWay(Buffer.from(JSON.stringify(payload)), key, link.publicOf(macKey)).message));
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [
    { key: lab.key, sign: lab.sign, address: lab.address, name: "lab", id: "lab.example" },
    { key: attic.pub, sign: link.signingKeyOf(link.newSigningSeed()), address: attic.address.toString("base64url"), name: "attic", id: "attic.example" },
  ], moved: [] })).status, 200);
  const loftKey = link.newKey();
  const loft = client({ name: "Someone's laptop", client: "desktop" }, loftKey);
  const fromAttic = vouch(loft.public);
  mailAs(attic.on, atticKey, { workQueue: { act: fromAttic }, at: Date.now() });
  const atticPaired = await actedFor(fromAttic.id, atticKey, attic.address);
  assert.deepEqual(atticPaired, { id: fromAttic.id, status: 200, value: { paired: true, pairing: atticPaired.value?.pairing } });
  assert.equal((await ask("GET", "kuvo/computers")).json.computers.find(each => each.key === attic.pub).vouching, undefined);

  // The computer kept from it can't allow itself: its own box says where to, the box's CLI (an
  // agent's command) is refused there and on the main computer, and nothing its box mails the main
  // computer is taken as that (sent before a vouch of its own, which is still refused).
  assert.deepEqual(await labApp("POST", allow), { status: 409, json: { message: "Allow it from Studio Mac.", primary: macPub } });
  const notFromHere = { status: 403, json: { message: "Only Kuvo Code on this computer, or one of your devices, allows a computer to add your devices again." } };
  assert.deepEqual(labRoute("POST", allow), notFromHere);
  assert.deepEqual(await ask("POST", allow, undefined, { fromMac: true }), notFromHere);
  assert.equal((await ask("GET", allow, undefined, { fromApp: true, served: true })).status, 405);
  assert.equal((await ask("POST", `kuvo/computers/${stray}/allow-devices`, undefined, { fromApp: true, served: true })).status, 404);
  const asLab = relay.device();
  const forged = id => ({ id: `wqa_${crypto.randomBytes(16).toString("base64url")}`, item: lab.key, at: Date.now(), ...id });
  for (const act of [forged({ area: "computers", method: "POST", action: "allow-devices" }), forged({ area: "devices", method: "POST", action: "allow-devices" }), forged({ area: "computers", method: "POST" }), forged({ method: "POST", action: "allow-devices" })]) {
    mailAs(asLab, labKey, { workQueue: { act }, at: Date.now() });
  }
  const stillOut = vouch(link.publicOf(link.newKey()).toString("base64url"));
  mailAs(asLab, labKey, { workQueue: { act: stillOut }, at: Date.now() });
  assert.deepEqual(await actedFor(stillOut.id, labKey, member.address), { id: stillOut.id, status: 409, value: keptOut("Lab screen").json });
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", again), keptOut("Lab screen"));

  // Allowed again from one of your devices, on the main computer: said at once, the new key is
  // paired on the member's word and reaches both; the key that was removed stays removed.
  assert.equal((await phone.open(main)).kind, "mac");
  assert.deepEqual(await phone.request("POST", allow), { status: 200, json: {} });
  assert.deepEqual(await saidOf(), [undefined, undefined]);
  assert.deepEqual(await phone.request("POST", allow), { status: 200, json: {} }, "the same when it isn't kept from it");
  const back = await labApp("POST", "kuvo/devices/pair", again);
  assert.deepEqual(back, { status: 200, json: { paired: true, pairing: back.json.pairing } });
  assert.equal((await second.open(main)).kind, "mac");
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", given), removed);
  // A device that computer vouched for never allows that computer: not now, and not while it's
  // kept from it (were one left paired then), so nobody there allows themselves.
  assert.deepEqual(await second.request("POST", allow), notFromHere);
  link.updateLink(state => ({ ...state, vouchBlocked: [{ key: lab.key, device: "Lab screen", at: Date.now() }] }));
  assert.deepEqual(await second.request("POST", allow), notFromHere);
  assert.equal((await saidOf())[0]?.blocked, true, "still kept from it");
  assert.deepEqual(await phone.request("POST", allow), { status: 200, json: {} });
  // It vouches for one more.
  const third = client({ name: "Third screen", client: "desktop" });
  const more = { key: third.public, name: "Third screen", kind: "desktop" };
  assert.equal((await labApp("POST", "kuvo/devices/pair", more)).status, 200);
  assert.equal((await third.open(main)).kind, "mac");

  // Removed on the main computer itself, by a paired device: kept from it again, at once. A
  // removal of a pairing it doesn't have now takes nothing off, and keeps no computer from it.
  assert.deepEqual(await phone.request("POST", "kuvo/phones/remove", { phones: [{ key: second.public, pairing: "an-older-one" }] }), { status: 200, json: {} });
  assert.deepEqual(await saidOf(), [undefined, undefined]);
  assert.deepEqual(await phone.request("POST", "kuvo/phones/remove", { phones: [{ key: second.public }] }), { status: 200, json: {} });
  assert.equal((await saidOf())[0]?.words, "Second screen was removed from your devices. lab can't add Kuvo Code's app again until you allow it.");
  // The other one it vouched for is gone with it, for good; a new key is kept out.
  assert.deepEqual([await second.open(main), await third.open(main)], [{ refused: true }, { refused: true }]);
  await memberHeardRemoved(third.public);
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", more), removed);
  const fresh = (name, key = link.publicOf(link.newKey()).toString("base64url")) => ({ key, name, kind: "desktop" });
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", fresh("Another")), keptOut("Second screen"));
  // Allowed by Kuvo Code's app beside the main computer's box; then two it vouches for, and one
  // removed by that app: the same, both gone.
  assert.deepEqual(await ask("POST", allow, undefined, { fromApp: true, served: true }), { status: 200, json: {} });
  const fourth = client({ name: "Fourth screen", client: "desktop" }), fifth = client({ name: "Fifth screen", client: "desktop" });
  for (const each of [fourth, fifth]) assert.equal((await labApp("POST", "kuvo/devices/pair", fresh(each === fourth ? "Fourth screen" : "Fifth screen", each.public))).status, 200);
  assert.deepEqual(await ask("POST", "kuvo/phones/remove", { phones: [{ key: fourth.public }] }), { status: 200, json: {} });
  assert.deepEqual([await fourth.open(main), await fifth.open(main)], [{ refused: true }, { refused: true }]);
  await memberHeardRemoved(fifth.public);
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", fresh("Fifth screen", fifth.public)), removed);
  assert.deepEqual(await labApp("POST", "kuvo/devices/pair", fresh("Another")), keptOut("Fourth screen"));
  assert.deepEqual(link.readLink().vouchBlocked.map(each => [each.key, each.device]), [[lab.key, "Fourth screen"]], "one entry a computer");
  // What another computer vouched for is untouched through all of it, and still let in; so is the phone.
  assert.equal((await loft.open(main)).kind, "mac");
  assert.equal((await ask("GET", "kuvo/phones")).json.phones.find(each => each.key === loft.public)?.name, "Someone's laptop");
  assert.equal((await phone.open(main)).kind, "mac");

  // The app beside the main computer's own box is paired there at once, and the member lets it in too.
  const studio = client({ name: "Studio screen", client: "desktop" });
  const here = await ask("POST", "kuvo/devices/pair", { key: studio.public, name: "Studio screen", kind: "desktop" }, { fromApp: true, served: true });
  assert.equal(here.status, 200, JSON.stringify(here.json));
  assert.equal((await studio.open(main)).kind, "mac");
  await eventually(() => (labLink()?.phones ?? []).some(each => each.key === studio.public));
  assert.equal((await studio.open(member)).kind, "machine");

  // (#753) Removing a computer takes Kuvo Code's app on it off your devices: the one attic's box
  // added reaches the main computer and the member until then, and neither after, though it still
  // has its key. Every other device stays.
  await eventually(() => (labLink()?.phones ?? []).some(each => each.key === loft.public));
  const loftOnMember = client({ name: "Someone's laptop", client: "desktop" }, loftKey);
  assert.equal((await loftOnMember.open(member)).kind, "machine");
  assert.equal((await loftOnMember.request("GET", "kuvo/computers")).status, 200);
  const phoneDocKey = `${link.signingKeyOf(app.keys.sign)} phones`;
  const older = labLink().docs[phoneDocKey];
  assert.ok(link.readDoc(older).body.phones.some(each => each.key === loft.public));
  assert.equal((await loft.open(main)).kind, "mac");
  assert.equal((await loft.request("GET", "kuvo/computers")).status, 200);
  const voucherPhone = client({ name: "Phone linked from the attic app" });
  const phoneInvite = new URL((await loft.request("POST", "kuvo/phones/pair")).json.link).searchParams.get("token");
  assert.equal((await voucherPhone.open(main, { pair: phoneInvite })).kind, "mac");
  await eventually(() => labLink()?.phones?.some(each => each.key === voucherPhone.public && each.by === attic.pub));
  const invitation = await loft.request("POST", "kuvo/phones/pair");
  assert.equal(invitation.status, 200);
  const pendingToken = new URL(invitation.json.link).searchParams.get("token");
  assert.equal((await studio.open(main)).kind, "mac");
  assert.equal((await studio.request("GET", "kuvo/computers")).status, 200);
  // (The Mac app before 1.0 isn't attached: it said its link's config as this began.)
  link.macLink.appSaid = undefined;
  const loftSessions = () => [...link.macLink.host.sessions.values()].filter(session => session.phone.key.toString("base64url") === loft.public);
  assert.ok(loftSessions().length > 0);
  const reconnect = relay.disconnect(member.address);
  await eventually(() => !relay.has(member.address));
  const list = path.join(macHome, "machines.json");
  write(list, [{ destination: "attic.example" }]);
  const rename = fs.renameSync;
  fs.renameSync = (from, to) => {
    if (to === list) throw new Error("could not save the list");
    return rename(from, to);
  };
  try {
    assert.equal((await ask("DELETE", `kuvo/computers/${attic.pub}`)).status, 503);
  } finally {
    fs.renameSync = rename;
  }
  assert.ok(labLink().phones.some(each => each.key === loft.public), "the offline member has only the older list");
  assert.equal(loftSessions().length, 0, "sessions are closed before the removal answers");
  assert.deepEqual(await loft.request("GET", "kuvo/computers"), { closed: true }, "its open session is gone too");
  assert.equal((await studio.request("GET", "kuvo/computers")).status, 200, "the current app keeps its open session");
  assert.deepEqual(await loft.open(main), { refused: true });
  assert.ok(!(await ask("GET", "kuvo/phones")).json.phones.some(each => each.key === loft.public));
  const resume = relay.pause(macAddress, member.address);
  try {
    const mailStart = relay.delivered.length;
    reconnect();
    await eventually(() => relay.has(member.address));
    assert.ok(labLink().phones.some(each => each.key === loft.public), "the member only has its cached list");
    const message = "Checking this app with your other computers…";
    assert.deepEqual(await loft.open(member), { waiting: message });
    assert.deepEqual(await voucherPhone.open(member), { waiting: message });
    assert.equal((await phone.open(member)).kind, "machine", "independent phones don't wait");
    assert.deepEqual(await labApp("POST", "kuvo/devices/pair", { key: studio.public, name: "Studio screen", kind: "desktop" }), {
      status: 503, json: { reason: "pairing-wait", message },
    });

    await t.test("a phones-only reply leaves the desktop waiting for signed membership", async () => {
      const asks = () => relay.delivered.slice(mailStart).flatMap(({ from, to, payload }) => {
        if (!from.equals(member.address) || !to.equals(macAddress)) return [];
        const frame = link.readFrame(payload);
        if (frame?.kind !== link.Frame.mail) return [];
        return [JSON.parse(link.openOneWay(frame.body, macKey, member.key).payload.toString())];
      });
      const id = await eventually(() => asks().find(each => each.phoneSync?.origin === link.ownSign(link.readLink()))?.phoneSync.id);
      const oldMac = relay.device();
      const reply = payload => oldMac.send(member.address, link.frame(link.Frame.mail, 0,
        link.sealOneWay(Buffer.from(JSON.stringify(payload)), macKey, member.key).message));
      try {
        const current = link.readLink();
        const phones = link.ownDoc(current, "phones", link.ownPhones(current)).envelope;
        const { n, h, origin } = link.readDoc(phones);
        reply({ docs: { give: [phones] } });
        reply({ phoneSync: { id, list: `${n}:${h}`, ack: true } });
        await memberHeardRemoved(loft.public);
        const oldPeers = link.readDoc(labLink().docs[`${origin} peers`]);
        assert.ok(oldPeers.body.boxes.some(each => each.key === attic.pub), "the member still trusts the removed box");
        assert.deepEqual(await studio.open(member), { waiting: message });

        const docs = await eventually(() => asks().find(each => each.docs?.reply === true && !each.docs.have.some(([key, kind]) => key === origin && kind === "peers"))?.docs);
        assert.ok(docs.have.some(([key, kind]) => key === origin && kind === "phones"));
        // The older Mac answers the ordinary document request over the same authenticated mail.
        const sender = { key: lab.key, address: lab.address, sign: lab.sign, kind: "machine" };
        await link.receiveDocs(sender, docs, { send: bytes => oldMac.send(member.address, bytes.subarray(17)) });
        await openOnMember(studio, member);
        assert.ok(!link.readDoc(labLink().docs[`${origin} peers`]).body.boxes.some(each => each.key === attic.pub));
      } finally {
        oldMac.leave();
      }
    });
  } finally {
    resume();
  }
  await memberHeardRemoved(loft.public);
  await memberHeardRemoved(voucherPhone.public);
  assert.deepEqual(await loftOnMember.request("GET", "kuvo/computers"), { closed: true }, "the offline member lost its old session");
  assert.deepEqual(await loft.open(member), { refused: true });
  assert.deepEqual(await voucherPhone.open(member), { refused: true });
  const synced = labLink().docs[phoneDocKey];
  assert.ok(link.readDoc(synced).n > link.readDoc(older).n);
  const delayed = relay.device();
  delayed.send(member.address, link.frame(link.Frame.mail, 0, link.sealOneWay(Buffer.from(JSON.stringify({ docs: { give: [older] }, at: Date.now() })), macKey, member.key).message));
  assert.deepEqual(await loft.open(member), { refused: true }, "the older signed list cannot restore the pairing");
  assert.deepEqual(labLink().docs[phoneDocKey], synced);
  assert.deepEqual(await loft.open(main, { pair: pendingToken }), { refused: true }, "its old invitation cannot restore the pairing either");
  assert.deepEqual([(await phone.open(main)).kind, (await studio.open(main)).kind, (await openOnMember(studio, member)).kind, (await phone.open(member)).kind], ["mac", "mac", "machine", "machine"]);
  await eventually(async () => !(await labApp("GET", "kuvo/computers")).json.computers.some(each => each.key === attic.pub));
  assert.deepEqual(await labApp("DELETE", `kuvo/computers/${attic.pub}`), { status: 200, json: {} });
  assert.deepEqual(read(list), []);

  const now = (await ask("GET", "kuvo/phones")).json.phones.map(each => each.key);
  assert.ok(!now.includes(stray), "the stray vouch paired no one");
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
});

test("failure bench: a removal reaches a member while the primary's inbound device switch stays off", { timeout: 18_000 }, async t => {
  signal = t.signal;
  await eventually(async () => (await ask("GET", "kuvo/setup")).json?.you?.role === "primary");
  await eventually(() => labLink()?.setup?.role === "member" && relay.has(macAddress));
  const member = { key: Buffer.from(lab.key, "base64url"), address: Buffer.from(lab.address, "base64url") };
  const other = { key: link.publicOf(link.newKey()).toString("base64url"), sign: link.signingKeyOf(link.newSigningSeed()), name: "Cellar",
    id: "cellar.example", address: link.addressOf(crypto.randomBytes(32)).toString("base64url") };
  const peers = link.readLink().macPeers;
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [...peers, other] })).status, 200);
  const desktop = client({ name: "Cellar desktop", client: "desktop" });
  assert.equal((await link.deviceAct(other, vouch(desktop.public))).status, 200);
  await eventually(() => labLink()?.phones?.some(each => each.key === desktop.public));
  await openOnMember(desktop, member);
  assert.equal((await desktop.request("GET", "kuvo/computers")).status, 200);
  const phonesDoc = packet => {
    if (!packet.from.equals(macAddress) || !packet.to.equals(member.address)) return;
    const frame = link.readFrame(packet.payload);
    if (frame?.kind !== link.Frame.mail) return;
    const opened = link.openOneWay(frame.body, Buffer.from(labLink().key, "base64url"), link.publicOf(macKey));
    const mail = JSON.parse(opened.payload.toString());
    return mail.docs?.give?.map(link.readDoc).find(doc => doc?.kind === "phones");
  };
  const hasGrant = doc => doc?.body.phones?.some(each => each.key === desktop.public);
  const earlier = relay.delivered.findLast(packet => hasGrant(phonesDoc(packet)));
  assert.ok(earlier, "the original signed grant reached the member");
  const held = [];
  const release = relay.intercept(macAddress, member.address, packet => {
    const doc = phonesDoc(packet);
    if (!doc?.body.removed?.some(each => each.key === desktop.public)) return false;
    held.push(packet);
    return true;
  });
  const config = link.macLink.config;
  const appSaid = link.macLink.appSaid;
  assert.equal((await ask("PUT", "kuvo/link/config", { ...config, phones: false })).status, 200);
  link.macLink.appSaid = undefined;
  try {
    assert.equal((await ask("DELETE", `kuvo/computers/${other.key}`)).status, 200);
    const dropped = await eventually(() => held[0]);
    assert.equal(memberHasRemoval(desktop.public), false, "the relay dropped the first revocation");
    // The next digest exchange repairs the loss. Hold the repair so the test chooses exactly
    // when it comes in, then release it before the stale copy of the grant.
    link.docsChanged(link.macLink.host, { tell: true });
    const revoked = await eventually(() => held.find(packet => !packet.payload.equals(dropped.payload)));
    assert.equal(phonesDoc(revoked).n, phonesDoc(dropped).n, "the repair carries the same signed revocation");
    assert.equal((await desktop.request("GET", "kuvo/computers")).status, 200);
    // A newer signed list arrives before a delayed copy of the grant. Both lists can be duplicated
    // by the network. Previously accepted grants must never overwrite a newer tombstone.
    relay.replay(revoked);
    relay.replay(revoked);
    relay.replay(earlier);
    relay.replay(earlier);
    const until = Date.now() + 4_000;
    assert.equal(await eventually(() => memberHasRemoval(desktop.public) ? "removed" : Date.now() >= until && "stale"), "removed",
      "the member installed the removal while the primary switch was off");
    assert.deepEqual(await desktop.request("GET", "kuvo/computers"), { closed: true });
    assert.deepEqual(await desktop.open(member), { refused: true });
    assert.equal(link.macLink.config.phones, false);
  } finally {
    release();
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

test("failure bench: a reconnecting member catches up from another box while the primary is away", { timeout: 100_000 }, async t => {
  signal = t.signal;
  const member = { key: Buffer.from(lab.key, "base64url"), address: Buffer.from(lab.address, "base64url") };
  const extraHome = path.join(support, "workshop");
  const extraFile = path.join(extraHome, "run", "link.json");
  const extraKey = link.newKey(), extraSign = link.newSigningSeed(), extraSecret = crypto.randomBytes(32);
  const extraAddress = link.addressOf(extraSecret);
  const extra = { key: link.publicOf(extraKey).toString("base64url"), sign: link.signingKeyOf(extraSign),
    address: extraAddress.toString("base64url"), name: "Workshop", id: "workshop.example" };
  const before = link.readLink().macPeers;
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
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [...before, extra] })).status, 200);
  await eventually(() => link.readDoc(labLink()?.docs?.[`${link.signingKeyOf(app.keys.sign)} peers`])?.body.boxes.some(each => each.key === extra.key));
  const seed = labLink();
  write(extraFile, { key: extraKey.toString("base64url"), secret: extraSecret.toString("base64url"), sign: extraSign,
    name: extra.name, relay: relay.url, mac: macPub, macSigns: seed.macSigns, docs: seed.docs, setup: seed.setup });
  write(path.join(extraHome, "run", "work-queue", "numbers.json"), { primary: macPub, leases: [], since: 2_000 });
  // Run the previous build verbatim. Lokum receives only the blob, without checkout history.
  const ref = fs.readFileSync(new URL("failure-bench-legacy-ref.txt", import.meta.url), "utf8").trim();
  const archived = new URL(".failure-bench-legacy.mjs", import.meta.url);
  let previous;
  if (fs.existsSync(archived)) {
    previous = fs.readFileSync(archived);
  } else {
    const checkout = fileURLToPath(new URL("../..", import.meta.url));
    const relative = path.relative(checkout, script);
    const result = spawnSync("git", ["--no-optional-locks", "-C", checkout, "show", `${ref}:${relative}`], { maxBuffer: 64 * 1024 * 1024 });
    if (result.status === 0) previous = result.stdout;
    else t.diagnostic(`History unavailable at ${ref}; running the isolation scenario with the current build.`);
  }
  let workshopScript = script;
  if (previous) {
    assert.ok(!previous.equals(fs.readFileSync(script)), "the two builds differ");
    workshopScript = path.join(extraHome, "bin", "kuvo-link.js");
    write(workshopScript, previous.toString("utf8"));
  }
  const child = spawn(process.execPath, [workshopScript, "run"], { env: { ...machineEnv, HOME: extraHome, KUVO_HOME: extraHome }, stdio: "ignore" });
  const stopped = new Promise(resolve => child.once("close", resolve));
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
    // The Workshop has the removal but the main computer can't answer. A legitimate desktop
    // confirms the list from the Workshop and gets in, while the revoked desktop stays out.
    assert.equal(labLink().setup?.primary, macPub);
    await opens(yours, "Lab accepted its current desktop after hearing from Workshop");
    await reached("Lab installed the removal from Workshop", () => memberHasRemoval(first.public));
    assert.deepEqual(await first.open(member), { refused: true });
    assert.equal(relay.has(macAddress), false);

    resumeMember = disconnect(member.address);
    await eventually(() => !relay.has(member.address));
    await resumeMain();
    await remove(second);
    resumeMain = disconnect(macAddress);
    const resumeExtra = disconnect(extraAddress);
    await eventually(() => !relay.has(macAddress) && !relay.has(extraAddress));
    await resumeMember();
    // Isolated from every peer, the member can use the authorizations it already has.
    assert.equal(labLink().setup?.primary, macPub);
    await opens(yours, "an isolated Lab accepted its cached desktop");
    await opens(second, "an isolated Lab still had the older grant");
    assert.equal((await second.request("GET", "kuvo/info")).status, 200);
    assert.equal(memberHasRemoval(second.public), false);

    // The Workshop comes back while the main computer is still off. Its signed update closes the
    // cached session and stops the same key from opening another.
    await resumeExtra();
    await reached("Lab installed the removal once Workshop returned", () => memberHasRemoval(second.public));
    assert.deepEqual(await second.request("GET", "kuvo/info"), { closed: true });
    assert.deepEqual(await second.open(member), { refused: true });
    assert.equal((await yours.request("GET", "kuvo/info")).status, 200);
    assert.equal(relay.has(macAddress), false);
    await resumeMain();
  } finally {
    for (const resume of restart) resume();
    child.kill("SIGTERM");
    await stopped;
    await ask("PUT", "kuvo/link/peers", { peers: before });
    link.macLink.wake?.();
  }
});

test("local uninstall on the primary confirms the removal on a member holding an open session", { timeout: 25_000 }, async t => {
  signal = t.signal;
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
  write(identity, `${key.toString("base64url")}.${crypto.randomBytes(32).toString("base64url")}`);
  try {
    const result = await new Promise(resolve => {
      const child = spawn(process.execPath, [script, "uninstall"], { env: { ...machineEnv, KUVO_HOME: macHome }, stdio: ["ignore", "pipe", "pipe"] });
      let stdout = "", stderr = "";
      child.stdout.on("data", bytes => { stdout += bytes; });
      child.stderr.on("data", bytes => { stderr += bytes; });
      child.on("close", status => resolve({ status, stdout, stderr }));
    });
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
    assert.deepEqual(await desktop.request("GET", "kuvo/computers"), { closed: true });
    assert.deepEqual(await desktop.open(member), { refused: true });
    assert.ok(labLink()?.removed?.some(each => each.key === desktop.public));
    assert.equal(fs.existsSync(identity), false);
  } finally {
    server.closeAllConnections();
    server.close();
    fs.rmSync(path.join(macHome, "run", "off"), { force: true });
    fs.rmSync(path.join(macHome, "run", "serve.json"), { force: true });
  }
});

test("local uninstall revokes its desktop before stopping the box, including a session open to its primary", { timeout: 60_000 }, async t => {
  signal = t.signal;
  const main = { key: link.publicOf(macKey), address: macAddress };
  assert.equal((await ask("POST", `kuvo/computers/${lab.key}/allow-devices`, undefined, { served: true, fromApp: true })).status, 200);
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
