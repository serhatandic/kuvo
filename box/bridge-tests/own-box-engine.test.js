// A Mac's own box in a home of its own (`box/kind` says `mac`), as Kuvo Helper runs it: its keeper
// (`kuvo-start keep`) starts its engine, though no Mac ever set it up over SSH to note how
// (`run/launch.env`): moved from an earlier Kuvo Code, its engine never started, and the app said
// the engine isn't running. A machine's box still starts nothing until it's set up. The real
// keeper and launcher (box/src/keeper), in homes of the test's own; only the engine's binary is a
// stand-in, and the tools the keeper's own loop reaches for (`pkill`, `caffeinate`, `sleep 15`).
//
// Run with `node --test box/bridge-tests/own-box-engine.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const checkout = fileURLToPath(new URL("../..", import.meta.url));
const write = (file, text, mode = 0o644) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
  fs.chmodSync(file, mode);
};
const read = file => fs.readFileSync(file, "utf8");
const exists = file => fs.existsSync(file);
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const command = pid => spawnSync("ps", ["-ww", "-o", "command=", "-p", String(pid)], { encoding: "utf8" }).stdout.trim();

/** A home of the test's own with the real keeper and launcher in its box (`kind`, if given), and an
 * engine whose binary is a stand-in: it says it's listening, as the launcher waits to read, and
 * answers as one does. */
function box(kind) {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-own-box-engine-")));
  removeAtEnd(home);
  const K = path.join(home, ".kuvo-code");
  for (const name of ["kuvo-start", "kuvo-launch", "kuvo-setsid"]) {
    write(path.join(K, "bin", name), read(path.join(checkout, "box", "src", "keeper", name)).replace("@ENGINE@", "1.0.0"), 0o700);
  }
  if (kind) write(path.join(K, "box", "kind"), `${kind}\n`, 0o600);
  const engine = path.join(home, "engine.mjs");
  write(engine, `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
http.createServer((request, response) => { response.writeHead(200, { "content-type": "application/json" }); response.end("[]"); })
  .listen(port, "127.0.0.1", () => console.log("server listening on http://127.0.0.1:" + port));
setInterval(() => { if (!fs.existsSync(${JSON.stringify(K)})) process.exit(0); }, 300);
`);
  write(path.join(K, "engine", "1.0.0", "opencode"), `#!/bin/sh
case "$1" in
serve) trap 'kill "$c" 2>/dev/null' TERM INT HUP; '${process.execPath}' '${engine}' "$@" & c=$!; wait "$c" ;;
esac
`, 0o755);
  // The keeper's own loop's tools, noting they were asked: never this computer's `pkill` or
  // `caffeinate`; and its `sleep 15` between rounds, which notes the round is over and waits.
  const tools = path.join(home, "tools");
  for (const tool of ["pkill", "caffeinate"]) write(path.join(tools, tool), `#!/bin/sh\necho "${tool} $*" >>'${home}/reached'\n`, 0o755);
  write(path.join(tools, "sleep"), `#!/bin/sh\ncase "$1" in 15) echo round >>'${home}/rounds'; exec /bin/sleep 3600 ;; esac\nexec /bin/sleep "$@"\n`, 0o755);
  // As Kuvo Helper starts it: a shell's few, and nothing of Kuvo Code's.
  const env = { HOME: home, USER: os.userInfo().username, SHELL: "/bin/zsh", LANG: "en_GB.UTF-8", TMPDIR: home, PATH: `${tools}:/usr/bin:/bin:/usr/sbin:/sbin` };
  // What it started, stopped as the test ends: the keeper's session (a launch started one too, as
  // it does where none runs) and the engine it noted, each only while it's still this test's.
  const run = path.join(K, "run");
  atEnd(() => {
    for (const [file, group] of [["box.pid", true], ["engine.json", false]]) {
      let pid = null;
      try { pid = Number(read(path.join(run, file)).match(/\d+/)?.[0]) || null; } catch {}
      if (pid && command(pid).includes(home)) try { process.kill(group ? -pid : pid); } catch {}
    }
  });
  return { home, K, run, env };
}

/** Once `file` is there. */
const there = file => new Promise(resolve => {
  const watcher = fs.watch(path.dirname(file), () => { if (exists(file)) { watcher.close(); resolve(); } });
  if (exists(file)) { watcher.close(); resolve(); }
});

/** `kuvo-start keep` started as Kuvo Helper starts it (in a session of its own), through its
 * first round. */
async function keptFirstRound(where) {
  const keeper = spawn("sh", [path.join(where.K, "bin", "kuvo-start"), "keep"], { cwd: where.K, env: where.env, detached: true, stdio: "ignore" });
  keeper.unref();
  // (A keeper that ends before its first round fails the test, rather than leave it waiting.)
  const ended = new Promise((resolve, reject) => keeper.once("exit", code => reject(new Error(`the keeper ended before its first round (${code})`))));
  ended.catch(() => {}); // (Its end as the test stops it is no failure.)
  await Promise.race([there(path.join(where.home, "rounds")), ended]);
  return keeper;
}

test("a Mac's own box, moved with no note of how its engine starts: its keeper notes it from what its helper gives it, and starts the engine", async () => {
  const where = box("mac");
  fs.mkdirSync(where.run, { recursive: true });
  assert.equal(exists(path.join(where.run, "launch.env")), false);
  await keptFirstRound(where);

  const launch = path.join(where.run, "launch.env");
  assert.equal(exists(launch), true, "how its engine starts, noted");
  assert.deepEqual(read(launch).trim().split("\n").sort(), [`LANG=en_GB.UTF-8`, "OPENCODE_CLIENT=desktop", `PATH=${where.env.PATH}`, "SHELL=/bin/zsh"]);
  assert.equal((fs.statSync(launch).mode & 0o777).toString(8), "600", "only yours to read");

  const noted = JSON.parse(read(path.join(where.run, "engine.json")));
  assert.equal(noted.version, "1.0.0");
  assert.ok(alive(noted.pid) && command(noted.pid).includes(path.join(where.K, "engine", "1.0.0", "opencode")), `its own engine runs: ${command(noted.pid)}`);
  assert.equal((await fetch(`http://127.0.0.1:${noted.port}/session`)).status, 200, "and answers where it's noted");
});

test("a Mac's own box keeps the note it has; a machine's box that was never set up starts nothing", async () => {
  // Noted already: left as it is.
  const mac = box("mac");
  write(path.join(mac.run, "launch.env"), "LANG=C\n", 0o600);
  assert.equal(spawnSync("sh", [path.join(mac.K, "bin", "kuvo-start"), "engine"], { cwd: mac.K, env: mac.env }).status, 0);
  assert.equal(read(path.join(mac.run, "launch.env")), "LANG=C\n");
  assert.ok(alive(JSON.parse(read(path.join(mac.run, "engine.json"))).pid));

  // A machine's: until a Mac sets it up (which notes how), its keeper starts no engine.
  const machine = box();
  fs.mkdirSync(machine.run, { recursive: true });
  await keptFirstRound(machine);
  assert.equal(exists(path.join(machine.run, "launch.env")), false);
  assert.equal(exists(path.join(machine.run, "engine.json")), false);
});
