// Two real residents share signed setup state through a local relay.
import assert from "node:assert/strict";
import { test } from "node:test";
import { until } from "./adapter.mjs";
import { bench } from "./bench.mjs";

test("two computers agree on membership and a rename travels through the relay", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const other = await computer("Lab");
  const primary = main.connect(await main.start().ready());
  const original = await primary("GET", "kuvo/setup");
  assert.equal(original.status, 200);
  assert.equal(original.json.you.role, "primary");
  await until(() => leases.leaseFor(original.json.setup), "The main computer did not claim a relay lease.");
  const invite = await primary("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  const joined = await other.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  const member = other.connect(await other.start().ready());
  const theirs = await member("GET", "kuvo/setup");
  const ours = await primary("GET", "kuvo/setup");
  assert.equal(theirs.status, 200, theirs.text);
  assert.equal(ours.status, 200, ours.text);
  assert.equal(theirs.json.setup, original.json.setup);
  assert.equal(theirs.json.you.role, "member");
  const key = theirs.json.you.key;
  const roster = json => json.computers.map(computer => [computer.key, computer.name, computer.role]);
  const expected = [[original.json.you.key, "Studio", "primary"], [key, "Lab", "member"]];
  assert.deepEqual(roster(ours.json), expected);
  assert.deepEqual(roster(theirs.json), expected);

  const rename = await primary("PATCH", `kuvo/computers/${key}`, { name: "Workshop" }, { "x-kuvo-from": "mac" });
  assert.equal(rename.status, 200, rename.text);
  const changed = await until(async () => {
    const reply = await member("GET", "kuvo/setup");
    assert.equal(reply.status, 200, reply.text);
    return reply.json.computers.some(computer => computer.key === key && computer.name === "Workshop") && reply.json;
  }, "The member did not receive the saved computer name.", 20000);
  assert.deepEqual(roster(changed), [[original.json.you.key, "Studio", "primary"], [key, "Workshop", "member"]]);
});
