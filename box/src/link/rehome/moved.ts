// Whether this Mac's box has moved out of the old Kuvo Code app's folder (#333, #713), as its
// move's record says: from then on the old app is out of date and in charge of nothing here.
// Platform: a Mac's alone (a Linux box never had the old app).

import fs from "node:fs";
import path from "node:path";
import { IS_MAC, K } from "../home";
import { readRecord, rehomeFolder } from "./state";

let moved: { stamp: string; is: boolean } | null = null;

/** Whether this Mac's box moved out of the old app's folder to a home of its own (#333, #713: its
 * move's record says it's done). From then on this box is this Mac's Kuvo: the old app, opened
 * again (a login item it left, a menu that opens it by its bundle ID), is out of date and never in
 * charge of anything here (`linkRoute` refuses it), so it can't stop what this box does itself
 * (adding machines, keeping them up to date, Update Now). Read again only when the record changes. */
export function movedFromOldApp(): boolean {
  if (!IS_MAC) return false;
  const file = path.join(rehomeFolder(K), "state.json");
  let stamp = "none";
  try {
    const seen = fs.statSync(file);
    stamp = `${seen.mtimeMs}:${seen.size}`;
  } catch {}
  if (moved?.stamp === stamp) return moved.is;
  const record = readRecord(K);
  moved = { stamp, is: record.state === "done" || (typeof record.movedAt === "number" && record.state !== "back") };
  return moved.is;
}
