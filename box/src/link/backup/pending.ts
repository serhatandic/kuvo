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
import { stat } from "./locations";

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
