// Launch the selected box in a private home and use the same endpoint as a desktop client.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { relayStandIn } from "../../box/bridge-tests/relay-stand-in.js";
import { createHash } from "node:crypto";

export function scratch() {
  // Keep Unix socket paths short even under macOS's long default TMPDIR.
  return fs.mkdtempSync(path.join(process.platform === "win32" ? os.tmpdir() : "/tmp", "kc-"));
}

export function write(file, data, options) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, data, options);
}

export function readJSON(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

export function isolatedEnv(root) {
  const env = {};
  for (const name of ["PATH", "SystemRoot", "WINDIR", "PATHEXT"]) {
    if (process.env[name]) env[name] = process.env[name];
  }
  const account = path.join(root, "account");
  const tmp = path.join(root, "tmp");
  for (const folder of [account, tmp]) fs.mkdirSync(folder, { recursive: true });
  return {
    ...env, HOME: account, USERPROFILE: account, TMPDIR: tmp, TEMP: tmp, TMP: tmp, TZ: "UTC",
    GIT_CONFIG_GLOBAL: os.devNull, GIT_CONFIG_NOSYSTEM: "1", GIT_TERMINAL_PROMPT: "0",
    GIT_AUTHOR_NAME: "Tests", GIT_AUTHOR_EMAIL: "tests@example.com",
    GIT_COMMITTER_NAME: "Tests", GIT_COMMITTER_EMAIL: "tests@example.com",
  };
}

// Only the child we started is ever signalled. Keep its exit promise even if it exits early.
export function launch(command, args, env, cwd, stdout = "ignore") {
  const child = spawn(command, args, { env, cwd, stdio: ["pipe", stdout, "pipe"] });
  let log = "";
  child.stderr.on("data", chunk => { log = (log + chunk).slice(-16000); });
  child.stdin.on("error", () => {});
  const exited = new Promise(resolve => {
    child.once("error", error => resolve({ error }));
    child.once("close", (code, signal) => resolve({ code, signal }));
  });
  const alive = () => child.pid && child.exitCode === null && child.signalCode === null;
  const stop = async () => {
    if (!alive()) return exited;
    child.kill("SIGTERM");
    const limit = setTimeout(() => { if (alive()) child.kill("SIGKILL"); }, 3000);
    try { return await exited; } finally { clearTimeout(limit); }
  };
  return { child, exited, stop, log: () => log };
}

export async function until(check, why, timeout = 10000, interval = 15) {
  const deadline = performance.now() + timeout;
  for (;;) {
    const result = await check();
    if (result) return result;
    if (performance.now() >= deadline) throw new Error(typeof why === "function" ? why() : why);
    await delay(interval);
  }
}

export function boxBinary(value = process.env.KUVO_BOX_BIN) {
  assert.ok(value, "Set KUVO_BOX_BIN to the built kuvo-link.js or kuvo-box executable.");
  const file = fs.realpathSync(value);
  return { file, typescript: /\.m?js$/.test(file) };
}

// The address derivation is part of the relay's hello protocol.
export const relayAddress = secret => createHash("sha256").update("kuvo-link address v1").update(secret).digest().subarray(0, 16);
export const localRelay = options => relayStandIn({ addressOf: relayAddress }, options);

export function typescriptRuntime() {
  const installed = path.join(os.homedir(), ".bun", "bin", process.platform === "win32" ? "bun.exe" : "bun");
  return process.env.KUVO_CONFORMANCE_BUN || process.env.BUN || (fs.existsSync(installed) ? installed : "bun");
}

export function boxCommand(binary, home, env, relay, args, extra = {}) {
  const ts = binary.typescript;
  return launch(ts ? typescriptRuntime() : binary.file,
    ts ? [`--config=${os.devNull}`, "--no-env-file", binary.file, ...args] : args,
    { ...env, ...(ts ? { BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" } : {}),
      KUVO_HOME: home, KUVO_TEST_RELAY: relay.url, KUVO_RELAY_URL: relay.url, ...extra }, home);
}

export function startBox({ binary, home, env, relay, mac = false, args = [] }) {
  const proc = boxCommand(binary, home, env, relay,
    ["serve", ...(binary.typescript ? ["--resident"] : []), ...args],
    mac ? { OPENCODE_PASSWORD: "conformance-local", KUVO_TEST_NO_PEERS: "1" } : {});
  const file = path.join(home, "run", "serve.json");
  const ready = () => Promise.race([
    until(() => {
      if (proc.child.exitCode !== null || proc.child.signalCode !== null || !proc.child.pid) {
        throw new Error(`serve exited before publishing ${file}\n${proc.log()}`);
      }
      if (!fs.existsSync(file)) return;
      const record = readJSON(file);
      if (record.pid !== proc.child.pid) return;
      assert.ok(Number.isInteger(record.port) && record.port > 0 && record.port <= 65535, "serve.json contains a TCP port");
      assert.ok(typeof record.password === "string" && record.password.length, "serve.json contains a password");
      return record;
    }, () => `serve did not publish ${file}\n${proc.log()}`, 25000),
    proc.exited.then(result => { throw new Error(`serve exited before readiness: ${JSON.stringify(result)}\n${proc.log()}`); }),
  ]);
  return { ...proc, ready };
}

const authorization = password => "Basic " + Buffer.from(`opencode:${password}`).toString("base64");

// Use origin-form paths directly so encoded paths reach the server without URL rewriting.
export async function request(record, method, route, body, headers = {}, timeout = 10000) {
  const data = body === undefined ? undefined : Buffer.from(typeof body === "string" ? body : JSON.stringify(body));
  const reply = await new Promise((resolve, reject) => {
    const req = http.request({
      host: "127.0.0.1", port: record.port, method, path: "/" + route, agent: false,
      signal: AbortSignal.timeout(timeout),
      headers: { authorization: authorization(record.password), "x-kuvo-route-version": "1",
        ...(data ? { "content-type": "application/json", "content-length": data.length } : {}),
        ...headers },
    }, resolve);
    req.on("error", reject);
    req.end(data);
  });
  const chunks = [];
  for await (const chunk of reply) chunks.push(chunk);
  const bytes = Buffer.concat(chunks);
  const text = bytes.toString("utf8");
  let json;
  try { json = JSON.parse(text); } catch {}
  return { status: reply.statusCode, type: reply.headers["content-type"] ?? "", text, json, bytes };
}

export async function openLine(record, query = "") {
  const stop = new AbortController();
  const response = await fetch(`http://127.0.0.1:${record.port}/kuvo/line${query}`, {
    headers: { authorization: authorization(record.password), "x-kuvo-route-version": "1" },
    redirect: "error", signal: AbortSignal.any([stop.signal, AbortSignal.timeout(10000)]),
  });
  return {
    status: response.status, type: response.headers.get("content-type"),
    async *frames() {
      let pending = "";
      for await (const chunk of response.body.pipeThrough(new TextDecoderStream())) {
        pending += chunk;
        const lines = pending.split("\n");
        pending = lines.pop();
        for (const line of lines) if (line.trim()) yield JSON.parse(line);
      }
      if (pending.trim()) yield JSON.parse(pending);
    },
    close: () => stop.abort(),
  };
}
