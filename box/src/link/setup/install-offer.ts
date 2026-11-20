import fs from "node:fs";
import path from "node:path";
import { exclusiveWhile, syncDurableFile, writeDurablePrivate } from "../../shared/files";
import { isObject } from "../../shared/json";
import { SETUP_ID } from "./record";
import { REMOVAL_ID } from "./removal-record";

export const REINSTALL_LOCAL = "reinstall-local" as const;

export const wipeLock = (home: string) => path.resolve(home) + "-wipe.lock";
export const wipeReceipts = (home: string) => path.join(path.dirname(home), ".kuvo-code-removal-receipts");
const LAST_WIPE = "last-setup-wipe";
const lastWipe = (home: string) => path.join(wipeReceipts(home), LAST_WIPE);
const retired = (home: string, operation: string) => path.join(wipeReceipts(home), `erase-${operation}`);

interface WipeRecord {
  setup: string;
  operation: string;
  /** The old directory's device and inode, while it still needs to be erased. */
  source?: string;
  phase?: "stopping" | "erasing" | "detached";
}

function privateReceipts(home: string): string {
  const folder = wipeReceipts(home);
  const info = fs.lstatSync(folder);
  if (!info.isDirectory() || (info.mode & 0o077) !== 0 || (process.getuid && process.getuid() !== info.uid)) {
    throw new Error("The wipe receipt folder must be private to this account.");
  }
  return folder;
}

export function keepWipeReceipt(home: string, name: string, receipt: unknown): string {
  fs.mkdirSync(wipeReceipts(home), { recursive: true, mode: 0o700 });
  const file = path.join(privateReceipts(home), name);
  writeDurablePrivate(file, JSON.stringify(receipt), path.dirname(home));
  return file;
}

function lastWipeRecord(home: string): WipeRecord | null {
  if (path.basename(home) !== ".kuvo-code") return null;
  try {
    privateReceipts(home);
    const file = lastWipe(home);
    const info = fs.lstatSync(file);
    if (!info.isFile() || (info.mode & 0o077) !== 0 || info.size > 200 || (process.getuid && info.uid !== process.getuid())) return null;
    const value: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    if (!isObject(value) || typeof value.setup !== "string" || !SETUP_ID.test(value.setup)
      || typeof value.operation !== "string" || !REMOVAL_ID.test(value.operation)
      || value.phase !== undefined && value.phase !== "stopping" && value.phase !== "erasing" && value.phase !== "detached"
      || (value.phase !== undefined || value.source !== undefined) && (typeof value.source !== "string" || !/^\d+:\d+$/.test(value.source) || !value.phase)) return null;
    return { setup: value.setup, operation: value.operation, source: value.source, phase: value.phase };
  } catch { return null; }
}

function directoryIdentity(folder: string): string | null {
  const info = fs.lstatSync(folder, { bigint: true, throwIfNoEntry: false });
  if (!info) return null;
  if (!info.isDirectory()) throw new Error("The folder to erase must be a directory.");
  return `${info.dev}:${info.ino}`;
}

/** With the wipe lock held, continue exactly the erase that was already authorized. */
function eraseLastSetup(home: string, record: WipeRecord): void {
  if (record.phase !== "erasing" && record.phase !== "detached") return;
  const target = retired(home, record.operation);
  const old = directoryIdentity(target);
  if (old && old !== record.source) throw new Error("The folder to erase has changed.");
  if (record.phase === "erasing") {
    if (!old) {
      if (directoryIdentity(home) !== record.source) throw new Error("The folder to erase has changed.");
      fs.renameSync(home, target);
    }
    // The rename and the phase that says it happened must both survive before deletion starts.
    keepWipeReceipt(home, LAST_WIPE, { ...record, phase: "detached" });
  }
  fs.rmSync(target, { recursive: true, force: true });
  keepWipeReceipt(home, LAST_WIPE, { setup: record.setup, operation: record.operation });
}

/** This box verified the accepted last wipe. Remember that approval before stopping the helper. */
export function prepareLastSetupWipe(home: string, setup: string, operation: string): void {
  const source = directoryIdentity(home);
  if (!source) throw new Error("The folder to erase is missing.");
  keepWipeReceipt(home, LAST_WIPE, { setup, operation, source, phase: "stopping" });
}

export function retainedWipeApproval(home: string, setup: string, operation: string): boolean {
  const record = lastWipeRecord(home);
  return record?.phase === "stopping" && record.setup === setup && record.operation === operation && record.source === directoryIdentity(home);
}

export function forgetWipeApproval(home: string, setup: string, operation: string): void {
  if (retainedWipeApproval(home, setup, operation)) fs.rmSync(lastWipe(home), { force: true });
}

/** Called after the last box stopped, its wipe was verified again and its credentials were removed. */
export function wipeLastSetup(home: string, setup: string, operation: string): void {
  const approved = lastWipeRecord(home);
  if (!approved || !retainedWipeApproval(home, setup, operation)) throw new Error("The folder to erase has changed.");
  const record = { ...approved, phase: "erasing" as const };
  keepWipeReceipt(home, LAST_WIPE, record);
  eraseLastSetup(home, record);
}

/** Opening the installer after a crash can finish the authorized wipe before offering reinstall. */
export async function resumeLastSetupWipe(home: string): Promise<void> {
  const phase = lastWipeRecord(home)?.phase;
  if (phase !== "erasing" && phase !== "detached") return;
  await exclusiveWhile(wipeLock(home), async () => {
    const record = lastWipeRecord(home);
    if (record) eraseLastSetup(home, record);
  });
}

export function lastSetupWipe(home: string): "erasing" | "erased" | null {
  const record = lastWipeRecord(home);
  return record ? record.phase ? "erasing" : "erased" : null;
}

/** Used to fence installer work before the verified wipe has left a receipt. */
export function localWipeAt(home: string): "last" | "member" | null {
  try {
    const link: unknown = JSON.parse(fs.readFileSync(path.join(home, "run", "link.json"), "utf8"));
    if (!isObject(link) || !isObject(link.localWipe) || !isObject(link.setup)
      || typeof link.localWipe.operation !== "string" || !REMOVAL_ID.test(link.localWipe.operation)
      || typeof link.setup.id !== "string" || !SETUP_ID.test(link.setup.id) || link.localWipe.setup !== link.setup.id) return null;
    if (link.setup.role === "primary" && link.localWipe.primary === undefined) return "last";
    return link.setup.role === "member" && isObject(link.localWipe.primary) ? "member" : null;
  } catch { return null; }
}

export function finishReinstall(home: string): void {
  if (lastSetupWipe(home) === "erased") {
    fs.rmSync(lastWipe(home), { force: true });
    syncDurableFile(wipeReceipts(home));
  }
}
