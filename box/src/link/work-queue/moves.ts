// A move's part of the Work Queue (PROTOCOL.md, "kuvo/moves", "The Work Queue"): the queues of the
// conversations a move carries go with them, items and files, under the same owner keys and
// numbers, and are gone from the box they left once the move is over. A conversation carried on as
// another here (a chat changed to Codex) takes its queue along.

import fs from "node:fs";
import path from "node:path";
import { locked, syncDurableFile, writeDurablePrivate } from "../../shared/files";
import { changeMove, type MoveRecord } from "../moves/records";
import { measurePayload, payloadPath, verifyPayload, type FileProof } from "../moves/payload";
import { validQueueFileList } from "../../shared/work-queue-file";
import { offlineEditsReady, recordKey, removeRecord } from "../ownership/store";
import { ownKey } from "../peers/known";
import { readLink } from "../state";
import { isObject } from "../../shared/json";
import { QUEUES_LOCK } from "./migrate";
import { keepLabel, labels } from "./labels";
import { statusNamed } from "./statuses";
import { intoTodo, placeLocalHand } from "./ops";
import { pruneRelations } from "./relations";
import { allItems, changeQueues, Refusal, filesOf, isLiveItem, itemOf, ITEM_ID, migratedToChange, deletionFor, keepDeletion, queueDeletions, OWNER, readKeptQueue, readQueue, FILES, WORK_QUEUE, type Item, type QueueDeletion } from "./store";

/** What a move carries: each owner's items, the labels they have, and which items
 * have files (packed beside `move.json`, in `work-queue/<item>/`). */
export interface CarriedQueues {
  items: Item[];
  labels: { id: string; name: string; color: string }[];
  withFiles: string[];
  deletions?: QueueDeletion[];
}

const QUEUE_MOVE_MARKER = ".kuvo-move-import";

export type QueuePlacement = { items: { id: string; owner: string }[]; deletions: QueueDeletion[] };

export function clearQueuePlacement(payloads: readonly QueuePayload[], move: string, rolledBack?: QueuePlacement): void {
  if (rolledBack) changeQueues(book => {
    for (const { id, owner } of rolledBack.items) {
      const queue = book.queue(owner);
      queue.items = queue.items.filter(item => item.id !== id);
    }
    for (const deletion of rolledBack.deletions) {
      const current = deletionFor(deletion.id);
      if (current?.queue === deletion.queue && current.at === deletion.at) removeRecord("edit-deletions", recordKey(deletion.id));
    }
  }, undefined, { durable: true });
  for (const id of new Set(payloads.map(file => file.relative.split("/")[0]!))) {
    const marker = payloadPath(FILES, `${id}/${QUEUE_MOVE_MARKER}`);
    if (!fs.lstatSync(marker, { throwIfNoEntry: false })?.isFile() || fs.readFileSync(marker, "utf8") !== move) continue;
    if (rolledBack) fs.rmSync(filesOf(id), { recursive: true, force: true });
    else fs.unlinkSync(marker);
  }
}

/** The queues of `owners`, as a move carries them; null for none. */
export function packWorkQueues(owners: readonly string[]): CarriedQueues | null {
  // A numbering write admitted before the move froze must finish before packing.
  fs.mkdirSync(WORK_QUEUE, { recursive: true, mode: 0o700 });
  migratedToChange();
  return locked(QUEUES_LOCK, () => collectWorkQueues(owners), { exclusive: true });
}

export function packQueuesForMove(owners: readonly string[]) {
  try { return { queues: packWorkQueues(owners) }; }
  catch (error) {
    if (error instanceof Refusal) return { refused: error.message };
    throw error;
  }
}

export function checkMoveQueueFiles(owners: readonly string[]): string[] {
  return unavailableQueuePayloads(packWorkQueues(owners));
}

function collectWorkQueues(owners: readonly string[]): CarriedQueues | null {
  // (One of a kind a newer box keeps goes only when it reads whole here: else it stays, as it is.)
  const wanted = new Set(owners);
  // Preserve the corrupt-queue check on the owners being moved.
  for (const owner of wanted) if (OWNER.test(owner)) readQueue(owner, true);
  const here = readLink();
  const own = here && ownKey(here);
  const all = allItems();
  if (own && all.some(item => isLiveItem(item) && item.handing?.key === own && (wanted.has(item.owner) || wanted.has(item.handing.to))))
    throw new Refusal("A Work Queue handoff is still in progress. Try the move again once it finishes.");
  const items = all.filter(item => wanted.has(item.owner));
  for (const owner of wanted) if (!OWNER.test(owner)) items.push(...readKeptQueue(owner, true)?.items ?? []);
  // Older carries could leave a pending handoff on a retained deletion.
  for (const item of items) if (!isLiveItem(item)) delete item.handing;
  const deletions = queueDeletions().filter(deletion => wanted.has(deletion.queue));
  if (!items.length && !deletions.length) return null;
  const travelling = new Set(items.map(item => item.id));
  const split = new Set(items.flatMap(item => (item.relations ?? []).map(link => link.item)).filter(id => !travelling.has(id)));
  pruneRelations(items, split);
  const used = new Set(items.flatMap(item => item.labels));
  return {
    items,
    labels: labels().filter(each => used.has(each.id)),
    withFiles: items.filter(item => item.files.length).map(item => item.id),
    ...(deletions.length ? { deletions } : {}),
  };
}

export interface QueuePayload extends FileProof { relative: string }

/** The item records, including their expected names and sizes, select the portable bytes. */
export function stageFiles(carried: CarriedQueues | undefined, staging: string, limit: number): QueuePayload[] {
  const payloads: QueuePayload[] = [];
  for (const item of carried?.items ?? []) for (const file of item.files) {
    const relative = `${item.id}/${file.name}`;
    const destination = payloadPath(staging, relative);
    const proof = measurePayload(FILES, relative, destination, Math.min(limit, file.size));
    if (proof.size !== file.size || (file.sha256 !== undefined && proof.sha256 !== file.sha256))
      throw new Refusal(`A Work Queue attachment no longer matches its recorded bytes: ${file.name}.`);
    file.sha256 = proof.sha256;
    payloads.push({ relative, ...proof });
    limit -= proof.size;
  }
  return payloads;
}

export const filesSize = (carried: CarriedQueues | undefined): number =>
  (carried?.items ?? []).reduce((sum, item) => sum + item.files.reduce((bytes, file) => bytes + file.size, 0), 0);

export function verifyQueuePayloads(payloads: readonly QueuePayload[]): void {
  for (const entry of payloads) verifyPayload(FILES, entry.relative, entry);
}

export function unavailableQueuePayloads(carried: CarriedQueues | null): string[] {
  const unavailable: string[] = [];
  for (const item of carried?.items ?? []) for (const file of item.files) {
    try {
      const proof = measurePayload(FILES, `${item.id}/${file.name}`, undefined, file.size);
      if (proof.size !== file.size || (file.sha256 !== undefined && proof.sha256 !== file.sha256)) unavailable.push(file.name);
    } catch {
      unavailable.push(file.name);
    }
  }
  return unavailable;
}

/** What came with a move put in place: the items of `owners` it brought (any other owner's left
 * out, and each checked as one read from this box's own files), their files from
 * `opened/work-queue`, and the labels they have (one this box can't keep is left off; Todo for
 * a status it doesn't know). */
export function placeWorkQueues(carried: unknown, owners: readonly string[], opened: string | null, requireProof = false,
  journal?: { id: string; beforePlacement: (payloads: QueuePayload[], placement: QueuePlacement) => void }): QueuePayload[] {
  if (carried === undefined) return [];
  if (!isObject(carried) || !Array.isArray(carried.items)) throw new Error("Invalid carried Work Queue.");
  const queueRoot = opened ? path.join(opened, "work-queue") : null;
  const allowed = new Set(owners);
  const items = carried.items.flatMap(each => (isObject(each) && typeof each.owner === "string" && allowed.has(each.owner) ? [itemOf(each, each.owner)] : [])).filter((each): each is Item => each !== null);
  const deletions = Array.isArray(carried.deletions) ? carried.deletions.flatMap(each => {
    if (!isObject(each) || typeof each.id !== "string" || !ITEM_ID.test(each.id) || typeof each.queue !== "string"
      || !allowed.has(each.queue) || !Number.isSafeInteger(each.at)) throw new Error("A carried deletion record is invalid");
    return [{ id: each.id, queue: each.queue, at: each.at as number }];
  }) : [];
  if (items.length !== carried.items.length) throw new Error("A carried Work Queue item is invalid.");
  const payloads: QueuePayload[] = [];
  const seen = new Set<string>();
  for (const each of carried.items) {
    if (!isObject(each) || !validQueueFileList(each.files)) throw new Error("Invalid carried Work Queue file list.");
  }
  for (const item of items) for (const file of item.files) {
    const relative = `${item.id}/${file.name}`;
    if (seen.has(relative) || !queueRoot || !Number.isSafeInteger(file.size) || file.size < 0 ||
      (requireProof && file.sha256 === undefined)) throw new Error("A carried Work Queue file inventory is invalid.");
    seen.add(relative);
    if (file.sha256 === undefined) {
      const proof = measurePayload(queueRoot, relative, undefined, file.size);
      if (proof.size !== file.size) throw new Error("A carried Work Queue file has the wrong size.");
      file.sha256 = proof.sha256;
    }
    const payload = { relative, size: file.size, sha256: file.sha256 };
    verifyPayload(queueRoot, relative, payload);
    payloads.push(payload);
  }
  if (!items.length && !deletions.length) return [];
  if (deletions.length && !offlineEditsReady()) throw new Error("The destination must enable offline Work Queue edits first");
  const carriedLabels = new Set(items.flatMap(item => item.labels));
  for (const each of Array.isArray(carried.labels) ? carried.labels : []) if (isObject(each) && carriedLabels.has(String(each.id))) keepLabel(each);
  const here = new Set(labels().map(each => each.id));
  for (const item of items) {
    item.labels = item.labels.filter(each => here.has(each));
    if (!statusNamed(item.status)) intoTodo(item, item.status, Date.now());
  }
  changeQueues(book => {
    const ids = new Set(items.map(item => item.id));
    if (ids.size !== items.length || items.some(item => book.find({ id: item.id }) || deletionFor(item.id) ||
        (item.files.length > 0 && fs.lstatSync(filesOf(item.id), { throwIfNoEntry: false })))) {
      throw new Error("A carried Work Queue item or its attachment directory already exists or was deleted here.");
    }
    if (deletions.some(deletion => book.find({ id: deletion.id }) || ids.has(deletion.id) || deletionFor(deletion.id)))
      throw new Error("A carried Work Queue deletion overlaps an item");
    journal?.beforePlacement(payloads, { items: items.map(({ id, owner }) => ({ id, owner })), deletions });
    const staging = opened && path.join(opened, "queue-install");
    for (const item of items) {
      if (item.files.length) {
        const staged = payloadPath(staging!, item.id);
        for (const file of item.files) {
          const relative = `${item.id}/${file.name}`;
          verifyPayload(queueRoot!, relative, { size: file.size, sha256: file.sha256! }, payloadPath(staging!, relative));
        }
        if (journal) writeDurablePrivate(path.join(staged, QUEUE_MOVE_MARKER), journal.id);
        const destination = payloadPath(FILES, item.id);
        fs.mkdirSync(FILES, { recursive: true, mode: 0o700 });
        fs.renameSync(staged, destination);
        syncDurableFile(destination, path.dirname(WORK_QUEUE));
        for (const file of item.files) verifyPayload(FILES, `${item.id}/${file.name}`, { size: file.size, sha256: file.sha256! });
      }
      book.queue(item.owner).items.push(item);
    }
    for (const deletion of deletions) keepDeletion(deletion);
  }, undefined, { durable: true });
  return payloads;
}

/** Record the attachment inventory before placing any of the move's Work Queue files. */
export function placeMoveWorkQueues(id: string, carried: unknown, owners: readonly string[], opened: string | null, requireProof: boolean | undefined,
  sessions: NonNullable<MoveRecord["payloads"]>["sessions"]): void {
  if (carried === undefined) return;
  placeWorkQueues(carried, owners, opened, requireProof, {
    id,
    beforePlacement: (queue, placement) => {
      if (!changeMove(id, each => { each.payloads = { sessions, queue }; each.queuePlacement = placement; }))
        throw new Error("The move journal is unavailable.");
    },
  });
}

/** The queues of `owners` gone from here (their files too): the move took them (nothing changes
 * them while it's under way, `refuseMoving`), or a copy that came is taken back. */
export function forgetWorkQueues(owners: readonly string[]): void {
  const gone = new Set(owners);
  changeQueues(book => {
    for (const owner of gone) {
      if (!OWNER.test(owner) && !readKeptQueue(owner)) continue;
      const queue = book.queue(owner);
      for (const item of queue.items) fs.rmSync(filesOf(item.id), { recursive: true, force: true });
      queue.items = [];
    }
  }, undefined, { onItsOwn: true });
}

/** A conversation carried on as another here: its queue, and its links, under the new one. */
export function carryWorkQueue(from: string, to: string): void {
  if (!OWNER.test(from) || !OWNER.test(to) || from === to) return;
  changeQueues(book => {
    for (const item of [...book.queue(from).items]) {
      if (!isLiveItem(item)) {
        delete item.handing;
        book.give(item, to);
        continue;
      }
      // The carry route may be called again from the new session's metadata.
      if (item.handing) continue;
      placeLocalHand(book, item, to, { user: true }, Date.now(), true);
    }
    for (const deletion of queueDeletions()) if (deletion.queue === from) keepDeletion({ ...deletion, queue: to });
    for (const item of book.items()) {
      for (const link of item.sessions) if (link.session === from) link.session = to;
      if (item.handedBy === from) item.handedBy = to;
    }
  }, undefined, { onItsOwn: true });
}
