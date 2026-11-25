// A signed release is the only source for repairing files an earlier Mac wrote.
// Both selected binaries execute their repair command against a disposable pack.
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import { createServer } from "node:http";
import { once } from "node:events";
import path from "node:path";
import { test } from "node:test";
import { gzipSync } from "node:zlib";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, until, readJSON, scratch, startBox, typescriptRuntime, write } from "./adapter.mjs";

import { withTag, tags } from "./compat/tagged.mjs";

const digest = bytes => createHash("sha256").update(bytes).digest("hex");
const encode = value => Buffer.from(JSON.stringify(value));
const version = "0.8.0", engine = "2.2.6";

function fixture(t, { feed = "http://127.0.0.1:1", live = false, wsl = false, binary = boxBinary() } = {}) {
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root), home = live || wsl ? path.join(env.HOME, ".kuvo-code") : path.join(root, "machine"), tree = path.join(root, "pack");
  privateHome(home);
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const key = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64url");
  const program = binary.typescript ? "kuvo-link.js" : process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box";
  const data = { "bin/box-version": version + "\n", "bin/kuvo-start": "#!/bin/sh\n# the release keeper\nV=2.2.6\n",
    "bin/kuvo-launch": "#!/bin/sh\n# the release launcher\n", "bin/kuvo-browser.js": "// the release browser\n",
    ["bin/" + program]: "the release program\n" };
  if (live) {
    data["bin/" + program] = fs.readFileSync(binary.file);
    data["bin/kuvo-start"] = fs.readFileSync(new URL("../../box/src/keeper/kuvo-start", import.meta.url), "utf8").replaceAll("@ENGINE@", engine);
    data["bin/.kuvo-start-keeper"] = data["bin/kuvo-start"];
    data["bin/kuvo-setsid"] = fs.readFileSync(new URL("../../box/src/keeper/kuvo-setsid", import.meta.url));
  }
  if (wsl) {
    // Each keeper says which one Windows started.
    data["bin/kuvo-start"] = '#!/bin/sh\n: > "$HOME/.kuvo-code/run/wsl-started-ordinary"\n';
    data["bin/.kuvo-start-keeper"] = '#!/bin/sh\n: > "$HOME/.kuvo-code/run/wsl-started"\n';
  }
  for (const [file, bytes] of Object.entries(data)) write(path.join(tree, file), bytes, { mode: 0o700 });
  const files = Object.fromEntries(Object.entries(data).map(([file, bytes]) => [file, digest(bytes)]));
  write(path.join(tree, "box.json"), encode({ kuvo: "box/1", version, engine, ...(binary.typescript ? {} : { launch: "rust" }), files }));
  const tar = path.join(root, "pack.tar");
  const made = spawnSync("tar", ["cf", tar, "-C", tree, "box.json", "bin"]);
  assert.equal(made.status, 0, String(made.stderr));
  const bytes = gzipSync(fs.readFileSync(tar), { level: 1 });
  const pack = { url: feed + "/v" + version + "/box.tar.gz", sha256: digest(bytes) };
  const target = (process.arch === "arm64" ? "aarch64" : "x86_64") + (process.platform === "darwin" ? "-apple-darwin" :
    process.platform === "win32" ? "-pc-windows-gnullvm" : "-unknown-linux-musl");
  const signed = version => encode({ kuvo: "box/1", version, engine, spreads: true, pack,
    ...(binary.typescript ? {} : { launch: "rust", packs: { [target]: pack } }) });
  const envelopeFor = version => {
    const body = signed(version);
    return encode({ key, manifest: body.toString("base64"),
      signature: sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), body]), privateKey).toString("base64") });
  };
  const envelope = envelopeFor(version);
  let folder;
  if (binary.typescript) {
    fs.cpSync(path.join(tree, "bin"), path.join(home, "bin"), { recursive: true });
    folder = path.join(home, "updates/release");
  } else {
    const selection = `versions/${version}-${pack.sha256.slice(0, 12)}/bin`;
    folder = path.join(home, path.dirname(selection));
    fs.cpSync(tree, folder, { recursive: true });
    write(path.join(folder, "bin/.installed.json"), encode({ formatVersion: 1, version, sha256: pack.sha256 }));
    if (process.platform === "win32") write(path.join(home, "active-release"), selection);
    else fs.symlinkSync(selection, path.join(home, "bin"), "dir");
  }
  const packFile = path.join(folder, binary.typescript ? "box.tar.gz" : "pack");
  write(packFile, bytes);
  write(path.join(folder, "boxes.json"), envelope);
  write(path.join(home, "folder-version.json"), '{"version":23}');
  const bin = binary.typescript || process.platform !== "win32" ? path.join(home, "bin") : path.join(folder, "bin");
  const args = ["repair", "--format-version", "1"];
  const runArgs = binary.typescript ? ["--no-env-file", binary.file, ...args] : args;
  const command = binary.typescript ? typescriptRuntime() : binary.file;
  const launchEnv = { ...env, KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
    KUVO_TEST_RELEASE_KEY: key, KUVO_TEST_RELEASE_FEED: feed };
  if (wsl) {
    for (const name of ["WSL_DISTRO_NAME", "WSL_INTEROP"]) if (process.env[name]) launchEnv[name] = process.env[name];
    // A scratch home registers a real (disposable) Windows task only when a test asks.
    launchEnv.KUVO_TEST_WSL_TASK = "1";
  }
  function repair() {
    const run = spawnSync(command, runArgs, {
        cwd: home, env: launchEnv, encoding: "utf8", timeout: 30000,
      });
    assert.equal(run.status, 0, run.stdout + run.stderr);
    return JSON.parse(run.stdout);
  }
  async function repairing() {
    const proc = launch(command, runArgs, launchEnv, home, "pipe");
    t.after(() => proc.stop());
    let output = "";
    proc.child.stdout.on("data", chunk => { output += chunk; });
    proc.child.stdin.end();
    const done = await proc.exited;
    assert.equal(done.code, 0, output + proc.log());
    return JSON.parse(output);
  }
  return { home, bin, folder, data, repair, repairing, bytes, packFile, envelopeFor, launchEnv, command, runArgs: args => binary.typescript ? ["--no-env-file", binary.file, ...args] : args };
}

test("an earlier Mac's files are put back from the selected signed release", { timeout: 60000 }, t => {
  const { home, bin, data, repair } = fixture(t);
  const restored = ["kuvo-start", "kuvo-launch", "kuvo-browser.js"];
  for (const name of restored) write(path.join(bin, name), "earlier Mac's file\n");
  write(path.join(bin, ".own/kuvo-start"), "stale saved copy");
  write(path.join(home, "run/mac-installing"), "");
  assert.deepEqual(repair(), { formatVersion: 1, result: "none", why: "busy" });
  assert.equal(fs.readFileSync(path.join(bin, "kuvo-launch"), "utf8"), "earlier Mac's file\n");
  fs.rmSync(path.join(home, "run/mac-installing"));
  const fixed = repair();
  assert.equal(fixed.result, "repaired", JSON.stringify(fixed));
  assert.equal(fixed.version, version);
  assert.deepEqual(fixed.files.sort(), restored.map(name => "bin/" + name).sort());
  for (const name of restored) assert.equal(fs.readFileSync(path.join(bin, name), "utf8"), data["bin/" + name]);
  assert.equal(fs.existsSync(path.join(bin, ".own")), false);
  assert.deepEqual(repair(), { formatVersion: 1, result: "sound", version });
  const record = readJSON(path.join(home, "run/repair.json"));
  assert.equal(record.result, "sound");
  assert.equal(record.version, version);
});

test("a damaged copy is refused, and repair fetches only the running signed release", { timeout: 60000 }, async t => {
  let offered, archive;
  const server = createServer((req, res) => {
    const bytes = req.url === "/v" + version + "/boxes.json" ? offered :
      req.url === "/v" + version + "/box.tar.gz" ? archive : undefined;
    if (!bytes) return res.writeHead(404).end();
    res.end(bytes);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => { server.closeAllConnections(); server.close(); });
  const fixture_ = fixture(t, { feed: "http://127.0.0.1:" + server.address().port });
  const { home, bin, data, repairing, packFile, envelopeFor, bytes } = fixture_;
  const file = path.join(bin, "kuvo-launch");
  write(file, "the old launch");
  write(path.join(bin, ".own/kuvo-launch"), "kept");
  fs.appendFileSync(packFile, "damaged");
  offered = envelopeFor("0.9.0");
  archive = bytes;
  assert.deepEqual(await repairing(), { formatVersion: 1, result: "damaged", version });
  assert.equal(fs.readFileSync(file, "utf8"), "the old launch");
  assert.equal(fs.existsSync(packFile), false);
  assert.equal(fs.existsSync(path.join(bin, ".own/kuvo-launch")), true);
  offered = envelopeFor(version);
  archive = Buffer.concat([bytes, Buffer.from("damaged download")]);
  assert.deepEqual(await repairing(), { formatVersion: 1, result: "none", why: "noRelease" });
  assert.equal(fs.existsSync(packFile), false);
  assert.equal(fs.readFileSync(file, "utf8"), "the old launch");
  archive = bytes;
  const result = await repairing();
  assert.deepEqual(result, { formatVersion: 1, result: "repaired", version, files: ["bin/kuvo-launch"] });
  assert.equal(fs.readFileSync(file, "utf8"), data["bin/kuvo-launch"]);
  assert.equal(fs.existsSync(path.join(bin, ".own")), false);
  assert.equal(readJSON(path.join(home, "run/repair.json")).version, version);
});

test("the saved keeper restores its overwritten launcher and leaves the resident answering", {
  timeout: 60000, skip: process.platform === "win32" || boxBinary().typescript,
}, async t => {
  const relay = await localRelay();
  t.after(() => relay.close());
  const { home, bin, data, launchEnv } = fixture(t, { live: true });
  const starter = path.join(bin, ".kuvo-start-keeper"), run = path.join(home, "run");
  const ordinary = path.join(bin, "kuvo-start"), launchFile = path.join(bin, "kuvo-launch");
  const older = new Date(Date.now() - 24 * 3600_000);
  const release = new Date(older.getTime() - 3600_000);
  fs.utimesSync(path.join(bin, "box-version"), release, release);
  write(ordinary, "#!/bin/sh\nexit 1 # retired Mac's boot script\n");
  write(launchFile, "#!/bin/sh\nexit 1 # retired Mac's launcher\n");
  write(path.join(run, "attached"), "");
  fs.utimesSync(path.join(run, "attached"), older, older);
  const env = { ...launchEnv, KUVO_TEST_KEEP_ROUND: "1", KUVO_TEST_RELAY: relay.url };
  const cron = path.join(env.HOME, "crontab.fixture");
  if (process.platform === "linux") {
    const tools = path.join(env.HOME, "tools");
    env.KUVO_TEST_UNINSTALL_TOOLS = tools;
    write(cron, '0 1 * * * echo another-job\n@reboot sh "$HOME/.kuvo-code/bin/kuvo-start" >/dev/null 2>&1 # kuvo-code: starts its engine again after a restart\n');
    write(path.join(tools, "crontab"), '#!/bin/sh\ncase "$1" in -l) cat "$HOME/crontab.fixture";; -) cat > "$HOME/crontab.fixture";; esac\n', { mode: 0o700 });
  }
  const diagnose = () => [keeper.log(), ...["repair.json", "repair.log", "serve.log", "box.pid"].map(name => {
    try { return name + ": " + fs.readFileSync(path.join(run, name), "utf8").slice(-1500); } catch { return name + ": absent"; }
  })].join("\n");
  const keeper = launch("sh", [starter, "keep"], env, home);
  try {
    await until(() => fs.readFileSync(ordinary, "utf8") === data["bin/kuvo-start"] &&
      fs.readFileSync(launchFile, "utf8") === data["bin/kuvo-launch"],
      () => "the saved keeper did not repair its files: " + diagnose(), 20000);
    if (process.platform === "linux") await until(() => fs.readFileSync(cron, "utf8") ===
      '0 1 * * * echo another-job\n@reboot sh "$HOME/.kuvo-code/bin/.kuvo-start-keeper" >/dev/null 2>&1 # kuvo-code: starts its engine again after a restart\n',
      "the saved keeper was not registered for restart");
    assert.equal(fs.existsSync(path.join(run, "attached")), false);
    assert.ok(Math.abs(fs.statSync(path.join(run, "older-mac")).mtimeMs - older.getTime()) < 1000);
    const serving = await until(async () => {
      try {
        const record = readJSON(path.join(run, "serve.json"));
        return (await request(record, "GET", "kuvo/info")).status === 200 && record;
      } catch { return false; }
    }, "the saved keeper did not start the resident", 20000);
    const repairRecord = path.join(run, "repair.json");
    await until(() => fs.existsSync(repairRecord) && readJSON(repairRecord).result === "repaired", "the first repair was not recorded");
    write(launchFile, "a later Mac visit");
    write(path.join(run, "attached"), "");
    const visit = new Date(fs.statSync(repairRecord).mtimeMs + 1000);
    fs.utimesSync(path.join(run, "attached"), visit, visit);
    await until(() => fs.readFileSync(launchFile, "utf8") === data["bin/kuvo-launch"], () => "later visit was not repaired: " + diagnose(), 20000);
    assert.equal(readJSON(path.join(run, "serve.json")).pid, serving.pid);
  } finally {
    // The real keeper's off verb stops only the recorded resident it started.
    const off = launch("sh", [starter, "off"], env, home);
    await off.exited;
    await keeper.stop();
  }
});

// The Windows side of WSL 2, through PowerShell as the box reaches it.
function windowsSide(distro, home) {
  const ps = execFileSync("/usr/bin/wslpath", ["-u", "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"], { encoding: "utf8" }).trim();
  const literal = text => "'" + text.replaceAll("'", "''") + "'";
  const encode = text => Buffer.from(text, "utf16le").toString("base64");
  const flags = "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand";
  const windows = script => execFileSync(ps, [...flags.split(" "), encode("$ErrorActionPreference = 'Stop';\n" + script)],
    { encoding: "utf8", timeout: 15000 }).trim();
  const user = windows("[Security.Principal.WindowsIdentity]::GetCurrent().User.Value");
  const task = "KuvoCode-WSL-" + digest(JSON.stringify([user, distro, home])).slice(0, 32);
  const action = keeper => flags + " " + encode(`& "$env:SystemRoot\\System32\\wsl.exe" -d ${literal(distro)} --cd ~ --exec /usr/bin/setsid --wait /usr/bin/env ${literal("HOME=" + path.dirname(home))} /bin/sh ${literal(path.join(home, "bin", keeper))} wsl-keep\nexit $LASTEXITCODE`);
  const name = literal(task);
  const details = () => JSON.parse(windows(`$task=Get-ScheduledTask -TaskName ${name} -ErrorAction SilentlyContinue
if (!$task) { 'null' } else { [pscustomobject]@{action=$task.Actions[0].Arguments;user=$task.Principal.UserId;trigger=$task.Triggers[0].UserId;hidden=$task.Settings.Hidden} | ConvertTo-Json -Compress }`));
  const remove = () => windows(`$task=Get-ScheduledTask -TaskName ${name} -ErrorAction SilentlyContinue
if ($task) { $task | Unregister-ScheduledTask -Confirm:$false }`);
  return { windows, user, task, name, literal, action, details, remove };
}

const onWsl = process.platform === "linux" && !!process.env.WSL_DISTRO_NAME;

test("a WSL login task keeps its account and trigger but starts the saved keeper", {
  timeout: 60000, skip: !onWsl || boxBinary().typescript,
}, async t => {
  const { home, bin, repairing } = fixture(t, { wsl: true });
  const side = windowsSide(process.env.WSL_DISTRO_NAME, home);
  // The disposable task has precisely the action written by the released TS
  // registration. No task is started until the repair has updated that action.
  side.windows(`
$name=${side.name}
if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) { throw 'Fixture task already exists' }
$program=Join-Path $env:SystemRoot 'System32\\WindowsPowerShell\\v1.0\\powershell.exe'
$action=New-ScheduledTaskAction -Execute $program -Argument ${side.literal(side.action("kuvo-start"))}
$trigger=New-ScheduledTaskTrigger -AtLogOn -User ${side.literal(side.user)}
$principal=New-ScheduledTaskPrincipal -UserId ${side.literal(side.user)} -LogonType Interactive -RunLevel Limited
$settings=New-ScheduledTaskSettingsSet -Hidden -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Principal $principal -Settings $settings | Out-Null
`);
  t.after(side.remove);
  write(path.join(home, "run/wsl-task"), side.task + "\n");
  const before = side.details();
  write(path.join(bin, "kuvo-start"), "old Mac's keeper");
  assert.equal((await repairing()).result, "repaired");
  assert.deepEqual(side.details(), { ...before, action: side.action(".kuvo-start-keeper") });
  // A further overwrite cannot stop the already registered login action from
  // reaching the saved script. This starts only the disposable fixture action.
  write(path.join(bin, "kuvo-start"), "#!/bin/sh\nexit 1\n");
  side.windows(`Start-ScheduledTask -TaskName ${side.name}`);
  await until(() => fs.existsSync(path.join(home, "run/wsl-started")), "the Windows task did not invoke the saved WSL launcher", 15000);
});

test("WSL 2 starts at Windows sign-in by its own task, which uninstall takes away", {
  timeout: 60000, skip: !onWsl,
}, async t => {
  const binary = boxBinary();
  const { home, launchEnv, command, runArgs } = fixture(t, { wsl: true });
  const side = windowsSide(process.env.WSL_DISTRO_NAME, home);
  assert.equal(side.details(), null);
  t.after(side.remove);
  write(path.join(home, "run/launch.env"), "PATH=/usr/bin:/mnt/c/Windows/System32:/bin\nOPENCODE_CLIENT=desktop\n");
  const run = (...args) => spawnSync(command, runArgs(args), { cwd: home, env: launchEnv, encoding: "utf8", timeout: 45000 });
  const other = run("box", "wsl-startup", "Another-Distro");
  assert.equal(other.status, 1, other.stdout + other.stderr);
  assert.equal(side.details(), null);
  const registered = run("box", "wsl-startup", process.env.WSL_DISTRO_NAME);
  assert.equal(registered.status, 0, registered.stdout + registered.stderr);
  assert.equal(fs.readFileSync(path.join(home, "run/wsl-task"), "utf8"), side.task + "\n");
  const keeper = binary.typescript ? "kuvo-start" : ".kuvo-start-keeper";
  const task = side.details();
  assert.equal(task.action, side.action(keeper));
  assert.equal(task.hidden, true);
  assert.ok(task.user && task.trigger, JSON.stringify(task));
  // Registered, it starts at once: Windows runs the keeper inside this distribution.
  const marker = path.join(home, "run", binary.typescript ? "wsl-started-ordinary" : "wsl-started");
  await until(() => fs.existsSync(marker), "Windows did not start the keeper", 20000);
  assert.equal(fs.readFileSync(path.join(home, "run/launch.env"), "utf8"), "PATH=/usr/bin:/bin\nOPENCODE_CLIENT=desktop\n");
  const check = run("uninstall", "--check");
  assert.equal(check.status, 0, check.stdout + check.stderr);
  assert.equal(JSON.parse(check.stdout).starts.windowsTask, true, check.stdout);
  const removed = run("uninstall");
  assert.equal(removed.status, 0, removed.stdout + removed.stderr);
  assert.equal(side.details(), null);
  assert.equal(fs.existsSync(path.join(home, "run/wsl-task")), false);
});

// Explicit side-by-side proof: the selected release layout differs, but the
// same overwritten tool bytes and repair transcript agree at the binary boundary.
test("both boxes give the same repair outcome for the same overwritten tools", {
  timeout: 60000, skip: !process.env.KUVO_BOX_BIN_A || !process.env.KUVO_BOX_BIN_B,
}, t => {
  const results = [process.env.KUVO_BOX_BIN_A, process.env.KUVO_BOX_BIN_B].map(file => {
    const { home, bin, repair } = fixture(t, { binary: boxBinary(file) });
    const launch = path.join(bin, "kuvo-launch");
    write(launch, "the visiting Mac's launcher");
    write(path.join(home, "run/mac-installing"), "");
    const busy = repair();
    fs.rmSync(path.join(home, "run/mac-installing"));
    const repaired = repair(), sound = repair();
    return { busy, repaired, sound, contents: fs.readFileSync(launch, "utf8") };
  });
  assert.deepEqual(results[0], results[1]);
});

test("the released 0.7 Mac leaves a machine running after authenticated activity", { timeout: 30000 }, async t => {
  const root = scratch(), env = isolatedEnv(root), home = path.join(env.HOME, ".kuvo-code");
  privateHome(home);
  const relay = await localRelay(), binary = boxBinary();
  const proc = startBox({ binary, home, env, relay });
  t.after(async () => { await proc.stop(); await relay.close(); fs.rmSync(root, { recursive: true, force: true }); });
  const seen = path.join(home, "run/link.seen");
  let leavesRunning;
  if (process.platform !== "win32") {
    // Execute just the released stopNow guard (never its actual stop command).
    let script;
    await withTag(tags[0], ({tree}) => {
      const swift = fs.readFileSync(path.join(tree, "Packages/OpenCodeKit/Sources/OpenCodeKit/Engine/RemoteEngine.swift"), "utf8");
      const waiting = swift.match(/static func waiting\(in folder: String\) -> String \{\s*"([^"]+)"/)[1];
      const marker = swift.split("\n").find(line => line.includes("run/link.seen -mtime -7") && line.includes("&& exit 0"));
      const rootText = '"$HOME/.kuvo-code"';
      script = waiting.replaceAll("\\(folder)", rootText) + " && exit 0\n" +
        marker.trim().slice(3, -1).replaceAll("\\(root)", rootText).replaceAll("\\\"", '"').replaceAll("\\n", "\n") + "exit 1\n";
    });
    leavesRunning = () => spawnSync("sh", ["-c", script], { env, cwd: home }).status === 0;
  } else {
    // Native Windows has the same activity file; the released SSH quit guard is Unix shell only.
    leavesRunning = () => fs.existsSync(seen) && Date.now() - fs.statSync(seen).mtimeMs < 7 * 24 * 3600_000;
  }
  const server = await proc.ready();
  assert.equal((await request(server, "GET", "kuvo/info", undefined, { "x-kuvo-from": "app" })).status, 200);
  assert.equal(fs.existsSync(seen), false);
  assert.equal(leavesRunning(), false);
  assert.equal((await request(server, "GET", "kuvo/setup")).status, 200);
  assert.equal(fs.readFileSync(seen).length, 0);
  assert.equal(leavesRunning(), true);
  const before = fs.statSync(seen).mtimeMs;
  assert.equal((await request(server, "GET", "kuvo/setup")).status, 200);
  assert.equal(fs.statSync(seen).mtimeMs, before);
  const expired = new Date(Date.now() - 8 * 24 * 3600_000);
  fs.utimesSync(seen, expired, expired);
  assert.equal(leavesRunning(), false);
  if (process.platform !== "win32") {
    write(path.join(home, "wakes/ses_fixture/wakes/wake"), "waiting");
    assert.equal(leavesRunning(), true);
  }
});
