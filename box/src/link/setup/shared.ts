// Shared values commit under the main computer's link lock and are published to members.
import { setupLayoutDeferred, setupMergeLayoutReady } from "../../shared/folder-version";
import crypto from "node:crypto";
import { SHARED_KINDS, sharedKind, sharedPolicy, type SharedKind } from "../../shared/data-declarations";
import type { Doc, SignedDoc } from "../../shared/documents";
import { type CommittedReceipt as Receipt } from "../../shared/routes/setup-save";
import { isObject } from "../../shared/json";
import { held, MOST_DOC, ownSign, signDoc } from "../documents/format";
import { K } from "../home";
import { fingerprint, type EditHistory, type EditRecord } from "../ownership/merge";
import { KEY } from "../peers/known";
import { updateLink, type LinkState } from "../state";
import { primaryFenced, relayLease } from "./lease";
import { applyAccess } from "./access";
import { acceptedMembership, membershipOf, projectMembership } from "./membership";
import { followedSetup } from "./record";
import { RELEASE_CHOICE } from "../updates/choice";
import { primaryMutationActor } from "../ownership/admission";

export type { Receipt, SharedKind };

type SharedEntry = { sequence: number; value: unknown; offlineEdits?: EditHistory };
export type SharedValues = Partial<Record<SharedKind, SharedEntry>>;
export type CompanionChanges = Partial<Record<SharedKind, Omit<SharedEntry, "sequence">>>;
const entryOf = (sequence: number, value: unknown, offlineEdits: unknown): SharedEntry =>
  ({ sequence, value, ...(offlineEdits ? { offlineEdits: offlineEdits as EditHistory } : {}) });

// The edit history’s additions track imported data across saves and handover.
const SEED_SOURCE = "setup-seed:";
const SEED_FIELDS = SEED_SOURCE + "fields:";
const SEED_REVISION = /^[A-Za-z0-9_-]{43}$/;
export type SeedSnapshot = { revision: string; fields: Record<string, string> };

export function seedSources(history: EditHistory | undefined): Record<string, SeedSnapshot> | undefined {
  const additions = history?.additions;
  if (!additions || additions[SEED_SOURCE] !== "1") return undefined;
  const sources: Record<string, SeedSnapshot> = {};
  const snapshots = new Map<string, SeedSnapshot>();
  for (const [key, revision] of Object.entries(additions)) {
    const source = key.slice(SEED_SOURCE.length);
    if (!key.startsWith(SEED_SOURCE) || !KEY.test(source) || !SEED_REVISION.test(revision)) continue;
    let snapshot = snapshots.get(revision);
    if (!snapshot) {
      try {
        const fields: unknown = JSON.parse(additions[SEED_FIELDS + revision] ?? "");
        if (!isObject(fields)) continue;
        snapshot = { revision, fields: fields as Record<string, string> };
        snapshots.set(revision, snapshot);
      } catch { continue; }
    }
    sources[source] = snapshot;
  }
  return sources;
}

export function withSeedSources(history: EditHistory | undefined, sources: Record<string, SeedSnapshot>): EditHistory {
  const additions = Object.fromEntries(Object.entries(history?.additions ?? {}).filter(([key]) => !key.startsWith(SEED_SOURCE)));
  additions[SEED_SOURCE] = "1";
  for (const [source, { revision, fields }] of Object.entries(sources)) {
    additions[SEED_SOURCE + source] = revision;
    additions[SEED_FIELDS + revision] ??= JSON.stringify(fields);
  }
  return { versions: [], choices: [], receipts: [], ...history, additions };
}
export type OperationOutcome = {
  kind: SharedKind;
  epoch: number;
  sequence: number;
  baseRevision: number;
  proposal: string;
  status: "committed" | "interrupted";
  offlineEdit?: EditRecord;
  mutationDigest?: string;
  transferRefusal?: string;
};
export type OperationOutcomes = Record<string, OperationOutcome>;

export const MAX_SHARED_BYTES = Math.floor(MOST_DOC / 3);
const MAX_MERGE_DENIAL_BYTES = 16 * 1024;
export class SharedCapacity extends Error {}
export const OPERATION_ID = /^[A-Za-z0-9_-]{22,43}$/;

/** A full setup must still be able to record and deliver a refusal. */
export function sharedFits(values: SharedValues): boolean {
  const { setupMergeDenials, ...ordinary } = values;
  return Buffer.byteLength(JSON.stringify(ordinary)) <= MAX_SHARED_BYTES
    && (!setupMergeDenials || Buffer.byteLength(JSON.stringify(setupMergeDenials)) <= MAX_MERGE_DENIAL_BYTES);
}

/** Compatibility projection for updaters through beta.40. It never authorizes a switch. */
export function publishLegacyRelease(value: { version: string; sha256: string; signed: string; layout?: number; clientProtocol?: number }): boolean {
  let changed = false;
  updateLink(link => {
    const setup = followedSetup(link);
    if (!link || !setup || setup.role !== "primary" || !link.sign) return null;
    scopeShared(link, setup.id);
    const previous = link.shared?.[RELEASE_CHOICE];
    if (isObject(previous?.value) && previous.value.sha256 === value.sha256) return null;
    const sequence = (previous?.sequence ?? 0) + 1;
    const first = isObject(previous?.value) && typeof previous.value.legacyFrom === "number"
      ? previous.value.legacyFrom : sequence;
    const next = { ...link.shared, [RELEASE_CHOICE]: entryOf(sequence, { ...value, legacyFrom: first }, undefined) };
    if (!sharedFits(next)) return null;
    link.shared = next;
    changed = true;
    return link;
  }, { durable: true });
  return changed;
}

/** Older standbys need contiguous proposal numbers. Fill any slots they missed with
 * the current running release so they can catch up without fetching intervening packs. */
export function legacyReleaseDocuments(link: LinkState): SignedDoc[] {
  const setup = followedSetup(link);
  const entry = link.shared?.[RELEASE_CHOICE];
  if (setup?.role !== "primary" || !setup.standby || setup.id !== link.sharedSetup || !entry || !link.sign
    || !isObject(entry.value) || typeof entry.value.legacyFrom !== "number") return [];
  const first = entry.value.legacyFrom;
  if (!Number.isSafeInteger(first) || first < 1 || first > entry.sequence) return [];
  return Array.from({ length: entry.sequence - first + 1 }, (_, offset) => {
    const sequence = first + offset;
    return signDoc(link.sign!, "setup-shared", sequence, {
      id: setup.id, epoch: setup.epoch, kind: RELEASE_CHOICE, value: entry.value,
      operation: fingerprint({ id: setup.id, epoch: setup.epoch, sequence }).slice(0, 32),
      baseRevision: sequence - 1,
    });
  });
}

export function publishedShared(link: LinkState): SignedDoc[] {
  const setup = followedSetup(link);
  if (!setup || setup.role !== "primary" || setup.id !== link.sharedSetup || !link.sign) return [];
  const departing = !!sharedValue(link, "setupDeparture") && !primaryFenced(link, true);
  if (primaryFenced(link) && !departing) {
    // Project the running release to old readers even while ordinary setup saves are fenced.
    const release = link.shared?.[RELEASE_CHOICE];
    if (!release || !isObject(release.value) || typeof release.value.legacyFrom !== "number") return [];
    if (!setup.checkpointed) return [signDoc(link.sign, "setup-shared", release.sequence, {
      id: setup.id, epoch: setup.epoch, kind: RELEASE_CHOICE, value: release.value, published: true,
    })];
    const membership = link.shared?.membershipAndAccess;
    return membership ? [signDoc(link.sign, "setup-shared", setup.epoch, { id: setup.id, epoch: setup.epoch,
      replica: { [RELEASE_CHOICE]: release, membershipAndAccess: membership },
    })] : [];
  }
  if (setup.checkpointed) {
    const values = Object.fromEntries(SHARED_KINDS.filter(kind => sharedPolicy(kind).publish && link.shared?.[kind])
      .map(kind => [kind, link.shared![kind]]));
    return [signDoc(link.sign, "setup-shared", setup.epoch, { id: setup.id, epoch: setup.epoch, relayTerm: relayLease(link)?.term, replica: values })];
  }
  return SHARED_KINDS.flatMap(kind => {
    if (!sharedPolicy(kind).publish) return [];
    if (sharedPolicy(kind).checkpointOnly) return [];
    const entry = link.shared?.[kind];
    return entry ? [signDoc(link.sign!, "setup-shared", entry.sequence, {
      id: setup.id, epoch: setup.epoch, kind, value: entry.value, offlineEdits: entry.offlineEdits, published: true,
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

function saveForProposal(seed: string, id: string, epoch: number, operation: string, kind: SharedKind, sequence: number,
  proposal: string): Receipt {
  // The ticket can identify an operation without exposing its value or effects.
  const proof = signDoc(seed, "setup-shared", sequence, { id, epoch, operation, kind, baseRevision: sequence - 1, proposal });
  return { setup: id, epoch, operation, proof, status: "committed" };
}

/** Retry a holder save using its original precondition and value. The signed setup
 * outcome is the receipt even after later saves have superseded the value. */
export function retrySharedValue(link: LinkState, kind: SharedKind, operation: string, baseRevision: unknown, value: unknown): Receipt | null {
  const setup = followedSetup(link);
  if (!setup || !link.sign || ownSign(link) !== setup.sign) return null;
  const lookup = operationOutcome(link, setup.id, operation);
  if (!lookup || lookup.kind !== kind || lookup.epoch !== setup.epoch || lookup.baseRevision !== baseRevision) return null;
  const digest = fingerprint({ kind, value });
  return lookup.status === "committed" && lookup.mutationDigest === digest
    ? saveForProposal(link.sign, setup.id, setup.epoch, operation, kind, lookup.sequence, lookup.proposal) : null;
}

/** Retry a membership operation whose accepted record still names this action. */
export function removalReceipt(link: LinkState, operation: string): Receipt | null {
  const setup = primaryMutationActor(link);
  if (!setup || !link.sign) return null;
  const result = operationOutcome(link, setup.id, operation);
  return result?.kind === "membershipAndAccess" && result.epoch === setup.epoch && result.status === "committed"
    ? saveForProposal(link.sign, setup.id, result.epoch, operation, result.kind, result.sequence, result.proposal) : null;
}

/** Bind a receipt to the authenticated caller of a membership operation. */
export function bindSave(save: Receipt, seed: string, binding: { pairedKey: string; pairing: string } | { requestingMember: string }): Receipt {
  const ticket = held(save.proof)!;
  return { ...save, proof: signDoc(seed, ticket.kind, ticket.n, { ...ticket.body, ...binding }) };
}

/** Commit under the link lock. */
export function stageShared(link: LinkState, kind: SharedKind, value: unknown, effects?: object, requested?: string, edits?: EditHistory,
  { reportCapacity = false, together }: { reportCapacity?: boolean; together?: CompanionChanges } = {}): Receipt | null {
  const setup = primaryMutationActor(link);
  const operation = requested ?? crypto.randomBytes(16).toString("base64url");
  if (setupLayoutDeferred(K) || !link.sign || !setup || primaryFenced(link, kind === "setupDeparture")
    || link.leadershipPending || link.setupMerge && kind !== "setupDeparture" || !sharedKind(kind) || kind === RELEASE_CHOICE || value === undefined || !OPERATION_ID.test(operation)) return null;
  scopeShared(link, setup.id);
  const companions: SharedValues = {};
  if (together) {
    if (!setupMergeLayoutReady(K) || !setup.checkpointed || kind !== "membershipAndAccess") return null;
    for (const key of Object.keys(together)) {
      if (!sharedKind(key) || key === kind || together[key]?.value === undefined) return null;
      const prior = link.shared?.[key];
      companions[key] = entryOf((prior?.sequence ?? 0) + 1, together[key]!.value, together[key]!.offlineEdits ?? prior?.offlineEdits);
    }
  }
  if (Object.hasOwn(link.sharedOutcomes ?? {}, operation)) return null;
  const previous = link.shared?.[kind];
  const offlineEdit = edits?.receipts.at(-1);
  let offlineEdits = edits ? { ...edits, receipts: [] } : previous?.offlineEdits;
  if (!previous && sharedPolicy(kind).seed && seedSources(offlineEdits) === undefined) offlineEdits = withSeedSources(offlineEdits, {});
  const sequence = (previous?.sequence ?? 0) + 1;
  if (!Number.isSafeInteger(sequence)) return null;
  const next = { ...link.shared, ...companions, [kind]: entryOf(sequence, value, offlineEdits) };
  if (!sharedFits(next)) {
    if (edits || reportCapacity) throw new SharedCapacity("This edit exceeds the setup’s saved preference capacity.");
    return null;
  }
  const proposal = fingerprint({ kind, value, operation });
  link.shared = next;
  link.sharedOutcomes = { ...link.sharedOutcomes, [operation]: {
    kind, epoch: setup.epoch, sequence, baseRevision: sequence - 1, proposal, status: "committed",
    ...(sharedPolicy(kind).localSave && !offlineEdit ? { mutationDigest: fingerprint({ kind, value }) } : {}),
    ...(offlineEdit ? { offlineEdit } : {}),
  } };
  if (kind === "membershipAndAccess") applyAccess(link, value, effects);
  return saveForProposal(link.sign, setup.id, setup.epoch, operation, kind, sequence, proposal);
}

function matchesMembership(link: LinkState, value: unknown): boolean {
  if (!isObject(value) || Object.hasOwn(value, "fields")) return false;
  const roster = membershipOf(value)?.boxes;
  const setup = link.setup!;
  return Boolean(roster?.some(peer => peer.key === setup.primary && peer.sign === setup.sign));
}

/** Copy published values from the main. */
export function takeShared(link: LinkState, sender: string, doc: Doc): void {
  const setup = followedSetup(link);
  const { id, epoch, kind, value } = doc.body;
  if (setupLayoutDeferred(K) || !setup || !link.sign || id !== setup.id || epoch !== setup.epoch) return;
  if (setup.checkpointed && setup.role === "member" && setup.primary === sender && setup.sign === doc.origin
    && doc.n === epoch && doc.body.replica !== undefined) {
    const replica = sharedValues(doc.body.replica);
    if (!replica || !matchesMembership(link, replica.membershipAndAccess?.value) || Object.keys(replica).some(key => !sharedPolicy(key as SharedKind).publish)) return;
    scopeShared(link, setup.id);
    const entries = Object.entries(replica) as [SharedKind, NonNullable<SharedValues[SharedKind]>][];
    if (entries.some(([key, entry]) => entry.sequence < (link.shared?.[key]?.sequence ?? 0))) return;
    if (entries.some(([key, entry]) => entry.sequence > (link.shared?.[key]?.sequence ?? 0))) {
      link.shared = { ...link.shared, ...replica };
      const membership = acceptedMembership(link);
      if (membership) {
        projectMembership(link, membership);
        if (setup.standby && !membership.boxes.some(peer => peer.key === setup.standby?.key)) {
          delete setup.standby;
          delete setup.numberCeiling;
        }
      }
    }
    return;
  }
  if (!sharedKind(kind)) return;
  if (doc.body.published === true) {
    if (setup.role === "member" && setup.primary === sender && setup.sign === doc.origin
      && !setup.checkpointed && sharedPolicy(kind).publish && !sharedPolicy(kind).checkpointOnly
      && Object.hasOwn(doc.body, "value")) {
      scopeShared(link, setup.id);
      if (doc.n <= (link.shared?.[kind]?.sequence ?? 0)) return;
      link.shared = { ...link.shared, [kind]: entryOf(doc.n, value, doc.body.offlineEdits) };
      const membership = kind === "membershipAndAccess" && acceptedMembership(link);
      if (membership) projectMembership(link, membership);
    }
    return;
  }
}

export function operationOutcome(link: LinkState | null | undefined, id: string, operation: string): OperationOutcome | null {
  return link?.sharedSetup === id && Object.hasOwn(link.sharedOutcomes ?? {}, operation) ? link.sharedOutcomes![operation] : null;
}

export function sharedSaveReply(value: Receipt) {
  return { status: 200, value } as const;
}

export function committedOperations(link: LinkState): OperationOutcomes {
  return Object.fromEntries(Object.entries(link.sharedOutcomes ?? {}).filter(([, outcome]) => outcome.status === "committed"));
}

/** Install the winning position. */
export function installShared(link: LinkState, values: SharedValues | undefined, outcomes?: OperationOutcomes): void {
  if (link.sharedSetup !== link.setup?.id) delete link.sharedOutcomes;
  link.sharedOutcomes = { ...link.sharedOutcomes, ...outcomes };
  if (values && Object.keys(values).length) link.shared = values;
  else delete link.shared;
  link.sharedSetup = values ? link.setup?.id : undefined;
}

/** Derive the new value under the link lock, after checking the writer again. */
export function changeShared(kind: SharedKind, change: (value: unknown, link: LinkState) => unknown, operation?: string): Receipt | null {
  let save: Receipt | null = null;
  updateLink(link => {
    if (!link || !primaryMutationActor(link)) return null;
    save = stageShared(link, kind, change(sharedValue(link, kind), link), undefined, operation);
    return save ? link : null;
  }, { durable: true });
  return save;
}

export function submitShared(kind: SharedKind, value: unknown): Receipt | null {
  return changeShared(kind, () => value);
}
