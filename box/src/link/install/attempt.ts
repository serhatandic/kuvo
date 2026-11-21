import fs from "node:fs";
import { additiveObjectFormat } from "../../shared/format-version";

export const installAttemptFormat = additiveObjectFormat("Interrupted installation marker");

/** The preceding writer used a bare timestamp. The old reader tests only for
 * presence, so an additive record can remain in place through rollback. */
export function hasInstallAttempt(file: string): boolean {
  let text: string;
  try { text = fs.readFileSync(file, "utf8").trim(); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  if (/^\d+$/.test(text)) return true;
  let value: unknown;
  try { value = JSON.parse(text); } catch {}
  if (typeof installAttemptFormat.read(value).at !== "number") {
    throw new Error("The interrupted installation marker is unreadable.");
  }
  return true;
}
