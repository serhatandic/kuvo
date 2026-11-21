import fs from "node:fs";
import path from "node:path";
import { syncDurableFile, writeDurablePrivate } from "../files";
import { BOX } from "../home";

export const APPLY_FOLDER = path.join(BOX, "config-apply");
const pendingFile = path.join(APPLY_FOLDER, "pending.json");

export interface ApplyResult {
  plan: string;
  status: "applied" | "alreadySet" | "nothingToApply" | "freshPreview" | "recoveryPending";
  revision?: string;
}

interface Pending {
  file: string;
  target: string;
  staged: string;
  device: string;
  inode: string;
  result: ApplyResult;
}

export function writeApplyResult(result: ApplyResult) {
  writeDurablePrivate(path.join(APPLY_FOLDER, result.plan + ".result.json"), JSON.stringify(result), BOX);
}

export function prepareApply(file: string, target: string, staged: string, result: ApplyResult) {
  const stat = fs.statSync(staged, { bigint: true });
  const pending: Pending = { file, target, staged, device: String(stat.dev), inode: String(stat.ino), result };
  syncDurableFile(path.dirname(staged));
  writeDurablePrivate(pendingFile, JSON.stringify(pending), BOX);
}

/** Under the config lock, before another box write can replace the published file. */
export function settleApply(file: string): void {
  let pending: Pending;
  try {
    pending = JSON.parse(fs.readFileSync(pendingFile, "utf8"));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
    throw error;
  }
  if (pending.file !== file) return;
  if (fs.existsSync(path.join(APPLY_FOLDER, pending.result.plan + ".result.json"))) {
    fs.rmSync(pendingFile);
    return;
  }
  const matches = (file: string) => {
    const stat = fs.statSync(file, { bigint: true, throwIfNoEntry: false });
    return stat && String(stat.dev) === pending.device && String(stat.ino) === pending.inode;
  };
  // A missing or different file is ambiguous after an external edit. Never replay the write.
  const published = matches(pending.target);
  const staged = !published && matches(pending.staged);
  const result: ApplyResult = published ? pending.result
    : { plan: pending.result.plan, status: staged ? "freshPreview" : "recoveryPending" };
  if (result.status === "applied") syncDurableFile(path.dirname(pending.target));
  writeApplyResult(result);
  fs.rmSync(pendingFile);
  if (staged) fs.rmSync(pending.staged, { force: true });
}
