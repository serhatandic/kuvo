// The box's own processes that serve clients (`run`, the link to your phones and other computers;
// `serve`, a Mac's server, which Kuvo Helper keeps running; `serve --kept`, a machine's, which its
// keeper keeps) kept to the box's code as an update leaves it (PROTOCOL.md, "Updates"): none holds a
// turn, so each goes once the files it runs changed under it, and what keeps it running starts it
// again on the new ones. Clients reconnect. Nothing an update does stops them, and none of them says
// whether an update is well (its judge does: updates/health.ts).

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { APP_FOLDER, K, REAL_BOX } from "../home";
import { boxVersion, isUpdating } from "./update";

/** The box's code a process runs on: its version (`bin/box-version`) and its link's SHA-256
 * (`bin/kuvo-link.js`), null for one that can't be read. */
export interface CodeInPlace {
  version: string | null;
  link: string | null;
}

export const boxLink = (root = K): string => path.join(root, "bin", "kuvo-link.js");

export function codeInPlace(root = K): CodeInPlace {
  let link: string | null = null;
  try { link = crypto.createHash("sha256").update(fs.readFileSync(boxLink(root))).digest("hex"); } catch {}
  return { version: boxVersion(), link };
}

/** The code in place as this process loaded it: what it runs. */
const AT_LOAD = codeInPlace();

/** What a process that started on `started` does now: "leave" once the code in place isn't what it
 * started on any more, no update being under way (one under way changes it as it goes, and may put
 * it back); otherwise "stay". `leaves`: whether it may go (one started again from the box's files).
 * The rest: a test's. */
export function codeCheck(
  started: CodeInPlace,
  { leaves = true, updating = isUpdating, now = codeInPlace }: { leaves?: boolean; updating?: () => boolean; now?: () => CodeInPlace } = {},
): "leave" | "stay" {
  if (!leaves || updating()) return "stay";
  const files = now();
  // (A file missing for a moment, as one is swapped in: not a change yet.)
  return files.version && files.link && (files.version !== started.version || files.link !== started.link) ? "leave" : "stay";
}

/** Whether a process running `script` is started again from the box's files once it goes: the box's
 * own `bin/kuvo-link.js`, never an app's own box (its app starts that). */
export const runsBoxCode = (script: string): boolean => !APP_FOLDER && path.resolve(script) === path.resolve(boxLink());

/** This process watched for new code in place: `leave` once (said why first), every 30 seconds (a
 * test's box as often as it says). Only where `script` is the box's own (`runsBoxCode`). */
export function leaveForNewCode(script: string, leave: () => void): void {
  if (!runsBoxCode(script)) return;
  const started = AT_LOAD;
  const every = !REAL_BOX && Number(process.env.KUVO_TEST_CODE_EVERY) > 0 ? Number(process.env.KUVO_TEST_CODE_EVERY) : 30_000;
  const timer = setInterval(() => {
    if (codeCheck(started) !== "leave") return;
    clearInterval(timer);
    console.error("Kuvo's files changed under it (an update): it goes, to start again on the new ones.");
    leave();
  }, every);
  timer.unref?.();
}
