// Stop Kuvo on this Mac (#761; PROTOCOL.md, "In the menu bar"): the keeper's `pause`, as
// `kuvo/box/stop` runs it. Everything a Mac's own box runs stops, not for good (that's `off`): no
// start under way starts the engine while it's marked, and the next keeper to start (its helper's,
// at the next login or as the app opens) takes the mark away. The real keeper script
// (box/src/keeper/kuvo-start), in a home of the test's own, with stand-ins for `pkill` and
// `caffeinate`: never this computer's processes but the ones it starts.
//
// Run with `node --test box/bridge-tests/keeper-pause.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const keeper = fileURLToPath(new URL("../src/keeper/kuvo-start", import.meta.url));
const write = (file, text, mode) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
  if (mode) fs.chmodSync(file, mode);
};
const exists = file => fs.existsSync(file);
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const until = async condition => { for (let waited = 0; waited < 400 && !condition(); waited++) await new Promise(done => setTimeout(done, 25)); };

/** A home of the test's own: its box of `kind`, its keeper the real one, and stand-in tools. */
function home(kind) {
  const folder = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-keeper-pause-")));
  removeAtEnd(folder);
  const K = path.join(folder, ".kuvo-code");
  write(path.join(K, "box", "kind"), `${kind}\n`);
  write(path.join(K, "bin", "kuvo-start"), fs.readFileSync(keeper, "utf8").replace("@ENGINE@", "1.0.0"), 0o700);
  const tools = path.join(folder, "tools");
  for (const tool of ["pkill", "caffeinate"]) write(path.join(tools, tool), `#!/bin/sh\necho "${tool} $*" >>"${folder}/reached"\n`, 0o755);
  const env = { PATH: `${tools}:/usr/bin:/bin:/usr/sbin:/sbin`, HOME: folder };
  const run = (...args) => spawnSync("sh", [path.join(K, "bin", "kuvo-start"), ...args], { env, encoding: "utf8", timeout: 120_000 });
  return { folder, K, R: path.join(K, "run"), env, run };
}

/** A process that runs as a keeper does (`kuvo-start` in its command line), in a group of its own. */
function standInKeeper(where) {
  const file = path.join(where.folder, "stand-in", "kuvo-start");
  write(file, "#!/bin/sh\nwhile :; do sleep 1; done\n", 0o755);
  const child = spawn("sh", [file, "keep"], { detached: true, stdio: "ignore" });
  atEnd(() => { try { process.kill(-child.pid, "SIGKILL"); } catch {} });
  return child.pid;
}

test("pause: a Mac's own box's keeper stops, marked until it next starts, and the engine isn't started meanwhile", { skip: process.platform === "win32" }, async () => {
  const where = home("mac");
  const kept = standInKeeper(where);
  await until(() => spawnSync("ps", ["-ww", "-o", "command=", "-p", String(kept)], { encoding: "utf8" }).stdout.includes("kuvo-start"));
  write(path.join(where.R, "box.pid"), `${kept}\n`);
  const paused = where.run("pause");
  assert.equal(paused.status, 0, paused.stderr);
  assert.equal(exists(path.join(where.R, "paused")), true);
  assert.equal(exists(path.join(where.R, "off")), false, "not for good");
  assert.equal(exists(path.join(where.R, "box.pid")), false);
  await until(() => !alive(kept));
  assert.equal(alive(kept), false, "its keeper, with its group");

  // A start under way, or one asked for, doesn't start the engine while it's marked.
  write(path.join(where.R, "launch.env"), "PATH=/usr/bin:/bin\n");
  write(path.join(where.K, "engine", "1.0.0", "opencode"), "#!/bin/sh\n", 0o755);
  write(path.join(where.K, "bin", "kuvo-launch"), `#!/bin/sh\necho launched >>"${where.folder}/launched"\n`, 0o755);
  assert.equal(where.run("engine").status, 0);
  assert.equal(exists(path.join(where.folder, "launched")), false, "no engine started while paused");

  // The next keeper to start (its helper's, at login or as the app opens) takes the mark away.
  fs.rmSync(path.join(where.R, "launch.env"));
  const next = spawn("sh", [path.join(where.K, "bin", "kuvo-start"), "keep"], { env: where.env, detached: true, stdio: "ignore" });
  atEnd(() => { try { process.kill(-next.pid, "SIGKILL"); } catch {} });
  await until(() => !exists(path.join(where.R, "paused")));
  assert.equal(exists(path.join(where.R, "paused")), false);
  const gone = new Promise(done => next.once("exit", done));
  process.kill(-next.pid, "SIGKILL");
  await gone;
});

test("pause: a machine's box isn't stopped so (nothing but its own start would start it again)", () => {
  const where = home("machine");
  const paused = where.run("pause");
  assert.equal(paused.status, 3);
  assert.match(paused.stderr, /Only a Mac's own box/);
  assert.equal(exists(path.join(where.R, "paused")), false);
});
