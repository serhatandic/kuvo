import "./test-safety.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { testBun } from "./bun-for-tests.js";
import { recoveryNeighborBuild } from "./failure-bench-legacy.js";
import { put } from "./recovery-bench.js";
import { deviceAt, link, network, peerOwns, rename, submit, until } from "./setup-handover-fixture.js";

test("failure bench: an older receiver refuses v3 move evidence and ignores evidence handoffs", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-old-evidence-"));
  try {
    const artifact = path.join(root, "neighbor.mjs");
    // Expose the bundled first reader without changing its implementation.
    fs.writeFileSync(artifact, Buffer.concat([recoveryNeighborBuild(), Buffer.from("\nexport { unpack as readMoveFixture };\n")]));
    const home = path.join(root, ".kuvo-code");
    const ran = spawnSync(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
      import assert from "node:assert/strict";
      import fs from "node:fs";
      import path from "node:path";
      import { pack, unpack } from ${JSON.stringify(new URL("../src/link/moves/bundle.ts", import.meta.url).href)};
      import * as old from ${JSON.stringify(pathToFileURL(artifact).href)};
      const home = process.env.KUVO_HOME;
      fs.mkdirSync(path.join(home, "run"), { recursive: true });
      fs.writeFileSync(path.join(home, "folder-version.json"), '{"version":7}');
      const own = Buffer.alloc(32, 42);
      const sender = { key: old.publicOf(Buffer.alloc(32, 43)).toString("base64url"), name: "Peer", address: "A".repeat(22), kind: "machine" };
      const key = old.publicOf(own).toString("base64url");
      const sign = old.newSigningSeed();
      const signer = old.signingKeyOf(sign);
      const setup = { id: "set_" + "A".repeat(22), primary: key, sign: signer, role: "primary", managed: true, checkpointed: true, epoch: 1 };
      old.writeLink({ key: own.toString("base64url"), sign, setup, sharedSetup: setup.id,
        shared: { membershipAndAccess: { sequence: 1, value: { left: [], boxes: [{ ...sender, key, sign: signer }, sender] } } } });
      const contents = { kind: "group", thing: "grp_fixture", name: "Fixture", exports: [], wasPaused: false };
      await pack("mov_legacy", { ...contents });
      assert.equal((await old.readMoveFixture("mov_legacy")).contents.thing, contents.thing);
      await pack("mov_evidence", { ...contents, forwardEvidence: {} });
      assert.ok((await unpack("mov_evidence")).contents.forwardEvidence);
      await assert.rejects(old.readMoveFixture("mov_evidence"), /manifest format version 3 is unsupported/i);
      const hand = { id: "wqh_abcdefghijklmno", to: "ses_fixture", from: "usr_you",
        item: { id: "wqi_" + "a".repeat(20), title: "Fixture", status: "todo" }, files: [] };
      let admissions = 0;
      const places = () => { admissions++; return { targets: [] }; };
      await old.takeWorkQueueMail(sender, { evidenceHand: { ...hand, evidence: {} } }, null, places);
      assert.equal(admissions, 0, "evidence handoff cannot reach the old admission adapter");
      await old.takeWorkQueueMail(sender, { hand }, null, places);
      assert.equal(admissions, 1, "the same old receiver recognizes the deployed handoff");
      assert.equal(fs.existsSync(path.join(home, "run", "owned-data", "mutations")), false);
    `], { encoding: "utf8", timeout: 30_000, env: { ...process.env, HOME: root, KUVO_HOME: home } });
    assert.equal(ran.status, 0, ran.stderr || String(ran.error ?? ran.signal));
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("failure bench: a real idle group move survives source loss and replacement restore", { timeout: 105_000 }, async t => {
  const group = { id: "grp_recovery", name: "Recovered group", members: ["bot_one", "bot_two"], created: "2026-11-01T00:00:00Z", seats: {}, heard: {} };
  const { a, b, c, groupCopies, relay, phase } = await network(t, false, undefined, false, group, false, false, undefined, { phaseDiagnostics: true });
  const groupPath = "groups/" + group.id;
  const phone = link.newKey();
  const invite = await a.route("POST", "kuvo/phones/pair");
  const paired = await deviceAt(relay, a, phone, { pair: new URL(invite.value.link).searchParams.get("token"), name: "Receipt fixture" });
  assert.ok(paired.greeting);
  t.after(paired.close);
  const at = box => until("paired Work Queue client on " + box.name, async () => {
    const device = await deviceAt(relay, box, phone);
    if (!device.greeting) return false;
    t.after(device.close);
    return device;
  }, t.signal);
  const lister = await at(b);
  phase("paired listing Box");
  const made = await c.route("POST", "kuvo/work-queue/items", { owner: group.id, title: "Before forwarding", operation: "queue-move-create" }, true);
  assert.equal(made.status, 200, JSON.stringify(made));
  const item = made.value.item.id;
  phase("created item in the source group queue");
  const itemPath = `kuvo/work-queue/items/${item}`;
  await until("the listing Box can read the group queue item", async () => (await lister.ask("GET", itemPath)).status === 200, t.signal);
  const headers = { "x-kuvo-operation": "queue-move-patch" };
  const intent = { title: "Accepted before move" };
  phase("listing Box can see the queued item");
  const forwarded = await lister.ask("PATCH", itemPath, intent, headers);
  assert.equal(forwarded.status, 200, JSON.stringify(forwarded));
  const saved = crypto.createHash("sha256").update(JSON.stringify(["scoped-forward", c.read().setup.id, headers["x-kuvo-operation"]])).digest("base64url");
  const savedPath = box => path.join(box.run, "owned-data", "mutations", saved + ".json");
  assert.equal(JSON.parse(fs.readFileSync(savedPath(c), "utf8")).forward.kind, "forwardedQueueDecisions");
  phase("source accepted the forwarding decision");
  const folder = fs.realpathSync(c.env.TMPDIR) + "/backups";
  fs.mkdirSync(folder);
  const destination = { kind: "disk", path: folder };
  const source = "b_" + crypto.randomBytes(16).toString("base64url");
  const recoveryKey = crypto.randomBytes(32).toString("base64url");
  put(path.join(c.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source, key: recoveryKey, dest: destination,
    disk: fs.statSync(folder).dev, projects: [],
  }));
  const backup = await c.route("POST", "kuvo/box/backups/now", {}, true);
  assert.equal(backup.status, 200, JSON.stringify(backup.value));
  const point = backup.value.point?.id;
  assert.ok(point);
  phase("captured the source backup");
  const moved = await c.route("POST", "kuvo/moves", { kind: "group", id: group.id, to: a.key, receipt: true }, true);
  assert.equal(moved.status, 202, JSON.stringify(moved.value));
  const receipt = path.join(c.run, "moves", moved.value.move.id + ".outcome.json");
  await until("move completed", () => fs.existsSync(receipt), t.signal);
  assert.equal(groupCopies.get("Studio")?.status, "active", JSON.stringify(JSON.parse(fs.readFileSync(receipt))));
  assert.equal(groupCopies.has("Lab"), false);
  phase("group move committed");
  assert.equal(JSON.parse(fs.readFileSync(savedPath(a), "utf8")).forward.owner, c.key);
  const destinationClient = await at(a);
  const retitled = await destinationClient.ask("PATCH", itemPath, { title: "New title at destination" });
  assert.equal(retitled.status, 200, JSON.stringify(retitled));
  const replay = await destinationClient.ask("PATCH", itemPath, intent, headers);
  assert.equal(replay.status, 200, JSON.stringify(replay));
  assert.equal(replay.value.item.title, intent.title);
  assert.equal((await destinationClient.ask("GET", itemPath)).value.item.title, "New title at destination");
  phase("destination replay recovered the old reply without changing the later title");
  await c.stop("SIGTERM");
  fs.rmSync(c.env.HOME, { recursive: true });
  assert.equal((await a.route("DELETE", "kuvo/computers/" + c.key)).status, 200);
  const matched = await a.route("POST", "kuvo/box/backups/match", { source, previous: c.key, replacement: b.key }, true);
  assert.equal(matched.status, 200, JSON.stringify(matched.value));
  await until("replacement ownership replica", () => b.read().shared?.ownershipRecovery?.value?.["replacement:" + c.key], t.signal);

  phase("replacement assignment verified");
  await until("the replacement heard the destination announce the group", () => peerOwns(b, a, group.id), t.signal);
  const recovered = await b.route("POST", "kuvo/box/backups/restore", { destination, source, id: point, recoveryKey, confirm: true }, true);
  assert.equal(recovered.status, 200, JSON.stringify(recovered.value));
  assert.ok(recovered.value.unavailable.some(entry => entry.entry === groupPath + "/group.json" && entry.reason === "moved"));
  assert.equal(fs.existsSync(path.join(b.env.HOME, groupPath, "group.json")), false);
  assert.equal(groupCopies.get("Studio")?.status, "active");
  assert.equal(fs.existsSync(savedPath(b)), false, "restore did not install an outcome for the moved item");
  assert.ok(recovered.value.held.includes(`home/run/owned-data/mutations/${saved}.json`));
});

test("failure bench: an away owner leaves saved reads and accepts an unsent message on return", { timeout: 90_000 }, async t => {
  const { a, b, c, relay, prompts, groupMessages } = await network(t, false, undefined, true);
  await until("both computers know the conversation and group owner", () =>
    [a, b].every(box => peerOwns(box, c, "ses_owned") && peerOwns(box, c, "grp_lab")), t.signal);
  const read = (box, query = "") => box.route("GET", `kuvo/data/owner?kind=conversations&id=ses_owned&computer=${c.key}${query}`);
  const before = await read(b);
  assert.equal(before.status, 200);
  assert.equal(before.value.owner.key, c.key);
  assert.equal(before.value.owner.name, "Lab");
  assert.equal(before.value.owner.reachable, true);
  assert.equal(before.value.state, "received");
  assert.equal(before.value.refresh, "live");
  assert.deepEqual(before.value.data.messages, []);
  assert.ok(before.value.lastReceivedAt > 0);
  const group = "kuvo/data/owner?kind=groupTranscript&id=grp_lab";
  const groupRead = await b.route("GET", group);
  assert.equal(groupRead.value.owner.key, c.key);
  assert.deepEqual(groupRead.value.data.messages, []);

  const rejoin = relay.disconnect(c.address);
  await c.stop("SIGTERM");
  const stale = await read(b, "&file=plan.md&message=msg_unknown");
  assert.equal(stale.value.owner.reachable, false);
  assert.equal(stale.value.stale, true);
  assert.equal(stale.value.state, "received");
  assert.equal(stale.value.refresh, "unavailable");
  assert.equal(stale.value.lastReceivedAt, before.value.lastReceivedAt);
  assert.deepEqual(stale.value.unavailable, { files: ["plan.md"], messages: ["msg_unknown"] });
  const unseen = await read(a);
  assert.equal(unseen.value.state, "neverReceived");
  assert.equal(unseen.value.refresh, "unavailable");
  assert.equal(unseen.value.lastReceivedAt, null);
  assert.equal(Object.hasOwn(unseen.value, "data"), false);

  const message = { id: "msg_owned", words: "This belongs to Lab." };
  const sent = await b.route("POST", "kuvo/sessions/ses_owned/send", message);
  assert.equal(sent.status, 202);
  const operation = sent.value.operation;
  const check = () => b.route("GET", `kuvo/data/messages/${operation}`);
  assert.equal((await check()).value.status, "pending");
  assert.deepEqual(prompts, []);
  const groupSend = await b.route("POST", `kuvo/data/messages?group=grp_lab&computer=${c.key}`, { id: "msg_group", text: "This belongs in Lab’s group." });
  assert.equal(groupSend.status, 202);
  const groupReceipt = () => b.route("GET", `kuvo/data/messages/${groupSend.value.operation}`);
  assert.equal((await groupReceipt()).value.status, "pending");
  assert.deepEqual(groupMessages, []);

  // The sender restarts, and the owner accepts without the primary in the network.
  await Promise.all([b.stop("SIGTERM"), a.stop("SIGTERM")]);
  b.start();
  assert.equal((await check()).value.status, "pending");
  rejoin();
  const releaseReceipt = relay.pause(c.address, b.address);
  c.start();
  await until("owner accepted the prompt", () => prompts.length === 1, t.signal);
  assert.equal((await check()).value.status, "pending", "the sender has no receipt yet");
  await c.stop("SIGTERM");
  releaseReceipt();
  c.start();
  // Wait for the durable receipt without starting a new CLI process on every poll.
  const savedReceipt = async (id, description, readStatus) => {
    const file = path.join(b.run, "owned-data", "outbox", id + ".json");
    await until(description, () => JSON.parse(fs.readFileSync(file, "utf8")).receipt.status !== "pending",
      AbortSignal.any([t.signal, AbortSignal.timeout(30_000)]));
    return (await readStatus()).value;
  };
  const receipt = await savedReceipt(operation, "conversation acceptance to reach the sender", check);
  assert.equal(receipt.status, "committed");
  assert.equal(receipt.operation, operation);
  assert.equal(receipt.owner, c.key);
  assert.ok(receipt.acceptedAt >= before.value.lastReceivedAt);
  assert.equal((await b.route("POST", "kuvo/sessions/ses_owned/send", message)).value.status, "committed");
  assert.equal((await c.route("POST", "kuvo/sessions/ses_owned/send", message)).status, 200);
  assert.deepEqual(prompts.map(prompt => prompt.id), [message.id]);
  assert.deepEqual((await read(b)).value.data.messages.map(prompt => prompt.id), [message.id]);
  const groupOutcome = await savedReceipt(groupSend.value.operation, "group acceptance to reach the sender", groupReceipt);
  assert.equal(groupOutcome.status, "committed");
  assert.deepEqual(groupMessages.map(message => message.id), ["msg_group"]);
});

test("failure bench: a member runs its own conversation and Work Queue while the primary is away", { timeout: 35_000 }, async t => {
  const { a, b, c, prompts } = await network(t, false, undefined, true);
  await a.stop("SIGTERM");
  assert.equal(c.read().setup.primary, a.key);
  assert.equal((await c.route("POST", "kuvo/sessions/ses_owned/send", { id: "msg_local", words: "Keep going." })).status, 200);
  assert.deepEqual(prompts.map(prompt => prompt.id), ["msg_local"]);
  const work = await c.route("POST", "kuvo/work-queue/items", { owner: "ses_owned", operation: "continue-on-lab", title: "Continue on Lab" }, true);
  assert.equal(work.status, 200);
  assert.equal((await c.route("GET", `kuvo/work-queue/items/${work.value.item.id}`)).value.item.title, "Continue on Lab");
  const create = { owner: "ses_owned", title: "Desk sends to Lab", operation: "one-create-from-desk" };
  const forwarded = await b.route("POST", "kuvo/work-queue/items", create, true);
  assert.equal(forwarded.status, 200);
  assert.deepEqual(await b.route("POST", "kuvo/work-queue/items", create, true), forwarded);
  assert.deepEqual(await c.route("POST", "kuvo/work-queue/items", create, true), forwarded);
  assert.equal((await c.route("GET", `kuvo/work-queue/items/${forwarded.value.item.id}`)).value.item.title, "Desk sends to Lab");
});

test("owner read distinguishes deletion, failed refresh and unavailable files on pc4", { timeout: 60_000 }, async t => {
  const { a, b, c, relay, see, showOwnedConversation, breakOwnedRead, refuseOwnedLookup, setOwnedMessages } = await network(t, true, undefined, true);
  await see(c, 2, a);
  await until("primary leased", () => a.lease(2, b), t.signal);
  // A completed move keeps the owner known even after both runtimes stop listing the session.
  const owner = { session: "ses_owned", key: c.key, at: Date.now() };
  put(path.join(b.run, "moves", "moved.json"), JSON.stringify([owner]));
  put(path.join(c.run, "moves", "moved.json"), JSON.stringify([{ ...owner, arrived: true }]));
  const query = `kuvo/data/owner?kind=conversations&id=ses_owned&computer=${c.key}`;
  const read = async (box = b, extra = "") => {
    const reply = await box.route("GET", query + extra);
    assert.equal(reply.status, 200, JSON.stringify(reply.value));
    return reply.value;
  };
  const empty = await read();
  assert.deepEqual(empty.data.messages, []);
  assert.equal(empty.owner.key, c.key);
  assert.equal(empty.state, "received");
  assert.equal(empty.refresh, "live");
  assert.equal(empty.stale, false);
  assert.ok(empty.lastReceivedAt > 0);

  const config = await rename(a, c, "Archive Lab");
  assert.equal(config.status, 200, JSON.stringify(config.value));
  await until("pc4 has the accepted name", () => b.read().shared?.setupNames?.value?.[c.key] === "Archive Lab", t.signal);
  const managed = path.join(c.home, "attachments", "ses_owned", "fixture", "managed.txt");
  put(managed, "managed bytes");
  const messages = [{ id: "msg_kept", files: [{ name: "inline.txt", source: { type: "inline" }, data: "aGk=" }, { name: "missing.txt" }],
    content: [{ type: "tool", state: { status: "completed", content: [{ type: "file", uri: pathToFileURL(managed).href }] } }] }];
  setOwnedMessages(messages);
  const refs = "&file=arbitrary&message=msg_kept&message=msg_elsewhere";
  const saved = await read(b, refs);
  assert.equal(saved.owner.name, "Archive Lab");
  assert.deepEqual(saved.unavailable, { files: ["inline.txt", "missing.txt", "managed.txt", "arbitrary"], messages: ["msg_elsewhere"] });
  const local = await read(c, refs);
  assert.deepEqual(local.unavailable.files, ["missing.txt", "arbitrary"]);
  fs.unlinkSync(managed);
  assert.deepEqual((await read(c, refs)).unavailable.files, ["missing.txt", "managed.txt", "arbitrary"]);
  await read(b, "&offset=older");

  const withheld = (await b.route("GET", query.replace("conversations", "pendingSessionWork"))).value;
  assert.equal(withheld.refresh, "withheld");
  assert.equal(withheld.lastReceivedAt, null);
  breakOwnedRead(503);
  const failed = await read(b, refs);
  assert.equal(failed.refresh, "failed");
  assert.equal(failed.owner.reachable, true);
  assert.equal(failed.stale, true);
  assert.equal(failed.lastReceivedAt, saved.lastReceivedAt);
  assert.deepEqual(failed.data.messages, messages);
  breakOwnedRead(403);
  const blocked = await read();
  assert.equal(blocked.refresh, "withheld");
  assert.equal(blocked.lastReceivedAt, null);
  assert.equal(Object.hasOwn(blocked, "data"), false);
  breakOwnedRead(503);
  const failedAfterRefusal = await read(b, "&offset=older");
  assert.equal(failedAfterRefusal.refresh, "failed");
  assert.equal(failedAfterRefusal.state, "neverReceived");

  breakOwnedRead(0);
  await read();
  refuseOwnedLookup(true);
  const refusedLookup = await read();
  // Its owner and conversation are already known, so a lookup refusal cannot withhold
  // an otherwise live messages response.
  assert.equal(refusedLookup.refresh, "live");
  assert.deepEqual(refusedLookup.data.messages, messages);
  refuseOwnedLookup(false);

  showOwnedConversation(false, { deleted: true });
  const absent = await until("owner confirms deletion", async () => {
    const value = await read();
    return value.state === "absent" && value;
  }, AbortSignal.any([t.signal, AbortSignal.timeout(8_000)]));
  assert.equal(absent.state, "absent");
  assert.equal(absent.refresh, "live");
  assert.equal(absent.stale, false);
  assert.ok(absent.lastReceivedAt >= saved.lastReceivedAt);
  assert.equal(Object.hasOwn(absent, "data"), false);
  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await c.stop("SIGTERM");
  const staleAbsence = await read(b, "&offset=older&file=missing.txt&message=msg_kept");
  assert.equal(staleAbsence.state, "absent");
  assert.equal(staleAbsence.refresh, "unavailable");
  assert.equal(staleAbsence.owner.name, "Archive Lab");
  assert.equal(staleAbsence.owner.reachable, false);
  assert.equal(staleAbsence.stale, true);
  assert.equal(staleAbsence.lastReceivedAt, absent.lastReceivedAt);
  assert.deepEqual(staleAbsence.unavailable, { files: ["missing.txt"], messages: ["msg_kept"] });

  showOwnedConversation(true);
  breakOwnedRead(0);
  c.start();
  reconnect();
  await until("pc5 is back and serves a new page", async () => (await read()).refresh === "live", t.signal);
  breakOwnedRead(503);
  const oldOffset = await read(b, "&offset=older");
  assert.equal(oldOffset.state, "neverReceived");
  assert.equal(oldOffset.refresh, "failed");
  assert.equal(oldOffset.lastReceivedAt, null);
});

for (const old of [false, true]) test(
  old ? "an updated sender uses a real beta.35 owner" : "a behind owner accepts relayed conversation prompts and forwarded Work Queue mutations",
  { timeout: 45_000 }, async t => {
  const version = "0.10.0", sha256 = "a".repeat(64);
  const signed = Buffer.from(JSON.stringify({ manifest: Buffer.from(JSON.stringify({ clientProtocol: 1 })).toString("base64") })).toString("base64");
  const prepare = ([a, b, c]) => {
    // Give each closed store its accepted snapshot before any keeper can write over it.
    for (const box of [a, b, c]) {
      const state = box.read();
      state.setup.checkpointed = true;
      state.sharedSetup = state.setup.id;
      state.shared = { ...state.shared, setupSecurityAndReleaseChoice: { sequence: 1, value: { version, sha256, signed } } };
      fs.writeFileSync(path.join(box.run, "link.json"), JSON.stringify(state));
      const bin = path.join(box.home, "bin");
      fs.mkdirSync(bin, { recursive: true });
      fs.writeFileSync(path.join(bin, "box-version"), box === c ? "0.8.0-beta.35" : version);
      if (box !== c) {
        fs.mkdirSync(path.join(box.home, "updates"), { recursive: true });
        fs.writeFileSync(path.join(box.home, "updates", "installed.json"), JSON.stringify({ version, sha256 }));
      }
    }
  };
  const { b, c, prompts } = await network(t, false, undefined, true, undefined, false, false, undefined, { beta35Owner: old, prepare });
  await until("the sender knows the conversation owner", () => {
    try {
      const peer = JSON.parse(fs.readFileSync(path.join(b.run, "peer-agents.json"), "utf8"))[c.key];
      return peer?.list.some(entry => entry.session === "ses_owned");
    } catch { return false; }
  }, t.signal);
  if (old) {
    await until("the sender heard the real beta.35 owner", () => {
      try {
        const peer = JSON.parse(fs.readFileSync(path.join(b.run, "peer-agents.json"), "utf8"))[c.key];
        return peer?.computer?.kuvo === "0.8.0-beta.35" && !peer.features.includes("update.fleet");
      } catch { return false; }
    }, t.signal);
  }
  const headers = { "x-kuvo-client-version": version };
  const create = { owner: "ses_owned", title: "Mixed-version queue", operation: "fleet-admission-queue" };
  const queued = await b.route("POST", "kuvo/work-queue/items", create, true, headers);
  assert.equal(queued.status, 200, JSON.stringify(queued));
  if (old) return; // beta.35 supports queue forwarding, before the scoped prompt wire format.
  const message = { id: "msg_fleetadmission", words: "Run on the owner while it updates." };
  const sent = await b.route("POST", "kuvo/sessions/ses_owned/send", message, true, headers);
  assert.equal(sent.status, 202, JSON.stringify(sent));
  const receipt = () => b.route("GET", `kuvo/data/messages/${sent.value.operation}`, undefined, true, headers);
  await until("the prompt commits while the owner runs the older release", async () => (await receipt()).value.status === "committed", t.signal);
  assert.deepEqual(prompts.map(prompt => prompt.id), [message.id]);
});

test("failure bench: an offline member follows two handovers and the former primary can be unlinked", { timeout: 50_000 }, async t => {
  const { a, b, c, see, phone, pairing } = await network(t);
  assert.deepEqual(await submit(c, b, 2), { status: 202, value: { primary: b.key, epoch: 3 } });
  await Promise.all([see(a, 3, b), see(b, 3, b), see(c, 3, b)]);
  assert.equal(b.read().phones.find(device => device.key === phone)?.pairing, pairing);
  assert.equal(a.read().phones.find(device => device.key === phone)?.mac, b.key);
  assert.equal(b.read().vouchBlocked, undefined);
  const transition = link.readDoc(b.read().leadership[3]);
  const stale = c.read();
  assert.equal(link.takeDoc(stale, b.read().leadership[3]), null);
  // The former main can't sign a certificate of its own: the relay's lease is the new main's.
  const forged = link.signDoc(a.seed, "leadership", 4, { ...transition.body, previous: b.key, setup: { ...transition.body.setup, epoch: 4, primary: a.key } });
  assert.equal(link.takeDoc(stale, forged), null);
  assert.equal(stale.setup.primary, b.key);
  const staleRevoke = link.signDoc(a.seed, "phones", 40, { phones: [], removed: [{ key: phone, pairing }] });
  assert.equal(link.takeDoc(stale, staleRevoke), null);
  assert.deepEqual(await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }), {
    status: 400, value: { message: "Only a device paired with your setup can remove devices." },
  });

  await c.stop("SIGTERM");
  assert.deepEqual(await submit(b, a, 3), { status: 202, value: { primary: a.key, epoch: 4 } });
  await Promise.all([see(a, 4, a), see(b, 4, a)]);
  assert.deepEqual(await submit(a, b, 4), { status: 202, value: { primary: b.key, epoch: 5 } });
  await Promise.all([see(a, 5, b), see(b, 5, b)]);
  assert.equal(c.read().setup.epoch, 3);
  c.start();
  await see(c, 5, b);
  assert.ok(c.read().leadership[5]);
  const setupID = b.read().setup.id;
  assert.equal((await b.route("DELETE", `kuvo/computers/${a.key}`)).status, 200);
  await until("removal received", () => a.read().setup?.id !== setupID, t.signal);
  assert.equal(a.read().macPeers, undefined);
  await see(b, 5, b);
});

test("failure bench: an SSH-enrolled member advertises its accepted epoch and catches up", { timeout: 35_000 }, async t => {
  const { a, b, c, see } = await network(t);
  assert.equal((await submit(a, b, 2)).status, 202);
  await Promise.all([see(a, 3, b), see(b, 3, b), see(c, 3, b)]);
  assert.equal((await submit(b, a, 3)).status, 202);
  await Promise.all([see(a, 4, a), see(b, 4, a), see(c, 4, a)]);
  await c.stop("SIGTERM");

  const primary = a.read();
  const gift = link.ownDoc(primary, "peers", link.primaryPeersBody(primary));
  const joined = { ...c.identity, mac: a.key, macs: [a.key], macSigns: { [a.key]: a.sign }, phones: [] };
  // SSH gives a fresh member the current signed roster, without the preceding epoch's authority.
  link.docsWork({ link: joined, library: { personalities: [], removed: [] } }, a.entry, { give: [gift.envelope] }, { stay: a.key });
  assert.equal(joined.setup.epoch, 4);
  assert.equal(joined.leadership, undefined);
  assert.ok(link.digestOf(joined).some(entry => entry[1] === "leadership" && entry[2] === 4));
  fs.writeFileSync(path.join(c.run, "link.json"), JSON.stringify(joined));

  assert.equal((await submit(a, b, 4)).status, 202);
  await Promise.all([see(a, 5, b), see(b, 5, b)]);
  c.start();
  await see(c, 5, b);
  assert.ok(c.read().leadership[5]);
});

test("failure bench: a successor that goes unreachable during handover follows the committed epoch when back", { timeout: 40_000 }, async t => {
  const { a, b, c, relay, see } = await network(t);
  let reconnect;
  const unhook = relay.intercept(b.address, a.address, packet => {
    const message = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), a.read())?.payload;
    if (message?.workQueue?.acted?.value?.sign !== b.sign) return false;
    relay.replay(packet);
    reconnect = relay.disconnect(b.address);
    unhook();
    return true;
  });
  assert.equal((await submit(a, b, 2)).status, 202);
  assert.equal(typeof reconnect, "function");
  // (The relay moved its lease: it takes over, and once back the others follow it.)
  reconnect();
  await Promise.all([see(a, 3, b), see(b, 3, b), see(c, 3, b)]);
});

test("failure bench: a primary killed after committing a lease transfer resumes as a member", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see } = await network(t);
  // The lease moves at the relay. Only the successor can publish the new epoch.
  const resumeWitness = relay.pause(b.address, c.address);
  let resumeFormerMain;
  const unhook = relay.intercept(b.address, a.address, packet => {
    const message = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), a.read())?.payload;
    if (message?.workQueue?.acted?.value?.sign !== b.sign) return false;
    resumeFormerMain = relay.pause(b.address, a.address);
    unhook();
    return false;
  });
  assert.equal((await submit(a, b, 2)).status, 202);
  await a.stop();
  await see(b, 3, b);
  assert.equal(c.read().setup.epoch, 2);
  assert.equal(typeof resumeFormerMain, "function");
  resumeFormerMain();
  resumeWitness();
  a.start();
  await Promise.all([see(a, 3, b), see(b, 3, b), see(c, 3, b)]);
});

test("failure bench: losing the successor before readiness keeps the old primary", { timeout: 40_000 }, async t => {
  const { a, b, c, relay, see } = await network(t);
  let reconnect;
  const unhook = relay.intercept(a.address, b.address, packet => {
    const message = link.openMail(link.readFrame(packet.payload)?.body ?? Buffer.alloc(0), b.read())?.payload;
    if (message?.workQueue?.act?.area !== "setup") return false;
    reconnect = relay.disconnect(b.address);
    unhook();
    return true;
  });
  assert.equal((await submit(a, b, 2)).status, 503);
  assert.equal(typeof reconnect, "function");
  await Promise.all([see(a, 2, a), see(b, 2, a), see(c, 2, a)]);
  assert.deepEqual(Object.keys(a.read().leadership), ["2"]);
  reconnect();
});

// Three computers whose main holds the relay's lease, the second one always on.
