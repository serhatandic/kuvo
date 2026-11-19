// A move's part of the Work Queue (PROTOCOL.md, "kuvo/moves", "The Work Queue"): the queues of the
// conversations a move carries go with them, items and files, under the same owner keys and
// numbers, and are gone from the box they left once the move is over. A conversation carried on as
// another here (a chat changed to Codex) takes its queue along.

import fs from "node:fs";
import path from "node:path";
import { writeDurablePrivate } from "../../shared/files";
import { offlineEditsReady } from "../ownership/store";
import { assignmentFor } from "../ownership/resolve";
import { ownKey } from "../peers/known";
import { readLink } from "../state";
import { isObject } from "../../shared/json";
import { keepLabel, labels } from "./labels";
import { statusNamed } from "./statuses";
import { intoTodo, stageLocalHand } from "./ops";
import { pruneRelations } from "./relations";
import { allItems, changeQueues, FILE_NAME, Refusal, filesOf, isLiveItem, itemOf, ITEM_ID, deletionFor, keepDeletion, queueDeletions, OWNER, readKeptQueue, readQueueSettingAside, WORK_QUEUE, type Item, type QueueDeletion } from "./store";

/** What a move carries: each owner's items, the labels they have, and which items
 * have files (packed beside `move.json`, in `work-queue/<item>/`). */
export interface CarriedQueues {
  items: Item[];
  labels: { id: string; name: string; color: string }[];
  withFiles: string[];
  deletions?: QueueDeletion[];
}

/** The queues of `owners`, as a move carries them; null for none. */
export function packWorkQueues(owners: readonly string[]): CarriedQueues | null {
  // (One of a kind a newer box keeps goes only when it reads whole here: else it stays, as it is.)
  const wanted = new Set(owners);
  // Preserve the corrupt-queue check on the owners being moved.
  for (const owner of wanted) if (OWNER.test(owner)) readQueueSettingAside(owner);
  const here = readLink();
  const own = here && ownKey(here);
  const all = allItems();
  if (own && all.some(item => isLiveItem(item) && item.handing?.key === own && (wanted.has(item.owner) || wanted.has(item.handing.to))))
    throw new Refusal("A Work Queue handoff is still in progress. Try the move again once it finishes.");
  const items = all.filter(item => wanted.has(item.owner));
  for (const owner of wanted) if (!OWNER.test(owner)) items.push(...readKeptQueue(owner)?.items ?? []);
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

/** The files of what's carried, copied into `staging/work-queue`, for the bundle. */
export function stageFiles(carried: CarriedQueues | undefined, staging: string): boolean {
  if (!carried?.withFiles.length) return false;
  for (const id of carried.withFiles) fs.cpSync(filesOf(id), path.join(staging, "work-queue", id), { recursive: true });
  return true;
}

/** The bytes the carried files come to. */
export const filesSize = (carried: CarriedQueues | undefined): number => (carried?.withFiles ?? []).reduce((sum, id) => {
  try {
    return sum + fs.readdirSync(filesOf(id)).reduce((each, name) => each + fs.statSync(path.join(filesOf(id), name)).size, 0);
  } catch {
    return sum;
  }
}, 0);

/** What came with a move put in place: the items of `owners` it brought (any other owner's left
 * out, and each checked as one read from this box's own files), their files from
 * `opened/work-queue`, and the labels they have (one this box can't keep is left off; Todo for
 * a status it doesn't know). */
export function placeWorkQueues(carried: unknown, owners: readonly string[], opened: string | null): void {
  if (!isObject(carried) || !Array.isArray(carried.items)) return;
  const allowed = new Set(owners);
  const items = carried.items.flatMap(each => (isObject(each) && typeof each.owner === "string" && allowed.has(each.owner) ? [itemOf(each, each.owner)] : [])).filter((each): each is Item => each !== null);
  const deletions = Array.isArray(carried.deletions) ? carried.deletions.flatMap(each => {
    if (!isObject(each) || typeof each.id !== "string" || !ITEM_ID.test(each.id) || typeof each.queue !== "string"
      || !allowed.has(each.queue) || !Number.isSafeInteger(each.at)) throw new Error("A carried deletion record is invalid");
    return [{ id: each.id, queue: each.queue, at: each.at as number }];
  }) : [];
  if (!items.length && !deletions.length) return;
  if (deletions.length && !offlineEditsReady()) throw new Error("The destination must enable offline Work Queue edits first");
  const carriedLabels = new Set(items.flatMap(item => item.labels));
  for (const each of Array.isArray(carried.labels) ? carried.labels : []) if (isObject(each) && carriedLabels.has(String(each.id))) keepLabel(each);
  const here = new Set(labels().map(each => each.id));
  for (const item of items) {
    item.labels = item.labels.filter(each => here.has(each));
    if (!statusNamed(item.status)) intoTodo(item, item.status, Date.now());
  }
  changeQueues(book => {
    if (deletions.some(deletion => book.find({ id: deletion.id }) || items.some(item => item.id === deletion.id)))
      throw new Error("A carried Work Queue deletion overlaps an item");
    for (const deletion of deletions) keepDeletion(deletion);
    for (const item of items) {
      // (Into a queue of a kind a newer box keeps here that this one can't read whole: refused, so
      // the move fails and the box it comes from keeps it.)
      const before = book.find({ id: item.id });
      if (before || deletionFor(item.id)) throw new Error("A carried Work Queue item already exists or was deleted here");
      book.queue(item.owner).items.push(item);
      const from = opened ? path.join(opened, "work-queue", item.id) : null;
      for (const file of item.files) {
        if (!from || !FILE_NAME.test(file.name)) throw new Error("A carried Work Queue file is missing");
        // Never follow an attachment symlink, even if the unpacked file was replaced.
        const handle = fs.openSync(path.join(from, file.name), fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0) | (fs.constants.O_NONBLOCK ?? 0));
        try {
          if (!fs.fstatSync(handle).isFile()) throw new Error("A carried Work Queue attachment is not a file");
          writeDurablePrivate(path.join(filesOf(item.id), file.name), fs.readFileSync(handle), path.dirname(WORK_QUEUE));
        } finally {
          fs.closeSync(handle);
        }
      }
    }
  }, undefined, { durable: true });
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
  const link = readLink(), now = Date.now();
  changeQueues(book => {
    for (const item of [...book.queue(from).items]) {
      if (!isLiveItem(item)) {
        delete item.handing;
        book.give(item, to);
        continue;
      }
      // The carry route may be called again from the new session's metadata.
      if (item.handing) continue;
      const tracked = assignmentFor(link, { kind: "queueItems", id: item.id, queue: from });
      if (!tracked || !stageLocalHand(item, to, { user: true }, now, link, true)) book.give(item, to);
    }
    for (const deletion of queueDeletions()) if (deletion.queue === from) keepDeletion({ ...deletion, queue: to });
    for (const item of book.items()) {
      for (const link of item.sessions) if (link.session === from) link.session = to;
      if (item.handedBy === from) item.handedBy = to;
    }
  }, undefined, { onItsOwn: true });
}
