// This Mac's box and your setup (PROTOCOL.md, "Your setup"; the Work Queue's "Numbers"): a Mac that
// numbers on its own leads a setup of its own, named in its peers document, and keeps numbering
// where it was; one that numbers through another Mac, by a machine both set up (or with a machine
// another Mac set up too), leads none and numbers as before, from the first Mac you set up.
//
// This Mac's box is the link in this process (`KUVO_BOX=mac`), its keys as its app gives them; its
// machines and the other Mac are keys whose mail is opened as the relay carries it, and what they
// told this Mac of their Work Queues is kept as it would be. Only the engine is a stand-in.
//
// Run with `node --test box/bridge-tests/setup-mac.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { beforeEach, test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const support = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-setup-mac-")));
removeAtEnd(support);
const home = path.join(support, "box");
Object.assign(process.env, {
  KUVO_HOME: home,
  KUVO_BOX: "mac",
  KUVO_ENGINE_CONFIG: path.join(support, "opencode"),
  KUVO_BOTS: path.join(support, "bots"),
  KUVO_WAKES: path.join(support, "wakes"),
});
const link = await import(LINK_BUNDLE.href);
const run = path.join(home, "run");
const queues = path.join(run, "work-queue");
const numbers = path.join(queues, "numbers.json");
const write = (file, value) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, typeof value === "string" ? value : JSON.stringify(value));
};
const read = file => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; } };

// The engine, with the session whose tool calls add items here.
const calling = new Map();
const engine = http.createServer((request, response) => {
  request.resume();
  request.on("end", () => {
    const reply = value => { response.writeHead(200, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
    const url = new URL(request.url, "http://x");
    if (url.pathname === "/api/session" && request.method === "GET") return reply({ data: [{ id: "ses_lead", title: "Lead", time: { created: 1, updated: 2 } }] });
    if (url.pathname === "/api/session/ses_lead") return reply({ data: { id: "ses_lead", title: "Lead", time: { created: 1, updated: 2 } } });
    if (url.pathname === "/api/session/ses_lead/message") {
      const call = calling.get("ses_lead");
      return reply({ data: call ? [{ id: "msg_1", type: "assistant", content: [{ type: "tool", name: `kuvo-browser_${call.name}`, callID: "call_1", state: { status: "running", input: call.args } }] }] : [] });
    }
    reply({ data: [] });
  });
});
await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
atEnd(() => { engine.closeAllConnections(); engine.close(); });
const target = { port: engine.address().port, password: "p" };

// This Mac (its keys, as its app gives them), its machine lab, and another Mac that set lab up too.
const k = key => key.toString("base64url");
const box = (kind, name) => {
  const key = link.newKey();
  const sign = link.newSigningSeed();
  const address = k(link.addressOf(crypto.randomBytes(32)));
  return { key, sign, pub: k(link.publicOf(key)), address, peer: { key: k(link.publicOf(key)), address, name, kind, sign: link.signingKeyOf(sign), id: name } };
};
const self = box("mac", "Studio"), lab = box("machine", "lab"), other = box("mac", "Old Mac");
link.macLink.overlay = { key: k(self.key), secret: k(crypto.randomBytes(32)), sign: self.sign, relay: "wss://relay.test", name: "Studio" };
link.macLink.config = { phones: false, machines: true, relay: "wss://relay.test", name: "Studio", app: "0.9.0" };

// Mail as the relay hands it on, and what this box sent, opened as `to` would.
const sent = [];
const host = { welcomed: true, send: bytes => sent.push(bytes) };
const mailTo = to => sent.filter(bytes => k(bytes.subarray(1, 17)) === to.address)
  .map(bytes => JSON.parse(link.openOneWay(link.readFrame(bytes.subarray(17)).body, to.key, link.publicOf(self.key)).payload.toString("utf8")));
// What lab told of how it numbers, as its Work Queue's mail leaves it here (a box that knows setups
// says whether it's shared).
const labTold = numbering => write(path.join(queues, "peers", `${lab.pub}.json`), { id: "wqt_labtoldlabtold", at: 1, heard: 1, kind: "machine", since: 50, items: [], owners: [], labels: [], shared: false, ...numbering });

const handle = link.engineHandler(() => target);
async function route(method, pathname) {
  const pieces = [];
  for await (const piece of handle({ method, path: pathname, headers: {}, body: undefined, fromApp: true }, undefined)) pieces.push(piece);
  return { status: pieces.find(each => each.head).head.status, json: JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString() || "null") };
}
async function addItem(title) {
  calling.set("ses_lead", { name: "queue_add", args: { title } });
  try {
    for await (const piece of handle({ method: "POST", path: "kuvo/work-queue/tools", headers: {}, body: Buffer.from(JSON.stringify({ name: "queue_add", arguments: { title }, session: "ses_lead" })), fromApp: true }, undefined)) {
      if (piece.body) return JSON.parse(piece.body.toString()).text;
    }
  } finally {
    calling.delete("ses_lead");
  }
}
// This Mac's own peers document as it signs it now.
const ownPeers = () => {
  let body = null;
  link.updateLink(current => {
    const made = link.ownDoc(current, "peers", link.macPeersBody(current));
    body = link.readDoc(made.envelope).body;
    return current;
  });
  return body;
};

beforeEach(() => {
  fs.rmSync(run, { recursive: true, force: true });
  write(path.join(run, "engine.json"), { ...target, version: "0.0.0" });
  sent.length = 0;
  link.writeLink({ phones: [], macPeers: [lab.peer], macMoved: [] });
});

test("legacy numbering can route through a peer to the recorded primary", () => {
  write(numbers, { primary: "self", next: 12, leases: [], since: 9_000 });
  labTold({ primary: { key: other.pub, kind: "mac", since: 1_000 } });
  const primary = link.primaryOf(link.readLink(), numbers);
  assert.equal(primary.peer?.key, other.pub, "the Mac set up first");
  assert.equal(primary.via?.key, lab.pub, "reached through lab");
  link.notePrimaryNow(link.readLink(), { file: numbers });
  const kept = link.readNumbers(numbers);
  assert.equal(kept.primary, other.pub);
  assert.equal(kept.through, lab.pub);
  assert.ok(kept.asking, "it asks at once, having numbered on its own");
  link.pushNumbers(host, link.readLink());
  const asked = mailTo(lab).map(each => each.workQueue);
  assert.equal(asked.length, 1);
  assert.equal(asked[0].to, other.pub);
  assert.ok(asked[0].ask.top >= 11, "telling how far it got");
});

test("this Mac, numbering on its own, leads a setup of its own: its peers document names it, its numbers go on where they were (#41, then #42), and kuvo/setup says it's the main computer", async () => {
  write(numbers, { primary: "self", next: 42, leases: [], since: 1_000 });
  labTold({ primary: { key: self.pub, kind: "mac", since: 1_000 }, shared: undefined });
  link.noteSetupNow(Date.now(), { soon: 0 });
  assert.deepEqual(link.primaryOf(link.readLink(), numbers), { self: true });
  const named = ownPeers().setup;
  assert.match(named?.id ?? "", /^set_[A-Za-z0-9_-]{22}$/);
  assert.deepEqual({ epoch: named.epoch, primary: named.primary }, { epoch: 1, primary: self.pub });
  // Looked at again: the same setup, never a new one.
  link.noteSetupNow(Date.now(), { soon: 0 });
  assert.equal(ownPeers().setup.id, named.id);

  assert.match(await addItem("Ship it"), /^Added #42 /);
  const shown = await route("GET", "kuvo/setup");
  assert.equal(shown.status, 200);
  assert.deepEqual(shown.json, {
    setup: named.id, epoch: 1,
    primary: { key: self.pub, name: "Studio", kind: "mac" },
    you: { key: self.pub, role: "primary" },
    computers: [{ key: self.pub, name: "Studio", kind: "mac", role: "primary", app: true }, { key: lab.pub, name: "lab", kind: "machine", role: "member", app: false }],
    startsFromOthers: { value: null, default: "off" },
    heldBy: [],
    welcomed: false,
  });
  // Its card, as it's next written: the same, with the item made since.
  link.writeCard();
  const card = read(path.join(run, "setup-card.json"));
  assert.deepEqual({ setup: card.setup, role: card.role, primary: card.primary.key, app: card.app, computers: card.computers, items: card.items }, { setup: named.id, role: "primary", primary: self.pub, app: true, computers: 2, items: 1 });
});

test("a Mac follows the numbering primary and ignores obsolete peer flags", () => {
  write(numbers, { primary: "self", next: 12, leases: [], since: 9_000 });
  labTold({ primary: { key: other.pub, kind: "mac", since: 1_000 } });
  link.noteSetupNow(Date.now(), { soon: 0 });
  assert.equal(link.readLink().setup, undefined, "it numbers from the Mac set up first: no setup of its own");
  assert.equal(ownPeers().setup, undefined);
  assert.equal(link.primaryOf(link.readLink(), numbers).via?.key, lab.pub, "and through lab, as before");

  // A legacy numbering record's shared field does not affect leadership.
  write(numbers, { primary: "self", next: 12, leases: [], since: 1_000 });
  labTold({ primary: { key: self.pub, kind: "mac", since: 1_000 }, shared: true });
  link.noteSetupNow(Date.now(), { soon: 0 });
  const led = ownPeers().setup;
  assert.ok(led);
  assert.deepEqual(link.setupView(link.readLink()).heldBy, []);
  // Set aside by an earlier Kuvo Code for that word: it leads again, the same setup at its epoch.
  link.updateLink(current => ({ ...current, setup: { ...current.setup, aside: true } }));
  link.noteSetupNow(Date.now(), { soon: 0 });
  assert.deepEqual(ownPeers().setup, led, "the same setup, at the same epoch");
  assert.equal(link.readLink().setup.aside, undefined);
});

test("a Mac whose keys were made anew while it doesn't lead lets the setup it led with its old ones go: it numbers as piece 3 says, not as that setup's main computer", () => {
  write(numbers, { primary: "self", next: 12, leases: [], since: 1_000 });
  labTold({ primary: { key: self.pub, kind: "mac", since: 1_000 } });
  link.noteSetupNow(Date.now(), { soon: 0 });
  assert.equal(link.readLink().setup?.role, "primary");
  const before = link.macLink.overlay;
  const fresh = link.newKey();
  try {
    link.macLink.overlay = { ...before, key: k(fresh), sign: link.newSigningSeed() };
    // Its machine now numbers from a Mac set up earlier, through which this one numbers too.
    labTold({ primary: { key: other.pub, kind: "mac", since: 500 } });
    assert.equal(link.primaryOf(link.readLink(), numbers).peer?.key, other.pub, "not its old setup's main computer");
    assert.equal(link.setupView(link.readLink()).setup, null);
    link.noteSetupNow(Date.now(), { soon: 0 });
    assert.equal(link.readLink().setup, undefined, "let go");
  } finally {
    link.macLink.overlay = before;
  }
});

test("kuvo/setup/events says it now, and again as link.json changes, written here or by another process (setup over SSH)", async () => {
  write(numbers, { primary: "self", next: 12, leases: [], since: 1_000 });
  labTold({ primary: { key: self.pub, kind: "mac", since: 1_000 } });
  const stop = new AbortController();
  const lines = handle({ method: "GET", path: "kuvo/setup/events", headers: {}, body: undefined, fromApp: true }, undefined, stop.signal)[Symbol.asyncIterator]();
  const next = async () => {
    for (;;) {
      const { value, done } = await lines.next();
      if (done) return null;
      if (value.body) return JSON.parse(value.body.toString());
    }
  };
  try {
    assert.equal((await next()).setup, null);
    link.noteSetupNow(Date.now(), { soon: 0 });
    const led = await next();
    assert.equal(led.you.role, "primary");
    // Another process sets it aside in the file.
    const kept = JSON.parse(fs.readFileSync(path.join(run, "link.json"), "utf8"));
    fs.writeFileSync(path.join(run, "link.json"), JSON.stringify({ ...kept, setup: { ...kept.setup, aside: true } }));
    assert.equal((await next()).setup, null);
  } finally {
    stop.abort();
    await lines.return?.();
  }
});

test("this Mac's setup outlives a restart: kept in its link.json, it's named again, the same, once its keys are back", () => {
  write(numbers, { primary: "self", next: 12, leases: [], since: 1_000 });
  labTold({ primary: { key: self.pub, kind: "mac", since: 1_000 } });
  link.noteSetupNow(Date.now(), { soon: 0 });
  const led = ownPeers().setup;
  assert.equal(read(path.join(run, "link.json")).setup.id, led.id, "kept on disk");
  const before = link.macLink.overlay;
  try {
    // Started again: no keys from its app yet, so no setup it can say it leads.
    link.macLink.overlay = null;
    assert.equal(link.setupView(link.readLink()).setup, null);
    link.macLink.overlay = before;
    link.noteSetupNow(Date.now(), { soon: 0 });
    assert.deepEqual(ownPeers().setup, led);
  } finally {
    link.macLink.overlay = before;
  }
});

test("this Mac's static key made anew: setting a machine up again lets the old key go, and the machine follows the new setup at once", async () => {
  write(numbers, { primary: "self", next: 12, leases: [], since: 1_000 });
  labTold({ primary: { key: self.pub, kind: "mac", since: 1_000 } });
  link.noteSetupNow(Date.now(), { soon: 0 });
  const script = fileURLToPath(LINK_BUNDLE);
  const machine = path.join(support, "machine");
  fs.mkdirSync(path.join(machine, "run"), { recursive: true });
  const env = { PATH: process.env.PATH, HOME: support, KUVO_HOME: machine, TMPDIR: support };
  // The bridge setup starts on the machine (detached, outliving setup): it names itself in link.pid
  // once it has loaded, which on a busy computer is after setup has returned.
  const alive = pid => { try { return pid > 0 && process.kill(pid, 0); } catch { return false; } };
  const bridgePid = () => { try { return Number(fs.readFileSync(path.join(machine, "run", "link.pid"), "utf8")) || 0; } catch { return 0; } };
  const bridgeVersion = () => { try { return fs.readFileSync(path.join(machine, "run", "link.version"), "utf8").trim(); } catch { return ""; } };
  const until = async (done, what) => {
    for (let tries = 0; !done(); tries++) {
      if (tries === 600) assert.fail(`${what}, after 30 seconds`);
      await new Promise(resolve => setTimeout(resolve, 50));
    }
  };
  // Stopped, and gone, before its folder is removed: one still running writes in it as it goes.
  atEnd(async () => {
    const pid = bridgePid();
    if (!pid) return;
    try { process.kill(pid, "SIGTERM"); } catch {}
    await until(() => !alive(pid), "the machine's bridge never stopped");
  });
  // Set up as the app sets it up: what this Mac's box gives (kuvo/link/setup), over SSH. Its bridge
  // up before the next step, so setting up again finds it running rather than starting a second.
  const runSetup = given => spawnSync(process.execPath, [script, "setup"], {
    input: JSON.stringify({ relay: "wss://relay.invalid", name: "lab", id: "lab", ...given }),
    env: { ...env, BUN_BE_BUN: process.env.BUN_BE_BUN }, encoding: "utf8",
  });
  const setUp = async () => {
    const given = (await route("GET", "kuvo/link/setup")).json;
    const result = runSetup(given);
    assert.equal(result.status, 0, result.stderr);
    // (Its version too, written just after: without it, setting up again starts a newer one.)
    await until(() => alive(bridgePid()) && bridgeVersion() !== "", "the machine's bridge never started");
    return { given, link: read(path.join(machine, "run", "link.json")) };
  };
  const first = await setUp();
  assert.deepEqual(first.link.macs, [self.pub]);
  assert.equal(first.link.setup?.primary, self.pub);

  const before = link.macLink.overlay;
  const fresh = link.newKey();
  try {
    link.macLink.overlay = { ...before, key: k(fresh), sign: link.newSigningSeed() };
    link.noteSetupNow(Date.now(), { soon: 0 });
    const now = k(link.publicOf(fresh));
    const offered = (await route("GET", "kuvo/link/setup")).json;
    const unrelated = runSetup({ ...offered, replaces: [] });
    assert.equal(unrelated.status, 4, unrelated.stderr);
    assert.equal(read(path.join(machine, "run", "link.json")).setup.id, first.link.setup.id);
    const again = await setUp();
    assert.deepEqual(again.link.macs, [now], "never two of one Mac");
    assert.equal(again.link.setup?.primary, now);
    assert.equal(again.link.setup?.aside, undefined, "the new key keeps its setup");
    assert.equal(again.link.setup?.id, ownPeers().setup.id);
    assert.deepEqual([first.given.replaces, again.given.replaces], [[], [self.pub]], "what this Mac's box gives: the key it had before");

    write(path.join(machine, "run", "link.json"), { ...again.link, setup: { ...again.link.setup, managed: true } });
    const changingManagedSetup = runSetup({ ...first.given, replaces: [now] });
    assert.equal(changingManagedSetup.status, 4, changingManagedSetup.stderr);
    assert.equal(read(path.join(machine, "run", "link.json")).setup.id, again.link.setup.id);
  } finally {
    link.macLink.overlay = before;
  }
});
