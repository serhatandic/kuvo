// Exercise discovery through the real resident with recorded SSH probe lines.
import assert from "node:assert/strict";
import { test } from "node:test";
import { bench } from "./bench.mjs";

test("the probe distinguishes an older installation from another setup on the same computer", async t => {
  const { computer } = await bench(t);
  const local = await computer("Studio");
  const ask = local.connect(await local.start().ready());
  const key = Buffer.alloc(32, 21).toString("base64url");
  const setup = "set_" + Buffer.alloc(16, 22).toString("base64url");
  const card = { formatVersion: 1, setup, role: "primary", self: { key, name: "Lab", kind: "machine" }, computers: 1, items: 0, macs: [] };
  const probe = [
    "box machine .kuvo-code", `link mac ${key}`, `link name ${key} "Old Mac"`,
    `link seen ${key} 1`, "box app Library/Application Support/run.kuvo.code/box",
    `card ${JSON.stringify(card)}`,
  ].join("\n");
  const judge = probe => ask("POST", "kuvo/setup/judge", { probe });
  const reply = await judge(probe);
  assert.equal(reply.status, 200, reply.text);
  assert.deepEqual(reply.json, {
    verdict: "two", joins: false, boxes: [
      { folder: ".kuvo-code", app: false, verdict: "older", key: null, name: "", kind: null, setup: null, role: null, primary: { key, name: "Old Mac", heard: 1 }, macs: [{ key, name: "Old Mac" }], computers: null, items: null },
      { folder: "Library/Application Support/run.kuvo.code/box", app: true, verdict: "main", key, name: "Lab", kind: "machine", setup, role: "primary", primary: null, macs: [], computers: 1, items: 0 },
    ],
  });

  const lone = `box machine .kuvo-code\ncard ${JSON.stringify(card)}`;
  assert.deepEqual((await judge(lone)).json, { verdict: "main", joins: true, boxes: [
    { ...reply.json.boxes[1], folder: ".kuvo-code", app: false },
  ] });
  const withApp = await judge(lone + "\napp Library/Application Support/run.kuvo.code");
  assert.equal(withApp.json.joins, false);
  const member = await judge(lone.replace('"primary"', '"member"'));
  assert.equal(member.json.verdict, "elsewhere");

  const future = await judge(lone.replace('"formatVersion":1', '"formatVersion":2'));
  assert.equal(future.status, 409, future.text);
  assert.equal(future.json.code, "unsupportedFormatVersion");
  assert.match(future.json.message, /Setup discovery card format version 2/);
});
