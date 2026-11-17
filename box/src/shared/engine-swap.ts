// A new engine taking the old one's place without cutting a turn (PROTOCOL.md, "A new engine"): the
// state of that change as the box's swapper (`kuvo-link.js engine-swap`, link/updates/engine-swap.ts)
// keeps it, in `run/engine-swap.json`, for every part of the box that sends into a conversation on
// the engine (the link's processes, the coding agents' bridge for a group chat's seats) to tell
// whether what it sends waits on the box meanwhile, and for clients to be told in words.

import fs from "node:fs";
import path from "node:path";
import { processRuns } from "./platform";

/** Where the change stands: `waiting` for a quiet moment (nothing held); `holding` new prompts while
 * the turns under way end; `swapping` (the old engine stopped, the new one starting); `back` (the new
 * one didn't start, and the one before is started again). */
export type SwapStage = "waiting" | "holding" | "swapping" | "back";

export interface EngineSwap {
  from: string;
  to: string;
  /** When the swapper first saw the new engine waiting (ms). */
  since: number;
  stage: SwapStage;
  /** When it began holding (ms), while it holds. */
  holding?: number;
  /** When it last let go of holding without swapping (ms): it holds again an hour on. */
  letGo?: number;
  /** The swapper's process. */
  pid: number;
  /** What keeps it waiting that holding wouldn't outlast: a terminal open, or a question waiting on you. */
  blocked?: "terminal" | "ask";
}

/** How long a hold lasts at most before it's let go of (as the coding agents' bridge's renewal), and
 * how long a swap's start may take on top of that (a slow migration) before nothing is held whatever
 * the file says: holding is always bounded, even by a swapper that went. */
export const HOLD_MOST = 30 * 60_000;
export const SWAP_MOST = 30 * 60_000;

export const swapFile = (run: string) => path.join(run, "engine-swap.json");

export function readSwap(run: string): EngineSwap | null {
  try {
    const said = JSON.parse(fs.readFileSync(swapFile(run), "utf8"));
    if (typeof said?.from !== "string" || typeof said.to !== "string" || typeof said.pid !== "number" || !["waiting", "holding", "swapping", "back"].includes(said.stage)) return null;
    return said as EngineSwap;
  } catch {
    return null;
  }
}

/** The swap under way, while its swapper runs (one that went holds nothing). */
export function swapUnderWay(run: string): EngineSwap | null {
  const swap = readSwap(run);
  return swap && processRuns(swap.pid, "kuvo-link") ? swap : null;
}

/** Whether what's sent into a conversation on the engine waits on the box now: while a swapper that
 * runs holds, swaps or goes back, and never longer than `HOLD_MOST` and `SWAP_MOST` from when it
 * began holding. */
export function swapHolds(run: string, now = Date.now()): boolean {
  const swap = swapUnderWay(run);
  if (!swap || swap.stage === "waiting") return false;
  return typeof swap.holding === "number" && now - swap.holding < HOLD_MOST + SWAP_MOST;
}

/** What a client shows while an engine change waits or goes, in one plain line (null: none under way). */
export function swapLine(swap: EngineSwap | null): string | null {
  if (!swap) return null;
  switch (swap.stage) {
    case "waiting": return swap.blocked === "terminal" ? "An update to Kuvo’s engine is ready. It installs once nothing is running: close the open terminals to let it."
      : swap.blocked === "ask" ? "An update to Kuvo’s engine is ready. It installs once nothing is running: answer what’s waiting on you to let it."
      : "An update to Kuvo’s engine is ready. It installs once nothing is running.";
    case "holding": return "Kuvo’s engine is updating once the running sessions finish. New messages wait and go right after.";
    case "swapping": return "Kuvo’s engine is updating. New messages wait and go once it’s back.";
    case "back": return "The engine update didn’t start, so Kuvo is going back to the engine it had. New messages go once it’s back.";
  }
}

/** Waits while the swap holds (a group chat's seat, before it sends to the engine): bounded as the
 * hold is. `every`: how often it looks. */
export async function untilSwapLets(run: string, every = 1000): Promise<void> {
  while (swapHolds(run)) await new Promise(resolve => setTimeout(resolve, every));
}
