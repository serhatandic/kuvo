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
import { watchTestProcesses } from "../box/bridge-tests/test-home-processes.js";

const checkout = fileURLToPath(new URL("..", import.meta.url));
const out = process.argv[2];
if (!out) { console.error("Usage: node scripts/engine-check.mjs <out.json>"); process.exit(2); }
const built = spawnSync("sh", ["scripts/build-built.sh"], { cwd: checkout, stdio: "inherit" });
if (built.status !== 0) process.exit(built.status ?? 1);
const pin = JSON.parse(fs.readFileSync(path.join(checkout, "box/src/shared/engine-pin.json"), "utf8"));
const system = process.platform === "darwin" ? "darwin" : "linux";
const kind = { x64: "x86_64", arm64: "aarch64" }[os.arch()];
const link = path.join(checkout, "box/dist/kuvo-link.js");
const agents = path.join(checkout, "box/dist/kuvo-agents.js");
const work = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-engine-check-"));
const lifetime = watchTestProcesses();
lifetime.addRoot(fs.realpathSync(work), true);
const fail = message => { throw new Error(message); };

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

// Whether `script` answers selftest on engine `program`; `required`: the release fails if not.
function selftest(program, script, version, required = true) {
  const home = path.join(work, "home-selftest");
  fs.mkdirSync(home, { recursive: true });
  const ran = spawnSync(program, ["--config=/dev/null", "--no-env-file", script, "selftest"], { env: lifetime.env({ PATH: process.env.PATH, HOME: home, KUVO_HOME: path.join(home, ".kuvo-code"), BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" }), encoding: "utf8", timeout: 120_000 });
  const ok = ran.status === 0 && /\bok\b/.test(`${ran.stdout}`);
  if (!ok && required) fail(`${path.basename(script)} doesn't answer selftest on engine ${version}:\n${ran.stdout}${ran.stderr}`);
  if (!ok) console.error(`${path.basename(script)} doesn't answer selftest on engine ${version}: boxes running it take this release with its engine, once nothing's at work.`);
  return ok;
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
  const child = spawn(program, ["serve", "--port", String(port), "--hostname", "127.0.0.1"], { env: lifetime.env(env), cwd: home, stdio: "ignore" });
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
    // (One that doesn't end within half a minute is ended.)
    const killer = setTimeout(() => child.kill("SIGKILL"), 30_000);
    await new Promise(resolve => (child.exitCode !== null || child.signalCode !== null ? resolve() : child.once("exit", resolve)));
    clearTimeout(killer);
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
      // (And writes there: going back, it gets what was held for it.)
      const third = (await call("POST", "api/session", { title: "Back", location })).data.id;
      await call("PATCH", `api/session/${encodeURIComponent(second)}`, { title: "After, renamed" });
      if (!ids(await call("GET", "api/session?limit=50")).includes(third)) throw new Error("The engine before can't write to what the new one wrote.");
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
  selftest(fresh.program, link, fresh.version);
  selftest(fresh.program, agents, fresh.version);
  // The engine before: where this release's code doesn't run on it, boxes running it take the release
  // whole (its engine with its code), as without a check.
  const withBefore = before && selftest(before.program, link, before.version, false) && selftest(before.program, agents, before.version, false);
  const found = withBefore ? { engines: [fresh.version, before.version], downgradeSafe: await downgradeSafe(before, fresh) } : { engines: [fresh.version] };
  fs.writeFileSync(out, JSON.stringify(found) + "\n");
  console.log(JSON.stringify(found));
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
} finally {
  await lifetime.stop();
}
