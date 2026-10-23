// Your other computers (PROTOCOL.md, "Peers"): who they are, their agents, and letters between
// them, by mail through the relay. Run with `node --test Packages/KuvoLink/Tests/Bridge/*.test.js`.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";

// The bridge keeps its state in KUVO_HOME, so give it a folder of its own before loading it.
const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-peers-"));
process.env.KUVO_HOME = home;
after(() => fs.rmSync(home, { recursive: true, force: true }));
const link = await import("../../Sources/KuvoLink/Resources/kuvo-link.js");

const run = path.join(home, "run");
const k = key => key.toString("base64url");
// A box: its key, and how a Mac lists it.
const box = (kind, id) => {
  const key = link.newKey();
  const address = k(link.addressOf(crypto.randomBytes(32)));
  return { key, pub: k(link.publicOf(key)), address, peer: { key: k(link.publicOf(key)), address, name: id ?? kind, kind, ...(id ? { id } : {}) } };
};
const here = box("machine", "lokum"), mac = box("mac"), second = box("machine", "second"), stranger = box("machine", "stranger");
const fresh = (extra = {}) => {
  fs.rmSync(run, { recursive: true, force: true });
  fs.mkdirSync(run, { recursive: true });
  link.writeLink({ secret: crypto.randomBytes(32).toString("base64url"), key: k(here.key), relay: "wss://relay.test", name: "lokum", id: "lokum", mac: mac.pub, macs: [mac.pub], phones: [], ...extra });
};
// Mail as the host hands it on (a mail frame's body), from `from` to this box.
const mailFrom = (from, payload) => link.sealOneWay(Buffer.from(JSON.stringify(payload)), from.key, link.publicOf(here.key)).message;
// What this box sent through the relay: to whom, and what it said (opened as `to` would).
const opened = (bytes, to) => {
  const relayed = { address: k(bytes.subarray(1, 17)), payload: bytes.subarray(17) };
  const body = link.readFrame(relayed.payload).body;
  return { address: relayed.address, payload: JSON.parse(link.openOneWay(body, to.key, link.publicOf(here.key)).payload.toString("utf8")) };
};

test("a Mac's peers come in its mail, a newer list replacing its last; this box left out, and no one else's list taken", () => {
  fresh();
  link.receiveMail(mailFrom(mac, { at: 10, peers: [mac.peer, second.peer, here.peer, { key: "bad", address: "bad", kind: "machine" }] }));
  assert.deepEqual(link.peersOf(link.readLink()).map(peer => peer.name), ["mac", "second"]);
  link.receiveMail(mailFrom(mac, { at: 5, peers: [mac.peer] }));
  assert.deepEqual(link.peersOf(link.readLink()).map(peer => peer.name), ["mac", "second"], "an older list changes nothing");
  // A peer, and anyone else, can't say who the peers are.
  link.receiveMail(mailFrom(second, { at: 20, peers: [stranger.peer] }));
  link.receiveMail(mailFrom(stranger, { at: 20, peers: [stranger.peer] }));
  assert.deepEqual(link.peersOf(link.readLink()).map(peer => peer.name), ["mac", "second"]);
  // The Mac removed `second`: it goes.
  link.receiveMail(mailFrom(mac, { at: 30, peers: [mac.peer] }));
  assert.deepEqual(link.peersOf(link.readLink()).map(peer => peer.name), ["mac"]);
  // Mail from a box that's no longer a peer isn't opened.
  const heard = [];
  link.receiveMail(mailFrom(second, { agents: { at: 1, list: [] } }), undefined, sender => heard.push(sender.key));
  link.receiveMail(mailFrom(mac, { agents: { at: 1, list: [] } }), undefined, sender => heard.push(sender.key));
  assert.deepEqual(heard, [mac.pub]);
});

test("each box's agents: a newer list kept, and list_agents here names each by where it is", () => {
  fresh();
  const file = path.join(run, "told.json"), lock = path.join(run, ".told.lock");
  const agent = (session, name, extra = {}) => ({ session, name, kind: "session", ...extra });
  assert.equal(link.takeAgents(second.peer, { at: 5, list: [agent("ses_b1", "API"), { session: "not-a-session", name: "x", kind: "session" }] }, file, lock), true);
  assert.equal(link.takeAgents(second.peer, { at: 4, list: [] }, file, lock), false, "an older list isn't taken");
  assert.equal(link.takeAgents(mac.peer, { at: 1, list: [agent("ses_m1", "Release", { kind: "bot" }), agent("ses_o1", "Old one", { machine: "old" }), agent("ses_b1", "Again")] }, file, lock), true);
  const told = link.readPeerAgents(file);
  assert.deepEqual(told[second.pub].list.map(each => each.session), ["ses_b1"]);
  const composed = link.composeAgents([agent("ses_h1", "Here")], "lokum", [second.peer, mac.peer], told);
  assert.deepEqual(composed, {
    machine: "lokum",
    agents: [
      { session: "ses_h1", name: "Here", kind: "session", machine: "lokum" },
      { session: "ses_b1", name: "API", kind: "session", machine: "second" },
      { session: "ses_m1", name: "Release", kind: "bot" },
      { session: "ses_o1", name: "Old one", kind: "session", machine: "old" },
    ],
  });
  assert.equal(link.peerOf("ses_o1", [second.peer, mac.peer], told)?.name, "mac", "one a Mac speaks for goes through it");
  assert.equal(link.peerOf("ses_nobody", [second.peer, mac.peer], told), null);
});

// An engine with one session, for letters to reach: its prompts, and an answer once it's done.
async function engine(t) {
  const prompts = [];
  let busy = 0;
  const server = http.createServer((request, response) => {
    let body = "";
    request.on("data", chunk => body += chunk);
    request.on("end", () => {
      const json = value => { response.writeHead(200, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
      if (request.headers.authorization !== `Basic ${Buffer.from("opencode:pw").toString("base64")}`) { response.writeHead(401); return response.end(); }
      if (request.url === "/api/session/ses_target") return json({ id: "ses_target" });
      if (request.url === "/api/session/ses_target/prompt" && request.method === "POST") {
        prompts.push(JSON.parse(body));
        busy = 1;
        return json({});
      }
      if (request.url === "/api/session/active") return json({ data: busy-- > 0 ? ["ses_target"] : [] });
      if (request.url.startsWith("/api/session/ses_target/message")) {
        const last = prompts.at(-1);
        return json({ data: last ? [{ type: "assistant", id: last.id + "z", content: [{ type: "text", text: "On it." }] }] : [] });
      }
      response.writeHead(404);
      response.end();
    });
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  t.after(() => server.close());
  return { prompts, target: { port: server.address().port, password: "pw" } };
}

test("a letter from a peer: from one of that box's agents, not too old, delivered once, said to be there, and with wait, answered", async t => {
  fresh();
  const { prompts, target } = await engine(t);
  const told = { [second.pub]: { at: 1, list: [{ session: "ses_sender", name: "API", kind: "session" }] } };
  const said = [];
  const send = (peer, message) => said.push({ to: peer.key, ...message });
  const letter = extra => ({ id: "ltr_" + crypto.randomBytes(6).toString("hex"), from: "ses_sender", name: "API", to: "ses_target", text: "Can you check the build?", at: Date.now(), ...extra });

  await link.takeLetter(second.peer, letter({ from: "ses_someone_else" }), send, [target], told);
  await link.takeLetter(second.peer, letter({ at: Date.now() - 2 * 86_400_000 }), send, [target], told);
  await link.takeLetter(stranger.peer, letter(), send, [target], told);
  assert.deepEqual(said.map(each => each.delivered.ok), [false, false, false]);
  assert.equal(prompts.length, 0);

  said.length = 0;
  const waiting = letter({ wait: true });
  await link.takeLetter(second.peer, waiting, send, [target], told);
  assert.equal(prompts.length, 1);
  assert.equal(prompts[0].text, link.composeLetter("API", "ses_sender", "Can you check the build?", true));
  assert.equal(prompts[0].delivery, "steer");
  assert.deepEqual(said, [{ to: second.pub, delivered: { id: waiting.id, ok: true } }, { to: second.pub, reply: { id: waiting.id, text: "On it." } }]);

  // The same letter again (the relay replayed it, or it was sent again): there already.
  said.length = 0;
  await link.takeLetter(second.peer, waiting, send, [target], told);
  assert.equal(prompts.length, 1);
  assert.deepEqual(said, [{ to: second.pub, delivered: { id: waiting.id, ok: true } }]);

  // A few at a time from each box.
  said.length = 0;
  for (let i = 0; i < 20; i++) await link.takeLetter(second.peer, letter(), send, [target], told);
  assert.equal(said.filter(each => each.delivered.ok).length, 19, "twenty in ten minutes, the one delivered before counted");
  assert.match(said.at(-1).delivered.why, /a lot of messages/);
});

test("this box's letters go to the box their agent is on, again each minute until it says it has them, cut to fit; only that box's word counts", () => {
  fresh({ peersFrom: { [mac.pub]: { at: 1, peers: [mac.peer, second.peer] } } });
  fs.writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify({ [second.pub]: { at: 1, kind: "machine", list: [{ session: "ses_target", name: "API", kind: "session" }] } }));
  const sending = path.join(run, "outbox", "sending");
  fs.mkdirSync(sending, { recursive: true });
  const now = Date.now();
  const write = (id, letter) => fs.writeFileSync(path.join(sending, `${id}.json`), JSON.stringify({ from: "ses_here", name: "Here", at: now, ...letter }));
  write("ltr_waits", { to: "ses_target", text: "é".repeat(40_000), wait: true });
  write("ltr_nobody", { to: "ses_unknown", text: "Hello", wait: true });
  // One left for the Mac (in the outbox itself) isn't this link's.
  fs.writeFileSync(path.join(run, "outbox", "msg_mac.json"), JSON.stringify({ from: "ses_here", to: "ses_target", text: "Hi" }));
  const sent = [];
  const host = { send: bytes => sent.push(bytes) };

  link.sendOutbox(host, link.readLink(), now);
  assert.equal(sent.length, 1);
  const { address, payload } = opened(sent[0], second);
  assert.equal(address, second.address);
  assert.equal(payload.letter.id, "ltr_waits");
  assert.equal(payload.letter.wait, true);
  assert.ok(payload.letter.text.length > 0 && Buffer.byteLength(payload.letter.text) <= 24_000 && "é".repeat(40_000).startsWith(payload.letter.text), "its words cut to fit, whole characters");
  assert.ok(sent[0].length <= 65_600 + 17);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(run, "replies", "ltr_nobody.json"), "utf8")), { failed: "Its agent couldn't be reached." });
  assert.ok(fs.existsSync(path.join(run, "outbox", "msg_mac.json")));

  link.sendOutbox(host, link.readLink(), now + 1000);
  assert.equal(sent.length, 1, "not again within the minute");
  link.sendOutbox(host, link.readLink(), now + 61_000);
  assert.equal(sent.length, 2, "again after a minute");

  // How it went: only from the box it went to.
  link.letterWent(stranger.peer, { id: "ltr_waits", ok: true });
  link.letterWent(mac.peer, { id: "ltr_waits", ok: false, why: "No." });
  assert.ok(fs.existsSync(path.join(sending, "ltr_waits.json")));
  link.letterWent(second.peer, { id: "ltr_waits", ok: true });
  assert.equal(fs.existsSync(path.join(sending, "ltr_waits.json")), false);
  assert.ok(fs.existsSync(path.join(run, "replies", "ltr_waits.wait")), "its answer awaited");
  link.sendOutbox(host, link.readLink(), now + 200_000);
  assert.equal(sent.length, 2, "there: not sent again");

  link.letterAnswered(stranger.peer, { id: "ltr_waits", text: "Forged." });
  assert.equal(fs.existsSync(path.join(run, "replies", "ltr_waits.json")), false);
  link.letterAnswered(second.peer, { id: "ltr_waits", text: "Done." });
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(run, "replies", "ltr_waits.json"), "utf8")), { text: "Done." });
  assert.equal(fs.existsSync(path.join(run, "replies", "ltr_waits.wait")), false);

  // One not there in two days goes, its tool told.
  write("ltr_old", { to: "ses_target", text: "Hi", wait: true, at: now - 3 * 86_400_000 });
  link.sendOutbox(host, link.readLink(), now);
  assert.equal(fs.existsSync(path.join(sending, "ltr_old.json")), false);
  assert.equal(sent.length, 2);
});

test("a box tells its peers its agents as they change, and every ten minutes", async t => {
  fresh({ peersFrom: { [mac.pub]: { at: 1, peers: [mac.peer, second.peer] } } });
  const { target } = await engine(t);
  const sent = [];
  const host = { send: bytes => sent.push(bytes) };
  const readEngine = () => target;
  const now = Date.now();
  await link.announce(host, readEngine, link.readLink(), now);
  const to = bytes => [mac, second].find(each => k(bytes.subarray(1, 17)) === each.address);
  assert.deepEqual(sent.map(bytes => opened(bytes, to(bytes)).address).sort(), [mac.address, second.address].sort());
  assert.deepEqual(opened(sent[0], to(sent[0])).payload.agents.list, []);
  await link.announce(host, readEngine, link.readLink(), now + 60_000);
  assert.equal(sent.length, 2, "the same, within ten minutes: not again");
  await link.announce(host, readEngine, link.readLink(), now + 601_000);
  assert.equal(sent.length, 4);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(run, "agents.json"), "utf8")), { machine: "lokum", agents: [] });
});

test("a machine a Mac removed leaves its peers, and forgets what they told it", () => {
  fresh({ peersFrom: { [mac.pub]: { at: 1, peers: [mac.peer, second.peer] } } });
  fs.writeFileSync(path.join(run, "peer-agents.json"), "{}");
  fs.writeFileSync(path.join(run, "agents.json"), "{}");
  link.leave();
  assert.deepEqual(link.peersOf(link.readLink()), []);
  assert.equal(link.readLink().peersFrom, undefined);
  assert.equal(fs.existsSync(path.join(run, "peer-agents.json")), false);
  assert.equal(fs.existsSync(path.join(run, "agents.json")), false);
});
