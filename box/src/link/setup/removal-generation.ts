// Version the source's durable work, including uncheckpointed SQLite writes and selected projects.
// Metadata includes the inode and change time so replacing a file or restoring its mtime is observable.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { BOTS } from "../home";
import { durableFiles } from "../backup/locations";
import { projectEntries } from "../backup/projects";

export function sourceGeneration(projects: readonly string[]): string {
  const hash = crypto.createHash("sha256");
  const fileStamp = (name: string, file: string, optional = false) => {
    const info = fs.lstatSync(file, { bigint: true, throwIfNoEntry: false });
    if (!info?.isFile() && !optional) throw new Error("Source work changed while it was being checked.");
    if (info?.isFile() && info.size <= 64n * 1024n && !/\.(?:sqlite|db)(?:-wal)?$/.test(name)) {
      hash.update(JSON.stringify([name, info.mode.toString(), crypto.createHash("sha256").update(fs.readFileSync(file)).digest("base64url")]));
      return;
    }
    hash.update(JSON.stringify([name, info?.ino.toString(), info?.size.toString(), info?.ctimeNs.toString(), info?.mtimeNs.toString()]));
  };
  const botWork = (folder: string) => {
    if (!fs.lstatSync(folder, { throwIfNoEntry: false })?.isDirectory()) return;
    for (const name of fs.readdirSync(folder).sort()) {
      if (/\.(?:tmp|new|lock)$/.test(name)) continue;
      const file = path.join(folder, name);
      const info = fs.lstatSync(file);
      if (info.isDirectory()) botWork(file);
      else if (info.isFile()) fileStamp(path.relative(BOTS, file), file);
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
    fileStamp(name, file);
    if (name.endsWith(".db") || name.endsWith(".sqlite")) fileStamp(name + "-wal", file + "-wal", true);
  }
  for (const entry of projectEntries(projects)) {
    if ("file" in entry) fileStamp(entry.name, entry.file);
    else hash.update(entry.bytes);
  }
  return hash.digest("base64url");
}
