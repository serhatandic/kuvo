// Connect, on a computer that's away (PROTOCOL.md, "Computers", Managing them): the box a client
// asks tells that peer its agents now and asks for theirs, so an answer marks it back at once
// (`noteHeard`) and none leaves it away. Its own, on every box: no other computer is asked.
//
// The ask is left in `run/reach-again.json` (`{key: when}`), as a machine's routes from a Mac over
// SSH are answered by a process of their own (`box route`) with no relay connection: the box's
// link sends it (this Mac's box's in the same process), and `kuvo/computers` says the computer is
// being reached until it answers or the try is over.

import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { locked, writePrivate } from "../files";
import { RUN } from "../home";

export const REACH_AGAIN = path.join(RUN, "reach-again.json");
const LOCK = path.join(RUN, ".reach-again.lock");

/** How long a try to reach a computer again lasts before it counts as away again. */
export const TRY_FOR = 20_000;

/** How an ask goes at once, where this process sends it (this Mac's box, a machine's link). */
export const reachAgainSoon: { send: () => void } = { send: () => {} };

/** When each computer was last asked to be reached again (ms), by its key. */
export function readTries(file = REACH_AGAIN): Record<string, number> {
  try {
    const kept: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    return isObject(kept) ? Object.fromEntries(Object.entries(kept).filter(([, at]) => typeof at === "number")) as Record<string, number> : {};
  } catch {
    return {};
  }
}

/** Since when the computer `key` is being reached again, or null when no try is under way. */
export function tryingSince(key: string, now = Date.now(), tries = readTries()): number | null {
  const at = tries[key];
  return at !== undefined && now - at >= 0 && now - at < TRY_FOR ? at : null;
}

/** A client asked to reach the computer `key` again: left for the link to send. False when it
 * couldn't be left. */
export function askReachAgain(key: string, now = Date.now()): boolean {
  try {
    fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
    locked(LOCK, () => {
      const tries = readTries();
      // (Those over long ago go.)
      for (const [each, at] of Object.entries(tries)) if (now - at >= TRY_FOR) delete tries[each];
      tries[key] = now;
      writePrivate(REACH_AGAIN, JSON.stringify(tries));
    });
  } catch {
    return false;
  }
  reachAgainSoon.send();
  return true;
}

// Which ask this process sent, by key: each goes once.
const sent = new Map<string, number>();

/** The asks under way, sent by this process's relay connection (`tell`: its agents to that peer
 * alone, asking for theirs), each once. */
export function sendReachAgain(welcomed: boolean, tell: (key: string) => void, now = Date.now()): void {
  if (!welcomed) return;
  for (const [key, at] of Object.entries(readTries())) {
    if (tryingSince(key, now, { [key]: at }) === null || sent.get(key) === at) continue;
    sent.set(key, at);
    try { tell(key); } catch {}
  }
}
