// Hash a consistent SQLite snapshot so committed WAL writes count once, wherever they are stored.
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { BOTS } from "../home";
import { durableFiles } from "../backup/locations";
import { projectEntries } from "../backup/projects";
import { snapshotDatabase } from "../backup/sqlite";

const sqlite = /\.(?:sqlite|db)$/;
const sidecar = /^(.*\.(?:sqlite|db))-(?:wal|shm|journal)$/;

function contents(file: string): string {
  const hash = crypto.createHash("sha256");
  const fd = fs.openSync(file, "r");
  const buffer = Buffer.allocUnsafe(128 * 1024);
  try {
    for (let bytes; (bytes = fs.readSync(fd, buffer, 0, buffer.length, null));) {
      hash.update(buffer.subarray(0, bytes));
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

export function sourceGeneration(projects: readonly string[]): string {
  const hash = crypto.createHash("sha256");
  const fileStamp = (name: string, file: string, database = false) => {
    const info = fs.lstatSync(file);
    if (!info.isFile()) throw new Error("Source work changed while it was being checked.");
    hash.update(JSON.stringify([name, info.mode, database ? sqliteContents(file) : contents(file)]));
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
  // Counters and telemetry change without any change to the selected work.
  for (const { name, file } of durableFiles(undefined, kind => kind !== "queueNumberUse" && kind !== "usageRecords")) {
    // The status migration stamp is rewritten at startup even when the queues are empty.
    if (name === "home/run/work-queue/model.json") continue;
    fileStamp(name, file, sqlite.test(name));
  }
  for (const entry of projectEntries(projects)) {
    if ("file" in entry) fileStamp(entry.name, entry.file);
    else hash.update(entry.bytes);
  }
  return hash.digest("base64url");
}
