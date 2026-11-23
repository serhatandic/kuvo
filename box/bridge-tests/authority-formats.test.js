import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";

const script = fileURLToPath(LINK_BUNDLE);

function machine() {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "kv-auth-"));
  removeAtEnd(home);
  const box = path.join(home, ".kuvo-code-audit");
  const run = path.join(box, "run");
  fs.mkdirSync(run, { recursive: true });
  const command = (args, input = "", bundle = script) => spawnSync(process.execPath, [bundle, ...args], {
    env: { ...process.env, HOME: home, KUVO_HOME: box },
    input, encoding: "utf8", timeout: 15_000,
  });
  const entries = () => fs.readdirSync(box, { recursive: true }).sort();
  return { home, box, run, command, entries };
}

test("SSH setup and seed settings reject explicit unknown versions before writing", () => {
  const { home, command, entries } = machine();
  const changes = { settings: {}, keys: { KUVO_KEY_NOTION_TOKEN: "test-key" } };
  const before = entries();
  for (const formatVersion of [2, 77, null, "1"]) {
    const adoption = command(["box", "adopt"], JSON.stringify({ ...changes, formatVersion }));
    assert.equal(adoption.status, 1, adoption.stderr);
    assert.match(adoption.stderr, /Box settings seed format version .* is unsupported/);
    assert.deepEqual(entries(), before);
    const setup = command(["setup"], JSON.stringify({ formatVersion }));
    assert.equal(setup.status, 2, setup.stderr);
    assert.match(setup.stderr, /SSH setup input format version .* is unsupported/);
    assert.deepEqual(entries(), before);
  }
  const token = "guarded-v2-" + "a".repeat(48);
  const reservation = path.join(home, ".kuvo-code-adding");
  fs.symlinkSync(token, reservation);
  for (const args of [["setup", token], ["setup"]]) {
    const stopped = command(args, JSON.stringify({ formatVersion: 1 }));
    assert.equal(stopped.status, 2, stopped.stderr);
    assert.match(stopped.stderr, /SSH setup reservation format is unsupported/);
  }
  assert.equal(fs.readlinkSync(reservation), token);
  assert.deepEqual(entries(), before);
});

test("the preceding and current settings seed and guarded setup token reach their deployed operations", () => {
  for (const marker of [{}, { formatVersion: 1 }]) {
    const { home, box, command } = machine();
    const accepted = command(["box", "adopt"], JSON.stringify({ ...marker, settings: {}, keys: { KUVO_KEY_NOTION_TOKEN: "seed-value" } }));
    assert.equal(accepted.status, 0, accepted.stderr);
    assert.equal(JSON.parse(accepted.stdout).seeded, true);
    assert.equal(fs.readFileSync(path.join(box, "box", "keys", "KUVO_KEY_NOTION_TOKEN"), "utf8").trim(), "seed-value");
    const token = "guarded-" + "a".repeat(48);
    fs.symlinkSync(token, path.join(home, ".kuvo-code-adding"));
    const setup = command(["setup", token], JSON.stringify(marker));
    assert.equal(setup.status, 0, setup.stderr);
    assert.deepEqual(JSON.parse(setup.stdout.trim().split("\n").at(-1)), {});
  }
});

test("installer code rejects an older and future prefix before install makes the Box", () => {
  const { command, box, home, entries } = machine();
  const bundle = path.join(home, "Audit.app");
  fs.mkdirSync(path.join(bundle, "Contents"), { recursive: true });
  fs.writeFileSync(path.join(bundle, "Contents", "Info.plist"), '<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.code.audit</string></dict></plist>');
  const before = entries();
  for (const version of [0, 2]) {
    const input = `kuvo-add-${version}.e30\n`;
    // No pack or engine exists. The reader must refuse the input before trying to install.
    const installed = command(["install", "--join", "--root", path.basename(box), "--app", bundle, "--release", "/dev/null", "--engine", "/dev/null"], input);
    assert.equal(installed.status, 1, installed.stderr);
    assert.match(JSON.parse(installed.stdout).message, new RegExp(`kuvo-add format version ${version} is unsupported`));
    const joined = command(["join"], input);
    assert.equal(joined.status, 1, joined.stderr);
    assert.match(joined.stderr, new RegExp(`kuvo-add format version ${version} is unsupported`));
    assert.deepEqual(entries(), before);
  }
});

test("the shell installer refuses future stdin before preparing a pack or engine", () => {
  const { box, home, entries } = machine();
  const bundle = path.join(home, "Audit.app");
  fs.mkdirSync(path.join(bundle, "Contents"), { recursive: true });
  fs.writeFileSync(path.join(bundle, "Contents", "Info.plist"), '<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.code.audit</string></dict></plist>');
  const install = fileURLToPath(new URL("../src/install/kuvo-install", import.meta.url));
  const before = entries();
  const ran = spawnSync("/bin/sh", [install, "join", "--root", path.basename(box), "--app", bundle], {
    env: { ...process.env, HOME: home },
    input: "kuvo-add-2.e30\n", encoding: "utf8", timeout: 15_000,
  });
  assert.equal(ran.status, 1, ran.stderr);
  assert.match(JSON.parse(ran.stdout).message, /Unsupported installer code format/);
  assert.deepEqual(entries(), before);
});

test("repair holds a future record and only replaces the preceding and current formats", () => {
  const { command, run, entries } = machine();
  const record = path.join(run, "repair.json");
  const empty = entries();
  const unsupported = command(["repair", "--format-version", "2"]);
  assert.equal(unsupported.status, 1, unsupported.stderr);
  assert.match(unsupported.stderr, /Repair request format version 2 is unsupported/);
  assert.deepEqual(entries(), empty);
  const future = '{"formatVersion":2,"result":"damaged","at":1,"next":{"keep":true}}\n';
  for (const [bytes, message] of [[future, /Repair record format version 2 is unsupported/], ['{"result":', /repair record is unreadable/]]) {
    fs.writeFileSync(record, bytes);
    const before = entries();
    const refused = command(["repair"]);
    assert.equal(refused.status, 1, refused.stderr);
    assert.match(refused.stderr, message);
    assert.deepEqual(entries(), before);
    assert.equal(fs.readFileSync(record, "utf8"), bytes);
  }
  for (const marker of [{}, { formatVersion: 1 }]) {
    fs.writeFileSync(record, JSON.stringify({ ...marker, result: "none", why: "busy", at: 1 }));
    const repaired = command(marker.formatVersion ? ["repair", "--format-version", "1"] : ["repair"]);
    assert.equal(repaired.status, 0, repaired.stderr);
    assert.equal(JSON.parse(repaired.stdout).formatVersion, 1);
    assert.equal(JSON.parse(fs.readFileSync(record, "utf8")).formatVersion, 1);
  }
});

test("interrupted machine-helper recovery keeps future and truncated records before launch work", () => {
  const { command, run, entries } = machine();
  const file = path.join(run, "machine-helper-job.json");
  const job = { phase: "waiting", owner: process.pid, plist: null, loaded: false, legacy: false };
  for (const marker of [{}, { formatVersion: 1 }]) {
    fs.writeFileSync(file, JSON.stringify({ ...job, ...marker }));
    const read = command(["machine-job"]);
    assert.equal(read.status, 0, read.stderr);
    assert.equal(read.stdout.trim(), `v1 waiting ${process.pid} fresh`);
  }
  const before = entries();
  for (const text of [JSON.stringify({ ...job, formatVersion: 2 }), '{"phase":"waiting",']) {
    fs.writeFileSync(file, text);
    const inspect = command(["machine-job"]);
    assert.equal(inspect.status, 1);
    assert.equal(inspect.stdout, "");
    const refused = command(["engine-swap", "--machine-helper-recover"]);
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /Machine helper job format version 2 is unsupported|JSON/);
    assert.deepEqual(entries(), before);
    assert.equal(fs.readFileSync(file, "utf8"), text);
  }
});


test("helper serve stdin rejects a future version before it takes secrets or starts a service", () => {
  const { command, entries } = machine();
  const before = entries();
  const served = command(["serve"], '{"formatVersion":2,"password":"disposable","hostVaultSecret":"disposable"}\n');
  assert.equal(served.status, 1, served.stderr);
  assert.match(served.stderr, /Box serve input format version 2 is unsupported/);
  assert.deepEqual(entries(), before);
  for (const input of ["{}\n", '{"formatVersion":1}\n']) {
    const missing = command(["serve"], input);
    assert.equal(missing.status, 2, missing.stderr);
    assert.match(missing.stderr, /serve answers only with a password/);
    assert.deepEqual(entries(), before);
  }
});


test("the actual audit predecessor reads additive settings and repair data on rollback", t => {
  const prior = spawnSync("git", ["show", "0dc01540a:swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"], {
    cwd: fileURLToPath(new URL("../..", import.meta.url)), maxBuffer: 16 * 1024 * 1024,
  });
  if (prior.status !== 0) return t.skip("the audit predecessor must be available in this checkout");
  const { box, home, run, command } = machine();
  const oldLink = path.join(home, "preceding-link.mjs");
  fs.writeFileSync(oldLink, prior.stdout);

  const adopted = command(["box", "adopt"], JSON.stringify({ formatVersion: 1, settings: {}, keys: { KUVO_KEY_NOTION_TOKEN: "rollback-key" } }), oldLink);
  assert.equal(adopted.status, 0, adopted.stderr);
  assert.equal(JSON.parse(adopted.stdout).seeded, true);
  const key = path.join(box, "box", "keys", "KUVO_KEY_NOTION_TOKEN");
  assert.equal(fs.readFileSync(key, "utf8").trim(), "rollback-key");

  const repaired = command(["repair"]);
  assert.equal(repaired.status, 0, repaired.stderr);
  const record = path.join(run, "repair.json");
  assert.equal(JSON.parse(fs.readFileSync(record, "utf8")).formatVersion, 1);
  const rolledBack = command(["repair"], "", oldLink);
  assert.equal(rolledBack.status, 0, rolledBack.stderr);
  assert.equal(JSON.parse(rolledBack.stdout).why, "noRelease");
  assert.equal(JSON.parse(fs.readFileSync(record, "utf8")).formatVersion, undefined);
  const recovered = command(["repair"]);
  assert.equal(recovered.status, 0, recovered.stderr);
  assert.equal(JSON.parse(fs.readFileSync(record, "utf8")).formatVersion, 1);
  assert.equal(fs.readFileSync(key, "utf8").trim(), "rollback-key");
});


test("install holds both kinds of future making marker before status recovery", () => {
  const { command, box, home, entries } = machine();
  const bundle = path.join(home, "Audit.app");
  fs.mkdirSync(path.join(bundle, "Contents"), { recursive: true });
  fs.writeFileSync(path.join(bundle, "Contents", "Info.plist"), '<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.code.audit</string></dict></plist>');
  for (const relative of ["updates/install/making", "updates/rehome/new"]) {
    const record = path.join(box, relative);
    fs.mkdirSync(path.dirname(record), { recursive: true });
    for (const marker of ["1\n", '{"at":1,"formatVersion":1}']) {
      fs.writeFileSync(record, marker);
      const status = command(["install", "--status", "--root", path.basename(box), "--app", bundle]);
      assert.equal(status.status, 0, status.stderr);
    }
    const future = '{"at":1,"formatVersion":2,"extra":{"keep":true}}';
    fs.writeFileSync(record, future);
    const before = entries();
    const status = command(["install", "--status", "--root", path.basename(box), "--app", bundle]);
    assert.equal(status.status, 1, status.stderr);
    assert.match(JSON.parse(status.stdout).message, /Interrupted installation marker format version 2 is unsupported/);
    assert.deepEqual(entries(), before);
    assert.equal(fs.readFileSync(record, "utf8"), future);
    fs.unlinkSync(record);
  }
});

test("a retired Mac Box holds a future served marker before startup", () => {
  const { home } = machine();
  const appBox = path.join(home, "retired", "box");
  const run = path.join(appBox, "run");
  fs.mkdirSync(run, { recursive: true });
  const marker = path.join(run, "served.json");
  const bytes = '{"formatVersion":2,"app":"0.7.0","bridge":"earlier"}';
  fs.writeFileSync(marker, bytes);
  const ran = spawnSync(process.execPath, [script, "serve"], {
    env: { ...process.env, HOME: home, KUVO_HOME: appBox, KUVO_BOX: "mac", KUVO_ENGINE_CONFIG: path.join(appBox, "config") },
    input: '{"password":"disposable"}\n', encoding: "utf8", timeout: 15_000,
  });
  assert.equal(ran.status, 1, ran.stderr);
  assert.match(ran.stderr, /Legacy Mac start marker format version 2 is unsupported/);
  assert.deepEqual(fs.readdirSync(appBox), ["run"]);
  assert.deepEqual(fs.readdirSync(run), ["served.json"]);
  assert.equal(fs.readFileSync(marker, "utf8"), bytes);
});

test("updated supervision readers hold future records before recovery or durable writes", () => {
  const cases = [
    ["run/bridge.json", ["supervision-record", "bridge"], { pid: process.pid }, /Coding agents bridge rendezvous/],
    ["run/engine-swap.json", ["engine-swap"], { from: "1", to: "2", pid: process.pid, stage: "waiting" }, /Engine swap record/],
    ["run/update-engine-hold.json", ["engine-swap"], { from: "1", to: "2", pid: process.pid, stage: "waiting" }, /Engine swap record/],
    ["run/helper-handover.json", ["helper", "hand-over", "--pid", String(process.pid)], { bundle: "test", v: 2 }, /Helper handover/],
    ["run/update.json", ["update", "--settle"], { trial: { to: "1" } }, /Box update state/],
    ["updates/installed.json", ["supervision-record", "rollback"], { version: "1", files: {} }, /Installed Box receipt/],
  ];
  for (const [relative, args, fields, format] of cases) {
    const { box, command, entries } = machine();
    const file = path.join(box, relative);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const bytes = JSON.stringify({ ...fields, formatVersion: 2 });
    fs.writeFileSync(file, bytes);
    const before = entries();
    const refused = command(args);
    assert.equal(refused.status, 1, `${relative}: ${refused.stderr}`);
    assert.match(refused.stderr, format);
    assert.deepEqual(entries(), before);
    assert.equal(fs.readFileSync(file, "utf8"), bytes);
  }
});

test("rehome refuses future action and state records before starting backout", () => {
  for (const [name, fields, format] of [
    ["state.json", { state: "moving" }, /Rehome state/],
    ["journal.acts", { act: "made", path: "/unused", folder: false }, /Rehome action/],
  ]) {
    const { box, command, entries } = machine();
    const records = path.join(box, "updates", "rehome");
    fs.mkdirSync(records, { recursive: true });
    const file = path.join(records, name);
    for (const formatVersion of [undefined, 1]) {
      fs.writeFileSync(file, JSON.stringify({ ...fields, formatVersion }) + "\n");
      const result = command(["rehome", path.basename(box), "--resume"]);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(JSON.parse(result.stdout).result, "nothing");
    }
    const bytes = JSON.stringify({ ...fields, formatVersion: 2 }) + "\n";
    fs.writeFileSync(file, bytes);
    const before = entries();
    const refused = command(["rehome", path.basename(box), "--resume"]);
    assert.equal(refused.status, 1, refused.stderr);
    assert.match(JSON.parse(refused.stdout).message, format);
    assert.deepEqual(entries(), before);
    assert.equal(fs.readFileSync(file, "utf8"), bytes);
  }
});
