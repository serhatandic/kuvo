// The coding agents never look down while their bridge hands over (PROTOCOL.md, "The coding agents'
// state"): a client that keeps asking through the box's server while the bridge makes way for a
// newer build (as for a new engine) gets every answer from a bridge, never an error; the next bridge
// starts at once, not at the keeper's next round. And a bridge that keeps stopping is said to, in
// one plain sentence with Try Again, which starts it again once it can run.
//
// Everything is the real thing: the keeper (`kuvo-start keep`, `kuvo-launch`), the box's server for
// clients (`serve --kept`) and the bridge as built. A stand-in only for the engine (an HTTP server).
// In a home of the test's own; the same on macOS and Linux. Run with
// `node --test box/bridge-tests/agents-handover.test.js`.
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

const bun = testBun();
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const builtLink = path.join(checkout, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const builtAgents = path.join(checkout, "box/dist/kuvo-agents.js");

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
}
const readJSON = file => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; } };
const runs = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const build = text => [...text.matchAll(/\/\/ Built from Packages\/Box\/src\/agents ([0-9a-f]{16,64})/g)].at(-1)?.[1];

/** A machine's box, kept by its keeper (a round a second, or `round`), with the box's server for
 * clients wanted, and its bridge's program as `agents` (the built one by default). */
function machine(prefix, { agents, round = "1", before } = {}) {
  const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), prefix)));
  removeAtEnd(base);
  const home = path.join(base, ".kuvo-code");
  const run = path.join(home, "run");
  const scratch = path.join(base, "scratch");
  const engine = path.join(scratch, "engine.mjs");
  write(engine, `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
http.createServer((request, response) => {
  if (request.url.startsWith("/api/event") || request.url.startsWith("/event")) { response.writeHead(200, { "content-type": "text/event-stream" }); response.write(": open\\n\\n"); return; }
  response.writeHead(200, { "content-type": "application/json" });
  response.end(JSON.stringify({ data: request.url.startsWith("/api/session/active") ? {} : [] }));
}).listen(port, "127.0.0.1", () => console.log("server listening on http://127.0.0.1:" + port));
setInterval(() => { if (!fs.existsSync(${JSON.stringify(home)})) process.exit(0); }, 300);
`);
  const folder = path.join(home, "engine", "1.0.0");
  fs.mkdirSync(folder, { recursive: true });
  const program = bunRunsAs(bun);
  try { fs.linkSync(program, path.join(folder, "bun")); } catch { fs.copyFileSync(program, path.join(folder, "bun")); fs.chmodSync(path.join(folder, "bun"), 0o755); }
  write(path.join(folder, "opencode"), `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo 1.0.0 ;;
*.js) case "$2" in browser) exit 0 ;; esac; BUN_BE_BUN=1 exec "${folder}/bun" --config=/dev/null --no-env-file "$@" ;;
serve) trap 'kill "$c" 2>/dev/null' TERM INT HUP; '${process.execPath}' '${engine}' "$@" & c=$!; wait "$c" ;;
esac
`, 0o755);
  for (const name of ["kuvo-start", "kuvo-launch", "kuvo-setsid", "kuvo-wake", "kuvo-npx"]) {
    write(path.join(home, "bin", name), fs.readFileSync(path.join(checkout, "box/src/keeper", name), "utf8").replaceAll("@ENGINE@", "1.0.0"), 0o700);
  }
  write(path.join(home, "bin", "kuvo-link.js"), fs.readFileSync(builtLink), 0o600);
  write(path.join(home, "bin", "kuvo-agents.js"), agents ?? fs.readFileSync(builtAgents, "utf8"));
  write(path.join(home, "bin", "box-version"), "0.10.0");
  write(path.join(home, "box", "kind"), "machine\n");
  for (const tool of ["pkill", "caffeinate", "crontab", "launchctl"]) write(path.join(base, "tools", tool), `#!/bin/sh\nexit 0\n`, 0o755);
  // How its engine starts: a bridge that makes way at once once nothing runs.
  write(path.join(run, "launch.env"), [`PATH=${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, "LANG=C", "KUVO_SANDBOX=off", "KUVO_TEST_RENEW_SETTLE=0", "KUVO_TEST_RENEW_UPTIME=0", "KUVO_TEST_NO_PEERS=1"].join("\n") + "\n", 0o600);
  write(path.join(run, "launch.notes"), "{}", 0o600);
  const key = () => crypto.randomBytes(32).toString("base64url");
  write(path.join(run, "link.json"), JSON.stringify({ id: "lab", name: "lab", relay: "ws://127.0.0.1:9", secret: key(), key: key(), phones: [] }), 0o600);
  write(path.join(run, "serve.needed"), "", 0o600);
  before?.(home, run);

  const keeper = spawn("sh", [path.join(home, "bin", "kuvo-start"), "keep"], { env: { HOME: base, PATH: `${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, KUVO_TEST_KEEP_ROUND: round }, detached: true, stdio: "ignore" });
  const gone = new Promise(resolve => keeper.once("exit", resolve));
  atEnd(async () => {
    spawnSync("sh", [path.join(home, "bin", "kuvo-start"), "off"], { env: { HOME: base, PATH: "/usr/bin:/bin" }, timeout: 60_000 });
    try { process.kill(-keeper.pid, "SIGKILL"); } catch {}
    await gone;
  });

  async function until(check, what, within = 180_000) {
    for (const end = Date.now() + within; Date.now() < end;) {
      const found = await check();
      if (found) return found;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    let log = "";
    try { log = fs.readFileSync(path.join(run, "agents.log"), "utf8").slice(-3000); } catch {}
    assert.fail(`never: ${what}\n${log}`);
  }
  /** A client's request, through the box's server (`serve --kept`), as Kuvo Code's apps send it. */
  async function ask(method, route, body) {
    const said = readJSON(path.join(run, "serve.json"));
    if (!said?.port) return { status: 0, json: null };
    try {
      const response = await fetch(`http://127.0.0.1:${said.port}/${route}`, { method, headers: { authorization: "Basic " + Buffer.from(`opencode:${said.password}`).toString("base64"), "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
      const text = await response.text();
      return { status: response.status, json: text ? JSON.parse(text) : null };
    } catch {
      return { status: 0, json: null };
    }
  }
  const serving = () => until(async () => (await ask("GET", "kuvo/info")).status === 200, "the box's server to answer");
  /** The bridge answering a client, through the box: its pid. */
  const bridgeUp = besides => until(async () => {
    const said = readJSON(path.join(run, "bridge.json"));
    if (!said?.pid || said.pid === besides || !runs(said.pid)) return null;
    const info = await ask("GET", "kuvo/agents/api/info");
    return info.status === 200 && info.json?.pid === said.pid ? said.pid : null;
  }, besides ? "a new bridge to answer" : "the bridge to answer");
  return { home, run, until, ask, serving, bridgeUp };
}

test("a client asking all through a bridge's hand-over to a newer one gets every answer, never 'not running', and the next starts at once", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const agents = fs.readFileSync(builtAgents, "utf8");
  // The bridge a build before this one, which makes way for this one once it's put in its place.
  // (The keeper looks only every minute: the next bridge comes from the hand-over itself.)
  const box = machine("kuvo-handover-", { agents: agents.replace(`// Built from Packages/Box/src/agents ${build(agents)}`, `// Built from Packages/Box/src/agents ${"a".repeat(64)}`), round: "60" });
  await box.serving();
  const first = await box.bridgeUp();

  // A client asking again and again, as the rings, Accounts and a conversation do.
  const answers = [];
  let asking = true;
  let sawHandOver = false;
  const client = (async () => {
    while (asking) {
      const handingOver = fs.existsSync(path.join(box.run, "agents-renewing")) || !readJSON(path.join(box.run, "bridge.json"));
      const answer = await box.ask("GET", "kuvo/agents/api/info");
      answers.push({ status: answer.status, pid: answer.json?.pid, message: answer.json?.message, handingOver });
      if (handingOver) sawHandOver = true;
      // (At a client's pace: the rings and a conversation ask every few moments, not without a break.)
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  })();
  await box.until(() => answers.length >= 3, "the client's first answers");

  const file = path.join(box.home, "bin", "kuvo-agents.js");
  fs.writeFileSync(`${file}.new`, agents);
  fs.renameSync(`${file}.new`, file);
  const second = await box.bridgeUp(first);
  // A few more answers from the new one, then the client stops.
  const from = answers.length;
  await box.until(() => answers.slice(from).filter(each => each.pid === second).length >= 3, "answers from the new bridge");
  asking = false;
  await client;

  assert.ok(!runs(first), "the bridge before made way");
  assert.ok(!fs.existsSync(path.join(box.run, "agents-renewing")), "the new bridge took the hand-over's mark away");
  const failed = answers.filter(each => each.status !== 200);
  assert.deepEqual(failed, [], "every answer came from a bridge: none said the agents aren't running");
  assert.ok(answers.some(each => each.pid === first) && answers.some(each => each.pid === second), "answered by the bridge before, then by the new one");
  assert.ok(sawHandOver, "some were asked while the bridge was handing over");
});

test("a bridge that keeps stopping is said to in one plain sentence with Try Again; Try Again starts it once it can run", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-crashing-", { agents: "throw new Error(\"a bridge that can't start\")\n" });
  await box.serving();
  // Its keeper starts it again and again, waiting longer each time: past half a minute, it's down.
  await box.until(() => {
    const retry = Number(fs.readFileSync(path.join(box.run, "agents.retry"), { encoding: "utf8", flag: "a+" }).trim());
    return retry * 1000 - Date.now() > 31_000;
  }, "the keeper to wait a while before its next start");
  const down = await box.ask("GET", "kuvo/agents/api/session");
  assert.equal(down.status, 503);
  assert.match(down.json.message, /^The coding agents keep stopping\. Kuvo tries again in about (a minute|\d+ minutes)\.$/);
  assert.equal(down.json.reason, "agentsDown");
  assert.deepEqual(down.json.action, { label: "Try Again", route: "kuvo/agents/start" });
  // The same sentence wherever it's needed: Accounts' coding agents too.
  const accounts = await box.ask("GET", "kuvo/sign-in/accounts?agents=1");
  assert.equal(accounts.status, 503);
  assert.equal(accounts.json.message, down.json.message);

  // Mended (an update brought a bridge that runs), Try Again starts it now, not at the keeper's next start.
  fs.writeFileSync(path.join(box.home, "bin", "kuvo-agents.js"), fs.readFileSync(builtAgents));
  const tried = await box.ask("POST", "kuvo/agents/start");
  assert.equal(tried.status, 200, JSON.stringify(tried.json));
  assert.ok(!fs.existsSync(path.join(box.run, "agents.retry")), "no wait noted any more");
  assert.equal((await box.ask("GET", "kuvo/agents/api/session")).status, 200);
});

test("a copy an older Mac's Kuvo Code kept of the bridge never goes back over the one a release put there", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-own-copy-", {
    before: (home, run) => {
      // The bridge as a Mac from before 1.0 kept it (bin/.own), from before its keeper kept it apart
      // from the engine: it can't start under this keeper.
      write(path.join(home, "bin", ".own", "kuvo-agents.js"), "console.error('KUVO_AGENTS_HOME, KUVO_HOME, KUVO_AGENTS_PORT, KUVO_AGENTS_PASSWORD, KUVO_ENGINE_PORT and KUVO_ENGINE_PID are needed.'); process.exit(1)\n");
      // The release's files, all with the one time a release gives them, its version's too.
      for (const file of ["box-version", "kuvo-agents.js"]) fs.utimesSync(path.join(home, "bin", file), 0, 0);
      // An older Mac connected since (it touches run/attached).
      write(path.join(run, "attached"), "", 0o600);
    },
  });
  await box.serving();
  // The release's bridge runs, and is still the one there.
  await box.bridgeUp();
  assert.equal(fs.readFileSync(path.join(box.home, "bin", "kuvo-agents.js"), "utf8"), fs.readFileSync(builtAgents, "utf8"));
});
