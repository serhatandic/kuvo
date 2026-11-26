// Two real residents share signed setup state through a local relay.
import assert from "node:assert/strict";
import { test } from "node:test";
import path from "node:path";
import { boxBinary, readJSON, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

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

test("each computer offers what can be done with it from here", { timeout: 120000 }, async t => {
  const { computer, leases } = await bench(t, { presenceEvents: true });
  const main = await computer("Studio");
  const other = await computer("Lab");
  const primary = main.connect(await main.start().ready());
  const setup = (await primary("GET", "kuvo/setup")).json;
  const alone = await primary("GET", "kuvo/computers");
  assert.equal(alone.status, 200, alone.text);
  // The only computer can't be unlinked, and a test home isn't one that can be wiped from here.
  assert.deepEqual(alone.json.computers.map(row => row.actions.map(action => action.id)), [["rename"]]);
  const invite = await primary("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(setup.setup), "The main computer did not claim a relay lease.");
  const joined = await other.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  const labRun = other.start();
  const member = other.connect(await labRun.ready());
  const offered = async (ask, expected) => {
    let last;
    return until(async () => {
      const reply = await ask("GET", "kuvo/computers");
      assert.equal(reply.status, 200, reply.text);
      last = reply.json.computers.map(row => [row.name, row.actions]);
      const rows = Object.fromEntries(last);
      const ids = Object.fromEntries(last.map(([name, actions]) => [name, actions.map(action => action.id)]));
      return JSON.stringify(ids) === JSON.stringify(expected) && rows;
    }, () => `unexpected actions: ${JSON.stringify(last)}`, 30000);
  };
  const handOver = "Lab becomes the main computer first. Then your other computers stop reaching Studio. It keeps running, with its sessions, and you can add it again later.";
  const onMain = await offered(primary, { Studio: ["rename", "remove"], Lab: ["rename", "remove"] });
  assert.deepEqual(onMain.Studio[1].removal, { choices: ["unlink", "wipe"], handOver });
  assert.deepEqual(onMain.Lab[1].removal, { choices: ["unlink", "wipe"] });
  assert.deepEqual(onMain.Lab[1].confirm, { title: "Remove Lab?", label: "Remove",
    message: "Your other computers stop reaching Lab. It keeps running, with its sessions, and you can add it again later." });
  // In a managed setup a member can unlink itself too, and the main hands over first.
  const onMember = await offered(member, { Lab: ["rename", "remove"], Studio: ["rename", "remove"] });
  assert.equal(onMember.Lab[1].label, "Unlink");
  assert.deepEqual(onMember.Studio[0], { id: "rename", label: "Rename" });
  assert.equal(onMember.Studio[1].removal.handOver, handOver);
  // Connect, on a computer that's away: asked once, it's being reached until it answers.
  const labKey = (await member("GET", "kuvo/setup")).json.you.key;
  await labRun.stop();
  const away = await offered(primary, { Studio: ["rename", "remove"], Lab: ["rename", "connect", "remove"] });
  assert.deepEqual(away.Lab[1], { id: "connect", label: "Connect" });
  const connect = () => primary("POST", `kuvo/computers/${labKey}/connect`, {}, { "x-kuvo-from": "mac" });
  const asked = await connect();
  assert.equal(asked.status, 202, asked.text);
  const listed = await primary("GET", "kuvo/computers");
  assert.equal(listed.json.computers.find(row => row.name === "Lab").reach, "connecting");
  const again = await connect();
  assert.equal(again.status, 409, again.text);
  assert.equal(again.json.message, "Studio is reaching Lab now.");
  const self = await primary("POST", `kuvo/computers/${setup.you.key}/connect`, {}, { "x-kuvo-from": "mac" });
  assert.equal(self.status, 409, self.text);
  assert.equal(self.json.message, "Studio is the computer you're asking.");
});

test("a conversation on another computer is read live, and from its last copy while that computer is away", { timeout: 120000 }, async t => {
  const { computer, leases } = await bench(t, { presenceEvents: true });
  const main = await computer("Studio");
  const other = await computer("Lab");
  const session = "ses_far";
  const messages = [{ info: { id: "msg_far", sessionID: session, role: "user" }, parts: [{ type: "text", text: "Hello from Lab" }] }];
  other.engine.sessions.push(sessionOf({ id: session, directory: other.home, title: "Far" }));
  other.engine.messages.set(session, messages);
  const primary = main.connect(await main.start().ready());
  const setup = (await primary("GET", "kuvo/setup")).json;
  const invite = await primary("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(setup.setup), "The main computer did not claim a relay lease.");
  const joined = await other.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  const labRun = other.start();
  const member = other.connect(await labRun.ready());
  const lab = (await member("GET", "kuvo/setup")).json.you.key;
  const desktop = { "x-kuvo-from": "app" };
  const base = `kuvo/peer-session/${lab}/${session}`;
  const read = what => primary("GET", `${base}/${what}`, undefined, desktop);
  let last;
  const live = await until(async () => {
    last = await read("messages?limit=10");
    return last.status === 200 && !last.json.away && last;
  }, () => `the newest page did not come from Lab: ${last?.status} ${last?.text}`, 30000);
  assert.deepEqual(live.json.data, messages);
  const location = await read("location");
  assert.equal(location.status, 200, location.text);
  assert.equal(location.json.place, "engine");
  const about = await read("session");
  assert.equal(about.status, 200, about.text);
  // Lab reads its own conversation the same way.
  const own = await member("GET", `kuvo/peer-session/here/${session}/messages?limit=10`, undefined, desktop);
  assert.equal(own.status, 200, own.text);
  assert.deepEqual(own.json.data, messages);
  await labRun.stop();
  const kept = await until(async () => {
    last = await read("messages?limit=10");
    return last.status === 200 && last.json.away && last;
  }, () => `the last copy was not answered: ${last?.status} ${last?.text}`, 30000);
  assert.deepEqual(kept.json.data, messages);
  assert.equal(kept.json.away.computer.id, lab);
});
