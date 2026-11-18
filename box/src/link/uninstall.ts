// Removing Kuvo Code from this computer (#333 piece 3; PROTOCOL.md, "Uninstalling"): the box's own
// command, which the app runs (Settings, once it's there) or you run in a Terminal, on a Mac and on
// Linux alike. `kuvo-link.js uninstall`: nothing of it starts again, at login or after a restart,
// and nothing of it runs (its keeper, engine, coding agents, link, server and browser). Its folder
// stays for you to remove (or move to the Trash): the command only says where it is.
// `uninstall --check` changes nothing, and says what it would stop.
//
// What starts the box from outside its folder is only taken away for the real `~/.kuvo-code`: the
// launch agent (`~/Library/LaunchAgents/run.kuvo.code.machine.plist`, which launchd lets go of
// first) and its line in your crontab (by its mark, the rest of it kept). A test's home names stand-
// ins for `launchctl` and `crontab` (KUVO_TEST_UNINSTALL_TOOLS), else leaves both alone; a demo or
// test copy's never added either.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../shared/json";
import { engineRole, runsAs } from "../shared/platform";
import { allWakes } from "./areas/wakes";
import { listBots } from "./bots/list";
import { APP_FOLDER, IN_TEST_FOLDER, IS_MAC, K, REAL_BOX, RUN, accountHome, homeFolder, located, recordedKind } from "./home";
import { KEY, ownKey, peersOf } from "./peers/known";
import { publicOf } from "./crypto/keys";
import { noteDocsChanged } from "./documents/changed";
import { removeAsMac } from "./phones";
import { SETUP_CARD } from "./setup/card";
import { mainElsewhere } from "./setup/main-only";
import { LAUNCH_AGENT, REBOOT_TAG } from "./marks";
import { SERVE_ROLE } from "./serve-kept";
import { readLink, updateLink } from "./state";
import { forwardAct } from "./work-queue/forward";

/** Where `launchctl`, `crontab` and `open` are, and the folder the launch agent's file is in: the
 * real ones for the real `~/.kuvo-code` alone, a test's stand-ins for a test's home, else none. (The
 * helper's launch agent is put in place with the same, and the helper opened, helper.ts.) */
export function launchTools(): { launchctl: string; crontab: string; open: string; security: string; agents: string } | null {
  if (REAL_BOX && located(K) === located(path.join(accountHome(), ".kuvo-code"))) {
    return { launchctl: "/bin/launchctl", crontab: "crontab", open: "/usr/bin/open", security: "/usr/bin/security", agents: path.join(accountHome(), "Library", "LaunchAgents") };
  }
  const tools = IN_TEST_FOLDER ? process.env.KUVO_TEST_UNINSTALL_TOOLS : undefined;
  if (!tools || !path.isAbsolute(tools)) return null;
  return { launchctl: path.join(tools, "launchctl"), crontab: path.join(tools, "crontab"), open: path.join(tools, "open"), security: path.join(tools, "security"), agents: path.join(homeFolder(), "Library", "LaunchAgents") };
}

const run = (program: string, args: string[], input?: string) =>
  spawnSync(program, args, { input, encoding: "utf8", timeout: 30_000, stdio: ["pipe", "pipe", "pipe"] });

/** This user's crontab, or null where there's none to read (no `crontab`, or it can't be read: then
 * it's left alone, never written over). */
function crontab(tools: { crontab: string }): string | null {
  const listed = run(tools.crontab, ["-l"]);
  if (listed.error) return null;
  if (listed.status === 0) return listed.stdout;
  return /no crontab/i.test(listed.stderr ?? "") ? "" : null;
}

/** What starts the box from outside its folder: the launch agent's file, and its crontab line. */
function starts() {
  const tools = launchTools();
  if (!tools) return { launchAgent: false, crontab: false, checked: false };
  const table = crontab(tools);
  return {
    launchAgent: process.platform === "darwin" && fs.existsSync(path.join(tools.agents, `${LAUNCH_AGENT}.plist`)),
    crontab: table !== null && table.includes(REBOOT_TAG),
    checked: true,
  };
}

/** What runs of the box, each by its record in `run` and the command line it runs with. */
const RECORDS: [file: string, role: RegExp, what: string][] = [
  ["box.pid", /kuvo-start(-keeper)? keep(\s|$)/, "keeper"],
  ["engine.json", engineRole(K), "engine"],
  ["agents.pid", /kuvo-agents/, "coding agents"],
  ["link.pid", /kuvo-link\.js run(\s|$)/, "link"],
  ["serve.json", SERVE_ROLE, "server"],
  ["browser.pid", /kuvo-browser\.js host(\s|$)/, "browser"],
];

/** A process recorded in `run` (`{"pid":…}` or a bare number), when it runs as `role` says (one
 * whose number another process took since isn't it). */
function recorded(file: string, role: RegExp): number | null {
  let text = "";
  try { text = fs.readFileSync(path.join(RUN, file), "utf8"); } catch { return null; }
  let pid: unknown = Number(text.trim());
  if (!Number.isInteger(pid)) try { pid = JSON.parse(text)?.pid; } catch {}
  return runsAs(pid, role) ? pid as number : null;
}

/** The coding agents' processes kept under holders (run/agents/hold, the bridge's `hold/`), by
 * their holders' records: each holder that still runs. */
function holders(): number[] {
  const folder = path.join(RUN, "agents", "hold");
  let names: string[] = [];
  try { names = fs.readdirSync(folder).filter(name => /^[0-9a-f]{12}\.json$/.test(name)); } catch { return []; }
  // (Each only once its process is that very holder: its program and its id on its command line.)
  return names.flatMap(name => {
    const id = name.slice(0, 12);
    try {
      const pid = JSON.parse(fs.readFileSync(path.join(folder, name), "utf8"))?.pid;
      return runsAs(pid, new RegExp(`kuvo-hold-[0-9a-f]+\\.js .* ${id} (jsonrpc|claude|raw)\\s*$`)) ? [pid as number] : [];
    } catch { return []; }
  });
}

/** What runs of the box now, by its records. */
function running(): string[] {
  return [...RECORDS.filter(([file, role]) => recorded(file, role) !== null).map(([, , what]) => what), ...(holders().length ? ["coding agents' processes"] : [])];
}

/** The box's keeper, `bin/kuvo-start`, when it names this very folder (it says `K="$HOME/<name>"`,
 * run with `HOME` the folder around it); null otherwise (none, or another folder's script). */
function ownKeeper(): string | null {
  const keeper = path.join(K, "bin", "kuvo-start");
  let text = "";
  try { text = fs.readFileSync(keeper, "utf8"); } catch { return null; }
  const named = text.match(/^K="\$HOME\/([^"/]+)"/m)?.[1];
  return named === path.basename(K) ? keeper : null;
}

/** What uninstalling stops, as facts for whoever asks to say: what runs, what starts it, its bots
 * and wake-ups, the phones that reach it and your other computers it talks to. */
export function uninstallCheck() {
  const link = readLink();
  const wakes = allWakes().reduce((count, each) => count + each.wakes.length + each.watches.length, 0);
  return {
    folder: K,
    kind: recordedKind() ?? "machine",
    running: running(),
    starts: starts(),
    bots: listBots().length,
    wakes,
    phones: (link?.phones ?? []).length,
    computers: peersOf(link).length,
  };
}

/** What's done: whether nothing of it runs (`stopped`), what's taken away (its launch agent, its
 * crontab line), what would still start it (`startsAgain`: one found but not taken away), what's
 * left (its folder), and what to say. */
export interface Uninstalled {
  stopped: boolean;
  launchAgent: boolean;
  crontab: boolean;
  startsAgain: string[];
  left: string[];
  message: string;
}

/** Removes what starts and runs the box. */
export function uninstall(): Uninstalled {
  const tools = launchTools();
  let launchAgent = false;
  let crontabLine = false;
  const startsAgain: string[] = [];
  // Its start at login first, so nothing starts it again as it stops.
  if (tools && process.platform === "darwin") {
    const plist = path.join(tools.agents, `${LAUNCH_AGENT}.plist`);
    if (fs.existsSync(plist)) {
      const uid = process.getuid?.() ?? 0;
      run(tools.launchctl, ["bootout", `gui/${uid}/${LAUNCH_AGENT}`]);
      try { fs.rmSync(plist, { force: true }); } catch {}
      if (fs.existsSync(plist)) startsAgain.push("its launch agent"); else launchAgent = true;
    }
  }
  if (tools) {
    const table = crontab(tools);
    if (table !== null && table.includes(REBOOT_TAG)) {
      const kept = table.split("\n").filter(line => !line.includes(REBOOT_TAG)).join("\n");
      crontabLine = run(tools.crontab, ["-"], kept.endsWith("\n") || !kept ? kept : `${kept}\n`).status === 0;
      if (!crontabLine) startsAgain.push("its line in your crontab");
    }
  }
  // Then everything it runs: marked off (so nothing starts it meanwhile), by its own keeper
  // (`kuvo-start off`, run for this folder alone), then what's still running by its records (its
  // server for clients, its browser; what a keeper of another folder's didn't stop).
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  for (const file of ["serve.needed", "link.needed"]) fs.rmSync(path.join(RUN, file), { force: true });
  fs.writeFileSync(path.join(RUN, "off"), "");
  const keeper = ownKeeper();
  let stopped = true;
  if (keeper) {
    const off = spawnSync("sh", [keeper, "off"], { cwd: K, encoding: "utf8", timeout: 120_000, stdio: ["ignore", "pipe", "pipe"], env: { ...process.env, HOME: path.dirname(K) } });
    stopped = off.status === 0;
  }
  const told: [number, RegExp][] = [];
  for (const [file, role] of RECORDS) {
    const pid = recorded(file, role);
    if (pid && pid !== process.pid) try { process.kill(pid, "SIGTERM"); told.push([pid, role]); } catch {}
  }
  // (Each holder stops its agent as it's told: its stdin closed, then told, then made to after 15 s.)
  for (const pid of holders()) try { process.kill(pid, "SIGTERM"); told.push([pid, /kuvo-hold-[0-9a-f]+\.js /]); } catch {}
  // (A moment for those to end, twenty seconds at most.)
  const pause = new Int32Array(new SharedArrayBuffer(4));
  for (let waited = 0; waited < 400 && told.some(([pid, role]) => runsAs(pid, role)); waited++) Atomics.wait(pause, 0, 0, 50);
  const left = running();
  stopped &&= left.length === 0;
  const ran = left.length ? `its ${left.join(", ")} still ran just now` : "it couldn't stop everything just now";
  const starts = `${startsAgain.join(" and ")} couldn't be taken away`;
  const message = !stopped && startsAgain.length
    ? `Kuvo Code isn't removed yet: ${ran}, and ${starts}. Run this again in a moment.`
    : !stopped
      ? `Kuvo Code won't start again on this computer, but ${ran}. Run this again in a moment.`
      : startsAgain.length
        ? `Kuvo Code is stopped on this computer, but ${starts}, so it may start again. Run this again in a moment.`
        : "Kuvo Code is stopped on this computer and won't start again. Its folder stays, with your sessions and settings.";
  return { stopped, launchAgent, crontab: crontabLine, startsAgain, left: [K], message };
}

const identityFile = path.join(K, "desktop", "identity");

function logCleanup(detail: unknown): void {
  try { fs.appendFileSync(path.join(RUN, "uninstall.log"), `${String(detail)}\n`, { mode: 0o600 }); } catch {}
}

function desktopKey(): string | undefined {
  try {
    const [key, secret] = fs.readFileSync(identityFile, "utf8").trim().split(".");
    if (key && secret && KEY.test(key) && KEY.test(secret)) return publicOf(Buffer.from(key, "base64url")).toString("base64url");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") logCleanup(error);
  }
  try {
    const kept: unknown = JSON.parse(fs.readFileSync(path.join(K, "desktop", "boxes.json"), "utf8"));
    if (isObject(kept) && typeof kept.paired === "string" && KEY.test(kept.paired)) return kept.paired;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") logCleanup(error);
  }
  return undefined;
}

async function revokeDesktop(): Promise<boolean> {
  try {
    const link = readLink();
    const main = mainElsewhere(link);
    if (main) {
      let key = link && ownKey(link);
      // A Mac command may have only its public setup card; its running box holds the private keys.
      if (!key && IS_MAC) {
        const card: unknown = JSON.parse(fs.readFileSync(SETUP_CARD, "utf8"));
        if (isObject(card) && isObject(card.self) && typeof card.self.key === "string") key = card.self.key;
      }
      if (!key || !KEY.test(key)) return false;
      const reply = await forwardAct({ ...main, name: main.name || "your main computer", kind: "mac" }, "This computer",
        { area: "devices", method: "DELETE", item: key }, undefined, link, Date.now(), { backgroundRelay: IS_MAC });
      if (!("refused" in reply) && reply.status === 200) return true;
      logCleanup("refused" in reply ? reply.refused : `Device revocation returned ${reply.status}`);
      return false;
    }
    const key = desktopKey();
    if (key) {
      const changed = updateLink(current => current && current.phones?.some(phone => phone.key === key) && removeAsMac(current, [{ key }]) ? current : null);
      if (changed) noteDocsChanged();
    }
    return true;
  } catch (error) {
    logCleanup(error);
    return false;
  }
}

function clearDesktopIdentity(): boolean {
  let cleared = true;
  try { fs.rmSync(identityFile, { force: true }); } catch (error) { logCleanup(error); cleared = false; }
  // Only the real Mac's own root can name the released app's item. Other roots own their files.
  if (process.platform === "darwin" && REAL_BOX && located(K) === located(path.join(accountHome(), ".kuvo-code"))) {
    const removed = run("/usr/bin/security", ["delete-generic-password", "-s", "run.kuvo.code.desktop", "-a", "link-identity"]);
    if (removed.status !== 0 && removed.status !== 44) {
      logCleanup(removed.error ?? removed.stderr);
      cleared = false;
    }
  }
  return cleared;
}

/** `uninstall [--check]`: prints one JSON line, says what it did on stderr, and exits 0 when it's
 * done, 1 when refused or incomplete, 2 for what it doesn't know. */
export async function uninstallCommand(args: string[]): Promise<number> {
  const unknown = args.filter(arg => arg !== "--check");
  if (unknown.length) {
    console.error("Usage: kuvo-link.js uninstall [--check]");
    return 2;
  }
  // This Mac's box in its app's folder (however it was started): the app's own keeper runs it,
  // which only the app removes.
  if (APP_FOLDER || path.basename(path.dirname(path.resolve(K))).toLowerCase().startsWith("run.kuvo.code")) {
    const message = "Kuvo on this Mac runs in Kuvo Code's own folder: remove Kuvo Code itself (its background work goes with it).";
    console.log(JSON.stringify({ result: "refused", message }));
    console.error(message);
    return 1;
  }
  // A folder that isn't a box (its keeper and run folder both missing: a name mistyped, say).
  if (!fs.existsSync(path.join(K, "bin", "kuvo-start")) && !fs.existsSync(RUN)) {
    const message = "There’s no Kuvo Code box in this folder.";
    console.log(JSON.stringify({ result: "refused", message }));
    console.error(message);
    return 1;
  }
  if (args.includes("--check")) {
    console.log(JSON.stringify(uninstallCheck()));
    return 0;
  }
  // The SSH controller has already revoked access before asking its remote to stop.
  const overSSH = process.env.KUVO_UNINSTALL_FROM_SSH === "1";
  const revoked = overSSH || await revokeDesktop();
  const identityCleared = overSSH || clearDesktopIdentity();
  let done: Uninstalled;
  try {
    done = uninstall();
  } catch (error) {
    logCleanup(error);
    done = { stopped: false, launchAgent: false, crontab: false, startsAgain: [], left: [K], message: "Kuvo Code couldn’t confirm it had stopped on this computer." };
  }
  if (!revoked) done.message += " Kuvo Code couldn’t confirm that your main computer revoked this desktop’s access.";
  if (!identityCleared) done.message += " Kuvo Code couldn’t delete this desktop’s local identity.";
  const finished = done.stopped && done.startsAgain.length === 0 && revoked && identityCleared;
  console.log(JSON.stringify({ result: finished ? "uninstalled" : "unfinished", ...done }));
  console.error(done.message);
  return finished ? 0 : 1;
}
