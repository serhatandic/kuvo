import { BOX_SERVICES_ROLE } from "../shared/box-services";
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
import type { CommittedReceipt, SaveEnvelope } from "../shared/routes/setup-save";
import { isObject, parsed } from "../shared/json";
import { engineRole, runsAs } from "../shared/platform";
import { uninstallCleanup } from "./add/scripts";
import { allWakes } from "./areas/wakes";
import { listBots } from "./bots/list";
import { APP_FOLDER, IN_TEST_FOLDER, IS_MAC, K, REAL_BOX, RUN, accountHome, homeFolder, located, recordedKind } from "./home";
import { KEY, ownKey, peersOf } from "./peers/known";
import { publicOf } from "./crypto/keys";
import { writeDurablePrivate } from "./files";
import { publishDesktopRemoval } from "./documents/phone-sync";
import { DEVICES_REMOVE_FEATURE, removeDevicesHere } from "./peers/devices";
import { SETUP_CARD } from "./setup/card";
import { mainElsewhere } from "./setup/main-only";
import { LAUNCH_AGENT, MACHINE_WATCH_AGENT, REBOOT_TAG } from "./marks";
import { SERVE_FILE, SERVE_ROLE } from "./serve-kept";
import { macLink, readLink, updateLink } from "./state";
import { installedWslTask, removeWslTask } from "./windows";
import { forwardAct, takesForwarded } from "./work-queue/forward";

import type { AnswerPiece, RouteContext } from "./server/context";

/** Uninstalling the main computer of more leaves the setup with none: change it first. */
const MAIN_UNINSTALL = "Choose another main computer first.";

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

/** Stop the old app's keeper and fence its launchd registration before the app data is detached. */
export async function stopAppOwnedKeeper(folder: string): Promise<void> {
  const failed = "Kuvo Code’s app-owned background work could not be stopped.";
  let record: unknown;
  try {
    const fd = fs.openSync(path.join(RUN, "keeper.json"), fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
    try {
      const info = fs.fstatSync(fd);
      if (!info.isFile() || info.size > 16_384 || (info.mode & 0o077) !== 0) throw new Error(failed);
      record = parsed(fs.readFileSync(fd));
      if (!isObject(record)) throw new Error(failed);
    } finally { fs.closeSync(fd); }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const real = process.platform === "darwin" && located(homeFolder()) === located(accountHome());
  if (real) {
    const service = `gui/${process.getuid?.() ?? 0}/${path.basename(folder)}.box`;
    if (run("/bin/launchctl", ["disable", service]).status !== 0) throw new Error(failed);
    run("/bin/launchctl", ["bootout", service]);
    const loaded = run("/bin/launchctl", ["print", service]);
    if (loaded.status !== 113 && !/could not find service/i.test(loaded.stderr ?? "")) throw new Error(failed);
  }
  if (!isObject(record) || !runsAs(record.pid, /(^|\s)--box(\s|$)/)) return;
  if (typeof record.url !== "string" || !/^http:\/\/127\.0\.0\.1:\d{1,5}\/?$/.test(record.url) || typeof record.password !== "string") throw new Error(failed);
  const stop = await fetch(`${record.url.replace(/\/$/, "")}/stop`, {
    method: "POST", headers: { authorization: `Bearer ${record.password}` }, signal: AbortSignal.timeout(20_000),
  });
  if (!stop.ok) throw new Error(failed);
  const deadline = Date.now() + 20_000;
  while (runsAs(record.pid, /(^|\s)--box(\s|$)/)) {
    if (Date.now() >= deadline) throw new Error(failed);
    await new Promise(resolve => setTimeout(resolve, 100));
  }
}

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
  const windowsTask = installedWslTask() !== undefined;
  if (!tools) return { launchAgent: false, crontab: false, checked: windowsTask, ...(windowsTask ? { windowsTask } : {}) };
  const table = crontab(tools);
  return {
    launchAgent: process.platform === "darwin" && fs.existsSync(path.join(tools.agents, `${LAUNCH_AGENT}.plist`)),
    crontab: table !== null && table.includes(REBOOT_TAG),
    checked: true,
    ...(windowsTask ? { windowsTask } : {}),
  };
}

/** What runs of the box, each by its record in `run` and the command line it runs with. */
function processRecords(): [file: string, role: RegExp, what: string][] {
  return [
    ["box.pid", /kuvo-start(-keeper)? keep(\s|$)/, "keeper"],
    ["engine.json", engineRole(K), "engine"],
    ["agents.pid", /kuvo-agents/, "coding agents"],
    ["link.pid", /kuvo-link\.js (?:run|serve --resident)(\s|$)/, "link"],
    ["serve.json", SERVE_ROLE, "server"],
    ["box-services.json", BOX_SERVICES_ROLE, "box services"],
    ["browser.pid", /kuvo-browser\.js host(\s|$)/, "browser"],
  ];
}

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
  return [...processRecords().filter(([file, role]) => recorded(file, role) !== null).map(([, , what]) => what), ...(holders().length ? ["coding agents' processes"] : [])];
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
  // App-owned wipes fence their own keeper separately. These account-wide entries belong
  // to the canonical installation, which can still be present beside an app-owned Box.
  const tools = APP_FOLDER ? null : launchTools();
  let launchAgent = false;
  let crontabLine = false;
  const startsAgain: string[] = [];
  try { removeWslTask(); }
  catch (error) { logCleanup(error); startsAgain.push("its Windows sign-in task"); }
  // Its start at login first, so nothing starts it again as it stops.
  if (tools && process.platform === "darwin") for (const label of [MACHINE_WATCH_AGENT, LAUNCH_AGENT]) {
    const plist = path.join(tools.agents, `${label}.plist`);
    if (fs.existsSync(plist)) {
      const uid = process.getuid?.() ?? 0;
      run(tools.launchctl, ["bootout", `gui/${uid}/${label}`]);
      try { fs.rmSync(plist, { force: true }); } catch {}
      if (fs.existsSync(plist)) startsAgain.push(label === LAUNCH_AGENT ? "its launch agent" : "its Files permission watcher");
      else if (label === LAUNCH_AGENT) launchAgent = true;
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
  const keeper = APP_FOLDER ? null : ownKeeper();
  let stopped = true;
  if (keeper) {
    const off = spawnSync("sh", [keeper, "off"], { cwd: K, encoding: "utf8", timeout: 120_000, stdio: ["ignore", "pipe", "pipe"], env: { ...process.env, HOME: path.dirname(K) } });
    stopped = off.status === 0;
  }
  const told: [number, RegExp][] = [];
  for (const [file, role] of processRecords()) {
    const pid = recorded(file, role);
    if (pid && pid !== process.pid && !told.some(([known]) => known === pid)) try { process.kill(pid, "SIGTERM"); told.push([pid, role]); } catch {}
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
export const DESKTOP_UNINSTALL_ROUTE = "kuvo/box/uninstall-desktop";

export function logCleanup(detail: unknown): void {
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

type DesktopRevocation = { status: "revoked" | "unconfirmed" | "retry" };
const REVOCATION_UNCONFIRMED = "Kuvo Code couldn’t confirm that your main computer revoked this desktop’s access.";

async function revokeMemberDesktop(link: ReturnType<typeof readLink>, main: NonNullable<ReturnType<typeof mainElsewhere>>): Promise<DesktopRevocation> {
  const desktop = desktopKey();
  if (!desktop) return { status: "revoked" };
  const unconfirmed = link?.setup?.checkpointed ? "retry" : "unconfirmed";
  if (!takesForwarded(main.key, DEVICES_REMOVE_FEATURE)) return { status: unconfirmed };
  try {
    let key = link && ownKey(link);
    if (!key && IS_MAC) {
      const card: unknown = JSON.parse(fs.readFileSync(SETUP_CARD, "utf8"));
      if (isObject(card) && isObject(card.self) && typeof card.self.key === "string") key = card.self.key;
    }
    if (!key || !KEY.test(key)) return { status: unconfirmed };
    const reply = await forwardAct(
      { ...main, name: main.name || "your main computer", kind: "mac" }, "This computer",
      { area: "devices", method: "DELETE", item: key, body: { phones: [{ key: desktop }], actor: null } },
      undefined, readLink(), Date.now(), { backgroundRelay: IS_MAC },
    );
    if (!("refused" in reply) && reply.status === 200) return { status: "revoked" };
    logCleanup("refused" in reply ? reply.refused : `Device revocation returned ${reply.status}`);
    if (reply.status === 202) return { status: "retry" };
  } catch (error) { logCleanup(error); }
  return { status: unconfirmed };
}

async function revokeDesktop(): Promise<DesktopRevocation> {
  try {
    const link = readLink();
    const main = mainElsewhere(link);
    if (main) return revokeMemberDesktop(link, main);
    const key = desktopKey();
    const needsPublish = IS_MAC && peersOf(link).length > 0;
    if (key) {
      if (needsPublish && await askLivePrimary(key)) return { status: "revoked" };
      const removed = await removeDevicesHere([{ key }], null);
      return { status: removed.status !== 200 || needsPublish ? "unconfirmed" : "revoked" };
    }
    const hasDesktop = (link?.phones ?? []).some(phone => phone.from === "pair" && phone.kind === "desktop");
    return { status: needsPublish && hasDesktop ? "unconfirmed" : "revoked" };
  } catch (error) {
    logCleanup(error);
    return { status: "unconfirmed" };
  }
}

async function askLivePrimary(key: string): Promise<boolean> {
  try {
    const record: unknown = JSON.parse(fs.readFileSync(SERVE_FILE(), "utf8"));
    if (!isObject(record) || typeof record.port !== "number" || !Number.isInteger(record.port) || record.port < 1 || record.port > 65_535 || typeof record.password !== "string" || !record.password) return false;
    const response = await fetch(`http://127.0.0.1:${record.port}/${DESKTOP_UNINSTALL_ROUTE}`, {
      method: "POST", redirect: "error", signal: AbortSignal.timeout(8_000),
      headers: { authorization: `Basic ${Buffer.from(`opencode:${record.password}`).toString("base64")}`, "x-kuvo-from": "app", "x-kuvo-internal": "1", "content-type": "application/json" },
      body: JSON.stringify({ key }),
    });
    await response.body?.cancel();
    return response.status === 200;
  } catch (error) {
    logCleanup(error);
    return false;
  }
}

/** Internal: only the local authenticated server can invoke local device revocation. */
export async function* desktopUninstallRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  if (!IS_MAC || !request.served || !request.fromApp) return yield* json(403, { message: "Run uninstall on this computer." });
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const body = parsed(request.body);
  if (!isObject(body) || typeof body.key !== "string" || !KEY.test(body.key)) return yield* json(400, { message: "The desktop’s key couldn’t be read." });
  try {
    if (await publishDesktopRemoval(macLink.host, body.key)) return yield* json(200, {});
  } catch (error) { logCleanup(error); }
  return yield* json(503, { message: "Kuvo Code couldn’t save the desktop’s removal." });
}

export function clearDesktopKeychain(): boolean {
  // Only the real Mac's own root can name the released app's item. Other roots own their files.
  if (process.platform === "darwin" && REAL_BOX && located(K) === located(path.join(accountHome(), ".kuvo-code"))) {
    const removed = run("/usr/bin/security", ["delete-generic-password", "-s", "run.kuvo.code.desktop", "-a", "link-identity"]);
    if (removed.status !== 0 && removed.status !== 44) {
      logCleanup(removed.error ?? removed.stderr);
      return false;
    }
  }
  return true;
}

/** The app-owned layout kept these three identity items alongside its integration Keychain. */
export function clearAppOwnedLinkKeys(folder: string): boolean {
  if (process.platform !== "darwin") return true;
  const real = located(homeFolder()) === located(accountHome());
  const fixture = process.env.KUVO_TEST_UNINSTALL_TOOLS;
  const security = real ? "/usr/bin/security" : fixture && path.isAbsolute(fixture) ? path.join(fixture, "security") : null;
  if (!security) return true;
  const service = `${path.basename(folder)}.phone-link`;
  for (const account of ["key", "secret", "sign"]) {
    const removed = run(security, ["delete-generic-password", "-s", service, "-a", account]);
    if (removed.status !== 0 && removed.status !== 44) {
      logCleanup(removed.error ?? removed.stderr);
      return false;
    }
  }
  return true;
}

const wipePrepared = (by: string) => path.join(RUN, `wipe-prepared-${by}.json`);

/** `uninstall [--check]`: prints one JSON line, says what it did on stderr, and exits 0 when it's
 * done, 1 when refused or incomplete, 2 for what it doesn't know. */
export async function uninstallCommand(args: string[]): Promise<number> {
  const prepareBy = args[0] === "--prepare-wipe" && args.length === 2 && KEY.test(args[1]) ? args[1] : undefined;
  const unknown = prepareBy ? [] : args.filter(arg => arg !== "--check");
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
  const needsHandover = (link: ReturnType<typeof readLink>) => {
    return link?.setup?.role === "primary" && peersOf(link).some(peer => peer.key !== link.setup!.primary);
  };
  const refuse = (message = MAIN_UNINSTALL, save?: CommittedReceipt) => {
    const reply = { result: "refused", message };
    console.log(JSON.stringify(save ? { ...reply, save } satisfies typeof reply & SaveEnvelope<CommittedReceipt> : reply));
    console.error(message);
    return 1;
  };
  if (needsHandover(readLink())) return refuse();
  const overSSH = process.env.KUVO_UNINSTALL_FROM_SSH === "1";
  const preparedBy = process.env.KUVO_UNINSTALL_PREPARED_BY;
  if (!overSSH && !prepareBy && readLink()?.localWipe) {
    const { wipeCommand } = await import("./setup/wipe");
    const status = await wipeCommand();
    const message = status === 0 ? "Kuvo Code and its data were wiped." : "The wipe is still pending. Check the pre-wipe backup and try again.";
    console.log(JSON.stringify({ result: status === 0 ? "uninstalled" : "unfinished", message }));
    console.error(message);
    return status;
  }
  try {
    const { backupBefore, config } = await import("./backup/engine");
    if (prepareBy) {
      if (!peersOf(readLink()).some(peer => peer.key === prepareBy))
        return refuse("Only a current member can prepare this computer for removal.");
      const point = await backupBefore("wipe");
      writeDurablePrivate(wipePrepared(prepareBy), JSON.stringify({ key: ownKey(readLink() ?? {}), point }), K);
      console.log(JSON.stringify({ result: "prepared" }));
      return 0;
    }
    if (overSSH && preparedBy && KEY.test(preparedBy) && config()?.enabled) {
      const mark = JSON.parse(fs.readFileSync(wipePrepared(preparedBy), "utf8"));
      if (!mark.point || mark.key !== ownKey(readLink() ?? {}))
        return refuse("The verified pre-wipe backup could not be confirmed.");
    } else {
      await backupBefore("wipe");
    }
  } catch (error) { return refuse((error as Error).message); }
  // The SSH controller has already revoked access before asking its remote to stop.
  const publishing = IS_MAC && !mainElsewhere(readLink());
  const revocation: DesktopRevocation = overSSH ? { status: "revoked" } : await revokeDesktop();
  if (revocation.status === "retry") {
    console.log(JSON.stringify({ result: "unfinished", message: REVOCATION_UNCONFIRMED }));
    console.error(REVOCATION_UNCONFIRMED);
    return 1;
  }
  const revoked = revocation.status === "revoked";
  let refusal = "Kuvo Code couldn’t check who leads this setup right now. Try uninstalling again.";
  let done: Uninstalled = { stopped: false, launchAgent: false, crontab: false, startsAgain: [], left: [K], message: "Kuvo Code couldn’t confirm it had stopped on this computer." };
  const cleared = { cleanup: null as ReturnType<typeof run> | null };
  let identityCleared = false;
  try {
    updateLink(link => {
      // Serialize the last authority check with enrollment and handover.
      if (needsHandover(link)) { refusal = MAIN_UNINSTALL; return null; }
      refusal = "";
      try { done = uninstall(); } catch (error) { logCleanup(error); }
      // The SSH wrapper runs the same cleanup locally on the target, including with older boxes.
      cleared.cleanup = overSSH ? null : run("sh", ["-c", `K=$1\nE=$2\n${uninstallCleanup}`, "kuvo-cleanup", K, process.execPath]);
      identityCleared = (overSSH || clearDesktopKeychain()) && cleared.cleanup?.status !== 81;
      return null;
    });
  } catch (error) { logCleanup(error); }
  if (refusal) return refuse(refusal);
  const result = cleared.cleanup;
  const credentialsCleared = result === null || result.status === 0;
  if (result && !credentialsCleared) logCleanup(result.error ?? result.stderr);
  if (!revoked) done.message += publishing
    ? " Kuvo Code couldn’t save the desktop’s removal."
    : ` ${REVOCATION_UNCONFIRMED}`;
  if (!identityCleared) done.message += " Kuvo Code couldn’t delete this desktop’s local identity.";
  if (!credentialsCleared && result?.status !== 81) done.message += " Kuvo Code couldn’t clear all its keys and sign-ins. Run uninstall again.";
  const finished = done.stopped && done.startsAgain.length === 0 && revoked && identityCleared && credentialsCleared;
  const reply = { result: finished ? "uninstalled" : "unfinished", ...done };
  console.log(JSON.stringify(reply));
  console.error(done.message);
  return finished ? 0 : 1;
}
