// Keep the reference bridge and both resident binaries on the same private, local CLI fixture.
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { standInClaude } from "../../box/bridge-tests/claude-stand-in.js";
import assert from "node:assert/strict";
import { boxCommand, launch, readJSON, request, typescriptRuntime, until, write } from "./adapter.mjs";

export const headers = { "x-kuvo-from": "app" };
export const provider = "claude-code";

export function claudeEnvironment(binary, home, env, fixture) {
  const bin = standInClaude({
    root: env.HOME,
    agents: binary.typescript ? path.join(home, "agents") : undefined,
    ...fixture,
  });
  const system = process.platform === "win32" ? path.join(env.SystemRoot, "System32") : "/usr/bin:/bin";
  return { ...env, KUVO_TEST_PATH_ONLY: "1", PATH: [bin, system].join(path.delimiter) };
}

export async function startClaudeBridge(binary, home, env) {
  if (!binary.typescript) return undefined;
  fs.rmSync(path.join(home, "run", "bridge.json"), { force: true });
  const bundle = fileURLToPath(new URL("../../box/dist/kuvo-agents.js", import.meta.url));
  const bridge = launch(typescriptRuntime(), ["--config=/dev/null", "--no-env-file", bundle], {
    ...env, KUVO_HOME: home, KUVO_AGENTS_REMOTE: "1", KUVO_AGENTS_PORT: String(await freePort()),
    KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_AGENTS_PASSWORD: "fixture",
    KUVO_SANDBOX: "off", BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
  }, home);
  try {
    await until(() => fs.existsSync(path.join(home, "run", "bridge.json")),
      () => "The TypeScript bridge never published its fixture listener. " + bridge.log());
    return bridge;
  } catch (error) {
    await bridge.stop();
    throw error;
  }
}

export async function keptListener(home, keeper, previous) {
  return until(async () => {
    const file = path.join(home, "run", "serve.json");
    if (!fs.existsSync(file)) return;
    const listener = readJSON(file);
    if (listener.pid === previous) return;
    try {
      const info = await request(listener, "GET", "kuvo/info", undefined, headers, 1000);
      return info.status === 200 && listener;
    } catch {
      return;
    }
  }, () => "The keeper did not publish a ready resident. " + keeper.log(), 25000);
}

export async function transcript(listener, route) {
  const answer = await request(listener, "GET", route + "/message?order=asc", undefined, headers);
  assert.equal(answer.status, 200, answer.text);
  return answer.json.data;
}

function holderRecords(home) {
  const folder = path.join(home, "run", "agents", "hold");
  if (!fs.existsSync(folder)) return [];
  // A holder removes its own record as it exits, so one listed may be gone when read.
  return fs.readdirSync(folder).filter(name => /^[a-f\d]{12}\.json$/.test(name)).flatMap(name => {
    try { return [readJSON(path.join(folder, name))]; }
    catch (error) { if (error.code === "ENOENT") return []; throw error; }
  });
}

export function heldClaude(home, session) {
  return holderRecords(home).find(record => record.meta?.provider === provider && record.meta?.session === session);
}

// Stops the holders this case started and waits for them to exit, so the scratch home can go.
export async function stopFixtureHolders(home) {
  await stopPids(holderRecords(home).map(record => record.pid));
}

export async function stopPids(pids) {
  pids = pids.filter(pid => Number.isInteger(pid) && pid > 1);
  const alive = pid => {
    try { process.kill(pid, 0); return true; }
    catch (error) { if (error.code === "ESRCH") return false; throw error; }
  };
  for (const pid of pids) {
    try { process.kill(pid, "SIGTERM"); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  }
  const deadline = Date.now() + 10000;
  while (pids.some(alive) && Date.now() < deadline) await new Promise(done => setTimeout(done, 50));
}

export function seedSession(home, id, { directory = home, model = "opus" } = {}) {
  write(path.join(home, "agents/sessions", id, "session.json"), JSON.stringify({
    id, title: "Claude fixture", projectID: "fixture", agent: "build",
    location: { directory }, model: { providerID: provider, id: model },
    time: { created: 1, updated: 1 }, cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
  }));
}

// Two Rust releases of the same binary in a scratch home, and the keeper command that picks one.
export function twoReleases(binary, home, env, relay) {
  const old = "versions/0.8.0-beta.1-123456789abc/bin";
  const next = "versions/0.8.0-beta.2-123456789abc/bin";
  for (const [folder, version] of [[old, "0.8.0-beta.1"], [next, "0.8.0-beta.2"]]) {
    const installed = path.join(home, folder, path.basename(binary.file));
    write(path.join(home, folder, "..", "box.json"), JSON.stringify({ kuvo: "box/1", launch: "rust", version }));
    fs.mkdirSync(path.dirname(installed), { recursive: true });
    try { fs.linkSync(binary.file, installed); }
    catch { fs.copyFileSync(binary.file, installed); }
  }
  const control = async args => {
    const command = boxCommand(binary, home, env, relay, ["keep", ...args]);
    assert.equal((await command.exited).code, 0, command.log());
  };
  return { old, next, control };
}

async function freePort() {
  const server = net.createServer();
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}
