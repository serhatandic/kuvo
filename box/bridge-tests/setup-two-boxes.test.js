// Your setup over the relay (PROTOCOL.md, "Your setup"): this Mac, numbering on its own, leads a
// setup, its peers document names it, and its machine takes it, as it updates with nothing for you
// to do: `kuvo/setup` says the same on both, each keeps its card, and both number on where they
// were (this Mac from #70, the machine from the block it had), the machine asking this Mac, its
// main computer, for more.
//
// This Mac's box (`serve`'s link, here, with its app a stand-in) and one of its machines
// (`kuvo-link.js run`, a process of its own, set up as the app sets one up over SSH). Only the
// relay, the app and the engines are stand-ins.
//
// Run with `node --test box/bridge-tests/setup-two-boxes.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { relayStandIn } from "./relay-stand-in.js";
import { recordRelayLease, relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const script = fileURLToPath(LINK_BUNDLE);
const support = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-setup-two-")));
removeAtEnd(support);
const macHome = path.join(support, "mac");
const machineHome = path.join(support, "machine");
for (const home of [macHome, machineHome]) fs.mkdirSync(path.join(home, "run"), { recursive: true });
const write = (file, value) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, typeof value === "string" ? value : JSON.stringify(value), { mode: 0o600 });
};
const read = file => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; } };

// What a box writes in another process, looked at every few milliseconds until it's there, or
// until the test runs out of time (`signal`).
let signal;
function eventually(found) {
  return new Promise((resolve, reject) => {
    const look = async () => {
      const value = await found();
      if (!value) return;
      clearInterval(timer);
      resolve(value);
    };
    const timer = setInterval(look, 50);
    signal?.addEventListener("abort", () => {
      clearInterval(timer);
      reject(new Error(`it never got there: ${found}`));
    }, { once: true });
    look();
  });
}
const body = request => new Promise(resolve => {
  const chunks = [];
  request.on("data", chunk => chunks.push(chunk));
  request.on("end", () => {
    try { resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}")); } catch { resolve({}); }
  });
});
const listening = server => new Promise(resolve => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

// An engine with `sessions`, and the tool call each is making now (`calling`).
async function engineWith(sessions) {
  const calling = new Map();
  const all = new Map(sessions.map(each => [each.id, { time: { created: Date.now(), updated: Date.now() }, directory: "/work", ...each }]));
  const server = http.createServer(async (request, response) => {
    await body(request);
    const url = new URL(request.url, "http://x");
    const send = (status, data) => response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(status < 400 ? { data } : { message: data }));
    if (request.headers.authorization !== `Basic ${Buffer.from("opencode:pw").toString("base64")}`) return response.writeHead(401).end();
    if (url.pathname === "/api/session" && request.method === "GET") return send(200, [...all.values()].filter(each => !each.parentID && (url.searchParams.get("limit") !== "100" || each.listed)));
    if (url.pathname === "/api/session/active") return send(200, {});
    const match = url.pathname.match(/^\/api\/session\/([^/]+)(\/.*)?$/);
    const id = match && decodeURIComponent(match[1]);
    const session = id && all.get(id);
    if (!session) return send(404, "No such session.");
    if (!match[2]) return send(200, session);
    if (match[2] === "/message") {
      const call = calling.get(id);
      return send(200, call ? [{ id: "msg_1", type: "assistant", content: [{ type: "tool", name: `kuvo-browser_${call.name}`, callID: "call_1", state: { status: "running", input: call.args } }] }] : []);
    }
    return send(200, []);
  });
  const port = await listening(server);
  atEnd(() => { server.closeAllConnections(); server.close(); });
  return { port, password: "pw", calling };
}

// The app, as its host interface answers this Mac's box: its keys.
const app = { keys: null };
const appHost = http.createServer(async (request, response) => {
  const given = await body(request);
  if (request.url === "/hello") {
    const proof = crypto.createHmac("sha256", "host-password").update(`kuvo-host-hello\u0000${given.nonce}\u0000127.0.0.1:${appHost.address().port}`).digest("hex");
    return response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ proof }));
  }
  if (request.headers.authorization !== "Bearer host-password") return response.writeHead(401).end();
  if (request.url === "/link/identity") return response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(app.keys));
  if (request.url === "/link/route") return response.writeHead(404, { "content-type": "application/json", "x-kuvo-answer": "1" }).end("{}");
  response.writeHead(404).end();
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

// A session here, and one on the machine.
const macEngine = await engineWith([{ id: "ses_lead", title: "Foreman", listed: true }]);
const labEngine = await engineWith([{ id: "ses_far", title: "Importer", listed: true }]);
write(path.join(macHome, "run", "engine.json"), { port: macEngine.port, password: "pw" });
write(path.join(machineHome, "run", "engine.json"), { port: labEngine.port, password: "pw" });

// This Mac's keys, as your Keychain keeps them.
const macKey = link.newKey();
const macSecret = crypto.randomBytes(32);
app.keys = { key: macKey.toString("base64url"), secret: macSecret.toString("base64url"), sign: link.newSigningSeed() };

link.macLink.hold = link.macNoticeHold();
const handle = link.engineHandler(link.readEngineFile, { name: "Mac" }, link.readBridgeFile, () => {}, link.macLink.hold);
async function ask(method, p, given) {
  const parts = [];
  for await (const part of handle({ method, path: p, headers: {}, body: given ? Buffer.from(JSON.stringify(given)) : Buffer.alloc(0), fromApp: true }, {}, undefined)) parts.push(part);
  const text = Buffer.concat(parts.filter(part => part.body).map(part => part.body)).toString();
  let json;
  try { json = JSON.parse(text); } catch {}
  return { status: parts[0].head.status, json };
}
// An agent's Work Queue tool on this Mac, while its conversation is making the call.
async function onMac(name, args, session = "ses_lead") {
  macEngine.calling.set(session, { name, args });
  try {
    return (await ask("POST", "kuvo/work-queue/tools", { name, arguments: args, session })).json;
  } finally {
    macEngine.calling.delete(session);
  }
}
const machineEnv = { PATH: process.env.PATH, HOME: support, KUVO_HOME: machineHome, TMPDIR: support };
const labPid = () => { try { return Number(fs.readFileSync(path.join(machineHome, "run", "link.pid"), "utf8")); } catch { return 0; } };

// This Mac numbered on its own up to #69.
write(path.join(macHome, "run", "work-queue", "numbers.json"), { primary: "self", next: 70, leases: [], since: 1_000 });

let lab = null;
before(async () => {
  assert.notEqual((await ask("PUT", "kuvo/link/config", { phones: false, machines: true, relay: relay.url, name: "Studio Mac", app: "0.9.0", features: ["mac", "work.elsewhere", "work-queue", "work-queue.elsewhere"] })).json?.state, "failed");
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
  const state = link.readLink().setup;
  assert.equal(state.role, "primary");
  const grant = leases.seed(state, { [state.primary]: state.sign, [peer.key]: peer.sign }, 500);
  recordRelayLease(path.join(macHome, "run"), state, { relay: grant });
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
  // As a member, lab has one number left of a block this Mac gave it.
  write(path.join(machineHome, "run", "work-queue", "numbers.json"), { primary: link.publicOf(macKey).toString("base64url"), leases: [[41, 41]], since: 2_000 });
  link.runMacLink();
  link.runMacPeers();
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

const labQueue = owner => read(path.join(machineHome, "run", "work-queue", "queues", `${owner}.json`))?.items ?? [];
const macPub = link.publicOf(macKey).toString("base64url");
// What the machine's box answers a client, as a Mac asks it over SSH (`box route`).
const labRoute = async (method, p) => {
  const child = spawn(process.execPath, [script, "box", "route"], { env: machineEnv, stdio: ["pipe", "pipe", "pipe"] });
  let out = "", err = "";
  child.stdout.on("data", chunk => { out += chunk; });
  child.stderr.on("data", chunk => { err += chunk; });
  child.stdin.end(JSON.stringify({ method, path: p }));
  assert.equal((await once(child, "close"))[0], 0, err);
  const answer = JSON.parse(out.trim().split("\n").at(-1));
  return { status: answer.status, json: JSON.parse(answer.body) };
};
// An agent's Work Queue tool on lab, while its conversation is making the call (lab's box asks
// this process's engine stand-in, so it's run without blocking).
async function onLab(name, args, session = "ses_far") {
  labEngine.calling.set(session, { name, args });
  try {
    const child = spawn(process.execPath, [script, "box", "route"], { env: machineEnv });
    let out = "";
    child.stdout.on("data", chunk => { out += chunk; });
    child.stdin.end(JSON.stringify({ method: "POST", path: "kuvo/work-queue/tools", body: { name, arguments: args, session } }));
    await new Promise(resolve => child.on("close", resolve));
    return JSON.parse(JSON.parse(out.trim().split("\n").at(-1)).body);
  } finally {
    labEngine.calling.delete(session);
  }
}
const sameAcross = view => ({ setup: view.setup, epoch: view.epoch, primary: view.primary, computers: view.computers });

test("this Mac leads a setup and its machine takes it: kuvo/setup says the same on both, each keeps its card, and both number on where they were, the machine from this Mac", { timeout: 120_000 }, async t => {
  signal = t.signal;
  // This Mac's setup, made on its rounds, reaches lab by its peers document.
  const ours = await eventually(async () => {
    const view = (await ask("GET", "kuvo/setup")).json;
    return view?.setup ? view : null;
  });
  assert.equal(ours.you.role, "primary");
  assert.deepEqual(ours.primary, { key: macPub, name: "Studio Mac", kind: "mac" });
  // (Looked for in lab's file: its box answers once it has it.)
  await eventually(() => read(path.join(machineHome, "run", "link.json"))?.setup?.role === "member");
  const theirs = (await labRoute("GET", "kuvo/setup")).json;
  assert.deepEqual(sameAcross(theirs), sameAcross(ours));
  assert.deepEqual(theirs.you, { key: lab.key, role: "member" });
  assert.deepEqual(ours.computers.map(each => [each.key, each.role, each.app]), [[macPub, "primary", true], [lab.key, "member", false]]);
  assert.deepEqual([ours.startsFromOthers.default, theirs.startsFromOthers.default], ["off", "on"]);
  // Each one's card.
  const card = await eventually(() => read(path.join(machineHome, "run", "setup-card.json"))?.setup ? read(path.join(machineHome, "run", "setup-card.json")) : null);
  assert.deepEqual([card.setup, card.role, card.primary.key, card.app], [ours.setup, "member", macPub, false]);
  assert.equal(read(path.join(macHome, "run", "setup-card.json"))?.role, "primary");

  // Numbers as they were: this Mac's from #70, lab's from the block it had.
  assert.match((await onMac("queue_add", { title: "Ship the release" })).text, /^Added #70 /);
  const nextBlock = read(path.join(macHome, "run", "work-queue", "numbers.json")).next;
  assert.match((await onLab("queue_add", { title: "Port the importer" })).text, /^Added #41 /);
  // That was its last: lab asks its main computer, this Mac, for more, which gives the next block
  // past its own, and lab's next item takes from it, now or once it comes.
  await eventually(() => Object.keys(read(path.join(macHome, "run", "work-queue", "numbers.json"))?.given ?? {}).length);
  assert.deepEqual(Object.values(read(path.join(macHome, "run", "work-queue", "numbers.json")).given).map(each => each.from), [nextBlock]);
  assert.match((await onLab("queue_add", { title: "Second" })).text, new RegExp(`^Added #${nextBlock} `));
  assert.equal(read(path.join(machineHome, "run", "work-queue", "numbers.json")).primary, macPub);
});

test("a Mac hands over to its machine, then follows its setup", { timeout: 30_000 }, async t => {
  signal = t.signal;
  await eventually(() => link.readPeerAgents()[lab.key]?.features?.includes("setup.handover"));
  const setup = (await ask("GET", "kuvo/setup")).json;
  assert.equal(setup.you.role, "primary");
  assert.deepEqual(await ask("POST", "kuvo/setup/handover", { primary: lab.key, epoch: setup.epoch }), {
    status: 202, json: { primary: lab.key, epoch: setup.epoch + 1 },
  });
  const there = await eventually(async () => {
    const view = (await labRoute("GET", "kuvo/setup")).json;
    return view?.you.role === "primary" && view;
  });
  const here = await eventually(async () => {
    const view = (await ask("GET", "kuvo/setup")).json;
    return view?.you.role === "member" && view.primary.key === lab.key && view;
  });
  assert.deepEqual(sameAcross(there), sameAcross(here));
  assert.deepEqual([here.you.role, there.primary.key], ["member", lab.key]);
});

test("unlinking the main computer starts a handover with access still intact", { timeout: 25_000 }, async () => {
  const setup = (await ask("GET", "kuvo/setup")).json;
  const removed = await ask("DELETE", `kuvo/computers/${lab.key}`);
  assert.deepEqual(removed, { status: 202, json: {
    primary: macPub, epoch: setup.epoch + 1, unlinking: true, removal: { accessRemoved: false, state: "handing-over" },
    message: "Studio Mac is becoming the main computer. Then lab is unlinked.",
  } });
  assert.equal(leases.leaseFor(setup.setup).holder, macPub);
});
