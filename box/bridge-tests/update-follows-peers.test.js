// One fleet, one version (PROTOCOL.md, "Updates", Your computers follow each other): a box that's
// behind takes the newer signed release another of your computers holds, as that one's mail says it
// holds it, part by part over peer mail, checks it as any release and offers it to itself; never one
// that's unsigned, signed by another key, older, or not what its signature says. Update Now is
// offered only where a computer can't take an update by itself, and this Mac's app is told when
// Kuvo here is newer than it. Here, in a folder of the test's own, with another computer stood in
// for (`peer-release.js`) and releases signed with a key of the test's; the taking itself, while
// work runs, is `update-stops-nothing.test.js`'s.
// Run with `node --test box/bridge-tests/update-follows-peers.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";
import { PART, peerHolding } from "./peer-release.js";

const testHome = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-follows-peers-"));
const home = path.join(testHome, ".kuvo-code");
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-follows-peers-releases-"));
removeAtEnd(testHome);
removeAtEnd(scratch);
process.env.KUVO_HOME = home;
const link = await import("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const RUN = path.join(home, "run");

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
}
const sha256 = data => crypto.createHash("sha256").update(data).digest("hex");

const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
const pinned = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
const other = crypto.generateKeyPairSync("ed25519").privateKey;

// A release of `version`, signed with `key`, in a folder as a box keeps one: `boxes.json` and its
// pack (`size` bytes, or `pack` itself).
function release(version, { key = privateKey, size = 10 * PART + 123, pack = null, spreads = true } = {}) {
  const data = pack ?? crypto.randomBytes(size);
  const name = `box-${version}.tar.gz`;
  const manifest = Buffer.from(JSON.stringify({ kuvo: "box/1", version, engine: "1.0.0", clientProtocol: 1, released: new Date().toISOString(), pack: { url: `https://example.invalid/v${version}/${name}`, sha256: sha256(data), size: data.length }, ...(spreads ? { spreads: true } : {}) }));
  const signature = crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), key);
  const folder = path.join(scratch, `${version}-${crypto.randomUUID()}`);
  write(path.join(folder, "boxes.json"), JSON.stringify({ manifest: manifest.toString("base64"), signature: signature.toString("base64"), key: crypto.createPublicKey(key).export({ type: "spki", format: "der" }).subarray(-32).toString("base64") }));
  write(path.join(folder, name), data);
  return folder;
}

// This box ("devbox", a machine on 0.9.0) and your other computers: the mini (a Mac) and lab.
const k = key => key.toString("base64url");
const computer = (kind, name, id) => {
  const sign = crypto.randomBytes(32).toString("base64url");
  const key = link.newKey();
  const address = k(link.addressOf(crypto.randomBytes(32)));
  return { key, pub: k(link.publicOf(key)), sender: { key: k(link.publicOf(key)), kind, ...(id ? { id } : {}) }, peer: { key: k(link.publicOf(key)), sign, address, name, kind, ...(id ? { id } : {}) } };
};
const here = computer("machine", "devbox", "devbox"), mini = computer("mac", "mini"), lab = computer("machine", "lab", "lab");
// Keys that are absent from the peer list.
const stranger = computer("machine", "stranger", "stranger");
// (Lab's key the lower of the two others', so ties go to it.)
const [first, second] = [mini, lab].sort((a, b) => (a.pub < b.pub ? -1 : 1));

let clock = 1_800_000_000_000;
let offers = 0;
function box(version = "0.9.0") {
  fs.rmSync(home, { recursive: true, force: true });
  fs.mkdirSync(RUN, { recursive: true });
  write(path.join(RUN, "update.json"), "{}");
  write(path.join(home, "bin", "box-version"), version);
  link.writeLink({
    secret: crypto.randomBytes(32).toString("base64url"), key: k(here.key), relay: "wss://relay.test", name: "devbox", id: "devbox", mac: mini.pub, macs: [mini.pub], phones: [],
    legacy: { [mini.pub]: { at: 1, peers: [mini.peer, lab.peer, here.peer] } },
  });
  link.forgetFleet();
  offers = 0;
}
// A release of `version` kept as the one this box holds (as an update or an offer keeps it).
function hold(version) {
  const folder = release(version);
  const name = fs.readdirSync(folder).find(each => each !== "boxes.json");
  link.keepRelease(fs.readFileSync(path.join(folder, "boxes.json")), fs.readFileSync(path.join(folder, name)));
}
// What a computer's agents mail says of it: the Kuvo it runs and the release it holds.
const says = (from, kuvo, held = kuvo) => link.peerMail(from.sender, { agents: { at: clock++, list: [], computer: { version: "1.0.0", kuvo, ...(held ? { release: held } : {}) } } }, null);
// Each computer's mail to the one it's for, taken by this box as it comes from that one.
function follow(peers) {
  link.fleetWith({ keys: [pinned], blocked: null, clock: () => clock, send: (key, mail) => peers[key].send(key, mail), offered: () => { offers++; } });
}
const exchange = (from, holding) => holding.drain(reply => link.takeReleaseMail(from.sender, reply));
const offered = () => link.offeredVersion();

test("the computer list reports the installed Kuvo version beside the Mac app version", async () => {
  box("0.8.0-beta.33");
  says(mini, "0.8.0-beta.32", null);
  const computers = await link.listComputers("devbox", null);
  assert.equal(computers.find(each => each.here)?.kuvo, "0.8.0-beta.33");
  const mac = computers.find(each => each.key === mini.pub);
  assert.equal(mac?.version, "1.0.0");
  assert.equal(mac?.kuvo, "0.8.0-beta.32");
});

test("a newer signed release another of your computers holds is taken from it as soon as its mail says so: pulled part by part, checked, and offered here in one exchange", () => {
  box();
  const holding = peerHolding(release("0.10.0"));
  follow({ [mini.pub]: holding });
  says(mini, "0.10.0");
  assert.deepEqual(holding.asked, [{ want: "0.10.0" }], "asked at once, as its mail came");
  exchange(mini, holding);
  assert.equal(offers, 1, "offered here, to be taken at once");
  assert.equal(offered(), "0.10.0");
  assert.equal(holding.asked.filter(mail => Number.isInteger(mail.part)).length, 11, "each part asked once");
  assert.equal(link.pullUnderWay(), null);
  assert.ok(!fs.existsSync(path.join(home, "updates", "incoming")), "nothing left of the pull");
  // Its mail again (every ten minutes): nothing asked again, it waits to be taken here.
  says(mini, "0.10.0");
  assert.equal(holding.asked.filter(mail => mail.want).length, 1);
});

test("a release from another computer is never taken signed by another key, older, the version here, or not what its signature says", () => {
  // Signed by a key this box doesn't trust: refused before any part is asked, and not asked again.
  box();
  let holding = peerHolding(release("0.10.0", { key: other }));
  follow({ [mini.pub]: holding });
  says(mini, "0.10.0");
  exchange(mini, holding);
  assert.deepEqual(holding.asked, [{ want: "0.10.0" }], "no part asked");
  assert.equal(offered(), null);
  says(mini, "0.10.0");
  link.followPeers();
  assert.equal(holding.asked.length, 1, "that computer isn't asked for it again for a while");

  // Older than this box, or the version it has (one it keeps a copy of, to repair from): nothing
  // asked at all.
  for (const version of ["0.8.0", "0.9.0", "0.9.0-beta.4"]) {
    box();
    hold("0.9.0");
    holding = peerHolding(release(version));
    follow({ [mini.pub]: holding });
    says(mini, version);
    assert.deepEqual(holding.asked, [], version);
  }

  // Signed for another version than it says it holds: refused.
  box();
  holding = peerHolding(release("0.10.0"), { change: { boxes: fs.readFileSync(path.join(release("0.9.0"), "boxes.json"), "utf8") } });
  follow({ [mini.pub]: holding });
  says(mini, "0.10.0");
  exchange(mini, holding);
  assert.equal(holding.asked.filter(mail => Number.isInteger(mail.part)).length, 0);
  assert.equal(offered(), null);

  // A pack that isn't what the signed release says: pulled, checked, refused, and nothing kept.
  box();
  holding = peerHolding(release("0.10.0"), { change: { pack: data => { const copy = Buffer.from(data); copy[PART + 7] ^= 1; return copy; } } });
  follow({ [mini.pub]: holding });
  says(mini, "0.10.0");
  exchange(mini, holding);
  assert.equal(offers, 0);
  assert.equal(offered(), null);
  assert.ok(!fs.existsSync(path.join(home, "updates", "incoming")));
});

test("a computer that keeps no copy of the version it runs takes one from another of yours that holds it, to repair from: never offered as an update, and handed on", () => {
  box();
  const holding = peerHolding(release("0.9.0"));
  // (Whatever blocks updates here, an older Mac say, doesn't block this.)
  link.fleetWith({ keys: [pinned], blocked: "An older Mac connects here.", clock: () => clock, send: (key, mail) => holding.send(key, mail), offered: () => { offers++; } });
  says(mini, "0.9.0");
  assert.deepEqual(holding.asked, [{ want: "0.9.0" }], "asked at once, as its mail came");
  exchange(mini, holding);
  assert.equal(offers, 0, "never offered: it's the version here");
  assert.equal(offered(), null);
  assert.equal(link.pullUnderWay(), null);
  assert.ok(fs.existsSync(path.join(home, "updates", "running", "boxes.json")), "kept apart, to repair from");
  assert.ok(!fs.existsSync(path.join(home, "updates", "incoming")), "nothing left of the pull");
  // Kept: not asked for again.
  says(mini, "0.9.0");
  assert.equal(holding.asked.filter(mail => mail.want).length, 1);
  // And handed to another computer of yours that asks for it.
  const told = [];
  link.fleetWith({ keys: [pinned], blocked: null, clock: () => clock, send: (_key, mail) => { told.push(mail); return true; }, offered: () => {} });
  link.takeReleaseMail(lab.sender, { want: "0.9.0" });
  assert.equal(told[0]?.version, "0.9.0");
  assert.equal(typeof told[0]?.boxes, "string");
});

test("a computer that was away takes the newer release once it hears from the others again; an exchange cut off goes on at the next minute's look", () => {
  box();
  // (With a copy of the version it runs: nothing to pull for that.)
  hold("0.9.0");
  const holding = peerHolding(release("0.10.0"));
  follow({ [mini.pub]: holding });
  // As it was before it went away: the mini as new as it.
  says(mini, "0.9.0");
  assert.deepEqual(holding.asked, []);
  // Back: the mini's mail says it holds 0.10.0 now. What it sends is lost on the way.
  says(mini, "0.10.0");
  assert.deepEqual(holding.asked, [{ want: "0.10.0" }]);
  holding.lose();
  link.followPeers();
  assert.equal(holding.asked.length, 1, "not asked again at once");
  clock += 60_000;
  link.followPeers();
  assert.deepEqual(holding.asked.at(-1), { want: "0.10.0" }, "asked again a minute on");
  // Its manifest comes; then the first parts are lost, and asked again a minute on.
  holding.drain(reply => link.takeReleaseMail(mini.sender, reply) ?? holding.lose());
  assert.ok(link.pullUnderWay()?.parts, "under way");
  clock += 60_000;
  link.followPeers();
  exchange(mini, holding);
  assert.equal(offers, 1);
  assert.equal(offered(), "0.10.0");
});

test("release mail ignores legacy computer announcements and requests from unknown keys", () => {
  box();
  const holding = peerHolding(release("0.10.0"));
  const sent = [];
  follow({ [stranger.pub]: holding });
  link.fleetWith({ keys: [pinned], blocked: null, clock: () => clock, send: (key, mail) => { sent.push([key, mail]); return holding.send(key, mail); }, offered: () => { offers++; } });
  link.peerMail(lab.sender, { agents: { at: clock++, list: [], boxes: [stranger.peer], computer: { kuvo: "0.9.0" } } }, null);
  link.takeReleaseMail(stranger.sender, { holds: "0.10.0" });
  assert.deepEqual(sent, [], "not followed");
  hold("0.9.0");
  link.takeReleaseMail(stranger.sender, { want: "0.9.0" });
  assert.deepEqual(sent, [], "not answered, nor told");
});

test("a build signed for trying an update is never handed to your other computers", () => {
  box();
  follow({});
  assert.equal(link.offerUpdate(release("0.10.0", { spreads: false }), { keys: [pinned], blocked: null }).status, 202);
  assert.deepEqual(link.fleetFacts(), { kuvo: "0.9.0" }, "taken here, but held for no one");
  const sent = [];
  link.fleetWith({ keys: [pinned], blocked: null, send: (key, mail) => { sent.push(mail); return true; } });
  link.takeReleaseMail(mini.sender, { want: "0.10.0" });
  assert.deepEqual(sent, [{ version: "0.10.0", none: true }]);
});

test("a release check under way here holds a pull, kept, and the pull goes on after; a holder hands its release over while it checks", () => {
  box();
  const holding = peerHolding(release("0.10.0"));
  follow({ [mini.pub]: holding });
  says(mini, "0.10.0");
  // The release check takes the update lock (as `update` does while it only looks; just taken,
  // its process not written yet).
  fs.mkdirSync(path.join(RUN, ".updating"), { recursive: true });
  exchange(mini, holding);
  assert.equal(offers, 0, "not offered while the check runs");
  assert.ok(link.pullUnderWay(), "the pull kept");
  const parts = holding.asked.filter(mail => Number.isInteger(mail.part)).length;
  // A box asked for its release meanwhile hands it over (here, the 0.9.0 it holds).
  hold("0.9.0");
  const sent = [];
  link.fleetWith({ keys: [pinned], blocked: null, clock: () => clock, send: (key, mail) => { sent.push(mail); return holding.send(key, mail); }, offered: () => { offers++; } });
  link.takeReleaseMail(lab.sender, { want: "0.9.0" });
  assert.equal(sent.at(-1).version, "0.9.0");
  assert.ok(sent.at(-1).boxes, "handed over while the check runs");
  // The check over: offered at the next look, nothing pulled again.
  fs.rmSync(path.join(RUN, ".updating"), { recursive: true, force: true });
  link.followPeers();
  assert.equal(offers, 1);
  assert.equal(holding.asked.filter(mail => Number.isInteger(mail.part)).length, parts, "nothing pulled again");
});

test("a version that failed here twice isn't followed again, but an older one still newer than this box is", () => {
  box();
  const newer = peerHolding(release("0.11.0")), older = peerHolding(release("0.10.0"));
  write(path.join(RUN, "update.json"), JSON.stringify({ offerFailed: { version: "0.11.0", at: 1, takenBack: true, tries: 2 } }));
  follow({ [mini.pub]: newer, [lab.pub]: older });
  says(mini, "0.11.0");
  says(lab, "0.10.0");
  assert.deepEqual(newer.asked, [], "0.11.0 failed here twice: not taken by itself");
  assert.deepEqual(older.asked, [{ want: "0.10.0" }]);
});

test("the newest release any of your computers holds is the one taken, the same on each: the newest version, then the lowest key", () => {
  box();
  const older = peerHolding(release("0.10.0")), newer = peerHolding(release("0.11.0"));
  follow({ [lab.pub]: older, [mini.pub]: newer });
  says(lab, "0.10.0");
  assert.deepEqual(older.asked, [{ want: "0.10.0" }]);
  // The mini holds a newer one: that one's taken instead.
  says(mini, "0.11.0");
  assert.equal(link.newestHeld(clock).version, "0.11.0");
  exchange(mini, newer);
  assert.equal(offered(), "0.11.0");

  box();
  const a = peerHolding(release("0.11.0")), b = peerHolding(release("0.11.0"));
  follow({ [first.pub]: a, [second.pub]: b });
  says(second, "0.11.0");
  link.forgetFleet();
  says(first, "0.11.0");
  assert.equal(link.newestHeld(clock).peer, first.pub, "the lowest key");
});

test("a box holds the signed release it runs, and hands it over only once it's judged well there; Kuvo Code on a Mac offering the one it runs leaves it held", () => {
  box();
  // As Kuvo Code here offers the release it carries, of the version here with the same files.
  const files = path.join(scratch, "files");
  write(path.join(files, "bin", "box-version"), "0.9.0");
  write(path.join(files, "box.json"), JSON.stringify({ version: "0.9.0", engine: "1.0.0", files: { "bin/box-version": sha256(Buffer.from("0.9.0")) } }));
  const packed = path.join(scratch, "pack.tgz");
  assert.equal(spawnSync("tar", ["czf", packed, "-C", files, "."], { env: { ...process.env, COPYFILE_DISABLE: "1" } }).status, 0);
  const carried = release("0.9.0", { pack: fs.readFileSync(packed) });
  follow({});
  assert.equal(link.heldRelease([pinned]), null, "none held yet");
  assert.equal(link.offerUpdate(carried, { keys: [pinned], blocked: null }).status, 409, "the same release: nothing to take");
  assert.equal(link.heldRelease([pinned])?.version, "0.9.0", "held, to hand over");
  assert.deepEqual(link.fleetFacts(), { kuvo: "0.9.0", release: "0.9.0" }, "as its mail tells the others");

  // Asked for it, part by part, by another computer that's behind.
  const sent = [];
  link.fleetWith({ keys: [pinned], blocked: null, send: (key, mail) => { sent.push([key, mail]); return true; } });
  link.takeReleaseMail(lab.sender, { want: "0.9.0" });
  assert.equal(sent[0][0], lab.pub);
  assert.equal(sent[0][1].size, fs.statSync(packed).size);
  link.takeReleaseMail(lab.sender, { version: "0.9.0", part: 0 });
  assert.deepEqual(Buffer.from(sent[1][1].d, "base64"), fs.readFileSync(packed).subarray(0, PART));
  link.takeReleaseMail(lab.sender, { want: "0.8.0" });
  assert.deepEqual(sent[2][1], { version: "0.8.0", none: true }, "another version: it holds none");

  // An update on trial here: handed over all the same, each computer judging it for itself.
  write(path.join(RUN, "update-trial"), "0.9.0");
  assert.deepEqual(link.fleetFacts(), { kuvo: "0.9.0", release: "0.9.0" });
  link.takeReleaseMail(lab.sender, { want: "0.9.0" });
  assert.equal(sent.at(-1)[1].version, "0.9.0");
  assert.equal(typeof sent.at(-1)[1].boxes, "string");
});

test("a newer release Kuvo Code here hands over is told to your other computers at once, before it's taken here, and stays held whatever comes of it here", async () => {
  box();
  follow({});
  // The link watches what's held: the moment Kuvo Code here hands a release over, it's told.
  let watch;
  const told = new Promise(resolve => { watch = link.watchHeld(resolve); });
  assert.equal(link.offerUpdate(release("0.10.0"), { keys: [pinned], blocked: null }).status, 202);
  await told;
  watch.close();
  assert.deepEqual(link.fleetFacts(), { kuvo: "0.9.0", release: "0.10.0" }, "still on 0.9.0 itself, holding 0.10.0");

  // The mini, behind, starts pulling it from this box while this box hasn't even taken it.
  const sent = [];
  link.fleetWith({ keys: [pinned], blocked: null, send: (key, mail) => { sent.push(mail); return true; } });
  link.takeReleaseMail(mini.sender, { want: "0.10.0" });
  assert.equal(sent[0].version, "0.10.0");
  assert.ok(sent[0].size > 0, "handed over before it's taken here");
  // Taken here and put back by its judge: still held, as the others judge it for themselves.
  write(path.join(RUN, "update.json"), JSON.stringify({ trial: { from: "0.9.0", to: "0.10.0", at: Date.now() } }));
  link.noteTrialEnded();
  assert.equal(link.fleetFacts().release, "0.10.0");
  // Never an older one over it.
  link.keepRelease(fs.readFileSync(path.join(release("0.9.0"), "boxes.json")), crypto.randomBytes(10));
  assert.equal(link.fleetFacts().release, "0.10.0");
});

test("Update Now is offered only where a computer can't take an update by itself: never while a release waits or is tried again, and for another computer that follows, never", () => {
  box();
  // (Its engine the release's: one it takes stops nothing.)
  write(path.join(RUN, "engine.json"), JSON.stringify({ version: "1.0.0" }));
  const folder = release("0.10.0");
  assert.equal(link.offerUpdate(folder, { keys: [pinned], blocked: null }).status, 202);
  const self = () => link.keepingSelfUp("devbox", 0, null);
  assert.deepEqual(self(), { update: { state: "waiting", title: "Kuvo on devbox updates in a moment.", detail: "" }, actions: [] });
  // Failed once: it tries again by itself.
  const failed = tries => write(path.join(RUN, "update.json"), JSON.stringify({ offerFailed: { version: "0.10.0", at: Date.now(), takenBack: true, tries } }));
  failed(1);
  assert.deepEqual(self(), { update: { state: "waiting", title: "Kuvo on devbox tries its update again in a while.", detail: "" }, actions: [] });
  // Twice: what it can't get past by itself.
  failed(2);
  assert.deepEqual(self().actions.map(action => [action.id, action.label]), [["update-here", "Update Now"]]);
  assert.equal(self().update.state, "failed");

  // Another computer, behind, that follows the others: it updates in a moment, nothing to press.
  for (const [kind, name] of [["mac", "mini"], ["machine", "lab"]]) {
    const key = kind === "mac" ? mini.pub : lab.pub;
    const row = { key, name, kind, reach: "connected", version: "1.0.0" };
    write(path.join(home, "bin", "box-version"), "0.10.0");
    assert.deepEqual(link.keepingUp({ ...row, kuvo: "0.9.0" }, "devbox"), { update: { state: "waiting", title: `Kuvo on ${name} updates in a moment`, detail: "" }, actions: [] }, kind);
    assert.deepEqual(link.keepingUp({ ...row, kuvo: "0.10.0" }, "devbox"), { actions: [] }, `${kind}: as new`);
    assert.deepEqual(link.keepingUp({ ...row, kuvo: "0.9.0", reach: "away" }, "devbox"), { actions: [] }, `${kind}: away, it takes it as it's back`);
  }
});

test("this Mac's app is told to look for its own update once Kuvo here runs a newer version than it", () => {
  box();
  link.registerDesktop("/Applications/Kuvo.app", Date.now(), "0.9.0");
  assert.equal(link.appBehind("0.9.0"), null, "the same");
  assert.equal(link.appBehind("0.10.0"), "0.10.0", "Kuvo here newer: the app looks");
  link.registerDesktop("/Applications/Kuvo.app", Date.now(), "0.10.0");
  assert.equal(link.appBehind("0.10.0"), null, "the app updated");
});

test("a box looks for a newer release itself once in each six hours, at any hour of the day", () => {
  const at = hour => Date.UTC(2026, 10, 17, hour, 5);
  const rounds = [0, 3, 6, 9, 12, 15, 18, 21].map(hour => link.releaseRound(at(hour), 0));
  assert.deepEqual(rounds.filter((round, i) => round !== rounds[i - 1]).length, 4, "one in each six hours, none waiting for night");
  assert.notEqual(link.releaseRound(at(14), 0), link.releaseRound(at(20), 0));
  // (Each box at its own point in its six hours, so not all of them ask at once.)
  assert.equal(typeof link.releaseRound(at(1)), "string");
});

function setup(mod, computer, folder, selected, { layout = 18 } = {}) {
  const id = "set_" + "a".repeat(22);
  const pack = mod.verifyBoxes(selected, [pinned]);
  write(path.join(folder, "bin", "box-version"), computer === mini ? pack.version : "0.8.0-beta.35");
  mod.writeLink({
    key: k(computer.key), secret: crypto.randomBytes(32).toString("base64url"), relay: "wss://relay.test", phones: [],
    setup: { id, epoch: 1, primary: mini.pub, sign: mini.peer.sign, role: computer === mini ? "primary" : "member",
      checkpointed: true, managed: true, standby: { key: lab.pub, sign: lab.peer.sign } },
    sharedSetup: id,
    shared: {
      membershipAndAccess: { sequence: 1, value: { boxes: [mini, lab, here].map(box => box.peer) } },
      setupSecurityAndReleaseChoice: { sequence: 1, value: { version: pack.version, sha256: pack.pack.sha256,
        signed: Buffer.from(selected).toString("base64"), clientProtocol: 1, layout } },
    },
  });
}

function installableRelease(version) {
  const files = path.join(scratch, `files-${crypto.randomUUID()}`);
  const contents = {
    "bin/box-version": version,
    "bin/kuvo-link.js": "// Kuvo folder layout 18 123456789abc\n",
    "bin/kuvo-start": "#!/bin/sh\nexit 0\n",
  };
  for (const [name, value] of Object.entries(contents)) write(path.join(files, name), value, 0o700);
  write(path.join(files, "box.json"), JSON.stringify({ version, engine: "1.0.0",
    files: Object.fromEntries(Object.entries(contents).map(([name, value]) => [name, sha256(value)])) }));
  const tar = path.join(files, "../pack-" + crypto.randomUUID());
  assert.equal(spawnSync("tar", ["czf", tar, "-C", files, "."], { env: { ...process.env, COPYFILE_DISABLE: "1" } }).status, 0);
  return release(version, { pack: fs.readFileSync(tar) });
}

function engineFixture(folder) {
  write(path.join(folder, "run", "engine.json"), JSON.stringify({ version: "1.0.0" }));
  write(path.join(folder, "engine", "1.0.0", "opencode"), "#!/bin/sh\necho ok\n", 0o700);
  write(path.join(folder, "folder-version.json"), '{"version":18}\n');
}

test("three computers install the main computer’s signed pack, including from the standby", async () => {
  const folder = installableRelease("0.10.0");
  const boxes = fs.readFileSync(path.join(folder, "boxes.json"), "utf8");
  const pack = fs.readFileSync(path.join(folder, "box-0.10.0.tar.gz"));
  const saved = process.env.KUVO_HOME;
  const rootParent = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-j-fleet-nodes-"));
  removeAtEnd(rootParent);
  const nodes = [];
  try {
    for (const computer of [mini, lab, here]) {
      const root = path.join(rootParent, computer.peer.name);
      fs.mkdirSync(root);
      process.env.KUVO_HOME = root;
      const mod = await import(`../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js?fleet-node=${computer.pub}`);
      setup(mod, computer, root, boxes);
      engineFixture(root);
      nodes.push({ computer, root, mod });
    }
  } finally { process.env.KUVO_HOME = saved; }
  const [primary, standby, member] = nodes;
  write(path.join(primary.root, "bin", "box-version"), "0.8.0-beta.35");
  const layouts = computer => Object.fromEntries(nodes.filter(node => node.computer !== computer)
    .map(node => [node.computer.pub, { computer: { layout: node.computer === lab ? 17 : 18 } }]));
  write(path.join(primary.root, "run", "peer-agents.json"), JSON.stringify(layouts(mini)));
  const options = { keys: [pinned], blocked: null, now: true, whileBusy: false };
  const blocked = await primary.mod.update({ ...options, from: folder });
  assert.equal(blocked.result, "blocked");
  assert.match(blocked.message, /Waiting for lab.*layout 18/);
  assert.equal(primary.mod.boxVersion(), "0.8.0-beta.35");
  assert.equal(fs.readFileSync(path.join(primary.root, "folder-version.json"), "utf8"), '{"version":18}\n');
  for (const node of nodes) {
    const facts = layouts(node.computer);
    for (const fact of Object.values(facts)) fact.computer.layout = 18;
    write(path.join(node.root, "run", "peer-agents.json"), JSON.stringify(facts));
  }
  assert.equal((await primary.mod.update({ ...options, from: folder })).result, "updated");
  assert.equal(primary.mod.boxVersion(), "0.10.0");
  const mail = [];
  for (const source of nodes) source.mod.fleetWith({
    keys: [pinned], blocked: null, clock: () => clock,
    send: (key, value) => {
      if (source === member && key === mini.pub) return false;
      mail.push({ from: source.computer.sender, to: nodes.find(node => node.computer.pub === key), value });
      return true;
    },
    offered: () => {},
  });
  const deliver = () => { while (mail.length) { const { from, to, value } = mail.shift(); to.mod.takeReleaseMail(from, value); } };
  assert.equal(primary.mod.fleetFacts().release, "0.10.0");
  standby.mod.peerMail(mini.sender, { agents: { at: clock++, list: [], computer: { ...primary.mod.fleetFacts(), layout: 18 } } }, null);
  deliver();
  assert.equal(standby.mod.offeredVersion(), "0.10.0");
  member.mod.peerMail(lab.sender, { agents: { at: clock++, list: [], computer: { ...standby.mod.fleetFacts(), layout: 18 } } }, null);
  deliver();
  assert.equal(member.mod.offeredVersion(), "0.10.0");
  for (const node of [standby, member]) {
    assert.deepEqual(fs.readFileSync(path.join(node.root, "updates", "offered", "box-0.10.0.tar.gz")), pack);
    assert.equal(node.mod.transferAttempt().step, "offer accepted");
    assert.equal((await node.mod.takeOffer(options)).result, "updated");
    assert.equal(node.mod.boxVersion(), "0.10.0");
  }
});

test("an interrupted manifest, pack and offer each leave a reason and retry", () => {
  for (const boundary of ["manifest", "pack", "offer"]) {
    box("0.8.0-beta.35");
    const folder = release("0.10.0");
    setup(link, here, home, fs.readFileSync(path.join(folder, "boxes.json"), "utf8"));
    const holding = peerHolding(folder);
    let connected = boundary !== "manifest";
    link.fleetWith({ keys: [pinned], blocked: null, clock: () => clock,
      send: (key, value) => key === mini.pub && connected && holding.send(key, value),
      offered: () => { offers++; } });
    says(mini, "0.8.0-beta.35", "0.10.0");
    if (boundary === "pack") {
      holding.drain(reply => { link.takeReleaseMail(mini.sender, reply); holding.lose(); });
      clock += 5 * 60_000;
      link.followPeers();
    } else if (boundary === "offer") {
      // The incoming files remain writable, but the offer cannot create its sibling folder.
      const updates = path.join(home, "updates");
      holding.drain(reply => {
        if (reply.i !== 10) return link.takeReleaseMail(mini.sender, reply);
        fs.chmodSync(updates, 0o500);
        try { link.takeReleaseMail(mini.sender, reply); } finally { fs.chmodSync(updates, 0o700); }
      });
    }
    const failed = link.localFleetUpdate("0.10.0", 0, clock);
    assert.equal(failed.state, "failed", boundary);
    assert.match(failed.reason, /relay|stopped sending|offer/i, boundary);
    assert.ok(failed.nextRetryAt > clock);
    connected = true;
    clock = failed.nextRetryAt + 1;
    link.followPeers();
    exchange(mini, holding);
    assert.equal(link.offeredVersion(), "0.10.0", boundary);
  }
});

test("a signed pack that fails in staging records an install failure and its retry time", async () => {
  box("0.8.0-beta.35");
  const folder = release("0.10.0");
  setup(link, here, home, fs.readFileSync(path.join(folder, "boxes.json"), "utf8"));
  engineFixture(home);
  const holding = peerHolding(folder);
  follow({ [mini.pub]: holding });
  says(mini, "0.8.0-beta.35", "0.10.0");
  exchange(mini, holding);
  assert.equal(link.transferAttempt().step, "offer accepted");
  const now = clock;
  assert.equal((await link.takeOffer({ now: true, whileBusy: false, keys: [pinned], blocked: null, clock: () => now })).result, "failed");
  const failed = link.localFleetUpdate("0.10.0", 0, now);
  assert.equal(failed.state, "failed");
  assert.match(failed.reason, /unpacked/);
  assert.equal(failed.rolledBackTo, null);
  assert.ok(failed.nextRetryAt > now);
  assert.match(fs.readFileSync(path.join(RUN, "fleet-update.log"), "utf8"), /"step":"staging"/);
});

test("the accepted target holds other work, old clients get a refusal, and layout compatibility comes from computers", () => {
  box("0.8.0-beta.35");
  const folder = release("0.10.0");
  setup(link, here, home, fs.readFileSync(path.join(folder, "boxes.json"), "utf8"));
  assert.equal(link.clientUpdateRequired("0.9.0", 1, true).reason, "client-update-required");
  assert.equal(link.clientUpdateRequired("0.10.0", null, true), null);
  assert.equal(link.clientUpdateRequired("1.0", null, false).target, "0.10.0");
  assert.equal(link.clientUpdateRequired("1.0", 1, false), null);
  assert.equal(link.computerUpdateRequired("kuvo/sessions/start").reason, "computer-update-required");
  assert.equal(link.computerUpdateRequired("kuvo/computers/events"), null);
  assert.equal(link.computerUpdateRequired("kuvo/sessions/ses_one/asks"), null);
  write(path.join(RUN, "peer-agents.json"), JSON.stringify({
    [mini.pub]: { computer: { layout: 19 } }, [lab.pub]: { computer: { layout: 18 } },
  }));
  assert.match(link.layoutHold(19), /lab.*layout 19/);
  assert.equal(link.layoutHold(18), null);
});

test("a fresh transfer refuses manual retry, then a stalled one can be scheduled", () => {
  box("0.8.0-beta.35");
  const folder = release("0.10.0");
  setup(link, here, home, fs.readFileSync(path.join(folder, "boxes.json"), "utf8"));
  const now = Date.now();
  const journal = path.join(RUN, "fleet-transfer.json");
  const state = { version: "0.10.0", peer: mini.pub, step: "receiving pack", at: now, failures: 0 };
  write(journal, JSON.stringify(state));
  const savedHome = process.env.HOME;
  process.env.HOME = testHome;
  try {
    assert.equal(link.requestFleetRetry("0.10.0").status, 409);
    write(journal, JSON.stringify({ ...state, at: now - 6 * 60_000 }));
    assert.equal(link.requestFleetRetry("0.10.0").status, 202);
    assert.equal(link.requestedRetry().version, "0.10.0");
    assert.equal(link.requestFleetRetry("0.10.0").status, 409);
  } finally { process.env.HOME = savedHome; }
});

test("header dismissal is per client and new failures reopen it through the subsequent retry", () => {
  box();
  const row = { key: lab.pub, fleetUpdate: { current: "0.9.0", target: "0.10.0", state: "behind", canRetry: false } };
  assert.equal(link.dismissFleetUpdate([row], "0.10.0", "device-a").dismissed, true);
  assert.equal(link.fleetSummary([row], "0.10.0", "device-b").dismissed, false);
  assert.equal(link.fleetSummary([row], "0.11.0", "device-a").dismissed, false);
  row.fleetUpdate = { ...row.fleetUpdate, state: "failed", lastFailureAt: 100 };
  assert.deepEqual(link.fleetSummary([row], "0.10.0", "device-a").stage, "failed");
  assert.equal(link.fleetSummary([row], "0.10.0", "device-a").dismissed, false);
  row.fleetUpdate = { ...row.fleetUpdate, state: "updating", stage: "downloading" };
  assert.equal(link.fleetSummary([row], "0.10.0", "device-a").dismissed, false);
  row.fleetUpdate = { ...row.fleetUpdate, current: "0.10.0", state: "up-to-date" };
  assert.equal(link.fleetSummary([row], "0.10.0", "device-a").stage, "finished");
});

test("Computers snapshots and the live channel expose the same dismissal and member counts", async () => {
  box("0.8.0-beta.35");
  const folder = release("0.10.0");
  setup(link, here, home, fs.readFileSync(path.join(folder, "boxes.json"), "utf8"));
  const handle = link.engineHandler(() => null, { name: "devbox" }, () => null);
  const ask = async (method, path, body, client = "a".repeat(32), version = "0.10.0") => {
    const answer = handle({ method, path, fromApp: true, served: true,
      headers: { "x-kuvo-client-version": version, "x-kuvo-client-id": client },
      ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}),
    }, undefined, AbortSignal.timeout(5_000));
    const head = (await answer.next()).value.head;
    const part = (await answer.next()).value;
    await answer.return();
    return { status: head.status, json: JSON.parse(part.body.toString()) };
  };
  const snapshot = await ask("GET", "kuvo/computers");
  assert.equal(snapshot.status, 200);
  assert.equal(snapshot.json.fleetUpdate.behind, 3);
  assert.equal(snapshot.json.computers.filter(row => row.fleetUpdate.state === "behind").length, 3);
  assert.equal((await ask("POST", "kuvo/update/dismiss", { target: "0.10.0" })).json.dismissed, true);
  assert.equal((await ask("GET", "kuvo/computers/events")).json.fleetUpdate.dismissed, true);
  assert.equal((await ask("GET", "kuvo/computers", null, "b".repeat(32))).json.fleetUpdate.dismissed, false);
  const old = await ask("GET", "kuvo/info", null, undefined, "0.9.0");
  assert.equal(old.status, 426);
  assert.equal(old.json.reason, "client-update-required");
});
