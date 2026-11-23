// Compare the Rust residents' peer frames to the reference sender and reassembler.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const [bundle, fixture] = process.argv.slice(2);
const ts = await import(pathToFileURL(bundle));
const secrets = [Buffer.alloc(32, 11), Buffer.alloc(32, 12)];
const keys = secrets.map(secret => ts.publicOf(secret));
const pending = new Map();
const counts = { ownerRequest: 0, ownerReply: 0 };

function open(frame, sender) {
  const { kind, session, body } = ts.readFrame(frame.subarray(17));
  assert.equal(kind, ts.Frame.mail);
  assert.equal(session, 0);
  return JSON.parse(ts.openOneWay(body, secrets[1 - sender], keys[sender]).payload);
}

for (const [sender, encoded] of JSON.parse(readFileSync(fixture, "utf8"))) {
  const frame = Buffer.from(encoded, "base64url");
  const mail = open(frame, sender);
  const part = mail.workQueue?.part;
  if (!part) {
    assert.equal(mail.workQueue, undefined);
    continue;
  }
  const id = `${sender}:${part.whole}`;
  const actual = pending.get(id) ?? [];
  actual.push(mail);
  pending.set(id, actual);
  const payload = ts.takeQueuePart({ key: keys[sender].toString("base64url") }, part);
  if (!payload) continue;
  pending.delete(id);
  const expected = ts.queueMailsFor({
    key: keys[1 - sender].toString("base64url"),
    address: frame.subarray(1, 17).toString("base64url"),
  }, payload, { key: secrets[sender].toString("base64url") }, mail.at);
  assert.deepEqual(actual, expected.map(each => open(each, sender)));
  const [field] = Object.keys(payload);
  counts[field]++;
}

assert.equal(pending.size, 0);
assert.deepEqual(counts, { ownerRequest: 2, ownerReply: 2 });
console.log("Four multipart ownership exchanges match TypeScript's Work Queue mail byte fields.");
