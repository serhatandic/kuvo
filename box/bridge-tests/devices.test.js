// Kuvo Code's desktop app as one of your devices, on a machine's box (PROTOCOL.md, "Paired phones",
// `kuvo/devices/pair`; "Sessions", `client`): the app beside a box that pairs phones is paired at
// once, with no link; beside a member of a setup, its box vouches for it to the main computer and
// answers with what that one said; a removed app stays out; and only the app on this computer asks
// (never a phone, the relay or `box route`). A desktop device is listed as one, carried in the
// phones documents, sent no pushes, and what it writes isn't marked as a phone's. The real box
// code, as a machine's box; only the engine and the relay are stand-ins. (The main computer's
// side, and two real boxes: devices-two-boxes.test.js.)
//
// Run with `node --test box/bridge-tests/devices.test.js`.
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

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-devices-")));
removeAtEnd(home);
process.env.KUVO_HOME = home;
const link = await import(LINK_BUNDLE.href);

const run = path.join(home, "run");
const forwards = path.join(run, "work-queue", "forward");
const k = key => key.toString("base64url");
const box = (kind, name) => {
  const key = link.newKey();
  const address = k(link.addressOf(crypto.randomBytes(32)));
  return { key, pub: k(link.publicOf(key)), address, peer: { key: k(link.publicOf(key)), address, name, kind } };
};
const here = box("machine", "devbox"), mac = box("mac", "Studio"), other = box("mac", "Their Mac");
const device = () => k(link.publicOf(link.newKey()));
// A box no Mac set up, with a relay: one that pairs phones by itself.
const alone = (extra = {}) => {
  fs.rmSync(run, { recursive: true, force: true });
  fs.mkdirSync(run, { recursive: true });
  link.forgetQueueActs();
  link.writeLink({ secret: k(crypto.randomBytes(32)), key: k(here.key), relay: "wss://relay.test", name: "devbox", phones: [], ...extra });
};
// A member of the setup Studio leads, with its legacy peer list already migrated.
const member = (extra = {}) => alone({ id: "devbox", mac: mac.pub, macs: [mac.pub], legacy: { [mac.pub]: { at: 1, peers: [mac.peer, here.peer], moved: [] } },
  setup: { id: `set_${"a".repeat(22)}`, epoch: 1, primary: mac.pub, sign: "s".repeat(43), role: "member" }, ...extra });
const mailFrom = (from, payload) => link.sealOneWay(Buffer.from(JSON.stringify(payload)), from.key, link.publicOf(here.key)).message;
const takeMail = (sender, payload) => link.peerMail(sender, payload, null);
const tell = (from, agents) => link.receiveMail(mailFrom(from, { at: Date.now(), agents: { at: Date.now(), list: [], ...agents } }), undefined, takeMail);
const opened = (bytes, to) => JSON.parse(link.openOneWay(link.readFrame(bytes.subarray(17)).body, to.key, link.publicOf(here.key)).payload.toString("utf8"));

const ended = [];
const handle = link.engineHandler(() => ({ port: 1, password: "pw" }), { name: "devbox" }, undefined, changed => ended.push((changed.phones ?? []).map(phone => phone.key)));
async function ask(from, method, route, body, phone) {
  const pieces = [];
  for await (const piece of handle({ method, path: route, headers: {}, ...from, ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) }, phone)) pieces.push(piece);
  return { status: pieces.find(each => each.head).head.status, body: JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString() || "{}") };
}
// Who asks: Kuvo Code's app on this computer, over the box's own server (as `serve` marks it); the
// box's CLI (`box route`: a Mac over SSH, or an agent's command here); a client over the relay.
const APP = { served: true, fromMac: true };
const CLI = { fromMac: true };
const RELAY = {};

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
// What this box left for its link to send (another process's, as its server leaves it), as its link sends it.
async function pairSent(to) {
  const left = await eventually(() => { try { return fs.readdirSync(forwards).find(name => /^wqa_[\w-]+\.json$/.test(name)); } catch { return null; } });
  const sent = [];
  link.sendQueueForwards({ welcomed: true, send: bytes => sent.push(bytes) }, link.readLink(), Date.now(), { ownOnly: false });
  assert.equal(sent.length, 1);
  assert.equal(k(sent[0].subarray(1, 17)), to.address, "to the main computer alone");
  const { act } = opened(sent[0], to).workQueue;
  assert.equal(left, `${act.id}.json`);
  return act;
}

test("the app beside a box that pairs phones is paired at once as a desktop device, the same when it asks again, and only the app on this computer may ask", async () => {
  const waiting = { token: "t".repeat(22), expires: Date.now() + 600_000 };
  alone({ pairing: waiting });
  const key = device();
  const given = { key, name: "Studio Display", kind: "desktop" };

  // Not a phone or any client over the relay, nor the box's CLI, which an agent's command could run.
  const refusal = { status: 403, body: { message: "Only Kuvo Code on this computer adds itself to your devices." } };
  assert.deepEqual(await ask(RELAY, "POST", "kuvo/devices/pair", given, { key: crypto.randomBytes(32) }), refusal);
  assert.deepEqual(await ask(RELAY, "POST", "kuvo/devices/pair", given, { key: Buffer.from(key, "base64url"), kind: "desktop" }), refusal);
  assert.deepEqual(await ask(CLI, "POST", "kuvo/devices/pair", given), refusal);
  assert.deepEqual(link.readLink().phones, []);
  assert.equal((await ask(APP, "GET", "kuvo/devices/pair")).status, 405);
  // Not a device: a key that isn't one, this box's own, a kind it doesn't pair this way, a name too long.
  for (const wrong of [{ ...given, key: "short" }, { ...given, key: here.pub }, { ...given, kind: "phone" }, { ...given, name: "x".repeat(101) }, { key, kind: "desktop" }]) {
    assert.equal((await ask(APP, "POST", "kuvo/devices/pair", wrong)).status, 400, JSON.stringify(wrong));
  }

  const paired = await ask(APP, "POST", "kuvo/devices/pair", given);
  assert.equal(paired.status, 200);
  // The same capacity applies to paired devices.
  for (let more = 1; more < 64; more++) assert.equal((await ask(APP, "POST", "kuvo/devices/pair", { ...given, key: device() })).status, 200);
  const full = await ask(APP, "POST", "kuvo/devices/pair", { ...given, key: device() });
  assert.deepEqual([full.status, /as many paired devices/.test(full.body.message)], [409, true]);
  link.updateLink(kept => ({ ...kept, phones: kept.phones.filter(phone => phone.key === key) }));
  assert.deepEqual(paired.body, { paired: true, pairing: paired.body.pairing });
  assert.match(paired.body.pairing, /^[A-Za-z0-9_-]{16}$/);
  const kept = link.readLink();
  assert.deepEqual(kept.phones.map(({ added, ...rest }) => rest), [{ key, name: "Studio Display", from: "pair", pairing: paired.body.pairing, kind: "desktop" }]);
  // Its phones document says what it is, for the boxes that take it; a phone's entry says nothing.
  assert.deepEqual(link.ownPhones({ ...kept, phones: [...kept.phones, { key: device(), name: "iPhone", from: "pair", pairing: "p" }] }).phones.map(each => each.kind), ["desktop", undefined]);
  // The link a phone is being shown still waits: this used no token, and ended none.
  assert.deepEqual(kept.pairing, waiting);
  // It's let in from now on, as a paired phone is.
  assert.equal(link.authorizePhone(Buffer.from(key, "base64url"), { v: 1, client: "desktop" }, undefined), true);

  // Asked again (the app opening again): the pairing it has, nothing new made.
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", { ...given, name: "Renamed" }), paired);
  assert.equal(link.readLink().phones.length, 1);

  // Listed for every client with what it is.
  const phone = crypto.randomBytes(32);
  link.updateLink(state => { link.pairPhone(state, { key: k(phone), name: "My iPhone", added: 5, from: "pair", pairing: "ph" }); return state; });
  const listed = await ask(RELAY, "GET", "kuvo/phones", undefined, { key: phone });
  assert.deepEqual(listed.body.phones.map(each => [each.name, each.kind, each.you]), [["Studio Display", "desktop", false], ["My iPhone", "phone", true]]);

  // Nobody vouched for it (this box's own app asked): removing it keeps its key out, and no computer.
  assert.equal(kept.phones[0].by, undefined);
  assert.deepEqual(await ask(RELAY, "POST", "kuvo/phones/remove", { phones: [{ key }] }, { key: phone }), { status: 200, body: {} });
  assert.equal(link.readLink().vouchBlocked, undefined);
  // The old voucher route no longer changes access.
  assert.equal((await ask(CLI, "POST", `kuvo/computers/${here.pub}/allow-devices`)).status, 404);
  assert.equal((await ask(RELAY, "POST", `kuvo/computers/${here.pub}/allow-devices`, undefined, { key: crypto.randomBytes(32) })).status, 404, "a device that isn't paired here");
  assert.equal((await ask(APP, "POST", `kuvo/computers/${here.pub}/allow-devices`)).status, 404);
});

test("an app removed from your devices stays out: this never pairs it again, and Kuvo Code on this computer removes a device as a paired one does, never a Mac", async () => {
  member();
  const phone = crypto.randomBytes(32);
  const key = device();
  link.updateLink(state => {
    state.phones = [{ key: k(phone), name: "My iPhone", added: 5, from: "mac", mac: mac.pub, pairing: "ph" }, { key, name: "Laptop", added: 6, from: "mac", mac: mac.pub, pairing: "dk", kind: "desktop" }];
    return state;
  });
  // The list has the desktop, but there's no live relay here to confirm it yet.
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", { key, name: "Laptop", kind: "desktop" }),
    { status: 503, body: { reason: "pairing-wait", message: "Checking this app with your other computers…" } });
  assert.ok(!fs.existsSync(forwards) || !fs.readdirSync(forwards).length, "nothing left to send");

  // A member waits for its primary. Without a setup, legacy enrollers still supply the local list.
  const entries = { phones: [{ key, pairing: "dk" }] };
  assert.deepEqual(await ask(APP, "POST", "kuvo/phones/remove", entries), {
    status: 409, body: { message: "Remove devices from your main computer, or update it to remove them from here.", primary: mac.pub },
  });
  assert.equal(link.readLink().phones.length, 2);
  link.updateLink(state => { delete state.setup; return state; });
  // The CLI removes no one. The authenticated local app removes devices, with enrollers excluded.
  assert.equal((await ask(CLI, "POST", "kuvo/phones/remove", { phones: [{ key }] })).status, 400);
  assert.equal((await ask(APP, "POST", "kuvo/phones/remove", { phones: [{ key: mac.pub }] })).status, 400, "never the Mac");
  assert.equal(link.readLink().phones.length, 2);
  assert.deepEqual(await ask(APP, "POST", "kuvo/phones/remove", { phones: [{ key }] }), { status: 200, body: {} });
  assert.deepEqual(link.readLink().phones.map(each => each.name), ["My iPhone"]);
  assert.deepEqual(link.readLink().removed, [{ key, pairing: "dk" }]);
  // Its removal goes out in this box's phones document, for every other computer.
  assert.deepEqual(link.ownPhones(link.readLink()).removed, [{ key, pairing: "dk" }]);
  assert.equal(link.authorizePhone(Buffer.from(key, "base64url"), { v: 1, client: "desktop" }, undefined), false);

  // Asking again for that key: refused for good, here, before anyone else is asked.
  const removed = { status: 409, body: { message: "This app was removed from your devices.", reason: "removed" } };
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", { key, name: "Laptop", kind: "desktop" }), removed);
  assert.ok(!fs.existsSync(forwards) || !fs.readdirSync(forwards).length, "nothing left to send");
  // And on a box that pairs by itself.
  alone({ removed: [{ key, pairing: "dk" }] });
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", { key, name: "Laptop", kind: "desktop" }), removed);
  assert.deepEqual(link.readLink().phones, []);
});

test("an inherited desktop whose signer is gone gets guidance for pairing again", async () => {
  const key = device();
  member({ phones: [{ key, name: "Laptop", from: "mac", mac: other.pub, pairing: "dk", kind: "desktop" }] });
  const answer = await ask(APP, "POST", "kuvo/devices/pair", { key, name: "Laptop", kind: "desktop" });
  assert.deepEqual(answer, { status: 409, body: {
    reason: "pairing-readd",
    title: "This app needs to be paired again",
    message: "The computer that added this app was removed. Pair this app again from one of your computers.",
    action: { label: "Pair Again…", route: "kuvo/devices/pair" },
  } });
});

test("a member's box forwards pairing for the app beside it to the main computer, and answers as that one said; with no answer it says to wait", async t => {
  member();
  tell(mac, { features: ["computers", "devices.pair", "work-queue.forward"] });
  const key = device();
  const given = { key, name: "  Lab\nscreen ", kind: "desktop" };

  // Handed to the main computer by mail, and answered with the pairing it made.
  let asked = ask(APP, "POST", "kuvo/devices/pair", given);
  let act = await pairSent(mac);
  assert.deepEqual({ ...act, id: undefined, at: undefined }, { id: undefined, at: undefined, area: "devices", method: "POST", item: key, body: { name: "Lab screen", kind: "desktop" } });
  link.receiveMail(mailFrom(mac, { workQueue: { acted: { id: act.id, status: 200, value: { paired: true, pairing: "from-main" } } }, at: Date.now() }), undefined, takeMail);
  assert.deepEqual(await asked, { status: 200, body: { paired: true, pairing: "from-main" } });
  // Nothing is paired here by that: this box lets it in once the main computer's phones document lists it.
  assert.deepEqual(link.readLink().phones, []);

  // The main computer says no (it lets no device in, say): its words, as they are.
  asked = ask(APP, "POST", "kuvo/devices/pair", given);
  act = await pairSent(mac);
  link.receiveMail(mailFrom(mac, { workQueue: { acted: { id: act.id, status: 409, value: { message: "“Let my devices connect” is off on Studio." } } }, at: Date.now() }), undefined, takeMail);
  assert.deepEqual(await asked, { status: 409, body: { message: "“Let my devices connect” is off on Studio." } });
  // Removed there: said so, with the reason the app goes by.
  asked = ask(APP, "POST", "kuvo/devices/pair", given);
  act = await pairSent(mac);
  link.receiveMail(mailFrom(mac, { workQueue: { acted: { id: act.id, status: 409, value: { message: "This app was removed from your devices.", reason: "removed" } } }, at: Date.now() }), undefined, takeMail);
  assert.deepEqual(await asked, { status: 409, body: { message: "This app was removed from your devices.", reason: "removed" } });
  // This computer is kept from adding its app (one it added was removed): the main computer's
  // words, and the reason the app goes by.
  const keptOut = { message: "Lab screen on devbox was removed from your devices, so devbox can't add Kuvo Code's app by itself now. Allow it again from Kuvo Code on Studio or another computer's app: Computers, devbox.", reason: "blocked" };
  asked = ask(APP, "POST", "kuvo/devices/pair", given);
  act = await pairSent(mac);
  link.receiveMail(mailFrom(mac, { workQueue: { acted: { id: act.id, status: 409, value: keptOut } }, at: Date.now() }), undefined, takeMail);
  assert.deepEqual(await asked, { status: 409, body: keptOut });
  // An answer from a box that isn't the main computer is no answer.
  const waits = { status: 503, body: { message: "Studio isn't answering. This app reaches your other computers once it does." } };
  const wait = link.queueForwardTiming.wait;
  link.queueForwardTiming.wait = 300;
  t.after(() => { link.queueForwardTiming.wait = wait; });
  asked = ask(APP, "POST", "kuvo/devices/pair", given);
  act = await pairSent(mac);
  link.receiveMail(mailFrom(here, { workQueue: { acted: { id: act.id, status: 200, value: { paired: true, pairing: "forged" } } }, at: Date.now() }), undefined, takeMail);
  // No answer in time: the app is told to wait, and asks again later.
  assert.deepEqual(await asked, waits);
  assert.ok(!fs.readdirSync(forwards).length, "nothing left behind");

  // The main computer found away, or this box off the relay: the same, at once, nothing sent.
  fs.writeFileSync(path.join(run, "peer-reach.json"), JSON.stringify({ [mac.pub]: Date.now() }));
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", given), waits);
  fs.rmSync(path.join(run, "peer-reach.json"));
  fs.writeFileSync(path.join(run, "relay-down"), String(Date.now()));
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", given), waits);
  fs.rmSync(path.join(run, "relay-down"));
  // Not heard from yet (what it takes isn't known): the same.
  member();
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", given), waits);
  // A main computer from before this: said so, nothing sent.
  tell(mac, { features: ["computers", "work-queue.forward"] });
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", given), { status: 409, body: { message: "Studio needs a newer Kuvo Code to add this app to your devices. Update it, then try again." } });
  assert.ok(!fs.existsSync(forwards) || !fs.readdirSync(forwards).length, "nothing left to send");
});

test("a member its main computer set aside adds no device and vouches for none", async () => {
  const key = device();
  const given = { key, name: "Laptop", kind: "desktop" };
  // Its main computer doesn't name it now: stepping aside opens no pairing.
  member();
  link.updateLink(state => { state.setup.aside = true; return state; });
  tell(mac, { features: ["devices.pair", "work-queue.forward"] });
  assert.deepEqual(await ask(APP, "POST", "kuvo/devices/pair", given), { status: 409, body: { message: "Studio doesn't list this computer in its setup now. Re-add this computer from Studio to pair this app." } });
  assert.deepEqual(link.readLink().phones, []);
  assert.ok(!fs.existsSync(forwards) || !fs.readdirSync(forwards).length, "nothing left to send");
});

test("a machine lets in the desktop device its Mac's phones document lists, as what it is, and sends its notifications to phones alone", () => {
  member();
  const seed = link.newSigningSeed();
  const desk = device(), phone = device();
  const deskAddress = k(link.addressOf(crypto.randomBytes(32))), phoneAddress = k(link.addressOf(crypto.randomBytes(32)));
  const state = link.updateLink(state => {
    state.macSigns = { [mac.pub]: link.signingKeyOf(seed) };
    state.docs = { [`${link.signingKeyOf(seed)} phones`]: link.signDoc(seed, "phones", 1, { phones: [
      { key: desk, name: "Laptop", address: deskAddress, pairing: "dk", kind: "desktop" },
      { key: phone, name: "My iPhone", address: phoneAddress, pairing: "ph", kind: "watch" },
    ], removed: [] }) };
    link.foldPhones(state);
    return state;
  });
  assert.deepEqual(state.phones.map(each => [each.name, each.from, each.kind]), [["Laptop", "mac", "desktop"], ["My iPhone", "mac", undefined]]);
  // A notice goes to the phone's address, never to the desktop app's (Apple has nothing for it).
  const sent = [];
  assert.equal(link.sendToPhones({ welcomed: true, send: bytes => { sent.push(bytes); return true; } }, link.readLink(), { title: "Done", body: "It finished.", category: "done", thread: "ses_1" }), true);
  assert.deepEqual(sent.map(bytes => k(bytes.subarray(1, 17))), [phoneAddress]);
});

test("what the desktop app writes over the link reaches the agent without the phone's note, while a phone's still has it", async t => {
  alone();
  const prompted = [];
  const server = http.createServer((request, response) => {
    let body = "";
    request.on("data", chunk => { body += chunk; });
    request.on("end", () => {
      const session = /^\/api\/session\/([^/?]+)$/.exec(request.url)?.[1];
      if (request.url === "/api/model/default") { response.writeHead(404); return response.end("{}"); }
      response.writeHead(200, { "content-type": "application/json" });
      if (session) return response.end(JSON.stringify({ data: { id: session, model: { id: "words", providerID: "p" } } }));
      if (request.url === "/api/model") return response.end(JSON.stringify({ data: [{ id: "words", providerID: "p", capabilities: { input: ["text"] } }] }));
      if (request.url.endsWith("/prompt")) prompted.push(JSON.parse(body).text);
      response.end("{}");
    });
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  const own = link.engineHandler(() => ({ port: server.address().port, password: "pw" }), { name: "devbox" });
  const send = async (id, phone) => {
    const pieces = [];
    for await (const piece of own({ method: "POST", path: "api/session/ses_here1/prompt", headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ id, text: "Ship it" })) }, phone)) pieces.push(piece);
    assert.equal(pieces[0].head.status, 200);
    return prompted.at(-1);
  };
  const key = crypto.randomBytes(32);
  assert.equal(await send("msg_desk1", { key, name: "Laptop", address: Buffer.alloc(16), kind: "desktop" }), "Ship it");
  assert.match(await send("msg_phone1", { key, name: "My iPhone", address: Buffer.alloc(16) }), /^Ship it\n\n<from-phone>\n/);
});
