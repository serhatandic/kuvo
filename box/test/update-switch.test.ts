import "./safety"
import { afterEach, expect, test } from "bun:test";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import { once } from "node:events";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { activeBin, convertBin, previousBin, stageVersion, versionBin } from "../src/link/updates/versions";

const repo = fileURLToPath(new URL("../../", import.meta.url));
const keeper = fs.readFileSync(path.join(repo, "box/src/keeper/kuvo-start"), "utf8");
const bundle = path.join(repo, "box/dist/kuvo-link.js");
const bridge = path.join(repo, "box/dist/kuvo-agents.js");
const homes: string[] = [];
const dirs = ["bin", "config/opencode/plugins", "run"];
function put(file: string, value: string | Buffer, mode = 0o600) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, value, { mode });
}

function home(version = "0.8.0-beta.35") {
  const user = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-switch-"));
  homes.push(user);
  const root = path.join(user, ".kuvo-code");
  for (const dir of dirs) fs.mkdirSync(path.join(root, dir), { recursive: true });
  put(path.join(root, "run/off"), "");
  files(root, version);
  return root;
}

function files(root: string, version: string, engine = "1.0.0") {
  put(path.join(root, "bin/kuvo-start"), keeper.replace("@ENGINE@", engine), 0o755);
  put(path.join(root, "bin/box-version"), version);
  for (const name of ["kuvo-link.js", "kuvo-agents.js", "kuvo-browser.js", "kuvo-wake", "runtime/tool"]) {
    put(path.join(root, "bin", name), version);
  }
  put(path.join(root, "config/opencode/plugins/kuvo.js"), version);
}

function assertParts(root: string, version: string, engine: string) {
  for (const file of ["bin/box-version", "bin/kuvo-link.js", "bin/kuvo-agents.js", "bin/kuvo-browser.js",
    "bin/kuvo-wake", "bin/runtime/tool", "config/opencode/plugins/kuvo.js"]) {
    expect(fs.readFileSync(path.join(root, file), "utf8")).toBe(version);
  }
  expect(fs.readFileSync(path.join(root, "bin/kuvo-start"), "utf8")).toContain(`\nV=${engine}\n`);
}

function run(root: string, script: string, ...args: string[]) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KUVO_")));
  return spawnSync("sh", [script, ...args], { env: { ...env, HOME: path.dirname(root), PATH: `${root}/test-bin:${env.PATH}` },
    encoding: "utf8", timeout: 10_000 });
}

afterEach(() => {
  for (const user of homes.splice(0)) fs.rmSync(user, { recursive: true, force: true });
});

for (const version of ["0.8.0-beta.35", "0.8.0-beta.40", "0.8.0-beta.41", "0.7.2"]) {
  test(`a plain ${version} bin survives interrupted conversion and all parts follow the switch`, async () => {
    const root = home(version);
    if (version === "0.7.2") put(path.join(root, "bin/.own/kuvo-start"), "legacy Mac copy");
    const helper = path.join(root, "bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper");
    let enrolled: number | undefined;
    if (process.platform === "darwin") {
      put(helper, "enrolled helper");
      enrolled = fs.statSync(helper).ino;
    }
    // A crash after exchanging the plugin directory, before exchanging bin.
    fs.renameSync(path.join(root, "config/opencode/plugins"), path.join(root, "bin/opencode-plugins"));
    fs.symlinkSync("../../bin/opencode-plugins", path.join(root, "config/opencode/plugins"));
    assertParts(root, version, "1.0.0");
    const old = await convertBin(root, version, "1".repeat(64));
    expect(await convertBin(root, version)).toBe(old);
    assertParts(root, version, "1.0.0");

    const release = path.join(root, "unpacked");
    files(release, "0.8.0-beta.42", "2.0.0");
    const next = versionBin("0.8.0-beta.42", "2".repeat(64));
    await stageVersion(root, release, next, "{}");
    // A crash after writing previous or the replacement link still boots only the old code.
    put(previousBin(root), old + "\n");
    fs.symlinkSync(next, path.join(root, "bin.new"));
    assertParts(root, version, "1.0.0");
    const switched = run(root, path.join(root, next, "kuvo-start"), "stop", "--switch", next);
    expect(switched.stderr).toBe("");
    expect(switched.status).toBe(0);
    // The keeper hasn't started yet. A restart after the rename sees the whole new release.
    expect(activeBin(root)).toBe(next);
    expect(fs.readFileSync(previousBin(root), "utf8").trim()).toBe(old);
    assertParts(root, "0.8.0-beta.42", "2.0.0");
    if (process.platform === "darwin") {
      expect(fs.realpathSync(path.join(root, "bin/Kuvo Helper.app"))).toBe(
        fs.realpathSync(path.join(root, "helper/Kuvo Helper.app")));
      expect(fs.statSync(helper).ino).toBe(enrolled!);
    }
  });
}

for (const version of ["0.8.0-beta.40", "0.8.0-beta.41", "0.7.2"]) test(`the keeper restores ${version} when a resident trial cannot start`, async () => {
  const root = home(version);
  const old = await convertBin(root, version, "1".repeat(64));
  const release = path.join(root, "unpacked");
  files(release, "0.8.0-beta.39", "2.0.0");
  put(path.join(release, "bin/kuvo-link.js"), "// Kuvo resident serve v1\nprocess.exit(1);\n");
  const next = versionBin("0.8.0-beta.39", "2".repeat(64));
  await stageVersion(root, release, next, "{}");
  put(previousBin(root), old + "\n");
  expect(run(root, path.join(root, next, "kuvo-start"), "stop", "--switch", next).status).toBe(0);
  // Advance the guard's clock past two minutes immediately. The driver is the keeper
  // identity for this test. There are no worker PIDs to signal.
  put(path.join(root, "test-bin/date"), `#!/bin/sh
tick="${root}/test-clock"
if [ -f "$tick" ]; then echo 222; else : >"$tick"; echo 100; fi
`, 0o700);
  put(path.join(root, "test-bin/sync"), "#!/bin/sh\nexit 0\n", 0o700);
  const keeperPath = path.join(root, "bin/kuvo-start");
  const guard = `
echo $$ >"${root}/run/box.pid"
sh "${keeperPath}" start-guard "${next}" "$$"
exit $?
`;
  const guardDriver = path.join(root, "bin/kuvo-start.guard-test");
  put(guardDriver, guard);
  const rolled = run(root, guardDriver);
  expect(rolled.stderr).toBe("");
  expect(rolled.status).toBe(0);
  expect(activeBin(root)).toBe(old);
  expect(fs.existsSync(previousBin(root))).toBe(false);
  expect(fs.readFileSync(path.join(root, "run/update-cannot"), "utf8")).toContain("0.8.0-beta.39");
  assertParts(root, version, "1.0.0");
}, 20_000);

test("prepare rejects the broken internal route argv and switches a signed release from beta.35", async () => {
  const root = home();
  fs.unlinkSync(path.join(root, "run/off"));
  put(path.join(root, "box/kind"), "machine\n");
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KUVO_")));
  const runtime = path.join(root, "engine/1.0.0/opencode");
  // The engine's Bun is the only stand-in. Both staged bundles otherwise run as shipped.
  fs.mkdirSync(path.dirname(runtime), { recursive: true });
  fs.symlinkSync(process.execPath, runtime);
  // The pinned beta.35 link is also the old owner in setup-handover.test.js.
  const oldFiles = {
    "kuvo-start": "box/src/keeper/kuvo-start",
    "kuvo-link.js": "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js",
    "kuvo-agents.js": "box/dist/kuvo-agents.js",
  };
  for (const [name, file] of Object.entries(oldFiles)) {
    const archived = spawnSync("git", ["show", `fccda2de1:${file}`], { cwd: repo, maxBuffer: 16_000_000 });
    expect(archived.status, archived.stderr.toString()).toBe(0);
    const bytes = name === "kuvo-start" ? archived.stdout.toString().replace("@ENGINE@", "1.0.0") : archived.stdout;
    put(path.join(root, "bin", name), bytes, 0o700);
  }
  const oldLink = sha(fs.readFileSync(path.join(root, "bin/kuvo-link.js")));
  const version = "0.8.0-beta.39";
  const data = path.join(root, "payload");
  files(data, version);
  const link = fs.readFileSync(bundle, "utf8");
  for (const broken of [true, false]) {
    const script = link.replace('flag === "--internal"', broken ? 'process.argv[5] === "--internal"' : 'flag === "--internal"');
    if (broken) expect(script).not.toBe(link);
    put(path.join(data, "bin/kuvo-link.js"), script);
    put(path.join(data, "bin/kuvo-agents.js"), fs.readFileSync(bridge));
    const entries = ["bin/box-version", "bin/kuvo-start", "bin/kuvo-link.js", "bin/kuvo-agents.js",
      "bin/kuvo-browser.js", "bin/kuvo-wake", "bin/runtime/tool", "config/opencode/plugins/kuvo.js"];
    const files = Object.fromEntries(entries.map(file => [file, sha(fs.readFileSync(path.join(data, file)))]));
    put(path.join(data, "box.json"), JSON.stringify({ version, engine: "1.0.0", files }));
    const pack = path.join(root, "box.tar.gz");
    expect(spawnSync("tar", ["-czf", pack, "-C", data, "."]).status).toBe(0);
    const manifest = Buffer.from(JSON.stringify({
      kuvo: "box/1", version, engine: "1.0.0", spreads: true, pack: { url: "https://example.invalid/box.tar.gz", sha256: sha(fs.readFileSync(pack)) },
    }));
    const key = crypto.generateKeyPairSync("ed25519");
    const pinned = key.publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
    put(path.join(root, "boxes.json"), JSON.stringify({ manifest: manifest.toString("base64"), key: pinned,
      signature: crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), key.privateKey).toString("base64") }));
    const expected = { version, sha256: sha(fs.readFileSync(pack)) };
    const updater = spawnSync(process.execPath, ["--config=/dev/null", "--no-env-file", "-e", `
      import fs from "node:fs";
      import * as box from ${JSON.stringify(bundle)};
      const from = ${JSON.stringify(root)};
      const keys = [${JSON.stringify(pinned)}];
      const expected = ${JSON.stringify(expected)};
      const options = { keys, blocked: null, expected };
      box.fleetWith(options);
      const prepared = await box.update({ ...options, from, phase: "prepare" });
      if (prepared.result !== "waiting") console.log(JSON.stringify(prepared));
      else {
        if (!box.keepRelease(fs.readFileSync(from + "/boxes.json"), fs.readFileSync(from + "/box.tar.gz"))) throw new Error("Could not keep release");
        console.log(JSON.stringify(await box.update({ ...options, phase: "activate" })));
      }
    `], { env: { ...env, HOME: path.dirname(root) }, encoding: "utf8", timeout: 30_000 });
    expect(updater.stderr).toBe("");
    expect(updater.status).toBe(0);
    const outcome = JSON.parse(updater.stdout);
    if (broken) {
      expect(outcome.result).toBe("failed");
      expect(outcome.message).toContain("internal kuvo/info");
      expect(activeBin(root)).toBeNull();
      expect(fs.readFileSync(path.join(root, "bin/box-version"), "utf8")).toBe("0.8.0-beta.35");
      expect(sha(fs.readFileSync(path.join(root, "bin/kuvo-link.js")))).toBe(oldLink);
    } else {
      expect<string>(outcome.result, outcome.message).toBe("updated");
      expect(fs.readFileSync(path.join(root, "bin/box-version"), "utf8")).toBe(version);
    }
  }
}, 60_000);

const sha = (data: Buffer) => crypto.createHash("sha256").update(data).digest("hex");

test("an interrupted engine turn gets one continue and an adopted bridge turn gets none", async () => {
  const root = home();
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KUVO_")));
  const source = path.join(root, "run/check-turns.ts");
  const module = path.join(repo, "box/src/link/updates/interrupted.ts");
  put(source, `
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { rememberCutTurns, continueCutTurns } from ${JSON.stringify(module)};

const run = path.join(process.env.HOME!, ".kuvo-code/run");
const sent: string[] = [];
let started = false;
const engine = Bun.serve({
  hostname: "127.0.0.1", port: 0,
  async fetch(request) {
    const route = new URL(request.url).pathname;
    if (route === "/api/session/active") return Response.json({ data: started ? [] : ["interrupted"] });
    if (route.includes("/prompt-receipt/")) return new Response(null, { status: 404 });
    if (route === "/api/session/interrupted/prompt") {
      sent.push((await request.json()).text);
      return Response.json({});
    }
    return new Response(null, { status: 400 });
  },
});
const bridge = Bun.serve({
  hostname: "127.0.0.1", port: 0,
  fetch(request) {
    assert.equal(new URL(request.url).pathname, "/api/session/active");
    return Response.json({ data: ["adopted"] });
  },
});
function records(pid: number) {
  fs.writeFileSync(path.join(run, "engine.json"), JSON.stringify({ pid, port: engine.port, password: "test" }));
  fs.writeFileSync(path.join(run, "bridge.json"), JSON.stringify({ pid, port: bridge.port, password: "test" }));
}
const root = path.dirname(run);
records(1);
await rememberCutTurns(root);
assert.equal(JSON.parse(fs.readFileSync(path.join(run, "continue-after-update.json"), "utf8")).length, 2);
started = true;
records(2);
await continueCutTurns(root);
await continueCutTurns(root);
assert.deepEqual(sent, ["continue"]);
assert.equal(fs.existsSync(path.join(run, "continue-after-update.json")), false);
engine.stop();
bridge.stop();
`);
  const check = spawnSync(process.execPath, ["--config=/dev/null", "--no-env-file", source], {
    env: { ...env, HOME: path.dirname(root) }, encoding: "utf8", timeout: 20_000,
  });
  expect(check.stderr).toBe("");
  expect(check.status).toBe(0);
}, 25_000);

test("the guard commits the switch after the selected resident generation answers", async () => {
  const root = home();
  const old = await convertBin(root, "0.8.0-beta.35", "1".repeat(64));
  const release = path.join(root, "unpacked");
  files(release, "0.8.0-beta.39");
  const script = fs.readFileSync(bundle);
  put(path.join(release, "bin/kuvo-link.js"), script);
  const layout = /^\/\/ Kuvo folder layout (\d+)/.exec(script.toString("utf8"))?.[1];
  expect(layout).toBeDefined();
  put(path.join(root, "folder-version.json"), `{"version":${layout}}\n`);
  const runtime = path.join(root, "engine/1.0.0/opencode");
  fs.mkdirSync(path.dirname(runtime), { recursive: true });
  fs.symlinkSync(process.execPath, runtime);
  const next = versionBin("0.8.0-beta.39", "2".repeat(64));
  await stageVersion(root, release, next, "{}");
  put(previousBin(root), old + "\n");
  expect(run(root, path.join(root, next, "kuvo-start"), "stop", "--switch", next).status).toBe(0);
  const keeperPath = path.join(root, "bin/kuvo-start");
  const driver = keeperPath + ".guard-test";
  put(driver, `
echo $$ >"${root}/run/box.pid"
echo $$ >"${root}/run/agents.pid"
printf '{"pid":%s,"port":1,"password":"test"}\\n' "$$" >"${root}/run/bridge.json"
printf '{"pid":%s,"port":1,"password":"test","version":"1.0.0"}\\n' "$$" >"${root}/run/engine.json"
sh "${keeperPath}" start-guard "${next}" "$$"
exit $?
`);
  fs.rmSync(path.join(root, "run/off"));
  const server = spawn(process.execPath, [path.join(root, "bin", "kuvo-link.js"), "serve", "--resident"], {
    env: { PATH: process.env.PATH, HOME: path.dirname(root), KUVO_HOME: root }, stdio: ["ignore", "pipe", "inherit"],
  });
  const ended = once(server, "exit");
  try {
    const ready = await Promise.race([once(server.stdout!, "data"), ended]);
    expect(String(ready[0])).toContain("127.0.0.1");
    const checked = run(root, driver, runtime, "kuvo-agents");
    expect(checked.stderr).toBe("");
    expect(checked.status).toBe(0);
    expect(fs.existsSync(previousBin(root))).toBe(false);
    expect(activeBin(root)).toBe(next);
  } finally {
    if (server.exitCode === null) server.kill();
    await ended;
  }
}, 32_000);

test("a server-only switch keeps the running engine and its generated sandbox files", async () => {
  const version = "0.8.0-beta.41";
  const root = home(version);
  const plugin = path.join(root, "config/opencode/plugins/kuvo-sandbox.js");
  const defaults = path.join(root, "config/opencode/plugins/kuvo-defaults.js");
  put(plugin, "generated plug-in");
  put(defaults, "generated defaults");
  put(path.join(root, "bin/kuvo-fence"), "generated launcher", 0o700);
  put(path.join(root, "bin/kuvo-fence.js"), "generated Linux runner");
  const timestamp = new Date(1_700_000_000_000);
  fs.utimesSync(plugin, timestamp, timestamp);
  const old = await convertBin(root, version, "1".repeat(64));
  const release = path.join(root, "unpacked");
  files(release, version);
  put(path.join(release, "bin/box-version"), "0.8.0-beta.42");
  put(path.join(release, "bin/kuvo-link.js"), "// Kuvo resident serve v1\n");
  const next = versionBin("0.8.0-beta.42", "2".repeat(64));
  await stageVersion(root, release, next, "{}");
  const script = path.join(root, "engine/1.0.0/worker.js");
  put(script, 'console.log("ready"); setInterval(() => {}, 1000);');
  // On Linux the test runner can be the embedded Bun inside the engine executable.
  const engine = spawn(process.execPath, [script], {
    env: { PATH: process.env.PATH, HOME: path.dirname(root), BUN_BE_BUN: "1" },
    stdio: ["ignore", "pipe", "inherit"],
  });
  const ended = once(engine, "exit");
  try {
    await once(engine.stdout!, "data");
    put(path.join(root, "run/engine.json"), JSON.stringify({ pid: engine.pid, version: "1.0.0" }));
    put(previousBin(root), old + "\n");
    expect(run(root, path.join(root, next, "kuvo-start"), "stop", "--switch", next).status).toBe(0);
    expect(activeBin(root)).toBe(next);
    expect(fs.readFileSync(plugin, "utf8")).toBe("generated plug-in");
    expect(fs.statSync(plugin).mtimeMs).toBe(timestamp.getTime());
    expect(fs.readFileSync(defaults, "utf8")).toBe("generated defaults");
    expect(fs.statSync(path.join(root, "bin/kuvo-fence")).mode & 0o777).toBe(0o700);
    expect(fs.readFileSync(path.join(root, "bin/kuvo-fence.js"), "utf8")).toBe("generated Linux runner");
    expect(JSON.parse(fs.readFileSync(path.join(root, "run/engine.json"), "utf8")).pid).toBe(engine.pid);
    expect(() => process.kill(engine.pid!, 0)).not.toThrow();
  } finally {
    if (engine.exitCode === null) engine.kill();
    await ended;
  }
}, 20_000);
