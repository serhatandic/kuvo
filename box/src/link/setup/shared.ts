// The standby's durable acceptance commits an operation. Proposals, outcomes and accepted
// values share the link lock. An operation keeps its identity even when its kind is edited again.
import { setupLayoutDeferred } from "../../shared/folder-version";
import crypto from "node:crypto";
import { SHARED_KINDS, sharedKind, sharedPolicy, type SharedKind } from "../../shared/data-declarations";
import type { Doc, SignedDoc } from "../../shared/documents";
import { INTERRUPTED_SAVE, PENDING_SAVE, type Receipt } from "../../shared/routes/setup-save";
import { isObject } from "../../shared/json";
import { held, MOST_DOC, ownSign, readDoc, signDoc } from "../documents/format";
import { K } from "../home";
import { ownKey } from "../peers/known";
import { readLink, updateLink, type LinkState } from "../state";
import { primaryFenced } from "./lease";
import { applyAccess } from "./access";
import { acceptedMembership, membershipOf, projectMembership } from "./membership";
import { followedSetup, requiresStandby } from "./record";
import { sealedPrimary } from "./checkpoint";

export type { Receipt, SharedKind };
export { PENDING_SAVE, INTERRUPTED_SAVE };
export type SharedValues = Partial<Record<SharedKind, { sequence: number; value: unknown }>>;
export type OperationOutcome = {
  kind: SharedKind;
  epoch: number;
  sequence: number;
  baseRevision: number;
  proposal: string;
  status: "committed" | "interrupted";
};
export type OperationOutcomes = Record<string, OperationOutcome>;

export const MAX_SHARED_BYTES = Math.floor(MOST_DOC / 3);
export const OPERATION_ID = /^[A-Za-z0-9_-]{22,43}$/;

export function publishedShared(link: LinkState): SignedDoc[] {
  const setup = followedSetup(link);
  if (!setup || setup.role !== "primary" || setup.id !== link.sharedSetup || !link.sign || primaryFenced(link)) return [];
  if (setup.checkpointed) {
    const values = Object.fromEntries(SHARED_KINDS.filter(kind => sharedPolicy(kind).publish && link.shared?.[kind])
      .map(kind => [kind, link.shared![kind]]));
    return [signDoc(link.sign, "setup-shared", setup.epoch, { id: setup.id, epoch: setup.epoch, replica: values })];
  }
  return SHARED_KINDS.flatMap(kind => {
    if (!sharedPolicy(kind).publish) return [];
    if (!setup.checkpointed && sharedPolicy(kind).checkpointOnly) return [];
    const entry = link.shared?.[kind];
    return entry ? [signDoc(link.sign!, "setup-shared", entry.sequence, {
      id: setup.id, epoch: setup.epoch, kind, value: entry.value, published: true,
    })] : [];
  });
}

export function sharedValues(given: unknown): SharedValues | null {
  if (!isObject(given)) return null;
  for (const [kind, entry] of Object.entries(given)) {
    if (!sharedKind(kind) || !isObject(entry) || !Number.isSafeInteger(entry.sequence)
      || Number(entry.sequence) < 1 || !Object.hasOwn(entry, "value")) return null;
  }
  return given as SharedValues;
}

export function sharedValue(link: LinkState | null | undefined, kind: SharedKind): unknown {
  const setup = link?.setup;
  return setup && !setup.aside && link?.sharedSetup === setup.id ? link.shared?.[kind]?.value : undefined;
}

function scopeShared(link: LinkState, id: string) {
  if (link.sharedSetup === id) return;
  installShared(link, undefined);
  link.sharedSetup = id;
}

function identity(doc: Doc): string | null {
  const { operation, baseRevision } = doc.body;
  // Layout 4 proposals already have unique signed bytes. This also works across mixed-version receipts.
  if (operation === undefined && baseRevision === undefined) return crypto.createHash("sha256").update(doc.envelope.s).digest("base64url");
  return typeof operation === "string" && OPERATION_ID.test(operation) && baseRevision === doc.n - 1 ? operation : null;
}

function pendingEntry(link: LinkState, kind: SharedKind) {
  for (const [key, envelope] of Object.entries(link.sharedPending ?? {})) {
    const doc = held(envelope);
    if (doc?.body.kind === kind) return { key, doc };
  }
  return null;
}

export function pendingShared(link: LinkState | null | undefined, kind: SharedKind): Doc | null {
  return link ? pendingEntry(link, kind)?.doc ?? null : null;
}

export function asSave(doc: Doc, seed: string, status: Receipt["status"] = "pending"): Receipt {
  const id = doc.body.id as string;
  const epoch = doc.body.epoch as number;
  const operation = identity(doc)!;
  // The caller can prove it received an operation without exposing the proposed value or effects.
  const proof = signDoc(seed, "setup-shared", doc.n, {
    id, epoch, operation, kind: doc.body.kind, baseRevision: doc.n - 1, proposal: doc.envelope.s,
  });
  return withSaveOutcome({ setup: id, epoch, operation, proof }, status);
}

/** Bind a receipt to the authenticated caller of a membership operation. */
export function bindSave(save: Receipt, seed: string, binding: { pairedKey: string; pairing: string } | { revokedVoucher: string }): Receipt {
  const ticket = held(save.proof)!;
  return { ...save, proof: signDoc(seed, ticket.kind, ticket.n, { ...ticket.body, ...binding }) };
}

/** Stage under the link lock. Independent kinds proceed independently; one kind is serialized. */
export function stageShared(link: LinkState, kind: SharedKind, value: unknown, effects?: object, requested?: string): Receipt | null {
  const setup = followedSetup(link);
  const operation = requested ?? crypto.randomBytes(16).toString("base64url");
  if (setupLayoutDeferred(K) || !link.sign || setup?.role !== "primary" || setup.sign !== ownSign(link) || (!setup.standby && (!setup.checkpointed || requiresStandby(setup)))
    || link.leadershipPending || !sharedKind(kind) || value === undefined || !OPERATION_ID.test(operation)) return null;
  scopeShared(link, setup.id);
  if (Object.hasOwn(link.sharedOutcomes ?? {}, operation)) return null;
  const pending = pendingShared(link, kind);
  if (pending) {
    return (!requested || identity(pending) === requested) && JSON.stringify(pending.body.value) === JSON.stringify(value)
      && JSON.stringify(pending.body.effects) === JSON.stringify(effects) ? asSave(pending, link.sign) : null;
  }
  if (Object.hasOwn(link.sharedPending ?? {}, operation)) return null;
  const sequence = (link.shared?.[kind]?.sequence ?? 0) + 1;
  if (!Number.isSafeInteger(sequence)) return null;
  const proposed = Object.values(link.sharedPending ?? {}).flatMap(envelope => {
    const doc = held(envelope);
    return doc ? [[doc.body.kind, { sequence: doc.n, value: doc.body.value }]] : [];
  });
  const next = { ...link.shared, ...Object.fromEntries(proposed), [kind]: { sequence, value } };
  if (Buffer.byteLength(JSON.stringify(next)) > MAX_SHARED_BYTES) return null;
  const envelope = signDoc(link.sign, "setup-shared", sequence, {
    id: setup.id, epoch: setup.epoch, kind, value, operation, baseRevision: sequence - 1, ...(effects ? { effects } : {}),
  });
  if (envelope.d.length > MOST_DOC * 4 / 3) return null;
  const doc = held(envelope)!;
  if (setup.standby) link.sharedPending = { ...link.sharedPending, [operation]: envelope };
  else {
    link.shared = next;
    remember(link, doc, operation, "committed");
    if (kind === "membershipAndAccess") applyAccess(link, value, effects);
  }
  return asSave(doc, link.sign, setup.standby ? "pending" : "committed");
}

function remember(link: LinkState, doc: Doc, operation: string, status: OperationOutcome["status"]) {
  link.sharedOutcomes = { ...link.sharedOutcomes, [operation]: {
    kind: doc.body.kind as SharedKind, epoch: doc.body.epoch as number, sequence: doc.n,
    baseRevision: doc.n - 1, proposal: doc.envelope.s, status,
  } };
}

function matchesMembership(link: LinkState, value: unknown): boolean {
  if (!isObject(value) || Object.hasOwn(value, "fields")) return false;
  const roster = membershipOf(value)?.boxes;
  const setup = link.setup!;
  return Boolean(roster?.some(peer => peer.key === setup.primary && peer.sign === setup.sign)
    && (!setup.standby || roster.some(peer => peer.key === setup.standby!.key && peer.sign === setup.standby!.sign)));
}

/** The caller persists these decisions before returning a receipt. */
export function takeShared(link: LinkState, sender: string, doc: Doc): SignedDoc | null {
  const setup = followedSetup(link);
  const { id, epoch, kind, value, receipt } = doc.body;
  if (setupLayoutDeferred(K) || !setup || !link.sign || id !== setup.id || epoch !== setup.epoch) return null;
  if (setup.checkpointed && setup.role === "member" && setup.primary === sender && setup.sign === doc.origin
    && setup.standby?.key !== ownKey(link) && doc.n === epoch && doc.body.replica !== undefined) {
    const replica = sharedValues(doc.body.replica);
    if (!replica || !matchesMembership(link, replica.membershipAndAccess?.value) || Object.keys(replica).some(key => !sharedPolicy(key as SharedKind).publish)) return null;
    scopeShared(link, setup.id);
    const entries = Object.entries(replica) as [SharedKind, NonNullable<SharedValues[SharedKind]>][];
    if (entries.some(([key, entry]) => entry.sequence < (link.shared?.[key]?.sequence ?? 0))) return null;
    if (entries.some(([key, entry]) => entry.sequence > (link.shared?.[key]?.sequence ?? 0))) {
      link.shared = { ...link.shared, ...replica };
      const membership = acceptedMembership(link);
      if (membership) projectMembership(link, membership);
    }
    return null;
  }
  if (!sharedKind(kind)) return null;
  if (doc.body.published === true) {
    if (setup.role === "member" && setup.primary === sender && setup.sign === doc.origin
      && !setup.checkpointed && setup.standby && sharedPolicy(kind).publish && !sharedPolicy(kind).checkpointOnly
      && Object.hasOwn(doc.body, "value")) {
      scopeShared(link, setup.id);
      if (doc.n <= (link.shared?.[kind]?.sequence ?? 0)) return null;
      link.shared = { ...link.shared, [kind]: { sequence: doc.n, value } };
      const membership = kind === "membershipAndAccess" && acceptedMembership(link);
      if (membership) projectMembership(link, membership);
    }
    return null;
  }
  if (!setup.standby) return null;
  if (receipt === undefined) {
    if (setup.standby.key !== ownKey(link) || setup.standby.sign !== ownSign(link)
      || sender !== setup.primary || doc.origin !== setup.sign || !Object.hasOwn(doc.body, "value")) return null;
    scopeShared(link, setup.id);
    const operation = identity(doc);
    if (!operation) return null;
    const answer = () => signDoc(link.sign!, "setup-shared", doc.n, { id, epoch, kind, receipt: doc.envelope.s });
    const outcome = link.sharedOutcomes?.[operation];
    if (outcome) return outcome.status === "committed" && outcome.proposal === doc.envelope.s ? answer() : null;
    const legacyReceipt = held(link.sharedReceipts?.[kind]);
    if (legacyReceipt?.body.receipt === doc.envelope.s) {
      remember(link, doc, operation, "committed");
      return legacyReceipt.envelope;
    }
    if (doc.n !== (link.shared?.[kind]?.sequence ?? 0) + 1
      || (setup.checkpointed && kind === "membershipAndAccess" && !matchesMembership(link, value))) return null;
    const next = { ...link.shared, [kind]: { sequence: doc.n, value } };
    if (Buffer.byteLength(JSON.stringify(next)) > MAX_SHARED_BYTES) return null;
    link.shared = next;
    remember(link, doc, operation, "committed");
    const membership = kind === "membershipAndAccess" && acceptedMembership(link);
    if (membership) projectMembership(link, membership);
    return answer();
  }
  if (setup.role !== "primary" || setup.sign !== ownSign(link) || setup.standby.key !== sender
    || setup.standby.sign !== doc.origin || primaryFenced(link)) return null;
  const pending = pendingEntry(link, kind);
  if (pending?.doc.n !== doc.n || pending.doc.envelope.s !== receipt) return null;
  const operation = identity(pending.doc);
  if (!operation) return null;
  link.shared = { ...link.shared, [kind]: { sequence: doc.n, value: pending.doc.body.value } };
  remember(link, pending.doc, operation, "committed");
  if (kind === "membershipAndAccess") applyAccess(link, pending.doc.body.value, pending.doc.body.effects);
  delete link.sharedPending![pending.key];
  if (!Object.keys(link.sharedPending!).length) delete link.sharedPending;
  return null;
}

export function operationOutcome(link: LinkState | null | undefined, id: string, operation: string) {
  if (!link || link.sharedSetup !== id || !OPERATION_ID.test(operation)) return null;
  const decided = Object.hasOwn(link.sharedOutcomes ?? {}, operation) && link.sharedOutcomes![operation];
  if (decided) return decided;
  const pending = Object.values(link.sharedPending ?? {}).map(held).find(doc => doc && identity(doc) === operation);
  return pending ? { status: "pending" as const, sequence: pending.n, baseRevision: pending.n - 1, epoch: pending.body.epoch, kind: pending.body.kind } : null;
}

/** An accepted epoch transition seals the original primary's outstanding operations. */
export function queryOperation(id: string, operation: string, proof: unknown, expectedKind?: SharedKind) {
  if (setupLayoutDeferred(K)) return null;
  const link = readLink();
  const known = operationOutcome(link, id, operation);
  if (proof === undefined) return !expectedKind || known?.kind === expectedKind ? known : null;
  const ticket = readDoc(proof);
  if (!ticket || ticket.kind !== "setup-shared" || ticket.body.id !== id || identity(ticket) !== operation) return null;
  const { epoch, kind, proposal } = ticket.body;
  if (!Number.isSafeInteger(epoch) || Number(epoch) < 1 || !sharedKind(kind) || (expectedKind && kind !== expectedKind)
    || typeof proposal !== "string" || !/^[\w-]{86}$/.test(proposal)) return null;
  const same = (outcome: OperationOutcome) => outcome.proposal === proposal && outcome.kind === kind
    && outcome.epoch === epoch && outcome.sequence === ticket.n;
  const setup = followedSetup(link);
  if (!link || !setup || setup.id !== id) return null;
  const authority = (state: LinkState) => {
    const position = followedSetup(state);
    if (position?.id !== id) return undefined;
    return epoch === position.epoch ? { sign: position.sign } : sealedPrimary(state, Number(epoch));
  };
  if (known) {
    const pending = "proposal" in known ? null : pendingShared(link, kind);
    const matches = "proposal" in known ? same(known)
      : pending?.envelope.s === proposal && pending.n === ticket.n && pending.body.epoch === epoch;
    return matches && ticket.origin === authority(link)?.sign ? known : null;
  }
  if (Number(epoch) >= setup.epoch) return null;
  let result: OperationOutcome | null = null;
  updateLink(state => {
    if (!state || state.sharedSetup !== id || Number(epoch) >= (state.setup?.epoch ?? 0)) return null;
    const seal = authority(state);
    if (ticket.origin !== seal?.sign) return null;
    const decided = state.sharedOutcomes?.[operation];
    if (decided) {
      result = same(decided) ? decided : null;
      return null;
    }
    if (seal.legacyWitness && ownSign(state) !== seal.legacyWitness) return null;
    result = { epoch: Number(epoch), kind, sequence: ticket.n, baseRevision: ticket.n - 1, proposal, status: "interrupted" };
    state.sharedOutcomes = { ...state.sharedOutcomes, [operation]: result };
    return state;
  }, { durable: true });
  return result;
}

export function withSaveOutcome({ setup, epoch, operation, proof }: Pick<Receipt, "setup" | "epoch" | "operation" | "proof">, status: Receipt["status"]): Receipt {
  const reference = { setup, epoch, operation, proof };
  if (status === "pending") return { ...reference, status, pending: true, message: PENDING_SAVE };
  if (status === "interrupted") return { ...reference, status, message: INTERRUPTED_SAVE };
  return { ...reference, status };
}

export function sharedSaveReply(receipt: Receipt) {
  if (receipt.status === "committed") return { status: 200, value: receipt } as const;
  if (receipt.status === "pending") return { status: 202, value: receipt } as const;
  return { status: 409, value: receipt } as const;
}

export async function awaitSharedSave(save: Receipt | null | undefined) {
  return save ? waitShared(save) : null;
}

export async function waitShared(save: Receipt, timeout = 3_000, signal?: AbortSignal): Promise<Receipt> {
  if (save.status !== "pending") return save;
  const until = Date.now() + timeout;
  while (!signal?.aborted) {
    const link = readLink();
    const sameEpoch = link?.setup?.id === save.setup && link.setup.epoch === save.epoch;
    const outcome = sameEpoch
      ? operationOutcome(link, save.setup, save.operation)
      : queryOperation(save.setup, save.operation, save.proof);
    if (outcome?.status === "committed" || outcome?.status === "interrupted") return withSaveOutcome(save, outcome.status);
    if (!sameEpoch) return withSaveOutcome(save, "interrupted");
    if (Date.now() >= until) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  return save;
}

export function committedOperations(link: LinkState): OperationOutcomes {
  return Object.fromEntries(Object.entries(link.sharedOutcomes ?? {}).filter(([, outcome]) => outcome.status === "committed"));
}

/** Install the winning position. Pending operations absent from its history are interrupted. */
export function installShared(link: LinkState, values: SharedValues | undefined, outcomes?: OperationOutcomes): void {
  if (link.sharedSetup !== link.setup?.id) delete link.sharedOutcomes;
  link.sharedOutcomes = { ...link.sharedOutcomes, ...outcomes };
  for (const envelope of [...Object.values(link.sharedPending ?? {}), ...Object.values(link.sharedInterrupted ?? {})]) {
    const proposal = held(envelope);
    if (!proposal || !sharedKind(proposal.body.kind)) continue;
    const operation = identity(proposal);
    if (!operation || link.sharedOutcomes[operation]) continue;
    const accepted = values?.[proposal.body.kind];
    const legacyMatch = !outcomes && proposal.n === accepted?.sequence
      && JSON.stringify(proposal.body.value) === JSON.stringify(accepted.value);
    remember(link, proposal, operation, legacyMatch ? "committed" : "interrupted");
  }
  if (values && Object.keys(values).length) link.shared = values;
  else delete link.shared;
  delete link.sharedPending;
  delete link.sharedInterrupted;
  delete link.sharedReceipts;
  link.sharedSetup = values ? link.setup?.id : undefined;
}

/** Derive the new value under the link lock, after checking the writer again. */
export function changeShared(kind: SharedKind, change: (value: unknown, link: LinkState) => unknown, operation?: string): Receipt | null {
  let save: Receipt | null = null;
  updateLink(link => {
    if (!link || link.setup?.role !== "primary" || !link.setup.standby) return null;
    save = stageShared(link, kind, change(sharedValue(link, kind), link), undefined, operation);
    return save ? link : null;
  }, { durable: true });
  return save;
}

export function submitShared(kind: SharedKind, value: unknown): Receipt | null {
  return changeShared(kind, () => value);
}
