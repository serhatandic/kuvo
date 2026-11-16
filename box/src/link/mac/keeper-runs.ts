// Whether this Mac's keeper runs its box now, and so what the box answers itself of the app's areas
// (its engine's restart). A leaf of its own, so areas ask it without importing the relay.

import fs from "node:fs";
import path from "node:path";
import { OWN_HOME_MAC, RUN } from "../home";

/** Those of the app's areas this box answers itself now, with the app closed too: its engine's
 * restart while the keeper runs this box (`kuvo-start restart` asks it, as a machine's box asks
 * its own), and always in a home of its own, where no app ever runs it. While the app runs the box
 * itself, the app restarts the engine: `bin/kuvo-start` stays once a keeper has run, and would
 * answer 202 with nothing to ask. */
export const boxAnswers = (route: string): boolean => route === "kuvo/engine/restart" && (OWN_HOME_MAC || keeperRuns());

/** Whether this Mac's keeper runs this box now: its record in `run/keeper.json` names the process
 * that started it, alive. (A record a keeper left as it crashed, its ID another's since, isn't.)
 * So the keeper starts the box itself, nothing between them: `MacBoxTests.theBoxIsItsKeepersOwnChild`. */
export function keeperRuns(): boolean {
  let pid = 0;
  try { pid = Number(JSON.parse(fs.readFileSync(path.join(RUN, "keeper.json"), "utf8"))?.pid); } catch { return false; }
  if (!Number.isInteger(pid) || pid <= 0 || pid !== process.ppid) return false;
  // (Another user's process with its ID, after a restart, isn't it: the keeper is always yours.)
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}
