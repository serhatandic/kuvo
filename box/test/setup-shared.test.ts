import "./safety"
import { expect, test } from "bun:test";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signDoc, signingKeyOf } from "../src/link/documents/format";
import { changedWork, digestOf, docsWork } from "../src/link/documents/work";
import { readLink, writeLink, type LinkState } from "../src/link/state";
import { leaseClock, rememberRelayLease } from "../src/link/setup/lease";
import { grantRelayLease, leaseFile } from "./relay-lease";
import { K, RUN } from "../src/link/home";
import { cacheCheckpoint, checkpointHash, checkpointedTransition, leadershipDocuments } from "../src/link/setup/checkpoint";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { membershipOf } from "../src/link/setup/membership";
import { acceptedMembership } from "../src/link/setup/membership";
import { takeLeadership } from "../src/link/setup/leadership";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { settleOwnSetup } from "../src/link/setup/lead";
import { installShared, MAX_SHARED_BYTES, operationOutcome, publishedShared, sharedValue, stageShared, takeShared } from "../src/link/setup/shared";
import { ADD_RESERVATION } from "../src/link/marks";
import { restoreLabels } from "../src/link/work-queue/labels";

function identity() {
  const secret = newKey();
  const sign = newSigningSeed();
  return { key: publicOf(secret).toString("base64url"), seed: secret.toString("base64url"), address: addressOf(secret).toString("base64url"), sign, signer: signingKeyOf(sign) };
}

test("a running machine waits for the SSH reservation before creating an independent setup", () => {
  const box = identity();
  const link: LinkState = { key: box.seed, sign: box.sign, phones: [] };
  const reserved = path.join(path.dirname(K), ADD_RESERVATION);
  fs.mkdirSync(path.dirname(reserved), { recursive: true });
  fs.symlinkSync("kuvo-fixture-reservation", reserved);
  try {
    settleOwnSetup(link);
    expect(link.setup).toBeUndefined();
  } finally {
    fs.unlinkSync(reserved);
  }
  settleOwnSetup(link);
  expect(link.setup?.role).toBe("primary");
  expect(link.setup?.primary).toBe(box.key);
});

test("a new setup starts in the current model, with no relay to convert it", () => {
  const box = identity();
  const link: LinkState = { key: box.seed, secret: crypto.randomBytes(32).toString("base64url"), sign: box.sign, name: "Lone", phones: [] };
  settleOwnSetup(link);
  expect(link.setup).toMatchObject({ epoch: 1, primary: box.key, role: "primary", managed: true, checkpointed: true });
  // Its membership is accepted, this computer its one member, under epoch 1's certificate.
  expect(acceptedMembership(link)?.boxes.map(each => each.key)).toEqual([box.key]);
  expect(readDoc(link.leadership?.[1])?.body.setup).toEqual({ id: link.setup!.id, epoch: 1, primary: box.key });
  // Settled again (each start), it stays the same setup.
  const id = link.setup!.id;
  settleOwnSetup(link);
  expect(link.setup?.id).toBe(id);
});

test("label restore previews are provisional until the setup accepts its library", () => {
  const [a, b] = [identity(), identity()] as const;
  const id = newSetupID();
  const primary: LinkState = { key: a.seed, sign: a.sign, sharedSetup: id, setup: {
    id, epoch: 2, primary: a.key, sign: a.signer, role: "primary", checkpointed: true,
  } };
  const file = path.join(RUN, "link.json"), lease = leaseFile;
  const saved = new Map([file, lease].map(file => [file, fs.existsSync(file) ? fs.readFileSync(file) : null]));
  const label = { id: "lbl_previewed", name: "Previewed", color: "amber" };
  try {
    writeLink(primary);
    grantRelayLease(primary);
    const before = fs.readFileSync(file);
    expect(restoreLabels([label], true, false)).toEqual({ result: {
      restored: 1, alreadyPresent: 0, conflicts: 0, details: [], provisional: true,
    } });
    expect(fs.readFileSync(file)).toEqual(before);
    writeLink({ ...primary, shared: { queueLabels: { sequence: 1, value: { labels: [], removed: [label.id] } } } });
    const accepted = fs.readFileSync(file);
    expect(restoreLabels([label], true, false)).toEqual({ result: {
      restored: 0, alreadyPresent: 0, conflicts: 1,
      details: [{ id: label.id, reason: "This label was removed. Use --force to restore it." }],
    } });
    expect(fs.readFileSync(file)).toEqual(accepted);
  } finally {
    for (const [file, bytes] of saved) {
      if (bytes) fs.writeFileSync(file, bytes);
      else fs.rmSync(file, { force: true });
    }
  }
});

test("a computer behind takes the current certificate alone, however far behind it is", () => {
  const a = identity(), b = identity();
  const id = newSetupID();
  const setup: SetupRecord = { id, epoch: 530, primary: a.key, sign: a.signer, role: "primary", managed: true };
  const sender: LinkState = {
    key: a.seed, sign: a.sign, setup,
    macPeers: [{ key: b.key, sign: b.signer, name: "Desk", kind: "machine", address: b.address }],
    leadership: Object.fromEntries(Array.from({ length: 529 }, (_, index) => {
      const epoch = index + 2;
      return [epoch, signDoc(a.sign, "leadership", epoch, { setup: { id, epoch, primary: a.key } })];
    })),
  };
  const have = digestOf(sender).filter(entry => entry[1] === "leadership");
  expect(have.map(entry => entry[2])).toEqual([530]);

  for (const epoch of [2, 512, 513, 529]) {
    const cursor = readDoc(sender.leadership![epoch])!;
    const state = { link: sender, library: { personalities: [], removed: [] } };
    const result = docsWork(state, { key: b.key, sign: b.signer }, { have: [[cursor.origin, cursor.kind, cursor.n, cursor.h]], reply: true });
    const transitions = result.send.flatMap(mail => mail.docs.give ?? []).flatMap(envelope => {
      const doc = readDoc(envelope);
      return doc?.kind === "leadership" ? [doc.n] : [];
    });
    expect(transitions).toEqual([530]);
  }
});

test("a checkpoint preserves committed operation pages after promotion", () => {
  const a = identity(), b = identity();
  const id = newSetupID();
  const setup: SetupRecord = { id, epoch: 2, primary: a.key, sign: a.signer, role: "primary", managed: true, checkpointed: true };
  const primary: LinkState = { key: a.seed, sign: a.sign, setup, sharedSetup: id };
  const standby: LinkState = { key: b.seed, sign: b.sign, setup: { ...setup, role: "member" }, sharedSetup: id };
  grantRelayLease(primary);
  const save = stageShared(primary, "setupNames", { [b.key]: "Renamed" })!;
  standby.shared = structuredClone(primary.shared);
  standby.sharedOutcomes = {
    ...Object.fromEntries(Array.from({ length: 520 }, (_, index) => [crypto.randomBytes(16).toString("base64url"), {
      kind: "queueLabels" as const, epoch: 1, sequence: index + 1, baseRevision: index,
      proposal: crypto.randomBytes(64).toString("base64url"), status: "committed" as const,
    }])),
    ...primary.sharedOutcomes,
  };
  const boxes = [a, b].map(box => ({ key: box.key, sign: box.signer, address: box.address, name: "Box", kind: "machine" }));
  // The relay hands the lease to the other computer at term 3; it signs the next certificate.
  const since = leaseClock();
  rememberRelayLease({ setup: id, holder: b.key, sign: b.signer, term: 3, high: 0, relayHost: false, since, validAfter: since, duration: 60_000, expires: since.wall + 60_000 });
  const transition = checkpointedTransition(standby, {
    setup: { id, epoch: 3, primary: b.key }, previous: a.key, relayTerm: 3,
  }, { ...standby.shared, membershipAndAccess: { sequence: 1, value: { boxes } } })!;
  const [manifest, ...tail] = leadershipDocuments(transition.envelope).map(envelope => readDoc(envelope)!);
  const pages = tail.slice(0, -1);
  expect(pages).toHaveLength(3);
  expect(manifest!.body.archives).toEqual(pages.map(checkpointHash));
  expect(transition.body.shared).toBeUndefined();
  expect(transition.body.outcomes).toBeUndefined();

  // Lose one page on the receiver. Its role and original commit stay in place.
  const missing = pages[1]!;
  const file = path.join(RUN, "setup-checkpoints", checkpointHash(missing) + ".json");
  fs.unlinkSync(file);
  expect(takeLeadership(primary, transition, manifest)).toBe(false);
  expect(operationOutcome(primary, id, save.operation)?.status).toBe("committed");

  const cache = new Map();
  cacheCheckpoint(primary, cache, manifest!);
  cacheCheckpoint(primary, cache, missing);
  expect(takeLeadership(primary, transition, cache.get(transition.body.checkpoint))).toBe(true);
  expect(operationOutcome(primary, id, save.operation)).toMatchObject({ status: "committed", epoch: 2, sequence: 1, baseRevision: 0 });
  expect(Object.keys(primary.sharedOutcomes!)).toHaveLength(521);
  const update = changedWork({ link: primary, library: { personalities: [], removed: [] } });
  expect(update.send[0]?.docs.have?.some(([, kind, epoch]) => kind === "leadership" && epoch === 3)).toBe(true);
  expect(update.send.flatMap(mail => mail.docs.give ?? []).some(envelope => readDoc(envelope)?.kind === "setup-checkpoint")).toBe(false);
});
