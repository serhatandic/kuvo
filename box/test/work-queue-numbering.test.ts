import "./safety"
import { afterEach, beforeEach, expect, test } from "bun:test";
import fs from "node:fs";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, signingKeyOf } from "../src/link/documents/format";
import { LINK_FILE } from "../src/link/home";
import { grantRelayLease, leaseFile } from "./relay-lease";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { writeLink, type LinkState } from "../src/link/state";
import { changeNumbers, giveBlock, NUMBERS_FILE, readNumbers, takeNumber } from "../src/link/work-queue/numbers";

import { changeQueues, changeQueuesLater } from "../src/link/work-queue/store";
import { addItem } from "../src/link/work-queue/ops";
import { actHere } from "../src/link/work-queue/acts";


const saved = new Map<string, Buffer | null>();
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

// A numbered setup upgrading from the previous allocation ledger.
function setupLedger(next = 10, numberCeiling = 100, leased = true) {
  const main = identity("Main");
  const witness = identity("Standby");
  const id = newSetupID();
  const setup: SetupRecord = {
    id, epoch: 4, role: "primary", primary: main.peer.key, sign: main.peer.sign,
    managed: true, checkpointed: true,
    numberCeiling,
  };
  const primary: LinkState = { key: main.key, sign: main.sign, setup, sharedSetup: id, shared: {
    membershipAndAccess: { sequence: 1, value: { boxes: [main.peer, witness.peer] } },
    queueNumberAllocation: { sequence: 1, value: { next } },
  } };
  writeLink(primary, true);
  if (leased) grantRelayLease(primary, { high: numberCeiling });
  return { primary, by: main.peer.key, signer: witness.peer.key };
}

test("replaying a retained grant ID returns the original range only to its recipient", () => {
  const ledger = setupLedger();
  const granted = { id: ask.id, from: 10, to: 11, granted: [] };
  expect(giveBlock(ledger.by, ask)).toEqual(granted);
  expect(giveBlock(ledger.by, { ...ask, count: 20, top: 90, claims: [[20, 20]] })).toEqual(granted);
  expect(giveBlock(ledger.signer, ask)).toBeNull();
  expect(readNumbers().next).toBe(12);
});

test("fresh allocations honor the inherited ceiling encoded in the accepted counter", () => {
  const inheritedCeiling = 2_000;
  const ledger = setupLedger(inheritedCeiling + 1, 4_000);
  changeNumbers(kept => { kept.next = 12; });
  const request = { ...ask, claims: [[inheritedCeiling, inheritedCeiling]] };
  expect(giveBlock(ledger.by, request)).toEqual({ id: ask.id, from: 2_001, to: 2_002, granted: [] });
  expect(readNumbers().next).toBe(2_003);
});

test("a fresh grant cannot cross the relay's high mark", () => {
  const ledger = setupLedger(100);
  expect(giveBlock(ledger.by, ask)).toBeNull();
  expect(readNumbers().next).toBe(100);
  expect(giveBlock(ledger.by, { ...ask, count: 1 })).toEqual({ id: ask.id, from: 100, to: 100, granted: [] });
  expect(giveBlock(ledger.by, { ...otherAsk, count: 1 })).toBeNull();
  expect(readNumbers().next).toBe(101);
});

test("a missing primary lease blocks a new grant while an already granted number stays usable", () => {
  const ledger = setupLedger(10, 100, false);
  changeNumbers(kept => { kept.leases = [[9, 9]]; });
  expect(fs.existsSync(leaseFile)).toBe(false);
  expect(giveBlock(ledger.by, ask)).toBeNull();
  expect(takeNumber(self)).toBe(9);
  expect(takeNumber(self)).toBeNull();
  grantRelayLease(ledger.primary, { high: 100 });
  expect(giveBlock(ledger.by, ask)).toEqual({ id: ask.id, from: 10, to: 11, granted: [] });
});

test("offline creation spends the final local lease and leaves the next item usable by id", async () => {
  setupLedger(100, 100, false);
  const owner = "ses_WaitingNumberTest";
  const make = (title: string) => changeQueuesLater(book => addItem(book, owner, { title },
    { who: { user: true }, now: Date.now(), fileOps: [] }, self));
  changeNumbers(kept => { kept.leases = [[99, 99]]; });
  const last = await make("Final leased number");
  const waiting = await make("Works before numbering");
  expect(last.number).toBe(99);
  expect(waiting.number).toBeUndefined();
  expect(readNumbers()).toMatchObject({ leases: [], asking: { id: expect.any(String) } });
  expect(await actHere({ method: "PATCH", ref: { id: waiting.id }, given: { title: "Edited while offline" }, targets: [] }))
    .toMatchObject({ status: 200, value: { item: { id: waiting.id, title: "Edited while offline", numberPending: true } } });
  expect(await actHere({ method: "GET", ref: { number: 100 }, targets: [] })).toMatchObject({ status: 404 });
  expect(await actHere({ method: "GET", ref: { id: waiting.id }, targets: [] }))
    .toMatchObject({ status: 200, value: { item: { id: waiting.id, title: "Edited while offline", numberPending: true } } });
  changeQueues(book => { book.queue(owner).items = []; });
});
