import { recoveredQueueDecision } from "../work-queue/recovery";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { changeQueue, isWaitingID, queueFiles, readQueues, readWaitingFile, waitingFile, writeWaitingFile } from "../../shared/queues";
import { queueFollowers } from "../engine/queue";
import { peerPlaces } from "../engine/client";
import { writeDurablePrivate } from "../files";
import { K, RUN } from "../home";
import { placeOfSession } from "../letters/letter";
import { DATA_ID, ownerOf } from "../ownership/resolve";
import { BACKUP, backupLock } from "./engine";
import { POINT } from "./destination";
import { restoreStatus } from "./restore";
import { outputPath, stat } from "./locations";
import { recordKey } from "../ownership/store";
import { MUTATION_ID } from "../ownership/mutation";
import { changeQueues, filesOf, ITEM_ID, itemOf, OWNER, type Item } from "../work-queue/store";
import { tellQueuesAgain } from "../work-queue/across";

function heldQueues() {
  const kept = restoreStatus();
  if (!isObject(kept) || kept.state !== "restored" || typeof kept.id !== "string" || !POINT.test(kept.id)) return null;
  return { id: kept.id, files: queueFiles(path.join(BACKUP, "recovered", kept.id, "home", "run")) };
}

export function pendingAfterRestore() {
  const held = heldQueues();
  if (!held) return [];
  return Object.entries(readQueues(held.files, true)).flatMap(([session, state]) =>
    (state.waiting ?? []).map(message => ({ session, ...message, paused: true, review: true,
      available: Boolean(stat(waitingFile(held.files, message.id))) })));
}

/** Return only the selected, confirmed message to the native paused queue. A durable review
 * receipt fences retries even if the user later runs it and the engine removes the payload. */
export function returnPending(session: string, id: string) {
  return backupLock(async () => {
    if (!DATA_ID.test(session) || !isWaitingID(id)) throw new Error("Choose one held message.");
    const held = heldQueues();
    const message = held && readQueues(held.files, true)[session]?.waiting?.find(each => each.id === id);
    if (!held || !message) throw new Error("That message is unavailable in this restore point.");
    const review = path.join(BACKUP, "reviewed", held.id, id + ".json");
    if (stat(review)) {
      const receipt: unknown = JSON.parse(fs.readFileSync(review, "utf8"));
      if (isObject(receipt) && receipt.session === session && receipt.state === "queued") return { paused: true, reviewed: true };
      throw new Error("Recovery stopped while queuing that message. Check the current conversation before deciding whether to send again.");
    }
    const saved = readWaitingFile(waitingFile(held.files, id));
    if (!saved || saved.id !== id || typeof saved.text !== "string") throw new Error("That message's payload is unavailable.");
    const owner = ownerOf({ kind: "sessionQueue", id: session }, { localRecord: true });
    if (!owner?.local || !await placeOfSession(session, peerPlaces().targets, 3_000) ||
      ownerOf({ kind: "sessionQueue", id: session }, { localRecord: true })?.local !== true) {
      throw new Error("Open the current owner’s conversation before recovering its pending message.");
    }
    const files = queueFiles(RUN);
    const payloadFile = waitingFile(files, id);
    // A restored path may point at a file absent on this replacement. Require a fresh
    // attachment in that case rather than submit it blind.
    if (Array.isArray(saved.files) && saved.files.some(file =>
      !isObject(file) || typeof file.uri !== "string" || !file.uri.startsWith("data:"))) {
      throw new Error("Reattach the files before sending this held message from the conversation.");
    }
    const existing = readQueues(files, true)[session]?.waiting?.some(each => each.id === id);
    if (!existing && fs.existsSync(payloadFile)) throw new Error("That message ID is already present. Check the current queue.");
    writeDurablePrivate(review, JSON.stringify({ session, id, state: "reviewing" }), K);
    if (!existing) {
      changeQueue(files, session, current => {
        if (current.waiting?.some(each => each.id === id)) return { ...current, paused: true };
        if (stat(payloadFile)) throw new Error("That message ID is already present. Check the current queue.");
        writeWaitingFile(payloadFile, {
          id, text: saved.text as string, delivery: "queue",
          ...(Array.isArray(saved.files) ? { files: saved.files } : {}),
        }, RUN);
        return { ...current, paused: true, waiting: [...(current.waiting ?? []), message] };
      }, () => { for (const tell of queueFollowers) tell(); }, true);
    }
    writeDurablePrivate(review, JSON.stringify({ session, id, state: "queued" }), K);
    return { paused: true, reviewed: true };
  });
}

type ItemReview = { id: string; operation: string; decision: "keep" | "discard"; status: "committed" };

function recoveredItems() {
  const held = heldQueues();
  if (!held) return null;
  const root = path.join(BACKUP, "recovered", held.id);
  return { point: held.id, root, items: path.join(root, "work-queue-items") };
}

function readItem(file: string): Item | null {
  if (!stat(file)?.isFile()) return null;
  const value: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
  return isObject(value) && typeof value.owner === "string" && OWNER.test(value.owner) ? itemOf(value, value.owner) : null;
}

const reviewFile = (point: string, id: string) => path.join(BACKUP, "reviewed", point, "work-queue-" + recordKey(id) + ".json");

export function pendingQueueItems() {
  const held = recoveredItems();
  if (!held || !stat(held.items)?.isDirectory()) return [];
  return fs.readdirSync(held.items).flatMap(name => {
    if (!/^[A-Za-z0-9_-]{43}\.json$/.test(name)) return [];
    const item = readItem(path.join(held.items, name));
    if (!item || stat(reviewFile(held.point, item.id))) return [];
    const { handing, ...kept } = item;
    return [{ id: item.id, queue: item.owner, title: item.title, body: item.body,
      canKeep: recoveredQueueDecision({ ...kept }, item.owner, undefined) === "active",
      files: item.files.map(file => ({ name: file.name, available: Boolean(stat(outputPath(held.root,
        ["home", "run", "work-queue", "files", item.id, file.name]))?.isFile()) })),
    }];
  });
}

export function settleQueueItem(id: string, operation: string, decision: unknown) {
  return backupLock(async () => {
    if (!ITEM_ID.test(id) || !MUTATION_ID.test(operation) || (decision !== "keep" && decision !== "discard")) {
      throw new Error("Choose a held item, an operation id and whether to keep or discard it.");
    }
    const held = recoveredItems();
    const item = held && readItem(path.join(held.items, recordKey(id) + ".json"));
    if (!held || !item || item.id !== id) throw new Error("That item is unavailable in this restore point.");
    const file = reviewFile(held.point, id);
    const receipt: ItemReview = { id, operation, decision, status: "committed" };
    const previous: unknown = stat(file) ? JSON.parse(fs.readFileSync(file, "utf8")) : undefined;
    if (previous) {
      if (isObject(previous) && previous.operation === operation && previous.decision === decision) return receipt;
      throw new Error("This item was already reviewed.");
    }
    const marker = recordKey(held.point, id, operation);
    changeQueues(book => {
      const existing = book.find({ id });
      if (existing?.restoreReview === marker && decision === "keep") return;
      if (existing?.restoreReview) throw new Error("This item was already kept under another review.");
      if (decision === "discard") return;
      const { handing: _cancelHandoff, ...kept } = item;
      if (existing || recoveredQueueDecision({ ...kept }, item.owner, undefined) !== "active") {
        throw new Error("The item’s owner or deletion conflicts with keeping this copy.");
      }
      kept.files = kept.files.filter(attachment => {
        const saved = outputPath(held.root, ["home", "run", "work-queue", "files", id, attachment.name]);
        if (!stat(saved)?.isFile()) return false;
        const bytes = fs.readFileSync(saved);
        const target = outputPath(filesOf(id), [attachment.name]);
        if (!stat(target)) writeDurablePrivate(target, bytes, K);
        else if (!stat(target)?.isFile() || !fs.readFileSync(target).equals(bytes)) throw new Error("A live file already uses that attachment name.");
        return true;
      });
      book.queue(item.owner).items.push({ ...kept, restoreReview: marker });
    }, Date.now(), { durable: true });
    writeDurablePrivate(file, JSON.stringify(receipt), K);
    if (decision === "keep") tellQueuesAgain();
    return receipt;
  });
}
