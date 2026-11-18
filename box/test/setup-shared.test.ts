import "./safety";
import { expect, test } from "bun:test";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signDoc, signingKeyOf } from "../src/link/documents/format";
import type { LinkState } from "../src/link/state";
import { grantLease, leaseClock } from "../src/link/setup/lease";
import { takeLeadership } from "../src/link/setup/leadership";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { pendingShared, sharedValue, stageShared, takeShared } from "../src/link/setup/shared";

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
  expect(stageShared(primary, "ownershipAssignments", ownership)).toBe(1);
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
