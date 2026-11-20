import "./safety"
import { expect, test } from "bun:test";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signDoc, signingKeyOf } from "../src/link/documents/format";
import { changedWork, digestOf, docsWork } from "../src/link/documents/work";
import { readLink, writeLink, type LinkState } from "../src/link/state";
import { grantLease, HEARTBEAT_MS, leaseClock, takeoverProbeWindow } from "../src/link/setup/lease";
import { K, RUN } from "../src/link/home";
import { cacheCheckpoint, checkpointHash, checkpointedTransition, leadershipDocuments } from "../src/link/setup/checkpoint";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { membershipOf } from "../src/link/setup/membership";
import { observeAvailability, rankComputers, RANK_INTERVAL, resetAvailability } from "../src/link/setup/reliability";
import { acceptedMembership } from "../src/link/setup/membership";
import { publishRevocations } from "../src/link/setup/revocations";
import { approveLeadership, takeLeadership } from "../src/link/setup/leadership";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { installShared, MAX_SHARED_BYTES, operationOutcome, pendingShared, queryOperation, sharedValue, stageShared, takeShared } from "../src/link/setup/shared";

function identity() {
  const secret = newKey();
  const sign = newSigningSeed();
  return { key: publicOf(secret).toString("base64url"), seed: secret.toString("base64url"), address: addressOf(secret).toString("base64url"), sign, signer: signingKeyOf(sign) };
}

test("an incompatible new reader is refused before and after the standby commits a release", () => {
  upgradeKuvoFolder(K);
  const [a, b, c] = [identity(), identity(), identity()] as const;
  const id = newSetupID();
  const roster = [a, b, c].map(box => ({ key: box.key, sign: box.signer, address: box.address, name: "Computer", kind: "machine" }));
  const initial = (): LinkState => ({
    key: a.seed, sign: a.sign, sharedSetup: id,
    setup: { id, epoch: 2, primary: a.key, sign: a.signer, role: "primary", managed: true,
      checkpointed: true, standby: { key: b.key, sign: b.signer } },
    shared: { membershipAndAccess: { sequence: 1, value: { boxes: roster.slice(0, 2) } } },
  });
  fs.mkdirSync(RUN, { recursive: true });
  const facts = path.join(RUN, "peer-agents.json");
  const reader = (layout: number) => fs.writeFileSync(facts, JSON.stringify({ [c.key]: { computer: { layout } } }));
  const layout19 = { version: "0.10.0", layout: 19 };
  const membership = { boxes: roster };
  try {
    reader(18);
    const releaseFirst = initial();
    expect(stageShared(releaseFirst, "setupSecurityAndReleaseChoice", layout19)?.status).toBe("pending");
    expect(stageShared(releaseFirst, "membershipAndAccess", membership)).toBeNull();
    const membershipFirst = initial();
    expect(stageShared(membershipFirst, "membershipAndAccess", membership)?.status).toBe("pending");
    expect(stageShared(membershipFirst, "setupSecurityAndReleaseChoice", layout19)).toBeNull();
    const committed = initial();
    const standby: LinkState = { ...initial(), key: b.seed, sign: b.sign,
      setup: { ...committed.setup!, role: "member" } };
    const save = stageShared(committed, "setupSecurityAndReleaseChoice", layout19)!;
    expect(save.status).toBe("pending");
    const proposal = pendingShared(committed, "setupSecurityAndReleaseChoice")!;
    const receipt = readDoc(takeShared(standby, a.key, proposal))!;
    grantLease(committed, leaseClock());
    takeShared(committed, b.key, receipt);
    expect(operationOutcome(committed, id, save.operation)?.status).toBe("committed");
    expect(pendingShared(committed, "setupSecurityAndReleaseChoice")).toBeNull();
    expect(stageShared(committed, "membershipAndAccess", membership)).toBeNull();
    expect(acceptedMembership(committed)?.boxes).toHaveLength(2);
    // An older primary projected only the signed envelope, version and hash.
    const earlier = structuredClone(committed);
    const signed = Buffer.from(JSON.stringify({ manifest: Buffer.from(JSON.stringify(layout19)).toString("base64") })).toString("base64");
    earlier.shared!.setupSecurityAndReleaseChoice!.value = { version: layout19.version, signed };
    expect(stageShared(earlier, "membershipAndAccess", membership)).toBeNull();
    expect(stageShared(earlier, "membershipAndAccess", membership, undefined, undefined, undefined,
      { reader: { key: c.key, layout: 19 } })?.status).toBe("pending");
    reader(19);
    expect(stageShared(releaseFirst, "membershipAndAccess", membership)?.status).toBe("pending");
    expect(stageShared(membershipFirst, "setupSecurityAndReleaseChoice", layout19)?.status).toBe("pending");
    expect(stageShared(committed, "membershipAndAccess", membership)?.status).toBe("pending");
  } finally { fs.rmSync(facts, { force: true }); }
});

test("takeover probes share one heartbeat regardless of how many predecessors are offline", () => {
  expect(takeoverProbeWindow(true, 0)).toBe(0);
  for (const count of [1, 12, 63]) {
    expect(takeoverProbeWindow(true, count)).toBe(HEARTBEAT_MS);
    expect(takeoverProbeWindow(false, count)).toBe(HEARTBEAT_MS);
  }
});

test("accepted successor order prefers servers at bootstrap and learns sustained availability", () => {
  upgradeKuvoFolder(K);
  const boxes = [identity(), identity(), identity()].map(({ key, signer, address }, index) => ({
    key, sign: signer, address, name: `Computer ${index}`, kind: index === 0 ? "machine" as const : "mac" as const,
  }));
  const [server, laptop, other] = boxes;
  resetAvailability();
  const bootstrap = rankComputers(boxes)!;
  expect(bootstrap[0]).toBe(server!.key);
  const clock = leaseClock();
  for (let elapsed = 0; elapsed <= RANK_INTERVAL; elapsed += 10_000) {
    observeAvailability(laptop!.key, { uptime: clock.uptime + elapsed, wall: clock.wall + elapsed }, false, "Laptop");
  }
  const now = { uptime: clock.uptime + RANK_INTERVAL, wall: clock.wall + RANK_INTERVAL };
  observeAvailability(server!.key, now, true, "Server");
  const order = rankComputers(boxes, bootstrap, now)!;
  expect(order[0]).toBe(laptop!.key);
  expect(rankComputers(boxes, bootstrap, now, server!.key)?.[0]).toBe(server!.key);
  expect(membershipOf({ boxes, successorOrder: order })?.successorOrder).toEqual(order);
  expect(membershipOf({ boxes })?.successorOrder).toBeUndefined();
  expect(membershipOf({ boxes, successorOrder: [laptop!.key, server!.key, server!.key] })).toBeNull();
  expect(membershipOf({ boxes, successorOrder: [laptop!.key, other!.key] })).toBeNull();
  resetAvailability();
  expect(rankComputers(boxes, order, now)).toEqual(order);
});

test("competing successors reconcile conflicting certificates at the same epoch using the common epoch checkpoint rank", () => {
  upgradeKuvoFolder(K);
  const a = identity(), b = identity(), c = identity(), phone = identity();
  const id = newSetupID();
  const boxes = [a, b, c].map(box => ({ key: box.key, sign: box.signer, address: box.address, name: "Computer", kind: "machine" }));
  const setup: SetupRecord = { id, epoch: 1, primary: a.key, sign: a.signer, role: "primary", managed: true, checkpointed: true };
  const source: LinkState = { key: a.seed, sign: a.sign, setup };
  const pairing = "paired";
  const membership = { boxes, successorOrder: [b.key, c.key, a.key], devices: { phones: [{ key: phone.key, name: "Phone", pairing }], removed: [] } };
  const values = { membershipAndAccess: { sequence: 1, value: membership } };
  const parent = checkpointedTransition(source, {
    setup: { id, epoch: 2, primary: a.key }, previous: a.key, successorChain: true, standby: b.key, numberCeiling: 2_000,
  }, values)!;
  expect(takeLeadership(source, parent)).toBe(true);

  const follower: LinkState = { ...structuredClone(source), key: b.seed, sign: b.sign, setup: { ...source.setup!, role: "member" } };
  const refresh = stageShared(source, "membershipAndAccess", { ...membership, successorOrder: [a.key, c.key, b.key] })!;
  const accepted = pendingShared(source, "membershipAndAccess")!;
  const receipt = readDoc(takeShared(follower, a.key, accepted))!;
  grantLease(source, leaseClock());
  takeShared(source, b.key, receipt);
  expect(operationOutcome(source, id, refresh.operation)?.status).toBe("committed");
  expect(source.setup?.epoch).toBe(2);
  expect(acceptedMembership(follower)?.successorOrder).toEqual([a.key, c.key, b.key]);
  const agreed = source.shared!;
  const proposal = checkpointedTransition(source, {
    setup: { id, epoch: 3, primary: a.key }, previous: a.key, proposal: true, successorChain: true, standby: b.key, numberCeiling: 4_000,
  }, { ...agreed, setupNames: { sequence: 1, value: { [a.key]: "Discarded" } } })!;
  const lower = readDoc(signDoc(b.sign, "leadership", 3, { ...proposal.body, approval: proposal.envelope.s }))!;
  const higher = checkpointedTransition(follower, {
    setup: { id, epoch: 3, primary: b.key }, previous: a.key, takeover: true, successorChain: true,
  }, follower.shared!)!;
  expect(takeLeadership(source, lower)).toBe(true);
  expect(takeLeadership(follower, higher)).toBe(true);
  expect(source.setup?.role).toBe("primary");
  expect(follower.setup?.role).toBe("primary");

  // These were accepted saves made on A's branch after its epoch certificate.
  source.shared!.membershipAndAccess = { sequence: 2, value: { ...membership, boxes: boxes.slice(0, 2),
    successorOrder: [b.key, a.key], left: [c.key], devices: { phones: [], removed: [{ key: phone.key, pairing }] } } };
  const revoked = publishRevocations(source)!;
  expect(revoked).toBeTruthy();
  const library = { personalities: [], removed: [] };
  const request = docsWork({ link: follower, library }, { key: a.key, sign: a.signer }, { have: digestOf(source) });
  expect(request.send.some(mail => mail.docs.give?.some(envelope => envelope.s === higher.envelope.s))).toBe(true);
  docsWork({ link: follower, library }, { key: a.key, sign: a.signer }, { give: [...leadershipDocuments(lower.envelope), revoked] });
  expect(acceptedMembership(follower)?.left).toEqual([c.key]);
  expect(acceptedMembership(follower)?.devices.phones).toEqual([]);
  const received = docsWork({ link: source, library }, { key: b.key, sign: b.signer }, { give: leadershipDocuments(higher.envelope) });
  expect(received.took.some(doc => doc.kind === "leadership" && doc.h === higher.h)).toBe(true);
  expect(source.setup).toMatchObject({ epoch: 3, primary: b.key, role: "member" });
  expect(source.shared?.setupNames).toBeUndefined();
  expect(acceptedMembership(source)?.left).toEqual([c.key]);
  expect(acceptedMembership(source)?.devices.removed).toEqual([{ key: phone.key, pairing }]);
  expect(takeLeadership(follower, lower)).toBe(false);
  expect(follower.setup?.role).toBe("primary");
});

test("a superseded standby cannot replay its lower-hash acceptance after a ranked takeover", () => {
  upgradeKuvoFolder(K);
  const a = identity(), b = identity(), c = identity(), former = identity();
  const id = newSetupID();
  const boxes = [a, b, c, former].map(box => ({ key: box.key, sign: box.signer, address: box.address, name: "Computer", kind: "machine" }));
  const initial: LinkState = { key: former.seed, sign: former.sign,
    setup: { id, epoch: 1, primary: former.key, sign: former.signer, role: "primary", managed: true, checkpointed: true } };
  const values = { membershipAndAccess: { sequence: 1, value: { boxes, successorOrder: boxes.map(box => box.key) } } };
  const chain = checkpointedTransition(initial, {
    setup: { id, epoch: 2, primary: former.key }, previous: former.key, standby: a.key, successorChain: true, numberCeiling: 2_000,
  }, values)!;
  expect(takeLeadership(initial, chain)).toBe(true);
  const member = (computer: typeof a, state: LinkState): LinkState => ({
    ...structuredClone(state), key: computer.seed, sign: computer.sign, setup: { ...state.setup!, role: "member" },
  });
  const primary = member(a, initial);
  const takeover = checkpointedTransition(primary, {
    setup: { id, epoch: 3, primary: a.key }, previous: former.key, takeover: true, successorChain: true,
  }, values)!;
  expect(takeLeadership(primary, takeover)).toBe(true);
  expect(primary.setup).toMatchObject({ epoch: 3, primary: a.key, role: "primary", successorChain: true });
  expect(primary.setup?.standby).toBeUndefined();
  const stale = member(b, primary);
  const standby = member(c, primary);
  const proposal = (computer: typeof a, numberCeiling: number, supersedes?: string) => checkpointedTransition(primary, {
    setup: { id, epoch: 4, primary: a.key }, previous: a.key, standby: computer.key, standbyAcceptance: true,
    successorChain: true, proposal: true, numberCeiling, ...(supersedes ? { supersedes, assignmentRevision: 1 } : {}),
  }, values)!;

  // Produce genuine signed certificates in the hash order that previously won
  // replay, regardless of the randomly generated computer keys.
  const assignments = Array.from({ length: 16 }, (_, i) => approveLeadership(stale, proposal(b, 4_000 + i))!);
  const earlier = assignments.reduce((lowest, each) => each.h < lowest.h ? each : lowest);
  const replacements = Array.from({ length: 16 }, (_, i) => {
    const request = proposal(c, 6_000 + i, earlier.body.approval as string);
    return { request, certificate: approveLeadership(standby, request)! };
  });
  const { request, certificate: later } = replacements.reduce((highest, each) => each.certificate.h > highest.certificate.h ? each : highest);
  expect(earlier.h < later.h).toBe(true);
  expect(takeLeadership(stale, earlier)).toBe(true);
  expect(takeLeadership(standby, later)).toBe(true);
  primary.leadershipPending = request.envelope;
  expect(takeLeadership(primary, later)).toBe(true);

  grantLease(primary, leaseClock());
  const saved = stageShared(primary, "setupNames", { [a.key]: "Updated name" })!;
  const save = pendingShared(primary, "setupNames")!;
  const staleReceipt = takeShared(stale, a.key, save)!;
  expect(readDoc(staleReceipt)?.origin).toBe(b.signer);
  const library = { personalities: [], removed: [] };
  for (const link of [primary, standby]) {
    const state = { link, library, successorForks: new Map() };
    const replay = docsWork(state, { key: b.key, sign: b.signer }, { give: [...leadershipDocuments(earlier.envelope), staleReceipt] });
    expect(replay.took.some(doc => doc.kind === "leadership")).toBe(false);
    expect(link.leadership?.[4]).toEqual(later.envelope);
    expect(link.setup?.standby?.key).toBe(c.key);
    expect(state.successorForks.has(earlier.h)).toBe(false);
  }
  expect(operationOutcome(primary, id, saved.operation)?.status).toBe("pending");
  const receipt = readDoc(takeShared(standby, a.key, save))!;
  takeShared(primary, c.key, receipt);
  expect(operationOutcome(primary, id, saved.operation)?.status).toBe("committed");
  expect(takeLeadership(stale, later)).toBe(true);
  expect(stale.setup?.standby?.key).toBe(c.key);
});

test("accepted rank changes keep no history after many full-roster reorderings", () => {
  upgradeKuvoFolder(K);
  const [a, b, ...others] = Array.from({ length: 65 }, identity);
  const boxes = [a!, b!, ...others].map(box => ({ key: box.key, sign: box.signer, address: box.address, name: "Computer", kind: "machine" }));
  const id = newSetupID();
  const setup: SetupRecord = { id, epoch: 2, primary: a!.key, sign: a!.signer, role: "primary", managed: true, checkpointed: true, successorChain: true, standby: { key: b!.key, sign: b!.signer } };
  const base = { membershipAndAccess: { sequence: 1, value: { boxes, successorOrder: boxes.map(peer => peer.key) } } };
  const leadership = { 2: signDoc(a!.sign, "leadership", 2, { setup }) };
  const primary: LinkState = { key: a!.seed, sign: a!.sign, setup, leadership, sharedSetup: id, shared: base };
  const standby: LinkState = { ...primary, key: b!.seed, sign: b!.sign, setup: { ...setup, role: "member" } };
  grantLease(primary, leaseClock());
  for (let i = 0; i < 90; i++) {
    const membership = acceptedMembership(primary)!;
    const [first, ...rest] = membership.successorOrder!;
    const save = stageShared(primary, "membershipAndAccess", { ...membership, successorOrder: [...rest, first] });
    expect(save).not.toBeNull();
    const receipt = readDoc(takeShared(standby, a!.key, pendingShared(primary, "membershipAndAccess")!));
    expect(receipt).not.toBeNull();
    takeShared(primary, b!.key, receipt!);
    expect(operationOutcome(primary, id, save!.operation)?.status).toBe("committed");
  }
  expect(primary.shared?.membershipAndAccess?.value).not.toHaveProperty("successorRevisions");
  expect(standby.shared?.membershipAndAccess?.value).not.toHaveProperty("successorRevisions");
  expect(Buffer.byteLength(JSON.stringify(primary.shared))).toBeLessThan(MAX_SHARED_BYTES);
});

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
  expect(readDoc(save.proof)?.n).toBe(1);
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
  expect(readDoc(second.proof)?.n).toBe(readDoc(first.proof)!.n);
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
