// What each session wrote to another with send_message lately (`run/work/sent.json`), so the box
// doesn't tell a lead the same again (#768): a session that sends its starter its report and then
// finishes saying it doesn't wake the lead a second time with it. Noted by whichever process runs
// the tool (a tool's `box route` too), read where its finish is told.

import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { locked, writePrivate } from "../files";
import { WORK_FOLDER } from "./changes";

export const WORK_SENT = path.join(WORK_FOLDER, "sent.json");
const LOCK = path.join(WORK_FOLDER, ".sent.lock");
// How much of each is kept, how many of each session's, of how many sessions, and for how long:
// a turn's, which is minutes or hours.
const KEPT_TEXT = 8000;
const EACH = 4;
const SESSIONS = 100;
const KEPT_FOR = 86_400_000;
/** How long what a reply says must be to count as said already when what was sent holds it (in
 * characters): "Done." isn't the report a message carried. */
export const SAID_ALREADY = 200;

interface Sent {
  to: string;
  text: string;
  at: number;
}

// Words as they're compared: whitespace aside, as a finish reads a reply.
const plain = (text: string) => text.replace(/\s+/g, " ").trim();

function readSent(file: string): Record<string, Sent[]> {
  try {
    const kept = JSON.parse(fs.readFileSync(file, "utf8"));
    if (!isObject(kept)) return {};
    return Object.fromEntries(Object.entries(kept).map(([session, list]) => [session, (Array.isArray(list) ? list : []).filter((each): each is Sent => isObject(each) && typeof each.to === "string" && typeof each.text === "string" && typeof each.at === "number")]));
  } catch {
    return {};
  }
}

/** `session` wrote `text` to `to` (send_message), now. */
export function noteSent(session: string, to: string, text: string, now = Date.now(), file = WORK_SENT): void {
  const words = plain(text).slice(0, KEPT_TEXT);
  if (!words) return;
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    locked(LOCK, () => {
      const all = readSent(file);
      const its = [...(all[session] ?? []).filter(each => now - each.at < KEPT_FOR), { to, text: words, at: now }].slice(-EACH);
      // Last, as the latest: the oldest are let go first.
      delete all[session];
      all[session] = its;
      const ids = Object.keys(all);
      for (const id of ids.slice(0, Math.max(0, ids.length - SESSIONS))) delete all[id];
      writePrivate(file, JSON.stringify(all));
    });
  } catch {}
}

/** Whether `session` has already written `to` what `reply` says, since `since` (ms): the same words,
 * or a long enough reply that's part of what it sent. */
export function saidAlready(session: string, to: string, reply: string, since: number, file = WORK_SENT): boolean {
  const words = plain(reply);
  if (!words) return false;
  return (readSent(file)[session] ?? []).some(each => each.to === to && each.at >= since && (each.text === words.slice(0, KEPT_TEXT) || (words.length >= SAID_ALREADY && each.text.includes(words.slice(0, KEPT_TEXT)))));
}
