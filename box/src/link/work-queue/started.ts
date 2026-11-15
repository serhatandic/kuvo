// What was being worked on when the Work Queue's items moved over to statuses (PROTOCOL.md, "The
// Work Queue", "From categories"): before statuses an item was in progress while a session linked
// to it was at work, which the move itself can't see (it runs before the engine answers). So it
// leaves each such item where its category put it and names it (`started` in `model.json`); once
// the box can say who's at work, those whose session is are In progress, and it's never looked at
// again: a status is only ever set from then on.

import { isObject } from "../../shared/json";
import { bridgeTarget, call, readEngineFile } from "../engine/client";
import { readBackground } from "../work/background";
import { forgetStarted, migrateOnce, startedThen } from "./migrate";
import { noteEvent, THE_BOX } from "./ops";
import { changeQueues, placeIn } from "./store";

/** The sessions at work now, for sure: thrown where the engine can't say yet (its file isn't
 * there, or it doesn't answer), or the coding agents' bridge is running and doesn't. (What counts
 * sessions elsewhere takes one that can't be asked for none at work, which here would leave an
 * item out of In progress for good.) A bridge that isn't running has no session at work. */
export async function atWorkForSure(signal?: AbortSignal): Promise<Set<string>> {
  const bridge = bridgeTarget();
  const working = new Set(Object.keys(readBackground()));
  for (const target of [readEngineFile(), ...(bridge ? [bridge] : [])]) {
    const active = await call(target, "GET", "/api/session/active", undefined, signal);
    if (!Array.isArray(active) && !isObject(active)) throw new Error("no answer of who's at work");
    for (const id of Array.isArray(active) ? active : Object.keys(active)) if (typeof id === "string") working.add(id);
  }
  return working;
}

let settled = false;
let settling = false;

/** For a test: as a box just started. */
export function forgetSettled(): void {
  settled = false;
}

/** Looked at once, as soon as `atWork` answers (it throws while the engine can't say: tried again
 * at the next round). */
export async function settleStarted(atWork: () => Promise<Set<string>>, now = Date.now()): Promise<void> {
  if (settled || settling) return;
  settling = true;
  try {
    migrateOnce();
    const ids = new Set(startedThen());
    if (ids.size) {
      const working = await atWork();
      changeQueues(book => {
        for (const item of book.items()) {
          if (!ids.has(item.id) || item.removed || !["todo", "backlog", "waiting"].includes(item.status) || !item.sessions.some(each => working.has(each.session))) continue;
          const from = item.status;
          item.status = "in-progress";
          item.order = placeIn(book.items().filter(each => each.id !== item.id), "in-progress");
          noteEvent(item, { at: now, by: THE_BOX, kind: "moved", from, to: "in-progress" });
        }
      }, now, { onItsOwn: true });
      forgetStarted();
    }
    settled = true;
  } catch {
    // (Not now: the engine isn't answering, or the items aren't moved over yet.)
  } finally {
    settling = false;
  }
}
