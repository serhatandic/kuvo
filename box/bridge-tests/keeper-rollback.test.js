// The box's keeper (`kuvo-start keep`) and what boxes from before left (PROTOCOL.md, "Updates"; "For
// released Macs"): a rollback kept by an update from before rollbacks were code only never puts the
// engine's data back over what was written since; and the watchdog a Mac with Kuvo Code 0.7 starts
// beside its engine never stops it, with nothing killed by a pattern. The real keeper script, in a
// home of the test's own, with stand-ins for what it would reach of this computer (`pkill`,
// `caffeinate`, `crontab`, `launchctl`); times set on the files, never waited for.
//
// Run with `node --test box/bridge-tests/keeper-rollback.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const keeper = fileURLToPath(new URL("../src/keeper/kuvo-start", import.meta.url));
const write = (file, text, mode = 0o600) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
};
const exists = file => fs.existsSync(file);
const aged = (file, ms) => fs.utimesSync(file, new Date(Date.now() - ms), new Date(Date.now() - ms));
async function until(check, what) {
  for (let i = 0; i < 600; i++) {
    if (check()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.fail(`never: ${what}`);
}

// A machine's box, its keeper's script as the release has it.
function home(kind = "machine") {
  const folder = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-keeper-rollback-")));
  removeAtEnd(folder);
  const K = path.join(folder, ".kuvo-code");
  write(path.join(K, "box", "kind"), `${kind}\n`);
  write(path.join(K, "bin", "kuvo-start"), fs.readFileSync(keeper, "utf8").replace("@ENGINE@", "1.0.0"), 0o700);
  write(path.join(K, "bin", "box-version"), "0.9.0");
  // (What it starts things in a session of their own with, where the system has no `setsid`.)
  write(path.join(K, "bin", "kuvo-setsid"), fs.readFileSync(fileURLToPath(new URL("../src/keeper/kuvo-setsid", import.meta.url)), "utf8"), 0o755);
  fs.mkdirSync(path.join(K, "run"), { recursive: true });
  const tools = path.join(folder, "tools");
  for (const tool of ["pkill", "caffeinate", "crontab", "launchctl"]) write(path.join(tools, tool), `#!/bin/sh\necho "${tool} $*" >>"${folder}/reached"\n`, 0o755);
  const env = { PATH: `${tools}:/usr/bin:/bin:/usr/sbin:/sbin`, HOME: folder };
  return { folder, K, R: path.join(K, "run"), env };
}

// Its keeper's loop, running until the test ends it (its group, which it started).
function keep(where) {
  const child = spawn("sh", [path.join(where.K, "bin", "kuvo-start"), "keep"], { env: where.env, detached: true, stdio: "ignore" });
  const gone = new Promise(done => child.once("exit", done));
  const stop = async () => {
    try { process.kill(-child.pid, "SIGKILL"); } catch {}
    await gone;
  };
  atEnd(() => { try { process.kill(-child.pid, "SIGKILL"); } catch {} });
  return stop;
}

test("a rollback an older update kept, with the engine's data in it, puts back code only: what was written since stays", async () => {
  const where = home();
  const { K, R } = where;
  // An update from before, on trial over twenty minutes, its rollback as that update wrote it: the
  // database it kept, and its restore.sh, which puts that back whenever it's there.
  write(path.join(K, "updates", "rollback", "data", "opencode.db"), "the data before the update");
  // The keeper it kept, which starts the engine from before (1.0.0); the engine that ran since is newer.
  write(path.join(K, "updates", "rollback", "files", "bin", "kuvo-start"), "#!/bin/sh\nV=1.0.0\ncase \"$1\" in\nkeep) ;;\nesac\n", 0o700);
  write(path.join(R, "engine.json"), JSON.stringify({ pid: 999999, port: 1, password: "x", version: "1.1.0" }));
  write(path.join(K, "updates", "rollback", "restore.sh"), [
    "#!/bin/sh",
    `K='${K}'; B='${K}/updates/rollback'`,
    `cp -p "$B/files/"'bin/kuvo-start' "$K/"'bin/kuvo-start'.new && mv -f "$K/"'bin/kuvo-start'.new "$K/"'bin/kuvo-start' || exit 1`,
    `D="$K/data/opencode"`,
    `if [ -f "$B/data/opencode.db" ]; then`,
    `  rm -f "$D/opencode.db-wal" "$D/opencode.db-shm" || exit 1`,
    `  for n in opencode.db opencode.db-wal opencode.db-shm; do if [ -f "$B/data/$n" ]; then mv -f "$B/data/$n" "$D/$n" || exit 1; fi; done`,
    `fi`,
    `rm -f "$K/run/update-trial" "$K/run/update-restore"`,
    `rm -rf "$B"`,
  ].join("\n") + "\n", 0o700);
  write(path.join(R, "update-trial"), "0.10.0");
  // What the new engine wrote since: a conversation.
  write(path.join(K, "data", "opencode", "opencode.db"), "a conversation after the update");
  write(path.join(K, "data", "opencode", "opencode.db-wal"), "its last message");

  // Judged unwell: put back as its judge does it (`kuvo-start rollback`).
  const back = spawnSync("sh", [path.join(K, "bin", "kuvo-start"), "rollback"], { env: where.env, encoding: "utf8" });
  assert.equal(back.status, 0, back.stderr);
  assert.equal(exists(path.join(R, "update-trial")), false, "the rollback ran");
  assert.equal(fs.readFileSync(path.join(K, "data", "opencode", "opencode.db"), "utf8"), "a conversation after the update");
  assert.equal(fs.readFileSync(path.join(K, "data", "opencode", "opencode.db-wal"), "utf8"), "its last message");
  // The copy from before, aside, where nothing puts it back.
  const kept = fs.readdirSync(path.join(K, "updates", "backups"));
  assert.equal(kept.length, 1);
  assert.match(kept[0], /^\d{4}-\d{2}-\d{2}-kept-for-a-rollback$/);
  assert.equal(fs.readFileSync(path.join(K, "updates", "backups", kept[0], "opencode.db"), "utf8"), "the data before the update");
  assert.equal(exists(path.join(K, "updates", "rollback")), false);
  // The keeper it put back starts the engine that ran (an older one never opens what it wrote).
  assert.match(fs.readFileSync(path.join(K, "bin", "kuvo-start"), "utf8"), /^V=1\.1\.0$/m);
});

test("an update on trial that nothing judges: the keeper starts a judge of its own, and puts the code back itself once three never got to look", async () => {
  const where = home();
  const { K, R, folder } = where;
  write(path.join(R, "update-trial"), "0.10.0");
  write(path.join(K, "updates", "rollback", "restore.sh"), `#!/bin/sh\ntouch '${folder}/restored'\nrm -f '${R}/update-trial'\n`, 0o700);
  // Its engine's program, which notes what the keeper's first round asks of the link.
  write(path.join(K, "bin", "kuvo-link.js"), "// the link\n");
  write(path.join(K, "engine", "1.0.0", "opencode"), `#!/bin/sh\necho "$4 $5" >>'${folder}/asked'\nexit 0\n`, 0o755);
  let stop = keep(where);
  await until(() => exists(path.join(folder, "asked")) && fs.readFileSync(path.join(folder, "asked"), "utf8").includes("update --settle"), "a judge started");
  await stop();
  assert.equal(exists(path.join(folder, "restored")), false, "not put back: a judge is to say");
  assert.equal(fs.readFileSync(path.join(R, "update-settle.tries"), "utf8").trim(), "1");

  // Three judges that never looked (the new link doesn't run at all): the keeper puts it back.
  write(path.join(R, "update-settle.tries"), "3");
  stop = keep(where);
  await until(() => exists(path.join(folder, "restored")) && !exists(path.join(R, "update-trial")), "the rollback finished");
  await stop();
  assert.equal(exists(path.join(R, "update-trial")), false);
  assert.equal(exists(path.join(R, "update-settle.tries")), false);
});

test("an update taken by a Mac's box from before: the server still on the old code is ended, for its helper to start the new one; nothing's taken back", { timeout: 60_000 }, async () => {
  const where = home("mac");
  const { K, R, folder, env } = where;
  write(path.join(K, "bin", "box-version"), "0.10.0");
  write(path.join(R, "update.json"), JSON.stringify({ trial: { from: "0.9.0", to: "0.10.0", at: Date.now(), healthy: true } }));
  write(path.join(R, "update-trial"), "0.10.0");
  write(path.join(K, "updates", "rollback", "restore.sh"), `#!/bin/sh\ntouch '${folder}/restored'\n`, 0o700);
  // Its server as its helper started it before the update, on the old link (which never says so).
  // (From the box's own link, as its helper runs it: the file is replaced under it by the update.)
  const old = path.join(K, "bin", "kuvo-link.js");
  const up = path.join(folder, "old-serve-up");
  write(old, `require("fs").writeFileSync(${JSON.stringify(up)}, "");\nsetInterval(() => {}, 1000);\n`);
  const server = spawn(process.execPath, [old, "serve"], { stdio: "ignore" });
  atEnd(() => { try { server.kill("SIGKILL"); } catch {} });
  const ended = new Promise(done => server.once("exit", (code, signal) => done(signal)));
  write(path.join(R, "serve.json"), JSON.stringify({ pid: server.pid, port: 1, password: "x", kept: false }));
  // (Running the old code before the update replaces the file under it.)
  await until(() => exists(up), "the old server");
  // The new link, put there after that server started, with the release's own time on it (a
  // release's files all say 2026-01-01, and a Mac's disk clones a big file with its time): only
  // when it was put there (its change time) is now.
  write(path.join(K, "bin", "kuvo-link.js"), fs.readFileSync(fileURLToPath(LINK_BUNDLE), "utf8"));
  const released = new Date("2026-01-01T00:00:00Z");
  fs.utimesSync(path.join(K, "bin", "kuvo-link.js"), released, released);

  const stop = keep(where);
  assert.equal(await ended, "SIGTERM", "the old server, ended");
  await stop();
  assert.equal(exists(path.join(folder, "restored")), false);
  assert.equal(exists(path.join(R, "update-trial")), true, "still on trial: its judge is to say");
});

test("the engine the keeper stops is no longer one a start looks for among those it began", async () => {
  const { K, R, env } = home();
  write(path.join(K, "engine", "1.0.0", "opencode"), "#!/bin/sh\nwhile :; do sleep 1; done\n", 0o755);
  // (Not this test's child, as the keeper's launch leaves it: gone the moment it's stopped.)
  const pid = Number(spawnSync("sh", ["-c", `sh '${path.join(K, "engine", "1.0.0", "opencode")}' serve --port 1 >/dev/null 2>&1 </dev/null & echo $!`], { encoding: "utf8" }).stdout.trim());
  atEnd(() => { try { process.kill(pid, "SIGKILL"); } catch {} });
  write(path.join(R, "engine.json"), `{"pid":${pid},"port":1,"password":"x"}`);
  write(path.join(R, "engines", String(pid)), "");
  const stopped = spawnSync("sh", [path.join(K, "bin", "kuvo-start"), "stop"], { env, encoding: "utf8", timeout: 60_000 });
  assert.equal(stopped.status, 0, stopped.stderr);
  assert.throws(() => process.kill(pid, 0), "the engine stopped");
  assert.equal(exists(path.join(R, "engines", String(pid))), false);
});

test("an older Mac's mark a day old is taken away, so the watchdog its start brought never stops the engine; its visit still counts", async () => {
  const where = home();
  const { K, R } = where;
  // The box marked two days ago; a Mac with Kuvo Code 0.7 connected since, 23½ hours ago (its
  // watchdog stops the engine once this mark is a day old).
  aged(path.join(K, "bin", "box-version"), 2 * 86_400_000);
  write(path.join(R, "attached"), "");
  aged(path.join(R, "attached"), 23.5 * 3_600_000);
  const visit = fs.statSync(path.join(R, "attached")).mtimeMs;

  const stop = keep(where);
  await until(() => !exists(path.join(R, "attached")), "the mark taken away");
  await stop();
  assert.equal(Math.round(fs.statSync(path.join(R, "older-mac")).mtimeMs / 1000), Math.round(visit / 1000), "the visit kept, when it was");

  // One from before the box was last marked (most likely the same Mac before it updated): taken
  // away, and not counted.
  const before = home();
  write(path.join(before.R, "attached"), "");
  aged(path.join(before.R, "attached"), 2 * 86_400_000);
  const stopBefore = keep(before);
  await until(() => !exists(path.join(before.R, "attached")), "the mark taken away");
  await stopBefore();
  assert.equal(exists(path.join(before.R, "older-mac")), false);
  // And nothing killed by a pattern.
  for (const each of [where, before]) assert.doesNotMatch(exists(path.join(each.folder, "reached")) ? fs.readFileSync(path.join(each.folder, "reached"), "utf8") : "", /pkill/);
});

test("a rollback that can't put it all back (a disk full) is tried three times, then gives up: the trial ends and the box starts as it is, never left stopped", async () => {
  const where = home();
  const { K, R, env, folder } = where;
  write(path.join(R, "update-trial"), "0.10.0");
  // An update from before's restore.sh: it stops the engine, then a copy fails.
  write(path.join(K, "updates", "rollback", "restore.sh"), `#!/bin/sh\ntouch '${R}/stopped'\necho tried >>'${folder}/tries'\nexit 1\n`, 0o700);
  const rollback = () => spawnSync("sh", [path.join(K, "bin", "kuvo-start"), "rollback"], { env, encoding: "utf8" }).status;
  for (let tries = 1; tries <= 3; tries++) {
    assert.notEqual(rollback(), 0, `try ${tries} fails`);
    assert.equal(exists(path.join(R, "update-trial")), true, "still on trial, to try again");
  }
  // (Off, so the start it gives up with starts no keeper that would outlive the test.)
  write(path.join(R, "off"), "");
  assert.equal(rollback(), 2, "it gives up");
  assert.equal(fs.readFileSync(path.join(folder, "tries"), "utf8"), "tried\ntried\ntried\n");
  assert.equal(exists(path.join(R, "update-trial")), false, "the trial ends");
  assert.equal(exists(path.join(R, "stopped")), false, "not left stopped");
  assert.equal(exists(path.join(R, "update-restore")), false);
  assert.equal(exists(path.join(R, ".rolling-back")), false, "its lock let go");
});

test("one rollback at a time: a second, while the first runs, leaves without touching anything", async () => {
  const where = home();
  const { K, R, env, folder } = where;
  write(path.join(R, "update-trial"), "0.10.0");
  write(path.join(K, "updates", "rollback", "restore.sh"), `#!/bin/sh\necho ran >>'${folder}/ran'\nrm -f '${R}/update-trial'\n`, 0o700);
  // The first, under way (a process running the keeper's script holds its lock).
  const first = spawn("sh", ["-c", "while :; do sleep 1; done", "kuvo-start"], { stdio: "ignore" });
  atEnd(() => { try { first.kill("SIGKILL"); } catch {} });
  fs.mkdirSync(path.join(R, ".rolling-back"));
  write(path.join(R, ".rolling-back", "pid"), String(first.pid));
  const rollback = () => spawnSync("sh", [path.join(K, "bin", "kuvo-start"), "rollback"], { env, encoding: "utf8" }).status;
  assert.equal(rollback(), 1, "the second leaves");
  assert.equal(exists(path.join(folder, "ran")), false, "nothing put back twice");
  assert.equal(exists(path.join(R, "update-restore")), false, "nor counted");
  assert.equal(fs.readFileSync(path.join(R, ".rolling-back", "pid"), "utf8").trim(), String(first.pid), "the first's lock as it was");
  // The first gone (cut off): its lock is taken over, and it's put back.
  first.kill("SIGKILL");
  await new Promise(resolve => first.once("exit", resolve));
  assert.equal(rollback(), 0);
  assert.equal(fs.readFileSync(path.join(folder, "ran"), "utf8"), "ran\n");
  assert.equal(exists(path.join(R, "update-trial")), false);
});


test("the current keeper holds an unknown update record before executing saved rollback", () => {
  const { folder, K, R, env } = home();
  const link = fs.readFileSync(LINK_BUNDLE);
  write(path.join(K, "bin", "kuvo-link.js"), link);
  write(path.join(K, "bin", "package.json"), '{"type":"module"}');
  write(path.join(K, "engine", "1.0.0", "opencode"), `#!/bin/sh\nshift 2\nexec '${process.execPath}' "$@"\n`, 0o700);
  write(path.join(R, "update-trial"), "0.10.0");
  const update = path.join(R, "update.json");
  const bytes = '{"formatVersion":2,"trial":{"from":"0.9.0","to":"0.10.0"}}';
  write(update, bytes);
  write(path.join(K, "updates", "rollback", "restore.sh"), `#!/bin/sh\ntouch '${folder}/restored'\n`, 0o700);
  const result = spawnSync("sh", [path.join(K, "bin", "kuvo-start"), "rollback"], { env, encoding: "utf8", timeout: 20_000 });
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stderr, /Box update state format version 2 is unsupported/);
  assert.equal(exists(path.join(folder, "restored")), false);
  assert.equal(exists(path.join(R, ".rolling-back")), false);
  assert.equal(fs.readFileSync(update, "utf8"), bytes);
  assert.equal(fs.readFileSync(path.join(R, "update-trial"), "utf8"), "0.10.0");
});
