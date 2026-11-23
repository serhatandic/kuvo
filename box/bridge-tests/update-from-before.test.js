// The first update into this release, from a box whose code has none of it (PROTOCOL.md, "Updates"):
// that box's own updater takes it (as beta.30 does: it stops the engine and its bridge, puts the
// files in place, starts them again and waits two minutes for them), and leaves the update on trial
// with nothing of its own to judge it. This release's keeper takes over (the box's keeper starts it
// again on its new script), starts its judge, and the update is judged well and kept: the engine runs
// on the new keeper, the coding agents' bridge kept by it apart from the engine, the data as it was.
//
// The box from before is the code at 5d9b7b8db (the update design's audit: the keeper's twenty-minute
// timer, the two-minute wait, a bridge tied to its engine), read from git; skipped where the
// checkout has no history. The engine is a stand-in; the rest is the real thing. The same on macOS
// and Linux. Run with `node --test box/bridge-tests/update-from-before.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { NO_BUN, bunRunsAs, testBun } from "./bun-for-tests.js";

const BEFORE = "5d9b7b8db";
const bun = testBun();
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const fromGit = file => {
  const read = spawnSync("git", ["-C", checkout, "show", `${BEFORE}:${file}`], { maxBuffer: 100_000_000 });
  return read.status === 0 ? read.stdout : null;
};
const history = fromGit("box/src/keeper/kuvo-start") !== null;
const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-from-before-")));
const home = path.join(base, ".kuvo-code");
const RUN = path.join(home, "run");
const scratch = path.join(base, "scratch");
removeAtEnd(base);

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
}
const readJSON = file => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; } };
const runs = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
async function until(check, what, within = 180_000) {
  for (const end = Date.now() + within; Date.now() < end;) {
    const found = await check();
    if (found) return found;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.fail(`never: ${what}`);
}

test("the first update into this release, taken by the box's updater from before: kept, judged well by this release's keeper, its bridge kept apart from the engine, its data as it was", { skip: !history ? "no git history here" : bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  // The engine, a stand-in that answers as one does.
  const engineScript = path.join(scratch, "engine.mjs");
  write(engineScript, `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
http.createServer((request, response) => {
  if (request.url.startsWith("/api/event")) { response.writeHead(200, { "content-type": "text/event-stream" }); response.write(": open\\n\\n"); return; }
  response.writeHead(200, { "content-type": "application/json" });
  response.end(JSON.stringify({ data: request.url.startsWith("/api/session/active") ? {} : [] }));
}).listen(port, "127.0.0.1", () => console.log("server listening on http://127.0.0.1:" + port));
setInterval(() => { if (!fs.existsSync(${JSON.stringify(home)})) process.exit(0); }, 300);
`);
  const folder = path.join(home, "engine", "1.0.0");
  fs.mkdirSync(folder, { recursive: true });
  // (The program itself, never a script that starts it: the bridge sees which engine it runs on by
  // where its program is.)
  const program = bunRunsAs(bun);
  try { fs.linkSync(program, path.join(folder, "bun")); } catch { fs.copyFileSync(program, path.join(folder, "bun")); fs.chmodSync(path.join(folder, "bun"), 0o755); }
  write(path.join(folder, "opencode"), `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo 1.0.0 ;;
*.js) case "$2" in browser) exit 0 ;; esac; BUN_BE_BUN=1 exec "${folder}/bun" --config=/dev/null --no-env-file "$@" ;;
serve) trap 'kill "$c" 2>/dev/null' TERM INT HUP; '${process.execPath}' '${engineScript}' "$@" & c=$!; wait "$c" ;;
esac
`, 0o755);
  // The box as the code before left it: its keeper, launcher, link and bridge, on 0.9.0.
  for (const name of ["kuvo-start", "kuvo-launch", "kuvo-setsid", "kuvo-wake", "kuvo-npx"]) write(path.join(home, "bin", name), fromGit(`box/src/keeper/${name}`).toString("utf8").replaceAll("@ENGINE@", "1.0.0"), 0o700);
  write(path.join(home, "bin", "kuvo-link.js"), fromGit("swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"), 0o600);
  write(path.join(home, "bin", "kuvo-agents.js"), fromGit("box/dist/kuvo-agents.js"));
  write(path.join(home, "bin", "box-version"), "0.9.0");
  write(path.join(home, "box", "kind"), "machine\n");
  write(path.join(home, "data", "opencode", "opencode.db"), "your conversations");
  for (const tool of ["pkill", "caffeinate", "crontab", "launchctl"]) write(path.join(base, "tools", tool), "#!/bin/sh\nexit 0\n", 0o755);
  write(path.join(RUN, "launch.env"), [`PATH=${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, "LANG=C", "KUVO_SANDBOX=off", "KUVO_TEST_HEALTH_WINDOW=1500", "KUVO_TEST_HEALTH_EVERY=200", "KUVO_TEST_NO_PEERS=1"].join("\n") + "\n", 0o600);
  write(path.join(RUN, "launch.notes"), "{}", 0o600);
  // A phone paired: the updater from before leaves the trial to its link then (here, to nothing of its own).
  const key = () => crypto.randomBytes(32).toString("base64url");
  write(path.join(RUN, "link.json"), JSON.stringify({ id: "lab", name: "lab", relay: "ws://127.0.0.1:9", secret: key(), key: key(), phones: [{ key: key(), name: "iPhone", added: Math.floor(Date.now() / 1000), from: "pair" }] }), 0o600);
  write(path.join(RUN, "link.needed"), "", 0o600);

  // Its keeper (the one from before, a round every 15 seconds; this release's goes round as the test says).
  const env = { HOME: base, PATH: `${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, KUVO_TEST_KEEP_ROUND: "1" };
  const keeper = spawn("sh", [path.join(home, "bin", "kuvo-start"), "keep"], { env, detached: true, stdio: "ignore" });
  const gone = new Promise(resolve => keeper.once("exit", resolve));
  atEnd(async () => {
    spawnSync("sh", [path.join(home, "bin", "kuvo-start"), "off"], { env, timeout: 60_000 });
    try { process.kill(-keeper.pid, "SIGKILL"); } catch {}
    await gone;
  });
  const before = await until(() => { const said = readJSON(path.join(RUN, "engine.json")); return said?.pid && runs(said.pid) && said.agentsPort ? said : null; }, "the engine from before, with its bridge");
  const linkPid = () => { try { return Number(fs.readFileSync(path.join(RUN, "link.pid"), "utf8")) || 0; } catch { return 0; } };
  const oldLink = await until(() => (runs(linkPid()) ? linkPid() : 0), "the link from before");

  // This release, packed as the release workflow does, for the same engine.
  const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
  const root = path.join(scratch, "sources");
  for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js", "box/dist/kuvo-agents.js", "box/dist/kuvo-link.js"]) {
    write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
  }
  const integrity = "sha512-" + crypto.createHash("sha512").update("unused").digest("base64");
  const builds = { x86_64: { url: "https://example.invalid/engine-1.0.0.tgz", integrity }, aarch64: { url: "https://example.invalid/engine-1.0.0.tgz", integrity } };
  write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: "1.0.0", linux: builds, darwin: builds }));
  const out = path.join(scratch, "release");
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), "0.10.0", "https://example.invalid/box-0.10.0.tar.gz", out], { env: { ...process.env, BUN: bun, NODE: process.execPath, BOX_SOURCES: root, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }) }, encoding: "utf8" });
  assert.equal(packed.status, 0, packed.stderr);
  const pinned = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");

  // Taken by the updater from before (Update Now), in a process of its own, as that code runs it.
  const updating = spawnSync(path.join(folder, "opencode"), [path.join(home, "bin", "kuvo-link.js"), "update", "--now", "--from", out, "--key", pinned], { env: { ...env, KUVO_HOME: home }, encoding: "utf8", timeout: 240_000 });
  assert.deepEqual(JSON.parse(updating.stdout.trim().split("\n").at(-1)), { result: "updated", version: "0.10.0" }, updating.stderr);
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8").trim(), "0.10.0");
  assert.equal(fs.existsSync(path.join(RUN, "update-trial")), true, "on trial, nothing of the code before to judge it");

  // This release's keeper takes over and judges it: kept, well.
  await until(() => !fs.existsSync(path.join(RUN, "update-trial")), "the update judged");
  const state = readJSON(path.join(RUN, "update.json"));
  assert.equal(state.last?.result, "updated", JSON.stringify(state));
  assert.equal(state.last.to, "0.10.0");
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8").trim(), "0.10.0", "kept");
  assert.equal(fs.existsSync(path.join(home, "updates", "rollback")), false);
  assert.equal(fs.readFileSync(path.join(home, "data", "opencode", "opencode.db"), "utf8"), "your conversations");
  // On this release's keeper: the engine runs, the bridge kept by it apart from the engine.
  const engine = readJSON(path.join(RUN, "engine.json"));
  assert.notEqual(engine.pid, before.pid, "the updater from before started it again");
  assert.equal(engine.agentsPort, undefined, "the bridge no longer started with the engine");
  const bridge = Number(fs.readFileSync(path.join(RUN, "agents.pid"), "utf8"));
  assert.ok(runs(bridge), "the bridge runs");
  assert.match(spawnSync("ps", ["-ww", "-o", "command=", "-p", String(bridge)], { encoding: "utf8" }).stdout, /kuvo-agents\.js/);
  assert.equal(Number(fs.readFileSync(path.join(RUN, "box.pid"), "utf8")), keeper.pid, "the same keeper, on this release's script");
  // The link from before (which never leaves for new code by itself) replaced by this release's.
  assert.equal(runs(oldLink), false, "the link from before went");
  await until(() => linkPid() && linkPid() !== oldLink && runs(linkPid()), "this release's link to run");
});
