import "./safety"
import { afterEach, expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signDoc, signingKeyOf } from "../src/link/documents/format";
import { RUN } from "../src/link/home";
import { takeLeadership } from "../src/link/setup/leadership";
import { relayLease } from "../src/link/setup/lease";
import { acceptedMembership } from "../src/link/setup/membership";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { leadAtTerm } from "../src/link/setup/takeover";
import { setupView } from "../src/link/setup/view";
import { readLink, writeLink, type LinkState } from "../src/link/state";
import { grantRelayLease, leaseFile } from "./relay-lease";

// Older setups, as their link.json has them, are converted by their main in one step and keep
// every member; nobody is ever named standby again.

function identity(name: string) {
  const secret = newKey();
  const sign = newSigningSeed();
  const key = publicOf(secret).toString("base64url");
  return { seed: secret.toString("base64url"), sign, key,
    peer: { key, sign: signingKeyOf(sign), address: addressOf(secret).toString("base64url"), name, kind: "machine" as const } };
}

const linkFile = path.join(RUN, "link.json");
afterEach(() => {
  fs.rmSync(linkFile, { force: true });
  fs.rmSync(leaseFile, { force: true });
});

const phone = { key: publicOf(newKey()).toString("base64url"), pairing: "pairing-one", name: "Phone" };

// A setup record as an older release wrote it, standby and ranks included.
type Legacy = SetupRecord & { standby?: { key: string; sign: string }; successorChain?: true };

function convert(main: LinkState) {
  writeLink(main);
  grantRelayLease(main);
  leadAtTerm(null, relayLease(main)!, "Converted the setup to the relay lease");
  return readLink()!;
}

/** The member's side: it takes the main's new certificate and keeps the whole setup. */
function follow(member: LinkState, main: LinkState) {
  const certificate = readDoc(main.leadership![main.setup!.epoch])!;
  expect(takeLeadership(member, certificate)).toBe(true);
  return member;
}

test("a layout 4 setup with a standby converts at its main in one step and keeps its members and devices", () => {
  const [a, b, c] = [identity("Studio"), identity("Desk"), identity("Lab")];
  const id = newSetupID();
  const legacy: Legacy = { id, epoch: 2, primary: a.key, sign: a.peer.sign, role: "primary", managed: true,
    standby: { key: b.key, sign: b.peer.sign }, numberCeiling: 500 };
  const old = signDoc(a.sign, "leadership", 2, { setup: { id, epoch: 2, primary: a.key }, previous: a.key,
    boxes: [a.peer, b.peer, c.peer], standby: b.key, numberCeiling: 500 });
  const converted = convert({
    key: a.seed, sign: a.sign, secret: a.seed, name: "Studio", setup: legacy, leadership: { 2: old },
    macPeers: [b.peer, c.peer], phones: [{ ...phone, from: "pair" }],
  });

  expect(converted.setup).toEqual({ id, epoch: 3, primary: a.key, sign: a.peer.sign, role: "primary", managed: true, checkpointed: true });
  const certificate = readDoc(converted.leadership![3])!;
  expect(certificate.body.relayTerm).toBe(2);
  expect(certificate.body.standby).toBeUndefined();
  const membership = acceptedMembership(converted)!;
  expect(membership.boxes.map(box => box.key)).toEqual([a.key, b.key, c.key]);
  expect(membership.devices.phones.map(each => each.key)).toEqual([phone.key]);

  const member = follow({ key: b.seed, sign: b.sign, setup: { ...legacy, role: "member" }, mac: a.key, macs: [a.key] }, converted);
  expect(member.setup).toEqual({ id, epoch: 3, primary: a.key, sign: a.peer.sign, role: "member", managed: true, checkpointed: true });
  expect(acceptedMembership(member)!.boxes.map(box => box.key)).toEqual([a.key, b.key, c.key]);
});

test("a checkpointed setup with ranks and a standby converts in one step and keeps its members and settings", () => {
  const [a, b, c] = [identity("Studio"), identity("Desk"), identity("Lab")];
  const id = newSetupID();
  const legacy: Legacy = { id, epoch: 4, primary: a.key, sign: a.peer.sign, role: "primary", managed: true, checkpointed: true,
    standby: { key: b.key, sign: b.peer.sign }, successorChain: true, numberCeiling: 900 };
  const old = signDoc(a.sign, "leadership", 4, { setup: { id, epoch: 4, primary: a.key }, previous: a.key,
    standby: b.key, successorChain: true, numberCeiling: 900 });
  const shared = {
    membershipAndAccess: { sequence: 7, value: { boxes: [a.peer, b.peer, c.peer], successorOrder: [b.key, a.key, c.key], successorPin: b.key,
      left: [], moved: [], devices: { phones: [phone], removed: [] }, removalRevisions: {} } },
    setupNames: { sequence: 2, value: { [c.key]: "Lab upstairs" } },
  };
  const converted = convert({ key: a.seed, sign: a.sign, secret: a.seed, setup: legacy, leadership: { 4: old }, sharedSetup: id, shared });

  expect(converted.setup).toEqual({ id, epoch: 5, primary: a.key, sign: a.peer.sign, role: "primary", managed: true, checkpointed: true });
  expect(readDoc(converted.leadership![5])!.body.successorChain).toBeUndefined();
  const membership = converted.shared!.membershipAndAccess!.value as Record<string, unknown>;
  expect(membership.successorOrder).toBeUndefined();
  expect(acceptedMembership(converted)!.boxes.map(box => box.key)).toEqual([a.key, b.key, c.key]);
  expect(converted.shared!.setupNames!.value).toEqual({ [c.key]: "Lab upstairs" });

  const member = follow({ key: c.seed, sign: c.sign, setup: { ...legacy, role: "member" }, sharedSetup: id, shared }, converted);
  expect(member.setup?.epoch).toBe(5);
  expect(acceptedMembership(member)!.boxes.map(box => box.key)).toEqual([a.key, b.key, c.key]);
  expect(member.shared!.setupNames!.value).toEqual({ [c.key]: "Lab upstairs" });
});

test("no computer is shown as standby, even where an older release saved one", () => {
  const [a, b] = [identity("Studio"), identity("Desk")];
  const id = newSetupID();
  const setup: Legacy = { id, epoch: 3, primary: a.key, sign: a.peer.sign, role: "primary", managed: true, checkpointed: true,
    standby: { key: b.key, sign: b.peer.sign } };
  const view = setupView({ key: a.seed, sign: a.sign, setup, sharedSetup: id,
    shared: { membershipAndAccess: { sequence: 1, value: { boxes: [a.peer, b.peer] } } } });
  expect(view.computers.map(computer => [computer.key, computer.role])).toEqual([[a.key, "primary"], [b.key, "member"]]);
  expect(view).not.toHaveProperty("standby");
});
