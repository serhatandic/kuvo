import "./safety";
import { afterEach, beforeEach, expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signingKeyOf } from "../src/link/documents/format";
import { LINK_FILE, RUN } from "../src/link/home";
import { grantLease, leaseClock } from "../src/link/setup/lease";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { pendingShared, takeShared } from "../src/link/setup/shared";
import { readLink, updateLink, writeLink, type LinkState } from "../src/link/state";
import { changeNumbers, giveBlock, NUMBERS_FILE, readNumbers, takeNumber } from "../src/link/work-queue/numbers";

const leaseFile = path.join(RUN, "setup-lease.json");
const saved = new Map<string, Buffer | null>();
const allocation = () => readLink()?.shared?.queueNumberAllocation;
const pending = () => pendingShared(readLink(), "queueNumberAllocation");
const self = { self: true } as const;
const ask = { id: "wqn_firstgrant", count: 2 };
const otherAsk = { id: "wqn_othergrant", count: 2 };

beforeEach(() => {
  for (const file of [LINK_FILE, leaseFile, NUMBERS_FILE]) {
    saved.set(file, fs.existsSync(file) ? fs.readFileSync(file) : null);
    if (saved.get(file) !== null) fs.unlinkSync(file);
  }
});

afterEach(() => {
  for (const [file, bytes] of saved) {
    if (bytes === null) {
      if (fs.existsSync(file)) fs.unlinkSync(file);
    } else fs.writeFileSync(file, bytes, { mode: 0o600 });
  }
  saved.clear();
});

function identity(name: string) {
  const secret = newKey();
  const sign = newSigningSeed();
  const peer = {
    key: publicOf(secret).toString("base64url"),
    address: addressOf(secret).toString("base64url"),
    sign: signingKeyOf(sign),
    kind: "machine" as const,
    name,
  };
  return { key: secret.toString("base64url"), sign, peer };
}

// One persisted allocation ledger. The standby processes a signed proposal in memory.
function setupLedger(next = 10, numberCeiling = 100, leased = true) {
  const main = identity("Main");
  const witness = identity("Standby");
  const id = newSetupID();
  const setup: SetupRecord = {
    id, epoch: 4, role: "primary", primary: main.peer.key, sign: main.peer.sign,
    managed: true, checkpointed: true, standby: { key: witness.peer.key, sign: witness.peer.sign },
    numberCeiling,
  };
  const primary: LinkState = { key: main.key, sign: main.sign, setup, sharedSetup: id, shared: {
    membershipAndAccess: { sequence: 1, value: { boxes: [main.peer, witness.peer] } },
    queueNumberAllocation: { sequence: 1, value: { next } },
  } };
  const standby: LinkState = { key: witness.key, sign: witness.sign, setup: { ...setup, role: "member" },
    sharedSetup: id, shared: structuredClone(primary.shared) };
  writeLink(primary, true);
  if (leased) grantLease(primary, leaseClock());
  return { primary, standby, by: main.peer.key, signer: witness.peer.key };
}

function standbyReceipt({ standby, by }: ReturnType<typeof setupLedger>) {
  const proposal = pending();
  expect(proposal).not.toBeNull();
  const receipt = readDoc(takeShared(standby, by, proposal!));
  expect(receipt).not.toBeNull();
  return receipt!;
}

function installReceipt({ signer }: ReturnType<typeof setupLedger>, receipt: ReturnType<typeof standbyReceipt>) {
  updateLink(link => {
    takeShared(link!, signer, receipt);
    return link;
  }, { durable: true });
}

test("replaying a retained grant ID returns the original range only to its recipient", () => {
  const ledger = setupLedger();
  expect(giveBlock(ledger.by, ask)).toBeNull();
  installReceipt(ledger, standbyReceipt(ledger));
  const granted = { id: ask.id, from: 10, to: 11, granted: [] };
  expect(giveBlock(ledger.by, ask)).toEqual(granted);
  const accepted = allocation();
  expect(accepted).toMatchObject({ sequence: 2, value: { next: 12, given: { [ask.id]: { by: ledger.by, from: 10, to: 11 } } } });
  expect(giveBlock(ledger.by, { ...ask, count: 20, top: 90, claims: [[20, 20]] })).toEqual(granted);
  expect(giveBlock(ledger.signer, ask)).toBeNull();
  expect(allocation()).toEqual(accepted);
  expect(readNumbers().next).toBe(12);
  expect(pending()).toBeNull();
});

test("fresh allocations honor the inherited ceiling encoded in the accepted counter", () => {
  const inheritedCeiling = 2_000;
  const ledger = setupLedger(inheritedCeiling + 1, 4_000);
  changeNumbers(kept => { kept.next = 12; });
  const request = { ...ask, claims: [[inheritedCeiling, inheritedCeiling]] };
  expect(giveBlock(ledger.by, request)).toBeNull();
  expect(readNumbers().next).toBe(inheritedCeiling + 1);
  installReceipt(ledger, standbyReceipt(ledger));
  expect(giveBlock(ledger.by, request)).toEqual({ id: ask.id, from: 2_001, to: 2_002, granted: [] });
  expect(allocation()).toMatchObject({ value: { next: 2_003 } });
});

test("exhaustion leaves a pending ask and a fresh grant cannot cross the reservation ceiling", () => {
  const ledger = setupLedger(100);
  changeNumbers(kept => { kept.leases = [[99, 99]]; });
  expect(takeNumber(self)).toBe(99);
  expect(takeNumber(self)).toBeNull();
  expect(readNumbers()).toMatchObject({ leases: [], used: [[99, 99]], asking: { id: expect.any(String) } });

  expect(giveBlock(ledger.by, ask)).toBeNull();
  expect(pending()).toBeNull();
  expect(allocation()).toEqual({ sequence: 1, value: { next: 100 } });

  const last = { ...ask, count: 1 };
  expect(giveBlock(ledger.by, last)).toBeNull();
  installReceipt(ledger, standbyReceipt(ledger));
  expect(giveBlock(ledger.by, last)).toEqual({ id: ask.id, from: 100, to: 100, granted: [] });
  expect(giveBlock(ledger.by, { ...otherAsk, count: 1 })).toBeNull();
  expect(pending()).toBeNull();
  expect(allocation()).toMatchObject({ sequence: 2, value: { next: 101 } });
});

test("a missing primary lease blocks a new grant while an already granted number stays usable", () => {
  const ledger = setupLedger(10, 100, false);
  changeNumbers(kept => { kept.leases = [[9, 9]]; });
  expect(fs.existsSync(leaseFile)).toBe(false);
  expect(giveBlock(ledger.by, ask)).toBeNull();
  expect(pending()).toBeNull();
  expect(allocation()).toEqual({ sequence: 1, value: { next: 10 } });
  expect(takeNumber(self)).toBe(9);
  expect(takeNumber(self)).toBeNull();

  grantLease(ledger.primary, leaseClock());
  expect(giveBlock(ledger.by, ask)).toBeNull();
  expect(pending()?.body.value).toMatchObject({ next: 12, given: { [ask.id]: { from: 10, to: 11 } } });
});

test("a newly staged grant stays withheld until the standby receipt is installed", () => {
  const ledger = setupLedger();
  expect(giveBlock(ledger.by, ask)).toBeNull();
  const proposal = pending();
  expect(proposal?.body.value).toMatchObject({ next: 12, given: { [ask.id]: { from: 10, to: 11 } } });
  expect(readNumbers().allocationSave).toMatchObject({ status: "pending", epoch: 4 });
  expect(allocation()).toEqual({ sequence: 1, value: { next: 10 } });
  expect(takeNumber(self)).toBeNull();

  const receipt = standbyReceipt(ledger);
  expect(giveBlock(ledger.by, ask)).toBeNull();
  expect(giveBlock(ledger.by, otherAsk)).toBeNull();
  expect(pending()?.envelope).toEqual(proposal?.envelope);
  expect(allocation()).toEqual({ sequence: 1, value: { next: 10 } });

  installReceipt(ledger, receipt);
  expect(pending()).toBeNull();
  expect(allocation()).toMatchObject({ sequence: 2, value: { next: 12 } });
  expect(giveBlock(ledger.by, ask)).toEqual({ id: ask.id, from: 10, to: 11, granted: [] });
});
