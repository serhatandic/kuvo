import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { setTimeout } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const file = fileURLToPath(import.meta.url);
const PROCESS_SCOPE = "KUVO_TEST_HOME_PROCESS_SCOPE";
const uid = process.getuid?.();
const darwin = process.platform === "darwin";
// Keep these outside test-safety's filesystem and process hooks.
const { appendFileSync, copyFileSync, mkdtempSync, rmSync } = fs;
const startNode = spawn;
const startBun = globalThis.Bun?.spawn.bind(globalThis.Bun);

function startMonitor(args, cwd) {
  const env = { PATH: "/usr/bin:/bin", LC_ALL: "C", BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" };
  if (startBun) {
    const monitor = startBun([process.execPath, "--no-env-file", ...args], {
      cwd, env, detached: true, stdin: "pipe", stdout: "ignore", stderr: "inherit",
    });
    monitor.unref();
    return { done: monitor.exited, stop: () => { monitor.ref(); monitor.stdin.end(); } };
  }
  const monitor = startNode(process.execPath, args, { cwd, env, detached: true, stdio: ["pipe", "ignore", "inherit"] });
  const done = new Promise((resolve, reject) => {
    monitor.once("error", reject);
    monitor.once("exit", (code, signal) => resolve(signal ?? code));
  });
  monitor.stdin.on("error", () => {}); // The monitor may already have closed its end.
  if (!monitor.pid) {
    done.catch(() => {});
    throw new Error("Could not start test process monitor");
  }
  monitor.stdin.unref();
  monitor.unref();
  return { done, stop: () => { monitor.ref(); monitor.stdin.end(); } };
}

/** The pipe is owned solely by the runner. Its EOF works even after SIGKILL. */
export function watchTestProcesses(parents = []) {
  const scope = randomUUID();
  // Keep the journal outside fixture roots, which the test or an outer script may remove first.
  const journal = mkdtempSync("/tmp/kuvo-test-processes-");
  let monitor;
  try {
    // A remote wrapper may delete its scratch checkout as soon as its runner dies.
    const script = path.join(journal, "monitor.mjs");
    copyFileSync(file, script);
    monitor = startMonitor([script, journal, scope, String(process.pid), ...parents.map(String)], journal);
  } catch (error) {
    rmSync(journal, { recursive: true, force: true });
    throw error;
  }
  const done = monitor.done.then(result => {
    if (result !== 0) throw new Error(`Test process cleanup failed (${result})`);
  });
  done.catch(error => console.error(error.message));
  let stopping = false;

  return {
    env: env => ({ ...env, [PROCESS_SCOPE]: scope }),
    addRoot(root, remove = false) {
      const stat = fs.lstatSync(root);
      if (!path.isAbsolute(root) || fs.realpathSync(root) !== root ||
          stat.uid !== uid || !stat.isDirectory() || (stat.mode & 0o077) !== 0 ||
          /\/(?:\.kuvo-code|run\.kuvo\.code[^/]*)\//i.test(root + "/")) return;
      appendFileSync(path.join(journal, "roots"), JSON.stringify({ root, dev: stat.dev, ino: stat.ino, remove }) + "\n", { mode: 0o600 });
    },
    stop() {
      if (!stopping) {
        stopping = true;
        monitor.stop();
      }
      return done;
    },
  };
}

async function processRows(stdout) {
  const rows = new Map();
  for await (const line of createInterface({ input: stdout })) {
    const row = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(\S+\s+\S+\s+\d+\s+\S+\s+\d+)\s+(.*)$/);
    if (row && Number(row[1]) === uid && !row[5].startsWith("Z")) {
      rows.set(Number(row[2]), { parent: Number(row[3]), group: Number(row[4]), leader: row[5].includes("s"), started: row[6], command: row[7] });
    }
  }
  return rows;
}

async function processes(args) {
  // Under load a process listing can take longer than the runner's grace period. Read it
  // incrementally and let it finish, even when many processes have large environments.
  for (;;) {
    const ps = startNode("/bin/ps", [
      ...(darwin ? ["-E"] : []), "-ww", "-o", "uid=,pid=,ppid=,pgid=,stat=,lstart=,command=", ...args,
    ], { stdio: ["ignore", "pipe", "inherit"] });
    try {
      const [[status, signal], rows] = await Promise.all([once(ps, "close"), processRows(ps.stdout)]);
      if (status !== 0 && !(args[0] === "-p" && status === 1 && !rows.size)) {
        throw new Error(`ps failed (${signal ?? status})`);
      }
      return rows;
    } catch (error) {
      if (!["EAGAIN", "ENOMEM", "EINTR"].includes(error.code)) throw error;
      await setTimeout(250);
    }
  }
}

const sameProcess = (known, current) => current?.started === known.started && current.command === known.command;

const scriptAt = command => command
  .replace(/^(?:\S+\/)?(?:sh|bash|dash|node|bun)(?: --?[\w=./-]*)* +/, "")
  .replace(/\/{2,}/g, "/");

function hasScope(pid, command, marker) {
  if (darwin) return command.includes(` ${marker} `) || command.endsWith(` ${marker}`);
  try {
    return fs.readFileSync(`/proc/${pid}/environ`, "utf8").split("\0").includes(marker);
  } catch (error) {
    if (!["ENOENT", "ESRCH", "EACCES", "EPERM"].includes(error.code)) throw error;
    return false;
  }
}

function scopedProcesses(rows, roots, owner, marker, groups) {
  for (const [pid, row] of rows) {
    if (pid === process.pid || / \/tmp\/kuvo-test-processes-[A-Za-z0-9]{6}\/monitor\.mjs /.test(row.command)) rows.delete(pid);
  }
  const scoped = new Set();
  for (const [pid, row] of rows) {
    if (pid === owner) continue;
    // Protected tools on macOS hide their environment, and some keepers clear it. The executable
    // (or interpreter's script) may still be in the test's registered home.
    const executable = scriptAt(row.command);
    if (roots.some(root => executable.startsWith(root + "/")) || hasScope(pid, row.command, marker)) scoped.add(pid);
  }
  // Keep an isolated group only as long as a known member still pins its identity.
  for (const [group, members] of groups) {
    if (![...members].some(([pid, previous]) => rows.get(pid)?.group === group && sameProcess(previous, rows.get(pid)))) {
      groups.delete(group);
    }
  }
  for (const pid of scoped) {
    const row = rows.get(pid);
    if (row.group === pid && row.leader) groups.set(pid, new Map());
  }
  for (const [pid, row] of rows) {
    const members = groups.get(row.group);
    if (members) {
      members.set(pid, row);
      scoped.add(pid);
    }
  }
  for (;;) {
    const size = scoped.size;
    for (const [pid, row] of rows) {
      if (scoped.has(row.parent)) scoped.add(pid);
    }
    if (size === scoped.size) return new Map([...scoped].map(pid => [pid, rows.get(pid)]));
  }
}

function rootsIn(journal) {
  let stored;
  try { stored = fs.readFileSync(path.join(journal, "roots"), "utf8"); }
  catch (error) { if (error.code !== "ENOENT") throw error; return []; }
  // An incomplete last write is possible if the runner was killed during registration, before it
  // could launch anything in that root.
  return stored.split("\n").slice(0, -1).map(line => JSON.parse(line));
}

function sameRoot({ root, dev, ino }) {
  try {
    const now = fs.lstatSync(root);
    return now.dev === dev && now.ino === ino && now.isDirectory();
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    return false;
  }
}

async function clean(journal, owner, scope) {
  const roots = rootsIn(journal);
  const managed = roots.filter(entry => entry.remove);
  for (const entry of managed) {
    if (!sameRoot(entry)) continue;
    try { fs.writeFileSync(path.join(entry.root, ".closing"), "", { flag: "wx" }); }
    catch (error) { if (error.code !== "EEXIST" && error.code !== "ENOENT") throw error; }
  }
  const aliases = roots.flatMap(({ root }) => [root, root.replace(/^\/private\//, "/")]);
  const groups = new Map();
  let quiet = 0;
  let live;
  for (let round = 0; round < 20; round++) {
    const rows = await processes(darwin ? ["-x", "-U", String(uid)] : ["-ax"]);
    live = scopedProcesses(rows, aliases, owner, `${PROCESS_SCOPE}=${scope}`, groups);
    if (!live.size) {
      if (++quiet === 2) {
        for (const entry of managed) if (sameRoot(entry)) rmSync(entry.root, { recursive: true, force: true });
        return;
      }
    } else {
      quiet = 0;
      const current = await processes(["-p", [...live.keys()].join(",")]);
      const keepers = [], workers = [];
      for (const [pid, row] of live) {
        if (!sameProcess(row, current.get(pid))) continue;
        const isKeeper = /^\S+\/(?:kuvo-start|\.kuvo-start-keeper) keep(?: |$)/.test(scriptAt(row.command));
        (isKeeper ? keepers : workers).push(pid);
      }
      // Stop the keep loops before their workers so they cannot launch replacements.
      for (const [pids, signal] of [[keepers, "SIGKILL"], [workers, round < 2 ? "SIGTERM" : "SIGKILL"]]) {
        for (const pid of pids) {
          try { process.kill(pid, signal); } catch (error) { if (error.code !== "ESRCH") throw error; }
        }
      }
    }
    await setTimeout(100);
  }
  throw new Error(`Test processes still running: ${[...live.keys()].join(", ")}`);
}

if (path.basename(file) === "monitor.mjs" && fs.realpathSync(process.argv[1]) === fs.realpathSync(file)) {
  const [journal, scope, ownerArg, ...parents] = process.argv.slice(2);
  const owner = Number(ownerArg);
  if (!/^\/tmp\/kuvo-test-processes-[A-Za-z0-9]{6}$/.test(journal ?? "") ||
      !/^[0-9a-f-]{36}$/.test(scope ?? "") || !Number.isSafeInteger(owner) || owner <= 1) {
    throw new Error("Invalid test process monitor arguments");
  }
  // node --test uses a separate worker. If the CLI or the test shell is killed, freeze the worker
  // before cleaning up so it can't start more boxes. Only signal this monitor's own parent.
  const parentCheck = parents.length ? setInterval(() => {
    if (process.ppid !== owner) return;
    for (const parent of parents) {
      try {
        if (Number(parent) <= 1) throw new Error("runner already gone");
        process.kill(Number(parent), 0);
      } catch (error) {
        if (error.code && error.code !== "ESRCH") continue;
        if (process.ppid === owner) {
          try { process.kill(owner, "SIGKILL"); }
          catch (error) { if (error.code !== "ESRCH") throw error; }
        }
        break;
      }
    }
  }, 250) : undefined;
  // The runner's pipe owns our lifetime even if cancellation also signals the watchdog.
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(signal, () => {});
  try {
    for await (const _ of process.stdin) { /* Wait for the runner's pipe to close. */ }
  } finally {
    clearInterval(parentCheck);
    await clean(journal, owner, scope);
    // If cleanup itself fails, leave the ownership journal available for diagnosis.
    rmSync(journal, { recursive: true, force: true });
  }
}
