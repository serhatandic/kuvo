// Reproducible native debug-box samples. Run under scripts/heavy.sh box with
// KUVO_BOX_BIN pointing to a debug executable and no other KUVO_/CLAUDE_ vars.
// The fixture creates its own home, signed-in scripted agent and local relay.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";
import { scriptedCodex, standInCodex } from "../protocol/conformance/codex.mjs";
import {
  boxBinary, boxCommand, isolatedEnv, localRelay, openLine, privateHome,
  readJSON, request, scratch, until, write,
} from "../protocol/conformance/adapter.mjs";
import { relayLeaseStandIn } from "../box/bridge-tests/relay-lease-stand-in.js";

assert.equal(process.platform, "darwin", "The comparable RSS and wake-up samples are Mac measurements");
const duration = Number(process.env.KUVO_IDLE_SAMPLE_SECONDS ?? 65);
assert.ok(Number.isFinite(duration) && duration >= 60 && duration <= 600);
const binary = boxBinary();
assert.equal(binary.typescript, false);
const follow = process.argv.includes("--follow");

function processes(owned) {
  const rows = execFileSync("ps", ["-axo", "pid=,ppid=,rss=,comm="], { encoding: "utf8" })
    .trim().split("\n").map(row => row.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/))
    .filter(Boolean).map(([, pid, ppid, rss, command]) => ({
      pid: Number(pid), ppid: Number(ppid), rssKiB: Number(rss), command,
    }));
  const selected = new Set(owned);
  for (;;) {
    const count = selected.size;
    for (const row of rows) if (selected.has(row.ppid)) selected.add(row.pid);
    if (count === selected.size) break;
  }
  return rows.filter(row => selected.has(row.pid));
}

async function counters(home, pids) {
  const folder = path.join(home, "run", "timer-fires");
  assert.ok(fs.existsSync(folder), "Use a debug executable with timer reporting enabled");
  return Promise.all(pids.map(pid => {
    const file = path.join(folder, `${pid}.json`);
    const read = () => {
      try { return readJSON(file); } catch { return null; }
    };
    const original = read()?.sequence ?? 0;
    process.kill(pid, "SIGUSR1"); // exact fixture-owned process
    return until(() => {
      const sample = read();
      return sample?.pid === pid && sample.sequence > original && sample;
    }, `Debug timer report missing for owned process ${pid}`, 5000);
  }));
}

async function sample(home, pids, label, more = () => ({})) {
  const before = await counters(home, pids);
  const began = performance.now();
  await delay(duration * 1000);
  const after = await counters(home, pids);
  const seconds = (performance.now() - began) / 1000;
  const timerFires = {};
  for (let i = 0; i < pids.length; i++) {
    const old = before[i].timers;
    for (const [name, count] of Object.entries(after[i].timers)) {
      assert.ok(name in old, `Missing baseline for ${name}`);
      assert.ok(count >= old[name], `Counter reversed for ${name}`);
      timerFires[name] = (timerFires[name] ?? 0) + count - old[name];
    }
  }
  const rows = processes(pids);
  assert.equal(rows.filter(row => pids.includes(row.pid)).length, pids.length);
  const total = Object.values(timerFires).reduce((sum, count) => sum + count, 0);
  console.log(JSON.stringify({ label, seconds, processes: rows,
    totalRssKiB: rows.reduce((sum, row) => sum + row.rssKiB, 0),
    timerFires, total, firesPerSecond: total / seconds, ...more() }));
}

async function app(record) {
  const headers = { "x-kuvo-from": "app" };
  assert.equal((await request(record, "GET", "kuvo/models/offered", undefined, headers)).status, 200);
  assert.equal((await request(record, "GET", "kuvo/agents/api/integration", undefined, headers)).status, 200);
  const topics = {
    sessions: "kuvo/sessions/list/events", bots: "kuvo/bots/events",
    work: "kuvo/work/events", background: "kuvo/work/background/events",
    queue: "kuvo/work-queue/events?owner=all&glance=1",
  };
  const query = follow ? "?watch=" + Buffer.from(JSON.stringify(topics)).toString("base64url") : "";
  const line = await openLine(record, query, "kuvo/line", headers, (duration + 15) * 1000);
  assert.equal(line.status, 200);
  return line;
}

async function direct() {
  const codex = await scriptedCodex();
  codex.env.KUVO_TEST_TIMER_REPORT = "1";
  let line;
  const agentStarts = () => ({ agentStarts: codex.heard().filter(row => row.argv).length });
  try {
    await codex.start();
    const record = codex.listener;
    await delay(3000);
    await sample(codex.home, [record.pid], "resident, no client", agentStarts);
    line = await app(record);
    await delay(3000);
    await sample(codex.home, [record.pid], `resident, one client${follow ? " following hot pages" : ""}`, agentStarts);
  } finally {
    line?.close();
    await codex.close();
  }
}

async function withKeeper() {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  privateHome(home);
  const base = isolatedEnv(root);
  const bin = standInCodex(base.HOME);
  const env = {
    ...base, CODEX_HOME: path.join(base.HOME, ".codex"), GROK_HOME: path.join(base.HOME, ".grok"),
    CLAUDE_CONFIG_DIR: path.join(base.HOME, ".claude"), KUVO_TEST_PRICE_FEEDS: "off",
    KUVO_TEST_PATH_ONLY: "1", KUVO_SANDBOX: "off", KUVO_CODEX_ROOT: base.HOME,
    KUVO_TEST_TIMER_REPORT: "1", PATH: [bin, "/usr/bin:/bin"].join(":"),
  };
  const leases = relayLeaseStandIn();
  const relay = await localRelay({ request: leases.request });
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "box/opencode.json"), "{}\n");
  const selected = "versions/0.8.0-000000000000/bin";
  write(path.join(home, "active-release"), selected + "\n");
  write(path.join(home, path.dirname(selected), "box.json"),
    JSON.stringify({ kuvo: "box/1", version: "0.8.0", launch: "rust" }));
  fs.mkdirSync(path.join(home, selected), { recursive: true });
  const executable = path.join(home, selected, "kuvo-box");
  fs.copyFileSync(binary.file, executable);
  fs.chmodSync(executable, 0o755);
  const keeper = boxCommand(binary, home, env, relay, ["keep"]);
  let record, line;
  try {
    record = await until(() => {
      try {
        const read = readJSON(path.join(home, "run/serve.json"));
        return read.pid !== keeper.child.pid && read.port && read;
      } catch { return false; }
    }, () => `Keeper did not start its resident: ${keeper.log()}`, 25000);
    const pids = [keeper.child.pid, record.pid];
    const heard = path.join(base.HOME, "codex-heard.jsonl");
    const more = () => ({
      leaseCalls: leases.calls.length,
      agentStarts: fs.existsSync(heard)
        ? fs.readFileSync(heard, "utf8").trim().split("\n")
            .filter(Boolean).map(JSON.parse).filter(row => row.argv).length
        : 0,
    });
    await delay(3000);
    await sample(home, pids, "keeper + resident, no client", more);
    line = await app(record);
    await delay(3000);
    await sample(home, pids, `keeper + resident, one client${follow ? " following hot pages" : ""}`, more);
  } finally {
    line?.close();
    const off = boxCommand(binary, home, env, relay, ["keep", "off"]);
    await off.exited;
    const ended = await Promise.race([keeper.exited, delay(10000).then(() => null)]);
    if (!ended) {
      if (record?.pid && Number(execFileSync("ps", ["-o", "ppid=", "-p", String(record.pid)], { encoding: "utf8" }).trim()) === keeper.child.pid) {
        try { process.kill(record.pid, "SIGTERM"); } catch {}
      }
      await keeper.stop();
    }
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  }
}

await (process.argv.includes("--keeper") ? withKeeper() : direct());
