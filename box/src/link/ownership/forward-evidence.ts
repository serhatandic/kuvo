import path from "node:path";
import fs from "node:fs";
import { dataOwner } from "../../shared/data-declarations";
import { locked } from "../../shared/files";
import { RUN } from "../home";
import { isObject, sortedJSON } from "../../shared/json";
import { KEY } from "../peers/known";
import { SETUP_ID } from "../setup/record";
import { readSubmitter, type Submitter } from "./authority";
import { readSubmissionID, submissionReference } from "./submission-address";
import type { DataReference } from "./resolve";
import { ownedRecordFormat, recordKey, records, readRecord, keepRecord } from "./store";
import type { Act } from "../work-queue/forward";

const decisions = {
  primary: "forwardedPrimaryDecisions",
  queue: "forwardedQueueDecisions",
  "session computer": "forwardedSessionDecisions",
  "group host": "forwardedGroupDecisions",
  "local computer": "forwardedComputerDecisions",
} as const;

export type ForwardDecisionKind = typeof decisions[keyof typeof decisions];

export interface ForwardEvidence {
  kind: ForwardDecisionKind | "forwardedOperationOrigins";
  setup: string;
  operation: string;
  owner: string;
  listedBy?: string;
  submitter: Submitter;
  area: NonNullable<Act["area"]> | "queue";
  record: string;
  ref?: DataReference;
}

export const draftKey = (setup: string, session: string, id: string) => recordKey("typed-draft", setup, session, id);

export function draftReplayFence(value: unknown, key: string) {
  if (value === undefined) return undefined;
  if (!isObject(value) || typeof value.setup !== "string" || !SETUP_ID.test(value.setup)
    || typeof value.session !== "string" || !/^ses_[A-Za-z0-9_-]{1,95}$/.test(value.session)
    || typeof value.id !== "string" || !/^[A-Za-z0-9_-]{1,80}$/.test(value.id)
    || key !== draftKey(value.setup, value.session, value.id))
    throw new Error("Invalid draft replay fence.");
  return value;
}

const keyOf = (kind: ForwardEvidence["kind"], setup: string, operation: string) =>
  recordKey(kind === "forwardedOperationOrigins" ? "forward-origin" : "scoped-forward", setup, operation);

export const forwardOriginKey = (setup: string, operation: string) => keyOf("forwardedOperationOrigins", setup, operation);
export const forwardDecisionKey = (setup: string, operation: string) => keyOf("forwardedComputerDecisions", setup, operation);

export function forwardDecision(ref: DataReference | undefined): ForwardDecisionKind {
  if (ref?.kind === "queueItems") return decisions.queue;
  const owner = ref && dataOwner(ref.kind, ref.queue);
  return owner === "primary" || owner === "session computer" || owner === "group host" ? decisions[owner] : decisions["local computer"];
}

export function readForwardEvidence(value: unknown, key: string): ForwardEvidence | undefined {
  if (value === undefined) return undefined;
  if (!isObject(value) || typeof value.setup !== "string" || !SETUP_ID.test(value.setup)
    || typeof value.operation !== "string" || !/^[A-Za-z0-9_-]{1,80}$/.test(value.operation)
    || typeof value.owner !== "string" || !KEY.test(value.owner) || !readSubmitter(value.submitter)
    || (value.listedBy !== undefined && (typeof value.listedBy !== "string" || !KEY.test(value.listedBy)))
    || typeof value.area !== "string" || typeof value.record !== "string" || !value.record
    || (value.kind !== "forwardedOperationOrigins" && !Object.values(decisions).some(kind => kind === value.kind)))
    throw new Error("Invalid forwarded operation evidence.");
  const evidence = value as unknown as ForwardEvidence;
  if (key !== keyOf(evidence.kind, evidence.setup, evidence.operation)) throw new Error("Forwarded operation evidence has the wrong address.");
  if (evidence.ref !== undefined) {
    const ref = evidence.ref;
    if (!isObject(ref) || typeof ref.kind !== "string" ||
        !["queueItems", "queueLabels", "groupSettings", "sessionQueue", "sessionSchedules", "setupNames", "setupTimeZone",
          "setupPersonalities", "membershipAndAccess", "clientIdentity", "setupLeadership"].includes(ref.kind)
        || (ref.id !== undefined && (typeof ref.id !== "string" || !/^[A-Za-z0-9_-]{1,100}$/.test(ref.id)))
        || (ref.queue !== undefined && (typeof ref.queue !== "string" || !/^[A-Za-z0-9_-]{1,100}$/.test(ref.queue))))
      throw new Error("Invalid forwarded operation record address.");
  }
  if (evidence.kind !== "forwardedOperationOrigins" && evidence.kind !== forwardDecision(evidence.ref))
    throw new Error("Forwarded operation evidence has the wrong owner policy.");
  return evidence;
}

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

export interface ForwardMoveScope {
  sessions: readonly string[];
  group?: string;
  items: ReadonlySet<string>;
}

function follows(evidence: ForwardEvidence, scope: ForwardMoveScope): boolean {
  const ref = evidence.ref;
  if (!ref) return false;
  if (evidence.kind === decisions.queue)
    return ref.id !== undefined && scope.items.has(ref.id);
  if (evidence.kind === decisions["session computer"])
    return ref.id !== undefined && scope.sessions.includes(ref.id);
  return evidence.kind === decisions["group host"] && ref.id === scope.group;
}

export const MUTATION_LOCK = path.join(RUN, "owned-data", ".mutations.lock");

export function packForwardEvidence(setup: string | undefined, scope: ForwardMoveScope): MovedForwardEvidence | undefined {
  if (!setup) return undefined;
  const entries = records("mutations").flatMap(key => {
    const record = readRecord<EvidenceRecord>("mutations", key)!;
    const evidence = readForwardEvidence(record?.forward, key);
    const draft = draftReplayFence(record?.draftReplayFence, key);
    const belongs = (evidence?.setup === setup && follows(evidence, scope))
      || (draft?.setup === setup && scope.sessions.includes(String(draft.session)));
    return belongs ? [[key, ownedRecordFormat.write(record)]] : [];
  });
  // Layouts before the typed shadow stamp still wrote a scoped fence beside
  // each submission. The immutable submission handle supplies its address.
  for (const key of records("submissions")) {
    const saved = readRecord<EvidenceRecord>("submissions", key)?.submission;
    const ref = submissionReference(saved, key);
    if (ref?.setup !== setup || !ref.id || !scope.sessions.includes(ref.id)) continue;
    const handle = isObject(saved) && readSubmissionID(saved.id);
    if (!handle) continue;
    const shadow = draftKey(setup, ref.id, handle.id);
    const known = readRecord<EvidenceRecord>("mutations", shadow);
    if (known?.digest !== `scoped:${key}` || known.draftReplayFence !== undefined) continue;
    const fence = draftReplayFence({ setup, session: ref.id, id: handle.id }, shadow);
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
    if (draft?.setup === setup && scope.sessions.includes(String(draft.session))
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
