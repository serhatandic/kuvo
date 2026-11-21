import "./safety";
import { afterEach, beforeEach, expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signDoc, signingKeyOf } from "../src/link/documents/format";
import { K, LINK_FILE, RUN } from "../src/link/home";
import { grantLease, leaseClock } from "../src/link/setup/lease";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { pendingShared, takeShared } from "../src/link/setup/shared";
import { readLink, updateLink, writeLink, type LinkState } from "../src/link/state";
import { changeNumbers, giveBlock, NUMBERS_FILE, primaryOf, pushNumbers, readNumbers, takeNumber, takeNumbersMail } from "../src/link/work-queue/numbers";

import { checkpointedTransition } from "../src/link/setup/checkpoint";
import { takeLeadership, type SuccessorForks } from "../src/link/setup/leadership";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { allItems, changeQueues, matching, WORK_QUEUE, YOURS } from "../src/link/work-queue/store";
import { addItem } from "../src/link/work-queue/ops";
import { actHere } from "../src/link/work-queue/acts";
import { refOf, summaryOf } from "../src/link/work-queue/view";
import { openMail } from "../src/link/peers/known";
import { readFrame, readRelay } from "../src/link/relay/frames";
import { interceptSync } from "./disk-fault";

type Computer = ReturnType<typeof identity>;

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

test("rival successor reservations wait for a majority and survive a losing history", () => {
  upgradeKuvoFolder(K);
  const [main, a, b, c, d] = ["Main", "A", "B", "C", "D"].map(identity) as [Computer, Computer, Computer, Computer, Computer];
  const id = newSetupID();
  const root: LinkState = {
    key: main.key, sign: main.sign, sharedSetup: id,
    setup: { id, epoch: 1, primary: main.peer.key, sign: main.peer.sign, role: "primary", managed: true, checkpointed: true },
    shared: {
      membershipAndAccess: { sequence: 1, value: { boxes: [main, a, b, c, d].map(box => box.peer),
        successorOrder: [a, b, c, d, main].map(box => box.peer.key) } },
      queueNumberAllocation: { sequence: 1, value: { next: 98 } },
    },
  };
  const activated = checkpointedTransition(root, {
    setup: { id, epoch: 2, primary: main.peer.key }, previous: main.peer.key,
    successorChain: true, standby: a.peer.key, numberCeiling: 100,
  }, root.shared!)!;
  expect(takeLeadership(root, activated)).toBe(true);

  const follower = (computer: Computer, link: LinkState): LinkState => ({
    ...structuredClone(link), key: computer.key, sign: computer.sign,
    setup: { ...link.setup!, role: "member" },
  });
  const takeover = (computer: Computer, link: LinkState) => {
    const successor = follower(computer, link);
    const { primary: previous, epoch, numberCeiling } = successor.setup!;
    const ledger = successor.shared!.queueNumberAllocation!;
    const next = Math.max((numberCeiling ?? 0) + 1, Number((ledger.value as { next: number }).next));
    const certificate = checkpointedTransition(successor, {
      setup: { id, epoch: epoch + 1, primary: computer.peer.key }, previous,
      successorChain: true, takeover: true, sharedSaves: true,
    }, { ...successor.shared, queueNumberAllocation: { ...ledger, value: { next } } })!;
    expect(takeLeadership(successor, certificate)).toBe(true);
    return { link: successor, certificate };
  };

  const left = takeover(a, root);
  const right = takeover(b, root);
  const later = takeover(d, right.link);
  const states = new Map<Computer, LinkState>([[main, follower(main, left.link)], [a, left.link], [b, right.link],
    [c, follower(c, left.link)], [d, follower(d, right.link)]]);
  const homes = fs.mkdtempSync(path.join(RUN, "number-voter-disk-"));
  const original = path.join(homes, "original");
  if (fs.existsSync(WORK_QUEUE)) fs.renameSync(WORK_QUEUE, original);
  const on = <T>(computer: Computer, run: () => T): T => {
    const disk = path.join(homes, computer.peer.name);
    if (fs.existsSync(disk)) fs.renameSync(disk, WORK_QUEUE);
    try {
      writeLink(states.get(computer)!, true);
      return run();
    } finally {
      if (fs.existsSync(WORK_QUEUE)) fs.renameSync(WORK_QUEUE, disk);
    }
  };
  const outgoing = (run: (host: { welcomed: true; send: (bytes: Buffer) => void }) => void) => {
    const messages: Buffer[] = [];
    run({ welcomed: true, send: bytes => { messages.push(bytes); } });
    return messages;
  };
  const forBox = (messages: Buffer[], computer: Computer) =>
    messages.filter(bytes => readRelay(bytes)?.address.toString("base64url") === computer.peer.address);
  const receive = (computer: Computer, messages: Buffer[]) => on(computer, () => outgoing(host => {
    for (const bytes of forBox(messages, computer)) {
      const packet = readRelay(bytes)!;
      if (packet.kind !== "deliver") throw new Error("Expected relay mail");
      const opened = openMail(readFrame(packet.payload)!.body, readLink());
      expect(opened).not.toBeNull();
      takeNumbersMail(opened!.sender, opened!.payload.workQueue, host);
    }
  }));
  const tick = (computer: Computer) => on(computer, () => outgoing(host => pushNumbers(host)));
  const vote = (issuer: Computer, voters: Computer[]) => {
    const proposals = tick(issuer);
    for (const voter of voters) receive(issuer, receive(voter, proposals));
  };
  const create = (computer: Computer, title: string) => on(computer, () =>
    changeQueues(book => addItem(book, YOURS, { title }, { who: { user: true }, now: Date.now(), fileOps: [] }, primaryOf())));
  const find = (computer: Computer, item: string) => on(computer, () => matching(allItems(), refOf(item)!)[0]);
  const askPeer = (computer: Computer, primary: Computer) => on(computer, () => {
    // Allow an immediate resend without advancing a real clock.
    changeNumbers(kept => { if (kept.asking) delete kept.asking.sent; });
    return forBox(outgoing(host => pushNumbers(host)), primary);
  });

  try {
    const itemA = create(a, "Created offline on A");
    const itemB = create(b, "Created offline on B");
    expect(itemA.number).toBeUndefined();
    expect(itemB.number).toBeUndefined();
    expect(summaryOf(itemB, new Set(), new Map())).toMatchObject({ id: itemB.id, numberPending: true });
    expect(find(b, itemB.id)?.title).toBe(itemB.title);
    expect(find(b, "#101")).toBeUndefined();

    tick(a);
    tick(b);
    const proposals = tick(a);
    const restoreSync = interceptSync(file => file === WORK_QUEUE);
    try {
      // A failed directory sync may leave a visible vote. A replay must still
      // seal it before sending an acknowledgment.
      expect(() => receive(c, proposals)).toThrow("Injected sync failure");
      expect(() => receive(c, proposals)).toThrow("Injected sync failure");
    } finally { restoreSync(); }
    receive(a, receive(c, proposals));
    vote(b, [d]);
    expect(on(a, () => readNumbers().quorum?.pending?.proposal.from)).toBe(101);
    expect(on(b, () => readNumbers().quorum?.pending?.proposal.from)).toBe(101);
    expect(find(a, itemA.id)?.number).toBeUndefined();
    expect(find(b, itemB.id)?.number).toBeUndefined();

    vote(a, [main]);
    tick(a);
    expect(find(a, itemA.id)?.number).toBe(101);

    on(a, () => { expect(giveBlock(main.peer.key, otherAsk)).toBeNull(); });
    vote(a, [c, main]);
    expect(on(a, () => giveBlock(main.peer.key, otherAsk))).toEqual({ id: otherAsk.id, from: 151, to: 152, granted: [] });

    const cLater = states.get(c)!;
    const forks: SuccessorForks = new Map();
    expect(takeLeadership(cLater, right.certificate, undefined, forks)).toBe(false);
    expect(takeLeadership(cLater, later.certificate, undefined, forks)).toBe(true);
    const changedElectorate = readDoc(signDoc(d.sign, "leadership", later.certificate.n, {
      ...later.certificate.body, numberVoters: [b, c, d].map(box => box.peer.key).sort(),
    }))!;
    expect(takeLeadership(states.get(b)!, changedElectorate)).toBe(false);
    expect(takeLeadership(states.get(b)!, later.certificate)).toBe(true);
    states.set(d, later.link);

    receive(d, askPeer(b, d));
    expect(on(d, () => readNumbers().quorum?.pending?.proposal.from)).toBe(151);
    vote(d, [b, c]); // C carries the higher floor even after discarding A's branch.
    expect(find(b, itemB.id)?.number).toBeUndefined();
    expect(on(d, () => readNumbers().quorum?.pending?.proposal.from)).toBe(201);
    vote(d, [b, c]);
    receive(b, receive(d, askPeer(b, d)));
    expect(find(b, itemB.id)?.number).toBe(201);
    expect(find(b, "#201")?.id).toBe(itemB.id);
    expect(summaryOf(find(b, itemB.id)!, new Set(), new Map()).numberPending).toBeUndefined();

    const staleAsk = { ...ask, id: "wqn_staleissuer" };
    expect(on(a, () => giveBlock(a.peer.key, staleAsk))).toBeNull();
    vote(a, [b, c, d]);
    expect(on(a, () => giveBlock(a.peer.key, staleAsk))).toBeNull();

    expect(takeLeadership(left.link, right.certificate, undefined, forks)).toBe(false);
    expect(takeLeadership(left.link, later.certificate, undefined, forks)).toBe(true);
    expect(find(a, itemA.id)?.number).toBe(101);
    expect(find(a, "#101")?.id).toBe(itemA.id);
  } finally {
    if (fs.existsSync(original)) fs.renameSync(original, WORK_QUEUE);
  }
});

test("offline creation spends the final local lease and leaves the next item usable by id", async () => {
  setupLedger(100, 100, false);
  const owner = "ses_WaitingNumberTest";
  const make = (title: string) => changeQueues(book => addItem(book, owner, { title },
    { who: { user: true }, now: Date.now(), fileOps: [] }, self));
  changeNumbers(kept => { kept.leases = [[99, 99]]; });
  const last = make("Final leased number");
  const waiting = make("Works before numbering");
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
