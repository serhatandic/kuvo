// `kuvo-link.js rehome <root> --run --app <bundle> --release <folder> --engine <folder>` (#713
// §4.2; PROTOCOL.md, "Moving this Mac's box"): an earlier Kuvo Code's sessions and settings moved
// into this Mac's box in a home of its own by the box itself, with no keeper before it. What the
// Swift keeper did around the move is done here: what still runs is stopped, this Mac's keys are
// handed over (`kuvo-keys`, which the app carries), the box's helper is started, and what's owed
// is settled. The installer starts it (`install --move`), in a session of its own.
//
// The earlier Kuvo Code's folder is never changed: everything is cloned and worked on in
// staging, so until the move is done a failure leaves this Mac exactly as it was. Nothing runs
// here until it succeeds, so every refusal says what to do, and trying again is yours: no wait
// between tries, no cap.
// Platform: a Mac's alone. Linux has no app's folder to move from.
// COMPAT(0.7 move): moves the data Kuvo Code 0.5 to 0.7.2, 0.8.0-beta.1 and keeper builds left in the app's folder into this Mac's box, with no keeper before it. Goes on the user's word, once no 0.7 Mac is expected to update.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { sleep } from "../../shared/wait";
import { OWN_RUNTIME, accountHome, located, recordedKind } from "../home";
import { checkedRelease } from "../install/command";
import { appBoxThere, machineBoxThere } from "../install/verdict";
import { launchTools } from "../uninstall";
import { databasesIn, schemaMark } from "./database";
import { takenSchema } from "./migrate";
import { NOTHING_TO_MOVE, diskKind, freeBytes, locked, nearest, planMove, releaseRefusal, resumed, run, runningIn, settled, stamp, stopPoint, takeLastChanged, type Folders, type RehomeOutcome } from "./move";
import { PATH_REWRITER, type Rewriter } from "./rewrite";
import { handOverPasswords } from "./passwords";
import { deleteOriginalKeys, heldRefusal } from "./run-steps";
import { noteRecord, notePhase, readRecord, rehomeFolder, rehomeStatus, type RunPlan } from "./state";

/** The rewriter of a move the box runs itself: the engine's database is judged from a clone of it
 * (`schemaRefusal` below), never opened where the earlier Kuvo Code keeps it. */
const RUN_REWRITER: Rewriter = { ...PATH_REWRITER, refuses: () => null };

const exists = (file: string): boolean => { try { fs.lstatSync(file); return true; } catch { return false; } };
const sizeOf = (file: string) => { try { return fs.statSync(file).size; } catch { return 0; } };

export interface RunOptions {
  /** The app that carries the release (its bundle), and its ID. */
  bundle: string;
  id: string;
  /** The release the installer unpacked, and the folder its engine's version is in. */
  release: string;
  engine: string;
  waiting?: () => void;
}

/** What `--run` says: the move's outcome, and once it's done, whether what starts the box runs
 * now and what's still owed (tried again at the next run). */
export interface RunOutcome extends RehomeOutcome {
  started?: boolean;
}

const NO_ENGINE = "Kuvo's engine couldn't be downloaded. Check that this Mac is online, then try again.";
const DAMAGED = "This copy of Kuvo Code is damaged. Download it again, then try again.";
const TWO = "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet.";
/** A move past the point it goes back from that can't be carried on, or whose record can't be read. */
export const STUCK = "Kuvo was stopped part way through moving your sessions and couldn't finish by itself, so it's changing nothing more. The earlier Kuvo Code's own folder is as it was. Try again; if this stays, send ~/.kuvo-code/updates/rehome/run.log to Kuvo's support.";
const NO_RELEASE = "Kuvo's own files weren't unpacked. Open Kuvo Code again to try again.";

/** Why an engine database in the earlier Kuvo Code's folder can't be brought along, or null: its
 * schema read from a clone of it (its log and index with it), in the move's own folder. */
function schemaRefusal(appFolder: string, home: string): string | null {
  const databases = databasesIn(path.join(appFolder, "opencode", "data", "opencode"));
  if (!databases.length) return null;
  if (!process.versions.bun) return "Your sessions can only be moved by Kuvo's own engine.";
  const scratch = path.join(rehomeFolder(home), "plan-database");
  fs.rmSync(scratch, { recursive: true, force: true });
  fs.mkdirSync(scratch, { recursive: true, mode: 0o700 });
  try {
    for (const database of databases) {
      const copy = path.join(scratch, path.basename(database));
      for (const suffix of ["", "-wal", "-shm"]) if (exists(database + suffix)) fs.copyFileSync(database + suffix, copy + suffix, fs.constants.COPYFILE_FICLONE);
      const mark = schemaMark(copy);
      if (!takenSchema(mark)) return `Your sessions were last used by a version of Kuvo Code that this one doesn't know how to bring along (${mark}). Nothing was changed. Update Kuvo Code if a newer version is out, then try again.`;
    }
    return null;
  } catch (error) {
    return `Your sessions couldn't be read (${(error as Error).message}). Try again.`;
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
}

/** Why the move can't go ahead now, or null: checked before anything changes, each in words that
 * say what to do. */
function runRefusal({ appFolder, home }: Folders, plan: RunPlan, pinned: string | null): string | null {
  const real = located(home) === located(path.join(accountHome(), ".kuvo-code"));
  if (real !== (plan.id === "run.kuvo.code")) return real ? "Only Kuvo Code itself moves into ~/.kuvo-code: a copy moves into a home of its own." : "Kuvo Code itself moves into ~/.kuvo-code only.";
  // (The release whole, every file as it says, with its helper; and its engine, fetched.)
  if (pinned === null || releaseRefusal(plan.release)) return exists(plan.release) ? DAMAGED : NO_RELEASE;
  if (!exists(path.join(plan.engine, pinned, "opencode"))) return NO_ENGINE;
  // (Kuvo Code itself always brings what hands its keys over: one without it isn't whole.)
  if (real && !plan.keys) return DAMAGED;
  if (!exists(appFolder)) return "There's nothing of an earlier Kuvo Code's on this Mac to move.";
  const old = runningIn(path.join(appFolder, "box", "run"));
  if (old.length) return `An earlier Kuvo Code is still running in the background on this Mac (${old.join(", ")}) and didn't stop when asked. Restart this Mac, then try again.`;
  if (runningIn(path.join(home, "run")).length) return "Kuvo is already running from ~/.kuvo-code on this Mac. Restart this Mac, then try again.";
  // One disk, cloned: never copied.
  const target = nearest(home);
  if (fs.statSync(appFolder).dev !== fs.statSync(target).dev || diskKind(appFolder) !== "apfs") return "Kuvo can't move your sessions on this disk: your home folder must be on one APFS disk.";
  // Room for what's rewritten: the engine's database and its log, and some to spare.
  const database = path.join(appFolder, "opencode", "data", "opencode");
  const needed = (sizeOf(path.join(database, "opencode.db")) + sizeOf(path.join(database, "opencode.db-wal"))) * 1.1 + 200_000_000;
  const free = freeBytes(target);
  if (free !== null && free < needed) return `There isn't enough room on this Mac to move your sessions. Free up ${Math.ceil(needed / 1_000_000)} MB, then try again.`;
  return heldRefusal(appFolder) ?? schemaRefusal(appFolder, home);
}

/** How many times a move runs, by itself, when something wrote in what it copied meanwhile. */
const ROUNDS = 3;

/** A process of yours the move looks at: one that runs a program of the earlier Kuvo Code's
 * (`old`: one in its folder, or in its app, a bundle with its ID that isn't the one moving it), or
 * has files open in its folder (`files`; `writes`, any of them for writing), and how to name it to
 * you (`name`: its app's, or its command's). */
export interface Toucher { pid: number; command: string; program: string; old: boolean; files: string[]; writes: boolean; name: string }

/** This process and those it was started by (the installer, the app): never stopped. */
function ancestors(): Set<number> {
  const own = new Set<number>([process.pid]);
  let pid = process.ppid;
  for (let depth = 0; pid > 1 && depth < 32 && !own.has(pid); depth++) {
    own.add(pid);
    const asked = spawnSync("/bin/ps", ["-o", "ppid=", "-p", String(pid)], { encoding: "utf8", timeout: 5_000, stdio: ["ignore", "pipe", "ignore"] });
    pid = Number((asked.stdout ?? "").trim());
    if (!Number.isInteger(pid)) break;
  }
  return own;
}

/** What an app bundle's Info.plist says (`key`), or null. */
function bundleSays(bundle: string, key: string): string | null {
  const asked = spawnSync("/usr/bin/plutil", ["-extract", key, "raw", "-o", "-", path.join(bundle, "Contents", "Info.plist")], { encoding: "utf8", timeout: 5_000, stdio: ["ignore", "pipe", "ignore"] });
  const said = (asked.stdout ?? "").trim();
  return asked.status === 0 && said ? said : null;
}

/** The processes of yours that touch the earlier Kuvo Code's folder (`Toucher`), as the system
 * says (`lsof`, once: by what each runs and has open, never by a name), but this move and what
 * started it. None but on a Mac. */
export function touching(appFolder: string, app: { id: string; bundle: string }): Toucher[] {
  if (process.platform !== "darwin" || typeof process.getuid !== "function") return [];
  const folders = [...new Set([appFolder, located(appFolder)])];
  const under = (file: string) => folders.some(folder => file === folder || file.startsWith(folder + "/"));
  const asked = spawnSync("/usr/sbin/lsof", ["-n", "-w", "-F", "pcfan", "-u", String(process.getuid())], { encoding: "utf8", timeout: 60_000, maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "ignore"] });
  const own = ancestors(), mover = located(app.bundle);
  const bundles = new Map<string, { id: string | null; name: string | null }>();
  const bundleOf = (program: string) => {
    // (The outermost app it's in: a helper inside an app is that app's.)
    const at = program.indexOf(".app/");
    if (at < 0) return null;
    const bundle = program.slice(0, at + 4);
    if (!bundles.has(bundle)) bundles.set(bundle, { id: bundleSays(bundle, "CFBundleIdentifier"), name: bundleSays(bundle, "CFBundleDisplayName") ?? bundleSays(bundle, "CFBundleName") });
    return { bundle, ...bundles.get(bundle)! };
  };
  const found: Toucher[] = [];
  let each: Toucher | null = null, fd = "", access = "";
  for (const line of (asked.stdout ?? "").split("\n")) {
    const value = line.slice(1);
    switch (line[0]) {
      case "p":
        each = own.has(Number(value)) ? null : { pid: Number(value), command: "", program: "", old: false, files: [], writes: false, name: "" };
        if (each) found.push(each);
        break;
      case "c": if (each) each.command = value; break;
      case "f": fd = value; access = ""; break;
      case "a": access = value; break;
      case "n":
        if (!each) break;
        if (fd === "txt" && !each.program) each.program = value;
        if (under(value) && fd !== "txt") {
          each.files.push(value);
          if (/[wu]/.test(access)) each.writes = true;
        }
        break;
    }
  }
  return found.flatMap(each => {
    const bundle = each.program ? bundleOf(each.program) : null;
    each.old = under(each.program) || (bundle !== null && bundle.id === app.id && located(bundle.bundle) !== mover);
    each.name = bundle?.name ?? each.command;
    return each.old || each.files.length ? [each] : [];
  });
}

/** Whether `pid` still runs: one that has ended, waiting only for its parent to hear of it (`Z`), doesn't. */
const alive = (pid: number): boolean => {
  try { process.kill(pid, 0); } catch (error) { if ((error as NodeJS.ErrnoException).code !== "EPERM") return false; }
  const asked = spawnSync("/bin/ps", ["-o", "stat=", "-p", String(pid)], { encoding: "utf8", timeout: 5_000, stdio: ["ignore", "pipe", "ignore"] });
  return asked.status === 0 && !(asked.stdout ?? "").trim().startsWith("Z");
};

/** Everything of the earlier Kuvo Code's still running stopped, each by its number: its keeper's
 * launch agent let go of first (so nothing starts it again), then each asked (`SIGTERM`) and made
 * to after a while; what it stopped goes to the move's log. Then another app writing in its
 * folder is named. Why the move can't go on, in words; else null. */
async function stopOldSetup(appFolder: string, app: { id: string; bundle: string }, { asked = 15_000, made = 5_000 } = {}): Promise<string | null> {
  let found = touching(appFolder, app);
  if (found.some(each => each.old)) {
    const tools = launchTools();
    if (tools) spawnSync(tools.launchctl, ["bootout", `gui/${process.getuid?.() ?? 0}/${app.id}.box`], { timeout: 30_000, stdio: "ignore" });
    const old = found.filter(each => each.old);
    console.error(`Stopping what still runs of the earlier Kuvo Code: ${old.map(each => `${each.pid} ${each.program || each.command}`).join(", ")}`);
    const waitFor = async (limit: number) => {
      for (const began = Date.now(); Date.now() - began < limit && old.some(each => alive(each.pid));) await sleep(100);
    };
    for (const each of old) try { process.kill(each.pid, "SIGTERM"); } catch {}
    await waitFor(asked);
    for (const each of old) if (alive(each.pid)) try { process.kill(each.pid, "SIGKILL"); } catch {}
    await waitFor(made);
    found = touching(appFolder, app);
    const left = found.filter(each => each.old);
    if (left.length) {
      console.error(`Still running of the earlier Kuvo Code: ${left.map(each => `${each.pid} ${each.program || each.command}`).join(", ")}`);
      return "An earlier Kuvo Code is still running in the background on this Mac and didn't stop. Restart this Mac, then try again.";
    }
  }
  const writers = found.filter(each => each.writes);
  if (!writers.length) return null;
  console.error(`Writing in the earlier Kuvo Code's folder: ${writers.map(each => `${each.pid} ${each.program || each.command}: ${each.files.slice(0, 3).join(", ")}`).join("; ")}`);
  return stillUsing(writers);
}

/** Another app using the earlier Kuvo Code's folder, said by its name, for you to quit. */
function stillUsing(writers: Toucher[]): string {
  const names = [...new Set(writers.map(each => each.name).filter(Boolean))];
  const named = names.length === 1 ? names[0] : names.length === 2 ? `${names[0]} and ${names[1]}` : names.length ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}` : "Another app";
  return `${named} ${names.length > 1 ? "are" : "is"} still using your earlier Kuvo Code's files. Quit ${names.length > 1 ? "them" : "it"}, then try again.`;
}

/** The old keeper (a keeper build's, `app-box`) asked to stop everything and itself (`POST stop`
 * on its control, `run/keeper.json`), then waited for; one that doesn't answer is stopped by its
 * pid, where that's still Kuvo Code's own keeper. What it ran (the engine, the bridge, `serve`)
 * goes with it. */
async function stopOldKeeper(appFolder: string): Promise<void> {
  const run = path.join(appFolder, "box", "run");
  let keeper: unknown = null;
  try { keeper = JSON.parse(fs.readFileSync(path.join(run, "keeper.json"), "utf8")); } catch {}
  const pid = isObject(keeper) && Number.isInteger(keeper.pid) && (keeper.pid as number) > 1 ? keeper.pid as number : null;
  // (Its own keeper by its command line, as `ps` says it now: a record left by one that's gone,
  // whose number something else has since, names nothing to stop. Never by its number alone.)
  const runs = () => {
    if (pid === null) return false;
    const asked = spawnSync("/bin/ps", ["-ww", "-o", "command=", "-p", String(pid)], { encoding: "utf8", timeout: 5_000, stdio: ["ignore", "pipe", "ignore"] });
    return asked.status === 0 && /(^|\s)--box(\s|$)/.test(asked.stdout ?? "");
  };
  if (!runs()) return;
  if (isObject(keeper) && typeof keeper.url === "string" && typeof keeper.password === "string" && /^http:\/\/127\.0\.0\.1:\d{1,5}\/?$/.test(keeper.url)) {
    try { await fetch(`${keeper.url.replace(/\/+$/, "")}/stop`, { method: "POST", headers: { authorization: `Bearer ${keeper.password}` }, signal: AbortSignal.timeout(20_000) }); } catch {}
  }
  const gone = async (limit: number) => {
    for (const began = Date.now(); Date.now() - began < limit;) {
      if (!runs() && !runningIn(run).length) return true;
      await sleep(250);
    }
    return !runs() && !runningIn(run).length;
  };
  if (await gone(30_000)) return;
  if (runs()) try { process.kill(pid!, "SIGTERM"); } catch {}
  await gone(15_000);
}

/** This Mac's box started through its helper by the link just put in place, on its own engine
 * (`helper start`, the whole start): whether it runs now, and what it said. Only for the real
 * home, or a test's with stand-ins; else it's said that it starts by itself only there. */
function startHelper(home: string, app: string): { started: boolean; message?: string } {
  if (!launchTools()) return { started: false, message: "It's moved, and starts by itself only in ~/.kuvo-code." };
  const engines = (() => { try { return fs.readdirSync(path.join(home, "engine")).filter(name => exists(path.join(home, "engine", name, "opencode"))).sort(); } catch { return []; } })();
  const version = engines.pop();
  if (!version) return { started: false, message: "Kuvo's engine isn't in place, so it couldn't be started. Try again." };
  const ran = spawnSync(path.join(home, "engine", version, "opencode"), [...OWN_RUNTIME, path.join(home, "bin", "kuvo-link.js"), "helper", "start", "--app", app], {
    cwd: home, encoding: "utf8", timeout: 180_000, env: { ...process.env, KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" },
  });
  let said: unknown = null;
  try { said = JSON.parse((ran.stdout ?? "").trim().split("\n").pop() ?? ""); } catch {}
  const started = ran.status === 0 && isObject(said) && said.started === true;
  // What it said is the helper's own (it names what it starts, and what launchd answered): kept
  // for support in the move's log, never shown. Started, there's nothing to say, but where it only
  // runs until you log out; not started, one sentence that says what to do.
  const off = isObject(said) && said.disabled === true;
  if (!started || off) console.error(`helper start: ${isObject(said) && typeof said.message === "string" ? said.message : (ran.stderr ?? "").trim().split("\n").pop() || "it said nothing"}`);
  const message = off
    ? started ? "Kuvo Helper is turned off in System Settings › General › Login Items, so Kuvo runs only until you log out. Turn it on there to have it start when you log in."
      : "Kuvo Helper is turned off in System Settings › General › Login Items, so Kuvo couldn't be started. Turn it on there, then try again."
    : started ? undefined : "Your sessions are moved, but Kuvo couldn't be started on this Mac. Try again.";
  return { started, ...(message ? { message } : {}) };
}

/** What a move that's done still owes, each settled once it's done and tried again at the next
 * run where it couldn't be: first the box's start (its helper's launch agent) and the Keychain's
 * originals taken away; then (`settle`) an earlier Kuvo Code's saved sign-ins, and the old keeper's
 * launch agent let go of. What the installer staged goes once the box runs from its own files. */
function settleStart(home: string, options: Pick<RunOptions, "bundle" | "id">) {
  const owes = (what: string) => readRecord(home).owed?.includes(what as never) === true;
  const record = readRecord(home);
  const bundle = exists(options.bundle) ? options.bundle : record.app?.bundle ?? options.bundle;
  const app = record.app?.id ?? options.id;
  let started: boolean | undefined;
  let message: string | undefined;
  if (owes("start")) {
    const start = startHelper(home, app);
    started = start.started;
    message = start.message;
    // (Where nothing outside the folder is written, a copy's home, there's no start to owe.)
    if (start.started || !launchTools()) settled(home, "start");
    stopPoint("after-start");
  }
  if (owes("keychain") && !owes("start")) {
    const keys = path.join(bundle, "Contents", "MacOS", "kuvo-keys");
    const deleted = exists(keys) ? deleteOriginalKeys(keys, home) : { done: false, why: "kuvo-keys isn't in this copy of Kuvo Code" };
    if (deleted.done) settled(home, "keychain");
    else {
      console.error(`kuvo-keys delete-originals: ${deleted.why}`);
      message ??= "Kuvo is moved and working. It couldn't finish with your Keychain yet, and tries again the next time it opens.";
    }
  }
  return { owes, bundle, app, started, message };
}

/** And an earlier Kuvo Code's saved sign-ins, into the vault, once the box's server answers
 * (`passwords.ts`): tried again at the next run where it couldn't be yet. */
async function settle(home: string, options: Pick<RunOptions, "bundle" | "id">): Promise<RunOutcome> {
  notePhase(home, "settle");
  const { owes, bundle, app, started, message } = settleStart(home, options);
  let said = message;
  if (owes("passwords") && !owes("start")) {
    const keys = path.join(bundle, "Contents", "MacOS", "kuvo-keys");
    const record = readRecord(home);
    const prefs = typeof record.from === "string" ? (await import("./upgrades")).appPreferences(record.from) : {};
    const sites = Array.isArray(prefs["passwords.agentSignIn"]) ? prefs["passwords.agentSignIn"].filter((each): each is string => typeof each === "string") : [];
    const accounts = isObject(prefs["passwords.agentSignInAccounts"]) ? Object.fromEntries(Object.entries(prefs["passwords.agentSignInAccounts"]).filter((each): each is [string, string] => typeof each[1] === "string")) : {};
    // (Where nothing starts the box, a copy's home, its server never answers: not waited for.)
    const handed = exists(keys) ? await handOverPasswords({ home, keys, agentSites: sites, agentAccounts: accounts, ...(launchTools() ? {} : { wait: 0 }) }) : { done: false, why: "kuvo-keys isn't in this copy of Kuvo Code", added: 0, short: 0, kept: 0 };
    if (handed.done) {
      if (handed.short) noteRecord(home, { dropped: [...(readRecord(home).dropped ?? []), `${handed.short === 1 ? "A saved password" : `${handed.short} saved passwords`} shorter than four characters ${handed.short === 1 ? "wasn't" : "weren't"} brought into the vault, which keeps none that short.`] });
      if (handed.kept) noteRecord(home, { dropped: [...(readRecord(home).dropped ?? []), `${handed.kept === 1 ? "A saved password stays" : `${handed.kept} saved passwords stay`} in your Keychain: Kuvo couldn't be sure the vault would keep ${handed.kept === 1 ? "it" : "them"} exactly. Add any you use to the vault yourself.`] });
      settled(home, "passwords");
    } else {
      // (Why goes to the move's own log, for support: it may name a site or an account.)
      console.error(`Your saved sign-ins couldn't be brought into the vault yet: ${handed.why}`);
      said ??= "Kuvo is moved and working. Your saved sign-ins aren't in its vault yet: it tries again the next time it opens.";
    }
  }
  if (owes("agent")) {
    // The old keeper's launch agent, let go of: its program is gone with the app it was in.
    const tools = launchTools();
    if (tools) spawnSync(tools.launchctl, ["bootout", `gui/${process.getuid?.() ?? 0}/${app}.box`], { timeout: 30_000, stdio: "ignore" });
    settled(home, "agent");
  }
  // What the installer staged, once the box runs from its own (the helper the copies trust is the
  // one in its `bin` now).
  if (!owes("start")) fs.rmSync(path.join(home, "updates", "install"), { recursive: true, force: true });
  notePhase(home, null);
  return { result: "done", status: rehomeStatus(home), ...(started !== undefined ? { started } : {}), ...(said ? { message: said } : {}) };
}

/** `rehome --run`: the earlier Kuvo Code's sessions and settings in `appFolder` moved into this
 * Mac's box in `home`. */
export async function rehomeRun(folders: Folders, options: RunOptions): Promise<RunOutcome> {
  const { appFolder, home } = folders;
  if (process.platform !== "darwin") return { result: "refused", message: NOTHING_TO_MOVE, status: rehomeStatus(home) };
  return locked(home, options.waiting ?? (() => {}), async () => {
    try {
      return await whole();
    } finally {
      notePhase(home, null);
    }
  });

  async function whole(): Promise<RunOutcome> {
    notePhase(home, "plan");
    // An earlier try first: one that stopped before it could only go forward goes back, and this
    // one starts afresh; one past that is carried on.
    const earlier = await resumed(home, RUN_REWRITER);
    // (One that can't be carried on is said in words, never as the move's own record reads.)
    if (earlier && earlier.result === "stuck") return { ...earlier, message: STUCK };
    if (earlier && earlier.result !== "failed") return earlier.result === "done" ? settle(home, options) : earlier;
    const record = readRecord(home);
    if (record.state === "done" && record.way === "move") return settle(home, options);
    // A machine's box here already, with an earlier Kuvo Code's data beside it (`two`): putting
    // the two together isn't built. Said before anything is stopped, and nothing is noted in
    // that box's home: both stay exactly as they are.
    if (machineBoxThere(home)) return { result: "refused", message: TWO, status: rehomeStatus(home) };
    // Nor is anything moved over this Mac's own Kuvo, made here already (a new Mac's, `--new`):
    // whoever asks, not only an installer that judged first.
    if (recordedKind(path.join(home, "box", "kind")) === "mac") return { result: "refused", message: "Kuvo is set up on this Mac already, so nothing was moved over it. An earlier Kuvo Code's sessions are still in its own folder.", status: rehomeStatus(home) };
    const keys = path.join(options.bundle, "Contents", "MacOS", "kuvo-keys");
    const plan: RunPlan = {
      bundle: options.bundle, id: options.id, release: path.resolve(options.release), engine: path.resolve(options.engine),
      source: appBoxThere(appFolder) ? "app-box" : "app-07",
      keys: exists(keys) ? keys : null,
    };
    const refuse = (refusal: string): RunOutcome => {
      // (Counted, never waited for: trying again is yours.)
      noteRecord(home, { state: "failed", way: "move", tries: (readRecord(home).tries ?? 0) + 1, nextTry: null, failed: { at: Date.now(), step: "plan", message: refusal }, asked: null });
      return { result: "refused", message: refusal, status: rehomeStatus(home) };
    };
    if (plan.source === "app-box") await stopOldKeeper(appFolder);
    // Everything else of the earlier Kuvo Code's still running stopped too, whatever started it
    // (and whether or not it left a record): it would go on writing in what's copied.
    const app = { id: options.id, bundle: options.bundle };
    const stopped = await stopOldSetup(appFolder, app);
    if (stopped) return refuse(stopped);
    let pinned: string | null = null;
    try { pinned = checkedRelease(plan.release).engine; } catch {}
    const refusal = runRefusal(folders, plan, pinned);
    if (refusal) return refuse(refusal);
    // (Only this release's engine goes into the box: never a download cut short, nor another version's.)
    // (And only where the installer stages it: never a folder named by mistake.)
    if (located(plan.engine) === located(path.join(home, "updates", "install", "engine"))) {
      for (const name of fs.readdirSync(plan.engine)) if (name !== pinned) fs.rmSync(path.join(plan.engine, name), { recursive: true, force: true });
    }
    // Something written there while it was copied: the move went back, whatever wrote it is
    // stopped where it's the earlier Kuvo Code's, and the move runs again, by itself, a few times.
    // One of another app's that goes on writing is named, in words, for you to quit.
    for (let round = 1; ; round++) {
      const now = Date.now();
      const outcome = await run({
        v: 1, way: "move", began: now, step: "plan", from: appFolder, to: home,
        ...planMove(folders, plan.release, { engine: plan.engine, whole: true }), kept: path.join(rehomeFolder(home), "kept", stamp(now)),
        run: plan,
      }, RUN_REWRITER);
      if (outcome.result === "stuck") return { ...outcome, message: STUCK };
      if (outcome.result === "done") return settle(home, options);
      const changed = takeLastChanged();
      if (!changed || round >= ROUNDS) return outcome;
      notePhase(home, "plan");
      const writers = touching(appFolder, app).filter(each => !each.old && each.files.some(file => file === changed || file.startsWith(changed + "/")));
      if (writers.length) {
        console.error(`Using what changed (${changed}): ${writers.map(each => `${each.pid} ${each.program || each.command}`).join(", ")}`);
        return refuse(stillUsing(writers));
      }
      const again = await stopOldSetup(appFolder, app);
      if (again) return refuse(again);
      console.error(`Running the move again (${round + 1} of ${ROUNDS}): ${changed} changed while it was copied.`);
    }
  }
}
