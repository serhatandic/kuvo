// The Work Queue's statuses, priorities and labels on a box (PROTOCOL.md, "The Work Queue", #674):
// items kept under categories by an older box moving over once, with nothing lost, and carried on
// after a run cut off midway; what a client adds and changes (`POST kuvo/work-queue/items`,
// `PATCH …/items/<item>`), what it may do to each (`can`), the search (`q`) and its counts, the
// labels, and the tools' fields. The real box script on a folder of the test's own; only the engine
// is a stand-in.
//
// Run with `node --test box/bridge-tests/work-queue-statuses.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import { randomUUID } from "node:crypto";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { beforeEach, test } from "node:test";
import { atEnd, removeAtEnd } from "./at-end.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-work-queue-statuses-")));
removeAtEnd(home);
fs.mkdirSync(path.join(home, "bin"), { recursive: true });
fs.writeFileSync(path.join(home, "bin", "gh"), "#!/bin/sh\necho 'gh auth login' >&2\nexit 1\n", { mode: 0o755 });
process.env.PATH = `${path.join(home, "bin")}${path.delimiter}${process.env.PATH}`;
process.env.KUVO_HOME = home;
const link = await import(LINK_BUNDLE.href);
const write = (file, text) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
};
const project = path.join(home, "work", "api");
fs.mkdirSync(project, { recursive: true });
const queues = path.join(home, "run", "work-queue");
const read = file => JSON.parse(fs.readFileSync(path.join(queues, file), "utf8"));

// The engine: a lead, a session it started, a bot's conversation; which are at work, and the tool
// call each is making now.
const sessions = new Map([
  ["ses_lead", { id: "ses_lead", title: "Lead", location: { directory: project }, time: { created: 1, updated: 5 } }],
  ["ses_child", { id: "ses_child", title: "Sign-in", location: { directory: project }, metadata: { kuvo: { startedBy: "ses_lead", depth: 1 } }, time: { created: 2, updated: 6 } }],
  ["ses_scout", { id: "ses_scout", title: "Scout", location: { directory: path.join(home, "bots", "bot_scout00001", "workspace") }, time: { created: 1, updated: 3 } }],
]);
let active = new Set();
// (The engine as it starts: it answers, but can't say anything yet.)
let engineDown = false;
const calling = new Map();
const engine = http.createServer((request, response) => {
  request.resume();
  request.on("end", () => {
    const reply = (value, status = 200) => { response.writeHead(status, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
    const url = new URL(request.url, "http://x");
    if (url.pathname === "/api/session" && request.method === "GET") return reply({ data: [...sessions.values()] });
    if (engineDown) return reply({ message: "Starting." }, 503);
    if (url.pathname === "/api/session/active") return reply({ data: Object.fromEntries([...active].map(id => [id, {}])) });
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
write(path.join(home, "run", "engine.json"), JSON.stringify({ port: engine.address().port, password: "p", version: "0.0.0" }));
const bot = path.join(home, "bots", "bot_scout00001");
write(path.join(bot, "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_scout00001", name: "Scout", job: "", look: { color: "sky", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z", updated: "2026-10-01T09:00:00Z" }));
write(path.join(bot, "session"), "ses_scout");
fs.mkdirSync(path.join(bot, "workspace"), { recursive: true });

beforeEach(() => {
  fs.rmSync(queues, { recursive: true, force: true });
  // (As a box just started: it looks at how its items are kept again.)
  link.forgetQueueMigrated();
  active = new Set();
  calling.clear();
});

const handle = link.engineHandler();
async function route(method, pathname, body, from = { fromApp: true }) {
  const pieces = [];
  for await (const piece of handle({ method, path: pathname, headers: {}, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)), ...from }, undefined)) pieces.push(piece);
  const head = pieces.find(each => each.head).head;
  let parsed = Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString();
  try { parsed = JSON.parse(parsed); } catch {}
  return { status: head.status, body: parsed };
}
async function tool(name, args, session) {
  calling.set(session, { name, args });
  try {
    return (await route("POST", "kuvo/work-queue/tools", { name, arguments: args, session })).body;
  } finally {
    calling.delete(session);
  }
}
const view = async (owner = "all", more = "") => (await route("GET", `kuvo/work-queue?owner=${owner}${more}`)).body;
const ok = async (method, pathname, body) => {
  const answer = await route(method, pathname, body);
  assert.equal(answer.status, 200, JSON.stringify(answer.body));
  return answer.body;
};

// ---- From categories ----

const YOU = { user: true };
const LEAD = { session: "ses_lead", name: "Lead" };
// (Deleted a moment ago: kept for bringing back.)
const DELETED = Date.now() - 1000;
const id = n => `wqi_${String(n).padStart(20, "0")}`;
// An item as a box from before statuses kept it.
const old = (n, category, more = {}) => ({
  id: id(n), number: n, owner: "ses_lead", title: `Item ${n}`, body: `What item ${n} is.`, category, order: n,
  notes: [{ at: 10 + n, by: LEAD, text: `A note on ${n}.` }], files: [], links: [], sessions: [],
  created: { at: n, by: LEAD }, updated: { at: 100 + n, by: YOU }, ...more,
});
// A box as 0.7 left it: every built-in category, the custom ones (one of them nothing is in), an
// item being worked on, a deleted one, a second owner's queue, and a field of a newer box's.
function keptBefore() {
  const lead = [
    old(1, "next"),
    old(2, "later"),
    old(3, "done", { doneAt: 500 }),
    old(4, "parked"),
    old(5, "cat_architecture", { progress: { value: 0.5, note: "Halfway" }, links: [{ url: "https://example.com/pr/5", title: "The PR" }], files: [{ name: "plan.md", size: 3, type: "text/markdown", at: 9 }] }),
    old(6, "cat_waiting-on-you"),
    old(7, "cat_correctness-sweep"),
    old(8, "cat_edge-cases"),
    old(9, "cat_security-privacy", { handedBy: "ses_child", handed: [{ from: "ses_child", to: "ses_lead", at: 50 }] }),
    // Marked as being worked on: linked, its session's last word that it started.
    old(10, "next", { order: 0.5, sessions: [{ session: "ses_lead", since: 60 }], events: [{ at: 55, by: YOU, kind: "moved", from: "later", to: "next", top: true }, { at: 60, by: LEAD, kind: "started" }] }),
    // Linked, but its session stopped: where its category puts it.
    old(11, "later", { sessions: [], events: [{ at: 60, by: LEAD, kind: "started" }, { at: 70, by: LEAD, kind: "stopped" }, { at: 80, by: YOU, kind: "moved", from: "cat_architecture", to: "later" }] }),
    old(12, "next", { removed: { at: DELETED, by: YOU }, somethingNewer: { kept: true } }),
  ];
  const child = [old(13, "next", { owner: "ses_child", order: 0.25 }), old(14, "cat_features", { owner: "ses_child" })];
  write(path.join(queues, "queues", "ses_lead.json"), JSON.stringify({ owner: "ses_lead", items: lead }));
  write(path.join(queues, "queues", "ses_child.json"), JSON.stringify({ owner: "ses_child", items: child }));
  write(path.join(queues, "categories.json"), JSON.stringify({ custom: [
    { id: "cat_architecture", name: "Architecture" }, { id: "cat_security-privacy", name: "Security & privacy" }, { id: "cat_features", name: "Features" },
    { id: "cat_correctness-sweep", name: "Correctness sweep" }, { id: "cat_edge-cases", name: "Edge cases" }, { id: "cat_waiting-on-you", name: "Waiting on you" }, { id: "cat_unused", name: "Never used" },
  ] }));
  write(path.join(queues, "numbers.json"), JSON.stringify({ next: 15 }));
  return { lead, child, files: { "queues/ses_lead.json": fs.readFileSync(path.join(queues, "queues", "ses_lead.json"), "utf8"), "queues/ses_child.json": fs.readFileSync(path.join(queues, "queues", "ses_child.json"), "utf8"), "categories.json": fs.readFileSync(path.join(queues, "categories.json"), "utf8") } };
}
// Everything an item holds but where it stands.
const held = ({ category: _, status: __, labels: ___, order: ____, events: _____, ...rest }) => rest;

test("items kept under categories move over to statuses and labels once, with nothing lost, from a copy of what was kept", async () => {
  const before = keptBefore();
  const seen = await view("all");
  const byNumber = new Map([...seen.items, ...seen.removed].map(item => [item.number, item]));
  const stands = n => [byNumber.get(n).status, byNumber.get(n).labels];
  // Next is Todo, Later Backlog, Done Done; Parked is Backlog with the label Parked.
  assert.deepEqual(stands(1), ["todo", []]);
  assert.deepEqual(stands(2), ["backlog", []]);
  assert.deepEqual(stands(3), ["done", []]);
  assert.deepEqual(stands(4), ["backlog", ["lbl_parked"]]);
  // A custom category is a label on a Todo item; "Waiting on you" is Needs you; Correctness sweep
  // and Edge cases are Backlog.
  assert.deepEqual(stands(5), ["todo", ["cat_architecture"]]);
  assert.deepEqual(stands(6), ["needs-you", []]);
  assert.deepEqual(stands(7), ["backlog", ["cat_correctness-sweep"]]);
  assert.deepEqual(stands(8), ["backlog", ["cat_edge-cases"]]);
  assert.deepEqual(stands(9), ["todo", ["cat_security-privacy"]]);
  assert.deepEqual(stands(14), ["todo", ["cat_features"]]);
  // One a session had said it was on stands where its category put it, until the box can say
  // whether that session is at work (below); one whose session stopped, for good.
  assert.deepEqual(stands(10), ["todo", []]);
  assert.deepEqual(stands(11), ["backlog", []]);
  assert.deepEqual(stands(12), ["todo", []], "a deleted one too, where it's kept for bringing back");
  assert.equal(seen.removed.length, 1);
  // The labels: every category you had, by its name (one nothing was in too), but the one that's a
  // status now; and Parked. None is built in.
  assert.deepEqual(seen.labels.map(each => [each.id, each.name]), [
    ["cat_architecture", "Architecture"], ["cat_security-privacy", "Security & privacy"], ["cat_features", "Features"],
    ["cat_correctness-sweep", "Correctness sweep"], ["cat_edge-cases", "Edge cases"], ["cat_unused", "Never used"], ["lbl_parked", "Parked"],
  ]);
  for (const each of seen.labels) assert.match(each.color, /^(coral|orange|amber|lime|mint|sky|indigo|violet|pink|graphite)$/);
  assert.equal(seen.labels.at(-1).color, "graphite");

  // Nothing an item holds is lost: number, title, description, notes, files, links, sessions,
  // progress, hand-offs, who made and changed it, a deletion, and what a newer box wrote.
  const kept = [...read("queues/ses_lead.json").items, ...read("queues/ses_child.json").items];
  assert.equal(kept.length, 14);
  for (const was of [...before.lead, ...before.child]) {
    const now = kept.find(each => each.id === was.id);
    assert.deepEqual(held(now), held(was), `#${was.number}`);
    assert.equal(now.category, undefined, `#${was.number}`);
  }
  // What happened to it stays, its moves naming statuses.
  assert.deepEqual(kept.find(each => each.number === 10).events, [{ at: 55, by: YOU, kind: "moved", from: "backlog", to: "todo", top: true }, { at: 60, by: LEAD, kind: "started" }]);
  // (And the label a category became, beside it: it still says it came from Architecture.)
  assert.deepEqual(kept.find(each => each.number === 11).events.at(-1), { at: 80, by: YOU, kind: "moved", from: "todo", to: "backlog", fromLabel: "cat_architecture" });
  // Items keep their order: within a status, by the category they came from (Next, Later, the
  // custom ones as listed, Parked), then their place there.
  const order = status => seen.items.filter(item => item.status === status).map(item => item.number);
  assert.deepEqual(order("todo"), [13, 10, 1, 5, 9, 14]);
  assert.deepEqual(order("backlog"), [2, 11, 7, 8, 4]);
  // The statuses, in the order a client shows them, the closed ones last.
  assert.deepEqual(seen.statuses.map(each => [each.id, each.closed]), [["needs-you", false], ["in-review", false], ["in-progress", false], ["waiting", false], ["todo", false], ["backlog", false], ["done", true], ["canceled", true]]);
  assert.deepEqual(seen.items.map(item => item.status).filter((each, index, all) => each !== all[index - 1]), ["needs-you", "todo", "backlog", "done"]);
  // The colours a label may be, as a picker offers them: the box's list, none a client keeps (#729).
  assert.deepEqual(seen.colors, ["coral", "orange", "amber", "lime", "mint", "sky", "indigo", "violet", "pink", "graphite"]);

  // What was kept before is beside it, whole, byte for byte.
  for (const [name, text] of Object.entries(before.files)) assert.equal(fs.readFileSync(path.join(queues, "before-statuses", name), "utf8"), text, name);
  assert.equal(read("model.json").model, 2);
  assert.equal(fs.existsSync(path.join(queues, "categories.json")), false);
  // Numbers go on from where they were.
  assert.equal(read("numbers.json").next, 15);
});

test("an item a session had said it was on is In progress only if that session is at work once the box can say, and that's looked at once", async () => {
  keptBefore();
  // A second one, whose session isn't at work any more.
  const kept = read("queues/ses_child.json");
  kept.items.push(old(15, "later", { owner: "ses_child", sessions: [{ session: "ses_child", since: 60 }], events: [{ at: 60, by: { session: "ses_child", name: "Sign-in" }, kind: "started" }] }));
  write(path.join(queues, "queues", "ses_child.json"), JSON.stringify(kept));
  const stands = async number => (await view("all")).items.find(item => item.number === number).status;
  assert.deepEqual([await stands(10), await stands(15)], ["todo", "backlog"]);
  // The engine can't say yet (it's starting: it answers nothing useful): nothing is decided, and
  // it's asked again at the next round. As the box asks it, of the engine itself.
  link.forgetQueueSettled();
  engineDown = true;
  await link.settleQueueStarted(() => link.queueAtWorkForSure());
  assert.deepEqual(read("model.json").started.sort(), [id(10), id(15)]);
  assert.deepEqual([await stands(10), await stands(15)], ["todo", "backlog"]);
  // It can: the lead is at work, the other session isn't.
  engineDown = false;
  active = new Set(["ses_lead"]);
  await link.settleQueueStarted(() => link.queueAtWorkForSure());
  assert.deepEqual([await stands(10), await stands(15)], ["in-progress", "backlog"]);
  assert.equal(read("model.json").started, undefined);
  const moved = (await ok("GET", "kuvo/work-queue/items/10")).item.events.at(-1);
  assert.deepEqual([moved.kind, moved.from, moved.to, moved.by.name], ["moved", "todo", "in-progress", "Kuvo Code"]);
  // Never again: a status is only ever set from then on.
  await ok("PATCH", "kuvo/work-queue/items/10", { status: "todo" });
  link.forgetQueueSettled();
  await link.settleQueueStarted(async () => new Set(["ses_lead", "ses_child"]));
  assert.deepEqual([await stands(10), await stands(15)], ["todo", "backlog"]);
});

test("a move over cut off midway is carried on to the same end, from the same copy", async () => {
  // What an uninterrupted run leaves.
  keptBefore();
  await view("all");
  const whole = { lead: read("queues/ses_lead.json"), child: read("queues/ses_child.json"), labels: read("labels.json") };
  const copy = path.join(home, "copy-of-before");
  fs.cpSync(path.join(queues, "before-statuses"), copy, { recursive: true });
  const sameEnd = async said => {
    await view("all");
    assert.deepEqual(read("queues/ses_lead.json"), whole.lead, said);
    assert.deepEqual(read("queues/ses_child.json"), whole.child, said);
    assert.deepEqual(read("labels.json"), whole.labels, said);
    assert.equal(read("model.json").model, 2, said);
    assert.deepEqual(fs.readdirSync(queues).filter(name => name.startsWith("before-statuses")), ["before-statuses"], said);
    assert.deepEqual(read("before-statuses/queues/ses_lead.json"), JSON.parse(fs.readFileSync(path.join(copy, "queues", "ses_lead.json"), "utf8")), said);
  };
  const again = () => {
    fs.rmSync(queues, { recursive: true, force: true });
    link.forgetQueueMigrated();
    return keptBefore();
  };

  // Cut off while copying: a part of a copy lies beside, never taken for the whole of it.
  again();
  write(path.join(queues, "before-statuses.0123456789ab", "queues", "ses_lead.json"), '{"owner":"ses_lead","items":[');
  await sameEnd("cut off while copying");

  // Cut off once the copy was made, before any queue was written.
  again();
  fs.cpSync(copy, path.join(queues, "before-statuses"), { recursive: true });
  await sameEnd("cut off after the copy");

  // Cut off between two queues: one written, the labels too, the other still under categories.
  again();
  fs.cpSync(copy, path.join(queues, "before-statuses"), { recursive: true });
  write(path.join(queues, "queues", "ses_child.json"), JSON.stringify(whole.child));
  write(path.join(queues, "labels.json"), JSON.stringify(whole.labels));
  await sameEnd("cut off between two queues");

  // Cut off just before it was said to be done.
  again();
  fs.cpSync(copy, path.join(queues, "before-statuses"), { recursive: true });
  write(path.join(queues, "queues", "ses_child.json"), JSON.stringify(whole.child));
  write(path.join(queues, "queues", "ses_lead.json"), JSON.stringify(whole.lead));
  write(path.join(queues, "labels.json"), JSON.stringify(whole.labels));
  await sameEnd("cut off before model.json");
});

test("it's one way and once: what you change after isn't moved again, and the copy stays as it was", async () => {
  const before = keptBefore();
  await view("all");
  await ok("PATCH", "kuvo/work-queue/items/1", { status: "in-review", priority: "high" });
  await ok("POST", "kuvo/work-queue/labels", { remove: "Parked" });
  // The box starts again.
  link.forgetQueueMigrated();
  const seen = await view("all");
  const one = seen.items.find(item => item.number === 1);
  assert.deepEqual([one.status, one.priority], ["in-review", "high"]);
  assert.equal(seen.labels.some(each => each.id === "lbl_parked"), false, "a label removed isn't made again");
  assert.deepEqual(seen.items.find(item => item.number === 4).labels, []);
  for (const [name, text] of Object.entries(before.files)) assert.equal(fs.readFileSync(path.join(queues, "before-statuses", name), "utf8"), text, name);
});

test("its mark of being done lost, nothing is moved over twice and a label you removed stays removed", async () => {
  const before = keptBefore();
  await view("all");
  await ok("POST", "kuvo/work-queue/labels", { remove: "Features" });
  await ok("PATCH", "kuvo/work-queue/items/1", { status: "waiting" });
  const kept = read("queues/ses_lead.json");
  fs.rmSync(path.join(queues, "model.json"));
  link.forgetQueueMigrated();
  const seen = await view("all");
  assert.equal(seen.labels.some(each => each.id === "cat_features"), false);
  assert.equal(seen.items.find(item => item.number === 1).status, "waiting");
  assert.deepEqual(read("queues/ses_lead.json"), kept);
  for (const [name, text] of Object.entries(before.files)) assert.equal(fs.readFileSync(path.join(queues, "before-statuses", name), "utf8"), text, name);
  assert.equal(read("model.json").model, 2);
});

test("one process moves the items over: another that comes meanwhile reads on, and changes nothing until it's done", async () => {
  keptBefore();
  // Another process is at it now.
  fs.mkdirSync(path.join(queues, ".moving-to-statuses"));
  const seen = await view("all");
  assert.equal(seen.items.length + seen.removed.length, 14, "what's there still shows, each item said anew as it's read");
  assert.equal(seen.items.find(item => item.number === 2).status, "backlog");
  const refused = await route("PATCH", "kuvo/work-queue/items/1", { status: "done" });
  assert.equal(refused.status, 400);
  assert.equal(refused.body.message, "The Work Queue's items are being moved over to statuses on this computer right now, so nothing was changed. Try again in a moment (two minutes at most, if what was moving them was cut off).");
  assert.equal((await tool("queue_add", { title: "Not yet" }, "ses_lead")).isError, true);
  assert.equal(fs.existsSync(path.join(queues, "before-statuses")), false, "it made no copy of its own beside the other's");
  assert.equal(read("queues/ses_lead.json").items[0].status, undefined);
  // It was cut off (its mark is old): this one carries on.
  const old = new Date(Date.now() - 10 * 60_000);
  fs.utimesSync(path.join(queues, ".moving-to-statuses"), old, old);
  assert.equal((await ok("PATCH", "kuvo/work-queue/items/1", { status: "done" })).item.status, "done");
  assert.equal(read("model.json").model, 2);
  assert.equal(fs.existsSync(path.join(queues, ".moving-to-statuses")), false);
});

test("a queue file that can't be read is left as it is, and copied; the others move over", async () => {
  keptBefore();
  write(path.join(queues, "queues", "ses_scout.json"), '{"owner":"ses_scout","items":[{"id":');
  const seen = await view("all");
  assert.equal(seen.items.length + seen.removed.length, 14);
  assert.equal(fs.readFileSync(path.join(queues, "queues", "ses_scout.json"), "utf8"), '{"owner":"ses_scout","items":[{"id":');
  assert.equal(fs.readFileSync(path.join(queues, "before-statuses", "queues", "ses_scout.json"), "utf8"), '{"owner":"ses_scout","items":[{"id":');
});

test("a box with no Work Queue yet has nothing to move: no copy is made, and its first item has a status", async () => {
  assert.deepEqual((await view("all")).items, []);
  const added = await tool("queue_add", { title: "The first" }, "ses_lead");
  assert.equal(added.text, 'Added #1 "The first" as Todo. Refer to it as #1.');
  assert.equal(fs.existsSync(path.join(queues, "before-statuses")), false);
  assert.equal(read("model.json").model, 2);
  assert.equal(read("queues/ses_lead.json").items[0].status, "todo");
});

// ---- What a client adds and changes ----

test("you add an item from a client: Todo and Normal unless said, numbered as an agent's, in your own queue or the bot's it's for", async () => {
  await tool("queue_add", { title: "An agent's" }, "ses_lead");
  await ok("POST", "kuvo/work-queue/labels", { add: "Bug", color: "coral" });
  const plain = (await ok("POST", "kuvo/work-queue/items", { operation: randomUUID(), title: "  Mine,\n plainly  " })).item;
  assert.deepEqual([plain.number, plain.title, plain.status, plain.priority, plain.labels, plain.owner, plain.assignee, plain.due], [2, "Mine, plainly", "todo", "normal", [], "usr_you", undefined, undefined]);
  assert.deepEqual(plain.created.by, { user: true });
  assert.deepEqual(plain.can, { edit: true, set: true, assign: true, close: true, delete: true, restore: false, attach: true });
  const full = (await ok("POST", "kuvo/work-queue/items", { operation: randomUUID(), title: "Fix the crash", body: "It **crashes**.", status: "Backlog", priority: "urgent", labels: ["Bug"], assignee: { kind: "agent", id: "codex-cli" }, due: "2026-12-01" })).item;
  assert.deepEqual([full.number, full.status, full.priority, full.labels, full.assignee, full.due, full.owner], [3, "backlog", "urgent", ["lbl_bug"], { kind: "agent", id: "codex-cli", name: "Codex" }, "2026-12-01", "usr_you"]);
  assert.equal((await ok("GET", `kuvo/work-queue/items/${full.id}`)).item.body, "It **crashes**.");
  // For a bot of this box: in that bot's queue, where its agent's tools see it.
  const forBot = (await ok("POST", "kuvo/work-queue/items", { operation: randomUUID(), title: "Scout's to do", assignee: { kind: "bot", id: "bot_scout00001" } })).item;
  assert.deepEqual([forBot.owner, forBot.assignee], ["ses_scout", { kind: "bot", id: "bot_scout00001", name: "Scout", look: { color: "sky", shape: "round", face: "calm" } }]);
  assert.match((await tool("queue_list", { owner: "ses_scout" }, "ses_scout")).text, /Todo:\n- #4 "Scout's to do" · for Scout/);
  // Into the queue a client is looking at.
  assert.equal((await ok("POST", "kuvo/work-queue/items", { operation: randomUUID(), title: "For the lead", owner: "ses_lead" })).item.owner, "ses_lead");

  // Your own queue is in the All view. Agents can read and change it.
  const all = await view("all");
  assert.deepEqual(all.items.map(item => item.number), [1, 2, 4, 5, 3]);
  assert.deepEqual(all.owners.find(each => each.owner === "usr_you"), { owner: "usr_you", kind: "you", title: "Your own" });
  // The day it is on the box, for what a client counts days from.
  assert.match(all.today, /^\d{4}-\d{2}-\d{2}$/);
  assert.deepEqual((await view("usr_you")).items.map(item => item.number), [2, 3]);
  assert.doesNotMatch((await tool("queue_list", { owner: "ses_lead" }, "ses_lead")).text, /Mine, plainly/);
  assert.match((await tool("queue_list", { owner: "usr_you" }, "ses_lead")).text, /Mine, plainly/);
  assert.equal((await tool("queue_update", { item: 2, note: "Agent edit" }, "ses_lead")).isError, false);
  // Who an item may be for: this box's bots, then the coding agents.
  assert.deepEqual(all.assignees.slice(0, 2).map(each => [each.kind, each.id, each.name]), [["bot", "bot_scout00001", "Scout"], ["agent", "claude-code", "Claude Code"]]);
  assert.deepEqual(all.assignees[0].look, { color: "sky", shape: "round", face: "calm" });
  // What an item added with nothing said starts as, for a client's form.
  assert.deepEqual(all.new, { status: "todo", priority: "normal" });

  // What isn't an item is refused, in words, and nothing is added.
  for (const [body, why] of [[{ title: "  " }, /Say what it is/], [{ title: "x", status: "soon" }, /no status soon/], [{ title: "x", priority: "max" }, /no priority max/], [{ title: "x", labels: ["Nope"] }, /no label Nope/],
    [{ title: "x", assignee: { kind: "bot", id: "bot_nobody" } }, /no bot or coding agent/], [{ title: "x", due: "next week" }, /due is a day/], [{ title: "x", due: "2026-02-30" }, /due is a day/], [{ title: "x", owner: "elsewhere" }, /no queue to add it to/]]) {
    const refused = await route("POST", "kuvo/work-queue/items", { ...body, operation: randomUUID() });
    assert.equal(refused.status, 400, JSON.stringify(body));
    assert.match(refused.body.message, why);
  }
  assert.equal((await view("all")).items.length, 5);
  // Only a client adds one this way: an agent has its tools.
  assert.equal((await route("POST", "kuvo/work-queue/items", { title: "Sneaky" }, {})).status, 403);
  // The box says which are late: due before today, and still open.
  const late = (await ok("POST", "kuvo/work-queue/items", { operation: randomUUID(), title: "Late", due: "2020-01-01" })).item;
  assert.equal(late.overdue, true);
  assert.equal((await ok("POST", "kuvo/work-queue/items", { operation: randomUUID(), title: "In time", due: "2999-01-01" })).item.overdue, undefined);
  assert.equal((await ok("PATCH", `kuvo/work-queue/items/${late.id}`, { status: "done" })).item.overdue, undefined, "a closed one isn't late");

});

test("you change what an item says and where it stands; only what's sent changes", async () => {
  await tool("queue_add", { title: "Write the tests", body: "All of them.", working: true }, "ses_lead");
  await tool("queue_add", { title: "Second" }, "ses_lead");
  await tool("queue_add", { title: "Third" }, "ses_lead");
  await ok("POST", "kuvo/work-queue/labels", { add: "Tests" });
  await ok("POST", "kuvo/work-queue/labels", { add: "Bug" });
  let item = (await ok("PATCH", "kuvo/work-queue/items/1", { title: "Write every test", priority: "High", labels: ["tests", "lbl_bug"], due: "2026-11-30", assignee: { kind: "agent", id: "claude-code" } })).item;
  assert.deepEqual([item.title, item.status, item.priority, item.labels, item.due, item.assignee.name], ["Write every test", "in-progress", "high", ["lbl_tests", "lbl_bug"], "2026-11-30", "Claude Code"]);
  assert.deepEqual(item.updated.by, { user: true });
  assert.equal((await ok("GET", "kuvo/work-queue/items/1")).item.body, "All of them.", "a description not sent stays");
  item = (await ok("PATCH", "kuvo/work-queue/items/1", { body: "Most of them.", labels: [], due: null, assignee: null, priority: "normal" })).item;
  assert.deepEqual([item.priority, item.labels, item.due, item.assignee], ["normal", [], undefined, undefined]);
  assert.equal((await ok("GET", "kuvo/work-queue/items/1")).item.body, "Most of them.");

  // Closing it says when, and unlinks everyone; opening it again takes that back.
  assert.equal(item.sessions.length, 1);
  item = (await ok("PATCH", "kuvo/work-queue/items/1", { status: "canceled" })).item;
  assert.deepEqual([item.status, item.sessions.length, typeof item.doneAt], ["canceled", 0, "number"]);
  item = (await ok("PATCH", "kuvo/work-queue/items/1", { status: "Needs you" })).item;
  assert.deepEqual([item.status, item.doneAt], ["needs-you", undefined]);
  item = (await ok("PATCH", "kuvo/work-queue/items/1", { status: "done" })).item;
  assert.equal(item.status, "done");
  const whole = (await ok("GET", "kuvo/work-queue/items/1")).item;
  assert.deepEqual(whole.events.map(each => [each.kind, each.from, each.to]), [["started", undefined, undefined], ["moved", "in-progress", "canceled"], ["moved", "canceled", "needs-you"], ["done", undefined, undefined]]);

  // Its place among those of its status.
  await ok("PATCH", "kuvo/work-queue/items/3", { before: 2 });
  assert.deepEqual((await view("ses_lead")).items.filter(each => each.status === "todo").map(each => each.number), [3, 2]);
  await ok("PATCH", "kuvo/work-queue/items/2", { top: true });
  assert.deepEqual((await view("ses_lead")).items.filter(each => each.status === "todo").map(each => each.number), [2, 3]);
  const beside = await route("PATCH", "kuvo/work-queue/items/3", { after: 1 });
  assert.equal(beside.status, 400);
  assert.match(beside.body.message, /is Done, not Todo/);

  // What can't be is refused, and changes nothing (the title sent with it neither).
  for (const [body, why] of [[{ title: "Changed", status: "soon" }, /no status soon/], [{ title: "Changed", labels: ["Nope"] }, /no label Nope/], [{ title: "Changed", due: "tomorrow" }, /due is a day/], [{ title: " " }, /can't be empty/]]) {
    const refused = await route("PATCH", "kuvo/work-queue/items/2", body);
    assert.equal(refused.status, 400, JSON.stringify(body));
    assert.match(refused.body.message, why);
  }
  assert.equal((await ok("GET", "kuvo/work-queue/items/2")).item.title, "Second");
  assert.equal((await route("PATCH", "kuvo/work-queue/items/2", { title: "An agent's command" }, {})).status, 403);
  assert.equal((await route("PATCH", "kuvo/work-queue/items/99", { title: "None" })).status, 404);
});

test("an item of your own that you give to a bot goes into that bot's queue", async () => {
  const mine = (await ok("POST", "kuvo/work-queue/items", { operation: randomUUID(), title: "Look into the logs" })).item;
  assert.equal(mine.owner, "usr_you");
  const given = (await ok("PATCH", `kuvo/work-queue/items/${mine.id}`, { assignee: { kind: "bot", id: "bot_scout00001" } })).item;
  assert.deepEqual([given.owner, given.number, given.handedBy], ["ses_scout", 1, "usr_you"]);
  assert.match((await tool("queue_show", { item: 1 }, "ses_scout")).text, /^#1 "Look into the logs" · Todo · for Scout/);
  // One in a conversation's queue stays there, whoever it's for.
  await tool("queue_add", { title: "The lead's" }, "ses_lead");
  assert.equal((await ok("PATCH", "kuvo/work-queue/items/2", { assignee: { kind: "bot", id: "bot_scout00001" } })).item.owner, "ses_lead");
});

test("the box says what may be done to each item, and refuses the rest", async () => {
  await tool("queue_add", { title: "Plain" }, "ses_lead");
  await tool("queue_add", { title: "To delete" }, "ses_lead");
  await ok("DELETE", "kuvo/work-queue/items/2");
  // An issue (its title and its closing are GitHub's), and one on its way to another computer.
  const kept = read("queues/ses_lead.json");
  const more = (n, extra) => ({ id: id(n), owner: "ses_lead", title: `Item ${n}`, body: "", status: "todo", order: 10 + n, labels: [], notes: [], files: [], links: [], sessions: [], created: { at: 1, by: LEAD }, updated: { at: 1, by: LEAD }, ...extra });
  kept.items.push(more(3, { source: { kind: "github-issue", ref: "gh:kuvo/api#12", url: "https://github.com/kuvo/api/issues/12", host: "github.com", id: "I_1" } }));
  kept.items.push(more(4, { number: 4, handing: { id: "wqh_0123456789ab", to: "ses_far", key: "k".repeat(43), name: "Far", at: 5, by: LEAD } }));
  write(path.join(queues, "queues", "ses_lead.json"), JSON.stringify(kept));
  const seen = await view("ses_lead");
  const can = title => [...seen.items, ...seen.removed].find(item => item.title === title).can;
  assert.deepEqual(can("Plain"), { edit: true, set: true, assign: true, close: true, delete: true, restore: false, attach: true });
  assert.deepEqual(can("To delete"), { edit: false, set: false, assign: false, close: false, delete: false, restore: true, attach: false, why: "It's deleted: bring it back to change it." });
  assert.deepEqual(can("Item 3"), { edit: false, set: true, assign: true, close: false, delete: true, restore: false, attach: false, why: "Its title, its description and closing it are GitHub's." });
  assert.deepEqual(can("Item 4"), { edit: false, set: false, assign: false, close: false, delete: true, restore: false, attach: false, why: "It's on its way to Far on another computer: it can be changed there once it's arrived." });
  assert.deepEqual((await ok("GET", `kuvo/work-queue/items/${id(3)}`)).item.can, can("Item 3"), "an item whole says the same");

  // What it says can't be done is refused.
  const refused = async (pathname, body, why, method = "PATCH") => {
    const answer = await route(method, pathname, body);
    assert.equal(answer.status, 400, `${pathname} ${JSON.stringify(body)}`);
    assert.match(answer.body.message, why);
  };
  await refused(`kuvo/work-queue/items/${id(3)}`, { title: "Mine now" }, /takes its title and description from GitHub/);
  await refused(`kuvo/work-queue/items/${id(3)}`, { status: "done" }, /closed on GitHub/);
  await refused(`kuvo/work-queue/items/${id(3)}`, { status: "canceled" }, /closed on GitHub/);
  assert.equal((await ok("PATCH", `kuvo/work-queue/items/${id(3)}`, { status: "in-review", priority: "low" })).item.status, "in-review");
  await refused("kuvo/work-queue/items/2", { title: "Back?" }, /deleted: bring it back first/);
  await refused("kuvo/work-queue/items/4", { status: "done" }, /being handed to Far/);
  await refused("kuvo/work-queue/items/1/restore", undefined, /isn't deleted/, "POST");
});

test("a search finds words in titles, descriptions and notes, and its counts are of what it found", async () => {
  await ok("POST", "kuvo/work-queue/labels", { add: "Billing" });
  await tool("queue_add", { title: "Invoices are late", body: "The cron job stalls.", labels: ["Billing"], priority: "high" }, "ses_lead");
  await tool("queue_add", { title: "Dark mode icons", status: "backlog" }, "ses_lead");
  await tool("queue_add", { title: "Sign-in loops" }, "ses_child");
  await tool("queue_update", { item: 3, note: "Traced it to the cron scheduler.", status: "done" }, "ses_child");
  await tool("queue_add", { title: "Old cron notes" }, "ses_lead");
  await ok("DELETE", "kuvo/work-queue/items/4");

  const all = await view("all", "&counts=1");
  assert.equal(all.q, undefined);
  assert.deepEqual([all.counts.open, all.counts.removed, all.counts.statuses.todo, all.counts.statuses.backlog, all.counts.statuses.done, all.counts.statuses["needs-you"], all.counts.priorities.high, all.counts.priorities.normal, all.counts.labels.lbl_billing], [2, 1, 1, 1, 1, 0, 1, 2, 1]);

  const found = await view("all", "&counts=1&q=CRON");
  assert.equal(found.q, "CRON");
  assert.deepEqual(found.items.map(item => item.number), [1, 3], "in a description, and in a note");
  assert.deepEqual(found.removed.map(item => item.number), [4], "and among what's deleted");
  assert.deepEqual([found.counts.open, found.counts.removed, found.counts.statuses.todo, found.counts.statuses.backlog, found.counts.statuses.done, found.counts.labels.lbl_billing], [1, 1, 1, 0, 1, 1]);
  // Every word, any case; a number; a label's name.
  assert.deepEqual((await view("all", "&q=cron%20stalls")).items.map(item => item.number), [1]);
  assert.deepEqual((await view("all", "&q=%232")).items.map(item => item.number), [2]);
  assert.deepEqual((await view("all", "&q=billing")).items.map(item => item.number), [1]);
  const none = await view("all", "&counts=1&q=nothing-like-this");
  assert.deepEqual([none.items.length, none.counts.open, none.counts.statuses.todo], [0, 0, 0]);
  // A lead's own view is searched the same.
  assert.deepEqual((await view("ses_child", "&q=cron")).items.map(item => item.number), [3]);
});

test("a view filtered by computer, source, owner or label lists only those, and its counts are of what it lists", async () => {
  await ok("POST", "kuvo/work-queue/labels", { add: "Bug" });
  await ok("POST", "kuvo/work-queue/labels", { add: "Docs" });
  await tool("queue_add", { title: "Lead's bug", labels: ["Bug"] }, "ses_lead");
  await tool("queue_add", { title: "Lead's docs", labels: ["Docs"], status: "backlog" }, "ses_lead");
  await tool("queue_add", { title: "Child's plain" }, "ses_child");
  await ok("POST", "kuvo/work-queue/items", { operation: randomUUID(), title: "Mine, a bug", labels: ["Bug"] });
  const kept = read("queues/ses_lead.json");
  kept.items.push({ id: id(9), owner: "ses_lead", title: "An issue", body: "", status: "todo", order: 9, labels: [], notes: [], files: [], links: [], sessions: [], created: { at: 1, by: LEAD }, updated: { at: 1, by: LEAD }, source: { kind: "github-issue", ref: "gh:kuvo/api#12", url: "https://github.com/kuvo/api/issues/12", host: "github.com", id: "I_1" } });
  write(path.join(queues, "queues", "ses_lead.json"), JSON.stringify(kept));
  const titles = async more => (await view("all", more)).items.map(item => item.title).sort();
  assert.deepEqual(await titles("&labels=lbl_bug"), ["Lead's bug", "Mine, a bug"]);
  assert.deepEqual(await titles("&labels=lbl_bug,lbl_docs"), ["Lead's bug", "Lead's docs", "Mine, a bug"], "one of a kind widens");
  assert.deepEqual(await titles("&labels=lbl_bug&owners=usr_you"), ["Mine, a bug"], "another kind narrows");
  assert.deepEqual(await titles("&labels=none"), ["An issue", "Child's plain"]);
  assert.deepEqual(await titles("&owners=ses_child,usr_you"), ["Child's plain", "Mine, a bug"]);
  assert.deepEqual(await titles("&sources=none&owners=ses_lead"), ["Lead's bug", "Lead's docs"]);
  assert.equal((await titles("&computers=")).length, 5, "this box's own");
  assert.deepEqual(await titles("&computers=some-other-key"), []);
  const issues = await view("all", "&counts=1&sources=" + (await view("all")).items.find(item => item.title === "An issue").source.kind);
  assert.deepEqual(issues.items.map(item => item.title), ["An issue"]);
  // Its counts are of what it lists, with the search too; and it says the filters back.
  const found = await view("all", "&counts=1&labels=lbl_bug,lbl_docs&q=lead");
  assert.deepEqual(found.filters, { labels: ["lbl_bug", "lbl_docs"] });
  assert.deepEqual([found.counts.open, found.counts.statuses.todo, found.counts.statuses.backlog, found.counts.labels.lbl_bug, found.counts.labels.lbl_docs], [2, 1, 1, 1, 1]);
  assert.equal((await view("all", "&counts=1")).filters, undefined);
});

test("labels: none built in; you and your agents make, rename, recolour and remove them, several on an item", async () => {
  assert.deepEqual((await view("all")).labels, []);
  assert.deepEqual((await ok("GET", "kuvo/work-queue/labels")).labels, []);
  const first = await ok("POST", "kuvo/work-queue/labels", { add: "  Bug  " });
  assert.deepEqual([first.label.id, first.label.name], ["lbl_bug", "Bug"]);
  const second = (await ok("POST", "kuvo/work-queue/labels", { add: "Design" })).label;
  assert.notEqual(second.color, first.label.color, "a new one takes a colour the fewest have");
  assert.equal((await ok("POST", "kuvo/work-queue/labels", { add: "Security", color: "violet" })).label.color, "violet");
  for (const [body, why] of [[{ add: "bug" }, /a label called bug already/], [{ add: "  " }, /Say what the label is called/], [{ add: "Ops", color: "beige" }, /isn't a label's colour/], [{ change: "Nope", name: "x" }, /no label Nope/],
    [{ change: "Design", name: "Bug" }, /a label called Bug already/], [{ change: "Design" }, /its new name/], [{ remove: "Nope" }, /no label Nope/], [{}, /Send \{add/]]) {
    const refused = await route("POST", "kuvo/work-queue/labels", body);
    assert.equal(refused.status, 400, JSON.stringify(body));
    assert.match(refused.body.message, why);
  }
  assert.equal((await route("POST", "kuvo/work-queue/labels", { add: "Sneaky" }, {})).status, 403);
  // Renamed and recoloured, it's the same label on the items that have it.
  await tool("queue_add", { title: "Crash on launch", labels: ["Bug", "Security"] }, "ses_lead");
  await tool("queue_add", { title: "Gone already", labels: ["Bug"] }, "ses_lead");
  await ok("DELETE", "kuvo/work-queue/items/2");
  const changed = await ok("POST", "kuvo/work-queue/labels", { change: "lbl_bug", name: "Defect", color: "pink" });
  assert.deepEqual(changed.label, { id: "lbl_bug", name: "Defect", color: "pink" });
  let seen = await view("all", "&counts=1");
  assert.deepEqual(seen.items[0].labels, ["lbl_bug", "lbl_security"]);
  assert.deepEqual(seen.labels.map(each => each.name), ["Defect", "Design", "Security"]);
  assert.deepEqual(seen.counts.labels, { lbl_bug: 1, lbl_design: 0, lbl_security: 1 });
  // An agent, as the user asked it to.
  assert.match((await tool("queue_labels", {}, "ses_lead")).text, /^Labels \(one set for every queue; an item carries several\): Defect \(pink\), Design \(\w+\), Security \(violet\)\./);
  assert.match((await tool("queue_labels", { add: "Docs", color: "sky" }, "ses_lead")).text, /^Made the label Docs\. Labels now: Defect \(pink\), Design \(\w+\), Security \(violet\), Docs \(sky\)\.$/);
  assert.match((await tool("queue_labels", { rename: "Docs", to: "Writing" }, "ses_lead")).text, /^Changed the label Writing\./);
  assert.match((await tool("queue_labels", { recolor: "Writing", color: "lime" }, "ses_lead")).text, /Writing \(lime\)/);
  const notOne = await tool("queue_labels", { add: "In progress" }, "ses_lead");
  assert.deepEqual([notOne.isError, notOne.text], [true, "In progress is a status, not a label: set it on an item with status."]);
  assert.equal((await tool("queue_update", { item: 1, labels: ["Nope"] }, "ses_lead")).text, "There's no label Nope: queue_labels lists them, and makes one.");
  // Removed, it comes off every item here, a deleted one too, and is gone.
  const removed = await ok("POST", "kuvo/work-queue/labels", { remove: "Defect" });
  assert.equal(removed.label, undefined);
  assert.deepEqual(read("labels.json").removed, ["lbl_bug"]);
  seen = await view("all");
  assert.deepEqual(seen.labels.map(each => each.name), ["Design", "Security", "Writing"]);
  assert.deepEqual(seen.items[0].labels, ["lbl_security"]);
  assert.deepEqual((await ok("POST", "kuvo/work-queue/items/2/restore")).item.labels, []);
  assert.match((await tool("queue_labels", { remove: "Writing" }, "ses_lead")).text, /^Removed \(and took off every item here\) the label Writing\. Labels now: Design \(\w+\), Security \(violet\)\.$/);
  // An item carries ten at most.
  for (let n = 0; n < 11; n++) await ok("POST", "kuvo/work-queue/labels", { add: `Kind ${n}` });
  const many = await route("PATCH", "kuvo/work-queue/items/1", { labels: Array.from({ length: 11 }, (_, n) => `Kind ${n}`) });
  assert.deepEqual([many.status, many.body.message], [400, "An item carries 10 labels at most."]);
  assert.equal((await ok("POST", "kuvo/work-queue/labels", { add: "Bug" })).label.id, "lbl_bug-2");
});

test("an agent says where its work stands: a status it sets, a priority, who it's for and when it's due", async () => {
  const added = await tool("queue_add", { title: "Ship the release", status: "Backlog", priority: "Urgent", assignee: "Scout", due: "2026-12-24" }, "ses_lead");
  assert.equal(added.text, 'Added #1 "Ship the release" as Backlog. Refer to it as #1.');
  await tool("queue_add", { title: "Write the notes" }, "ses_lead");
  // Saying it's on it makes a Todo, Backlog or Waiting one In progress; stopping leaves it there.
  assert.equal((await tool("queue_update", { item: 1, working: true }, "ses_lead")).text, 'Changed #1 "Ship the release" (In progress, 1 session linked).');
  assert.equal((await tool("queue_update", { item: 1, working: false }, "ses_lead")).text, 'Changed #1 "Ship the release" (In progress).');
  // A status said in the same call is the one it has.
  assert.equal((await tool("queue_update", { item: 2, working: true, status: "waiting" }, "ses_lead")).text, 'Changed #2 "Write the notes" (Waiting, 1 session linked).');
  assert.equal((await tool("queue_update", { item: 2, status: "In review" }, "ses_lead")).text, 'Changed #2 "Write the notes" (In review, 1 session linked).');
  assert.equal((await tool("queue_update", { item: 2, working: true }, "ses_lead")).text, 'Changed #2 "Write the notes" (In review, 1 session linked).', "one In review stays so while its agent works on");
  active = new Set(["ses_lead"]);
  const listed = await tool("queue_list", {}, "ses_lead");
  assert.equal(listed.text, 'Work Queue (2 open):\n\nIn review:\n- #2 "Write the notes" · "Lead" at work · updated just now\n\nIn progress:\n- #1 "Ship the release" · Urgent · for Scout · due 2026-12-24 · updated just now');
  assert.match((await tool("queue_list", { status: "in progress" }, "ses_lead")).text, /^Work Queue \(2 open\):\n\nIn progress:\n- #1/);
  assert.equal((await tool("queue_list", { status: "Todo" }, "ses_lead")).text, "Nothing in the Work Queue matches.");
  assert.equal((await tool("queue_update", { item: 1, status: "Canceled" }, "ses_lead")).text, 'Marked #1 "Ship the release" canceled.');
  assert.doesNotMatch((await tool("queue_list", {}, "ses_lead")).text, /Ship the release/);
  assert.match((await tool("queue_list", { done: true }, "ses_lead")).text, /Canceled:\n- #1 "Ship the release" · Urgent · for Scout · due 2026-12-24 · canceled just now/);
  // What isn't one is refused, saying what is.
  assert.equal((await tool("queue_update", { item: 2, status: "soon" }, "ses_lead")).text, "There's no status soon: it's one of Needs you, In review, In progress, Waiting, Todo, Backlog, Done, Canceled.");
  assert.equal((await tool("queue_update", { item: 2, priority: "max" }, "ses_lead")).text, "There's no priority max: it's one of Urgent, High, Normal, Low.");
  assert.match((await tool("queue_update", { item: 2, assignee: "Nobody" }, "ses_lead")).text, /^There's no bot or coding agent Nobody here to assign it to\. It can be: Scout, Claude Code, Codex/);
  assert.equal((await tool("queue_update", { item: 2, assignee: null, due: null }, "ses_lead")).isError, false);
  // An agent that still says category is told what to say, and nothing changes.
  const old = await tool("queue_update", { item: 2, category: "done" }, "ses_lead");
  assert.deepEqual([old.isError, old.text], [true, "Items have a status and labels now, not a category. Give status (Needs you, In review, In progress, Waiting, Todo, Backlog, Done, Canceled) for where it stands, and labels for what kind of work it is (queue_labels lists them)."]);
  assert.match((await tool("queue_show", { item: 2 }, "ses_lead")).text, /^#2 "Write the notes" · In review/);
});
