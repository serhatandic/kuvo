import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

const bun = process.env.KUVO_TEST_BUN || "bun";
const url = name => JSON.stringify(new URL("../src/link/backup/" + name + ".ts", import.meta.url).href);
const setup = JSON.stringify(new URL("../src/link/documents/format.ts", import.meta.url).href);
const cryptoKeys = JSON.stringify(new URL("../src/link/crypto/keys.ts", import.meta.url).href);
const wake = new URL("../src/keeper/kuvo-wake", import.meta.url).pathname;
const source = "b_" + "a".repeat(22);
const key = Buffer.alloc(32, 7).toString("base64url");
const epoch = Date.UTC(2026, 10, 19);
const id = "2026-11-19T000000.000Z-daily-0123456789abcdef";

const scratch = () => {
  const folder = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-backup-bench-"));
  removeAtEnd(folder);
  return fs.realpathSync(folder);
};
const home = root => {
  const dir = path.join(root, ".kuvo-code");
  fs.mkdirSync(dir, { recursive: true });
  return dir;
};
const put = (dir, name, value) => {
  const at = path.join(dir, name);
  fs.mkdirSync(path.dirname(at), { recursive: true });
  fs.writeFileSync(at, value);
  return at;
};
function bunIn(root, code, env = {}) {
  const ran = spawnSync(bun, ["--config=/dev/null", "--no-env-file", "-e", code], {
    encoding: "utf8", timeout: 45_000,
    env: { ...process.env, HOME: root, KUVO_HOME: home(root), ...env },
  });
  assert.equal(ran.status, 0, ran.stderr || String(ran.error ?? ran.signal));
  return ran.stdout.trim();
}
const args = root => JSON.stringify({ dir: root, source, key, epoch, id });
const install = root => `
  const a = ${args(root)};
  const destination = { kind: "disk", path: a.dir };
`;

function take(owner, dest) {
  return bunIn(owner, `
    import fs from "node:fs";
    import path from "node:path";
    import { captureBackup, scratchFolder } from ${url("engine")};
    import { archiveContext, authenticate } from ${url("archive")};
    import { destinationStore } from ${url("destination")};
    ${install(dest)}
    const scratch = scratchFolder();
    try {
      const file = path.join(scratch, "archive");
      const manifest = captureBackup(file, a.key, a.source, a.id, a.epoch, [], scratch);
      const store = destinationStore(destination, a.source);
      await store.put(a.id, file);
      authenticate(await store.get(a.id, scratch), a.key, archiveContext(a.source, a.id));
      console.log(JSON.stringify(manifest));
    } finally { fs.rmSync(scratch, { recursive: true }); }
  `);
}

function recover(owner, dest, given = key) {
  return JSON.parse(bunIn(owner, `
    import { restore } from ${url("restore")};
    ${install(dest)}
    console.log(JSON.stringify(await restore(destination, a.source, a.id, ${JSON.stringify(given)})));
  `));
}

test("failure bench: restore verified data after the old disk is erased", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const box = home(owner);
  put(box, "folder-version.json", '{"version":6}');
  put(box, "bots/bot_a/MEMORY.md", "durable memory");
  put(box, "bots/bot_a/notes.md", "bot note");
  put(box, "groups/grp_a/messages.json", '["group history"]');
  put(box, "run/work-queue/queues/usr_you.json", '{"items":[{"id":"wqi_item","title":"owned item"}]}');
  put(box, "run/work-queue/files/wqi_item/file.bin", Buffer.of(0, 4, 255));
  put(box, "run/moves/decisions/mov_one.json", '{"decision":"switched"}');
  put(box, "attachments/ses_a/file.bin", Buffer.of(0, 64, 255));
  put(box, "artifacts/copies/aa/aabb", Buffer.of(20, 255));
  put(box, "artifacts/key", "private-artifact-key");
  put(box, "browser/profile/Cookies", "private-browser-state");
  put(box, "box/keys/GITHUB_TOKEN", "private-credential");
  put(box, "agents/sessions/ses_a/messages.json", '["conversation"]');
  put(box, "agents/usage/usage.json", '{"requests":12}');
  fs.mkdirSync(path.join(box, "data/opencode"), { recursive: true });
  bunIn(owner, `
    import { Database } from "bun:sqlite";
    const db = new Database(process.env.KUVO_HOME + "/data/opencode/opencode.db");
    db.exec("PRAGMA journal_mode = WAL; CREATE TABLE session(id TEXT PRIMARY KEY); CREATE TABLE credential(secret TEXT); CREATE TABLE session_inbox(message TEXT); INSERT INTO session VALUES ('ses_a'); INSERT INTO credential VALUES ('removed-secret-value'); INSERT INTO session_inbox VALUES ('would replay');");
    db.close();
  `);
  assert.equal(JSON.parse(take(owner, dest)).version, 6);
  fs.rmSync(box, { recursive: true });

  assert.throws(() => recover(replacement, dest, Buffer.alloc(32, 8).toString("base64url")), /authenticate/);
  const currentBox = home(replacement);
  assert.equal(fs.existsSync(path.join(currentBox, "bots")), false);
  const restored = recover(replacement, dest);
  assert.deepEqual(restored.unavailable, []);
  assert.equal(restored.savedAfterPoint, "unavailable unless verified elsewhere");
  assert.equal(restored.credentials, "sign-in required");
  for (const [file, value] of [
    ["bots/bot_a/MEMORY.md", "durable memory"], ["bots/bot_a/notes.md", "bot note"],
    ["groups/grp_a/messages.json", '["group history"]'], ["agents/sessions/ses_a/messages.json", '["conversation"]'],
    ["agents/usage/usage.json", '{"requests":12}'], ["run/moves/decisions/mov_one.json", '{"decision":"switched"}'],
  ]) assert.equal(fs.readFileSync(path.join(currentBox, file), "utf8"), value);
  assert.deepEqual(fs.readFileSync(path.join(currentBox, "attachments/ses_a/file.bin")), Buffer.of(0, 64, 255));
  assert.deepEqual(fs.readFileSync(path.join(currentBox, "run/work-queue/files/wqi_item/file.bin")), Buffer.of(0, 4, 255));
  assert.deepEqual(fs.readFileSync(path.join(currentBox, "artifacts/copies/aa/aabb")), Buffer.of(20, 255));
  for (const name of ["box/keys", "browser/profile", "artifacts/key", "run/link.json"]) assert.equal(fs.existsSync(path.join(currentBox, name)), false);
  assert.equal(fs.readFileSync(path.join(currentBox, "data/opencode/opencode.db")).includes("removed-secret-value"), false);
  assert.equal(bunIn(replacement, `
    import { Database } from "bun:sqlite";
    const db = new Database(process.env.KUVO_HOME + "/data/opencode/opencode.db");
    console.log(JSON.stringify(["session", "credential", "session_inbox"].map(name => db.query("SELECT count(*) AS count FROM " + name).get().count)));
  `), "[1,0,0]");
  assert.deepEqual(recover(replacement, dest), restored);
});

test("failure bench: restored wakes stay paused and pending work never replays", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const box = home(owner);
  put(box, "folder-version.json", '{"version":7}');
  put(box, "bots/bot_a/session", "ses_a");
  put(box, "bots/bot_a/wakes/1-abcd", "already-due");
  put(box, "bots/bot_a/watches/check/check", "echo should-not-run");
  put(box, "wakes/ses_b/session", "ses_b");
  put(box, "wakes/ses_b/wakes/1-efgh", "may-have-fired");
  put(box, "run/outbox/sending/letter.json", '{"external":"may already have acted"}');
  put(box, "run/queues.json", '{"ses_a":{"paused":false,"waiting":[{"id":"msg_a","text":"pending"}]}}');
  put(box, "agents/sessions/ses_a/agent.json", '{"inbox":[{"id":"msg_a","text":"pending"}],"threads":{}}');
  take(owner, dest);
  const result = recover(replacement, dest);
  const target = home(replacement);
  assert.equal(fs.readFileSync(path.join(target, "bots/bot_a/paused"), "utf8"), "restore-review");
  assert.equal(fs.readFileSync(path.join(target, "wakes/ses_b/paused"), "utf8"), "restore-review");
  assert.equal(fs.readFileSync(path.join(target, "bots/bot_a/wakes/1-abcd"), "utf8"), "already-due");
  assert.ok(result.held.includes("home/run/queues.json"));
  assert.equal(fs.existsSync(path.join(target, "run/queues.json")), false);
  assert.equal(fs.existsSync(path.join(target, "run/outbox/sending/letter.json")), false);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(target, "agents/sessions/ses_a/agent.json"), "utf8")), { threads: {} });
  assert.ok(result.held.includes("agents/sessions/ses_a/inbox.json"));
  const fired = path.join(replacement, "deliveries");
  const bin = path.join(replacement, "bin");
  const curl = put(bin, "curl", `#!/bin/sh
for argument; do url=$argument; done
case "$url" in
  */prompt) printf 'sent\n' >> "$BACKUP_TEST_SENT" ;;
  *) printf '{}\n200' ;;
esac
`);
  fs.chmodSync(curl, 0o700);
  const record = put(target, "run/engine.json", '{"port":23456,"password":"fixture"}');
  const fire = () => {
    const ran = spawnSync("sh", [wake, "fire", path.join(target, "bots"), record],
      { env: { ...process.env, PATH: bin + path.delimiter + process.env.PATH, HOME: replacement, KUVO_HOME: target, BACKUP_TEST_SENT: fired }, encoding: "utf8", timeout: 10_000 });
    assert.equal(ran.status, 0, ran.stderr);
  };
  fire();
  fire();
  assert.equal(fs.existsSync(fired), false);
  assert.equal(fs.readFileSync(path.join(target, "bots/bot_a/wakes/1-abcd"), "utf8"), "already-due");
  // The fixture can deliver. Only this reviewed schedule is released. A second round has no replay.
  fs.unlinkSync(path.join(target, "bots/bot_a/paused"));
  fire();
  fire();
  assert.equal(fs.readFileSync(fired, "utf8"), "sent\n");
  assert.equal(fs.existsSync(path.join(target, "bots/bot_a/wakes/1-abcd")), false);
});

test("failure bench: an update in progress cannot publish a mixed-layout backup", () => {
  const owner = scratch(), dest = scratch();
  const box = home(owner);
  put(box, "folder-version.json", '{"version":6,"upgradingTo":7}');
  assert.throws(() => take(owner, dest), /folder upgrade/);
  put(box, "folder-version.json", '{"version":7}');
  const trial = put(box, "run/update-trial", "test-release");
  assert.throws(() => take(owner, dest), /update to finish/);
  assert.deepEqual(fs.readdirSync(dest), []);
  fs.unlinkSync(trial);
  assert.equal(JSON.parse(take(owner, dest)).version, 7);
});

test("failure bench: keep 30 daily points and expire older pre-operation points", () => {
  const owner = scratch(), dest = scratch();
  bunIn(owner, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { pointId } from ${url("engine")};
    import { destinationStore, expiredPoints } from ${url("destination")};
    ${install(dest)}
    const store = destinationStore(destination, a.source);
    const dummy = path.join(process.env.HOME, "test-archive");
    fs.writeFileSync(dummy, "fixture");
    const aDay = 86_400_000;
    for (let age = 34; age >= 0; age--) await store.put(pointId("daily", a.epoch - age * aDay), dummy);
    await store.put(pointId("daily", a.epoch + 500), dummy);
    await store.put(pointId("update", a.epoch - 31 * aDay), dummy);
    await store.put(pointId("move", a.epoch), dummy);
    await store.put(pointId("vault", a.epoch - 31 * aDay), dummy);
    for (const id of expiredPoints(await store.list(), a.epoch + 500)) await store.erase(id);
    const points = await store.list();
    assert.equal(points.filter(id => id.includes("-daily-")).length, 30);
    assert.equal(points.filter(id => id.includes("-update-")).length, 0);
    assert.equal(points.filter(id => id.includes("-move-")).length, 1);
    assert.equal(points.filter(id => id.includes("-vault-")).length, 1);
  `);
});

test("SSH deposits ciphertext on the chosen member and verifies the readback", () => {
  const owner = scratch(), target = scratch();
  const destination = home(target);
  const ssh = put(owner, "fake-ssh", '#!/bin/sh\nfor argument; do command=$argument; done\nHOME="$BACKUP_TEST_REMOTE" exec sh -c "$command"\n');
  fs.chmodSync(ssh, 0o700);
  const peerKey = bunIn(owner, `
    import { publicOf } from ${cryptoKeys};
    console.log(publicOf(Buffer.alloc(32, 47)).toString("base64url"));
  `);
  put(destination, "bin/kuvo-start", "V=fixture\n");
  const runtime = put(destination, "engine/fixture/opencode", `#!/bin/sh\nprintf '%s\\n' '${peerKey}'\n`);
  fs.chmodSync(runtime, 0o700);
  put(home(owner), "bots/bot_a/MEMORY.md", "fixture plaintext only on owner");
  bunIn(owner, `
    import assert from "node:assert/strict";
    import { newSigningSeed } from ${setup};
    import { publicOf } from ${cryptoKeys};
    import { backupBefore, backupStatus, configure } from ${url("engine")};
    import { writeLink } from ${JSON.stringify(new URL("../src/link/state.ts", import.meta.url).href)};
    const key = Buffer.alloc(32, 42).toString("base64url");
    const own = publicOf(Buffer.from(key, "base64url")).toString("base64url");
    const peer = ${JSON.stringify(peerKey)};
    writeLink({ key, sign: newSigningSeed(), setup: { id: "set_fixture", primary: own, role: "primary", epoch: 1 },
      macPeers: [{ key: peer, name: "Receiving computer", kind: "machine", address: "A".repeat(22) }] });
    const chosen = { kind: "computer", computer: peer, ssh: "fixture" };
    const recovery = await configure(chosen, []);
    const first = await backupBefore("daily");
    assert.deepEqual(backupStatus().lastVerifiedRestorePoint, first);
    await backupBefore("wipe");
    assert.deepEqual(await backupBefore("daily"), first);
    assert.equal(recovery.recoveryKey.length, 43);
  `, { KUVO_TEST_SSH: ssh, BACKUP_TEST_REMOTE: target });
  const saved = path.join(target, ".kuvo-backups");
  const [namespace] = fs.readdirSync(saved);
  const files = fs.readdirSync(path.join(saved, namespace));
  assert.equal(files.length, 2);
  for (const file of files) {
    const ciphertext = fs.readFileSync(path.join(saved, namespace, file));
    assert.equal(ciphertext.includes("fixture plaintext only on owner"), false);
  }
});

test("the live primary matches a fresh key and moves the old ownership address", () => {
  const owner = scratch();
  bunIn(owner, `
    import assert from "node:assert/strict";
    import { publicOf } from ${cryptoKeys};
    import { newSigningSeed, signingKeyOf } from ${setup};
    import { matchReplacement, restoreForPeer } from ${url("restore")};
    import { writeLink, readLink } from ${JSON.stringify(new URL("../src/link/state.ts", import.meta.url).href)};
    import { ownerOf } from ${JSON.stringify(new URL("../src/link/ownership/resolve.ts", import.meta.url).href)};
    const key = Buffer.alloc(32, 42).toString("base64url");
    const keyOf = n => publicOf(Buffer.alloc(32, n)).toString("base64url");
    const own = keyOf(42), old = keyOf(43), replacement = keyOf(44), other = keyOf(45);
    const peer = key => ({ key, kind: "machine", name: key, address: "A".repeat(22) });
    const sign = newSigningSeed();
    const id = "set_" + "A".repeat(22);
    writeLink({ key, sign, setup: { id, primary: own, sign: signingKeyOf(sign), role: "primary", managed: true, checkpointed: true, epoch: 1 },
      sharedSetup: id, shared: {
        membershipAndAccess: { sequence: 1, value: { left: [old], boxes: [own, replacement, other].map(peer) } },
        ownershipAssignments: { sequence: 1, value: { "session computer:ses_a": { computer: old } } },
      } });
    const source = ${JSON.stringify(source)};
    const act = { body: { source, origin: old, setup: id } };
    assert.equal(restoreForPeer(peer(replacement), act).status, 409);
    await assert.rejects(matchReplacement(source, old, old), /new membership/);
    assert.equal((await matchReplacement(source, old, replacement)).status, "committed");
    assert.equal(restoreForPeer(peer(replacement), act).status, 200);
    assert.equal(restoreForPeer(peer(other), act).status, 409);
    assert.equal(restoreForPeer(peer(old), act).status, 409);
    assert.equal(ownerOf({ kind: "conversations", id: "ses_a" }, { link: readLink() }).key, replacement);
  `);
});

test("vault export needs its separate key and imports without inherited grants", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const exported = JSON.parse(bunIn(owner, `
    import { makeVault, addItem } from ${JSON.stringify(new URL("../src/link/vault/store.ts", import.meta.url).href)};
    import { exportVault } from ${url("vault")};
    ${install(dest)}
    makeVault();
    await addItem({ kind: "login", name: "Fixture login", sites: ["example.com"], values: { password: "test-secret-849" } });
    console.log(JSON.stringify(await exportVault(destination, a.source)));
  `));
  assert.equal(exported.recoveryKey.length, 43);
  assert.notEqual(exported.recoveryKey, key);
  const file = path.join(dest, "kuvo-backups", source, exported.id + ".kuvo");
  assert.equal(fs.readFileSync(file).includes("test-secret-849"), false);
  bunIn(replacement, `
    import assert from "node:assert/strict";
    import { makeVault, readGrants, readItems, valuesOf } from ${JSON.stringify(new URL("../src/link/vault/store.ts", import.meta.url).href)};
    import { importVault } from ${url("vault")};
    ${install(dest)}
    const point = ${JSON.stringify(exported)};
    makeVault();
    await assert.rejects(importVault(destination, a.source, point.id, a.key), /authenticate/);
    assert.equal(readItems().length, 0);
    assert.equal((await importVault(destination, a.source, point.id, point.recoveryKey)).imported, 1);
    assert.equal((await valuesOf(readItems()[0].id)).password, "test-secret-849");
    assert.deepEqual(readGrants(), []);
  `);
});

test("the built box previews a destination before enabling and requires the local app", () => {
  const owner = scratch(), dest = scratch();
  const link = JSON.stringify(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url).href);
  bunIn(owner, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import { serveBox } from ${link};
    const server = serveBox("fixture");
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
    const url = "http://127.0.0.1:" + server.address().port + "/kuvo/box/backups";
    const headers = { authorization: "Basic " + Buffer.from("opencode:fixture").toString("base64") };
    try {
      assert.equal((await fetch(url, { headers })).status, 403);
      const local = { ...headers, "x-kuvo-from": "app", "content-type": "application/json" };
      const status = await fetch(url, { headers: local });
      assert.equal(status.status, 200);
      assert.equal(status.headers.get("cache-control"), "no-store");
      assert.equal((await status.json()).restorePointStatus, "no separate backup");
      const destination = { kind: "disk", path: ${JSON.stringify(dest)} };
      const choose = confirm => fetch(url + "/destination", { method: "POST", headers: local,
        body: JSON.stringify({ destination, confirm }) });
      const preview = await choose(false);
      assert.deepEqual((await preview.json()).destination, destination);
      assert.equal(fs.existsSync(process.env.KUVO_HOME + "/backup/configuration.json"), false);
      assert.equal((await choose(true)).status, 409); // Same volume as the box.
      assert.equal((await (await fetch(url, { headers: local })).json()).enabled, false);
    } finally { server.closeAllConnections(); server.close(); }
  `);
});

test("accepted moves fence the same old copy during normal writes and restore", () => {
  bunIn(scratch(), `
    import assert from "node:assert/strict";
    import { publicOf } from ${cryptoKeys};
    import { newSigningSeed, signingKeyOf } from ${setup};
    import { writeLink, readLink } from ${JSON.stringify(new URL("../src/link/state.ts", import.meta.url).href)};
    import * as ownership from ${JSON.stringify(new URL("../src/link/ownership/resolve.ts", import.meta.url).href)};
    import { noteMovedAway } from ${JSON.stringify(new URL("../src/link/moves/records.ts", import.meta.url).href)};
    const seed = Buffer.alloc(32, 42).toString("base64url");
    const keyOf = n => publicOf(Buffer.alloc(32, n)).toString("base64url");
    const own = keyOf(42), old = keyOf(43), target = keyOf(44);
    const sign = newSigningSeed(), id = "set_" + "A".repeat(22);
    const peer = key => ({ key, kind: "machine", name: key, address: "A".repeat(22) });
    writeLink({ key: seed, sign, setup: { id, primary: own, sign: signingKeyOf(sign), role: "primary", managed: true, checkpointed: true, epoch: 1 },
      sharedSetup: id, shared: {
        membershipAndAccess: { sequence: 1, value: { left: [old], boxes: [own, target].map(peer), moved: [{ session: "ses_a", key: target }] } },
        ownershipAssignments: { sequence: 1, value: { ["replacement:" + old]: { computer: own } } },
      } });
    noteMovedAway(["ses_a"], own);
    const link = readLink(), ref = { kind: "conversations", id: "ses_a" };
    assert.equal(ownership.ownerOf(ref, { link, localRecord: true }).key, target);
    assert.equal(ownership.mayActivate(ref, { link }), false);
    assert.equal(ownership.mayActivate(ref, { link, origin: old }), false);
    assert.deepEqual(ownership.sessionsFencedHere({ link, origin: old }), ["ses_a"]);
    assert.equal(ownership.mayActivate({ ...ref, id: "ses_b" }, { link, origin: old }), true);
  `);
});

test("a restored queue item with a newer departure is held with its files", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const box = home(owner), target = home(replacement), item = "wqi_" + "A".repeat(20);
  put(box, "folder-version.json", '{"version":7}');
  put(target, "folder-version.json", '{"version":7}');
  put(box, "run/work-queue/queues/usr_you.json", JSON.stringify({ owner: "usr_you", items: [{ id: item, title: "At the backup point" }] }));
  put(box, `run/work-queue/files/${item}/file.bin`, "keep with held item");
  take(owner, dest);
  bunIn(replacement, `
    import { noteDeparture } from ${JSON.stringify(new URL("../src/link/work-queue/store.ts", import.meta.url).href)};
    noteDeparture(${JSON.stringify(item)}, { queue: "ses_elsewhere", computer: "A".repeat(43) });
  `);
  const result = recover(replacement, dest);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(target, "run/work-queue/queues/usr_you.json"), "utf8")).items, []);
  const held = result.held.find(entry => entry.startsWith("work-queue-items/"));
  assert.ok(held);
  assert.equal(JSON.parse(fs.readFileSync(path.join(target, "backup/recovered", id, held), "utf8")).id, item);
  const file = `run/work-queue/files/${item}/file.bin`;
  assert.equal(fs.existsSync(path.join(target, file)), false);
  assert.equal(fs.readFileSync(path.join(target, "backup/recovered", id, "home", file), "utf8"), "keep with held item");
});

test("restore inventories the upgraded home and external roots", () => {
  bunIn(scratch(), `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { upgradeFolder } from ${JSON.stringify(new URL("../src/shared/folder-version.ts", import.meta.url).href)};
    import { stageRestore } from ${url("stage")};
    const stage = path.join(process.env.HOME, "staged"), legacy = path.join(process.env.HOME, "legacy");
    fs.writeFileSync(legacy, "legacy");
    const upgraded = stageRestore(stage, 0, [
      { name: "home/run/named.json", file: legacy },
      { name: "bots/bot_a/notes.md", file: legacy },
    ], folder => upgradeFolder(folder, [at => {
      fs.renameSync(path.join(at, "run/named.json"), path.join(at, "run/archived.json"));
      const notes = path.join(at, "bots/bot_a");
      fs.renameSync(path.join(notes, "notes.md"), path.join(notes, "MEMORY.md"));
    }]));
    assert.deepEqual([...upgraded.keys()].sort(), ["bots/bot_a/MEMORY.md", "home/run/archived.json"]);
    for (const file of upgraded.values()) assert.equal(fs.readFileSync(file, "utf8"), "legacy");
  `);
});

test("a paired backup cannot reactivate an item handed off after that backup", () => {
  const old = scratch(), primary = scratch(), replacement = scratch(), dest = scratch();
  const oldBox = home(old), item = "wqi_" + "B".repeat(20);
  put(oldBox, "folder-version.json", '{"version":7}');
  put(oldBox, "run/work-queue/queues/usr_you.json", JSON.stringify({ owner: "usr_you", items: [{ id: item, title: "Before handoff" }] }));
  const authority = `
    import { publicOf } from ${cryptoKeys};
    import { signingKeyOf } from ${setup};
    import { writeLink } from ${JSON.stringify(new URL("../src/link/state.ts", import.meta.url).href)};
    const seed = n => Buffer.alloc(32, n).toString("base64url");
    const keyOf = n => publicOf(Buffer.alloc(32, n)).toString("base64url");
    const primary = keyOf(42), old = keyOf(43), replacement = keyOf(44), recipient = keyOf(45);
    const peer = key => ({ key, kind: "machine", name: key, address: "A".repeat(22) });
    const id = "set_" + "A".repeat(22), source = ${JSON.stringify(source)};
    const setup = { id, primary, sign: signingKeyOf(seed(46)), managed: true, checkpointed: true, epoch: 1 };
    const state = (n, after) => ({
      key: seed(n), sign: seed(n + 4), setup: { ...setup, role: n === 42 ? "primary" : "member" }, sharedSetup: id,
      shared: {
        membershipAndAccess: { sequence: after ? 2 : 1, value: {
          left: after ? [old] : [], boxes: [primary, after ? replacement : old, recipient].map(peer),
        } },
        ...(after ? { ownershipAssignments: { sequence: 1, value: { ["replacement:" + old]: { computer: replacement, source } } } } : {}),
      },
    });
  `;
  bunIn(old, `${authority} writeLink(state(43, false));`);
  assert.equal(JSON.parse(take(old, dest)).paired, true);
  // The source learned about the handoff after the backup. Its local fence
  // is deliberately unavailable to the freshly enrolled replacement.
  const record = n => JSON.stringify(new URL("../src/link/" + n + ".ts", import.meta.url).href);
  bunIn(old, `${authority}
    import { noteDeparture } from ${record("work-queue/store")};
    noteDeparture(${JSON.stringify(item)}, { queue: "ses_recipient", computer: recipient });
  `);
  fs.rmSync(oldBox, { recursive: true });
  const approved = JSON.parse(bunIn(primary, `${authority}
    import { restoreForPeer } from ${url("restore")};
    writeLink(state(42, true));
    console.log(JSON.stringify(restoreForPeer(peer(replacement), { body: { source, origin: old, setup: id } })));
  `));
  assert.equal(approved.status, 200);
  const result = JSON.parse(bunIn(replacement, `
    import fs from "node:fs";
    import path from "node:path";
    import { restore } from ${url("restore")};
    import { FORWARDS, forwardsSoon, takeActed } from ${record("work-queue/forward")};
    ${authority}
    ${install(dest)}
    writeLink(state(44, true));
    const run = path.join(process.env.KUVO_HOME, "run");
    fs.writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify({ [primary]: { features: ["data.ownership"], heard: Date.now(), list: [] } }));
    forwardsSoon.send = () => {
      const [file] = fs.readdirSync(FORWARDS);
      const { act } = JSON.parse(fs.readFileSync(path.join(FORWARDS, file), "utf8"));
      if (act.item !== "backup-restore") throw new Error("Unexpected owner request");
      takeActed(peer(primary), { id: act.id, ...${JSON.stringify(approved)} });
    };
    console.log(JSON.stringify(await restore(destination, a.source, a.id, a.key)));
  `));
  assert.ok(result.held.some(entry => entry.startsWith("work-queue-items/")));
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(home(replacement), "run/work-queue/queues/usr_you.json"), "utf8")).items, []);
});
