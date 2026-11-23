// Your computers as a machine's box lists them, and what you do with them from it (PROTOCOL.md,
// "Computers", #735): what each answers (`features`, whole or not at all), which of its sessions
// are at work, `working` left out where it was never said; renaming and removing a machine handed
// to the computer that writes its record (its one Mac here), or refused in words where that can't
// be done; and Connect, this box's own, for one that's away. The real box code, as a machine's box;
// only the engine and the relay are stand-ins.
//
// Run with `node --test box/bridge-tests/computers.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-computers-")));
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
const fresh = (extra = {}) => {
  fs.rmSync(run, { recursive: true, force: true });
  fs.mkdirSync(run, { recursive: true });
  link.forgetQueueActs();
  link.writeLink({ secret: crypto.randomBytes(32).toString("base64url"), key: k(here.key), relay: "wss://relay.test", name: "devbox", id: "devbox", mac: mac.pub, macs: [mac.pub], phones: [],
    legacy: { [mac.pub]: { at: 1, peers: [mac.peer, here.peer, second.peer, third.peer] } }, ...extra });
};
const mailFrom = (from, payload) => link.sealOneWay(Buffer.from(JSON.stringify(payload)), from.key, link.publicOf(here.key)).message;
const takeMail = (sender, payload) => link.peerMail(sender, payload, null);
const tell = (from, agents) => link.receiveMail(mailFrom(from, { at: Date.now(), agents: { at: Date.now(), list: [], ...agents } }), undefined, takeMail);
const opened = (bytes, to) => {
  const body = link.readFrame(bytes.subarray(17)).body;
  return JSON.parse(link.openOneWay(body, to.key, link.publicOf(here.key)).payload.toString("utf8"));
};
// What the box answers, as a client asks it.
const handle = link.engineHandler(() => ({ port: 1, password: "pw" }), { name: "devbox" });
const ask = async (method, route, body) => {
  const pieces = [];
  for await (const piece of handle({ method, path: route, headers: {}, fromApp: true, ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) }, undefined)) pieces.push(piece);
  return { status: pieces.find(each => each.head).head.status, body: JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString() || "{}") };
};
const listed = async () => {
  const { status, body } = await ask("GET", "kuvo/computers");
  assert.equal(status, 200);
  return Object.fromEntries(body.computers.map(each => [each.here ? "here" : each.name, each]));
};
const ids = computer => computer.actions.map(action => action.id);
// The Mac writes your machines' records, and takes what you do to them from your other computers.
const MAC_TAKES = ["computers", "computers.manage", "work-queue.forward"];

// What a box left for its link to send, there once it is (another process's, as `box route` leaves it).
function eventually(found) {
  return new Promise(resolve => {
    const look = () => {
      const value = found();
      if (!value) return;
      clearInterval(timer);
      resolve(value);
    };
    const timer = setInterval(look, 10);
    look();
  });
}

// An engine whose `ses_busy` is at work (its own turn runs) and `ses_idle` isn't.
async function engine(t) {
  const now = Date.now();
  const sessions = [
    { id: "ses_busy", title: "Fix the login page", directory: "/work", time: { created: now - 10, updated: now } },
    { id: "ses_idle", title: "Read the docs", directory: "/work", time: { created: now - 20, updated: now - 5 } },
  ];
  const server = http.createServer((request, response) => {
    request.resume();
    request.on("end", () => {
      const json = value => { response.writeHead(200, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
      if (request.url.startsWith("/api/session?")) return json({ data: sessions });
      if (request.url === "/api/session/active") return json({ data: ["ses_busy"] });
      response.writeHead(404);
      response.end();
    });
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  return { port: server.address().port, password: "pw" };
}

test("each computer says what its box answers, whole, and which of its sessions are at work; one that never said how many leaves it out", async t => {
  fresh();
  const names = Array.from({ length: 301 }, (_, i) => `feature.${String(i).padStart(3, "0")}`);
  // A newer box: what it answers, and which of its sessions are at work (one that isn't a session's id left out).
  tell(second, { features: ["computers", "sessions.list"], computer: { version: "1.1.0", working: 2, atWork: [{ id: "ses_a1", title: "Ship the release" }, { id: "ses_b2" }, { id: "not a session" }] } });
  // An older box: its version, but nothing of what's at work; and more names than a peer keeps.
  tell(third, { features: names, computer: { version: "0.9.180" } });
  const target = await engine(t);
  const own = link.engineHandler(() => target, { name: "devbox" });
  const pieces = [];
  for await (const piece of own({ method: "GET", path: "kuvo/computers", headers: {}, fromApp: true }, undefined)) pieces.push(piece);
  const computers = Object.fromEntries(JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString()).computers.map(each => [each.here ? "here" : each.name, each]));

  // This box: what it answers, as its `kuvo/info` says, and its own session at work by title.
  assert.ok(computers.here.features.includes("computers.facts") && computers.here.features.includes("computers.manage"));
  assert.deepEqual([computers.here.working, computers.here.atWork], [1, [{ id: "ses_busy", title: "Fix the login page" }]]);
  assert.deepEqual([computers.second.features, computers.second.working, computers.second.atWork], [["computers", "sessions.list"], 2, [{ id: "ses_a1", title: "Ship the release" }, { id: "ses_b2" }]]);
  // Nothing said of what's at work there, or what it answers past the names a peer keeps: left out, not none.
  assert.deepEqual([computers.third.version, "working" in computers.third, "atWork" in computers.third, "features" in computers.third], ["0.9.180", false, false, false]);
  // The Mac never said anything of itself yet.
  assert.deepEqual(["working" in computers.Studio, "features" in computers.Studio], [false, false]);

  // Keep the last work list through a brief interruption, then clear it when the computer stays away.
  fs.writeFileSync(path.join(run, "peer-reach.json"), JSON.stringify({ [second.pub]: Date.now() }));
  const away = (await listed()).second;
  assert.deepEqual([away.reach, away.working, away.atWork], ["away", 2, computers.second.atWork]);
  fs.writeFileSync(path.join(run, "peer-reach.json"), JSON.stringify({ [second.pub]: Date.now() - 3 * 60_000 }));
  const gone = (await listed()).second;
  assert.deepEqual([gone.reach, gone.working, gone.atWork], ["away", 0, []]);

  // What it tells its peers of itself: which of its sessions are at work, too.
  const sent = [];
  await link.announce({ send: bytes => sent.push(bytes) }, () => target, link.readLink(), Date.now(), true);
  const toSecond = sent.find(bytes => k(bytes.subarray(1, 17)) === second.address);
  assert.deepEqual(opened(toSecond, second).agents.computer.atWork, [{ id: "ses_busy", title: "Fix the login page" }]);
});

test("a machine offers renaming and removing your machines where the Mac that writes their records takes it, and hands what you do to that Mac", async () => {
  fresh();
  tell(mac, { features: MAC_TAKES, computer: { version: "1.1.0", working: 0 } });
  tell(second, { features: MAC_TAKES, computer: { version: "1.1.0", working: 0 } });
  let computers = await listed();
  // A machine: renamed, or removed from another computer; a Mac goes by its own name.
  assert.deepEqual(ids(computers.here), ["rename"]);
  assert.deepEqual(ids(computers.second), ["rename", "remove"]);
  assert.deepEqual(ids(computers.Studio), []);
  const remove = computers.second.actions.find(action => action.id === "remove");
  assert.deepEqual(remove, { id: "remove", label: "Remove", confirm: { title: "Remove second?", message: "Your other computers stop reaching second. It keeps running, with its sessions, and you can add it again later.", label: "Remove" } });

  // Renamed from here: handed to the Mac, which writes it, and answered as it said.
  const asked = ask("PATCH", `kuvo/computers/${second.pub}`, { name: "  Build\nbox  " });
  const left = await eventually(() => { try { return fs.readdirSync(path.join(run, "work-queue", "forward"), { withFileTypes: true }).find(each => each.name.endsWith(".json")); } catch { return null; } });
  const sent = [];
  link.sendQueueForwards({ welcomed: true, send: bytes => sent.push(bytes) }, link.readLink(), Date.now(), { ownOnly: false });
  assert.equal(sent.length, 1);
  assert.equal(k(sent[0].subarray(1, 17)), mac.address, "to the Mac alone");
  const { act } = opened(sent[0], mac).workQueue;
  assert.deepEqual({ ...act, id: undefined, at: undefined }, { id: undefined, at: undefined, area: "computers", method: "PATCH", item: second.pub, body: { name: "Build box" } });
  assert.equal(left.name, `${act.id}.json`);
  link.receiveMail(mailFrom(mac, { workQueue: { acted: { id: act.id, status: 200, value: { name: "Build box" } } }, at: Date.now() }), undefined, takeMail);
  assert.deepEqual(await asked, { status: 200, body: { name: "Build box" } });

  // The Mac's record names it so: listed by it here, and this machine by its own new name too.
  const mine = { ...mac.peer, name: "Studio" };
  link.writeLink({ ...link.readLink(), legacy: { [mac.pub]: { at: 2, peers: [mine, { ...here.peer, name: "Garage" }, { ...second.peer, name: "Build box" }, third.peer], moved: [] } } });
  computers = await listed();
  assert.deepEqual([computers.here?.name, computers["Build box"]?.key], ["Garage", second.pub]);

  // Not a name: refused here, nothing sent.
  assert.deepEqual(await ask("PATCH", `kuvo/computers/${second.pub}`, { name: " " }), { status: 400, body: { message: "Give it a name of up to 100 characters." } });
  assert.equal((await ask("PATCH", `kuvo/computers/${second.pub}`, { name: "x".repeat(101) })).status, 400);
  // Not one of yours, or not what it offers: said so.
  const missing = box("machine").pub;
  const removing = ask("DELETE", `kuvo/computers/${missing}`);
  await eventually(() => {
    const forwards = path.join(run, "work-queue", "forward");
    return fs.existsSync(forwards) && fs.readdirSync(forwards).some(file => file.endsWith(".json"));
  });
  sent.length = 0;
  link.sendQueueForwards({ welcomed: true, send: bytes => sent.push(bytes) }, link.readLink(), Date.now(), { ownOnly: false });
  assert.equal(sent.length, 1);
  const retry = opened(sent[0], mac).workQueue.act;
  assert.deepEqual({ ...retry, id: undefined, at: undefined }, { id: undefined, at: undefined, area: "computers", method: "DELETE", item: missing, body: { cleanup: true } });
  link.receiveMail(mailFrom(mac, { workQueue: { acted: { id: retry.id, status: 404, value: { message: "That isn't one of your computers." } } }, at: Date.now() }), undefined, takeMail);
  assert.equal((await removing).status, 404);
  assert.deepEqual(await ask("DELETE", `kuvo/computers/${here.pub}`), { status: 409, body: { message: "Garage can't remove itself: remove it from another of your computers." } });
  assert.deepEqual(await ask("PATCH", `kuvo/computers/${mac.pub}`, { name: "Desk" }), { status: 409, body: { message: "Studio is a Mac: it goes by the name it has there." } });
  assert.equal((await ask("POST", `kuvo/computers/${second.pub}`)).status, 405);
});

test("what you do to a machine can't reach the Mac that writes its record: refused in words, nothing done", async () => {
  fresh();
  tell(mac, { features: MAC_TAKES, computer: { version: "1.1.0", working: 0 } });
  // The Mac isn't answering now.
  fs.writeFileSync(path.join(run, "peer-reach.json"), JSON.stringify({ [mac.pub]: Date.now() }));
  const refused = await ask("DELETE", `kuvo/computers/${second.pub}`);
  assert.equal(refused.status, 503);
  assert.match(refused.body.message, /^Studio isn't answering now \(it may be asleep, off or offline\), so nothing was changed there\. Try again once it's back\.$/);
  assert.ok(!fs.existsSync(path.join(run, "work-queue", "forward")) || !fs.readdirSync(path.join(run, "work-queue", "forward")).some(name => name.endsWith(".json")), "nothing left to send");

  // A Mac from before: renaming and removing aren't offered, and asked anyway, it says why.
  fs.rmSync(path.join(run, "peer-reach.json"));
  tell(mac, { features: ["computers", "work-queue.forward"], computer: { version: "1.0.0", working: 0 } });
  assert.deepEqual(ids((await listed()).second), []);
  assert.deepEqual(await ask("PATCH", `kuvo/computers/${second.pub}`, { name: "Build box" }), { status: 409, body: { message: "Studio needs a newer Kuvo Code to rename your machines. Update it, then try again." } });

  // A machine two of your Macs set up, in no setup yet: every computer either Mac names is listed
  // (#783), and with no one Mac writing its record, rename and remove aren't offered here.
  const other = box("mac", "Air");
  fresh({ macs: [mac.pub, other.pub], legacy: { [mac.pub]: { at: 1, peers: [mac.peer, here.peer, second.peer] }, [other.pub]: { at: 1, peers: [other.peer, here.peer] } } });
  const computers = await listed();
  assert.deepEqual(Object.keys(computers).sort(), ["Air", "Studio", "here", "second"]);
  assert.deepEqual(ids(computers.here), []);
  assert.equal(computers.second.apart, undefined);
  // In Studio's setup too: Studio writes the records of its own machines, never of Air's, which Air
  // keeps: Rename and Remove are offered for Studio's alone.
  const fourth = box("machine", "fourth");
  fresh({ macs: [mac.pub, other.pub], setup: { id: "set_AAAAAAAAAAAAAAAAAAAAAA", epoch: 1, primary: mac.pub, sign: mac.pub, role: "member" },
    legacy: { [mac.pub]: { at: 1, peers: [mac.peer, here.peer, second.peer] }, [other.pub]: { at: 1, peers: [other.peer, here.peer, fourth.peer] } } });
  tell(mac, { features: MAC_TAKES, computer: { version: "1.1.0", working: 0 } });
  const both = await listed();
  assert.deepEqual([ids(both.second), ids(both.fourth)], [["rename", "remove"], []]);
});

test("while the old app is attached to the Mac that writes your machines' records, a machine's Remove says to quit it first, as that Mac told; not while that Mac is away", async () => {
  fresh();
  tell(mac, { features: MAC_TAKES, computer: { version: "1.1.0", working: 0, oldApp: true } });
  let remove = (await listed()).second.actions.find(action => action.id === "remove");
  assert.deepEqual(remove, { id: "remove", label: "Remove", why: "Quit the old Kuvo Code app on Studio first: it keeps its own list of machines, and would add second back." });
  assert.deepEqual(await ask("DELETE", `kuvo/computers/${second.pub}`), { status: 409, body: { message: remove.why } });
  // The Mac away: that's what's said, not what it last said of its app.
  fs.writeFileSync(path.join(run, "peer-reach.json"), JSON.stringify({ [mac.pub]: Date.now() }));
  remove = (await listed()).second.actions.find(action => action.id === "remove");
  assert.equal(remove.why, undefined);
  assert.equal((await ask("DELETE", `kuvo/computers/${second.pub}`)).status, 503);
});

test("Connect, for a computer that's away: this box tells it its agents and asks for theirs, says it's being reached, and its answer brings it back", async t => {
  fresh();
  tell(second, { features: MAC_TAKES, computer: { version: "1.1.0", working: 1 } });
  fs.writeFileSync(path.join(run, "peer-reach.json"), JSON.stringify({ [second.pub]: Date.now() - 60_000 }));
  let computers = await listed();
  assert.equal(computers.second.reach, "away");
  assert.ok(ids(computers.second).includes("connect"));
  assert.ok(!ids(computers.Studio).includes("connect"), "one heard from isn't offered it");
  assert.equal((await ask("POST", `kuvo/computers/${mac.pub}/connect`)).status, 409);

  assert.deepEqual(await ask("POST", `kuvo/computers/${second.pub}/connect`), { status: 202, body: {} });
  // Off the relay itself, it can't try: said so, rather than seeming to.
  fs.rmSync(path.join(run, "reach-again.json"));
  fs.writeFileSync(path.join(run, "relay-down"), "");
  assert.deepEqual(await ask("POST", `kuvo/computers/${second.pub}/connect`), { status: 503, body: { message: "devbox isn't connected to your other computers now, so it can't reach second. Try again in a moment." } });
  fs.rmSync(path.join(run, "relay-down"));
  assert.equal((await ask("POST", `kuvo/computers/${second.pub}/connect`)).status, 202);
  const asked = link.readTries()[second.pub];
  computers = await listed();
  assert.deepEqual([computers.second.reach, ids(computers.second).includes("connect")], ["connecting", false]);
  assert.equal((await ask("POST", `kuvo/computers/${second.pub}/connect`)).status, 409, "asked already");

  // The link tells it, once, asking for its agents back.
  const target = await engine(t);
  const told = [];
  const send = key => link.announce({ send: bytes => told.push(bytes) }, () => target, link.readLink(), Date.now(), false, key);
  const tells = [];
  link.sendReachAgain(true, key => tells.push(send(key)));
  link.sendReachAgain(true, key => tells.push(send(key)));
  await Promise.all(tells);
  assert.equal(told.length, 1);
  assert.equal(k(told[0].subarray(1, 17)), second.address, "to that computer alone");
  assert.equal(opened(told[0], second).ask, "agents");
  // Off the relay, nothing goes (the next round sends it).
  link.sendReachAgain(false, () => assert.fail("sent off the relay"));

  // No answer for the length of a try: away again, and offered again.
  const later = await link.listComputers("devbox", null, undefined, asked + link.TRY_FOR + 1);
  const again = later.find(each => each.key === second.pub);
  assert.deepEqual([again.reach, again.actions.map(action => action.id).includes("connect")], ["away", true]);

  // It answers: back.
  link.receiveMail(mailFrom(second, { at: Date.now(), agents: { at: Date.now() + 1, list: [], features: MAC_TAKES, computer: { version: "1.1.0", working: 1 } } }), undefined, takeMail);
  computers = await listed();
  assert.deepEqual([computers.second.reach, computers.second.working, ids(computers.second).includes("connect")], ["connected", 1, false]);
});

// Where a client of your own reaches each computer itself (`computers.reach`, #742): the desktop
// app connects to every box directly, so each of your own computers says its box's address on the
// relay and the relay it's reached through: every computer, one an earlier Kuvo Code called shared too.
test("each peer says where its box is reached", async () => {
  fresh();
  const hereAddress = k(link.addressOf(Buffer.from(link.readLink().secret, "base64url")));
  tell(mac, { features: MAC_TAKES, computer: { version: "1.1.0", working: 0 } });
  tell(second, { features: MAC_TAKES, computer: { version: "1.1.0", working: 0 } });
  // A machine another of your Macs set up too, as an earlier Kuvo Code says it.
  tell(third, { features: MAC_TAKES, shared: true });
  let computers = await listed();
  const reach = computer => [computer.address, computer.relay];
  assert.deepEqual(reach(computers.here), [hereAddress, "wss://relay.test"]);
  assert.deepEqual(reach(computers.Studio), [mac.address, "wss://relay.test"]);
  assert.deepEqual(reach(computers.second), [second.address, "wss://relay.test"]);
  assert.equal(computers.third.apart, undefined);
  assert.deepEqual(reach(computers.third), [third.address, "wss://relay.test"]);

  // A box with no relay knows no way in to say.
  link.writeLink({ ...link.readLink(), relay: undefined });
  computers = await listed();
  assert.deepEqual([reach(computers.here), reach(computers.second)], [[undefined, undefined], [undefined, undefined]]);
});
