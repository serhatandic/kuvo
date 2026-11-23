// The Work Queue across your computers (PROTOCOL.md, "The Work Queue", "Across your computers"):
// one list for everything. What each computer tells the others of its queue shows in the All view
// and in a lead's view, with which computer it's on; chips and an item's page answer from it, and a
// change to one goes to that computer. Each box tells its own as it changes (in parts when it's too
// big for one mail). Numbers come from the first computer you set up, wherever a box hears of it.
// An item handed to a conversation on another computer moves once. And what a newer box keeps (an
// outside tracker's author, a queue of a kind this box doesn't know) is kept.
//
// One box (a machine, here, by the real link in this process), its Mac and another machine as keys
// whose mail is sealed and opened as the relay carries it; only the engine is a stand-in.
//
// Run with `node --test box/bridge-tests/work-queue-across.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { beforeEach, test } from "node:test";
import { atEnd, removeAtEnd } from "./at-end.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-work-queue-across-")));
removeAtEnd(home);
// GitHub's `gh`, standing in: it says each time it's asked (`gh-asked`), and answers what a test
// says GitHub would (`gh-answer.json`), else that it's signed out. Never GitHub itself.
// It writes each query it was asked on a line, and answers one by GitHub's id with `gh-answer-id.json`
// while that's there.
const ghAsked = path.join(home, "gh-asked"), ghAnswer = path.join(home, "gh-answer.json"), ghAnswerID = path.join(home, "gh-answer-id.json");
fs.mkdirSync(path.join(home, "bin"), { recursive: true });
fs.writeFileSync(path.join(home, "bin", "gh"), `#!/bin/sh\nfor last; do :; done\nprintf '%s\\n' "$last" | tr '\\n' ' ' >> '${ghAsked}'\necho >> '${ghAsked}'\ncase "$last" in *"node(id:"*) if [ -f '${ghAnswerID}' ]; then cat '${ghAnswerID}'; exit 0; fi ;; esac\nif [ -f '${ghAnswer}' ]; then cat '${ghAnswer}'; exit 0; fi\necho 'gh auth login' >&2\nexit 1\n`, { mode: 0o755 });
const ghQueries = () => (fs.existsSync(ghAsked) ? fs.readFileSync(ghAsked, "utf8").split("\n").filter(Boolean) : []);
const ghAsks = () => (fs.existsSync(ghAsked) ? fs.readFileSync(ghAsked, "utf8").split("\n").filter(Boolean).length : 0);
process.env.PATH = `${path.join(home, "bin")}${path.delimiter}${process.env.PATH}`;
process.env.KUVO_HOME = home;
const link = await import(LINK_BUNDLE.href);
const run = path.join(home, "run");
const queues = path.join(run, "work-queue");
const write = (file, value) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, typeof value === "string" ? value : JSON.stringify(value));
};
const read = file => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; } };

// The engine: a lead and the session it started, both here, and what each calls now.
const sessions = new Map([
  ["ses_lead", { id: "ses_lead", title: "Lead", time: { created: 1, updated: 5 } }],
  ["ses_child", { id: "ses_child", title: "Sign-in", metadata: { kuvo: { startedBy: "ses_lead", depth: 1 } }, time: { created: 2, updated: 6 } }],
]);
const calling = new Map();
const engine = http.createServer((request, response) => {
  let body = "";
  request.on("data", chunk => { body += chunk; });
  request.on("end", () => {
    const reply = (value, status = 200) => { response.writeHead(status, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
    const url = new URL(request.url, "http://x");
    if (url.pathname === "/api/session" && request.method === "GET") return reply({ data: [...sessions.values()] });
    if (url.pathname === "/api/session/active") return reply({ data: {} });
    const one = url.pathname.match(/^\/api\/session\/([^/]+)(\/.*)?$/);
    const id = one && decodeURIComponent(one[1]);
    if (one && !one[2]) return sessions.has(id) ? reply({ data: sessions.get(id) }) : reply({ message: "No such session." }, 404);
    if (one?.[2] === "/message") {
      const call = calling.get(id);
      return reply({ data: call ? [{ id: "msg_1", type: "assistant", content: [{ type: "tool", name: `kuvo-browser_${call.name}`, callID: "call_1", state: { status: "running", input: call.args } }] }] : [] });
    }
    reply({ data: [] });
  });
});
await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
atEnd(() => { engine.closeAllConnections(); engine.close(); });
const target = { port: engine.address().port, password: "p" };
const targets = [target];
const places = () => ({ targets, bridge: true });

// This box (a machine, "devbox"), the Mac that set it up, a later Mac, and another machine ("lab").
const k = key => key.toString("base64url");
const box = (kind, name, id) => {
  const key = link.newKey();
  const address = k(link.addressOf(crypto.randomBytes(32)));
  return { key, pub: k(link.publicOf(key)), address, peer: { key: k(link.publicOf(key)), address, name, kind, ...(id ? { id } : {}) } };
};
const here = box("machine", "devbox", "devbox"), mac = box("mac", "Studio"), later = box("mac", "Laptop"), lab = box("machine", "lab", "lab");
const fresh = ({ macs = [mac], peers = [mac, lab, here] } = {}) => {
  link.writeLink({
    secret: crypto.randomBytes(32).toString("base64url"), key: k(here.key), relay: "wss://relay.test", name: "devbox", id: "devbox", mac: macs.at(-1).pub, macs: macs.map(each => each.pub), phones: [],
    // (Each Mac's list of its boxes, as a Mac that signs no documents gives it: who the peers are.)
    legacy: Object.fromEntries(macs.map(each => [each.pub, { at: 1, peers: [each.peer, ...peers.filter(peer => peer.peer.kind === "machine").map(peer => peer.peer)] }])),
  });
};
// An enrolled machine with a direct route to its setup's main computer.
const inSetup = () => {
  const sign = link.signingKeyOf(link.newSigningSeed());
  link.updateLink(current => ({
    ...current,
    macSigns: { [mac.pub]: sign },
    setup: { id: "set_member".padEnd(26, "0"), role: "member", primary: mac.pub, sign, epoch: 1 },
  }));
};
// What each peer's agents' mail said: their agents, and that they take the Work Queue's mail.
const agentsTold = (extra = {}) => write(path.join(run, "peer-agents.json"), {
  [lab.pub]: { at: 1, heard: Date.now(), kind: "machine", id: "lab", list: [{ session: "ses_far", name: "Far", kind: "session" }, { session: "ses_wren", name: "Wren", kind: "bot" }], features: ["work-queue", "work-queue.elsewhere"] },
  [mac.pub]: { at: 1, heard: Date.now(), kind: "mac", list: [], features: ["work-queue", "work-queue.elsewhere"] },
  ...extra,
});

// Mail as the relay hands it on, from `from` to this box; and what this box sent, opened as `to` would.
const mailFrom = (from, payload) => link.sealOneWay(Buffer.from(JSON.stringify({ ...payload, at: Date.now() })), from.key, link.publicOf(here.key)).message;
const opened = (bytes, to) => {
  const body = link.readFrame(bytes.subarray(17)).body;
  return { address: k(bytes.subarray(1, 17)), payload: JSON.parse(link.openOneWay(body, to.key, link.publicOf(here.key)).payload.toString("utf8")) };
};
// A relay connection that keeps what's sent; and mail taken as this box's link takes it.
const sent = [];
const host = { welcomed: true, send: bytes => sent.push(bytes) };
const receive = (from, payload) => link.receiveMail(mailFrom(from, payload), host, (sender, given) => link.peerMail(sender, given, host, places));
// Mail a computer's link takes as it comes, waited on until it's done with (a hand-off looks up
// its conversation first).
const takeFrom = (from, workQueue) => link.takeWorkQueueMail(from.peer, workQueue, host, places);
const receiveHanded = (from, handed) => {
  let handled;
  link.receiveMail(mailFrom(from, { workQueue: { handed } }), host, (sender, given) => {
    handled = link.takeWorkQueueMail(sender, given.workQueue, host, places);
  });
  return handled;
};
const mailTo = to => {
  const theirs = sent.filter(bytes => k(bytes.subarray(1, 17)) === to.address);
  for (const bytes of theirs) sent.splice(sent.indexOf(bytes), 1);
  return theirs.map(bytes => opened(bytes, to).payload);
};

// The box's answer to one request, from a client (this Mac's app).
const handle = link.engineHandler(() => target);
async function route(method, pathname, body, as = { fromApp: true }) {
  const pieces = [];
  for await (const piece of handle({ method, path: pathname, headers: {}, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)), ...as }, undefined)) pieces.push(piece);
  const bytes = Buffer.concat(pieces.filter(each => each.body).map(each => each.body));
  let parsed = bytes.toString();
  try { parsed = JSON.parse(parsed); } catch {}
  return { status: pieces.find(each => each.head).head.status, body: parsed };
}
async function tool(name, args, session) {
  calling.set(session, { name, args });
  try {
    return (await route("POST", "kuvo/work-queue/tools", { name, arguments: args, session })).body;
  } finally {
    calling.delete(session);
  }
}
const view = async owner => (await route("GET", `kuvo/work-queue?owner=${owner}`)).body;

// An item as another computer tells it.
const told = (number, owner, extra = {}) => ({
  id: `wqi_${String(number).padStart(20, "0")}`, number, owner, title: `Item ${number}`, status: "todo", order: number, priority: "normal", labels: [], sessions: [], noteCount: 0, fileCount: 0, linkCount: 0,
  created: { at: 1000, by: { session: owner, name: "Far" } }, updated: { at: 2000, by: { session: owner, name: "Far" } }, ...extra,
});
const telling = (items, extra = {}) => ({ told: { id: `wqt_${crypto.randomBytes(9).toString("base64url")}`, at: Date.now(), kind: "machine", since: 5000, items, owners: [{ owner: "ses_far", kind: "session", title: "Far away" }], labels: [], ...extra } });

beforeEach(() => {
  fs.rmSync(ghAsked, { force: true });
  fs.rmSync(ghAnswer, { force: true });
  fs.rmSync(ghAnswerID, { force: true });
  fs.rmSync(run, { recursive: true, force: true });
  fs.mkdirSync(run, { recursive: true });
  write(path.join(run, "engine.json"), { ...target, version: "0.0.0" });
  sent.length = 0;
  calling.clear();
  link.forgetQueueMigrated();
  link.forgetQueuesTold();
  link.forgetQueueActs();
  link.queueForwardsSoon.send = () => {};
  link.queueForwardTiming.wait = 20_000;
  fresh();
  agentsTold();
});

test("what your other computers told shows in the All view and a lead's view, with the computer it's on; chips and its page answer from it; it's changed there", async () => {
  // This box's own, numbered from a block its Mac gave.
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  const own = await tool("queue_add", { title: "Ship the release" }, "ses_lead");
  assert.match(own.text, /^Added #100 /);
  // On lab: a session the lead started there (lab's rows say so), a stranger's, and one the lead
  // handed to a session there.
  write(path.join(run, "work", "elsewhere", `${lab.pub}.json`), { at: Date.now(), heard: Date.now(), leads: { ses_lead: [{ session: "ses_far", title: "Far away", folder: "", startedBy: "ses_lead", depth: 1, state: "idle", started: 1, tokens: 0 }] } });
  const tellsNow = telling([told(500, "ses_far", { progress: { note: "Writing the test" }, status: "in-progress", priority: "high", labels: ["lbl_tests"] }), told(501, "ses_stranger"), told(502, "ses_bob", { handedBy: "ses_lead" })], { labels: [{ id: "lbl_tests", name: "Tests", color: "mint" }] });
  receive(lab, { workQueue: tellsNow });
  assert.deepEqual(mailTo(lab).map(each => each.workQueue), [{ had: tellsNow.told.id }], "lab hears this box has it");

  const lead = await view("ses_lead");
  assert.deepEqual(new Set(lead.items.map(item => item.number)), new Set([100, 500, 502]), "its own, its session's on lab, and the one it handed there; never a stranger's");
  const far = lead.items.find(item => item.number === 500);
  assert.deepEqual(far.computer, { key: lab.pub, name: "lab", kind: "machine", id: "lab" });
  assert.equal(far.progress.note, "Writing the test");
  assert.deepEqual([far.status, far.priority, far.labels], ["in-progress", "high", ["lbl_tests"]], "where it stands, as lab said");
  assert.deepEqual(lead.labels, [{ id: "lbl_tests", name: "Tests", color: "mint" }], "a label only lab has is listed for its items");
  assert.deepEqual(far.can, { edit: false, set: false, assign: false, close: false, delete: false, restore: false, attach: false, why: "It's on lab: change it there." }, "lab takes no change from another box yet, and the box says so");
  assert.equal(lead.owners.find(each => each.owner === "ses_far")?.title, "Far away");
  const all = await view("all");
  assert.deepEqual(new Set(all.items.map(item => item.number)), new Set([100, 500, 501, 502]));
  assert.deepEqual(all.computers.map(each => each.name), ["lab"]);

  // A chip, and the item's page: as lab told it, without what only lab keeps.
  const refs = (await route("GET", "kuvo/work-queue/refs?refs=500,999,100")).body.refs;
  assert.equal(refs["500"].title, "Item 500");
  assert.equal(refs["500"].computer.name, "lab");
  assert.equal(refs["999"], null);
  assert.equal(refs["100"].computer, undefined);
  const posted = (await route("POST", "kuvo/work-queue/refs", { refs: ["#500", "gh:owner/repo#12"] })).body.refs;
  assert.equal(posted["#500"].number, 500);
  assert.equal(posted["gh:owner/repo#12"], null, "an outside ref no source answers yet stays plain text");
  const page = await route("GET", "kuvo/work-queue/items/500");
  assert.equal(page.status, 200);
  assert.equal(page.body.item.computer.id, "lab");
  assert.equal(page.body.item.body, "");

  // You change it on lab; an agent asks the conversation it belongs to.
  const moved = await route("DELETE", "kuvo/work-queue/items/500");
  assert.equal(moved.status, 409);
  assert.equal(moved.body.computer.id, "lab");
  assert.match(moved.body.message, /#500 "Item 500" is on lab: change it there\./);
  const listed = await tool("queue_list", {}, "ses_lead");
  assert.match(listed.text, /On your other computers \(as each last told it; changed there\):\n- #500 "Item 500" · In progress · High · \[Tests\] · Writing the test · in "Far away" \(ses_far\) · on lab, updated/);
  assert.match(listed.text, /#501 "Item 501".*on lab/);
  const onlyLabLead = await tool("queue_list", { owner: "ses_far" }, "ses_lead");
  assert.match(onlyLabLead.text, /#500 "Item 500"/);
  assert.doesNotMatch(onlyLabLead.text, /#501 "Item 501"/);
  const labeled = await tool("queue_list", { owner: "ses_far", label: "Tests" }, "ses_lead");
  assert.equal(labeled.isError, false, labeled.text);
  assert.match(labeled.text, /#500 "Item 500" · In progress · High · \[Tests\]/);
  const changed = await tool("queue_update", { item: "#500", note: "Hi" }, "ses_lead");
  assert.equal(changed.isError, true);
  assert.equal(changed.text, '#500 "Item 500" is on lab. Update Kuvo there so it can record which agent made the change.');
  const shown = await tool("queue_show", { item: 500 }, "ses_lead");
  assert.match(shown.text, /^#500 "Item 500" · In progress · High · \[Tests\] · on lab, in the queue of ses_far/);

  // What lab told a moment before, come late, isn't kept over what it told since, nor said to be
  // had (had, lab would never send it again, were it what lab tells next).
  const late = telling([told(503, "ses_far")], { at: tellsNow.told.at - 5000 });
  receive(lab, { workQueue: late });
  assert.deepEqual(mailTo(lab), []);
  assert.equal((await view("all")).items.some(item => item.number === 503), false);
  // The same again (lab's had lost): had again.
  receive(lab, { workQueue: tellsNow });
  assert.deepEqual(mailTo(lab).map(each => each.workQueue), [{ had: tellsNow.told.id }]);
  // One an hour "older" is lab's clock set back: its newer word after all.
  const setBack = telling([told(504, "ses_far")], { at: tellsNow.told.at - 3_600_000 });
  receive(lab, { workQueue: setBack });
  sent.length = 0;
  assert.equal((await view("all")).items.some(item => item.number === 504), true);
  receive(lab, { workQueue: tellsNow });
  sent.length = 0;
  // An agent can read a remote item that belongs to an unrelated conversation.
  const notTheirs = await tool("queue_show", { item: 501 }, "ses_lead");
  assert.equal(notTheirs.isError, false, notTheirs.text);
  assert.match(notTheirs.text, /^#501 "Item 501" · Todo · on lab/);
  assert.deepEqual((await tool("queue_update", { item: 501, note: "Hi" }, "ses_lead")), {
    isError: true,
    text: '#501 "Item 501" is on lab. Update Kuvo there so it can record which agent made the change.',
  });
  // A box that isn't one of your computers isn't heard at all.
  const stranger = box("machine", "stranger");
  link.receiveMail(link.sealOneWay(Buffer.from(JSON.stringify({ workQueue: telling([told(600, "ses_x")]), at: Date.now() })), stranger.key, link.publicOf(here.key)).message, host, (sender, given) => link.peerMail(sender, given, host, places));
  assert.equal((await view("all")).items.some(item => item.number === 600), false);
});

test("an item linked across computers stays in the named view, even when its keeper is too old for forwarded agent changes", async () => {
  // Lab's lead put ses_child (here) on its item: lab tells it with ses_child among its sessions.
  receive(lab, { workQueue: telling([told(700, "ses_far", { sessions: [{ session: "ses_child", title: "Sign-in", working: false }] }), told(701, "ses_far")]) });
  sent.length = 0;
  const worker = await view("ses_child");
  assert.deepEqual(worker.items.map(item => item.number), [700]);
  assert.equal(worker.items[0].computer.name, "lab");
  assert.equal(worker.owners.find(each => each.owner === "ses_far")?.title, "Far away");
  // Both items are readable. Changes still go through the box that keeps the item.
  assert.match((await tool("queue_list", {}, "ses_child")).text, /#700 "Item 700"/);
  assert.match((await tool("queue_list", {}, "ses_child")).text, /#701 "Item 701"/);
  assert.equal((await tool("queue_update", { item: 700, note: "Hi" }, "ses_child")).isError, true);
});

test("a computer that isn't answering: its items show as it last told them, none of their sessions at work, and say since when; heard again, they're as it tells them", async () => {
  const tellsNow = telling([told(500, "ses_far", { status: "in-progress", progress: { note: "Writing the test" }, sessions: [{ session: "ses_far", title: "Far away", working: true }] }), told(501, "ses_far")]);
  receive(lab, { workQueue: tellsNow });
  const live = await view("all");
  assert.deepEqual(live.items.find(item => item.number === 500).sessions.map(each => each.working), [true], "at work as lab says, while lab answers");
  assert.equal(live.computers[0].away, undefined);

  // The relay finds nothing connected at lab (it's switched off).
  const since = Date.now() - 60_000;
  write(path.join(run, "peer-reach.json"), { [lab.pub]: since });
  const away = await view("all");
  const item = away.items.find(each => each.number === 500);
  assert.deepEqual(item.sessions.map(each => each.working), [false], "nothing on a computer that's off can be seen at work");
  assert.equal(item.status, "in-progress", "its status is as lab set it: nothing works that out");
  assert.deepEqual(item.can, { edit: false, set: false, assign: false, close: false, delete: false, restore: false, attach: false, why: "lab isn't answering now (it may be asleep, off or offline): it can be changed once it's back." }, "and nothing may be done to it, the box saying why");
  assert.equal(item.progress.note, "Writing the test", "what it said is kept");
  assert.deepEqual(away.computers.map(each => [each.name, each.away]), [["lab", since]]);
  assert.equal((await route("GET", "kuvo/work-queue/refs?refs=500")).body.refs["500"].status, "in-progress", "its chip says where it stands");
  assert.deepEqual((await route("GET", "kuvo/work-queue/items/500")).body.item.sessions.map(each => each.working), [false], "and its page says no one's at work");

  // Lab's mail comes again: it answers.
  receive(lab, { workQueue: telling([told(500, "ses_far", { status: "in-progress", sessions: [{ session: "ses_far", title: "Far away", working: true }] })]) });
  const back = await view("all");
  assert.deepEqual(back.items.find(each => each.number === 500).sessions.map(each => each.working), [true]);
  assert.equal(back.computers[0].away, undefined);
});

test("this box tells your other computers its queue as it changes: again until it's had, then not until it changes; too big for one mail, in parts", async () => {
  inSetup();
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  await tool("queue_add", { title: "Fix the login page" }, "ses_child");
  const now = Date.now();
  await link.pushQueues(host, link.readLink(), targets, now);
  const toLab = mailTo(lab);
  assert.equal(toLab.length, 1);
  const first = toLab[0].workQueue.told;
  assert.deepEqual(first.items.map(item => [item.number, item.title, item.owner]), [[100, "Fix the login page", "ses_child"]]);
  assert.deepEqual(first.owners.find(each => each.owner === "ses_child"), { owner: "ses_child", kind: "session", title: "Sign-in", startedBy: "ses_lead" });
  assert.equal(first.kind, "machine");
  assert.deepEqual(first.primary, { key: mac.pub, kind: "mac" }, "it numbers from its Mac");
  assert.ok(first.top >= 149, "it tells the highest number it holds");
  assert.equal(mailTo(mac).length, 1, "its Mac is told too");
  // Nothing again while it isn't had yet and a minute hasn't gone by; again after.
  await link.pushQueues(host, link.readLink(), targets, now + 1000);
  assert.equal(sent.length, 0);
  await link.pushQueues(host, link.readLink(), targets, now + 61_000);
  assert.equal(mailTo(lab)[0].workQueue.told.id, first.id);
  sent.length = 0;
  // Had: nothing more while it's the same.
  receive(lab, { workQueue: { had: first.id } });
  sent.length = 0;
  await link.pushQueues(host, link.readLink(), targets, now + 200_000);
  assert.equal(mailTo(lab).length, 0);
  sent.length = 0;

  // Too big for one mail: in parts, kept once the last is here, and never from part of it.
  const many = Array.from({ length: 400 }, (_, index) => told(1000 + index, "ses_far", { title: `${"A long title that fills the mail ".repeat(5)} ${index}`.slice(0, 200), progress: { note: "x".repeat(290) } }));
  const big = telling(many);
  const mails = link.queueMailsFor(here.peer, big, link.readLink());
  assert.ok(mails.length > 2, `${mails.length} parts`);
  // (Sealed for this box by lab: the same parts, as lab would send them.)
  const parts = mails.map(bytes => JSON.parse(link.openOneWay(link.readFrame(bytes.subarray(17)).body, here.key, link.publicOf(here.key)).payload.toString("utf8")).workQueue.part);
  for (const part of parts.slice(0, -1)) receive(lab, { workQueue: { part } });
  assert.equal((await view("all")).items.some(item => item.number === 1000), false, "not until the last part");
  receive(lab, { workQueue: { part: parts.at(-1) } });
  assert.equal((await view("all")).items.filter(item => item.computer?.name === "lab").length, 400);
  assert.deepEqual(mailTo(lab).map(each => each.workQueue), [{ had: big.told.id }]);
  // A part that isn't the whole's (its digest doesn't hold) puts nothing together.
  const bad = telling([told(2000, "ses_far")]);
  const badParts = link.queueMailsFor(here.peer, { ...bad, padding: "y".repeat(90_000) }, link.readLink()).map(bytes => JSON.parse(link.openOneWay(link.readFrame(bytes.subarray(17)).body, here.key, link.publicOf(here.key)).payload.toString("utf8")).workQueue.part);
  badParts[0] = { ...badParts[0], d: badParts[0].d.replace(/^./, c => (c === "A" ? "B" : "A")) };
  for (const part of badParts) receive(lab, { workQueue: { part } });
  assert.equal((await view("all")).items.some(item => item.number === 2000), false);
});

test("legacy setup discovery ranks a Mac before a machine and the earlier Mac before the later one", () => {
  const file = path.join(queues, "numbers.json");
  // A machine set up from two Macs, the later one last (piece 1 took that one): neither told yet.
  fresh({ macs: [mac, later] });
  write(file, { primary: later.pub, leases: [[200, 249]] });
  assert.equal(link.rankedPrimary(link.readLink()).peer.key, later.pub, "kept, while nothing says another was set up first");
  // Each tells when it was set up: the earlier one is the primary; lab, a machine set up "earlier"
  // still, never is while there's a Mac.
  write(path.join(queues, "peers", `${mac.pub}.json`), { id: "wqt_aaaaaaaaaaaa", at: 1, heard: 1, kind: "mac", since: 1_000, items: [], owners: [], labels: [] });
  write(path.join(queues, "peers", `${later.pub}.json`), { id: "wqt_bbbbbbbbbbbb", at: 1, heard: 1, kind: "mac", since: 9_000, items: [], owners: [], labels: [] });
  write(path.join(queues, "peers", `${lab.pub}.json`), { id: "wqt_cccccccccccc", at: 1, heard: 1, kind: "machine", since: 10, items: [], owners: [], labels: [] });
  assert.equal(link.rankedPrimary(link.readLink()).peer.key, mac.pub);
  // Never back to the later one, whatever it says next; and never away from the one kept for one
  // that hasn't said when it was set up (an older box mid-update): only for one known to be earlier.
  write(file, { ...link.readNumbers(file), primary: later.pub });
  write(path.join(queues, "peers", `${later.pub}.json`), { id: "wqt_bbbbbbbbbbbb", at: 1, heard: 1, kind: "mac", items: [], owners: [], labels: [] });
  assert.equal(link.rankedPrimary(link.readLink()).peer.key, later.pub, "kept: it hasn't said, and may be the first");
  write(path.join(queues, "peers", `${later.pub}.json`), { id: "wqt_bbbbbbbbbbbb", at: 1, heard: 1, kind: "mac", since: 9_000, items: [], owners: [], labels: [] });
  assert.equal(link.rankedPrimary(link.readLink()).peer.key, mac.pub, "once it says, the earlier one");
  write(file, { ...link.readNumbers(file), primary: mac.pub });
  write(path.join(queues, "peers", `${later.pub}.json`), { id: "wqt_bbbbbbbbbbbb", at: 2, heard: 2, kind: "mac", since: 9_000, items: [], owners: [], labels: [], primary: { key: later.pub, kind: "mac", since: 9_000 } });
  assert.equal(link.rankedPrimary(link.readLink()).peer.key, mac.pub);

  // A Mac this box can't write to (not one of its own), set up first, as lab says: reached through lab.
  const first = box("mac", "Old Mac");
  write(path.join(queues, "peers", `${lab.pub}.json`), { id: "wqt_cccccccccccc", at: 3, heard: 3, kind: "machine", since: 10, items: [], owners: [], labels: [], primary: { key: first.pub, kind: "mac", since: 500 } });
  const through = link.rankedPrimary(link.readLink());
  assert.equal(through.peer.key, first.pub);
  assert.equal(through.via.key, lab.pub);
  assert.deepEqual(link.primaryOf(link.readLink()), { away: true }, "ranking does not route calls or numbering");
});

test("a machine in a setup numbers from its main computer, whatever a peer says of a Mac set up earlier; a second Mac of yours setting it up leaves it in that setup, never saying it's shared", async () => {
  const file = path.join(queues, "numbers.json");
  // Its Mac leads a setup, as the Mac's signed peers document says.
  const seed = link.newSigningSeed();
  const sign = link.signingKeyOf(seed);
  const body = { boxes: [{ ...mac.peer, sign }, lab.peer, here.peer], moved: [], left: [], setup: { id: "set_AAAAAAAAAAAAAAAAAAAAAA", epoch: 1, primary: mac.pub } };
  link.updateLink(() => ({ secret: crypto.randomBytes(32).toString("base64url"), key: k(here.key), sign: link.newSigningSeed(), relay: "wss://relay.test", name: "devbox", id: "devbox", mac: mac.pub, macs: [mac.pub], macSigns: { [mac.pub]: sign }, phones: [] }));
  link.updateLink(current => { link.docsWork({ link: current, library: { personalities: [], removed: [] } }, { key: mac.pub, sign }, { give: [link.signDoc(seed, "peers", 1, body)] }); return current; });
  assert.equal(link.readLink().setup?.primary, mac.pub);
  write(file, { primary: mac.pub, leases: [], asking: { id: "wqn_tothemainmac01", count: 50, since: 1 } });
  // lab says it numbers from a Mac set up earlier (as piece 3 would follow): this box doesn't.
  const first = box("mac", "Old Mac");
  write(path.join(queues, "peers", `${lab.pub}.json`), { id: "wqt_cccccccccccc", at: 3, heard: 3, kind: "machine", since: 10, items: [], owners: [], labels: [], primary: { key: first.pub, kind: "mac", since: 500 } });
  assert.deepEqual(link.primaryOf(link.readLink(), file), { peer: link.peersOf(link.readLink()).find(peer => peer.key === mac.pub) });
  link.notePrimaryNow(link.readLink(), { file });
  assert.equal(link.readNumbers(file).primary, mac.pub);
  sent.length = 0;
  await link.pushNumbers(host, link.readLink());
  assert.equal(mailTo(lab).length, 0, "nothing through lab");
  const asked = mailTo(mac).map(each => each.workQueue);
  assert.equal(asked[0]?.ask?.id, "wqn_tothemainmac01");
  assert.equal(asked[0].to, undefined, "to its main computer itself");
  // The removed shared field no longer goes out in the queue snapshot.
  await link.pushQueues(host, link.readLink(), targets, Date.now());
  assert.equal(Object.hasOwn(mailTo(lab)[0].workQueue.told, "shared"), false);

  // A second Mac (one of yours: only you set your computers up) sets it up, leading no setup: it
  // stays in the one it has, numbering from its main computer, and never says it's shared.
  const second = link.newSigningSeed();
  link.updateLink(current => {
    current.macs = [mac.pub, later.pub];
    current.macSigns = { ...current.macSigns, [later.pub]: link.signingKeyOf(second) };
    link.docsWork({ link: current, library: { personalities: [], removed: [] } }, { key: later.pub, sign: link.signingKeyOf(second) }, { give: [link.signDoc(second, "peers", 1, { boxes: [{ ...later.peer, sign: link.signingKeyOf(second) }, here.peer], moved: [], left: [] })] });
    return current;
  });
  assert.equal(link.readLink().setup.aside, undefined);
  sent.length = 0;
  link.forgetQueuesTold();
  // (What it tells is made again at most once a minute.)
  await link.pushQueues(host, link.readLink(), targets, Date.now() + 120_000);
  assert.equal(Object.hasOwn(mailTo(lab)[0].workQueue.told, "shared"), false);
  assert.equal(link.followedSetup(link.readLink())?.primary, mac.pub);
  assert.deepEqual(link.primaryOf(link.readLink(), file), { peer: link.peersOf(link.readLink()).find(peer => peer.key === mac.pub) });
  sent.length = 0;
});

test("a machine explicitly removed from its setup numbers on its own", () => {
  const file = path.join(queues, "numbers.json");
  const seed = link.newSigningSeed();
  const sign = link.signingKeyOf(seed);
  const leads = { setup: { id: "set_BBBBBBBBBBBBBBBBBBBBBB", epoch: 1, primary: mac.pub } };
  const library = { personalities: [], removed: [] };
  link.updateLink(() => ({ secret: crypto.randomBytes(32).toString("base64url"), key: k(here.key), sign: link.newSigningSeed(), relay: "wss://relay.test", name: "devbox", id: "devbox", mac: mac.pub, macs: [mac.pub], macSigns: { [mac.pub]: sign }, phones: [] }));
  link.updateLink(current => { link.docsWork({ link: current, library }, { key: mac.pub, sign }, { give: [link.signDoc(seed, "peers", 1, { boxes: [{ ...mac.peer, sign }, lab.peer, here.peer], moved: [], left: [], ...leads })] }); return current; });
  write(file, { primary: mac.pub, leases: [], since: 9_000 });
  assert.equal(link.readLink().setup?.role, "member");
  // Its Mac removes it.
  link.updateLink(current => { link.docsWork({ link: current, library }, { key: mac.pub, sign }, { give: [link.signDoc(seed, "peers", 2, { boxes: [{ ...mac.peer, sign }, lab.peer], moved: [], left: [here.pub], ...leads })] }); return current; });
  link.noteSetupNow(Date.now(), { soon: 0 });
  assert.equal(link.readLink().setup, undefined, "the departed computer no longer has a setup");
  assert.deepEqual(link.primaryOf(link.readLink()), { self: true });
});

test("the primary moves past what a box that numbered on its own used, and what any of your computers tell it they hold", () => {
  const file = path.join(queues, "primary-numbers.json");
  assert.equal(link.takeNumber({ self: true }, { file }), 1);
  const block = link.giveBlock("box-a", { id: "wqn_movedpast001", count: 50, top: 480 }, { file });
  assert.deepEqual([block.from, block.to], [481, 530], "past the 480 it used");
  assert.deepEqual(link.giveBlock("box-a", { id: "wqn_movedpast001", count: 50, top: 9000 }, { file }), block, "an ask sent again gets the same block");
  // A number an import was refused (far past the others) never moves the counter, nor does a top
  // past what a chip reads.
  const far = link.giveBlock("box-b", { id: "wqn_movedpast002", count: 50, top: 999_999 }, { file });
  assert.ok(far.to < 1_000_000, `${far.from}–${far.to} still reads as a chip`);
  // A box that was its own primary and joins a setup asks at once.
  inSetup();
  const own = path.join(queues, "numbers.json");
  write(own, { primary: "self", next: 77, leases: [] });
  link.notePrimaryNow(link.readLink());
  const kept = link.readNumbers(own);
  assert.equal(kept.primary, mac.pub, "a machine set up from a Mac never numbers on its own");
  assert.ok(kept.asking, "and asks its Mac at once");
  assert.ok(link.askToSend({ file: own, now: Date.now() }).top >= 76, "telling it how far it got");
});

test("an item handed to a session on another computer moves once: unchanged here until that one has it, sent again until then, gone from here once it's had; one refused comes back with why", async () => {
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  const added = await tool("queue_add", { title: "Port the importer", body: "Use the **new** parser." }, "ses_lead");
  assert.match(added.text, /^Added #100 /);
  const handed = await tool("queue_hand", { item: "#100", to: "ses_far" }, "ses_lead");
  assert.equal(handed.isError, false, handed.text);
  assert.match(handed.text, /^Handing #100 "Port the importer" to Far on lab: it goes into its queue there/);
  const meanwhile = await tool("queue_update", { item: "#100", note: "More" }, "ses_lead");
  assert.equal(meanwhile.isError, true);
  assert.match(meanwhile.text, /is being handed to Far on another computer/);
  assert.equal((await route("PATCH", "kuvo/work-queue/items/100", { status: "backlog" })).status, 400);
  // It goes to lab with this box's rounds, whole; again after a minute while lab doesn't say.
  const now = Date.now();
  await link.pushQueues(host, link.readLink(), targets, now);
  const hand = mailTo(lab).map(each => each.workQueue).find(each => each.hand)?.hand;
  assert.equal(hand.to, "ses_far");
  assert.equal(hand.from, "ses_lead");
  assert.equal(hand.item.number, 100);
  assert.equal(hand.item.body, "Use the **new** parser.");
  await link.pushQueues(host, link.readLink(), targets, now + 1000);
  assert.equal(mailTo(lab).filter(each => each.workQueue.hand).length, 0);
  await link.pushQueues(host, link.readLink(), targets, now + 61_000);
  assert.equal(mailTo(lab).filter(each => each.workQueue.hand).length, 1);
  // Only lab's word counts; then it's gone from here.
  await receiveHanded(mac, { id: hand.id, ok: true });
  assert.equal((await view("ses_lead")).items.some(item => item.number === 100 && !item.computer), true);
  await receiveHanded(lab, { id: hand.id, ok: true });
  assert.equal((await view("all")).items.some(item => item.number === 100), false);

  // One lab says to send again later (its conversation moving): again at its next minute, not at once.
  await tool("queue_add", { title: "Retry me" }, "ses_lead");
  await tool("queue_hand", { item: "#101", to: "ses_far" }, "ses_lead");
  await link.pushQueues(host, link.readLink(), targets, now + 100_000);
  const retried = mailTo(lab).map(each => each.workQueue).find(each => each.hand)?.hand;
  await receiveHanded(lab, { id: retried.id, ok: false, retry: true });
  await link.pushQueues(host, link.readLink(), targets, Date.now());
  assert.equal(mailTo(lab).filter(each => each.workQueue.hand).length, 0, "not at once");
  await receiveHanded(lab, { id: retried.id, ok: true });

  // One lab can't take comes back as it was, with why.
  await tool("queue_add", { title: "Second" }, "ses_lead");
  await tool("queue_hand", { item: "#102", to: "Wren" }, "ses_lead");
  await link.pushQueues(host, link.readLink(), targets, now + 200_000);
  const second = mailTo(lab).map(each => each.workQueue).find(each => each.hand)?.hand;
  assert.equal(second.to, "ses_wren", "a bot on lab, by its name");
  await receiveHanded(lab, { id: second.id, ok: false, why: "That queue has 2,000 open items already." });
  const back = await tool("queue_show", { item: 102 }, "ses_lead");
  assert.match(back.text, /Couldn't be handed to Wren on another computer: That queue has 2,000 open items already\. It's still here\./);
  assert.equal((await tool("queue_update", { item: 102, note: "ok" }, "ses_lead")).isError, false);

  // To a computer that runs an older Kuvo Code: refused, saying so.
  agentsTold({ [lab.pub]: { at: 1, heard: Date.now(), kind: "machine", id: "lab", list: [{ session: "ses_far", name: "Far", kind: "session" }], features: ["work-queue"] } });
  const older = await tool("queue_hand", { item: 102, to: "ses_far" }, "ses_lead");
  assert.equal(older.isError, true);
  assert.match(older.text, /runs an older Kuvo Code/);
});

test("an item handed here from another computer lands in its conversation's queue once, linked to it, with its files; one for no conversation here is refused", async () => {
  const id = "wqi_handedfromlab0000001";
  const item = told(700, "ses_farlead", { id, body: "From lab.", notes: [{ at: 1, by: { source: "github", name: "octocat" }, text: "Seen upstream" }], files: [{ name: "shot.png", size: 3, type: "image/png", at: 1 }], links: [], handed: [] });
  const hand = { id: "wqh_fromlab00000001", to: "ses_child", from: "ses_farlead", by: { session: "ses_farlead", name: "Far lead" }, at: Date.now(), item, files: [{ name: "shot.png", data: Buffer.from("png").toString("base64") }] };
  await takeFrom(lab, { hand });
  assert.deepEqual(mailTo(lab).map(each => each.workQueue), [{ handed: { id: hand.id, ok: true } }]);
  const landed = (await route("GET", "kuvo/work-queue/items/700")).body.item;
  assert.equal(landed.owner, "ses_child");
  assert.equal(landed.body, "From lab.");
  assert.deepEqual(landed.sessions.map(each => each.session), ["ses_child"]);
  assert.equal(landed.handedBy, "ses_farlead");
  assert.deepEqual(landed.notes[0].by, { source: "github", name: "octocat" }, "an outside tracker's author kept as it was");
  assert.equal(fs.readFileSync(path.join(queues, "files", id, "shot.png"), "utf8"), "png");
  // Sent again (lab didn't hear the answer): had, still once.
  await takeFrom(lab, { hand });
  assert.deepEqual(mailTo(lab).map(each => each.workQueue), [{ handed: { id: hand.id, ok: true } }]);
  assert.equal((await view("all")).items.filter(each => each.number === 700).length, 1);
  // Handed back here before this box heard it arrived there (its receipt lost): it waits.
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  await tool("queue_add", { title: "Round trip" }, "ses_lead");
  await tool("queue_hand", { item: "#100", to: "ses_far" }, "ses_lead");
  const mine = (await route("GET", "kuvo/work-queue/items/100")).body.item;
  await takeFrom(lab, { hand: { ...hand, id: "wqh_fromlab00000009", to: "ses_lead", item: { ...told(100, "ses_far"), id: mine.id } } });
  const waits = mailTo(lab).map(each => each.workQueue.handed);
  assert.equal(waits[0].ok, false);
  assert.equal(waits[0].retry, true);
  assert.equal((await view("all")).items.filter(each => each.id === mine.id).length, 1);
  // For no conversation here.
  await takeFrom(lab, { hand: { ...hand, id: "wqh_fromlab00000002", to: "ses_nobody", item: { ...item, id: "wqi_handedfromlab0000002" } } });
  const refused = mailTo(lab).map(each => each.workQueue.handed);
  assert.equal(refused[0].ok, false);
  assert.match(refused[0].why, /There's no conversation ses_nobody on /);
});

test("a hand-off to a computer that's no longer one of yours comes back after a few days, relay or not; one that waits 30 days too; deleting one calls it off; and what had got there stays there's", async () => {
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  await tool("queue_add", { title: "Stuck" }, "ses_lead");
  assert.equal((await tool("queue_hand", { item: "#100", to: "ses_far" }, "ses_lead")).isError, false);
  // lab drops out of the list of your computers, the only one that took the Work Queue's mail:
  // the hand-off waits a while (a computer can be missing a moment), then comes back, though
  // nothing's told now.
  fresh({ peers: [mac, here] });
  agentsTold({ [mac.pub]: { at: 1, heard: Date.now(), kind: "mac", list: [], features: ["work-queue"] } });
  const now = Date.now();
  await link.pushQueues(host, link.readLink(), targets, now);
  assert.equal((await view("ses_lead")).items.find(item => item.number === 100).handing?.to, "ses_far", "a moment missing: still on its way");
  await link.pushQueues(host, link.readLink(), targets, now + 4 * 86_400_000);
  const back = await tool("queue_show", { item: 100 }, "ses_lead");
  assert.match(back.text, /Couldn't be handed to Far on another computer: that computer hasn't been one of yours for 3 days\. It's still here\./);
  assert.equal((await tool("queue_update", { item: 100, note: "Mine again" }, "ses_lead")).isError, false);

  // One waiting on lab while this box is off the relay: given up after 30 days all the same.
  fresh();
  agentsTold();
  assert.equal((await tool("queue_hand", { item: "#100", to: "ses_far" }, "ses_lead")).isError, false);
  await link.handUpkeep(link.readLink(), Date.now() + 1000);
  assert.equal((await view("ses_lead")).items.find(item => item.number === 100).handing?.to, "ses_far", "on its way still");
  await link.handUpkeep(link.readLink(), Date.now() + 31 * 86_400_000);
  assert.match((await tool("queue_show", { item: 100 }, "ses_lead")).text, /it couldn't be reached for 30 days\. It's still here\./);

  // You delete one on its way: its hand-off is called off, and it goes no more.
  await tool("queue_hand", { item: "#100", to: "ses_far" }, "ses_lead");
  const deleted = await route("DELETE", "kuvo/work-queue/items/100");
  assert.equal(deleted.status, 200, JSON.stringify(deleted.body));
  sent.length = 0;
  await link.pushQueues(host, link.readLink(), targets, Date.now() + 120_000);
  assert.equal(mailTo(lab).filter(each => each.workQueue.hand).length, 0);
  const restored = await route("POST", "kuvo/work-queue/items/100/restore");
  assert.equal(restored.status, 200);
  assert.match((await tool("queue_show", { item: 100 }, "ses_lead")).text, /Its hand-off to Far on another computer was called off\./);
  // An agent's queue_remove calls one off too.
  await tool("queue_hand", { item: "#100", to: "ses_far" }, "ses_lead");
  assert.equal((await tool("queue_remove", { item: 100 }, "ses_lead")).isError, false);
  // One that had got to lab already (its receipt lost) is lab's: the copy deleted here never hides
  // it, and isn't brought back beside it.
  const id = (await view("ses_lead")).removed[0].id;
  receive(lab, { workQueue: telling([told(100, "ses_far", { id, title: "Stuck on lab", updated: { at: Date.now(), by: { user: true } } })]) });
  sent.length = 0;
  assert.equal((await view("all")).items.find(item => item.id === id)?.computer?.name, "lab");
  const twice = await route("POST", "kuvo/work-queue/items/100/restore");
  assert.equal(twice.status, 400);
  assert.match(twice.body.message, /is on lab now \(it got there before its hand-off was called off\): change it there\./);
  // lab hands it back here: the live one takes the deleted copy's place, never answered as had.
  await takeFrom(lab, { hand: { id: "wqh_backhere00000001", to: "ses_lead", from: "ses_far", by: { user: true }, at: Date.now(), item: { ...told(100, "ses_far", { id, title: "Stuck on lab" }), body: "Worked on there.", notes: [], files: [], links: [], handed: [] }, files: [] } });
  assert.deepEqual(mailTo(lab).map(each => each.workQueue), [{ handed: { id: "wqh_backhere00000001", ok: true } }]);
  const home = (await route("GET", `kuvo/work-queue/items/${id}`)).body.item;
  assert.equal(home.removed, undefined);
  assert.equal(home.body, "Worked on there.");
  assert.equal(home.owner, "ses_lead");

  // One lab handed here, deleted here before lab told again: lab's word of it on its way here
  // doesn't show it as lab's.
  const coming = "wqi_handedhereanddelete1";
  await takeFrom(lab, { hand: { id: "wqh_cominghere000001", to: "ses_lead", from: "ses_far", by: { user: true }, at: Date.now(), item: { ...told(300, "ses_far", { id: coming }), body: "", notes: [], files: [], links: [], handed: [] }, files: [] } });
  receive(lab, { workQueue: telling([told(300, "ses_far", { id: coming, handing: { to: "ses_lead", name: "Lead" } })]) });
  sent.length = 0;
  assert.equal((await route("DELETE", `kuvo/work-queue/items/${coming}`)).status, 200);
  assert.equal((await view("all")).items.some(item => item.id === coming), false);
  const deletedHere = await tool("queue_show", { item: coming }, "ses_child");
  assert.equal(deletedHere.isError, false, deletedHere.text);
  assert.match(deletedHere.text, /DELETED/);
  assert.doesNotMatch(deletedHere.text, /on lab/);
  assert.match((await tool("queue_show", { item: coming }, "ses_lead")).text, /DELETED/);
});

test("what happened to an item goes with it to another computer, as its notes do, and comes in with one handed here", async () => {
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  await tool("queue_add", { title: "Port the importer", working: true }, "ses_lead");
  await route("PATCH", "kuvo/work-queue/items/100", { status: "backlog", top: true });
  const events = (await route("GET", "kuvo/work-queue/items/100")).body.item.events;
  assert.deepEqual(events.map(event => [event.kind, event.by]), [["started", { session: "ses_lead", name: "Lead" }], ["moved", { user: true }]]);
  assert.equal((await tool("queue_hand", { item: "#100", to: "ses_far" }, "ses_lead")).isError, false);
  await link.pushQueues(host, link.readLink(), targets, Date.now());
  const hand = mailTo(lab).map(each => each.workQueue).find(each => each.hand)?.hand;
  assert.deepEqual(hand.item.events, events, "it goes whole and a pending transfer is not marked complete");
  await receiveHanded(lab, { id: hand.id, ok: true });

  // One handed here keeps what happened to it there, who and when.
  const far = [{ at: 1500, by: { session: "ses_farlead", name: "Far lead" }, kind: "started" }, { at: 1600, by: { user: true }, kind: "moved", from: "todo", to: "backlog" }];
  const item = told(701, "ses_farlead", { id: "wqi_handedfromlab0000003", body: "", notes: [], files: [], links: [], handed: [], events: far });
  await takeFrom(lab, { hand: { id: "wqh_fromlab00000003", to: "ses_child", from: "ses_farlead", by: { session: "ses_farlead", name: "Far lead" }, at: Date.now(), item, files: [] } });
  assert.equal(mailTo(lab)[0].workQueue.handed.ok, true);
  const completion = { by: { session: "ses_farlead", name: "Far lead" }, kind: "handed", to: "ses_child" };
  const received = (await route("GET", "kuvo/work-queue/items/701")).body.item.events;
  assert.deepEqual(received, [...far, { at: received.at(-1).at, ...completion }]);
  // One handed here from an older box starts its history with the completed hand-off.
  await takeFrom(lab, { hand: { id: "wqh_fromlab00000004", to: "ses_child", from: "ses_farlead", by: { session: "ses_farlead", name: "Far lead" }, at: Date.now(), item: told(702, "ses_farlead", { id: "wqi_handedfromlab0000004", body: "", notes: [], files: [], links: [], handed: [] }), files: [] } });
  assert.deepEqual((await route("GET", "kuvo/work-queue/items/702")).body.item.events.map(({ at: _, ...event }) => event), [completion]);
  // One of a status this box doesn't know (a newer box's) lands in Todo, its feed saying so; a
  // label it has comes with it, and one that didn't is left off.
  await takeFrom(lab, { hand: { id: "wqh_fromlab00000005", to: "ses_child", from: "ses_farlead", by: { session: "ses_farlead", name: "Far lead" }, at: Date.now(), item: told(703, "ses_farlead", { id: "wqi_handedfromlab0000005", status: "triage", labels: ["lbl_far", "lbl_nowhere"], priority: "urgent", due: "2026-12-01", body: "", notes: [], files: [], links: [], handed: [], events: far }), files: [], labels: [{ id: "lbl_far", name: "Far", color: "sky" }] } });
  const landed = (await route("GET", "kuvo/work-queue/items/703")).body.item;
  assert.deepEqual([landed.status, landed.labels, landed.priority, landed.due], ["todo", ["lbl_far"], "urgent", "2026-12-01"]);
  assert.deepEqual((await route("GET", "kuvo/work-queue/labels")).body.labels, [{ id: "lbl_far", name: "Far", color: "sky" }]);
  assert.deepEqual(landed.events.slice(0, 2), far);
  assert.deepEqual(landed.events.slice(2).map(({ at: _, ...event }) => event), [{ by: { source: "kuvo", name: "Kuvo Code" }, kind: "moved", from: "triage", to: "todo" }, completion]);
});

test("a Work Queue handoff works across versions and keeps its receipt", async () => {
  const state = link.readLink();
  const id = "set_" + "a".repeat(22);
  const version = "0.10.0", sha256 = "a".repeat(64);
  const signed = Buffer.from(JSON.stringify({ manifest: Buffer.from(JSON.stringify({ clientProtocol: 1 })).toString("base64") })).toString("base64");
  link.writeLink({ ...state, setup: { id, epoch: 1, primary: mac.pub, role: "member", sharedSaves: true },
    sharedSetup: id, shared: { setupSecurityAndReleaseChoice: { sequence: 1, value: { version, sha256, signed } } } });
  const mark = path.join(home, "bin", "box-version");
  const installed = path.join(home, "updates", "installed.json");
  write(mark, "0.8.0-beta.35");
  const hand = { id: "wqh_fleetwaiting1", to: "ses_child", from: "ses_farlead", by: { user: true }, at: Date.now(),
    item: told(701, "ses_farlead", { id: "wqi_fleetwaiting00000001", body: "", notes: [], files: [], links: [], handed: [] }), files: [] };
  try {
    await takeFrom(lab, { hand });
    assert.deepEqual(mailTo(lab)[0].workQueue.handed, { id: hand.id, ok: true });
    write(mark, version);
    write(installed, { version, sha256 });
    await takeFrom(lab, { hand });
    assert.deepEqual(mailTo(lab)[0].workQueue.handed, { id: hand.id, ok: true });
    write(mark, "0.8.0-beta.35");
    await takeFrom(lab, { hand });
    assert.deepEqual(mailTo(lab)[0].workQueue.handed, { id: hand.id, ok: true });
  } finally {
    fs.rmSync(mark, { force: true });
    fs.rmSync(installed, { force: true });
  }
});

test("an item handed here without a number takes one as it arrives", async () => {
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  const { number: _, ...item } = told(1, "ses_farlead", { id: "wqi_handedwithoutnumber1", body: "", notes: [], files: [], links: [], handed: [] });
  await takeFrom(lab, { hand: { id: "wqh_nonumber00000001", to: "ses_child", from: "ses_farlead", by: { user: true }, at: Date.now(), item, files: [] } });
  assert.deepEqual(mailTo(lab).map(each => each.workQueue), [{ handed: { id: "wqh_nonumber00000001", ok: true } }]);
  assert.equal((await route("GET", "kuvo/work-queue/items/wqi_handedwithoutnumber1")).body.item.number, 100);
});

test("an outside source's item never takes a number an import claimed", async () => {
  const file = path.join(queues, "numbers.json");
  write(file, { primary: mac.pub, leases: [], asking: { id: "wqn_sourcesource01", count: 50, since: 1, claims: [[40, 41]] } });
  await tool("queue_add", { title: "Ours" }, "ses_lead");
  const lead = path.join(queues, "queues", "ses_lead.json");
  const kept = read(lead);
  kept.items.push({ ...kept.items[0], id: "wqi_sourcewantedsource01", wanted: 40, title: "An issue", source: { kind: "github", ref: "gh:o/r#1" } });
  kept.items.push({ ...kept.items[0], id: "wqi_importedwantedimp001", wanted: 41, title: "Imported" });
  write(lead, kept);
  assert.equal(link.blockArrived({ id: "wqn_sourcesource01", from: 300, to: 349, granted: [[40, 41]] }, link.primaryOf(link.readLink())), true);
  const items = (await view("ses_lead")).items;
  assert.equal(items.find(item => item.title === "An issue").number, undefined);
  assert.equal(items.find(item => item.title === "Imported").number, 41);
});

test("a duplicate told by two computers (one handed between them, the first not told again yet) shows once, as the newer word says", async () => {
  const id = "wqi_handedbetweenboxes01";
  receive(mac, { workQueue: telling([told(800, "ses_far", { id, title: "Stale", updated: { at: 2000, by: { user: true } } })], { kind: "mac" }) });
  receive(lab, { workQueue: telling([told(800, "ses_far", { id, title: "Live", updated: { at: 9000, by: { user: true } } })]) });
  sent.length = 0;
  const shown = (await view("all")).items.filter(item => item.id === id);
  assert.deepEqual(shown.map(item => [item.title, item.computer.name]), [["Live", "lab"]]);
  assert.equal((await route("GET", "kuvo/work-queue/refs?refs=800")).body.refs["800"].computer.name, "lab");
});

test("a change from a client tells your other computers sooner, never more often than every 15 seconds, and not what they have", async () => {
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  await tool("queue_add", { title: "First" }, "ses_lead");
  const now = Date.now();
  await link.pushQueues(host, link.readLink(), targets, now);
  assert.equal(mailTo(lab).length, 1);
  sent.length = 0;
  // You move it: lab hasn't said it has the first yet. Not within 15 seconds of the last.
  link.tellQueuesAgain();
  await link.pushQueues(host, link.readLink(), targets, now + 1000);
  assert.equal(mailTo(lab).length, 0, "not within 15 seconds");
  await link.pushQueues(host, link.readLink(), targets, now + 16_000);
  const again = mailTo(lab);
  assert.equal(again.length, 1, "sooner than a minute");
  sent.length = 0;
  // Had: a client's change that changed nothing told sends nothing.
  receive(lab, { workQueue: { had: again[0].workQueue.told.id } });
  receive(mac, { workQueue: { had: again[0].workQueue.told.id } });
  sent.length = 0;
  link.tellQueuesAgain();
  await link.pushQueues(host, link.readLink(), targets, now + 40_000);
  assert.equal(mailTo(lab).length, 0);
});

test("the primary's labels are this box's: added, renamed, recoloured and removed as it says, one it removes coming off every item here; this box changes none of them, and keeps its own", async () => {
  inSetup();
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  const names = async () => (await route("GET", "kuvo/work-queue/labels")).body.labels.map(each => `${each.id}=${each.name}`);
  receive(mac, { workQueue: telling([], { kind: "mac", labels: [{ id: "lbl_design", name: "Design", color: "sky" }, { id: "lbl_review", name: "Review", color: "mint" }, { id: "lbl_used", name: "Used", color: "pink" }] }) });
  assert.deepEqual(await names(), ["lbl_design=Design", "lbl_review=Review", "lbl_used=Used"]);
  const labelsFile = path.join(queues, "labels.json");
  // The queue summary accepts clock resets. Two older label lists still cannot
  // establish that Design or Used was deleted.
  for (const age of [700_000, 690_000]) {
    // Emulate an older label file, then a restart before it recorded a watermark.
    const { primaryAt: _clock, ...legacyLabels } = JSON.parse(fs.readFileSync(labelsFile, "utf8"));
    write(labelsFile, legacyLabels);
    receive(mac, { workQueue: telling([], { kind: "mac", at: Date.now() - age, labels: [{ id: "lbl_review", name: "Review", color: "mint" }] }) });
    assert.deepEqual(await names(), ["lbl_design=Design", "lbl_review=Review", "lbl_used=Used"]);
  }
  // A partially understood report is also insufficient to prove deletion.
  receive(mac, { workQueue: telling([], { kind: "mac", labels: [
    { id: "lbl_review", name: "Review", color: "mint" }, { id: "newLabelFormat" },
  ] }) });
  assert.deepEqual(await names(), ["lbl_design=Design", "lbl_review=Review", "lbl_used=Used"]);
  await tool("queue_labels", { add: "Mine" }, "ses_lead");
  await tool("queue_add", { title: "In use", labels: ["Used"] }, "ses_lead");
  await tool("queue_update", { item: 100, labels: ["Used", "Mine"] }, "ses_lead");
  // A label that's the primary's is changed and removed there: here it's refused, saying so, by
  // you and by an agent alike, and stays as it is.
  const words = change => `Used is one of the labels this computer takes from your first computer (the one that gives the Work Queue its numbers). ${change} it there, and it's ${change === "Remove" ? "gone" : "changed"} here too.`;
  assert.deepEqual((await route("POST", "kuvo/work-queue/labels", { remove: "Used" })).body, { message: words("Remove") });
  assert.deepEqual((await route("POST", "kuvo/work-queue/labels", { change: "Used", name: "Used up" })).body, { message: words("Change") });
  assert.deepEqual(await tool("queue_labels", { remove: "Used" }, "ses_lead"), { isError: true, text: words("Remove") });
  assert.deepEqual((await view("ses_lead")).items[0].labels, ["lbl_used", "lbl_mine"]);
  // The primary removes it (and Design), and renames another: gone here with its word, off every
  // item that had it; this box's own label stays, on the item too.
  receive(mac, { workQueue: telling([], { kind: "mac", labels: [{ id: "lbl_review", name: "Reviewing", color: "violet" }] }) });
  assert.deepEqual(await names(), ["lbl_review=Reviewing", "lbl_mine=Mine"]);
  assert.equal((await route("GET", "kuvo/work-queue/labels")).body.labels[0].color, "violet", "its colours too");
  assert.deepEqual((await view("ses_lead")).items[0].labels, ["lbl_mine"]);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(queues, "queues", "ses_lead.json"), "utf8")).items[0].labels, ["lbl_mine"], "as it's kept, not only as it's shown");
  assert.deepEqual(JSON.parse(fs.readFileSync(labelsFile, "utf8")).removed, ["lbl_design", "lbl_used"]);
  // A later report carrying a stale copy cannot undo the deletion.
  receive(mac, { workQueue: telling([], { kind: "mac", labels: [
    { id: "lbl_review", name: "Reviewing", color: "violet" }, { id: "lbl_used", name: "Used", color: "pink" },
  ] }) });
  assert.deepEqual(await names(), ["lbl_review=Reviewing", "lbl_mine=Mine"]);
  assert.deepEqual((await view("ses_lead")).items[0].labels, ["lbl_mine"]);
  // Its own it removes as ever.
  assert.equal((await route("POST", "kuvo/work-queue/labels", { remove: "Mine" })).status, 200);
  await tool("queue_labels", { add: "Mine" }, "ses_lead");
  // Not from another computer than the primary.
  receive(lab, { workQueue: telling([], { labels: [{ id: "lbl_labs", name: "Lab's", color: "coral" }] }) });
  assert.deepEqual(await names(), ["lbl_review=Reviewing", "lbl_mine-2=Mine"]);
  // An empty list from the same main cannot delete the labels it already told this box.
  receive(mac, { workQueue: telling([], { kind: "mac", labels: [] }) });
  assert.deepEqual(await names(), ["lbl_review=Reviewing", "lbl_mine-2=Mine"]);
  sent.length = 0;
});

test("an issue another computer told of shows as it said, with no gh here; this box reads it only once what was told is old; the newer word wins, never one from a clock set ahead", async () => {
  write(path.join(run, "work", "elsewhere", `${lab.pub}.json`), { at: Date.now(), heard: Date.now(), leads: {} });
  const issue = seen => ({ ...told(510, "ses_far", { title: "Upload retries", status: "done", keptIn: "backlog", seen,
    source: { kind: "github", ref: "gh:kuvo-test/app#12", url: "https://github.com/kuvo-test/app/issues/12", host: "github.com", id: "I_kw12" } }), number: undefined });
  const shown = async () => (await view("all")).items.find(item => item.source?.ref === "gh:kuvo-test/app#12");

  // Told a moment ago that it's closed (as it has been for two hours): shown so, GitHub not asked.
  receive(lab, { workQueue: telling([issue({ state: "completed", at: Date.now() - 7_200_000, checked: Date.now() - 30_000 })]) });
  let item = await shown();
  assert.deepEqual([item.status, item.keptIn, item.seen.state, item.source.name, item.computer.name], ["done", "backlog", "completed", "app#12", "lab"]);
  assert.equal(ghAsks(), 0, "lab reads what it shows, and told it lately");

  // A clock set ahead there: what it says is no later than when it told it.
  receive(lab, { workQueue: telling([issue({ state: "completed", at: Date.now() + 3_600_000, checked: Date.now() + 3_600_000 })]) });
  item = await shown();
  assert.ok(item.seen.at <= Date.now() && item.seen.checked <= Date.now());

  // Told long ago that it's open: read here, and GitHub's word now (reopened, then closed again) wins.
  receive(lab, { workQueue: telling([issue({ state: "open", at: Date.now() - 7_200_000, checked: Date.now() - 600_000 })]) });
  write(ghAnswer, { data: { p0: { issueOrPullRequest: { __typename: "Issue", id: "I_kw12", number: 12, title: "Upload retries drop the name", url: "https://github.com/kuvo-test/app/issues/12", state: "CLOSED", stateReason: "NOT_PLANNED", closedAt: "2026-11-10T09:00:00Z" } } } });
  item = await shown();
  assert.equal(ghAsks(), 1);
  assert.deepEqual([item.status, item.seen.state, item.struck, item.title], ["done", "not_planned", true, "Upload retries drop the name"]);

  // (This box's read a second earlier than it was: what lab tells next is surely later.)
  const seenFile = path.join(queues, "seen.json");
  const kept = read(seenFile);
  for (const each of Object.values(kept.reads)) { each.at -= 1000; if (each.checked) each.checked -= 1000; }
  write(seenFile, kept);
  // Then lab tells it asked GitHub after this box did, and it says open (as it has for two hours,
  // by lab's word): lab's, as the later word.
  receive(lab, { workQueue: telling([issue({ state: "open", at: Date.now() - 7_200_000, checked: Date.now() })]) });
  item = await shown();
  assert.deepEqual([item.status, item.seen.state, item.struck], ["backlog", "open", undefined]);
  assert.equal(ghAsks(), 1, "nor asked again while lab's word is fresh");
});

// ---- What you do to an item another computer keeps (PROTOCOL.md, "The Work Queue", What you do to one)

// Lab and the Mac take what you do from another box.
const forwardsTold = (attachments = false) => agentsTold({
  [lab.pub]: { at: 1, heard: Date.now(), kind: "machine", id: "lab", list: [], features: ["work-queue", "work-queue.elsewhere", "work-queue.forward", "work-queue.edit", ...(attachments ? ["work-queue.attachments"] : [])] },
  [mac.pub]: { at: 1, heard: Date.now(), kind: "mac", list: [], features: ["work-queue", "work-queue.elsewhere", "work-queue.forward", "work-queue.edit"] },
});
// This box's relay connection as its link has it: what goes to `to`, as it's sent.
const relay = () => {
  let waiting = null;
  const connection = { welcomed: true, send: bytes => { sent.push(bytes); waiting?.(); } };
  return { connection, sentTo: async to => {
    for (;;) {
      const mails = mailTo(to);
      if (mails.length) return mails;
      await new Promise(resolve => { waiting = resolve; });
    }
  } };
};
const actID = () => `wqa_${crypto.randomBytes(16).toString("base64url")}`;
const forwards = path.join(queues, "forward");

test("what you do to an item another computer keeps, asked of this box, is done there: this box hands it to that computer's box and answers with what it said", async () => {
  forwardsTold();
  receive(lab, { workQueue: telling([told(500, "ses_far"), told(501, "ses_far", { status: "backlog" })]) });
  sent.length = 0;
  // This box's link sends what's left as it's left.
  const { connection, sentTo } = relay();
  link.queueForwardsSoon.send = () => link.sendQueueForwards(connection);

  const unsupported = await route("PATCH", "kuvo/work-queue/items/500", { body: "Image", files: [{ uri: "data:image/png;base64,eA==", name: "image.png" }] });
  assert.equal(unsupported.status, 409);
  assert.match(unsupported.body.message, /needs an update/);
  assert.equal(mailTo(lab).length, 0, "never silently drop attachments on an older peer");

  forwardsTold(true);
  assert.equal((await view("all")).items.find(item => item.number === 500).can.attach, true, "lab keeps attached files now");
  // A file of this computer named by a phone, or more than goes between boxes at once: refused
  // here, nothing sent.
  const named = path.join(home, "named.png");
  fs.writeFileSync(named, "x");
  const fromPhone = await route("PATCH", "kuvo/work-queue/items/500", { files: [{ uri: new URL(`file://${named}`).href }] }, { fromApp: true });
  assert.equal(fromPhone.status, 400);
  const tooBig = await route("PATCH", "kuvo/work-queue/items/500", { files: [1, 2].map(n => ({ uri: "data:image/png;base64," + Buffer.alloc(2_100_000).toString("base64"), name: `${n}.png` })) });
  assert.equal(tooBig.status, 413);
  assert.match(tooBig.body.message, /too big to send to lab/);
  assert.equal(mailTo(lab).length, 0, "nothing went to lab");
  fs.rmSync(named);
  // A picture of this computer goes to lab with its bytes: lab never reads a path it's sent.
  const attaching = path.join(home, "shot.png");
  fs.writeFileSync(attaching, "x");
  const files = [{ uri: new URL(`file://${attaching}`).href, name: "image.png" }];
  // Moved: the move goes to lab, and lab's answer is this box's.
  const moving = route("PATCH", `kuvo/work-queue/items/500`, { status: "backlog", top: true, files }, { fromApp: true, served: true });
  const [asked] = await sentTo(lab);
  const act = asked.workQueue.act;
  assert.deepEqual({ method: act.method, action: act.action, item: act.item, body: act.body }, { method: "PATCH", action: undefined, item: told(500).id, body: { status: "backlog", top: true, files: [{ uri: "data:image/png;base64,eA==", name: "image.png" }] } });
  fs.rmSync(attaching);
  // (Only from the box it went to: one from the Mac for it isn't lab's answer.)
  receive(mac, { workQueue: { acted: { id: act.id, status: 200, value: { item: told(500, "ses_far", { status: "todo" }) } } } });
  receive(lab, { workQueue: { acted: { id: act.id, status: 200, value: { item: told(500, "ses_far", { status: "backlog", order: 0 }) } } } });
  const moved = await moving;
  assert.equal(moved.status, 200);
  assert.equal(moved.body.item.status, "backlog", "lab's word, not the Mac's");
  assert.deepEqual(moved.body.item.computer, { key: lab.pub, name: "lab", kind: "machine", id: "lab" }, "said to be on lab, as this box names it");
  assert.deepEqual(fs.readdirSync(forwards), [], "nothing left behind");
  assert.equal((await view("all")).items.find(item => item.number === 500).status, "backlog", "shown where it went at once, not when lab next tells");
  // Lab tells something newer while its answer is on the way: lab's newer word stays.
  const racing = route("PATCH", `kuvo/work-queue/items/500`, { status: "todo", top: true });
  const [raced] = await sentTo(lab);
  receive(lab, { workQueue: telling([told(500, "ses_far", { status: "done", updated: { at: 9000, by: { session: "ses_far", name: "Far" } } }), told(501, "ses_far", { status: "backlog" })]) });
  mailTo(lab);
  receive(lab, { workQueue: { acted: { id: raced.workQueue.act.id, status: 200, value: { item: told(500, "ses_far", { status: "todo" }) } } } });
  assert.equal((await racing).status, 200);
  assert.equal((await view("all")).items.find(item => item.number === 500).status, "done", "never over what lab told since");

  // Refused there: lab's words, as lab said them.
  const restoring = route("POST", "kuvo/work-queue/items/501/restore");
  const [again] = await sentTo(lab);
  assert.equal(again.workQueue.act.action, "restore");
  receive(lab, { workQueue: { acted: { id: again.workQueue.act.id, status: 400, value: { message: "It isn't deleted." } } } });
  assert.deepEqual(await restoring, { status: 400, body: { message: "It isn't deleted.", computer: { key: lab.pub, name: "lab", kind: "machine", id: "lab" } } });

  // Read whole, from a client: lab's whole item, said to be read there.
  const reading = route("GET", "kuvo/work-queue/items/500");
  const [read] = await sentTo(lab);
  assert.deepEqual({ method: read.workQueue.act.method, item: read.workQueue.act.item }, { method: "GET", item: told(500).id });
  receive(lab, { workQueue: { acted: { id: read.workQueue.act.id, status: 200, value: { item: { ...told(500, "ses_far"), can: { edit: true, set: true, assign: true, close: true, delete: true, restore: false }, body: "What only lab keeps", notes: [], events: [], files: [], links: [], handed: [] } } } } });
  const whole = await reading;
  assert.equal(whole.body.item.body, "What only lab keeps");
  assert.equal(whole.body.item.told, false);
  // (Who it's for is set on the computer that keeps it: this one lists only its own bots.)
  assert.deepEqual([whole.body.item.can.assign, whole.body.item.can.set], [false, true]);
  assert.equal(whole.body.item.computer.id, "lab");
  // One of its files, for a client: from lab, its bytes as they are.
  const fetching = route("GET", "kuvo/work-queue/items/500/files/notes%20one.txt");
  const [file] = await sentTo(lab);
  assert.deepEqual({ method: file.workQueue.act.method, action: file.workQueue.act.action, name: file.workQueue.act.name }, { method: "GET", action: "file", name: "notes one.txt" });
  receive(lab, { workQueue: { acted: { id: file.workQueue.act.id, status: 200, value: { type: "text/plain", data: Buffer.from("From lab.").toString("base64") } } } });
  assert.deepEqual(await fetching, { status: 200, body: "From lab." });
  // A picture too big for one mail: in parts, put together here.
  const picture = crypto.randomBytes(120_000);
  const fetchingPicture = (async () => {
    const pieces = [];
    for await (const piece of handle({ method: "GET", path: "kuvo/work-queue/items/500/files/shot.png", headers: {}, fromApp: true }, undefined)) pieces.push(piece);
    return { status: pieces.find(each => each.head).head.status, bytes: Buffer.concat(pieces.filter(each => each.body).map(each => each.body)) };
  })();
  const [shot] = await sentTo(lab);
  const answer = { acted: { id: shot.workQueue.act.id, status: 200, value: { type: "image/png", data: picture.toString("base64") } } };
  const d = Buffer.from(JSON.stringify(answer)).toString("base64url");
  const of = Math.ceil(d.length / 40_000);
  assert.ok(of > 1);
  const digest = crypto.createHash("sha256").update(d).digest("base64url");
  for (let i = 0; i < of; i++) receive(lab, { workQueue: { part: { whole: digest, i, of, d: d.slice(i * 40_000, (i + 1) * 40_000), s: "" } } });
  const got = await fetchingPicture;
  assert.equal(got.status, 200);
  assert.ok(got.bytes.equals(picture), "its bytes as they are");
  // An answer that isn't a status is never taken.
  const odd = route("GET", "kuvo/work-queue/items/500/files/big.png");
  const [big] = await sentTo(lab);
  receive(lab, { workQueue: { acted: { id: big.workQueue.act.id, status: 1000, value: {} } } });
  receive(lab, { workQueue: { acted: { id: big.workQueue.act.id, status: 413, value: { message: "big.png is 9 MB: too big to bring from lab (4 MB at most). Open it there." } } } });
  assert.deepEqual(await odd, { status: 413, body: { message: "big.png is 9 MB: too big to bring from lab (4 MB at most). Open it there.", computer: { key: lab.pub, name: "lab", kind: "machine", id: "lab" } } });
  // Anyone else on this computer (an agent's `box route`, unproven) reads it only as lab told it.
  const pieces = [];
  for await (const piece of handle({ method: "GET", path: "kuvo/work-queue/items/500", headers: {}, fromMac: true }, undefined)) pieces.push(piece);
  const told500 = JSON.parse(Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString());
  assert.deepEqual([told500.item.body, told500.item.told], ["", true]);
  assert.deepEqual(mailTo(lab), [], "and lab isn't asked");
  // And never changes it: only a client does.
  const refused = [];
  for await (const piece of handle({ method: "DELETE", path: "kuvo/work-queue/items/500", headers: {}, fromMac: true }, undefined)) refused.push(piece);
  assert.equal(refused.find(each => each.head).head.status, 403);
  assert.deepEqual(mailTo(lab), []);
});

test("a Mac's request over SSH (`box route`, a process with no relay connection) is left for this box's link, which sends it and leaves lab's answer for it", async () => {
  forwardsTold();
  receive(lab, { workQueue: telling([told(500, "ses_far")]) });
  sent.length = 0;
  // (And one left by something else on this computer.)
  const planted = actID();
  write(path.join(forwards, `${planted}.json`), { to: lab.pub, act: { id: planted, method: "DELETE", item: told(500).id, at: Date.now() } });
  // (`box route`: nothing in this process sends; the link, another process, finds what's left. Its
  // leaving is waited for as the route says so, not by a watch of the folder, which on macOS never
  // tells a change as it starts: a stall under load. And never past the route's own answer.)
  const theRoutes = () => fs.readdirSync(forwards).some(name => /^wqa_.+\.json$/.test(name) && name !== `${planted}.json` && !name.endsWith(".answer.json"));
  const left = new Promise(resolve => { link.queueForwardsSoon.send = resolve; });
  const deleting = route("DELETE", "kuvo/work-queue/items/500");
  const first = await Promise.race([left.then(() => "left"), deleting.then(() => "answered", () => "answered")]);
  link.queueForwardsSoon.send = () => {};
  assert.equal(first, "left", "the route leaves its ask for the link before it answers");
  assert.ok(theRoutes(), "its ask is in the folder, for the link");
  const { connection } = relay();
  // This Mac's box, whose routes run where it sends, sends only what they left: what anything else
  // on the Mac leaves there never goes out as you.
  link.sendQueueForwards(connection, undefined, undefined, { ownOnly: true });
  const [asked, ...others] = mailTo(lab);
  assert.deepEqual(others, []);
  assert.notEqual(asked.workQueue.act.id, planted);
  fs.rmSync(path.join(forwards, `${planted}.json`));
  assert.equal(asked.workQueue.act.method, "DELETE");
  // Sent again only after a few seconds while it's waited for (lab takes each id once anyway).
  link.sendQueueForwards(connection);
  assert.deepEqual(mailTo(lab), []);
  link.sendQueueForwards(connection, undefined, Date.now() + 6_000);
  assert.equal(mailTo(lab)[0].workQueue.act.id, asked.workQueue.act.id);
  receive(lab, { workQueue: { acted: { id: asked.workQueue.act.id, status: 200, value: { item: told(500, "ses_far", { removed: { at: 3000, by: { user: true } } }) } } } });
  const deleted = await deleting;
  assert.equal(deleted.status, 200);
  assert.equal(deleted.body.item.removed.by.user, true);
  assert.deepEqual(fs.readdirSync(forwards), []);
  // Gone from this box's views at once, beside Undo; and Undo still brings it back on lab once lab
  // has told without it.
  assert.equal((await view("all")).items.some(item => item.number === 500), false);
  receive(lab, { workQueue: telling([]) });
  mailTo(lab);
  // It stays out of lookups for chips. Explicit history reads use the remembered keeper.
  assert.equal((await route("GET", "kuvo/work-queue/refs?refs=500")).body.refs["500"], null);
  // (Lab doesn't answer the read: as it last told it, without waiting out a real wait.)
  const wait = link.queueForwardTiming.wait;
  link.queueForwardTiming.wait = 0;
  const agentHistory = await tool("queue_show", { item: 500 }, "ses_lead");
  assert.equal(agentHistory.isError, false, agentHistory.text);
  assert.match(agentHistory.text, /^#500 "Item 500" · Todo · DELETED · on lab/);
  assert.match(agentHistory.text, /Last shared summary/);
  assert.equal((await route("GET", "kuvo/work-queue/items/500")).body.item.told, true);
  link.queueForwardTiming.wait = wait;
  // Told again by lab (brought back there some other way): forgotten here.
  const deletedThere = path.join(queues, "deleted-there.json");
  const keptForUndo = fs.readFileSync(deletedThere, "utf8");
  receive(lab, { workQueue: telling([told(500, "ses_far")]) });
  assert.equal(fs.existsSync(deletedThere), false);
  fs.writeFileSync(deletedThere, keptForUndo);
  receive(lab, { workQueue: telling([]) });
  mailTo(lab);
  const again = relay();
  link.queueForwardsSoon.send = () => link.sendQueueForwards(again.connection);
  const undoing = route("POST", `kuvo/work-queue/items/${told(500).id}/restore`);
  const [undo] = await again.sentTo(lab);
  assert.equal(undo.workQueue.act.action, "restore");
  receive(lab, { workQueue: { acted: { id: undo.workQueue.act.id, status: 200, value: { item: told(500, "ses_far") } } } });
  assert.equal((await undoing).status, 200);
  assert.equal((await view("all")).items.some(item => item.number === 500), true, "back at once");
  link.queueForwardsSoon.send = () => {};
  // One whose asker went (its process stopped mid-wait), and an answer left with no ask: cleared at
  // the link's next round, never sent.
  const stale = actID();
  write(path.join(forwards, `${stale}.json`), { to: lab.pub, act: { id: stale, method: "DELETE", item: told(500).id, at: Date.now() - 60_000 } });
  write(path.join(forwards, `${actID()}.answer.json`), { status: 200, value: {} });
  link.sendQueueForwards(connection);
  assert.deepEqual(mailTo(lab), []);
  assert.deepEqual(fs.readdirSync(forwards), []);
});

test("a computer that keeps the item but isn't answering: said at once, plainly, and nothing goes; one that doesn't take it from another box yet is changed there, as before", async () => {
  forwardsTold();
  receive(lab, { workQueue: telling([told(500, "ses_far")]) });
  sent.length = 0;
  const { connection } = relay();
  link.queueForwardsSoon.send = () => link.sendQueueForwards(connection);
  write(path.join(run, "peer-reach.json"), { [lab.pub]: Date.now() - 60_000 });
  const away = await route("DELETE", "kuvo/work-queue/items/500");
  assert.equal(away.status, 503);
  assert.match(away.body.message, /^lab isn't answering now \(it may be asleep, off or offline\), so nothing was changed there\. Try again once it's back\.$/);
  assert.equal(away.body.computer.id, "lab");
  assert.deepEqual(mailTo(lab), []);
  // Read whole: as lab last told it, and said so.
  const page = await route("GET", "kuvo/work-queue/items/500");
  assert.deepEqual([page.status, page.body.item.title, page.body.item.told], [200, "Item 500", true]);
  assert.deepEqual(mailTo(lab), []);

  // Sent, and lab stops answering before it answers: it may have been done, and it says so.
  fs.rmSync(path.join(run, "peer-reach.json"));
  const { sentTo } = relay();
  const stopping = route("PATCH", "kuvo/work-queue/items/500", { status: "backlog" });
  await sentTo(lab);
  write(path.join(run, "peer-reach.json"), { [lab.pub]: Date.now() });
  const stopped = await stopping;
  assert.equal(stopped.status, 503);
  assert.match(stopped.body.message, /^lab stopped answering, so devbox can't tell whether it was done there\. Look again once it's back\.$/);
  // No answer in time: the same.
  fs.rmSync(path.join(run, "peer-reach.json"));
  link.queueForwardTiming.wait = 0;
  const late = await route("DELETE", "kuvo/work-queue/items/500");
  assert.equal(late.status, 503);
  assert.match(late.body.message, /^lab didn't answer in time, so devbox can't tell whether it was done there\. Look again in a moment\.$/);
  link.queueForwardTiming.wait = 20_000;
  mailTo(lab);

  fresh();

  // A lab from before: changed there, as it was.
  agentsTold();
  const older = await route("PATCH", "kuvo/work-queue/items/500", { status: "backlog" });
  assert.equal(older.status, 409);
  assert.match(older.body.message, /#500 "Item 500" is on lab: change it there\./);
  assert.deepEqual(mailTo(lab), []);
});

test("an item no computer told this box of, asked of this box by a client, comes from the computer that keeps it: read whole, its files and what you do to it; on none, not found; while one can't say, it may be there, said plainly with which", async () => {
  forwardsTold();
  // (Lab told of #500 only: #7, an old Done one on the Mac, is in nothing this box was told.)
  receive(lab, { workQueue: telling([told(500, "ses_far")]) });
  sent.length = 0;
  const { connection, sentTo: mailSentTo } = relay();
  link.queueForwardsSoon.send = () => link.sendQueueForwards(connection);
  // What this box sends `to` for the request under way: never waited for past that request's answer.
  let pending = null;
  const ask = (...request) => (pending = route(...request));
  const sentTo = to => Promise.race([mailSentTo(to), pending.then(answer => assert.fail(`it answered ${answer.status} ${JSON.stringify(answer.body)} and asked ${to.peer.name} nothing`))]);
  const studio = { key: mac.pub, name: "Studio", kind: "mac" };
  const whole7 = { ...told(7, "ses_lead", { status: "done" }), can: { edit: true, set: true, assign: true, close: true, delete: true, restore: false }, body: "What only the Mac keeps", notes: [], events: [], files: [{ name: "notes.txt", type: "text/plain", size: 9 }], links: [], handed: [] };
  const gone = { status: 404, value: { message: "It's no longer on lab: it may have moved. Look again in a moment." } };
  // Both asked for it, as the client named it; each answers `said` (by its name).
  const asked = async said => {
    const acts = {};
    for (const each of [lab, mac]) {
      const [mail] = await sentTo(each);
      acts[each.peer.name] = mail.workQueue.act;
      const answer = said[each.peer.name];
      if (answer) receive(each, { workQueue: { acted: { id: mail.workQueue.act.id, ...answer } } });
    }
    return acts;
  };

  // Read: every other computer is asked, and the Mac, which keeps it, answers it whole.
  const reading = ask("GET", "kuvo/work-queue/items/7");
  const acts = await asked({ lab: gone, Studio: { status: 200, value: { item: whole7 } } });
  assert.deepEqual([acts.lab, acts.Studio].map(act => ({ method: act.method, item: act.item })), [{ method: "GET", item: "7" }, { method: "GET", item: "7" }]);
  const read = await reading;
  assert.equal(read.status, 200);
  assert.deepEqual([read.body.item.title, read.body.item.body, read.body.item.told], ["Item 7", "What only the Mac keeps", false]);
  assert.deepEqual(read.body.item.computer, studio, "said to be on the Mac, as this box names it");
  // (What may be done to it is the Mac's word, but for who it's for: set there, among its bots.)
  assert.deepEqual(read.body.item.can, { edit: true, set: true, assign: false, close: true, delete: true, restore: false, attach: false }, "nor attaching, as this box doesn't know it keeps files");
  assert.deepEqual(fs.readdirSync(forwards), [], "nothing left behind");

  // One of its files: found the same way, then its bytes from the Mac.
  const fetching = ask("GET", "kuvo/work-queue/items/7/files/notes.txt");
  await asked({ lab: gone, Studio: { status: 200, value: { item: whole7 } } });
  const [file] = await sentTo(mac);
  assert.deepEqual({ method: file.workQueue.act.method, action: file.workQueue.act.action, item: file.workQueue.act.item, name: file.workQueue.act.name }, { method: "GET", action: "file", item: told(7).id, name: "notes.txt" });
  receive(mac, { workQueue: { acted: { id: file.workQueue.act.id, status: 200, value: { type: "text/plain", data: Buffer.from("From Mac.").toString("base64") } } } });
  assert.deepEqual(await fetching, { status: 200, body: "From Mac." });

  // Changed: found, then done on the Mac, by its id; the answer is the Mac's.
  const moving = ask("PATCH", "kuvo/work-queue/items/7", { status: "todo" });
  await asked({ lab: gone, Studio: { status: 200, value: { item: whole7 } } });
  const [change] = await sentTo(mac);
  assert.deepEqual({ method: change.workQueue.act.method, item: change.workQueue.act.item, body: change.workQueue.act.body }, { method: "PATCH", item: told(7).id, body: { status: "todo" } });
  receive(mac, { workQueue: { acted: { id: change.workQueue.act.id, status: 200, value: { item: told(7, "ses_lead", { status: "todo" }) } } } });
  const moved = await moving;
  assert.deepEqual([moved.status, moved.body.item.status, moved.body.item.computer], [200, "todo", studio]);
  mailTo(lab);
  mailTo(mac);

  // On no computer, and every one said so: not found.
  const missing = ask("GET", "kuvo/work-queue/items/8");
  await asked({ lab: gone, Studio: gone });
  assert.deepEqual(await missing, { status: 404, body: { message: "There's no such item." } });
  // An answer that isn't the item asked for is no answer: the Mac couldn't say, so it may be there.
  const other = ask("GET", "kuvo/work-queue/items/8");
  await asked({ lab: gone, Studio: { status: 200, value: { item: whole7 } } });
  assert.deepEqual(await other, { status: 503, body: { message: "It isn't on devbox, and Studio couldn't say, so it may be there.", computers: [studio] } });
  // One that refuses (its clock is far off): its own words, and no "try again", which wouldn't help.
  const apart = ask("GET", "kuvo/work-queue/items/8");
  await asked({ lab: gone, Studio: { status: 409, value: { message: "Studio's clock and devbox's are too far apart to take it: check both computers' time." } } });
  assert.equal((await apart).body.message, `It isn't on devbox, and Studio couldn't say ("Studio's clock and devbox's are too far apart to take it: check both computers' time."), so it may be there.`);
  // Deleted on lab, live on the Mac (an issue is one item): the live one, whichever answers first.
  const deleted7 = { ...whole7, removed: { at: 3000, by: { user: true } } };
  const twice = ask("GET", "kuvo/work-queue/items/7");
  await asked({ lab: { status: 200, value: { item: deleted7 } }, Studio: { status: 200, value: { item: whole7 } } });
  assert.deepEqual([(await twice).body.item.removed, (await twice).body.item.computer.name], [undefined, "Studio"]);
  // Deleted on lab and on no other: that one, once the Mac said it hasn't it.
  const once = ask("GET", "kuvo/work-queue/items/7");
  await asked({ lab: { status: 200, value: { item: deleted7 } }, Studio: gone });
  assert.deepEqual([(await once).body.item.removed.at, (await once).body.item.computer.name], [3000, "lab"]);

  // Lab isn't answering: it may be there, so it's never "not found". Said at once, with which.
  write(path.join(run, "peer-reach.json"), { [lab.pub]: Date.now() - 60_000 });
  const unsure = ask("GET", "kuvo/work-queue/items/8");
  const [toMac] = await sentTo(mac);
  receive(mac, { workQueue: { acted: { id: toMac.workQueue.act.id, ...gone } } });
  assert.deepEqual(await unsure, { status: 503, body: {
    message: "It isn't on devbox, and lab isn't answering now (it may be asleep, off or offline), so it may be there. Try again once lab answers.",
    computers: [{ key: lab.pub, name: "lab", kind: "machine", id: "lab" }],
  } });
  assert.deepEqual(mailTo(lab), [], "and nothing goes to lab");
  // A change to it: refused the same, nothing done anywhere.
  const changing = ask("DELETE", "kuvo/work-queue/items/8");
  const [again] = await sentTo(mac);
  assert.equal(again.workQueue.act.method, "GET", "looked for, never deleted by its number on a computer that may not keep it");
  receive(mac, { workQueue: { acted: { id: again.workQueue.act.id, ...gone } } });
  assert.equal((await changing).status, 503);
  // But found on the Mac, it's read though lab is away.
  const found = ask("GET", "kuvo/work-queue/items/7");
  const [seven] = await sentTo(mac);
  receive(mac, { workQueue: { acted: { id: seven.workQueue.act.id, status: 200, value: { item: whole7 } } } });
  assert.equal((await found).body.item.number, 7);
  fs.rmSync(path.join(run, "peer-reach.json"));

  // Neither answers in time: both named.
  link.queueForwardTiming.wait = 0;
  const late = await ask("GET", "kuvo/work-queue/items/8");
  assert.equal(late.status, 503);
  assert.match(late.body.message, /^It isn't on devbox, and (lab and Studio|Studio and lab) aren't answering now \(they may be asleep, off or offline\), so it may be there\. Try again once they answer\.$/);
  assert.deepEqual(late.body.computers.map(each => each.name).sort(), ["Studio", "lab"]);
  link.queueForwardTiming.wait = 20_000;
  mailTo(lab);
  mailTo(mac);
  assert.deepEqual(fs.readdirSync(forwards), [], "nothing left behind");

  // A lab from before, which takes nothing from another box: it can't be asked, and that's said.
  agentsTold({ [mac.pub]: { at: 1, heard: Date.now(), kind: "mac", list: [], features: ["work-queue", "work-queue.elsewhere", "work-queue.forward", "work-queue.edit"] } });
  const older = ask("GET", "kuvo/work-queue/items/8");
  const [only] = await sentTo(mac);
  receive(mac, { workQueue: { acted: { id: only.workQueue.act.id, ...gone } } });
  assert.deepEqual((await older).body.message, "It isn't on devbox, and lab needs a newer Kuvo Code to be asked from devbox, so it may be there.");
  assert.deepEqual(mailTo(lab), []);
  forwardsTold();

  // Anyone else on this computer (an agent's `box route`, unproven): only what this box has; no
  // other computer is asked for it.
  const pieces = [];
  for await (const piece of handle({ method: "GET", path: "kuvo/work-queue/items/7", headers: {}, fromMac: true }, undefined)) pieces.push(piece);
  assert.equal(pieces.find(each => each.head).head.status, 404);
  assert.deepEqual([mailTo(lab), mailTo(mac)], [[], []]);
});

test("asked by another of your computers' boxes, this box does it to the item it keeps as its own client would, answers once for each ask, and takes it only from your computers", async () => {
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  await tool("queue_add", { title: "Ship the release" }, "ses_lead");
  await tool("queue_add", { title: "Write the notes" }, "ses_lead");
  const id = (await view("ses_lead")).items.find(item => item.number === 100).id;
  sent.length = 0;
  const act = { id: actID(), method: "PATCH", item: id, body: { status: "backlog" }, at: Date.now() };
  await takeFrom(mac, { act });
  const [answer] = mailTo(mac);
  assert.equal(answer.workQueue.acted.id, act.id);
  assert.equal(answer.workQueue.acted.status, 200);
  assert.equal(answer.workQueue.acted.value.item.status, "backlog");
  assert.equal((await view("ses_lead")).items.find(item => item.number === 100).status, "backlog", "moved here, numbers as they were");
  const moves = async () => (await route("GET", `kuvo/work-queue/items/${id}`)).body.item.events.filter(event => event.kind === "moved");
  assert.deepEqual((await moves()).map(event => [event.by, event.to]), [[{ user: true }, "backlog"]], "by you, in its activity");

  // The same ask again at once (sent again while it was on its way back): not answered twice in a
  // moment; nor done twice, after this box starts again (its answer lost), when it's answered as it was.
  await takeFrom(mac, { act: { ...act, body: { status: "todo" } } });
  assert.deepEqual(mailTo(mac), []);
  link.forgetQueueActs();
  await takeFrom(mac, { act: { ...act, body: { status: "todo" } } });
  assert.deepEqual(mailTo(mac).map(each => each.workQueue.acted), [answer.workQueue.acted]);
  assert.equal((await moves()).length, 1);
  // A file of this computer named by another box: refused, nothing read or kept (it sends bytes).
  const secret = path.join(home, "secret.txt");
  fs.writeFileSync(secret, "not for another box");
  await takeFrom(mac, { act: { id: actID(), method: "PATCH", item: id, body: { files: [{ uri: new URL(`file://${secret}`).href, name: "secret.txt" }] }, at: Date.now() } });
  assert.equal(mailTo(mac)[0].workQueue.acted.status, 400);
  assert.deepEqual((await route("GET", `kuvo/work-queue/items/${id}`)).body.item.files, []);
  fs.rmSync(secret);

  // One that was under way as this box stopped: it can't tell whether it was done.
  const cut = { id: actID(), method: "PATCH", item: id, body: { status: "todo" }, at: Date.now() };
  write(path.join(queues, "acted.json"), { ...read(path.join(queues, "acted.json")), [`${mac.pub} ${cut.id}`]: { until: Date.now() + 600_000, answer: null } });
  await takeFrom(mac, { act: cut });
  assert.deepEqual(mailTo(mac)[0].workQueue.acted, { id: cut.id, status: 409, value: { message: "devbox stopped while it was doing this, so it can't tell whether it was done. Look again in a moment." } });
  assert.equal((await moves()).length, 1);

  // A file kept with it: its bytes, for the box that asked; one that isn't there, 404.
  const files = path.join(queues, "files", id);
  write(path.join(files, "plan.txt"), "The plan.");
  const book = read(path.join(queues, "queues", "ses_lead.json"));
  book.items.find(item => item.id === id).files = [{ name: "plan.txt", size: 9, type: "text/plain", at: 1 }];
  write(path.join(queues, "queues", "ses_lead.json"), book);
  await takeFrom(lab, { act: { id: actID(), method: "GET", action: "file", name: "plan.txt", item: id, at: Date.now() } });
  const [bytes] = mailTo(lab);
  assert.deepEqual([bytes.workQueue.acted.status, bytes.workQueue.acted.value.type, Buffer.from(bytes.workQueue.acted.value.data, "base64").toString()], [200, "text/plain", "The plan."]);
  await takeFrom(lab, { act: { id: actID(), method: "GET", action: "file", name: "none.txt", item: id, at: Date.now() } });
  assert.equal(mailTo(lab)[0].workQueue.acted.status, 404);

  // Read whole, deleted, and refused as the route would.
  const readAsk = { id: actID(), method: "GET", item: "100", at: Date.now() };
  await takeFrom(lab, { act: readAsk });
  const [whole] = mailTo(lab);
  assert.deepEqual([whole.workQueue.acted.status, whole.workQueue.acted.value.item.title, whole.workQueue.acted.value.item.computer], [200, "Ship the release", undefined]);
  await takeFrom(lab, { act: { id: actID(), method: "DELETE", item: id, at: Date.now() } });
  assert.equal(mailTo(lab)[0].workQueue.acted.status, 200);
  const twice = { id: actID(), method: "DELETE", item: id, at: Date.now() };
  await takeFrom(lab, { act: twice });
  assert.deepEqual(mailTo(lab)[0].workQueue.acted, { id: twice.id, status: 400, value: { message: "It's deleted already." } });
  const deleted = async () => (await view("all")).removed.some(item => item.id === id);
  // One from long ago (sent again, or a clock far off): not done.
  const old = { id: actID(), method: "POST", action: "restore", item: id, at: Date.now() - 3_600_000 };
  await takeFrom(lab, { act: old });
  assert.equal(mailTo(lab)[0].workQueue.acted.status, 409);
  assert.equal(await deleted(), true);
  // An ask dated ahead of this clock by more than a minute: refused, as one long ago is. One a little
  // ahead is remembered until it would be too old to take, so it's never taken twice.
  const ahead = { id: actID(), method: "POST", action: "restore", item: id, at: Date.now() + 5 * 60_000 };
  await takeFrom(mac, { act: ahead });
  assert.equal(mailTo(mac)[0].workQueue.acted.status, 409);
  assert.equal(await deleted(), true);
  const doneFor = act => link.queueActForPeer(act, targets);
  const answers = [];
  const start = Date.now();
  const soon = { id: actID(), method: "POST", action: "restore", item: id, at: start + 50_000 };
  await link.takeQueueAct(mac.peer, soon, payload => answers.push(payload.acted), doneFor, start);
  assert.equal(answers[0].status, 200);
  await link.takeQueueAct(mac.peer, { id: actID(), method: "DELETE", item: id, at: start }, payload => answers.push(payload.acted), doneFor, start);
  assert.equal(await deleted(), true);
  link.forgetQueueActs();
  await link.takeQueueAct(mac.peer, soon, payload => answers.push(payload.acted), doneFor, start + 630_000);
  assert.deepEqual(answers.at(-1), answers[0], "answered as it was, not brought back again");
  assert.equal(await deleted(), true);

  // One it knows only as a third computer told it: no longer here, and never handed on (no loops).
  receive(lab, { workQueue: telling([told(500, "ses_far")]) });
  sent.length = 0;
  await takeFrom(mac, { act: { id: actID(), method: "DELETE", item: told(500).id, at: Date.now() } });
  const [noLonger] = mailTo(mac);
  assert.deepEqual([noLonger.workQueue.acted.status, noLonger.workQueue.acted.value], [404, { message: "It's no longer on devbox: it may have moved. Look again in a moment." }]);
  assert.deepEqual(mailTo(lab), [], "nothing sent on to lab");

  // From a box that isn't one of your computers: nothing at all.
  const stranger = box("machine", "stranger");
  await link.takeWorkQueueMail(stranger.peer, { act: { id: actID(), method: "POST", action: "restore", item: id, at: Date.now() } }, host, places);
  assert.deepEqual(sent, []);
  assert.equal(await deleted(), true);
});

// GitHub's answer for one issue, as a repository's (`p0`), or as the node itself when it's asked by
// its id.
const ghIssue = (number, { state = "OPEN", byID = false } = {}) => {
  const node = { __typename: "Issue", id: `I_kw${number}`, number, title: `Issue ${number}`, url: `https://github.com/kuvo-test/app/issues/${number}`, state, stateReason: state === "CLOSED" ? "COMPLETED" : null, closedAt: state === "CLOSED" ? "2026-11-10T09:00:00Z" : null, body: "" };
  write(ghAnswer, { data: { p0: byID ? node : { issueOrPullRequest: node } } });
};
const issueSource = number => ({ kind: "github", ref: `gh:kuvo-test/app#${number}`, url: `https://github.com/kuvo-test/app/issues/${number}`, host: "github.com", id: `I_kw${number}` });

test("one item for each issue, wherever it's handed: one handed here that another item here is already comes back with why; one isn't handed to a computer that told it has that issue", async () => {
  ghIssue(12);
  assert.equal((await tool("queue_add", { issue: "gh:kuvo-test/app#12" }, "ses_lead")).isError, false);
  const mine = (await view("ses_lead")).items.find(item => item.source?.ref === "gh:kuvo-test/app#12");
  // Lab hands its own item for the same issue (added there while this box's word hadn't reached it).
  const item = { ...told(1, "ses_farlead", { id: "wqi_handedsameissue00001", title: "Issue 12", source: { ...issueSource(12), ref: "gh:Kuvo-Test/App#12" }, body: "", notes: [], files: [], links: [], handed: [] }), number: undefined };
  await takeFrom(lab, { hand: { id: "wqh_sameissue00000001", to: "ses_child", from: "ses_farlead", by: { user: true }, at: Date.now(), item, files: [] } });
  const [answer] = mailTo(lab).map(each => each.workQueue.handed);
  assert.equal(answer.ok, false);
  assert.equal(answer.why, 'gh:kuvo-test/app#12 is in the Work Queue there already, in the queue of "Lead": one item for each issue', "by its title, as you know it");
  assert.deepEqual((await view("all")).items.filter(each => each.source?.ref.toLowerCase() === "gh:kuvo-test/app#12").map(each => each.id), [mine.id], "still one");

  // Lab told it has gh:kuvo-test/app#13 in ses_far's queue: this box's item for it isn't handed there.
  ghIssue(13);
  receive(lab, { workQueue: telling([]) });
  assert.equal((await tool("queue_add", { issue: "gh:kuvo-test/app#13" }, "ses_lead")).isError, false);
  receive(lab, { workQueue: telling([{ ...told(2, "ses_wren", { id: "wqi_labssameissue0000013", title: "Issue 13", source: issueSource(13) }), number: undefined }]) });
  const handed = await tool("queue_hand", { item: "gh:kuvo-test/app#13", to: "ses_far" }, "ses_lead");
  assert.deepEqual(handed, { isError: true, text: "gh:kuvo-test/app#13 is in the Work Queue on lab already, in the queue of ses_wren: one item for each issue. Ask that conversation (send_message), or the user." });
});

test("one item for each issue, wherever it's restored: a deleted one stays deleted while another item is that issue, here or on another computer", async () => {
  ghIssue(12);
  await tool("queue_add", { issue: "gh:kuvo-test/app#12" }, "ses_lead");
  const deleted = (await view("ses_lead")).items[0];
  assert.equal((await tool("queue_remove", { item: "gh:kuvo-test/app#12" }, "ses_lead")).isError, false);
  // Meanwhile lab added it (it never heard of a deleted one).
  receive(lab, { workQueue: telling([{ ...told(3, "ses_far", { id: "wqi_labaddeditlater00012", title: "Issue 12", source: issueSource(12) }), number: undefined }]) });
  const undone = await tool("queue_remove", { item: deleted.id, undo: true }, "ses_lead");
  assert.deepEqual(undone, { isError: true, text: "Another item is gh:kuvo-test/app#12 now, on lab: one item for each issue, so this one stays deleted." });
  const restored = await route("POST", `kuvo/work-queue/items/${deleted.id}/restore`);
  assert.deepEqual([restored.status, restored.body.message], [400, "Another item is gh:kuvo-test/app#12 now, on lab: one item for each issue, so this one stays deleted."]);
  // Here too: one handed here meanwhile (the deleted one doesn't hold it off).
  receive(lab, { workQueue: telling([]) });
  const item = { ...told(4, "ses_farlead", { id: "wqi_handedafterdelete012", title: "Issue 12", source: issueSource(12), body: "", notes: [], files: [], links: [], handed: [] }), number: undefined };
  await takeFrom(lab, { hand: { id: "wqh_afterdelete000001", to: "ses_child", from: "ses_farlead", by: { user: true }, at: Date.now(), item, files: [] } });
  assert.equal(mailTo(lab).map(each => each.workQueue.handed).find(Boolean).ok, true);
  const again = await route("POST", `kuvo/work-queue/items/${deleted.id}/restore`);
  assert.deepEqual([again.status, again.body.message], [400, 'Another item is gh:kuvo-test/app#12 now, in the queue of "Sign-in": one item for each issue, so this one stays deleted.']);
  // The same when the Mac's box hands the restore on for its client (#341).
  sent.length = 0;
  await takeFrom(mac, { act: { id: `wqa_${crypto.randomBytes(16).toString("base64url")}`, method: "POST", action: "restore", item: deleted.id, at: Date.now() } });
  assert.deepEqual(mailTo(mac)[0].workQueue.acted.value, { message: 'Another item is gh:kuvo-test/app#12 now, in the queue of "Sign-in": one item for each issue, so this one stays deleted.' });
  assert.equal((await view("all")).items.filter(each => each.source?.ref === "gh:kuvo-test/app#12").length, 1);
});

test("what this box tells of an issue goes again only once it changes, never because GitHub was asked again and said the same", async () => {
  ghIssue(12);
  await tool("queue_add", { issue: "gh:kuvo-test/app#12" }, "ses_lead");
  const now = Date.now();
  await link.pushQueues(host, link.readLink(), targets, now);
  const first = mailTo(lab)[0].workQueue.told;
  receive(lab, { workQueue: { had: first.id } });
  receive(mac, { workQueue: { had: first.id } });
  sent.length = 0;
  // Asked again each few minutes while a panel shows it, and the same each time: nothing goes.
  const seenFile = path.join(queues, "seen.json");
  for (const step of [1, 2, 3]) {
    const kept = read(seenFile);
    for (const each of Object.values(kept.reads)) each.checked = Date.now() - 600_000;
    write(seenFile, kept);
    const asks = ghAsks();
    await view("ses_lead");
    assert.equal(ghAsks(), asks + 1, "read again");
    // (As though each read were minutes after the last: this test runs in less than one.)
    const after = read(seenFile);
    for (const each of Object.values(after.reads)) each.checked = Date.now() + step * 180_000;
    write(seenFile, after);
    await link.pushQueues(host, link.readLink(), targets, now + step * 180_000);
    assert.equal(mailTo(lab).length, 0, `nothing told after read ${step}`);
  }
  // Closed there: that's a change, told once.
  const kept = read(seenFile);
  for (const each of Object.values(kept.reads)) each.checked = Date.now() - 600_000;
  write(seenFile, kept);
  ghIssue(12, { state: "CLOSED" });
  await view("ses_lead");
  await link.pushQueues(host, link.readLink(), targets, now + 800_000);
  const closed = mailTo(lab);
  assert.equal(closed.length, 1);
  assert.deepEqual([closed[0].workQueue.told.items[0].seen.state, closed[0].workQueue.told.items[0].status], ["completed", "done"]);
});

test("closed and gone issues are among the latest Done told to your other computers, by when GitHub closed them or they went, not left out behind a hundred older ones", async () => {
  ghIssue(12, { state: "CLOSED" });
  await tool("queue_add", { issue: "gh:kuvo-test/app#12" }, "ses_lead");
  ghIssue(13);
  await tool("queue_add", { issue: "gh:kuvo-test/app#13" }, "ses_lead");
  // #13 deleted on GitHub since: missing where it was, and by its id.
  write(ghAnswer, { data: { p0: { issueOrPullRequest: null } }, errors: [{ type: "NOT_FOUND", path: ["p0", "issueOrPullRequest"], message: "Could not resolve." }] });
  write(ghAnswerID, { data: { p0: null }, errors: [{ type: "NOT_FOUND", path: ["p0"], message: "Could not resolve." }] });
  const seenFile = path.join(queues, "seen.json");
  const kept = read(seenFile);
  const gone = kept.reads[Object.keys(kept.reads).find(key => key.endsWith("#13"))];
  gone.at = gone.checked = Date.now() - 3_600_000;
  write(seenFile, kept);
  const shown = (await view("ses_lead")).items.find(item => item.source?.ref === "gh:kuvo-test/app#13");
  assert.equal(shown.seen.gone, true);
  // A hundred Done before either, in the same queue.
  const file = path.join(queues, "queues", "ses_lead.json");
  const whole = read(file);
  const { source: _, ...plain } = whole.items.find(item => item.source?.ref === "gh:kuvo-test/app#13");
  const before = Date.parse("2026-11-01T00:00:00Z");
  whole.items.push(...Array.from({ length: 100 }, (__, index) => ({ ...plain, id: `wqi_done${String(index).padStart(16, "0")}`, title: `Done ${index}`, status: "done", doneAt: before, number: 5000 + index, order: 5000 + index })));
  write(file, whole);
  await link.pushQueues(host, link.readLink(), targets, Date.now());
  const told = mailTo(lab).flatMap(mail => mail.workQueue?.told?.items ?? []);
  assert.deepEqual(told.map(item => item.source?.ref).filter(Boolean).sort(), ["gh:kuvo-test/app#12", "gh:kuvo-test/app#13"]);
  assert.equal(told.length, 100, "the latest hundred Done");
});

test("an issue another computer told of, never read here and moved since, is found by the id its item knows: at once where that computer found it gone or elsewhere, else once it's missing where it was", async () => {
  write(path.join(run, "work", "elsewhere", `${lab.pub}.json`), { at: Date.now(), heard: Date.now(), leads: {} });
  const old = Date.now() - 7_200_000;
  const moved = { __typename: "Issue", id: "I_kw20", number: 3, title: "Moved here", url: "https://github.com/kuvo-test/moved/issues/3", state: "OPEN", stateReason: null, closedAt: null };
  const told20 = seen => ({ ...told(520, "ses_far", { id: "wqi_toldmovedissue000020", title: "Issue 20", seen, source: issueSource(20) }), number: undefined });
  const shown = async ref => (await view("all")).items.find(item => item.source?.ref === ref);

  // Lab found it gone: read here by its id at once.
  receive(lab, { workQueue: telling([told20({ state: "open", at: old, checked: old, gone: true })]) });
  write(ghAnswerID, { data: { p0: moved } });
  let item = await shown("gh:kuvo-test/app#20");
  assert.equal(ghQueries().length, 1);
  assert.match(ghQueries()[0], /p0: node\(id: "I_kw20"\)/);
  assert.deepEqual([item.title, item.source.url, item.seen.gone, item.status], ["Moved here", "https://github.com/kuvo-test/moved/issues/3", undefined, "todo"]);

  // Lab told it as it was: missing where it was here, then found by the id its item knows.
  fs.rmSync(path.join(queues, "seen.json"), { force: true });
  fs.rmSync(ghAsked, { force: true });
  receive(lab, { workQueue: telling([told20({ state: "open", at: old, checked: old })]) });
  write(ghAnswer, { data: { p0: { issueOrPullRequest: null } } });
  item = await shown("gh:kuvo-test/app#20");
  assert.equal(ghQueries().length, 2);
  assert.match(ghQueries()[0], /repository\(owner: "kuvo-test", name: "app"\) \{ issueOrPullRequest\(number: 20\)/);
  assert.match(ghQueries()[1], /p0: node\(id: "I_kw20"\)/);
  assert.deepEqual([item.source.url, item.seen.gone, item.status], ["https://github.com/kuvo-test/moved/issues/3", undefined, "todo"]);
});

test("a deleted item never stands for its issue while another item is it: adding it, showing it and its chip name the live one on another computer, and an agent's ref the live one in another queue here", async () => {
  ghIssue(12);
  await tool("queue_add", { issue: "gh:kuvo-test/app#12" }, "ses_lead");
  assert.equal((await tool("queue_remove", { item: "gh:kuvo-test/app#12" }, "ses_lead")).isError, false);
  receive(lab, { workQueue: telling([{ ...told(3, "ses_far", { id: "wqi_labaddeditlater00012", title: "Issue 12", source: issueSource(12) }), number: undefined }]) });
  // Adding it: it's on lab, not "undo the delete" (which the one-item rule refuses).
  assert.deepEqual(await tool("queue_add", { issue: "gh:kuvo-test/app#12" }, "ses_lead"), { isError: true, text: 'gh:kuvo-test/app#12 "Issue 12" is in the Work Queue already, on lab: one item for each issue.' });
  // Its ref names lab's: shown, and drawn as a chip.
  const shownText = (await tool("queue_show", { item: "gh:kuvo-test/app#12" }, "ses_lead")).text;
  assert.match(shownText, /^gh:kuvo-test\/app#12 "Issue 12".* · on lab/);
  const ref = (await route("POST", "kuvo/work-queue/refs", { refs: ["gh:kuvo-test/app#12"] })).body.refs["gh:kuvo-test/app#12"];
  assert.deepEqual([ref.id, ref.removed, ref.computer?.name], ["wqi_labaddeditlater00012", undefined, "lab"]);

  // The live one in another conversation's queue here, one ses_lead doesn't oversee: an agent's ref
  // changes the live item in the other queue, leaving the deleted match alone.
  receive(lab, { workQueue: telling([]) });
  const whole = read(path.join(queues, "queues", "ses_lead.json"));
  const { removed: _, ...live } = whole.items[0];
  write(path.join(queues, "queues", "ses_someone.json"), { ...whole, owner: "ses_someone", items: [{ ...live, id: "wqi_someoneslive00000012", owner: "ses_someone", order: 9 }] });
  const updated = await tool("queue_update", { item: "gh:kuvo-test/app#12", note: "More" }, "ses_lead");
  assert.equal(updated.isError, false, updated.text);
  assert.equal(read(path.join(queues, "queues", "ses_someone.json")).items[0].notes.at(-1).text, "More");
  assert.deepEqual(read(path.join(queues, "queues", "ses_lead.json")), whole);
});

test("a file replaced by a symlink is skipped in a hand-off and refused in a peer fetch", async () => {
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[100, 149]] });
  const added = await route("POST", "kuvo/work-queue/items", {
    operation: "attachment-kept-here", owner: "ses_lead", title: "Attachment kept here",
    files: [{ name: "photo.png", uri: "data:image/png;base64,bG9jYWw=" }],
  });
  assert.equal(added.status, 200);
  const id = added.body.item.id;
  const file = path.join(queues, "files", id, "photo.png");
  const outside = path.join(run, "outside-item.png");
  write(outside, "other bytes");
  fs.unlinkSync(file);
  fs.symlinkSync(outside, file);
  await takeFrom(lab, { act: { id: actID(), method: "GET", action: "file", name: "photo.png", item: id, at: Date.now() } });
  assert.equal(mailTo(lab)[0].workQueue.acted.status, 404);
  assert.equal((await tool("queue_hand", { item: "#100", to: "ses_far" }, "ses_lead")).isError, false);
  await link.pushQueues(host, link.readLink(), targets, Date.now());
  const hand = mailTo(lab).map(each => each.workQueue.hand).find(Boolean);
  assert.ok(hand);
  assert.deepEqual(hand.files, []);
});

test("a split mention follows its original item through the move despite the destination reusing its number", async () => {
  write(path.join(queues, "numbers.json"), { primary: mac.pub, leases: [[42, 61]] });
  const target = (await route("POST", "kuvo/work-queue/items", { operation: "original-42", owner: "ses_lead", title: "Original #42" })).body.item;
  const source = (await route("POST", "kuvo/work-queue/items", { operation: "follow-up-42", owner: "ses_child", title: "Follow-up", body: "See #42" })).body.item;
  const relationIds = async id => (await route("GET", `kuvo/work-queue/items/${id}`)).body.item.itemRelations.map(({ kind, id }) => [kind, id]);
  assert.deepEqual(await relationIds(target.id), [["mentioned-in", source.id]]);

  const manual = await route("PATCH", `kuvo/work-queue/items/${source.id}`, { itemRelation: { item: target.id, kind: "blocks" } });
  assert.equal(manual.status, 200);
  const packed = link.packWorkQueues(["ses_child"]);
  assert.equal(packed.items[0].relations, undefined, "manual links do not travel across a split");
  link.forgetWorkQueues(["ses_child"]);
  assert.equal(read(path.join(queues, "queues", "ses_lead.json")).items[0].relations, undefined, "manual links are pruned on the origin too");
  const summary = told(source.number, "ses_far", { id: source.id, title: "Follow-up", mentionTargets: packed.items[0].mentionIndex?.targets });
  await takeFrom(lab, telling([summary, told(42, "ses_far", { title: "Different #42" })]));
  assert.deepEqual(await relationIds(target.id), [["mentioned-in", source.id]], "the source computer still sees the backlink");

  // Replay the bundle and peer summaries as seen on the receiving computer.
  link.forgetWorkQueues(["ses_lead"]);
  await takeFrom(lab, telling([told(target.number, "ses_far", { id: target.id, title: target.title })]));
  write(path.join(queues, "queues", "ses_lead.json"), {
    owner: "ses_lead", items: [{ ...told(42, "ses_lead"), title: "Different #42" }],
  });
  link.placeWorkQueues(packed, ["ses_child"], null);
  assert.deepEqual(await relationIds(source.id), [["mentions", target.id]]);
  const rewritten = await route("PATCH", `kuvo/work-queue/items/${source.id}`, { body: "Later edit. See #42." });
  assert.equal(rewritten.status, 200);
  const moved = (await route("GET", `kuvo/work-queue/items/${source.id}`)).body.item;
  assert.equal(moved.bodyMentions[0].item, target.id);
  assert.deepEqual(await relationIds(told(42).id), []);
  await link.pushQueues(host, link.readLink(), targets, Date.now());
  assert.deepEqual(mailTo(lab).flatMap(mail => mail.workQueue.told?.items ?? []).find(item => item.id === source.id)?.mentionTargets, [target.id]);
});
