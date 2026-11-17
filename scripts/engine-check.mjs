#!/usr/bin/env node
// The release check of a box's engines (protocol/PROTOCOL.md, "A new engine"), run as a release is
// built, before scripts/box-release.sh packs it: the link and the coding agents' bridge as built
// each answer `selftest` on the engine pinned (box/src/shared/engine-pin.json) and on the one the
// release before ran (its `previous`), and the engine before opens what the new one wrote (a data
// folder made by the engine before, opened by the new one, which migrates it and writes to it, then
// opened by the one before again: both conversations listed and read). What it found goes in a file,
// `{engines, downgradeSafe}`, which box-release.sh puts in `box.json` (`ENGINE_CHECK`): a box running
// one of `engines` takes the release's code at once and its engine at a quiet moment; one whose new
// engine doesn't start goes back to the one before only where `downgradeSafe` is true.
//
// Usage: node scripts/engine-check.mjs <out.json>
// Engines are downloaded for this computer's system and kind, each checked against npm's integrity,
// into a folder of its own that goes at the end, with every process it started (by its PID).

import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const checkout = fileURLToPath(new URL("..", import.meta.url));
const out = process.argv[2];
if (!out) { console.error("Usage: node scripts/engine-check.mjs <out.json>"); process.exit(2); }
const pin = JSON.parse(fs.readFileSync(path.join(checkout, "box/src/shared/engine-pin.json"), "utf8"));
const system = process.platform === "darwin" ? "darwin" : "linux";
const kind = { x64: "x86_64", arm64: "aarch64" }[os.arch()];
const link = path.join(checkout, "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js");
const agents = path.join(checkout, "box/dist/kuvo-agents.js");
const work = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-engine-check-"));
const started = new Set();
const end = code => {
  for (const pid of started) try { process.kill(pid, "SIGKILL"); } catch {}
  fs.rmSync(work, { recursive: true, force: true });
  process.exit(code);
};
const fail = message => { console.error(message); end(1); };

async function engine(pinned) {
  const build = pinned?.[system]?.[kind];
  if (!build?.url || !build.integrity) fail(`No ${system} ${kind} build of engine ${pinned?.version}.`);
  const response = await fetch(build.url);
  if (!response.ok) fail(`Engine ${pinned.version} couldn't be downloaded (${response.status}).`);
  const tarball = Buffer.from(await response.arrayBuffer());
  if (crypto.createHash("sha512").update(tarball).digest("base64") !== build.integrity.slice(7)) fail(`Engine ${pinned.version} isn't what npm says.`);
  const folder = path.join(work, "engine", pinned.version);
  fs.mkdirSync(folder, { recursive: true });
  fs.writeFileSync(path.join(folder, "engine.tgz"), tarball);
  if (spawnSync("tar", ["xzf", path.join(folder, "engine.tgz"), "-C", folder, "package/bin/opencode"]).status !== 0) fail(`Engine ${pinned.version} couldn't be unpacked.`);
  return path.join(folder, "package", "bin", "opencode");
}

function selftest(program, script, version) {
  const home = path.join(work, "home-selftest");
  fs.mkdirSync(home, { recursive: true });
  const ran = spawnSync(program, ["--config=/dev/null", "--no-env-file", script, "selftest"], { env: { PATH: process.env.PATH, HOME: home, KUVO_HOME: path.join(home, ".kuvo-code"), BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" }, encoding: "utf8", timeout: 120_000 });
  if (ran.status !== 0 || !/\bok\b/.test(`${ran.stdout}`)) fail(`${path.basename(script)} doesn't answer selftest on engine ${version}:\n${ran.stdout}${ran.stderr}`);
}

const freePort = () => new Promise(resolve => {
  const server = net.createServer();
  server.listen(0, "127.0.0.1", () => { const { port } = server.address(); server.close(() => resolve(port)); });
});

// An engine serving the data in `home` until `use` is done with it.
async function serving(program, home, use) {
  const port = await freePort();
  const password = crypto.randomBytes(16).toString("hex");
  const env = { PATH: process.env.PATH, HOME: home, XDG_DATA_HOME: path.join(home, "data"), XDG_CONFIG_HOME: path.join(home, "config"), XDG_STATE_HOME: path.join(home, "state"), XDG_CACHE_HOME: path.join(home, "cache"), OPENCODE_PASSWORD: password };
  const child = spawn(program, ["serve", "--port", String(port), "--hostname", "127.0.0.1"], { env, cwd: home, stdio: "ignore" });
  started.add(child.pid);
  const call = async (method, route, body) => {
    const response = await fetch(`http://127.0.0.1:${port}/${route}`, { method, headers: { authorization: `Basic ${Buffer.from(`opencode:${password}`).toString("base64")}`, "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
    const text = await response.text();
    if (!response.ok) throw new Error(`${method} ${route}: ${response.status} ${text.slice(0, 300)}`);
    return text ? JSON.parse(text) : null;
  };
  try {
    for (let tries = 0; ; tries++) {
      try { await call("GET", "api/session?limit=1"); break; } catch (error) {
        if (tries > 600 || child.exitCode !== null) throw new Error(`It didn't start: ${error.message}`);
        await new Promise(resolve => setTimeout(resolve, 200));
      }
    }
    return await use(call);
  } finally {
    child.kill("SIGTERM");
    await new Promise(resolve => (child.exitCode !== null ? resolve() : child.once("exit", resolve)));
    started.delete(child.pid);
  }
}

// Whether the engine before opens what `fresh` wrote over its own data: both conversations there.
async function downgradeSafe(before, fresh) {
  const home = path.join(work, "home-data");
  fs.mkdirSync(path.join(home, "project"), { recursive: true });
  const location = { directory: path.join(home, "project") };
  const ids = (page) => (page?.data ?? []).map(each => each.id);
  try {
    const first = await serving(before.program, home, call => call("POST", "api/session", { title: "Before", location }).then(made => made.data.id));
    const second = await serving(fresh.program, home, async call => {
      if (!ids(await call("GET", "api/session?limit=50")).includes(first)) throw new Error("The new engine doesn't list the conversation the one before made.");
      return (await call("POST", "api/session", { title: "After", location })).data.id;
    });
    await serving(before.program, home, async call => {
      const listed = ids(await call("GET", "api/session?limit=50"));
      if (!listed.includes(first) || !listed.includes(second)) throw new Error("The engine before doesn't list both conversations.");
      for (const id of [first, second]) await call("GET", `api/session/${encodeURIComponent(id)}/message?limit=10`);
    });
    return true;
  } catch (error) {
    console.error(`Engine ${before.version} can't be gone back to from ${fresh.version}: ${error.message}`);
    return false;
  }
}

try {
  const fresh = { version: pin.version, program: await engine(pin) };
  const before = pin.previous?.version && pin.previous.version !== pin.version ? { version: pin.previous.version, program: await engine(pin.previous) } : null;
  for (const each of [fresh, before].filter(Boolean)) {
    selftest(each.program, link, each.version);
    selftest(each.program, agents, each.version);
  }
  const found = before ? { engines: [fresh.version, before.version], downgradeSafe: await downgradeSafe(before, fresh) } : { engines: [fresh.version] };
  fs.writeFileSync(out, JSON.stringify(found) + "\n");
  console.log(JSON.stringify(found));
  end(0);
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
}
