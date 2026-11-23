import "./test-safety.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { put } from "./recovery-bench.js";
import {
  acceptedEdit, deviceAt, editAddress, editPath, editRead, editSubmit, enrollDesktop, itemID,
  leasedNetwork, link, network, peerOwns, pendingOperation, rename, script, setupName, until,
} from "./setup-handover-fixture.js";
import { inlineFiles } from "./setup-removal-fixture.js";

const personality = (i, length = 40) => ({ id: `custom.personal-${i}`, name: `Personality ${i}`, description: "x".repeat(length), updated: i + 1 });

test("failure bench: the SSH personalities command saves at the main", { timeout: 35_000 }, async t => {
  const { a, b } = await leasedNetwork(t);
  const file = path.join(a.env.HOME, "bots", "personalities.json");
  const oldBytes = () => fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null;
  const before = oldBytes();
  const document = { personalities: [personality(1)], removed: [] };
  const saved = await a.command(["box", "personalities"], JSON.stringify(document));
  assert.equal(saved.status, 0, saved.err);
  assert.equal(saved.value?.status, 200, JSON.stringify(saved));
  assert.equal(saved.value.save?.status, "committed");
  const { setup, operation, proof } = saved.value.save;
  assert.equal(link.readDoc(proof)?.body.operation, operation);
  assert.equal(link.readDoc(proof)?.body.id, setup);
  await until("the member keeps the main's decision", () => b.read().sharedOutcomes?.[operation]?.status === "committed", t.signal);
  assert.deepEqual((await a.route("GET", "kuvo/personalities")).value.personalities.map(each => each.id), ["custom.personal-1"]);
  assert.deepEqual(b.read().shared.setupPersonalities.value.personalities.map(each => each.id), ["custom.personal-1"]);
  assert.equal(oldBytes(), before, "the SSH save did not write to the legacy file");
});

test("box labels restore recovers 344 items' original labels through a two-computer setup", { timeout: 100_000 }, async t => {
  const definitions = [
    ["cat_architecture", "Architecture", "coral"],
    ["cat_security-privacy", "Security & privacy", "orange"],
    ["cat_tests-tooling", "Tests & tooling", "mint"],
    ["cat_features", "Features", "indigo"],
    ["cat_edge-cases", "Edge cases", "coral"],
    ["cat_correctness-sweep", "Correctness sweep", "mint"],
    ["lbl_parked", "Parked", "graphite"],
    ["lbl_my-feedbax", "My Feedbax", "amber"],
    ["lbl_gpui-migration-leftovers", "GPUI Migration Leftovers", "lime"],
    ["lbl_my-feedbacks", "My Feedbacks", "coral"],
  ].map(([id, name, color]) => ({ id, name, color }));
  const owner = "ses_recovered";
  const items = Array.from({ length: 344 }, (_, i) => ({
    id: `wqi_${String(i).padStart(20, "0")}`, title: `Item ${i + 1}`, number: i + 1, status: "todo", order: i + 1,
    labels: [definitions[i % definitions.length].id],
  }));
  const queueOf = box => path.join(box.run, "work-queue", "queues", owner + ".json");
  const { a, b, relay, see } = await network(t, true, undefined, false, undefined, false, true, undefined, {
    macPrimary: true,
    prepare([primary]) {
      const queue = queueOf(primary);
      fs.mkdirSync(path.dirname(queue), { recursive: true });
      fs.writeFileSync(queue, JSON.stringify({ owner, items }));
    },
  });
  const version = "0.10.0", sha256 = "e".repeat(64);
  const macRequest = async (method, route, body, clientVersion = version) => {
    const { port, password } = JSON.parse(fs.readFileSync(path.join(a.run, "serve.json"), "utf8"));
    const response = await fetch(`http://127.0.0.1:${port}/${route}`, {
      method,
      headers: { authorization: `Basic ${Buffer.from(`opencode:${password}`).toString("base64")}`, "x-kuvo-from": "app", "x-kuvo-client-version": clientVersion },
      ...(body ? { body: JSON.stringify(body) } : {}),
      signal: t.signal,
    });
    return { status: response.status, value: await response.json() };
  };
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  await until("primary leased", () => a.lease(2, b), t.signal);
  const signed = Buffer.from(JSON.stringify({ manifest: Buffer.from(JSON.stringify({ clientProtocol: 1 })).toString("base64") })).toString("base64");
  for (const box of [a, b]) {
    fs.mkdirSync(path.join(box.home, "bin"), { recursive: true });
    fs.mkdirSync(path.join(box.home, "updates"), { recursive: true });
    fs.writeFileSync(path.join(box.home, "bin", "box-version"), version);
    fs.writeFileSync(path.join(box.home, "updates", "installed.json"), JSON.stringify({ version, sha256 }));
    const state = box.read();
    state.shared.setupSecurityAndReleaseChoice = { sequence: 1, value: { version, sha256, signed } };
    const file = path.join(box.run, "link.json");
    fs.writeFileSync(file + ".fleet", JSON.stringify(state));
    fs.renameSync(file + ".fleet", file);
  }
  const oldDesktop = await macRequest("GET", "kuvo/info", undefined, "0.8.0-beta.35");
  assert.equal(oldDesktop.status, 200, JSON.stringify(oldDesktop.value));
  const emptyLabels = { labels: [], removed: [] };
  const shared = box => box.read().shared?.queueLabels;
  const labels = box => shared(box)?.value ?? emptyLabels;
  // The member copies the main's labels as soon as they're saved.
  const sameCopy = what => until(what, () => JSON.stringify(shared(b)) === JSON.stringify(shared(a)), t.signal);
  for (const field of ["key", "secret", "sign"]) assert.equal(a.read()[field], undefined);
  assert.deepEqual(labels(a), emptyLabels);
  assert.deepEqual(labels(b), emptyLabels);
  const library = { labels: definitions, removed: [] };
  const file = path.join(a.env.TMPDIR, "saved-labels.json");
  const restore = (box, ...flags) => box.evaluate(process.execPath, [script, "box", "labels", "restore", file, ...flags], "", false);
  const run = async (box, ...flags) => {
    const answer = await restore(box, ...flags);
    assert.equal(answer.status, 0, answer.err);
    return answer.out;
  };
  fs.writeFileSync(file, JSON.stringify(library));
  const versionFile = path.join(a.home, "bin", "box-version");
  try {
    fs.writeFileSync(versionFile, "0.8.0-beta.35");
    const outdated = await restore(a, "--dry-run");
    assert.equal(outdated.status, 0, outdated.err);
    assert.deepEqual(labels(a), emptyLabels);
  } finally {
    fs.writeFileSync(versionFile, version);
  }

  const queue = queueOf(a);
  const readView = async () => {
    const view = await macRequest("GET", `kuvo/work-queue?owner=${owner}&glance=1`);
    assert.equal(view.status, 200, JSON.stringify(view.value));
    assert.equal(view.value.items.length, items.length);
    return view.value;
  };
  const before = await readView();
  assert.deepEqual(before.labels, []);
  assert.ok(before.items.every(item => item.labels.length === 0));
  const unchanged = fs.readFileSync(queue, "utf8");
  await until("initial queue numbering", () => [a, b].every(box => {
    try { return JSON.parse(fs.readFileSync(path.join(box.run, "work-queue", "numbers.json"), "utf8")).since; }
    catch { return false; }
  }), t.signal);

  for (const box of [a, b]) {
    const response = box === a ? await macRequest("POST", "kuvo/work-queue/labels/restore", { library })
      : await b.route("POST", "kuvo/work-queue/labels/restore", { library }, true, { "x-kuvo-client-version": version });
    assert.equal(response.status, 403);
  }
  // (Leaving out the relay lease record, which the box refreshes by itself every few seconds.)
  const snapshot = box => Object.fromEntries(fs.readdirSync(box.home, { recursive: true, withFileTypes: true })
    .filter(entry => entry.isFile() && !entry.name.startsWith("setup-relay-lease.json")).flatMap(entry => {
      const file = path.join(entry.parentPath, entry.name);
      try { return [[path.relative(box.home, file), fs.readFileSync(file)]]; }
      catch (error) {
        if (error.code === "ENOENT") return [];
        throw error;
      }
    }));
  // Keep periodic replication out of the snapshot window. Let the preview and the previous
  // forwarding envelopes through so the old implementation would leave its durable receipt.
  let request;
  let reply;
  const allowRead = to => packet => {
    const wire = link.readFrame(packet.payload);
    const message = wire && link.openMail(wire.body, { ...to.read(), ...to.identity })?.payload;
    const queue = message?.workQueue;
    if (queue?.act?.item === "kuvo/work-queue/labels/restore") request = queue;
    if (queue?.acted?.id && queue.acted.id === request?.act.id) {
      reply = { queue, packet };
      return true;
    }
    return !queue?.act && !queue?.acted;
  };
  const releaseA = relay.intercept(b.address, a.address, allowRead(a));
  const releaseB = relay.intercept(a.address, b.address, allowRead(b));
  try {
    await delay(500);
    fs.writeFileSync(path.join(a.run, "peer-reach-checked.json"), JSON.stringify({ [b.key]: Date.now() }));
    fs.writeFileSync(path.join(b.run, "peer-reach-checked.json"), JSON.stringify({ [a.key]: Date.now() }));
    const snapshots = [snapshot(a), snapshot(b)];
    // (Before the first restore there's no shared library yet: the preview says it uses the main's labels.)
    const syncing = "The shared label library is syncing. This preview uses the main computer’s current labels.\n";
    assert.equal(await run(a, "--dry-run"), syncing + "Dry run: would restore 10, already present 0, conflicts 0.");
    assert.deepEqual(snapshot(a), snapshots[0], "the primary's files are byte-identical after its local dry run");
    const preview = run(b, "--dry-run");
    preview.catch(() => {});
    await until("the forwarded preview reply", () => reply, t.signal);
    assert.equal(request.act.method, "GET");
    assert.equal(request.act.area, "ownership");
    assert.equal(request.memory, true);
    assert.equal(reply.queue.memory, true);
    assert.deepEqual(snapshot(a), snapshots[0], "the primary has no in-flight preview files");
    assert.deepEqual(snapshot(b), snapshots[1], "the member has no in-flight preview files");
    relay.replay(reply.packet);
    assert.equal(await preview, syncing + "Dry run: would restore 10, already present 0, conflicts 0.");
    assert.deepEqual(snapshot(a), snapshots[0], "the primary's files are byte-identical after the forwarded dry run");
    assert.deepEqual(snapshot(b), snapshots[1], "the member's files are byte-identical after the forwarded dry run");
  } finally {
    releaseA();
    releaseB();
  }
  assert.deepEqual(labels(a), emptyLabels);
  assert.deepEqual(labels(b), emptyLabels);
  assert.equal(pendingOperation(a, "queueLabels"), undefined);
  assert.equal(fs.readFileSync(queue, "utf8"), unchanged);

  assert.equal(await run(b), "Restored 10, already present 0, conflicts 0.");
  assert.deepEqual(labels(a), library);
  await sameCopy("the member copied the restored labels");
  const restoredAt = shared(a).sequence;
  const after = await readView();
  assert.deepEqual(after.labels, definitions);
  const byId = new Map(after.items.map(item => [item.id, item.labels]));
  for (const item of items) assert.deepEqual(byId.get(item.id), item.labels);
  assert.equal(fs.readFileSync(queue, "utf8"), unchanged);

  assert.equal(await run(a), "Restored 0, already present 10, conflicts 0.");
  assert.deepEqual(labels(a), library);
  assert.equal(shared(a).sequence, restoredAt);
  await sameCopy("the member kept the main's labels");
  assert.equal(pendingOperation(a, "queueLabels"), undefined);

  const [removed, kept] = definitions;
  const deletion = await macRequest("POST", "kuvo/work-queue/labels", { remove: removed.id });
  assert.equal(deletion.status, 200, JSON.stringify(deletion.value));
  assert.deepEqual(labels(a), { labels: definitions.slice(1), removed: [removed.id] });
  assert.equal(shared(a).sequence, restoredAt + 1);
  // A forced restore may clear the tombstone, but any existing definition still wins.
  fs.writeFileSync(file, JSON.stringify({ labels: [removed, { ...kept, name: "Different name", color: "pink" }], removed: [kept.id] }));
  assert.equal(await run(a),
    `Restored 0, already present 0, conflicts 2.\n${removed.id}: This label was removed. Use --force to restore it.\n${kept.id}: This ID already has a different name or color.`);
  assert.equal(shared(a).sequence, restoredAt + 1);
  await sameCopy("the member kept the main's labels");
  assert.equal(await run(a, "--force", "--dry-run"),
    `Dry run: would restore 1, already present 0, conflicts 1.\n${kept.id}: This ID already has a different name or color.`);
  assert.equal(shared(a).sequence, restoredAt + 1);
  await sameCopy("the member kept the main's labels");

  // With the member cut off, the main saves alone; the member copies it once back.
  const resume = relay.pause(a.address, b.address);
  try {
    const forced = await restore(a, "--force");
    assert.equal(forced.status, 0, forced.err);
    assert.equal(shared(a).sequence, restoredAt + 2);
    assert.equal(shared(b).sequence, restoredAt + 1);
  } finally { resume(); }
  assert.deepEqual(labels(a), { labels: [...definitions.slice(1), removed], removed: [] });
  await sameCopy("the member copied the forced restore");
});

test("box labels restore dry run asks for an update before sending to an old primary", { timeout: 100_000 }, async t => {
  const { a, b, relay, see } = await network(t, true, undefined, false, undefined, false, true, undefined, { macPrimary: true, oldPrimary: true });
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  const featureList = JSON.parse(fs.readFileSync(path.join(b.run, "peer-agents.json"), "utf8"))[a.key].features;
  assert.ok(featureList.includes("work-queue.labels.restore"));
  assert.ok(!featureList.includes("work-queue.labels.restore.preview"));
  assert.ok(!featureList.includes("work-queue.forward.memory"));
  const file = path.join(b.env.TMPDIR, "preview-labels.json");
  fs.writeFileSync(file, JSON.stringify([{ id: "cat_tests", name: "Tests", color: "mint" }]));
  let sent = 0;
  const stop = relay.intercept(b.address, a.address, packet => {
    const wire = link.readFrame(packet.payload);
    const payload = wire && link.openMail(wire.body, { ...a.read(), ...a.identity })?.payload;
    if (payload?.workQueue?.act?.item === "kuvo/work-queue/labels/restore") ++sent;
    return false;
  });
  try {
    const reply = await b.evaluate(process.execPath, [script, "box", "labels", "restore", file, "--dry-run"], "", false);
    assert.equal(reply.status, 1);
    assert.equal(reply.out, "");
    assert.equal(reply.err.trim(), "Update the box on the main computer before previewing a restore from another computer.");
    assert.equal(sent, 0);
  } finally { stop(); }
});

test("box labels restore dry run returns promptly when the relay can't reach the primary", { timeout: 100_000 }, async t => {
  const { a, b, relay, see } = await network(t, true, undefined, false, undefined, false, true, undefined, { macPrimary: true });
  await Promise.all([see(a, 2, a), see(b, 2, a)]);
  const file = path.join(b.env.TMPDIR, "offline-preview.json");
  fs.writeFileSync(file, JSON.stringify([{ id: "cat_tests", name: "Tests", color: "mint" }]));
  const reach = () => ["peer-reach.json", "peer-reach-checked.json", "phone-trust.json"].map(name => {
    const file = path.join(b.run, name);
    return fs.existsSync(file) ? fs.readFileSync(file) : null;
  });
  let sent = false;
  const stop = relay.intercept(b.address, a.address, packet => {
    const wire = link.readFrame(packet.payload);
    const payload = wire && link.openMail(wire.body, { ...a.read(), ...a.identity })?.payload;
    if (payload?.workQueue?.memory && payload.workQueue.act?.item === "kuvo/work-queue/labels/restore") {
      sent = true;
      return false;
    }
    // Only the preview should generate an unreachable delivery during this check.
    return true;
  });
  const reconnect = relay.disconnect(a.address);
  try {
    await until("the main computer to go offline", () => !relay.has(a.address), t.signal);
    const before = reach();
    const started = performance.now();
    const result = await b.evaluate(process.execPath, [script, "box", "labels", "restore", file, "--dry-run"], "", false);
    assert.equal(result.status, 1);
    assert.equal(result.out, "");
    assert.equal(result.err.trim(), `${a.name} isn't reachable. Try again once it's back.`);
    assert.ok(sent, "the preview went through the relay");
    assert.ok(performance.now() - started < 10_000, "the CLI returned before the 20-second read timeout");
    assert.deepEqual(reach(), before, "the failed delivery leaves reach and phone trust files untouched");
  } finally {
    stop();
    reconnect();
  }
});

test("failure bench: a saved change survives killing the main before it publishes to everyone", { timeout: 45_000 }, async t => {
  const { a, b, c, relay, see, leaseRelay, phone, pairing } = await leasedNetwork(t);
  // Prevent the third computer from learning the change from the old main.
  const reconnect = [relay.pause(a.address, c.address), relay.pause(b.address, c.address)];
  for (const [box, name] of [[b, "Renamed Desk"], [c, "Accepted name"]]) {
    const done = await rename(a, box, name);
    assert.equal(done.status, 200, JSON.stringify(done.value));
    assert.equal(done.value.status, "committed");
  }
  await until("the other member copied the names", () => b.read().shared?.setupNames?.value?.[c.key] === "Accepted name", t.signal);
  assert.equal(await setupName(c, c.key), c.name);
  assert.equal((await a.route("POST", "kuvo/phones/remove", { phones: [{ key: phone, pairing }] }, true)).status, 200);
  await until("the other member copied the revocation", () => b.read().shared?.membershipAndAccess?.value?.devices?.removed?.some(device => device.key === phone), t.signal);
  await a.stop();
  await b.stop("SIGTERM");
  const saved = b.read();
  // Drop the ordinary roster publication while retaining the certificate and the copied
  // setup state. The takeover must carry the accepted names.
  delete saved.docs?.[`${a.sign} peers`];
  delete saved.docs?.[`${a.sign} phones`];
  fs.writeFileSync(path.join(b.run, "link.json"), JSON.stringify(saved));
  b.start();
  reconnect.forEach(resume => resume());
  const lateClaim = leaseRelay.disconnect(c.key);
  leaseRelay.advance(125_000);
  await see(b, 3, b);
  lateClaim();
  await see(c, 3, b);
  assert.equal(await setupName(b, c.key), "Accepted name");
  assert.equal(await setupName(c, c.key), "Accepted name");
  assert.equal(await setupName(b, b.key), "Renamed Desk");
  assert.equal((await b.route("GET", "kuvo/computers")).value.computers.find(box => box.key === b.key)?.name, "Renamed Desk");
  assert.equal(await setupName(c, b.key), "Renamed Desk");
  const discovery = await b.route("GET", "kuvo/hosts");
  assert.equal(discovery.status, 200);
  assert.deepEqual(discovery.value.hosts.map(({ key, name }) => ({ key, name })),
    [{ key: c.key, name: "Accepted name" }, { key: a.key, name: a.name }]);
  assert.deepEqual(await c.route("GET", "kuvo/hosts"), discovery);
  assert.equal(b.read().phones.some(device => device.key === phone), false);
  assert.ok(b.read().removed.some(device => device.key === phone && device.pairing === pairing));
});

test("failure bench: number blocks stay unique across a takeover", { timeout: 55_000 }, async t => {
  const { a, b, c, relay, see, leaseRelay } = await leasedNetwork(t);
  const make = async (box, title) => {
    const answer = await box.route("POST", "kuvo/work-queue/items", { title, operation: crypto.randomUUID() }, true);
    assert.equal(answer.status, 200);
    return answer.value.item;
  };
  const numbered = async (box, id) => {
    while (!t.signal.aborted) {
      const answer = await box.route("GET", `kuvo/work-queue/items/${id}`);
      if (Number.isInteger(answer.value.item?.number)) return answer.value.item.number;
      await delay(150);
    }
    assert.fail("Timed out waiting for item number");
  };
  const first = await make(c, "Numbered by the old main");
  const firstNumber = await numbered(c, first.id);

  // The main stops; the third computer keeps numbering from its block while cut off from the new main.
  const reconnect = [relay.pause(b.address, c.address), relay.pause(c.address, b.address)];
  await a.stop();
  const offline = await make(c, "Uses the existing block offline");
  assert.equal(offline.number, firstNumber + 1);
  // (The always-on member takes over: the third computer is kept off the relay meanwhile.)
  const lateClaim = leaseRelay.disconnect(c.key);
  leaseRelay.advance(125_000);
  await see(b, 3, b);
  lateClaim();
  const next = await make(b, "Numbered by the new main");
  const nextNumber = await numbered(b, next.id);

  reconnect.forEach(resume => resume());
  await see(c, 3, b);
  const stillOffline = await make(c, "Keeps the remainder of the old block");
  assert.equal(stillOffline.number, offline.number + 1);
  assert.equal(new Set([firstNumber, offline.number, nextNumber, stillOffline.number]).size, 4);
});

test("paired devices share ordinary edits and queued grants survive restart and revocation safely", { timeout: 120_000 }, async t => {
  const { a, b, c, relay, prompts, see, showOwnedConversation } = await network(t, true, undefined, true);
  await see(c, 2, a);
  await until("primary leased", () => a.lease(2, b), t.signal);
  const managerKey = link.newKey();
  const desktopKey = link.newKey();
  await enrollDesktop(a, managerKey, "Managing desktop");
  const managerA = await deviceAt(relay, a, managerKey, { client: "desktop" });
  assert.ok(managerA.greeting);
  t.after(managerA.close);
  const desktop = { key: link.publicOf(desktopKey).toString("base64url"), name: "Desktop", kind: "desktop" };
  const enrolled = await a.route("POST", "kuvo/devices/pair", desktop, true);
  assert.equal(enrolled.status, 200, JSON.stringify(enrolled));
  async function open(box, key, hello = { client: "desktop" }) {
    return until("fresh device session", async () => {
      const connection = await deviceAt(relay, box, key, hello);
      if (!connection.greeting) return false;
      t.after(connection.close);
      return connection;
    }, t.signal);
  }
  const manager = await open(b, managerKey);
  const desk = await open(b, desktopKey, { client: "desktop" });
  const configuration = path.join(b.home, "backup", "configuration.json");
  fs.mkdirSync(path.dirname(configuration), { recursive: true });
  fs.writeFileSync(configuration, JSON.stringify({ version: 1, enabled: true, source: "private-source", key: "private-key",
    dest: { kind: "disk", path: "/private/path" }, projects: ["/private/project"], last: { id: "private-id", at: 1234, version: 1 },
    error: "private error" }));
  for (const device of [manager, desk]) {
    assert.deepEqual(await device.ask("GET", "kuvo/box/backups/status"), { status: 200, value: {
      enabled: true, destinationKind: "disk", destinationAvailable: false, lastVerifiedAt: 1234, attention: true,
    } });
    assert.equal((await device.ask("GET", "kuvo/box/backups")).status, 403);
    assert.equal((await device.ask("POST", "kuvo/box/backups/destination", { enabled: false })).status, 403);
    assert.equal((await device.ask("POST", "kuvo/setup/save-local", { kind: "setupSecurityAndReleaseChoice", value: {} })).status, 403);

    const ref = editAddress(c, itemID());
    const draft = { operation: ref.id, baseRevision: null, id: ref.id, owner: ref.queue, computer: c.key,
      fields: { title: "Paired edit" } };
    assert.equal((await device.ask("GET", editPath(ref))).status, 200);
    const saved = await device.ask("POST", "kuvo/work-queue/items", draft);
    assert.equal(saved.status, 202, JSON.stringify(saved));
    assert.equal((await acceptedEdit(b, saved.value.operation, t)).receipt.status, "committed");
    assert.deepEqual(await device.ask("POST", "kuvo/work-queue/items", draft),
      { status: 200, value: (await acceptedEdit(b, saved.value.operation, t)).receipt });
    assert.equal((await editRead(c, ref)).value.fields.title, "Paired edit");
    const path = `kuvo/work-queue/items/${ref.id}`;
    const headers = { "x-kuvo-operation": "scoped-" + ref.id };
    const direct = await device.ask("PATCH", path, { title: "Forwarded edit" }, headers);
    assert.equal(direct.status, 200, JSON.stringify(direct));
    const replay = await device.ask("PATCH", path, { title: "Forwarded edit" }, headers);
    assert.equal(replay.status, 200, JSON.stringify(replay));
    assert.equal(replay.value.item.title, "Forwarded edit");
    assert.equal((await device.ask("PATCH", path, { title: "Different edit" }, headers)).status, 409);
    assert.equal((await editRead(c, ref)).value.fields.title, "Forwarded edit");
  }

  const lost = relay.intercept(c.address, b.address, packet => {
    const payload = link.openMail(link.readFrame(packet.payload)?.body, b.read())?.payload?.workQueue;
    return Boolean(payload?.offlineEditReceipt || payload?.ownerMessageReceipt);
  });
  t.after(lost);
  const alreadyRef = editAddress(c, itemID());
  const submit = async (ref, title) => desk.ask("POST", "kuvo/data/edits", {
    ref, operation: ref.id, baseRevision: null, create: true, fields: { title }, submitter: { kind: "computer" },
  });
  const send = (id, files) => desk.ask("POST", `kuvo/data/messages?session=ses_owned&computer=${c.key}`, { id, words: id, files });
  const alreadyEdit = await submit(alreadyRef, "Accepted with a lost reply");
  const alreadySent = await send("msg_beforerevocation");
  assert.equal(alreadyEdit.status, 202, JSON.stringify(alreadyEdit));
  assert.equal(alreadySent.status, 202, JSON.stringify(alreadySent));
  await until("owner accepted both operations before revocation", async () =>
    prompts.length === 1 && (await editRead(c, alreadyRef)).value.fields.title === "Accepted with a lost reply", t.signal);

  // Capture a pre-revocation envelope and deliver it after reconnect, before the owner has access updates.
  const held = [];
  const releaseWork = relay.intercept(b.address, c.address, packet => {
    const mail = link.openMail(link.readFrame(packet.payload)?.body, c.read())?.payload;
    const payload = mail?.workQueue;
    if (!payload?.scopedOfflineEdit && !payload?.scopedOwnerMessage && !mail?.scopedBotEdit) return false;
    held.push({ packet, payload, bot: mail.scopedBotEdit });
    return true;
  });
  t.after(releaseWork);
  const deniedRef = editAddress(c, itemID());
  const deniedEdit = await submit(deniedRef, "Keep this draft");
  const deniedSend = await send("msg_revokeddevice", inlineFiles({ "revoked.txt": Buffer.from("do not keep") }));
  const bot = "bot_Protected";
  const botFolder = path.join(c.home, "bots", bot);
  fs.mkdirSync(botFolder, { recursive: true });
  fs.writeFileSync(path.join(botFolder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Protected" }));
  fs.writeFileSync(path.join(botFolder, "session"), "ses_owned\n");
  const heldDelete = await desk.ask("DELETE", `kuvo/bots/${bot}?on=${c.key}`);
  assert.equal(heldDelete.status, 202, JSON.stringify(heldDelete));
  assert.equal((await manager.ask("PATCH", `kuvo/bots/${bot}?on=${c.key}`, { name: "Different grant" })).status, 409);
  assert.equal(deniedEdit.status, 202);
  assert.equal(deniedSend.status, 202);
  const beforeRevocation = await until("new work in flight", () => {
    const edit = held.find(({ payload }) => payload?.scopedOfflineEdit?.edit.operation === deniedEdit.value.operation);
    // Messages preserve per-conversation order. The first accepted message still lacks its receipt.
    return edit && edit.packet;
  }, t.signal);
  assert.deepEqual(held.find(({ packet }) => packet === beforeRevocation).payload.scopedOfflineEdit.submitter,
    { kind: "paired", key: desktop.key, pairing: enrolled.value.pairing });

  const oldRef = editAddress(c, itemID());
  const old = await submit(oldRef, "Recover the old draft");
  assert.equal(old.status, 202);
  // Previous layout used an unattributed envelope. Preserve its payload and original digest.
  const oldFile = path.join(b.run, "owned-data", "edit-outbox", old.value.operation + ".json");
  const oldRecord = JSON.parse(fs.readFileSync(oldFile, "utf8"));
  const { queuedEdit, submitter, ...record } = oldRecord;
  const { submitter: _unattributed, ...envelope } = queuedEdit;
  fs.writeFileSync(oldFile, JSON.stringify({ ...record, envelope }));

  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await c.stop("SIGTERM");
  const revoked = await managerA.ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.key, pairing: enrolled.value.pairing }] });
  assert.equal(revoked.status, 200, JSON.stringify(revoked));
  assert.ok(c.read().phones.some(each => each.key === desktop.key));
  await b.stop("SIGTERM");
  b.start();
  const keepStale = relay.pause(a.address, c.address);
  t.after(keepStale);
  c.start();
  reconnect();
  await until("owner is back with cached grants", () => relay.has(c.address), t.signal);
  relay.replay(beforeRevocation);
  assert.equal((await deviceAt(relay, c, desktopKey, { client: "desktop" })).waiting, "Checking this app with your other computers…");
  assert.deepEqual((await editRead(c, deniedRef)).value.fields, {});
  assert.deepEqual(prompts.map(e => e.id), ["msg_beforerevocation"]);
  keepStale();
  releaseWork();
  showOwnedConversation(false);
  lost();

  const messageReceipt = operation => b.route("GET", `kuvo/data/messages/${operation}`);
  for (const [sent, status] of [[alreadySent, "committed"], [deniedSend, "interrupted"]]) {
    const checked = await until(`message is ${status}`, async () => {
      const reply = await messageReceipt(sent.value.operation);
      return reply.value.status !== "pending" && reply;
    }, t.signal);
    assert.equal(checked.value.status, status, JSON.stringify(checked));
    if (status === "committed") showOwnedConversation(true);
  }
  assert.equal((await acceptedEdit(b, alreadyEdit.value.operation, t)).receipt.status, "committed");
  const denied = await acceptedEdit(b, deniedEdit.value.operation, t);
  assert.equal(denied.receipt.status, "interrupted");
  assert.match(denied.receipt.message, /submitting device no longer has access/);
  assert.deepEqual(denied.recovery, { title: "Keep this draft" });
  const legacy = await acceptedEdit(b, old.value.operation, t);
  assert.equal(legacy.receipt.status, "interrupted");
  assert.match(legacy.receipt.message, /predates device tracking/);
  assert.deepEqual(legacy.recovery, { title: "Recover the old draft" });
  await until("the restored session owns its queue again", async () => (await editRead(c, alreadyRef)).status === 200,
    AbortSignal.any([t.signal, AbortSignal.timeout(12_000)]));
  assert.deepEqual((await editRead(c, deniedRef)).value.fields, {});
  await until("held bot receipt preserves the revoked action", async () => {
    const listed = await b.route("GET", "kuvo/bots");
    return listed.value.held?.find(entry => entry.bot === bot)?.waits?.includes("submitting device no longer has access");
  }, t.signal);
  assert.ok(fs.existsSync(path.join(botFolder, "bot.json")));
  assert.ok(!fs.existsSync(path.join(c.home, "attachments", "ses_owned")));
  assert.deepEqual((await editRead(c, oldRef)).value.fields, {});
  assert.deepEqual(prompts.map(e => e.id), ["msg_beforerevocation"]);

  const managerAgain = await open(b, managerKey);
  // An old managed draft stays visible until a deliberate edit replaces it with its own grant.
  const heldFile = path.join(b.run, "bots-held", bot + ".json");
  const oldBot = { id: "bte_abcdefghijklmnop", bot, to: c.key, change: { name: "Original draft", jobLine: "Old text" },
    times: { name: 1, jobLine: 1 }, since: 1, at: 1 };
  const key = crypto.createHmac("sha256", Buffer.from(b.read().secret, "base64url")).update("kuvo bot edits held").digest();
  const sealed = [oldBot.id, bot, c.key, null, oldBot.change, oldBot.times, false, 1, 1];
  fs.writeFileSync(heldFile, JSON.stringify({ ...oldBot, seal: crypto.createHmac("sha256", key).update(JSON.stringify(sealed)).digest("base64url") }));
  const reviewed = await managerAgain.ask("GET", "kuvo/bots");
  assert.match(reviewed.value.held.find(entry => entry.bot === bot).waits, /predates device tracking/);
  const resume = relay.pause(b.address, c.address);
  t.after(resume);
  const replaced = await managerAgain.ask("PATCH", `kuvo/bots/${bot}?on=${c.key}`, { name: "New edit" });
  assert.equal(replaced.status, 202, JSON.stringify(replaced));
  assert.deepEqual(replaced.value.held.change, { name: "New edit" });
});

test("two desktops submit complete drafts to the conversation owner", { timeout: 110_000 }, async t => {
  const engineModels = {
    "/api/provider": [{ id: "deepseek" }, { id: "opencode" }],
    "/api/model": [{ id: "v4", providerID: "deepseek", name: "DeepSeek V4", enabled: true, status: "active", cost: [{ input: 1, output: 2 }] }],
    "/api/integration": [{ id: "deepseek", name: "DeepSeek", methods: [{ type: "key" }], connections: [{ type: "credential", method: "key" }] }],
    "/api/model/default": { providerID: "deepseek", id: "v4" },
  };
  const { a, b, c, relay, prompts, see, showOwnedConversation, tellOwned } = await network(t, true, undefined, true, undefined, false, false, undefined, { engineModels });
  await see(c, 2, a);
  await until("primary lease", () => a.lease(2, b), t.signal);
  const manager = link.newKey();
  const desk = link.newKey();
  await enrollDesktop(a, manager, "Managing desktop");
  const managerA = await deviceAt(relay, a, manager, { client: "desktop" });
  assert.ok(managerA.greeting);
  t.after(managerA.close);
  const paired = await a.route("POST", "kuvo/devices/pair", { key: link.publicOf(desk).toString("base64url"), name: "Desk", kind: "desktop" }, true);
  assert.equal(paired.status, 200, JSON.stringify(paired));
  const route = "kuvo/sessions/ses_owned/draft";
  const provider = engineModels["/api/integration"][0];
  const credentials = provider.connections;
  const available = engineModels["/api/provider"];
  const waitForModel = (device, runs) => until(`owner's model readiness is ${runs}`, async () => {
    const reply = await device.ask("GET", "kuvo/sessions/models?sessions=ses_owned");
    return reply.value?.sessions?.ses_owned?.runs === runs;
  }, AbortSignal.any([t.signal, AbortSignal.timeout(12_000)]));
  for (const [key, hello, id] of [[manager, { client: "desktop" }, "msg_draftfirst"], [desk, { client: "desktop" }, "msg_draftdesk"]]) {
    const device = await until("owner device session", async () => {
      const connection = await deviceAt(relay, c, key, hello);
      return connection.greeting && connection;
    }, t.signal);
    t.after(device.close);
    const draft = { id, text: "Continue", quotedText: "> Context\n\nContinue", message: {
      delivery: "steer", files: [{ uri: "data:text/plain;base64,Y29udGV4dA==", name: "context.txt" }],
    } };
    const initial = await device.ask("POST", route, draft);
    assert.equal(initial.status, 200, JSON.stringify(initial));
    assert.equal(initial.value.message, id, JSON.stringify(initial));
    assert.equal(initial.value.answer.send.text, "Continue");
    assert.match(prompts.at(-1).text, /> Context/);
    assert.ok(prompts.at(-1).files?.length || prompts.at(-1).text.includes("context.txt"));
    showOwnedConversation(false);
    assert.deepEqual(await device.ask("POST", route, draft), initial);
    assert.equal((await device.ask("POST", route, { ...draft, text: "Changed" })).status, 409);
    showOwnedConversation(true);
    provider.connections = [];
    engineModels["/api/provider"] = available.filter(each => each.id !== provider.id);
    tellOwned({ type: "credential.updated" });
    await waitForModel(device, false);
    const refused = await device.ask("POST", route, { id: id + "stale", text: "Did readiness change?", message: {} });
    assert.equal(refused.value.answer?.restore?.text, "Did readiness change?", JSON.stringify(refused));
    provider.connections = credentials;
    engineModels["/api/provider"] = available;
    tellOwned({ type: "credential.updated" });
    await waitForModel(device, true);
  }
  const member = await until("paired session on the listing Box", async () => {
    const connection = await deviceAt(relay, b, manager, { client: "desktop" });
    return connection.greeting && connection;
  }, t.signal);
  t.after(member.close);
  const permit = relay.pause(b.address, c.address);
  t.after(permit);
  const queued = { id: "msg_ownerdraft", text: "Send from the listing Box", message: { delivery: "steer" } };
  const waits = await member.ask("POST", route, queued);
  assert.equal(waits.status, 202, JSON.stringify(waits));
  const outbox = JSON.parse(fs.readFileSync(path.join(b.run, "owned-data", "outbox", waits.value.operation + ".json"), "utf8"));
  assert.equal(outbox.queuedMessage, undefined);
  assert.equal(outbox.operation, undefined);
  assert.equal(outbox.queuedDraft.body.text, queued.text);
  assert.deepEqual(prompts.map(each => each.id), ["msg_draftfirst", "msg_draftdesk"]);
  permit();
  await until("the owner accepts the queued full draft", async () => {
    const receipt = await b.route("GET", `kuvo/data/messages/${waits.value.operation}`);
    return receipt.value.status === "committed";
  }, t.signal);
  assert.deepEqual(prompts.map(each => each.id), ["msg_draftfirst", "msg_draftdesk", "msg_ownerdraft"]);
  const settled = await member.ask("POST", route, queued);
  assert.equal(settled.value.message, queued.id);
  assert.equal(typeof settled.value.acceptedAt, "number");
  assert.deepEqual(await member.ask("POST", route, queued), settled);

  const submit = "kuvo/sessions/ses_owned/submit";
  const recover = "kuvo/sessions/ses_owned/submissions";
  const scope = await member.ask("GET", recover + "?scope=1");
  assert.equal(scope.value.currentOwner, c.key);
  assert.equal(scope.value.canSubmit, true);
  const pinned = { ...queued, id: "msg_recoverdraft", submission: scope.value.setup + "." + c.key + ".msg_recoverdraft" };
  const loseAnswer = relay.pause(c.address, b.address);
  t.after(loseAnswer);
  const holding = await member.ask("POST", submit, pinned);
  assert.equal(holding.status, 202, JSON.stringify(holding));
  await until("owner prompt before listing's receipt", () => prompts.some(prompt => prompt.id === pinned.id), t.signal);
  const recovered = await managerA.ask("GET", recover + "/" + pinned.submission);
  assert.equal(recovered.status, 200, JSON.stringify(recovered));
  assert.equal(recovered.value.outcomes[0]?.status, "committed", JSON.stringify(recovered));
  assert.equal(recovered.value.outcomes[0]?.effect.message, pinned.id);
  assert.equal(recovered.value.outcomes[0]?.originalOwner, c.key);
  loseAnswer();
  await until("original listing receipt", async () => {
    const saved = await member.ask("GET", "kuvo/data/messages/" + holding.value.operation);
    return saved.value.status === "committed";
  }, t.signal);
  assert.equal(prompts.filter(prompt => prompt.id === pinned.id).length, 1);
  const changed = await managerA.ask("POST", submit, { ...pinned, text: "Changed intent on another lister" });
  assert.equal(changed.status, 202, JSON.stringify(changed));
  let changedReceipt;
  await until("changed intent refused by the original owner", async () => {
    const receipt = await managerA.ask("GET", "kuvo/data/messages/" + changed.value.operation);
    if (receipt.value.status !== "interrupted") return false;
    changedReceipt = receipt.value;
    return true;
  }, t.signal);
  const conflict = await managerA.ask("GET", recover + "/" + pinned.submission);
  assert.equal(conflict.value.outcomes[0]?.conflict, true);
  const different = await managerA.ask("GET", recover + "/" + pinned.submission + "?digest=" + changedReceipt.digest);
  assert.equal(different.value.outcomes[0]?.status, "unknown");
  assert.equal(different.value.outcomes[0]?.draft.text, "Changed intent on another lister");
  assert.equal(prompts.filter(prompt => prompt.id === pinned.id).length, 1);

  const deskListing = await until("desktop on the listing Box", async () => {
    const connection = await deviceAt(relay, b, desk, { client: "desktop" });
    return connection.greeting && connection;
  }, t.signal);
  t.after(deskListing.close);
  const acceptedThenRevoked = { ...pinned, id: "msg_beforerevoke", submission: scope.value.setup + "." + c.key + ".msg_beforerevoke" };
  const resumeAccepted = relay.pause(c.address, b.address);
  t.after(resumeAccepted);
  const acceptedReceipt = await deskListing.ask("POST", submit, acceptedThenRevoked);
  assert.equal(acceptedReceipt.status, 202, JSON.stringify(acceptedReceipt));
  const claimFile = path.join(c.run, "owned-data", "submissions", acceptedReceipt.value.operation + ".json");
  const acceptedAt = await until("owner commits the desktop draft before revocation", () => {
    if (!fs.existsSync(claimFile)) return false;
    const claim = JSON.parse(fs.readFileSync(claimFile, "utf8"));
    return claim.answer?.value?.message === acceptedThenRevoked.id && claim.at;
  }, t.signal);
  const parked = { ...pinned, id: "msg_revokeddraft", submission: scope.value.setup + "." + c.key + ".msg_revokeddraft" };
  const resumeDelivery = relay.pause(b.address, c.address);
  t.after(resumeDelivery);
  const waiting = await deskListing.ask("POST", submit, parked);
  assert.equal(waiting.status, 202, JSON.stringify(waiting));
  const revoked = await managerA.ask("POST", "kuvo/phones/remove", { phones: [{ key: link.publicOf(desk).toString("base64url"), pairing: paired.value.pairing }] });
  assert.equal(revoked.status, 200, JSON.stringify(revoked));
  await until("owner has the desktop revocation", () => !c.read().phones.some(device => device.key === link.publicOf(desk).toString("base64url")), t.signal);
  resumeAccepted();
  resumeDelivery();
  const archiveFolder = path.join(b.run, "owned-data", "message-receipts");
  const acceptedFile = path.join(archiveFolder, acceptedReceipt.value.operation + ".json");
  await until("committed receipt survives the revocation", () => fs.existsSync(acceptedFile), t.signal);
  const recoveredAcceptance = JSON.parse(fs.readFileSync(acceptedFile, "utf8")).receipt;
  assert.equal(recoveredAcceptance.status, "committed");
  assert.equal(recoveredAcceptance.acceptedAt, acceptedAt);
  assert.equal(prompts.filter(prompt => prompt.id === acceptedThenRevoked.id).length, 1);
  const retired = path.join(archiveFolder, waiting.value.operation + ".json");
  await until("interrupted receipt from the revoked pairing", () => fs.existsSync(retired), t.signal);
  assert.equal(JSON.parse(fs.readFileSync(retired, "utf8")).receipt.status, "interrupted");
  assert.equal(prompts.filter(prompt => prompt.id === parked.id).length, 0);
  const hidden = await managerA.ask("GET", recover + "/" + parked.submission);
  assert.equal(hidden.value.outcomes.length, 0);
});

test("failure bench: offline fields and notes converge in either reconnect order, and a clash is recorded once", { timeout: 85_000 }, async t => {
  const { a, b, c, relay } = await network(t, false, undefined, true);
  for (const [first, second] of [[a, b], [b, a]]) {
    const ref = editAddress(c, itemID());
    const create = { operation: `create-${ref.id}`, baseRevision: null, create: true, fields: { title: "Original" },
      notes: [{ id: "draft", text: "Alpha red. Beta green." }] };
    const allowCreate = relay.pause(first.address, c.address);
    const made = await editSubmit(first, ref, create);
    assert.equal(made.status, 202, JSON.stringify(made));
    assert.deepEqual((await editRead(c, ref)).value.fields, {});
    allowCreate();
    assert.equal((await acceptedEdit(first, made.value.operation, t)).receipt.status, "committed");
    assert.equal((await editSubmit(first, ref, create)).value.status, "committed");
    const [left, right] = await Promise.all([editRead(first, ref), editRead(second, ref)]);
    assert.equal(left.status, 200, JSON.stringify(left));
    assert.equal(right.status, 200, JSON.stringify(right));
    const baseRevision = left.value.revision;
    assert.equal(right.value.revision, baseRevision);

    const reconnectFirst = relay.pause(first.address, c.address);
    const reconnectSecond = relay.pause(second.address, c.address);
    const attachment = { id: "same-file", file: { uri: "data:text/plain;base64,cXVldWUgYXR0YWNobWVudA==", name: "queue.txt" } };
    const one = { operation: `first-${ref.id}`, baseRevision, fields: { title: first.name, "note:draft": "Alpha blue. Beta green." },
      notes: [{ id: "first-note", text: "First addition" }], attachments: [attachment] };
    const two = { operation: `second-${ref.id}`, baseRevision, fields: { title: second.name, status: "waiting", "note:draft": "Alpha red. Beta orange." },
      notes: [{ id: "second-note", text: "Second addition" }], attachments: [attachment] };
    const [sentOne, sentTwo] = await Promise.all([editSubmit(first, ref, one), editSubmit(second, ref, two)]);
    assert.equal(sentOne.status, 202, JSON.stringify(sentOne));
    assert.equal(sentTwo.status, 202, JSON.stringify(sentTwo));
    assert.equal((await editRead(c, ref)).value.fields.title, "Original");
    reconnectFirst();
    assert.equal((await acceptedEdit(first, sentOne.value.operation, t)).receipt.status, "committed");
    const releaseReceipt = relay.pause(c.address, second.address);
    reconnectSecond();
    let owner;
    while (!t.signal.aborted) {
      owner = await editRead(c, ref);
      if (owner.value.choices?.length) break;
      await delay(100);
    }
    assert.equal(owner.status, 200);
    assert.equal(owner.value.choices.length, 1);
    assert.equal(owner.value.choices[0].accepted, first.name);
    assert.equal(owner.value.choices[0].proposed, second.name);
    assert.equal(owner.value.fields.status, "waiting");
    assert.equal(owner.value.fields["note:draft"], "Alpha blue. Beta orange.");
    assert.equal(owner.value.fields["note:first-note"], "First addition");
    assert.equal(owner.value.fields["note:second-note"], "Second addition");

    await c.stop("SIGTERM");
    releaseReceipt();
    c.start();
    const clash = (await acceptedEdit(second, sentTwo.value.operation, t)).receipt;
    assert.equal(clash.status, "choice");
    assert.deepEqual(clash.choices, [owner.value.choices[0].id]);
    const both = await Promise.all([editRead(first, ref), editRead(second, ref)]);
    for (const result of both) {
      assert.equal(result.status, 200, JSON.stringify(result));
      assert.equal(result.value.stale, false);
      assert.deepEqual(result.value.choices, owner.value.choices);
    }
    const item = await c.route("GET", `kuvo/work-queue/items/${ref.id}`);
    assert.equal(item.status, 200);
    assert.deepEqual(item.value.item.files.map(file => file.name), ["queue.txt"]);
    const resolved = await editSubmit(c, ref, { operation: `resolve-${ref.id}`, baseRevision: owner.value.revision,
      resolve: { id: clash.choices[0], use: "proposed" } });
    assert.equal(resolved.value.status, "committed", JSON.stringify(resolved));
    const current = (await editRead(c, ref)).value;
    assert.equal(current.fields.title, second.name);
    assert.deepEqual(current.choices, []);
  }
});

test("failure bench: deletion recovers late text and a long-absent copy must refresh", { timeout: 55_000 }, async t => {
  const { a, b, c, relay } = await network(t, false, undefined, true);
  const ref = editAddress(c, itemID());
  const edit = { operation: "creation", create: true, baseRevision: null, fields: { title: "Original" } };
  const made = await editSubmit(c, ref, edit);
  assert.equal(made.value.status, "committed", JSON.stringify(made));
  const baseRevision = (await editRead(b, ref)).value.revision;
  assert.equal((await editRead(a, ref)).value.revision, baseRevision);
  const reconnectB = relay.pause(b.address, c.address);
  const reconnectA = relay.pause(a.address, c.address);
  const first = await editSubmit(b, ref, { operation: "late", baseRevision, fields: { body: "Text to recover" } });
  const last = await editSubmit(a, ref, { operation: "months-late", baseRevision, fields: { body: "Text from the old copy" } });
  assert.equal(first.status, 202);
  assert.equal(last.status, 202);
  assert.equal((await editSubmit(c, ref, { operation: "delete", baseRevision, delete: true })).value.status, "committed");
  reconnectB();
  const deleted = await acceptedEdit(b, first.value.operation, t);
  assert.equal(deleted.receipt.status, "deleted");
  assert.deepEqual(deleted.recovery, { body: "Text to recover" });
  const outbox = await b.route("GET", "kuvo/data/edits/outbox", undefined, true);
  assert.equal(outbox.status, 200);
  assert.ok(outbox.value.entries.some(entry => entry.ref.id === ref.id && entry.receipt.operation === first.value.operation));
  assert.equal((await editRead(c, ref)).value.deleted, true);

  // Move the owner’s deletion past the supported window without changing any writer's clock.
  // Stand-in homes are the only files touched.
  const queue = path.join(c.run, "work-queue", "queues", "ses_owned.json");
  const kept = JSON.parse(fs.readFileSync(queue, "utf8"));
  kept.items.find(item => item.id === ref.id).removed.at -= 90 * 86_400_000;
  fs.writeFileSync(queue + ".bench", JSON.stringify(kept));
  fs.renameSync(queue + ".bench", queue);
  const snapshot = await editRead(c, ref);
  assert.equal(snapshot.value.deleted, true);
  assert.deepEqual(snapshot.value.fields, {});
  reconnectA();
  const old = await acceptedEdit(a, last.value.operation, t);
  assert.equal(old.receipt.status, "refresh");
  assert.deepEqual(old.recovery, { body: "Text from the old copy" });
  const fresh = await editRead(a, ref);
  assert.equal(fresh.value.stale, false);
  assert.equal(fresh.value.revision, old.receipt.revision);
  assert.equal(fresh.value.deleted, true);
  const resurrect = await editSubmit(c, ref, { ...edit, operation: "create-again" });
  assert.equal(resurrect.value.status, "refresh");
  assert.equal((await c.route("GET", `kuvo/work-queue/items/${ref.id}`)).status, 404);
});

test("failure bench: setup preferences merge at the main and survive a takeover", { timeout: 55_000 }, async t => {
  const { a, b, c, leaseRelay } = await leasedNetwork(t);
  const ref = { kind: "setupUsageDisplay" };
  const read = await editRead(c, ref);
  assert.equal(read.status, 200, JSON.stringify(read));
  const { revision: baseRevision } = read.value;
  const first = await editSubmit(c, ref, { operation: "usage-card", baseRevision, fields: { totalSpend: { show: true } } });
  assert.ok(first.status === 200 || first.status === 202, JSON.stringify(first));
  assert.equal((await acceptedEdit(c, first.value.operation, t)).receipt.status, "committed");
  const second = await editSubmit(c, ref, { operation: "usage-model", baseRevision, fields: { codexFallback: "gpt-5.4" } });
  assert.equal((await acceptedEdit(c, second.value.operation, t)).receipt.status, "committed");
  const actual = await editRead(a, ref);
  assert.deepEqual(actual.value.fields, { codexFallback: "gpt-5.4", totalSpend: { show: true } });
  const third = await editSubmit(c, ref, { operation: "usage-other-model", baseRevision, fields: { codexFallback: "gpt-5.3-codex" } });
  const conflict = (await acceptedEdit(c, third.value.operation, t)).receipt;
  assert.equal(conflict.status, "choice");
  await until("the members copied the choice", () => [b, c].every(box => box.read().shared?.setupUsageDisplay?.offlineEdits?.choices?.length), t.signal);
  const copies = [a, b, c].map(box => box.read().shared?.setupUsageDisplay?.offlineEdits?.choices);
  assert.equal(copies[0][0].id, conflict.choices[0]);
  assert.deepEqual(copies[0], copies[1]);
  const current = await editRead(c, ref);
  assert.deepEqual(current.value.choices, copies[0]);
  await a.stop();
  leaseRelay.disconnect(c.key);
  leaseRelay.advance(125_000);
  await until("the always-on member takes over with the accepted preference choices", () => b.read().setup?.role === "primary" && b.read().setup?.epoch === 3, t.signal);
  const after = await editRead(b, ref);
  assert.deepEqual(after.value.fields, actual.value.fields);
  assert.deepEqual(after.value.choices, copies[0]);
});

test("failure bench: a bot's offline name and instructions are applied only at its home", { timeout: 45_000 }, async t => {
  const id = "bot_Lab";
  const { a, b, c, relay } = await network(t, false, undefined, true, undefined, false, false, undefined, {
    prepare: ([, , home]) => {
      const folder = path.join(home.home, "bots", id);
      fs.mkdirSync(path.join(home.home, "bin"), { recursive: true });
      fs.copyFileSync(new URL("../src/shared/bot-words.json", import.meta.url), path.join(home.home, "bin", "bot-words.json"));
      put(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id, name: "Lab bot" }));
      put(path.join(folder, "session"), "ses_owned\n");
      put(path.join(folder, "job.json"), JSON.stringify({ line: "Report status", details: "Check the queue." }));
    },
  });
  const ref = { kind: "botSettings", id, computer: c.key };
  await until("both computers know the bot's home", () => [a, b].every(box => peerOwns(box, c, id)), t.signal);
  const { value: before, status } = await editRead(a, ref);
  assert.equal(status, 200, JSON.stringify(before));
  const baseRevision = before.revision;
  const reconnect = relay.pause(a.address, c.address);
  const name = await editSubmit(a, ref, { operation: "name", baseRevision, fields: { name: "Ready bot" } });
  const instructions = await editSubmit(b, ref, { operation: "instructions", baseRevision, fields: { jobDetails: "Check the queue and summarize." } });
  assert.equal(name.status, 202);
  assert.equal((await acceptedEdit(b, instructions.value.operation, t)).receipt.status, "committed");
  assert.equal((await editRead(c, ref)).value.fields.name, "Lab bot");
  reconnect();
  assert.equal((await acceptedEdit(a, name.value.operation, t)).receipt.status, "committed");
  const { value: after } = await editRead(b, ref);
  assert.equal(after.fields.name, "Ready bot");
  assert.equal(after.fields.jobDetails, "Check the queue and summarize.");
  assert.deepEqual(after.choices, []);
  assert.equal(fs.existsSync(path.join(path.dirname(a.run), "bots", ref.id)), false);
  assert.equal(fs.existsSync(path.join(path.dirname(b.run), "bots", ref.id)), false);
});
