// Sandbox prerequisites for Auto and for the vault.

import path from "node:path";
import { agentsInSandbox, engineCommandsInSandbox, fenceWhole } from "../fence/state";
import { IN_TEST_FOLDER, K } from "../home";

/** What's missing for that: the sandbox (`off`), keeping commands from all it hides (`partial`), or
 * the coding agents' own programs in it (`agents`, some started before it still run outside it);
 * null where nothing is. */
export type SandboxGap = "off" | "partial" | "agents";

/** Whether Auto can run on this box now, for a bot on the engine, or on a coding agent
 * (`agents`). A test box (its own folder, never yours, nor the real app's: `run.kuvo.code/box`) can
 * enable it with KUVO_TEST_SANDBOX_ON=1. */
export function sandboxOn(agents = false): boolean {
  return sandboxGap(agents) === null;
}

export function sandboxGap(agents = false): SandboxGap | null {
  const realApp = path.basename(path.dirname(path.resolve(K))) === "run.kuvo.code";
  if (IN_TEST_FOLDER && !realApp && process.env.KUVO_TEST_SANDBOX_ON === "1") return null;
  if (!engineCommandsInSandbox()) return "off";
  if (!fenceWhole()) return "partial";
  return agents && !agentsInSandbox() ? "agents" : null;
}
