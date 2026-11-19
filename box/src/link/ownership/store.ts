import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { additiveObjectFormat } from "../../shared/format-version";
import { offlineEditLayoutReady, ownershipLayoutReady } from "../../shared/folder-version";
import { writeDurablePrivate } from "../../shared/files";
import { K, RUN } from "../home";

const root = path.join(RUN, "owned-data");
const format = additiveObjectFormat("Owned data");
type Family = "reads" | "outbox" | "accepted" | "edit-outbox" | "edit-snapshots" | "edit-deletions" | "edit-departures";

export const offlineEditsReady = () => offlineEditLayoutReady(K);
const ready = (family: Family) => family.startsWith("edit-") ? offlineEditsReady() : ownershipReady();

export const ownershipReady = () => ownershipLayoutReady(K);

export function recordKey(...parts: string[]): string {
  return createHash("sha256").update(JSON.stringify(parts)).digest("base64url");
}

export function records(family: Family): string[] {
  if (!ready(family)) return [];
  try { return fs.readdirSync(path.join(root, family)).filter(name => /^[A-Za-z0-9_-]{43}\.json$/.test(name)).map(name => name.slice(0, -5)); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return [];
  }
}

export function readRecord<T extends object>(family: Family, key: string): T | undefined {
  if (!ready(family)) return undefined;
  try { return format.read(JSON.parse(fs.readFileSync(path.join(root, family, key + ".json"), "utf8"))) as T; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return undefined;
  }
}

export function keepRecord(family: Family, key: string, value: object): void {
  if (!ready(family)) throw new Error("Wait for Kuvo Code to finish updating.");
  writeDurablePrivate(path.join(root, family, key + ".json"), JSON.stringify(format.write(value)), K);
}
