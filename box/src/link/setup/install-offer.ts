import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import fs from "node:fs";
import path from "node:path";
import { exclusiveWhile, syncDurableFile, writeDurablePrivate } from "../../shared/files";
import { isObject } from "../../shared/json";
import { SETUP_ID } from "./record";
import { REMOVAL_ID } from "./removal-record";

export const REINSTALL_LOCAL = "reinstall-local" as const;

export const wipeLock = (home: string) => path.resolve(home) + "-wipe.lock";
const APP_ID = /^run\.kuvo\.code(?:\.[a-zA-Z0-9-]+)*$/;
export const isAppWipeFolder = (folder: string) => APP_ID.test(path.basename(folder))
  && path.basename(path.dirname(folder)) === "Application Support"
  && path.basename(path.dirname(path.dirname(folder))) === "Library";
export const wipeReceipts = (home: string) => path.join(path.dirname(home),
  isAppWipeFolder(home) ? `.${path.basename(home)}-removal-receipts` : ".kuvo-code-removal-receipts");
const wipeFormat = additiveObjectFormat("Directory wipe receipt");
const installedLinkFormat = additiveObjectFormat("Link state");
const LAST_WIPE = "last-setup-wipe";
const APP_MEMBER_WIPE = "app-member-wipe";
const wipeRecordName = (member: boolean) => member ? APP_MEMBER_WIPE : LAST_WIPE;
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
  if (isObject(receipt) && (name === LAST_WIPE || name === APP_MEMBER_WIPE)) {
    wipeRecord(home, name === APP_MEMBER_WIPE);
    receipt = wipeFormat.write(receipt);
  }
  writeDurablePrivate(file, JSON.stringify(receipt), path.dirname(home));
  return file;
}

function wipeRecord(home: string, member = false): WipeRecord | null {
  if (path.basename(home) !== ".kuvo-code" && !isAppWipeFolder(home)) return null;
  try {
    privateReceipts(home);
    const file = path.join(wipeReceipts(home), wipeRecordName(member));
    const info = fs.lstatSync(file);
    if (!info.isFile() || (info.mode & 0o077) !== 0 || (process.getuid && info.uid !== process.getuid())) return null;
    if (info.size > 4096) throw new UnsupportedFormatVersion("Directory wipe receipt", undefined, 1);
    const parsed: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    if (!isObject(parsed)) return null;
    const value = wipeFormat.read(parsed);
    if (info.size > 220 || typeof value.setup !== "string" || !SETUP_ID.test(value.setup)
      || typeof value.operation !== "string" || !REMOVAL_ID.test(value.operation)
      || value.phase !== undefined && value.phase !== "stopping" && value.phase !== "erasing" && value.phase !== "detached"
      || (value.phase !== undefined || value.source !== undefined) && (typeof value.source !== "string" || !/^\d+:\d+$/.test(value.source) || !value.phase)) return null;
    return { setup: value.setup, operation: value.operation, source: value.source, phase: value.phase };
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return null;
  }
}

function directoryIdentity(folder: string): string | null {
  const info = fs.lstatSync(folder, { bigint: true, throwIfNoEntry: false });
  if (!info) return null;
  if (!info.isDirectory()) throw new Error("The folder to erase must be a directory.");
  return `${info.dev}:${info.ino}`;
}

/** With the wipe lock held, continue exactly the erase that was already authorized. */
function eraseApprovedDirectory(home: string, record: WipeRecord, member: boolean, beforeDelete?: () => void): void {
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
    keepWipeReceipt(home, wipeRecordName(member), { ...record, phase: "detached" });
  }
  beforeDelete?.();
  fs.rmSync(target, { recursive: true, force: true });
  keepWipeReceipt(home, wipeRecordName(member), { setup: record.setup, operation: record.operation });
}

/** This Box verified the wipe. Remember its directory identity before stopping the helper. */
export function prepareWipeDirectory(home: string, setup: string, operation: string, member: boolean): void {
  const source = directoryIdentity(home);
  if (!source) throw new Error("The folder to erase is missing.");
  const prior = wipeRecord(home, member);
  if (prior?.phase && (prior.phase !== "stopping" || prior.source !== source || prior.setup !== setup || prior.operation !== operation)) {
    throw new Error("A different directory erase is still pending.");
  }
  keepWipeReceipt(home, wipeRecordName(member), { setup, operation, source, phase: "stopping" });
}

export function retainedWipeApproval(home: string, setup: string, operation: string, member = false): boolean {
  const record = wipeRecord(home, member);
  return record?.phase === "stopping" && record.setup === setup && record.operation === operation && record.source === directoryIdentity(home);
}

export function forgetWipeApproval(home: string, setup: string, operation: string, member = false): void {
  if (retainedWipeApproval(home, setup, operation, member)) fs.rmSync(path.join(wipeReceipts(home), wipeRecordName(member)), { force: true });
}

/** Called after the last box stopped, its wipe was verified again and its credentials were removed. */
export function wipeApprovedDirectory(home: string, setup: string, operation: string, member: boolean, beforeDelete?: () => void): void {
  const approved = wipeRecord(home, member);
  if (!approved || !retainedWipeApproval(home, setup, operation, member)) throw new Error("The folder to erase has changed.");
  const record = { ...approved, phase: "erasing" as const };
  keepWipeReceipt(home, wipeRecordName(member), record);
  eraseApprovedDirectory(home, record, member, beforeDelete);
}

/** Opening the installer after a crash can finish the authorized wipe before offering reinstall. */
export async function resumeWipeDirectory(home: string, member = false, beforeDelete?: () => void): Promise<void> {
  const phase = wipeRecord(home, member)?.phase;
  if (phase !== "erasing" && phase !== "detached") return;
  await exclusiveWhile(wipeLock(home), async () => {
    const record = wipeRecord(home, member);
    if (record) eraseApprovedDirectory(home, record, member, beforeDelete);
  });
}

export function lastSetupWipe(home: string): "erasing" | "erased" | null {
  const record = wipeRecord(home);
  return record ? record.phase ? "erasing" : "erased" : null;
}

export function lastSetupWipeReceipt(home: string): { setup: string; operation: string } | undefined {
  const record = wipeRecord(home);
  if (record && !record.phase && (!isAppWipeFolder(home) || !fs.existsSync(home))) return { setup: record.setup, operation: record.operation };
  return undefined;
}

export function appMemberWipePending(home: string): boolean {
  return Boolean(wipeRecord(home, true)?.phase);
}

export function appMemberWipeReceipt(home: string): { setup: string; operation: string } | undefined {
  const record = wipeRecord(home, true);
  if (record && !record.phase) return { setup: record.setup, operation: record.operation };
  return undefined;
}

/** Used to fence installer work before the verified wipe has left a receipt. */
export function localWipeAt(home: string): "last" | "member" | "member-review" | null {
  try {
    const parsed: unknown = JSON.parse(fs.readFileSync(path.join(home, "run", "link.json"), "utf8"));
    if (!isObject(parsed)) return null;
    const link = installedLinkFormat.read(parsed);
    if (!isObject(link.setup)) return null;
    if (link.setup.role === "member" && isObject(link.removalSource) && link.removalSource.setup === link.setup.id &&
      typeof link.removalSource.operation === "string" && REMOVAL_ID.test(link.removalSource.operation)) {
      return link.removalSource.stopped === true && !link.localWipe ? "member-review" : "member";
    }
    if (!isObject(link.localWipe)
      || typeof link.localWipe.operation !== "string" || !REMOVAL_ID.test(link.localWipe.operation)
      || typeof link.setup.id !== "string" || !SETUP_ID.test(link.setup.id) || link.localWipe.setup !== link.setup.id) return null;
    if (link.setup.role === "primary" && link.localWipe.primary === undefined) return "last";
    return link.setup.role === "member" && isObject(link.localWipe.primary) ? "member" : null;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return null;
  }
}

export function finishReinstall(home: string): void {
  if (lastSetupWipe(home) === "erased") {
    fs.rmSync(path.join(wipeReceipts(home), LAST_WIPE), { force: true });
    syncDurableFile(wipeReceipts(home));
  }
}
