// The box's keeper (`kuvo-start keep`) and what boxes from before left (PROTOCOL.md, "Updates"; "For
// released Macs"): a rollback kept by an update from before rollbacks were code only never puts the
// engine's data back over what was written since; and the watchdog a Mac with Kuvo Code 0.7 starts
// beside its engine never stops it, with nothing killed by a pattern. The real keeper script, in a
// home of the test's own, with stand-ins for what it would reach of this computer (`pkill`,
// `caffeinate`, `crontab`, `launchctl`); times set on the files, never waited for.
//
// Run with `node --test box/bridge-tests/keeper-rollback.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
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
function home() {
  const folder = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-keeper-rollback-")));
  removeAtEnd(folder);
  const K = path.join(folder, ".kuvo-code");
  write(path.join(K, "box", "kind"), "machine\n");
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
  write(path.join(K, "updates", "rollback", "restore.sh"), [
    "#!/bin/sh",
    `K='${K}'; B='${K}/updates/rollback'`,
    `D="$K/data/opencode"`,
    `if [ -f "$B/data/opencode.db" ]; then`,
    `  rm -f "$D/opencode.db-wal" "$D/opencode.db-shm" || exit 1`,
    `  for n in opencode.db opencode.db-wal opencode.db-shm; do if [ -f "$B/data/$n" ]; then mv -f "$B/data/$n" "$D/$n" || exit 1; fi; done`,
    `fi`,
    `rm -f "$K/run/update-trial" "$K/run/update-restore"`,
    `rm -rf "$B"`,
  ].join("\n") + "\n", 0o700);
  write(path.join(R, "update-trial"), "0.10.0");
  aged(path.join(R, "update-trial"), 30 * 60_000);
  // What the new engine wrote since: a conversation.
  write(path.join(K, "data", "opencode", "opencode.db"), "a conversation after the update");
  write(path.join(K, "data", "opencode", "opencode.db-wal"), "its last message");

  const stop = keep(where);
  await until(() => !exists(path.join(R, "update-trial")), "the rollback ran");
  await stop();
  assert.equal(fs.readFileSync(path.join(K, "data", "opencode", "opencode.db"), "utf8"), "a conversation after the update");
  assert.equal(fs.readFileSync(path.join(K, "data", "opencode", "opencode.db-wal"), "utf8"), "its last message");
  // The copy from before, aside, where nothing puts it back.
  const kept = fs.readdirSync(path.join(K, "updates", "backups"));
  assert.equal(kept.length, 1);
  assert.match(kept[0], /^\d{4}-\d{2}-\d{2}-kept-for-a-rollback$/);
  assert.equal(fs.readFileSync(path.join(K, "updates", "backups", kept[0], "opencode.db"), "utf8"), "the data before the update");
  assert.equal(exists(path.join(K, "updates", "rollback")), false);
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
