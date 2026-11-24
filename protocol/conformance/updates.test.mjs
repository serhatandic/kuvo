// The update command admits only signed manifests and checks the selected pack before staging.
// Each scenario uses freshly generated test signing keys and isolated local packs.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { Worker } from "node:worker_threads";
import { gzipSync } from "node:zlib";

import { boxBinary, isolatedEnv, launch, readJSON, relayAddress, request, scratch, startBox, typescriptRuntime, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import "./update-helper.mjs";

const keypair = () => {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  return {
    privateKey,
    key: publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64url"),
  };
};
const digest = bytes => createHash("sha256").update(bytes).digest("hex");
const encode = body => Buffer.from(JSON.stringify(body));
const envelope = (body, signer, claimed = signer.key) => ({
  key: claimed,
  manifest: encode(body).toString("base64"),
  signature: sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), encode(body)]), signer.privateKey).toString("base64"),
});

function privateWindowsHome(home) {
  if (process.platform !== "win32") return;
  fs.mkdirSync(home, { recursive: true });
  const command = Buffer.from(`
    $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $acl = New-Object System.Security.AccessControl.DirectorySecurity
    $acl.SetSecurityDescriptorSddlForm(('O:{0}D:P(A;OICI;FA;;;{0})' -f $sid))
    Set-Acl -LiteralPath $env:KUVO_FIXTURE_HOME -AclObject $acl -ErrorAction Stop
  `, "utf16le").toString("base64");
  const done = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-EncodedCommand", command], {
    env: { ...process.env, KUVO_FIXTURE_HOME: home },
    encoding: "utf8", timeout: 15000,
  });
  assert.equal(done.status, 0, done.error?.message ?? done.stderr);
}

function box(t) {
  const binary = boxBinary();
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, "update-home");
  const release = path.join(root, "release");
  const key = keypair();
  if (!binary.typescript) privateWindowsHome(home);
  write(path.join(home, "bin/box-version"), "0.8.0-beta.1\n");
  const body = {
    kuvo: "box/1", version: "0.8.0", engine: "2.2.6",
    spreads: true, pack: { url: "https://example.invalid/box.tar.gz", sha256: digest("pack") },
  };
  function update(signed, pack = "pack", implementation = binary) {
    write(path.join(release, "boxes.json"), JSON.stringify(signed));
    write(path.join(release, "box.tar.gz"), pack);
    const args = ["update", "--phase", "prepare", "--from", release, "--key", key.key];
    const run = spawnSync(implementation.typescript ? typescriptRuntime() : implementation.file,
      implementation.typescript ? [`--config=${process.platform === "win32" ? "NUL" : "/dev/null"}`, "--no-env-file", implementation.file, ...args] : args, {
        cwd: home, env: { ...env, KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" },
        encoding: "utf8", timeout: 90000,
      });
    if (run.error) throw run.error;
    assert.equal(run.status, 0, run.stdout + run.stderr);
    return JSON.parse(run.stdout);
  }
  return { binary, home, release, key, body, update, env: { ...env, KUVO_HOME: home } };
}

test("an update from a stranger or with a changed signature cannot stage code", t => {
  const { home, key, body, update } = box(t);
  const untrusted = keypair();
  assert.deepEqual(update(envelope(body, untrusted)), {
    result: "failed", message: "It isn't signed with a key this machine trusts.",
  });
  assert.deepEqual(update(envelope(body, untrusted, key.key)), {
    result: "failed", message: "Its signature doesn't match.",
  });
  const edited = envelope(body, key);
  edited.manifest = encode({ ...body, version: "0.9.0" }).toString("base64");
  assert.deepEqual(update(edited), { result: "failed", message: "Its signature doesn't match." });
  assert.equal(fs.existsSync(path.join(home, "versions")), false);
  assert.equal(fs.existsSync(path.join(home, "run/fleet-ready.json")), false);
});

test("the signed digest rejects a changed pack and preserves the selected release", t => {
  const { home, key, body, update } = box(t);
  assert.deepEqual(update(envelope(body, key), "different bytes"), {
    result: "failed", version: "0.8.0", message: "The release's files aren't what it says.",
  });
  assert.equal(fs.readFileSync(path.join(home, "bin/box-version"), "utf8"), "0.8.0-beta.1\n");
  assert.equal(fs.existsSync(path.join(home, "run/fleet-ready.json")), false);
});

test("an update cannot move to an earlier release or spread a private build", t => {
  const { home, key, body, update } = box(t);
  assert.deepEqual(update(envelope({ ...body, version: "0.7.2" }, key)), {
    result: "current", version: "0.8.0-beta.1",
  });
  assert.deepEqual(update(envelope({ ...body, spreads: false }, key)), {
    result: "failed", message: "This release cannot be shared with your setup.",
  });
  const installed = "0.10.0-beta.10";
  write(path.join(home, "bin/box-version"), installed + "\n");
  write(path.join(home, "updates/installed.json"), encode({
    formatVersion: 1, version: installed, sha256: digest("the installed pack"), files: {},
  }));
  for (const version of ["0.9.99", "0.10.0-beta.9", installed]) {
    assert.deepEqual(update(envelope({ ...body, version }, key)), {
      result: "current", version: installed,
    });
  }
});

test("the update status reports the installed release and a keeper's failed trial", async t => {
  const { computer } = await bench(t);
  const here = await computer("update-status", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  write(path.join(here.home, "bin/box-version"), "0.8.0-beta.1\n");
  write(path.join(here.home, "run/update-cannot"), "1740000000 0.8.0 123456789abc\n");
  write(path.join(here.home, "run/update.json"), '{"formatVersion":1,"latest":"0.8.0"}\n');
  const ready = await here.start().ready();
  const answer = await request(ready, "GET", "kuvo/update");
  assert.equal(answer.status, 200, answer.text);
  assert.deepEqual(answer.json, {
    setUp: true, version: "0.8.0-beta.1", engine: here.engine.record.version, latest: "0.8.0",
    last: {
      at: 1740000000000, result: "failed", to: "0.8.0", message: "0.8.0 didn't start here",
    },
    updating: null, blocked: "Only a machine's own Kuvo Code folder updates itself.",
  });
  const refused = await request(ready, "POST", "kuvo/update", { now: true });
  assert.equal(refused.status, 409, refused.text);
  assert.deepEqual(refused.json, { message: answer.json.blocked });
});

test("a member asks its main to check the release feed now or when it returns", async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform === "win32") return t.skip("This uses the Rust resident in a disposable Unix account.");
  const { computer, relay, leases } = await bench(t);
  const [main, member] = await Promise.all([computer("Main"), computer("Member")]);
  const old = main.start();
  let mainKey, mainAddress;
  try {
    const ready = await old.ready();
    const setup = await request(ready, "GET", "kuvo/setup");
    mainKey = setup.json.you.key;
    const identity = readJSON(path.join(main.home, "run/link.json"));
    mainAddress = relayAddress(Buffer.from(identity.secret, "base64url"));
    await until(() => leases.leaseFor(setup.json.setup), "the main must hold a lease");
    const invite = await request(ready, "POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
    assert.equal(invite.status, 201, invite.text);
    assert.equal((await member.join(invite.json.code)).code, 0);
  } finally {
    await old.stop();
  }

  const key = keypair();
  let checked = 0;
  const feed = createServer((_req, res) => {
    checked += 1;
    res.end(encode(envelope({ kuvo: "box/1", version: "0.8.0", engine: "2.2.6", spreads: true,
      pack: { url: "https://example.invalid/current.tar.gz", sha256: digest("current") } }, key)));
  });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const start = peer => {
    const home = path.join(peer.env.HOME, ".kuvo-code");
    if (!fs.existsSync(home)) {
      fs.renameSync(peer.home, home);
      write(path.join(home, "bin/box-version"), "0.8.0\n");
    }
    return startBox({ binary, home, relay, env: { ...peer.env,
      KUVO_TEST_RELEASE_FEED: `http://127.0.0.1:${feed.address().port}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key,
    } });
  };
  const primary = start(main);
  let other;
  try {
    other = start(member);
    const [first, second] = await Promise.all([primary.ready(), other.ready()]);
    await until(async () => (await request(first, "GET", "kuvo/update")).json.latest === "0.8.0", "the first check must finish");
    assert.equal(checked, 1);
    const asked = await request(second, "POST", "kuvo/update", { now: true });
    assert.equal(asked.status, 202, asked.text);
    assert.match(asked.json.message, /is checking for updates\.$/);
    await until(() => checked === 2, "the main must check upon the member's authenticated request");

    const run = path.join(member.env.HOME, ".kuvo-code/run");
    const carrier = path.join(run, "fleet-check.json");
    const reach = path.join(run, "peer-reach.json");
    const memberLink = readJSON(path.join(run, "link.json"));
    const memberAddress = relayAddress(Buffer.from(memberLink.secret, "base64url"));
    const resume = relay.pause(mainAddress, memberAddress);
    const beforeReach = fs.existsSync(reach) ? readJSON(reach) : {};
    try {
      // Preserve the interval after the peer receiver writes the observation
      // and before its new in-memory view is available to the updater.
      write(reach, encode({ ...beforeReach, [mainKey]: Date.now() }));
      const pending = await request(second, "POST", "kuvo/update", { now: true });
      assert.equal(pending.status, 202, pending.text);
      assert.match(pending.json.message, /checks for updates when .+ is back\.$/);
      assert.equal(typeof readJSON(carrier).at, "number");
    } finally {
      fs.rmSync(carrier, { force: true });
      write(reach, encode(beforeReach));
      resume();
    }
    const reconnect = relay.disconnect(mainAddress);
    try {
      await until(() => fs.existsSync(reach) && readJSON(reach)[mainKey], "the member must detect that its main is away");
      const pending = await request(second, "POST", "kuvo/update", { now: true });
      assert.equal(pending.status, 202, pending.text);
      assert.match(pending.json.message, /checks for updates when .+ is back\.$/);
      assert.equal(typeof readJSON(carrier).at, "number");
      assert.equal(checked, 2);
      await other.stop();
      other = start(member);
      await other.ready();
      assert.equal(typeof readJSON(carrier).at, "number");
    } finally {
      reconnect();
    }
    await until(() => checked === 3 && !fs.existsSync(carrier), "the kept request must reach the main after it reconnects");
  } finally {
    await other?.stop();
    await primary.stop();
  }
});

test("a discarded release signature does not stop the next signed feed check", async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform === "win32") return t.skip("This uses a Rust resident in a disposable Unix account.");
  const { computer } = await bench(t);
  const here = await computer("Standalone");
  const key = keypair();
  const signed = (version, signer) => envelope({
    kuvo: "box/1", version, engine: "2.2.6", spreads: true,
    pack: { url: "https://example.invalid/current.tar.gz", sha256: digest("current") },
  }, signer);
  write(path.join(here.home, "bin/box-version"), "0.8.0\n");
  write(path.join(here.home, "updates/release/boxes.json"), encode(signed("0.9.0", keypair())));
  const feed = createServer((_req, res) => res.end(encode(signed("0.8.0", key))));
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const child = here.start(binary, {
    KUVO_TEST_RELEASE_FEED: `http://127.0.0.1:${feed.address().port}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key,
  });
  const resident = await child.ready();
  const update = () => request(resident, "GET", "kuvo/update");
  await until(async () => (await update()).json?.latest === "0.8.0", () => `the valid feed should still be checked\n${child.log()}`);
  assert.equal((await update()).json.version, "0.8.0");
  assert.equal(fs.existsSync(path.join(here.home, "run/fleet-ready.json")), false);
});

test("a machine refuses the Mac Helper's Stop request", async t => {
  const { computer } = await bench(t);
  const here = await computer("stop-machine", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const ready = await here.start().ready();
  const answer = await request(ready, "POST", "kuvo/box/stop", {});
  assert.equal(answer.status, 409, answer.text);
  assert.deepEqual(answer.json, { message: "Only Kuvo on a Mac, run by Kuvo Helper, is stopped this way." });
  assert.equal(fs.existsSync(path.join(here.home, "run/paused")), false);
});

const targets = {
  "darwin:arm64": "aarch64-apple-darwin",
  "darwin:x64": "x86_64-apple-darwin",
  "linux:arm64": "aarch64-unknown-linux-musl",
  "linux:x64": "x86_64-unknown-linux-musl",
  "win32:x64": "x86_64-pc-windows-gnullvm",
};
const target = targets[`${process.platform}:${process.arch}`];

async function engineFeed(t, release, fixture) {
  const executable = process.platform === "win32" ? "opencode.exe" : "opencode";
  const source = path.join(release, "engine-source");
  const engine = path.join(source, "package/bin", executable);
  if (fixture) {
    write(engine, fixture, { mode: 0o700 });
  } else if (process.platform === "win32") {
    fs.mkdirSync(path.dirname(engine), { recursive: true });
    fs.copyFileSync(process.execPath, engine);
  } else {
    write(engine, '#!/bin/sh\n[ "$1" = --version ]\n', { mode: 0o700 });
  }
  const archive = path.join(release, "engine.tar.gz");
  const packed = spawnSync("tar", ["czf", archive, "-C", source, "package"], { encoding: "utf8", timeout: 90000 });
  assert.equal(packed.status, 0, packed.stderr);
  const calls = path.join(release, "engine-downloads");
  const feed = new Worker(String.raw`
    const { createServer } = require("node:http");
    const { appendFileSync, createReadStream } = require("node:fs");
    const { parentPort, workerData } = require("node:worker_threads");
    let requests = 0;
    const server = createServer((request, response) => {
      if (request.url !== "/engine.tar.gz") { response.writeHead(404).end(); return; }
      appendFileSync(workerData.calls, "download\n");
      if (++requests === 1 && workerData.damageFirst) { response.end("incomplete download"); return; }
      createReadStream(workerData.archive).pipe(response);
    });
    server.listen(0, "127.0.0.1", () => parentPort.postMessage(server.address().port));
  `, { eval: true, workerData: { archive, calls, damageFirst: !fixture } });
  t.after(() => feed.terminate());
  const [port] = await once(feed, "message");
  return {
    calls, executable, engine,
    build: {
      url: `http://127.0.0.1:${port}/engine.tar.gz`,
      integrity: "sha512-" + createHash("sha512").update(fs.readFileSync(archive)).digest("base64"),
    },
  };
}

test("Rust prepares its target without changing the selected release and its keeper can start it", async t => {
  const { binary, home, key, release, body, update, env } = box(t);
  if (binary.typescript) return t.skip("A TypeScript stager still reads the additive manifest's legacy pack.");
  if (!target) return t.skip("This platform has no Rust release pack.");
  const executable = process.platform === "win32" ? "bin/kuvo-box.exe" : "bin/kuvo-box";
  const packRoot = path.join(release, "source");
  const version = "0.8.0";
  const members = { [executable]: fs.readFileSync(binary.file), "bin/box-version": Buffer.from(version + "\n") };
  for (const [name, bytes] of Object.entries(members)) write(path.join(packRoot, name), bytes, { mode: 0o700 });
  const engine = await engineFeed(t, release);
  const metadata = path.join(packRoot, "box.json");
  const system = { darwin: "darwin", linux: "linux", win32: "windows" }[process.platform];
  const architecture = process.arch === "arm64" ? "aarch64" : "x86_64";
  write(metadata, encode({
    kuvo: "box/1", version, launch: "rust", engine: body.engine,
    files: Object.fromEntries(Object.entries(members).map(([name, bytes]) => [name, digest(bytes)])),
    [system]: { [architecture]: engine.build },
  }));
  const archive = (tree, name) => {
    const tar = path.join(release, `${name}.tar`);
    const archived = spawnSync("tar", ["cf", tar, "-C", tree, "box.json", "bin"], { encoding: "utf8", timeout: 90000 });
    assert.equal(archived.status, 0, archived.stderr);
    const bytes = gzipSync(fs.readFileSync(tar), { level: 1 });
    write(path.join(release, `${name}.tar.gz`), bytes);
    return digest(bytes);
  };
  const file = path.join(release, "target.tar.gz");
  const hash = archive(packRoot, "target");
  const legacy = path.join(release, "legacy.tar.gz");
  write(legacy, "pack");
  const signer = fileURLToPath(new URL("../../scripts/sign-box.mjs", import.meta.url));
  const entries = Object.values(targets).flatMap(name => [
    "--target", name, name === target ? file : legacy, `https://example.invalid/${name === target ? "target" : name}.tar.gz`,
  ]);
  const signing = spawnSync(process.execPath, [
    signer, version, legacy, body.pack.url, metadata, ...entries,
  ], {
    env: { ...isolatedEnv(release), KUVO_BOX_SPREADS: "1", KUVO_BOX_SIGNING_KEY: key.privateKey.export({ type: "pkcs8", format: "pem" }) },
    encoding: "utf8", timeout: 15000,
  });
  assert.equal(signing.status, 0, signing.stderr);
  const signed = JSON.parse(signing.stdout);
  const manifest = JSON.parse(Buffer.from(signed.manifest, "base64"));
  assert.equal(manifest.pack.sha256, body.pack.sha256);
  assert.deepEqual(Object.keys(manifest.packs).sort(), Object.values(targets).sort());
  const prepared = path.join(home, "versions", `${version}-${hash.slice(0, 12)}`);
  function keep(...args) {
    const called = spawnSync(binary.file, ["keep", ...args], { env, cwd: home, encoding: "utf8", timeout: 15000 });
    assert.equal(called.status, 0, called.error?.message ?? called.stderr);
  }
  const initial = "versions/0.8.0-beta.1-000000000000";
  const accountEntries = fs.readdirSync(env.HOME).sort();
  const tempEntries = fs.readdirSync(env.TMPDIR).sort();
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, initial, "box.json"), encode({ kuvo: "box/1", version: "0.8.0-beta.1", launch: "rust" }));
  write(path.join(home, initial, executable), members[executable], { mode: 0o700 });
  keep("select", `${initial}/bin`);
  let offer;
  let keeperEnv = env;
  if (process.platform === "win32") {
    const served = new Promise(resolve => { offer = resolve; });
    const feed = createServer(async (_req, res) => {
      await served;
      res.end(encode(signed));
    });
    feed.listen(0, "127.0.0.1");
    await once(feed, "listening");
    t.after(() => {
      offer();
      feed.close();
      feed.closeAllConnections();
    });
    keeperEnv = {
      ...env, KUVO_TEST_RELEASE_FEED: `http://127.0.0.1:${feed.address().port}/boxes.json`,
      KUVO_TEST_RELEASE_KEY: key.key,
    };
  }
  const keeper = launch(binary.file, ["keep"], keeperEnv, home);
  const serveRecord = path.join(home, "run/serve.json");
  async function readiness(previous) {
    return until(async () => {
      if (!fs.existsSync(serveRecord)) return;
      const record = readJSON(serveRecord);
      if (record.generation === previous) return;
      const reply = await request(record, "GET", "kuvo/info").catch(() => null);
      return reply?.status === 200 ? record : false;
    }, () => {
      const log = path.join(home, "run/serve.log");
      return `the selected release should answer kuvo/info\n${fs.existsSync(log) ? fs.readFileSync(log, "utf8") : keeper.log()}`;
    }, 25000);
  }
  try {
    const previous = await readiness();
    const installedEngine = path.join(home, "engine", body.engine, engine.executable);
    assert.deepEqual(update(signed), { result: "failed", version, message: "The engine isn't what the release says." });
    assert.equal(fs.existsSync(installedEngine), false);
    assert.equal(fs.existsSync(path.join(home, "run/fleet-ready.json")), false);
    assert.equal(fs.existsSync(prepared), false);
    const brokenRoot = path.join(release, "failed-probe");
    const broken = process.platform === "win32" ? fs.readFileSync(process.execPath) : Buffer.from("#!/bin/sh\nexit 1\n");
    write(path.join(brokenRoot, executable), broken, { mode: 0o700 });
    write(path.join(brokenRoot, "bin/box-version"), members["bin/box-version"]);
    const contents = readJSON(metadata);
    contents.files[executable] = digest(broken);
    write(path.join(brokenRoot, "box.json"), encode(contents));
    const unusable = { url: "https://example.invalid/failed-probe.tar.gz", sha256: archive(brokenRoot, "failed-probe") };
    const unusableRelease = envelope({ ...manifest, packs: { ...manifest.packs, [target]: unusable } }, key);
    const brokenResult = update(unusableRelease);
    assert.equal(brokenResult.result, "failed");
    assert.match(brokenResult.message, /The new box doesn't run with its engine here\./);
    assert.equal(fs.existsSync(installedEngine), false);
    assert.equal(fs.existsSync(path.join(home, "run/fleet-ready.json")), false);
    assert.equal(readJSON(serveRecord).generation, previous.generation);
    const start = performance.now();
    for (let retry = 0; retry < 2; retry++) {
      assert.deepEqual(update(signed), { result: "waiting", version });
      assert.equal(fs.readFileSync(path.join(home, "active-release"), "utf8"), `${initial}/bin`);
      assert.equal(readJSON(serveRecord).generation, previous.generation);
      assert.deepEqual(readJSON(path.join(home, "run/fleet-ready.json")), { version, sha256: hash });
      assert.equal(readJSON(path.join(prepared, "box.json")).launch, "rust");
      assert.equal(digest(fs.readFileSync(path.join(prepared, executable))), digest(members[executable]));
    }
    t.diagnostic(`prepared and retried in ${Math.round(performance.now() - start)} ms`);
    assert.equal(fs.readFileSync(engine.calls, "utf8"), "download\ndownload\ndownload\n");
    assert.equal(digest(fs.readFileSync(installedEngine)), digest(fs.readFileSync(engine.engine)));
    const selected = `versions/${path.basename(prepared)}/bin`;
    const switching = performance.now();
    if (offer) offer();
    else keep("select", selected);
    await readiness(previous.generation);
    await until(() => !fs.existsSync(path.join(home, "run/previous")), "the keeper should confirm the trial");
    assert.equal(fs.readFileSync(path.join(home, "active-release"), "utf8"), selected);
    t.diagnostic(`selected and confirmed in ${Math.round(performance.now() - switching)} ms`);
  } finally {
    try {
      keep("off");
      await until(() => keeper.child.exitCode !== null, () => keeper.log(), 15000);
    } finally {
      await keeper.stop();
    }
  }
  write(path.join(home, "desktop/identity.json"), "{}");
  const uninstall = spawnSync(binary.file, ["uninstall"], { env, cwd: home, encoding: "utf8", timeout: 15000 });
  assert.equal(uninstall.status, 0, uninstall.error?.message ?? uninstall.stdout + uninstall.stderr);
  assert.equal(JSON.parse(uninstall.stdout).result, "uninstalled");
  assert.deepEqual(JSON.parse(uninstall.stdout).left, [home]);
  assert.equal(fs.existsSync(path.join(home, "desktop/identity.json")), false);
  assert.deepEqual(fs.readdirSync(env.HOME).sort(), accountEntries);
  assert.deepEqual(fs.readdirSync(env.TMPDIR).sort(), tempEntries);
});

test("the shell keepers wait for a preparing member, then an offline computer catches up", { timeout: 180000 }, t => fleetRun(t, { shellKeepers: true }));
test("a new main finishes a prepared release after the old main goes away", { timeout: 240000 }, t => fleetRun(t, { failover: true }));
test("a TypeScript beta computer that missed the switch catches up to Rust", { timeout: 240000 }, t => fleetRun(t, { returningTypescript: true }));
test("the deployed beta refuses a direct Rust pack", { timeout: 240000 }, t => fleetRun(t, { returningTypescript: true, expectRefusal: true }));
test("the deployed beta takes the bridge before joining a Rust fleet", { timeout: 360000 }, t => fleetRun(t, { returningTypescript: true, bridgeBeta: true }));

test("the shell keeper records a failed first Rust launch without selecting TypeScript again", async t => {
  if (boxBinary().typescript || process.platform === "win32") {
    return t.skip("This boundary is the Unix keeper's one-time Rust switch.");
  }
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const starter = fs.readFileSync(fileURLToPath(new URL("../../box/src/keeper/kuvo-start", import.meta.url)), "utf8")
    .replaceAll("@ENGINE@", "2.2.6");
  const previous = "versions/0.8.0-beta.1-000000000000/bin";
  const selected = "versions/0.8.0-123456789abc/bin";
  for (const relative of [previous, selected]) {
    write(path.join(home, relative, "kuvo-start"), starter, { mode: 0o700 });
  }
  write(path.join(home, selected, "..", "box.json"), '{"launch":"rust"}');
  fs.symlinkSync(selected, path.join(home, "bin"));
  const keep = path.join(home, "bin/kuvo-start");
  write(path.join(home, "run/previous"), previous + "\n");
  const tools = path.join(root, "tools");
  write(path.join(tools, "date"), `#!/bin/sh
if [ -e "$KUVO_FIXTURE_CLOCK" ]; then echo 1121; else touch "$KUVO_FIXTURE_CLOCK"; echo 1000; fi
`, { mode: 0o700 });
  write(path.join(tools, "sync"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  const keeper = launch(process.execPath, ["-e", "process.stdin.resume()", keep], env, home);
  try {
    await until(() => spawnSync("/bin/ps", ["-ww", "-o", "command=", "-p", String(keeper.child.pid)], {
      encoding: "utf8",
    }).stdout?.includes(keep), "the stand-in keeper must own the trial");
    write(path.join(home, "run/box.pid"), String(keeper.child.pid) + "\n");
    const judged = spawnSync("/bin/sh", [keep, "start-guard", selected, String(keeper.child.pid)], {
      cwd: home,
      env: { ...env, PATH: `${tools}:${env.PATH}`, KUVO_FIXTURE_CLOCK: path.join(root, "clock") },
      encoding: "utf8", timeout: 10000,
    });
    assert.equal(judged.status, 1, judged.error?.message ?? judged.stdout + judged.stderr);
    assert.equal(fs.readlinkSync(path.join(home, "bin")), selected);
    assert.equal(fs.readFileSync(path.join(home, "run/update-cannot"), "utf8"), "1121 0.8.0 123456789abc\n");
    assert.equal(fs.existsSync(path.join(home, "run/previous")), false);
  } finally {
    await keeper.stop();
  }
});

async function fleetRun(t, { failover, returningTypescript, shellKeepers, bridgeBeta, expectRefusal }) {
  const binary = boxBinary();
  const windows = process.platform === "win32";
  if (binary.typescript || (windows && !failover)) {
    return t.skip("This fixture needs the corresponding Rust keeper.");
  }
  const deployed = bridgeBeta || expectRefusal;
  const typescript = deployed ? process.env.KUVO_CONFORMANCE_DEPLOYED_TS_BIN : process.env.KUVO_CONFORMANCE_TS_BIN;
  const previousKeeper = deployed && process.env.KUVO_CONFORMANCE_DEPLOYED_TS_KEEPER;
  const previousVersion = deployed && "0.8.0-beta.48";
  const bridgeLink = process.env.KUVO_CONFORMANCE_TS_BIN;
  const bridgeAgents = process.env.KUVO_CONFORMANCE_TS_AGENTS;
  if (deployed && (!typescript || !previousKeeper)) return t.skip("Supply the deployed beta bundle and keeper.");
  if (returningTypescript && !typescript) return t.skip("Supply the TypeScript bundle.");
  if (bridgeBeta && (!bridgeLink || !bridgeAgents)) return t.skip("Supply the bridge beta's link and agents bundles.");
  const { release, key, body } = box(t);
  const { computer, leases, relay } = await bench(t);
  const create = name => computer(name, undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const [main, member, away] = await Promise.all([create("Studio"), create("Lab"), create("Away")]);
  const old = main.start();
  const before = main.connect(await old.ready());
  const setup = await before("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  await until(() => leases.leaseFor(setup.json.setup), "the main must hold a relay lease");
  for (const peer of [member, away]) {
    const invite = await before("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
    assert.equal(invite.status, 201, invite.text);
    const joined = await peer.join(invite.json.code);
    assert.equal(joined.code, 0, joined.log);
  }
  await until(async () => (await before("GET", "kuvo/setup")).json.computers.length === 3,
    "enrollment should have reached the main");
  await old.stop();

  const version = "0.8.0";
  const program = windows ? "kuvo-box.exe" : "kuvo-box";
  const executable = fs.readFileSync(binary.file);
  const packRoot = path.join(release, "fleet-source");
  const keeperFile = name => fileURLToPath(new URL(`../../box/src/keeper/${name}`, import.meta.url));
  const starter = fs.readFileSync(keeperFile("kuvo-start"), "utf8").replaceAll("@ENGINE@", body.engine);
  const setsid = fs.readFileSync(keeperFile("kuvo-setsid"));
  const launcher = bridgeBeta && fs.readFileSync(keeperFile("kuvo-launch"), "utf8").replaceAll("@ENGINE@", body.engine);
  const files = {
    [`bin/${program}`]: executable,
    "bin/box-version": Buffer.from(version + "\n"),
    ...(!windows && { "bin/kuvo-start": starter, "bin/kuvo-setsid": setsid }),
  };
  for (const [file, bytes] of Object.entries(files)) write(path.join(packRoot, file), bytes, { mode: 0o700 });
  fs.mkdirSync(path.join(packRoot, "bin/opencode-plugins"));
  write(path.join(packRoot, "box.json"), encode({
    kuvo: "box/1", version, launch: "rust", engine: body.engine,
    files: Object.fromEntries(Object.entries(files).map(([file, bytes]) => [file, digest(bytes)])),
  }));
  const packFile = path.join(release, "fleet.tar");
  const archived = spawnSync("tar", ["cf", packFile, "-C", packRoot, "box.json", "bin"], { encoding: "utf8" });
  assert.equal(archived.status, 0, archived.stderr);
  const pack = gzipSync(fs.readFileSync(packFile), { level: 1 });
  const hash = digest(pack);
  const selected = `versions/${version}-${hash.slice(0, 12)}/bin`;
  const bridgeVersion = "0.8.0-beta.49";
  let bridgePack, bridgeHash;
  if (bridgeBeta) {
    const tree = path.join(release, "bridge-source");
    const files = {
      "bin/kuvo-start": starter, "bin/kuvo-setsid": setsid, "bin/kuvo-launch": launcher,
      "bin/box-version": bridgeVersion + "\n",
      "bin/kuvo-link.js": fs.readFileSync(bridgeLink), "bin/kuvo-agents.js": fs.readFileSync(bridgeAgents),
    };
    for (const [file, bytes] of Object.entries(files)) write(path.join(tree, file), bytes, { mode: 0o700 });
    write(path.join(tree, "box.json"), encode({
      kuvo: "box/1", version: bridgeVersion, engine: body.engine,
      files: Object.fromEntries(Object.entries(files).map(([file, bytes]) => [file, digest(bytes)])),
    }));
    const archive = path.join(release, "bridge.tar");
    const done = spawnSync("tar", ["cf", archive, "-C", tree, "box.json", "bin"], { encoding: "utf8" });
    assert.equal(done.status, 0, done.stderr);
    bridgePack = gzipSync(fs.readFileSync(archive), { level: 1 });
    bridgeHash = digest(bridgePack);
  }
  const waiting = [];
  let hold = true;
  let retryDownload = false;
  let peerOnly = false;
  let refusedPacks = 0;
  let refusedBridges = 0;
  let downloads = 0;
  let holdChecks = false;
  let heldChecks = 0;
  let signedBridgeRelease;
  const feed = createServer((req, res) => {
    const url = `http://127.0.0.1:${feed.address().port}/fleet.tar.gz`;
    if (req.url === "/boxes.json") {
      if (holdChecks) {
        heldChecks += 1;
        return;
      }
      const descriptor = { url, sha256: hash };
      const signed = signedBridgeRelease ?? envelope({ ...body, pack: descriptor, launch: "rust", packs: { [target]: descriptor } }, key);
      res.end(encode(signed));
    } else if (req.url === "/fleet.tar.gz") {
      if (peerOnly) {
        refusedPacks += 1;
        res.writeHead(404).end();
      }
      else if (retryDownload) {
        retryDownload = false;
        res.writeHead(503).end();
      } else if (++downloads > 1 && hold) waiting.push(res);
      else res.end(pack);
    } else if (req.url === "/bridge.tar.gz" && bridgePack) {
      if (peerOnly) {
        refusedBridges += 1;
        res.writeHead(404).end();
      } else res.end(bridgePack);
    } else {
      res.writeHead(404).end();
    }
  });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  if (bridgeBeta) {
    const base = `http://127.0.0.1:${feed.address().port}`;
    const bridge = path.join(release, "bridge-boxes.json");
    write(bridge, encode(envelope({ ...body, version: bridgeVersion,
      pack: { url: `${base}/bridge.tar.gz`, sha256: bridgeHash } }, key)));
    const archive = path.join(release, "fleet.tar.gz");
    write(archive, pack);
    const signer = fileURLToPath(new URL("../../scripts/sign-box.mjs", import.meta.url));
    const addresses = Object.values(targets).flatMap(triple => ["--target", triple, archive, `${base}/fleet.tar.gz`]);
    const signed = spawnSync(process.execPath, [signer, version, archive, `${base}/fleet.tar.gz`,
      path.join(packRoot, "box.json"), "--bridge", bridge, ...addresses], {
      env: { ...isolatedEnv(release), KUVO_BOX_SPREADS: "1",
        KUVO_BOX_SIGNING_KEY: key.privateKey.export({ type: "pkcs8", format: "pem" }) },
      encoding: "utf8", timeout: 15000,
    });
    assert.equal(signed.status, 0, signed.error?.message ?? signed.stderr);
    signedBridgeRelease = JSON.parse(signed.stdout);
  }
  const feedURL = `http://127.0.0.1:${feed.address().port}/boxes.json`;
  const env = computer => ({
    ...computer.env, KUVO_HOME: computer.home, KUVO_TEST_RELAY: relay.url, KUVO_RELAY_URL: relay.url,
    KUVO_TEST_RELEASE_FEED: feedURL, KUVO_TEST_RELEASE_KEY: key.key, KUVO_TEST_KEEP_ROUND: "1",
  });
  const legacy = peer => returningTypescript && peer === away;
  const versionBefore = peer => legacy(peer) && previousVersion ? previousVersion : "0.8.0-beta.1";
  const initial = peer => `versions/${versionBefore(peer)}-000000000000/bin`;
  const shell = peer => shellKeepers || legacy(peer);
  function control(computer, ...args) {
    const result = shell(computer)
      ? spawnSync("/bin/sh", [path.join(computer.home, "bin/kuvo-start"), ...args], { cwd: computer.home, env: env(computer), encoding: "utf8", timeout: 15000 })
      : spawnSync(binary.file, ["keep", ...args], { cwd: computer.home, env: env(computer), encoding: "utf8", timeout: 15000 });
    assert.equal(result.status, 0, result.error?.message ?? result.stderr);
  }
  for (const peer of [main, member, away]) {
    const bin = initial(peer);
    if (shell(peer)) {
      const machineHome = path.join(peer.env.HOME, ".kuvo-code");
      fs.renameSync(peer.home, machineHome);
      peer.home = machineHome;
      const script = legacy(peer) && previousKeeper
        ? fs.readFileSync(previousKeeper, "utf8").replaceAll("@ENGINE@", body.engine) : starter;
      write(path.join(peer.home, bin, "kuvo-start"), script, { mode: 0o700 });
      write(path.join(peer.home, bin, "kuvo-setsid"), setsid, { mode: 0o700 });
      write(path.join(peer.home, bin, "box-version"), versionBefore(peer) + "\n");
      fs.mkdirSync(path.join(peer.home, bin, "opencode-plugins"));
      fs.symlinkSync(bin, path.join(peer.home, "bin"));
    }
    const engine = path.join(peer.home, "engine", body.engine, windows ? "opencode.exe" : "opencode");
    if (legacy(peer)) {
      write(path.join(peer.home, bin, "kuvo-link.js"), fs.readFileSync(typescript));
      fs.rmSync(path.join(peer.home, "run/engine.json"));
      // Let the shell keeper use Bun in place of the embedded runtime.
      const quote = word => "'" + word.replaceAll("'", "'\\''") + "'";
      let fixtureTrust = "";
      let fixtureEngine = "";
      if (bridgeBeta) {
        // The normal TS trial checks a live engine and agents bridge before confirming.
        // Run its own launcher and answer engine traffic through the bench engine.
        write(path.join(peer.home, "run/launch.env"), "TZ=UTC\n");
        const proxy = path.join(peer.home, "engine-fixture.cjs");
        write(proxy, `const http = require("node:http");
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
http.createServer((request, response) => {
  if (request.method === "GET" && request.url.split("?")[0] === "/api/debug/location") {
    response.writeHead(200, { "content-type":"application/json" }).end('{"data":[]}');
    return;
  }
  const forward = http.request({ host:"127.0.0.1", port:${peer.engine.record.port}, path:request.url,
    method:request.method, headers:request.headers }, reply => {
    response.writeHead(reply.statusCode, reply.headers);
    reply.pipe(response);
  });
  forward.on("error", () => response.destroy());
  response.on("close", () => forward.destroy());
  request.pipe(forward);
}).listen(port, "127.0.0.1", () => console.log("server listening"));\n`);
        fixtureEngine = `if [ "$1" = serve ]; then exec ${quote(process.execPath)} ${quote(proxy)} "$0" "$@"; fi\n`;
      }
      if (previousKeeper) {
        // This resident predates the fixture-key environment switch. Keep the bundled
        // program unchanged, and substitute only the public trust anchor it decodes.
        const preload = path.join(peer.env.HOME, "fixture-release-key.mjs");
        write(preload, `import { Buffer } from "node:buffer";
const from = Buffer.from;
Buffer.from = function (value, encoding, ...args) {
  const release = value === "iLSFoygG8wg+r2yj+/qnoMHpZIhPKRNLlf9vpEKmx0A=" && encoding === "base64";
  return from.call(this, release ? process.env.KUVO_TEST_RELEASE_KEY : value, encoding, ...args);
};\n`);
        // The resident starts another Bun process to verify and stage the pack.
        fixtureTrust = `export BUN_OPTIONS=${quote(`--preload=${preload}`)}\n`;
      }
      write(engine, `#!/bin/sh\n[ "$1" != --version ] || exit 0\n${fixtureEngine}${fixtureTrust}exec ${quote(typescriptRuntime())} "$@"\n`, { mode: 0o700 });
      continue;
    }
    write(path.join(peer.home, bin, program), executable, { mode: 0o700 });
    write(path.join(peer.home, bin, "..", "box.json"), encode({
      kuvo: "box/1", version: versionBefore(peer), launch: "rust", engine: body.engine,
    }));
    write(path.join(peer.home, "run/engine.json"), encode({ ...peer.engine.record, version: body.engine }));
    if (windows) write(engine, fs.readFileSync(process.execPath));
    else write(engine, '#!/bin/sh\n[ "$1" = --version ]\n', { mode: 0o700 });
    if (!shell(peer)) control(peer, "select", bin);
  }
  const keepers = new Map();
  function start(peer) {
    const keeper = shell(peer)
      ? launch("/bin/sh", [path.join(peer.home, "bin/kuvo-start"), "keep"], env(peer), peer.home)
      : launch(binary.file, ["keep"], env(peer), peer.home);
    keepers.set(peer, keeper);
  }
  const active = peer => shell(peer)
    ? fs.readlinkSync(path.join(peer.home, "bin"))
    : fs.readFileSync(path.join(peer.home, "active-release"), "utf8");
  const staged = peer => fs.existsSync(path.join(peer.home, "run/fleet-ready.json"));
  const confirmed = peer => active(peer) === selected && !fs.existsSync(path.join(peer.home, "run/previous"));
  const diagnose = () => [...keepers.keys()].map(peer => {
    return ["serve.log", "engine.log", "agents.log", "update-start.log", "update.log", "update.json", "repair.log", "fleet-ready.json", "fleet-stage-clock.json", "peer-reach.json", "setup-relay-lease.json"].map(name => {
      const log = path.join(peer.home, "run", name);
      return fs.existsSync(log) ? `${peer.home} ${name}\n${fs.readFileSync(log, "utf8")}` : "";
    }).join("\n") + keepers.get(peer).log();
  }).join("\n");
  const turnId = "feeda1234567";
  const sessionId = "ses_update_turn";
  const holder = path.join(member.home, "run/agents", `kuvo-hold-${turnId}.js`);
  write(holder, "process.stdin.pipe(process.stdout)\n");
  const turn = launch(process.execPath, [holder, "release-fixture", turnId, "raw"], env(member), member.home, "pipe");
  const reply = async text => {
    const answer = once(turn.child.stdout, "data");
    turn.child.stdin.write(text + "\n");
    const [chunk] = await answer;
    assert.equal(chunk.toString(), text + "\n");
  };
  const runningTurn = () => until(async () => {
    try {
      const resident = readJSON(path.join(member.home, "run/serve.json"));
      const answer = await request(resident, "GET", "api/session/active");
      return answer.status === 200 && answer.json?.data?.[sessionId]?.type === "busy";
    } catch { return false; }
  }, "the resident must still see the stand-in engine's running turn");
  if (shellKeepers) member.engine.running.add(sessionId);
  write(path.join(member.home, "run/agents/hold", `${turnId}.json`), encode({
    proto: "hold/1", holder: "release-fixture", id: turnId, kind: "codex", pid: turn.child.pid, fixture: true,
  }));
  try {
    const began = performance.now();
    start(main);
    start(member);
    await until(() => waiting.length && (staged(main) || staged(member)), diagnose, 90000);
    assert.equal(readJSON(path.join(main.home, "run/update.json")).latest, version);
    assert.equal(active(main), initial(main));
    assert.equal(active(member), initial(member));
    assert.equal(active(away), initial(away));
    t.diagnostic(`first preparation in ${Math.round(performance.now() - began)} ms`);

    await reply("turn before release switch");
    if (shellKeepers) await runningTurn();
    let leader = main;
    if (failover) {
      holdChecks = true;
      control(main, "off");
      await keepers.get(main).exited;
      assert.equal(active(main), initial(main));
      const offset = 130_000;
      for (const peer of [member, away]) write(path.join(peer.home, "run/test-setup-clock.json"), encode({ offset }));
      leases.advance(offset);
      await until(async () => {
        const lease = leases.leaseFor(setup.json.setup);
        if (!lease || lease.holder === setup.json.you.key) return false;
        try {
          const resident = readJSON(path.join(member.home, "run/serve.json"));
          const now = await request(resident, "GET", "kuvo/setup");
          return now.json?.you?.role === "primary" && lease.holder === now.json.you.key;
        } catch { return false; }
      },
        "the remaining online computer must claim the lease", 30000);
      leader = member;
      t.diagnostic(`a new main claimed the lease ${Math.round(performance.now() - began)} ms after the first boot`);
    }

    hold = false;
    const released = performance.now();
    for (const res of waiting) res.end(pack);
    await until(() => confirmed(leader), diagnose, 75000);
    if (failover) assert.ok(heldChecks > 0, "the new main checks the feed without waiting to activate");
    t.diagnostic(`main confirmed ${Math.round(performance.now() - released)} ms after member download resumed`);
    await until(() => confirmed(member), diagnose, 75000);
    await reply("same turn after release switch");
    if (shellKeepers) await runningTurn();
    assert.equal(active(away), initial(away));
    t.diagnostic(`online member confirmed ${Math.round(performance.now() - released)} ms after its download resumed`);

    const returned = performance.now();
    retryDownload = Boolean(shellKeepers);
    peerOnly = Boolean(failover || returningTypescript);
    start(away);
    const refused = () => {
      const cannot = path.join(away.home, "run/fleet-cannot.json");
      return fs.existsSync(cannot) ? readJSON(cannot) : undefined;
    };
    if (expectRefusal) {
      const failure = await until(() => {
        const failure = refused();
        return failure?.version === version && failure;
      }, diagnose, 120000);
      assert.equal(failure.reason, "The new link doesn't work with it here.");
      assert.equal(active(away), initial(away));
      assert.ok(refusedPacks > 0);
      t.diagnostic(`returning ${previousVersion} refused the release in ${Math.round(performance.now() - returned)} ms`);
      return;
    }
    const bridgeBin = bridgeBeta ? `versions/${bridgeVersion}-${bridgeHash.slice(0, 12)}/bin` : undefined;
    if (bridgeBin) {
      await until(() => active(away) === bridgeBin && !fs.existsSync(path.join(away.home, "run/previous")), diagnose, 120000);
      assert.ok(refusedBridges > 0, "the old beta must have reached a peer for its bridge pack");
      t.diagnostic(`returning ${previousVersion} confirmed the bridge in ${Math.round(performance.now() - returned)} ms`);
    }
    const rustBegan = performance.now();
    await until(() => {
      const failure = bridgeBeta && refused();
      if (failure?.version === version) throw new Error(failure.reason);
      return confirmed(away);
    }, diagnose, peerOnly ? 120000 : 65000);
    await until(async () => {
      try {
        const resident = readJSON(path.join(away.home, "run/serve.json"));
        return (await request(resident, "GET", "kuvo/info")).status === 200;
      } catch {
        return false;
      }
    }, diagnose);
    assert.equal(retryDownload, false);
    if (peerOnly) assert.ok(refusedPacks > 0);
    if (returningTypescript) {
      const copy = path.join(away.home, "backups/before-rust");
      assert.equal(fs.readlinkSync(path.join(copy, "bin")), bridgeBin ?? initial(away));
      assert.equal(fs.readFileSync(path.join(copy, "bin/box-version"), "utf8"), (bridgeBin ? bridgeVersion : versionBefore(away)) + "\n");
      assert.equal(readJSON(path.join(away.home, selected, "..", "box.json")).launch, "rust");
    }
    if (bridgeBin) t.diagnostic(`the bridge then confirmed Rust in ${Math.round(performance.now() - rustBegan)} ms`);
    t.diagnostic(`returning computer confirmed in ${Math.round(performance.now() - returned)} ms`);
  } finally {
    feed.close();
    feed.closeAllConnections();
    await turn.stop();
    const stopped = await Promise.allSettled([...keepers].map(async ([peer, child]) => {
      try {
        control(peer, "off");
        await until(() => child.child.exitCode !== null || child.child.signalCode !== null, diagnose, 15000);
      } finally {
        await child.stop();
      }
    }));
    for (const result of stopped) if (result.status === "rejected") throw result.reason;
  }
}

function seedReleasedMachine(root, tag, fixture) {
  const account = path.join(root, `${tag}-released-machine`);
  const machineTools = path.join(account, "tools");
  const home = path.join(account, ".kuvo-code");
  fs.cpSync(path.join(fixture, "machine"), home, { recursive: true });
  fs.mkdirSync(path.join(account, "tmp"), { recursive: true });
  write(path.join(machineTools, "caffeinate"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  const identity = "5a".repeat(16);
  write(path.join(home, "machine-id"), identity + "\n");
  const link = path.join(home, "run/link.json");
  const before = readJSON(link);
  // It was disconnected while the new release was installed on the Mac.
  write(link, encode({ ...before, relay: "ws://127.0.0.1:1" }));
  write(path.join(home, "workspace/notes"), "work kept on the released machine");
  const legacyBin = path.join(home, "bin/kuvo-start");
  write(legacyBin, "#!/bin/sh\nexit 77\n", { mode: 0o700 });
  if (tag !== "v0.7.2") write(path.join(home, "bin/box-version"), tag.slice(1) + "\n");
  return { account, machineTools, home, identity, link, before, legacyBin };
}

async function checkReleasedMachine(t, { root, tag, fixture, environment, install, installedRelease }) {
  const { account, machineTools, home, identity, link, before, legacyBin } = seedReleasedMachine(root, tag, fixture);
  const updateMachine = value => install("released-machine", account, machineTools, ["--identity", value]);
  const wrong = updateMachine("bb".repeat(16));
  assert.equal(wrong.status, 1, wrong.stdout + wrong.stderr);
  assert.match(JSON.parse(wrong.stdout).message, /saved installation/);
  assert.equal(fs.existsSync(path.join(home, "folder-version.json")), false);
  const active = launch("/bin/sleep", ["30"], environment, home);
  const priorPid = path.join(home, "run/link.pid");
  try {
    write(priorPid, String(active.child.pid));
    const busy = updateMachine(identity);
    assert.equal(busy.status, 4, busy.stdout + busy.stderr);
    assert.equal(JSON.parse(busy.stdout).result, "waits");
    assert.equal(active.child.exitCode, null);
    assert.equal(fs.existsSync(path.join(home, "folder-version.json")), false);
  } finally {
    await active.stop();
    fs.rmSync(priorPid, { force: true });
  }
  write(path.join(home, "run/engine.json"), encode({ pid: active.child.pid, version: "2.0.32" }));
  const readyAt = performance.now();
  try {
    const upgraded = updateMachine(identity);
    assert.equal(upgraded.status, 0, upgraded.error?.message ?? upgraded.stdout + upgraded.stderr);
    assert.deepEqual(JSON.parse(upgraded.stdout), { result: "done", started: true });
    installedRelease(home);
    const ready = await until(() => {
      try { return readJSON(path.join(home, "run/serve.json")); } catch { return undefined; }
    }, "the released machine must start the new keeper", 15000);
    const info = await request(ready, "GET", "kuvo/info");
    assert.equal(info.status, 200, info.text);
    assert.equal(readJSON(path.join(home, "folder-version.json")).version, 23);
    const after = readJSON(link);
    assert.equal(after.key, before.key);
    assert.equal(after.secret, before.secret);
    assert.equal(fs.readFileSync(path.join(home, "workspace/notes"), "utf8"), "work kept on the released machine");
    t.diagnostic(`released ${tag} machine applied its signed installer and answered in ${Math.round(performance.now() - readyAt)} ms`);
  } finally {
    if (fs.lstatSync(path.join(home, "bin")).isSymbolicLink()) {
      const stopped = spawnSync("/bin/sh", [legacyBin, "off"], {
        cwd: home, env: { ...environment, HOME: account, TMPDIR: path.join(account, "tmp") },
        encoding: "utf8", timeout: 15000,
      });
      assert.equal(stopped.status, 0, stopped.error?.message ?? stopped.stdout + stopped.stderr);
    }
  }
}

async function linuxInstallMedia(t, binary, root, key, beforeStart = "") {
  const media = path.join(root, "media");
  const tree = path.join(media, "files");
  const version = "0.8.0";
  const pin = "2.2.6";
  const starter = fs.readFileSync(new URL("../../box/src/keeper/kuvo-start", import.meta.url), "utf8")
    .replaceAll("@ENGINE@", pin).replace("#!/bin/sh\n", `#!/bin/sh\n${beforeStart}`);
  const payload = {
    "bin/box-version": Buffer.from(version + "\n"),
    "bin/kuvo-start": Buffer.from(starter),
    "bin/kuvo-box": fs.readFileSync(binary.file),
  };
  for (const [file, bytes] of Object.entries(payload)) write(path.join(tree, file), bytes, { mode: 0o700 });
  write(path.join(tree, "box.json"), encode({ kuvo: "box/1", version, engine: pin, launch: "rust",
    files: Object.fromEntries(Object.entries(payload).map(([file, bytes]) => [file, digest(bytes)])) }));
  const archive = path.join(media, "move.tar");
  const packed = spawnSync("tar", ["cf", archive, "-C", tree, "box.json", "bin"], { encoding: "utf8", timeout: 90000 });
  assert.equal(packed.status, 0, packed.stderr);
  const bytes = gzipSync(fs.readFileSync(archive), { level: 1 });
  write(path.join(media, "move.tar.gz"), bytes);
  const pack = { url: "https://example.invalid/move.tar.gz", sha256: digest(bytes) };
  const feed = await engineFeed(t, media, `#!/bin/sh\n[ "$1" = --version ] && echo ${pin}\n`);
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  write(path.join(media, "boxes.json"), encode(envelope({ kuvo: "box/1", version, engine: pin, launch: "rust", spreads: true,
    pack, packs: { [target]: pack }, linux: { [arch]: feed.build } }, key)));
  fs.copyFileSync(new URL("../../box/src/install/kuvo-install", import.meta.url), path.join(media, "kuvo-install"));
  fs.copyFileSync(binary.file, path.join(media, "kuvo-install-bin"));
  return { media, tree, version, pack, bytes };
}

async function freshLinuxInstaller(t, binary, beforeStart = "") {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const key = keypair();
  const { media, version, pack } = await linuxInstallMedia(t, binary, root, key,
    `export KUVO_TEST_RELAY=ws://127.0.0.1:1\n${beforeStart}`);
  const home = path.join(env.HOME, ".kuvo-code");
  const tools = path.join(env.HOME, "tools");
  const cron = path.join(tools, "table");
  const cronBefore = "30 7 * * * /bin/true\n";
  write(cron, cronBefore);
  write(path.join(tools, "crontab"), `#!/bin/sh
hold="$KUVO_TEST_UNINSTALL_TOOLS/hold-crontab"
if [ "$1" = -l ] && [ -f "$hold" ]; then
  printf '%s\\n' "$$" > "$hold.pid"
  exec /bin/sleep 300
fi
case "$1" in
  -l) cat "$KUVO_TEST_UNINSTALL_TOOLS/table" ;;
  -) cat >"$KUVO_TEST_UNINSTALL_TOOLS/table" ;;
  *) exit 1 ;;
esac
`, { mode: 0o700 });
  const fixtureEnv = { ...env, PATH: `${tools}${path.delimiter}${env.PATH}`, KUVO_TEST_UNINSTALL_TOOLS: tools,
    KUVO_TEST_RELEASE_KEY: key.key };
  const entry = path.join(media, "kuvo-install");
  const install = verb => spawnSync("/bin/sh", [entry, verb], {
    cwd: env.HOME, env: fixtureEnv, encoding: "utf8", timeout: 90000,
  });
  const stop = () => {
    if (!fs.existsSync(path.join(home, "bin/kuvo-start"))) return;
    const stopped = spawnSync("/bin/sh", [path.join(home, "bin/kuvo-start"), "off"], {
      cwd: home, env: fixtureEnv, encoding: "utf8", timeout: 15000,
    });
    assert.equal(stopped.status, 0, stopped.stderr);
  };
  const begin = verb => launch("/bin/sh", [entry, verb], fixtureEnv, env.HOME);
  return { env, home, cron, cronBefore, version, pack, install, begin, stop };
}

test("signed Linux installer makes a fresh machine and keeps unrelated reboot jobs", { timeout: 120000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "linux") return t.skip("Uses the native Linux installer.");
  const { env, home, cron, cronBefore, version, pack, install, stop } = await freshLinuxInstaller(t, binary);
  const status = install("status");
  assert.equal(status.status, 0, status.error?.message ?? status.stdout + status.stderr);
  const { verdict, verb } = JSON.parse(status.stdout);
  assert.deepEqual({ verdict, verb }, { verdict: "fresh", verb: "new" });
  assert.equal(fs.existsSync(home), false);
  const move = install("move");
  assert.equal(move.status, 1, move.error?.message ?? move.stdout + move.stderr);
  assert.equal(JSON.parse(move.stdout).message, "There's nothing to move on this computer: Kuvo is where it belongs already.");
  assert.equal(fs.existsSync(home), false);
  write(path.join(home, "workspace/old-work"), "keep earlier work");
  write(path.join(home, "desktop/identity"), "keep this desktop");
  try {
    const began = performance.now();
    const made = install("new");
    assert.equal(made.status, 0, made.error?.message ?? made.stdout + made.stderr);
    const result = JSON.parse(made.stdout.trim().split("\n").at(-1));
    assert.deepEqual([result.result, result.verdict, result.started], ["done", "machine", true]);
    assert.equal(fs.readFileSync(path.join(result.setAside, "workspace/old-work"), "utf8"), "keep earlier work");
    assert.equal(fs.readFileSync(path.join(home, "desktop/identity"), "utf8"), "keep this desktop");
    assert.equal(fs.readlinkSync(path.join(home, "bin")), `versions/${version}-${pack.sha256.slice(0, 12)}/bin`);
    assert.equal(readJSON(path.join(home, "run/link.json")).setup.role, "primary");
    const info = await request(readJSON(path.join(home, "run/serve.json")), "GET", "kuvo/info");
    assert.equal(info.status, 200, info.text);
    assert.match(fs.readFileSync(path.join(home, "machine-id"), "utf8"), /^[a-f0-9]{32}$/);
    assert.match(fs.readFileSync(path.join(home, "run/launch.env"), "utf8"), /^PATH=/m);
    assert.equal(fs.readFileSync(cron, "utf8"), cronBefore +
      '@reboot sh "$HOME/.kuvo-code/bin/kuvo-start" >/dev/null 2>&1 # kuvo-code: starts its engine again after a restart\n');
    t.diagnostic(`fresh Linux machine made and answered in ${Math.round(performance.now() - began)} ms`);
    const refused = install("new");
    assert.equal(refused.status, 1, refused.stdout + refused.stderr);
    assert.equal(JSON.parse(refused.stdout).verdict, "machine");
    assert.equal(fs.readlinkSync(path.join(home, "bin")), `versions/${version}-${pack.sha256.slice(0, 12)}/bin`);
    assert.deepEqual(fs.readdirSync(env.TMPDIR), []);
  } finally {
    stop();
  }
});

test("a Linux installer retries a failed first start and keeps its earlier files", { timeout: 180000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "linux") return t.skip("Uses the native Linux installer.");
  const interruptedLink = JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] });
  const { env, home, cron, install, stop } = await freshLinuxInstaller(t, binary, `
if [ "$1" = box ] && [ -f "$HOME/refuse-start" ]; then
  printf '%s' '${interruptedLink}' > "$HOME/.kuvo-code/run/link.json"
  exit 1
fi
`);
  const fault = path.join(env.HOME, "refuse-start");
  const previousWork = path.join("workspace", "old-work");
  write(fault, "");
  write(path.join(home, previousWork), "keep earlier work");
  try {
    const failed = install("new");
    assert.equal(failed.status, 1, failed.error?.message ?? failed.stdout + failed.stderr);
    const failure = JSON.parse(failed.stdout);
    assert.deepEqual([failure.result, failure.verdict, failure.started], ["failed", "fresh", false]);
    const firstId = fs.readFileSync(path.join(home, "machine-id"), "utf8");
    assert.equal(fs.readFileSync(path.join(failure.setAside, previousWork), "utf8"), "keep earlier work");
    const table = fs.readFileSync(cron, "utf8");
    const status = install("status");
    assert.equal(status.status, 0, status.error?.message ?? status.stdout + status.stderr);
    const { verdict, verb } = JSON.parse(status.stdout);
    assert.deepEqual({ verdict, verb }, { verdict: "fresh", verb: "new" });
    fs.rmSync(fault);
    const made = install("new");
    assert.equal(made.status, 0, made.error?.message ?? made.stdout + made.stderr);
    const result = JSON.parse(made.stdout);
    assert.deepEqual([result.result, result.verdict, result.started], ["done", "machine", true]);
    assert.equal(fs.readFileSync(path.join(result.setAside, "machine-id"), "utf8"), firstId);
    assert.equal(fs.readFileSync(path.join(result.setAside, "run/link.json"), "utf8"), interruptedLink);
    const link = readJSON(path.join(home, "run/link.json"));
    assert.equal(link.setup.role, "primary");
    assert.equal(link.mac, undefined);
    assert.notEqual(fs.readFileSync(path.join(home, "machine-id"), "utf8"), firstId);
    assert.equal(fs.readFileSync(path.join(failure.setAside, previousWork), "utf8"), "keep earlier work");
    assert.equal(fs.readFileSync(cron, "utf8"), table);
    assert.equal(fs.existsSync(path.join(home, "updates/install")), false);
  } finally {
    stop();
  }
});

test("a fresh Linux install leaves the earlier home alone while its keeper is still running", { timeout: 15000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "linux") return t.skip("Uses the native Linux installer.");
  const { env, home, cron, cronBefore, install } = await freshLinuxInstaller(t, binary);
  write(path.join(home, "run/off"), "");
  write(path.join(home, "workspace/old-work"), "keep earlier work");
  const writer = launch("/bin/sleep", ["30"], env, home);
  try {
    write(path.join(home, "run/box.pid"), String(writer.child.pid));
    const refused = install("new");
    assert.equal(refused.status, 1, refused.error?.message ?? refused.stdout + refused.stderr);
    assert.deepEqual(JSON.parse(refused.stdout), {
      result: "refused", verdict: "fresh",
      message: "Something is running in this computer's ~/.kuvo-code: Kuvo is installed there once that's stopped.",
    });
    assert.equal(writer.child.exitCode, null);
    assert.deepEqual(fs.readdirSync(home).sort(), ["run", "workspace"]);
    assert.equal(fs.readFileSync(path.join(home, "workspace/old-work"), "utf8"), "keep earlier work");
    assert.equal(fs.readFileSync(cron, "utf8"), cronBefore);
  } finally {
    await writer.stop();
  }
});

test("a Linux installer resumes after it was interrupted between publication and startup", { timeout: 120000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "linux") return t.skip("Uses the native Linux installer.");
  const { home, cron, cronBefore, install, begin, stop } = await freshLinuxInstaller(t, binary);
  const hold = path.join(path.dirname(cron), "hold-crontab");
  const oldWork = path.join("workspace", "old-work");
  write(path.join(home, oldWork), "keep earlier work");
  write(hold, "");
  const interrupted = begin("new");
  let cronPid;
  try {
    cronPid = await until(() => {
      const file = `${hold}.pid`;
      if (!fs.existsSync(file)) return;
      const value = Number(fs.readFileSync(file, "utf8"));
      return value > 1 ? value : undefined;
    }, "the installer reaches the pre-start crontab command", 60000);
    const machineId = path.join(home, "machine-id");
    const publishedId = fs.readFileSync(machineId, "utf8");
    const selected = fs.readlinkSync(path.join(home, "bin"));
    assert.equal(fs.existsSync(path.join(home, "run/serve.json")), false);
    assert.equal(fs.readFileSync(cron, "utf8"), cronBefore);
    assert.equal((await interrupted.stop()).signal, "SIGTERM");
    process.kill(cronPid);
    cronPid = undefined;
    fs.rmSync(hold);
    const pending = install("status");
    assert.equal(pending.status, 0, pending.error?.message ?? pending.stdout + pending.stderr);
    const { verdict, verb } = JSON.parse(pending.stdout);
    assert.deepEqual({ verdict, verb }, { verdict: "fresh", verb: "new" });
    const resumed = install("new");
    assert.equal(resumed.status, 0, resumed.error?.message ?? resumed.stdout + resumed.stderr);
    const result = JSON.parse(resumed.stdout);
    assert.deepEqual([result.result, result.verdict, result.started], ["done", "machine", true]);
    assert.equal(fs.readFileSync(machineId, "utf8"), publishedId);
    assert.equal(fs.readlinkSync(path.join(home, "bin")), selected);
    assert.equal(fs.readFileSync(path.join(result.setAside, oldWork), "utf8"), "keep earlier work");
    assert.equal(fs.readdirSync(path.join(home, "trash")).length, 1);
    assert.equal(fs.existsSync(path.join(home, "updates/install")), false);
  } finally {
    await interrupted.stop();
    if (cronPid !== undefined) process.kill(cronPid);
    stop();
  }
});

test("a Linux installer clears an earlier incomplete installation after making the new home", { timeout: 120000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "linux") return t.skip("Uses the native Linux installer.");
  const { home, install, stop } = await freshLinuxInstaller(t, binary);
  const making = path.join(home, "updates/install/making");
  write(making, "1\n");
  write(path.join(home, "bin/kuvo-start"), "half an installation", { mode: 0o700 });
  try {
    const before = install("status");
    assert.equal(before.status, 0, before.error?.message ?? before.stdout + before.stderr);
    assert.equal(JSON.parse(before.stdout).verdict, "fresh");
    const made = install("new");
    assert.equal(made.status, 0, made.error?.message ?? made.stdout + made.stderr);
    const result = JSON.parse(made.stdout);
    assert.deepEqual([result.result, result.verdict, result.started], ["done", "machine", true]);
    assert.equal(fs.readFileSync(path.join(result.setAside, "bin/kuvo-start"), "utf8"), "half an installation");
    assert.equal(fs.existsSync(making), false);
    assert.equal(fs.existsSync(path.dirname(making)), false);
  } finally {
    stop();
  }
});

test("signed Linux media moves the two released machines after they return", { timeout: 240000 }, async t => {
  const binary = boxBinary();
  const cache = process.env.KUVO_CONFORMANCE_RELEASED_MACHINES;
  if (binary.typescript || process.platform !== "linux" || !cache) {
    return t.skip("Supply the released machine fixtures for the Linux installer.");
  }
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const environment = isolatedEnv(root);
  const key = keypair();
  const { media, version, pack } = await linuxInstallMedia(t, binary, root, key);
  const install = (verb, account, _tools, extra) => spawnSync("/bin/sh", [path.join(media, "kuvo-install"), verb, ...extra], {
    cwd: account,
    env: { ...environment, HOME: account, TMPDIR: path.join(account, "tmp"), KUVO_TEST_RELEASE_KEY: key.key },
    encoding: "utf8", timeout: 90000,
  });
  const installedRelease = home => {
    const selected = fs.readlinkSync(path.join(home, "bin"));
    assert.equal(selected, `versions/${version}-${pack.sha256.slice(0, 12)}/bin`);
    assert.deepEqual(readJSON(path.join(home, selected, "../boxes.json")), readJSON(path.join(media, "boxes.json")));
  };
  for (const tag of ["v0.7.2", "v0.8.0-beta.1"]) {
    await checkReleasedMachine(t, { root, tag, fixture: path.join(cache, tag), environment, install, installedRelease });
  }
});

test("the main sends signed installer media to a returning 0.7.2 machine", { timeout: 150000 }, async t => {
  const binary = boxBinary();
  const cache = process.env.KUVO_CONFORMANCE_RELEASED_MACHINES;
  if (binary.typescript || process.platform !== "linux" || !cache) {
    return t.skip("Supply the released machine fixture for the Linux SSH installer.");
  }
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const environment = isolatedEnv(root);
  const key = keypair();
  const { media, tree, version, pack, bytes } = await linuxInstallMedia(t, binary, root, key, `
if [ "\${1:-}" = box ] && [ ! -f "$HOME/.kuvo-code/run/keeper-start-refused" ]; then
  touch "$HOME/.kuvo-code/run/keeper-start-refused"
  exit 1
fi
`);
  const returned = seedReleasedMachine(root, "v0.7.2", path.join(cache, "v0.7.2"));
  const home = path.join(environment.HOME, ".kuvo-code");
  const selected = `versions/${version}-${pack.sha256.slice(0, 12)}`;
  fs.cpSync(tree, path.join(home, selected), { recursive: true });
  fs.copyFileSync(path.join(media, "boxes.json"), path.join(home, selected, "boxes.json"));
  write(path.join(home, selected, "pack"), bytes);
  fs.symlinkSync(selected + "/bin", path.join(home, "bin"));
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "machines.json"), encode([{ destination: "released-lab", identity: returned.identity }]));
  const online = path.join(root, "online");
  const reached = path.join(root, "reached");
  const fake = path.join(environment.HOME, "ssh-fixture");
  write(fake, `#!/bin/sh
printf 'ssh\\n' >> "$KC_REACHED"
[ -f "$KC_ONLINE" ] || exit 255
for command do :; done
export HOME="$KC_REMOTE_HOME"
exec /bin/sh -c "$command"
`, { mode: 0o700 });
  const releaseFeed = createServer((request, response) => {
    if (request.url !== "/boxes.json") return response.writeHead(404).end();
    response.end(fs.readFileSync(path.join(media, "boxes.json")));
  });
  releaseFeed.listen(0, "127.0.0.1");
  await once(releaseFeed, "listening");
  const env = { ...environment, KUVO_TEST_SSH: fake, KC_ONLINE: online,
    KC_REACHED: reached, KC_REMOTE_HOME: returned.account, KUVO_TEST_RELEASE_KEY: key.key,
    KUVO_TEST_RELEASE_FEED: `http://127.0.0.1:${releaseFeed.address().port}/boxes.json` };
  const main = startBox({ binary, home, env, relay: { url: "ws://127.0.0.1:1" } });
  try {
    assert.equal((await request(await main.ready(), "GET", "kuvo/info")).status, 200);
    await until(() => fs.existsSync(reached), () => `the main should try the saved SSH address\n${main.log()}`, 15000);
    assert.equal(fs.existsSync(path.join(returned.home, "folder-version.json")), false);
    const started = performance.now();
    write(online, "");
    const ready = await until(() => {
      try { return readJSON(path.join(returned.home, "run/serve.json")); } catch { return undefined; }
    }, () => `the returning machine should answer from the Rust keeper\n${main.log()}`, 90000);
    assert.equal((await request(ready, "GET", "kuvo/info")).status, 200);
    assert.equal(fs.readlinkSync(path.join(returned.home, "bin")), selected + "/bin");
    assert.equal(fs.existsSync(path.join(returned.home, "run/keeper-start-refused")), true);
    assert.equal(readJSON(returned.link).key, returned.before.key);
    assert.equal(readJSON(returned.link).secret, returned.before.secret);
    assert.equal(fs.readFileSync(path.join(returned.home, "workspace/notes"), "utf8"), "work kept on the released machine");
    const answered = performance.now();
    const reservation = path.join(returned.account, ".kuvo-code-adding");
    const installer = path.join(returned.home, "updates/install");
    await until(() => !fs.lstatSync(reservation, { throwIfNoEntry: false }) && !fs.existsSync(installer),
      () => `the SSH reservation and installer staging should clear\n${main.log()}`);
    t.diagnostic(`the returning 0.7.2 machine answered in ${Math.round(answered - started)} ms`);
    t.diagnostic(`its SSH reservation and staging cleared in ${Math.round(performance.now() - answered)} ms more`);
  } finally {
    releaseFeed.close();
    releaseFeed.closeAllConnections();
    await main.stop();
    if (fs.lstatSync(path.join(returned.home, "bin")).isSymbolicLink()) {
      const stopped = spawnSync("/bin/sh", [returned.legacyBin, "off"], {
        cwd: returned.home, env: { ...environment, HOME: returned.account, TMPDIR: path.join(returned.account, "tmp") },
        encoding: "utf8", timeout: 15000,
      });
      assert.equal(stopped.status, 0, stopped.stderr);
    }
  }
});

test("kuvo-install creates a new Mac home and moves the released homes", { timeout: 420000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "darwin") {
    return t.skip("The released Mac installer fixture needs the Rust Mac bootstrap and APFS.");
  }
  const { generateRelease } = await import("./compat/generate.mjs");
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const environment = isolatedEnv(root);
  const key = keypair();
  for (const tag of ["v0.7.2", "v0.8.0-beta.1"]) {
    const start = performance.now();
    const fixture = path.join(root, tag);
    await generateRelease(tag, fixture);
    t.diagnostic(`${tag} golden home generated in ${Math.round(performance.now() - start)} ms`);
    const app = readJSON(path.join(fixture, "app-world.json"));
    const media = path.join(fixture, "media");
    const tree = path.join(media, "files");
    fs.cpSync(app.release, tree, { recursive: true, preserveTimestamps: true });
    const version = "0.8.0";
    const pin = readJSON(path.join(tree, "box.json")).engine;
    const helper = "bin/Kuvo Helper.app";
    const helperPath = "Contents/MacOS/Kuvo Helper";
    const manifest = readJSON(path.join(tree, "box.json"));
    const starter = fs.readFileSync(new URL("../../box/src/keeper/kuvo-start", import.meta.url), "utf8")
      .replaceAll("@ENGINE@", pin);
    const setsid = fs.readFileSync(new URL("../../box/src/keeper/kuvo-setsid", import.meta.url));
    write(path.join(tree, "bin/kuvo-start"), starter, { mode: 0o700 });
    write(path.join(tree, "bin/kuvo-setsid"), setsid, { mode: 0o700 });
    write(path.join(tree, "bin/kuvo-box"), fs.readFileSync(binary.file), { mode: 0o700 });
    write(path.join(tree, "bin/box-version"), version + "\n");
    manifest.files["bin/kuvo-start"] = digest(starter);
    manifest.files["bin/kuvo-setsid"] = digest(setsid);
    manifest.files["bin/kuvo-box"] = digest(fs.readFileSync(path.join(tree, "bin/kuvo-box")));
    manifest.files["bin/box-version"] = digest(version + "\n");
    manifest.bundles = { [helper]: { [helperPath]: digest(fs.readFileSync(path.join(tree, helper, helperPath))) } };
    write(path.join(tree, "box.json"), encode({ ...manifest, kuvo: "box/1", version, launch: "rust" }));

    const engine = fs.readFileSync(path.join(app.engine, pin, "opencode"), "utf8")
      .replace("#!/bin/sh\n", `#!/bin/sh\nif [ "$1" = --version ]; then echo ${pin}; exit 0; fi\n`);
    const feed = await engineFeed(t, media, engine);
    const archive = path.join(media, "move.tar");
    const packed = spawnSync("tar", ["cf", archive, "-C", tree, "box.json", "bin", "config"], { encoding: "utf8", timeout: 90000 });
    assert.equal(packed.status, 0, packed.stderr);
    const bytes = gzipSync(fs.readFileSync(archive), { level: 1 });
    write(path.join(media, "move.tar.gz"), bytes);
    const pack = { url: "https://example.invalid/move.tar.gz", sha256: digest(bytes) };
    const machine = process.arch === "arm64" ? "aarch64" : "x86_64";
    write(path.join(media, "boxes.json"), encode(envelope({
      kuvo: "box/1", version, engine: pin, launch: "rust",
      pack, packs: { [target]: pack }, darwin: { [machine]: feed.build },
    }, key)));
    fs.copyFileSync(fileURLToPath(new URL("../../box/src/install/kuvo-install", import.meta.url)), path.join(media, "kuvo-install"));
    fs.copyFileSync(binary.file, path.join(media, "kuvo-install-bin"));

    const tools = path.join(app.home, "launch-fixture");
    const logs = path.join(tools, "calls");
    const registration = path.join(tools, "registered");
    write(path.join(tools, "launchctl"), `#!/bin/sh
printf '%s\\n' "$1" >> "$KUVO_TEST_UNINSTALL_TOOLS/calls"
registered="$KUVO_TEST_UNINSTALL_TOOLS/registered"
case "$1" in
  print | kickstart) [ -f "$registered" ] ;;
  bootstrap) [ -f "$3" ] && touch "$registered" ;;
  *) exit 1 ;;
esac
`, { mode: 0o700 });
    const original = fs.readFileSync(path.join(app.S, "Chats/c1/notes.md"), "utf8");
    const install = (verb, account = app.home, launchTools = tools, extra = []) => spawnSync("/bin/sh", [path.join(media, "kuvo-install"), verb, "--app", app.bundle, ...extra], {
      cwd: account,
      env: {
        ...environment, HOME: account, TMPDIR: path.join(account, "tmp"),
        PATH: `${launchTools}${path.delimiter}${environment.PATH}`,
        KUVO_TEST_RELEASE_KEY: key.key, KUVO_TEST_UNINSTALL_TOOLS: launchTools,
      },
      encoding: "utf8", timeout: 150000,
    });
    const installedRelease = home => {
      const selected = fs.readlinkSync(path.join(home, "bin"));
      assert.equal(selected, `versions/${version}-${pack.sha256.slice(0, 12)}/bin`);
      const release = path.join(home, selected, "..");
      assert.equal(readJSON(path.join(release, "box.json")).launch, "rust");
      assert.deepEqual(readJSON(path.join(release, "boxes.json")), readJSON(path.join(media, "boxes.json")));
    };
    if (tag === "v0.7.2") {
      const fresh = path.join(root, "fresh");
      const freshTools = path.join(fresh, "launch-fixture");
      fs.mkdirSync(path.join(fresh, "tmp"), { recursive: true });
      fs.cpSync(tools, freshTools, { recursive: true });
      const starts = performance.now();
      const status = install("status", fresh, freshTools);
      assert.equal(status.status, 0, status.stderr);
      assert.equal(JSON.parse(status.stdout).verb, "new");
      const made = install("new", fresh, freshTools);
      assert.equal(made.status, 0, made.error?.message ?? made.stdout + made.stderr);
      const answer = JSON.parse(made.stdout.trim().split("\n").at(-1));
      assert.equal(answer.result, "done");
      assert.equal(answer.started, true);
      const home = path.join(fresh, ".kuvo-code");
      assert.equal(readJSON(path.join(home, "box.json")).launch, "rust");
      installedRelease(home);
      assert.equal(readJSON(path.join(home, "folder-version.json")).version, 23);
      assert.equal(fs.existsSync(path.join(home, "updates/install")), false);
      assert.equal(fs.readFileSync(path.join(freshTools, "calls"), "utf8"), "print\nbootstrap\n");
      assert.deepEqual(fs.readdirSync(path.join(fresh, "tmp")), []);
      t.diagnostic(`fresh signed Mac media installed and started in ${Math.round(performance.now() - starts)} ms`);
    }
    const verdict = install("status");
    assert.equal(verdict.status, 0, verdict.error?.message ?? verdict.stdout + verdict.stderr);
    assert.equal(JSON.parse(verdict.stdout).verb, "move");
    const moving = performance.now();
    const ran = install("move");
    assert.equal(ran.status, 0, ran.error?.message ?? ran.stdout + ran.stderr);
    const lines = ran.stdout.trim().split("\n").map(line => JSON.parse(line));
    const result = lines.pop();
    assert.equal(result.result, "done");
    assert.equal(result.started, true, ran.stderr);
    assert.equal(lines[0]?.step, "plan");
    assert.ok(lines.some(line => line.step === "database"));
    assert.ok(lines.every((line, i) => !i || line.index > lines[i - 1].index));
    assert.equal(readJSON(path.join(app.K, "box.json")).launch, "rust");
    installedRelease(app.K);
    assert.equal(readJSON(path.join(app.K, "folder-version.json")).version, 23);
    assert.equal(fs.readFileSync(path.join(app.S, "Chats/c1/notes.md"), "utf8"), original);
    assert.ok(fs.statSync(path.join(app.K, "bin/kuvo-box")).isFile());
    assert.equal(fs.readFileSync(logs, "utf8"), "print\nbootstrap\n");
    const plist = path.join(app.home, "Library/LaunchAgents/run.kuvo.code.machine.plist");
    assert.ok(fs.readFileSync(plist, "utf8").includes(path.join(app.K, "bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper")));
    t.diagnostic(`${tag} installer moved the home in ${Math.round(performance.now() - moving)} ms`);

    const restarting = performance.now();
    fs.rmSync(registration);
    const missing = install("status");
    assert.equal(missing.status, 0, missing.stderr);
    assert.equal(JSON.parse(missing.stdout).verb, "new");
    const restarted = install("new");
    assert.equal(restarted.status, 0, restarted.stderr);
    assert.equal(JSON.parse(restarted.stdout).started, true);
    assert.ok(fs.existsSync(registration));
    assert.equal(fs.readFileSync(logs, "utf8"), "print\nbootstrap\nprint\nprint\nbootstrap\nkickstart\n");
    t.diagnostic(`${tag} relaunched its Helper in ${Math.round(performance.now() - restarting)} ms`);

    // The old app was closed throughout. Start through the entry point its
    // replacement Helper uses, from the installation the signed media put down.
    const serving = performance.now();
    const homeEnv = { ...environment, HOME: app.home, TMPDIR: path.join(app.home, "tmp"),
      KUVO_HOME: app.K, KUVO_TEST_RELAY: "ws://127.0.0.1:1", KUVO_RELAY_URL: "ws://127.0.0.1:1" };
    const resident = launch("/bin/sh", [path.join(app.K, "bin/kuvo-start"), "serve", "--resident"], homeEnv, app.K);
    try {
      resident.child.stdin.write(JSON.stringify({ password: "installed-fixture", renews: true }) + "\n");
      const ready = await until(() => {
        if (resident.child.exitCode !== null || resident.child.signalCode !== null) throw new Error(resident.log());
        try {
          const record = readJSON(path.join(app.K, "run/serve.json"));
          return record.pid === resident.child.pid ? record : undefined;
        } catch { return undefined; }
      }, () => {
        const log = path.join(app.K, "run/serve.log");
        return `the installed keeper should start the Rust resident\n${fs.existsSync(log) ? fs.readFileSync(log, "utf8") : resident.log()}`;
      });
      const info = await request(ready, "GET", "kuvo/info");
      assert.equal(info.status, 200, info.text);
      assert.equal(ready.password, "installed-fixture");
      t.diagnostic(`${tag} installed keeper answered in ${Math.round(performance.now() - serving)} ms`);
    } finally {
      await resident.stop();
    }
    await checkReleasedMachine(t, { root, tag, fixture, environment, install, installedRelease });
  }
});

test("a TypeScript updater stages the signed Rust pack for its own target", t => {
  const { binary, home, release, key, body, update } = box(t);
  const rust = process.env.KUVO_CONFORMANCE_RUST_BIN;
  if (!binary.typescript || !rust || process.platform === "win32" || !target) {
    return t.skip("This transition needs the TypeScript updater and a native Rust executable.");
  }
  const source = path.join(release, "transition");
  const starter = fileURLToPath(new URL("../../box/src/keeper/kuvo-start", import.meta.url));
  const keeper = fs.readFileSync(starter, "utf8").replaceAll("@ENGINE@", body.engine);
  const version = "0.8.0";
  const payload = {
    "bin/box-version": Buffer.from(version + "\n"),
    "bin/kuvo-start": Buffer.from(keeper),
    "bin/kuvo-box": fs.readFileSync(rust),
  };
  write(path.join(home, "bin/kuvo-start"), keeper, { mode: 0o700 });
  write(path.join(home, "engine", body.engine, "opencode"), '#!/bin/sh\n[ "$1" = --version ]\n', { mode: 0o700 });
  for (const [file, bytes] of Object.entries(payload)) write(path.join(source, file), bytes, { mode: 0o700 });
  write(path.join(source, "box.json"), encode({
    kuvo: "box/1", version, engine: body.engine, launch: "rust",
    files: Object.fromEntries(Object.entries(payload).map(([file, bytes]) => [file, digest(bytes)])),
  }));
  const archive = path.join(release, "transition.tar");
  const packed = spawnSync("tar", ["cf", archive, "-C", source, "box.json", "bin"], { encoding: "utf8", timeout: 90000 });
  assert.equal(packed.status, 0, packed.stderr);
  const bytes = gzipSync(fs.readFileSync(archive), { level: 1 });
  const hash = digest(bytes);
  write(path.join(release, "transition.tar.gz"), bytes);
  const signed = envelope({
    ...body, version, launch: "rust",
    packs: { [target]: { url: "https://example.invalid/transition.tar.gz", sha256: hash } },
  }, key);
  if (process.env.KUVO_CONFORMANCE_DEPLOYED_TS_BIN) {
    const old = boxBinary(process.env.KUVO_CONFORMANCE_DEPLOYED_TS_BIN);
    assert.equal(old.typescript, true);
    assert.deepEqual(update(signed, "pack", old), { result: "failed", version, message: "no box.json" });
  }
  assert.deepEqual(update(signed), { result: "waiting", version });
  const prepared = path.join(home, "versions", `${version}-${hash.slice(0, 12)}`);
  assert.equal(readJSON(path.join(prepared, "box.json")).launch, "rust");
  assert.deepEqual(readJSON(path.join(home, "run/fleet-ready.json")), { version, sha256: hash });
  assert.equal(fs.readFileSync(path.join(home, "bin/box-version"), "utf8"), "0.8.0-beta.1\n");
});
