// Every one of your computers in the menu bar (PROTOCOL.md, "In the menu bar", `kuvo/box/at-work`'s
// `computers`, #781): this box first, then each of your other computers as it last told this box
// with its agents: whether it's heard from, what's at work there and what waits on you, and the
// rings of the accounts signed in there, as the plan limits say whose accounts are where. One that's
// away shows nothing at work (what it said is old); an older box that never said leaves it out. And
// what this box tells its peers of itself now says what waits on you here too. The real box code,
// as a machine's box; only the engine, the coding agents' bridge and the relay are stand-ins.
//
// Run with `node --test box/bridge-tests/at-work-computers.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-at-work-computers-")));
removeAtEnd(home);
process.env.KUVO_HOME = home;
const link = await import(LINK_BUNDLE.href);

const run = path.join(home, "run");
const k = key => key.toString("base64url");
const box = (kind, id) => {
  const key = link.newKey();
  const address = k(link.addressOf(crypto.randomBytes(32)));
  return { key, pub: k(link.publicOf(key)), address, peer: { key: k(link.publicOf(key)), address, name: id ?? kind, kind, ...(id ? { id } : {}) } };
};
const here = box("machine", "devbox"), mac = box("mac", "Studio"), second = box("machine", "second"), third = box("machine", "third");
const fresh = () => {
  fs.rmSync(run, { recursive: true, force: true });
  fs.mkdirSync(run, { recursive: true });
  link.writeLink({ secret: crypto.randomBytes(32).toString("base64url"), key: k(here.key), relay: "wss://relay.test", name: "devbox", id: "devbox", mac: mac.pub, macs: [mac.pub], phones: [],
    legacy: { [mac.pub]: { at: 1, peers: [mac.peer, here.peer, second.peer, third.peer] } } });
};
const mailFrom = (from, payload) => link.sealOneWay(Buffer.from(JSON.stringify(payload)), from.key, link.publicOf(here.key)).message;
const takeMail = (sender, payload) => link.peerMail(sender, payload, null);
const tell = (from, agents) => link.receiveMail(mailFrom(from, { at: Date.now(), agents: { at: Date.now(), list: [], ...agents } }), undefined, takeMail);
const opened = (bytes, to) => {
  const body = link.readFrame(bytes.subarray(17)).body;
  return JSON.parse(link.openOneWay(body, to.key, link.publicOf(here.key)).payload.toString("utf8"));
};

// An engine with one session waiting on you (what it asks, as the box noted it) and nothing at
// work, and the coding agents' bridge, whose plan limits say which
// accounts are signed in on which computer: Claude here and on `second`, Codex on `third`, Gemini on
// the Mac, and a plan that ended on `second`.
async function standIns(t) {
  const serve = async answer => {
    const server = http.createServer((request, response) => {
      request.resume();
      request.on("end", () => {
        const value = answer(request.url);
        response.writeHead(value === undefined ? 404 : 200, { "content-type": "application/json" });
        response.end(value === undefined ? "" : JSON.stringify(value));
      });
    });
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
    t.after(() => { server.closeAllConnections(); server.close(); });
    return server.address().port;
  };
  const now = Date.now();
  const sessions = [
    { id: "ses_ask", title: "Deploy the docs", directory: "/work", time: { created: now - 20, updated: now - 5 } },
    { id: "ses_idle", title: "Read the docs", directory: "/work", time: { created: now - 30, updated: now - 10 } },
  ];
  fs.mkdirSync(path.join(run, "glance"), { recursive: true });
  fs.writeFileSync(path.join(run, "glance", "asks.json"), JSON.stringify({ asks: { per_1: { session: "ses_ask", at: now - 1, asks: { kind: "permission", summary: "asks to run a command" } } } }));
  const engine = await serve(url => (url.startsWith("/api/session?") ? { data: sessions } : url === "/api/session/active" ? { data: [] } : undefined));
  const rows = [
    { service: "claude", title: "Claude", rings: { short: { window: "five_hour", used: 0.62, level: "calm" } }, boxes: [{ key: here.pub, here: true }, { key: second.pub }] },
    { service: "codex", title: "Codex", rings: { short: { window: "five_hour", used: 0.95, level: "near" }, long: { window: "week", used: 0.4, level: "calm" } }, boxes: [{ key: third.pub }] },
    { service: "gemini", title: "Gemini", rings: { long: { window: "day", used: 0.1, level: "calm" } }, boxes: [{ key: mac.pub, away: true }] },
    { service: "cursor", title: "Cursor", ended: true, rings: {}, boxes: [{ key: second.pub }] },
  ];
  const bridge = await serve(url => (url === "/api/kuvo/limits" ? { rows } : undefined));
  // (The bridge counts as running while a process of that name is.)
  const agents = spawn(process.execPath, ["-e", "setTimeout(() => {}, 120_000)", "kuvo-agents"], { stdio: "ignore" });
  t.after(() => agents.kill());
  return { engine: { port: engine, password: "pw" }, bridge: { port: bridge, password: "pw", pid: agents.pid } };
}

const atWork = async handle => {
  const pieces = [];
  for await (const piece of handle({ method: "GET", path: "kuvo/box/at-work", headers: {}, fromApp: true }, undefined)) pieces.push(piece);
  assert.equal(pieces.find(each => each.head).head.status, 200);
  return JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString());
};

test("the menu bar's computers: this one first, then each of yours with what's at work there and what waits on you, and the accounts signed in there", async t => {
  fresh();
  const { engine, bridge } = await standIns(t);
  const handle = link.engineHandler(() => engine, { name: "devbox" }, () => bridge);
  // A newer box: two at work, one waiting on you with what it asks (one that isn't a session's id left out).
  tell(second, { computer: { version: "1.1.0", working: 2, atWork: [{ id: "ses_a1", title: "Ship the release" }, { id: "ses_b2" }],
    waiting: 1, asking: [{ id: "ses_q1", title: "Deploy to staging", asks: "asks to run a command" }, { id: "not a session" }] } });
  // An older box: its version alone.
  tell(third, { computer: { version: "0.9.180" } });
  // The Mac said it was at work, then went away.
  tell(mac, { computer: { version: "1.1.0", working: 3, atWork: [{ id: "ses_m1", title: "Old news" }], waiting: 2, asking: [{ id: "ses_m2" }] } });
  fs.writeFileSync(path.join(run, "peer-reach.json"), JSON.stringify({ [mac.pub]: Date.now() }));

  const said = await atWork(handle);
  // This box's own facts stand as they were.
  assert.equal(said.working, 0);
  assert.deepEqual(said.rings.map(each => each.service), ["claude", "codex", "gemini"]);
  const computers = Object.fromEntries(said.computers.map(each => [each.here ? "here" : each.name, each]));
  assert.equal(said.computers[0].here, true);
  assert.deepEqual(said.computers.map(each => each.name).slice(1).sort(), ["Studio", "second", "third"]);

  const asking = [{ id: "ses_ask", title: "Deploy the docs", asks: "asks to run a command" }];
  assert.deepEqual(said.asking, asking);
  assert.deepEqual(computers.here, { key: here.pub, name: "devbox", kind: "machine", here: true, reach: "connected", working: 0, sessions: [], waiting: 1, asking,
    rings: [{ service: "claude", title: "Claude", short: { used: 0.62, level: "calm" } }] });
  assert.deepEqual(computers.second, { key: second.pub, name: "second", kind: "machine", reach: "connected",
    working: 2, sessions: [{ id: "ses_a1", title: "Ship the release" }, { id: "ses_b2" }],
    waiting: 1, asking: [{ id: "ses_q1", title: "Deploy to staging", asks: "asks to run a command" }],
    rings: [{ service: "claude", title: "Claude", short: { used: 0.62, level: "calm" } }] });
  // An older box: nothing said of what's at work or waits there, so neither is counted; its account's rings still are.
  assert.deepEqual([computers.third.reach, "working" in computers.third, "waiting" in computers.third, computers.third.sessions, computers.third.asking], ["connected", false, false, [], []]);
  assert.deepEqual(computers.third.rings, [{ service: "codex", title: "Codex", short: { used: 0.95, level: "near" }, long: { used: 0.4, level: "calm" } }]);
  // Away: what it said then is old, so nothing is shown at work or waiting there; its rings stay, as the plan limits keep them.
  assert.deepEqual([computers.Studio.kind, computers.Studio.reach, "working" in computers.Studio, "waiting" in computers.Studio, computers.Studio.sessions, computers.Studio.asking],
    ["mac", "away", false, false, [], []]);
  assert.deepEqual(computers.Studio.rings.map(each => each.service), ["gemini"]);

  // Back: what it says now shows again.
  fs.rmSync(path.join(run, "peer-reach.json"));
  tell(mac, { computer: { version: "1.1.0", working: 1, atWork: [{ id: "ses_m3", title: "Now" }], waiting: 0, asking: [] } });
  const back = (await atWork(handle)).computers.find(each => each.name === "Studio");
  assert.deepEqual([back.reach, back.working, back.sessions, back.waiting], ["connected", 1, [{ id: "ses_m3", title: "Now" }], 0]);

  // What this box tells its peers of itself: what waits on you here too (the session asking, with
  // what it asks), beside what's at work.
  const sent = [];
  await link.announce({ send: bytes => sent.push(bytes) }, () => engine, link.readLink(), Date.now(), true);
  const toSecond = sent.find(bytes => k(bytes.subarray(1, 17)) === second.address);
  const told = opened(toSecond, second).agents.computer;
  assert.deepEqual([told.working, told.waiting, told.asking], [0, 1, asking]);
});
