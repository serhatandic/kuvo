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
import { removeTestHomeAtExit } from "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import http from "node:http";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { NO_BUN, bunRunsAs, testBun } from "./bun-for-tests.js";

const bun = testBun();
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const builtLink = path.join(checkout, "box/dist/kuvo-link.js");
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
function machine(prefix, { agents, round = "1", before, release, kept = true, held = true, feed = null } = {}) {
  const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), prefix)));
  removeTestHomeAtExit(base);
  const home = path.join(base, ".kuvo-code");
  const run = path.join(home, "run");
  const scratch = path.join(base, "scratch");
  const engine = path.join(scratch, "engine.mjs");
  write(engine, `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
http.createServer((request, response) => {
  if (request.url.startsWith("/api/event") || request.url.startsWith("/event")) { response.writeHead(200, { "content-type": "text/event-stream" }); response.write(": open\\n\\n"); return; }
  if (/^\\/api\\/session\\/ses_[^/]+$/.test(request.url)) { response.writeHead(404); response.end(); return; }
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
  // These scenarios start with a request for an engine, which the resident now starts on demand.
  write(path.join(run, "engine.now"), "", 0o600);
  const key = () => crypto.randomBytes(32).toString("base64url");
  write(path.join(run, "link.json"), JSON.stringify({ id: "lab", name: "lab", relay: "ws://127.0.0.1:9", secret: key(), key: key(), phones: [] }), 0o600);
  write(path.join(run, "serve.needed"), "", 0o600);
  // A release installed here, as an update leaves it: its files in place, and held (updates/release),
  // with its key for a test's folder (`KUVO_TEST_RELEASE_KEY`).
  if (release) {
    const unpacked = path.join(scratch, "unpacked");
    fs.mkdirSync(unpacked, { recursive: true });
    const pack = fs.readdirSync(release.out).find(name => name.endsWith(".tar.gz"));
    assert.equal(spawnSync("tar", ["xzf", path.join(release.out, pack), "-C", unpacked]).status, 0);
    const box = JSON.parse(fs.readFileSync(path.join(unpacked, "box.json"), "utf8"));
    for (const file of Object.keys(box.files)) {
      fs.mkdirSync(path.dirname(path.join(home, file)), { recursive: true });
      fs.copyFileSync(path.join(unpacked, file), path.join(home, file));
      fs.chmodSync(path.join(home, file), fs.statSync(path.join(unpacked, file)).mode & 0o777);
    }
    // (Held, as this box's own updater keeps it; or not, as an older updater left it.)
    if (held) {
      fs.mkdirSync(path.join(home, "updates", "release"), { recursive: true });
      for (const name of fs.readdirSync(release.out)) fs.copyFileSync(path.join(release.out, name), path.join(home, "updates", "release", name));
    }
    fs.appendFileSync(path.join(run, "launch.env"), `KUVO_TEST_RELEASE_KEY=${release.pinned}\nKUVO_TEST_FETCH_WAIT=0\n${feed ? `KUVO_TEST_RELEASE_FEED=${feed}\n` : ""}`);
  }
  before?.(home, run);

  const keeper = kept && spawn("sh", [path.join(home, "bin", "kuvo-start"), "keep"], { env: { HOME: base, PATH: `${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, KUVO_TEST_KEEP_ROUND: round }, detached: true, stdio: "ignore" });
  const gone = keeper ? new Promise(resolve => keeper.once("exit", resolve)) : null;
  if (keeper) atEnd(async () => {
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
  async function ask(method, route, body, headers = {}) {
    const said = readJSON(path.join(run, "serve.json"));
    if (!said?.port) return { status: 0, json: null };
    try {
      const response = await fetch(`http://127.0.0.1:${said.port}/${route}`, { method, headers: { authorization: "Basic " + Buffer.from(`opencode:${said.password}`).toString("base64"), "content-type": "application/json", ...headers }, body: body ? JSON.stringify(body) : undefined });
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
  /** A look of the box's own (`kuvo-link.js repair`), as its keeper starts one: what it came to. */
  const repair = (key = release?.pinned, feed = null) => {
    const ran = spawnSync(program, ["--config=/dev/null", "--no-env-file", path.join(home, "bin", "kuvo-link.js"), "repair"], { env: { HOME: base, PATH: "/usr/bin:/bin", KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_TEST_FETCH_WAIT: "0", ...(key ? { KUVO_TEST_RELEASE_KEY: key } : {}), ...(feed ? { KUVO_TEST_RELEASE_FEED: feed } : {}) }, encoding: "utf8", timeout: 120_000 });
    return JSON.parse(ran.stdout.trim().split("\n").pop());
  };
  /** The same, not holding up this process (whose feed it may fetch from). */
  const repairing = (key = release?.pinned, feed = null) => new Promise((resolve, reject) => {
    const child = spawn(program, ["--config=/dev/null", "--no-env-file", path.join(home, "bin", "kuvo-link.js"), "repair"], { env: { HOME: base, PATH: "/usr/bin:/bin", KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_TEST_FETCH_WAIT: "0", ...(key ? { KUVO_TEST_RELEASE_KEY: key } : {}), ...(feed ? { KUVO_TEST_RELEASE_FEED: feed } : {}) }, stdio: ["ignore", "pipe", "ignore"] });
    let out = "";
    child.stdout.on("data", chunk => { out += chunk; });
    child.on("error", reject);
    child.on("close", () => { try { resolve(JSON.parse(out.trim().split("\n").pop())); } catch (error) { reject(error); } });
  });
  return { home, run, until, ask, serving, bridgeUp, repair, repairing };
}

test("a client asking all through a bridge's hand-over to a newer one gets every answer, never 'not running', and the next starts at once", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const agents = fs.readFileSync(builtAgents, "utf8");
  // The bridge a build before this one, which makes way for this one once it's put in its place.
  // (The keeper looks only every minute: the next bridge comes from the hand-over itself.)
  const box = machine("kuvo-handover-", {
    agents: agents.replace(`// Built from Packages/Box/src/agents ${build(agents)}`, `// Built from Packages/Box/src/agents ${"a".repeat(64)}`),
    round: "60",
    before(home, run) {
      const file = path.join(home, "bin", "kuvo-link.js");
      const link = fs.readFileSync(file, "utf8");
      const entry = "async function* agentsRoute(context) {";
      assert.ok(link.includes(entry));
      const seen = JSON.stringify(path.join(run, "test-cutover-request"));
      write(file, link.replace(entry, entry + `\nif (context.request.path.endsWith("/api/info?test-cutover=1")) (await import("node:fs")).writeFileSync(${seen}, "");`), 0o600);
    },
  });
  await box.serving();
  const first = await box.bridgeUp();

  // A client asking again and again, as the rings, Accounts and a conversation do.
  const answers = [];
  let asking = true;
  const client = (async () => {
    while (asking) {
      const answer = await box.ask("GET", "kuvo/agents/api/info");
      answers.push({ status: answer.status, pid: answer.json?.pid, message: answer.json?.message });
      // (At a client's pace: the rings and a conversation ask every few moments, not without a break.)
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  })();
  let second;
  let cutover;
  const publish = path.join(box.run, "test-publish-bridge");
  try {
    await box.until(() => answers.length >= 3, "the client's first answers");

    const file = path.join(box.home, "bin", "kuvo-agents.js");
    // Keep the standby unpublished until a client has issued a request during the handover.
    const checkpoint = "transfer = standby?.transfer ?? recovered;";
    assert.ok(agents.includes(checkpoint));
    const candidate = agents.replace(checkpoint, checkpoint + `\nif (standby) while (!(await Bun.file(${JSON.stringify(publish)}).exists())) await Bun.sleep(20);`);
    fs.writeFileSync(`${file}.new`, candidate);
    fs.renameSync(`${file}.new`, file);
    await box.until(() => fs.existsSync(path.join(box.run, "agents-renewing")), "the bridge's handover fence");
    const during = box.ask("GET", "kuvo/agents/api/info?test-cutover=1");
    await box.until(() => fs.existsSync(path.join(box.run, "test-cutover-request")), "the Box to receive the request before publication");
    fs.writeFileSync(publish, "");
    cutover = await during;
    second = await box.bridgeUp(first);
    const from = answers.length;
    await box.until(() => answers.slice(from).filter(each => each.pid === second).length >= 3, "answers from the new bridge");
  } finally {
    asking = false;
    fs.closeSync(fs.openSync(publish, "a"));
    await client;
  }

  assert.ok(!runs(first), "the bridge before made way");
  assert.ok(!fs.existsSync(path.join(box.run, "agents-renewing")), "the new bridge took the hand-over's mark away");
  const failed = answers.filter(each => each.status !== 200);
  assert.deepEqual(failed, [], "every answer came from a bridge: none said the agents aren't running");
  assert.ok(answers.some(each => each.pid === first) && answers.some(each => each.pid === second), "answered by the bridge before, then by the new one");
  assert.deepEqual([cutover.status, cutover.json?.pid], [200, second], "the replacement answers a request issued during the handover");
});

test("owner-routed app Send can retry the same ID after a live bridge refuses it before execution", { skip: bun ? false : NO_BUN, timeout: 180_000 }, async () => {
  const agents = fs.readFileSync(builtAgents, "utf8");
  const id = "msg_handoverretry001";
  const marks = {};
  const box = machine("kuvo-send-handover-", {
    round: "60",
    before(home, run) {
      for (const name of ["lookup", "fenced", "refused", "finish", "accepted"]) marks[name] = path.join(run, "test-send-" + name);
      const quoted = Object.fromEntries(Object.entries(marks).map(([name, file]) => [name, JSON.stringify(file)]));
      const linkFile = path.join(run, "link.json");
      const link = readJSON(linkFile);
      const key = () => crypto.randomBytes(32).toString("base64url");
      write(linkFile, JSON.stringify({ ...link, setup: { id: "set_" + crypto.randomBytes(16).toString("base64url"), epoch: 1, role: "member", primary: key(), sign: key() } }));

      // The app discovered the old bridge. Hold only this first request until its real fence is up.
      const boxFile = path.join(home, "bin", "kuvo-link.js");
      const boxSource = fs.readFileSync(boxFile, "utf8");
      const located = "const target = place?.target ?? null;";
      assert.ok(boxSource.includes(located));
      write(boxFile, boxSource.replace(located, located + `
if (given.id === ${JSON.stringify(id)}) {
  const files = await import("node:fs");
  if (!files.existsSync(${quoted.lookup})) {
    files.writeFileSync(${quoted.lookup}, "");
    while (!files.existsSync(${quoted.fenced})) await new Promise(resolve => setTimeout(resolve, 20));
  }
}`));

      // Give the retiring bridge time to serve its actual pre-execution fence before it exits.
      const fenced = "transferring = true;";
      const refusal = /if \(transferring\)\s+return (answer\d*)\(request\);/;
      assert.ok(agents.includes(fenced) && refusal.test(agents));
      const old = agents.replace(fenced, fenced + `
await Bun.write(${quoted.fenced}, "");
while (!(await Bun.file(${quoted.finish}).exists())) await Bun.sleep(20);
`).replace(refusal, (_matched, answer) => `
if (transferring) return ${answer}(request).then(async response => {
  if (request.method === "POST" && new URL(request.url).pathname.endsWith("/prompt") &&
      response.headers.get("x-kuvo-bridge-handover") === "1") {
    await Bun.write(${quoted.refused}, (await request.clone().json()).id);
  }
  return response;
});
`).replace(`// Built from Packages/Box/src/agents ${build(agents)}`, `// Built from Packages/Box/src/agents ${"b".repeat(64)}`);
      write(path.join(home, "bin", "kuvo-agents.js"), old);
    },
  });
  await box.serving();
  const first = await box.bridgeUp();
  const made = await box.ask("POST", "kuvo/agents/api/session", {
    title: "Message across handover", location: { directory: box.home }, model: { providerID: "codex-cli", id: "gpt-test" },
  });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const route = `kuvo/sessions/${made.json.data.id}/send`;
  const send = () => box.ask("POST", route, { id, words: "send this once" }, { "x-kuvo-from": "mac" });
  const during = send();
  const rawID = "msg_handoverraw0001";
  let raw;
  let rawFinished = false;
  try {
    await box.until(() => fs.existsSync(marks.lookup), "the app to find the old bridge");
    // This is an established bridge. Its old PID file cannot stand in for a startup
    // timer once the bridge has explicitly refused a request for handover.
    const earlier = new Date(Date.now() - 120_000);
    fs.utimesSync(path.join(box.run, "agents.pid"), earlier, earlier);
    const responder = /return (answer\d*)\(request\)\.finally\(/;
    assert.ok(responder.test(agents));
    const next = agents.replace(responder, (_matched, answer) => `return (async () => {
  const prompt = request.method === "POST" && new URL(request.url).pathname.endsWith("/prompt") ? await request.clone().json() : null;
  const response = await ${answer}(request);
  if (prompt && response.ok) {
    const accepted = Bun.file(${JSON.stringify(marks.accepted)});
    await Bun.write(accepted, (await accepted.exists() ? await accepted.text() : "") + prompt.id + "\\n");
  }
  return response;
})().finally(`);
    const file = path.join(box.home, "bin", "kuvo-agents.js");
    write(file + ".new", next);
    fs.renameSync(file + ".new", file);
    const rejected = message => fs.existsSync(marks.refused) && fs.readFileSync(marks.refused, "utf8") === message;
    await box.until(() => rejected(id), "the retiring bridge to refuse the app's POST before execution");
    const refused = await during;
    assert.equal(refused.status, 503);
    assert.equal(refused.json?.reason, "agentsRestarting");
    assert.deepEqual(fs.readdirSync(path.join(box.run, "owned-data", "mutations")), [], "the owner released the pre-execution refusal");
    assert.equal(fs.existsSync(marks.accepted), false);
    raw = box.ask("POST", `kuvo/agents/api/session/${made.json.data.id}/prompt`, { id: rawID, text: "follow the replacement" }).finally(() => { rawFinished = true; });
    await box.until(() => rejected(rawID), "the raw prompt to hit the same pre-execution fence");
    await Promise.race([raw, new Promise(resolve => setTimeout(resolve, 400))]);
    assert.equal(rawFinished, false, "the fence is expected even though the old bridge's PID record predates this update");
  } finally {
    write(marks.fenced, "");
    write(marks.finish, "");
    await during;
  }
  assert.equal((await raw).status, 200, "the raw prompt follows the replacement on its first request");
  await box.bridgeUp(first);
  assert.equal((await send()).status, 200);
  assert.equal((await send()).status, 200, "the successful ID can be retried");
  assert.deepEqual(fs.readFileSync(marks.accepted, "utf8").trim().split("\n"), [rawID, id], "the replacement accepted each prompt once");
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


/** The release this checkout makes, of 0.10.0 for engine 1.0.0, signed with a key of the test's. */
function signedRelease({ version = "0.10.0", url = `https://example.invalid/releases/download/v${version}/box-${version}.tar.gz`, keyPair = crypto.generateKeyPairSync("ed25519") } = {}) {
  const scratch = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-release-")));
  removeAtEnd(scratch);
  const { privateKey, publicKey } = keyPair;
  const root = path.join(scratch, "sources");
  for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js", "box/dist/kuvo-agents.js"]) {
    write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
  }
  write(path.join(root, "box/dist/kuvo-link.js"), fs.readFileSync(builtLink));
  const integrity = "sha512-" + crypto.createHash("sha512").update("unused").digest("base64");
  const builds = { x86_64: { url: "https://example.invalid/engine-1.0.0.tgz", integrity }, aarch64: { url: "https://example.invalid/engine-1.0.0.tgz", integrity } };
  write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: "1.0.0", linux: builds, darwin: builds }));
  const out = path.join(scratch, "release");
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), version, url, out], {
    env: { ...process.env, BUN: bun, NODE: process.execPath, BOX_SOURCES: root, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }), KUVO_BOX_SPREADS: "1" }, encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);
  return { out, pinned: publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64") };
}
const release = bun ? signedRelease() : null;
// The coding agents' bridge as a Mac from before 1.0 put it there: it can't start under this keeper.
const OLDER_BRIDGE = "console.error('KUVO_AGENTS_HOME, KUVO_HOME, KUVO_AGENTS_PORT, KUVO_AGENTS_PASSWORD, KUVO_ENGINE_PORT and KUVO_ENGINE_PID are needed.'); process.exit(1)\n";

test("an older copy put over the coding agents' program is repaired from the release the box runs, and they come back", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-repair-", { release, before: home => write(path.join(home, "bin", "kuvo-agents.js"), OLDER_BRIDGE) });
  await box.serving();
  await box.bridgeUp();
  // (The release's own: the bundle as built, with the line a release marks it with.)
  assert.ok(fs.readFileSync(path.join(box.home, "bin", "kuvo-agents.js"), "utf8").startsWith(fs.readFileSync(builtAgents, "utf8")), "the release's own, back");
  const repaired = JSON.parse(fs.readFileSync(path.join(box.run, "repair.json"), "utf8"));
  assert.ok(["repaired", "sound"].includes(repaired.result) && repaired.version === "0.10.0", JSON.stringify(repaired));
});

test("an older Mac's tools put over a machine's while it runs are repaired once that Mac's visit is noted", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-older-mac-", { release, before: home => write(path.join(home, "bin", ".own", "kuvo-wake"), "# an older Mac's copy\n", 0o755) });
  await box.serving();
  await box.bridgeUp();
  const wake = path.join(box.home, "bin", "kuvo-wake");
  const own = fs.readFileSync(wake);
  // An older Kuvo Code on a Mac connects: it notes its visit and puts its own wake-ups there.
  write(wake, "# an older Mac's wake-ups\n", 0o644);
  write(path.join(box.run, "attached"), "", 0o600);
  await box.until(() => fs.readFileSync(wake).equals(own), "the box's own wake-ups back");
  assert.equal(fs.statSync(wake).mode & 0o777, 0o755, "a program again, as the release has it");
  assert.ok(!fs.existsSync(path.join(box.home, "bin", ".own")), "and no copy kept apart for an older Mac's app to put back");
});

test("a held release that isn't what its manifest says is never used, and goes; one of another version isn't used either", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-damaged-", { release, kept: false });
  const agents = path.join(box.home, "bin", "kuvo-agents.js");
  write(agents, OLDER_BRIDGE);
  // Another version than the one running: not this box's to repair from.
  fs.writeFileSync(path.join(box.home, "bin", "box-version"), "0.9.0");
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "none", why: "otherVersion" });
  fs.writeFileSync(path.join(box.home, "bin", "box-version"), "0.10.0");
  // Signed with a key the box doesn't know: not used.
  assert.deepEqual(box.repair(crypto.randomBytes(32).toString("base64")), { formatVersion: 1, result: "none", why: "noRelease" });
  assert.equal(fs.readFileSync(agents, "utf8"), OLDER_BRIDGE);
  // While a Mac puts its files in place, or a rollback runs: not now.
  write(path.join(box.run, "mac-installing"), "");
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "none", why: "busy" });
  fs.rmSync(path.join(box.run, "mac-installing"));
  fs.mkdirSync(path.join(box.run, ".rolling-back"));
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "none", why: "busy" });
  fs.rmSync(path.join(box.run, ".rolling-back"), { recursive: true });
  // Its pack damaged on disk.
  const held = path.join(box.home, "updates", "release");
  const pack = path.join(held, fs.readdirSync(held).find(name => name.endsWith(".tar.gz")));
  fs.appendFileSync(pack, "damage");
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "damaged", version: "0.10.0" });
  assert.equal(fs.readFileSync(agents, "utf8"), OLDER_BRIDGE, "nothing put back from it");
  assert.ok(!fs.existsSync(held), "and it's gone, never handed to another computer");
});

test("the release a box runs, kept apart once a newer one is held, is what it repairs from", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-repair-running-", { release, kept: false });
  fs.renameSync(path.join(box.home, "updates", "release"), path.join(box.home, "updates", "running"));
  const agents = path.join(box.home, "bin", "kuvo-agents.js");
  write(agents, OLDER_BRIDGE);
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "repaired", version: "0.10.0", files: ["bin/kuvo-agents.js"] });
  // And looked at again: all as the release says.
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "sound", version: "0.10.0" });
});

test("Try Again repairs the coding agents' program and starts them, where they'd stopped", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  // (Its keeper looks only every ten minutes: what brings them back is Try Again.)
  const box = machine("kuvo-repair-again-", { release, round: "600" });
  await box.serving();
  const first = await box.bridgeUp();
  // An older Kuvo puts its own over it, and the bridge stops (this test's own, by its pid).
  write(path.join(box.home, "bin", "kuvo-agents.js"), OLDER_BRIDGE);
  process.kill(first, "SIGTERM");
  await box.until(() => !runs(first), "the bridge to stop");
  const tried = await box.ask("POST", "kuvo/agents/start");
  assert.equal(tried.status, 200, JSON.stringify(tried.json));
  await box.bridgeUp(first);
  assert.ok(fs.readFileSync(path.join(box.home, "bin", "kuvo-agents.js"), "utf8").startsWith(fs.readFileSync(builtAgents, "utf8")));
});

test("nothing is repaired while an update is under way", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-repair-updating-", { release, kept: false });
  const agents = path.join(box.home, "bin", "kuvo-agents.js");
  write(agents, OLDER_BRIDGE);
  // An update's process at work (this test's, as `run/.updating/pid` names it).
  const updating = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)", "kuvo-link"], { stdio: "ignore" });
  atEnd(() => { updating.kill(); });
  write(path.join(box.run, ".updating", "pid"), String(updating.pid));
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "none", why: "busy" });
  assert.equal(fs.readFileSync(agents, "utf8"), OLDER_BRIDGE);
  updating.kill();
  await new Promise(resolve => updating.once("exit", resolve));
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "repaired", version: "0.10.0", files: ["bin/kuvo-agents.js"] });
});

/** A release feed of the test's own on 127.0.0.1: `/v<version>/boxes.json` and its pack, for each
 * release given; anything else is 404. */
async function feedOf() {
  const served = new Map();
  const server = http.createServer((request, response) => {
    const file = served.get(request.url);
    if (!file) { response.writeHead(404); return response.end(); }
    response.writeHead(200);
    response.end(fs.readFileSync(file));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  atEnd(() => new Promise(resolve => { server.closeAllConnections(); server.close(resolve); }));
  const base = `http://127.0.0.1:${server.address().port}`;
  return {
    base,
    /** A release of `version`, signed with `keyPair`, its pack where the feed serves it, served. */
    sign(version, keyPair) {
      const made = signedRelease({ version, keyPair });
      for (const name of fs.readdirSync(made.out)) served.set(`/v${version}/${name}`, path.join(made.out, name));
      return made;
    },
  };
}
const testKeys = bun ? crypto.generateKeyPairSync("ed25519") : null;
const feed = bun ? await feedOf() : null;
const served = bun ? feed.sign("0.10.0", testKeys) : null;

test("a box that keeps no release (an older updater kept none) keeps bin/.own until it has one: then it fetches the version it runs, repairs from it, and only then lets bin/.own go", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-no-release-", { release: served, held: false, kept: false, before: home => write(path.join(home, "bin", ".own", "kuvo-wake"), "# the box's own wake-ups\n", 0o755) });
  const agents = path.join(box.home, "bin", "kuvo-agents.js");
  write(agents, OLDER_BRIDGE);
  // No source: nothing to repair from, and what an older Kuvo kept stays, as what the keeper puts back.
  assert.deepEqual(await box.repairing(served.pinned, `${feed.base}/nothing`), { formatVersion: 1, result: "none", why: "noRelease" });
  assert.ok(fs.existsSync(path.join(box.home, "bin", ".own", "kuvo-wake")));
  assert.equal(fs.readFileSync(agents, "utf8"), OLDER_BRIDGE);
  // The feed has it: fetched, checked, kept apart, and repaired from.
  assert.deepEqual(await box.repairing(served.pinned, feed.base), { formatVersion: 1, result: "repaired", version: "0.10.0", files: ["bin/kuvo-agents.js"] });
  assert.ok(fs.existsSync(path.join(box.home, "updates", "running", "boxes.json")), "kept, to repair from again");
  assert.ok(!fs.existsSync(path.join(box.home, "bin", ".own")), "and only now, what an older Kuvo kept goes");
  // Its feed signed by another key: never used.
  fs.rmSync(path.join(box.home, "updates", "running"), { recursive: true });
  write(agents, OLDER_BRIDGE);
  assert.deepEqual(await box.repairing(crypto.randomBytes(32).toString("base64"), feed.base), { formatVersion: 1, result: "none", why: "noRelease" });
  assert.equal(fs.readFileSync(agents, "utf8"), OLDER_BRIDGE);
});

test("a box holding a newer release than the one it runs fetches the one it runs to repair from, never the newer", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const newer = feed.sign("0.11.0", testKeys);
  const box = machine("kuvo-other-version-", { release: served, held: false, kept: false });
  fs.mkdirSync(path.join(box.home, "updates", "release"), { recursive: true });
  for (const name of fs.readdirSync(newer.out)) fs.copyFileSync(path.join(newer.out, name), path.join(box.home, "updates", "release", name));
  write(path.join(box.home, "bin", "kuvo-agents.js"), OLDER_BRIDGE);
  assert.deepEqual(await box.repairing(served.pinned, `${feed.base}/nothing`), { formatVersion: 1, result: "none", why: "otherVersion" });
  assert.deepEqual(await box.repairing(served.pinned, feed.base), { formatVersion: 1, result: "repaired", version: "0.10.0", files: ["bin/kuvo-agents.js"] });
  assert.equal(fs.readFileSync(path.join(box.home, "bin", "box-version"), "utf8").trim(), "0.10.0", "still the version it ran");
});

test("a 0.7 Mac's tools put over a machine that keeps no release are put back: the feed's release once it has it", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-07-no-release-", { release: served, held: false, feed: feed.base });
  await box.serving();
  await box.bridgeUp();
  const wake = path.join(box.home, "bin", "kuvo-wake");
  const own = fs.readFileSync(wake);
  write(wake, "# a 0.7 Mac's wake-ups\n", 0o644);
  write(path.join(box.run, "attached"), "", 0o600);
  await box.until(() => fs.readFileSync(wake).equals(own), "the release's wake-ups back");
  assert.equal(fs.statSync(wake).mode & 0o777, 0o755);
  assert.ok(fs.existsSync(path.join(box.home, "updates", "running", "boxes.json")), "the version it runs, fetched to repair from");
});

test("the keeper's script with another engine line (a rollback that kept a newer engine wrote it) is as the release says, and stays so", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const box = machine("kuvo-keeper-line-", { release, kept: false });
  const keeper = path.join(box.home, "bin", "kuvo-start");
  write(keeper, fs.readFileSync(keeper, "utf8").replace(/^V=.*$/m, "V=1.1.0"), 0o700);
  const before = fs.readFileSync(keeper);
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "sound", version: "0.10.0" });
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "sound", version: "0.10.0" }, "every look");
  assert.ok(fs.readFileSync(keeper).equals(before), "its engine line as it was");
  // Something else changed in it: put back, with its engine line.
  write(keeper, before.toString("utf8") + "# changed\n", 0o700);
  assert.deepEqual(box.repair(), { formatVersion: 1, result: "repaired", version: "0.10.0", files: ["bin/kuvo-start"] });
  assert.ok(fs.readFileSync(keeper).equals(before));
});
