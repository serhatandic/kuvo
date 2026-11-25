// The update command admits only signed manifests and checks the selected pack before staging.
// Each scenario uses freshly generated test signing keys and isolated local packs.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { Worker } from "node:worker_threads";
import { gzipSync } from "node:zlib";

import { boxBinary, isolatedEnv, launch, openLine, readJSON, relayAddress, request, scratch, startBox, typescriptRuntime, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { seedPeerSetup } from "./peer-fixture.mjs";
import { sessionOf } from "./engine.mjs";
import "./update-helper.mjs";

const { peerTransferMs, keeperTrialMs, releaseDrainMs, releaseDrainRetryMs } = readJSON(new URL("../../box/src/shared/fleet-timing.json", import.meta.url));
const releaseReturnMs = peerTransferMs + keeperTrialMs;
const firstPreparationMs = 90_000;
const leaseElectionMs = 30_000;
const failoverTestMs = firstPreparationMs + leaseElectionMs + keeperTrialMs + releaseReturnMs;
const bridgeTestMs = firstPreparationMs + 2 * keeperTrialMs + 2 * releaseReturnMs;

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

async function fixtureReleaseFeed(t, packBytes, { holdPack = false, engines = {}, engineVersion = "2.2.6" } = {}) {
  const key = keypair();
  const version = "0.8.0";
  const hash = digest(packBytes ?? "signed target");
  let downloads = 0;
  let signed;
  const feed = createServer((req, res) => {
    if (req.url !== "/boxes.json") {
      downloads += 1;
      if (holdPack) return;
      return packBytes ? res.end(packBytes) : res.writeHead(404).end();
    }
    res.end(signed);
  });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const pack = { url: `http://127.0.0.1:${feed.address().port}/target.tar.gz`, sha256: hash };
  signed = encode(envelope({ kuvo: "box/1", version, engine: engineVersion, spreads: true,
    launch: "rust", pack, packs: { [target]: pack }, ...engines }, key));
  const updateEnv = {
    KUVO_TEST_RELEASE_FEED: `http://127.0.0.1:${feed.address().port}/boxes.json`,
    KUVO_TEST_RELEASE_KEY: key.key,
  };
  return { version, hash, signed, updateEnv, downloaded: () => downloads };
}

test("only the local app can offer a release from a full folder path", async t => {
  const { computer } = await bench(t);
  const here = await computer("Local release offer");
  const resident = await here.start().ready();
  const denied = await request(resident, "POST", "kuvo/box/update", { from: "relative" });
  assert.equal(denied.status, 403, denied.text);
  assert.deepEqual(denied.json, { message: "Only Kuvo Code offers Kuvo on this computer a release." });
  for (const origin of ["app", "mac"]) {
    for (const body of [{ from: "relative" }, {}]) {
      const answer = await request(resident, "POST", "kuvo/box/update", body, { "x-kuvo-from": origin });
      assert.equal(answer.status, 400, answer.text);
      assert.deepEqual(answer.json, { message: "Send the full path to the release folder." });
    }
    const missing = await request(resident, "POST", "kuvo/box/update", { from: path.join(here.env.HOME, "no-release") }, { "x-kuvo-from": origin });
    assert.equal(missing.status, 422, missing.text);
    assert.deepEqual(missing.json, { message: "That release can't be read." });
  }
});

test("the local app's exact signed target offer survives restart and is used without downloading the pack", { timeout: releaseReturnMs + firstPreparationMs }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Rust target-specific local release media; the TS signed-offer reference uses receiveRelease.");
  const { computer, relay } = await bench(t);
  const here = await computer("App offered release");
  const home = path.join(here.env.HOME, ".kuvo-code");
  fs.renameSync(here.home, home);
  const version = "0.8.0", previous = "0.8.0-beta.1", engineVersion = "2.2.6";
  const program = process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box";
  const key = keypair();
  const oldBytes = Buffer.from("installed signed pack");
  const oldHash = digest(oldBytes), old = `versions/${previous}-${oldHash.slice(0,12)}/bin`;
  const native = fs.readFileSync(binary.file);
  write(path.join(home, old, program), native, { mode: 0o700 });
  write(path.join(home, old, "box-version"), previous + "\n");
  write(path.join(home, path.dirname(old), "box.json"), encode({ kuvo: "box/1", version: previous, launch: "rust", engine: engineVersion }));
  write(path.join(home, "active-release"), old);
  write(path.join(home, "run/engine.json"), encode({ ...here.engine.record, version: engineVersion }));
  const release = path.join(here.env.HOME, "release-material"), tree = path.join(release, "tree");
  const files = { [`bin/${program}`]: native, "bin/box-version": Buffer.from(version + "\n") };
  if (process.platform !== "win32") {
    for (const name of ["kuvo-start", "kuvo-setsid"]) files[`bin/${name}`] = fs.readFileSync(new URL(`../../box/src/keeper/${name}`, import.meta.url));
  }
  for (const [file, bytes] of Object.entries(files)) write(path.join(tree, file), bytes, { mode: 0o700 });
  fs.mkdirSync(path.join(tree, "bin/opencode-plugins"));
  write(path.join(tree, "box.json"), encode({ kuvo: "box/1", version, launch: "rust", engine: engineVersion,
    files: Object.fromEntries(Object.entries(files).map(([file, bytes]) => [file, digest(bytes)])) }));
  const tar = path.join(release, "signed.tar");
  assert.equal(spawnSync("tar", ["cf", tar, "-C", tree, "box.json", "bin"], { timeout: 90000 }).status, 0);
  const packBytes = gzipSync(fs.readFileSync(tar), { level: 1 });
  const engine = await engineFeed(t, path.join(release, "engine"), undefined, { damageFirst: false });
  const system = process.platform === "win32" ? "windows" : process.platform;
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  let packRequests = 0;
  const feed = createServer((req, res) => {
    if (req.url !== "/boxes.json") packRequests++;
    res.writeHead(404).end();
  });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const url = `http://127.0.0.1:${feed.address().port}`;
  const local = { url: `${url}/local.tar.gz`, sha256: digest(packBytes) };
  const foreignTarget = target === "x86_64-unknown-linux-musl" ? "aarch64-apple-darwin" : "x86_64-unknown-linux-musl";
  const foreign = { url: `${url}/foreign.tar.gz`, sha256: digest("foreign target") };
  const base = { kuvo: "box/1", engine: engineVersion, spreads: true, launch: "rust" };
  const manifest = { ...base, version, pack: foreign, packs: { [target]: local, [foreignTarget]: foreign },
    [system]: { [arch]: engine.build } };
  const signed = encode(envelope(manifest, key));
  const oldPack = { url: `${url}/old.tar.gz`, sha256: oldHash };
  write(path.join(home, path.dirname(old), "boxes.json"), encode(envelope({ ...base, version: previous, pack: oldPack, packs: { [target]: oldPack } }, key)));
  write(path.join(home, path.dirname(old), "pack"), oldBytes);
  const media = path.join(here.env.HOME, "app-offer-media");
  const source = (name, body, signer = key) => {
    const folder = path.join(media, name);
    write(path.join(folder, "boxes.json"), encode(envelope(body, signer)));
    return folder;
  };
  const offer = source("valid", manifest);
  write(path.join(offer, "local.tar.gz"), packBytes);
  // Deliberately no foreign or legacy pack file: the local target is authoritative.
  const damaged = source("damaged", manifest);
  write(path.join(damaged, "local.tar.gz"), "changed bytes");
  const stranger = source("stranger", manifest, keypair());
  const privateRelease = source("private", { ...manifest, spreads: false });
  const missing = source("missing", manifest);
  const older = source("older", { ...manifest, version: "0.7.2" });
  const conflict = source("same-version-conflict", { ...manifest, version: previous });
  const env = { ...here.env, KUVO_HOME: home, KUVO_TEST_RELAY: relay.url, KUVO_RELAY_URL: relay.url,
    KUVO_TEST_RELEASE_FEED: `${url}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key };
  const start = () => startBox({ binary, home, relay, env });
  const keep = (...args) => {
    const done = spawnSync(binary.file, ["keep", ...args], { env, cwd: home, encoding: "utf8", timeout: 15000 });
    assert.equal(done.status, 0, done.error?.message ?? done.stderr);
  };
  let child = start(), keeper;
  try {
    const resident = await child.ready();
    const share = from => request(resident, "POST", "kuvo/box/update", { from }, { "x-kuvo-from": "app" }, firstPreparationMs);
    const refused = await share(stranger);
    assert.equal(refused.status, 422, refused.text);
    assert.deepEqual(refused.json, { message: "It isn't signed with a key this machine trusts." });
    const restricted = await share(privateRelease);
    assert.equal(restricted.status, 422, restricted.text);
    assert.deepEqual(restricted.json, { message: "This release cannot be shared with your setup." });
    const unreadable = await share(missing);
    assert.equal(unreadable.status, 422, unreadable.text);
    assert.deepEqual(unreadable.json, { message: "That release can't be read." });
    const changed = await share(damaged);
    assert.equal(changed.status, 422, changed.text);
    assert.deepEqual(changed.json, { message: "The release's files aren't what it says." });
    for (const folder of [older, conflict]) {
      const ignored = await share(folder);
      assert.equal(ignored.status, 202, ignored.text);
      assert.equal(ignored.json.version, previous);
    }
    const accepted = await share(offer);
    assert.equal(accepted.status, 202, accepted.text);
    assert.equal(accepted.json.version, previous);
    assert.equal(fs.readFileSync(path.join(home, "active-release"), "utf8").trim(), old);
    await child.stop();
    child = undefined;
    // The app's original media can disappear after its accepted handoff.
    fs.rmSync(media, { recursive: true, force: true });
    keeper = launch(binary.file, ["keep"], env, home);
    const selected = `versions/${version}-${local.sha256.slice(0,12)}/bin`;
    await until(() => fs.readFileSync(path.join(home, "active-release"), "utf8").trim() === selected && !fs.existsSync(path.join(home, "run/previous")),
      () => "the keeper did not confirm the signed local offer\n" + keeper.log(), releaseReturnMs);
    const retained = path.join(home, path.dirname(selected));
    assert.equal(digest(fs.readFileSync(path.join(retained, "pack"))), local.sha256);
    assert.deepEqual(readJSON(path.join(retained, "boxes.json")), JSON.parse(signed));
    assert.equal(packRequests, 0, "the accepted local bytes, not a pack URL, must be used");
    assert.equal((await request(readJSON(path.join(home, "run/serve.json")), "GET", "kuvo/update")).json.version, version);
  } finally {
    await child?.stop();
    if (keeper) {
      try { keep("off"); await until(() => keeper.child.exitCode !== null, () => keeper.log(), 15000); }
      finally { await keeper.stop(); }
    }
  }
});

test("a member shares its newer app offer while following the main's earlier signed choice", { timeout: releaseReturnMs }, async t => {
  const binary = boxBinary();
  if (binary.typescript || !fs.existsSync(fileURLToPath(new URL("../../box/dist/kuvo-link.js", import.meta.url)))) return t.skip("Rust target-specific release media and reference relay codec.");
  const { computer, relay } = await bench(t);
  const here = await computer("Member with app media");
  const { main, member } = seedPeerSetup(here.home, relay.url, 2);
  const old = "0.8.0-beta.1", chosen = "0.8.0-beta.2", version = "0.8.0";
  const key = keypair();
  let downloads = 0;
  const feed = createServer((req, res) => {
    if (req.url !== "/boxes.json") downloads++;
    res.writeHead(404).end();
  });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const url = `http://127.0.0.1:${feed.address().port}`;
  const bytes = Buffer.from("app's retained newer signed archive");
  const pack = { url: `${url}/app.tar.gz`, sha256: digest(bytes) };
  const priorBytes = Buffer.from("main chosen archive");
  const prior = { url: `${url}/chosen.tar.gz`, sha256: digest(priorBytes) };
  const base = { kuvo: "box/1", engine: "2.2.6", launch: "rust", spreads: true };
  const signed = encode(envelope({ ...base, version, pack, packs: { [target]: pack } }, key));
  const earlier = encode(envelope({ ...base, version: chosen, pack: prior, packs: { [target]: prior } }, key));
  const source = path.join(here.env.HOME, "app-offer");
  write(path.join(source, "boxes.json"), signed);
  write(path.join(source, "app.tar.gz"), bytes);
  const earlierSource = path.join(here.env.HOME, "earlier-app-media");
  write(path.join(earlierSource, "boxes.json"), earlier);
  write(path.join(earlierSource, "chosen.tar.gz"), priorBytes);
  write(path.join(here.home, "bin/box-version"), old + "\n");
  const options = { KUVO_TEST_RELEASE_FEED: `${url}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key };
  let child = here.start(binary, options);
  const peer = await releaseMailPeer(t, relay, member, 1);
  try {
    const resident = await child.ready();
    await peer.ready();
    peer.announce({ kuvo: old, releaseTarget: target });
    const share = from => request(resident, "POST", "kuvo/box/update", { from }, { "x-kuvo-from": "app" });
    const older = await share(earlierSource);
    assert.equal(older.status, 202, older.text);
    await peer.receiveComputer(facts => facts.candidate === chosen);
    peer.send({ want: chosen, sha256: prior.sha256 });
    const held = await peer.receive(value => value.version === chosen && (value.boxes || value.none));
    assert.equal(held.size, priorBytes.length);
    const olderFolder = path.join(here.home, "updates/app-offers", `${chosen}-${prior.sha256}`);
    const accepted = await share(source);
    assert.equal(accepted.status, 202, accepted.text);
    const first = await peer.receiveComputer(facts => facts.candidate === version);
    assert.equal(first.releaseManifest, signed.toString());
    assert.equal(first.staged, undefined, "the member does not choose its own offer");
    assert.equal(downloads, 0);
    assert.ok(fs.existsSync(olderFolder), "the active transfer protects superseded media");
    peer.send({ version: chosen, sha256: prior.sha256, part: 0 });
    const oldPart = await peer.receive(value => value.version === chosen && value.i === 0);
    assert.deepEqual(Buffer.from(oldPart.d, "base64"), priorBytes);
    peer.send({ want: version, sha256: pack.sha256 });
    const newHeader = await peer.receive(value => value.version === version && (value.boxes || value.none));
    assert.equal(newHeader.size, bytes.length);
    assert.equal((await share(source)).status, 202);
    assert.equal(fs.existsSync(olderFolder), false, "unleased superseded media is retired");
    peer.announce({ kuvo: old, releaseTarget: target, candidate: chosen, releaseManifest: earlier.toString() });
    const choice = path.join(here.home, "updates/release/boxes.json");
    await until(() => fs.existsSync(choice) && fs.readFileSync(choice, "utf8") === earlier.toString(), "the member follows its main's earlier offer");
    fs.rmSync(source, { recursive: true, force: true });
    fs.rmSync(earlierSource, { recursive: true, force: true });
    await child.stop();
    child = here.start(binary, options);
    await child.ready();
    await peer.ready();
    peer.announce({ kuvo: old, releaseTarget: target, candidate: chosen, releaseManifest: earlier.toString() });
    const after = await peer.receiveComputer(facts => facts.candidate === version);
    assert.equal(after.releaseManifest, signed.toString());
    peer.send({ want: version, sha256: pack.sha256 });
    const header = await peer.receive(value => value.version === version && (value.boxes || value.none));
    assert.equal(header.boxes, signed.toString());
    assert.equal(header.size, bytes.length);
    peer.send({ version, sha256: pack.sha256, part: 0 });
    const part = await peer.receive(value => value.version === version && value.i === 0);
    assert.deepEqual(Buffer.from(part.d, "base64"), bytes);
    assert.equal(fs.readFileSync(path.join(here.home, "bin/box-version"), "utf8").trim(), old);
  } finally { await child.stop(); }
});

test("a six-hour release round survives restart and Update Now admits a fresh release", { timeout: releaseReturnMs + firstPreparationMs }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("The TypeScript reference checks the six-hour round and immediate offer.");
  const { computer, relay } = await bench(t);
  const here = await computer("Scheduled update");
  const home = path.join(here.env.HOME, ".kuvo-code");
  fs.renameSync(here.home, home);
  const key = keypair();
  const version = "0.8.0", previous = "0.8.0-beta.1", engineVersion = "2.2.6";
  const program = process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box";
  const old = `versions/${previous}-000000000000`;
  const native = fs.readFileSync(binary.file);
  write(path.join(home, old, "bin", program), native, { mode: 0o700 });
  write(path.join(home, old, "bin/box-version"), previous + "\n");
  write(path.join(home, old, "box.json"), encode({ kuvo: "box/1", version: previous, launch: "rust", engine: engineVersion }));
  write(path.join(home, "active-release"), `${old}/bin`);
  write(path.join(home, "run/engine.json"), encode({ ...here.engine.record, version: engineVersion }));
  const release = path.join(here.env.HOME, "release");
  const tree = path.join(release, "tree");
  const files = { [`bin/${program}`]: native, "bin/box-version": Buffer.from(version + "\n") };
  if (process.platform !== "win32") {
    for (const name of ["kuvo-start", "kuvo-setsid"]) files[`bin/${name}`] = fs.readFileSync(new URL(`../../box/src/keeper/${name}`, import.meta.url));
  }
  for (const [file, bytes] of Object.entries(files)) write(path.join(tree, file), bytes, { mode: 0o700 });
  fs.mkdirSync(path.join(tree, "bin/opencode-plugins"));
  write(path.join(tree, "box.json"), encode({ kuvo: "box/1", version, launch: "rust", engine: engineVersion,
    files: Object.fromEntries(Object.entries(files).map(([file, bytes]) => [file, digest(bytes)])) }));
  const tar = path.join(release, "signed.tar");
  assert.equal(spawnSync("tar", ["cf", tar, "-C", tree, "box.json", "bin"], { timeout: 90000 }).status, 0);
  const packBytes = gzipSync(fs.readFileSync(tar), { level: 1 });
  const engine = await engineFeed(t, path.join(release, "engine"), undefined, { damageFirst: false });
  const system = process.platform === "win32" ? "windows" : process.platform;
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  let checks = 0, packs = 0, signed;
  const feed = createServer((req, res) => {
    if (req.url === "/boxes.json") { checks++; return res.end(signed); }
    if (req.url === "/fresh.tar.gz") { packs++; return res.end(packBytes); }
    res.writeHead(404).end();
  });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const url = `http://127.0.0.1:${feed.address().port}`;
  const pack = { url: `${url}/fresh.tar.gz`, sha256: digest(packBytes) };
  const older = { url: `${url}/old.tar.gz`, sha256: digest("the old signed pack") };
  const state = path.join(home, "run/update.json"), kept = path.join(home, "updates/release/boxes.json");
  const base = { kuvo: "box/1", engine: engineVersion, spreads: true, launch: "rust" };
  write(kept, encode(envelope({ ...base, version: previous, pack: older, packs: { [target]: older } }, key)));
  signed = encode(envelope({ ...base, version, released: new Date().toISOString(), pack, packs: { [target]: pack },
    [system]: { [arch]: engine.build } }, key));
  const roundSize = 6 * 3600_000;
  const offset = parseInt(digest(`${os.hostname()} ${home}`).slice(0, 8), 16) % roundSize;
  const round = () => String(Math.floor((Date.now() + offset) / roundSize));
  const env = { ...here.env, KUVO_HOME: home, KUVO_TEST_RELAY: relay.url, KUVO_RELAY_URL: relay.url, KUVO_TEST_RELEASE_AGE: "1", KUVO_TEST_RELEASE_FEED: `${url}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key };
  const start = () => startBox({ binary, home, relay, env });
  const keep = (...args) => {
    const done = spawnSync(binary.file, ["keep", ...args], { env, cwd: home, encoding: "utf8", timeout: 15000 });
    assert.equal(done.status, 0, done.error?.message ?? done.stderr);
  };
  let child = start(), keeper;
  try {
    await child.ready();
    await until(() => fs.existsSync(state) && readJSON(state).latest === version,
      () => `the initial automatic check did not finish\n${child.log()}`);
    assert.equal(readJSON(state).round, round());
    assert.equal(checks, 1);
    assert.equal(packs, 0, "ordinary checks wait three days after publication");
    const checked = readJSON(state).checked;
    await child.stop();
    child = start();
    await child.ready();
    // The resident runs its fleet round immediately and every two seconds.
    await new Promise(resolve => setTimeout(resolve, 4000));
    assert.equal(checks, 1, "the same automatic round must not fetch after restart");
    assert.equal(readJSON(state).checked, checked);
    assert.equal(packs, 0);
    await child.stop();
    // A saved earlier round proves the elapsed-round branch without waiting six hours.
    write(state, encode({ ...readJSON(state), round: String(Number(round()) - 1) }));
    child = start();
    await child.ready();
    await until(() => checks === 2 && readJSON(state).round === round(), "the next elapsed round must check once");
    assert.equal(packs, 0);
    await child.stop();
    child = undefined;
    keeper = launch(binary.file, ["keep"], env, home);
    const serve = path.join(home, "run/serve.json");
    const resident = await until(async () => {
      try { const record = readJSON(serve); return (await request(record, "GET", "kuvo/info")).status === 200 && record; } catch { return false; }
    }, () => "the earlier resident did not answer\n" + keeper.log(), 25000);
    const manual = await request(resident, "POST", "kuvo/update", { now: true });
    assert.equal(manual.status, 202, manual.text);
    await until(() => checks === 3 && packs === 1 && fs.readFileSync(kept, "utf8") === signed.toString(),
      "the manual check must pick the fresh signed offer despite the earlier choice");
    const selected = `versions/${version}-${pack.sha256.slice(0,12)}/bin`;
    await until(() => fs.readFileSync(path.join(home, "active-release"), "utf8").trim() === selected && !fs.existsSync(path.join(home, "run/previous")),
      () => "the keeper did not confirm the manual signed release\n" + keeper.log(), releaseReturnMs);
    const current = await request(readJSON(serve), "GET", "kuvo/update");
    assert.equal(current.json.version, version);
    assert.equal(readJSON(state).round, round());
  } finally {
    await child?.stop();
    if (keeper) {
      try { keep("off"); await until(() => keeper.child.exitCode !== null, () => keeper.log(), 15000); }
      finally { await keeper.stop(); }
    }
  }
});

test("an empty release feed is checked only once in the saved automatic round", { timeout: 35000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the native automatic release worker.");
  const { computer } = await bench(t);
  const here = await computer("Empty feed");
  write(path.join(here.home, "bin/box-version"), "0.8.0-beta.1\n");
  const key = keypair();
  let checks = 0;
  const feed = createServer((_req, res) => { checks++; res.writeHead(404).end(); });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const env = { KUVO_TEST_RELEASE_FEED: `http://127.0.0.1:${feed.address().port}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key };
  const state = path.join(here.home, "run/update.json");
  let child = here.start(binary, env);
  await child.ready();
  const kept = await until(() => fs.existsSync(state) && readJSON(state).round, "a completed 404 check must save its round");
  assert.equal(checks, 1);
  await child.stop();
  child = here.start(binary, env);
  const resident = await child.ready();
  await new Promise(resolve => setTimeout(resolve, 4000));
  assert.equal(checks, 1);
  assert.equal(readJSON(state).round, kept);
  assert.equal((await request(resident, "GET", "kuvo/update")).json.latest, null);
});

test("a failed fleet download can be retried for the same signed target", { timeout: 30000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { computer } = await bench(t);
  const here = await computer("Standalone", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const { version, hash, updateEnv, downloaded } = await fixtureReleaseFeed(t);
  const installed = "0.8.0-beta.1";
  write(path.join(here.home, "bin/box-version"), installed + "\n");
  const child = here.start(binary, updateEnv);
  await child.ready();
  await until(() => downloaded() === 1, "the updater must try its signed target");
  const failed = path.join(here.home, "run/fleet-cannot.json");
  await until(() => fs.existsSync(failed), () => `the fleet failure must be available for retry\n${child.log()}`);
  assert.deepEqual([readJSON(failed).version, readJSON(failed).sha256, readJSON(failed).fixable], [version, hash, true]);
  await child.stop();
  const restarted = here.start(binary, updateEnv);
  const resident = await restarted.ready();
  const stale = await request(resident, "POST", "kuvo/update/retry", { target: version, targetSha256: digest("older offer") });
  assert.equal(stale.status, 409, stale.text);
  assert.equal(downloaded(), 1);
  const retry = await request(resident, "POST", "kuvo/update/retry", { target: version, targetSha256: hash });
  assert.equal(retry.status, 202, retry.text);
  assert.deepEqual([retry.json.target, retry.json.targetSha256, retry.json.state, retry.json.canRetry], [version, hash, "cannot", false]);
  await until(() => downloaded() === 2 && fs.existsSync(failed), "the updater must try the same signed target once more");
  assert.equal((await request(resident, "GET", "kuvo/update")).json.version, installed);
});

async function pairedUpdateComputers(t) {
  const { computer, leases, relay } = await bench(t);
  const make = name => computer(name, undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const [main, member] = await Promise.all([make("Main"), make("Member")]);
  const old = main.start();
  const previous = await old.ready();
  const setup = await request(previous, "GET", "kuvo/setup");
  await until(() => leases.leaseFor(setup.json.setup), "the main must hold a lease");
  const invite = await request(previous, "POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  const joined = await member.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  await old.stop();
  return { main, member, relay };
}

async function runsOn(record) {
  const line = await openLine(record, "", "kuvo/setup/runs-on/events");
  try {
    assert.equal(line.status, 200);
    for await (const frame of line.frames()) return frame;
  } finally {
    line.close();
  }
  assert.fail("Runs on returned no snapshot");
}

test("the returning bridge stays out of work placement during its Rust trial", { timeout: 60000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust resident's trial announcement.");
  const { main, member, relay } = await pairedUpdateComputers(t);
  const { body, key } = box(t);
  const feed = createServer((_request, response) => response.writeHead(404).end());
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const url = `http://127.0.0.1:${feed.address().port}`;
  const pack = { url: `${url}/rust.tar.gz`, sha256: digest("Rust pack") };
  const bridgePack = { url: `${url}/bridge.tar.gz`, sha256: digest("bridge pack") };
  const bridgeVersion = "0.8.0-beta.49";
  const bridge = encode(envelope({ ...body, version: bridgeVersion, pack: bridgePack }, key)).toString();
  const signed = encode(envelope({ ...body, pack, bridge, launch: "rust", packs: { [target]: pack } }, key));
  const selected = `versions/${body.version}-${pack.sha256.slice(0, 12)}`;
  const previous = `versions/${bridgeVersion}-${bridgePack.sha256.slice(0, 12)}`;
  const trial = path.join(member.home, "run/previous");
  for (const peer of [main, member]) {
    write(path.join(peer.home, "active-release"), selected + "/bin");
    write(path.join(peer.home, selected, "boxes.json"), signed);
  }
  write(path.join(member.home, previous, "boxes.json"), bridge);
  write(trial, previous + "/bin");
  const sender = "ses_confirmedWork", recipient = "ses_returningWork";
  const mainChild = "ses_childOfReturning", memberChild = "ses_childOfConfirmed";
  for (const [computer, id, child, lead] of [[main, sender, mainChild, recipient], [member, recipient, memberChild, sender]]) {
    const directory = path.join(computer.home, "chats");
    fs.mkdirSync(directory, { recursive: true });
    computer.engine.sessions.push(sessionOf({ id, title: id, directory }));
    computer.engine.sessions.push({ ...sessionOf({ id: child, title: child, directory }), metadata: {
      kuvo: { startedBy: lead, line: [lead], wake: ["finished"], depth: 1 },
    } });
  }
  const updateEnv = { KUVO_TEST_RELEASE_FEED: `${url}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key };
  const [observer, returning] = await Promise.all([main.start(binary, updateEnv).ready(), member.start(binary, updateEnv).ready()]);
  const peerKey = (await request(returning, "GET", "kuvo/setup")).json.you.key;
  const announced = path.join(main.home, "run/peer-agents.json");
  const facts = () => fs.existsSync(announced) && readJSON(announced)[peerKey]?.computer;
  await until(() => {
    const said = facts();
    return said?.kuvo === bridgeVersion && said.releaseTarget === target &&
      said.installedSha256 === bridgePack.sha256 && said.switching?.sha256 === pack.sha256;
  }, "the Rust resident should announce its trial while the bridge remains current");
  const row = async () => (await runsOn(observer)).computers.find(item => item.computer.id === peerKey);
  const waiting = await until(row, "the returning computer should have a placement row");
  assert.deepEqual([waiting.away, waiting.updating], [true, true]);
  const mainKey = (await request(observer, "GET", "kuvo/setup")).json.you.key;
  await until(() => {
    try { return readJSON(path.join(member.home, "run/peer-agents.json"))[mainKey]?.features?.includes("moves.two-computers"); }
    catch { return false; }
  }, "the returning resident should know the confirmed main");
  const codec = await referenceMailCodec(t);
  const mail = (from, to, peer, fields) => {
    const source = readJSON(path.join(from.home, "run/link.json"));
    const destination = readJSON(path.join(to.home, "run/link.json"));
    const secret = Buffer.from(source.key, "base64url"), key = Buffer.from(peer, "base64url");
    const payload = codec.frame(codec.Frame.mail, 0, codec.sealOneWay(encode({ at: Date.now(), ...fields }), secret, key).message);
    const packet = { from: relayAddress(Buffer.from(source.secret, "base64url")),
      to: relayAddress(Buffer.from(destination.secret, "base64url")), payload };
    relay.replay(packet);
    return { packet, secret, key };
  };
  const calls = [];
  for (const [from, to, lead, child, peer, callerKey] of [
    [main, member, sender, memberChild, peerKey, mainKey],
    [member, main, recipient, mainChild, mainKey, peerKey],
  ]) {
    write(path.join(to.home, "run/work/homes.json"), encode({ [lead]: { key: callerKey, at: Date.now() } }));
    const id = from === main ? "cal_trialStopWorkMain0001" : "cal_trialStopWorkMember0001";
    const file = path.join(from.home, "run/outbox/sending", id + ".json");
    const at = Date.now();
    const call = { tool: "stop_session", args: { session: child }, as: { name: "Lead", free: true, rules: [] } };
    const saved = { call, from: lead, to: "", peer, at, wait: false, quiet: true };
    write(file, encode(saved));
    // An already in-flight authenticated request must be ignored during the trial too.
    const inFlight = mail(from, to, peer, { call: { id, from: lead, made: at, at, ...call } });
    calls.push({ id, file, to, child, inFlight, result: path.join(to.home, "run/work/call-results", id + ".json") });
  }
  const pending = [];
  for (const [computer, local, remote, here, there] of [
    [main, sender, recipient, mainKey, peerKey], [member, recipient, sender, peerKey, mainKey],
  ]) {
    const outbox = path.join(computer.home, "run/outbox/sending");
    const remoteFile = path.join(outbox, "ltr_trialWork.json");
    const controlFile = path.join(outbox, "ltr_localWork.json");
    const letter = (to, peer, words, at) => ({ to, peer, text: words, at, wait: false,
      note: { from: "Your computers", words, delivery: "queue" } });
    write(remoteFile, JSON.stringify(letter(remote, there, "Work after confirmation", Date.now() - 1000)));
    write(controlFile, JSON.stringify(letter(local, here, "Local work", Date.now())));
    await until(() => !fs.existsSync(controlFile), "the local letter should finish a real outbox round");
    assert.equal(readJSON(remoteFile).sentTo, undefined, "the remote letter should not have been attempted");
    pending.push(remoteFile);
  }
  for (const [resident, id, to] of [[observer, sender, peerKey], [returning, recipient, mainKey]]) {
    await until(async () => (await request(resident, "GET", `api/session/${id}`)).status === 200,
      "the saved conversation should be readable");
    const direct = await request(resident, "POST", "kuvo/moves", { kind: "chat", id, to, receipt: true });
    assert.equal(direct.status, 409, direct.text);
    const places = await request(resident, "GET", `kuvo/moves/places?kind=chat&id=${id}`);
    assert.equal(places.status, 200, places.text);
    assert.equal(places.json.places.find(place => place.key === to)?.ok, false);
  }

  // Four half-second remote-work rounds: a held call cannot be used as the normal away probe.
  await new Promise(resolve => setTimeout(resolve, 2000));
  for (const { file, to, child, result } of calls) {
    assert.equal(readJSON(file).sentTo, undefined, "the held call must not be marked sent");
    assert.equal(fs.existsSync(result), false, "an in-flight call must not be accepted on trial");
    assert.equal(to.engine.interrupted.filter(each => each.session === child).length, 0);
  }

  // The keeper owns confirmation. Publish its saved result and let the resident observe it.
  fs.unlinkSync(trial);
  await until(() => facts()?.kuvo === body.version && !facts()?.switching, "the main should hear the confirmed Rust release");
  await until(async () => {
    const ready = await row();
    return ready && !ready.away && !ready.updating;
  }, "the confirmed Rust resident should be available for work");
  await until(() => pending.every(file => !fs.existsSync(file)), "both saved remote letters should settle after confirmation");
  for (const [computer, session] of [[main, sender], [member, recipient]]) {
    await until(() => computer.engine.messages.get(session)?.some(message => message.text?.includes("Work after confirmation")),
      "the remote work should arrive in the conversation");
    assert.equal(computer.engine.messages.get(session).filter(message => message.text?.includes("Work after confirmation")).length, 1);
  }
  await until(() => calls.every(({ result }) => fs.existsSync(result)),
    "the saved work calls should reach the actual resident adapter after confirmation", 20000);
  for (const { id, to, child, inFlight } of calls) {
    // The exact packet held on trial is valid, not just an unrecognized wire shape.
    const before = relay.delivered.length;
    relay.replay(inFlight.packet);
    await until(() => relay.delivered.slice(before).some(({ from, to, payload }) => {
      if (!from.equals(inFlight.packet.to) || !to.equals(inFlight.packet.from)) return false;
      const packet = codec.readFrame(payload);
      if (packet?.kind !== codec.Frame.mail) return false;
      const value = JSON.parse(codec.openOneWay(packet.body, inFlight.secret, inFlight.key).payload);
      return value.delivered?.id === id && value.delivered.ok === true;
    }), "the receiver should acknowledge a replay of the previously held call");
    assert.equal(to.engine.interrupted.filter(each => each.session === child).length, 1,
      "the real stop call must take effect once, only after confirmation");
  }
  const direct = await request(observer, "POST", "kuvo/moves", { kind: "chat", id: sender, to: peerKey, receipt: true });
  assert.equal(direct.status, 202, direct.text);
});

test("the main can ask a reachable member to retry its failed fleet download", { timeout: 60000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { main, member } = await pairedUpdateComputers(t);

  const { version, hash, signed, updateEnv, downloaded } = await fixtureReleaseFeed(t);
  for (const peer of [main, member]) write(path.join(peer.home, "bin/box-version"), "0.8.0-beta.1\n");
  const [first, second] = await Promise.all([main.start(binary, updateEnv).ready(), member.start(binary, updateEnv).ready()]);
  const memberSetup = await request(second, "GET", "kuvo/setup");
  const key = memberSetup.json.you.key;
  const mainFailure = path.join(main.home, "run/fleet-cannot.json");
  const memberFailure = path.join(member.home, "run/fleet-cannot.json");
  await until(() => downloaded() === 2 && fs.existsSync(mainFailure) && fs.existsSync(memberFailure),
    "both computers must see the failed download");
  const announced = path.join(main.home, "run/peer-agents.json");
  const peerFailure = await until(() => fs.existsSync(announced) && readJSON(announced)[key]?.computer?.cannot,
    "the main must receive the member's failed target");
  assert.deepEqual([peerFailure.version, peerFailure.sha256, peerFailure.fixable], [version, hash, true]);
  const initial = file => fs.existsSync(file) && fs.statSync(file, { bigint: true }).ino;
  const oldMain = initial(mainFailure);
  const oldMember = initial(memberFailure);
  // The main already switched while the member's download was unavailable.
  const release = `versions/${version}-${hash.slice(0, 12)}`;
  write(path.join(main.home, release, "boxes.json"), signed);
  write(path.join(main.home, "active-release"), `${release}/bin`);
  const row = await until(async () => {
    const answer = await request(first, "GET", "kuvo/computers");
    const update = answer.json?.computers?.find(row => row.key === key)?.fleetUpdate;
    return answer.status === 200 && update?.state === "cannot" && update;
  }, "the member row must show the signed failure");
  assert.deepEqual(row, {
    current: "0.8.0-beta.1", target: version, targetSha256: hash, state: "cannot",
    reason: peerFailure.reason, fixable: true, canRetry: true,
    line: { words: `beta.1 · ${peerFailure.reason}`, tone: "warning", short: "Can’t update" },
  });
  const stale = await request(first, "POST", "kuvo/update/retry", { target: version, targetSha256: digest("older offer"), key });
  assert.equal(stale.status, 409, stale.text);
  const retry = await request(first, "POST", "kuvo/update/retry", { target: version, targetSha256: hash, key });
  assert.equal(retry.status, 202, retry.text);
  assert.deepEqual([retry.json.target, retry.json.targetSha256, retry.json.canRetry], [version, hash, false]);
  await until(() => downloaded() === 3 && initial(memberFailure) && initial(memberFailure) !== oldMember,
    "the member must try its signed target again");
  assert.equal(initial(mainFailure), oldMain);
});

test("a member keeps its signed switching row during a quiet relay restart", { timeout: 60000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { main, member } = await pairedUpdateComputers(t);
  const { version, hash, signed, updateEnv } = await fixtureReleaseFeed(t);
  const installed = "0.8.0-beta.1";
  for (const computer of [main, member]) write(path.join(computer.home, "bin/box-version"), installed + "\n");
  const sender = member.start(binary, updateEnv);
  let observer = main.start(binary, updateEnv);
  const [first, second] = await Promise.all([observer.ready(), sender.ready()]);
  const key = (await request(second, "GET", "kuvo/setup")).json.you.key;
  const row = async resident => {
    const answer = await request(resident, "GET", "kuvo/computers");
    return answer.json?.computers?.find(row => row.key === key)?.fleetUpdate;
  };
  await until(async () => (await row(first))?.state === "cannot",
    "the main must receive the member's first preparation status");

  const release = `versions/${version}-${hash.slice(0, 12)}`;
  write(path.join(member.home, release, "boxes.json"), signed);
  write(path.join(member.home, "run/previous"), `versions/${installed}-000000000000/bin`);
  write(path.join(member.home, "active-release"), `${release}/bin`);
  const switching = async resident => until(async () => {
    const update = await row(resident);
    return update?.state === "updating" && update;
  }, "the member's row must report the signed switch on trial");
  const active = await switching(first);
  assert.deepEqual([active.current, active.target, active.targetSha256, active.stage, active.canRetry],
    [installed, version, hash, "switching", false]);
  assert.deepEqual(active.line, { words: `Switching to ${version}…`, tone: "working", phase: "switching" });
  const retry = await request(first, "POST", "kuvo/update/retry", { key, target: version, targetSha256: hash });
  assert.equal(retry.status, 409, retry.text);
  const localRetry = await request(second, "POST", "kuvo/update/retry", { target: version, targetSha256: hash });
  assert.equal(localRetry.status, 409, localRetry.text);

  await sender.stop();
  const heardAt = async (age, { withoutComputer = false } = {}) => {
    await observer.stop();
    const peers = path.join(main.home, "run/peer-agents.json");
    const announced = readJSON(peers);
    const heard = Date.now() - age;
    announced[key].heard = heard;
    if (withoutComputer) delete announced[key].computer;
    write(peers, encode(announced));
    const reach = path.join(main.home, "run/peer-reach.json");
    write(reach, encode({ [key]: heard }));
    observer = main.start(binary, updateEnv);
    return { resident: await observer.ready(), heard };
  };
  const quiet = await heardAt(3 * 60_000);
  assert.deepEqual(await switching(quiet.resident), active);
  const offline = async (age, words, withoutComputer = false) => {
    const expired = await heardAt(age, { withoutComputer });
    const update = await until(async () => {
      const update = await row(expired.resident);
      return update?.state === "off" && update;
    }, `the quiet computer must say ${words}`);
    assert.equal(update.seen, expired.heard);
    assert.deepEqual(update.line, { words, tone: "off", phase: "off" });
    if (withoutComputer) assert.equal(update.current, null);
  };
  await offline(11 * 60_000 + 10_000, "beta.1 · seen 11 min ago");
  await offline(2 * 3_600_000 + 5 * 60_000, "beta.1 · seen 2h ago");
  await offline(26 * 3_600_000, "beta.1 · seen yesterday");
  await offline(3 * 86_400_000 + 60_000, "beta.1 · seen 3 days ago");
  await offline(5_000, "Seen just now", true);
});

test("a quiet member keeps preparing or shows its staged release switching", { timeout: 60000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { main, member } = await pairedUpdateComputers(t);
  const { version, hash, signed, updateEnv, downloaded } = await fixtureReleaseFeed(t, undefined, { holdPack: true });
  const installed = "0.8.0-beta.1";
  for (const computer of [main, member]) write(path.join(computer.home, "bin/box-version"), installed + "\n");
  let observer = main.start(binary, updateEnv);
  let sender = member.start(binary, updateEnv);
  const [first, second] = await Promise.all([observer.ready(), sender.ready()]);
  const key = (await request(second, "GET", "kuvo/setup")).json.you.key;
  const peers = path.join(main.home, "run/peer-agents.json");
  const reach = path.join(main.home, "run/peer-reach.json");
  const facts = () => fs.existsSync(peers) && readJSON(peers)[key]?.computer;
  const rowIn = (resident, state) => until(async () => {
    const answer = await request(resident, "GET", "kuvo/computers");
    const row = answer.json?.computers?.find(computer => computer.key === key)?.fleetUpdate;
    return row?.state === state && row;
  }, `the member must report ${state}`);
  const quiet = async age => {
    await sender.stop();
    await observer.stop();
    const announced = readJSON(peers);
    const heard = Date.now() - age;
    announced[key].heard = heard;
    write(peers, encode(announced));
    write(reach, encode({ [key]: heard }));
    observer = main.start(binary, updateEnv);
    return observer.ready();
  };

  await until(() => downloaded() >= 2 && facts()?.kuvo === installed,
    "the main must have heard from its member while both pack downloads are held");
  const preparing = await rowIn(first, "getting-ready");
  assert.deepEqual(await rowIn(await quiet(5_000), "getting-ready"), preparing);

  // Resume with the normal preparation receipt already written, before any trial.
  const release = path.join(member.home, `versions/${version}-${hash.slice(0, 12)}`);
  write(path.join(release, "box.json"), encode({ kuvo: "box/1", version, launch: "rust" }));
  write(path.join(release, "boxes.json"), signed);
  const program = path.join(release, "bin", process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box");
  fs.mkdirSync(path.dirname(program), { recursive: true });
  fs.copyFileSync(binary.file, program);
  write(path.join(member.home, "run/fleet-ready.json"), encode({ version, sha256: hash }));
  sender = member.start(binary, updateEnv);
  await sender.ready();
  const ready = await rowIn(await observer.ready(), "ready");
  assert.deepEqual([ready.current, ready.target, ready.targetSha256], [installed, version, hash]);
  assert.deepEqual(facts().staged, { version, sha256: hash });
  assert.equal(facts().switching, undefined);
  const switching = await rowIn(await quiet(3 * 60_000), "updating");
  assert.deepEqual([switching.current, switching.target, switching.targetSha256, switching.stage, switching.canRetry],
    [installed, version, hash, "switching", false]);
  assert.deepEqual(switching.line, { words: `Switching to ${version}…`, tone: "working", phase: "switching" });
});

test("the computers list shows a retryable failure for the signed fleet target", { timeout: 30000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { computer } = await bench(t);
  const here = await computer("Standalone", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const { version, hash, updateEnv } = await fixtureReleaseFeed(t);
  const installed = "0.8.0-beta.1";
  write(path.join(here.home, "bin/box-version"), installed + "\n");
  const resident = await here.start(binary, updateEnv).ready();
  const failed = path.join(here.home, "run/fleet-cannot.json");
  await until(() => fs.existsSync(failed), "the updater must record its failure");
  const { reason } = readJSON(failed);
  const progress = await until(async () => {
    const answer = await request(resident, "GET", "kuvo/computers");
    const update = answer.json?.computers?.find(row => row.here)?.fleetUpdate;
    return answer.status === 200 && update?.state === "cannot" && update;
  }, "the computer row must include the retryable failure");
  assert.deepEqual(progress, {
    current: installed, target: version, targetSha256: hash, state: "cannot",
    reason, fixable: true, canRetry: true,
    line: { words: `beta.1 · ${reason}`, tone: "warning", short: "Can’t update" },
  });
});

test("an invalid signed pack stays blocked without offering retry", { timeout: 30000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { computer } = await bench(t);
  const here = await computer("Standalone", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const { version, hash, updateEnv, downloaded } = await fixtureReleaseFeed(t, Buffer.from("not a release archive"));
  const installed = "0.8.0-beta.1";
  write(path.join(here.home, "bin/box-version"), installed + "\n");
  const starting = here.start(binary, updateEnv);
  const before = await starting.ready();
  const blocked = resident => until(async () => {
    const answer = await request(resident, "GET", "kuvo/computers");
    const row = answer.json?.computers?.find(computer => computer.here)?.fleetUpdate;
    return row?.state === "cannot" && row;
  }, () => `the invalid signed pack must leave a blocked row after ${downloaded()} downloads`);
  const failure = await blocked(before);
  assert.deepEqual([failure.current, failure.target, failure.targetSha256, failure.fixable, failure.canRetry],
    [installed, version, hash, false, false]);
  assert.deepEqual(failure.line, {
    words: `beta.1 · ${failure.reason} · Updates with the next release`,
    tone: "blocked",
  });
  await starting.stop();
  const after = await here.start(binary, updateEnv).ready();
  assert.deepEqual(await blocked(after), failure);
  const retry = await request(after, "POST", "kuvo/update/retry", { target: version, targetSha256: hash });
  assert.equal(retry.status, 409, retry.text);
  assert.equal(downloaded(), 1);
  assert.equal((await request(after, "GET", "kuvo/update")).json.version, installed);
});

test("a keeper refusal blocks only its signed target", { timeout: 30000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { computer } = await bench(t);
  const here = await computer("Standalone", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const { version, hash, updateEnv, downloaded } = await fixtureReleaseFeed(t);
  const installed = "0.8.0-beta.1";
  const refusal = path.join(here.home, "run/update-cannot");
  const time = 1740000000;
  const reason = `${version} didn't start here`;
  write(path.join(here.home, "bin/box-version"), installed + "\n");
  write(refusal, `${time} ${version} ${hash.slice(0, 12)}\n`);
  const first = here.start(binary, updateEnv);
  const resident = await first.ready();
  const blocked = await until(async () => {
    const answer = await request(resident, "GET", "kuvo/computers");
    const row = answer.json?.computers?.find(computer => computer.here)?.fleetUpdate;
    return row?.state === "cannot" && row;
  }, "the keeper's refusal must appear on the computer row");
  assert.deepEqual(blocked, {
    current: installed, target: version, targetSha256: hash, state: "cannot",
    reason, fixable: false, canRetry: false,
    line: { words: `beta.1 · ${reason} · Updates with the next release`, tone: "blocked" },
  });
  assert.deepEqual((await request(resident, "GET", "kuvo/update")).json.last,
    { at: time * 1000, result: "failed", to: version, message: reason });
  assert.equal((await request(resident, "POST", "kuvo/update/retry", { target: version, targetSha256: hash })).status, 409);
  assert.equal(downloaded(), 0);
  await first.stop();

  write(refusal, `${time} ${version} ${digest("another signed pack").slice(0, 12)}\n`);
  const second = await here.start(binary, updateEnv).ready();
  await until(() => downloaded() === 1, "a refusal for another pack must leave this download available");
  const row = await until(async () => {
    const answer = await request(second, "GET", "kuvo/computers");
    return answer.json?.computers?.find(computer => computer.here)?.fleetUpdate?.canRetry;
  }, "the actual download failure must offer retry");
  assert.equal(row, true);
});

test("the computers list keeps the update visible from trial to settlement", { timeout: 45000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { computer } = await bench(t);
  const make = name => computer(name, undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const here = await make("Standalone");
  const { version, hash, signed, updateEnv } = await fixtureReleaseFeed(t);
  const installed = "0.8.0-beta.1";
  write(path.join(here.home, "bin/box-version"), installed + "\n");
  const running = here.start(binary, updateEnv);
  let resident = await running.ready();
  const rowInState = state => until(async () => {
    const answer = await request(resident, "GET", "kuvo/computers");
    const row = answer.json?.computers?.[0]?.fleetUpdate;
    return row?.state === state && { row, summary: answer.json.fleetUpdate };
  }, `the computers list must show ${state}`);
  const { row: failed, summary: warning } = await rowInState("cannot");
  assert.ok(warning, "the computers list must include the fleet warning");
  assert.deepEqual([warning.shown, warning.stage, warning.warning, warning.title, warning.detail],
    [true, "failed", true, `Standalone can’t update to ${version}`, `Standalone: ${failed.reason}`]);

  // Hold the keeper's publication at its trial boundary until the resident describes it.
  const release = `versions/${version}-${hash.slice(0, 12)}`;
  const previous = path.join(here.home, "run/previous");
  write(path.join(here.home, release, "boxes.json"), signed);
  write(previous, `versions/${installed}-000000000000/bin`);
  write(path.join(here.home, "active-release"), `${release}/bin`);
  const { row: switching, summary: moving } = await rowInState("updating");
  assert.deepEqual(switching, {
    current: installed, target: version, targetSha256: hash, state: "updating", stage: "switching", canRetry: false,
    line: { words: `Switching to ${version}…`, tone: "working", phase: "switching" },
  });

  assert.deepEqual([moving.shown, moving.stage, moving.phase, moving.title, moving.detail],
    [true, "switching", "switching", `Switching to ${version} · 0 of 1 done`, "1 still on beta.1."]);
  fs.unlinkSync(previous);
  const { row: done, summary: settled } = await rowInState("up-to-date");
  assert.deepEqual(done, {
    current: version, currentSha256: hash, target: version, targetSha256: hash, state: "up-to-date", canRetry: false,
    line: { words: `${version} · Up to date`, tone: "done", phase: "done" },
  });
  assert.deepEqual([settled.shown, settled.phase, settled.title, settled.detail],
    [true, "done", `Up to date on ${version}`, undefined]);
  const record = path.join(here.home, "run/fleet-finished.json");
  const saved = readJSON(record);
  assert.equal(saved.target, version);
  assert.ok(Number.isFinite(saved.finishedAt));
  await running.stop();
  const resumed = here.start(binary, updateEnv);
  resident = await resumed.ready();
  const { summary: restarted } = await rowInState("up-to-date");
  assert.equal(restarted.shown, true);
  assert.deepEqual(readJSON(record), saved);
  await resumed.stop();
  const expiredReceipt = { ...saved, finishedAt: saved.finishedAt - 600_001 };
  write(record, encode(expiredReceipt));
  resident = await here.start(binary, updateEnv).ready();
  const { summary: expired } = await rowInState("up-to-date");
  assert.deepEqual([expired.phase, expired.shown], ["done", false]);
  assert.deepEqual(readJSON(record), expiredReceipt);
  const newComputer = await make("Already current");
  write(path.join(newComputer.home, release, "boxes.json"), signed);
  write(path.join(newComputer.home, "active-release"), `${release}/bin`);
  resident = await newComputer.start(binary, updateEnv).ready();
  const { summary: unseen } = await rowInState("up-to-date");
  assert.deepEqual([unseen.phase, unseen.shown], ["done", false]);
  assert.equal(fs.existsSync(path.join(newComputer.home, "run/fleet-finished.json")), false);
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

async function engineFeed(t, release, fixture, { damageFirst = !fixture } = {}) {
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
    const { appendFileSync, createReadStream, existsSync } = require("node:fs");
    const { parentPort, workerData } = require("node:worker_threads");
    let requests = 0;
    const server = createServer((request, response) => {
      if (request.url !== "/engine.tar.gz") {
        response.writeHead(404).end(); return;
      }
      appendFileSync(workerData.calls, "download\n");
      if (!existsSync(workerData.archive)) { response.writeHead(404).end(); return; }
      if (++requests === 1 && workerData.damageFirst) { response.end("incomplete download"); return; }
      createReadStream(workerData.archive).pipe(response);
    });
    server.listen(0, "127.0.0.1", () => parentPort.postMessage(server.address().port));
  `, { eval: true, workerData: { archive, calls, damageFirst } });
  t.after(() => feed.terminate());
  const [port] = await once(feed, "message");
  return {
    archive, calls, executable, engine,
    build: {
      url: `http://127.0.0.1:${port}/engine.tar.gz`,
      integrity: "sha512-" + createHash("sha512").update(fs.readFileSync(archive)).digest("base64"),
    },
  };
}

for (const mode of ["switch", "rollback", "borrowed"]) test({
  switch: "a differing engine finishes its turn and carries waiting work through a release switch",
  rollback: "a differing engine restores waiting work after a failed release trial",
  borrowed: "a differing engine leaves a borrowed worker's inbox alone",
}[mode], { timeout: firstPreparationMs + releaseReturnMs + releaseDrainRetryMs }, async t => {
  const rollback = mode === "rollback";
  const borrowed = mode === "borrowed";
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust keeper and engine supervisor.");
  const { computer } = await bench(t);
  const here = await computer("engine-release-drain", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const { home, engine } = here;
  const workspace = path.join(home, "workspace");
  const oldVersion = rollback ? "2.2.6" : "2.2.5", newVersion = rollback ? "2.2.7" : "2.2.6";
  const exe = process.platform === "win32" ? "opencode.exe" : "opencode";
  const oldEngine = borrowed ? path.join(here.env.HOME, `borrowed-${exe}`) : path.join(home, "engine", oldVersion, exe);
  const runtime = typescriptRuntime();
  const fixture = process.platform === "darwin" && !path.isAbsolute(runtime)
    ? process.env.PATH.split(path.delimiter).map(folder => path.join(folder, runtime)).find(file => fs.existsSync(file))
    : process.platform === "darwin" ? runtime : process.execPath;
  assert.ok(fixture, "The Mac engine fixture needs a self-contained Bun executable");
  const engineBytes = fs.readFileSync(fixture);
  write(oldEngine, engineBytes, { mode: 0o700 });
  // The stand-in's durable session data stays in the test harness, like the
  // engine's database. Each verified engine process has its own listening address.
  write(path.join(workspace, "serve"), `const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const portArg = process.argv.indexOf("--port");
// The failed trial is a signed fixture that passes the release probe but
// exits rather than answering as a resident. Engine workers get --port.
if (portArg < 0) process.exit(1);
const port = Number(process.argv[portArg + 1]);
const server = http.createServer((request, response) => {
  const forward = http.request({ host: "127.0.0.1", port: ${engine.record.port}, path: request.url,
    method: request.method, headers: request.headers }, reply => {
    response.writeHead(reply.statusCode, reply.headers); reply.pipe(response);
  });
  forward.on("error", () => response.destroy());
  response.on("close", () => forward.destroy());
  request.pipe(forward);
});
server.listen(port, "127.0.0.1", () => console.log(server.address().port));
process.on("exit", () => fs.writeFileSync(path.join(__dirname, "exit-" + process.pid), ""));
process.on("SIGTERM", () => process.exit(0));
setInterval(() => { if (fs.existsSync(path.join(__dirname, "fixture-stop"))) process.exit(0); }, 50).unref();
`);
  if (rollback) write(path.join(home, "selftest"), `process.exit(process.argv.slice(2).join(" ") === "--engine ${newVersion}" ? 0 : 1);`);
  const worker = launch(oldEngine, ["serve", "--port", "0", "--hostname", "127.0.0.1"], here.env, workspace, "pipe");
  let port = "";
  worker.child.stdout.on("data", chunk => { port += chunk; });
  const program = process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box";
  const previous = "versions/0.8.0-beta.1-000000000000";
  const current = "0.8.0-beta.1";
  const targetVersion = "0.8.0";
  const release = path.join(here.env.HOME, "release");
  let keeper;
  let keeperEnv;
  const keep = (...args) => {
    const result = spawnSync(binary.file, ["keep", ...args], { env: keeperEnv, cwd: home, encoding: "utf8", timeout: 15000 });
    assert.equal(result.status, 0, result.error?.message ?? result.stderr);
  };
  try {
    await until(() => /^\d+\s*$/.test(port), () => "the earlier engine did not listen\n" + worker.log());
    const oldRecord = { ...engine.record, version: oldVersion, pid: worker.child.pid, port: Number(port) };
    write(path.join(home, "run/engine.json"), encode(oldRecord));
    const session = "ses_engineDrain";
    engine.sessions.push(sessionOf({ id: session, title: "Engine drain", directory: workspace }));
    engine.runAfterPrompt = "msg_originalEngine";
    for (const [id, text] of [["msg_originalEngine", "original turn"], ["msg_inboxEngine", "earlier inbox"]]) {
      const response = await fetch(`http://127.0.0.1:${oldRecord.port}/api/session/${session}/prompt`, {
        method: "POST", body: JSON.stringify({ id, text, delivery: "queue" }) });
      assert.equal(response.status, 200);
    }
    assert.ok(engine.running.has(session));
    const tree = path.join(release, "tree");
    const oldBinary = fs.readFileSync(binary.file);
    const members = { [`bin/${program}`]: rollback ? engineBytes : oldBinary, "bin/box-version": Buffer.from(targetVersion + "\n") };
    for (const [file, bytes] of Object.entries(members)) write(path.join(tree, file), bytes, { mode: 0o700 });
    write(path.join(tree, "box.json"), encode({ kuvo: "box/1", launch: "rust", version: targetVersion, engine: newVersion,
      downgradeSafe: true, engines: [oldVersion, newVersion],
      files: Object.fromEntries(Object.entries(members).map(([file, bytes]) => [file, digest(bytes)])) }));
    const tar = path.join(release, "box.tar");
    assert.equal(spawnSync("tar", ["cf", tar, "-C", tree, "box.json", "bin"], { timeout: 90000 }).status, 0);
    const assets = await engineFeed(t, path.join(release, "engine"), engineBytes, { damageFirst: false });
    const system = process.platform === "win32" ? "windows" : process.platform;
    const architecture = process.arch === "arm64" ? "aarch64" : "x86_64";
    const { hash, updateEnv } = await fixtureReleaseFeed(t, gzipSync(fs.readFileSync(tar), { level: 1 }), { engineVersion: newVersion, engines: { [system]: { [architecture]: assets.build } } });
    write(path.join(home, previous, "box.json"), encode({ kuvo: "box/1", version: current, launch: "rust", engine: oldVersion }));
    write(path.join(home, previous, "bin", program), oldBinary, { mode: 0o700 });
    write(path.join(home, previous, "bin/box-version"), current + "\n");
    keeperEnv = { ...here.env, KUVO_HOME: home, ...updateEnv };
    keep("select", `${previous}/bin`);
    keeper = launch(binary.file, ["keep"], keeperEnv, home);
    const serve = path.join(home, "run/serve.json");
    const before = await until(async () => {
      try { const record = readJSON(serve); return (await request(record, "GET", "kuvo/info")).status === 200 && record; } catch { return false; }
    }, () => "the earlier resident did not answer\n" + keeper.log(), 25000);
    await until(() => {
      const failed = path.join(home, "run/fleet-cannot.json");
      if (fs.existsSync(failed)) assert.fail(fs.readFileSync(failed, "utf8"));
      return fs.existsSync(path.join(home, "run/fleet-ready.json"));
    }, () => `the signed target should be prepared\n${fs.readFileSync(path.join(home, "run/serve.log"), "utf8").slice(-4000)}\n${keeper.log()}`, 120000);
    const hold = path.join(home, "run/fleet-engine-hold.json");
    await until(() => fs.existsSync(hold), "the updater should hold new engine work while the old turn finishes", 15000);
    assert.equal(fs.readFileSync(path.join(home, "active-release"), "utf8"), `${previous}/bin`);
    assert.equal(readJSON(path.join(home, "run/engine.json")).pid, oldRecord.pid);
    assert.ok(engine.running.has(session));
    const app = { "x-kuvo-from": "app" };
    const changeHold = fields => {
      write(hold + ".test", encode({ ...readJSON(hold), ...fields }));
      fs.renameSync(hold + ".test", hold);
    };
    if (borrowed) {
      // An engine outside this home's engine directory is borrowed, even when
      // its authenticated health endpoints answer. Give several fleet rounds
      // to check it before its hold expires.
      const engineRecord = path.join(home, "run/engine.json");
      assert.deepEqual(readJSON(engineRecord), oldRecord);
      // Losing the record must not make an already borrowed process ours.
      fs.rmSync(engineRecord);
      changeHold({ since: Date.now() - releaseDrainMs + 12000 });
      engine.running.delete(session);
      await until(() => readJSON(hold).stage === "deferred", "a borrowed worker should defer the switch", releaseDrainRetryMs);
      assert.equal(fs.readFileSync(path.join(home, "active-release"), "utf8"), `${previous}/bin`);
      assert.equal(worker.child.exitCode, null);
      assert.equal(fs.existsSync(engineRecord), false);
      assert.equal(engine.asked.filter(row => row.method === "DELETE" && row.path.includes("/inbox/")).length, 0, "a borrowed inbox must never be moved");
      assert.deepEqual(engine.inboxes.get(session).map(item => item.id), ["msg_inboxEngine"]);
      return;
    }
    if (!rollback) {
      // Exercise the persisted deadline instead of sleeping five minutes.
      const deferred = await request(before, "POST", `api/session/${session}/prompt`, { id: "msg_deferredEngine", text: "wait then retry", delivery: "queue" }, app);
      assert.equal(deferred.status, 200, deferred.text);
      assert.deepEqual(deferred.json, { waits: true });
      changeHold({ since: Date.now() - releaseDrainMs - 1000 });
      await until(() => readJSON(hold).stage === "deferred", "an expired drain should release the queue and defer the update", releaseDrainRetryMs);
      assert.ok(readJSON(hold).retryAt > Date.now());
      assert.equal(readJSON(path.join(home, "run/engine.json")).pid, oldRecord.pid);
      assert.deepEqual(engine.inboxes.get(session).map(item => item.id), ["msg_inboxEngine", "msg_deferredEngine"]);
      changeHold({ retryAt: Date.now() - 1 });
      await until(() => fs.existsSync(hold) && readJSON(hold).stage === "holding", "the signed release should retry after the defer time", releaseDrainRetryMs);
    }
    const queued = await request(before, "POST", `kuvo/sessions/${session}/send`, { id: "msg_afterEngine", words: "after the switch" }, { "x-kuvo-from": "app" });
    assert.equal(queued.status, 200, queued.text);
    assert.deepEqual(queued.json, { waits: true });
    const inline = { name: "held-note.txt", uri: "data:text/plain;base64," + Buffer.from("held note bytes").toString("base64") };
    const raw = { id: "msg_rawEngine", text: "a raw prompt behind the held send", delivery: "queue", files: [inline] };
    for (let attempt = 0; attempt < 2; attempt++) {
      const response = await request(before, "POST", `api/session/${session}/prompt`, raw, app);
      assert.equal(response.status, 200, response.text);
      assert.deepEqual(response.json, { waits: true });
    }
    if (!rollback) {
      const promoted = await request(before, "POST", `kuvo/sessions/${session}/queue/${raw.id}/now`, {}, app);
      assert.equal(promoted.status, 200, promoted.text);
      assert.equal(promoted.json.waiting[0].id, raw.id);
      assert.deepEqual(engine.interrupted, []);
    }
    const status = await request(before, "GET", "kuvo/update", undefined, app);
    assert.equal(status.status, 200, status.text);
    assert.equal(status.json.updating, "installing");
    assert.deepEqual(status.json.engineChange, { stage: "holding", from: oldVersion, to: newVersion,
      line: "Kuvo’s engine is updating once the running sessions finish. New messages wait and go right after." });
    const blocked = await request(before, "POST", `api/session/${session}/command`, { command: "test" }, app);
    assert.equal(blocked.status, 503, blocked.text);
    assert.equal(blocked.json.engineChanging, true);
    const ask = { id: "per_drain", sessionID: session, action: "edit", resources: ["notes.txt"] };
    engine.permissions.set(session, [ask]);
    const answered = await request(before, "POST", `api/session/${session}/permission/${ask.id}/reply`, { decision: "once" }, app);
    assert.equal(answered.status, 200, answered.text);
    assert.deepEqual(engine.permissions.get(session), []);
    const made = await request(before, "POST", "kuvo/sessions", { title: "Made during drain", task: "Start when the engine is ready", chat: true, approval: "askFirst" }, app);
    assert.equal(made.status, 200, made.text);
    const madeSession = made.json.session;
    assert.ok(madeSession);
    assert.equal(engine.messages.get(madeSession)?.length ?? 0, 0, "a new conversation's first task must also wait");
    if (!rollback) {
      // A saved queue can outlive its engine session (for example after a
      // deletion that occurred while the resident was away). It must not
      // prevent the other conversations from resuming or updates from finishing.
      const gone = "ses_goneDuringDrain";
      engine.sessions.push(sessionOf({ id: gone, title: "Vanished", directory: workspace }));
      const saved = await request(before, "POST", `api/session/${gone}/prompt`, { id: "msg_goneEngine", text: "keep these words", delivery: "queue" }, app);
      assert.deepEqual(saved.json, { waits: true });
      engine.sessions = engine.sessions.filter(row => row.id !== gone);
    }
    const earlier = rollback ? ["msg_inboxEngine"] : ["msg_inboxEngine", "msg_deferredEngine"];
    assert.deepEqual(engine.inboxes.get(session).map(item => item.id), earlier);
    const earlierReads = engine.asked.length;
    engine.open.add(workspace);
    engine.shells = [{ id: "shell_drain", status: "running", folder: workspace }];
    engine.running.delete(session);
    engine.emit({ type: "session.execution.succeeded", data: { sessionID: session } });
    await until(() => engine.asked.slice(earlierReads).filter(row => row.path === "/api/shell").length >= 2, "the updater must check commands left running after a turn");
    assert.equal(fs.readFileSync(path.join(home, "active-release"), "utf8"), `${previous}/bin`);
    assert.equal(readJSON(path.join(home, "run/engine.json")).pid, oldRecord.pid);
    assert.ok(fs.existsSync(hold));
    engine.shells = [];
    const selected = `versions/${targetVersion}-${hash.slice(0, 12)}/bin`;
    if (rollback) {
      await until(() => fs.readFileSync(path.join(home, "active-release"), "utf8") === selected && fs.existsSync(path.join(home, "run/previous")), "the keeper should try the signed target", keeperTrialMs);
      assert.ok(fs.existsSync(hold), "the queue stays held during the failed health trial");
      assert.equal(fs.existsSync(path.join(home, "run/engine.json")), false, "no engine starts before the keeper verdict");
      assert.deepEqual(engine.messages.get(session).map(message => message.id), ["msg_originalEngine"]);
      await until(() => fs.readFileSync(path.join(home, "active-release"), "utf8") === `${previous}/bin` && fs.existsSync(path.join(home, "run/update-cannot")) && !fs.existsSync(path.join(home, "run/previous")),
        () => "the keeper did not restore the previous release\n" + keeper.log(), keeperTrialMs + 15000);
    } else {
      await until(() => fs.readFileSync(path.join(home, "active-release"), "utf8") === selected && !fs.existsSync(path.join(home, "run/previous")),
        () => "the keeper did not confirm the new release\n" + keeper.log(), keeperTrialMs);
    }
    await until(() => {
      try { const record = readJSON(path.join(home, "run/engine.json")); return record.version === (rollback ? oldVersion : newVersion) && record.pid !== oldRecord.pid; } catch { return false; }
    }, "the selected release's engine should start after the trial", 30000);
    await worker.exited;
    await until(() => !fs.existsSync(hold) && engine.messages.get(session)?.some(message => message.id === "msg_rawEngine"),
      () => `the waiting work should reach the selected engine and clear the fleet hold: ${fs.existsSync(hold) ? fs.readFileSync(hold, "utf8") : "no hold"}`, 30000);
    assert.deepEqual(engine.messages.get(session).filter(message => message.type === "user").map(message => message.id),
      rollback ? ["msg_originalEngine", ...earlier, "msg_afterEngine", raw.id]
        : ["msg_originalEngine", raw.id, ...earlier, "msg_afterEngine"]);
    const attachedRoot = path.join(home, "attachments", session);
    const attached = fs.readdirSync(attachedRoot).map(folder => path.join(attachedRoot, folder, inline.name));
    assert.equal(attached.length, 1, "retrying the held raw prompt doesn't copy its attachment twice");
    assert.equal(fs.readFileSync(attached[0], "utf8"), "held note bytes");
    const received = engine.messages.get(session).find(message => message.id === raw.id);
    assert.ok(received.text.includes(attached[0]));
    assert.deepEqual(received.files, [inline]);
    assert.deepEqual(engine.messages.get(madeSession).filter(message => message.type === "user").map(message => message.text), ["Start when the engine is ready"]);
    if (!rollback) {
      assert.equal(readJSON(path.join(home, "run/waiting/msg_goneEngine.json")).text, "keep these words");
      assert.deepEqual(readJSON(path.join(home, "run/queues.json")).ses_goneDuringDrain.waiting.map(row => row.id), ["msg_goneEngine"]);
    }
  } finally {
    let currentEngine;
    try { currentEngine = readJSON(path.join(home, "run/engine.json")); } catch {}
    if (keeper) {
      try { keep("off"); await until(() => keeper.child.exitCode !== null, () => keeper.log(), 15000); }
      finally { await keeper.stop(); }
    }
    write(path.join(workspace, "fixture-stop"), "");
    if (currentEngine && currentEngine.pid !== worker.child.pid) {
      await until(() => {
        if (fs.existsSync(path.join(workspace, `exit-${currentEngine.pid}`))) return true;
        try { process.kill(currentEngine.pid, 0); return false; }
        catch (error) { return error.code === "ESRCH"; }
      }, "the replacement fixture engine should exit");
    }
    await worker.stop();
  }
});

async function releaseHeldSession(t) {
  const binary = boxBinary();
  const { computer } = await bench(t);
  const here = await computer("release-queue-controls", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const session = "ses_releaseQueue";
  here.engine.sessions.push(sessionOf({ id: session, title: "Held work", directory: here.home }));
  here.engine.running.add(session);
  const ready = await here.start().ready();
  const run = path.join(here.home, "run");
  const now = Date.now();
  const change = { from: "0.0.0", to: "2.2.6", since: now, stage: "holding" };
  // Reopen the durable release state used by each implementation. No feed or
  // keeper changes are needed to exercise the normal queue and update routes.
  if (binary.typescript) {
    write(path.join(run, ".updating/pid"), String(process.pid));
    write(path.join(run, "update-engine-hold.json"), encode({ formatVersion: 1, ...change, holding: now, pid: process.pid }));
  } else {
    write(path.join(run, "fleet-engine-hold.json"), encode({ formatVersion: 1, ...change, release: "0.8.0:fixture",
      owner: { pid: process.pid, startIdentity: "fixture" }, retryAt: null }));
  }
  return { ...here, ready, session, route: `kuvo/sessions/${session}/queue`, app: { "x-kuvo-from": "app" } };
}

test("a release hold lets the waiting queue be edited without starting a turn", async t => {
  const { engine, ready, session, route, app } = await releaseHeldSession(t);
  const ids = ["msg_holdFirst", "msg_holdSecond", "msg_holdThird"];
  for (const id of ids) {
    const sent = await request(ready, "POST", `api/session/${session}/prompt`, { id, text: id, delivery: "queue" }, app);
    assert.equal(sent.status, 200, sent.text);
    assert.deepEqual(sent.json, { waits: true });
  }
  const pause = async () => {
    const held = await request(ready, "POST", route + "/hold", {}, app);
    assert.equal(held.status, 200, held.text);
    assert.equal(held.json.paused, true);
  };
  await pause();
  const now = await request(ready, "POST", `${route}/${ids[2]}/now`, {}, app);
  assert.equal(now.status, 200, now.text);
  assert.equal(now.json.paused, false);
  assert.deepEqual(now.json.waiting.map(row => row.id), [ids[2], ids[0], ids[1]]);
  await pause();
  const steer = await request(ready, "POST", `${route}/${ids[1]}/steer`, {}, app);
  assert.equal(steer.status, 200, steer.text);
  assert.equal(steer.json.paused, false);
  assert.deepEqual(steer.json.waiting.map(row => row.id), [ids[1], ids[2], ids[0]]);
  await pause();
  const resumed = await request(ready, "POST", route + "/resume", {}, app);
  assert.equal(resumed.status, 200, resumed.text);
  assert.equal(resumed.json.paused, false);
  assert.deepEqual(resumed.json.waiting.map(row => row.id), [ids[1], ids[2], ids[0]]);
  assert.equal((await request(ready, "POST", route + "/msg_missing/now", {}, app)).status, 404);
  assert.deepEqual(engine.messages.get(session) ?? [], []);
  assert.deepEqual(engine.inboxes.get(session) ?? [], []);
  assert.deepEqual(engine.interrupted, []);
});

test("the update status explains the engine release hold", async t => {
  const { ready, app } = await releaseHeldSession(t);
  const answer = await request(ready, "GET", "kuvo/update", undefined, app);
  assert.equal(answer.status, 200, answer.text);
  assert.deepEqual(answer.json.engineChange, { stage: "holding", from: "0.0.0", to: "2.2.6",
    line: "Kuvo’s engine is updating once the running sessions finish. New messages wait and go right after." });
});

test("a release hold saves a raw prompt's inline attachment", async t => {
  const { engine, home, ready, session, app } = await releaseHeldSession(t);
  const id = "msg_heldAttachment";
  const words = "the attached note";
  const inline = { name: "note.txt", uri: "data:text/plain;base64," + Buffer.from(words).toString("base64") };
  const answer = await request(ready, "POST", `api/session/${session}/prompt`, { id, text: "Read the note", delivery: "queue", files: [inline] }, app);
  assert.equal(answer.status, 200, answer.text);
  assert.deepEqual(answer.json, { waits: true });
  const root = path.join(home, "attachments", session);
  const saved = fs.readdirSync(root).map(folder => path.join(root, folder, "note.txt"));
  assert.equal(saved.length, 1);
  assert.equal(fs.readFileSync(saved[0], "utf8"), words);
  const payload = readJSON(path.join(home, "run/waiting", id + ".json"));
  assert.ok(payload.text.includes(saved[0]));
  assert.deepEqual(engine.messages.get(session) ?? [], []);
  assert.deepEqual(engine.inboxes.get(session) ?? [], []);
});

test("a signed release without rollback compatibility leaves the earlier engine in place", t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("The Rust preparation guard runs before the engine download.");
  const { home, release, key, body, update } = box(t);
  const before = { pid: process.pid, version: "2.2.5", password: "fixture", port: 12345 };
  write(path.join(home, "run/engine.json"), encode(before));
  const tree = path.join(release, "tree");
  const program = process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box";
  const members = { [`bin/${program}`]: Buffer.from("fixture never runs"), "bin/box-version": Buffer.from(body.version + "\n") };
  for (const [file, bytes] of Object.entries(members)) write(path.join(tree, file), bytes, { mode: 0o700 });
  write(path.join(tree, "box.json"), encode({ kuvo: "box/1", launch: "rust", version: body.version, engine: body.engine,
    downgradeSafe: true, engines: [body.engine], files: Object.fromEntries(Object.entries(members).map(([file, bytes]) => [file, digest(bytes)])) }));
  const tar = path.join(release, "box.tar");
  assert.equal(spawnSync("tar", ["cf", tar, "-C", tree, "box.json", "bin"], { timeout: 15000 }).status, 0);
  const pack = gzipSync(fs.readFileSync(tar));
  const descriptor = { ...body.pack, sha256: digest(pack) };
  const answer = update(envelope({ ...body, launch: "rust", pack: descriptor, packs: { [target]: descriptor } }, key), pack);
  assert.deepEqual(answer, { result: "failed", version: body.version,
    message: "The new engine hasn't been checked for going back to the engine running here." });
  assert.deepEqual(readJSON(path.join(home, "run/engine.json")), before);
  assert.equal(fs.readFileSync(path.join(home, "bin/box-version"), "utf8"), "0.8.0-beta.1\n");
  assert.equal(fs.existsSync(path.join(home, "run/fleet-ready.json")), false);
  assert.equal(fs.existsSync(path.join(home, "run/fleet-engine-hold.json")), false);
});

test("a failed engine download can retry preparation for the same signed release", { timeout: 180000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript) return t.skip("Uses the Rust fleet worker.");
  const { computer } = await bench(t);
  const here = await computer("Standalone", undefined, folder => {
    privateWindowsHome(path.join(folder, "box"));
    return {};
  });
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const engine = "2.2.6";
  const installed = "0.8.0-beta.1";
  const version = "0.8.0";
  const executable = process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box";
  const files = {
    [`bin/${executable}`]: fs.readFileSync(binary.file),
    "bin/box-version": Buffer.from(version + "\n"),
  };
  const tree = path.join(root, "tree");
  for (const [file, data] of Object.entries(files)) {
    write(path.join(tree, file), data, { mode: 0o700 });
  }
  write(path.join(tree, "box.json"), encode({
    kuvo: "box/1", launch: "rust", version, engine,
    files: Object.fromEntries(Object.entries(files).map(([file, data]) => [file, digest(data)])),
  }));
  const tar = path.join(root, "box.tar");
  const packed = spawnSync("tar", ["cf", tar, "-C", tree, "box.json", "bin"], { encoding: "utf8", timeout: 90000 });
  assert.equal(packed.status, 0, packed.stderr);
  const bytes = gzipSync(fs.readFileSync(tar), { level: 1 });
  const standIn = process.platform === "win32" ? undefined : '#!/bin/sh\n[ "$1" = --version ]\n';
  const feed = await engineFeed(t, path.join(root, "engine"), standIn, { damageFirst: false });
  const unavailable = feed.archive + ".unavailable";
  fs.renameSync(feed.archive, unavailable);
  const system = process.platform === "win32" ? "windows" : process.platform;
  const architecture = process.arch === "arm64" ? "aarch64" : "x86_64";
  const { hash, updateEnv } = await fixtureReleaseFeed(t, bytes, { engines: { [system]: { [architecture]: feed.build } } });
  write(path.join(here.home, "bin/box-version"), installed + "\n");
  write(path.join(here.home, "run/engine.json"), encode({ ...here.engine.record, version: engine }));
  const child = here.start(binary, updateEnv);
  const resident = await child.ready();
  const failed = path.join(here.home, "run/fleet-cannot.json");
  const failure = await until(() => fs.existsSync(failed) && readJSON(failed),
    () => `the engine download failure must be recorded\n${child.log()}`, 90000);
  assert.deepEqual([failure.version, failure.sha256, failure.fixable], [version, hash, true]);
  assert.match(failure.reason, /couldn't download the engine/);
  assert.equal((await request(resident, "GET", "kuvo/update")).json.version, installed);
  fs.renameSync(unavailable, feed.archive);
  const retried = await request(resident, "POST", "kuvo/update/retry", { target: version, targetSha256: hash });
  assert.equal(retried.status, 202, retried.text);
  const release = path.join(here.home, "versions", `${version}-${hash.slice(0, 12)}`, "pack");
  await until(() => fs.existsSync(release) && !fs.existsSync(failed), "the same release must prepare after retry", 90000);
  assert.equal(digest(fs.readFileSync(release)), hash);
  assert.equal(fs.readFileSync(feed.calls, "utf8"), "download\ndownload\n");
});

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
  const unused = path.join(home, "versions/0.7.1-111111111111");
  const used = path.join(home, "versions/0.7.2-222222222222");
  const unrelated = path.join(home, "versions/notes");
  const priorRetirement = path.join(home, "versions/.retired-interrupted");
  for (const folder of [unused, used, unrelated, priorRetirement]) write(path.join(folder, "marker"), "kept data");
  const olderProgram = path.join(used, executable);
  write(olderProgram, members[executable], { mode: 0o700 });
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
    const holderDir = path.join(home, "run/agents/retirement-fixture");
    const holdOpen = path.join(home, "run/retirement-fixture-open");
    write(holdOpen, "");
    const id = "0123456789ab";
    const holder = launch(olderProgram, ["hold", holderDir, id, "raw"], env, home, "pipe");
    const finishHolder = () => fs.rmSync(holdOpen, { force: true });
    t.after(async () => {
      finishHolder();
      await holder.stop();
    });
    holder.child.stdin.end(encode({ key: "fixture-only-key", spec: {
      command: [process.execPath, "-e", 'setInterval(() => { if (!require("node:fs").existsSync(process.argv[1])) process.exit(0); }, 25);', holdOpen],
      cwd: home, bridge: process.pid, orphan: 120000, linger: 100,
    } }));
    await until(() => fs.existsSync(path.join(holderDir, id + ".json")), () => holder.log());
    assert.equal(fs.existsSync(unused), true, "failed preparation leaves existing releases alone");
    const start = performance.now();
    for (let retry = 0; retry < 2; retry++) {
      assert.deepEqual(update(signed), { result: "waiting", version });
      assert.equal(fs.existsSync(unused), false, "a superseded release has been retired");
      assert.equal(fs.existsSync(priorRetirement), false, "an interrupted retirement is swept");
      assert.equal(fs.existsSync(path.join(unrelated, "marker")), true);
      assert.equal(fs.existsSync(path.join(home, initial, executable)), true);
      assert.equal(fs.existsSync(path.join(used, "marker")), retry === 0, "a running holder protects its own release");
      if (retry === 0) {
        assert.equal(holder.child.exitCode, null);
        finishHolder();
        await until(() => holder.child.exitCode !== null || holder.child.signalCode !== null, () => holder.log());
        assert.equal((await holder.exited).code, 0, holder.log());
      }
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
test("a new main finishes a prepared release after the old main goes away", { timeout: failoverTestMs }, t => fleetRun(t, { failover: true }));
async function referenceMailCodec(t) {
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 }));
  const isolated = { ...isolatedEnv(root), KUVO_HOME: path.join(root, "box") };
  const before = Object.fromEntries(Object.keys(isolated).map(key => [key, process.env[key]]));
  Object.assign(process.env, isolated);
  const wireFixture = fileURLToPath(new URL("../../box/dist/kuvo-link.js", import.meta.url));
  let wireAPI;
  try { wireAPI = await import(pathToFileURL(wireFixture).href); }
  finally {
    for (const [key, value] of Object.entries(before)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
  return wireAPI;
}

async function releaseMailPeer(t, relay, observer, seed) {
  const { sealOneWay, openOneWay, readFrame, frame, Frame } = await referenceMailCodec(t);
  const wire = relay.device(Buffer.alloc(32, 50 + seed));
  t.after(() => wire.leave());
  const address = Buffer.from(observer.address, "base64url");
  const key = Buffer.from(observer.key, "base64url"), secret = Buffer.alloc(32, seed);
  async function receiveField(field, match) {
    const deadline = Date.now() + 12000;
    while (Date.now() < deadline) {
      const packet = readFrame((await wire.next(Math.max(1, deadline - Date.now()))).subarray(17));
      if (packet?.kind !== Frame.mail) continue;
      const value = field(JSON.parse(openOneWay(packet.body, secret, key).payload));
      if (value && match(value)) return value;
    }
    assert.fail("the expected signed peer mail did not arrive");
  }
  return {
    ready: () => until(() => relay.has(address), "the resident must connect"),
    offline: () => until(() => !relay.has(address), "the earlier resident must disconnect"),

    send: release => wire.send(address, frame(Frame.mail, 0, sealOneWay(encode({ at: Date.now(), release }), secret, key).message)),
    announce: computer => wire.send(address, frame(Frame.mail, 0, sealOneWay(encode({ at: Date.now(), agents: { at: Date.now(), list: [], computer } }), secret, key).message)),
    receive: match => receiveField(payload => payload.release, match),
    receiveComputer: match => receiveField(payload => payload.agents?.computer, match),
  };
}

test("release mail sends the signed pack for a computer with a different target", { timeout: 45000 }, async t => {
  const binary = boxBinary();
  const wireFixture = fileURLToPath(new URL("../../box/dist/kuvo-link.js", import.meta.url));
  if (binary.typescript || !fs.existsSync(wireFixture)) return t.skip("Uses the Rust target-pack mail with a reference wire peer.");
  const { computer, relay } = await bench(t);
  const here = await computer("Mixed target holder");
  const { main, member } = seedPeerSetup(here.home, relay.url);
  const version = "0.8.0";
  const localBytes = Buffer.from("this computer's signed pack");
  const foreignBytes = Buffer.alloc(70013, 42);
  const foreignTarget = target === "x86_64-unknown-linux-musl" ? "aarch64-apple-darwin" : "x86_64-unknown-linux-musl";
  const key = keypair();
  let signed, downloads = 0, online = true, damaged = true;
  const feed = createServer((req, res) => {
    if (req.url === "/boxes.json") return res.end(signed);
    if (req.url === "/foreign.tar.gz") {
      downloads++;
      return online ? res.end(damaged ? Buffer.from("damaged") : foreignBytes) : res.writeHead(404).end();
    }
    res.writeHead(404).end();
  });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const url = `http://127.0.0.1:${feed.address().port}`;
  const local = { url: `${url}/local.tar.gz`, sha256: digest(localBytes) };
  const foreign = { url: `${url}/foreign.tar.gz`, sha256: digest(foreignBytes) };
  signed = encode(envelope({ kuvo: "box/1", version, engine: "2.2.6", spreads: true, launch: "rust",
    pack: local, packs: { [target]: local, [foreignTarget]: foreign } }, key));
  const selected = `versions/${version}-${local.sha256.slice(0,12)}/bin`;
  const folder = path.join(here.home, path.dirname(selected));
  write(path.join(here.home, "active-release"), selected + "\n");
  write(path.join(folder, "boxes.json"), signed);
  write(path.join(folder, "pack"), localBytes);
  write(path.join(folder, "bin/box-version"), version + "\n");
  const updateEnv = { KUVO_TEST_RELEASE_FEED: `${url}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key };
  let child = here.start(binary, updateEnv);
  const peer = await releaseMailPeer(t, relay, main, 2);
  const resident = await child.ready();
  await peer.ready();
  peer.announce({ kuvo: version, releaseTarget: foreignTarget, installedSha256: foreign.sha256 });
  const heard = path.join(here.home, "run/peer-agents.json");
  await until(() => fs.existsSync(heard) && readJSON(heard)[member.key]?.computer?.releaseTarget === foreignTarget,
    "the holder should hear its member before the request");
  const { send, receive } = peer;
  const header = async sha256 => {
    send({ want: version, sha256 });
    try { return await receive(value => value.version === version && (value.none || value.boxes)); }
    catch (error) { throw new Error(`${error.message} for ${sha256 === local.sha256 ? "local" : "foreign"}\n${child.log()}`, { cause: error }); }
  };
  await until(async () => (await request(resident, "GET", "kuvo/setup")).json.computers.length === 2,
    "the local resident must publish the two-computer setup");
  assert.equal((await header(local.sha256)).size, localBytes.length);
  assert.deepEqual(await header(digest("not in the signed manifest")), { version, none: true });
  assert.equal(downloads, 0);
  assert.deepEqual(await header(foreign.sha256), { version, none: true });
  assert.equal(downloads, 1);
  assert.equal(fs.existsSync(path.join(folder, `relay-pack-${foreign.sha256}`)), false);
  damaged = false;
  const offered = await header(foreign.sha256);
  assert.equal(offered.boxes, signed.toString());
  assert.equal(offered.size, foreignBytes.length);
  assert.equal(downloads, 2);
  const chunks = [];
  for (let part = 0; part * 30000 < foreignBytes.length; part++) {
    send({ version, sha256: foreign.sha256, part });
    const answer = await receive(value => value.version === version && value.i === part && value.d);
    assert.equal(answer.sha256, foreign.sha256);
    chunks.push(Buffer.from(answer.d, "base64"));
  }
  assert.deepEqual(Buffer.concat(chunks), foreignBytes);
  online = false;
  assert.equal((await header(local.sha256)).size, localBytes.length);
  assert.equal((await header(foreign.sha256)).size, foreignBytes.length);
  // A hash-less legacy request cannot receive an earlier foreign response.
  send({ version, part: 0 });
  assert.deepEqual(Buffer.from((await receive(value => value.version === version && value.i === 0 && value.d)).d, "base64"), localBytes);
  const earlier = readJSON(heard)[member.key].at;
  await child.stop();
  await peer.offline();
  child = here.start(binary, updateEnv);
  const restarted = await child.ready();
  await peer.ready();
  peer.announce({ kuvo: version, releaseTarget: foreignTarget, installedSha256: foreign.sha256 });
  await until(() => readJSON(heard)[member.key]?.at > earlier, "the restarted holder must hear its member");
  assert.ok(fs.existsSync(path.join(folder, `relay-pack-${foreign.sha256}`)));
  assert.equal((await request(restarted, "GET", "kuvo/setup")).json.computers.length, 2);
  assert.equal((await request(restarted, "GET", "kuvo/update")).json.version, version);
  assert.equal((await header(local.sha256)).size, localBytes.length);
  assert.equal((await header(foreign.sha256)).size, foreignBytes.length);
  assert.equal(downloads, 2, "the donor retains the verified bytes across restart while the pack URL is unavailable");
  assert.equal(fs.readFileSync(path.join(here.home, "active-release"), "utf8").trim(), selected);
  assert.deepEqual(fs.readFileSync(path.join(folder, "pack")), localBytes);
});
for (const appOffer of [false, true]) test(appOffer
  ? "a main prepares its own signed pack from a member that has only offered it"
  : "a computer prepares its own signed pack from a holder on another target",
{ timeout: releaseReturnMs + firstPreparationMs }, async t => {
  const binary = boxBinary();
  const wireFixture = fileURLToPath(new URL("../../box/dist/kuvo-link.js", import.meta.url));
  if (binary.typescript || !fs.existsSync(wireFixture)) return t.skip("Uses Rust target-pack mail with a reference wire peer.");
  const { computer, relay } = await bench(t);
  const here = await computer("Mixed target recipient");
  const { main, member } = seedPeerSetup(here.home, relay.url, appOffer ? 1 : 2);
  const version = "0.8.0", previous = "0.8.0-beta.1";
  const program = process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box";
  const release = path.join(here.env.HOME, "release");
  const tree = path.join(release, "tree");
  const files = { [`bin/${program}`]: fs.readFileSync(binary.file), "bin/box-version": Buffer.from(version + "\n") };
  if (process.platform !== "win32") {
    for (const name of ["kuvo-start", "kuvo-setsid"]) files[`bin/${name}`] = fs.readFileSync(new URL(`../../box/src/keeper/${name}`, import.meta.url));
  }
  for (const [file, bytes] of Object.entries(files)) write(path.join(tree, file), bytes, { mode: 0o700 });
  fs.mkdirSync(path.join(tree, "bin/opencode-plugins"));
  write(path.join(tree, "box.json"), encode({ kuvo: "box/1", version, launch: "rust", engine: "2.2.6",
    files: Object.fromEntries(Object.entries(files).map(([file, bytes]) => [file, digest(bytes)])) }));
  const tar = path.join(release, "signed.tar");
  assert.equal(spawnSync("tar", ["cf", tar, "-C", tree, "box.json", "bin"], { timeout: 90000 }).status, 0);
  const pack = gzipSync(fs.readFileSync(tar), { level: 1 });
  const engine = await engineFeed(t, path.join(release, "engine"), undefined, { damageFirst: false });
  const system = process.platform === "win32" ? "windows" : process.platform;
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  const feed = createServer((_req, res) => res.writeHead(404).end());
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  t.after(() => { feed.close(); feed.closeAllConnections(); });
  const url = `http://127.0.0.1:${feed.address().port}`;
  const local = { url: `${url}/local.tar.gz`, sha256: digest(pack) };
  const foreignTarget = target === "x86_64-unknown-linux-musl" ? "aarch64-apple-darwin" : "x86_64-unknown-linux-musl";
  const foreign = { url: `${url}/foreign.tar.gz`, sha256: digest("other architecture bytes") };
  const key = keypair();
  const signed = encode(envelope({ kuvo: "box/1", version, engine: "2.2.6", spreads: true, launch: "rust",
    pack: foreign, packs: { [target]: local, [foreignTarget]: foreign }, [system]: { [arch]: engine.build } }, key)).toString();
  write(path.join(here.home, "bin/box-version"), previous + "\n");
  write(path.join(here.home, "run/engine.json"), encode({ ...here.engine.record, version: "2.2.6" }));
  const child = here.start(binary, { KUVO_TEST_RELEASE_FEED: `${url}/boxes.json`, KUVO_TEST_RELEASE_KEY: key.key });
  const donor = await releaseMailPeer(t, relay, appOffer ? main : member, appOffer ? 2 : 1);
  await child.ready();
  await donor.ready();
  donor.announce({ kuvo: previous, releaseTarget: foreignTarget, candidate: version, releaseManifest: signed,
    ...(!appOffer && { staged: { version, sha256: foreign.sha256 } }) });
  const asked = await donor.receive(value => value.want === version);
  assert.deepEqual(asked, { want: version, sha256: local.sha256 });
  donor.send({ version, sha256: local.sha256, boxes: signed, size: pack.length });
  const served = new Set();
  const parts = Math.ceil(pack.length / 30000);
  while (served.size < parts) {
    const { part, sha256 } = await donor.receive(value => value.version === version && Number.isInteger(value.part));
    assert.equal(sha256, local.sha256);
    const start = part * 30000;
    assert.ok(start < pack.length);
    donor.send({ version, sha256, i: part, d: pack.subarray(start, start + 30000).toString("base64") });
    served.add(part);
  }
  const marker = path.join(here.home, "run/fleet-ready.json");
  await until(() => {
    const failed = path.join(here.home, "run/fleet-cannot.json");
    if (fs.existsSync(failed)) assert.fail(fs.readFileSync(failed, "utf8"));
    return fs.existsSync(marker);
  }, () => `the requested target was not prepared\n${child.log()}`, firstPreparationMs);
  assert.deepEqual(readJSON(marker), { version, sha256: local.sha256 });
  const retained = path.join(here.home, `versions/${version}-${local.sha256.slice(0, 12)}`);
  assert.equal(digest(fs.readFileSync(path.join(retained, "pack"))), local.sha256);
  assert.equal(fs.readFileSync(path.join(retained, "boxes.json"), "utf8"), signed);
  assert.equal(fs.readFileSync(path.join(here.home, "bin/box-version"), "utf8").trim(), previous);
  assert.equal(fs.existsSync(path.join(here.home, `versions/${version}-${foreign.sha256.slice(0,12)}`)), false);
});

test("the main takes a signed release from a member already running it", { timeout: failoverTestMs }, t => fleetRun(t, { aheadMember: true }));
test("a release pulled over relay reports its checked progress on both computers", { timeout: failoverTestMs }, t => fleetRun(t, { aheadMember: true, pullProgress: true }));
test("peer transfer keeps its checked progress after the first holder times out", { timeout: failoverTestMs + peerTransferMs },
  t => fleetRun(t, { aheadMember: true, pullProgress: true, secondSource: true }));
test("a slow member can catch up after the main's preparation deadline", { timeout: 180000 }, t => fleetRun(t, { shellKeepers: true, stageDeadline: true }));
test("a TypeScript beta computer that missed the switch catches up to Rust", { timeout: 240000 }, t => fleetRun(t, { returningTypescript: true }));
test("the deployed beta refuses a direct Rust pack", { timeout: 240000 }, t => fleetRun(t, { returningTypescript: true, expectRefusal: true }));
test("the deployed beta takes the bridge before joining a Rust fleet", { timeout: bridgeTestMs }, t => fleetRun(t, { returningTypescript: true, bridgeBeta: true }));

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

async function fleetRun(t, { failover, returningTypescript, shellKeepers, bridgeBeta, expectRefusal, aheadMember, stageDeadline, pullProgress, secondSource }) {
  const binary = boxBinary();
  const windows = process.platform === "win32";
  if (binary.typescript || (windows && !failover && !aheadMember)) {
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
  let peerOnly = Boolean(aheadMember);
  let refusedPacks = 0;
  let refusedBridges = 0;
  let downloads = 0;
  let holdChecks = false;
  let heldChecks = 0;
  let signedBridgeRelease;
  const descriptor = () => ({ url: `http://127.0.0.1:${feed.address().port}/fleet.tar.gz`, sha256: hash });
  const signedTarget = () => {
    const pack = descriptor();
    return envelope({ ...body, pack, launch: "rust", packs: { [target]: pack } }, key);
  };
  const feed = createServer((req, res) => {
    if (req.url === "/boxes.json") {
      if (aheadMember) return res.writeHead(404).end();
      if (holdChecks) {
        heldChecks += 1;
        return;
      }
      res.end(encode(signedBridgeRelease ?? signedTarget()));
    } else if (req.url === "/fleet.tar.gz") {
      if (peerOnly) {
        if (secondSource && hold) return waiting.push(res);
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
  const holders = aheadMember ? (secondSource ? [member, away] : [member]) : [];
  const initial = peer => holders.includes(peer) ? selected : `versions/${versionBefore(peer)}-000000000000/bin`;
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
    if (holders.includes(peer)) {
      const folder = path.join(peer.home, bin, "..");
      fs.cpSync(packRoot, folder, { recursive: true });
      write(path.join(folder, "boxes.json"), encode(signedTarget()));
      write(path.join(folder, "pack"), pack);
    } else {
      write(path.join(peer.home, bin, program), executable, { mode: 0o700 });
      write(path.join(peer.home, bin, "..", "box.json"), encode({
        kuvo: "box/1", version: versionBefore(peer), launch: "rust", engine: body.engine,
      }));
    }
    write(path.join(peer.home, "run/engine.json"), encode({ ...peer.engine.record, version: body.engine }));
    if (windows) write(engine, fs.readFileSync(process.execPath));
    else write(engine, '#!/bin/sh\n[ "$1" = --version ]\n', { mode: 0o700 });
    if (!shell(peer)) control(peer, "select", bin);
  }
  const now = Date.now();
  const launchers = shellKeepers && process.platform === "darwin" ? [
    ["older-mac", Buffer.alloc(0)],
    ["launchers.json", encode({ "an older Mac": now - 26 * 86_400_000, "mac-old": now })],
  ] : [];
  for (const [file, data] of launchers) write(path.join(main.home, "run", file), data);
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
  const checkRows = failover || (shellKeepers && !stageDeadline);
  const computers = async peer => {
    try {
      const resident = readJSON(path.join(peer.home, "run/serve.json"));
      const answer = await request(resident, "GET", "kuvo/computers");
      const rows = Object.fromEntries(answer.json.computers.map(row => [row.name, row.fleetUpdate]));
      return { rows, summary: answer.json.fleetUpdate };
    } catch { return { rows: {} }; }
  };
  const startOnline = () => {
    start(main);
    start(member);
  };
  const checkPullProgress = async () => {
    const addressOf = peer => relayAddress(Buffer.from(readJSON(path.join(peer.home, "run/link.json")).secret, "base64url"));
    const partSize = 30_000;
    const packets = new Map(holders.map(peer => [peer, []]));
    const stopHolding = holders.map(peer => relay.intercept(addressOf(peer), addressOf(main), packet => {
      if (packet.payload.length <= partSize) return false;
      packets.get(peer).push(packet);
      return true;
    }));
    const replied = peer => packets.get(peer).length >= 8;
    let transfer;
    const expectFill = async count => {
      const fill = count / Math.ceil(pack.length / partSize);
      let last;
      const rows = await until(async () => {
        const local = (await computers(main)).rows.Studio;
        const peer = (await computers(member)).rows.Studio;
        last = [local, peer];
        return local?.fill === fill && peer?.fill === fill && [local, peer];
      }, () => `both computers must report ${count} accepted release parts, last rows ${JSON.stringify(last)}`);
      for (const row of rows) {
        assert.deepEqual([row.state, row.target, row.targetSha256], ["getting-ready", version, hash]);
        assert.deepEqual(row.line, { words: `Getting ready · ${Math.floor(fill * 100)}%`, tone: "working", phase: "getting-ready", fill });
      }
    };
    try {
      if (secondSource) start(away);
      startOnline();
      if (secondSource) {
        const seen = path.join(main.home, "run/peer-agents.json");
        await until(() => waiting.length && fs.existsSync(seen) &&
          Object.values(readJSON(seen)).filter(entry => entry.computer?.installedSha256 === hash).length === holders.length,
        "the main must hear that both members hold the signed pack before the feed fails", firstPreparationMs);
        hold = false;
        for (const response of waiting) {
          refusedPacks += 1;
          response.writeHead(404).end();
        }
      }
      transfer = await until(() => holders.find(replied), "the relay must hold signed release parts", firstPreparationMs);
      const held = packets.get(transfer);
      await expectFill(0);
      const received = held.splice(0, 4);
      for (const packet of received) relay.replay(packet);
      await expectFill(4);
      for (const packet of received) relay.replay(packet);
      relay.replay(held.shift());
      await expectFill(5);

      if (secondSource) {
        const alternate = holders.find(peer => peer !== transfer);
        const waitingSince = performance.now();
        await until(() => replied(alternate), "the second member must take over the incomplete transfer", peerTransferMs);
        t.diagnostic(`second holder answered after ${Math.round(performance.now() - waitingSince)} ms`);
        transfer = alternate;
        const second = packets.get(transfer);
        const recipient = readJSON(path.join(main.home, "run/serve.json"));
        // Two route requests guarantee the fleet worker completed a row update
        // after the alternate holder's manifest was accepted.
        for (let check = 0; check < 2; check += 1) {
          const reply = await request(recipient, "POST", "kuvo/update/retry", { target: version, targetSha256: digest("other pack") });
          assert.equal(reply.status, 409, reply.text);
        }
        await expectFill(5);
        for (const packet of second.splice(0, 6)) relay.replay(packet);
        await expectFill(6);
      }
    } finally {
      for (const stop of stopHolding) stop();
      if (transfer) for (const packet of packets.get(transfer)) relay.replay(packet);
    }
  };
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
    if (aheadMember) assert.equal(active(main), initial(main));
    if (pullProgress) await checkPullProgress();
    else startOnline();
    if (aheadMember) {
      await until(() => confirmed(main), diagnose, releaseReturnMs);
      t.diagnostic(`Retained pack transfer and keeper confirmation: ${Math.round(performance.now() - began)} ms`);
      assert.equal(active(member), selected);
      assert.equal(active(away), initial(away));
      assert.ok(refusedPacks > 0, "the main must get the pack from its member after the download fails");
      await until(async () => {
        try {
          const record = readJSON(path.join(main.home, "run/serve.json"));
          const answer = await request(record, "GET", "kuvo/update");
          return answer.status === 200 && answer.json.version === version;
        } catch { return false; }
      }, diagnose, 15000);
      return;
    }
    await until(() => waiting.length && (staged(main) || staged(member)), diagnose, firstPreparationMs);
    assert.equal(readJSON(path.join(main.home, "run/update.json")).latest, version);
    assert.equal(active(main), initial(main));
    assert.equal(active(member), initial(member));
    assert.equal(active(away), initial(away));
    t.diagnostic(`first preparation in ${Math.round(performance.now() - began)} ms`);
    if (checkRows) {
      const { pair: [preparing, ready], summary } = await until(async () => {
        const { rows, summary } = await computers(main);
        const pair = [rows.Studio, rows.Lab].sort((a, b) => (a?.state ?? "").localeCompare(b?.state ?? ""));
        return pair[0]?.state === "getting-ready" && pair[1]?.state === "ready"
          && rows.Away?.state === "off" && { pair, summary };
      }, "the computers list must show one ready computer and one still preparing");
      for (const row of [preparing, ready]) {
        assert.deepEqual([row.current, row.target, row.targetSha256, row.canRetry],
          ["0.8.0-beta.1", version, hash, false]);
      }
      assert.deepEqual(preparing.line, { words: "Getting ready…", tone: "working", phase: "getting-ready" });
      assert.deepEqual(ready.line, { words: `Ready for ${version}`, tone: "done", phase: "ready" });
      assert.deepEqual([summary.shown, summary.phase, summary.done, summary.updating, summary.behind,
        summary.title, summary.detail],
        [true, "getting-ready", 1, 1, 1, `Getting ${version} ready · 1 of 3`, "Your computers stay on beta.1 meanwhile."]);
      const prepared = staged(main) ? main : member;
      const watching = prepared === main ? member : main;
      const seen = path.join(watching.home, "run/peer-agents.json");
      const facts = await until(() => {
        if (!fs.existsSync(seen)) return;
        return Object.values(readJSON(seen)).find(entry => entry.computer?.staged?.sha256 === hash)?.computer;
      }, "the other computer must receive the prepared release announcement");
      assert.deepEqual(facts.staged, { version, sha256: hash });
      assert.equal(facts.kuvo, versionBefore(prepared));
      assert.equal(facts.runningManifest, undefined);
      assert.deepEqual(JSON.parse(facts.releaseManifest), signedTarget());
      const retained = path.join(prepared.home, selected, "..");
      assert.deepEqual(readJSON(path.join(retained, "boxes.json")), signedTarget());
      assert.equal(digest(fs.readFileSync(path.join(retained, "pack"))), hash);
    }

    await reply("turn before release switch");
    if (shellKeepers) await runningTurn();
    if (stageDeadline) {
      // Age only the keeper's saved preparation clock. The relay and the
      // live member keep running on their normal clocks during this check.
      const clock = path.join(main.home, "run/fleet-stage-clock.json");
      await until(() => fs.existsSync(clock) && staged(main), diagnose, 15000);
      const first = readJSON(clock);
      const draft = clock + ".fixture";
      write(draft, encode({ ...first, at: first.at - 300_000 }));
      fs.renameSync(draft, clock);
      await until(() => confirmed(main), diagnose, 75000);
      assert.equal(active(member), initial(member));
      assert.equal(staged(member), false);
      t.diagnostic("main confirmed while the member was still waiting for its download");
    }
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
        "the remaining online computer must claim the lease", leaseElectionMs);
      leader = member;
      t.diagnostic(`a new main claimed the lease ${Math.round(performance.now() - began)} ms after the first boot`);
    }

    hold = false;
    const released = performance.now();
    for (const res of waiting) res.end(pack);
    await until(() => confirmed(leader), diagnose, failover ? keeperTrialMs : 75000);
    if (failover) assert.ok(heldChecks > 0, "the new main checks the feed without waiting to activate");
    if (!stageDeadline) t.diagnostic(`main confirmed ${Math.round(performance.now() - released)} ms after member download resumed`);
    await until(() => confirmed(member), diagnose, 75000);
    await reply("same turn after release switch");
    if (shellKeepers) await runningTurn();
    if (launchers.length) {
      const resident = readJSON(path.join(main.home, "run/serve.json"));
      const reply = await request(resident, "GET", "kuvo/update");
      assert.equal(reply.status, 200, reply.text);
      assert.deepEqual([reply.json.version, reply.json.blocked], [version, null]);
      for (const [file, data] of launchers) {
        assert.deepEqual(fs.readFileSync(path.join(main.home, "run", file)), data);
      }
    }
    assert.equal(active(away), initial(away));
    if (checkRows) {
      const { Lab: current, Away: offline } = await until(async () => {
        const { rows } = await computers(leader);
        return rows.Lab?.state === "up-to-date" && rows.Away?.state === "off" && rows;
      }, "after confirmation the computers list must show the updated and offline computers");
      assert.deepEqual([current.current, current.currentSha256, current.target, current.targetSha256],
        [version, hash, version, hash]);
      assert.deepEqual(current.line, { words: `${version} · Up to date`, tone: "done", phase: "done" });
      assert.deepEqual(offline.line, { words: "Can’t be reached: updates when it’s back", tone: "off", phase: "off" });
      if (!failover) {
        const waiting = await until(async () => {
          const { summary } = await computers(leader);
          return summary?.phase === "waiting" && summary;
        }, "the summary must count the updated and offline computers");
        assert.deepEqual([waiting.shown, waiting.title, waiting.detail],
          [true, `${version} · 2 of 3 up to date`, "Away updates when it’s back."]);
      }
    }
    t.diagnostic(`online member confirmed ${Math.round(performance.now() - released)} ms after its download resumed`);

    const returned = performance.now();
    retryDownload = Boolean(shellKeepers);
    peerOnly = Boolean(failover || returningTypescript);
    const bridgeBin = bridgeBeta ? `versions/${bridgeVersion}-${bridgeHash.slice(0, 12)}/bin` : undefined;
    const placement = () => runsOn(readJSON(path.join(leader.home, "run/serve.json")));
    if (bridgeBin) {
      const source = path.join(leader.home, "run/peer-agents.json");
      const to = readJSON(path.join(away.home, "run/link.json"));
      const targetAddress = relayAddress(Buffer.from(to.secret, "base64url"));
      const resident = readJSON(path.join(leader.home, "run/serve.json"));
      const roster = await request(resident, "GET", "kuvo/computers");
      const awayKey = roster.json?.computers?.find(row => row.name === "Away")?.key;
      assert.ok(awayKey, "the Rust main should know the returning computer");
      const held = { bridge: [], rust: [] };
      const holding = { bridge: true, rust: true };
      const stopHolding = [main, member].map(peer => {
        const link = readJSON(path.join(peer.home, "run/link.json"));
        const from = relayAddress(Buffer.from(link.secret, "base64url"));
        return relay.intercept(from, targetAddress, packet => {
          if (packet.payload.length <= 30_000) return false;
          const phase = active(away) === bridgeBin ? "rust" : "bridge";
          if (!holding[phase]) return false;
          held[phase].push(packet);
          return true;
        });
      });
      const release = phase => {
        holding[phase] = false;
        for (const packet of held[phase].splice(0)) relay.replay(packet);
      };
      try {
        start(away);
        for (const [phase, prior] of [["bridge", previousVersion], ["rust", bridgeVersion]]) {
          await until(() => held[phase].length > 0 && readJSON(source)[awayKey]?.computer?.kuvo === prior,
            () => `the returning beta must ask for its ${phase} release at ${prior}\n${diagnose()}`, peerTransferMs);
          const { computers } = await placement();
          assert.ok(computers.some(row => row.here), "the Rust main should be available for work");
          assert.equal(computers.some(row => row.computer.name === "Away" && !row.away), false,
            `the returning beta must stay out of work placement at ${prior}`);
          release(phase);
          if (phase === "bridge") {
            await until(() => active(away) === bridgeBin && !fs.existsSync(path.join(away.home, "run/previous")), diagnose, releaseReturnMs);
            assert.ok(refusedBridges > 0, "the old beta must have reached a peer for its bridge pack");
            t.diagnostic(`returning ${previousVersion} confirmed the bridge in ${Math.round(performance.now() - returned)} ms`);
          }
        }
      } finally {
        for (const stop of stopHolding) stop();
        release("bridge");
        release("rust");
      }
    } else {
      start(away);
    }
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
    const rustBegan = performance.now();
    const returnMs = failover || bridgeBeta ? releaseReturnMs : peerOnly ? 120000 : 65000;
    await until(() => {
      const failure = (failover || bridgeBeta) && refused();
      if (failure?.version === version) throw new Error(failure.reason);
      return confirmed(away);
    }, diagnose, returnMs);
    await until(async () => {
      try {
        const resident = readJSON(path.join(away.home, "run/serve.json"));
        return (await request(resident, "GET", "kuvo/info")).status === 200;
      } catch {
        return false;
      }
    }, diagnose);
    if (checkRows && !failover) {
      const finished = await until(async () => {
        const { summary } = await computers(leader);
        return summary?.phase === "done" && summary;
      }, "the summary must settle after the last computer returns");
      assert.deepEqual([finished.shown, finished.done, finished.behind, finished.title],
        [true, 3, 0, `All 3 on ${version}`]);
    }
    assert.equal(retryDownload, false);
    if (peerOnly) assert.ok(refusedPacks > 0);
    if (returningTypescript) {
      const copy = path.join(away.home, "backups/before-rust");
      assert.equal(fs.readlinkSync(path.join(copy, "bin")), bridgeBin ?? initial(away));
      assert.equal(fs.readFileSync(path.join(copy, "bin/box-version"), "utf8"), (bridgeBin ? bridgeVersion : versionBefore(away)) + "\n");
      assert.equal(readJSON(path.join(away.home, selected, "..", "box.json")).launch, "rust");
    }
    if (bridgeBin) t.diagnostic(`the bridge then confirmed Rust in ${Math.round(performance.now() - rustBegan)} ms`);
    if (bridgeBin) {
      await until(async () => (await placement()).computers.some(row => row.computer.name === "Away" && !row.away),
        "the returning computer must be available for work after confirming Rust");
    }
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
  return { media, tree, version, pack, bytes, feed };
}

async function freshLinuxInstaller(t, binary, beforeStart = "") {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const key = keypair();
  const { media, version, pack, feed } = await linuxInstallMedia(t, binary, root, key,
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
  const begin = (verb, stdout) => launch("/bin/sh", [entry, verb], fixtureEnv, env.HOME, stdout);
  return { env, home, cron, cronBefore, version, pack, media, key, install, begin, stop, feed };
}

function engineDownloadInstallerStatus(install, home, { archive, calls }) {
  assert.equal(fs.existsSync(home), false);
  const saved = fs.readFileSync(archive);
  const checkFailure = message => {
    const result = install("status");
    assert.equal(result.status, 1, result.error?.message ?? result.stdout + result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), { result: "failed", message });
    assert.equal(fs.existsSync(home), false);
  };
  try {
    fs.appendFileSync(archive, "damaged");
    checkFailure("The engine that was downloaded isn't what this copy of Kuvo's release says. Try again.");
    fs.unlinkSync(archive);
    const here = process.platform === "darwin" ? "this Mac" : "this computer";
    checkFailure(`The engine couldn't be downloaded. Check that ${here} is online, then try again.`);
    const downloads = fs.readFileSync(calls);
    const engine = path.join(home, "engine/0.0.0", process.platform === "win32" ? "opencode.exe" : "opencode");
    if (process.platform === "win32") {
      fs.mkdirSync(path.dirname(engine), { recursive: true });
      fs.copyFileSync(process.execPath, engine);
    } else {
      write(engine, "#!/bin/sh\nexit 0\n", { mode: 0o700 });
    }
    const status = install("status");
    assert.equal(status.status, 0, status.error?.message ?? status.stdout + status.stderr);
    assert.equal(JSON.parse(status.stdout).verdict, "fresh");
    assert.deepEqual(fs.readFileSync(calls), downloads);
    assert.deepEqual(fs.readdirSync(home), ["engine"]);
  } finally {
    fs.writeFileSync(archive, saved);
    fs.rmSync(home, { recursive: true, force: true });
  }
}

test("Windows installer status checks the signed engine without making a home", { timeout: 120000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "win32") return t.skip("Uses native Windows status.");
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const key = keypair();
  const media = path.join(root, "media");
  const home = path.join(env.HOME, ".kuvo-code");
  const feed = await engineFeed(t, media);
  const bytes = Buffer.from("checks the signed archive digest before fetching the engine");
  const pack = { url: "https://example.invalid/status.tar.gz", sha256: digest(bytes) };
  write(path.join(media, "status.tar.gz"), bytes);
  write(path.join(media, "boxes.json"), encode(envelope({
    kuvo: "box/1", version: "0.8.0", engine: "2.2.6", launch: "rust",
    pack, packs: { [target]: pack }, windows: { x86_64: feed.build },
  }, key)));
  const status = () => spawnSync(binary.file, ["install", "--status", "--root", ".kuvo-code", "--media", media], {
    cwd: env.HOME, env: { ...env, KUVO_HOME: home, KUVO_TEST_RELEASE_KEY: key.key }, encoding: "utf8", timeout: 90000,
  });
  engineDownloadInstallerStatus(status, home, feed);
  const answer = status();
  assert.equal(answer.status, 0, answer.error?.message ?? answer.stdout + answer.stderr);
  assert.equal(JSON.parse(answer.stdout).verdict, "fresh");
  assert.equal(fs.existsSync(home), false);
});

function damagedInstallerStatus(install, media, home) {
  for (const name of ["move.tar.gz", "boxes.json"]) {
    const file = path.join(media, name);
    const original = fs.statSync(file).size;
    try {
      fs.appendFileSync(file, "damaged");
      const status = install("status");
      assert.equal(status.status, 1, status.error?.message ?? status.stdout + status.stderr);
      assert.deepEqual(JSON.parse(status.stdout), {
        result: "failed", message: "This copy of Kuvo is damaged. Download it again, then try again.",
      });
      assert.equal(fs.existsSync(home), false);
    } finally {
      fs.truncateSync(file, original);
    }
  }
}

function fileEngineInstallerStatus(install, media, home, key) {
  const file = path.join(media, "boxes.json");
  const original = fs.readFileSync(file);
  const signed = JSON.parse(original);
  const manifest = JSON.parse(Buffer.from(signed.manifest, "base64"));
  const system = process.platform === "darwin" ? "darwin" : "linux";
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  manifest[system][arch].url = pathToFileURL(path.join(media, "engine.tar.gz")).href;
  try {
    write(file, encode(envelope(manifest, key)));
    const refused = install("status");
    assert.equal(refused.status, 1, refused.error?.message ?? refused.stdout + refused.stderr);
    assert.deepEqual(JSON.parse(refused.stdout), {
      result: "failed", message: "This copy of Kuvo is damaged. Download it again, then try again.",
    });
    assert.equal(fs.existsSync(home), false);
  } finally {
    write(file, original);
  }
}

async function concurrentInstallStatus(start, home) {
  const children = Array.from({ length: 4 }, start);
  try {
    const replies = await Promise.all(children.map(async child => {
      let output = "";
      child.child.stdout.setEncoding("utf8");
      child.child.stdout.on("data", text => { output += text; });
      const exited = await child.exited;
      assert.equal(exited.code, 0, exited.error?.message ?? output + child.log());
      return JSON.parse(output);
    }));
    for (const reply of replies) {
      assert.deepEqual([reply.verdict, reply.verb], ["fresh", "new"]);
      assert.deepEqual(reply, replies[0]);
    }
    assert.equal(fs.existsSync(home), false);
  } finally {
    await Promise.all(children.map(child => child.stop()));
  }
}

test("Linux installer status distinguishes a machine launcher from an interrupted making", t => {
  if (process.platform !== "linux") return t.skip("Uses the Linux machine installer verdict.");
  const binary = boxBinary();
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const starter = path.join(home, "bin/kuvo-start");
  const making = path.join(home, "updates/install/making");
  const link = path.join(home, "run/link.json");
  const key = path.join(home, "box/keys/k");
  const starterBody = "#!/bin/sh\nexit 0\n";
  const linkBody = JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] });
  const command = ["install", "--status", "--root", ".kuvo-code"];
  const ask = (verdict, verb) => {
    const result = spawnSync(binary.typescript ? typescriptRuntime() : binary.file,
      binary.typescript ? ["--config=/dev/null", "--no-env-file", binary.file, ...command] : command,
      { cwd: env.HOME, env: { ...env, KUVO_HOME: home }, encoding: "utf8", timeout: 15000 });
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    const reply = JSON.parse(result.stdout);
    assert.deepEqual([reply.verdict, reply.verb], [verdict, verb]);
    return reply;
  };
  write(starter, starterBody, { mode: 0o700 });
  const machine = ask("machine", null);
  write(making, "1\n");
  const fresh = ask("fresh", "new");
  write(link, linkBody);
  write(key, "saved key");
  assert.deepEqual(ask("fresh", "new"), fresh);
  assert.deepEqual(machine.status, fresh.status);
  assert.deepEqual([starter, making, link, key].map(file => fs.readFileSync(file, "utf8")),
    [starterBody, "1\n", linkBody, "saved key"]);
  assert.equal(fs.existsSync(path.join(home, "active-release")), false);
  t.diagnostic(JSON.stringify({ machine, fresh }));
});

test("signed Linux installer makes a fresh machine and keeps unrelated reboot jobs", { timeout: 120000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "linux") return t.skip("Uses the native Linux installer.");
  const { env, home, cron, cronBefore, version, pack, media, key, install, begin, stop, feed } = await freshLinuxInstaller(t, binary);
  damagedInstallerStatus(install, media, home);
  fileEngineInstallerStatus(install, media, home, key);
  engineDownloadInstallerStatus(install, home, feed);
  await concurrentInstallStatus(() => begin("status", "pipe"), home);
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
    const resident = readJSON(path.join(home, "run/serve.json"));
    const info = await request(resident, "GET", "kuvo/info");
    assert.equal(info.status, 200, info.text);
    const identity = fs.readFileSync(path.join(home, "machine-id"), "utf8");
    assert.match(identity, /^[a-f0-9]{32}$/);
    assert.match(fs.readFileSync(path.join(home, "run/launch.env"), "utf8"), /^PATH=/m);
    assert.equal(fs.readFileSync(cron, "utf8"), cronBefore +
      '@reboot sh "$HOME/.kuvo-code/bin/kuvo-start" >/dev/null 2>&1 # kuvo-code: starts its engine again after a restart\n');
    t.diagnostic(`fresh Linux machine made and answered in ${Math.round(performance.now() - began)} ms`);
    const downloads = fs.readFileSync(feed.calls);
    const unavailable = feed.archive + ".unavailable";
    fs.renameSync(feed.archive, unavailable);
    try {
      const status = install("status");
      assert.equal(status.status, 0, status.stdout + status.stderr);
      const { verdict, verb } = JSON.parse(status.stdout);
      assert.deepEqual({ verdict, verb }, { verdict: "machine", verb: null });
      const refused = install("new");
      assert.equal(refused.status, 1, refused.stdout + refused.stderr);
      assert.deepEqual(JSON.parse(refused.stdout), {
        result: "refused", verdict: "machine",
        message: "This computer is set up as one of your machines already: Kuvo uses what's here.",
      });
      assert.equal(install("update").status, 2);
      assert.deepEqual(fs.readFileSync(feed.calls), downloads);
    } finally {
      fs.renameSync(unavailable, feed.archive);
    }
    assert.equal(fs.readFileSync(path.join(home, "machine-id"), "utf8"), identity);
    assert.equal(readJSON(path.join(home, "run/serve.json")).pid, resident.pid);
    assert.equal(fs.existsSync(path.join(home, "updates/install")), false);
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

test("a Linux installer resumes after it was interrupted between publication and startup", { timeout: 120000 }, t => resumeLinuxInstall(t));
test("a Linux installer resumes between publishing the engine and the release", { timeout: 120000 }, t => resumeLinuxInstall(t, "release"));
test("a Linux installer resumes while setting earlier files aside", { timeout: 120000 }, t => resumeLinuxInstall(t, "archive"));

async function resumeLinuxInstall(t, boundary = "startup") {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "linux") return t.skip("Uses the native Linux installer.");
  const { home, cron, cronBefore, install, begin, stop } = await freshLinuxInstaller(t, binary);
  const hold = path.join(path.dirname(cron), "hold-crontab");
  const oldWork = path.join("workspace", "old-work");
  const oldChat = path.join("chats", "old-chat");
  write(path.join(home, oldWork), "keep earlier work");
  write(path.join(home, oldChat), "keep earlier conversation");
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
    const engine = path.join(home, "engine");
    const engineInode = fs.statSync(engine).ino;
    if (boundary !== "startup") {
      // Recreate the earlier journal boundary before publishing the release.
      const staging = path.join(home, "updates/install/fresh-machine-media");
      fs.renameSync(path.join(home, "versions"), path.join(staging, "versions"));
      for (const file of ["bin", "folder-version.json", "machine-id", "run/launch.env"]) {
        fs.unlinkSync(path.join(home, file));
      }
      assert.equal(fs.existsSync(path.join(staging, "engine")), false);
      assert.equal(fs.existsSync(path.join(home, "versions")), false);
      if (boundary === "archive") {
        const marker = path.join(home, "updates/install/fresh-machine.json");
        const attempt = readJSON(marker);
        const aside = path.join(home, "trash", attempt.aside);
        fs.renameSync(engine, path.join(staging, "engine"));
        fs.renameSync(path.join(aside, "workspace"), path.join(home, "workspace"));
        write(marker, encode({ ...attempt, preserved: false }));
        assert.equal(fs.existsSync(path.join(home, "chats")), false);
        assert.equal(fs.readFileSync(path.join(aside, oldChat), "utf8"), "keep earlier conversation");
      }
    }
    const pending = install("status");
    assert.equal(pending.status, 0, pending.error?.message ?? pending.stdout + pending.stderr);
    const { verdict, verb } = JSON.parse(pending.stdout);
    assert.deepEqual({ verdict, verb }, { verdict: "fresh", verb: "new" });
    const resumed = install("new");
    assert.equal(resumed.status, 0, resumed.error?.message ?? resumed.stdout + resumed.stderr);
    const result = JSON.parse(resumed.stdout);
    assert.deepEqual([result.result, result.verdict, result.started], ["done", "machine", true]);
    const resumedId = fs.readFileSync(machineId, "utf8");
    if (boundary === "startup") assert.equal(resumedId, publishedId);
    else assert.match(resumedId.trim(), /^[0-9a-f]{32}$/);
    assert.equal(fs.statSync(engine).ino, engineInode);
    assert.equal(fs.readlinkSync(path.join(home, "bin")), selected);
    assert.equal(fs.readFileSync(path.join(result.setAside, oldWork), "utf8"), "keep earlier work");
    assert.equal(fs.readFileSync(path.join(result.setAside, oldChat), "utf8"), "keep earlier conversation");
    assert.equal(fs.readdirSync(path.join(home, "trash")).length, 1);
    assert.equal(fs.existsSync(path.join(home, "updates/install")), false);
  } finally {
    await interrupted.stop();
    if (cronPid !== undefined) process.kill(cronPid);
    stop();
  }
}

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

test("a Linux install stays complete when retired installer files cannot be removed", { timeout: 120000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "linux") return t.skip("Uses the native Linux installer.");
  const { home, install, stop } = await freshLinuxInstaller(t, binary);
  const updates = path.join(home, "updates");
  const abandoned = path.join(updates, "install/abandoned");
  write(path.join(updates, "install/making"), "1\n");
  write(path.join(abandoned, "incomplete-download"), "discard this");
  fs.chmodSync(abandoned, 0o500);
  try {
    const made = install("new");
    const resident = readJSON(path.join(home, "run/serve.json"));
    assert.equal((await request(resident, "GET", "kuvo/info")).status, 200);
    assert.equal(made.status, 0, made.error?.message ?? made.stdout + made.stderr);
    const result = JSON.parse(made.stdout);
    assert.deepEqual([result.result, result.verdict], ["done", "machine"]);
    assert.equal(fs.existsSync(path.join(updates, "install")), false);
    const retired = fs.readdirSync(updates).find(folder => folder.startsWith("retired-install-"));
    assert.ok(retired, "the old staging must be retired even when deletion fails");
    assert.equal(fs.readFileSync(path.join(updates, retired, "abandoned/incomplete-download"), "utf8"), "discard this");
    const status = install("status");
    assert.equal(status.status, 0, status.error?.message ?? status.stdout + status.stderr);
    assert.equal(JSON.parse(status.stdout).verdict, "machine");
  } finally {
    for (const folder of fs.readdirSync(updates)) {
      const remaining = path.join(updates, folder, "abandoned");
      if (fs.existsSync(remaining)) fs.chmodSync(remaining, 0o700);
    }
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

async function startReleasedWorkers({ account, home }, prior, environment) {
  const executable = path.join(home, "engine/2.0.32/opencode");
  fs.mkdirSync(path.dirname(executable), { recursive: true });
  fs.copyFileSync(path.join(prior, "opencode"), executable);
  fs.chmodSync(executable, 0o700);
  for (const file of ["kuvo-link.js", "kuvo-agents.js"]) fs.copyFileSync(path.join(prior, file), path.join(home, "bin", file));
  const env = { ...environment, HOME: account, TMPDIR: path.join(account, "tmp"), KUVO_HOME: home,
    XDG_DATA_HOME: path.join(home, "data"), XDG_CONFIG_HOME: path.join(home, "config"),
    XDG_STATE_HOME: path.join(home, "state"), XDG_CACHE_HOME: path.join(home, "cache"), OPENCODE_PASSWORD: "fixture-password" };
  const engine = launch(executable, ["serve", "--port", "0", "--hostname", "127.0.0.1"], env, path.join(home, "workspace"), "pipe");
  let output = "";
  engine.child.stdout.on("data", chunk => { output += chunk; });
  let oldLink, supervisor, bridgePid, bridgeStart;
  const startIdentity = pid => {
    try {
      const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
      return stat.slice(stat.lastIndexOf(")") + 2).split(" ")[19];
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  };
  const stop = async () => {
    await Promise.all([engine.stop(), oldLink?.stop(), supervisor?.stop()]);
    if (bridgeStart && startIdentity(bridgePid) === bridgeStart) {
      try { process.kill(bridgePid, "SIGTERM"); }
      catch (error) { if (error.code !== "ESRCH") throw error; }
    }
  };
  try {
    const listening = await until(() => /server listening[^\n]*127\.0\.0\.1:(\d+)/i.exec(output + engine.log()),
      () => "the released engine did not answer\n" + output + engine.log(), 30000);
    const port = Number(listening[1]);
    write(path.join(home, "run/engine.json"), JSON.stringify({ pid: engine.child.pid, port, password: env.OPENCODE_PASSWORD, version: "2.0.32", notes: {} }));
    oldLink = launch(executable, [path.join(home, "bin/kuvo-link.js"), "run"], { ...env, BUN_BE_BUN: "1" }, home);
    await until(() => fs.existsSync(path.join(home, "run/link.pid")) && Number(fs.readFileSync(path.join(home, "run/link.pid"), "utf8")) === oldLink.child.pid,
      () => "the released link did not start\n" + oldLink.log());
    const reserve = createServer();
    reserve.listen(0, "127.0.0.1");
    await once(reserve, "listening");
    const agentsPort = reserve.address().port;
    await new Promise(resolve => reserve.close(resolve));
    supervisor = launch("/bin/sh", ["-c", 'while kill -0 "$1" 2>/dev/null; do "$2" "$3" >>"$4" 2>&1; sleep 5; done',
      "kuvo-agents", String(engine.child.pid), executable, path.join(home, "bin/kuvo-agents.js"), path.join(home, "run/agents.log")],
      { ...env, BUN_BE_BUN: "1", KUVO_AGENTS_REMOTE: "1", KUVO_AGENTS_HOME: path.join(home, "agents"),
        KUVO_AGENTS_PORT: String(agentsPort), KUVO_AGENTS_PASSWORD: "fixture-agents", KUVO_ENGINE_PORT: String(port), KUVO_ENGINE_PID: String(engine.child.pid) }, home);
    write(path.join(home, "run/agents.pid"), String(supervisor.child.pid));
    bridgePid = await until(() => {
      try { return readJSON(path.join(home, "run/bridge.json")).pid; } catch { return undefined; }
    }, "the released agents bridge did not start", 30000);
    bridgeStart = startIdentity(bridgePid);
    return { env, engine, oldLink, supervisor, bridgePid, stop };
  } catch (error) {
    await stop();
    throw error;
  }
}

test("a signed installer takes over a live released 0.7.2 machine", { timeout: 240000 }, async t => {
  const binary = boxBinary();
  const cache = process.env.KUVO_CONFORMANCE_RELEASED_MACHINES;
  const prior = process.env.KUVO_CONFORMANCE_RELEASED_ENGINE;
  if (binary.typescript || process.platform !== "linux" || !cache || !prior) {
    return t.skip("Supply the released 0.7.2 engine and programs.");
  }
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const environment = isolatedEnv(root);
  const key = keypair();
  const { media, version, pack } = await linuxInstallMedia(t, binary, root, key);
  const { account, home, identity, link, before, legacyBin } = seedReleasedMachine(root, "v0.7.2", path.join(cache, "v0.7.2"));
  const workers = await startReleasedWorkers({ account, home }, prior, environment);
  const { env, engine, oldLink, supervisor } = workers;
  const install = () => spawnSync("/bin/sh", [path.join(media, "kuvo-install"), "released-machine", "--identity", identity], {
    cwd: account, env: { ...env, KUVO_TEST_RELEASE_KEY: key.key }, encoding: "utf8", timeout: 90000,
  });
  try {

    const launching = path.join(home, "run/.launching");
    fs.mkdirSync(launching);
    const starting = install();
    assert.equal(starting.status, 4, starting.stdout + starting.stderr);
    assert.ok(fs.statSync(launching).isDirectory());
    fs.rmdirSync(launching);
    const unrelated = launch("/bin/sleep", ["60"], env, home);
    try {
      write(path.join(home, "run/box.pid"), String(unrelated.child.pid));
      const refused = install();
      assert.equal(refused.status, 4, refused.stdout + refused.stderr);
      for (const pid of [unrelated.child.pid, engine.child.pid, oldLink.child.pid, supervisor.child.pid, workers.bridgePid]) {
        assert.ok(fs.readFileSync(`/proc/${pid}/cmdline`).length > 0, "a refused handoff must leave each live writer alone");
      }
    } finally {
      await unrelated.stop();
      fs.rmSync(path.join(home, "run/box.pid"), { force: true });
    }
    write(path.join(launching, "released-install.json"), JSON.stringify({ pid: unrelated.child.pid, startIdentity: "an earlier start" }));
    const upgraded = install();
    assert.equal(upgraded.status, 0, upgraded.stdout + upgraded.stderr);
    assert.deepEqual(JSON.parse(upgraded.stdout), { result: "done", started: true });
    await Promise.all([engine.exited, oldLink.exited, supervisor.exited]);
    assert.ok(!fs.existsSync(`/proc/${workers.bridgePid}/cmdline`) || !fs.readFileSync(`/proc/${workers.bridgePid}/cmdline`).length,
      "the earlier agents bridge must stop as well");
    const selected = fs.readlinkSync(path.join(home, "bin"));
    assert.equal(selected, `versions/${version}-${pack.sha256.slice(0, 12)}/bin`);
    const ready = readJSON(path.join(home, "run/serve.json"));
    assert.equal((await request(ready, "GET", "kuvo/info")).status, 200);
    assert.equal(readJSON(link).key, before.key);
    assert.equal(readJSON(link).secret, before.secret);
    assert.equal(fs.readFileSync(path.join(home, "workspace/notes"), "utf8"), "work kept on the released machine");
    assert.equal(fs.existsSync(launching), false);
  } finally {
    await workers.stop();
    if (fs.lstatSync(path.join(home, "bin")).isSymbolicLink()) {
      const stopped = spawnSync("/bin/sh", [legacyBin, "off"], { cwd: home, env, encoding: "utf8", timeout: 15000 });
      assert.equal(stopped.status, 0, stopped.stdout + stopped.stderr);
    }
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
  let workers;
  try {
    assert.equal((await request(await main.ready(), "GET", "kuvo/info")).status, 200);
    await until(() => fs.existsSync(reached), () => `the main should try the saved SSH address\n${main.log()}`, 15000);
    assert.equal(fs.existsSync(path.join(returned.home, "folder-version.json")), false);
    if (process.env.KUVO_CONFORMANCE_RELEASED_ENGINE) {
      workers = await startReleasedWorkers(returned, process.env.KUVO_CONFORMANCE_RELEASED_ENGINE, environment);
    }
    const started = performance.now();
    write(online, "");
    const ready = await until(() => {
      try { return readJSON(path.join(returned.home, "run/serve.json")); } catch { return undefined; }
    }, () => `the returning machine should answer from the Rust keeper\n${main.log()}`, 90000);
    assert.equal((await request(ready, "GET", "kuvo/info")).status, 200);
    if (workers) await Promise.all([workers.engine.exited, workers.oldLink.exited, workers.supervisor.exited]);
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
    await workers?.stop();
    if (fs.lstatSync(path.join(returned.home, "bin")).isSymbolicLink()) {
      const stopped = spawnSync("/bin/sh", [returned.legacyBin, "off"], {
        cwd: returned.home, env: { ...environment, HOME: returned.account, TMPDIR: path.join(returned.account, "tmp") },
        encoding: "utf8", timeout: 15000,
      });
      assert.equal(stopped.status, 0, stopped.stderr);
    }
  }
});

function macInstallerBundle(profile, identifier) {
  const bundle = path.join(profile, "Fixture.app");
  write(path.join(bundle, "Contents/Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${identifier}</string></dict></plist>`);
  return bundle;
}

test("the Mac installer recognizes another spelling of the same home", t => {
  if (boxBinary().typescript || process.platform !== "darwin") return t.skip("Uses the native Mac installer.");
  const { binary, env } = box(t);
  const bundle = macInstallerBundle(env.HOME, "run.kuvo.code.alias-fixture");
  const home = path.join(env.HOME, ".kuvo-code");
  const ask = root => spawnSync(binary.file, ["install", "--status", "--root", root, "--app", bundle], {
    cwd: env.HOME, env: { ...env, KUVO_HOME: home }, encoding: "utf8", timeout: 15000,
  });
  const current = ask(".kuvo-code");
  const alias = ask(".KuVo-CoDe");
  for (const answer of [current, alias]) {
    assert.equal(answer.status, 0, answer.error?.message ?? answer.stdout + answer.stderr);
  }
  assert.deepEqual(JSON.parse(alias.stdout), JSON.parse(current.stdout));
  assert.equal(ask(".kuvo-demo").status, 2);
  macInstallerBundle(env.HOME, "run.kuvo.code");
  const refused = ask(".KuVo-CoDe");
  assert.equal(refused.status, 1, refused.error?.message ?? refused.stdout + refused.stderr);
  assert.deepEqual(JSON.parse(refused.stdout), {
    result: "refused", message: "Kuvo itself sets up ~/.kuvo-code only.",
  });
  assert.equal(fs.existsSync(home), false);
  assert.equal(fs.existsSync(path.join(env.HOME, ".KuVo-CoDe")), false);
});

test("the Mac installer preserves a machine and the earlier app's home", t => {
  if (boxBinary().typescript || process.platform !== "darwin") return t.skip("Uses the native Mac installer.");
  const { binary, env, key, release, body } = box(t);
  const identifier = "run.kuvo.code.installer-fixture";
  const bundle = macInstallerBundle(env.HOME, identifier);
  const home = path.join(env.HOME, ".kuvo-code");
  const legacy = path.join(env.HOME, "Library/Application Support", identifier);
  write(path.join(home, "box/kind"), "machine\n");
  write(path.join(home, "bin/kuvo-start"), "#!/bin/sh\nexit 1\n", { mode: 0o700 });
  write(path.join(home, `engine/${body.engine}/opencode`), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  write(path.join(home, "workspace/notes"), "machine's own work");
  write(path.join(legacy, "Chats/c1/notes.md"), "the earlier app's work");
  write(path.join(release, "box.tar.gz"), "pack");
  write(path.join(release, "boxes.json"), encode(envelope({
    ...body, launch: "rust", packs: { [target]: body.pack },
  }, key)));
  const entry = path.join(release, "kuvo-install");
  fs.copyFileSync(new URL("../../box/src/install/kuvo-install", import.meta.url), entry);
  fs.copyFileSync(binary.file, path.join(release, "kuvo-install-bin"));
  const replies = ["status", "move", "new"].map(verb => {
    const run = spawnSync("/bin/sh", [entry, verb, "--app", bundle], {
      cwd: env.HOME, env: { ...env, KUVO_TEST_RELEASE_KEY: key.key }, encoding: "utf8", timeout: 15000,
    });
    assert.ifError(run.error);
    const { verdict, verb: offered, message, result } = JSON.parse(run.stdout);
    return [run.status, verdict, offered ?? result, message];
  });
  assert.deepEqual(replies, [
    [0, "two", undefined, "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet."],
    [1, "two", "refused", "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet."],
    [1, "two", "refused", "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: both stay as they are."],
  ]);
  assert.equal(fs.readFileSync(path.join(home, "workspace/notes"), "utf8"), "machine's own work");
  assert.equal(fs.readFileSync(path.join(legacy, "Chats/c1/notes.md"), "utf8"), "the earlier app's work");
  assert.deepEqual(fs.readdirSync(home).sort(), ["bin", "box", "engine", "workspace"]);
});

function macInstallerRequest({ binary, env, release }, bundle, verb) {
  const home = path.join(env.HOME, ".kuvo-code");
  const install = ["install", `--${verb}`, "--root", ".kuvo-code", "--app", bundle];
  const args = binary.typescript
    ? ["--config=/dev/null", "--no-env-file", binary.file, ...install, "--release", release, "--engine", path.join(env.HOME, "engine")]
    : verb === "status" ? install : ["install-media", release, ".kuvo-code", bundle, ...(verb === "new" ? ["--start"] : [])];
  const run = spawnSync(binary.typescript ? typescriptRuntime() : binary.file, args, {
    cwd: env.HOME, env: { ...env, KUVO_HOME: home }, encoding: "utf8", timeout: 15000,
  });
  assert.ifError(run.error);
  return [run.status, JSON.parse(run.stdout)];
}

test("the Mac installer offers a move when the earlier app still has work", t => {
  if (process.platform !== "darwin") return t.skip("Uses the earlier Mac app's home.");
  const fixture = box(t);
  const { env } = fixture;
  const identifier = "run.kuvo.code.earlier-fixture";
  const bundle = macInstallerBundle(env.HOME, identifier);
  const earlier = path.join(env.HOME, "Library/Application Support", identifier);
  const notes = path.join(earlier, "Chats/c1/notes.md");
  write(notes, "an earlier conversation");
  for (const verdict of ["app-07", "app-box"]) {
    if (verdict === "app-box") write(path.join(earlier, "box/run/keeper.json"), "{}");
    const [code, status] = macInstallerRequest(fixture, bundle, "status");
    assert.equal(code, 0);
    assert.equal(status.verdict, verdict);
    assert.equal(status.verb, "move");
    assert.deepEqual(macInstallerRequest(fixture, bundle, "new"), [1, {
      result: "refused", verdict, status: status.status,
      message: "Kuvo Code has your sessions and settings on this Mac already: they move into their new place instead.",
    }]);
    assert.equal(fs.readFileSync(notes, "utf8"), "an earlier conversation");
    assert.equal(fs.existsSync(path.join(env.HOME, ".kuvo-code")), false);
  }
});

test("the Mac installer keeps an existing machine in place", t => {
  if (process.platform !== "darwin") return t.skip("Uses a Mac machine installation.");
  const fixture = box(t);
  const { env } = fixture;
  const bundle = macInstallerBundle(env.HOME, "run.kuvo.code.machine-fixture");
  const home = path.join(env.HOME, ".kuvo-code");
  const keeper = "#!/bin/sh\nexit 64\n";
  write(path.join(home, "box/kind"), "machine\n");
  write(path.join(home, "bin/kuvo-start"), keeper, { mode: 0o700 });
  write(path.join(home, "workspace/notes"), "machine work");
  const [code, status] = macInstallerRequest(fixture, bundle, "status");
  assert.equal(code, 0);
  assert.equal(status.verdict, "machine");
  assert.equal(status.verb, null);
  const messages = {
    move: "This Mac is set up as one of your machines already: Kuvo uses what's here.",
    new: "This computer is set up as one of your machines already: Kuvo uses what's here.",
  };
  for (const [verb, message] of Object.entries(messages)) {
    assert.deepEqual(macInstallerRequest(fixture, bundle, verb), [1, {
      result: "refused", verdict: "machine", status: status.status, message,
    }]);
  }
  assert.equal(fs.readFileSync(path.join(home, "bin/kuvo-start"), "utf8"), keeper);
  assert.equal(fs.readFileSync(path.join(home, "workspace/notes"), "utf8"), "machine work");
  assert.deepEqual(fs.readdirSync(home).sort(), ["bin", "box", "workspace"]);
});

test("the Mac installer leaves a fresh home alone when asked to move", t => {
  if (process.platform !== "darwin") return t.skip("Uses the Mac move action.");
  const fixture = box(t);
  const home = path.join(fixture.env.HOME, ".kuvo-code");
  const bundle = macInstallerBundle(fixture.env.HOME, "run.kuvo.code.fresh-fixture");
  const [code, status] = macInstallerRequest(fixture, bundle, "status");
  assert.equal(code, 0);
  assert.deepEqual([status.verdict, status.verb], ["fresh", "new"]);
  assert.deepEqual(macInstallerRequest(fixture, bundle, "move"), [1, {
    result: "refused", verdict: "fresh", status: status.status,
    message: "There's nothing of an earlier Kuvo Code's on this Mac to move.",
  }]);
  assert.equal(fs.existsSync(home), false);
});

test("the Mac installer keeps a home whose sessions were moved back", t => {
  if (process.platform !== "darwin") return t.skip("Uses the Mac move record.");
  const fixture = box(t);
  const home = path.join(fixture.env.HOME, ".kuvo-code");
  const bundle = macInstallerBundle(fixture.env.HOME, "run.kuvo.code.moved-back-fixture");
  const record = path.join(home, "updates/rehome/state.json");
  const saved = '{"state":"back","way":"back","movedBack":true}';
  write(record, saved);
  const [code, status] = macInstallerRequest(fixture, bundle, "status");
  assert.equal(code, 0);
  assert.deepEqual([status.verdict, status.verb], ["back", null]);
  for (const verb of ["move", "new"]) {
    assert.deepEqual(macInstallerRequest(fixture, bundle, verb), [1, {
      result: "refused", verdict: "back", status: status.status,
      message: "Kuvo Code's sessions on this Mac were moved back: they move again when you ask.",
    }]);
  }
  assert.equal(fs.readFileSync(record, "utf8"), saved);
  assert.deepEqual(fs.readdirSync(path.join(home, "updates")), ["rehome"]);
});

test("the Mac installer keeps a saved move when asked for new setup", t => {
  if (process.platform !== "darwin") return t.skip("Uses the Mac move journal.");
  const fixture = box(t);
  const home = path.join(fixture.env.HOME, ".kuvo-code");
  const bundle = macInstallerBundle(fixture.env.HOME, "run.kuvo.code.saved-move-fixture");
  const journal = path.join(home, "updates/rehome/journal.json");
  const cases = [
    ["clone", "moving", "Kuvo is moving your sessions to their new place on this Mac: that's finished first."],
    ["clear", "stuck", "Kuvo was moving your sessions to their new place on this Mac and stopped part way: that's finished first."],
  ];
  for (const [step, verdict, message] of cases) {
    const saved = JSON.stringify({ v: 1, way: "move", began: 1, step,
      from: path.join(fixture.env.HOME, "earlier"), to: home, placements: [], clearing: [] });
    write(journal, saved);
    const [code, status] = macInstallerRequest(fixture, bundle, "status");
    assert.equal(code, 0);
    assert.deepEqual([status.verdict, status.verb, status.status.step], [verdict, "move", step]);
    assert.deepEqual(macInstallerRequest(fixture, bundle, "new"), [1, {
      result: "refused", verdict, status: status.status, message,
    }]);
    assert.equal(fs.readFileSync(journal, "utf8"), saved);
    assert.deepEqual(fs.readdirSync(path.join(home, "updates")), ["rehome"]);
  }
});

test("the Mac installer leaves an unreadable move journal untouched", t => {
  if (process.platform !== "darwin") return t.skip("Uses a Mac move journal.");
  const fixture = box(t);
  const { env, body, key, release } = fixture;
  const home = path.join(env.HOME, ".kuvo-code");
  const bundle = macInstallerBundle(env.HOME, "run.kuvo.code.bad-journal-fixture");
  const journal = path.join(home, "updates/rehome/journal.json");
  write(journal, "{");
  write(path.join(release, "box.tar.gz"), "pack");
  write(path.join(release, "boxes.json"), encode(envelope({
    ...body, launch: "rust", packs: { [target]: body.pack },
  }, key)));
  env.KUVO_TEST_RELEASE_KEY = key.key;
  const expected = macInstallerRequest(fixture, bundle, "status");
  assert.equal(expected[0], 1);
  assert.equal(expected[1].result, "refused");
  assert.match(expected[1].message, /can't tell how far it got/);
  for (const verb of ["move", "new"]) assert.deepEqual(macInstallerRequest(fixture, bundle, verb), expected);
  assert.equal(fs.readFileSync(journal, "utf8"), "{");
  assert.deepEqual(fs.readdirSync(path.join(home, "updates")), ["rehome"]);
});

test("the Mac installer offers to start a loaded Helper that has stopped", t => {
  if (process.platform !== "darwin") return t.skip("Uses the Mac Helper registration.");
  const fixture = box(t);
  const { env } = fixture;
  env.HOME = fs.realpathSync(env.HOME);
  const home = path.join(env.HOME, ".kuvo-code");
  const bundle = macInstallerBundle(env.HOME, "run.kuvo.code.helper-status-fixture");
  const tools = path.join(env.HOME, "launch-fixture");
  env.KUVO_TEST_UNINSTALL_TOOLS = tools;
  const helper = path.join(home, "bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper");
  write(path.join(home, "box/kind"), "mac\n");
  write(path.join(home, "bin/kuvo-box"), "");
  write(helper, "");
  const job = path.join(env.HOME, "Library/LaunchAgents/run.kuvo.code.machine.plist");
  const contents = `<plist><array><string>${helper}</string></array></plist>`;
  write(job, contents);
  write(path.join(tools, "launchctl"), `#!/bin/sh
[ "$1" = print ] || exit 64
cat "$KUVO_TEST_UNINSTALL_TOOLS/print-state"
`, { mode: 0o700 });
  for (const [state, verb] of [["state = running", null], ["state = waiting", "new"], ["service = job", null]]) {
    write(path.join(tools, "print-state"), state + "\n");
    const [code, status] = macInstallerRequest(fixture, bundle, "status");
    assert.equal(code, 0);
    assert.deepEqual([status.verdict, status.verb], ["ours", verb]);
  }
  write(path.join(tools, "print-state"), "state = running\n");
  write(path.join(home, "run/paused"), "");
  const [code, status] = macInstallerRequest(fixture, bundle, "status");
  assert.equal(code, 0);
  assert.deepEqual([status.verdict, status.verb], ["ours", "new"]);
  assert.equal(fs.readFileSync(job, "utf8"), contents);
  assert.equal(fs.existsSync(path.join(home, "updates/install")), false);
});

test("the Mac installer waits for a live move", async t => {
  if (process.platform !== "darwin") return t.skip("Uses the Mac move lock.");
  const fixture = box(t);
  const { env } = fixture;
  const bundle = macInstallerBundle(env.HOME, "run.kuvo.code.wait-fixture");
  const home = path.join(env.HOME, ".kuvo-code");
  const lock = path.join(home, "updates/rehome/lock/pid");
  const holder = launch(process.execPath, ["-e", "console.log('ready');process.stdin.resume()", "rehome"], env, env.HOME, "pipe");
  const ask = verb => macInstallerRequest(fixture, bundle, verb);
  try {
    await once(holder.child.stdout, "data");
    write(lock, String(holder.child.pid));
    const [code, status] = ask("status");
    assert.equal(code, 0);
    assert.equal(status.verdict, "waits");
    assert.equal(status.verb, null);
    for (const verb of ["move", "new"]) {
      assert.deepEqual(ask(verb), [4, {
        result: "waits", verdict: "waits", message: "Another move is under way.", status: status.status,
      }]);
    }
    assert.equal(fs.readFileSync(lock, "utf8"), String(holder.child.pid));
    assert.deepEqual(fs.readdirSync(path.join(home, "updates")), ["rehome"]);
  } finally {
    await holder.stop();
  }
  const [code, status] = ask("status");
  assert.equal(code, 0);
  assert.equal(status.verdict, "fresh");
  assert.equal(status.verb, "new");
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
    const installArgs = (verb, extra = []) => [path.join(media, "kuvo-install"), verb, "--app", app.bundle, ...extra];
    const installEnv = (account, launchTools) => ({
      ...environment, HOME: account, TMPDIR: path.join(account, "tmp"),
      PATH: `${launchTools}${path.delimiter}${environment.PATH}`,
      KUVO_TEST_RELEASE_KEY: key.key, KUVO_TEST_UNINSTALL_TOOLS: launchTools,
    });
    const install = (verb, account = app.home, launchTools = tools, extra = []) => spawnSync("/bin/sh", installArgs(verb, extra), {
      cwd: account, env: installEnv(account, launchTools), encoding: "utf8", timeout: 150000,
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
      const futureHome = path.join(fresh, ".kuvo-code");
      damagedInstallerStatus(verb => install(verb, fresh, freshTools), media, futureHome);
      fileEngineInstallerStatus(verb => install(verb, fresh, freshTools), media, futureHome, key);
      engineDownloadInstallerStatus(verb => install(verb, fresh, freshTools), futureHome, feed);
      await concurrentInstallStatus(
        () => launch("/bin/sh", installArgs("status"), installEnv(fresh, freshTools), fresh, "pipe"), futureHome,
      );
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

      const interrupted = path.join(root, "interrupted");
      const interruptedTools = path.join(interrupted, "launch-fixture");
      const unfinished = path.join(interrupted, ".kuvo-code");
      fs.mkdirSync(path.join(interrupted, "tmp"), { recursive: true });
      fs.cpSync(tools, interruptedTools, { recursive: true });
      write(path.join(unfinished, "updates/rehome/new"), "1\n");
      write(path.join(unfinished, "bin/kuvo-link.js"), "// half of the earlier install");
      write(path.join(unfinished, "bin.new/kuvo-start"), "half");
      write(path.join(unfinished, "updates/install/release.new/bin/kuvo-link.js"), "half");
      write(path.join(unfinished, "updates/install/engine/.fetch.1/engine.tgz"), "half");
      const before = install("status", interrupted, interruptedTools);
      assert.equal(before.status, 0, before.error?.message ?? before.stdout + before.stderr);
      assert.equal(JSON.parse(before.stdout).verdict, "fresh");
      const resumed = install("new", interrupted, interruptedTools);
      assert.equal(resumed.status, 0, resumed.error?.message ?? resumed.stdout + resumed.stderr);
      const resumedAnswer = JSON.parse(resumed.stdout);
      assert.deepEqual([resumedAnswer.result, resumedAnswer.verdict, resumedAnswer.started], ["done", "ours", true]);
      assert.equal(typeof resumedAnswer.setAside, "string", "the reply should name the saved earlier files");
      assert.equal(path.dirname(resumedAnswer.setAside), path.join(unfinished, "trash"));
      assert.match(path.basename(resumedAnswer.setAside), /^earlier-box-/);
      assert.equal(fs.readFileSync(path.join(resumedAnswer.setAside, "bin/kuvo-link.js"), "utf8"), "// half of the earlier install");
      for (const old of ["bin.new", "updates/install", "updates/rehome/new"]) {
        assert.equal(fs.existsSync(path.join(unfinished, old)), false, old);
      }
      installedRelease(unfinished);
      assert.equal(fs.readFileSync(path.join(interruptedTools, "calls"), "utf8"), "print\nbootstrap\n");
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
