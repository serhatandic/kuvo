import "./test-safety.js";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { once } from "node:events";
import { spawn } from "node:child_process";
import { test } from "node:test";
import { put } from "./recovery-bench.js";
import {
  assertReceipt, deviceAt, enrollDesktop, leasedNetwork, link, network, rename, script, submit,
  until,
} from "./setup-handover-fixture.js";

test("failure bench: unlink retries credential cleanup before acknowledging completion", { timeout: 45_000 }, async t => {
  const { a, b, c, see } = await leasedNetwork(t);
  await see(c, 2, a);
  const folder = path.join(c.home, "data", "opencode");
  const database = path.join(folder, "opencode.db");
  const next = path.join(folder, "opencode-next.db");
  const auth = path.join(folder, "auth.json");
  const agentAuth = path.join(c.home, "agents", "codex", "accounts", "test-account", "auth.json");
  const otherFolder = path.join(c.home, "engine-data-fixture");
  const other = path.join(otherFolder, "opencode-preview.db");
  c.env.KUVO_ENGINE_DB_FOLDER = otherFolder;
  const keys = path.join(c.home, "box", "keys", "KUVO_KEY_NOTION_TOKEN");
  const bot = path.join(c.home, "bots", "bot_fixture", "bot.json");
  const native = path.join(c.env.HOME, ".codex", "auth.json");
  for (const file of [database, keys, bot, native, auth, agentAuth]) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, "fixture");
  }
  const operation = "restart-unlink-cleanup-test";
  const removed = await a.route("DELETE", `kuvo/computers/${c.key}`, { operation });
  assert.equal(removed.status, 200, JSON.stringify(removed.value));
  await until("unlink journal persisted", () => c.read().localUnlink?.operation === operation, t.signal);
  const record = () => a.read().shared.membershipAndAccess.value.removals[c.key];
  assert.equal(record().delivered, undefined);
  assert.equal(c.read().setup.id, a.read().setup.id);
  assert.ok(fs.existsSync(keys));

  await c.stop("SIGTERM");
  fs.unlinkSync(database);
  fs.mkdirSync(otherFolder);
  for (const file of [database, next, other]) {
    const db = new DatabaseSync(file);
    db.exec("CREATE TABLE credential (value TEXT); INSERT INTO credential VALUES ('fixture'); CREATE TABLE session (id TEXT); INSERT INTO session VALUES ('session');");
    db.close();
  }
  const keychain = new Set(["KUVO_KEY_NOTION_TOKEN", "KUVO_KEY_NOTION_TOKEN__BOT_LEGACY"]);
  const hostPassword = crypto.randomBytes(24).toString("hex");
  const host = http.createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const given = JSON.parse(Buffer.concat(chunks));
    if (request.url === "/hello") {
      const proof = crypto.createHmac("sha256", hostPassword)
        .update(`kuvo-host-hello\0${given.nonce}\0${request.headers.host}`).digest("hex");
      return response.end(JSON.stringify({ proof }));
    }
    if (request.headers.authorization !== `Bearer ${hostPassword}`) return response.writeHead(401).end();
    if (request.url === "/keys/names") return response.end(JSON.stringify({ names: [...keychain] }));
    if (request.url === "/keys/delete") {
      keychain.delete(given.name);
      return response.end("{}");
    }
    response.writeHead(404).end();
  });
  host.listen(0, "127.0.0.1");
  await once(host, "listening");
  t.after(() => { host.closeAllConnections(); return new Promise(resolve => host.close(resolve)); });
  c.env.KUVO_KEYS_ON_HOST = "1";
  fs.writeFileSync(path.join(c.run, "host.json"), JSON.stringify({ url: `http://127.0.0.1:${host.address().port}`, password: hostPassword }));
  c.start();
  await until("primary receives completed unlink", () => record().delivered === true, t.signal);
  await until("source receives the acknowledgement", () => !c.read().localUnlink, t.signal);
  assert.notEqual(c.read().setup.id, a.read().setup.id);
  for (const file of [database, next, other]) {
    const remaining = new DatabaseSync(file);
    assert.equal(remaining.prepare("SELECT COUNT(*) AS n FROM credential").get().n, 0, path.basename(file));
    assert.equal(remaining.prepare("SELECT id FROM session").get().id, "session");
    remaining.close();
  }
  for (const file of [auth, agentAuth]) assert.equal(fs.existsSync(file), false);
  assert.equal(keychain.size, 0);
  assert.equal(fs.existsSync(keys), false);
  for (const file of [bot, native]) assert.equal(fs.readFileSync(file, "utf8"), "fixture");
});

// A computer's own pairing is independent work: SSH refuses that computer, and it joins through a
// setup merge (installer-enrollment.test.js). A member keeps a pairing it made before joining aside
// (`preJoinPairings`) until its main grants it; re-enrolling it takes an explicit decision.
test("an unmatched pre-join pairing requires an explicit enrollment decision", { timeout: 45_000 }, async t => {
  // The main grants this phone with `pairing`, which the member paired on its own before joining.
  const { a, c, phone, pairing, relay } = await network(t);
  await c.stop("SIGTERM");
  const file = path.join(c.run, "link.json");
  const local = { ...c.read(), preJoinPairings: [{ key: phone, pairing, from: "pair", local: true }] };
  fs.writeFileSync(file, JSON.stringify(local));
  // Keep this CLI fixture from launching a second, untracked bridge.
  fs.mkdirSync(path.join(c.run, ".updating"));
  fs.writeFileSync(path.join(c.run, ".updating", "pid"), String(process.pid));
  const token = "guarded-" + crypto.randomBytes(24).toString("hex");
  fs.symlinkSync(token, path.join(path.dirname(c.home), ".kuvo-code-adding"));
  const signed = link.signDoc(a.seed, "phones", 1, { phones: [{ key: phone, pairing: "different-pairing" }], removed: [] });
  const refused = await c.command(["setup", token], JSON.stringify({ mac: a.key, macSign: a.sign, docs: [signed] }));
  assert.equal(refused.status, 5, refused.err);
  assert.match(refused.err, /Confirm discarding unmatched local device pairings/);
  assert.deepEqual(c.read(), local);

  const roster = link.signDoc(a.seed, "peers", 2, { setup: { id: a.read().setup.id, epoch: 1, primary: a.key },
    boxes: [a.entry, c.entry], moved: [], left: [] });
  const matching = link.signDoc(a.seed, "phones", 2, { phones: [{ key: phone, pairing }], removed: [] });
  const joined = await c.command(["setup", token], JSON.stringify({ mac: a.key, macSign: a.sign, relay: relay.url, docs: [roster, matching] }));
  assert.equal(joined.status, 0, joined.err);
  assert.equal(c.read().phones.find(grant => grant.key === phone)?.pairing, pairing);
  assert.equal(c.read().phones.find(grant => grant.key === phone)?.from, "mac");
  assert.equal(c.read().preJoinPairings, undefined, "the main's grant replaced the pairing kept aside");
});

test("failure bench: local uninstall refuses a primary before clearing identity or stopping it", { timeout: 30_000 }, async t => {
  const { a } = await leasedNetwork(t);
  const identity = path.join(a.env.HOME, "desktop", "identity");
  fs.mkdirSync(path.dirname(identity));
  fs.writeFileSync(identity, "keep this identity");
  const before = a.read().setup;
  const done = await a.command(["uninstall"]);
  assert.equal(done.status, 1, done.err);
  assert.deepEqual(done.value, { result: "refused", message: "Choose another main computer first." });
  assert.deepEqual(a.read().setup, before);
  assert.equal(fs.readFileSync(identity, "utf8"), "keep this identity");
  assert.equal(a.child.exitCode, null);
  const home = path.join(a.env.HOME, "standalone");
  fs.mkdirSync(path.join(home, "run"), { recursive: true });
  const child = spawn(process.execPath, [script, "uninstall"], { env: { ...a.env, HOME: home, KUVO_HOME: home }, stdio: "ignore" });
  assert.equal((await once(child, "exit"))[0], 0);
  assert.ok(fs.existsSync(path.join(home, "run", "off")));
});

test("failure bench: access routes return committed receipts from the main and through a member", { timeout: 90_000 }, async t => {
  const { a, c } = await leasedNetwork(t);
  const device = name => ({ key: link.publicOf(link.newKey()).toString("base64url"), name, kind: "desktop" });
  const pair = (box, asking) => box.route("POST", "kuvo/devices/pair", asking, true);
  const remove = (box, asking) => box.route("POST", "kuvo/phones/remove", { phones: [{ key: asking.key }] }, true);
  const access = async request => {
    const reply = await request();
    assert.equal(reply.status, 200, JSON.stringify(reply));
    assertReceipt(a, "membershipAndAccess", "committed", reply.value);
    return reply.value;
  };
  const local = device("Local");
  const member = device("Member");
  await access(() => pair(a, local));
  assert.equal((await access(() => pair(c, member))).paired, true);
  await access(() => remove(a, local));
  await access(() => remove(c, member));
  await access(() => a.route("DELETE", `kuvo/computers/${c.key}`));
});

test("failure bench: forwarded computer changes carry receipts through the member route", { timeout: 45_000 }, async t => {
  const { a, b, c } = await leasedNetwork(t);
  const named = await rename(b, c, "Forwarded");
  assert.equal(named.status, 200, JSON.stringify(named));
  assertReceipt(a, "setupNames", "committed", named.value);
  const removed = await b.route("DELETE", `kuvo/computers/${c.key}`);
  assert.equal(removed.status, 200, JSON.stringify(removed));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
});

test("a scoped rename cannot overwrite a later name after primary handover", { timeout: 105_000 }, async t => {
  const { a, b, c, relay, see } = await leasedNetwork(t);
  const key = link.newKey();
  await enrollDesktop(a, key, "Receipt fixture");
  const at = box => until("paired rename client on " + box.name, async () => {
    const device = await deviceAt(relay, box, key, { client: "desktop" });
    if (!device.greeting) return false;
    t.after(device.close);
    return device;
  }, t.signal);
  const listing = await at(c);
  const route = `kuvo/computers/${c.key}`;
  const headers = { "x-kuvo-operation": "stable-forwarded-rename" };
  const intent = { name: "First accepted name" };
  const first = await listing.ask("PATCH", route, intent, headers);
  assert.equal(first.status, 200, JSON.stringify(first));
  const digest = purpose => crypto.createHash("sha256").update(JSON.stringify([purpose, a.read().setup.id, headers["x-kuvo-operation"]])).digest("base64url");
  const readReceipt = (box, family, purpose) => JSON.parse(fs.readFileSync(path.join(box.run, "owned-data", family, digest(purpose) + ".json"), "utf8"));
  assert.equal(readReceipt(c, "forward-origins", "forward-origin").forward.kind, "forwardedOperationOrigins");
  assert.equal(readReceipt(a, "mutations", "scoped-forward").forward.kind, "forwardedPrimaryDecisions");
  const changed = await submit(a, b, 2);
  assert.equal(changed.status, 202, JSON.stringify(changed));
  await Promise.all([see(b, 3, b), see(c, 3, b)]);
  await until("the new main holds the relay lease", () => b.lease(), t.signal);
  const direct = await at(b);
  assert.equal((await direct.ask("PATCH", route, intent, headers)).status, 409);
  const renamed = await rename(b, c, "Name after handover");
  assert.ok([200, 202].includes(renamed.status), JSON.stringify(renamed));
  if (renamed.status === 202) await until("newer name committed", () => b.read().sharedOutcomes?.[renamed.value.operation]?.status === "committed", t.signal);
  assert.equal((await listing.ask("PATCH", route, intent, headers)).status, 409);
  assert.equal((await direct.ask("PATCH", route, intent, headers)).status, 409);
  assert.equal(b.read().shared.setupNames.value[c.key], "Name after handover");
});

test("live permission replies from paired clients expire across pc5 reconnect", { timeout: 80_000 }, async t => {
  const { a, b, c, relay, see, setPermissionAsk, permissionEffects } =
    await network(t, true, undefined, true, undefined, false, false, undefined, { liveAsks: true });
  await see(c, 2, a);
  await until("primary leased", () => a.lease(2, b), t.signal);
  const managerKey = link.newKey();
  const deskKey = link.newKey();
  const desktop = { key: link.publicOf(deskKey).toString("base64url"), name: "Desktop", kind: "desktop" };
  await enrollDesktop(a, managerKey, "Primary desktop");
  const primaryDesktop = await deviceAt(relay, a, managerKey, { client: "desktop" });
  assert.ok(primaryDesktop.greeting);
  t.after(primaryDesktop.close);
  const enrolled = await a.route("POST", "kuvo/devices/pair", desktop, true);
  assert.equal(enrolled.status, 200, JSON.stringify(enrolled.value));
  const open = async (box, key, hello = { client: "desktop" }) => {
    const client = await until("current pairing at " + box.name, async () => {
      const client = await deviceAt(relay, box, key, hello);
      return client.greeting && client;
    }, t.signal);
    t.after(client.close);
    return client;
  };
  const manager = await open(c, managerKey);
  const desk = await open(c, deskKey, { client: "desktop" });
  const lister = await open(b, managerKey);
  const apps = [manager, desk];
  const sessions = ["ses_owned", "ses_native"];
  const native = (session, id) => `${session === "ses_native" ? "kuvo/agents/" : ""}api/session/${session}/permission/${id}/reply`;
  const askRoute = (session, id) => `kuvo/sessions/${session}/asks/${id}`;
  for (const [i, client] of apps.entries()) {
    for (const session of sessions) {
      const id = `per_live_${i}_${session}`;
      setPermissionAsk(session, id);
      const path = i === 0 ? askRoute(session, id) : native(session, id);
      const body = i === 0 ? { answer: "once" } : { decision: "once" };
      assert.equal((await client.ask("POST", path, body)).status, 200);
      assert.equal((await client.ask("POST", path, body)).status, 404);
    }
  }
  assert.equal(permissionEffects.length, 4);

  const settings = path.join(c.home, "box", "opencode.json");
  put(settings, JSON.stringify({ permissions: [{ action: "write", resource: "fixture.txt", effect: "deny" }] }));
  for (const session of sessions) {
    setPermissionAsk(session, "per_localdeny");
    const denied = await desk.ask("POST", native(session, "per_localdeny"), { decision: "always" });
    assert.equal(denied.status, 403, JSON.stringify(denied.value));
    assert.equal((await manager.ask("POST", askRoute(session, "per_localdeny"), { answer: "once" })).status, 403);
    setPermissionAsk(session, "per_denied_sibling", "per_allowed_sibling");
    assert.equal((await desk.ask("POST", native(session, "per_allowed_sibling"), { decision: "always" })).status, 403);
    assert.equal((await manager.ask("POST", askRoute(session, "per_allowed_sibling"), { answer: "always" })).status, 403);
  }
  assert.equal(permissionEffects.length, 4);
  fs.unlinkSync(settings);

  setPermissionAsk("ses_owned", "per_old_engine");
  setPermissionAsk("ses_native", "per_old_bridge");
  assert.equal((await lister.ask("POST", askRoute("ses_owned", "per_old_engine"), { answer: "once" })).status, 409);
  const held = [];
  const release = apps.map(client => relay.intercept(client.address, c.address, packet => { held.push(packet); return true; }));
  release.forEach(stop => t.after(stop));
  const pending = [
    manager.ask("POST", native("ses_owned", "per_old_engine"), { decision: "once" }),
    desk.ask("POST", native("ses_native", "per_old_bridge"), { decision: "always" }),
  ].map(answer => answer.catch(() => ({ closed: true })));
  await until("both live replies are held in transport", () => held.length >= 2, t.signal);
  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await c.stop("SIGTERM");
  const revoked = await primaryDesktop.ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.key, pairing: enrolled.value.pairing }] });
  assert.equal(revoked.status, 200, JSON.stringify(revoked.value));
  setPermissionAsk("ses_owned", "per_replacement_engine");
  setPermissionAsk("ses_native", "per_replacement_bridge");
  c.start();
  reconnect();
  await until("pc5 welcomed", () => relay.has(c.address), t.signal);
  release.forEach(stop => stop());
  held.forEach(packet => relay.replay(packet));
  await until("pc5 received revocation", () => c.read().removed?.some(entry => entry.key === desktop.key), t.signal);
  const refused = await deviceAt(relay, c, deskKey, { client: "desktop" });
  assert.equal(refused.greeting, undefined);
  const connected = await open(c, managerKey);
  for (const [session, id] of [["ses_owned", "per_old_engine"], ["ses_native", "per_old_bridge"]]) {
    assert.equal((await connected.ask("POST", native(session, id), { decision: "once" })).status, 404);
    assert.equal((await connected.ask("POST", askRoute(session, id), { answer: "once" })).status, 404);
  }
  for (const result of await Promise.all(pending)) assert.ok(result.closed, JSON.stringify(result));
  assert.equal(permissionEffects.length, 4);

  // A committed departure leaves old engine bytes accessible to the fixture.
  put(path.join(c.run, "moves", "moved.json"), JSON.stringify([{ session: "ses_owned", key: b.key, at: Date.now() }]));
  const moved = await connected.ask("POST", "api/session/%73es_owned/permission/per_replacement_engine/reply", { decision: "once" });
  assert.equal(moved.status, 409, JSON.stringify(moved.value));
  assert.equal(permissionEffects.length, 4);
});

test("failure bench: clients share machine changes and revocations across three boxes", { timeout: 90_000 }, async t => {
  const { a, b, c, relay, see } = await leasedNetwork(t);
  await see(c, 2, a);
  const open = async (box, key, hello = { client: "desktop" }) => {
    const device = await deviceAt(relay, box, key, hello);
    if (!device.greeting) return device;
    t.after(device.close);
    return device;
  };
  async function byClient(label, read) {
    while (!t.signal.aborted) {
      const value = await read();
      if (value) return value;
      await delay(25);
    }
    throw new Error(`Timed out waiting for ${label}`);
  }
  const pairedAt = (box, key, hello) => byClient(`the paired device reaches ${box.name}`, async () => {
    const device = await open(box, key, hello);
    return device.greeting && device;
  });
  const managerKey = link.newKey();
  const managerPublic = link.publicOf(managerKey).toString("base64url");
  await enrollDesktop(a, managerKey, "Managing desktop");
  const managerA = await open(a, managerKey);
  assert.ok(managerA.greeting);
  const managerB = await pairedAt(b, managerKey);
  const managerC = await pairedAt(c, managerKey);

  const launchKey = link.newKey();
  const desktop = { key: link.publicOf(launchKey).toString("base64url"), name: "Desktop", kind: "desktop" };
  const save = await b.route("POST", "kuvo/devices/pair", desktop, true);
  assert.equal(save.status, 200, JSON.stringify(save));
  assertReceipt(a, "membershipAndAccess", "committed", save.value);
  const deskA = await pairedAt(a, launchKey, { client: "desktop" });
  const deskB = await pairedAt(b, launchKey, { client: "desktop" });
  const deskC = await pairedAt(c, launchKey, { client: "desktop" });
  for (const device of [deskA, deskB, deskC, managerA, managerB, managerC]) {
    const roster = await device.ask("GET", "kuvo/computers");
    assert.equal(roster.status, 200);
    assert.deepEqual(new Set(roster.value.computers.map(peer => peer.key)), new Set([a.key, b.key, c.key]));
  }

  const renamed = await managerB.ask("PATCH", `kuvo/computers/${c.key}`, { name: "Shared name" });
  assert.equal(renamed.status, 200, JSON.stringify(renamed));
  assertReceipt(a, "setupNames", "committed", renamed.value);
  for (const device of [deskA, deskB, deskC]) {
    await byClient("the desktop reads the shared name", async () =>
      (await device.ask("GET", "kuvo/computers")).value.computers.find(peer => peer.key === c.key)?.name === "Shared name");
  }

  const reconnect = relay.disconnect(c.address);
  t.after(reconnect);
  await until("the third computer goes offline", () => !relay.has(c.address), t.signal);
  const revoked = await managerB.ask("POST", "kuvo/phones/remove", { phones: [{ key: desktop.key, pairing: save.value.pairing }] });
  assert.equal(revoked.status, 200, JSON.stringify(revoked));
  assertReceipt(a, "membershipAndAccess", "committed", revoked.value);
  assert.ok(c.read().phones.some(device => device.key === desktop.key));
  const unpause = relay.pause(a.address, c.address);
  t.after(unpause);
  reconnect();
  await until("the third computer reconnects with cached grants", () => relay.has(c.address), t.signal);
  assert.equal((await open(c, launchKey, { client: "desktop" })).waiting, "Checking this app with your other computers…");
  unpause();
  await until("the third computer installs the tombstone", () => c.read().removed.some(entry => entry.key === desktop.key), t.signal);
  for (const box of [a, b, c]) {
    assert.deepEqual(await open(box, launchKey, { client: "desktop" }), { refused: true });
    assert.ok(box.read().phones.some(device => device.key === managerPublic));
  }

  const removed = await managerB.ask("DELETE", `kuvo/computers/${c.key}`);
  assert.equal(removed.status, 200, JSON.stringify(removed));
  assertReceipt(a, "membershipAndAccess", "committed", removed.value);
  for (const device of [managerA, managerB]) {
    await byClient("the removed machine is absent from the roster", async () =>
      (await device.ask("GET", "kuvo/computers")).value.computers.every(peer => peer.key !== c.key));
    assert.equal((await device.ask("POST", `kuvo/computers/${c.key}/connect`)).status, 404);
  }
  await until("the removed computer received its unlink", () => !c.read().phones.some(device => device.key === managerPublic), t.signal);
  assert.deepEqual(await open(c, managerKey), { refused: true });
});
