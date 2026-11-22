// The only extra runtime in the container is this sampler. Exclude its memory from Kuvo.
import fs from "node:fs";
import path from "node:path";

const box = path.join(process.env.HOME, ".kuvo-code");
const run = path.join(box, "run");
const idleMs = Number(process.argv[2]) * 1000;
const check = process.argv[3] === "--check";
const record = name => JSON.parse(fs.readFileSync(path.join(run, name + ".json"), "utf8"));

async function api(record, route, body) {
  const response = await fetch(`http://127.0.0.1:${record.port}/${route}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      authorization: "Basic " + Buffer.from("opencode:" + record.password).toString("base64"),
      "content-type": "application/json",
      "x-kuvo-from": "app",
    },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(45_000),
  });
  const text = await response.text();
  if (!response.ok) throw new Error(`${route} answered ${response.status} ${text.slice(0, 500)}`);
  return text ? JSON.parse(text) : null;
}

async function until(run, description, ms) {
  const end = performance.now() + ms;
  let last;
  while (performance.now() < end) {
    try {
      const value = await run();
      if (value) return value;
    } catch (error) { last = error; }
    await Bun.sleep(500);
  }
  throw new Error(`Timed out waiting for ${description}`, { cause: last });
}

function role(argv) {
  if (argv.includes("kuvo-link.js")) return "link " + argv.slice(argv.indexOf("kuvo-link.js") + 13).trim();
  if (argv.includes("kuvo-agents.js")) return "agents bridge";
  if (argv.includes("kuvo-browser.js")) return "browser " + argv.split("kuvo-browser.js ")[1];
  if (argv.includes("/opencode serve")) return "engine";
  if (argv.includes("kuvo-start")) return "keeper";
  if (argv.includes("/chrome")) return "Chrome";
  if (argv.startsWith("sleep ")) return "keeper sleep";
  return argv.split(" ")[0];
}

function memory() {
  const rows = new Map();
  for (const pid of fs.readdirSync("/proc").filter(name => /^\d+$/.test(name))) {
    if (Number(pid) === process.pid || pid === "1") continue;
    try {
      const argv = fs.readFileSync(`/proc/${pid}/cmdline`, "utf8").replaceAll("\0", " ").trim();
      if (!argv) continue;
      const stats = fs.readFileSync(`/proc/${pid}/smaps_rollup`, "utf8");
      const rss = Number(stats.match(/^Rss:\s+(\d+)/m)?.[1]);
      const pss = Number(stats.match(/^Pss:\s+(\d+)/m)?.[1]);
      if (!Number.isFinite(rss) || !Number.isFinite(pss)) throw new Error(`Missing RSS/PSS for ${pid}`);
      rows.set(pid, { process: role(argv), rss, pss });
    } catch (error) {
      if (!["ENOENT", "ESRCH"].includes(error.code)) throw error;
    }
  }
  return rows;
}

const cpu = () => Number(fs.readFileSync("/sys/fs/cgroup/cpu.stat", "utf8").match(/^usage_usec (\d+)/m)?.[1]);

async function measure(label, done) {
  const started = performance.now();
  const initial = cpu();
  let sample;
  let peak = 0;
  do {
    const rows = memory();
    const pss = [...rows.values()].reduce((total, row) => total + row.pss, 0);
    if (pss > peak) {
      peak = pss;
      sample = [...rows.values()];
    }
    await Bun.sleep(label === "idle" ? 1000 : 200);
  } while (!done(performance.now() - started));
  const percent = (cpu() - initial) / ((performance.now() - started) * 10);
  const mib = kib => Number((kib / 1024).toFixed(1));
  const rows = (sample ?? []).map(({ process, rss, pss }) => ({ process, rssMiB: mib(rss), pssMiB: mib(pss) }));
  console.log(JSON.stringify({ label, seconds: Math.round((performance.now() - started) / 1000), cpuPercentOneCore: Number(percent.toFixed(2)), peakPssMiB: mib(peak), processesAtPeak: rows }));
  return { pss: peak, percent };
}

try {
  const serve = await until(async () => {
    const serve = record("serve");
    await api(serve, "kuvo/info");
    return serve;
  }, "the resident box", 120_000);
  // Allow the keeper, bridge and services to finish their first rounds before sampling idle.
  await Bun.sleep(20_000);
  const idle = await measure("idle", elapsed => elapsed >= idleMs);
  if (check && (idle.pss > 300 * 1024 || idle.percent > 5)) {
    console.error("Idle budget exceeded (300 MiB PSS and 5% of one CPU core).");
    process.exitCode = 1;
  }
  for (const count of [1, 2]) {
    let finished = false;
    const sample = measure(`${count} session(s)`, () => finished);
    try {
      await Promise.all(Array.from({ length: count }, async () => {
        const session = (await api(serve, "api/session", { location: { directory: path.join(box, "workspace") } })).data;
        await api(serve, `api/session/${session.id}/model`, { model: { providerID: "opencode", id: "big-pickle" } });
        await api(serve, `api/session/${session.id}/prompt`, { text: "Write a short paragraph about the water cycle without calling tools.", delivery: "steer" });
        const done = await until(async () => {
          const messages = (await api(serve, `api/session/${session.id}/message?limit=10`)).data;
          return messages.find(message => message.type === "idle");
        }, "the free model's turn to end", 120_000);
        if (done.outcome === "failed") throw new Error(`The free model's turn failed: ${JSON.stringify(done.error)}`);
      }));
    } finally {
      finished = true;
      await sample;
    }
  }
} catch (error) {
  console.error(error);
  for (const name of ["serve", "engine", "agents", "box-services"]) {
    try { console.error(name, fs.readFileSync(path.join(run, name + ".log"), "utf8").split("\n").slice(-15).join("\n")); } catch {}
  }
  process.exitCode = 1;
}
