// Renaming and removing your machines on the Mac that writes their records (PROTOCOL.md,
// "Computers", Managing them, #735): asked by a client here, or handed here by another of your
// computers' boxes; one writer, so two boxes renaming at once never lose either, and the last of
// two names for one machine is the one every computer takes. The real box code, as a Mac's box;
// the app's list of machines a file it left.
//
// Run with `node --test box/bridge-tests/computers-mac.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

// (The box's folder inside its app's, as on a Mac: the app's own files, `phone-link.json`, beside it.)
const app = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-computers-mac-")));
removeAtEnd(app);
const home = path.join(app, "box");
fs.mkdirSync(home);
process.env.KUVO_HOME = home;
process.env.KUVO_BOX = "mac";
process.env.KUVO_ENGINE_CONFIG = path.join(app, "engine");
const link = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");

const run = path.join(home, "run");
const k = key => key.toString("base64url");
const box = (kind, id) => {
  const key = link.newKey();
  const pub = k(link.publicOf(key));
  return { key, pub, peer: { key: pub, address: k(link.addressOf(crypto.randomBytes(32))), sign: k(crypto.randomBytes(32)), name: id, kind, id } };
};
const mac = link.newKey();
const devbox = box("machine", "devbox"), mini = box("machine", "mini"), attic = box("machine", "attic");
const fresh = () => {
  fs.rmSync(run, { recursive: true, force: true });
  fs.mkdirSync(run, { recursive: true });
  link.forgetQueueActs();
  // (Its keys from the app, kept in memory only, as the app gives them.)
  link.macLink.overlay = { key: k(mac), secret: k(crypto.randomBytes(32)), sign: link.newSigningSeed(), relay: "wss://relay.test", name: "Studio" };
  link.writeLink({ phones: [] });
  // (What its app says of it: its name, and the relay.)
  link.macLink.config = { phones: true, machines: true, relay: "wss://relay.test", name: "Studio", app: "1.0.0" };
  link.updateLink(state => link.setMacPeers(state, { peers: [devbox.peer, mini.peer, attic.peer], moved: [] }).link);
  // The app's list of machines, as it keeps it.
  fs.writeFileSync(path.join(home, "machines.json"), JSON.stringify([{ destination: "devbox" }, { destination: "mini" }, { destination: "attic" }]));
};
const names = () => link.macPeersBody(link.readLink()).boxes.filter(each => each.kind === "machine").map(each => each.name);
// What another of your computers' box hands here, as its mail brings it; and what this box answered.
const handed = (from, act) => {
  const answers = [];
  const done = link.takeQueueAct(from.peer, { id: `wqa_${crypto.randomBytes(16).toString("base64url")}`, at: Date.now(), area: "computers", ...act }, reply => answers.push(reply.acted), queued => link.writerAct(from.peer, queued));
  return done.then(() => answers);
};
const ask = async (method, route, body) => {
  const handle = link.engineHandler(() => ({ port: 1, password: "pw" }), { name: "Studio" });
  const pieces = [];
  for await (const piece of handle({ method, path: route, headers: {}, fromApp: true, ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) }, undefined)) pieces.push(piece);
  return { status: pieces.find(each => each.head).head.status, body: JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString() || "{}") };
};

test("this Mac writes its machines' records: it offers renaming and removing them, and does it itself, its peers document naming them so", async () => {
  fresh();
  const listed = await link.listComputers("Studio", null);
  assert.deepEqual(listed.map(each => [each.name, each.actions.map(action => action.id)]), [["Studio", []], ["attic", ["rename", "remove"]], ["devbox", ["rename", "remove"]], ["mini", ["rename", "remove"]]]);
  assert.deepEqual(await ask("PATCH", `kuvo/computers/${devbox.pub}`, { name: "Build box" }), { status: 200, body: { name: "Build box" } });
  assert.deepEqual(names(), ["Build box", "mini", "attic"]);
  // The app saying its machines again keeps the name you gave.
  link.updateLink(state => link.setMacPeers(state, { peers: [devbox.peer, mini.peer, attic.peer], moved: [] }).link);
  assert.deepEqual(names(), ["Build box", "mini", "attic"]);

  assert.deepEqual(await ask("DELETE", `kuvo/computers/${attic.pub}`), { status: 200, body: {} });
  const body = link.macPeersBody(link.readLink());
  assert.deepEqual([names(), body.left], [["Build box", "mini"], [attic.pub]]);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(home, "machines.json"), "utf8")).map(each => each.destination), ["devbox", "mini"], "off this Mac's list of machines too");
  assert.equal((await ask("DELETE", `kuvo/computers/${attic.pub}`)).status, 404, "gone already");
});

test("two of your computers rename machines at once: this Mac does each in turn, so neither is lost, and of two names for one machine the last is kept", async () => {
  fresh();
  // Each a different machine: both kept.
  const [one, other] = await Promise.all([handed(devbox, { method: "PATCH", item: mini.pub, body: { name: "Mini" } }), handed(mini, { method: "PATCH", item: attic.pub, body: { name: "Attic" } })]);
  assert.deepEqual([one[0].status, other[0].status], [200, 200]);
  assert.deepEqual(names(), ["devbox", "Mini", "Attic"]);
  // The same machine: the last one done is its name, on every computer the document reaches.
  await Promise.all([handed(devbox, { method: "PATCH", item: attic.pub, body: { name: "First" } }), handed(mini, { method: "PATCH", item: attic.pub, body: { name: "Second" } })]);
  assert.deepEqual(names(), ["devbox", "Mini", "Second"]);
  // One handed again (its mail sent twice) is done once, and answered as it was.
  const act = { id: `wqa_${crypto.randomBytes(16).toString("base64url")}`, at: Date.now(), area: "computers", method: "DELETE", item: mini.pub };
  const answers = [];
  const take = () => link.takeQueueAct(devbox.peer, act, reply => answers.push(reply.acted), queued => link.writerAct(devbox.peer, queued), Date.now() + 10_000);
  await take();
  await take();
  assert.deepEqual(answers.map(each => each.status), [200, 200]);
  assert.deepEqual(names(), ["devbox", "Second"]);
  // Not a name, or not one of this Mac's machines: said so, nothing done.
  const [unnamed] = await handed(devbox, { method: "PATCH", item: attic.pub, body: { name: "" } });
  assert.deepEqual([unnamed.status, unnamed.value], [400, { message: "Give it a name of up to 100 characters." }]);
  assert.deepEqual(names(), ["devbox", "Second"]);
  assert.equal((await handed(devbox, { method: "DELETE", item: box("machine", "x").pub }))[0].status, 404);
});

test("a machine removed from any of your computers stays removed when this Mac's app says its machines again or sets it up as it connects, until it's added again; a name the app changes is the app's", async () => {
  fresh();
  assert.equal((await handed(devbox, { method: "DELETE", item: attic.pub }))[0].status, 200);
  // The app, as it was, still lists it: it isn't brought back, and is still among those removed.
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [devbox.peer, mini.peer, attic.peer], moved: [] })).status, 200);
  assert.deepEqual([names(), link.macPeersBody(link.readLink()).left], [["devbox", "mini"], [attic.pub]]);
  // The app setting it up as it connects (its key named, as it always is) doesn't bring it back either.
  assert.equal((await ask("GET", `kuvo/link/setup?key=${attic.pub}`)).status, 200);
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [devbox.peer, mini.peer, attic.peer], moved: [] })).status, 200);
  assert.deepEqual(names(), ["devbox", "mini"]);
  // Added again (back on this Mac's list of machines): it's one of this Mac's machines again.
  fs.writeFileSync(path.join(home, "machines.json"), JSON.stringify([{ destination: "devbox" }, { destination: "mini" }, { destination: "attic" }]));
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [devbox.peer, mini.peer, attic.peer], moved: [] })).status, 200);
  assert.deepEqual(names(), ["devbox", "mini", "attic"]);
  // Removed again later, and it's removed again.
  assert.equal((await handed(devbox, { method: "DELETE", item: attic.pub }))[0].status, 200);
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [devbox.peer, mini.peer, attic.peer], moved: [] })).status, 200);
  assert.deepEqual(names(), ["devbox", "mini"]);

  // Renamed here, and missing from the app's list for a moment (it couldn't be set up just then):
  // the name you gave it stays.
  assert.equal((await handed(mini, { method: "PATCH", item: mini.pub, body: { name: "Mini" } }))[0].status, 200);
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [devbox.peer, attic.peer], moved: [] })).status, 200);
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [devbox.peer, mini.peer, attic.peer], moved: [] })).status, 200);
  assert.deepEqual(names(), ["devbox", "Mini"]);
  // Renamed here, then renamed in the app: the app's name is kept from then on.
  assert.equal((await handed(mini, { method: "PATCH", item: devbox.pub, body: { name: "Build box" } }))[0].status, 200);
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [{ ...devbox.peer, name: "CI" }, mini.peer, attic.peer], moved: [] })).status, 200);
  assert.deepEqual(names(), ["CI", "Mini"]);
});

test("a rename or remove this Mac took and was cut short by a restart is done again when it's asked again, never left in doubt", async () => {
  fresh();
  const id = `wqa_${crypto.randomBytes(16).toString("base64url")}`;
  // As a box that stopped mid-act leaves it: taken, with no answer.
  fs.mkdirSync(path.join(run, "work-queue"), { recursive: true });
  fs.writeFileSync(path.join(run, "work-queue", "acted.json"), JSON.stringify({ [`${devbox.pub} ${id}`]: { until: Date.now() + 600_000, answer: null } }));
  const answers = [];
  await link.takeQueueAct(devbox.peer, { id, at: Date.now() - 900_000, area: "computers", method: "DELETE", item: mini.pub }, reply => answers.push(reply.acted), queued => link.writerAct(devbox.peer, queued));
  assert.deepEqual(answers.map(each => each.status), [200]);
  assert.deepEqual(names(), ["devbox", "attic"]);
  // Removed already when it's done again: the same answer.
  assert.deepEqual(link.removeHere(mini.pub), { status: 200, value: {} });
});

test("a machine this Mac set up that another setup shares too is still this Mac's to rename and remove", async () => {
  fresh();
  // It says it's shared with another setup's Mac.
  link.receiveMail(link.sealOneWay(Buffer.from(JSON.stringify({ at: Date.now(), agents: { at: Date.now(), list: [], shared: true } })), devbox.key, link.publicOf(mac)).message, undefined, (sender, payload) => link.peerMail(sender, payload, null));
  const shared = (await link.listComputers("Studio", null)).find(each => each.key === devbox.pub);
  assert.deepEqual([shared.apart, shared.actions.map(action => action.id)], [true, ["rename", "remove"]]);
  assert.deepEqual(await ask("DELETE", `kuvo/computers/${devbox.pub}`), { status: 200, body: {} });
});

// The old Kuvo Code app attached to this box, as it is while it's open: it follows the box's link.
async function oldAppOpens() {
  const handle = link.engineHandler(() => ({ port: 1, password: "pw" }), { name: "Studio" });
  const closes = new AbortController();
  const following = handle({ method: "GET", path: "kuvo/link/status/events", headers: {}, fromApp: true }, undefined, closes.signal)[Symbol.asyncIterator]();
  await following.next();
  await following.next();
  assert.equal(link.macLink.statusFollowers.size, 1);
  return async () => {
    closes.abort();
    await following.return();
    assert.equal(link.macLink.statusFollowers.size, 0);
  };
}

test("while the old Kuvo Code app is attached to this Mac's box, removing a machine isn't offered or taken, from here or another computer, saying why; once it's quit, a removal holds when the app opens again", async () => {
  fresh();
  const quit = await oldAppOpens();
  const why = "Quit the old Kuvo Code app on Studio first: it keeps its own list of machines, and would add attic back.";
  const listed = (await link.listComputers("Studio", null)).find(each => each.key === attic.pub);
  // Renaming holds (the app keeps names it didn't change); Remove says why it waits.
  assert.deepEqual(listed.actions, [{ id: "rename", label: "Rename" }, { id: "remove", label: "Remove", why }]);
  assert.deepEqual(await ask("DELETE", `kuvo/computers/${attic.pub}`), { status: 409, body: { message: why } });
  const [forwarded] = await handed(devbox, { method: "DELETE", item: attic.pub });
  assert.deepEqual([forwarded.status, forwarded.value], [409, { message: why }]);
  // What it tells your other computers of itself says so, so they don't offer it either.
  assert.equal(link.ownComputer().oldApp, true);
  assert.deepEqual(link.keptComputer({ working: 0, oldApp: true }), { working: 0, oldApp: true });
  // The app saves its list again and says its machines: all of them still there, nothing undone.
  fs.writeFileSync(path.join(home, "machines.json"), JSON.stringify([{ destination: "devbox" }, { destination: "mini" }, { destination: "attic" }]));
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [devbox.peer, mini.peer, attic.peer], moved: [] })).status, 200);
  assert.deepEqual(names(), ["devbox", "mini", "attic"]);

  // Attaching again, it says its link's config first: attached from then, before it follows.
  await quit();
  assert.equal((await ask("PUT", "kuvo/link/config", { phones: true, machines: true, relay: "wss://relay.test", name: "Studio", app: "0.9.200" })).status, 200);
  assert.deepEqual(await ask("DELETE", `kuvo/computers/${attic.pub}`), { status: 409, body: { message: why } });
  link.macLink.appSaid = undefined;
  await (await oldAppOpens())();
  // Quit: removed now; and the app opening again saves the list it reads (without it) and says the
  // machines it still keeps bridges for (with it): it stays removed.
  assert.deepEqual(await ask("DELETE", `kuvo/computers/${attic.pub}`), { status: 200, body: {} });
  const reopened = await oldAppOpens();
  fs.writeFileSync(path.join(home, "machines.json"), fs.readFileSync(path.join(home, "machines.json")));
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [devbox.peer, mini.peer, attic.peer], moved: [] })).status, 200);
  assert.deepEqual(names(), ["devbox", "mini"]);
  await reopened();
});

test("a machine's other computers can't remove or rename one shared with another setup through this Mac: only this Mac itself can", async () => {
  fresh();
  link.receiveMail(link.sealOneWay(Buffer.from(JSON.stringify({ at: Date.now(), agents: { at: Date.now(), list: [], shared: true } })), devbox.key, link.publicOf(mac)).message, undefined, (sender, payload) => link.peerMail(sender, payload, null));
  const [removed] = await handed(mini, { method: "DELETE", item: devbox.pub });
  assert.deepEqual([removed.status, removed.value], [409, { message: "devbox is shared with another setup: only Studio itself can remove it." }]);
  const [renamed] = await handed(mini, { method: "PATCH", item: devbox.pub, body: { name: "Shared" } });
  assert.equal(renamed.status, 409);
  assert.deepEqual(names(), ["devbox", "mini", "attic"]);
});

test("a name you gave a machine is the one your phones are given for it too", async () => {
  fresh();
  // What the app keeps of your machines' bridges, as it wrote it.
  fs.writeFileSync(path.join(app, "phone-link.json"), JSON.stringify({ machines: { devbox: { name: "devbox", address: devbox.peer.address, key: devbox.pub } } }));
  assert.equal((await ask("PATCH", `kuvo/computers/${devbox.pub}`, { name: "Build box" })).status, 200);
  const front = link.macFront(link.engineHandler(() => ({ port: 1, password: "pw" }), { name: "Studio" }), { wait: 10, look: 60_000 });
  const pieces = [];
  for await (const piece of front({ method: "GET", path: "kuvo/hosts", headers: {} }, undefined)) pieces.push(piece);
  const hosts = JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString()).hosts;
  assert.deepEqual(hosts.map(each => [each.machine, each.name]), [["devbox", "Build box"]]);
});

test("removing a computer takes Kuvo Code's app on it off your devices, however it's removed: every device its box added, on every computer, and no other (#753)", async () => {
  fresh();
  link.macLink.appSaid = undefined;
  const device = () => k(link.publicOf(link.newKey()));
  const phone = device(), own = device(), atticOne = device(), atticTwo = device(), miniOne = device(), devboxOne = device();
  link.updateLink(state => {
    link.pairPhone(state, { key: phone, name: "My iPhone", added: 1, from: "pair", pairing: "its-own" });
    // (Kuvo Code's app beside this box, and those your machines' boxes vouched for: `by`.)
    link.pairDevice(state, { key: own, name: "Studio screen" });
    link.pairDevice(state, { key: atticOne, name: "Attic screen", by: attic.pub });
    link.pairDevice(state, { key: atticTwo, name: "Attic laptop", by: attic.pub });
    link.pairDevice(state, { key: miniOne, name: "Mini screen", by: mini.pub });
    link.pairDevice(state, { key: devboxOne, name: "Devbox screen", by: devbox.pub });
    return state;
  });
  const devices = async () => (await ask("GET", "kuvo/phones")).body.phones.map(each => each.name).sort();
  const pairingOf = key => link.readLink().phones.find(each => each.key === key).pairing;
  const removals = () => link.ownPhones(link.readLink()).removed;
  // Its confirm says so before you remove it, where its app is one of your devices.
  const confirm = async (computer, id = "remove") => (await link.listComputers("Studio", null)).find(each => each.key === computer.pub).actions.find(action => action.id === id).confirm.message;
  assert.equal(await confirm(attic), "Your other computers stop reaching attic. Kuvo Code's app on attic is taken off your devices. It keeps running, with its sessions, and you can add it again later.");

  // Removed from this Mac: both devices its box added go, noted among the removals by the pairing
  // each had, which this box's phones document carries to your other computers.
  const gone = [atticOne, atticTwo].map(key => ({ key, pairing: pairingOf(key) }));
  assert.deepEqual(await ask("DELETE", `kuvo/computers/${attic.pub}`), { status: 200, body: {} });
  assert.deepEqual(await devices(), ["Devbox screen", "Mini screen", "My iPhone", "Studio screen"]);
  assert.deepEqual(removals(), gone);
  // Removed as any device is: asking again pairs neither, whichever computer vouches.
  assert.deepEqual(link.pairDevice(link.readLink(), { key: atticOne, name: "Attic screen", by: mini.pub }), { removed: true });
  // No computer is kept from adding its app by it: one that's removed adds none anyway.
  assert.equal(link.readLink().vouchBlocked, undefined);

  // Removed from another of your computers, which hands it here: the same.
  assert.equal((await handed(devbox, { method: "DELETE", item: mini.pub }))[0].status, 200);
  assert.deepEqual(await devices(), ["Devbox screen", "My iPhone", "Studio screen"]);
  // And as the Mac app before 1.0 removes one (its machines said again, with the one it removed).
  assert.equal((await ask("PUT", "kuvo/link/peers", { peers: [], moved: [], removed: [devbox.pub] })).status, 200);
  assert.deepEqual(await devices(), ["My iPhone", "Studio screen"], "the phone, and the app this Mac paired itself, stay");
  assert.deepEqual(removals().map(each => each.key), [atticOne, atticTwo, miniOne, devboxOne]);
});

test("a computer removed before its devices went with it loses them when it's removed again (#753)", async () => {
  fresh();
  link.macLink.appSaid = undefined;
  const left = k(link.publicOf(link.newKey()));
  // As a box before this left it: the computer gone from its machines, the device it added still in.
  link.updateLink(state => {
    link.pairDevice(state, { key: left, name: "Attic screen", by: attic.pub });
    return { ...link.setMacPeers(state, { peers: [devbox.peer, mini.peer], moved: [] }).link, removedHere: [attic.pub] };
  });
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones.map(each => each.name), ["Attic screen"]);
  assert.deepEqual(link.removeHere(attic.pub), { status: 200, value: {} });
  assert.deepEqual((await ask("GET", "kuvo/phones")).body.phones, []);
  assert.deepEqual(link.ownPhones(link.readLink()).removed.map(each => each.key), [left]);
});
