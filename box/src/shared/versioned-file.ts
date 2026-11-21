import fs from "node:fs";
import type { additiveObjectFormat } from "./format-version";
import { UnsupportedFormatVersion } from "./format-version";

/** An absent file denotes no record. Keep an unreadable record for recovery. */
export function readVersionedFile<T>(file: string, format: ReturnType<typeof additiveObjectFormat>): T | null {
  try { return format.read(JSON.parse(fs.readFileSync(file, "utf8"))) as T; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    if (error instanceof UnsupportedFormatVersion) throw error;
    throw new Error(`Kuvo cannot read ${file}; its bytes were kept.`, { cause: error });
  }
}
