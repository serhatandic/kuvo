// A new engine taking the old one's place without cutting a turn (PROTOCOL.md, "A new engine"): the
// state of that change as the box's swapper (`kuvo-link.js engine-swap`, link/updates/engine-swap.ts)
// keeps it, in `run/engine-swap.json`, for every part of the box that sends into a conversation on
// the engine (the link's processes, the coding agents' bridge for a group chat's seats) to tell
// whether what it sends waits on the box meanwhile, and for clients to be told in words.

import fs from "node:fs";
import path from "node:path";
import { readMachineJob } from "./machine-job";
import { alive, processRuns } from "./platform";

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
  /** The machine’s first restart under Kuvo Helper. */
  machineHelper?: true;
  /** What keeps it waiting that holding wouldn't outlast: a terminal open, or a question waiting on you. */
  blocked?: "terminal" | "ask";
}

/** How long a hold lasts at most before it's let go of (as the coding agents' bridge's renewal), and
 * how long an ordinary swap's start may take on top of that (a slow migration). A saved machine
 * launch replacement keeps its hold while recovery is still pending. */
export const HOLD_MOST = 30 * 60_000;
export const SWAP_MOST = 30 * 60_000;

export const swapFile = (run: string) => path.join(run, "engine-swap.json");

export const installHoldFile = (run: string) => path.join(run, "update-engine-hold.json");

export function readSwap(run: string, file = swapFile(run)): EngineSwap | null {
  try {
    const said = JSON.parse(fs.readFileSync(file, "utf8"));
    if (typeof said?.from !== "string" || typeof said.to !== "string" || typeof said.pid !== "number" || !["waiting", "holding", "swapping", "back"].includes(said.stage)) return null;
    return said as EngineSwap;
  } catch {
    return null;
  }
}

function machineRecovery(run: string, swap: EngineSwap): boolean {
  if (!swap.machineHelper || typeof swap.holding !== "number") return false;
  return !!readMachineJob(run)?.version;
}

/** The swap under way, including a machine whose launch replacement still needs recovery. */
export function swapUnderWay(run: string): EngineSwap | null {
  const install = readSwap(run, installHoldFile(run));
  if (install && alive(install.pid)) {
    try {
      if (Number(fs.readFileSync(path.join(run, ".updating", "pid"), "utf8")) === install.pid) return install;
    } catch {}
  }
  const swap = readSwap(run);
  if (!swap) return null;
  if (processRuns(swap.pid, "kuvo-link")) return swap;
  if (machineRecovery(run, swap)) return { ...swap, stage: "back" };
  return null;
}

/** Holds last until a machine's launch recovery restores service. Ordinary engine changes hold
 * for at most `HOLD_MOST` and `SWAP_MOST` from when they began holding. */
export function swapHolds(run: string, now = Date.now()): boolean {
  const swap = swapUnderWay(run);
  if (!swap || swap.stage === "waiting") return false;
  return machineRecovery(run, swap) || (typeof swap.holding === "number" && now - swap.holding < HOLD_MOST + SWAP_MOST);
}

/** What a client shows while an engine change waits or goes, in one plain line (null: none under way). */
export function swapLine(swap: EngineSwap | null): string | null {
  if (!swap) return null;
  if (swap.machineHelper) {
    if (swap.stage === "waiting") return "Kuvo will move to Kuvo Helper once the running sessions finish.";
    return "Kuvo is moving to Kuvo Helper. New messages wait and go once it’s ready.";
  }
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
