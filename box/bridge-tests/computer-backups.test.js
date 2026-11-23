import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
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
const archiveMagic = Buffer.from("KUVO backup 1\n");
// A main renews its relay lease every 10 s; a main without one pauses setup-wide changes.
const leaseHelper = JSON.stringify(new URL("../test/relay-lease.ts", import.meta.url).href);

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
  put(box, "bots/bot_a/groups/grp_a.md", "private group memory");
  put(box, "box/approval.json", '{"fullAccess":true,"last":"acceptEdits","since":123}');
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
  const archived = fs.readFileSync(path.join(dest, "kuvo-backups", source, id + ".kuvo"));
  assert.deepEqual(archived.subarray(0, archiveMagic.length), archiveMagic);
  fs.rmSync(box, { recursive: true });

  assert.throws(() => recover(replacement, dest, Buffer.alloc(32, 8).toString("base64url")), /authenticate/);
  const currentBox = home(replacement);
  assert.equal(fs.existsSync(path.join(currentBox, "bots")), false);
  bunIn(replacement, `
    import fs from "node:fs";
    import { Database } from "bun:sqlite";
    const folder = process.env.KUVO_HOME + "/data/opencode";
    fs.mkdirSync(folder, { recursive: true });
    const db = new Database(folder + "/opencode.db");
    db.exec("PRAGMA journal_mode=WAL; CREATE TABLE session(id TEXT PRIMARY KEY); CREATE TABLE migration(id TEXT); INSERT INTO migration VALUES ('bootstrap');");
    db.close();
  `);
  const restored = recover(replacement, dest);
  assert.deepEqual(restored.unavailable, []);
  assert.equal(restored.savedAfterPoint, "unavailable unless verified elsewhere");
  assert.equal(restored.credentials, "sign-in required");
  for (const [file, value] of [
    ["bots/bot_a/MEMORY.md", "durable memory"], ["bots/bot_a/notes.md", "bot note"],
    ["bots/bot_a/groups/grp_a.md", "private group memory"],
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
  assert.equal(bunIn(replacement, `
    import { fullAccessAllowed, defaultApproval } from ${JSON.stringify(new URL("../src/link/work/approval.ts", import.meta.url).href)};
    console.log(JSON.stringify([fullAccessAllowed(), defaultApproval()]));
  `), '[false,"acceptEdits"]');
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
while [ "$#" -gt 0 ]; do
  if [ "$1" = -D ]; then
    printf 'HTTP/1.1 200 OK\\r\\nX-Kuvo-Route-Version: 1\\r\\n\\r\\n' > "$2"
    shift
  else
    url=$1
  fi
  shift
done
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
    assert.deepEqual(ciphertext.subarray(0, archiveMagic.length), archiveMagic);
    assert.equal(ciphertext.includes("fixture plaintext only on owner"), false);
  }
});

test("Relay-only backup survives source loss and restores through accepted replacement membership", () => {
  const owner = scratch(), receiving = scratch(), replacement = scratch(), unrelated = scratch();
  const ownerBox = home(owner), receivingBox = home(receiving);
  const secret = "private payload only on the original box";
  const bytes = Buffer.concat([Buffer.from(secret), Buffer.alloc(300_000, 19)]);
  put(ownerBox, "folder-version.json", '{"version":7}');
  put(home(replacement), "folder-version.json", '{"version":7}');
  put(ownerBox, "bots/bot_a/MEMORY.md", bytes);
  const receivingStub = new URL("./relay-backup-fixture.ts", import.meta.url);
  const linkURL = name => JSON.stringify(new URL("../src/link/" + name + ".ts", import.meta.url).href);
  const stateCode = `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { spawn, spawnSync } from "node:child_process";
    import crypto from "node:crypto";
    import { createInterface } from "node:readline";
    import { publicOf } from ${cryptoKeys};
    import { signingKeyOf } from ${setup};
    import { readLink, writeLink } from ${linkURL("state")};
    import { mailFor } from ${linkURL("peers/known")};
    import { deliver } from ${JSON.stringify(receivingStub.href)};
    import { grantRelayLease } from ${leaseHelper};
    import { forwardAct, forwardsSoon, sendForwards } from ${linkURL("work-queue/forward")};
    const seed = n => Buffer.alloc(32, n).toString("base64url");
    const keyOf = n => publicOf(Buffer.alloc(32, n)).toString("base64url");
    const peer = n => ({ key: keyOf(n), kind: "machine", name: "Fixture " + n, address: Buffer.alloc(16, n).toString("base64url") });
    const source = peer(43), target = peer(42), replacement = peer(44), unrelated = peer(45);
    const setup = { id: "set_" + "Z".repeat(22), primary: target.key, sign: signingKeyOf(seed(46)),
      managed: true, checkpointed: true, epoch: 1 };
    const state = (n, sourceId, setupId = setup.id) => ({
      key: seed(n), sign: seed(n + 4), setup: { ...setup, id: setupId, role: n === 42 ? "primary" : "member" },
      sharedSetup: setupId, relay: "ws://127.0.0.1:1", secret: seed(n + 9),
      shared: {
        membershipAndAccess: { sequence: sourceId ? 2 : 1, value: { boxes: [target, sourceId ? replacement : source, unrelated],
          left: sourceId ? [source.key] : [] } },
        ...(sourceId ? { ownershipAssignments: { sequence: 1, value: { ["replacement:" + source.key]: { computer: replacement.key, source: sourceId }, "session computer:ses_archive": { computer: target.key } } } } : {}),
      },
    });
    const receiver = ${JSON.stringify(receiving)};
    const configPeer = (n, sourceId, setupId) => {
      writeLink(state(n, sourceId, setupId));
      if (n === 42) grantRelayLease(readLink());
      const run = path.join(process.env.KUVO_HOME, "run");
      fs.writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify({ [target.key]: { features: ["box.backups.relay", "data.ownership"], heard: Date.now(), list: [] } }));
    };
    let relay;
    const responses = [];
    function send(frames) {
      if (!relay) {
        relay = spawn(process.execPath, ["--config=/dev/null", "--no-env-file", ${JSON.stringify(receivingStub.pathname)}], {
          env: { ...process.env, HOME: receiver, KUVO_HOME: path.join(receiver, ".kuvo-code") },
          stdio: ["pipe", "pipe", "inherit"],
        });
        createInterface({ input: relay.stdout }).on("line", line => responses.shift().resolve(JSON.parse(line)));
        relay.on("error", error => { for (const waiter of responses.splice(0)) waiter.reject(error); });
        relay.on("exit", code => {
          relay = undefined;
          for (const waiter of responses.splice(0)) waiter.reject(new Error("Fixture relay exited " + code));
        });
      }
      return new Promise((resolve, reject) => {
        responses.push({ resolve, reject });
        relay.stdin.write(JSON.stringify(frames) + String.fromCharCode(10));
      });
    }
    const closeRelay = () => relay?.stdin.end();
    let delegateNext = false;
    let onReply = deliver;
    forwardsSoon.send = () => {
      let frames = [];
      if (delegateNext) {
        delegateNext = false;
        const result = spawnSync(process.execPath, ["--config=/dev/null", "--no-env-file",
          ${JSON.stringify(receivingStub.pathname)}, "--emit-mac-spool"], { env: process.env, encoding: "utf8" });
        assert.equal(result.status, 0, result.stderr);
        frames = JSON.parse(result.stdout);
        assert.ok(frames.length, "the Mac link sends the authenticated spool from a different process");
      } else {
        sendForwards({ welcomed: true, send: bytes => { frames.push(bytes.toString("base64url")); } }, readLink(), Date.now(), { ownOnly: true });
      }
      if (frames.length) void send(frames).then(onReply).catch(error => { console.error(error); process.exit(1); });
    };
    const destination = { kind: "computer", computer: target.key };
  `;
  bunIn(receiving, `${stateCode} configPeer(42);`);
  const saved = JSON.parse(bunIn(owner, `
    ${stateCode}
    import { backupBefore, backupStatus, configure } from ${url("engine")};
    import { destinationOf } from ${url("destination")};
    import { runMutation } from ${linkURL("ownership/mutation")};
    import { draftKey } from ${linkURL("ownership/forward-evidence")};
    import { keepRecord, recordKey } from ${linkURL("ownership/store")};
    import { submissionIntent, submissionKey } from ${linkURL("ownership/submissions")};
    import { noteMovedAway } from ${linkURL("moves/records")};
    configPeer(43);
    // The source still owns its historical decision after the conversation moves. Its move journal
    // says the conversation went to the target.
    noteMovedAway(["ses_archive"], target.key);
    const submission = setup.id + "." + source.key + ".msg_archived";
    const draft = { id: "msg_archived", submission, text: "!example", message: {} };
    const submitter = { kind: "ssh", key: target.key };
    await runMutation(submissionKey(setup.id, "ses_archive", submission), submissionIntent(draft, submitter, target.key),
      async () => ({ status: 200, value: { answer: { shell: "example" } } }), undefined, undefined,
      { setup: setup.id, session: "ses_archive", id: submission, origin: source.key, submitter, submittedThrough: target.key, draft },
      draftKey(setup.id, "ses_archive", draft.id));
    keepRecord("mutations", recordKey("legacy-opaque"), { digest: "opaque evidence" });
    assert.deepEqual(destinationOf(destination), destination);
    assert.deepEqual(backupStatus().computers, [{ key: target.key, name: target.name, relay: true }]);
    delegateNext = true;
    const recovery = await configure(destination, []);
    const spool = ${JSON.stringify("wqa_" + "S".repeat(20))};
    const spoolDir = path.join(process.env.KUVO_HOME, "run", "work-queue", "forward");
    fs.mkdirSync(spoolDir, { recursive: true });
    fs.writeFileSync(path.join(spoolDir, spool + ".json"), JSON.stringify({ to: target.key, act: {
      area: "backup", id: spool, method: "GET", item: recovery.source, body: { action: "list" }, at: Date.now(),
    } }));
    const frames = [];
    sendForwards({ welcomed: true, send: bytes => { frames.push(bytes.toString("base64url")); } }, readLink(), Date.now(), { ownOnly: true });
    assert.deepEqual(frames, [], "an unsigned spool cannot borrow the Mac’s peer identity");
    fs.unlinkSync(path.join(spoolDir, spool + ".json"));
    const point = await backupBefore("daily");
    assert.deepEqual(backupStatus().lastVerifiedRestorePoint, point);
    console.log(JSON.stringify({ ...recovery, point, setup: setup.id, owner: source.key }));
    closeRelay();
  `));
  const deposit = path.join(receiving, ".kuvo-backups", "relay", saved.setup, saved.owner, saved.source, saved.point.id + ".kuvo");
  const ciphertext = fs.readFileSync(deposit);
  assert.deepEqual(ciphertext.subarray(0, archiveMagic.length), archiveMagic);
  assert.equal(ciphertext.includes(secret), false);
  assert.equal(ciphertext.includes(saved.recoveryKey), false);
  assert.equal(fs.existsSync(path.join(receivingBox, "backup/configuration.json")), false);
  bunIn(owner, `
    ${stateCode}
    import { destinationStore } from ${url("destination")};
    const saved = ${JSON.stringify(saved)};
    const store = destinationStore(destination, saved.source);
    const folder = path.dirname(${JSON.stringify(deposit)});
    const oversized = path.join(folder, "2026-11-18T000000.000Z-daily-aaaaaaaaaaaaaaaa.kuvo");
    fs.closeSync(fs.openSync(oversized, "wx", 0o600));
    fs.truncateSync(oversized, 32 * 1024 ** 3);
    const spec = { id: ${JSON.stringify(id)}, size: 512, sha256: "a".repeat(64) };
    const ask = body => forwardAct(target, "Encrypted backup", { area: "backup", method: "POST",
      item: saved.source, body: { setup: setup.id, ...body } }, undefined, readLink(), Date.now(), { backgroundRelay: true });
    const begin = () => ask({ ...spec, action: "begin" });
    const full = await begin();
    assert.equal(full.status, 409);
    assert.match(full.value.message, /capacity/);
    fs.unlinkSync(oversized);
    assert.equal((await begin()).status, 200);
    assert.equal((await ask({ ...spec, action: "append", offset: 0, data: Buffer.alloc(128).toString("base64url") })).status, 200);
    assert.equal((await ask({ ...spec, action: "commit" })).status, 409);
    assert.deepEqual(await store.list(), [saved.point.id]);
    const ownerFolder = path.dirname(folder);
    for (const leaf of [".upload.json", ".upload.partial"]) fs.utimesSync(path.join(ownerFolder, leaf), new Date(0), new Date(0));
    const next = { ...spec, id: "2026-11-18T000000.000Z-daily-bbbbbbbbbbbbbbbb" };
    assert.equal((await ask({ ...next, action: "begin" })).status, 200);
    const journal = path.join(ownerFolder, ".upload.json");
    fs.writeFileSync(journal, "{");
    assert.equal((await begin()).status, 409, "a corrupt active reservation is kept");
    fs.utimesSync(journal, new Date(0), new Date(0));
    fs.utimesSync(path.join(ownerFolder, ".upload.partial"), new Date(0), new Date(0));
    assert.equal((await begin()).status, 200, "a stale corrupt reservation is disposable");
    assert.equal((await ask({ ...spec, action: "abort" })).status, 200);
    assert.equal(fs.existsSync(path.join(ownerFolder, ".upload.partial")), false);
    assert.deepEqual(await store.list(), [saved.point.id]);
    closeRelay();
  `);
  const uploadFolder = path.dirname(path.dirname(deposit));
  const journal = path.join(uploadFolder, ".upload.json");
  const partial = path.join(uploadFolder, ".upload.partial");
  const pointFile = point => path.join(path.dirname(deposit), point + ".kuvo");
  const erasePoint = "2026-11-19T000000.000Z-daily-0123456789abcdee";
  const transfer = `
    import { seal, authenticate, archiveContext } from ${url("archive")};
    import { destinationStore } from ${url("destination")};
    const saved = ${JSON.stringify(saved)};
    const journal = ${JSON.stringify(journal)}, partial = ${JSON.stringify(partial)};
    const store = destinationStore(destination, saved.source);
    const specOf = (id, file) => ({
      id, size: fs.statSync(file).size, sha256: crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex"),
    });
    const request = (action, spec, extra = {}) => ({
      area: "backup", method: "POST", item: saved.source, body: { setup: setup.id, action, ...spec, ...extra },
    });
    const ask = (action, spec, extra) => forwardAct(target, "Encrypted backup",
      request(action, spec, extra), undefined, readLink(), Date.now(), { backgroundRelay: true });
    const archiveFor = id => path.join(process.env.HOME, id + ".archive");
    const waitForReceiver = async () => {
      for (let attempt = 0; ; attempt++) {
        try { return await store.list(); }
        catch (error) {
          if (attempt === 4 || !error.message.includes("already running")) throw error;
        }
      }
    };
  `;
  bunIn(owner, `
    ${stateCode}
    ${transfer}
    const id = ${JSON.stringify(id)};
    const archive = archiveFor(id);
    seal(archive, saved.recoveryKey, [{ name: "private", bytes: Buffer.from("encrypted fixture") }], archiveContext(saved.source, id));
    const spec = specOf(id, archive);
    // Write the prior journal shape with different key order and spacing.
    fs.writeFileSync(journal, JSON.stringify({ sha256: spec.sha256, size: spec.size, id, source: saved.source }, null, 2), { mode: 0o600 });
    fs.writeFileSync(partial, fs.readFileSync(archive).subarray(0, 12), { mode: 0o600 });
  `);
  fs.rmSync(receivingBox, { recursive: true });
  assert.equal(fs.existsSync(journal), true, "Box removal preserves the receiver reservation");
  assert.equal(fs.existsSync(deposit), true);
  bunIn(receiving, `${stateCode} configPeer(42);`);
  bunIn(owner, `
    ${stateCode}
    ${transfer}
    const id = ${JSON.stringify(id)};
    const archive = archiveFor(id);
    const spec = specOf(id, archive);
    const legacy = JSON.parse(fs.readFileSync(journal, "utf8"));
    fs.writeFileSync(journal, JSON.stringify({ ...legacy, formatVersion: 99 }));
    fs.utimesSync(journal, new Date(0), new Date(0));
    fs.utimesSync(partial, new Date(0), new Date(0));
    const future = fs.readFileSync(journal, "utf8");
    assert.match((await ask("begin", spec)).value.message, /format version 99 is unsupported/);
    assert.equal(fs.readFileSync(journal, "utf8"), future);
    fs.writeFileSync(journal, JSON.stringify(legacy, null, 2));
    const different = { ...spec, sha256: "0".repeat(64) };
    assert.equal((await ask("begin", different)).status, 409);
    assert.equal(JSON.parse(fs.readFileSync(journal, "utf8")).formatVersion, undefined);
    const began = await ask("begin", spec);
    assert.equal(began.status, 200);
    assert.equal(began.value.offset, 12);
    const migrated = JSON.parse(fs.readFileSync(journal, "utf8"));
    assert.deepEqual(migrated, { ...legacy, formatVersion: 1 });
    assert.equal(fs.readFileSync(journal, "utf8").includes(saved.recoveryKey), false);
    const chunk = { offset: 12, data: fs.readFileSync(archive).subarray(12).toString("base64url") };
    for (let attempt = 0; attempt < 2; attempt++) assert.deepEqual((await ask("append", spec, chunk)).value, { offset: spec.size });
    assert.equal((await ask("append", spec, { offset: 12, data: Buffer.from("different").toString("base64url") })).status, 409);
    closeRelay();
  `);
  const cutCommit = point => bunIn(owner, `
    ${stateCode}
    ${transfer}
    const id = ${JSON.stringify(point)};
    const spec = specOf(id, archiveFor(id));
    const mail = mailFor(target, { workQueue: { act: {
      ...request("commit", spec), id: "wqa_" + "c".repeat(20), at: Date.now(),
    } } }, readLink());
    await assert.rejects(send([mail.toString("base64url")]), /exited 93/);
    assert.equal(fs.statSync(partial).ino, fs.statSync(${JSON.stringify(pointFile(point))}).ino);
    assert.equal(fs.existsSync(journal), true);
  `, { KUVO_TEST_CUT_BACKUP_PUBLISH: point });
  cutCommit(id);
  bunIn(owner, `
    ${stateCode}
    ${transfer}
    const id = ${JSON.stringify(id)};
    const replay = specOf(id, archiveFor(id));
    await waitForReceiver();
    let lost = 0;
    onReply = async replies => {
      assert.equal(replies.length, 1);
      lost++;
      onReply = deliver;
      const frames = [];
      sendForwards({ welcomed: true, send: bytes => frames.push(bytes.toString("base64url")) }, readLink(), Date.now() + 5_000, { ownOnly: true });
      assert.ok(frames.length, "retry the same authenticated request ID");
      return deliver(await send(frames));
    };
    assert.equal((await ask("commit", replay)).status, 200);
    assert.equal(lost, 1);
    assert.equal(fs.existsSync(journal), false);
    assert.equal(fs.existsSync(partial), false);
    assert.equal((await ask("commit", { ...replay, sha256: "0".repeat(64) })).status, 409);
    assert.deepEqual(await store.list(), [id, saved.point.id].sort().reverse());
    authenticate(await store.get(id, process.env.HOME), saved.recoveryKey, archiveContext(saved.source, id));
    await store.erase(id);

    const next = ${JSON.stringify(erasePoint)};
    const file = archiveFor(next);
    seal(file, saved.recoveryKey, [{ name: "private", bytes: Buffer.from("erase before retry") }], archiveContext(saved.source, next));
    const started = specOf(next, file);
    assert.equal((await ask("begin", started)).status, 200);
    fs.unlinkSync(partial);
    assert.deepEqual((await ask("begin", started)).value, { offset: 0 }, "recover a reservation written before its partial existed");
    assert.equal((await ask("append", started, { offset: 0, data: fs.readFileSync(file).toString("base64url") })).status, 200);
    closeRelay();
  `);
  cutCommit(erasePoint);
  fs.unlinkSync(pointFile(erasePoint)); // Simulate the power cut losing the unsynced directory entry.
  bunIn(owner, `
    ${stateCode}
    ${transfer}
    const id = ${JSON.stringify(erasePoint)};
    await waitForReceiver();
    await store.erase(id);
    assert.equal((await ask("commit", specOf(id, archiveFor(id)))).status, 409);
    assert.equal(fs.existsSync(journal), false);
    assert.equal(fs.existsSync(partial), false);
    assert.deepEqual(await store.list(), [saved.point.id]);
    closeRelay();
  `);
  bunIn(owner, `
    ${stateCode}
    ${transfer}
    const id = ${JSON.stringify(erasePoint)};
    await assert.rejects(store.erase(id), /could not sync the receiver directory/, "an erase retry must sync even an absent point");
    closeRelay();
  `, { KUVO_TEST_FAIL_BACKUP_SYNC: "1" });
  put(ownerBox, "box/kind", "mac");
  const macPoint = JSON.parse(bunIn(owner, `
    ${stateCode}
    import { backupBefore } from ${url("engine")};
    import { archiveContext, openArchive } from ${url("archive")};
    configPeer(43);
    assert.equal(readLink().key, undefined, "the detached worker starts with no persisted Mac keys");
    const password = "fixture-identity-host";
    const host = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
      const at = new URL(request.url);
      if (at.pathname === "/hello") {
        const given = await request.json();
        const proof = crypto.createHmac("sha256", password).update(["kuvo-host-hello", given.nonce, at.host].join(String.fromCharCode(0))).digest("hex");
        return Response.json({ proof });
      }
      assert.equal(request.headers.get("authorization"), "Bearer " + password);
      assert.equal(at.pathname, "/link/identity");
      return Response.json({ key: seed(43), sign: seed(47), secret: seed(52) });
    } });
    fs.writeFileSync(path.join(process.env.KUVO_HOME, "run", "host.json"), JSON.stringify({ url: host.url.href, password }));
    const point = await backupBefore("update", { insideUpdate: true });
    assert.equal(readLink().key, seed(43));
    const saved = ${JSON.stringify(saved)};
    const archive = path.join(path.dirname(${JSON.stringify(deposit)}), point.id + ".kuvo");
    const opened = openArchive(archive, saved.recoveryKey, archiveContext(saved.source, point.id), process.env.HOME);
    const manifest = JSON.parse(fs.readFileSync(opened.find(entry => entry.name === "meta/manifest.json").file, "utf8"));
    assert.equal(manifest.origin, source.key);
    assert.equal(manifest.setup, setup.id);
    console.log(JSON.stringify(point));
    host.stop(true);
    closeRelay();
  `, { KUVO_TEST_RELAY: "ws://127.0.0.1:1" }));
  fs.rmSync(ownerBox, { recursive: true });
  bunIn(receiving, `${stateCode} configPeer(42, ${JSON.stringify(saved.source)});`);
  assert.equal(fs.existsSync(deposit), true);
  bunIn(unrelated, `
    ${stateCode}
    import { destinationStore } from ${url("destination")};
    const saved = ${JSON.stringify(saved)};
    configPeer(45, saved.source);
    const store = destinationStore(destination, saved.source);
    assert.deepEqual(await store.list(), []);
    await assert.rejects(store.get(saved.point.id, process.env.HOME), /unavailable/);
    const mail = mailFor(target, { workQueue: { act: { area: "backup", id: "wqa_" + "e".repeat(20), method: "GET",
      item: saved.source, body: { action: "read", id: saved.point.id, offset: 0 }, at: Date.now() } } }, state(43));
    assert.deepEqual(await send([mail.toString("base64url")]), [], "the revoked owner cannot read");
    closeRelay();
  `);
  const wrongSetup = "set_" + "Y".repeat(22);
  bunIn(receiving, `${stateCode} configPeer(42, ${JSON.stringify(saved.source)}, ${JSON.stringify(wrongSetup)});`);
  bunIn(replacement, `
    ${stateCode}
    import { destinationStore } from ${url("destination")};
    const saved = ${JSON.stringify(saved)};
    configPeer(44, saved.source, ${JSON.stringify(wrongSetup)});
    const store = destinationStore(destination, saved.source);
    assert.deepEqual(await store.list(), []);
    await assert.rejects(store.get(saved.point.id, process.env.HOME), /unavailable/);
    await store.erase(saved.point.id);
    const wrong = await forwardAct(target, "Encrypted backup", { area: "backup", method: "GET",
      item: saved.source, body: { setup: saved.setup, action: "read", id: saved.point.id, offset: 0 },
    }, undefined, readLink(), Date.now(), { backgroundRelay: true });
    assert.equal(wrong.status, 409);
    assert.match(wrong.value.message, /accepted backup membership/);
    closeRelay();
  `);
  assert.equal(fs.existsSync(deposit), true, "the same source and replacement keys in another setup cannot erase the point");
  bunIn(receiving, `${stateCode} configPeer(42, ${JSON.stringify(saved.source)});`);
  bunIn(replacement, `
    ${stateCode}
    import { destinationStore } from ${url("destination")};
    const saved = ${JSON.stringify(saved)};
    configPeer(44, saved.source);
    const receiverLink = path.join(receiver, ".kuvo-code", "run", "link.json");
    let repliesSeen = 0;
    onReply = async replies => {
      assert.ok(replies.length, "a chunk reply can span several peer-mail frames");
      if (++repliesSeen === 1) {
        const changed = state(42, saved.source);
        changed.shared.membershipAndAccess.sequence++;
        changed.shared.membershipAndAccess.value.boxes = [target, replacement];
        changed.shared.membershipAndAccess.value.left.push(unrelated.key);
        changed.shared.ownershipAssignments = { sequence: 2, value: {} };
        fs.writeFileSync(receiverLink + ".fixture", JSON.stringify(changed), { mode: 0o600 });
        fs.renameSync(receiverLink + ".fixture", receiverLink);
      }
      return deliver(replies);
    };
    await assert.rejects(destinationStore(destination, saved.source).get(saved.point.id, process.env.HOME), /unavailable/);
    assert.equal(repliesSeen, 2, "the first chunk was authorized and the next one was rechecked");
    assert.equal(fs.existsSync(path.join(process.env.HOME, "received")), false);
    closeRelay();
  `);
  bunIn(receiving, `${stateCode} configPeer(42, ${JSON.stringify(saved.source)});`);
  bunIn(replacement, `
    ${stateCode}
    import { restore } from ${url("restore")};
    import { destinationStore } from ${url("destination")};
    import { readSubmissionForPeer } from ${linkURL("ownership/submissions")};
    import { draftKey } from ${linkURL("ownership/forward-evidence")};
    import { readRecord, recordKey } from ${linkURL("ownership/store")};
    import { engineHandler } from ${linkURL("server/router")};
    import { collected } from ${linkURL("server/sent-once")};
    const saved = ${JSON.stringify(saved)};
    configPeer(44, saved.source);
    assert.deepEqual(new Set(await destinationStore(destination, saved.source).list()), new Set([saved.point.id, ${JSON.stringify(macPoint.id)}]));
    const scope = { setup: setup.id, session: "ses_archive", viewer: { kind: "ssh", key: target.key } };
    assert.deepEqual((await readSubmissionForPeer(target, { body: scope })).value, { complete: true, outcomes: [] });
    await restore(destination, saved.source, saved.point.id, saved.recoveryKey);
    const receipt = await readSubmissionForPeer(target, { body: { setup: setup.id, session: "ses_archive",
      submission: setup.id + "." + source.key + ".msg_archived", viewer: { kind: "ssh", key: target.key } } });
    assert.equal(receipt.value.outcomes[0]?.originalOwner, source.key, JSON.stringify(receipt.value));
    assert.equal(receipt.value.outcomes[0]?.status, "committed");
    assert.equal(receipt.value.outcomes[0]?.effect.answer.shell, "example");
    assert.ok(receipt.value.outcomes[0]?.acceptedAt > 0);
    const indexed = (await readSubmissionForPeer(target, { body: scope })).value;
    assert.equal(indexed.complete, true);
    assert.deepEqual(indexed.outcomes.map(row => [row.originalOwner, row.acceptedAt]), [[source.key, receipt.value.outcomes[0].acceptedAt]]);
    assert.equal(readRecord("mutations", draftKey(setup.id, "ses_archive", "msg_archived")), undefined, "replay fence belongs with the moved session");
    assert.deepEqual(readRecord("mutations", recordKey("legacy-opaque")), { digest: "opaque evidence" });
    const submission = setup.id + "." + source.key + ".msg_archived";
    const request = { fromApp: true, method: "POST", path: "kuvo/sessions/ses_archive/submit", headers: {},
      body: Buffer.from(JSON.stringify({ id: "msg_archived", submission, text: "!changed", message: {} })) };
    const replay = await collected(engineHandler(undefined, { name: "Replacement" })(request));
    assert.equal(replay.status, 409, "the restored receipt cannot authorize execution at the replacement");
    closeRelay();
  `);
  assert.deepEqual(fs.readFileSync(path.join(home(replacement), "bots/bot_a/MEMORY.md")), bytes);
  fs.rmSync(receivingBox, { recursive: true });
  assert.equal(fs.existsSync(deposit), true, "wiping the receiving Box leaves the encrypted archive");
});

test("failure bench: backup restore matches the enrolled replacement and keeps the old key revoked", () => {
  const owner = scratch();
  bunIn(owner, `
    import assert from "node:assert/strict";
    import { publicOf } from ${cryptoKeys};
    import { newSigningSeed, signingKeyOf } from ${setup};
    import { docsNoted } from ${JSON.stringify(new URL("../src/link/documents/changed.ts", import.meta.url).href)};
    import { matchReplacement, restoreForPeer } from ${url("restore")};
    import { writeLink, readLink } from ${JSON.stringify(new URL("../src/link/state.ts", import.meta.url).href)};
    import { ownerOf, ownedDigest } from ${JSON.stringify(new URL("../src/link/ownership/resolve.ts", import.meta.url).href)};
    import { grantRelayLease } from ${leaseHelper};
    import fs from "node:fs";
    import path from "node:path";
    const key = Buffer.alloc(32, 42).toString("base64url");
    const keyOf = n => publicOf(Buffer.alloc(32, n)).toString("base64url");
    const own = keyOf(42), old = keyOf(43), replacement = keyOf(44), other = keyOf(45);
    const peer = key => ({ key, kind: "machine", name: key, address: "A".repeat(22) });
    const sign = newSigningSeed();
    const id = "set_" + "A".repeat(22);
    writeLink({ key, sign, setup: { id, primary: own, sign: signingKeyOf(sign), role: "primary", managed: true, checkpointed: true, epoch: 1 },
      sharedSetup: id, shared: {
        membershipAndAccess: { sequence: 1, value: { left: [old], boxes: [own, replacement, other].map(peer),
          computerHistory: { setup: id, slots: [{ key: replacement, previous: [old] }], retiredSigns: [] } } },
      } });
    grantRelayLease(readLink());
    const source = ${JSON.stringify(source)};
    const act = { body: { source, origin: old, setup: id } };
    assert.equal(restoreForPeer(peer(replacement), act).status, 409);
    await assert.rejects(matchReplacement(source, old, old), /new membership/);
    await assert.rejects(matchReplacement(source, old, other), /conflicts with the setup/);
    assert.equal(docsNoted(), false, "refused matches leave no publication notification");
    assert.equal((await matchReplacement(source, old, replacement)).status, "committed");
    assert.equal(docsNoted(), true, "a durable replacement match wakes peer publication");
    assert.equal(restoreForPeer(peer(replacement), act).status, 200);
    assert.equal(restoreForPeer(peer(other), act).status, 409);
    assert.equal(restoreForPeer(peer(old), act).status, 409);
    // The replacement restored the old computer's conversation and says it has it: it's found there.
    fs.writeFileSync(path.join(process.env.KUVO_HOME, "run", "peer-agents.json"),
      JSON.stringify({ [replacement]: { heard: Date.now(), list: [], owns: ownedDigest("ses_a") } }));
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
    import { BACKUP } from ${url("engine")};
    import fs from "node:fs";
    import path from "node:path";
    ${install(dest)}
    const point = ${JSON.stringify(exported)};
    makeVault();
    const leftover = path.join(BACKUP, ".scratch-interrupted");
    fs.mkdirSync(leftover, { recursive: true });
    fs.writeFileSync(path.join(leftover, "plain"), "test-secret-849");
    const write = fs.writeSync;
    let inspected = false;
    fs.writeSync = (fd, data, ...args) => {
      if (Buffer.isBuffer(data) && data.includes("test-secret-849")) {
        inspected = true;
        for (const dir of fs.readdirSync(BACKUP).filter(name => name.startsWith(".scratch-"))) {
          assert.deepEqual(fs.readdirSync(path.join(BACKUP, dir)), []);
        }
      }
      return write(fd, data, ...args);
    };
    await assert.rejects(importVault(destination, a.source, point.id, a.key), /authenticate/);
    assert.equal(readItems().length, 0);
    assert.equal((await importVault(destination, a.source, point.id, point.recoveryKey)).imported, 1);
    assert.equal(inspected, true);
    assert.equal(fs.existsSync(leftover), false);
    assert.equal((await valuesOf(readItems()[0].id)).password, "test-secret-849");
    assert.deepEqual(readGrants(), []);
  `);
});

test("the built box previews a destination before enabling and requires the local app", () => {
  const owner = scratch(), dest = scratch();
  const link = JSON.stringify(LINK_BUNDLE.href);
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

test("a conversation the old computer moved away stays fenced on its replacement, during normal writes and restore", () => {
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
        membershipAndAccess: { sequence: 1, value: { left: [old], boxes: [own, target].map(peer) } },
        ownershipAssignments: { sequence: 1, value: { ["replacement:" + old]: { computer: own } } },
      } });
    // The old computer's move journal, restored here, says ses_a went to target.
    noteMovedAway(["ses_a"], target);
    const link = readLink(), ref = { kind: "conversations", id: "ses_a" };
    assert.equal(ownership.ownerOf(ref, { link }).key, target);
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

test("backup and restore keep the Work Queue attachments named in the item", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const box = home(owner), target = home(replacement), item = "wqi_" + "F".repeat(20);
  const names = ["design.new", "yarn.lock", "report.tmp", ".details"];
  const folder = `run/work-queue/files/${item}`;
  put(box, "folder-version.json", '{"version":7}');
  put(target, "folder-version.json", '{"version":7}');
  put(box, "run/work-queue/queues/usr_you.json", JSON.stringify({ owner: "usr_you", items: [{
    id: item, title: "Attachments", files: names.map(name => ({ name, size: name.length })),
  }] }));
  for (const name of names) put(box, `${folder}/${name}`, name);
  put(box, `${folder}/uncommitted.tmp`, "exclude");
  put(box, `${folder}/unlisted.bin`, "exclude");
  put(box, "run/work-queue/queues/stale.json.new", "exclude");

  take(owner, dest);
  fs.rmSync(box, { recursive: true });
  assert.deepEqual(recover(replacement, dest).unavailable, []);
  for (const name of names) assert.equal(fs.readFileSync(path.join(target, folder, name), "utf8"), name);
  assert.equal(fs.existsSync(path.join(target, folder, "uncommitted.tmp")), false);
  assert.equal(fs.existsSync(path.join(target, folder, "unlisted.bin")), false);
  assert.equal(fs.existsSync(path.join(target, "run/work-queue/queues/stale.json.new")), false);
});

test("an export rejects different attachment bytes and a short Work Queue payload", () => {
  const owner = scratch(), box = home(owner), item = "wqi_" + "A".repeat(20);
  const sessionFile = put(box, "attachments/ses_a/012345ab/yarn.lock", Buffer.of(1, 0, 255));
  const queueFile = put(box, `run/work-queue/files/${item}/queue.db`, Buffer.of(0, 1, 2));
  put(box, "run/work-queue/queues/usr_you.json", JSON.stringify({ owner: "usr_you", items: [{
    id: item, files: [{ name: "queue.db", size: 3 }],
  }] }));
  bunIn(owner, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { captureBackup, scratchFolder } from ${url("engine")};
    import { sourceGeneration } from ${JSON.stringify(new URL("../src/link/setup/removal-generation.ts", import.meta.url).href)};
    ${install(owner)}
    const capture = attachmentHashes => {
      const scratch = scratchFolder();
      try { captureBackup(path.join(scratch, "archive"), a.key, a.source, a.id, a.epoch, [], scratch, false, attachmentHashes); }
      finally { fs.rmSync(scratch, { recursive: true }); }
    };
    const hashes = new Map();
    sourceGeneration([], hashes);
    fs.writeFileSync(${JSON.stringify(sessionFile)}, Buffer.of(2, 0, 255));
    assert.throws(() => capture(hashes), /Attachment changed during export.*yarn[.]lock/);
    fs.truncateSync(${JSON.stringify(queueFile)}, 2);
    assert.throws(() => capture(), /attachment size differs.*queue[.]db/);
  `);
});

test("restore reports omissions in old points and keeps the bytes for a soft-deleted item's undo", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const item = "wqi_" + "A".repeat(20), deleted = "wqi_" + "B".repeat(20);
  const queue = "home/run/work-queue/queues/usr_you.json";
  const leaf = `home/run/work-queue/files/${deleted}/proof.tmp`;
  bunIn(owner, `
    import path from "node:path";
    import { archiveContext, seal } from ${url("archive")};
    import { bytes } from ${url("engine")};
    import { destinationStore } from ${url("destination")};
    ${install(dest)}
    const file = path.join(process.env.KUVO_HOME, "legacy-point");
    seal(file, a.key, [
      bytes("meta/manifest.json", { id: a.id, format: 1, source: a.source, at: a.epoch, version: 7,
        origin: null, signer: null, setup: null, paired: false, projects: [] }),
      bytes(${JSON.stringify(queue)}, { owner: "usr_you", items: [
        { id: "${item}", files: [{ name: "missing.db", size: 4 }, { name: "short.txt", size: 2 }] },
        { id: "${deleted}", removed: { at: a.epoch, by: { user: true } }, files: [{ name: "proof.tmp", size: 3 }] },
      ] }),
      { name: ${JSON.stringify(leaf)}, bytes: Buffer.of(0, 255, 7) },
      { name: "home/run/work-queue/files/${item}/short.txt", bytes: Buffer.of(0) },
    ], archiveContext(a.source, a.id));
    await destinationStore(destination, a.source).put(a.id, file);
  `);
  const target = home(replacement);
  const result = recover(replacement, dest);
  assert.deepEqual(result.unavailable, [
    { entry: `home/run/work-queue/files/${item}/missing.db`, reason: "attachment missing from restore point" },
    { entry: `home/run/work-queue/files/${item}/short.txt`, reason: "attachment size differs from its record" },
  ]);
  assert.deepEqual(fs.readFileSync(path.join(target, leaf.slice(5))), Buffer.of(0, 255, 7));
  assert.ok(JSON.parse(fs.readFileSync(path.join(target, queue.slice(5)), "utf8")).items[1].removed);
});

test("held Work Queue items can be kept or discarded through recovery", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const box = home(owner), target = home(replacement);
  const keep = "wqi_" + "K".repeat(20), discard = "wqi_" + "D".repeat(20);
  const queue = "run/work-queue/queues/usr_you.json";
  const handing = { id: "wqh_restore", to: "ses_other", key: "A".repeat(43), at: 1, by: { user: true } };
  put(box, "folder-version.json", '{"version":7}');
  put(target, "folder-version.json", '{"version":7}');
  put(box, queue, JSON.stringify({ owner: "usr_you", items: [
    { id: keep, title: "Keep this", handing, files: [{ name: "proof.txt", size: 5 }] },
    { id: discard, title: "Discard this", handing },
  ] }));
  put(box, `run/work-queue/files/${keep}/proof.txt`, "proof");
  take(owner, dest);
  recover(replacement, dest);
  bunIn(replacement, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import { engineHandler } from ${JSON.stringify(new URL("../src/link/server/router.ts", import.meta.url).href)};
    import { collected } from ${JSON.stringify(new URL("../src/link/server/sent-once.ts", import.meta.url).href)};
    const keep = ${JSON.stringify(keep)}, discard = ${JSON.stringify(discard)};
    const handle = engineHandler();
    const ask = async (method, given = {}, fromApp = true) => {
      const reply = await collected(handle({ fromApp, served: true, method, path: "kuvo/box/backups/pending", headers: {},
        body: Buffer.from(JSON.stringify(given)) }, {}));
      return { status: reply.status, body: JSON.parse(reply.body) };
    };
    const pending = (await ask("GET")).body.queueItems;
    assert.deepEqual(pending.map(item => item.id).sort(), [discard, keep]);
    assert.equal(pending.find(item => item.id === keep).canKeep, true);
    const choice = { kind: "queueItem", id: keep, operation: "keep-recovered-item", decision: "keep" };
    assert.equal((await ask("POST", choice, false)).status, 403);
    const reviewed = await ask("POST", choice);
    assert.equal(reviewed.status, 200, JSON.stringify(reviewed.body));
    assert.deepEqual(await ask("POST", choice), reviewed);
    assert.equal((await ask("POST", { ...choice, decision: "discard" })).status, 409);
    assert.equal((await ask("POST", { kind: "queueItem", id: discard, operation: "discard-recovered-item", decision: "discard" })).status, 200);
    assert.deepEqual((await ask("GET")).body.queueItems, []);
    const restored = JSON.parse(fs.readFileSync(process.env.KUVO_HOME + "/${queue}", "utf8")).items;
    assert.deepEqual(restored.map(item => item.id), [keep]);
    assert.equal(restored[0].handing, undefined);
    assert.equal(fs.readFileSync(process.env.KUVO_HOME + "/run/work-queue/files/" + keep + "/proof.txt", "utf8"), "proof");
  `);
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
    const { entries: upgraded, queueProblems } = stageRestore(stage, 0, [
      { name: "home/run/named.json", file: legacy },
      { name: "bots/bot_a/notes.md", file: legacy },
      { name: "macAttachments/ses_a/.keep", file: legacy },
    ], folder => upgradeFolder(folder, [at => {
      fs.renameSync(path.join(at, "run/named.json"), path.join(at, "run/archived.json"));
      const notes = path.join(at, "bots/bot_a");
      fs.renameSync(path.join(notes, "notes.md"), path.join(notes, "MEMORY.md"));
    }]));
    assert.deepEqual([...upgraded.keys()].sort(), ["bots/bot_a/MEMORY.md", "home/run/archived.json", "macAttachments/ses_a/.keep"]);
    for (const file of upgraded.values()) assert.equal(fs.readFileSync(file, "utf8"), "legacy");
    assert.equal(queueProblems.size, 0);
  `);
});

test("failure bench: a paired backup cannot reactivate an item handed off after that backup", () => {
  const old = scratch(), primary = scratch(), replacement = scratch(), dest = scratch();
  const oldBox = home(old), item = "wqi_" + "B".repeat(20);
  put(oldBox, "folder-version.json", '{"version":7}');
  put(oldBox, "run/work-queue/queues/usr_you.json", JSON.stringify({ owner: "usr_you", items: [{ id: item, title: "Before handoff" }, { id: "wqi_" + "C".repeat(20), title: "Unchanged" }] }));
  const uncertain = "agents/sessions/ses_ambiguous/messages.json";
  const current = "agents/sessions/ses_current/messages.json";
  put(oldBox, uncertain, '["ambiguous history"]');
  put(oldBox, current, '["current history"]');
  const legacy = "storage/message/ses_ambiguous/msg_held.json";
  const legacyCurrent = "storage/message/ses_current/msg_current.json";
  const opaque = "storage/part/msg_held/prt_opaque.json";
  put(oldBox, "data/opencode/" + legacy, '{"id":"msg_held","sessionID":"ses_ambiguous"}');
  put(oldBox, "data/opencode/" + legacyCurrent, '{"id":"msg_current","sessionID":"ses_current"}');
  put(oldBox, "data/opencode/" + opaque, '{"id":"prt_opaque","messageID":"msg_held"}');
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
        ...(after ? { ownershipAssignments: { sequence: 1, value: { ["replacement:" + old]: { computer: replacement, source },
          "session computer:ses_current": { computer: replacement }, ["queue item:wqi_" + "B".repeat(20)]: { computer: recipient, queue: "ses_recipient" } } } } : {}),
      },
    });
  `;
  bunIn(old, `${authority} writeLink(state(43, false));`);
  assert.equal(JSON.parse(take(old, dest)).paired, true);
  // The handoff after the backup reached the recipient. The primary accepted its new owner.
  const record = n => JSON.stringify(new URL("../src/link/" + n + ".ts", import.meta.url).href);
  bunIn(old, `${authority}
    import { noteDeparture } from ${record("work-queue/store")};
    noteDeparture(${JSON.stringify(item)}, { queue: "ses_recipient", computer: recipient });
  `);
  fs.rmSync(oldBox, { recursive: true });
  const approved = JSON.parse(bunIn(primary, `${authority}
    import { restoreForPeer } from ${url("restore")};
    import { readLink } from ${JSON.stringify(new URL("../src/link/state.ts", import.meta.url).href)};
    import { grantRelayLease } from ${leaseHelper};
    writeLink(state(42, true));
    grantRelayLease(readLink());
    console.log(JSON.stringify(restoreForPeer(peer(replacement), { body: { source, origin: old, setup: id } })));
  `));
  assert.equal(approved.status, 200);
  const result = JSON.parse(bunIn(replacement, `
    import fs from "node:fs";
    import path from "node:path";
    import { restore } from ${url("restore")};
    import { ownerOf } from ${record("ownership/resolve")};
    import { keepTold } from ${record("work-queue/told")};
    import { FORWARDS, forwardsSoon, takeActed } from ${record("work-queue/forward")};
    ${authority}
    ${install(dest)}
    writeLink(state(44, true));
    const run = path.join(process.env.KUVO_HOME, "run");
    // After the backup, ses_ambiguous moved on to the recipient, which says it has it. Nobody else
    // has ses_current: only the old computer did.
    fs.writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify({
      [primary]: { features: ["data.ownership"], heard: Date.now(), list: [] },
      [recipient]: { features: ["data.ownership"], heard: Date.now(), list: [{ session: "ses_ambiguous" }] },
    }));
    if (ownerOf({ kind: "sessionFiles", id: "ses_ambiguous" })?.key !== recipient) throw Error("Expected the recipient to have it");
    if (ownerOf({ kind: "sessionFiles", id: "ses_current" }) !== null) throw Error("Expected no other computer to have it");
    // The handed-off item is in the recipient's queue, as the recipient tells it.
    keepTold(recipient, { id: "told_recipient", at: Date.now(), kind: "machine", owners: [], labels: [],
      items: [{ id: ${JSON.stringify(item)}, owner: "ses_recipient", title: "Before handoff" }] });
    forwardsSoon.send = () => {
      const [file] = fs.readdirSync(FORWARDS);
      const { act } = JSON.parse(fs.readFileSync(path.join(FORWARDS, file), "utf8"));
      if (act.item !== "backup-restore") throw new Error("Unexpected owner request");
      takeActed(peer(primary), { id: act.id, ...${JSON.stringify(approved)} });
    };
    console.log(JSON.stringify(await restore(destination, a.source, a.id, a.key)));
  `));
  assert.ok(result.held.includes(uncertain));
  assert.equal(fs.existsSync(path.join(home(replacement), uncertain)), false);
  assert.equal(fs.readFileSync(path.join(home(replacement), "backup/recovered", id, uncertain), "utf8"), '["ambiguous history"]');
  assert.equal(fs.readFileSync(path.join(home(replacement), current), "utf8"), '["current history"]');
  for (const file of [legacy, opaque]) {
    assert.ok(result.held.includes("engine/" + file));
    assert.equal(fs.existsSync(path.join(home(replacement), "data/opencode", file)), false);
    assert.ok(fs.existsSync(path.join(home(replacement), "backup/recovered", id, "engine", file)));
  }
  assert.ok(fs.existsSync(path.join(home(replacement), "data/opencode", legacyCurrent)));
  assert.ok(result.held.some(entry => entry.startsWith("work-queue-items/")));
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(home(replacement), "run/work-queue/queues/usr_you.json"), "utf8")).items.map(item => item.title), ["Unchanged"]);
});

test("failure bench: the replacement group runner holds meetings, resumed turns and waiting work for review", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const box = home(owner);
  const time = "2026-11-01T00:00:00.000Z";
  const group = { id: "grp_a", name: "Fixture", members: ["bot_a", "bot_b"], created: time, seats: {}, heard: {},
    meeting: { next: time, topic: "Overdue meeting" } };
  const turning = { bot: "bot_a", seat: "ses_a", agents: false, prompt: "msg_a", sent: 123 };
  const messages = [{ id: "gm_wrap", kind: "wrapUp", text: "Decisions", time, tasks: [
    { id: "tsk_sent", bot: "bot_a", task: "Already sent", state: "working", prompt: "peer:old" },
    { id: "tsk_wait", bot: "bot_b", task: "Still waiting", state: "waiting" },
  ] }];
  put(box, "groups/grp_a/group.json", JSON.stringify({ kuvo: 1, status: "active", group, turning }));
  put(box, "groups/grp_a/messages.json", JSON.stringify(messages));
  put(box, "groups/peer-work.json", '{"unsure":{"task":"remote part"}}');
  put(box, "groups/peer-turns.json", '{"unsure":{"turn":"remote turn"}}');
  take(owner, dest);
  const restored = recover(replacement, dest);
  assert.ok(restored.held.includes("groups/grp_a/group.json"));
  assert.ok(restored.held.includes("groups/peer-work.json"));
  assert.ok(restored.held.includes("groups/peer-turns.json"));
  const groups = new URL("../src/agents/groups/", import.meta.url);
  bunIn(replacement, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import { GroupRunner } from ${JSON.stringify(new URL("runner.ts", groups).href)};
    import { GroupStore } from ${JSON.stringify(new URL("store.ts", groups).href)};
    import { linkRoutes } from ${JSON.stringify(new URL("link.ts", groups).href)};
    const root = process.env.KUVO_HOME;
    const store = new GroupStore(root + "/groups", root + "/bots");
    const runner = new GroupRunner({ store, engine: () => undefined, bridge: () => undefined, trash: root + "/trash",
      peers: { bots: () => [], home: () => { throw new Error("Resubmitted a peer task"); } } });
    runner.meet = () => { throw new Error("Held an overdue meeting"); };
    runner.startReady = () => { throw new Error("Started waiting work"); };
    runner.start();
    runner.meetDue(${epoch});
    assert.equal(runner.activate("grp_a"), false);
    assert.equal(runner.get("grp_a").restoreReview, true);
    assert.deepEqual(store.read("grp_a").turning, ${JSON.stringify(turning)});
    assert.deepEqual(runner.get("grp_a").messages, ${JSON.stringify(messages)});
    assert.deepEqual(runner.get("grp_a").group.meeting, ${JSON.stringify(group.meeting)});
    assert.equal(fs.existsSync(root + "/groups/grp_a/awake"), false);
    assert.deepEqual(store.peerWork(), {});
    const list = await (await linkRoutes(runner, store, "GET", [], async () => {})).json();
    assert.equal(list.groups[0].restoreReview, true);
    const detail = await (await linkRoutes(runner, store, "GET", ["grp_a"], async () => {})).json();
    assert.equal(detail.messages[0].text, "Decisions");
    assert.equal((await linkRoutes(runner, store, "POST", ["grp_a", "keep-going"], async () => {})).status, 409);
    const review = async (decision, operation) => {
      const response = await linkRoutes(runner, store, "POST", ["grp_a", "restore-review"], async () => ({ decision, operation }));
      return { status: response.status, body: await response.json() };
    };
    const done = await review("discard", "discard-review-1");
    assert.equal(done.status, 200);
    assert.deepEqual(await review("discard", "discard-review-1"), done);
    assert.equal((await review("resume", "discard-review-1")).status, 409);
    assert.equal(store.read("grp_a").status, "active");
    assert.equal(store.read("grp_a").restoreReview, undefined);
    assert.equal(store.read("grp_a").turning, undefined);
    assert.equal(store.read("grp_a").group.meeting, undefined);
    assert.ok(store.read("grp_a").messages[0].tasks.every(task => task.state === "failed"));
    runner.keepGoing = () => {};
    assert.equal((await linkRoutes(runner, store, "POST", ["grp_a", "keep-going"], async () => {})).status, 200);
  `);
});

test("group memory restores its peer recaps and points to the replacement's transcript", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const box = home(owner);
  const leaf = "prefix-grp_a.md";
  const transcript = put(box, "bots/bot_a/groups/" + leaf, "Private transcript");
  const memory = { name: "Elsewhere", with: ["Other bot"], recap: "Decision", file: transcript, time: "2026-11-01T00:00:00.000Z", at: 1 };
  put(box, "groups/peer-memory.json", JSON.stringify({ bot_a: { "peer/grp_a": memory, "peer/grp_b": { forgotten: true, at: 2 } } }));
  take(owner, dest);
  recover(replacement, dest);
  const target = home(replacement);
  const file = path.join(target, "bots/bot_a/groups", leaf);
  assert.equal(fs.readFileSync(file, "utf8"), "Private transcript");
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(target, "groups/peer-memory.json"), "utf8")), {
    bot_a: { "peer/grp_a": { ...memory, file }, "peer/grp_b": { forgotten: true, at: 2 } },
  });
});

test("a legacy archive never installs the old Full Access grant", () => {
  const dest = scratch(), owner = scratch(), replacement = scratch();
  bunIn(owner, `
    import { archiveContext, seal } from ${url("archive")};
    import { bytes } from ${url("engine")};
    import { destinationStore } from ${url("destination")};
    ${install(dest)}
    const file = process.env.HOME + "/archive";
    seal(file, a.key, [
      bytes("meta/manifest.json", { format: 1, version: 7, id: a.id, at: a.epoch, source: a.source, origin: null, signer: null, setup: null, paired: false, projects: [] }),
      bytes("home/box/approval.json", { fullAccess: true, last: "fullAccess", since: 123 }),
    ], archiveContext(a.source, a.id));
    await destinationStore(destination, a.source).put(a.id, file);
  `);
  recover(replacement, dest);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(home(replacement), "box/approval.json"), "utf8")), { last: "fullAccess", since: 123 });
});

test("a populated replacement database is retained and its recovered snapshot held", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  for (const [root, row] of [[owner, "ses_archived"], [replacement, "ses_current"]]) bunIn(root, `
    import fs from "node:fs";
    import { Database } from "bun:sqlite";
    const dir = process.env.KUVO_HOME + "/data/opencode";
    fs.mkdirSync(dir, { recursive: true });
    const db = new Database(dir + "/opencode.db");
    db.exec("PRAGMA journal_mode=WAL; CREATE TABLE session_v2(id TEXT PRIMARY KEY)");
    db.query("INSERT INTO session_v2 VALUES (?)").run(${JSON.stringify(row)});
    db.close();
  `);
  take(owner, dest);
  const result = recover(replacement, dest);
  assert.ok(result.unavailable.some(item => item.entry === "engine/opencode.db"));
  bunIn(replacement, `
    import assert from "node:assert/strict";
    import { Database } from "bun:sqlite";
    for (const [folder, row] of [["data/opencode", "ses_current"], ["backup/recovered/" + ${JSON.stringify(id)} + "/engine", "ses_archived"]]) {
      const db = new Database(process.env.KUVO_HOME + "/" + folder + "/opencode.db", { readonly: true });
      assert.deepEqual(db.query("SELECT id FROM session_v2").all(), [{ id: row }]);
      db.close();
    }
  `);
});

test("failure bench: a Work Queue handoff cannot land between the two queue reads", () => {
  const owner = scratch();
  for (const [queue, items] of [["ses_a", []], ["usr_you", [{ id: "wqi_fixture", title: "Move me" }]]]) {
    put(home(owner), "run/work-queue/queues/" + queue + ".json", JSON.stringify({ owner: queue, items }));
  }
  bunIn(owner, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { captureBackup, scratchFolder } from ${url("engine")};
    import { archiveContext, openArchive } from ${url("archive")};
    import { locked } from ${JSON.stringify(new URL("../src/shared/files.ts", import.meta.url).href)};
    ${install(owner)}
    const folder = process.env.KUVO_HOME + "/run/work-queue";
    const to = folder + "/queues/ses_a.json", from = folder + "/queues/usr_you.json";
    const handoff = () => locked(folder + "/.lock", () => {
      const item = { id: "wqi_fixture", title: "Move me" };
      fs.writeFileSync(to, JSON.stringify({ owner: "ses_a", items: [item] }));
      fs.writeFileSync(from, JSON.stringify({ owner: "usr_you", items: [] }));
    }, { exclusive: true, wait: 30 });
    const copy = fs.cpSync, open = fs.openSync;
    let tried = false, held = false;
    const tryHandoff = file => {
      if (file !== from || tried) return;
      tried = true;
      try { handoff(); }
      catch (error) {
        if (!String(error).includes("updating its link")) throw error;
        held = true;
      }
    };
    fs.cpSync = (file, ...args) => { tryHandoff(file); return copy(file, ...args); };
    fs.openSync = (file, ...args) => { tryHandoff(file); return open(file, ...args); };
    const scratch = scratchFolder(), file = path.join(scratch, "archive");
    captureBackup(file, a.key, a.source, a.id, a.epoch, [], scratch);
    const entries = openArchive(file, a.key, archiveContext(a.source, a.id), scratch);
    const queues = entries.filter(entry => entry.name.startsWith("home/run/work-queue/queues/"));
    const items = queues.flatMap(entry => JSON.parse(fs.readFileSync(entry.file, "utf8")).items);
    assert.deepEqual(items.map(item => item.id), ["wqi_fixture"]);
    assert.ok(tried && held);
    handoff();
    assert.equal(JSON.parse(fs.readFileSync(to)).items.length, 1);
  `);
});

test("a disk that rejects hard links can publish a point without overwriting it on retry", () => {
  const owner = scratch(), dest = scratch();
  bunIn(owner, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import { destinationStore } from ${url("destination")};
    ${install(dest)}
    fs.linkSync = () => { throw Object.assign(new Error("Hard links unavailable on this filesystem"), { code: "ENOTSUP" }); };
    const file = process.env.HOME + "/ciphertext";
    fs.writeFileSync(file, "first ciphertext");
    const store = destinationStore(destination, a.source);
    await store.put(a.id, file);
    fs.writeFileSync(file, "different ciphertext");
    await assert.rejects(store.put(a.id, file), /already exists/);
    assert.deepEqual(await store.list(), [a.id]);
    assert.equal(fs.readFileSync(await store.get(a.id, ""), "utf8"), "first ciphertext");
  `);
});

test("opted-in projects recover executable modes, empty directories and safe links", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const project = path.join(owner, "project");
  const script = put(project, "bin/check", "#!/bin/sh\n");
  fs.chmodSync(script, 0o755);
  fs.mkdirSync(path.join(project, "empty"), { mode: 0o750 });
  fs.symlinkSync("./bin/check", path.join(project, "check"));
  fs.symlinkSync("../elsewhere", path.join(project, "escape"));
  execFileSync("git", ["init", "-q", project]);
  execFileSync("git", ["-C", project, "add", "bin/check"]);
  fs.appendFileSync(script, "exit 0\n");
  put(project, "untracked", "new work");
  bunIn(owner, `
    import assert from "node:assert/strict";
    import path from "node:path";
    import { captureBackup, scratchFolder } from ${url("engine")};
    import { destinationStore } from ${url("destination")};
    import { previewProjects } from ${url("projects")};
    ${install(dest)}
    const [preview] = previewProjects([${JSON.stringify(project)}]);
    assert.equal(preview.folder, ${JSON.stringify(project)});
    assert.ok(preview.changes.some(file => file.path === "bin/check" && file.status === "AM" && !file.untracked));
    assert.ok(preview.changes.some(file => file.path === "untracked" && file.untracked));
    const scratch = scratchFolder(), file = path.join(scratch, "archive");
    captureBackup(file, a.key, a.source, a.id, a.epoch, [${JSON.stringify(project)}], scratch);
    await destinationStore(destination, a.source).put(a.id, file);
  `);
  const result = recover(replacement, dest);
  assert.ok(result.unavailable.some(entry => entry.entry === "projects/0/escape" && entry.reason === "unsafe symbolic link"));
  const tree = path.join(home(replacement), "backup/recovered", id, "projects/0");
  assert.equal(fs.statSync(path.join(tree, "bin/check")).mode & 0o777, 0o755);
  assert.deepEqual(fs.readdirSync(path.join(tree, "empty")), []);
  assert.equal(fs.statSync(path.join(tree, "empty")).mode & 0o777, 0o750);
  assert.equal(fs.readlinkSync(path.join(tree, "check")), "./bin/check");
  assert.equal(fs.lstatSync(path.join(tree, "escape"), { throwIfNoEntry: false }), undefined);
});

test("restored transcripts open managed bytes at changed and occupied paths and history reports pruned versions", () => {
  const owner = scratch(), dest = scratch(), replacement = scratch();
  const ownerHome = home(owner), replacementHome = home(replacement);
  const leaf = "attachments/ses_retained/msg_upload/proof.bin";
  const missing = "attachments/ses_retained/msg_upload/expired.bin";
  const file = put(ownerHome, leaf, Buffer.of(3, 0, 255));
  const record = { type: "file", url: pathToFileURL(file).href };
  const expired = { type: "file", url: pathToFileURL(path.join(ownerHome, missing)).href };
  const quote = `I wrote "${file}" yesterday and logged ${record.url}.`;
  const heading = "Open these attached files with your tools if you need them:";
  const note = `${quote}\n\n<attached-files>\n${heading}\n${file}\n</attached-files>`;
  put(ownerHome, "data/opencode/storage/message/ses_retained/msg_legacy.json", JSON.stringify({
    id: "msg_legacy", role: "user", metadata: { kuvo: { parts: { words: quote, paths: [file] } } },
  }));
  put(ownerHome, "data/opencode/storage/part/msg_legacy/prt_note.json", JSON.stringify({
    type: "text", text: note, sessionID: "ses_retained", messageID: "msg_legacy",
  }));
  put(ownerHome, "run/waiting/msg_restore.json", JSON.stringify({ id: "msg_restore", text: note, files: [{ uri: record.url }], delivery: "queue" }));
  put(ownerHome, "agents/sessions/ses_retained/messages.json", JSON.stringify([
    { parts: [record, expired, { type: "file", url: record.url + "%00" }] },
    { type: "user", text: note, files: [{ source: { type: "uri", uri: file } }] },
    { type: "shell", command: quote, output: { output: file } },
  ]));
  const occupied = put(replacementHome, leaf, Buffer.of(9, 0, 255));
  put(ownerHome, missing, Buffer.of(9));
  fs.unlinkSync(path.join(ownerHome, missing)); // The transcript still names a file that expired.
  const artifact = path.join(owner, "deliverable.png");
  const artifactStore = JSON.stringify(new URL("../src/link/artifacts/store.ts", import.meta.url).href);
  const history = JSON.parse(bunIn(owner, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { Database } from "bun:sqlite";
    import { ArtifactStore } from ${artifactStore};
    const root = process.env.KUVO_HOME;
    fs.mkdirSync(path.join(root, "data/opencode"), { recursive: true });
    const db = new Database(path.join(root, "data/opencode/opencode.db"));
    db.exec("CREATE TABLE session(id TEXT PRIMARY KEY); CREATE TABLE message(id TEXT PRIMARY KEY, data TEXT); CREATE TABLE part(id TEXT PRIMARY KEY, message_id TEXT, data TEXT); CREATE TABLE session_message(id TEXT PRIMARY KEY, data TEXT);");
    db.prepare("INSERT INTO message VALUES (?, ?)").run("msg_user", JSON.stringify({ role: "user", metadata: { kuvo: { parts: { words: ${JSON.stringify(quote)}, paths: [${JSON.stringify(file)}] } } } }));
    db.prepare("INSERT INTO message VALUES (?, ?)").run("msg_assistant", JSON.stringify({ role: "assistant" }));
    const part = db.prepare("INSERT INTO part VALUES (?, ?, ?)");
    part.run("prt_a", "msg_user", ${JSON.stringify(JSON.stringify(record))});
    for (const [id, owner] of [["prt_b", "msg_user"], ["prt_c", "msg_assistant"]]) {
      part.run(id, owner, ${JSON.stringify(JSON.stringify({ type: "text", text: note }))});
    }
    db.prepare("INSERT INTO session_message VALUES (?, ?)").run("msg_a", ${JSON.stringify(JSON.stringify({ content: [record, expired] }))});
    db.close();
    const store = new ArtifactStore(path.join(root, "artifacts"), { disk: () => ({ free: 100e9, total: 120e9 }) });
    const file = ${JSON.stringify(artifact)}, made = { by: { kind: "outside" }, via: "write", separate: true };
    fs.writeFileSync(file, "first artifact");
    const first = await store.change(file, made);
    fs.writeFileSync(file, "latest artifact bytes");
    const latest = await store.change(file, made);
    assert.equal(store.clearOlder(), 1);
    console.log(JSON.stringify({ id: latest.id, first: first.versions[0].n, latest: latest.versions.at(-1).n }));
  `));
  take(owner, dest);
  fs.unlinkSync(file);
  fs.unlinkSync(artifact);
  put(ownerHome, missing, "different file created at the old address");
  const result = recover(replacement, dest);
  assert.ok(result.unavailable.some(entry => entry.entry === missing));
  assert.ok(result.unavailable.some(entry => entry.entry === `artifacts/${history.id}/versions/${history.first}`));
  assert.deepEqual(fs.readFileSync(occupied), Buffer.of(9, 0, 255));
  bunIn(replacement, `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { fileURLToPath } from "node:url";
    import { Database } from "bun:sqlite";
    import { serveBox } from ${JSON.stringify(LINK_BUNDLE.href)};
    import { ArtifactStore } from ${artifactStore};
    import { restoreStatus } from ${url("restore")};
    const root = process.env.KUVO_HOME;
    const words = JSON.parse(fs.readFileSync(path.join(root, "agents/sessions/ses_retained/messages.json"), "utf8"));
    const attached = fileURLToPath(words[0].parts[0].url);
    assert.equal(words[1].files[0].source.uri, attached);
    assert.notEqual(attached, ${JSON.stringify(occupied)});
    assert.ok(attached.startsWith(path.join(root, "attachments/ses_retained")));
    const db = new Database(path.join(root, "data/opencode/opencode.db"));
    const part = id => JSON.parse(db.prepare("SELECT data FROM part WHERE id = ?").get(id).data);
    const original = ${JSON.stringify(note)};
    const expected = original.replace(${JSON.stringify("\n" + file + "\n</attached-files>")}, ["", attached, "</attached-files>"].join(String.fromCharCode(10)));
    assert.equal(words[1].text, expected);
    const legacyPart = JSON.parse(fs.readFileSync(path.join(root, "data/opencode/storage/part/msg_legacy/prt_note.json"), "utf8"));
    assert.equal(legacyPart.text, expected);
    const legacyMessage = JSON.parse(fs.readFileSync(path.join(root, "data/opencode/storage/message/ses_retained/msg_legacy.json"), "utf8"));
    assert.deepEqual(legacyMessage.metadata.kuvo.parts, { words: ${JSON.stringify(quote)}, paths: [attached] });
    assert.equal(words[2].command, ${JSON.stringify(quote)});
    assert.equal(words[2].output.output, ${JSON.stringify(file)});
    assert.equal(part("prt_a").url, words[0].parts[0].url);
    assert.equal(part("prt_b").text, expected);
    assert.equal(part("prt_c").text, original);
    const user = JSON.parse(db.prepare("SELECT data FROM message WHERE id = 'msg_user'").get().data);
    assert.deepEqual(user.metadata.kuvo.parts, { words: ${JSON.stringify(quote)}, paths: [attached] });
    const pending = JSON.parse(fs.readFileSync(path.join(root, "backup", "recovered", ${JSON.stringify(result.id)}, "home/run/waiting/msg_restore.json"), "utf8"));
    assert.equal(pending.text, expected);
    assert.equal(pending.files[0].uri, words[0].parts[0].url);
    assert.equal(JSON.parse(db.prepare("SELECT data FROM session_message").get().data).content[0].url, words[0].parts[0].url);
    db.close();
    const store = new ArtifactStore(path.join(root, "artifacts"));
    const { id, first, latest } = ${JSON.stringify(history)};
    assert.equal(store.bytes(id, first), null);
    assert.equal(store.bytes(id, latest).toString(), "latest artifact bytes");
    assert.notEqual(words[0].parts[1].url, ${JSON.stringify(expired.url)});
    const server = serveBox("fixture");
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
    const base = "http://127.0.0.1:" + server.address().port;
    const headers = { authorization: "Basic " + Buffer.from("opencode:fixture").toString("base64"), "x-kuvo-from": "app" };
    try {
      const response = await fetch(base + "/kuvo/file?uri=" + encodeURIComponent(words[0].parts[0].url), { headers });
      assert.equal(response.status, 200);
      assert.deepEqual(Buffer.from(await response.arrayBuffer()), Buffer.of(3, 0, 255));
      assert.equal((await fetch(base + "/kuvo/file?uri=" + encodeURIComponent(words[0].parts[1].url), { headers })).status, 404);
      const report = await fetch(base + "/kuvo/box/backups", { headers });
      assert.deepEqual((await report.json()).restore.result, restoreStatus().result);
      assert.deepEqual(restoreStatus().result, ${JSON.stringify(result)});
      assert.equal(restoreStatus().result.savedAfterPoint, "unavailable unless verified elsewhere");
    } finally { server.closeAllConnections(); server.close(); }
  `);
});
