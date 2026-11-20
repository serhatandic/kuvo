import { isObject } from "./json";

export const ITEM_ID = /^wqi_[A-Za-z0-9_-]{20}$/;
export const FILE_NAME = /^(?!\.\.?$)[^/\\\0]{1,200}$/;

/** Older item records can omit the list. A present list promises each named file. */
export function validQueueFileList(files: unknown): boolean {
  if (files === undefined) return true;
  if (!Array.isArray(files)) return false;
  const names = new Set<string>();
  for (const file of files) {
    if (!isObject(file) || typeof file.name !== "string" || !FILE_NAME.test(file.name) || file.name === ".kuvo-move-import"
      || names.has(file.name) || !Number.isSafeInteger(file.size) || (file.size as number) < 0
      || (file.sha256 !== undefined && (typeof file.sha256 !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(file.sha256)))) return false;
    names.add(file.name);
  }
  return true;
}
