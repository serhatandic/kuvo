import path from "node:path";
import { additiveObjectFormat } from "./format-version";
import { readVersionedFile } from "./versioned-file";

export const boxFilesFormat = additiveObjectFormat("Box pack file list");
export const installedFormat = additiveObjectFormat("Installed Box receipt");

export function readInstalled(uploads: string) {
  return readVersionedFile<Record<string, unknown>>(path.join(uploads, "installed.json"), installedFormat);
}
