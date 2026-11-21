import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";

export const SERVED_FILE = "served.json";
export const servedFormat = additiveObjectFormat("Legacy Mac start marker");

export function readServed(run: string) {
  try { return servedFormat.read(JSON.parse(fs.readFileSync(path.join(run, SERVED_FILE), "utf8"))); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    if (error instanceof UnsupportedFormatVersion) throw error;
    throw new Error("The old Mac Box start marker is unreadable. Kuvo has paused the move.");
  }
}
