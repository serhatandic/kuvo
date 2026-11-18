// The certificate authorizes a content hash. Persist the verified bytes before recording
// the epoch, so recovery and backups can follow the same reference as replication.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import type { Doc, SignedDoc } from "../../shared/documents";
import { isObject } from "../../shared/json";
import { readDoc, signDoc } from "../documents/format";
import { privateFolder, writeDurablePrivate } from "../files";
import { RUN } from "../home";
import type { LinkState } from "../state";
import { membershipOf } from "./membership";
import { setupOf } from "./record";
import { committedOperations, OPERATION_ID, sharedValues, type OperationOutcomes, type SharedValues } from "./shared";
import { sharedKind } from "../../shared/data-declarations";

export const CHECKPOINT_FEATURE = "setup.checkpoints";
const HASH = /^[\w-]{43}$/;
const OPERATIONS_PER_PAGE = 256;
export const checkpointHash = (doc: Doc) => crypto.createHash("sha256")
  .update(doc.body.archive === true ? "outcomes\0" + JSON.stringify(doc.body) : doc.envelope.d).digest("base64url");
const location = (hash: string) => path.join(RUN, "setup-checkpoints", hash + ".json");

function checkpointFile(hash: unknown): Doc | null {
  if (typeof hash !== "string" || !HASH.test(hash)) return null;
  try {
    const doc = readDoc(JSON.parse(fs.readFileSync(location(hash), "utf8")));
    return doc?.kind === "setup-checkpoint" && checkpointHash(doc) === hash ? doc : null;
  } catch {
    return null;
  }
}

export function preserveCheckpoint(doc: Doc) {
  const hash = checkpointHash(doc);
  if (checkpointFile(hash)) return;
  const file = location(hash);
  privateFolder(path.dirname(file));
  writeDurablePrivate(file, JSON.stringify(doc.envelope), RUN);
}

function outcomesOf(value: unknown, epoch: number): OperationOutcomes | null {
  if (!isObject(value)) return null;
  for (const [operation, outcome] of Object.entries(value)) {
    if (!OPERATION_ID.test(operation) || !isObject(outcome) || !sharedKind(outcome.kind) || outcome.status !== "committed"
      || !Number.isSafeInteger(outcome.epoch) || Number(outcome.epoch) < 1 || Number(outcome.epoch) >= epoch
      || !Number.isSafeInteger(outcome.sequence) || Number(outcome.sequence) < 1
      || outcome.baseRevision !== Number(outcome.sequence) - 1 || typeof outcome.proposal !== "string") return null;
  }
  return value as OperationOutcomes;
}

function archiveRefs(doc: Doc): string[] | null {
  const refs = doc.body.archives;
  return Array.isArray(refs) && refs.every(ref => typeof ref === "string" && HASH.test(ref)) && new Set(refs).size === refs.length ? refs : null;
}

function archivesOf(doc: Doc): Doc[] | null {
  const refs = archiveRefs(doc);
  if (!refs) return null;
  const pages: Doc[] = [];
  for (const ref of refs) {
    const page = checkpointFile(ref);
    if (!page || page.body.archive !== true || page.body.id !== doc.body.id) return null;
    pages.push(page);
  }
  return pages;
}

/** Receive dependencies ahead of the transition. Only a checkpoint signed by an authorized
 * computer at the current position can stage disk writes. The certificate then binds its hash. */
export function cacheCheckpoint(link: LinkState, cache: Map<string, Doc>, doc: Doc): void {
  const setup = link.setup;
  if (!setup || doc.kind !== "setup-checkpoint" || doc.body.id !== setup.id) return;
  const hash = checkpointHash(doc);
  if (doc.body.archive === true) {
    for (const manifest of cache.values()) {
      if (manifest.n === setup.epoch + 1 && (manifest.origin === setup.sign || manifest.origin === setup.standby?.sign)
        && archiveRefs(manifest)?.includes(hash) && outcomesOf(doc.body.outcomes, manifest.n)) {
        preserveCheckpoint(doc);
        return;
      }
    }
    return;
  }
  if (doc.n !== setup.epoch + 1 || (doc.origin !== setup.sign && doc.origin !== setup.standby?.sign) || !archiveRefs(doc)) return;
  if (!cache.has(hash) && cache.size >= 8) cache.delete(cache.keys().next().value!);
  cache.set(hash, doc);
}

export function resolveCheckpoint(transition: Doc, candidate?: Doc) {
  const ref = transition.body.checkpoint;
  const checkpoint = candidate ?? checkpointFile(ref);
  if (!checkpoint || checkpoint.kind !== "setup-checkpoint" || ref !== checkpointHash(checkpoint)
    || checkpoint.n !== transition.n || checkpoint.body.id !== setupOf(transition.body)?.id) return null;
  const shared = sharedValues(checkpoint.body.shared);
  const value = shared?.membershipAndAccess?.value;
  const membership = isObject(value) && !Object.hasOwn(value, "fields") ? membershipOf(value) : null;
  const pages = archivesOf(checkpoint);
  if (!shared || !membership || !pages) return null;
  const outcomes: OperationOutcomes = {};
  for (const page of pages) {
    const entries = outcomesOf(page.body.outcomes, checkpoint.n);
    if (!entries) return null;
    for (const [operation, outcome] of Object.entries(entries)) {
      if (Object.hasOwn(outcomes, operation)) return null;
      outcomes[operation] = outcome;
    }
  }
  return { checkpoint, shared, membership, outcomes };
}

export function checkpointedTransition(link: LinkState, authority: Record<string, unknown>, shared: SharedValues): Doc | null {
  const next = setupOf(authority);
  if (!next || !link.sign) return null;
  // Keep the order stable once an epoch closes, so complete pages are reused by later epochs.
  const entries = Object.entries(committedOperations(link)).sort(([left, a], [right, b]) =>
    a.epoch - b.epoch || a.sequence - b.sequence || (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : left < right ? -1 : 1));
  const archives: string[] = [];
  for (let offset = 0; offset < entries.length; offset += OPERATIONS_PER_PAGE) {
    const page = readDoc(signDoc(link.sign, "setup-checkpoint", 1, {
      archive: true, id: next.id, outcomes: Object.fromEntries(entries.slice(offset, offset + OPERATIONS_PER_PAGE)),
    }));
    if (!page) return null;
    preserveCheckpoint(page);
    archives.push(checkpointHash(page));
  }
  const checkpoint = readDoc(signDoc(link.sign, "setup-checkpoint", next.epoch, { id: next.id, shared, archives }));
  if (!checkpoint) return null;
  const doc = readDoc(signDoc(link.sign, "leadership", next.epoch, {
    ...authority, checkpoint: checkpointHash(checkpoint),
  }));
  if (!doc || !resolveCheckpoint(doc, checkpoint)) return null;
  preserveCheckpoint(checkpoint);
  return doc;
}

/** Send the manifest and its archive before the certificate, including during catch-up. */
export function leadershipDocuments(envelope: SignedDoc): SignedDoc[] {
  const doc = readDoc(envelope);
  if (!doc) return [];
  if (!Object.hasOwn(doc.body, "checkpoint")) return [envelope];
  const checkpoint = checkpointFile(doc.body.checkpoint);
  const pages = checkpoint && archivesOf(checkpoint);
  return checkpoint && pages ? [checkpoint.envelope, ...pages.map(page => page.envelope), envelope] : [];
}
