// A new engine takes the old one's place without cutting a turn (PROTOCOL.md, "A new engine"): a box
// takes a release with another engine whose code was checked with the engine at work, and its code at
// once; the engine changes once the turns on it end, what's sent meanwhile held on the box, shown
// waiting with a line, and sent to the new engine once it answers, each once and in order. A coding
// agent's turn on the bridge runs through it untouched. A new engine that doesn't start goes back to
// the one before, which gets what was held; a hold the turns don't end within its bound is let go of,
// what it held going to the engine at work.
//
// Everything is the real thing: the keeper (`kuvo-start keep`, `kuvo-launch`), the link and the
// bridge as built, the swapper they start, releases packed by scripts/box-release.sh and taken by
// the link. Stand-ins only for the engines (HTTP servers that note each prompt with their version,
// and whose own turn runs while the test says) and for `codex app-server`. In a home of the test's
// own; the same on macOS and Linux. Run with `node --test box/bridge-tests/engine-swap.test.js`.
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
import { CODEX_STAND_IN } from "./codex-stand-in.js";

const bun = testBun();
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const builtLink = path.join(checkout, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const builtAgents = path.join(checkout, "box/dist/kuvo-agents.js");
const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-engine-swap-")));
const home = path.join(base, ".kuvo-code");
const scratch = path.join(base, "scratch");
removeAtEnd(base);
Object.assign(process.env, { HOME: base, KUVO_HOME: home, TMPDIR: `${base}/` });
const link = await import(builtLink);

function write(file, text, mode = 0o644) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode });
  fs.chmodSync(file, mode);
}
const readJSON = file => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; } };
const runs = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
const RUN = path.join(home, "run");
async function until(check, what, within = 120_000) {
  for (const end = Date.now() + within; Date.now() < end;) {
    const found = await check();
    if (found) return found;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  let log = "";
  for (const file of ["engine-swap.log", "engine.log"]) try { log += `${file}: ${fs.readFileSync(path.join(RUN, file), "utf8").slice(-2000)}\n`; } catch {}
  let keeper = "";
  try { const pid = Number(fs.readFileSync(path.join(RUN, "box.pid"), "utf8")); keeper = `keeper ${pid} ${runs(pid) ? "runs" : "gone"}`; } catch {}
  const procs = spawnSync("ps", ["-axww", "-o", "pid,ppid,etime,command"], { encoding: "utf8" }).stdout.split("\n").filter(line => line.includes(base)).join("\n");
  keeper += `\n${procs}`;
  let files = "";
  try { files = fs.readdirSync(RUN).join(" "); } catch {}
  assert.fail(`never: ${what}\n${log}\n${JSON.stringify(readJSON(path.join(RUN, "engine-swap.json")))}\n${keeper}\n${files}\nengine-ran ${fs.existsSync(path.join(RUN, "engine-ran")) ? fs.readFileSync(path.join(RUN, "engine-ran"), "utf8") : "-"} engine ${JSON.stringify(readJSON(path.join(RUN, "engine.json"))?.version)}`);
}

// The engine: one conversation (`ses_engine`), whose own turn runs while `engine-busy` is there; each
// prompt it gets noted in `prompts.log` with the engine's version; its feed stays open.
const ENGINE = `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
const version = process.env.STAND_IN_VERSION;
const scratch = ${JSON.stringify(scratch)};
const inbox = [];
const read = request => new Promise(resolve => { let body = ""; request.on("data", chunk => { body += chunk; }); request.on("end", () => resolve(body)); });
http.createServer(async (request, response) => {
  const url = new URL(request.url, "http://x");
  if (url.pathname.startsWith("/api/event") || url.pathname.startsWith("/event")) { response.writeHead(200, { "content-type": "text/event-stream" }); response.write(": open\\n\\n"); return; }
  const answer = (status, body) => { response.writeHead(status, { "content-type": "application/json" }); response.end(JSON.stringify(body)); };
  // (A test's: a message for after the run, in this engine's inbox, kept only while it runs.)
  if (url.pathname === "/test/inbox" && request.method === "POST") { inbox.push(JSON.parse(await read(request))); return answer(200, {}); }
  if (url.pathname === "/api/session/ses_engine/inbox") return answer(200, { data: inbox.map(each => ({ id: each.id, delivery: "queue", type: "user", payload: { text: each.text } })) });
  const gone = /^\\/api\\/session\\/ses_engine\\/inbox\\/([^/]+)$/.exec(url.pathname);
  if (gone && request.method === "DELETE") { const at = inbox.findIndex(each => each.id === decodeURIComponent(gone[1])); if (at < 0) return answer(404, {}); inbox.splice(at, 1); return answer(200, {}); }
  if (url.pathname === "/api/session/active") return answer(200, { data: fs.existsSync(scratch + "/engine-busy") ? { ses_engine: { type: "busy" } } : {} });
  if (url.pathname === "/api/session") return answer(200, { data: [{ id: "ses_engine", directory: ${JSON.stringify(home)}, time: { updated: Date.now() } }] });
  const one = /^\\/api\\/session\\/([^/]+)(\\/.*)?$/.exec(url.pathname);
  if (one && one[1] !== "ses_engine") return answer(404, { message: "no such session" });
  if (one && !one[2]) return answer(200, { data: { id: "ses_engine", directory: ${JSON.stringify(home)}, time: { updated: Date.now() } } });
  if (one && one[2] === "/prompt" && request.method === "POST") {
    const body = JSON.parse(await read(request));
    fs.appendFileSync(scratch + "/prompts.log", JSON.stringify({ engine: version, id: body.id, text: body.text, delivery: body.delivery }) + "\\n");
    return answer(200, { data: { id: body.id } });
  }
  answer(200, { data: [] });
}).listen(port, "127.0.0.1", () => console.log("server listening on http://127.0.0.1:" + port));
setInterval(() => { if (!fs.existsSync(${JSON.stringify(home)})) process.exit(0); }, 300);
`;

/** An engine of `version` in its folder: the stand-in, Bun for the box's scripts beside it (the
 * bridge and link run on the engine's own program); `starts` false for one whose server never does. */
function engine(version, starts = true, migrates = false) {
  const folder = path.join(home, "engine", version);
  fs.mkdirSync(folder, { recursive: true });
  const program = bunRunsAs(bun);
  try { fs.linkSync(program, path.join(folder, "bun")); } catch { fs.copyFileSync(program, path.join(folder, "bun")); fs.chmodSync(path.join(folder, "bun"), 0o755); }
  write(path.join(folder, "opencode"), `#!/bin/sh
[ "$1" = --config=/dev/null ] && shift
[ "$1" = --no-env-file ] && shift
case "$1" in
--version) echo ${version} ;;
*.js) case "$2" in browser) exit 0 ;; esac; BUN_BE_BUN=1 exec "${folder}/bun" --config=/dev/null --no-env-file "$@" ;;
serve) ${starts ? `trap 'kill "$c" 2>/dev/null' TERM INT HUP; STAND_IN_VERSION=${version} '${process.execPath}' '${path.join(scratch, "engine.mjs")}' "$@" & c=$!; wait "$c"` : `${migrates ? `mkdir -p '${home}/data/opencode'; echo 'migrated by ${version}' >>'${home}/data/opencode/opencode.db'; ` : ""}echo 'this engine never starts' >&2; exit 1`} ;;
esac
`, 0o755);
}

/** A box on 0.9.0, its engine 1.0.0, as a machine's keeper runs it, its server for clients wanted,
 * its code (link, bridge) as this checkout builds it: what an update brings is its engine. */
function box() {
  write(path.join(scratch, "engine.mjs"), ENGINE);
  engine("1.0.0");
  for (const name of ["kuvo-start", "kuvo-launch", "kuvo-setsid", "kuvo-wake", "kuvo-npx"]) {
    write(path.join(home, "bin", name), fs.readFileSync(path.join(checkout, "box/src/keeper", name), "utf8").replaceAll("@ENGINE@", "1.0.0"), 0o700);
  }
  write(path.join(home, "bin", "kuvo-link.js"), fs.readFileSync(builtLink, "utf8"), 0o600);
  write(path.join(home, "bin", "kuvo-agents.js"), fs.readFileSync(builtAgents, "utf8"));
  write(path.join(home, "bin", "box-version"), "0.9.0");
  write(path.join(home, "box", "kind"), "machine\n");
  for (const tool of ["pkill", "caffeinate", "crontab", "launchctl"]) write(path.join(base, "tools", tool), `#!/bin/sh\nexit 0\n`, 0o755);
  const fake = path.join(scratch, "fake");
  write(path.join(fake, "app-server.js"), CODEX_STAND_IN);
  write(path.join(fake, "codex"), `#!/bin/sh\nexport BUN_BE_BUN=1\n[ "$1" = "--version" ] && { echo "codex-cli 0.166.0"; exit 0; }\nexec "${bun}" "${path.join(fake, "app-server.js")}" "${fake}"\n`, 0o755);
  timing({ holdMost: 300_000 });
  write(path.join(RUN, "launch.notes"), "{}", 0o600);
  write(path.join(RUN, "serve.needed"), "", 0o600);
  return { fake };
}

/** How its engine starts, and how the swapper goes here: it looks often, holds at once (whatever
 * is at work, rather than after a day), and lets go of a hold after `holdMost`. */
function timing({ holdMost }) {
  const fake = path.join(scratch, "fake");
  write(path.join(RUN, "launch.env"), [`PATH=${fake}:${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, "LANG=C", "KUVO_SANDBOX=off", "KUVO_TEST_RENEW_SETTLE=0", "KUVO_TEST_RENEW_UPTIME=0", "KUVO_TEST_CODE_EVERY=200", "KUVO_TEST_NO_PEERS=1", "KUVO_TEST_HEALTH_WINDOW=1500", "KUVO_TEST_HEALTH_EVERY=200",
    "KUVO_TEST_SWAP_EVERY=200", "KUVO_TEST_SWAP_QUIET=600", "KUVO_TEST_SWAP_LONGEST=0", "KUVO_TEST_SWAP_PATIENCE=600000", `KUVO_TEST_SWAP_HOLD_MOST=${holdMost}`, "KUVO_TEST_SWAP_FAILING_FOR=1000", "KUVO_TEST_SWAP_START_MOST=20000"].join("\n") + "\n", 0o600);
}

function keep() {
  const keeper = spawn("sh", [path.join(home, "bin", "kuvo-start"), "keep"], { env: { HOME: base, PATH: `${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, KUVO_TEST_KEEP_ROUND: "1" }, detached: true, stdio: "ignore" });
  const gone = new Promise(resolve => keeper.once("exit", resolve));
  atEnd(async () => {
    spawnSync("sh", [path.join(home, "bin", "kuvo-start"), "off"], { env: { HOME: base, PATH: "/usr/bin:/bin" }, timeout: 60_000 });
    try { process.kill(-keeper.pid, "SIGKILL"); } catch {}
    await gone;
  });
}

/** Release `version` of this checkout's code for engine `to`, checked with `from` (its `engines`),
 * the engine before opening what it writes; signed with a key of the test's, in a folder as an app
 * carries it. */
function pack(version, to, from, downgradeSafe = true) {
  const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
  const root = path.join(scratch, `sources-${version}`);
  for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js", "box/dist/kuvo-agents.js"]) {
    write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
  }
  write(path.join(root, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"), fs.readFileSync(builtLink));
  const integrity = "sha512-" + crypto.createHash("sha512").update("unused").digest("base64");
  const builds = engine => ({ x86_64: { url: `https://example.invalid/engine-${engine}.tgz`, integrity }, aarch64: { url: `https://example.invalid/engine-${engine}.tgz`, integrity } });
  write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: to, linux: builds(to), darwin: builds(to), previous: { version: from, linux: builds(from), darwin: builds(from) } }));
  const check = path.join(scratch, `check-${version}.json`);
  write(check, JSON.stringify({ engines: [to, from], downgradeSafe }));
  const out = path.join(scratch, `release-${version}`);
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), version, `https://example.invalid/releases/download/v${version}/box-${version}.tar.gz`, out], {
    env: { ...process.env, BUN: bun, NODE: process.execPath, BOX_SOURCES: root, ENGINE_CHECK: check, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }) }, encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);
  return { out, pinned: publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64") };
}

/** Taken as the box takes a release an app offers: at once, though something works (its code stops
 * nothing), and judged well. */
async function take(version, to, from, downgradeSafe = true) {
  const { out, pinned } = pack(version, to, from, downgradeSafe);
  assert.deepEqual(link.offerUpdate(out, { keys: [pinned], blocked: null }), { status: 202, version });
  assert.deepEqual(await link.takeOffer({ keys: [pinned], blocked: null }), { result: "updated", version });
  await until(() => !fs.existsSync(path.join(RUN, "update-trial")), `${version} judged`);
  assert.equal(readJSON(path.join(RUN, "update.json")).last?.to, version, JSON.stringify(readJSON(path.join(RUN, "update.json"))));
}

const engineNow = () => { const said = readJSON(path.join(RUN, "engine.json")); return said?.pid && runs(said.pid) ? said : null; };
const prompts = () => { try { return fs.readFileSync(path.join(scratch, "prompts.log"), "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); } catch { return []; } };
const swap = () => readJSON(path.join(RUN, "engine-swap.json"));
const busy = on => (on ? write(path.join(scratch, "engine-busy"), "") : fs.rmSync(path.join(scratch, "engine-busy"), { force: true }));

/** The box's server for clients, as a client reaches it. */
async function served(method, route, body) {
  const said = await until(async () => {
    const record = readJSON(path.join(RUN, "serve.json"));
    if (!record?.pid || !runs(record.pid)) return null;
    const answer = await fetch(`http://127.0.0.1:${record.port}/kuvo/info`, { headers: { authorization: "Basic " + Buffer.from(`opencode:${record.password}`).toString("base64") } }).catch(() => null);
    return answer?.status === 200 ? record : null;
  }, "the server for clients to answer");
  const response = await fetch(`http://127.0.0.1:${said.port}/${route}`, { method, headers: { authorization: "Basic " + Buffer.from(`opencode:${said.password}`).toString("base64"), "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
  const text = await response.text();
  return { status: response.status, json: text ? JSON.parse(text) : null };
}
const send = (id, words) => served("POST", "kuvo/sessions/ses_engine/send", { id, words, delivery: "steer" });

/** The bridge, as clients reach it. */
async function bridgeAPI(method, route, body) {
  const said = readJSON(path.join(RUN, "bridge.json"));
  const response = await fetch(`http://127.0.0.1:${said.port}/${route}`, { method, headers: { authorization: "Basic " + Buffer.from(`opencode:${said.password}`).toString("base64"), "content-type": "application/json", "x-opencode-directory": encodeURIComponent(home) }, body: body ? JSON.stringify(body) : undefined }).catch(() => null);
  const text = response ? await response.text() : "";
  return { status: response?.status ?? 0, json: text ? JSON.parse(text) : null };
}
const turns = fake => { try { return fs.readFileSync(path.join(fake, "turns.log"), "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); } catch { return []; } };

test("an engine change waits for the turns on it, holds what's sent meanwhile on the box, and sends it to the new engine once, in order; a coding agent's turn runs through it", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const { fake } = box();
  keep();
  const old = await until(engineNow, "the engine to run");
  assert.equal(old.version, "1.0.0");
  const bridge = await until(async () => { const said = readJSON(path.join(RUN, "bridge.json")); return said?.pid && runs(said.pid) && (await bridgeAPI("GET", "api/info")).status === 200 ? said : null; }, "the bridge to answer");

  // A coding agent's turn under way on the bridge, and the engine's own turn.
  const made = await bridgeAPI("POST", "api/session", { title: "Long one", location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const agentSession = made.json.data.id;
  assert.equal((await bridgeAPI("POST", `api/session/${agentSession}/prompt`, { id: "msg_swapagent0000001", delivery: "steer", text: "hold one" })).status, 200);
  await until(() => turns(fake).length === 1, "the agent's turn to start");
  busy(true);

  // The release, with engine 1.1.0 (already here): its code taken at once, the engine left at work.
  engine("1.1.0");
  await take("0.10.0", "1.1.0", "1.0.0");
  assert.equal(engineNow().pid, old.pid, "the engine at work was never stopped for the code");
  assert.equal(readJSON(path.join(home, "updates", "installed.json")).downgradeSafe, true);

  // A message for after the run, in the old engine's inbox (which goes with it).
  assert.equal((await fetch(`http://127.0.0.1:${old.port}/test/inbox`, { method: "POST", body: JSON.stringify({ id: "msg_swapinbox000001", text: "queued before" }) })).status, 200);
  // The swapper holds (at once, here), waiting for the engine's turn to end: what's sent waits on the box.
  await until(() => swap()?.stage === "holding", "the swapper to hold");
  for (const [id, words] of [["msg_swapfirst000001", "first"], ["msg_swapsecond00001", "second"]]) {
    const sent = await send(id, words);
    assert.equal(sent.status, 200, JSON.stringify(sent.json));
    assert.deepEqual(sent.json, { waits: true }, "held on the box, not sent to the engine stopped next");
  }
  const queue = await served("GET", "kuvo/sessions/ses_engine/queue?items=1");
  assert.equal(queue.json.paused, false);
  assert.deepEqual(queue.json.items.map(item => [item.words, item.on]), [["queued before", "engine"], ["first", "box"], ["second", "box"]], "shown waiting, in order");
  assert.match(queue.json.line, /engine is updating/, "with a line saying why");
  assert.match((await served("GET", "kuvo/update")).json.engineChange.line, /engine is updating/);
  assert.deepEqual(prompts(), [], "nothing reached the old engine");
  assert.equal(engineNow().pid, old.pid, "its turn isn't cut");

  // The engine's turn ends: the new engine takes its place, and what was held goes to it, each once, in order.
  busy(false);
  const fresh = await until(() => { const now = engineNow(); return now?.version === "1.1.0" ? now : null; }, "the new engine to run");
  assert.ok(!runs(old.pid), "the old engine stopped");
  await until(() => prompts().length >= 3, "what was held to reach the new engine");
  await until(() => !fs.existsSync(path.join(RUN, "engine-swap.json")), "the swap to end");
  const said = text => (text.includes("first") ? "first" : text.includes("second") ? "second" : text);
  assert.deepEqual(prompts().map(each => [each.engine, each.id, said(each.text)]), [["1.1.0", "msg_swapinbox000001", "queued before"], ["1.1.0", "msg_swapfirst000001", "first"], ["1.1.0", "msg_swapsecond00001", "second"]],
    "the old engine's inbox moved onto the box first, then what was sent during the hold: each once, in order, on the new engine");
  assert.equal(prompts()[0].delivery, "steer", "the first starts a turn");
  assert.equal(readJSON(path.join(RUN, "update.json")).engine?.result, "changed");
  assert.ok(fs.readdirSync(path.join(home, "updates", "backups")).some(name => name.endsWith("1.0.0-to-1.1.0")), "a copy of its data kept");
  assert.ok(fs.existsSync(path.join(home, "engine", "1.0.0", "opencode")), "the engine before kept, to go back to");
  // Sent now, it goes straight to the new engine.
  assert.deepEqual((await send("msg_swapthird000001", "third")).json?.waits ?? false, false);
  await until(() => prompts().length === 4, "a message sent after to go at once");
  assert.equal(prompts()[3].engine, "1.1.0");

  // The coding agent's turn ran through it, on the bridge it began on, which the swap never ended.
  assert.ok(runs(bridge.pid), "the bridge runs on");
  assert.equal(readJSON(path.join(RUN, "bridge.json")).pid, bridge.pid);
  write(path.join(fake, "release-one"), "");
  const messages = await until(async () => {
    const page = (await bridgeAPI("GET", `api/session/${agentSession}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the agent's turn to end");
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"], "never cut");
  assert.deepEqual(turns(fake), [{ text: "hold one", bridge: bridge.pid }]);
  assert.equal(fresh.version, engineNow().version);
});

test("a new engine that doesn't start goes back to the one before, which gets what was held, once", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const before = await until(engineNow, "the engine to run");
  assert.equal(before.version, "1.1.0");
  const sent = prompts().length;
  busy(true);
  engine("1.2.0", false);
  await take("0.11.0", "1.2.0", "1.1.0");
  await until(() => swap()?.stage === "holding" && swap().to === "1.2.0", "the swapper to hold");
  assert.deepEqual((await send("msg_swapback0000001", "while it fails")).json, { waits: true });
  busy(false);

  await until(() => readJSON(path.join(RUN, "update.json")).engine?.to === "1.2.0", "the swap to end", 180_000);
  const last = readJSON(path.join(RUN, "update.json")).engine;
  assert.equal(last.result, "back", JSON.stringify(last));
  assert.match(last.message, /didn’t start, so Kuvo went back/);
  const after = await until(engineNow, "the engine before to run again");
  assert.equal(after.version, "1.1.0");
  await until(() => prompts().length === sent + 1, "what was held to reach the engine before");
  assert.deepEqual(prompts().slice(sent).map(each => [each.engine, each.id]), [["1.1.0", "msg_swapback0000001"]], "once, on the engine before");
  assert.equal(fs.readFileSync(path.join(RUN, "engine-back"), "utf8").split(" ").slice(0, 2).join(" "), "1.2.0 1.1.0", "the new engine rests");
  assert.equal(swap(), null, "the swapper done");
  assert.equal(readJSON(path.join(RUN, "update.json")).last?.to, "0.11.0", "the code stays");
});

test("a hold the turns don't end within its bound is let go of: what it held goes to the engine at work, and nothing is left waiting", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const at = await until(engineNow, "the engine to run");
  timing({ holdMost: 3000 });
  const sent = prompts().length;
  busy(true);
  engine("1.3.0");
  await take("0.12.0", "1.3.0", "1.1.0");
  await until(() => swap()?.stage === "holding" && swap().to === "1.3.0", "the swapper to hold");
  assert.deepEqual((await send("msg_swaplong0000001", "while it holds")).json, { waits: true });

  // The engine's turn never ends: the hold is let go of, in a plain state.
  await until(() => swap()?.stage === "waiting" && swap().letGo, "the hold let go of");
  await until(() => prompts().length === sent + 1, "what was held to reach the engine at work");
  assert.deepEqual(prompts().slice(sent).map(each => [each.engine, each.id]), [["1.1.0", "msg_swaplong0000001"]]);
  assert.equal(engineNow().pid, at.pid, "the turn at work isn't cut");
  const queue = await served("GET", "kuvo/sessions/ses_engine/queue?items=1");
  assert.deepEqual(queue.json.items, [], "nothing left waiting");
  assert.equal(queue.json.line, undefined);
  assert.match((await served("GET", "kuvo/update")).json.engineChange.line, /ready\. It installs once nothing is running/);
  // What's sent now goes at once: nothing is held.
  assert.equal((await send("msg_swaplong0000002", "after")).json?.waits ?? false, false);
  await until(() => prompts().length === sent + 2, "a message sent after to go at once");
  // The engine ends meanwhile (a crash): the keeper starts the one that ran, never the new one on
  // its own (that one's start is the swapper's, its data copied first).
  const now = engineNow();
  process.kill(now.pid, "SIGKILL");
  const again = await until(() => { const record = engineNow(); return record && record.pid !== now.pid ? record : null; }, "the keeper to start the engine again");
  assert.equal(again.version, "1.1.0");
  busy(false);
  await until(() => engineNow()?.version === "1.3.0", "the new engine at the next quiet moment");
  assert.ok(fs.readdirSync(path.join(home, "updates", "backups")).some(name => name.endsWith("1.1.0-to-1.3.0")), "its data copied first");
});

test("a new engine that doesn't start, where the one before can't open what it wrote, stays: the keeper starts it again, never the one before on its data", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const before = await until(engineNow, "the engine to run");
  assert.equal(before.version, "1.3.0");
  timing({ holdMost: 300_000 });
  // (One that begins its migration before it fails: what it wrote, the one before can't open.)
  engine("1.4.0", false, true);
  await take("0.13.0", "1.4.0", "1.3.0", false);
  await until(() => readJSON(path.join(RUN, "update.json")).engine?.to === "1.4.0", "the swap to end", 180_000);
  const last = readJSON(path.join(RUN, "update.json")).engine;
  assert.equal(last.result, "failed", JSON.stringify(last));
  assert.match(last.message, /can’t open what it wrote/);
  assert.equal(fs.readFileSync(path.join(RUN, "engine-ran"), "utf8").trim(), "1.4.0", "the engine to run from now on is the new one");
  assert.ok(!fs.existsSync(path.join(RUN, "engine-back")), "nothing to go back to");
  // The keeper tries the new one again (as one of its own failing), never the one before.
  const record = readJSON(path.join(RUN, "engine.json"));
  assert.ok(!record?.pid || !runs(record.pid) || record.version !== "1.3.0", JSON.stringify(record));
  assert.ok(!runs(before.pid), "the engine before stays stopped");
});
