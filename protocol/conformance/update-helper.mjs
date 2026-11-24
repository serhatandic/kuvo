// A disposable Helper and launchctl exercise the keeper's process handover.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { boxBinary, isolatedEnv, launch, readJSON, request, scratch, startBox, until, write } from "./adapter.mjs";

const read = file => { try { return fs.readFileSync(file, "utf8"); } catch { return ""; } };
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const pathOf = relative => fileURLToPath(new URL(relative, import.meta.url));

function fixture(t, binary, mode) {
  const root = fs.realpathSync(scratch());
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const tools = path.join(env.HOME, "helper-tools");
  const bundle = path.join(home, "bin/Kuvo Helper.app");
  const helper = path.join(bundle, "Contents/MacOS/Kuvo Helper");
  const log = path.join(env.HOME, "helpers.log");
  const operations = path.join(env.HOME, "launchctl.log");
  const pidFile = path.join(env.HOME, "helper.pid");
  const run = path.join(home, "run");
  const record = path.join(run, "helper-handover.json");
  const bundleApp = "run.kuvo.code.fixture";
  const job = path.join(env.HOME, "Library/LaunchAgents/run.kuvo.code.machine.plist");
  Object.assign(env, { KUVO_HOME: home, KUVO_TEST_UNINSTALL_TOOLS: tools, KUVO_TEST_CODE_EVERY: "35", KUVO_FIXTURE_MODE: mode });
  write(path.join(home, "box/kind"), "mac\n");
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "box.json"), '{"version":"0.8.0","launch":"rust"}\n');
  write(path.join(home, "bin/kuvo-box"), fs.readFileSync(binary.file), { mode: 0o700 });
  const starter = fs.readFileSync(pathOf("../../box/src/keeper/kuvo-start"), "utf8")
    .replaceAll("@ENGINE@", "2.2.6");
  const keeper = path.join(home, "bin/kuvo-start");
  write(keeper, starter, { mode: 0o700 });
  write(path.join(run, "link.json"), '{"phones":[],"name":"Scratch Mac"}\n');
  write(job, `<plist><dict><key>ProgramArguments</key><array><string>${helper}</string></array><key>AssociatedBundleIdentifiers</key><array><string>${bundleApp}</string></array></dict></plist>\n`);
  const startTool = path.join(tools, "start-helper");
  write(startTool, `#!/bin/sh
unset KUVO_FIXTURE_OLD
if [ "$KUVO_FIXTURE_MODE" = still-old ] && [ ! -f "$HOME/start-new" ]; then export KUVO_FIXTURE_OLD=1; fi
exec "$KUVO_HOME/bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper" keep --home "$KUVO_HOME"
`, { mode: 0o700 });
  write(path.join(tools, "launchctl"), `#!/bin/sh
echo "$1" >>"$HOME/launchctl.log"
case "$1" in
  print)
    p=$(cat "$HOME/helper.pid" 2>/dev/null)
    [ -n "$p" ] && kill -0 "$p" 2>/dev/null && printf 'state = running\\npid = %s\\n' "$p"
    exit 0 ;;
  print-disabled) exit 0 ;;
  kickstart)
    [ "$KUVO_FIXTURE_MODE" != refused ] || exit 5
    "$KUVO_TEST_UNINSTALL_TOOLS/start-helper" </dev/null >/dev/null 2>&1 & ;;
  *) exit 1 ;;
esac
`, { mode: 0o700 });
  write(path.join(tools, "open"), `#!/bin/sh
echo "$#" >"$HOME/opened"
[ "$1 $2 $3 $5 $6 $7" = "-g -j -n --args keep --home" ] || exit 2
"$KUVO_TEST_UNINSTALL_TOOLS/start-helper" </dev/null >/dev/null 2>&1 &
`, { mode: 0o700 });

  function replace(renews) {
    // Replace the entry instead of changing the bytes of the already running Helper.
    const fresh = bundle + ".new";
    write(path.join(fresh, "Contents/Info.plist"), `<plist><dict>${renews ? "<key>KuvoHelperRenews</key><true/>" : ""}</dict></plist>`);
    write(path.join(fresh, "Contents/_CodeSignature/CodeResources"), renews ? "future" : "original");
    write(path.join(fresh, "Contents/MacOS/Kuvo Helper"), `#!${process.execPath}
const fs = require("node:fs");
const cp = require("node:child_process");
const renews = ${renews} && !process.env.KUVO_FIXTURE_OLD;
fs.appendFileSync(${JSON.stringify(log)}, process.pid + " " + Number(renews) + "\\n");
fs.writeFileSync(${JSON.stringify(pidFile)}, String(process.pid));
let serve, stopping = false;
function start() {
  if (stopping) return;
  serve = cp.spawn("/bin/sh", [${JSON.stringify(keeper)}, "serve", "--resident"], { stdio:["pipe","ignore","ignore"] });
  serve.stdin.on("error", () => {});
  serve.stdin.write(JSON.stringify({ password:"fixture-helper", renews }) + "\\n");
  const child = serve;
  serve.once("exit", () => {
    const record = ${JSON.stringify(path.join(run, "serve.json"))};
    try {
      if (JSON.parse(fs.readFileSync(record, "utf8")).pid === child.pid) fs.rmSync(record);
    } catch {}
    setTimeout(start, 100);
  });
}
process.on("SIGUSR1", () => serve.kill("SIGTERM"));
process.on("SIGTERM", () => {
  stopping = true;
  if (process.env.KUVO_FIXTURE_MODE === "stop-during") fs.writeFileSync(${JSON.stringify(path.join(run, "paused"))}, "");
  if (serve.exitCode !== null || serve.signalCode !== null) return process.exit(0);
  serve.once("exit", () => process.exit(0));
  serve.stdin.end();
});
start();
setInterval(() => {}, 60000);
`, { mode: 0o700 });
    fs.rmSync(bundle, { recursive: true, force: true });
    fs.renameSync(fresh, bundle);
  }

  const starts = () => read(log).trim().split("\n").filter(Boolean).map(line => line.split(" ").map(Number));
  const old = (() => {
    replace(mode === "self-renews");
    return launch(helper, ["keep", "--home", home], env, home);
  })();
  const turn = launch(process.execPath, ["-e", "process.stdin.pipe(process.stdout)"], env, home, "pipe");
  t.after(async () => {
    await old.stop();
    for (const [pid] of starts()) {
      if (pid !== old.child.pid && alive(pid)) {
        process.kill(pid, "SIGTERM");
        await until(() => !alive(pid), "the fixture Helper must exit", 5000);
      }
    }
    await turn.stop();
    fs.rmSync(root, { recursive: true, force: true });
  });
  const resident = () => {
    try { return readJSON(path.join(run, "serve.json")).pid; } catch { return undefined; }
  };
  return {
    home, env, old, run, resident, starts, turn, replace,
    handover: () => { try { return readJSON(record); } catch { return undefined; } },
    kickstarts: () => read(operations).split("\n").filter(line => line === "kickstart").length,
    opened: () => fs.existsSync(path.join(env.HOME, "opened")),
  };
}

test("the Mac keeper hands over a disposable Helper only when its release is settled", { timeout: 90000 }, async t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "darwin") return t.skip("The handover fixture runs the Rust Mac resident.");
  for (const mode of ["normal", "deferred", "refused", "still-old", "stop-before", "stop-during", "self-renews"]) {
    await t.test(mode, async t => {
      const box = fixture(t, binary, mode);
      await until(box.resident, "the original Helper should start Rust", 15000);
      let updater;
      if (mode === "deferred") {
        updater = launch(process.execPath, ["-e", "process.stdin.resume()", "kuvo-link-updating"], box.env, box.home);
        t.after(() => updater.stop());
        write(path.join(box.run, ".updating/pid"), String(updater.child.pid));
      }
      if (mode === "stop-before") write(path.join(box.run, "paused"), "");
      const serving = box.resident();
      box.replace(true);
      box.old.child.kill("SIGUSR1");
      await until(() => box.resident() !== serving, "the old resident must exit");
      if (mode === "stop-before" || mode === "self-renews" || mode === "deferred") {
        if (mode !== "stop-before") await until(box.resident, "the original Helper must start the resident again");
        assert.equal(box.handover(), undefined);
        assert.equal(box.kickstarts(), 0);
        assert.equal(box.old.child.exitCode, null);
        if (!updater) return;
        fs.rmSync(path.join(box.run, ".updating"), { recursive: true });
        await updater.stop();
      }
      const outcome = mode === "stop-during" ? "stopped" : "handed";
      await until(() => box.handover()?.outcome === outcome,
        () => `expected ${outcome}, got ${JSON.stringify(box.handover())}`, 15000);
      await box.old.exited;
      assert.equal(box.turn.child.exitCode, null);
      assert.equal(box.turn.child.signalCode, null);
      assert.equal(box.kickstarts(), mode === "stop-during" ? 0 : 1);
      if (mode === "stop-during") {
        assert.equal(box.starts().length, 1);
        return;
      }
      await until(() => box.starts().length === 2 && box.resident(), "the replacement Helper should answer");
      assert.equal(box.opened(), mode === "refused");
      assert.equal(box.starts()[1][1], mode === "still-old" ? 0 : 1);
      if (mode === "still-old") {
        await until(() => box.handover()?.stillOld?.pid === box.starts()[1][0], "record the Helper that is still old");
        const heard = box.handover().stillOld;
        const resident = box.resident();
        process.kill(heard.pid, "SIGUSR1");
        await until(() => box.resident() && box.resident() !== resident, "the old Helper can restart its resident");
        assert.deepEqual(box.handover().stillOld, heard);
        assert.equal(box.kickstarts(), 1);
        const previous = box.handover().copy;
        write(path.join(box.env.HOME, "start-new"), "");
        box.replace(true);
        process.kill(heard.pid, "SIGUSR1");
        await until(() => box.starts().length === 3 && box.handover()?.outcome === "handed",
          "taking the same release again gives the new copy one handover");
        assert.notEqual(box.handover().copy, previous);
        assert.equal(box.starts()[2][1], 1);
        assert.equal(box.kickstarts(), 2);
      }
    });
  }
});

test("the Mac Helper can ask the local resident to pause its keeper", async t => {
  const binary = boxBinary();
  if (process.platform !== "darwin") return t.skip("Only the Mac Helper can pause Kuvo this way.");
  const root = fs.realpathSync(scratch());
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const asked = path.join(env.HOME, "keeper-asked");
  write(path.join(home, "box/kind"), "mac\n");
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "bin/kuvo-start"), `#!/bin/sh\nprintf '%s\\n' "$1" "$HOME" >"$HOME/keeper-asked"\n`, { mode: 0o700 });
  const resident = startBox({ binary, home, env, relay: { url: "ws://127.0.0.1:1" }, mac: true });
  t.after(async () => {
    await resident.stop();
    fs.rmSync(root, { recursive: true, force: true });
  });
  const ready = await resident.ready();
  const result = await request(ready, "POST", "kuvo/box/stop", {});
  assert.equal(result.status, 202, result.text);
  assert.deepEqual(result.json, { stopping: true });
  await until(() => fs.existsSync(asked), "the keeper must receive pause");
  assert.equal(read(asked), `pause\n${env.HOME}\n`);
});

test("uninstall removes only the stopped Mac's own Helper login job", t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "darwin") return t.skip("The fixture runs the Rust Mac installer.");
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const tools = path.join(env.HOME, "tools");
  const jobs = path.join(env.HOME, "Library/LaunchAgents");
  const plist = path.join(jobs, "run.kuvo.code.machine.plist");
  const other = path.join(jobs, "some-other-job.plist");
  const notes = path.join(home, "workspace/notes");
  Object.assign(env, { KUVO_HOME: home, KUVO_TEST_UNINSTALL_TOOLS: tools });
  write(path.join(home, "box/kind"), "mac\n");
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "bin/kuvo-start"), "#!/bin/sh\nexit 1\n", { mode: 0o700 });
  write(path.join(home, "bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper"), "#!/bin/sh\n", { mode: 0o700 });
  write(path.join(home, "run/paused"), "");
  write(path.join(home, "desktop/identity"), "local identity");
  write(notes, "saved work");
  write(other, "other job");
  write(path.join(tools, "launchctl"), `#!/bin/sh
tools="$KUVO_TEST_UNINSTALL_TOOLS"
[ "$1" != print ] || [ "$2" != "gui/$(/usr/bin/id -u)/run.kuvo.code.machine.permissions" ] || exit 113
[ "$2" = "gui/$(/usr/bin/id -u)/run.kuvo.code.machine" ] || exit 2
case "$1" in
  print) [ -f "$tools/registered" ] || exit 113 ;;
  bootout) [ ! -f "$tools/refuse" ] || exit 1; rm "$tools/registered" ;;
  *) exit 2 ;;
esac
`, { mode: 0o700 });
  const call = (...args) => spawnSync(binary.file, args, { env, cwd: home, encoding: "utf8", timeout: 15000 });
  const job = call("helper", "install");
  assert.equal(job.status, 0, job.stderr);
  const original = read(plist);
  write(path.join(tools, "registered"), "");
  const profileEntries = fs.readdirSync(env.HOME).sort();
  const check = call("uninstall", "--check");
  assert.equal(check.status, 0, check.stderr);
  assert.deepEqual(JSON.parse(check.stdout).starts, { launchAgent: true, crontab: false, checked: true });

  write(plist, original.replace("/bin/Kuvo Helper.app", "/elsewhere/Kuvo Helper.app"));
  assert.equal(call("uninstall").status, 1);
  assert.equal(fs.existsSync(path.join(tools, "registered")), true);
  assert.equal(read(path.join(home, "desktop/identity")), "local identity");
  write(plist, original);
  write(path.join(tools, "refuse"), "");
  assert.equal(call("uninstall").status, 1);
  assert.equal(read(plist), original);
  fs.rmSync(path.join(tools, "refuse"));
  const removed = call("uninstall");
  assert.equal(removed.status, 0, removed.stderr);
  assert.equal(JSON.parse(removed.stdout).launchAgent, true);
  assert.deepEqual(JSON.parse(removed.stdout).startsAgain, []);
  assert.equal(fs.existsSync(plist), false);
  assert.equal(fs.existsSync(path.join(tools, "registered")), false);
  assert.equal(fs.existsSync(path.join(home, "desktop/identity")), false);
  assert.equal(read(other), "other job");
  assert.equal(read(notes), "saved work");
  assert.deepEqual(fs.readdirSync(env.HOME).sort(), profileEntries);
  assert.deepEqual(fs.readdirSync(env.TMPDIR), []);
  const again = call("uninstall");
  assert.equal(again.status, 0, again.stderr);
  assert.equal(JSON.parse(again.stdout).launchAgent, false);
});

test("uninstall removes only Kuvo's tagged reboot line from a stopped Unix home", t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform === "win32") return t.skip("The fixture runs the Rust Unix installer.");
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const tools = path.join(env.HOME, "tools");
  const table = path.join(tools, "table");
  const other = "PATH=/usr/bin:/bin\n\n@reboot another-program\n0 12 * * * backup\n";
  const original = other + '@reboot sh "$HOME/.kuvo-code/bin/kuvo-start" >/dev/null 2>&1 # kuvo-code: starts its engine again after a restart\n';
  Object.assign(env, { KUVO_HOME: home, KUVO_TEST_UNINSTALL_TOOLS: tools });
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "bin/kuvo-start"), "#!/bin/sh\nexit 1\n", { mode: 0o700 });
  write(path.join(home, "run/link.json"), '{"phones":[]}\n');
  write(path.join(home, "workspace/notes"), "saved work");
  write(table, original);
  write(path.join(tools, "crontab"), `#!/bin/sh
case "$1" in
  -l) [ ! -f "$KUVO_TEST_UNINSTALL_TOOLS/deny" ] || exit 1
      cat "$KUVO_TEST_UNINSTALL_TOOLS/table" ;;
  -) cat >"$KUVO_TEST_UNINSTALL_TOOLS/table" ;;
  *) exit 2 ;;
esac
`, { mode: 0o700 });
  if (process.platform === "darwin") {
    write(path.join(tools, "launchctl"), "#!/bin/sh\n[ \"$1\" != print ] || exit 113\nexit 2\n", { mode: 0o700 });
  }
  const call = (...args) => spawnSync(binary.file, ["uninstall", ...args], { env, cwd: home, encoding: "utf8", timeout: 20000 });
  const check = call("--check");
  assert.equal(check.status, 0, check.stderr);
  assert.deepEqual(JSON.parse(check.stdout).starts, { launchAgent: false, crontab: true, checked: true });
  write(path.join(tools, "deny"), "");
  assert.equal(call().status, 1);
  assert.equal(read(table), original);
  assert.equal(fs.existsSync(path.join(home, "run/off")), false);
  fs.rmSync(path.join(tools, "deny"));
  const removed = call();
  assert.equal(removed.status, 0, removed.stderr);
  assert.equal(JSON.parse(removed.stdout).crontab, true);
  assert.equal(read(table), other);
  assert.equal(read(path.join(home, "workspace/notes")), "saved work");
  assert.deepEqual(fs.readdirSync(env.TMPDIR), []);
  assert.equal(JSON.parse(call().stdout).crontab, false);
});

test("the Mac Helper install writes a launchd job and can reload it once the old job goes", t => {
  const binary = boxBinary();
  if (binary.typescript || process.platform !== "darwin") return t.skip("The fixture runs the Rust Mac Helper command.");
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = isolatedEnv(root);
  env.HOME = path.join(root, "account & <copy>");
  const home = path.join(env.HOME, ".kuvo-code");
  const tools = path.join(env.HOME, "tools");
  const plist = path.join(env.HOME, "Library/LaunchAgents/run.kuvo.code.machine.plist");
  const log = path.join(tools, "calls");
  Object.assign(env, { KUVO_HOME: home, KUVO_TEST_UNINSTALL_TOOLS: tools });
  write(path.join(home, "box/kind"), "mac\n");
  write(path.join(home, "bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper"), "#!/bin/sh\n", { mode: 0o700 });
  write(path.join(tools, "launchctl"), `#!/bin/sh
echo "$1" >>"$KUVO_TEST_UNINSTALL_TOOLS/calls"
tools="$KUVO_TEST_UNINSTALL_TOOLS"
case "$1" in
  bootout) touch "$tools/old" ;;
  print)
    if [ -e "$tools/old" ]; then rm "$tools/old"; exit 0; fi
    if [ -e "$tools/ending" ]; then
      rm "$tools/ending"
      printf 'state = running\\npid = 12345\\n'
      exit 0
    fi
    [ -e "$tools/loaded" ] ;;
  bootstrap)
    if [ ! -e "$tools/refused" ]; then touch "$tools/refused"; exit 5; fi
    touch "$tools/loaded" ;;
  kickstart) [ -e "$tools/loaded" ] && [ ! -e "$tools/disabled" ] ;;
  print-disabled) printf '"run.kuvo.code.machine" => disabled\\n' ;;
  *) exit 1 ;;
esac
`, { mode: 0o700 });
  write(path.join(tools, "open"), `#!/bin/sh
[ "$1 $2 $3 $5 $6 $7" = "-g -j -n --args keep --home" ] && [ "$8" = "$KUVO_HOME" ]
`, { mode: 0o700 });
  const call = (...words) => spawnSync(binary.file, ["helper", ...words], { env, cwd: home, encoding: "utf8", timeout: 15000 });
  const wrote = call("install", "--app", "run.kuvo.code.fixture");
  assert.equal(wrote.status, 0, wrote.stderr);
  assert.equal(fs.existsSync(log), false);
  assert.deepEqual(JSON.parse(wrote.stdout), {
    written: true, loaded: false, message: "Kuvo Helper starts Kuvo on this Mac at login.",
  });
  const extract = key => {
    const value = spawnSync("/usr/bin/plutil", ["-extract", key, "raw", "-o", "-", plist], { encoding: "utf8" });
    assert.equal(value.status, 0, value.stderr);
    return value.stdout.trim();
  };
  assert.equal(extract("Label"), "run.kuvo.code.machine");
  assert.equal(extract("ProgramArguments.0"), path.join(home, "bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper"));
  assert.equal(extract("ProgramArguments.3"), home);
  assert.equal(extract("AssociatedBundleIdentifiers.0"), "run.kuvo.code.fixture");
  assert.equal(extract("RunAtLoad"), "true");
  assert.equal(extract("KeepAlive.SuccessfulExit"), "false");
  assert.equal(extract("AbandonProcessGroup"), "true");
  assert.deepEqual(JSON.parse(call("status").stdout), {
    helper: true, agent: true, loaded: false, running: false, handOver: null,
  });
  const loaded = call("install", "--load");
  assert.equal(loaded.status, 0, loaded.stderr);
  assert.equal(JSON.parse(loaded.stdout).loaded, true);
  assert.equal(read(log), "print\nbootout\nprint\nprint\nbootstrap\nbootstrap\n");
  assert.deepEqual(JSON.parse(call("status").stdout), {
    helper: true, agent: true, loaded: true, running: null, handOver: null,
  });
  assert.equal(JSON.parse(call("start").stdout).started, true);
  assert.equal(JSON.parse(call("start", "--if-missing").stdout).left, true);
  write(path.join(home, "run/paused"), "");
  write(path.join(tools, "ending"), "");
  assert.equal(JSON.parse(call("start").stdout).started, true);
  assert.match(read(log), /print\nprint\nkickstart\n$/);
  fs.rmSync(path.join(home, "run/paused"));
  write(path.join(tools, "disabled"), "");
  const opened = call("start");
  assert.equal(opened.status, 0, opened.stderr);
  assert.equal(JSON.parse(opened.stdout).disabled, true);
  assert.equal(JSON.parse(opened.stdout).opened, true);
  for (const args of [[], ["remove"], ["install", "--now"], ["install", "--app"],
    ["install", "--app", "not an id"], ["status", "--load"], ["start", "--load"]]) {
    assert.equal(call(...args).status, 2, args.join(" "));
  }
  fs.rmSync(plist);
  write(path.join(home, "box/kind"), "machine\n");
  assert.equal(call("install").status, 1);
  assert.equal(fs.existsSync(plist), false);
  write(path.join(home, "box/kind"), "mac\n");
  delete env.KUVO_TEST_UNINSTALL_TOOLS;
  assert.match(JSON.parse(call("install").stdout).message, /Only Kuvo in ~\/\.kuvo-code starts at login/);
  env.KUVO_TEST_UNINSTALL_TOOLS = tools;
  fs.rmSync(path.join(home, "bin/Kuvo Helper.app"), { recursive: true });
  assert.match(JSON.parse(call("install").stdout).message, /isn't in Kuvo's bin folder/);
  assert.equal(fs.existsSync(plist), false);
});

test("a real disposable Swift Helper renews its own signed bundle", { timeout: 60000 }, async t => {
  const binary = boxBinary();
  const swift = process.env.KUVO_UPDATE_HELPER_BIN;
  if (process.platform !== "darwin" || binary.typescript || !swift) {
    return t.skip("Set KUVO_UPDATE_HELPER_BIN to the debug Mac Helper.");
  }
  const root = fs.realpathSync(scratch());
  const env = isolatedEnv(root);
  const home = path.join(env.HOME, ".kuvo-code");
  const run = path.join(home, "run");
  const tools = path.join(env.HOME, "tools");
  const keychain = path.join(root, "fixture.keychain-db");
  const password = "temporary-keychain-fixture";
  const command = (exe, args) => execFileSync(exe, args, { env, stdio: "ignore" });
  command("/usr/bin/security", ["create-keychain", "-p", password, keychain]);
  let guard;
  t.after(async () => {
    await guard?.stop();
    command("/usr/bin/security", ["delete-keychain", keychain]);
    fs.rmSync(root, { recursive: true, force: true });
  });
  Object.assign(env, { KUVO_HOME: home, KUVO_TEST_UNINSTALL_TOOLS: tools });
  write(path.join(tools, "launchctl"), "#!/bin/sh\ntouch \"$HOME/launchd-contacted\"\nexit 1\n", { mode: 0o700 });
  write(path.join(tools, "caffeinate"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
  env.PATH = tools + path.delimiter + env.PATH;
  write(path.join(home, "box/kind"), "mac\n");
  write(path.join(home, "box.json"), '{"launch":"rust"}\n');
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(run, "mac-link.json"), '{"phones":false,"relay":"ws://127.0.0.1:9"}\n');
  write(path.join(run, "launch.env"), "KUVO_TEST_RELAY=ws://127.0.0.1:9\nKUVO_TEST_NO_PEERS=1\nKUVO_SANDBOX=off\n");
  write(path.join(home, "bin/kuvo-box"), fs.readFileSync(binary.file), { mode: 0o700 });
  write(path.join(home, "bin/kuvo-start"), fs.readFileSync(pathOf("../../box/src/keeper/kuvo-start"), "utf8")
    .replaceAll("@ENGINE@", "2.2.6"), { mode: 0o700 });
  write(path.join(home, "bin/kuvo-setsid"), fs.readFileSync(pathOf("../../box/src/keeper/kuvo-setsid")), { mode: 0o700 });

  const bundle = path.join(home, "bin/Kuvo Helper.app");
  const executable = "Contents/MacOS/Kuvo Helper";
  const replacement = path.join(root, "replacement", "Kuvo Helper.app");
  function signed(folder, version) {
    const id = "run.kuvo.code.updatetrial.box";
    write(path.join(folder, executable), fs.readFileSync(swift), { mode: 0o700 });
    write(path.join(folder, "Contents/Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>${id}</string>
<key>CFBundleExecutable</key><string>Kuvo Helper</string>
<key>CFBundleName</key><string>Kuvo Helper</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>${version}</string>
<key>KuvoHelperRenews</key><true/>
</dict></plist>
`);
    command("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", id, folder]);
  }
  const boot = () => launch(path.join(bundle, executable), [
    "keep", "--home", home, "--test-keychain", keychain, "--test-keychain-password", password,
    "--test-look-every", "0.2",
  ], env, home);
  const diagnostics = () => {
    const pid = (() => { try { return readJSON(path.join(run, "serve.json")).pid; } catch { return null; } })();
    return `Helper exit ${guard?.child.exitCode}, signal ${guard?.child.signalCode}, resident ${pid}\n${guard?.log()}\n${read(path.join(run, "helper-trouble"))}\n${read(path.join(run, "serve.log"))}`;
  };
  const wait = () => until(async () => {
    if (guard.child.exitCode !== null) throw new Error(diagnostics());
    const file = path.join(run, "serve.json");
    if (!fs.existsSync(file)) return;
    const serve = readJSON(file);
    const info = await request(serve, "GET", "kuvo/info").catch(() => null);
    return info?.status === 200 ? serve : undefined;
  }, diagnostics, 20000);
  signed(bundle, 1);
  signed(replacement, 2);
  guard = boot();
  await wait();
  const began = performance.now();
  fs.renameSync(bundle, bundle + ".before");
  fs.renameSync(replacement, bundle);
  const ended = await guard.exited;
  assert.equal(ended.code, 75, guard.log());
  assert.equal(fs.existsSync(path.join(run, "helper-handover.json")), false);
  assert.equal(fs.existsSync(path.join(env.HOME, "launchd-contacted")), false);
  t.diagnostic(`the real Helper left for its replacement in ${Math.round(performance.now() - began)} ms`);
});
