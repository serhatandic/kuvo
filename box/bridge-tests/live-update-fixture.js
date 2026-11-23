import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { removeTestHomeAtExit } from "./test-safety.js";
import { atEnd, removeAtEnd } from "./at-end.js";
import { bunRunsAs, testBun } from "./bun-for-tests.js";
import { CODEX_STAND_IN } from "./codex-stand-in.js";
import { splitLaunchBuild, splitLaunchKeeper } from "./failure-bench-legacy.js";

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
/** The bridge's build as its bundle names it, another one for the version before. */
const build = text => [...text.matchAll(/\/\/ Built from Packages\/Box\/src\/agents ([0-9a-f]{16,64})/g)].at(-1)?.[1];
const OLD_BUILD = "a".repeat(64);

export { bun, builtLink, builtAgents, write, readJSON, runs, build, OLD_BUILD };
export const turns = fake => { try { return fs.readFileSync(path.join(fake, "turns.log"), "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); } catch { return []; } };

export function liveUpdateFixture({ preResident = false, sandbox = false, identity } = {}) {
  const base = fs.realpathSync(fs.mkdtempSync(path.join(process.platform === "darwin" ? "/tmp" : os.tmpdir(), "k774-")));
  const home = path.join(base, ".kuvo-code");
  const scratch = path.join(base, "scratch");
  removeTestHomeAtExit(base);
  removeAtEnd(base);
  const env = { ...process.env, HOME: base, KUVO_HOME: home, TMPDIR: base + "/", BUN_BE_BUN: "1" };
  const RUN = path.join(home, "run");
  async function until(check, what, within = 120_000) {
    for (const end = Date.now() + within; Date.now() < end;) {
      const found = await check();
      if (found) return found;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    // (What the bridge said, for what didn't come.)
    const logs = ["agents.log", "serve.log", "link.log", "update-start.log", "engine.log"].flatMap(name => {
      try { return [name + ":\n" + fs.readFileSync(path.join(RUN, name), "utf8").slice(-1500)]; } catch { return []; }
    });
    assert.fail(`never: ${what}\n${logs.join("\n")}`);
  }

  /** A box on 0.9.0, its engine 1.0.0, as a machine's keeper runs it: its link wanted (a phone
   * paired, its relay out of reach) and its server for clients wanted. Its bridge and link a build
   * before the one this checkout makes. */
  function box() {
    // The engine keeps each turn in this process. The test decides when it can answer.
    const engine = path.join(scratch, "engine.mjs");
    write(engine, String.raw`
  import fs from "node:fs"; import http from "node:http";
  const port = Number(process.argv[process.argv.indexOf("--port") + 1]);
  const scratch = ${JSON.stringify(scratch)};
  const sessions = new Map([["ses_engine", {}]]);
  const feeds = new Set();
  const say = (type, data) => {
    for (const feed of feeds) feed.write("data: " + JSON.stringify({ type, data }) + "\n\n");
  };
  http.createServer(async (request, response) => {
    const route = request.url.split("?")[0];
    if (route === "/api/event" || route === "/event") {
      fs.writeFileSync(scratch + "/feed-" + port, "");
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write(": open\n\n");
      feeds.add(response);
      response.on("close", () => feeds.delete(response));
      return;
    }
    const reply = data => response.end(JSON.stringify({ data }));
    response.setHeader("content-type", "application/json");
    if (route === "/api/session/active") {
      const busy = [...sessions].filter(([, turn]) => turn.state === "busy").map(([id]) => [id, { type: "busy" }]);
      if (fs.existsSync(scratch + "/engine-busy")) busy.push(["ses_engine", { type: "busy" }]);
      return reply(Object.fromEntries(busy));
    }
    if (route === "/api/debug/location") return reply([{ directory: scratch + "/project" }]);
    if (route === "/api/shell") return reply(fs.existsSync(scratch + "/engine-shell")
      ? [{ id: "fixture-shell", status: "running", metadata: { sessionID: "ses_engine" } }] : []);
    if (route === "/api/provider") return reply([{ id: "fixture", name: "Fixture" }]);
    if (route === "/api/model") return reply([{ providerID: "fixture", id: "test", limit: {} }]);
    if (route === "/api/session" && request.method === "POST") {
      const id = "ses_hidden";
      sessions.set(id, {});
      return reply({ id });
    }
    const [, id, action] = /^\/api\/session\/([^/]+)(?:\/([^/]+))?$/.exec(route) ?? [];
    const turn = sessions.get(id);
    if (turn) {
      if (!action) return reply({ id, time: { idle: turn.idle }, outcome: turn.state === "busy" ? undefined : turn.state });
      if (action === "prompt" && request.method === "POST") {
        const chunks = [];
        for await (const chunk of request) chunks.push(chunk);
        const body = JSON.parse(Buffer.concat(chunks).toString());
        Object.assign(turn, { state: "busy", prompt: body.text, count: (turn.count ?? 0) + 1 });
        say("session.inbox.delivered", { sessionID: id, inboxID: body.id });
        return reply({});
      }
      if (action === "interrupt" || action === "fixture-finish") {
        if (turn.state !== "busy") { response.statusCode = 409; return reply(turn.state); }
        const outcome = action === "interrupt" ? "interrupted" : "succeeded";
        if (outcome === "succeeded") {
          const step = "msg_fixtureengineanswer";
          say("session.step.started", { sessionID: id, assistantMessageID: step });
          say("session.text.started", { sessionID: id, ordinal: 0 });
          say("session.text.delta", { sessionID: id, ordinal: 0, delta: "Done: " + turn.prompt });
          say("session.text.ended", { sessionID: id, ordinal: 0 });
          say("session.step.ended", { sessionID: id, assistantMessageID: step, finish: "stop" });
        }
        Object.assign(turn, { state: outcome, idle: Date.now() });
        say("session.execution." + outcome, { sessionID: id });
        return reply({ outcome, count: turn.count });
      }
    }
    reply([]);
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
    for (const name of ["kuvo-start", "kuvo-launch", "kuvo-setsid", "kuvo-wake", "kuvo-npx"]) {
      const source = `box/src/keeper/${name}`;
      const code = preResident && name === "kuvo-start" ? splitLaunchKeeper().toString("utf8") : fs.readFileSync(path.join(checkout, source), "utf8");
      write(path.join(home, "bin", name), code.replaceAll("@ENGINE@", "1.0.0"), 0o700);
    }
    const previousLink = preResident ? splitLaunchBuild() : fs.readFileSync(builtLink, "utf8") + "\n// the version before\n";
    write(path.join(home, "bin", "kuvo-link.js"), previousLink, 0o600);
    const agents = fs.readFileSync(builtAgents, "utf8");
    const checksum = crypto.createHash("sha256").update(agents).digest("hex").slice(0, 16);
    write(path.join(home, "bin", "kuvo-agents.js"), preResident ? `${agents}\n// kuvo-agents ${checksum}\n` :
      agents.replace(`// Built from Packages/Box/src/agents ${build(agents)}`, `// Built from Packages/Box/src/agents ${OLD_BUILD}`));
    // The installed plugin also ships in the next release. Keep it in the real versioned layout.
    const plugins = path.join(home, "config", "opencode", "plugins");
    fs.mkdirSync(plugins, { recursive: true });
    fs.copyFileSync(path.join(checkout, "box/src/plugins/kuvo-code.js"), path.join(plugins, "kuvo-code.js"));
    if (preResident) fs.copyFileSync(path.join(checkout, "box/dist/kuvo-browser.js"), path.join(home, "bin", "kuvo-browser.js"));
    write(path.join(home, "bin", "box-version"), "0.9.0");
    write(path.join(home, "box", "kind"), "machine\n");
    // What only a Mac's keeper loop reaches for (keeping it awake, older Macs' leftovers): noted only.
    for (const tool of ["pkill", "caffeinate", "crontab", "launchctl"]) write(path.join(base, "tools", tool), `#!/bin/sh\nexit 0\n`, 0o755);
    // The coding agent, on the PATH its engine starts with.
    const fake = path.join(scratch, "fake");
    const project = path.join(scratch, "project");
    fs.mkdirSync(project, { recursive: true });
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
    const signals = sandbox ? project : fake;
    const provider = path.join(fake, "app-server.js");
    write(provider, sandbox ? asking.replace('JSON.stringify({ text, bridge: bridge() })', 'JSON.stringify({ text, bridge: bridge(), pid: process.pid })').replace('let buffer =', `
const forbidden = ${JSON.stringify(path.join(RUN, "launch.env"))};
let canReadBox = false;
try { fs.readFileSync(forbidden); canReadBox = true; } catch {}
fs.writeFileSync(path.join(folder, "provider-" + process.pid + ".json"), JSON.stringify({ pid: process.pid, canReadBox }));
let buffer =`) : asking);
    write(path.join(fake, "codex"), `#!/bin/sh\nexport BUN_BE_BUN=1\n[ "$1" = "--version" ] && { echo "codex-cli 0.166.0"; exit 0; }\nexec "${bun}" "${provider}" "${signals}"\n`, 0o755);
    // How its engine starts (the keeper's `run/launch.env`): with a bridge that makes way at once
    // once nothing runs, and processes that look for new code often.
    write(path.join(RUN, "launch.env"), [`PATH=${fake}:${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, "LANG=C", ...(sandbox ? [] : ["KUVO_SANDBOX=off"]), "KUVO_TEST_RENEW_SETTLE=0", "KUVO_TEST_RENEW_UPTIME=0", "KUVO_TEST_CODE_EVERY=200", "KUVO_TEST_NO_PEERS=1", "KUVO_TEST_HEALTH_WINDOW=1500", "KUVO_TEST_HEALTH_EVERY=200"].join("\n") + "\n", 0o600);
    write(path.join(RUN, "launch.notes"), "{}", 0o600);
    const key = () => crypto.randomBytes(32).toString("base64url");
    write(path.join(RUN, "link.json"), JSON.stringify(identity ?? { id: "lab", name: "lab", relay: "ws://127.0.0.1:9", secret: key(), key: key(), phones: [{ key: key(), name: "iPhone", added: Math.floor(Date.now() / 1000), from: "pair" }] }), 0o600);
    write(path.join(RUN, "link.needed"), "", 0o600);
    write(path.join(RUN, "serve.needed"), "", 0o600);
    write(path.join(RUN, "engine.now"), "", 0o600);
    return { fake, project, signals };
  }

  /** The keeper's loop, as at boot: a round a second here. Ended with the box (`kuvo-start off`). */
  function keep() {
    const keeper = spawn("sh", [path.join(home, "bin", "kuvo-start"), "keep"], { env: { HOME: base, PATH: `${path.join(base, "tools")}:/usr/bin:/bin:/usr/sbin:/sbin`, KUVO_TEST_KEEP_ROUND: "1" }, detached: true, stdio: "ignore" });
    const gone = new Promise(resolve => keeper.once("exit", resolve));
    atEnd(async () => {
      const services = readJSON(path.join(RUN, "box-services.json"))?.pid;
      spawnSync("sh", [path.join(home, "bin", "kuvo-start"), "off"], { env: { HOME: base, PATH: "/usr/bin:/bin" }, timeout: 60_000 });
      try { process.kill(-keeper.pid, "SIGKILL"); } catch {}
      await gone;
      if (services) await until(() => !runs(services), "the fixture's services to see off", 8_000);
    });
  }

  /** Package a release signed by the test, in a folder as an app carries it. */
  function pack(version = "0.10.0", editLink = source => source) {
    const engine = "1.0.0";
    const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
    const root = path.join(scratch, "sources");
    for (const file of ["box/src/keeper/kuvo-start", "box/src/keeper/kuvo-launch", "box/src/keeper/kuvo-setsid", "box/src/keeper/kuvo-wake", "box/src/keeper/kuvo-npx", "box/src/keeper/runtime/node", "box/src/plugins/kuvo-code.js", "box/src/shared/bot-words.json", "box/dist/kuvo-browser.js", "box/dist/kuvo-agents.js"]) {
      write(path.join(root, file), fs.readFileSync(path.join(checkout, file)));
    }
    write(path.join(root, "box/dist/kuvo-link.js"), editLink(fs.readFileSync(builtLink, "utf8")));
    const integrity = "sha512-" + crypto.createHash("sha512").update("unused").digest("base64");
    const url = `https://example.invalid/engine-${engine}.tgz`;
    const builds = { x86_64: { url, integrity }, aarch64: { url, integrity } };
    write(path.join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: engine, linux: builds, darwin: builds }));
    const out = path.join(scratch, `release-${version}`);
    const packed = spawnSync("sh", [path.join(checkout, "scripts", "box-release.sh"), version, `https://example.invalid/releases/download/v${version}/box-${version}.tar.gz`, out], {
      env: { ...env, BUN: bun, NODE: process.execPath, BOX_SOURCES: root, KUVO_BOX_SIGNING_KEY: privateKey.export({ type: "pkcs8", format: "pem" }), KUVO_BOX_SPREADS: "1" }, encoding: "utf8",
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
  const bridgeUp = (besides, wantedBuild) => until(async () => {
    const said = readJSON(path.join(RUN, "bridge.json"));
    if (!said?.pid || said.pid === besides || (wantedBuild && said.build !== wantedBuild) || !runs(said.pid)) return null;
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
  function holderFor(session) {
    const folder = path.join(RUN, "agents", "hold");
    return fs.readdirSync(folder)
      .filter(name => name.endsWith(".json"))
      .map(name => readJSON(path.join(folder, name)))
      .find(record => record && Object.values(record.meta.turns ?? {}).some(note => note.session === session));
  }

  return { base, home, scratch, RUN, env, box, keep, pack, bridgeAPI, bridgeUp, serveUp, linkUp, holderFor, until };
}
