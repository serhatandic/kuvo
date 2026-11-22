// Hash a consistent SQLite snapshot so committed WAL writes count once, wherever they are stored.
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { locked } from "../files";
import { BOTS } from "../home";
import { attachmentKind, queueBackupKind, roots, storageInventory, type InventoryEntry } from "../backup/locations";
import { projectEntries } from "../backup/projects";
import { snapshotDatabase } from "../backup/sqlite";
import { QUEUES_LOCK } from "../work-queue/migrate";

const sqlite = /\.(?:sqlite|db)$/;
const sidecar = /^(.*\.(?:sqlite|db))-(?:wal|shm|journal)$/;

function contents(file: string, checkRevision = false): string {
  const hash = crypto.createHash("sha256");
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  const buffer = Buffer.allocUnsafe(128 * 1024);
  try {
    const before = checkRevision ? fs.fstatSync(fd, { bigint: true }) : undefined;
    if (before && !before.isFile()) throw new Error("Source work changed while it was being checked.");
    for (let bytes; (bytes = fs.readSync(fd, buffer, 0, buffer.length, null));) {
      hash.update(buffer.subarray(0, bytes));
    }
    if (before) {
      const after = fs.lstatSync(file, { bigint: true });
      if (before.ino !== after.ino || before.dev !== after.dev || before.ctimeNs !== after.ctimeNs ||
          before.mtimeNs !== after.mtimeNs || before.size !== after.size) {
        throw new Error("Source work changed while it was being checked.");
      }
    }
  } finally { fs.closeSync(fd); }
  return hash.digest("base64url");
}

function sqliteContents(file: string): string {
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-generation-"));
  try {
    const copy = path.join(folder, "snapshot");
    snapshotDatabase(file, copy);
    return contents(copy);
  } finally { fs.rmSync(folder, { recursive: true, force: true }); }
}

function hasSQLiteHeader(file: string): boolean {
  if (!fs.lstatSync(file, { throwIfNoEntry: false })?.isFile()) return false;
  const fd = fs.openSync(file, "r");
  const header = Buffer.alloc(16);
  try { return fs.readSync(fd, header, 0, header.length, 0) === header.length && header.toString("ascii") === "SQLite format 3\0"; }
  finally { fs.closeSync(fd); }
}

export function sourceGeneration(projects: readonly string[], attachmentHashes?: Map<string, string>): string {
  // Older wipe approvals were made without the promised attachment inventory.
  const hash = crypto.createHash("sha256").update("wipe generation with attachment inventory\0");
  const fileStamp = (name: string, file: string, database = false, checkRevision = false) => {
    const info = fs.lstatSync(file);
    if (!info.isFile()) throw new Error("Source work changed while it was being checked.");
    const digest = database ? sqliteContents(file) : contents(file, checkRevision);
    hash.update(JSON.stringify([name, info.mode, digest]));
    return digest;
  };
  const botWork = (folder: string) => {
    if (!fs.lstatSync(folder, { throwIfNoEntry: false })?.isDirectory()) return;
    for (const name of fs.readdirSync(folder).sort()) {
      if (/\.(?:tmp|new|lock)$/.test(name)) continue;
      const file = path.join(folder, name);
      const info = fs.lstatSync(file);
      if (info.isDirectory()) botWork(file);
      else if (info.isFile()) {
        const side = sidecar.exec(name);
        if (!side || !hasSQLiteHeader(path.join(folder, side[1]!))) {
          fileStamp(path.relative(BOTS, file), file, sqlite.test(name) && hasSQLiteHeader(file));
        }
      }
      else if (info.isSymbolicLink()) hash.update(JSON.stringify([path.relative(BOTS, file), fs.readlinkSync(file)]));
    }
  };
  if (fs.lstatSync(BOTS, { throwIfNoEntry: false })?.isDirectory()) {
    for (const name of fs.readdirSync(BOTS).sort()) {
      if (/^bot_[A-Za-z0-9]+$/.test(name)) botWork(path.join(BOTS, name));
    }
  }
  const stamp = ({ name, file, snapshot, kind }: InventoryEntry) => {
    if (file) {
      const attachment = attachmentKind(kind);
      const digest = fileStamp(name, file, snapshot !== "bytes", attachment);
      if (attachment) attachmentHashes?.set(name, digest);
    }
    // A revised choice without an export can still identify exactly what will be erased.
    else hash.update(JSON.stringify([name, "missing"]));
  };
  // Counters, telemetry and the startup status migration stamp change independently of work.
  const queueFiles = locked(QUEUES_LOCK, () => {
    const payloads: InventoryEntry[] = [];
    for (const entry of storageInventory(roots, kind => queueBackupKind(kind) && kind !== "queueNumberUse")) {
      if (entry.kind === "queueFiles") payloads.push(entry);
      else if (entry.name !== "home/run/work-queue/model.json") stamp(entry);
    }
    return payloads;
  }, { exclusive: true });
  // Capture checks the sealed bytes. Generation only holds the writer lock for
  // the queue records and inventory, so hashing big payloads won't block edits.
  for (const entry of queueFiles) stamp(entry);
  for (const entry of storageInventory(roots, kind => !queueBackupKind(kind) && kind !== "usageRecords")) stamp(entry);
  for (const entry of projectEntries(projects)) {
    if ("file" in entry) fileStamp(entry.name, entry.file);
    else hash.update(entry.bytes);
  }
  return hash.digest("base64url");
}

/** An inventory with the exact attachment hashes captured by the same scan. */
export function scanSourceGeneration(projects: readonly string[], includeAttachments: boolean) {
  const attachmentHashes = includeAttachments ? new Map<string, string>() : undefined;
  return { generation: sourceGeneration(projects, attachmentHashes), attachmentHashes };
}
