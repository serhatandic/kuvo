// The standby's receipt is the commit point. Both boxes keep their side of the
// exchange in link.json so a relay outage or process restart only delays delivery.
import { DATA_DECLARATIONS } from "../../shared/data-declarations";
import type { Doc, SignedDoc } from "../../shared/documents";
import { isObject } from "../../shared/json";
import { held, MOST_DOC, ownSign, signDoc } from "../documents/format";
import { ownKey } from "../peers/known";
import { readLink, updateLink, type LinkState } from "../state";
import { primaryFenced } from "./lease";
import { applyAccess } from "./access";
import { followedSetup } from "./record";

type PrimaryKind = {
  [K in keyof typeof DATA_DECLARATIONS]: typeof DATA_DECLARATIONS[K]["owner"] extends "primary" ? K : never
}[keyof typeof DATA_DECLARATIONS];
export type SharedKind = Exclude<PrimaryKind, "setupLeadership" | "setupReplica">;
export type SharedValues = Partial<Record<SharedKind, { sequence: number; value: unknown }>>;

// Leadership also carries a compatible roster, device list and number ledger.
export const MAX_SHARED_BYTES = Math.floor(MOST_DOC / 3);
export const PENDING_SAVE = "Pending until the standby is back.";
const published = new Set<SharedKind>(["setupTimeZone", "setupPersonalities", "queueLabels", "setupUsageDisplay", "setupSecurityAndReleaseChoice"]);

export function publishedShared(link: LinkState): SignedDoc[] {
  const setup = followedSetup(link);
  if (!setup || setup.role !== "primary" || setup.id !== link.sharedSetup || !link.sign || primaryFenced(link)) return [];
  return [...published].flatMap(kind => {
    const entry = link.shared?.[kind];
    return entry ? [signDoc(link.sign!, "setup-shared", entry.sequence, {
      id: setup.id, epoch: setup.epoch, kind, value: entry.value, published: true,
    })] : [];
  });
}

function sharedKind(kind: unknown): kind is SharedKind {
  return typeof kind === "string" && Object.hasOwn(DATA_DECLARATIONS, kind)
    && DATA_DECLARATIONS[kind as PrimaryKind]?.owner === "primary"
    && kind !== "setupLeadership" && kind !== "setupReplica";
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

export function pendingShared(link: LinkState, kind: SharedKind): Doc | null {
  return held(link.sharedPending?.[kind]);
}

/** Stage under the link lock, returning the sequence callers can wait for. Only one
 * unreceipted value of each kind is allowed. Independent kinds proceed independently. */
export function stageShared(link: LinkState, kind: SharedKind, value: unknown): number | null {
  const setup = followedSetup(link);
  if (!link.sign || setup?.role !== "primary" || setup.sign !== ownSign(link) || !setup.standby
    || link.leadershipPending || !sharedKind(kind)) return null;
  scopeShared(link, setup.id);
  const pending = pendingShared(link, kind);
  if (pending) return JSON.stringify(pending.body.value) === JSON.stringify(value) ? pending.n : null;
  const current = link.shared?.[kind];
  if (current && JSON.stringify(current.value) === JSON.stringify(value)) return current.sequence;
  const sequence = (current?.sequence ?? 0) + 1;
  if (!Number.isSafeInteger(sequence) || value === undefined) return null;
  // Leave room in the signed leadership certificate for its roster and other authority data.
  const proposed = Object.fromEntries(Object.entries(link.sharedPending ?? {}).map(([key, envelope]) => {
    const doc = held(envelope)!;
    return [key, { sequence: doc.n, value: doc.body.value }];
  }));
  const next = { ...link.shared, ...proposed, [kind]: { sequence, value } };
  if (Buffer.byteLength(JSON.stringify(next)) > MAX_SHARED_BYTES) return null;
  link.sharedPending = { ...link.sharedPending,
    [kind]: signDoc(link.sign, "setup-shared", sequence, { id: setup.id, epoch: setup.epoch, kind, value }) };
  return sequence;
}

/** Called under the same link lock as installing a leadership certificate. The caller
 * must fsync that write before letting the receipt out. */
export function takeShared(link: LinkState, sender: string, doc: Doc): SignedDoc | null {
  const setup = followedSetup(link);
  const { id, epoch, kind, value, receipt } = doc.body;
  if (!setup?.standby || !link.sign || id !== setup.id || epoch !== setup.epoch || !sharedKind(kind)) return null;
  scopeShared(link, setup.id);
  const current = link.shared?.[kind];
  if (doc.body.published === true) {
    if (setup.role === "member" && setup.primary === sender && setup.sign === doc.origin
      && published.has(kind) && Object.hasOwn(doc.body, "value") && doc.n > (current?.sequence ?? 0)) {
      link.shared = { ...link.shared, [kind]: { sequence: doc.n, value } };
    }
    return null;
  }
  if (receipt === undefined) {
    if (setup.standby.key !== ownKey(link) || setup.standby.sign !== ownSign(link)
      || sender !== setup.primary || doc.origin !== setup.sign || !Object.hasOwn(doc.body, "value")) return null;
    const last = link.sharedReceipts?.[kind];
    if (doc.n === current?.sequence) return held(last)?.body.receipt === doc.envelope.s ? last! : null;
    if (doc.n !== (current?.sequence ?? 0) + 1) return null;
    const next = { ...link.shared, [kind]: { sequence: doc.n, value } };
    if (Buffer.byteLength(JSON.stringify(next)) > MAX_SHARED_BYTES) return null;
    const answer = signDoc(link.sign, "setup-shared", doc.n, { id, epoch, kind, receipt: doc.envelope.s });
    link.shared = next;
    link.sharedReceipts = { ...link.sharedReceipts, [kind]: answer };
    return answer;
  }
  if (setup.role !== "primary" || setup.sign !== ownSign(link) || setup.standby.key !== sender
    || setup.standby.sign !== doc.origin || primaryFenced(link)) return null;
  const pending = pendingShared(link, kind);
  if (pending?.n !== doc.n || pending.envelope.s !== receipt) return null;
  link.shared = { ...link.shared, [kind]: { sequence: doc.n, value: pending.body.value } };
  if (kind === "membershipAndAccess") applyAccess(link, pending.body.value);
  delete link.sharedPending![kind];
  if (!Object.keys(link.sharedPending!).length) delete link.sharedPending;
  return null;
}

export async function waitShared(kind: SharedKind, id: string, epoch: number, sequence: number) {
  const until = Date.now() + 3_000;
  while (Date.now() < until) {
    const link = readLink();
    if (link?.setup?.id !== id || link.setup.epoch !== epoch) break;
    if ((link.shared?.[kind]?.sequence ?? 0) >= sequence) return "saved";
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  return "pending";
}

/** A new accepted epoch inherits the winning values. Receipts from the previous
 * standby cannot commit anything in it. */
export function installShared(link: LinkState, doc: Doc | undefined): void {
  const values = sharedValues(doc?.body.shared);
  if (values && Object.keys(values).length) link.shared = values;
  else delete link.shared;
  for (const [key, envelope] of Object.entries(link.sharedPending ?? {})) {
    if (!sharedKind(key)) continue;
    const proposal = held(envelope);
    const accepted = values?.[key];
    if (proposal?.n === accepted?.sequence && JSON.stringify(proposal?.body.value) === JSON.stringify(accepted?.value)) continue;
    link.sharedInterrupted = { ...link.sharedInterrupted, [key]: envelope };
  }
  delete link.sharedPending;
  delete link.sharedReceipts;
  link.sharedSetup = values ? link.setup?.id : undefined;
}

export interface SharedSave { id: string; epoch: number; sequence: number }

/** Derive the new value under the link lock, after checking the writer again. */
export function changeShared(kind: SharedKind, change: (value: unknown, link: LinkState) => unknown): SharedSave | null {
  let sequence: number | null = null;
  const link = updateLink(link => {
    if (!link) return null;
    if (link.setup?.role !== "primary" || !link.setup.standby) return null;
    const value = change(sharedValue(link, kind), link);
    sequence = stageShared(link, kind, value);
    return sequence === null ? null : link;
  }, { durable: true });
  return link?.setup && sequence !== null ? { id: link.setup.id, epoch: link.setup.epoch, sequence } : null;
}

export function submitShared(kind: SharedKind, value: unknown): SharedSave | null {
  return changeShared(kind, () => value);
}
