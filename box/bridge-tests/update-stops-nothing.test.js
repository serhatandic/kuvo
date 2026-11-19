// An update stops nothing (PROTOCOL.md, "Updates"): a box takes a release while a coding agent's turn
// and an engine's turn both run, and both finish; the new code is live afterwards in its server for
// clients (`serve --kept`), its link (`run`) and its coding agents' bridge, each having taken it by
// itself. The bridge switches while the Codex turn still has an open approval. The new bridge
// shows the same approval, continues that turn once, and accepts a new turn while the engine is
// still busy. Then the update is judged well.
//
// Everything is the real thing: the keeper (`kuvo-start keep`, `kuvo-launch`), the link and the
// bridge as built, the release packed by scripts/box-release.sh and taken by the link. Stand-ins
// only for the engine (an HTTP server whose own turn runs while the test says) and for `codex
// app-server` (its turn holds until let go). In a home of the test's own; the same on macOS and
// Linux. Run with `node --test box/bridge-tests/update-stops-nothing.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { NO_BUN, bunRunsAs, testBun } from "./bun-for-tests.js";
import { CODEX_STAND_IN } from "./codex-stand-in.js";
import { peerHolding } from "./peer-release.js";

const bun = testBun();
const checkout = fileURLToPath(new URL("../..", import.meta.url));
const builtLink = path.join(checkout, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const builtAgents = path.join(checkout, "box/dist/kuvo-agents.js");
const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "k774-")));
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
  // (What the bridge said, for what didn't come.)
  let log = "";
  try { log = fs.readFileSync(path.join(RUN, "agents.log"), "utf8").slice(-3000); } catch {}
  assert.fail(`never: ${what}\n${log}`);
}
const freePort = () => new Promise(resolve => {
  const server = net.createServer();
  server.listen(0, "127.0.0.1", () => { const { port } = server.address(); server.close(() => resolve(port)); });
});

/** The bridge's build as its bundle names it, another one for the version before. */
const build = text => [...text.matchAll(/\/\/ Built from Packages\/Box\/src\/agents ([0-9a-f]{16,64})/g)].at(-1)?.[1];
const OLD_BUILD = "a".repeat(64);

/** A box on 0.9.0, its engine 1.0.0, as a machine's keeper runs it: its link wanted (a phone
 * paired, its relay out of reach) and its server for clients wanted. Its bridge and link a build
 * before the one this checkout makes. */
function box() {
  // The engine: its own turn runs while `engine-busy` is there; its feed stays open.
  const engine = path.join(scratch, "engine.mjs");
  write(engine, `
import fs from "node:fs"; import http from "node:http";
const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
const busy = ${JSON.stringify(path.join(scratch, "engine-busy"))};
http.createServer((request, response) => {
  if (request.url.startsWith("/api/event") || request.url.startsWith("/event")) { fs.writeFileSync(${JSON.stringify(scratch)} + "/feed-" + port, ""); response.writeHead(200, { "content-type": "text/event-stream" }); response.write(": open\\n\\n"); return; }
  response.writeHead(200, { "content-type": "application/json" });
  if (request.url.startsWith("/api/session/active")) return response.end(JSON.stringify({ data: fs.existsSync(busy) ? { ses_engine: { type: "busy" } } : {} }));
  response.end(JSON.stringify({ data: [] }));
}).listen(port, "127.0.0.1", () => console.log("server listening on http://127.0.0.1:" + port));
setInterval(() => { if (!fs.existsSync(${JSON.stringify(home)})) process.exit(0); }, 300);
`);
  // Its program: the engine, and Bun for the box's scripts, from the engine's own folder (as the
  // bridge sees it runs on the engine its keeper starts). Never the browser's setup, which would
  // download one.
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
serve) trap 'kill "$c" 2>/dev/null' TERM INT HUP; '${process.execPath}' '${engine}' "$@" & c=$!; wait "$c" ;;
esac
`, 0o755);
  // The keeper's files as this checkout has them; the link and bridge as the version before.
  for (const name of ["kuvo-start", "kuvo-launch", "kuvo-setsid", "kuvo-wake", "kuvo-npx"]) {
    write(path.join(home, "bin", name), fs.readFileSync(path.join(checkout, "box/src/keeper", name), "utf8").replaceAll("@ENGINE@", "1.0.0"), 0o700);
  }
  write(path.join(home, "bin", "kuvo-link.js"), fs.readFileSync(builtLink, "utf8") + "\n// the version before\n", 0o600);
  const agents = fs.readFileSync(builtAgents, "utf8");
  write(path.join(home, "bin", "kuvo-agents.js"), agents.replace(`// Built from Packages/Box/src/agents ${build(agents)}`, `// Built from Packages/Box/src/agents ${OLD_BUILD}`));
  write(path.join(home, "bin", "box-version"), "0.9.0");
  write(path.join(home, "box", "kind"), "machine\n");
  // What only a Mac's keeper loop reaches for (keeping it awake, older Macs' leftovers): noted only.
  for (const tool of ["pkill", "caffeinate", "crontab", "launchctl"]) write(path.join(base, "tools", tool), `#!/bin/sh\nexit 0\n`, 0o755);
  // The coding agent, on the PATH its engine starts with.
  const fake = path.join(scratch, "fake");
  // The first turn waits both for an approval and for an independent completion signal.
  const asking = CODEX_STAND_IN
    .replace('"t" + ++thread', '"t" + process.pid + "_" + ++thread')
    .replace('async function turn(', 'let approval;\nasync function turn(')
    .replace('  if (/hold/.test(text)) {', `  if (text === "hold one" || text === "hold recovery") {
    const id = 701;
    send({ method: "item/completed", params: { threadId, turnId, item: { type: "agentMessage", id: "before" + turnId, text: "Before the update." } } });
    await new Promise(resolve => {
      approval = resolve;
      send({ id, method: "item/commandExecution/requestApproval", params: { threadId, turnId, itemId: "approval" + turnId, command: "echo held" } });
    });
  }
  if (/hold/.test(text)) {`)
    .replace('    if (message.method === "thread/start")', `    if (message.id === 701 && !message.method && approval) {
      fs.appendFileSync(path.join(folder, "approvals.log"), JSON.stringify({ pid: process.pid, decision: message.result?.decision }) + "\\n");
      approval();
      approval = undefined;
    } else if (message.method === "thread/start")`);
  write(path.join(fake, "app-server.js"), asking);
  write(path.join(fake, "codex"), `#!/bin/sh\nexport BUN_BE_BUN=1\n[ "$1" = "--version" ] && { echo "codex-cli 0.166.0"; exit 0; }\nexec "${bun}" "${path.join(fake, "app-server.js")}" "${fake}"\n`, 0o755);
  // How its engine starts (the keeper's `run/launch.env`): with a bridge that makes way at once
  // once nothing runs, and processes that look for new code often.
  write(path.join(RUN, "launch.env"), [`PATH=${fake}:${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, "LANG=C", "KUVO_SANDBOX=off", "KUVO_TEST_RENEW_SETTLE=0", "KUVO_TEST_RENEW_UPTIME=0", "KUVO_TEST_CODE_EVERY=200", "KUVO_TEST_NO_PEERS=1", "KUVO_TEST_HEALTH_WINDOW=1500", "KUVO_TEST_HEALTH_EVERY=200"].join("\n") + "\n", 0o600);
  write(path.join(RUN, "launch.notes"), "{}", 0o600);
  const key = () => crypto.randomBytes(32).toString("base64url");
  write(path.join(RUN, "link.json"), JSON.stringify({ id: "lab", name: "lab", relay: "ws://127.0.0.1:9", secret: key(), key: key(), phones: [{ key: key(), name: "iPhone", added: Math.floor(Date.now() / 1000), from: "pair" }] }), 0o600);
  write(path.join(RUN, "link.needed"), "", 0o600);
  write(path.join(RUN, "serve.needed"), "", 0o600);
  return { fake };
}

/** The keeper's loop, as at boot: a round a second here. Ended with the box (`kuvo-start off`). */
function keep() {
  const keeper = spawn("sh", [path.join(home, "bin", "kuvo-start"), "keep"], { env: { HOME: base, PATH: `${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, KUVO_TEST_KEEP_ROUND: "1" }, detached: true, stdio: "ignore" });
  const gone = new Promise(resolve => keeper.once("exit", resolve));
  atEnd(async () => {
    spawnSync("sh", [path.join(home, "bin", "kuvo-start"), "off"], { env: { HOME: base, PATH: "/usr/bin:/bin" }, timeout: 60_000 });
    try { process.kill(-keeper.pid, "SIGKILL"); } catch {}
    await gone;
  });
}

/** Package a release signed by the test, in a folder as an app carries it. */
function pack(version = "0.10.0", engine = "1.0.0", agentsBuild) {
  const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
  const root = path.join(scratch, "sources");
  for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js", "box/dist/kuvo-agents.js"]) {
    write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
  }
  if (agentsBuild) {
    const agents = fs.readFileSync(builtAgents, "utf8");
    write(path.join(root, "box/dist/kuvo-agents.js"), agents.replace(build(agents), agentsBuild));
  }
  write(path.join(root, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"), fs.readFileSync(builtLink));
  const integrity = "sha512-" + crypto.createHash("sha512").update("unused").digest("base64");
  const url = `https://example.invalid/engine-${engine}.tgz`;
  const builds = { x86_64: { url, integrity }, aarch64: { url, integrity } };
  write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: engine, linux: builds, darwin: builds }));
  const out = path.join(scratch, `release-${version}`);
  const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), version, `https://example.invalid/releases/download/v${version}/box-${version}.tar.gz`, out], {
    env: { ...process.env, BUN: bun, NODE: process.execPath, BOX_SOURCES: root, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }), KUVO_BOX_SPREADS: "1" }, encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);
  return { out, pinned: publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64") };
}

/** The bridge, as clients reach it (`run/bridge.json`). */
function bridgeAPI() {
  const said = readJSON(path.join(RUN, "bridge.json"));
  const api = async (method, route, body) => {
    try {
      const response = await fetch(`http://127.0.0.1:${said.port}/${route}`, { method, headers: { authorization: "Basic " + Buffer.from(`opencode:${said.password}`).toString("base64"), "content-type": "application/json", "x-opencode-directory": encodeURIComponent(home) }, body: body ? JSON.stringify(body) : undefined });
      const text = await response.text();
      return { status: response.status, json: text ? JSON.parse(text) : null };
    } catch { return { status: 0, json: null }; }
  };
  return { said, api };
}
/** The bridge answering, as a process other than `besides`: its pid and build. */
const bridgeUp = besides => until(async () => {
  const said = readJSON(path.join(RUN, "bridge.json"));
  if (!said?.pid || said.pid === besides || !runs(said.pid)) return null;
  const info = await bridgeAPI().api("GET", "api/info");
  return info.status === 200 && info.json.pid === said.pid ? { pid: said.pid, build: info.json.build } : null;
}, besides ? "a new bridge to answer" : "the bridge to answer");
/** The server for clients answering, as a process other than `besides`. */
const serveUp = besides => until(async () => {
  const said = readJSON(path.join(RUN, "serve.json"));
  if (!said?.pid || said.pid === besides || !runs(said.pid)) return null;
  const answer = await fetch(`http://127.0.0.1:${said.port}/kuvo/info`, { headers: { authorization: "Basic " + Buffer.from(`opencode:${said.password}`).toString("base64") } }).catch(() => null);
  return answer?.status === 200 ? said.pid : null;
}, besides ? "a new server to answer" : "the server to answer");
/** The link (`run`) running, as a process other than `besides`. */
const linkUp = besides => until(() => {
  // (None for a moment, as one goes and the next starts.)
  let pid = 0;
  try { pid = Number(fs.readFileSync(path.join(RUN, "link.pid"), "utf8").trim() || 0); } catch {}
  return pid && pid !== besides && runs(pid) ? pid : null;
}, besides ? "a new link to run" : "the link to run");
const turns = fake => { try { return fs.readFileSync(path.join(fake, "turns.log"), "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); } catch { return []; } };

test("an update another of your computers hands over while a coding agent's turn and an engine's turn run stops neither: both finish, and the new code is live in the server, the link and the bridge", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const { fake } = box();
  keep();
  const engine = await until(() => { const said = readJSON(path.join(RUN, "engine.json")); return said?.pid && runs(said.pid) ? said : null; }, "the engine to run");
  const firstBridge = await bridgeUp();
  assert.equal(firstBridge.build, OLD_BUILD);
  const firstServe = await serveUp();
  const firstLink = await linkUp();

  // A coding agent's turn under way, and the engine's own.
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Long one", permissions: [{ action: "shell", resource: "*", effect: "ask" }], location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  assert.equal((await api("POST", `api/session/${session}/prompt`, { id: "msg_stopsnothing00001", delivery: "steer", text: "hold one" })).status, 200);
  await until(() => turns(fake).length === 1, "the agent's turn to start");
  const pending = async () => (await bridgeAPI().api("GET", `api/session/${session}/permission`)).json?.data;
  const before = await until(async () => (await pending())?.[0], "the permission to appear");
  const visibleId = (await api("GET", `api/session/${session}/message?limit=100&order=asc`)).json.data.find(message => message.type === "assistant").id;
  const holder = () => {
    const folder = path.join(RUN, "agents", "hold");
    return fs.readdirSync(folder).filter(name => name.endsWith(".json")).map(name => readJSON(path.join(folder, name))).find(record => record && Object.values(record.meta.turns ?? {}).some(note => note.session === session));
  };
  const firstHolder = holder();
  assert.ok(firstHolder, "the active turn has a holder");
  assert.ok(runs(firstHolder.child));
  write(path.join(scratch, "engine-busy"), "");

  // The release, held by another of your computers (the mini, whose app updated): its mail says so,
  // and this box pulls it from it over peer mail in that one exchange, checks it, and takes it at
  // once, though both work.
  const { out, pinned } = pack();
  const mini = peerHolding(out);
  const miniKey = link.publicOf(link.newKey()).toString("base64url");
  const lab = readJSON(path.join(RUN, "link.json"));
  link.writeLink({ ...lab, mac: miniKey, macs: [miniKey], legacy: { [miniKey]: { at: 1, peers: [{ key: miniKey, address: link.addressOf(crypto.randomBytes(32)).toString("base64url"), name: "mini", kind: "mac" }] } } });
  let offered = 0;
  link.fleetWith({ keys: [pinned], blocked: null, send: mini.send, offered: () => { offered++; } });
  link.peerMail({ key: miniKey, kind: "mac" }, { agents: { at: Date.now(), list: [], computer: { version: "1.0.0", kuvo: "0.10.0", release: "0.10.0" } } }, null);
  mini.drain(reply => link.takeReleaseMail({ key: miniKey, kind: "mac" }, reply));
  assert.equal(offered, 1, "pulled and offered here in one exchange");
  assert.equal(link.offeredVersion(), "0.10.0");
  const handle = link.engineHandler(() => engine, {}, () => bridgeAPI().said);
  const status = [];
  for await (const part of handle({ method: "GET", path: "kuvo/update", headers: {} }, null, null)) if (part.body) status.push(part.body);
  const waiting = JSON.parse(Buffer.concat(status)).waiting;
  assert.equal(waiting.version, "0.10.0");
  assert.equal(waiting.sessionsAtWork, 2);
  assert.ok(waiting.detail);

  assert.deepEqual(await link.takeOffer({ keys: [pinned], blocked: null }), { result: "updated", version: "0.10.0" }, "taken though both are at work: it stops nothing");
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8").trim(), "0.10.0");
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid, "the engine never stopped");
  // The holder stays up as the published bridge changes. Neither turn was given its completion signal.
  const secondBridge = await bridgeUp(firstBridge.pid);
  assert.equal(secondBridge.build, build(fs.readFileSync(builtAgents, "utf8")), "the new bridge runs the release's build");
  assert.equal(fs.existsSync(path.join(fake, "release-one")), false);
  assert.equal(holder().child, firstHolder.child);
  assert.ok(holder().generation > firstHolder.generation);
  assert.ok(runs(firstHolder.child), "the same agent process continued");
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  const again = await pending();
  assert.equal(again.length, 1);
  assert.equal(again[0].id, before.id, "the open permission retains its original ID");
  assert.equal((await bridgeAPI().api("POST", `api/session/${session}/permission/${before.id}/reply`, { decision: "once" })).status, 200);
  write(path.join(fake, "release-one"), "");
  const messages = await until(async () => {
    const page = (await bridgeAPI().api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the agent's turn to end");
  assert.deepEqual(messages.filter(each => each.type === "assistant").flatMap(each => each.content ?? []).filter(block => block.type === "text").map(block => block.text), ["Before the update.", "Done: hold one"]);
  assert.deepEqual(messages.filter(each => each.type === "assistant").map(each => each.id), [visibleId], "the original reply continued in place");
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"], "never cut");
  assert.deepEqual(turns(fake), [{ text: "hold one", bridge: firstBridge.pid }]);

  assert.equal(fs.readFileSync(path.join(fake, "approvals.log"), "utf8").trim().split("\n").length, 1, "the original agent accepted one answer");
  const second = await bridgeAPI().api("POST", "api/session", { title: "New work", location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(second.status, 200);
  const another = second.json.data.id;
  assert.equal((await bridgeAPI().api("POST", `api/session/${another}/prompt`, { id: "msg_stopsnothing00003", delivery: "steer", text: "new work" })).status, 200);
  await until(() => turns(fake).some(turn => turn.text === "new work"), "the adopted server to accept new work");
  await until(async () => (await bridgeAPI().api("GET", `api/session/${another}/message?limit=100&order=asc`)).json?.data?.at(-1)?.type === "idle", "the new work to finish");

  // The engine's turn ends on the engine it began on.
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  fs.rmSync(path.join(scratch, "engine-busy"));
  assert.equal(await link.agentsWorking(), false, "both finished");

  // Live afterwards: each answers on the new code, and the update is judged well (by the judge the
  // keeper started for it, the updater here being a test's).
  // (A server or link started again within a second of the new link's arrival may be ended once
  // more by the keeper, which can't tell it from one on the code before, and started again.)
  await serveUp(firstServe);
  await linkUp(firstLink);
  assert.ok(runs(secondBridge.pid));
  await until(() => !fs.existsSync(path.join(RUN, "update-trial")), "the update judged");
  assert.equal(readJSON(path.join(RUN, "update.json")).last?.to, "0.10.0", JSON.stringify(readJSON(path.join(RUN, "update.json"))));
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8").trim(), "0.10.0", "kept");
});

test("the engine started again while a coding agent's turn runs: the turn completes on the same bridge, which follows the new engine", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  // (The box the test before left running, on 0.10.0.)
  const fake = path.join(scratch, "fake");
  const engine = readJSON(path.join(RUN, "engine.json"));
  const bridge = await bridgeUp();
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Through a restart", location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  const before = turns(fake).length;
  assert.equal((await api("POST", `api/session/${session}/prompt`, { id: "msg_stopsnothing00002", delivery: "steer", text: "hold two" })).status, 200);
  await until(() => turns(fake).length === before + 1, "the agent's turn to start");

  // The engine ends (a crash, or a hung one ended): its keeper starts another, on a port and
  // password of its own.
  process.kill(engine.pid, "SIGTERM");
  const restarted = await until(() => { const said = readJSON(path.join(RUN, "engine.json")); return said?.pid && said.pid !== engine.pid && runs(said.pid) ? said : null; }, "the keeper to start the engine again");
  assert.notEqual(restarted.port, engine.port);
  // The bridge never went, and follows the new engine where its record says.
  assert.equal(readJSON(path.join(RUN, "bridge.json")).pid, bridge.pid);
  assert.ok(runs(bridge.pid), "the bridge runs on");
  await until(() => fs.existsSync(path.join(scratch, `feed-${restarted.port}`)), "the bridge to follow the new engine");

  // The turn ends as it would have: on that bridge, once.
  write(path.join(fake, "release-two"), "");
  const messages = await until(async () => {
    const page = (await bridgeAPI().api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the agent's turn to end");
  assert.deepEqual(messages.filter(each => each.type === "assistant").flatMap(each => each.content ?? []).filter(block => block.type === "text").map(block => block.text), ["Done: hold two"]);
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"], "never cut");
  assert.deepEqual(turns(fake).slice(before), [{ text: "hold two", bridge: bridge.pid }]);
  assert.equal(readJSON(path.join(RUN, "bridge.json")).pid, bridge.pid);
});

test("if the new bridge exits at the cutover, the keeper resumes the turn on the previous build", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const first = await bridgeUp();
  const fake = path.join(scratch, "fake");
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Failed take-over", permissions: [{ action: "shell", resource: "*", effect: "ask" }], location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200, JSON.stringify(made.json));
  const session = made.json.data.id;
  assert.equal((await api("POST", `api/session/${session}/prompt`, { id: "msg_stopsnothing00004", delivery: "steer", text: "hold recovery" })).status, 200);
  const pending = async () => (await bridgeAPI().api("GET", `api/session/${session}/permission`)).json?.data;
  const ask = await until(async () => (await pending())?.[0], "the permission to appear");

  const bundle = path.join(home, "bin", "kuvo-agents.js");
  const broken = "b".repeat(64);
  const failsOnCutover = 'if (process.argv.includes("handover-standby")) process.prependOnceListener("disconnect", () => process.exit(6));\n';
  write(bundle + ".new", failsOnCutover + fs.readFileSync(builtAgents, "utf8").replace(first.build, broken));
  fs.renameSync(bundle + ".new", bundle);
  const recovered = await bridgeUp(first.pid);
  assert.equal(recovered.build, first.build, "the previous executable answered after the candidate failed");
  assert.equal((await until(async () => (await pending())?.[0], "the same pending permission")).id, ask.id);
  assert.equal((await bridgeAPI().api("POST", `api/session/${session}/permission/${ask.id}/reply`, { decision: "once" })).status, 200);
  write(path.join(fake, "release-recovery"), "");
  const messages = await until(async () => {
    const page = (await bridgeAPI().api("GET", `api/session/${session}/message?limit=100&order=asc`)).json?.data ?? [];
    return page.at(-1)?.type === "idle" ? page : null;
  }, "the recovered turn to finish");
  assert.deepEqual(messages.filter(each => each.type === "idle").map(each => each.outcome), ["succeeded"]);
  assert.deepEqual(turns(fake).filter(turn => turn.text === "hold recovery"), [{ text: "hold recovery", bridge: first.pid }]);
});


test("Update Now uses the no-cut path even when an engine turn is active", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async t => {
  process.env.KUVO_TEST_HEALTH_WINDOW = "1500";
  process.env.KUVO_TEST_HEALTH_EVERY = "200";
  t.after(() => {
    delete process.env.KUVO_TEST_HEALTH_WINDOW;
    delete process.env.KUVO_TEST_HEALTH_EVERY;
  });
  const engine = readJSON(path.join(RUN, "engine.json"));
  const marker = path.join(scratch, "engine-busy");
  write(marker, "");
  const first = await bridgeUp();

  async function requestUpdate(version, engineVersion, bridgeBuild) {
    const { out, pinned } = pack(version, engineVersion, bridgeBuild);
    // Only this scratch box's route and detached subprocess trust the fixture's signing key.
    const testLink = path.join(scratch, `kuvo-link-${version}.js`);
    write(testLink, fs.readFileSync(builtLink, "utf8").replace(link.UPDATE_KEYS[0], pinned));
    const own = await import(testLink);
    assert.equal(own.offerUpdate(out).status, 202);
    const handle = own.engineHandler(() => engine, {}, () => bridgeAPI().said);
    const parts = [];
    for await (const part of handle({ method: "POST", path: "kuvo/update", headers: {}, body: Buffer.from('{"now":true}') }, null, null)) parts.push(part);
    assert.equal(parts[0].head.status, 202);
    return own;
  }

  const nextBuild = "c".repeat(64);
  await requestUpdate("0.11.0", "1.0.0", nextBuild);
  const next = await bridgeUp(first.pid);
  assert.equal(next.build, nextBuild);
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  await until(() => !fs.existsSync(path.join(RUN, "update-trial")), "the manual update to settle");
  assert.equal(readJSON(path.join(RUN, "update.json")).last.to, "0.11.0");

  const incompatible = await requestUpdate("0.12.0", "1.1.0");
  await until(() => readJSON(path.join(RUN, "update.json")).running === "downloading" && !incompatible.isUpdating(), "the manual check to leave the incompatible engine alone");
  assert.equal(incompatible.offeredVersion(), "0.12.0");
  assert.equal(incompatible.updateStatus().version, "0.11.0");
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  assert.ok(runs(engine.pid));
  // Leave the incompatible fixture intact while the box is at work. Cleanup stops this box.
});

test("the one-time pre-holder update restarts its bridge cleanly and leaves an engine turn running", { skip: bun ? false : NO_BUN, timeout: 300_000 }, async () => {
  const first = await bridgeUp();
  const bundle = path.join(home, "bin", "kuvo-agents.js");
  const installed = fs.readFileSync(builtAgents, "utf8");
  const legacyBuild = "e".repeat(64);
  // Run the real bridge's normal shutdown path, with pre-holder process ownership and no
  // handover capability or marker as on a pre-holder installation.
  const legacy = installed.replace(build(installed), legacyBuild)
    .replaceAll("kuvo-live-handover/1", "legacy-test-bridge")
    .replaceAll("agents-live-handover", "agents-legacy-fixture")
    .replace(/function agentHoldFolder\(kind\) \{\n/, "$&  return undefined;\n");
  assert.notEqual(legacy, installed);
  const crossing = path.join(RUN, "agents-live-handover");
  const replace = contents => {
    write(bundle + ".new", contents);
    fs.renameSync(bundle + ".new", bundle);
  };
  replace(legacy);
  process.kill(first.pid, "SIGTERM");
  const old = await bridgeUp(first.pid);
  assert.equal(old.build, legacyBuild);
  fs.rmSync(crossing);
  const holderRecords = () => {
    try { return fs.readdirSync(path.join(RUN, "agents", "hold")).filter(name => name.endsWith(".json")); }
    catch { return []; }
  };
  await until(() => holderRecords().length === 0, "the old fixture to have no holders");

  const engine = readJSON(path.join(RUN, "engine.json"));
  const { api } = bridgeAPI();
  const made = await api("POST", "api/session", { title: "Pre-holder turn", location: { directory: home }, model: { providerID: "codex-cli", id: "gpt-test" } });
  assert.equal(made.status, 200);
  const session = made.json.data.id;
  const fake = path.join(scratch, "fake");
  assert.equal((await api("POST", `api/session/${session}/prompt`, { id: "msg_stopsnothing00005", delivery: "steer", text: "hold legacy" })).status, 200);
  await until(() => turns(fake).some(turn => turn.text === "hold legacy" && turn.bridge === old.pid), "the unheld turn to start");
  assert.equal(holderRecords().length, 0, "the pre-holder bridge owns this agent directly");

  replace(installed);
  const after = await bridgeUp(old.pid);
  assert.equal(after.build, build(installed));
  assert.equal(readJSON(path.join(RUN, "engine.json")).pid, engine.pid);
  assert.equal(fs.readFileSync(crossing, "utf8"), "kuvo-live-handover/1");
  const page = (await bridgeAPI().api("GET", `api/session/${session}/message?limit=100&order=asc`)).json.data;
  assert.deepEqual(page.filter(each => each.type === "idle").map(each => each.outcome), ["interrupted"]);
  assert.deepEqual(page.filter(each => each.type === "user").map(each => each.text), ["hold legacy"]);
  assert.equal(turns(fake).filter(turn => turn.text === "hold legacy").length, 1);
});
