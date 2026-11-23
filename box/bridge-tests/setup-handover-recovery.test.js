import "./test-safety.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { NO_BUN, testBun } from "./bun-for-tests.js";
import { recoveryNeighborBuild } from "./failure-bench-legacy.js";
import { benchRelease, put } from "./recovery-bench.js";
import {
  assertReceipt, editAddress, editRead, editSubmit, itemID, link, network, peerOwns, rename,
  script, setupName, until,
} from "./setup-handover-fixture.js";
import { operationID, seedLocalWork, uploadQueueFiles } from "./setup-removal-fixture.js";

for (const populated of [false, true]) test(
  `failure bench: retired pc5 Insights origin survives restore into a ${populated ? "populated" : "fresh"} index`,
  { timeout: 105_000 }, async t => {
    const bun = testBun();
    assert.ok(bun, NO_BUN);
    const { a, b, c, boxes: [, , , replacement], relay } = await network(t, false, undefined, false, undefined,
      false, false, ["pc2", "pc4", "Original pc5", "Replacement"], { acceptedRoster: true });
    const backup = async (box, action, body) => {
      const answer = await box.route("POST", "kuvo/box/backups/" + action, body, true);
      assert.equal(answer.status, 200, JSON.stringify(answer.value));
      return answer.value;
    };
    const wait = (what, fn, timeout = 15_000) => until(what, fn, AbortSignal.any([t.signal, AbortSignal.timeout(timeout)]));
    const index = box => path.join(box.home, "agents", "usage", "insights.sqlite");
    const query = (box, sql, ...params) => {
      const db = new DatabaseSync(index(box), { readOnly: true });
      try { return db.prepare(sql).get(...params); } finally { db.close(); }
    };
    const cursor = box => JSON.parse(query(box, "SELECT body FROM state WHERE key = ?", `cursor/${b.key}`)?.body ?? "0");
    const record = {
      key: crypto.createHash("sha256").update("pc5-provider-call").digest("hex"),
      computer: c.key, session: "ses_history", call: "provider-call", provider: "claude", attributed: true,
      account: "claude#aabbccddeeff0011", accountObserved: true,
      at: Date.now() - 1_000, tokens: { input: 100, output: 50, cacheRead: 0, cacheWrite5m: 0, cacheWrite1h: 0 },
    };
    const writeIndex = async (box, records, state = {}) => {
      fs.mkdirSync(path.dirname(index(box)), { recursive: true });
      const store = new URL("../src/usage/insights-store.ts", import.meta.url).href;
      const pricing = new URL("../src/usage/pricing.ts", import.meta.url).href;
      const result = await box.evaluate(bun, ["--config=/dev/null", "--no-env-file", "-e", `
        import { InsightsStore } from ${JSON.stringify(store)};
        import { PricingStore } from ${JSON.stringify(pricing)};
        const db = new InsightsStore(${JSON.stringify(index(box))});
        db.take(${JSON.stringify(records)}, new PricingStore(undefined, null).current(), Date.now());
        for (const [key, value] of Object.entries(${JSON.stringify(state)})) db.write(key, value);
        db.close();
      `], "", false);
      assert.equal(result.status, 0, result.err);
    };
    const services = async box => {
      const child = box.launch(bun, ["--config=/dev/null", "--no-env-file", script, "box-services"]);
      const file = path.join(box.run, "box-services.json");
      const r = await wait(`${box.name} services`, () => {
        assert.equal(child.exitCode, null);
        const saved = fs.existsSync(file) && JSON.parse(fs.readFileSync(file));
        return saved?.pid === child.pid && saved;
      });
      return {
        async get(route) {
          const result = await fetch(`http://127.0.0.1:${r.port}/api/kuvo/${route}`, {
            headers: { authorization: "Basic " + Buffer.from(`opencode:${r.password}`).toString("base64") },
            signal: AbortSignal.any([t.signal, AbortSignal.timeout(4_000)]),
          });
          assert.equal(result.status, 200, await result.clone().text());
          return (await result.json()).data;
        },
        async stop() {
          const exited = once(child, "exit");
          child.kill();
          await exited;
        },
      };
    };
    const overview = (box, filter = "") => box.get(`insights?period=7&zone=UTC${filter}`);
    const checkHistory = async (box, afterRestore = false) => {
      const data = await overview(box);
      assert.equal(data.totals.calls, 1);
      assert.equal(data.totals.tokens, 150);
      assert.deepEqual(data.computers.find(row => row.key === c.key), { key: c.key, name: c.name });
      assert.equal((await overview(box, `&computer=${c.key}`)).totals.calls, 1);
      assert.equal((await overview(box, `&computer=${replacement.key}`)).totals.calls, 0);
      assert.equal((await overview(box, `&account=${encodeURIComponent(record.account)}`)).totals.calls, 1);
      const detail = await box.get(`insights/sessions/${record.session}?period=7&zone=UTC`);
      assert.deepEqual(detail.calls.map(({ key, computer, call, account }) => ({ key, computer, call, account })),
        [{ key: record.key, computer: c.key, call: record.call, account: record.account }]);
      if (afterRestore) {
        assert.deepEqual(detail.destination, { status: "unavailable" }, "a restore match alone does not locate the archived conversation");
        assert.equal(detail.calls[0].attributed, true);
      }
      const sessions = await box.get("insights/sessions?period=7&zone=UTC");
      assert.equal(sessions.sessions.find(row => row.session === record.session).computer, c.key);
    };
    const opened = (packet, to) => {
      const frame = link.readFrame(packet.payload);
      return frame && link.openMail(frame.body, to.read())?.payload;
    };
    const sendPacket = (from, to, state, seat) => {
      const seatID = operationID();
      const wire = link.readRelay(link.mailFor(to.entry, { at: Date.now(), seatID, seat }, state));
      return { from: from.address, to: to.address, payload: wire.payload, seatID };
    };
    const deliver = async (packet, from, to) => {
      const start = relay.delivered.length;
      relay.replay(packet);
      await wait("authenticated services page applied", () => relay.delivered.slice(start).some(reply =>
        reply.from.equals(to.address) && reply.to.equals(from.address) &&
          opened(reply, from)?.delivered?.id === packet.seatID && opened(reply, from)?.delivered?.ok));
    };
    const unrelated = { ...record, key: "f".repeat(64), computer: link.publicOf(link.newKey()).toString("base64url"),
      at: record.at + 1, supersedes: record.key };
    const usageSource = new URL("../src/usage/usage.ts", import.meta.url).href;
    const alias = await c.evaluate(bun, ["--config=/dev/null", "--no-env-file", "-e", `
      import { ownComputer } from ${JSON.stringify(usageSource)};
      import { createHash } from "node:crypto";
      console.log(createHash("sha256").update(JSON.stringify(["kuvo insight computer", ownComputer()])).digest("hex").slice(0, 32));
    `], "", false);
    assert.equal(alias.status, 0, alias.err);
    assert.match(alias.out, /^[a-f0-9]{32}$/);
    const clonedAlias = { ...unrelated, key: "e".repeat(64), computer: alias.out };
    const legacy = { ...record, attributed: undefined };
    await writeIndex(c, [legacy]);
    const old = await services(c);
    const surviving = await services(b);
    await wait("pc4 services knows the current pc5 peer", async () =>
      (await overview(surviving)).computers.some(row => row.key === c.key), 35_000);
    // Seed the survivor through the authenticated wire, independently of whether the
    // two newly started readers have both subscribed before their first automatic pull.
    await deliver(sendPacket(c, b, c.read(), { kind: "insights", action: "page", after: 0, next: 1,
      epoch: "original", more: false, records: [legacy] }), c, b);
    await wait("pc4 synced the call from pc5", async () => (await overview(surviving)).totals.calls === 1);
    assert.equal(query(b, "SELECT sender, seq FROM event WHERE key = ?", record.key).seq, null);
    await old.stop();
    await writeIndex(c, [clonedAlias]);
    const cloneSession = "ses_clone_alias";
    const savedSessions = new URL("../src/agents/store.ts", import.meta.url).href;
    const saved = await c.evaluate(bun, ["--config=/dev/null", "--no-env-file", "-e", `
      import { Store } from ${JSON.stringify(savedSessions)};
      const store = new Store(${JSON.stringify(path.join(c.home, "agents", "sessions"))});
      store.addWhole({
        session: { id: ${JSON.stringify(cloneSession)}, projectID: "p", title: "Unbound clone call", cost: 0,
          time: { created: ${record.at}, updated: ${record.at} }, location: { directory: "/tmp" },
          tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } } },
        messages: [{ id: "cloned-call", type: "assistant", model: { providerID: "claude-code", id: "test" },
          agent: "build", content: [], time: { created: ${record.at}, completed: ${record.at}, origin: ${JSON.stringify(alias.out)} } }],
        agent: {},
      });
    `], "", false);
    assert.equal(saved.status, 0, saved.err);
    const skipped = { [`cursor/${b.key}`]: 90, [`peer-epoch/${b.key}`]: "legacy" };
    await writeIndex(a, [], skipped);
    if (populated) await writeIndex(replacement, [unrelated, { ...record, computer: unrelated.computer }], skipped);
    else assert.equal(fs.existsSync(index(replacement)), false);

    const root = a.env.TMPDIR;
    const destination = { kind: "disk", path: path.join(root, "archive") };
    fs.mkdirSync(destination.path);
    const source = "b_" + operationID();
    const recoveryKey = crypto.randomBytes(32).toString("base64url");
    put(path.join(c.home, "backup", "configuration.json"), JSON.stringify({
      version: 1, enabled: true, source, key: recoveryKey, dest: destination,
      disk: fs.statSync(destination.path).dev, projects: [],
    }));
    const { point } = await backup(c, "now");
    const oldCredential = c.read();
    await c.stop();
    fs.rmSync(c.home, { recursive: true });
    assert.equal(fs.existsSync(c.home), false);
    const removed = await a.route("DELETE", `kuvo/computers/${c.key}`);
    assert.ok([200, 202].includes(removed.status), JSON.stringify(removed.value));
    await wait("pc5 is no longer a current peer", () => link.seatPeers(replacement.read(), {}).boxes.every(peer => peer.key !== c.key));
    const oldPacket = sendPacket(c, replacement, oldCredential, { kind: "insights", action: "page", after: 0, next: 999,
      epoch: "retired", more: false, records: [{ ...record, key: "a".repeat(64) }] });
    assert.equal(opened(oldPacket, replacement), undefined, "the old transport credential is still revoked");
    relay.replay(oldPacket);
    await backup(a, "match", { source, previous: c.key, replacement: replacement.key });
    await wait("replacement and survivor have accepted the restore match and historical provenance", () => [replacement, b].every(box => {
      const state = box.read();
      return state.shared?.ownershipRecovery?.value?.[`replacement:${c.key}`]?.computer === replacement.key &&
        link.seatPeers(state, {}).insightOrigins?.some(origin => origin.key === c.key);
    }));
    const restored = await backup(replacement, "restore", { destination, source, id: point.id, recoveryKey, confirm: true });
    assert.equal(restored.unavailable.some(entry => entry.entry === "agents/usage/insights.sqlite"), false);
    assert.equal(query(replacement, "SELECT computer FROM event WHERE key = ?", record.key).computer, c.key);
    assert.equal(query(replacement, "SELECT computer FROM event WHERE key = ?", clonedAlias.key), undefined);
    if (populated) assert.equal(cursor(replacement), 90, "restore preserves the live index's wire cursor for scoped rewind");

    await checkHistory(surviving);
    const recovering = await services(a);
    // The link can be in its discovery backoff when a services reader starts after the restore.
    await wait("pc2 backfilled its skipped survivor page", async () => (await overview(recovering)).totals.calls === 1, 35_000);
    assert.notEqual(cursor(a), 90);
    await checkHistory(recovering);

    // The provider readings below are scripted. The idle readers in the fixture
    // would otherwise advertise their empty sign-in lists during reconnection.
    const idleLimits = payload => payload?.limits || payload?.seat?.kind === "limits";
    const unintercept = [
      relay.intercept(a.address, replacement.address, packet => idleLimits(opened(packet, replacement))),
      relay.intercept(b.address, replacement.address, packet => {
        const payload = opened(packet, replacement);
        return idleLimits(payload) || payload?.seat?.kind === "insights";
      }),
    ];
    let recovered = await services(replacement);
    await wait("replacement services knows retired pc5", async () => (await overview(recovered)).computers.some(row => row.key === c.key));
    await wait("scanner reconsidered the restored clone transcript", () =>
      query(replacement, "SELECT key FROM event WHERE session = ? AND computer = ?", cloneSession, alias.out));
    assert.equal((await overview(recovered, `&session=${cloneSession}`)).calls.length, 0);
    await checkHistory(recovered);
    const page = (after, next) => sendPacket(b, replacement, b.read(), { kind: "insights", action: "page", after, next,
      epoch: "replay", more: false, records: [record, unrelated] });
    const first = page(0, 1), second = page(1, 2);
    for (const packet of populated ? [second, first, second, first] : [first, second, second, first]) {
      await deliver(packet, b, replacement);
    }
    assert.equal(cursor(replacement), 2);
    assert.equal(query(replacement, "SELECT body FROM state WHERE key = ?", `cursor/${c.key}`), undefined);
    if (!populated) assert.equal(query(replacement, "SELECT computer FROM event WHERE key = ?", unrelated.key), undefined);
    await checkHistory(recovered, true);

    const checked = Date.now() - 1_000;
    const limit = (used, read) => ({ kind: "limits", at: Date.now(), accounts: [{ provider: "claude-code", service: "claude",
      identity: "aabbccddeeff0011", checked: read, windows: [{ id: "five-hour", used, read, minutes: 300 }] }] });
    await deliver(sendPacket(a, replacement, a.read(), limit(0.2, checked - 1000)), a, replacement);
    await deliver(sendPacket(b, replacement, b.read(), limit(0.6, checked)), b, replacement);
    relay.replay(sendPacket(c, replacement, oldCredential, limit(0.9, checked + 500)));
    const checkLimits = async () => {
      const row = (await recovered.get("limits")).rows.find(row => row.id === record.account);
      assert.ok(row, "current peers still supply the provider reading");
      assert.equal(row.usage.updatedAt, checked);
      assert.equal(row.usage.meters[0].share, 0.6);
      assert.deepEqual(row.usage.computers, [{ name: b.name, here: false }]);
      assert.ok(row.boxes.every(box => box.key !== c.key));
    };
    await checkLimits();
    await recovered.stop();
    await replacement.stop();
    replacement.start();
    recovered = await services(replacement);
    await wait("restarted replacement rejoined the relay", () => relay.has(replacement.address));
    await wait("restarted replacement services knows the accepted history", async () =>
      (await overview(recovered)).computers.some(row => row.key === c.key), 35_000);
    await deliver(second, b, replacement);
    assert.equal(cursor(replacement), 2);
    await checkHistory(recovered, true);
    await checkLimits();
    unintercept.forEach(resume => resume());
  });

test("failure bench: a mixed-build update cut and lost move reply survive disk loss, restore, and replay",
  { timeout: 210_000 }, async t => {
    const bun = testBun();
    assert.ok(bun, NO_BUN);
    const session = "ses_owned";
    const recovery = { build: recoveryNeighborBuild(), session };
    const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
    const pin = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
    const { a, b, c, relay, see, leaseRelay, fixtureEngine, phase } = await network(t, true, pin, false, undefined, false, false,
      ["pc2", "pc4", "pc5"], { recovery });
    const root = a.env.TMPDIR;
    const read = file => JSON.parse(fs.readFileSync(file, "utf8"));
    const backup = (box, action, body) => box.route("POST", "kuvo/box/backups/" + action, body, true);
    const good = (result, status = 200) => {
      assert.equal(result.status, status, JSON.stringify(result.value));
      return result.value;
    };
    const mail = (packet, recipient, saved = recipient.read()) => {
      const frame = link.readFrame(packet.payload);
      return frame && link.openMail(frame.body, saved)?.payload;
    };
    const name = "Recovered with receipt";
    await see(a, 2, a);
    await see(b, 2, a);
    await see(c, 2, a);
    await until("pc2 holds its lease", () => a.lease(2, b), t.signal);
    phase("epoch 2 leased to the primary with the neighboring build enrolled on pc5");

    // Save the receipt on the client side, outside the disk about to be lost.
    const saved = good(await rename(a, c, name));
    assertReceipt(a, "setupNames", "committed", saved);
    await until("pc4 keeps the main's decision", () => b.read().sharedOutcomes?.[saved.operation]?.status === "committed", t.signal);
    const receiptFile = path.join(root, "client-receipt.json");
    put(receiptFile, JSON.stringify(saved));
    const bytes = Buffer.of(0, 255, 3, 128);
    const { id: item, paths: [attachment] } = await uploadQueueFiles(a, { "recovery.lock": bytes });
    await until("pc2 has listed its session", () => peerOwns(b, a, session), t.signal);
    const deleted = editAddress(a, itemID());
    const created = good(await editSubmit(a, deleted, {
      operation: "create-before-delete", create: true, baseRevision: null, fields: { title: "Delete this item" },
    }));
    const baseRevision = created.revision;
    assert.ok(baseRevision);
    assert.equal(good(await editSubmit(a, deleted, {
      operation: "delete-before-move", baseRevision, delete: true,
    })).status, "committed");
    const wake = `wakes/${session}/wakes/1-abcd`;
    put(path.join(a.home, wake), "One recovered wake");
    put(path.join(a.home, "wakes", session, "session"), session);
    const savedConversation = `agents/sessions/${session}/messages.json`;
    put(path.join(a.home, savedConversation), JSON.stringify(fixtureEngine.messages));
    const [retained] = seedLocalWork(a);
    const retainedFile = path.relative(a.home, retained);
    fs.writeFileSync(retained, Buffer.of(253, 0, 56));

    const usageProcesses = new Map();
    const service = async box => {
      const child = box.launch(bun, ["--config=/dev/null", "--no-env-file", script, "box-services"]);
      usageProcesses.set(box, child);
      const file = path.join(box.run, "box-services.json");
      const record = await until(`${box.name} Box services`, () => {
        assert.equal(child.exitCode, null, `${box.name} Box services exited`);
        return fs.existsSync(file) && read(file).pid === child.pid && read(file);
      }, t.signal);
      const ask = async (route, method = "GET") => {
        const response = await fetch(`http://127.0.0.1:${record.port}/api/kuvo/insights${route}`, {
          method, headers: { authorization: "Basic " + Buffer.from(`opencode:${record.password}`).toString("base64") },
          signal: AbortSignal.any([t.signal, AbortSignal.timeout(4_000)]),
        });
        assert.equal(response.status, method === "GET" ? 200 : 202, await response.clone().text());
        return (await response.json()).data;
      };
      await until(`${box.name} service learned the fleet roster`, async () =>
        (await ask("?period=7")).computers.some(({ key }) => key === b.key), t.signal);
      return ask;
    };
    const callID = "one-provider-call";
    const call = {
      key: crypto.createHash("sha256").update(JSON.stringify(["claude", session, callID])).digest("hex"),
      at: Date.now(), session, computer: b.key, call: callID, provider: "claude", model: "claude-test",
      tokens: { input: 100, output: 50, cacheRead: 0, cacheWrite5m: 0, cacheWrite1h: 0 },
    };
    // Inject the provider's normalized observation at the peer boundary.
    // The enrolled sender's real wire format, Relay ingress, services reader
    // and persistent Insights index are used for both deliveries.
    const insightsPage = (to, after, next, epoch, reset = false) => {
      const seatID = operationID();
      const wire = link.readRelay(link.mailFor(to.entry, {
        at: Date.now(), seatID, seat: { kind: "insights", action: "page", after, next, epoch, reset, more: false, records: [call] },
      }, b.read()));
      return { from: b.address, to: to.address, payload: wire.payload, seatID };
    };
    const sourceInsights = await service(a);
    relay.replay(insightsPage(a, 0, 1, "original"));
    const sourceUsage = await until("pc2's Insights index receives pc4's call", async () => {
      const data = await sourceInsights("?period=7&zone=UTC");
      return data.totals.calls > 0 && data;
    }, AbortSignal.any([t.signal, AbortSignal.timeout(10_000)]));
    assert.deepEqual([sourceUsage.totals.calls, sourceUsage.totals.tokens], [1, 150]);
    phase("one Insights call replicated from pc4 to pc2");
    const numbered = async (box, id) => until("number assigned", async () => {
      const read = await box.route("GET", `kuvo/work-queue/items/${id}`);
      const number = read.value.item?.number;
      return Number.isInteger(number) && number;
    }, t.signal);
    const make = async (box, operation) => good(await box.route("POST", "kuvo/work-queue/items", {
      operation, title: operation,
    }, true)).item;
    const itemNumber = await numbered(a, item);
    const first = await make(c, "before-partition");
    const firstNumber = await numbered(c, first.id);
    const unpause = [a, b].flatMap(box => [relay.pause(c.address, box.address), relay.pause(box.address, c.address)]);
    const offline = await make(c, "numbered-offline");
    assert.equal(offline.number, firstNumber + 1);
    assert.equal((await make(c, "numbered-offline")).id, offline.id);
    phase("number grant replicated and consumed once on the partitioned computer");

    // A separate disposable folder models external disk media without mounting a volume.
    const destination = { kind: "disk", path: fs.realpathSync(root) + "/archive" };
    fs.mkdirSync(destination.path);
    const source = "b_" + operationID();
    const recoveryKey = crypto.randomBytes(32).toString("base64url");
    put(path.join(a.home, "backup", "configuration.json"), JSON.stringify({
      version: 1, enabled: true, source, key: recoveryKey, dest: destination,
      disk: fs.statSync(destination.path).dev, projects: [],
    }));
    const { point } = good(await backup(a, "now"));
    assert.ok(point?.id);
    phase("verified encrypted recovery point published outside pc2");

    const sender = a.read();
    const inbound = new Map();
    let arrived, switchedReplies = 0;
    const holdArrival = relay.intercept(b.address, a.address, packet => {
      const move = mail(packet, a, sender)?.move;
      if (move?.op === "arrived") { arrived ??= packet; return true; }
      if (move?.op === "switched") { switchedReplies++; return true; }
      return false;
    });
    const recordPackets = relay.intercept(a.address, b.address, packet => {
      const move = mail(packet, b)?.move;
      if (move?.op !== "part" && move?.op !== "switch") return false;
      const first = !inbound.has(move.op);
      if (first) inbound.set(move.op, packet);
      // pc4 activates from pc2's first switch: the hand-over is between the two computers.
      // Every switch delivered after that comes from the replay harness, so its answers are causal.
      return move.op === "switch" && !first;
    });
    const { move } = good(await a.route("POST", "kuvo/moves", { kind: "chat", id: session, to: b.key, receipt: true }, true), 202);
    await until("pc4's staged move reply", () => arrived, t.signal);
    const staged = path.join(b.run, "moves", move.id + ".json");
    assert.equal(read(staged).complete, true);
    assert.match(fs.readFileSync(path.join(b.home, "wakes", session, "paused"), "utf8"), /moving/);
    phase("pc4 durably staged the incoming conversation and pc2 has not seen its arrival");

    const fire = async box => {
      const wakeRunner = fileURLToPath(new URL("../src/keeper/kuvo-wake", import.meta.url));
      const result = await box.evaluate("sh", [wakeRunner, "fire", path.join(box.home, "bots"), path.join(box.run, "engine.json")], "", false);
      assert.equal(result.status, 0, result.err);
    };
    await fire(a);
    await fire(b);
    assert.deepEqual(fixtureEngine.wakes, [], "neither staged copy may deliver before transfer");

    const release = benchRelease(root, script, privateKey, pin);
    put(path.join(a.home, "bin", "kuvo-link.js"), fs.readFileSync(script));
    put(path.join(a.home, "bin", "box-version"), "0.8.0");
    put(path.join(a.home, "bin", "kuvo-start"), `#!/bin/sh\n# Kuvo release switch v1\nV=${release.engine}\nexit 0\n`);
    const runner = path.join(a.home, "engine", release.engine, "opencode");
    put(runner, `#!/bin/sh\nBUN_BE_BUN=1 exec '${bun.replaceAll("'", "'\\''")}' "$@"\n`);
    fs.chmodSync(runner, 0o700);
    put(path.join(a.run, "engine.json"), JSON.stringify({ ...read(path.join(a.run, "engine.json")), version: release.engine }));
    // Cut the updater as the verified release lands beside the running one, before it says it's
    // ready for the main's switch. (The updater runs on the engine's Bun, as the keeper starts it.)
    const cut = path.join(root, "update-cut.mjs");
    put(cut, `
      import fs from "node:fs";
      import path from "node:path";
      const rename = fs.renameSync;
      fs.renameSync = (from, to) => {
        rename(from, to);
        if (/\\/versions\\/${release.version.replaceAll(".", "\\.")}-[0-9a-f]{12}\\/bin$/.test(String(to))) {
          const dir = fs.openSync(path.dirname(String(to)), "r");
          fs.fsyncSync(dir);
          fs.closeSync(dir);
          process.kill(process.pid, "SIGKILL");
        }
      };
    `);
    const interrupted = await a.evaluate(bun, [
      "--config=/dev/null", "--no-env-file", "--preload", cut,
      script, "update", "--now", "--from", release.folder, "--key", release.key,
    ]);
    assert.equal(interrupted.signal, "SIGKILL", JSON.stringify(interrupted));
    assert.ok(fs.readdirSync(path.join(a.home, "versions")).some(name => name.startsWith(release.version + "-")));
    assert.equal(fs.readFileSync(path.join(a.home, "bin", "box-version"), "utf8"), "0.8.0", "pc2 still runs its own version");
    assert.equal(fs.existsSync(path.join(a.run, "previous")), false, "nothing switched");
    assert.equal(read(staged).complete, true);
    phase("signed release cut while it was prepared beside the running one, with the move still held");

    relay.replay(arrived);
    await until("pc2 sent its switch and pc4's activation reply was dropped",
      () => inbound.has("switch") && switchedReplies > 0, t.signal);
    const decisionFile = path.join(b.run, "moves", "decisions", move.id + ".json");
    const decision = read(decisionFile);
    assert.equal(decision.as, "switched");
    const owner = () => read(path.join(b.run, "moves", "moved.json")).find(entry => entry.session === session);
    assert.equal(owner()?.key, b.key);
    assert.equal(owner()?.arrived, true);
    assert.equal(b.read().shared?.ownershipAssignments?.value?.[`session computer:${session}`], undefined);
    assert.equal(read(path.join(a.run, "moves", move.id + ".json")).stage, "switching");
    assert.ok(fs.existsSync(fixtureEngine.file(a.name)));
    await fire(a);
    assert.deepEqual(fixtureEngine.wakes, [], "the stale source remains held after it loses ownership");
    phase("pc4 committed ownership and activated once; its switch reply was dropped");

    // While the old sender is still enrolled, exercise the destination's decision
    // on both orders of the original bundle packets. Replay them after revocation too.
    const bundle = [inbound.get("part"), inbound.get("switch")];
    assert.ok(bundle.every(Boolean));
    const orders = [bundle, [...bundle].reverse()];
    for (const order of orders) {
      const replies = switchedReplies;
      order.forEach(relay.replay);
      await until("pc4 answers a replay from its saved move decision", () => switchedReplies > replies, t.signal);
    }

    const stopService = async box => {
      const services = usageProcesses.get(box);
      const stopped = once(services, "exit");
      services.kill();
      await stopped;
    };
    await stopService(a);
    await a.stop();
    fs.rmSync(a.home, { recursive: true });
    assert.equal(fs.existsSync(a.home), false);
    assert.equal(fs.existsSync(path.join(destination.path, "kuvo-backups", source, point.id + ".kuvo")), true);
    phase("pc2 disk erased with its prepared release and move journals unresolved");
    leaseRelay.advance(125_000);
    await see(b, 3, b);
    const decided = b.read().sharedOutcomes?.[read(receiptFile).operation];
    assert.equal(decided?.status, "committed", JSON.stringify(decided));
    assert.equal(decided.kind, "setupNames");
    assert.equal(await setupName(b, c.key), name);
    phase("pc4 elected and the client receipt resolved from its durable replica");

    unpause.forEach(resume => resume());
    // A build from before the relay lease takes a certificate only from its main (or that main's
    // standby, which the converted setup no longer has), so it can't follow a takeover. pc5 follows
    // pc4 once it runs a build that knows the lease.
    await c.stop();
    fs.copyFileSync(path.join(root, "kuvo-link.mjs"), path.join(root, "neighbor.mjs"));
    c.start();
    await until("pc5 follows the replacement primary", () => c.read().setup.primary === b.key && b.lease(), t.signal);
    phase("pc5, on this build now, follows pc4 at its relay term");
    good(await b.route("DELETE", `kuvo/computers/${a.key}`));
    const restore = { destination, source, id: point.id, recoveryKey, confirm: true };
    assert.match(good(await backup(c, "restore", restore), 409).message, /Confirm the replacement on the main computer/);
    good(await backup(b, "match", { source, previous: a.key, replacement: c.key }));
    await until("pc5 sees its approved replacement mapping", () => c.read().shared?.ownershipRecovery?.value?.["replacement:" + a.key], t.signal);
    await until("pc5 heard pc4 announce the conversation it owns", () => peerOwns(c, b, session), t.signal);
    const restored = good(await backup(c, "restore", restore));
    assert.ok(restored.unavailable.some(entry => entry.entry === item && entry.reason === "moved or transfer needs review"),
      JSON.stringify(restored.unavailable));
    assert.equal(restored.unavailable.some(entry => entry.entry === "agents/usage/insights.sqlite"), false);
    assert.deepEqual(fs.readFileSync(path.join(c.home, retainedFile)), Buffer.of(253, 0, 56));
    assert.ok(restored.unavailable.some(entry => entry.entry === savedConversation && entry.reason === "moved"));
    assert.equal(fs.existsSync(path.join(c.home, savedConversation)), false);
    assert.equal(fs.existsSync(path.join(c.home, wake)), false);
    assert.equal(fs.existsSync(path.join(c.home, attachment)), false);
    phase("pc5 restored retained bytes and fenced the stale copy of pc4's conversation");

    const replacementInsights = await service(c);
    const overview = () => replacementInsights("?period=7&zone=UTC");
    const restoredUsage = await overview();
    assert.deepEqual([restoredUsage.totals.calls, restoredUsage.totals.tokens], [1, 150]);
    // A reindexed peer resends the call already held in the restored index.
    // Let each delivery reach the reader before checking its durable cursor.
    const deliverPage = async packet => {
      const start = relay.delivered.length;
      relay.replay(packet);
      await until("pc5 processed the Insights page", () => relay.delivered.slice(start).some(reply =>
        reply.from.equals(c.address) && reply.to.equals(b.address) &&
        mail(reply, b)?.delivered?.id === packet.seatID && mail(reply, b)?.delivered?.ok === true), t.signal);
    };
    const index = path.join(c.home, "agents", "usage", "insights.sqlite");
    const usageCursor = () => {
      const db = new DatabaseSync(index, { readOnly: true });
      try {
        return JSON.parse(db.prepare("SELECT body FROM state WHERE key = ?").get(`cursor/${b.key}`)?.body ?? "null");
      } finally {
        db.close();
      }
    };
    // pc2 is a historical origin on pc5 now, so pc5 reads pc4's stream again from zero.
    const reset = insightsPage(c, 0, 2, "reindexed", true);
    const following = insightsPage(c, 2, 3, "reindexed");
    await deliverPage(following);
    assert.equal(usageCursor(), 1);
    await deliverPage(reset);
    assert.equal(usageCursor(), 2);
    await deliverPage(reset);
    await deliverPage(following);
    assert.equal(usageCursor(), 3);
    const usage = await overview();
    assert.deepEqual([usage.totals.calls, usage.totals.tokens], [1, 150]);
    const usageDetail = await replacementInsights(`?period=7&zone=UTC&session=${session}`);
    assert.deepEqual(usageDetail.calls.map(({ call, computer }) => [call, computer]), [[callID, b.key]]);
    phase("restored Insights index counts the peer call once after duplicate delivery");

    const kept = good(await b.route("GET", `kuvo/work-queue/items/${item}`)).item;
    assert.equal(kept.owner, session);
    assert.equal(kept.number, itemNumber);
    assert.deepEqual(fs.readFileSync(path.join(b.home, attachment)), bytes);
    assert.deepEqual(read(fixtureEngine.file(b.name)).messages, fixtureEngine.messages);
    // The new main numbers from its own counter, with pc2 gone.
    const next = await make(b, "numbered-on-successor");
    const nextNumber = await numbered(b, next.id);
    assert.ok(![itemNumber, firstNumber, offline.number].includes(nextNumber));
    const replayedCreate = await make(c, "numbered-offline");
    assert.equal(replayedCreate.id, offline.id);
    assert.equal(replayedCreate.number, offline.number);
    assert.equal(new Set([itemNumber, firstNumber, offline.number, nextNumber]).size, 4);

    const lateRef = editAddress(b, deleted.id);
    const late = { operation: "old-edit", baseRevision, fields: { body: "Late text" } };
    assert.equal(good(await editSubmit(b, lateRef, late), 409).status, "deleted");
    assert.equal(good(await editSubmit(b, lateRef, late), 409).status, "deleted");
    assert.equal(good(await editRead(b, lateRef)).deleted, true);
    const tombstone = good(await b.route("GET", `kuvo/work-queue/items/${deleted.id}`)).item;
    assert.ok(tombstone.removed?.at);
    assert.equal(tombstone.body, "");
    assert.equal(tombstone.events.filter(event => event.kind === "deleted").length, 1);

    await fire(c);
    await fire(b);
    recordPackets();
    holdArrival();
    for (const order of orders) order.forEach(relay.replay);
    await fire(b);
    await fire(c);
    assert.deepEqual(fixtureEngine.imports, [b.name]);
    assert.deepEqual(fixtureEngine.wakes.map(({ box, text }) => [box, text]), [[b.name, "[Wake-up] One recovered wake"]]);
    assert.deepEqual(read(decisionFile), decision);
    assert.equal(owner()?.key, b.key);
    phase("replayed move and edits kept one import, one wake, the deletion, and distinct numbers");
  });
