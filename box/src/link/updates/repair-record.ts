import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";

const repairCommandFormat = additiveObjectFormat("Repair request");

export function checkRepairRequest(args: string[]): void {
  if (args.length === 0) return;
  if (args.length !== 2 || args[0] !== "--format-version" || !/^\d+$/.test(args[1]!)) {
    throw new Error("Use repair with --format-version 1, or omit the version for an older keeper.");
  }
  repairCommandFormat.read({ formatVersion: Number(args[1]) });
}

export const repairRecordFormat = additiveObjectFormat("Repair record");
export const repairReplyFormat = additiveObjectFormat("Repair reply");
export const repairFile = (run: string) => path.join(run, "repair.json");

export function readRepairRecord(run: string) {
  try { return repairRecordFormat.read(JSON.parse(fs.readFileSync(repairFile(run), "utf8"))); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    if (error instanceof UnsupportedFormatVersion) throw error;
    throw new Error("The repair record is unreadable. Kuvo has paused repair.");
  }
}
