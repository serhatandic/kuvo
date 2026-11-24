// Compare answers and the complete post-shutdown home inventories.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { DatabaseSync } from "node:sqlite";
import { boxBinary, isolatedEnv, localRelay, readJSON, request, scratch, startBox, until, write } from "../adapter.mjs";
import { standInEngine } from "../engine.mjs";

// These are individual fields, scoped to a route or home file. A missing field,
// a changed type, a changed path, and every other value remain visible.
const volatile = new Map([
  ["GET kuvo/info", {
    started: "engine record mtime",
    serverGeneration: "resident instance ID",
  }],
  ["run/serve.json", {
    pid: "resident process ID",
    port: "allocated loopback port",
    password: "random resident HTTP password",
    startedAt: "resident startup time",
    generation: "resident instance ID",
  }],
  ["run/engine.json", { port: "allocated stand-in engine port" }],
  ["box/marketplace/mcp-relay-v1.json", { port: "allocated Marketplace relay port" }],
  ["run/lists/sessions.json", {
    epoch: "session list epoch created from startup time and PID",
    owner: "session list writer PID",
    started: "session list writer start identity",
  }],
  ["run/work/told.json", { since: "notice scan start time", "sinces/bridge": "bridge notice scan start time" }],
  ["run/notices/teller", { "": "notice publisher PID" }],
  ["agents/usage/insights.sqlite/state/local", { scanned: "local index scan timestamp" }],
  ["updates/rehome/journal.json", { began: "move start time", clonedAt: "clone start time" }],
  ["updates/rehome/state.json", { movedAt: "move publication time" }],
]);

const reachFiles = ["run/peer-reach.json", "run/peer-reach-checked.json"];

function normalise(value, name, home, enginePort, field = "") {
  if (reachFiles.includes(name) && /^[A-Za-z0-9_-]{43}$/.test(field) && typeof value === "number")
    return "<number: relay observation timestamp>";
  const usageScan = name === "agents/usage/usage.json" && /^accounts\/[^/]+\/scanned$/.test(field);
  const botSince = name === "GET kuvo/bots" && /^bots\/\d+\/now\/since$/.test(field);
  const why = usageScan ? "local account scan timestamp" : botSince ? "when this box saw the bot's state begin" : volatile.get(name)?.[field];
  if (why && ["number", "string"].includes(typeof value)) return `<${typeof value}: ${why}>`;
  if (name === "updates/rehome/journal.json" && field === "kept" && typeof value === "string") {
    value = value.replace(/\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z$/, "<move start time>");
  }
  if (typeof value === "string") return value.replaceAll(home, "<home>");
  if (Array.isArray(value)) return value.map((item, index) => normalise(item, name, home, enginePort, field + "/" + index));
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) =>
      [name === "run/queues-scanned.json" && !field && key === String(enginePort) ? "<stand-in engine port>" : key.replaceAll(home, "<home>"),
        normalise(item, name, home, enginePort, field ? field + "/" + key : key)]));
  }
  return value;
}

// SQLite can place the same committed data in either the database or its WAL.
// Compare schema and typed rows from a private read copy. The inventory still
// checks that the journal and its rebuildable shared-memory index paths exist.
const insights = "agents/usage/insights.sqlite";
const databases = new Set([insights, "data/opencode/opencode.db"]);
export function sqliteContents(file, home, { migrationClockAfter } = {}) {
  const tmp = scratch();
  const target = path.join(tmp, "copy.sqlite");
  try {
    fs.copyFileSync(file, target);
    if (fs.existsSync(file + "-wal")) fs.copyFileSync(file + "-wal", target + "-wal");
    const db = new DatabaseSync(target, { readOnly: true });
    try {
      db.exec("PRAGMA trusted_schema = OFF");
      const schema = db.prepare("SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name").all();
      const tables = Object.fromEntries(schema.filter(row => row.type === "table").map(({ name }) => {
        const read = db.prepare('SELECT * FROM "' + name.replaceAll('"', '""') + '"');
        read.setReadBigInts(true);
        const rows = read.all().map(row => {
          if (migrationClockAfter && name === "migration" && typeof row.id === "string"
              && row.id > migrationClockAfter && typeof row.time_completed === "bigint"
              && row.time_completed > 0n) {
            row.time_completed = "<engine migration time>";
          }
          if (name === "state" && row.key === "local" && typeof row.body === "string") {
            let body;
            try { body = JSON.parse(row.body); } catch {}
            if (body && typeof body === "object") {
              row.body = JSON.stringify(normalise(body, insights + "/state/local", home));
            }
          }
          return JSON.stringify(row, (_, value) => typeof value === "bigint" ? { integer: String(value) }
            : value instanceof Uint8Array ? { bytes: Buffer.from(value).toString("hex") } : value);
        });
        return [name, rows.sort()];
      }));
      return { schema, version: db.prepare("PRAGMA user_version").get(), tables };
    } finally {
      db.close();
    }
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

function inventory(home, { seed = false, enginePort } = {}) {
  const entries = new Map();
  function visit(folder, prefix = "") {
    for (const name of fs.readdirSync(folder).sort()) {
      const file = path.join(folder, name);
      const key = prefix + name;
      const stat = fs.lstatSync(file);
      if (stat.isDirectory()) {
        entries.set(key, { type: "directory" });
        visit(file, key + "/");
      } else if (stat.isSymbolicLink()) {
        const target = fs.readlinkSync(file);
        if (seed) {
          const inside = path.relative(home, fs.realpathSync(file));
          assert.ok(!path.isAbsolute(target) && !path.isAbsolute(inside) && inside !== ".." && !inside.startsWith(".." + path.sep),
            `Seed has a link outside its home at ${key}`);
        }
        entries.set(key, { type: "symlink", target: normalise(target, key, home, enginePort) });
      } else if (stat.isFile()) {
        if (!seed && databases.has(key)) {
          try {
            entries.set(key, { type: "sqlite", value: normalise(sqliteContents(file, home), key, home, enginePort) });
            continue;
          } catch (error) {
            if (error.code !== "ERR_SQLITE_ERROR") throw error;
            // A damaged database and its sidecars still show their raw differences.
          }
        }
        if (!seed && /-(wal|shm)$/.test(key) && entries.get(key.slice(0, -4))?.type === "sqlite") {
          entries.set(key, { type: key.endsWith("-wal") ? "sqlite journal" : "sqlite journal index" });
          continue;
        }
        const bytes = fs.readFileSync(file);
        const text = bytes.toString("utf8");
        if (key.endsWith(".json")) {
          let json;
          try { json = JSON.parse(text); } catch {}
          if (json !== undefined) {
            entries.set(key, { type: "json", value: normalise(json, key, home, enginePort) });
            continue;
          }
        }
        if (Buffer.from(text).equals(bytes) && !text.includes("\0")) {
          entries.set(key, { type: "file", text: normalise(text, key, home, enginePort) });
        } else {
          entries.set(key, { type: "file", sha256: createHash("sha256").update(bytes).digest("hex") });
        }
      } else {
        assert.ok(!seed, `Seed contains a running endpoint or special file at ${key}`);
        assert.ok(stat.isSocket(), `Unexpected special file at ${key}`);
        entries.set(key, { type: "socket" });
      }
    }
  }
  visit(home);
  return entries;
}

// A seed is an offline fixture. Never launch with another process's runtime records.
function checkSeed(home) {
  const actual = fs.realpathSync(home);
  const repo = fs.realpathSync(fileURLToPath(new URL("../../../", import.meta.url)));
  const insideRepo = actual.startsWith(repo + path.sep);
  const live = path.join(os.userInfo().homedir, ".kuvo-code");
  assert.ok((insideRepo || (actual !== live && !actual.startsWith(live + path.sep)))
    && !actual.split(path.sep).some(part => part.startsWith("run.kuvo.code")),
  "Use an offline test fixture in a scratch folder.");
  const files = inventory(actual, { seed: true });
  for (const file of ["link.pid", "box.pid", "serve.json", "engine.json", "bridge.json", "box-services.json", "browser.pid"]) {
    assert.ok(!files.has("run/" + file), `Remove the runtime record run/${file} from the offline fixture.`);
  }
  return actual;
}

function wireRelay(home, relay) {
  for (const name of ["run/link.json", "run/mac-link.json"]) {
    const file = path.join(home, name);
    if (!fs.existsSync(file)) continue;
    const saved = readJSON(file);
    if (typeof saved?.relay !== "string") continue;
    write(file, JSON.stringify({ ...saved, relay: relay.url }));
  }
}

function substitute(value, home) {
  if (typeof value === "string") return value.replaceAll("{{home}}", home);
  if (Array.isArray(value)) return value.map(item => substitute(item, home));
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, substitute(item, home)]));
  }
  return value;
}

function seenReach(home, { key, state }) {
  const locations = reachFiles.map(file => path.join(home, file));
  if (state === "absent") return locations.every(file => !fs.existsSync(file)) && [null, null];
  if (locations.some(file => !fs.existsSync(file))) return false;
  const maps = locations.map(readJSON);
  return maps.every(map => map && typeof map === "object" && !Array.isArray(map)
    && (state === "away" ? typeof map[key] === "number" : !Object.hasOwn(map, key))) && maps;
}

async function run(binary, home, sequence, relay, peerSeed) {
  const env = isolatedEnv(path.dirname(home));
  const engine = await standInEngine(path.dirname(home), env);
  const engineFile = path.join(home, "run", "engine.json");
  let box, peer, peerEngine;
  const answers = [];
  try {
    wireRelay(home, relay);
    write(engineFile, JSON.stringify(engine.record));
    const mac = fs.existsSync(path.join(home, "box", "kind")) && fs.readFileSync(path.join(home, "box", "kind"), "utf8").trim() === "mac";
    box = startBox({ binary, home, env: { ...env, KUVO_SANDBOX: "off" }, relay, mac });
    let record = await box.ready();
    for (const { method, path: route, body, headers, engineRunning, restartAfterAcceptance, peerReach } of sequence) {
      if (engineRunning) engine.running = new Set(engineRunning);
      const dispatch = () => request(record, method, route.replaceAll("{{home}}", encodeURIComponent(home)), substitute(body, home), headers);
      if (restartAfterAcceptance) {
        const accepted = new Promise(resolve => { engine.holdPrompt = { id: body.id, accepted: resolve }; });
        const response = dispatch().then(answer => ({ answer }), error => ({ error }));
        await Promise.race([accepted, response.then(() => { throw new Error("Send finished before the engine held its answer."); })]);
        box.child.kill("SIGKILL");
        await box.exited;
        assert.ok((await response).error, "The interrupted sender must lose its reply");
        answers.push({ interrupted: true });
        box = startBox({ binary, home, env: { ...env, KUVO_SANDBOX: "off" }, relay, mac });
        record = await box.ready();
        continue;
      }
      let reach;
      if (peerReach) {
        if (peerReach.state === "answered" && !peer) {
          assert.ok(peerSeed, "Provide the answering peer's seed home.");
          const peerRoot = path.join(path.dirname(home), "returning");
          const peerHome = path.join(peerRoot, "box");
          fs.cpSync(peerSeed, peerHome, { recursive: true, verbatimSymlinks: true });
          const peerEnv = isolatedEnv(peerRoot);
          wireRelay(peerHome, relay);
          peerEngine = await standInEngine(peerRoot, peerEnv);
          write(path.join(peerHome, "run/engine.json"), JSON.stringify(peerEngine.record));
          peer = startBox({ binary, home: peerHome, env: { ...peerEnv, KUVO_SANDBOX: "off" }, relay });
          await peer.ready();
        }
        const maps = await until(() => seenReach(home, peerReach), "The peer reach files did not reach " + peerReach.state, 18000);
        reach = maps.map((map, index) => normalise(map, reachFiles[index], home));
      }
      const answer = await dispatch();
      const name = `${method.toUpperCase()} ${route.split("?")[0]}`;
      const output = { status: answer.status, ...(reach === undefined ? {} : { reach }) };
      if (answer.json !== undefined) output.json = normalise(answer.json, name, home);
      else if (Buffer.from(answer.text).equals(answer.bytes)) output.text = normalise(answer.text, name, home);
      else output.sha256 = createHash("sha256").update(answer.bytes).digest("hex");
      answers.push(output);
    }
  } finally {
    try {
      if (box) await box.stop();
      if (peer) await peer.stop();
    } finally {
      await Promise.all([engine.close(), peerEngine?.close()]);
    }
  }
  const transcript = normalise({
    sessions: engine.sessions,
    messages: Object.fromEntries(engine.messages),
    inboxes: Object.fromEntries(engine.inboxes),
  }, "engine", home);
  return { answers, files: inventory(home, { enginePort: engine.record.port }), transcript };
}

export function difference(label, a, b, out) {
  if (isDeepStrictEqual(a, b)) return;
  if (a && b && typeof a === "object" && typeof b === "object" && Array.isArray(a) === Array.isArray(b)) {
    for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
      difference(`${label}/${key}`, Object.hasOwn(a, key) ? a[key] : undefined, Object.hasOwn(b, key) ? b[key] : undefined, out);
    }
    return;
  }
  const show = value => value === undefined ? "<missing>" : JSON.stringify(value);
  out.push(`${label}\n  A  ${show(a)}\n  B  ${show(b)}`);
}

export async function compare(seed, sequence, a, b, { peerSeed, secondSeed = seed } = {}) {
  seed = checkSeed(seed);
  secondSeed = checkSeed(secondSeed);
  if (peerSeed) peerSeed = checkSeed(peerSeed);
  assert.ok(Array.isArray(sequence) && sequence.length, "The sequence must be a nonempty JSON list.");
  for (const step of sequence) {
    assert.ok(typeof step.method === "string" && /^[A-Za-z]+$/.test(step.method) && typeof step.path === "string"
      && step.path && !step.path.startsWith("/"), 'Each request needs a method and a box-relative path.');
    if (step.engineRunning !== undefined) {
      assert.ok(Array.isArray(step.engineRunning) && step.engineRunning.every(id => typeof id === "string"));
    }
    if (step.peerReach) {
      const { key, state } = step.peerReach;
      assert.ok(typeof key === "string" && /^[A-Za-z0-9_-]{43}$/.test(key)
        && ["absent", "away", "answered"].includes(state), "Name a peer key and its expected reach state.");
    }
    if (step.restartAfterAcceptance) {
      assert.ok(step.method.toUpperCase() === "POST" && typeof step.body?.id === "string",
        "To interrupt a send, name its prompt ID in the body.");
    }
  }
  const root = fs.realpathSync(scratch());
  const relay = await localRelay();
  try {
    const one = path.join(root, "a", "box");
    const two = path.join(root, "b", "box");
    const options = { recursive: true, verbatimSymlinks: true, preserveTimestamps: true };
    fs.cpSync(seed, one, options);
    fs.cpSync(secondSeed, two, options);
    const first = await run(a, one, sequence, relay, peerSeed);
    const second = await run(b, two, sequence, relay, peerSeed);
    const out = [];
    for (const [index, step] of sequence.entries()) {
      difference(`request ${index + 1} ${step.method} ${step.path}`, first.answers[index], second.answers[index], out);
    }
    difference("engine", first.transcript, second.transcript, out);
    for (const name of [...new Set([...first.files.keys(), ...second.files.keys()])].sort()) {
      difference(`home/${name}`, first.files.get(name), second.files.get(name), out);
    }
    return out;
  } finally {
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  }
}

export async function cli(args) {
  assert.ok(args.length === 2 || args.length === 3,
    "Usage: node protocol/conformance/differential SEED_HOME REQUESTS.json [PEER_SEED_HOME]");
  const a = boxBinary(process.env.KUVO_BOX_BIN_A || "");
  const b = boxBinary(process.env.KUVO_BOX_BIN_B || "");
  const out = await compare(args[0], readJSON(args[1]), a, b, { peerSeed: args[2] });
  if (out.length) {
    console.error(out.join("\n"));
    process.exitCode = 1;
  } else {
    console.log("Answers and home files match.");
  }
}
