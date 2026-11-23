// Fixed pre-switch archive bytes for the conformance and differential runners.
import assert from "node:assert/strict";
import { createCipheriv, createDecipheriv } from "node:crypto";
import path from "node:path";
import { write } from "./adapter.mjs";

export const source = "b_" + "a".repeat(22);
export const id = "2026-11-19T000000.000Z-wipe-0123456789abcdef";
export const key = Buffer.alloc(32, 7);
export const at = Date.UTC(2026, 10, 19);
const magic = Buffer.from("KUVO backup 1\n");

export function unpackPoint(archive, source, id, key) {
  assert.deepEqual(archive.subarray(0, magic.length), magic);
  const header = archive.subarray(0, magic.length + 12);
  const decipher = createDecipheriv("aes-256-gcm", key, header.subarray(-12));
  decipher.setAAD(Buffer.concat([header, Buffer.from(`backup\0${source}\0${id}`)]));
  decipher.setAuthTag(archive.subarray(-16));
  const payload = Buffer.concat([decipher.update(archive.subarray(header.length, -16)), decipher.final()]);
  const records = new Map();
  let cursor = 0;
  for (;;) {
    const length = payload.readUInt32BE(cursor);
    cursor += 4;
    if (length === 0) {
      assert.equal(cursor, payload.length);
      return records;
    }
    const { name, size } = JSON.parse(payload.subarray(cursor, cursor + length));
    cursor += length;
    assert.ok(!records.has(name));
    assert.ok(Number.isSafeInteger(size) && size >= 0 && cursor + size <= payload.length - 4);
    records.set(name, payload.subarray(cursor, cursor + size));
    cursor += size;
  }
}

// A point from a pre-switch installation with neither membership nor secrets.
// These records use the same GCM, associated data and framing as archive.ts.
export function oldPoint(disk, extra = []) {
  const header = Buffer.concat([magic, Buffer.alloc(12, 1)]);
  const cipher = createCipheriv("aes-256-gcm", key, header.subarray(-12));
  cipher.setAAD(Buffer.concat([header, Buffer.from(`backup\0${source}\0${id}`)]));
  const frame = (name, value) => {
    const item = Buffer.from(JSON.stringify({ name, size: Buffer.byteLength(value) }));
    const size = Buffer.alloc(4);
    size.writeUInt32BE(item.length);
    return Buffer.concat([size, item, Buffer.from(value)]);
  };
  const payload = Buffer.concat([
    frame("meta/manifest.json", JSON.stringify({ format: 1, source, id, at, version: 6,
      origin: null, signer: null, setup: null, paired: false, projects: [] })),
    frame("home/box/approval.json", '{"fullAccess":true,"last":"askFirst","since":10}'),
    frame("bots/bot_a/notes.md", "History from the old disk."),
    frame("wakes/ses_a/wakes/morning.json", '{"at":1795046400000}'),
    frame("home/run/queues.json", '{"ses_a":{"waiting":[{"id":"msg_due","text":"Review the old send"}]}}'),
    frame("home/run/waiting/msg_due.json", '{"id":"msg_due","text":"Review the old send"}'),
    ...extra.map(([name, value]) => frame(name, value)),
    Buffer.alloc(4),
  ]);
  const archive = Buffer.concat([header, cipher.update(payload), cipher.final(), cipher.getAuthTag()]);
  write(path.join(disk, "kuvo-backups", source, id + ".kuvo"), archive);
}
