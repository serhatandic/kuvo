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

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-computers-mac-")));
removeAtEnd(home);
process.env.KUVO_HOME = home;
process.env.KUVO_BOX = "mac";
process.env.KUVO_ENGINE_CONFIG = path.join(home, "engine");
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
