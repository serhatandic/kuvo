import "./test-safety.js";
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { assertReceipt, deviceAt, link, network, until } from "./setup-handover-fixture.js";
import { operationID } from "./setup-removal-fixture.js";

async function relayLeaseNetwork(t, { ranked = false, standby = true, oldMember = false, resident = false } = {}) {
  const leaseRelay = relayLeaseStandIn();
  const net = await network(t, true, undefined, false, undefined, false, true, undefined, {
    leaseRelay,
    oldMember,
    resident,
    acceptedRoster: true,
    prepare([a, b]) {
      for (const box of [a, b]) {
        const current = box.read();
        if (standby) Object.assign(current.setup, { standby: { key: b.key, sign: b.sign }, numberCeiling: 500 });
        if (ranked) current.setup.successorChain = true;
        current.shared.queueNumberAllocation = { sequence: 1, value: { next: 401 } };
        fs.writeFileSync(path.join(box.run, "link.json"), JSON.stringify(current));
      }
    },
  });
  return { ...net, leaseRelay };
}

test("relay lease: a two-computer main pairs a phone and adds a computer with the other stopped", { timeout: 35_000 }, async t => {
  const { a, b, relay, join } = await relayLeaseNetwork(t);
  await b.stop();
  a.clock(65_000);

  const pairing = await a.route("POST", "kuvo/phones/pair");
  assert.equal(pairing.status, 200, JSON.stringify(pairing.value));
  const token = new URL(pairing.value.link).searchParams.get("token");
  const key = link.newKey();
  const phone = await deviceAt(relay, a, key, { name: "New phone", pair: token });
  assert.equal(phone.greeting?.save?.status, "committed", JSON.stringify(phone));
  t.after(() => phone.close());
  const pairedKey = link.publicOf(key).toString("base64url");
  assert.ok(a.read().shared.membershipAndAccess.value.devices.phones.some(phone => phone.key === pairedKey));

  const invitation = await a.route("POST", "kuvo/computers/add/code", {}, true);
  assert.equal(invitation.status, 201, JSON.stringify(invitation.value));
  const enrolled = await join(invitation.value.code);
  assert.equal(enrolled.setup.id, a.read().setup.id);
  assert.equal(enrolled.setup.primary, a.key);
  assert.ok(a.read().shared.membershipAndAccess.value.boxes.some(box => box.sign === link.signingKeyOf(enrolled.sign)));
  assert.equal(b.child.signalCode, "SIGKILL");
});

test("relay lease: restart grace keeps the same main even after its old grant expires", { timeout: 22_000 }, async t => {
  const { a, b, relay, leaseRelay, see } = await relayLeaseNetwork(t);
  await until("the main's first relay grant", () => leaseRelay.lease, AbortSignal.any([t.signal, AbortSignal.timeout(5_000)]));
  assert.match((await a.route("GET", "kuvo/setup")).value.leadershipWarning, /also runs Kuvo's relay/);
  const reconnect = leaseRelay.disconnect(a.key);
  const initial = leaseRelay.lease;
  leaseRelay.advance(125_000);
  leaseRelay.restart();
  relay.drop();
  const before = leaseRelay.calls.length;
  await until("a member checks the restarted relay", () => leaseRelay.calls.slice(before).some(call => call.computer === b.key), t.signal);
  await delay(600);
  await see(b, 2, a);
  assert.equal(leaseRelay.lease.term, initial.term);
  assert.equal(leaseRelay.lease.expires, initial.expires);

  reconnect();
  await until("the returning main renews the same relay term", () => leaseRelay.lease.expires > initial.expires, t.signal);
  await see(a, 2, a);
  await see(b, 2, a);
  assert.equal(leaseRelay.lease.term, initial.term);
});

test("relay lease: a cut-off main fences changes and steps down when the always-on member takes over", { timeout: 25_000 }, async t => {
  const { a, b, relay, leaseRelay, see } = await relayLeaseNetwork(t);
  const initial = await until("the main's first relay grant", () => leaseRelay.lease, AbortSignal.any([t.signal, AbortSignal.timeout(5_000)]));
  const returnLease = leaseRelay.disconnect(a.key);
  const returnMail = relay.disconnect(a.address);
  a.clock(65_000);

  const blocked = await a.route("POST", "kuvo/phones/pair");
  assert.equal(blocked.status, 409);
  assert.match(blocked.value.message, /Kuvo's relay can't be reached/);
  const work = await a.route("POST", "kuvo/work-queue/items", { title: "Everyday offline work" }, true, { "x-kuvo-operation": operationID() });
  assert.equal(work.status, 200, JSON.stringify(work.value));

  const previousHigh = leaseRelay.lease.high;
  leaseRelay.advance(125_000);
  await see(b, 3, b);
  assert.equal(leaseRelay.lease.term, initial.term + 1);
  assert.ok(previousHigh >= 500);
  const made = await b.route("POST", "kuvo/work-queue/items", { title: "Numbered after takeover" }, true, { "x-kuvo-operation": operationID() });
  assert.equal(made.status, 200, JSON.stringify(made.value));
  const number = await until("the new main allocates above the old main's high mark", async () => {
    const reply = await b.route("GET", `kuvo/work-queue/items/${made.value.item.id}`);
    return reply.value.item?.number;
  }, t.signal);
  assert.ok(number > previousHigh && number <= leaseRelay.lease.high);

  returnLease();
  returnMail();
  await see(a, 3, b);
  assert.equal((await a.route("GET", "kuvo/setup")).value.you.role, "member");
  assert.equal(leaseRelay.lease.holder, b.key);
});

test("main saves alone: after unlinking one of two computers new items get numbers", { timeout: 20_000 }, async t => {
  const { a, b, leaseRelay } = await relayLeaseNetwork(t, { ranked: true, standby: false });
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  const removal = await a.route("DELETE", `kuvo/computers/${b.key}`, { wipe: false });
  assert.equal(removal.status, 200, JSON.stringify(removal.value));
  const created = await a.route("POST", "kuvo/work-queue/items", { title: "After unlink" }, true, { "x-kuvo-operation": operationID() });
  assert.equal(created.status, 200, JSON.stringify(created.value));
  await until("item gets a number from the remaining main", async () => {
    const reply = await a.route("GET", `kuvo/work-queue/items/${created.value.item.id}`);
    return Number.isInteger(reply.value.item?.number);
  }, AbortSignal.any([t.signal, AbortSignal.timeout(4_000)]));
});

test("main saves alone: the new main pairs a phone after a two-computer takeover", { timeout: 20_000 }, async t => {
  const { a, b, relay, leaseRelay, see } = await relayLeaseNetwork(t);
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  await a.stop();
  leaseRelay.advance(125_000);
  await see(b, 3, b);
  const pairing = await b.route("POST", "kuvo/phones/pair");
  assert.equal(pairing.status, 200, JSON.stringify(pairing.value));
  const token = new URL(pairing.value.link).searchParams.get("token");
  const key = link.newKey();
  const phone = await deviceAt(relay, b, key, { name: "After takeover", pair: token });
  t.after(() => phone.close());
  assert.equal(phone.greeting?.save?.status, "committed");
  const pairedKey = link.publicOf(key).toString("base64url");
  assert.ok(b.read().shared.membershipAndAccess.value.devices.phones.some(phone => phone.key === pairedKey));
  const label = await b.route("POST", "kuvo/work-queue/labels", { add: "After takeover" }, true);
  assert.equal(label.status, 200, JSON.stringify(label.value));
  assert.ok(b.read().shared.queueLabels.value.labels.some(label => label.name === "After takeover"));
});

test("main saves alone: a dead standby is unlinked in one request", { timeout: 20_000 }, async t => {
  const { a, b, leaseRelay } = await relayLeaseNetwork(t);
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  await b.stop();
  const removal = await a.route("DELETE", `kuvo/computers/${b.key}`, { wipe: false });
  assert.equal(removal.status, 200, JSON.stringify(removal.value));
  assert.equal(a.read().setup.epoch, 2);
  assert.equal(a.read().setup.standby, undefined);
  assert.ok(!a.read().shared.membershipAndAccess.value.boxes.some(peer => peer.key === b.key));
});

test("main saves alone: a resident member catches up to a save committed while it was down", { timeout: 40_000 }, async t => {
  const { a, b, leaseRelay } = await relayLeaseNetwork(t, { standby: false, resident: true });
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  await b.stop();
  const { port, password, pid } = JSON.parse(fs.readFileSync(path.join(a.run, "serve.json"), "utf8"));
  assert.equal(pid, a.child.pid);
  const result = await fetch(`http://127.0.0.1:${port}/kuvo/computers/${b.key}`, {
    method: "PATCH",
    headers: { authorization: "Basic " + Buffer.from("opencode:" + password).toString("base64"),
      "x-kuvo-from": "mac", "content-type": "application/json" },
    body: JSON.stringify({ name: "Renamed while asleep" }),
    signal: t.signal,
  });
  const receipt = await result.json();
  assert.equal(result.status, 200, JSON.stringify(receipt));
  assertReceipt(a, "setupNames", "committed", receipt);
  assert.equal(a.read().shared.setupNames.value[b.key], "Renamed while asleep");
  b.start();
  await until("member copies the committed name and its receipt", () =>
    b.read().shared?.setupNames?.value[b.key] === "Renamed while asleep" &&
    b.read().sharedOutcomes?.[receipt.operation]?.status === "committed", t.signal);
});

for (const splitIndex of [0, 1]) {
  test(`mixed resident pilot: ${splitIndex === 0 ? "split" : "resident"} main hands over with receipts and a direct move`, { timeout: 80_000 }, async t => {
    const group = { id: "grp_pilot", name: "Pilot group", members: ["bot_one", "bot_two"], created: "2026-11-21T00:00:00Z", seats: {}, heard: {} };
    const { a, b, see, leaseRelay, groupCopies } = await network(t, true, undefined, false, group, false, true, ["Studio", "Lab"], {
      resident: true, splitIndex, acceptedRoster: true,
    });
    const split = [a, b][splitIndex];
    const ask = (box, ...args) => box === split ? box.route(...args) : box.residentRoute(...args);
    const term = leaseRelay.lease.term;
    assert.equal(leaseRelay.lease.holder, a.key);

    const saveName = async (main, member, name) => {
      const receipt = await ask(member, "PATCH", `kuvo/computers/${member.key}`, { name });
      assert.equal(receipt.status, 200, JSON.stringify(receipt));
      assertReceipt(main, "setupNames", "committed", receipt.value);
      const operation = receipt.value.operation;
      await until("the member receives the committed receipt", () => member.read().sharedOutcomes?.[operation]?.status === "committed", t.signal);
      assert.equal(member.read().shared.setupNames.value[member.key], name);
      return operation;
    };
    const before = await saveName(a, b, "Before handover");
    const handover = await ask(a, "POST", "kuvo/setup/handover", { primary: b.key, epoch: 2 });
    assert.ok([200, 202].includes(handover.status), JSON.stringify(handover));
    await Promise.all([see(a, 3, b), see(b, 3, b)]);
    assert.equal(leaseRelay.lease.holder, b.key);
    assert.equal(leaseRelay.lease.term, term + 1);
    assert.equal(b.read().sharedOutcomes[before].status, "committed");
    await saveName(b, a, "After handover");

    const folder = path.join(b.home, "groups", group.id);
    fs.mkdirSync(folder, { recursive: true });
    fs.writeFileSync(path.join(folder, "group.json"), JSON.stringify({ kuvo: 1, status: "active", group }));
    fs.writeFileSync(path.join(folder, "messages.json"), "[]");
    const moved = await ask(b, "POST", "kuvo/moves", { kind: "group", id: group.id, to: a.key, receipt: true });
    assert.equal(moved.status, 202, JSON.stringify(moved));
    const receiptRoute = `kuvo/moves/receipt?id=${moved.value.move.id}`;
    const done = await until("the source reports its move outcome", async () => {
      const receipt = await ask(b, "GET", receiptRoute);
      return receipt.status === 200 && receipt.value.state && receipt.value.state !== "moving" && receipt;
    }, t.signal);
    assert.equal(done.value.state, "done", JSON.stringify(done));
    assert.equal(done.value.move.to, a.key);
    assert.equal(groupCopies.get("Studio")?.status, "active");
    await until("the source exports and releases its old copy", () => !groupCopies.has("Lab"), t.signal);
  });
}

test("unlinking the main of two computers hands its role to the other, on an older version, and unlinks it", { timeout: 30_000 }, async t => {
  const { a, b, leaseRelay, see } = await relayLeaseNetwork(t, { oldMember: true });
  await until("main holds the lease", () => leaseRelay.lease?.holder === a.key, t.signal);
  const listed = (await a.route("GET", "kuvo/computers")).value.computers.find(computer => computer.key === a.key);
  assert.match(listed.actions.find(action => action.id === "remove")?.confirm?.message ?? "", /Desk becomes the main computer first/);

  const unlinked = await a.route("POST", `kuvo/computers/${a.key}/unlink`);
  assert.equal(unlinked.status, 202, JSON.stringify(unlinked.value));
  assert.deepEqual(unlinked.value, { primary: b.key, epoch: 3, unlinking: true, removal: { state: "handing-over", accessRemoved: false },
    message: "Desk is becoming the main computer. Then Studio is unlinked." });
  assert.equal(leaseRelay.lease.holder, b.key);
  await see(b, 3, b);
  // The relay starts the new main's grant when the old one ends.
  leaseRelay.advance(65_000);
  b.clock(65_000);
  await until("the new main unlinks the old one", () => !b.read().shared.membershipAndAccess.value.boxes.some(peer => peer.key === a.key), t.signal);
  assert.ok(b.read().shared.membershipAndAccess.value.left.includes(a.key));
  await until("the new main forgets the finished unlink", () => b.read().unlinkAfterHandover === undefined, t.signal);
  await until("the old main lets the setup go", () => a.read().setup?.primary !== a.key, t.signal);
});
