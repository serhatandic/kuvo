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

const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-follows-peers-"));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-follows-peers-releases-"));
removeAtEnd(home);
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
function release(version, { key = privateKey, size = 10 * PART + 123, pack = null } = {}) {
  const data = pack ?? crypto.randomBytes(size);
  const name = `box-${version}.tar.gz`;
  const manifest = Buffer.from(JSON.stringify({ kuvo: "box/1", version, engine: "1.0.0", released: new Date().toISOString(), pack: { url: `https://example.invalid/v${version}/${name}`, sha256: sha256(data), size: data.length } }));
  const signature = crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), key);
  const folder = path.join(scratch, `${version}-${crypto.randomUUID()}`);
  write(path.join(folder, "boxes.json"), JSON.stringify({ manifest: manifest.toString("base64"), signature: signature.toString("base64"), key: crypto.createPublicKey(key).export({ type: "spki", format: "der" }).subarray(-32).toString("base64") }));
  write(path.join(folder, name), data);
  return folder;
}

// This box ("devbox", a machine on 0.9.0) and your other computers: the mini (a Mac) and lab.
const k = key => key.toString("base64url");
const computer = (kind, name, id) => {
  const key = link.newKey();
  const address = k(link.addressOf(crypto.randomBytes(32)));
  return { key, pub: k(link.publicOf(key)), sender: { key: k(link.publicOf(key)), kind, ...(id ? { id } : {}) }, peer: { key: k(link.publicOf(key)), address, name, kind, ...(id ? { id } : {}) } };
};
const here = computer("machine", "devbox", "devbox"), mini = computer("mac", "mini"), lab = computer("machine", "lab", "lab");
// Another of your Macs that isn't this box's peer: known through lab, a machine both set up.
const studio = computer("mac", "studio");
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
// What a computer's agents mail says of it: the Kuvo it runs and the release it holds.
const says = (from, kuvo, held = kuvo) => link.peerMail(from.sender, { agents: { at: clock++, list: [], computer: { version: "1.0.0", kuvo, ...(held ? { release: held } : {}) } } }, null);
// Each computer's mail to the one it's for, taken by this box as it comes from that one.
function follow(peers) {
  link.fleetWith({ keys: [pinned], blocked: null, clock: () => clock, send: (key, mail) => peers[key].send(key, mail), offered: () => { offers++; } });
}
const exchange = (from, holding) => holding.drain(reply => link.takeReleaseMail(from.sender, reply));
const offered = () => link.offeredVersion();

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

  // Older than this box, or the version it has: nothing asked at all.
  for (const version of ["0.8.0", "0.9.0", "0.9.0-beta.4"]) {
    box();
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

test("a computer that was away takes the newer release once it hears from the others again; an exchange cut off goes on at the next minute's look", () => {
  box();
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

test("another of your Macs that isn't a peer (known through a machine both set up) hands over its release too, and is told what this one holds", () => {
  box();
  const holding = peerHolding(release("0.10.0"));
  follow({ [studio.pub]: holding });
  // Lab says it knows studio: a box it vouches for, which sends no agents mail here.
  link.peerMail(lab.sender, { agents: { at: clock++, list: [], boxes: [studio.peer], computer: { kuvo: "0.9.0" } } }, null);
  link.takeReleaseMail(studio.sender, { holds: "0.10.0" });
  assert.deepEqual(holding.asked, [{ want: "0.10.0" }], "asked at once, as studio said it holds it");
  exchange(studio, holding);
  assert.equal(offers, 1);
  assert.equal(offered(), "0.10.0");

  // The other way: what this box holds is told to studio, once, and again ten minutes on.
  box();
  const sent = [];
  link.fleetWith({ keys: [pinned], blocked: null, clock: () => clock, send: (key, mail) => { sent.push([key, mail]); return true; } });
  link.peerMail(lab.sender, { agents: { at: clock++, list: [], boxes: [studio.peer], computer: { kuvo: "0.9.0" } } }, null);
  link.keepRelease(fs.readFileSync(path.join(release("0.9.0"), "boxes.json")), crypto.randomBytes(100));
  link.tellHeld();
  link.tellHeld();
  assert.deepEqual(sent, [[studio.pub, { holds: "0.9.0" }]]);
  clock += 10 * 60_000;
  link.tellHeld();
  assert.equal(sent.length, 2);
});

test("a version put back after its trial counts as a failure each time: the second time, Update Now", () => {
  box();
  write(path.join(RUN, "engine.json"), JSON.stringify({ version: "1.0.0" }));
  assert.equal(link.offerUpdate(release("0.10.0"), { keys: [pinned], blocked: null }).status, 202);
  const putBack = () => {
    // As the keeper leaves it once its judge took the update back: the trial noted, its file gone.
    const state = JSON.parse(fs.readFileSync(path.join(RUN, "update.json"), "utf8").trim() || "{}");
    write(path.join(RUN, "update.json"), JSON.stringify({ ...state, trial: { from: "0.9.0", to: "0.10.0", at: Date.now() } }));
    link.noteTrialEnded();
  };
  putBack();
  assert.deepEqual(link.keepingSelfUp("devbox", 0, null).actions, [], "once: it tries again by itself");
  putBack();
  assert.deepEqual(link.keepingSelfUp("devbox", 0, null).actions.map(action => action.label), ["Update Now"], "twice: what it can't get past by itself");
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

  // An update on trial here: nothing handed over until it's judged well.
  write(path.join(RUN, "update-trial"), "0.9.0");
  assert.deepEqual(link.fleetFacts(), { kuvo: "0.9.0" });
  link.takeReleaseMail(lab.sender, { want: "0.9.0" });
  assert.deepEqual(sent.at(-1)[1], { version: "0.9.0", none: true });
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
  const rounds = [0, 3, 6, 9, 12, 15, 18, 21].map(hour => link.releaseRound(at(hour)));
  assert.deepEqual(rounds.filter((round, i) => round !== rounds[i - 1]).length, 4, "one in each six hours, none waiting for night");
  assert.notEqual(link.releaseRound(at(14)), link.releaseRound(at(20)));
});
