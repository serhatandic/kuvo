// The update command admits only signed manifests and checks the selected pack before staging.
// Each scenario uses freshly generated test signing keys and isolated local packs.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { once } from "node:events";
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

function box(t) {
  const binary = boxBinary();
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, "update-home");
  const release = path.join(root, "release");
  const key = keypair();
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
  const here = await computer("update-status");
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

async function engineFeed(t, release) {
  const executable = process.platform === "win32" ? "opencode.exe" : "opencode";
  const source = path.join(release, "engine-source");
  const engine = path.join(source, "package/bin", executable);
  if (process.platform === "win32") {
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
      if (++requests === 1) { response.end("incomplete download"); return; }
      createReadStream(workerData.archive).pipe(response);
    });
    server.listen(0, "127.0.0.1", () => parentPort.postMessage(server.address().port));
  `, { eval: true, workerData: { archive, calls } });
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
  const keeper = launch(binary.file, ["keep"], env, home);
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
    keep("select", selected);
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
