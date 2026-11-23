import "./test-safety.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { testBun } from "./bun-for-tests.js";
import { assertReceipt, deviceAt, link, network, submit, until } from "./setup-handover-fixture.js";
import {
  operationID, removalMessage, removalOf, seedLocalWork, separateExportRoot, waitForWipe,
  wipeRoute,
} from "./setup-removal-fixture.js";

test("failure bench: online unlink from a member keeps the target’s sessions and bots standalone", { timeout: 25_000 }, async t => {
  const { a, b, c } = await network(t);
  const data = seedLocalWork(b);
  const original = b.read().setup.id;
  const operation = crypto.randomBytes(16).toString("base64url");
  const removed = await c.route("POST", `kuvo/computers/${b.key}/unlink`, { operation });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
  assert.deepEqual(removed.value.removal, { operation, state: "unlinked", accessRemoved: true });
  await until("new standalone setup", () => b.read().setup?.id !== original && b.read().setup?.role === "primary", t.signal);
  await until("unlink reached all members", () => removalOf(a, b)?.delivered && removalOf(c, b)?.delivered, t.signal);
  assert.deepEqual(b.read().phones, []);
  assert.deepEqual(link.peersOf(b.read()), []);
  assert.deepEqual(data.map(file => fs.readFileSync(file, "utf8")), ["local work", "local work"]);
  assert.deepEqual(link.peersOf(c.read()).map(peer => peer.key), [a.key]);
  const retry = await a.route("POST", `kuvo/computers/${b.key}/unlink`, { operation });
  assert.deepEqual(retry.value.proof, removed.value.proof);
});

test("failure bench: an offline member is unlinked at commit and wipes after returning", { timeout: 85_000 }, async t => {
  const { a, b, c, relay } = await network(t, false, undefined, false, undefined, true, false, undefined, { acceptedRoster: true });
  const data = seedLocalWork(b);
  const dest = path.join(b.env.HOME, "external-backup");
  fs.mkdirSync(dest);
  fs.mkdirSync(path.join(b.home, "backup"));
  const source = "b_" + crypto.randomBytes(16).toString("base64url");
  fs.writeFileSync(path.join(b.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source, key: crypto.randomBytes(32).toString("base64url"),
    dest: { kind: "disk", path: dest }, disk: fs.statSync(dest).dev, projects: [],
  }));
  await b.stop();
  await until("target offline", () => !relay.has(b.address), t.signal);
  const operation = crypto.randomBytes(16).toString("base64url");
  const removed = await a.route("POST", `kuvo/computers/${b.key}/unlink-and-wipe`, { operation });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
  assert.deepEqual(removed.value.removal, { operation, state: "wipe-pending", accessRemoved: true });
  assert.equal(link.peersOf(a.read()).some(peer => peer.key === b.key), false);
  await until("pending wipe on another client’s box", () => removalOf(c, b)?.operation === operation, t.signal);
  assert.deepEqual((await c.route("GET", "kuvo/computers")).value.wipePending, [{ key: b.key, name: b.name, operation }]);
  assert.ok(data.every(file => fs.existsSync(file)));
  b.start();
  await until("target returned to the relay", () => relay.has(b.address) || !fs.existsSync(b.home), t.signal);
  await waitForWipe(b, t.signal, 25_000);
  await until("completion replicated", () => removalOf(a, b)?.delivered && removalOf(c, b)?.delivered, AbortSignal.any([t.signal, AbortSignal.timeout(8500)]));
  assert.equal((await c.route("GET", "kuvo/computers")).value.wipePending, undefined);
  assert.ok(fs.readdirSync(path.join(dest, "kuvo-backups", source)).some(file => file.includes("-wipe-")), "verified backup survived wipe");
});

for (const scenario of ["headless member", "app-owned Mac with a shutdown write", "app-owned Mac with a lost export"]) test(
  `failure bench: revoked Relay-only ${scenario} accepts a verified removal export after reconnect`,
  { timeout: 160_000 }, async t => {
    const appOwner = scenario.startsWith("app-owned Mac");
    const lostExport = scenario.endsWith("lost export");
    const { a, b: receiver, c: source, relay } = await network(t, false, undefined, false, undefined, true, false, undefined, { appOwner, acceptedRoster: true });
    const backup = async (action, body) => {
      const route = "kuvo/box/backups/" + action;
      if (!appOwner) {
        const reply = await source.command(["box", "backups", action], JSON.stringify(body ?? {}));
        assert.equal(reply.status, 0, reply.err);
        assert.equal(reply.value.recoveryKey, undefined);
        return { status: reply.value.status, value: reply.value };
      }
      const { port, password } = JSON.parse(fs.readFileSync(path.join(source.run, "serve.json"), "utf8"));
      const response = await fetch(`http://127.0.0.1:${port}/${route}`, {
        method: "POST", headers: { authorization: "Basic " + Buffer.from(`opencode:${password}`).toString("base64"), "x-kuvo-from": "app" },
        body: JSON.stringify(body), signal: t.signal,
      });
      return { status: response.status, value: await response.json() };
    };
    const data = seedLocalWork(source, { botWork: false });
    const sibling = source.appFolder && path.join(source.appFolder, "Attachments", "ses_retained", "message", "design.new");
    if (sibling) {
      fs.mkdirSync(path.dirname(sibling), { recursive: true });
      fs.writeFileSync(sibling, Buffer.of(0, 255, 18));
      data.push(sibling);
    }
    const outside = path.join(source.env.HOME, "unrelated");
    fs.writeFileSync(outside, "keep");
    const destination = { kind: "computer", computer: receiver.key };
    const configured = await backup("destination", { destination, confirm: true });
    assert.equal(configured.status, 200, JSON.stringify(configured.value));
    const daily = await backup("now", {});
    assert.equal(daily.status, 200, JSON.stringify(daily.value));
    assert.match(daily.value.point.id, /-daily-/);
    const configFile = path.join(source.home, "backup", "configuration.json");
    assert.equal(fs.statSync(configFile).mode & 0o777, 0o600);
    const ownedConfig = JSON.parse(fs.readFileSync(configFile, "utf8"));
    assert.equal(typeof ownedConfig.key, "string");
    const phone = link.newKey();
    const invite = await a.route("POST", "kuvo/phones/pair");
    const first = await deviceAt(relay, a, phone, { pair: new URL(invite.value.link).searchParams.get("token"), name: "Removal fixture" });
    assert.ok(first.greeting);
    t.after(first.close);
    const open = box => until("paired phone reaches " + box.name, async () => {
      const client = await deviceAt(relay, box, phone);
      return client.greeting && client;
    }, t.signal);
    let initiator = await open(receiver);
    t.after(() => initiator.close());
    const sourceClient = await open(source);
    const remoteHealth = await sourceClient.ask("GET", "kuvo/box/backups/status");
    assert.deepEqual(remoteHealth.value, { enabled: true, destinationKind: "computer", destinationAvailable: true,
      lastVerifiedAt: daily.value.point.at, attention: false });
    assert.equal((await sourceClient.ask("POST", "kuvo/box/backups/destination", { destination, confirm: true })).status, 403);
    assert.equal((await sourceClient.ask("GET", "kuvo/box/backups")).status, 403);
    sourceClient.close();
    const setup = a.read().setup.id;
    const deposit = path.join(receiver.env.HOME, ".kuvo-backups", "relay", setup, source.key, configured.value.source, daily.value.point.id + ".kuvo");
    const original = fs.readFileSync(deposit);
    fs.writeFileSync(data[0], "saved after the daily backup");
    await source.stop();
    await until("source offline", () => !relay.has(source.address), t.signal);
    let operation = operationID();
    const started = await initiator.ask("POST", wipeRoute(source), { operation });
    assert.equal(started.status, 200, JSON.stringify(started.value));
    assert.deepEqual(started.value.removal, { operation, state: "wipe-pending", accessRemoved: true });
    await until("receiver replicated revocation", () => removalOf(receiver, source)?.operation === operation, t.signal);
    assert.equal(link.peersOf(receiver.read()).some(peer => peer.key === source.key), false);
    const denied = await receiver.evaluate(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
      import { relayBackupAct } from ${JSON.stringify(new URL("../src/link/backup/relay-store.ts", import.meta.url).href)};
      const sender = { key: ${JSON.stringify(source.key)} };
      const item = ${JSON.stringify(configured.value.source)};
      const body = { setup: ${JSON.stringify(setup)}, id: ${JSON.stringify(daily.value.point.id)} };
      console.log(JSON.stringify(await Promise.all([
        relayBackupAct(sender, { method: "GET", item, body: { ...body, action: "list" } }),
        relayBackupAct(sender, { method: "GET", item, body: { ...body, action: "read", offset: 0 } }),
        relayBackupAct(sender, { method: "POST", item, body: { ...body, action: "begin", size: 5, sha256: "a".repeat(64) } }),
        relayBackupAct(sender, { method: "DELETE", item, body }),
      ])));
    `]);
    assert.equal(denied.status, 0, denied.err);
    assert.deepEqual(denied.value.map(reply => reply.status), [409, 409, 409, 409]);
    const diskParent = separateExportRoot(t, source);
    const target = { kind: "disk", path: path.join(diskParent, "new-export") };
    if (appOwner) source.onShutdown = `
      import fs from "node:fs";
      process.once("SIGTERM", () => {
        if (fs.existsSync(${JSON.stringify(target.path)})) {
          ${lostExport ? `fs.renameSync(${JSON.stringify(target.path)}, ${JSON.stringify(target.path + "-away")});` :
            `fs.writeFileSync(${JSON.stringify(data[0])}, "saved while stopping");`}
        }
        fs.writeFileSync(${JSON.stringify(path.join(source.env.TMPDIR, "keeper-stopped"))}, "");
        process.exit(0);
      });
    `;
    let plainGrant;
    const observeGrant = relay.intercept(a.address, source.address, packet => {
      const instruction = link.readDoc(removalMessage(packet, source, a)?.instruction);
      if (instruction?.body.wipe && !instruction.body.plan) plainGrant = packet;
      return false;
    });
    t.after(observeGrant);
    source.start();
    await until("review replicated after reconnect", () => removalOf(receiver, source)?.needsExport,
      AbortSignal.any([t.signal, AbortSignal.timeout(35_000)]));
    const pending = await a.route("POST", wipeRoute(source), { operation: started.value.operation });
    assert.equal(pending.value.removal.state, "review");
    assert.equal(pending.value.removal.nextAction, "choose-export");
    assert.match(pending.value.message, /Relay.*separate disk or SSH/);
    assert.equal((await receiver.route("GET", "kuvo/computers")).value.wipePending[0].nextAction, "choose-export");
    assert.ok(data.every(file => fs.existsSync(file)));
    assert.equal((await deviceAt(relay, source, phone)).refused, true);
    let supersedes;
    if (!appOwner) {
      const disabled = await backup("destination", { enabled: false });
      assert.equal(disabled.value.enabled, false);
      assert.deepEqual(fs.readFileSync(deposit), original);
      const originalGrant = plainGrant;
      assert.ok(originalGrant);
      const after = relay.delivered.length;
      relay.replay(originalGrant);
      await until("fresh plain wipe instruction still held", () => plainGrant !== originalGrant, t.signal);
      await until("source repeated its refusal", () => relay.delivered.slice(after).some(packet =>
        packet.to.equals(a.address) && removalMessage(packet, a, source)?.review === "choose-export"), t.signal);
      assert.ok(data.every(file => fs.existsSync(file)));
      assert.equal(source.read().localWipe, undefined);
      const sameVolume = fs.mkdtempSync(path.join(source.env.HOME, "same-disk-"));
      const rejected = await a.route("POST", wipeRoute(source), { operation, prepare: {
        export: { destination: { kind: "disk", path: sameVolume }, recoveryKey: crypto.randomBytes(32).toString("base64url") },
      } }, true);
      assert.equal(rejected.status, 200);
      const refusal = await until("same-disk export refused", () => removalOf(a, source)?.error, t.signal);
      assert.match(refusal, /separate disk/);
      assert.deepEqual(fs.readdirSync(sameVolume), []);
      supersedes = operation;
      operation = operationID();
    }
    const recoveryKey = crypto.randomBytes(32).toString("base64url");
    const remoteRevision = await initiator.ask("POST", wipeRoute(source), { operation, prepare: { export: { destination: target, recoveryKey } } });
    assert.equal(remoteRevision.status, 403);
    const select = () => a.route("POST", wipeRoute(source), { operation, supersedes, prepare: { export: { destination: target, recoveryKey } } }, true);
    assert.equal((await select()).value.removal.state, "exporting");
    await until("unavailable destination reported", () => removalOf(a, source)?.error, t.signal);
    assert.match(removalOf(a, source).error, /selected export disk is unavailable/);
    assert.equal(removalOf(a, source).error.includes(target.path), false);
    assert.ok(data.every(file => fs.existsSync(file)));
    await a.stop();
    await until("primary connection gone", () => !relay.has(a.address), t.signal);
    a.start();
    await until("primary restarted", () => relay.has(a.address), t.signal);
    assert.equal(removalOf(a, source)?.operation, operation);
    fs.mkdirSync(target.path);
    let done;
    const releaseDone = relay.intercept(null, a.address, packet => {
      if (removalMessage(packet, a, source)?.done !== operation) return false;
      done = packet;
      return true;
    });
    t.after(releaseDone);
    const retry = await a.route("POST", wipeRoute(source), { operation, retryPreparation: true }, true);
    assert.equal(retry.status, 200, JSON.stringify(retry.value));
    if (appOwner) {
      await until("shutdown invalidated readiness", () =>
        (lostExport ? /selected export disk is unavailable/ : /Source work changed/).test(removalOf(a, source)?.error ?? ""), t.signal);
      assert.equal(removalOf(a, source).delivered, undefined);
      assert.equal(fs.readFileSync(data[0], "utf8"), lostExport ? "saved after the daily backup" : "saved while stopping");
      if (lostExport) fs.renameSync(target.path + "-away", target.path);
      const again = await a.route("POST", wipeRoute(source), { operation, retryPreparation: true }, true);
      assert.equal(again.status, 200, JSON.stringify(again.value));
    }
    await waitForWipe(source, t.signal, 30_000);
    await until("completion sent after erase", () => done, t.signal);
    assert.equal(removalOf(a, source).delivered, undefined);
    if (lostExport) {
      await a.stop();
      await until("primary offline after detach", () => !relay.has(a.address), t.signal);
      a.start();
      await until("primary resumed the removal", () => relay.has(a.address), t.signal);
      assert.equal(removalOf(a, source)?.operation, operation);
      assert.equal(removalOf(a, source)?.delivered, undefined);
    }
    releaseDone();
    relay.replay(done);
    await until("completion replicated", () => removalOf(receiver, source)?.delivered, t.signal);
    assert.ok(data.every(file => !fs.existsSync(file)));
    if (source.appFolder) {
      assert.equal(fs.existsSync(source.appFolder), false);
      if (process.platform === "darwin") assert.deepEqual(fs.readFileSync(path.join(source.env.HOME, "keychain-cleared"), "utf8").trim().split("\n"),
        ["key", "secret", "sign"].map(account => `delete-generic-password -s run.kuvo.code.fixture.phone-link -a ${account}`));
    }
    assert.equal(fs.readFileSync(outside, "utf8"), "keep");
    assert.deepEqual(fs.readFileSync(deposit), original);
    assert.equal(link.peersOf(receiver.read()).some(peer => peer.key === source.key), false);
    const ended = await a.route("POST", wipeRoute(source), { operation });
    assert.equal(ended.value.removal.state, "wiped");
    assert.equal(ended.value.offer, undefined);
    assert.equal((await receiver.route("GET", "kuvo/computers")).value.wipePending, undefined);
    const point = ended.value.export;
    const archive = path.join(target.path, "kuvo-backups", point.source, point.id + ".kuvo");
    const bytes = await a.evaluate(testBun(), ["--config=/dev/null", "--no-env-file", "-e", `
      import fs from "node:fs";
      import { archiveContext, openArchive } from ${JSON.stringify(new URL("../src/link/backup/archive.ts", import.meta.url).href)};
      const point = ${JSON.stringify(point)};
      const entries = openArchive(${JSON.stringify(archive)}, ${JSON.stringify(recoveryKey)},
        archiveContext(point.source, point.id), fs.mkdtempSync(process.env.TMPDIR + "/inspect-"));
      console.log(JSON.stringify(Object.fromEntries(entries.filter(({ name }) => name.endsWith("/ses_retained/messages.json") ||
        name.endsWith("/design.new")).map(({ name, file }) => [name.split("/").at(-1), fs.readFileSync(file).toString("base64")] ))));
    `]);
    assert.equal(bytes.status, 0, bytes.err);
    assert.equal(bytes.value["messages.json"], Buffer.from(appOwner && !lostExport ? "saved while stopping" : "saved after the daily backup").toString("base64"));
    if (sibling) assert.equal(bytes.value["design.new"], Buffer.of(0, 255, 18).toString("base64"));
  },
);

test("failure bench: unsupported erase layout stays pending and can cancel without losing work", { timeout: 35_000 }, async t => {
  const { a, b: source } = await network(t);
  const kept = seedLocalWork(source);
  const operation = operationID();
  const started = await a.route("POST", wipeRoute(source), { operation });
  assert.equal(started.value.removal.state, "wipe-pending");
  await until("unsupported layout held for review", () => removalOf(a, source)?.error, t.signal);
  const review = await a.route("POST", wipeRoute(source), { operation: started.value.operation });
  assert.equal(review.value.removal.state, "review");
  assert.equal(review.value.removal.nextAction, "cancel-wipe");
  const cancelled = await a.route("POST", wipeRoute(source), { operation, resolve: "cancel" }, true);
  assert.equal(cancelled.value.removal.state, "cancelled");
  await until("standalone setup retained the work", () => source.read().setup.role === "primary", t.signal);
  assert.ok(kept.every(file => fs.existsSync(file)));
  assert.equal(link.peersOf(a.read()).some(peer => peer.key === source.key), false);
});

test("failure bench: a missing computer stays wipe-pending across primary restart", { timeout: 25_000 }, async t => {
  const { a, b, c, relay } = await network(t);
  await b.stop();
  const result = await a.route("POST", `kuvo/computers/${b.key}/unlink-and-wipe`);
  assert.equal(result.status, 200, JSON.stringify(result.value));
  await a.stop("SIGTERM");
  a.start();
  await until("primary reconnected", () => relay.has(a.address), t.signal);
  await until("pending removal replicated", () => removalOf(c, b)?.operation === result.value.operation, t.signal);
  assert.deepEqual((await a.route("GET", "kuvo/computers")).value.wipePending, [{ key: b.key, name: b.name, operation: result.value.operation }]);
  assert.equal(link.peersOf(a.read()).some(peer => peer.key === b.key), false);
});

test("failure bench: a lost disk closes a pending wipe and preserves the key revocations", { timeout: 25_000 }, async t => {
  const { a, b, relay } = await network(t, false, undefined, false, undefined, true);
  await b.stop();
  await until("target offline", () => !relay.has(b.address), t.signal);
  const operation = operationID();
  const started = await a.route("POST", wipeRoute(b), { operation });
  assert.equal(started.status, 200);
  const lost = await a.route("POST", wipeRoute(b), { operation, resolve: "lost" }, true);
  assert.equal(lost.status, 200, JSON.stringify(lost.value));
  assert.deepEqual(lost.value.removal, { operation, state: "lost", accessRemoved: true });
  assert.deepEqual(await a.route("POST", wipeRoute(b), { operation: started.value.operation }), lost);
  const membership = a.read().shared.membershipAndAccess.value;
  assert.ok(membership.left.includes(b.key));
  assert.equal(membership.boxes.some(box => box.key === b.key), false);
  assert.equal(membership.removals[b.key].peer.sign, b.sign);
  assert.equal((await a.route("GET", "kuvo/computers")).value.wipePending, undefined);
});

test("failure bench: unlinking the only computer wipes its local setup", { timeout: 25_000 }, async t => {
  const { a, b, c, phase } = await network(t, false, undefined, false, undefined, true, false, undefined,
    { phaseDiagnostics: true });
  const data = seedLocalWork(a);
  // What each computer's Remove offers is the box's: unlink or wipe for a member, and for the main
  // of more, who takes its role first; the last computer can only be wiped.
  const offered = async key => (await a.route("GET", "kuvo/computers")).value.computers.find(each => each.key === key)?.actions.find(action => action.id === "remove")?.removal;
  assert.deepEqual(await offered(b.key), { choices: ["unlink", "wipe"] });
  const main = await offered(a.key);
  assert.deepEqual(main?.choices, ["unlink", "wipe"]);
  assert.match(main?.handOver ?? "", / becomes the main computer first\. /);
  for (const peer of [b, c]) {
    const result = await a.route("POST", `kuvo/computers/${peer.key}/unlink`);
    assert.equal(result.status, 200, JSON.stringify(result.value));
    assert.equal(result.value.offer, undefined);
  }
  phase("both member unlink decisions saved");
  await until("both unlinks delivered", () => removalOf(a, b)?.delivered && removalOf(a, c)?.delivered, AbortSignal.any([t.signal, AbortSignal.timeout(8500)]));
  phase("both member unlink deliveries acknowledged");
  assert.deepEqual(await offered(a.key), { choices: ["wipe"], last: true });
  const setup = a.read().setup.id;
  const last = await a.route("POST", `kuvo/computers/${a.key}/unlink`);
  assert.equal(last.status, 200, JSON.stringify(last.value));
  assert.deepEqual(last.value.removal, { operation: last.value.operation, state: "wipe-pending", accessRemoved: true });
  assert.equal(last.value.offer, "reinstall-local");
  phase("last-computer wipe accepted with local reinstall offer");
  await waitForWipe(a, t.signal);
  phase("old installation detached");
  assert.ok(data.every(file => !fs.existsSync(file)));
  const marker = path.join(path.dirname(a.home), ".kuvo-code-removal-receipts", "last-setup-wipe");
  const receipt = await until("the detached installation is erased", () => {
    const value = JSON.parse(fs.readFileSync(marker, "utf8"));
    return !value.phase && value;
  }, t.signal);
  phase("detached wipe journal completed");
  assert.deepEqual(receipt, { setup, operation: last.value.operation, formatVersion: 1 });
  assert.equal(fs.statSync(marker).mode & 0o777, 0o600);
});

test("failure bench: a sole primary can supersede and cancel a prepared removal", { timeout: 35_000 }, async t => {
  const { a, b } = await network(t, false, undefined, false, undefined, true, true);
  const data = seedLocalWork(a);
  assert.equal((await a.route("POST", `kuvo/computers/${b.key}/unlink`)).status, 200);
  await until("unlink acknowledged", () => removalOf(a, b)?.delivered, t.signal);
  const prepare = name => ({ export: { destination: { kind: "disk", path: path.join(a.env.TMPDIR, name) },
    recoveryKey: crypto.randomBytes(32).toString("base64url") } });
  const operation = operationID();
  const first = await a.route("POST", wipeRoute(a), { operation, prepare: prepare("missing-original") }, true);
  assert.equal(first.status, 200, JSON.stringify(first.value));
  await until("local preparation failed", () => removalOf(a, a)?.error, AbortSignal.timeout(8_000));
  const revised = operationID();
  const second = await a.route("POST", wipeRoute(a), { operation: revised, supersedes: operation, prepare: prepare("missing-replacement") }, true);
  assert.equal(second.status, 200, JSON.stringify(second.value));
  assert.equal((await a.route("POST", wipeRoute(a), { operation: first.value.operation })).value.removal.state, "cancelled");
  await until("revised preparation failed", () => removalOf(a, a)?.operation === revised && removalOf(a, a)?.error, AbortSignal.timeout(8_000));
  const cancelled = await a.route("POST", wipeRoute(a), { operation: revised, resolve: "cancel" }, true);
  assert.equal(cancelled.status, 200, JSON.stringify(cancelled.value));
  assert.equal(cancelled.value.removal.state, "cancelled");
  assert.equal(a.read().setup.role, "primary");
  assert.equal(a.read().localWipe, undefined);
  assert.ok(data.every(file => fs.existsSync(file)));
  const fresh = operationID();
  const started = await a.route("POST", wipeRoute(a), { operation: fresh, prepare: prepare("missing-fresh") }, true);
  assert.equal(started.status, 200, JSON.stringify(started.value));
  assert.equal((await a.route("POST", wipeRoute(a), { operation: revised })).value.removal.state, "cancelled");
  assert.equal(removalOf(a, a).operation, fresh);
});

test("failure bench: a pending wipe follows the setup through handover", { timeout: 30_000 }, async t => {
  const { a, b, c, see, relay } = await network(t, false, undefined, false, undefined, true);
  seedLocalWork(b);
  await b.stop();
  await until("target offline", () => !relay.has(b.address), t.signal);
  const result = await a.route("POST", `kuvo/computers/${b.key}/unlink-and-wipe`);
  assert.equal(result.status, 200, JSON.stringify(result.value));
  await until("candidate learns the pending wipe before taking over", () => removalOf(c, b)?.operation === result.value.operation, t.signal);
  assert.equal((await submit(a, c, 2)).status, 202);
  await Promise.all([see(a, 3, c), see(c, 3, c)]);
  await until("new primary installs the pending wipe", () => removalOf(c, b)?.operation === result.value.operation, t.signal);
  assert.equal(removalOf(c, b).delivered, undefined);
  b.start();
  await waitForWipe(b, t.signal);
  await until("new primary records completion", () => removalOf(c, b)?.delivered, t.signal);
});

test("failure bench: a failed pre-wipe backup leaves the removed computer’s data in place", { timeout: 25_000 }, async t => {
  const { a, b } = await network(t, false, undefined, false, undefined, true);
  const data = seedLocalWork(b);
  fs.mkdirSync(path.join(b.home, "backup"));
  fs.writeFileSync(path.join(b.home, "backup", "configuration.json"), JSON.stringify({
    version: 1, enabled: true, source: "b_" + crypto.randomBytes(16).toString("base64url"),
    key: crypto.randomBytes(32).toString("base64url"), disk: 1, projects: [],
    dest: { kind: "disk", path: path.join(b.env.HOME, "missing-disk") },
  }));
  const result = await a.route("POST", `kuvo/computers/${b.key}/unlink-and-wipe`);
  assert.equal(result.status, 200, JSON.stringify(result.value));
  const log = path.join(b.run, "uninstall.log");
  await until("backup attempt failed", () => fs.existsSync(log) && fs.readFileSync(log, "utf8").includes("missing-disk"), t.signal);
  assert.deepEqual(data.map(file => fs.readFileSync(file, "utf8")), ["local work", "local work"]);
  assert.equal(b.read().localWipe.operation, result.value.operation);
  assert.equal(b.read().localWipe.prepared, undefined);
  assert.equal(removalOf(a, b).delivered, undefined);
});
