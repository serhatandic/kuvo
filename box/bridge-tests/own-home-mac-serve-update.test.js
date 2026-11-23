// This Mac's own box after an update (PROTOCOL.md, "Updates"): its server (`serve`, which Kuvo Helper
// starts from the box's `bin/kuvo-link.js` and starts again only when it ends) goes once an update
// changed the box's files under it, for its helper to start the new ones; whether the update is well
// is its judge's to say, never a server's. The real box script, run
// from a home of the test's own laid out as a Mac's; its helper and keeper are stand-ins.
//
// Run with `node --test box/bridge-tests/own-home-mac-serve-update.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-own-home-serve-update-")));
removeAtEnd(home);
const K = path.join(home, ".kuvo-code");
const write = (file, text, mode = 0o600) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
};
const readJSON = file => JSON.parse(fs.readFileSync(file, "utf8"));
const built = fileURLToPath(LINK_BUNDLE);

// A Mac's box in a home of its own, on 0.10.0, its update to it on trial and up (the update saw its
// engine and bridge answer), your phones paired: what the update had kept to put back, there.
write(path.join(K, "box", "kind"), "mac\n");
write(path.join(K, "folder-version.json"), '{"version":18}\n');
write(path.join(K, "bin", "box-version"), "0.10.0");
write(path.join(K, "bin", "kuvo-link.js"), fs.readFileSync(built, "utf8"));
write(path.join(K, "bin", "kuvo-start"), "#!/bin/sh\nexit 0\n", 0o755);
write(path.join(K, "run", "link.json"), JSON.stringify({ phones: [{ key: "phone-key", name: "iPhone" }] }));
write(path.join(K, "run", "update.json"), JSON.stringify({ trial: { from: "0.9.0", to: "0.10.0", at: Date.now(), healthy: true } }));
write(path.join(K, "run", "update-trial"), "0.10.0");
write(path.join(K, "updates", "rollback", "restore.sh"), "#!/bin/sh\n");
write(path.join(K, "updates", "rollback", "files", "bin", "kuvo-link.js"), fs.readFileSync(built, "utf8"));
// The copy of the engine's data an update kept 31 days ago (a Mac runs no bridge whose rounds would
// take it away).
write(path.join(K, "updates", "backups", "2026-10-16-1.0.0-to-1.1.0", "opencode.db"), "the data from before");
for (const each of ["2026-10-16-1.0.0-to-1.1.0/opencode.db", "2026-10-16-1.0.0-to-1.1.0"]) fs.utimesSync(path.join(K, "updates", "backups", each), new Date(Date.now() - 31 * 86_400_000), new Date(Date.now() - 31 * 86_400_000));

Object.assign(process.env, { HOME: home, KUVO_HOME: K, TMPDIR: home });
delete process.env.KUVO_BOX;
const link = await import(built);

// The box's server, as its helper starts it (`kuvo-start serve`: the box's own script), looking at
// its files as often as a test's says (`every`, in ms).
const running = new Set();
atEnd(() => { for (const child of running) child.kill(); });
async function serve(every = 100) {
  const child = spawn(process.execPath, [path.join(K, "bin", "kuvo-link.js"), "serve"], {
    env: { ...process.env, OPENCODE_PASSWORD: "box-pw", KUVO_TEST_CODE_EVERY: String(every), KUVO_TEST_NO_PEERS: "1" },
    stdio: ["pipe", "pipe", "pipe"],
  });
  running.add(child);
  let out = "", err = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", chunk => { err += chunk; });
  const exited = new Promise(resolve => child.once("exit", code => { running.delete(child); resolve(code); }));
  await new Promise((resolve, reject) => {
    child.stdout.on("data", chunk => {
      out += chunk;
      if (out.includes("\n")) resolve();
    });
    exited.then(code => reject(new Error(`serve exited ${code}: ${err}`)));
  });
  // Gone as its app or helper closes its input.
  const stop = async () => { child.stdin.end(); await exited; };
  return { exited, said: () => err, stop };
}

async function until(check, what) {
  for (let i = 0; i < 300; i++) {
    if (check()) return;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.fail(`never: ${what}`);
}

test("a Mac's box's server never says an update is well (its judge does, by the one health rule): the trial stays open while it runs; the old copy of the data goes", async () => {
  const first = await serve(100);
  const bundle = path.join(K, "bin", "kuvo-link.js");
  const route = async routePath => {
    const stdout = await new Promise((resolve, reject) =>
      execFile(process.execPath, [bundle, "box", "route", "--internal", routePath], { timeout: 15_000 },
        (error, stdout) => error ? reject(error) : resolve(stdout)));
    return JSON.parse(stdout);
  };
  try {
    assert.equal(readJSON(path.join(K, "run", "serve.json")).formatVersion, 1);
    assert.equal((await route("kuvo/attention")).status, 200, "the alias uses the Mac server's live hold");
    await until(() => fs.readdirSync(path.join(K, "updates", "backups")).length === 0, "the copy past its 30 days, gone");
  } finally { await first.stop(); }
  assert.equal((await route("kuvo/attention")).status, 503, "the selected pipe stays selected while stopped");
  assert.equal((await route("kuvo/info")).status, 200, "the installed-file probe still answers while stopped");
  assert.equal(fs.existsSync(path.join(K, "run", "update-trial")), true, "still on trial");
  assert.equal(fs.existsSync(path.join(K, "updates", "rollback", "restore.sh")), true, "what was kept to put back stays");
  assert.equal(readJSON(path.join(K, "run", "update.json")).last, undefined);
});

test("a server whose files an update changed goes, once no update is under way, for its helper to start the new ones", { timeout: 60_000 }, async () => {
  const started = { version: "0.10.0", link: "abc" };
  const after = { version: "0.11.0", link: "def" };
  const check = options => link.codeCheck(started, { updating: () => false, now: () => after, ...options });
  assert.equal(check(), "leave");
  // Not while an update changes them (it may yet put them back), nor for a file missing a moment.
  assert.equal(check({ updating: () => true }), "stay");
  assert.equal(check({ now: () => ({ version: null, link: "def" }) }), "stay");
  assert.equal(check({ now: () => started }), "stay", "the same files: it stays");
  assert.equal(check({ leaves: false }), "stay", "one its app starts (an app's own box) stays");

  // The real one: an update put a new version in place, and is done.
  const served = await serve();
  write(path.join(K, "bin", "box-version"), "0.11.0");
  assert.equal(await served.exited, 0, "it ends cleanly, as its helper starts it again");
  assert.match(served.said(), /files changed under it/);
  assert.equal(fs.existsSync(path.join(K, "run", "serve.json")), false, "where it answered goes with it");
});
