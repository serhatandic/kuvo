import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { inside, outputPath } from "../backup/locations";

export interface FileProof {
  size: number;
  sha256: string;
}

export const validProof = (proof: FileProof) => Number.isSafeInteger(proof.size) && proof.size >= 0
  && typeof proof.sha256 === "string" && /^[A-Za-z0-9_-]{43}$/.test(proof.sha256);

export function payloadPath(root: string, relative: string): string {
  const parts = relative.split("/");
  if (parts.some(part => !part || part === "." || part === ".." || /[\\\0\r\n]/.test(part))) throw new Error("Invalid managed file name.");
  return outputPath(root, parts);
}

/** Hash the bytes read from the same regular file we inspected. Optionally stage those exact bytes. */
export function measurePayload(root: string, relative: string, into?: string, limit = Infinity): FileProof {
  const file = payloadPath(root, relative);
  const before = fs.lstatSync(file);
  if (!before.isFile() || before.nlink !== 1 || before.size > limit) throw new Error("A managed file is unavailable or too large.");
  const source = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  let target: number | undefined;
  try {
    const opened = fs.fstatSync(source);
    if (opened.dev !== before.dev || opened.ino !== before.ino || !inside(fs.realpathSync(root), fs.realpathSync(file))) {
      throw new Error("A managed file changed during verification.");
    }
    if (into) {
      fs.mkdirSync(path.dirname(into), { recursive: true, mode: 0o700 });
      target = fs.openSync(into, "wx", 0o600);
    }
    const hash = crypto.createHash("sha256");
    const buffer = Buffer.allocUnsafe(Math.min(1024 * 1024, Math.max(1, before.size)));
    let size = 0;
    for (;;) {
      const read = fs.readSync(source, buffer);
      if (!read) break;
      size += read;
      if (size > before.size) throw new Error("A managed file changed during verification.");
      hash.update(buffer.subarray(0, read));
      if (target !== undefined) for (let written = 0; written < read;) {
        written += fs.writeSync(target, buffer, written, read - written);
      }
    }
    const after = fs.fstatSync(source);
    if (size !== before.size || before.mtimeMs !== after.mtimeMs || before.ctimeMs !== after.ctimeMs || before.mode !== after.mode) {
      throw new Error("A managed file changed during verification.");
    }
    if (target !== undefined) fs.fsyncSync(target);
    return { size, sha256: hash.digest("base64url") };
  } catch (error) {
    if (target !== undefined) fs.unlinkSync(into!);
    throw error;
  } finally {
    fs.closeSync(source);
    if (target !== undefined) fs.closeSync(target);
  }
}

export function verifyPayload(root: string, relative: string, expected: FileProof, into?: string): void {
  if (!validProof(expected)) throw new Error("A managed file has an invalid inventory entry.");
  const actual = measurePayload(root, relative, into, expected.size);
  if (actual.size !== expected.size || actual.sha256 !== expected.sha256) {
    if (into) fs.unlinkSync(into);
    throw new Error("A managed file no longer matches its recorded bytes.");
  }
}
