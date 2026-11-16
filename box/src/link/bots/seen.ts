// When you last saw each bot here (PROTOCOL.md, "What a bot is doing now", Seen): a client says so
// as it shows you a bot (`POST kuvo/bots/<id>/seen`), and from then on what its sessions did before
// is no longer Done, nor a hard stop Blocked, on any client. Kept in `run/bots-seen/seen.json`
// (`{"bots": {"<id>": <ms>}}`, written whole), in a folder of its own so a stream watches it alone;
// read again only as it changes. A bot that no longer lives here is let go as it's next written.

import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { locked, writePrivate } from "../files";
import { watchFolders } from "../follow";
import { RUN } from "../home";
import { botHere } from "./here";

export const SEEN_FOLDER = path.join(RUN, "bots-seen");
export const SEEN_FILE = path.join(SEEN_FOLDER, "seen.json");
const LOCK = path.join(RUN, ".bots-seen.lock");

/** Told as a bot is seen in this process (another's write is the folder's watch to see). */
export const seenFollowers = new Set<() => void>();

// The file as last read, by what it was then (another process may write it).
let read: { as: string; seen: ReadonlyMap<string, number> } | null = null;

/** When you last saw each bot here (ms), by its id. */
export function readSeen(): ReadonlyMap<string, number> {
  let as = "";
  try {
    const stat = fs.statSync(SEEN_FILE);
    as = `${stat.ino}:${stat.mtimeMs}:${stat.size}`;
  } catch {}
  if (read?.as === as) return read.seen;
  const seen = new Map<string, number>();
  if (as) {
    try {
      const kept: unknown = JSON.parse(fs.readFileSync(SEEN_FILE, "utf8"));
      const bots = isObject(kept) && isObject(kept.bots) ? kept.bots : {};
      for (const [id, at] of Object.entries(bots)) if (/^bot_[A-Za-z0-9]+$/.test(id) && typeof at === "number" && Number.isFinite(at)) seen.set(id, at);
    } catch {}
  }
  read = { as, seen };
  return seen;
}

/** `id` seen at `at` (ms; a time before the one kept leaves that), kept, and those following told. */
export function noteSeen(id: string, at = Date.now()): void {
  locked(LOCK, () => {
    read = null;
    const seen = new Map(readSeen());
    seen.set(id, Math.max(seen.get(id) ?? 0, at));
    for (const each of [...seen.keys()]) if (each !== id && !botHere(each)) seen.delete(each);
    writePrivate(SEEN_FILE, JSON.stringify({ bots: Object.fromEntries(seen) }));
    read = null;
  });
  for (const tell of [...seenFollowers]) try { tell(); } catch {}
}

/** `then` as a bot is seen, here or in another process (a `box route`). How to stop. */
export function followSeen(then: () => void): () => void {
  seenFollowers.add(then);
  const watch = watchFolders([SEEN_FOLDER], then, { recursive: false });
  return () => {
    seenFollowers.delete(then);
    watch.close();
  };
}
