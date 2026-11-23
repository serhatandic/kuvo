// Computer version display and the automatic check schedule.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

const testHome = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-follows-peers-"));
const home = path.join(testHome, ".kuvo-code");
removeAtEnd(testHome);
process.env.KUVO_HOME = home;
const link = await import(LINK_BUNDLE.href);
const RUN = path.join(home, "run");

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
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
let clock = 1_800_000_000_000;
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
}
const says = (from, kuvo, held = kuvo) => link.peerMail(from.sender, { agents: { at: clock++, list: [], computer: { version: "1.0.0", kuvo, ...(held ? { release: held } : {}) } } }, null);
test("the computer list reports the installed Kuvo version beside the Mac app version", async () => {
  box("0.8.0-beta.33");
  says(mini, "0.8.0-beta.32", null);
  const computers = await link.listComputers("devbox", null);
  assert.equal(computers.find(each => each.here)?.kuvo, "0.8.0-beta.33");
  const mac = computers.find(each => each.key === mini.pub);
  assert.equal(mac?.version, "1.0.0");
  assert.equal(mac?.kuvo, "0.8.0-beta.32");
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
