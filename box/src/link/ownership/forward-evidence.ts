import path from "node:path";
import fs from "node:fs";
import { locked } from "../../shared/files";
import { RUN } from "../home";
import { isObject, sortedJSON } from "../../shared/json";
import { KEY } from "./record-address";
import { readSubmissionID } from "./submission-address";
import { ownedRecordFormat, readRecord, keepRecord } from "./store";
import { draftKey, draftReplayFence, readForwardEvidence, type ForwardEvidence } from "./forward-evidence-address";
import { moveReceiptKeys } from "./receipt-index";
import { movesReceipt, type ForwardMoveScope } from "./admission";
export type { ForwardMoveScope } from "./admission";

export { draftKey, draftReplayFence, forwardDecision, forwardOriginKey, forwardDecisionKey, readForwardEvidence } from "./forward-evidence-address";
export type { ForwardDecisionKind, ForwardEvidence } from "./forward-evidence-address";

export const MUTATION_EVIDENCE = /^home\/run\/owned-data\/mutations\/([A-Za-z0-9_-]{43})\.json$/;

/** Shape-only legacy claims are opaque. A paired origin record identifies the old shadow. */
export function mutationEvidenceKind(file: string, key: string) {
  const record = JSON.parse(fs.readFileSync(file, "utf8"));
  // Capture preserves unsupported records intact. Restore checks the format before placement.
  if (!isObject(record) || (record.formatVersion !== undefined && record.formatVersion !== 1 && record.formatVersion !== 0))
    return "legacyMutationEvidence";
  if (draftReplayFence(record.draftReplayFence, key)) return "draftSubmissionReplayFences";
  const evidence = readForwardEvidence(record.forward, key);
  if (evidence) return evidence.kind;
  try {
    const sibling = path.join(path.dirname(file), "..", "forward-origins", key + ".json");
    const origin = ownedRecordFormat.read(JSON.parse(fs.readFileSync(sibling, "utf8")));
    if (origin.digest === record.digest && record.answer === undefined) return "forwardedOperationOrigins";
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  return "legacyMutationEvidence";
}

export const FORWARD_EVIDENCE_FEATURE = "data.forward-evidence";

type EvidenceRecord = Record<string, unknown>;
export type MovedForwardEvidence = Record<string, EvidenceRecord>;

function follows(evidence: ForwardEvidence, scope: ForwardMoveScope): boolean {
  return evidence.kind !== "forwardedOperationOrigins" && movesReceipt(evidence.ref, scope);
}

export const MUTATION_LOCK = path.join(RUN, "owned-data", ".mutations.lock");

export function packForwardEvidence(setup: string | undefined, scope: ForwardMoveScope): MovedForwardEvidence | undefined {
  if (!setup) return undefined;
  const indexed = moveReceiptKeys(setup, scope.sessions, scope.items, scope.group);
  const entries = indexed.mutations.flatMap(key => {
    const record = readRecord<EvidenceRecord>("mutations", key)!;
    const evidence = readForwardEvidence(record?.forward, key);
    const draft = draftReplayFence(record?.draftReplayFence, key);
    const belongs = (evidence?.setup === setup && follows(evidence, scope))
      || (draft?.setup === setup && movesReceipt({ kind: "draftSubmissionReplayFences", id: String(draft.session), setup }, scope));
    return belongs ? [[key, ownedRecordFormat.write(record)]] : [];
  });
  // Layouts before the typed shadow stamp still wrote a scoped fence beside
  // each submission. The immutable submission handle supplies its address.
  const stamped = new Set(entries.map(([key]) => key));
  for (const { key, handle: saved, session } of indexed.submissions) {
    const handle = readSubmissionID(saved);
    if (!handle || !movesReceipt({ kind: "draftSubmissionReplayFences", id: session, setup }, scope)) continue;
    const shadow = draftKey(setup, session, handle.id);
    if (stamped.has(shadow)) continue;
    const known = readRecord<EvidenceRecord>("mutations", shadow);
    if (known?.digest !== `scoped:${key}` || known.draftReplayFence !== undefined) continue;
    const fence = draftReplayFence({ setup, session, id: handle.id }, shadow);
    entries.push([shadow, ownedRecordFormat.write({ ...known, draftReplayFence: fence })]);
  }
  return entries.length ? Object.fromEntries(entries) : undefined;
}

export function checkForwardEvidence(value: unknown, setup: string | undefined, scope: ForwardMoveScope): MovedForwardEvidence {
  if (!isObject(value) || !setup) throw new Error("Unreadable forwarded decisions in the move.");
  for (const [key, raw] of Object.entries(value)) {
    const record = ownedRecordFormat.read(raw);
    const evidence = readForwardEvidence(record.forward, key);
    const draft = draftReplayFence(record.draftReplayFence, key);
    if (draft?.setup === setup && movesReceipt({ kind: "draftSubmissionReplayFences", id: String(draft.session), setup }, scope)
      && typeof record.digest === "string" && /^scoped:[A-Za-z0-9_-]{43}$/.test(record.digest) && record.answer === undefined) continue;
    if (!evidence || evidence.setup !== setup || !follows(evidence, scope)
      || typeof record.digest !== "string" || !KEY.test(record.digest))
      throw new Error("The move includes a forwarded decision for another record.");
  }
  return value as MovedForwardEvidence;
}

/** Called before arrival is acknowledged, while the destination is held. A claim lacking
 * an answer remains a durable unknown. A conflicting local record blocks arrival. */
export function placeForwardEvidence(records: MovedForwardEvidence): void {
  locked(MUTATION_LOCK, () => {
    const writes: [string, EvidenceRecord][] = [];
    for (const [key, raw] of Object.entries(records)) {
      const record = ownedRecordFormat.read(raw);
      const existing = readRecord<EvidenceRecord>("mutations", key);
      if (!existing) {
        writes.push([key, record]);
        continue;
      }
      if (sortedJSON(existing) === sortedJSON(record)) continue;
      const { answer, at, ...identity } = record;
      const { answer: prior, at: _priorAt, ...priorIdentity } = existing;
      if (sortedJSON(identity) === sortedJSON(priorIdentity) && (answer === undefined || prior === undefined)) {
        if (answer !== undefined) writes.push([key, record]);
        continue;
      }
      if (record.draftReplayFence && !existing.draftReplayFence && record.digest === existing.digest
        && answer === undefined && prior === undefined) {
        writes.push([key, record]);
        continue;
      }
      throw new Error("A different forwarded decision already exists at the destination.");
    }
    for (const [key, record] of writes) keepRecord("mutations", key, record);
  }, { required: true });
}
