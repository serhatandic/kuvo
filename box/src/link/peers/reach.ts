// Whether each of your other computers answers now (`run/peer-reach.json`, PROTOCOL.md, "Peers"): the
// relay says a box is unreachable when something sent to it finds nothing connected there, and any
// mail from it says it's back. Kept in a file, not in memory, as a machine's tools are answered by a
// process of their own (`box route`) that has no relay connection to hear it on.

import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { locked, writePrivate } from "../files";
import { RUN } from "../home";
import { readLink } from "../state";
import { peersOf } from "./known";

export const PEER_REACH = path.join(RUN, "peer-reach.json");
const LOCK = path.join(RUN, ".peer-reach.lock");

/** Since when each box that doesn't answer hasn't (ms), by its key; a box that answers isn't in it. */
export function readReach(file = PEER_REACH): Record<string, number> {
  try {
    const kept: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    return isObject(kept) ? Object.fromEntries(Object.entries(kept).filter(([, at]) => typeof at === "number")) as Record<string, number> : {};
  } catch {
    return {};
  }
}

const change = (edit: (reach: Record<string, number>) => boolean, file = PEER_REACH): void => {
  try {
    fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
    locked(LOCK, () => {
      const reach = readReach(file);
      if (edit(reach)) writePrivate(file, JSON.stringify(reach));
    });
  } catch {}
};

/** When the relay last found each box that doesn't answer not there (ms), by its key: as recent as
 * a few minutes, so a box that was itself off the relay a while doesn't take one that was away
 * when it left to be away all that time (the Work Queue's numbers, `work-queue/numbers.ts`). */
export const PEER_CHECKED = path.join(RUN, "peer-reach-checked.json");
const CHECKED_EVERY = 300_000;

/** Told in this process each time the relay finds a peer not there, by its key (a conversation
 * waiting on it answers what it kept at once: `session-follow.ts`). */
export const unreachableFollowers = new Set<(key: string) => void>();

/** The relay found nothing connected at `address`: the peer there (if it's one) isn't answering, from now. */
export function noteUnreachable(address: Buffer, now = Date.now()): void {
  const peer = peersOf(readLink()).find(each => each.address === address.toString("base64url"));
  if (!peer) return;
  for (const follower of unreachableFollowers) follower(peer.key);
  if (now - (readReach(PEER_CHECKED)[peer.key] ?? 0) > CHECKED_EVERY) change(checked => {
    checked[peer.key] = now;
    return true;
  }, PEER_CHECKED);
  if (readReach()[peer.key] !== undefined) return;
  let away = false;
  change(reach => {
    if (reach[peer.key] !== undefined) return false;
    reach[peer.key] = now;
    away = true;
    return true;
  });
  if (away) console.error(JSON.stringify({ at: new Date(now).toISOString(), pid: process.pid, event: "relay peer unreachable", peer: peer.key }));
}

/** Mail from the box `key`: it answers. True when it didn't before. (Read first, so mail that comes
 * again and again writes nothing.) */
export function noteHeard(key: string): boolean {
  if (readReach(PEER_CHECKED)[key] !== undefined) change(checked => delete checked[key], PEER_CHECKED);
  if (readReach()[key] === undefined) return false;
  let back = false;
  change(reach => {
    if (reach[key] === undefined) return false;
    delete reach[key];
    back = true;
    return true;
  });
  if (back) console.error(JSON.stringify({ at: new Date().toISOString(), pid: process.pid, event: "relay peer reachable", peer: key }));
  return back;
}

/** Since when the box `key` hasn't answered, or null when nothing says it doesn't. */
export const awaySince = (key: string, reach = readReach()): number | null => reach[key] ?? null;

/** When the relay last found the box `key` not there, or null. */
export const lastFoundAway = (key: string, checked = readReach(PEER_CHECKED)): number | null => checked[key] ?? null;

/** A peer found away by the relay or unheard from for three normal mail intervals. */
export function peerIsAway(key: string, heard: number | undefined, reach = readReach(), now = Date.now()): boolean {
  return awaySince(key, reach) !== null || (heard !== undefined && now - heard > 30 * 60_000);
}
