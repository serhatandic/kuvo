// What an earlier Kuvo Code kept in its own files and preferences, brought into the box's own
// files in the staged copy (#713 piece 4; the inventory: `.claude/notes/713-piece-4-inventory.md`).

import type { MoveContext } from "./rewrite";
import type { RunPlan } from "./state";

/** Every upgrade, run in the staged copy: what wasn't carried, in words to show once. */
export function runUpgrades(_run: RunPlan, _move: MoveContext): string[] {
  return [];
}
