import "./safety"
import { expect, test } from "bun:test";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { K } from "../src/link/home";
import { filesOf } from "../src/link/work-queue/store";
import { clearQueuePlacement, placeWorkQueues, type QueuePayload, type QueuePlacement } from "../src/link/work-queue/moves";

test("destination rejects truncated and same-length changed staged queue attachments before publishing", () => {
  const opened = fs.mkdtempSync(path.join(K, "queue-move-corruption-"));
  const owner = "ses_movevalidation";
  const id = "wqi_AAAAAAAAAAAAAAAAAAA1";
  const name = "proof.lock";
  const file = path.join(opened, "work-queue", id, name);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const bytes = Buffer.of(0, 255, 4);
  const sha256 = crypto.createHash("sha256").update(bytes).digest("base64url");
  const carried = { items: [{ id, owner, title: "Attachment proof", status: "todo", number: 1, created: { at: 1, by: "you" },
    updated: { at: 1, by: "you" }, files: [{ name, size: bytes.length, sha256, type: "application/octet-stream", at: 1 }] }] };
  for (const changed of [Buffer.of(0), Buffer.of(0, 255, 5)]) {
    fs.writeFileSync(file, changed);
    expect(() => placeWorkQueues(carried, [owner], opened, true)).toThrow(/managed file/i);
    expect(fs.existsSync(path.join(filesOf(id), name))).toBe(false);
  }
});

test("destination refuses a raw queue record whose file would be filtered out", () => {
  const owner = "ses_movevalidation";
  const carried = { items: [{ id: "wqi_AAAAAAAAAAAAAAAAAAA2", owner, title: "Invalid promise", files: [{ name: "promised.lock", size: "3" }] }] };
  expect(() => placeWorkQueues(carried, [owner], null, true)).toThrow("Invalid carried Work Queue file list");
});


test("a failed queue import rolls back bytes published before the queue record", () => {
  const opened = fs.mkdtempSync(path.join(K, "queue-move-interrupted-"));
  const owner = "ses_moveinterrupted";
  const ids = ["wqi_AAAAAAAAAAAAAAAAAAB1", "wqi_AAAAAAAAAAAAAAAAAAB2"];
  const name = "proof.lock";
  const bytes = Buffer.of(0, 255, 9);
  const sha256 = crypto.createHash("sha256").update(bytes).digest("base64url");
  const items = ids.map(id => ({ id, owner, title: "Attachment proof", status: "todo", files: [{ name, size: bytes.length, sha256 }] }));
  for (const id of ids) {
    const staged = path.join(opened, "work-queue", id, name);
    fs.mkdirSync(path.dirname(staged), { recursive: true });
    fs.writeFileSync(staged, bytes);
  }
  const move = "mov_rollback";
  let recorded: { payloads: QueuePayload[]; placement: QueuePlacement } | undefined;
  expect(() => placeWorkQueues({ items }, [owner], opened, true, {
    id: move,
    beforePlacement: (payloads, placement) => {
      recorded = { payloads, placement };
      const obstruction = path.join(opened, "queue-install", ids[1]!);
      fs.mkdirSync(path.dirname(obstruction));
      fs.writeFileSync(obstruction, "second item cannot be installed");
    },
  })).toThrow();
  expect(recorded).toBeDefined();
  expect(fs.readFileSync(path.join(filesOf(ids[0]!), name))).toEqual(bytes);
  clearQueuePlacement(recorded!.payloads, move, recorded!.placement);
  expect(fs.existsSync(filesOf(ids[0]!))).toBe(false);
});
