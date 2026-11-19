import "./safety";
import { expect, test } from "bun:test";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signDoc, signingKeyOf } from "../src/link/documents/format";
import { changedWork, digestOf, docsWork } from "../src/link/documents/work";
import { readLink, writeLink, type LinkState } from "../src/link/state";
import { grantLease, leaseClock } from "../src/link/setup/lease";
import { RUN } from "../src/link/home";
import { cacheCheckpoint, checkpointHash, checkpointedTransition, leadershipDocuments } from "../src/link/setup/checkpoint";
import { takeLeadership } from "../src/link/setup/leadership";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { installShared, operationOutcome, pendingShared, queryOperation, sharedValue, stageShared, takeShared } from "../src/link/setup/shared";

function identity() {
  const secret = newKey();
  const sign = newSigningSeed();
  return { key: publicOf(secret).toString("base64url"), seed: secret.toString("base64url"), address: addressOf(secret).toString("base64url"), sign, signer: signingKeyOf(sign) };
}

test("only the assigned standby receipts the primary's exact save and pending ownership transfers survive takeover", () => {
  const a = identity(), b = identity(), c = identity();
  const setup: SetupRecord = {
    id: newSetupID(), epoch: 2, primary: a.key, sign: a.signer, role: "primary", managed: true,
    standby: { key: b.key, sign: b.signer }, numberCeiling: 2_000,
  };
  const primary: LinkState = { key: a.seed, sign: a.sign, setup };
  const standby: LinkState = { key: b.seed, sign: b.sign, setup: { ...setup, role: "member" } };
  const member: LinkState = { key: c.seed, sign: c.sign, setup: { ...setup, role: "member" } };
  const ownership = { pending: [{ id: "move-one", source: a.key, destination: c.key }] };

  expect(stageShared(member, "ownershipAssignments", ownership)).toBeNull();
  const save = stageShared(primary, "ownershipAssignments", ownership)!;
  expect(save.sequence).toBe(1);
  expect(save.operation).toMatch(/^[\w-]{22}$/);
  expect(sharedValue(primary, "ownershipAssignments")).toBeUndefined();
  const proposal = pendingShared(primary, "ownershipAssignments")!;
  expect(takeShared(member, a.key, proposal)).toBeNull();
  expect(takeShared(standby, c.key, proposal)).toBeNull();
  const receipt = takeShared(standby, a.key, proposal)!;
  expect(readDoc(receipt)?.origin).toBe(b.signer);
  expect(takeShared(standby, a.key, proposal)).toEqual(receipt);

  grantLease(primary, leaseClock());
  const signed = readDoc(receipt)!;
  const replay = readDoc(signDoc(b.sign, "setup-shared", signed.n, { ...signed.body, epoch: 1 }))!;
  takeShared(primary, b.key, replay);
  expect(sharedValue(primary, "ownershipAssignments")).toBeUndefined();
  const wrongProposal = readDoc(signDoc(b.sign, "setup-shared", signed.n, { ...signed.body, receipt: "different" }))!;
  takeShared(primary, b.key, wrongProposal);
  expect(sharedValue(primary, "ownershipAssignments")).toBeUndefined();
  takeShared(primary, b.key, signed);
  expect(sharedValue(primary, "ownershipAssignments")).toEqual(ownership);

  const certificate = readDoc(signDoc(b.sign, "leadership", 3, {
    setup: { id: setup.id, epoch: 3, primary: b.key }, previous: a.key, takeover: true,
    boxes: [a, b, c].map(box => ({ key: box.key, sign: box.signer, address: box.address, name: "Box", kind: "machine" })),
    left: [], shared: standby.shared,
  }))!;
  expect(takeLeadership(standby, certificate)).toBe(true);
  expect(sharedValue(standby, "ownershipAssignments")).toEqual(ownership);
  expect(pendingShared(standby, "ownershipAssignments")).toBeNull();
});

test("a receiver requests exactly the next authority record beyond the old digest limit", () => {
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
    expect(transitions).toEqual([epoch + 1]);
  }
});

test("each interrupted operation remains queryable when a later epoch reuses its base revision", () => {
  const a = identity(), b = identity();
  const setup: SetupRecord = {
    id: newSetupID(), epoch: 2, primary: a.key, sign: a.signer, role: "primary", managed: true,
    standby: { key: b.key, sign: b.signer }, numberCeiling: 2_000,
  };
  const primary: LinkState = { key: a.seed, sign: a.sign, setup };
  const first = stageShared(primary, "setupNames", { [b.key]: "First attempt" })!;
  expect(operationOutcome(primary, setup.id, first.operation)?.status).toBe("pending");
  setup.epoch++;
  installShared(primary, {}, {});
  const second = stageShared(primary, "setupNames", { [b.key]: "Second attempt" })!;
  expect(second.sequence).toBe(first.sequence);
  expect(second.operation).not.toBe(first.operation);
  setup.epoch++;
  installShared(primary, {}, {});
  expect(operationOutcome(primary, setup.id, first.operation)).toMatchObject({ status: "interrupted", epoch: 2, baseRevision: 0 });
  expect(operationOutcome(primary, setup.id, second.operation)).toMatchObject({ status: "interrupted", epoch: 3, baseRevision: 0 });
});

test("a checkpoint preserves operation pages and accepts a lost receipt after promotion", () => {
  const a = identity(), b = identity();
  const id = newSetupID();
  const setup: SetupRecord = {
    id, epoch: 2, primary: a.key, sign: a.signer, role: "primary", managed: true, checkpointed: true,
    standby: { key: b.key, sign: b.signer }, numberCeiling: 2_000,
  };
  const primary: LinkState = { key: a.seed, sign: a.sign, setup, sharedSetup: id };
  const standby: LinkState = { key: b.seed, sign: b.sign, setup: { ...setup, role: "member" }, sharedSetup: id };
  const save = stageShared(primary, "setupNames", { [b.key]: "Renamed" })!;
  const proposal = pendingShared(primary, "setupNames")!;
  expect(takeShared(standby, a.key, proposal)).not.toBeNull();
  expect(operationOutcome(primary, id, save.operation)?.status).toBe("pending");
  standby.sharedOutcomes = {
    ...Object.fromEntries(Array.from({ length: 520 }, (_, index) => [crypto.randomBytes(16).toString("base64url"), {
      kind: "queueLabels" as const, epoch: 1, sequence: index + 1, baseRevision: index,
      proposal: crypto.randomBytes(64).toString("base64url"), status: "committed" as const,
    }])),
    ...standby.sharedOutcomes,
  };
  const boxes = [a, b].map(box => ({ key: box.key, sign: box.signer, address: box.address, name: "Box", kind: "machine" }));
  const transition = checkpointedTransition(standby, {
    setup: { id, epoch: 3, primary: b.key }, previous: a.key, takeover: true, sharedSaves: true,
  }, { ...standby.shared, membershipAndAccess: { sequence: 1, value: { boxes } } })!;
  const [manifest, ...tail] = leadershipDocuments(transition.envelope).map(envelope => readDoc(envelope)!);
  const pages = tail.slice(0, -1);
  expect(pages).toHaveLength(3);
  expect(manifest!.body.archives).toEqual(pages.map(checkpointHash));
  expect(transition.body.shared).toBeUndefined();
  expect(transition.body.outcomes).toBeUndefined();

  // Lose one page on the receiver. Neither its role nor its pending operation changes.
  const missing = pages[1]!;
  const file = path.join(RUN, "setup-checkpoints", checkpointHash(missing) + ".json");
  fs.unlinkSync(file);
  expect(takeLeadership(primary, transition, manifest)).toBe(false);
  expect(operationOutcome(primary, id, save.operation)?.status).toBe("pending");

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

for (const accepted of [true, false]) {
  test(`operation proofs cross a legacy roster takeover (${accepted ? "accepted" : "unreplicated"})`, () => {
    const a = identity(), b = identity(), c = identity();
    const id = newSetupID();
    const setup: SetupRecord = {
      id, epoch: 2, primary: a.key, sign: a.signer, role: "primary", managed: true, sharedSaves: true,
      standby: { key: b.key, sign: b.signer }, numberCeiling: 2_000,
    };
    const primary: LinkState = { key: a.seed, sign: a.sign, setup };
    const standby: LinkState = { key: b.seed, sign: b.sign, setup: { ...setup, role: "member" } };
    const member: LinkState = { key: c.seed, sign: c.sign, setup: { ...setup, role: "member" } };
    const save = stageShared(primary, "setupNames", { [b.key]: "Renamed" })!;
    if (accepted) expect(takeShared(standby, a.key, pendingShared(primary, "setupNames")!)).not.toBeNull();
    const boxes = [a, b, c].map(box => ({ key: box.key, sign: box.signer, address: box.address, name: "Box", kind: "machine" }));
    const legacy = readDoc(signDoc(b.sign, "leadership", 3, {
      setup: { id, epoch: 3, primary: b.key }, previous: a.key, takeover: true, sharedSaves: true,
      boxes, shared: standby.shared ?? {},
    }))!;
    const previous = readLink();
    try {
      // A member sees the roster but did not accept saves. It cannot infer interruption.
      expect(takeLeadership(member, legacy)).toBe(true);
      writeLink(member);
      expect(queryOperation(id, save.operation, save.proof)).toBeNull();

      expect(takeLeadership(standby, legacy)).toBe(true);
      writeLink(standby);
      expect(queryOperation(id, save.operation, undefined)?.status).toBe(accepted ? "committed" : undefined);
      expect(queryOperation(id, save.operation, save.proof)?.status).toBe(accepted ? "committed" : "interrupted");
      const ticket = readDoc(save.proof)!;
      expect(queryOperation(id, save.operation, signDoc(b.sign, ticket.kind, ticket.n, ticket.body))).toBeNull();
      const decided = readLink()!;
      expect(decided.sharedOutcomes?.[save.operation]?.status).toBe(accepted ? "committed" : "interrupted");

      const checkpoint = checkpointedTransition(decided, {
        setup: { id, epoch: 4, primary: b.key }, previous: b.key, standby: c.key, numberCeiling: 4_000,
      }, { ...decided.shared, membershipAndAccess: { sequence: 1, value: { boxes } } })!;
      expect(takeLeadership(decided, checkpoint)).toBe(true);
      writeLink(decided);
      expect(queryOperation(id, save.operation, save.proof)?.status).toBe(accepted ? "committed" : "interrupted");
    } finally {
      if (previous) writeLink(previous);
      else fs.rmSync(path.join(RUN, "link.json"), { force: true });
    }
  });
}
