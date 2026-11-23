// Edits to a bot whose computer is away, held by the box that was asked (PROTOCOL.md, `kuvo/bots`,
// "Held for another computer"): a client changes or deletes a bot on another of your computers
// through any box it reaches; that box takes it at once, keeps it (each field the last given, a
// delete over all of it), says so in its bots' list, and hands it on when that computer answers,
// which does it once and answers. What it refuses is let go, and said in plain words.
//
// One box (a machine, here, by the real link in this process), its Mac and another machine as keys
// whose mail is sealed and opened as the relay carries it; only the engine is a stand-in.
//
// Run with `node --test box/bridge-tests/bots-held.test.js`.
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

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-bots-held-")));
removeAtEnd(home);
process.env.KUVO_HOME = home;
const link = await import(LINK_BUNDLE.href);
const run = path.join(home, "run");
const bots = path.join(home, "bots");
const held = path.join(run, "bots-held");
const write = (file, value) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, typeof value === "string" ? value : JSON.stringify(value));
};

// What this box makes bots with (as bots.test.js has it, shortened).
write(path.join(home, "bin", "bot-words.json"), {
  kuvo: 1,
  templates: [],
  personalities: [{ id: "preset.straight", name: "Straight Shooter", description: "Direct.", symbol: "bolt" }],
  personalityWords: "Your personality: ⟦name⟧. ⟦description⟧",
  looks: { colors: ["mint", "sky"], shapes: ["round"], faces: ["calm", "happy"] },
  instructions: ["You are ⟦name⟧, one of the user's bots⟦jobLine⟧.", "⟦personality⟧", "⟦jobPart⟧"],
  here: { mac: { place: "this Mac" }, machine: { place: "the machine \"⟦machine⟧\"" } },
  job: { none: "", line: ": ⟦job⟧", details: "Your job:\n⟦jobDetails⟧" },
  projectNote: "",
  identityNote: "",
  memory: "# ⟦name⟧'s memory\n",
  greeting: "Write first.",
});
const kit = { kuvo: 1, personalities: [], startingModel: { id: "big-pickle", providerID: "opencode" }, projects: [], timeZone: "Europe/Istanbul" };

// The engine: what it was asked.
const asked = [];
const engine = http.createServer((request, response) => {
  let body = "";
  request.on("data", chunk => { body += chunk; });
  request.on("end", () => {
    asked.push({ method: request.method, url: request.url, body: body ? JSON.parse(body) : undefined });
    const reply = (value, status = 200) => { response.writeHead(status, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
    if (request.url === "/api/provider") return reply({ data: [{ id: "opencode" }] });
    if (request.url === "/api/model") return reply({ data: [{ id: "big-pickle", providerID: "opencode", name: "Big Pickle", variants: [{ id: "low" }, { id: "high" }] }] });
    reply({});
  });
});
await new Promise(resolve => engine.listen(0, "127.0.0.1", resolve));
atEnd(() => { engine.closeAllConnections(); engine.close(); });
const target = { port: engine.address().port, password: "p" };

// This box (a machine, "devbox"), the Mac that set it up, and another machine ("lab").
const k = key => key.toString("base64url");
const box = (kind, name, id) => {
  const key = link.newKey();
  const address = k(link.addressOf(crypto.randomBytes(32)));
  return { key, pub: k(link.publicOf(key)), address, peer: { key: k(link.publicOf(key)), address, name, kind, ...(id ? { id } : {}) } };
};
const here = box("machine", "devbox", "devbox"), mac = box("mac", "Studio"), lab = box("machine", "lab", "lab");
const secret = crypto.randomBytes(32).toString("base64url");
const fresh = () => link.writeLink({
  secret, key: k(here.key), relay: "wss://relay.test", name: "devbox", id: "devbox", mac: mac.pub, macs: [mac.pub], phones: [],
  legacy: { [mac.pub]: { at: 1, peers: [mac.peer, lab.peer, here.peer] } },
});
// What each peer's agents' mail said: lab keeps Wren, and takes held edits.
const agentsTold = ({ labFeatures = ["bots.held"], labBots = [{ session: "ses_wren", name: "Wren", kind: "bot", id: "bot_wren0000001" }], macBots = [], labTrash = "kuvo", macTrash = "trash" } = {}) => write(path.join(run, "peer-agents.json"), {
  [lab.pub]: { at: 1, heard: Date.now(), kind: "machine", id: "lab", list: labBots, features: labFeatures, ...(labTrash ? { trash: labTrash } : {}) },
  [mac.pub]: { at: 1, heard: Date.now(), kind: "mac", list: macBots, features: ["bots.held"], ...(macTrash ? { trash: macTrash } : {}) },
});

// Mail as the relay hands it on, from `from` to this box; and what this box sent, opened as `to` would.
const mailFrom = (from, payload) => link.sealOneWay(Buffer.from(JSON.stringify({ ...payload, at: Date.now() })), from.key, link.publicOf(here.key)).message;
const opened = (bytes, to) => JSON.parse(link.openOneWay(link.readFrame(bytes.subarray(17)).body, to.key, link.publicOf(here.key)).payload.toString("utf8"));
const sent = [];
const host = { welcomed: true, send: bytes => sent.push(bytes) };
const receive = (from, payload) => link.receiveMail(mailFrom(from, payload), host, (sender, given) => link.peerMail(sender, given, host, () => ({ targets: [target], bridge: false })));
const mailTo = to => {
  const theirs = sent.filter(bytes => k(bytes.subarray(1, 17)) === to.address);
  for (const bytes of theirs) sent.splice(sent.indexOf(bytes), 1);
  return theirs.map(bytes => opened(bytes, to));
};

// A client's request to this box.
const handle = link.engineHandler(() => target, { name: "devbox" }, () => null);
async function route(method, pathname, body) {
  const pieces = [];
  for await (const piece of handle({ method, path: pathname, headers: {}, body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)), fromApp: true }, undefined)) pieces.push(piece);
  const text = Buffer.concat(pieces.filter(each => each.body).map(each => each.body)).toString();
  let parsed = text;
  try { parsed = JSON.parse(text); } catch {}
  return { status: pieces.find(each => each.head).head.status, body: parsed };
}
const listed = async () => (await route("GET", "kuvo/bots")).body;
const wren = "bot_wren0000001";

beforeEach(() => {
  fs.rmSync(run, { recursive: true, force: true });
  fs.rmSync(bots, { recursive: true, force: true });
  write(path.join(run, "engine.json"), { ...target, version: "0.0.0" });
  sent.length = 0;
  asked.length = 0;
  link.forgetHeldSends();
  link.heldSoon.send = () => link.sendHeld(host);
  fresh();
  agentsTold();
});

test("an edit to a bot whose computer is away is held by the box asked, merged a field at a time, and handed on when that computer is back", async () => {
  // Lab doesn't answer (the relay found nothing there).
  write(path.join(run, "peer-reach.json"), { [lab.pub]: Date.now() - 60_000 });
  const first = await route("PATCH", `kuvo/bots/${wren}`, { name: "  Wren\nthe Second ", look: { color: "mint", shape: "round", face: "calm" } });
  assert.equal(first.status, 202);
  assert.deepEqual({ ...first.body.held, since: 0 }, { bot: wren, box: lab.pub, computer: "lab", name: "Wren", change: { name: "Wren the Second", look: { color: "mint", shape: "round", face: "calm" } }, since: 0 });
  // A later one on top: each field the last given.
  const second = await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren III", personality: null });
  assert.equal(second.status, 202);
  assert.equal(second.body.held.since, first.body.held.since, "held since the first");
  assert.deepEqual(second.body.held.change, { name: "Wren III", look: { color: "mint", shape: "round", face: "calm" }, personality: null });
  // Its work settings change only where it lives, while that computer answers.
  const work = await route("PATCH", `kuvo/bots/${wren}`, { work: { limits: null } });
  assert.equal(work.status, 409);
  assert.match(work.body.message, /lab isn’t connected: change its work settings once it’s back/);
  // What every client following this box's bots sees: the edit held, for lab.
  const list = await listed();
  assert.deepEqual(list.held.map(each => [each.bot, each.computer, each.change.name]), [[wren, "lab", "Wren III"]]);
  assert.deepEqual(list.refused, []);

  // It went to lab as it was held (the relay keeps nothing), and isn't sent again within the minute.
  const tries = mailTo(lab);
  assert.ok(tries.length >= 1);
  const edit = tries.at(-1).botEdit;
  assert.deepEqual({ bot: edit.bot, change: edit.change }, { bot: wren, change: second.body.held.change });
  assert.deepEqual(Object.keys(edit.ages).sort(), ["look", "name", "personality"], "how long ago each field was given");
  assert.ok(Object.values(edit.ages).every(age => age >= 0));
  link.sendHeld(host);
  assert.deepEqual(mailTo(lab), [], "not again at once");
  // Lab is back (any mail from it): it goes now, not at its next minute.
  receive(lab, {});
  const [again] = mailTo(lab);
  assert.equal(again.botEdit.id, edit.id);
  // Only lab's answer is taken; then it's let go.
  receive(mac, { botEdited: { id: edit.id, bot: wren, status: 200 } });
  assert.equal((await listed()).held.length, 1, "the Mac's word for lab's isn't taken");
  receive(lab, { botEdited: { id: edit.id, bot: wren, status: 200 } });
  assert.deepEqual((await listed()).held, []);
  assert.deepEqual(fs.readdirSync(held), [], "nothing left behind");
});

test("a delete wins over what's held, and nothing more is held for that bot; one lab refuses is let go and said in plain words", async () => {
  await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren II" });
  const deleting = await route("DELETE", `kuvo/bots/${wren}`);
  assert.equal(deleting.status, 202);
  assert.equal(deleting.body.held.delete, true);
  assert.equal(deleting.body.held.change, undefined);
  const after = await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren IV" });
  assert.equal(after.status, 404);
  assert.match(after.body.message, /deleted: it goes from lab once it’s back/);
  const [, sentDelete] = mailTo(lab);
  assert.deepEqual([sentDelete.botEdit.delete, sentDelete.botEdit.change], [true, undefined]);
  // Lab can't now (the bot moving): it waits on, and goes again later.
  receive(lab, { botEdited: { id: sentDelete.botEdit.id, bot: wren, status: 409, message: "It’s moving." } });
  assert.equal((await listed()).held.length, 1);
  // Gone there already: done.
  receive(lab, { botEdited: { id: sentDelete.botEdit.id, bot: wren, status: 404, message: "That bot isn’t there any more." } });
  assert.deepEqual(await listed().then(list => [list.held, list.refused]), [[], []]);

  // A change lab refuses: let go, and every client sees why, until a new edit for that bot.
  const change = await route("PATCH", `kuvo/bots/${wren}`, { personality: "preset.gone" });
  assert.equal(change.status, 202);
  const [went] = mailTo(lab);
  receive(lab, { botEdited: { id: went.botEdit.id, bot: wren, status: 400, message: "There’s no personality “preset.gone”." } });
  const list = await listed();
  assert.deepEqual(list.held, []);
  assert.deepEqual(list.refused.map(each => [each.bot, each.computer, each.message]), [[wren, "lab", "lab didn’t take the changes you made while it was away. There’s no personality “preset.gone”."]]);
  await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren V" });
  assert.deepEqual((await listed()).refused, [], "a new edit for it takes its place");
});

test("what can't be held: a bot no computer lists (unless the client names its computer), a change that isn't one, and anything planted in the folder", async () => {
  const unknown = await route("PATCH", "kuvo/bots/bot_nobody000001", { name: "X" });
  assert.equal(unknown.status, 404);
  // Named by the client (as a Mac names its machine): held for it.
  const named = await route("PATCH", "kuvo/bots/bot_nobody000001?on=lab", { name: "X" });
  assert.equal(named.status, 202);
  assert.equal(named.body.held.box, lab.pub);
  assert.equal((await route("DELETE", `kuvo/bots/bot_other000001?on=${encodeURIComponent(mac.pub)}`)).body.held.computer, "Studio");
  for (const body of [{ name: "  " }, { look: { color: "plaid", shape: "round", face: "calm" } }, { model: "big" }, { job: "x" }, { colour: "mint" }]) {
    const refused = await route("PATCH", `kuvo/bots/${wren}`, body);
    assert.equal(refused.status, 400, JSON.stringify(body));
    assert.equal(typeof refused.body.message, "string");
  }
  // Too much for one mail: never held, rather than waiting for ever.
  assert.equal((await route("PATCH", `kuvo/bots/${wren}`, { name: "x".repeat(40_000) })).status, 400);
  // A computer this box doesn't know now (a machine you removed), named by the client: held, and it
  // goes there once it's one of this box's computers again.
  assert.equal((await route("DELETE", "kuvo/bots/bot_gone00000001?on=attic")).status, 404, "one it doesn't know, unless the client says it's one you removed");
  const removed = await route("DELETE", "kuvo/bots/bot_gone00000001?on=attic&removed");
  assert.equal(removed.status, 202);
  assert.deepEqual([removed.body.held.box, removed.body.held.computer, removed.body.held.delete], ["", "attic", true]);
  const attic = box("machine", "attic", "attic");
  link.writeLink({ ...link.readLink(), legacy: { [mac.pub]: { at: 2, peers: [mac.peer, lab.peer, here.peer, attic.peer] } } });
  write(path.join(run, "peer-agents.json"), { ...JSON.parse(fs.readFileSync(path.join(run, "peer-agents.json"), "utf8")), [attic.pub]: { at: 1, heard: Date.now(), kind: "machine", id: "attic", list: [], features: ["bots.held"] } });
  link.sendHeld(host);
  assert.deepEqual(mailTo(attic).map(each => [each.botEdit.bot, each.botEdit.delete]), [["bot_gone00000001", true]]);
  assert.equal((await listed()).held.find(each => each.bot === "bot_gone00000001").box, attic.pub);
  // Left by something else on this computer, not sealed with this box's secret: never sent, never shown.
  sent.length = 0;
  write(path.join(held, "bot_planted00001.json"), { id: `bte_${"a".repeat(22)}`, bot: "bot_planted00001", to: lab.pub, delete: true, since: 1, at: 1, seal: "x".repeat(43) });
  // (And a refusal: not shown.)
  write(path.join(held, "refused.json"), { refused: [{ bot: wren, box: lab.pub, computer: "lab", message: "Planted.", at: Date.now() }], seal: "x".repeat(43) });
  assert.deepEqual((await listed()).refused, []);
  link.forgetHeldSends();
  link.sendHeld(host);
  assert.deepEqual(mailTo(lab).filter(each => each.botEdit.bot === "bot_planted00001"), []);
  assert.equal((await listed()).held.some(each => each.bot === "bot_planted00001"), false);
  // Lab not up to date yet: it waits until it is.
  agentsTold({ labFeatures: [] });
  link.forgetHeldSends();
  link.sendHeld(host);
  assert.deepEqual(mailTo(lab), []);
});

test("a bot that moved meanwhile: what's held for it goes to the computer it's on now", async () => {
  await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren II" });
  const [toLab] = mailTo(lab);
  // Now the Mac lists it, and lab, whose word is older, still does.
  agentsTold({ macBots: [{ session: "ses_wren", name: "Wren", kind: "bot", id: wren }] });
  receive(lab, { botEdited: { id: toLab.botEdit.id, bot: wren, status: 404, message: "That bot isn’t there any more." } });
  assert.equal((await listed()).held[0].computer, "Studio");
  const [toMac] = mailTo(mac);
  assert.deepEqual(toMac.botEdit.change, { name: "Wren II" });
  // And back to lab: the Mac, which says it isn't there, still lists it from before; it goes to lab.
  agentsTold({ macBots: [{ session: "ses_wren", name: "Wren", kind: "bot", id: wren }] });
  receive(mac, { botEdited: { id: toMac.botEdit.id, bot: wren, status: 404, message: "That bot isn’t there any more." } });
  assert.equal((await listed()).held[0].computer, "lab");
});

test("the computer that keeps the bot does a held edit once, as Edit Bot would, and answers; a delete too", async () => {
  write(path.join(bots, "kit.json"), kit);
  const folder = path.join(bots, "bot_scout00001");
  write(path.join(folder, "MEMORY.md"), "# Scout's memory\n");
  write(path.join(folder, "session"), "ses_scout");
  write(path.join(folder, "bot.json"), { kuvo: 1, id: "bot_scout00001", name: "Scout", job: "", look: { color: "sky", shape: "round", face: "calm" }, model: { id: "big-pickle", providerID: "opencode" }, created: "2026-10-01T09:00:00Z" });
  const id = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id, bot: "bot_scout00001", change: { name: "Scout II" }, at: Date.now() } });
  const answer = async () => {
    for (;;) {
      const [mail] = mailTo(lab);
      if (mail) return mail.botEdited;
      await new Promise(resolve => setImmediate(resolve));
    }
  };
  assert.deepEqual(await answer(), { id, bot: "bot_scout00001", status: 200 });
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).name, "Scout II");
  assert.equal(asked.filter(each => each.method === "PATCH" && each.url === "/api/session/ses_scout").at(-1)?.body?.title, "Scout II", "its conversation renamed");
  // Changed here since, then sent again (its answer lost): answered as it went, not done again.
  await route("PATCH", "kuvo/bots/bot_scout00001", { name: "Scout III" });
  receive(lab, { botEdit: { id, bot: "bot_scout00001", change: { name: "Scout II" }, at: Date.now() } });
  assert.deepEqual(await answer(), { id, bot: "bot_scout00001", status: 200 });
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).name, "Scout III");
  // One held elsewhere that you made before the change here (its Mac was offline): it doesn't undo
  // it, though its look, given later, lands; one you made after it does. (The change here, a moment
  // ago, as its folder notes it.)
  const edited = path.join(folder, "edited.json");
  write(edited, { ...JSON.parse(fs.readFileSync(edited, "utf8")), name: Date.now() - 30_000 });
  const older = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id: older, bot: "bot_scout00001", change: { name: "Scout Old", look: { color: "mint", shape: "round", face: "happy" } }, ages: { name: 60_000, look: 1_000 }, at: Date.now() } });
  assert.equal((await answer()).status, 200);
  const now = JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8"));
  assert.deepEqual([now.name, now.look.face], ["Scout III", "happy"]);
  const newer = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id: newer, bot: "bot_scout00001", change: { name: "Scout IV" }, ages: { name: 1_000 }, at: Date.now() } });
  assert.equal((await answer()).status, 200);
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).name, "Scout IV");
  // One with a bad field: the rest lands, and what didn't is said.
  const mixed = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id: mixed, bot: "bot_scout00001", change: { name: "Scout V", personality: "preset.gone" }, ages: { name: 0, personality: 0 }, at: Date.now() } });
  const partly = await answer();
  assert.equal(partly.status, 200);
  assert.match(partly.message, /preset\.gone/);
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).name, "Scout V");
  // Its work settings never come this way.
  const work = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id: work, bot: "bot_scout00001", change: { work: { limits: null, allowed: ["makeProject"] } }, at: Date.now() } });
  assert.equal((await answer()).status, 200);
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).work, undefined);
  // Refused: its words go back.
  const odd = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id: odd, bot: "bot_scout00001", change: { personality: "preset.gone" }, at: Date.now() } });
  const refused = await answer();
  assert.equal(refused.status, 400);
  assert.match(refused.message, /preset\.gone/);
  // Deleted, as Delete Bot… deletes it.
  const gone = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id: gone, bot: "bot_scout00001", delete: true, at: Date.now() } });
  assert.equal((await answer()).status, 200);
  assert.equal(fs.existsSync(folder), false);
});

test("a computer that says the bot isn't there, with none listing it yet (it's on its way): asked again a few times before it's refused", async () => {
  await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren II" });
  agentsTold({ labBots: [] });
  for (let round = 0; round < 2; round++) {
    const [toLab] = mailTo(lab);
    receive(lab, { botEdited: { id: toLab.botEdit.id, bot: wren, status: 404, message: "That bot isn’t there any more." } });
    assert.equal((await listed()).held.length, 1, `still held after ${round + 1}`);
    assert.deepEqual((await listed()).refused, []);
    link.sendHeld(host, undefined, Date.now() + 61_000);
  }
  const [last] = mailTo(lab);
  receive(lab, { botEdited: { id: last.botEdit.id, bot: wren, status: 404, message: "That bot isn’t there any more." } });
  assert.deepEqual((await listed()).held, []);
  assert.match((await listed()).refused[0].message, /lab didn’t take the changes/);
});

test("some of a held change taken, some not: let go, and what wasn't is said", async () => {
  await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren II", personality: "preset.gone" });
  const [toLab] = mailTo(lab);
  receive(lab, { botEdited: { id: toLab.botEdit.id, bot: wren, status: 200, message: "There’s no personality “preset.gone”." } });
  const list = await listed();
  assert.deepEqual(list.held, []);
  assert.equal(list.refused[0].message, "lab didn’t take all the changes you made while it was away. There’s no personality “preset.gone”.");
});

// #783: a machine more than one of your Macs set up is yours, never "someone else's": what an
// earlier Kuvo Code says of it being shared changes nothing.
test("a machine an earlier Kuvo Code says is shared has its edits done here, as any of your computers", async () => {
  agentsTold({ labFeatures: ["bots.held"] });
  const told = JSON.parse(fs.readFileSync(path.join(run, "peer-agents.json"), "utf8"));
  told[lab.pub].shared = true;
  write(path.join(run, "peer-agents.json"), told);
  write(path.join(bots, "kit.json"), kit);
  const folder = path.join(bots, "bot_scout00001");
  write(path.join(folder, "session"), "ses_scout");
  write(path.join(folder, "bot.json"), { kuvo: 1, id: "bot_scout00001", name: "Scout", job: "", look: { color: "sky", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z" });
  const id = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id, bot: "bot_scout00001", change: { name: "Taken" }, ages: { name: 0 }, at: Date.now() } });
  for (;;) {
    const [mail] = mailTo(lab);
    if (mail) {
      assert.equal(mail.botEdited.status, 200);
      break;
    }
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).name, "Taken");
});

test("when a bot's fields last changed goes with it to another computer, and the clock of the computer that held an edit doesn't count", async () => {
  write(path.join(bots, "kit.json"), kit);
  // Scout, just moved here: its folder says its name changed ten seconds ago where it was.
  const folder = path.join(bots, "bot_scout00001");
  write(path.join(folder, "session"), "ses_scout");
  write(path.join(folder, "bot.json"), { kuvo: 1, id: "bot_scout00001", name: "Scout Moved", job: "", look: { color: "sky", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z" });
  write(path.join(folder, "edited.json"), { name: Date.now() - 10_000 });
  // An edit held for a minute by a computer whose clock is an hour ahead (as it says the time).
  const id = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id, bot: "bot_scout00001", change: { name: "Scout Old" }, ages: { name: 60_000 }, times: { name: Date.now() + 3_600_000 }, at: Date.now() + 3_600_000 } });
  for (;;) {
    const [mail] = mailTo(lab);
    if (mail) {
      assert.equal(mail.botEdited.status, 200);
      break;
    }
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).name, "Scout Moved", "the newer name stays");
  assert.ok(JSON.parse(fs.readFileSync(path.join(folder, "edited.json"), "utf8")).name <= Date.now(), "never a time ahead of this clock");
});

test("a held change its computer can't do now (its engine starting) waits on, rather than being refused for a field it can't take", async () => {
  write(path.join(bots, "kit.json"), kit);
  const folder = path.join(bots, "bot_scout00001");
  write(path.join(folder, "session"), "ses_scout");
  write(path.join(folder, "bot.json"), { kuvo: 1, id: "bot_scout00001", name: "Scout", job: "", look: { color: "sky", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z" });
  // Its engine not started yet.
  fs.rmSync(path.join(run, "engine.json"));
  const id = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id, bot: "bot_scout00001", change: { name: "Scout II", personality: "preset.gone" }, ages: { name: 0, personality: 0 }, at: Date.now() } });
  for (;;) {
    const [mail] = mailTo(lab);
    if (mail) {
      assert.equal(mail.botEdited.status, 503);
      break;
    }
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).name, "Scout");
});

test("asked again after it said the bot isn't there: done once the bot has arrived, not the old answer again", async () => {
  write(path.join(bots, "kit.json"), kit);
  const id = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  const edit = { botEdit: { id, bot: "bot_scout00001", change: { name: "Scout II" }, ages: { name: 0 }, at: Date.now() } };
  const answer = async () => {
    for (;;) {
      const [mail] = mailTo(lab);
      if (mail) return mail.botEdited;
      await new Promise(resolve => setImmediate(resolve));
    }
  };
  receive(lab, edit);
  assert.equal((await answer()).status, 404, "not here yet");
  // It arrives (a move from lab), and the same edit comes again.
  const folder = path.join(bots, "bot_scout00001");
  write(path.join(folder, "session"), "ses_scout");
  write(path.join(folder, "bot.json"), { kuvo: 1, id: "bot_scout00001", name: "Scout", job: "", look: { color: "sky", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z" });
  receive(lab, edit);
  assert.equal((await answer()).status, 200);
  assert.equal(JSON.parse(fs.readFileSync(path.join(folder, "bot.json"), "utf8")).name, "Scout II");
});

test("a new edit to a bot its computer said isn't there starts its tries over", async () => {
  await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren II" });
  agentsTold({ labBots: [] });
  for (let round = 0; round < 2; round++) {
    const [toLab] = mailTo(lab);
    receive(lab, { botEdited: { id: toLab.botEdit.id, bot: wren, status: 404, message: "That bot isn’t there any more." } });
    link.sendHeld(host, undefined, Date.now() + 61_000);
  }
  mailTo(lab);
  const again = await route("PATCH", `kuvo/bots/${wren}?on=lab`, { name: "Wren III" });
  assert.equal(again.status, 202);
  const [newer] = mailTo(lab);
  receive(lab, { botEdited: { id: newer.botEdit.id, bot: wren, status: 404, message: "That bot isn’t there any more." } });
  assert.equal((await listed()).held.length, 1, "still held: its first 404");
  assert.deepEqual((await listed()).refused, []);
});

// PROTOCOL.md, `kuvo/bots`, "The bots of your other computers" (`bots.elsewhere`).
const wrenLook = { color: "sky", shape: "round", face: "happy" };
const labWren = { session: "ses_wren", name: "Wren", kind: "bot", id: wren, about: "Watches the build", look: wrenLook };

test("the bots of your other computers are listed by the box asked, as each last told it, with what deleting one takes there", async () => {
  agentsTold({
    labBots: [labWren, { session: "ses_plain", name: "A session", kind: "session" }],
    macBots: [{ session: "ses_mira", name: "Mira", kind: "bot", id: "bot_mira0000001" }],
  });
  const { elsewhere, bots: own } = await listed();
  assert.deepEqual(own, []);
  assert.deepEqual(elsewhere.map(each => [each.id, each.name, each.computer, each.kind, each.box]), [
    ["bot_mira0000001", "Mira", "Studio", "mac", mac.pub],
    [wren, "Wren", "lab", "machine", lab.pub],
  ], "bots only, by computer");
  const there = elsewhere[1];
  assert.deepEqual({ ...there, deletes: [] }, { id: wren, session: "ses_wren", name: "Wren", look: wrenLook, job: "Watches the build", box: lab.pub, computer: "lab", kind: "machine", deletes: [] });
  assert.equal("look" in elsewhere[0] || "job" in elsewhere[0], false, "what a computer didn't say is left out");
  // What deleting one takes with it there, in words: a Mac's folder goes to the Trash, a machine's
  // to Kuvo Code's trash folder; its wake-ups aren't told between computers, so no count.
  assert.deepEqual(there.deletes, [
    { what: "conversation", words: "Its conversation is deleted." },
    { what: "folder", words: "Its folder, with its memory and what it made there, goes to Kuvo Code’s trash folder on lab." },
    { what: "wakes", words: "Its wake-ups and watches are cancelled." },
    { what: "groups", words: "It leaves the group chats on lab." },
    { what: "sessions", words: "The sessions it started stay.", kept: true },
  ]);
  // Each as that computer says where its deleted bots' folders go: the Mac's app takes them to the
  // Trash; with its app closed it says Kuvo Code's trash folder, as a machine does; and one that
  // hasn't said (from before it did) is said both ways. Always by the computer's name.
  const macFolder = async macTrash => {
    agentsTold({ macBots: [{ session: "ses_mira", name: "Mira", kind: "bot", id: "bot_mira0000001" }], macTrash });
    return (await listed()).elsewhere[0].deletes[1].words;
  };
  assert.equal(await macFolder("trash"), "Its folder, with its memory and what it made there, goes to the Trash on Studio.");
  assert.equal(await macFolder("kuvo"), "Its folder, with its memory and what it made there, goes to Kuvo Code’s trash folder on Studio.");
  assert.equal(await macFolder(null), "Its folder, with its memory and what it made there, goes to the Trash on Studio, or to Kuvo Code’s trash folder there.");
  // What it says in its agents' mail is what's kept (and nothing but those two words).
  receive(mac, { agents: { at: Date.now() + 5, list: [{ session: "ses_mira", name: "Mira", kind: "bot", id: "bot_mira0000001" }], trash: "kuvo" } });
  assert.match((await listed()).elsewhere[0].deletes[1].words, /Kuvo Code’s trash folder on Studio\./);
  receive(mac, { agents: { at: Date.now() + 10, list: [{ session: "ses_mira", name: "Mira", kind: "bot", id: "bot_mira0000001" }], trash: "/etc" } });
  assert.match((await listed()).elsewhere[0].deletes[1].words, /or to Kuvo Code’s trash folder there/);
});

test("bots are listed from each of your computers: one in another setup, one an earlier Kuvo Code says is shared, and on a machine two of your Macs set up (#783)", async () => {
  const setup = id => `set_${id.repeat(22)}`;
  link.writeLink({ ...link.readLink(), setup: { id: setup("a"), role: "member", primary: mac.pub } });
  const told = JSON.parse(fs.readFileSync(path.join(run, "peer-agents.json"), "utf8"));
  told[lab.pub].setup = setup("b");
  told[lab.pub].shared = true;
  told[lab.pub].list = [labWren];
  write(path.join(run, "peer-agents.json"), told);
  assert.deepEqual((await listed()).elsewhere.map(each => each.id), [wren]);
  const other = box("mac", "Other");
  link.writeLink({ ...link.readLink(), setup: undefined, macs: [mac.pub, other.pub] });
  assert.deepEqual((await listed()).elsewhere.map(each => each.id), [wren]);
});

test("a bot two computers list (it's moving) is the one's that said so last; one that lives here is this box's own", async () => {
  write(path.join(run, "peer-agents.json"), {
    [lab.pub]: { at: 1, heard: 1_000, kind: "machine", id: "lab", list: [labWren], features: [] },
    [mac.pub]: { at: 1, heard: 2_000, kind: "mac", list: [{ ...labWren, name: "Wren, moved" }], features: [] },
  });
  assert.deepEqual((await listed()).elsewhere.map(each => [each.name, each.computer]), [["Wren, moved", "Studio"]]);
  // It arrived here: listed among this box's own, and no longer as elsewhere.
  write(path.join(bots, "kit.json"), kit);
  write(path.join(bots, wren, "bot.json"), { kuvo: 1, id: wren, name: "Wren", look: wrenLook, created: "2026-10-01T09:00:00Z" });
  write(path.join(bots, wren, "session"), "ses_wren");
  const list = await listed();
  assert.deepEqual([list.bots.map(each => each.id), list.elsewhere], [[wren], []]);
});

test("a peer's agents' mail changes the list at once for those following this box's bots", async () => {
  agentsTold({ labBots: [] });
  const stop = new AbortController();
  const lines = handle({ method: "GET", path: "kuvo/bots/events", headers: {}, fromApp: true }, undefined, stop.signal);
  const next = async () => {
    for (;;) {
      const piece = (await lines.next()).value;
      if (piece?.body) return JSON.parse(piece.body.toString());
    }
  };
  try {
    assert.deepEqual((await next()).elsewhere, []);
    receive(lab, { agents: { at: Date.now(), list: [labWren], features: ["bots.held"] } });
    assert.deepEqual((await next()).elsewhere.map(each => each.name), ["Wren"]);
  } finally {
    stop.abort();
    await lines.return?.();
  }
});

test("what's held and what was refused name the bot and its look, as its computer last said, and still once it no longer lists it", async () => {
  agentsTold({ labBots: [labWren] });
  const first = await route("PATCH", `kuvo/bots/${wren}`, { name: "Wren II" });
  assert.deepEqual([first.body.held.name, first.body.held.look, first.body.held.change], ["Wren", wrenLook, { name: "Wren II" }], "who it is, and what you changed");
  // Among the bots elsewhere it's as you left it, and waiting: a client draws that as it is.
  const [left] = (await listed()).elsewhere;
  assert.deepEqual([left.id, left.name, left.look, left.waiting, left.kind, "unlisted" in left], [wren, "Wren II", wrenLook, true, "machine", false]);
  await route("PATCH", `kuvo/bots/${wren}`, { look: { color: "mint", shape: "round", face: "calm" } });
  assert.deepEqual((await listed()).elsewhere[0].look, { color: "mint", shape: "round", face: "calm" });
  // Lab renames it meanwhile: as it says now.
  agentsTold({ labBots: [{ ...labWren, name: "Wren the Elder" }] });
  assert.equal((await listed()).held[0].name, "Wren the Elder");
  // A delete on top, then lab stops listing it: who it was is kept with what's held.
  await route("DELETE", `kuvo/bots/${wren}`);
  agentsTold({ labBots: [] });
  let list = await listed();
  assert.deepEqual([list.held[0].name, list.held[0].look, list.held[0].delete], ["Wren the Elder", wrenLook, true]);
  // And it's still among the bots elsewhere, by what's held alone: not listed by any computer now.
  assert.deepEqual(list.elsewhere, [{ id: wren, name: "Wren the Elder", look: wrenLook, box: lab.pub, computer: "lab", waiting: true, unlisted: true }]);
  // Lab refuses it: the refusal names it the same way.
  const [{ botEdit }] = mailTo(lab).slice(-1);
  receive(lab, { botEdited: { id: botEdit.id, bot: wren, status: 403, message: "Not from here." } });
  list = await listed();
  assert.deepEqual(list.held, []);
  assert.deepEqual(list.refused.map(each => [each.bot, each.name, each.look, each.computer]), [[wren, "Wren the Elder", wrenLook, "lab"]]);
  assert.deepEqual(list.elsewhere, [{ id: wren, name: "Wren the Elder", look: wrenLook, box: lab.pub, computer: "lab", refused: true, unlisted: true }]);
  // One no computer of yours ever listed to this box has neither.
  const unknown = await route("PATCH", "kuvo/bots/bot_ghost000001?on=lab", { name: "Ghost" });
  assert.equal(unknown.status, 202);
  assert.equal("name" in unknown.body.held || "look" in unknown.body.held, false);
  // (Among the bots elsewhere, by the name you gave it.)
  assert.deepEqual((await listed()).elsewhere.find(each => each.id === "bot_ghost000001"), { id: "bot_ghost000001", name: "Ghost", box: lab.pub, computer: "lab", waiting: true, unlisted: true });
});

test("an effort held for another computer waits until that computer takes one", async () => {
  const held = await route("PATCH", `kuvo/bots/${wren}`, { effort: "high" });
  assert.deepEqual([held.status, held.body.held.change], [202, { effort: "high" }]);
  assert.deepEqual(mailTo(lab), [], "lab doesn't take an effort yet: it would drop it and call the change done");
  // Why it waits though lab answers is said, in words, to whoever follows this box's bots.
  assert.equal((await listed()).held[0].waits, "lab needs an update before it takes an effort: this goes once Kuvo Code is updated on lab.");
  assert.equal((await route("PATCH", `kuvo/bots/${wren}`, { effort: 3 })).status, 400);
  agentsTold({ labFeatures: ["bots.held", "bots.effort"] });
  assert.equal("waits" in (await listed()).held[0], false);
  link.sendHeld(host);
  assert.deepEqual(mailTo(lab).at(-1).botEdit.change, { effort: "high" });
  // A model picked after it, with no effort of its own, runs at its own: the effort held for the
  // model it had is let go, as two changes made where it lives would end.
  const model = await route("PATCH", `kuvo/bots/${wren}`, { model: { id: "opus", providerID: "anthropic" } });
  assert.deepEqual(model.body.held.change, { model: { id: "opus", providerID: "anthropic" } });
  assert.deepEqual(Object.keys(mailTo(lab).at(-1).botEdit.ages), ["model"]);
});

test("individual Job and Instructions edits wait for a computer that can take them", async () => {
  const path = `kuvo/bots/${wren}`;
  assert.equal((await route("PATCH", path, { jobDetails: "Check the build." })).status, 202);
  const job = await route("PATCH", path, { jobLine: "Build monitor" });
  assert.deepEqual(job.body.held.change, { jobLine: "Build monitor", jobDetails: "Check the build." });
  assert.deepEqual(mailTo(lab), []);
  assert.match((await listed()).held[0].waits, /separate Job and Instructions changes/);
  agentsTold({ labFeatures: ["bots.held", "bots.job-fields"] });
  link.sendHeld(host);
  assert.deepEqual(mailTo(lab).at(-1).botEdit.change, job.body.held.change);
  const whole = await route("PATCH", path, { job: { line: "New job", details: "New instructions." } });
  assert.deepEqual(whole.body.held.change, { job: { line: "New job", details: "New instructions." } });
});

test("a held whole job keeps the Instructions edited locally since it was sent", async () => {
  write(path.join(bots, "kit.json"), kit);
  const bot = "bot_scout00001";
  const folder = path.join(bots, bot);
  write(path.join(folder, "session"), "ses_scout");
  write(path.join(folder, "bot.json"), { kuvo: 1, id: bot, name: "Scout", job: "", look: { color: "sky", shape: "round", face: "calm" }, created: "2026-10-01T09:00:00Z" });
  assert.equal((await route("PATCH", `kuvo/bots/${bot}`, { jobDetails: "New local instructions." })).status, 200);
  const id = `bte_${crypto.randomBytes(16).toString("base64url")}`;
  receive(lab, { botEdit: { id, bot, change: { job: { line: "Build monitor", details: "Old instructions." } }, ages: { job: 10_000 }, at: Date.now() } });
  let answer;
  for (let attempt = 0; !answer && attempt < 100; attempt++) {
    await new Promise(resolve => setTimeout(resolve, 10));
    answer = mailTo(lab).find(mail => mail.botEdited?.id === id);
  }
  assert.equal(answer?.botEdited.status, 200);
  const { line, details } = JSON.parse(fs.readFileSync(path.join(folder, "job.json"), "utf8"));
  assert.deepEqual({ line, details }, { line: "Build monitor", details: "New local instructions." });
});

test("a peer saying its agents again changes nothing for those following this box's bots, and one it gives no name isn't listed", async () => {
  agentsTold({ labBots: [labWren, { ...labWren, id: "bot_blank000001", session: "ses_blank", name: "" }] });
  const before = JSON.stringify(await listed());
  // (The same list, heard later.)
  const told = JSON.parse(fs.readFileSync(path.join(run, "peer-agents.json"), "utf8"));
  told[lab.pub].heard += 60_000;
  write(path.join(run, "peer-agents.json"), told);
  assert.equal(JSON.stringify(await listed()), before);
  assert.deepEqual(JSON.parse(before).elsewhere.map(each => each.id), [wren]);
});

test("an edit held before a bot's name was kept with it is still this box's own, and still goes", async () => {
  // As a box from before `bots.elsewhere` wrote and sealed it: no `known`.
  const record = { id: `bte_${crypto.randomBytes(16).toString("base64url")}`, bot: wren, to: lab.pub, change: { name: "Wren II" }, times: { name: 5 }, since: 5, at: 5 };
  const key = crypto.createHmac("sha256", Buffer.from(secret, "base64url")).update("kuvo bot edits held").digest();
  const seal = crypto.createHmac("sha256", key).update(JSON.stringify([record.id, record.bot, record.to, null, record.change, record.times, false, record.since, record.at])).digest("base64url");
  write(path.join(held, `${wren}.json`), { ...record, seal });
  assert.deepEqual((await listed()).held.map(each => [each.bot, each.name, each.change]), [[wren, "Wren", { name: "Wren II" }]]);
  link.sendHeld(host);
  assert.equal(mailTo(lab).at(-1).botEdit.id, record.id);
});

test("an effort held for a bot here is its model's: not done once its model changed here since, and done as one given here otherwise", async () => {
  write(path.join(bots, "kit.json"), kit);
  write(path.join(bots, wren, "bot.json"), { kuvo: 1, id: wren, name: "Wren", look: wrenLook, model: { id: "big-pickle", providerID: "opencode" }, created: "2026-10-01T09:00:00Z" });
  write(path.join(bots, wren, "session"), "ses_wren");
  const answer = async () => {
    for (;;) {
      const [mail] = mailTo(mac).filter(each => each.botEdited);
      if (mail) return mail.botEdited;
      await new Promise(resolve => setImmediate(resolve));
    }
  };
  const edit = (effort, age) => receive(mac, { botEdit: { id: `bte_${crypto.randomBytes(16).toString("base64url")}`, bot: wren, change: { effort }, ages: { effort: age }, at: Date.now() } });
  const model = async () => (await listed()).bots[0].model;
  // Its model was picked here a moment ago; an effort you gave a minute before that, held on the
  // Mac meanwhile, was for the model it had then.
  assert.equal((await route("PATCH", `kuvo/bots/${wren}`, { model: { id: "big-pickle", providerID: "opencode" } })).status, 200);
  write(path.join(bots, wren, "edited.json"), { model: Date.now() - 30_000 });
  edit("low", 60_000);
  assert.equal((await answer()).status, 200);
  assert.deepEqual(await model(), { id: "big-pickle", providerID: "opencode" });
  // One given since is done, as `PATCH` with `effort` here would be; one its model hasn't is refused in words.
  edit("high", 1_000);
  assert.equal((await answer()).status, 200);
  assert.deepEqual(await model(), { id: "big-pickle", providerID: "opencode", variant: "high" });
  edit("max", 0);
  const refused = await answer();
  assert.deepEqual([refused.status, refused.message], [400, "Big Pickle has no “max” effort: its efforts are low, high."]);
});

test("a model and the effort held with it are one: a model this computer can't use takes no effort to the model it has, and the rest still lands", async () => {
  write(path.join(bots, "kit.json"), kit);
  write(path.join(bots, wren, "bot.json"), { kuvo: 1, id: wren, name: "Wren", look: wrenLook, model: { id: "big-pickle", providerID: "opencode" }, created: "2026-10-01T09:00:00Z" });
  write(path.join(bots, wren, "session"), "ses_wren");
  const answer = async () => {
    for (;;) {
      const [mail] = mailTo(mac).filter(each => each.botEdited);
      if (mail) return mail.botEdited;
      await new Promise(resolve => setImmediate(resolve));
    }
  };
  const edit = (change, ages) => receive(mac, { botEdit: { id: `bte_${crypto.randomBytes(16).toString("base64url")}`, bot: wren, change, ages, at: Date.now() } });
  const now = async () => { const [bot] = (await listed()).bots; return [bot.name, bot.model]; };
  // Luna isn't signed in here: refused with its effort, which isn't put on Big Pickle; the name lands.
  edit({ name: "Wren II", model: { id: "luna", providerID: "openai" }, effort: "high" }, { name: 0, model: 0, effort: 0 });
  const partly = await answer();
  assert.equal(partly.status, 200);
  assert.match(partly.message, /luna/);
  assert.deepEqual(await now(), ["Wren II", { id: "big-pickle", providerID: "opencode" }]);
  // A model given before its model last changed here is stale, and the effort given with it goes
  // with it, though given later.
  write(path.join(bots, wren, "edited.json"), { model: Date.now() - 30_000 });
  edit({ model: { id: "luna", providerID: "openai" }, effort: "high" }, { model: 60_000, effort: 1_000 });
  assert.equal((await answer()).status, 200);
  assert.deepEqual(await now(), ["Wren II", { id: "big-pickle", providerID: "opencode" }]);
});
