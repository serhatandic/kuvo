// Authenticated streaming archives. Only a private staging area sees bytes before authentication.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const MAGIC = Buffer.from("KUVO backup 1\n");
const BLOCK = 256 * 1024;
const MAX_HEADER = 32 * 1024;
const MAX_PAYLOAD = 2 ** 36 - 32; // GCM counter length limit per nonce.
const NOFOLLOW = fs.constants.O_NOFOLLOW;
const KEY = /^[A-Za-z0-9_-]{43}$/;
export const newRecoveryKey = () => crypto.randomBytes(32).toString("base64url");

function keyBytes(given: string): Buffer {
  const bytes = Buffer.from(given, "base64url");
  if (!KEY.test(given) || bytes.length !== 32 || bytes.toString("base64url") !== given) throw new Error("Enter the recovery key shown when this backup was set up.");
  return bytes;
}

export const archiveContext = (source: string, point: string) => Buffer.from(`backup\0${source}\0${point}`);

export type Entry = { name: string; file: string } | { name: string; bytes: Buffer };
export const archivePath = (name: string) => name.length <= 4096 && !name.includes("\\") &&
  name.split("/").every(part => part && part !== "." && part !== ".." && !part.includes("\0"));

function read(fd: number, buffer: Buffer, at: number, length: number): Buffer {
  for (let offset = 0; offset < length;) {
    const bytes = fs.readSync(fd, buffer, offset, length - offset, at + offset);
    if (!bytes) throw new Error("Backup data ended unexpectedly.");
    offset += bytes;
  }
  return buffer.subarray(0, length);
}

function write(fd: number, bytes: Buffer): void {
  for (let at = 0; at < bytes.length;) at += fs.writeSync(fd, bytes, at, bytes.length - at);
}

export function seal(file: string, key: string, entries: Iterable<Entry>, context: Buffer): void {
  const header = Buffer.concat([MAGIC, crypto.randomBytes(12)]);
  const cipher = crypto.createCipheriv("aes-256-gcm", keyBytes(key), header.subarray(MAGIC.length));
  cipher.setAAD(Buffer.concat([header, context]));
  const out = fs.openSync(file, "wx", 0o600);
  const block = Buffer.allocUnsafe(BLOCK);
  try {
    write(out, header);
    let total = 4;
    const entry = (name: string, size: number) => {
      if (!archivePath(name)) throw new Error("Invalid backup entry name.");
      const json = Buffer.from(JSON.stringify({ name, size }));
      total += 4 + json.length + size;
      if (total > MAX_PAYLOAD) throw new Error("The backup exceeds this archive format’s size limit.");
      const length = Buffer.alloc(4);
      length.writeUInt32BE(json.length);
      write(out, cipher.update(length));
      write(out, cipher.update(json));
    };
    for (const item of entries) {
      if ("bytes" in item) {
        entry(item.name, item.bytes.length);
        write(out, cipher.update(item.bytes));
        continue;
      }
      const fd = fs.openSync(item.file, fs.constants.O_RDONLY | NOFOLLOW | fs.constants.O_NONBLOCK);
      try {
        const before = fs.fstatSync(fd, { bigint: true });
        if (!before.isFile() || before.size > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error("Backup needs regular files.");
        const size = Number(before.size);
        entry(item.name, size);
        for (let at = 0; at < size;) {
          const piece = read(fd, block, at, Math.min(BLOCK, size - at));
          write(out, cipher.update(piece));
          at += piece.length;
        }
        const after = fs.lstatSync(item.file, { bigint: true });
        if (before.ino !== after.ino || before.dev !== after.dev || before.ctimeNs !== after.ctimeNs || before.mtimeNs !== after.mtimeNs || before.size !== after.size) {
          throw new Error("Kuvo data changed during backup. Try again.");
        }
      } finally { fs.closeSync(fd); }
    }
    write(out, cipher.update(Buffer.alloc(4)));
    write(out, cipher.final());
    write(out, cipher.getAuthTag());
    fs.fsyncSync(out);
  } finally { fs.closeSync(out); }
}

/** Decrypt to a private descriptor when restoring, or discard the plaintext when verifying. */
export function authenticate(file: string, key: string, context: Buffer, out?: number): number {
  const fd = fs.openSync(file, fs.constants.O_RDONLY | NOFOLLOW | fs.constants.O_NONBLOCK);
  try {
    const stat = fs.fstatSync(fd);
    const headerSize = MAGIC.length + 12;
    const end = stat.size - 16;
    if (!stat.isFile() || end <= headerSize || end - headerSize > MAX_PAYLOAD) throw new Error("Incomplete backup.");
    const header = read(fd, Buffer.alloc(headerSize), 0, headerSize);
    if (!header.subarray(0, MAGIC.length).equals(MAGIC)) throw new Error("Unsupported backup archive format.");
    const decipher = crypto.createDecipheriv("aes-256-gcm", keyBytes(key), header.subarray(MAGIC.length));
    decipher.setAAD(Buffer.concat([header, context]));
    decipher.setAuthTag(read(fd, Buffer.alloc(16), end, 16));
    const block = Buffer.allocUnsafe(BLOCK);
    for (let at = headerSize; at < end;) {
      const piece = read(fd, block, at, Math.min(BLOCK, end - at));
      const decoded = decipher.update(piece);
      if (out !== undefined) write(out, decoded);
      at += piece.length;
    }
    const final = decipher.final();
    if (out !== undefined) write(out, final);
    return end - headerSize;
  } finally { fs.closeSync(fd); }
}

/** Unlink before decrypting, so a process death cannot leave the decoded stream on disk. */
function withArchive<T>(file: string, key: string, context: Buffer, scratch: string, work: (out: number, limit: number) => T): T {
  const plain = path.join(scratch, "plain");
  const out = fs.openSync(plain, "wx+", 0o600);
  try {
    fs.unlinkSync(plain);
    return work(out, authenticate(file, key, context, out));
  } finally { fs.closeSync(out); }
}

function readEntries(out: number, limit: number, entry: (name: string, size: number, at: number) => void) {
  const block = Buffer.allocUnsafe(MAX_HEADER);
  const seen = new Set<string>();
  let at = 0;
  for (;;) {
    const size = read(out, block, at, 4).readUInt32BE();
    at += 4;
    if (!size && at === limit) return;
    if (!size || size > MAX_HEADER || size > limit - at) throw new Error("Invalid backup header.");
    const item = JSON.parse(read(out, block, at, size).toString("utf8"));
    at += size;
    if (typeof item?.name !== "string" || !archivePath(item.name) || seen.has(item.name) ||
      !Number.isSafeInteger(item.size) || item.size < 0 || item.size > limit - at - 4) throw new Error("Invalid backup entry.");
    seen.add(item.name);
    entry(item.name, item.size, at);
    at += item.size;
  }
}

/** Authenticate before parsing. Computer recovery needs staged files for folder upgrades. */
export function openArchive(file: string, key: string, context: Buffer, scratch: string): { name: string; file: string }[] {
  return withArchive(file, key, context, scratch, (out, limit) => {
    const block = Buffer.allocUnsafe(BLOCK);
    const entries: { name: string; file: string }[] = [];
    readEntries(out, limit, (name, size, at) => {
      const file = path.join(scratch, String(entries.length));
      const entry = fs.openSync(file, "wx", 0o600);
      try {
        for (let left = size; left > 0;) {
          const piece = read(out, block, at, Math.min(BLOCK, left));
          write(entry, piece);
          at += piece.length;
          left -= piece.length;
        }
      } finally { fs.closeSync(entry); }
      entries.push({ name, file });
    });
    return entries;
  });
}

export function openSingleEntry(file: string, key: string, context: Buffer, scratch: string, expected: string): Buffer {
  return withArchive(file, key, context, scratch, (out, limit) => {
    let value: Buffer | undefined;
    readEntries(out, limit, (name, size, at) => {
      if (name !== expected || value) throw new Error("Unexpected entry in the backup.");
      value = read(out, Buffer.alloc(size), at, size);
    });
    if (!value) throw new Error("The backup entry is missing.");
    return value;
  });
}
