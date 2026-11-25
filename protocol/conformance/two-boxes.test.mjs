// Two real residents share signed setup state through a local relay.
import assert from "node:assert/strict";
import { test } from "node:test";
import path from "node:path";
import { boxBinary, readJSON, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

test("two computers agree on membership and a rename travels through the relay", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Studio");
  const other = await computer("Lab");
  const primary = main.connect(await main.start().ready());
  const original = await primary("GET", "kuvo/setup");
  assert.equal(original.status, 200);
  assert.equal(original.json.you.role, "primary");
  const invite = await primary("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(original.json.setup), "The main computer did not claim a relay lease.");
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
  if (!boxBinary().typescript) {
    const observed = home => {
      try { return readJSON(path.join(home, "run", "setup-relay-lease.json")); } catch { return null; }
    };
    await until(() => observed(other.home)?.expires === leases.leaseFor(original.json.setup)?.expires,
      "The member must observe the main's authoritative lease.");
    const calls = leases.calls.length;
    await new Promise(resolve => setTimeout(resolve, 3200));
    assert.equal(leases.calls.length, calls, "A settled pair must not poll the relay between lease deadlines.");
  }
});


test("solo numbers stay unique when another computer joins", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const main = await computer("Solo"), lab = await computer("Lab");
  let resident = main.start();
  let ask = main.connect(await resident.ready());
  const setup = (await ask("GET", "kuvo/setup")).json;
  const ts = boxBinary().typescript;
  if (ts) await until(() => leases.leaseFor(setup.setup), "the TypeScript reference claims its solo lease");
  else {
    await new Promise(resolve => setTimeout(resolve, 2600));
    assert.equal(leases.calls.length, 0, "an idle single computer does not read or renew a relay lease");
  }
  const app = { "x-kuvo-from": "app" };
  const add = async (call, operation) => {
    const made = await call("POST", "kuvo/work-queue/items", { title: operation, operation: operation.toLowerCase().replaceAll(" ", "-") }, app);
    assert.equal(made.status, 200, made.text);
    return until(async () => (await call("GET", "kuvo/work-queue/items/" + made.json.item.id)).json?.item?.number,
      "the Work Queue item must be numbered", 30000);
  };
  const solo = [await add(ask, "Solo first"), await add(ask, "Solo second")];
  assert.notEqual(solo[0], solo[1]);
  if (!ts) assert.equal(leases.calls.length, 0, "local numbering needs no relay either");
  // Keep a persisted range reserved by a merge, as the number ledger does. The actual
  // merge scenarios belong to setup merging; this proves the bootstrap's high-water mark.
  await resident.stop();
  const file = path.join(main.home, "run", "work-queue", "numbers.json");
  const ledger = readJSON(file);
  const from = Math.max(800, ledger.next ?? 1), to = from + 19;
  const required = to + 100;
  write(file, JSON.stringify({ ...ledger, next: to + 1, reserveThrough: required,
    given: { ...ledger.given, "merge:idle-fixture": { by: setup.you.key, from, to, granted: [], at: Date.now() } } }));
  resident = main.start();
  ask = main.connect(await resident.ready());
  if (!ts) assert.equal(leases.calls.length, 0, "restarting the solo computer does not claim a lease");
  const invite = await ask("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(setup.setup)?.high >= required,
    "the first invitation must carry persisted and reserved numbers to the relay", 25000);
  const joined = await lab.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  const far = lab.connect(await lab.start().ready());
  const newNumbers = await Promise.all([add(ask, "Main after join"), add(far, "Member after join")]);
  assert.equal(new Set([...solo, ...newNumbers]).size, 4, "joining must never issue an already allocated number");
  assert.ok(newNumbers.every(number => number < from || number > to), "the merge's reserved range stays reserved");
  const member = (await far("GET", "kuvo/setup")).json.you.key;
  const beforeRemoval = leases.leaseFor(setup.setup).high;
  const removed = await ask("DELETE", `kuvo/computers/${member}`, { wipe: false }, { "x-kuvo-from": "mac" });
  assert.equal(removed.status, 200, removed.text);
  if (!ts) {
    await until(() => Object.keys(leases.leaseFor(setup.setup)?.roster ?? {}).join() === setup.you.key,
      "The relay must fence the removed member before the now-solo computer stops renewing.");
    const calls = leases.calls.filter(call => call.computer === setup.you.key).length;
    const after = await add(ask, "Solo after removal");
    assert.ok(![...solo, ...newNumbers].includes(after), "a local number must not reuse a granted number");
    await new Promise(resolve => setTimeout(resolve, 2600));
    assert.equal(leases.calls.filter(call => call.computer === setup.you.key).length, calls,
      "The sole remaining main does not renew its relay lease.");
    assert.ok(leases.leaseFor(setup.setup).high >= beforeRemoval, "The relay floor does not go backwards.");
  }
});
