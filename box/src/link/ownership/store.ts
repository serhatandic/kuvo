import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat } from "../../shared/format-version";
import { offlineEditLayoutReady, ownershipLayoutReady } from "../../shared/folder-version";
import { writeDurablePrivate } from "../../shared/files";
import { K, RUN } from "../home";
import { changeReceiptFile } from "./receipt-index";

const root = path.join(RUN, "owned-data");
export const ownedRecordFormat = additiveObjectFormat("Owned data");
type Family = "queue-handoffs" | "mutations" | "forward-origins" | "submissions" | "reads" | "outbox" | "message-receipts" | "accepted" | "edit-outbox" | "edit-snapshots" | "edit-deletions" | "edit-departures";

export const offlineEditsReady = () => offlineEditLayoutReady(K);
const ready = (family: Family) => family.startsWith("edit-") ? offlineEditsReady() : ownershipReady();

export const ownershipReady = () => ownershipLayoutReady(K);

export { recordKey } from "./record-address";

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
  try { return ownedRecordFormat.read(JSON.parse(fs.readFileSync(path.join(root, family, key + ".json"), "utf8"))) as T; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return undefined;
  }
}

export async function recordsAsync(family: Family): Promise<string[]> {
  if (!ready(family)) return [];
  try { return (await fs.promises.readdir(path.join(root, family))).filter(name => /^[A-Za-z0-9_-]{43}\.json$/.test(name)).map(name => name.slice(0, -5)); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return [];
  }
}

const activeOutbox = new Map<string, { stamp: number; size: number; inode: number; record: object }>();
export function pruneActiveOutbox(keys: ReadonlySet<string>) {
  for (const key of activeOutbox.keys()) if (!keys.has(key)) activeOutbox.delete(key);
}

/** Queue follows, sends and retries share the active records without rereading their attachments. */
export async function readRecordAsync<T extends object>(family: Family, key: string): Promise<T | undefined> {
  if (!ready(family)) return undefined;
  const file = path.join(root, family, key + ".json");
  try {
    const cached = family === "outbox" ? activeOutbox.get(key) : undefined;
    const info = family === "outbox" ? await fs.promises.stat(file) : undefined;
    if (cached && info && cached.stamp === info.mtimeMs && cached.size === info.size && cached.inode === info.ino)
      return cached.record as T;
    const record = ownedRecordFormat.read(JSON.parse(await fs.promises.readFile(file, "utf8"))) as T;
    if (info) {
      if (("queuedDelivery" in record) && info.size <= 256 * 1024 && activeOutbox.size < 256)
        activeOutbox.set(key, { stamp: info.mtimeMs, size: info.size, inode: info.ino, record });
      else activeOutbox.delete(key);
    }
    return record;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    if (family === "outbox") activeOutbox.delete(key);
    return undefined;
  }
}

export function keepRecord(family: Family, key: string, value: object, index = true): void {
  if (!ready(family)) throw new Error("Wait for Kuvo Code to finish updating.");
  const write = () => writeDurablePrivate(path.join(root, family, key + ".json"), JSON.stringify(ownedRecordFormat.write(value)), K);
  if (index) changeReceiptFile(family, key, value, write);
  else write();
}

export function removeRecord(family: Family, key: string): void {
  if (ready(family)) changeReceiptFile(family, key, undefined, () => fs.rmSync(path.join(root, family, key + ".json"), { force: true }));
}
