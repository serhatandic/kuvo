import { randomUUID } from "node:crypto";
import fs from "node:fs";
import { fork, spawnSync } from "node:child_process";
import { once } from "node:events";
import { setTimeout } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const file = fileURLToPath(import.meta.url);
export const PROCESS_SCOPE = "KUVO_TEST_HOME_PROCESS_SCOPE";

/** Own the processes and the directory even when the test worker is killed before its hooks run. */
export async function watchTestHome(root) {
  const { dev, ino } = fs.statSync(root);
  const scope = randomUUID();
  const watcher = fork(file, [root, String(dev), String(ino)], {
    env: { ...process.env, [PROCESS_SCOPE]: scope },
    detached: true, execArgv: [], stdio: ["ignore", "ignore", "inherit", "ipc"],
  });
  const done = once(watcher, "exit");
  done.catch(() => {});
  const ready = await Promise.race([once(watcher, "message"), done]);
  if (ready[0] !== "ready") throw new Error("Could not watch processes in " + root);
  watcher.unref();
  watcher.channel.unref();
  const stop = async () => {
    // Keep the calling hook alive without adding the watcher to at-end's child kill list.
    const lease = setInterval(() => {}, 60_000);
    try {
      if (watcher.connected) watcher.disconnect();
      const [code, signal] = await done;
      if (code !== 0) throw new Error(`Test home process cleanup failed for ${root} (watcher: ${signal ?? code})`);
    } finally {
      clearInterval(lease);
    }
  };
  return { scope, stop };
}

const uid = process.getuid();
function output(command, args, emptyStatus) {
  const { [PROCESS_SCOPE]: _, ...env } = process.env;
  env.LC_ALL = "C";
  const result = spawnSync(command, args, { env, encoding: "utf8", timeout: 5000, maxBuffer: 8 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0 && result.status !== emptyStatus) throw new Error(`${command}: ${result.stderr.trim() || result.signal || result.status}`);
  return result.stdout;
}

const columns = ["-ww", "-o", "uid=,pid=,ppid=,pgid=,stat=,lstart=,command="];
function processes(args) {
  const rows = new Map();
  const environment = process.platform === "darwin" ? ["-E"] : [];
  for (const line of output("/bin/ps", [...environment, ...columns, ...args], 1).split("\n")) {
    const row = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(\S+\s+\S+\s+\d+\s+\S+\s+\d+)\s+(.*)$/);
    if (!row || Number(row[1]) !== uid || row[5].startsWith("Z")) continue;
    rows.set(Number(row[2]), { parent: Number(row[3]), group: Number(row[4]), leader: row[5].includes("s"), started: row[6], command: row[7] });
  }
  return rows;
}

const sameProcess = (known, current) => current?.started === known.started && current.command === known.command;

function scopedProcesses(root, owner, groups) {
  const rows = processes(["-ax"]);
  rows.delete(process.pid);
  rows.delete(owner);
  const aliases = [root, root.replace(/^\/private\/tmp\//, "/tmp/")];
  const scope = process.env[PROCESS_SCOPE];
  const marker = `${PROCESS_SCOPE}=${scope}`;
  const scoped = new Set();
  for (const [pid, row] of rows) {
    // Darwin does not show the environment of protected tools such as /bin/sh. Those workers
    // put the fixture's executable or script first. A mention in a later argument is insufficient.
    const executable = row.command.replace(/^(?:\S+\/)?(?:sh|bash|node|bun)(?: --[^ ]+)* /, "");
    if (aliases.some(alias => executable.startsWith(`${alias}/`))) {
      scoped.add(pid);
    } else if (process.platform === "darwin") {
      if (row.command.includes(` ${marker} `) || row.command.endsWith(` ${marker}`)) scoped.add(pid);
    } else {
      try {
        if (fs.readFileSync(`/proc/${pid}/environ`, "utf8").split("\0").includes(marker)) scoped.add(pid);
      } catch (error) {
        if (!["ENOENT", "ESRCH", "EACCES", "EPERM"].includes(error.code)) throw error;
      }
    }
  }
  // A detached session can outlive its leader. Keep an isolated group only while a previously
  // observed member pins its identity, so a recycled group number never authorizes any signals.
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
    for (const [pid, row] of rows) if (scoped.has(row.parent)) scoped.add(pid);
    if (size === scoped.size) return new Map([...scoped].map(pid => [pid, rows.get(pid)]));
  }
}

async function clean(root, owner) {
  // Close the fake registration endpoint before any of its existing workers are stopped.
  fs.writeFileSync(`${root}/.closing`, "", { flag: "wx" });
  const groups = new Map();
  let quiet = 0;
  let live;
  for (let round = 0; round < 12; round++) {
    live = scopedProcesses(root, owner, groups);
    if (!live.size) {
      if (++quiet === 2) return;
    } else {
      quiet = 0;
      const current = processes(["-p", [...live.keys()].join(",")]);
      for (const [pid, row] of live) {
        const now = current.get(pid);
        if (!sameProcess(row, now)) continue;
        try { process.kill(pid, round < 2 ? "SIGTERM" : "SIGKILL"); } catch (error) {
          if (error.code !== "ESRCH") throw error;
        }
      }
    }
    await setTimeout(100);
  }
  throw new Error(`Test home still has live processes in ${root}: ${[...live.keys()].join(", ")}`);
}

if (process.argv[1] === file && process.send) {
  const [root, device, inode] = process.argv.slice(2);
  const owner = process.ppid;
  const checkRoot = () => {
    const stat = fs.lstatSync(root);
    if (!/^\/(?:private\/)?tmp\/k[A-Za-z0-9]{6}$/.test(root) || fs.realpathSync(root) !== root ||
        !stat.isDirectory() || stat.uid !== uid || (stat.mode & 0o077) !== 0 ||
        String(stat.dev) !== device || String(stat.ino) !== inode) throw new Error(`Invalid test home: ${root}`);
  };
  checkRoot();
  if (!/^[0-9a-f-]{36}$/.test(process.env[PROCESS_SCOPE] ?? "")) throw new Error("Missing test process scope");
  const disconnected = process.connected ? once(process, "disconnect") : Promise.resolve();
  if (process.connected) process.send("ready", () => {});
  await disconnected;
  checkRoot();
  await clean(root, owner);
  checkRoot();
  fs.rmSync(root, { recursive: true });
}
