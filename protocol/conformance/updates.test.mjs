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

import { boxBinary, isolatedEnv, launch, readJSON, request, scratch, typescriptRuntime, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

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
  function update(signed, pack = "pack") {
    write(path.join(release, "boxes.json"), JSON.stringify(signed));
    write(path.join(release, "box.tar.gz"), pack);
    const args = ["update", "--phase", "prepare", "--from", release, "--key", key.key];
    const run = spawnSync(binary.typescript ? typescriptRuntime() : binary.file,
      binary.typescript ? [`--config=${process.platform === "win32" ? "NUL" : "/dev/null"}`, "--no-env-file", binary.file, ...args] : args, {
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
  const { key, body, update } = box(t);
  assert.deepEqual(update(envelope({ ...body, version: "0.7.2" }, key)), {
    result: "current", version: "0.8.0-beta.1",
  });
  assert.deepEqual(update(envelope({ ...body, spreads: false }, key)), {
    result: "failed", message: "This release cannot be shared with your setup.",
  });
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
  const file = path.join(release, "target.tar.gz");
  const archived = spawnSync("tar", ["cf", path.join(release, "target.tar"), "-C", packRoot, "box.json", "bin"], { encoding: "utf8", timeout: 90000 });
  assert.equal(archived.status, 0, archived.stderr);
  write(file, gzipSync(fs.readFileSync(path.join(release, "target.tar")), { level: 1 }));
  const hash = digest(fs.readFileSync(file));
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
    assert.equal(fs.readFileSync(engine.calls, "utf8"), "download\ndownload\n");
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
test("a new main finishes a prepared release after the old main goes away", { timeout: 180000 }, t => fleetRun(t, { failover: true }));
test("a TypeScript beta computer that missed the switch catches up to Rust", { timeout: 180000 }, t => fleetRun(t, { returningTypescript: true }));

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

async function fleetRun(t, { failover, returningTypescript, shellKeepers }) {
  const binary = boxBinary();
  if (binary.typescript || process.platform === "win32") {
    return t.skip("The fleet process fixture uses the Rust keeper and Unix enrollment.");
  }
  const typescript = process.env.KUVO_CONFORMANCE_TS_BIN;
  if (returningTypescript && !typescript) return t.skip("Set KUVO_CONFORMANCE_TS_BIN to the TypeScript bundle.");
  const { release, key, body } = box(t);
  const { computer, leases, relay } = await bench(t);
  const [main, member, away] = await Promise.all([computer("Studio"), computer("Lab"), computer("Away")]);
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
  const initial = "versions/0.8.0-beta.1-000000000000/bin";
  const executable = fs.readFileSync(binary.file);
  const packRoot = path.join(release, "fleet-source");
  const keeperFile = name => fileURLToPath(new URL(`../../box/src/keeper/${name}`, import.meta.url));
  const starter = fs.readFileSync(keeperFile("kuvo-start"), "utf8").replaceAll("@ENGINE@", body.engine);
  const setsid = fs.readFileSync(keeperFile("kuvo-setsid"));
  const files = {
    "bin/kuvo-box": executable,
    "bin/box-version": Buffer.from(version + "\n"),
    "bin/kuvo-start": starter,
    "bin/kuvo-setsid": setsid,
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
  const waiting = [];
  let hold = true;
  let retryDownload = false;
  let downloads = 0;
  const feed = createServer((req, res) => {
    const url = `http://127.0.0.1:${feed.address().port}/fleet.tar.gz`;
    if (req.url === "/boxes.json") {
      const descriptor = { url, sha256: hash };
      const signed = envelope({ ...body, pack: descriptor, launch: "rust", packs: { [target]: descriptor } }, key);
      res.end(encode(signed));
    } else if (req.url === "/fleet.tar.gz") {
      if (retryDownload) {
        retryDownload = false;
        res.writeHead(503).end();
      } else if (++downloads > 1 && hold) waiting.push(res);
      else res.end(pack);
    } else {
      res.writeHead(404).end();
    }
  });
  feed.listen(0, "127.0.0.1");
  await once(feed, "listening");
  const feedURL = `http://127.0.0.1:${feed.address().port}/boxes.json`;
  const env = computer => ({
    ...computer.env, KUVO_HOME: computer.home, KUVO_TEST_RELAY: relay.url, KUVO_RELAY_URL: relay.url,
    KUVO_TEST_RELEASE_FEED: feedURL, KUVO_TEST_RELEASE_KEY: key.key, KUVO_TEST_KEEP_ROUND: "1",
  });
  const legacy = peer => returningTypescript && peer === away;
  const shell = peer => shellKeepers || legacy(peer);
  function control(computer, ...args) {
    const result = shell(computer)
      ? spawnSync("/bin/sh", [path.join(computer.home, "bin/kuvo-start"), ...args], { cwd: computer.home, env: env(computer), encoding: "utf8", timeout: 15000 })
      : spawnSync(binary.file, ["keep", ...args], { cwd: computer.home, env: env(computer), encoding: "utf8", timeout: 15000 });
    assert.equal(result.status, 0, result.error?.message ?? result.stderr);
  }
  for (const peer of [main, member, away]) {
    if (shell(peer)) {
      const machineHome = path.join(peer.env.HOME, ".kuvo-code");
      fs.renameSync(peer.home, machineHome);
      peer.home = machineHome;
      write(path.join(peer.home, initial, "kuvo-start"), starter, { mode: 0o700 });
      write(path.join(peer.home, initial, "kuvo-setsid"), setsid, { mode: 0o700 });
      write(path.join(peer.home, initial, "box-version"), "0.8.0-beta.1\n");
      fs.mkdirSync(path.join(peer.home, initial, "opencode-plugins"));
      fs.symlinkSync(initial, path.join(peer.home, "bin"));
    }
    const engine = path.join(peer.home, "engine", body.engine, "opencode");
    if (legacy(peer)) {
      write(path.join(peer.home, initial, "kuvo-link.js"), fs.readFileSync(typescript));
      fs.rmSync(path.join(peer.home, "run/engine.json"));
      // Let the shell keeper use Bun in place of the embedded runtime.
      const runtime = "'" + typescriptRuntime().replaceAll("'", "'\\''") + "'";
      write(engine, `#!/bin/sh\n[ "$1" != --version ] || exit 0\nexec ${runtime} "$@"\n`, { mode: 0o700 });
      continue;
    }
    write(path.join(peer.home, initial, "kuvo-box"), executable, { mode: 0o700 });
    write(path.join(peer.home, initial, "..", "box.json"), encode({
      kuvo: "box/1", version: "0.8.0-beta.1", launch: "rust", engine: body.engine,
    }));
    write(path.join(peer.home, "run/engine.json"), encode({ ...peer.engine.record, version: body.engine }));
    write(engine, '#!/bin/sh\n[ "$1" = --version ]\n', { mode: 0o700 });
    if (!shell(peer)) control(peer, "select", initial);
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
    return ["serve.log", "update-start.log", "update.log", "repair.log"].map(name => {
      const log = path.join(peer.home, "run", name);
      return fs.existsSync(log) ? `${peer.home} ${name}\n${fs.readFileSync(log, "utf8")}` : "";
    }).join("\n") + keepers.get(peer).log();
  }).join("\n");
  const turnId = "feeda1234567";
  const holder = path.join(member.home, "run/agents", `kuvo-hold-${turnId}.js`);
  write(holder, "process.stdin.pipe(process.stdout)\n");
  const turn = launch(process.execPath, [holder, "release-fixture", turnId, "raw"], env(member), member.home, "pipe");
  const reply = async text => {
    const answer = once(turn.child.stdout, "data");
    turn.child.stdin.write(text + "\n");
    const [chunk] = await answer;
    assert.equal(chunk.toString(), text + "\n");
  };
  write(path.join(member.home, "run/agents/hold", `${turnId}.json`), encode({
    proto: "hold/1", holder: "release-fixture", id: turnId, kind: "codex", pid: turn.child.pid, fixture: true,
  }));
  try {
    const began = performance.now();
    start(main);
    start(member);
    await until(() => waiting.length && (staged(main) || staged(member)), diagnose, 90000);
    assert.equal(active(main), initial);
    assert.equal(active(member), initial);
    assert.equal(active(away), initial);
    t.diagnostic(`first preparation in ${Math.round(performance.now() - began)} ms`);

    await reply("turn before release switch");
    let leader = main;
    if (failover) {
      control(main, "off");
      await keepers.get(main).exited;
      assert.equal(active(main), initial);
      const offset = 130_000;
      for (const peer of [member, away]) write(path.join(peer.home, "run/test-setup-clock.json"), encode({ offset }));
      leases.advance(offset);
      await until(() => leases.leaseFor(setup.json.setup)?.holder !== setup.json.you.key,
        "the remaining online computer must claim the lease", 30000);
      leader = member;
      t.diagnostic(`a new main claimed the lease ${Math.round(performance.now() - began)} ms after the first boot`);
    }

    hold = false;
    const released = performance.now();
    for (const res of waiting) res.end(pack);
    await until(() => confirmed(leader), diagnose, 45000);
    t.diagnostic(`main confirmed ${Math.round(performance.now() - released)} ms after member download resumed`);
    await until(() => confirmed(member), diagnose, 45000);
    await reply("same turn after release switch");
    assert.equal(active(away), initial);
    t.diagnostic(`online member confirmed ${Math.round(performance.now() - released)} ms after its download resumed`);

    const returned = performance.now();
    retryDownload = Boolean(shellKeepers);
    start(away);
    await until(() => confirmed(away), diagnose, 65000);
    const resident = readJSON(path.join(away.home, "run/serve.json"));
    const info = await request(resident, "GET", "kuvo/info");
    assert.equal(info.status, 200, info.text);
    assert.equal(retryDownload, false);
    if (returningTypescript) {
      const copy = path.join(away.home, "backups/before-rust");
      assert.equal(fs.readlinkSync(path.join(copy, "bin")), initial);
      assert.equal(fs.readFileSync(path.join(copy, "bin/box-version"), "utf8"), "0.8.0-beta.1\n");
      assert.equal(readJSON(path.join(away.home, selected, "..", "box.json")).launch, "rust");
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

test("kuvo-install hands the two released Mac homes to the Rust move", { timeout: 180000 }, async t => {
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
    write(path.join(tree, "bin/kuvo-box"), fs.readFileSync(binary.file), { mode: 0o700 });
    write(path.join(tree, "bin/box-version"), version + "\n");
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
    const install = verb => spawnSync("/bin/sh", [path.join(media, "kuvo-install"), verb, "--app", app.bundle], {
      cwd: app.home,
      env: {
        ...environment, HOME: app.home, TMPDIR: path.join(app.home, "tmp"),
        KUVO_TEST_RELEASE_KEY: key.key, KUVO_TEST_UNINSTALL_TOOLS: tools,
      },
      encoding: "utf8", timeout: 90000,
    });
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
  assert.deepEqual(update(signed), { result: "waiting", version });
  const prepared = path.join(home, "versions", `${version}-${hash.slice(0, 12)}`);
  assert.equal(readJSON(path.join(prepared, "box.json")).launch, "rust");
  assert.deepEqual(readJSON(path.join(home, "run/fleet-ready.json")), { version, sha256: hash });
  assert.equal(fs.readFileSync(path.join(home, "bin/box-version"), "utf8"), "0.8.0-beta.1\n");
});
