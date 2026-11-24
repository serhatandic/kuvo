// `kuvo-link.js install --status | --new | --move | --join` (PROTOCOL.md, "Installing this computer's box";
// #713 §4.1): what `kuvo-install` hands over to, from the release an app carries, on the engine it
// took: this process's `KUVO_HOME` is the box's home, and its script is the release's, unpacked in
// `<home>/updates/install/release`.
//
//   install --status --root <name> [--app <bundle>]                       what this computer holds
//   install --new --root <name> --release <folder> --engine <folder> [--app <bundle>]   a new box, made and started
//   install --move --root <name> --release <folder> --engine <folder> --app <bundle>    an earlier Kuvo Code's sessions and settings moved
//
// One JSON line on stdout. Exit: 0 done; 1 refused or failed, in words; 2 not asked as above; 4
// another move holds the lock.
// Platform: a Mac's new box is its own (`box/kind` `mac`, started by Kuvo Helper, `rehome --new`);
// on Linux it's a machine's, in `~/.kuvo-code`, started as a computer that's added is (the one
// start script, `add/scripts.ts`), with its `@reboot` line and no helper.

import { boxFilesFormat, installedFormat, readInstalled } from "../../shared/release-formats";
import { UnsupportedFormatVersion } from "../../shared/format-version";
import { hasInstallAttempt, installAttemptFormat } from "./attempt";
import { joinWithInstallerCode, requireJoinCode, type InstallerMembership } from "../add/invite";
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { RELEASE_FILES } from "../add/files";
import * as scripts from "../add/scripts";
import { syncDurableFile, writePrivate } from "../files";
import { PAUSED } from "../at-work";
import { helperStatus } from "../helper";
import { accountHome, homeFolder, K, located, OWN_RUNTIME } from "../home";
import { computerName } from "../mac/link";
import { homeOfRoot, newMarker, rehomeNew, runningIn, settled } from "../rehome/move";
import { checkRehomeFormats, giveLock, progressOf, rehomeFolder, rehomeStatus, stepIndex, takeLock } from "../rehome/state";
import { Said } from "../rehome/said";
import { sleep } from "../../shared/wait";
import { keeperEngine } from "../updates/engine-swap";
import { launchTools } from "../uninstall";
import { finishReinstall, localWipeAt, resumeWipeDirectory } from "../setup/install-offer";
import { initializeMachineSetup } from "../setup/lead";
import { resumeAppWipe, wipeCommand } from "../setup/wipe";
import { installOffersLayoutReady } from "../../shared/folder-version";
import { judgeInstall, MAKING, type InstallVerdict, type Judgement } from "./verdict";

const USAGE = "Usage: kuvo-link.js install --status | --new | --move | --join, with --root <name> [--app <bundle>] [--release <folder> --engine <folder>]";
/** The app whose folder is the real one, and the only one that installs into the real `~/.kuvo-code`. */
const REAL_APP = "run.kuvo.code";

interface Answer {
  result: "done" | "nothing" | "refused" | "failed" | "waits";
  verdict: InstallVerdict;
  message?: string;
  /** Whether what was made runs now (its helper, or a machine's keeper). */
  started?: boolean;
  /** Where what was in the home before was set aside, whole, when anything was. */
  setAside?: string;
}
const EXIT: Record<Answer["result"], number> = { done: 0, nothing: 0, refused: 1, failed: 1, waits: 4 };

/** Why each verdict gets no new box, in words an app shows as they are. */
const NOT_NEW: Record<Exclude<InstallVerdict, "fresh" | "ours">, string> = {
  "app-07": "Kuvo Code has your sessions and settings on this Mac already: they move into their new place instead.",
  "app-box": "Kuvo Code has your sessions and settings on this Mac already: they move into their new place instead.",
  moving: "Kuvo is moving your sessions to their new place on this Mac: that's finished first.",
  stuck: "Kuvo was moving your sessions to their new place on this Mac and stopped part way: that's finished first.",
  waits: "Another move is under way.",
  back: "Kuvo Code's sessions on this Mac were moved back: they move again when you ask.",
  machine: "This computer is set up as one of your machines already: Kuvo uses what's here.",
  two: "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: both stay as they are.",
};

/** An app bundle's ID (`CFBundleIdentifier`), read from its Info.plist; null where it has none. */
export function bundleID(bundle: string): string | null {
  const plist = path.join(bundle, "Contents", "Info.plist");
  if (process.platform === "darwin") {
    const asked = spawnSync("/usr/bin/plutil", ["-extract", "CFBundleIdentifier", "raw", "-o", "-", plist], { encoding: "utf8", timeout: 30_000 });
    const said = asked.status === 0 ? asked.stdout.trim() : "";
    if (/^[A-Za-z0-9][A-Za-z0-9.-]*$/.test(said)) return said;
  }
  try {
    return /<key>CFBundleIdentifier<\/key>\s*<string>([A-Za-z0-9][A-Za-z0-9.-]*)<\/string>/.exec(fs.readFileSync(plist, "utf8"))?.[1] ?? null;
  } catch {
    return null;
  }
}

const sha256 = (file: string): string => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");

/** The release the installer unpacked, checked against its own `box.json`: its version, its
 * engine's, and every file it names there as it says. Throws in words. */
export function checkedRelease(release: string): { version: string; engine: string; files: Record<string, string> } {
  const damaged = new Said("This copy of Kuvo is damaged. Download it again, then try again.");
  let box: unknown;
  try { box = boxFilesFormat.read(JSON.parse(fs.readFileSync(path.join(release, "box.json"), "utf8"))); }
  catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw new Said(error.message);
    throw damaged;
  }
  if (!isObject(box) || typeof box.version !== "string" || typeof box.engine !== "string" || !/^[0-9A-Za-z][0-9A-Za-z.-]*$/.test(box.engine) || !isObject(box.files)) throw damaged;
  for (const [name, sum] of Object.entries(box.files)) {
    const file = path.join(release, name);
    if (name.split("/").includes("..") || path.isAbsolute(name) || fs.lstatSync(file, { throwIfNoEntry: false })?.isFile() !== true || sha256(file) !== sum) throw damaged;
  }
  for (const [name] of RELEASE_FILES) if (!Object.hasOwn(box.files, name)) throw damaged;
  return { version: box.version, engine: box.engine, files: box.files as Record<string, string> };
}

export async function installCommand(args: string[]): Promise<number> {
  // (Whether something runs is told by its process, never a stale record: as `rehome`.)
  delete process.env.KUVO_BOX;
  const option = (name: string): string | undefined => (args.indexOf(name) >= 0 ? args[args.indexOf(name) + 1] : undefined);
  const say = (value: unknown) => console.log(JSON.stringify(value));
  const verbs = ["--status", "--new", "--move", "--join"].filter(verb => args.includes(verb));
  let home: string;
  try {
    if (verbs.length !== 1) throw new Error("Say one of --status, --new, --move and --join.");
    home = homeOfRoot(option("--root"));
    if (located(home) !== located(K)) throw new Error("kuvo-link.js install runs with KUVO_HOME the box's home.");
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error), USAGE);
    return 2;
  }
  const mac = process.platform === "darwin";
  const real = located(home) === located(path.join(accountHome(), ".kuvo-code"));
  // The old app's folder, by the bundle that runs this: a Mac's alone.
  const bundle = option("--app");
  const app = mac && bundle ? bundleID(bundle) : null;
  const appFolder = app ? path.join(homeFolder(), "Library", "Application Support", app) : null;
  const judged = (): Judgement => judgeInstall(home, appFolder, { helperInPlace: () => (mac && launchTools() ? helperRuns() : null) });
  try {
    if (mac && !app) {
      say({ result: "refused", message: "Kuvo couldn't tell which app started it. Open Kuvo again." });
      return EXIT.refused;
    }
    // (A copy of the app, a demo's or a test's, never judges or makes the real one's box, nor the
    // real app a copy's: each has its own folder and its own home. Nothing's said of what's there.)
    if (mac && real !== (app === REAL_APP)) {
      say({ result: "refused", message: real ? "Only Kuvo itself sets up ~/.kuvo-code: a copy of it keeps a folder of its own." : "Kuvo itself sets up ~/.kuvo-code only." });
      return EXIT.refused;
    }
    // A marker can authorize restarting a making. Hold before checking status,
    // which can also resume a previously authorized wipe.
    try {
      checkRehomeFormats(home);
      readInstalled(path.join(home, "updates"));
      hasInstallAttempt(path.join(home, MAKING));
      hasInstallAttempt(newMarker(home));
    } catch (error) {
      say({ result: "refused", message: error instanceof Error ? error.message : String(error) });
      return EXIT.refused;
    }
    if (verbs[0] === "--status") {
      // A crash can leave an authorized last wipe to finish. If it is still running, judge waits.
      await resumeWipeDirectory(home).catch(() => {});
      if (appFolder) {
        await resumeWipeDirectory(appFolder).catch(() => {});
        await resumeAppWipe(appFolder).catch(() => {});
      }
      if (localWipeAt(home) === "last" && installOffersLayoutReady(home)) await wipeCommand();
      const judgement = judged();
      tidyStaging(home, judgement, option("--release"));
      // (What's here with nothing to run that the person at the Mac should still be told: an
      // earlier Kuvo Code's sessions left where they are, unseen, beside a machine's setup.)
      say({ ...judgement, ...(judgement.verdict === "two" ? { message: NOT_MOVED.two } : {}) });
      return 0;
    }
    const release = option("--release"), engine = option("--engine");
    if (!release || !engine || !path.isAbsolute(release) || !path.isAbsolute(engine)) {
      console.error(USAGE);
      return 2;
    }
    if (verbs[0] === "--move") {
      const answer = mac ? await moved({ home, root: option("--root")!, bundle: bundle!, release, engine }, judged, say) : { result: "refused" as const, verdict: judged().verdict, message: "There's nothing to move on this computer: Kuvo is where it belongs already." };
      say({ ...answer, status: judged().status });
      return EXIT[answer.result];
    }
    const joining = verbs[0] === "--join";
    const code = joining ? await installerCode() : null;
    if (code !== null) verifiedJoinCode(code);
    const existing = judged();
    const answer = joining && (existing.verdict === "machine" || existing.verdict === "ours" && helperRuns())
      ? { result: "nothing" as const, verdict: existing.verdict }
      : mac ? await newMac({ home, appFolder: appFolder!, app: app!, release, engine }, judged) : await newMachine({ home, release, engine }, judged);
    if (!mac && !joining && answer.started) initializeMachineSetup();
    const membership = code !== null && (answer.result === "done" || answer.result === "nothing")
      ? joinInstalledBox(home, code) : undefined;
    if (answer.started || membership) {
      finishReinstall(home);
      if (appFolder) finishReinstall(appFolder);
    }
    say({ ...answer, status: judged().status, ...(membership ? { membership } : {}) });
    return EXIT[answer.result];
  } catch (error) {
    // (Only what's said for the person at the computer is shown: anything else in one sentence,
    // with what went wrong on standard error, for support.)
    if (!(error instanceof Said)) console.error(error instanceof Error ? error.stack ?? error.message : String(error));
    say({ result: "failed", message: error instanceof Said ? error.message : "Kuvo couldn't finish setting up on this computer this time. Try again.", ...judged() });
    return EXIT.failed;
  }
}

export async function installerCode(prompt = "Enter the installer code shown on your device"): Promise<string> {
  if (process.stdin.isTTY) process.stderr.write(prompt + ": ");
  let text = "";
  for await (const chunk of process.stdin.iterator({ destroyOnReturn: false })) {
    text += chunk.toString();
    if (text.length > 4200) throw new Said("That installer code is too long.");
    if (text.includes("\n")) break;
  }
  process.stdin.pause();
  if (!text.trim()) throw new Said(prompt + ".");
  return text.split("\n")[0]!;
}

function joinInstalledBox(home: string, code: string): InstallerMembership {
  // A fresh Mac has just written box/kind. Its installed process must read the new kind before
  // obtaining its identity from its helper. The installer’s staging engine can already be gone.
  const version = keeperEngine();
  if (!version) throw new Said("The installed computer's engine is unavailable. Restart Kuvo and try again.");
  const runtime = path.join(home, "engine", version, "opencode");
  const joined = spawnSync(runtime, ["--config=/dev/null", "--no-env-file", path.join(home, "bin", "kuvo-link.js"), "join"], {
    cwd: home, input: `${code}\n`, encoding: "utf8", timeout: 180_000, env: { ...process.env, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" },
  });
  if (joined.status !== 0) throw new Said((joined.stderr ?? "").trim().split("\n").at(-1) || "Kuvo couldn’t finish joining your setup. Retry with the same installer code.");
  try {
    const value: unknown = JSON.parse((joined.stdout ?? "").trim().split("\n").at(-1) ?? "");
    if (isObject(value) && typeof value.setup === "string" && typeof value.primary === "string") {
      const missing = isObject(value.credentials) && typeof value.credentials.missing === "number" ? value.credentials.missing : 0;
      return { setup: value.setup, primary: value.primary, ...(missing > 0 ? { credentials: { missing } } : {}) };
    }
  } catch {}
  throw new Said("Kuvo couldn’t read the membership from this computer. Retry with the same installer code.");
}

function verifiedJoinCode(input: string) {
  try { return requireJoinCode(input); }
  catch (error) { throw new Said(error instanceof Error ? error.message : String(error)); }
}

export async function enrollInstalled(code: string, prepare: () => void) {
  const parsed = verifiedJoinCode(code);
  try {
    prepare();
    return await joinWithInstallerCode(parsed);
  }
  catch (error) { throw new Said(error instanceof Error ? error.message : "The installer could not join your setup."); }
}

/** Whether what starts a Mac's own box is in place and launchd has it: its launch agent's file
 * naming this box's helper, loaded (where that couldn't be asked, the file alone). One written
 * that launchd never took, or took and let go of, isn't in place: `new` starts it. Nor is one
 * stopped until it next starts (Stop Kuvo on this Mac, `run/paused`): the app opening starts it. */
export function helperRuns(): boolean {
  const status = helperStatus();
  return status.agent && status.loaded !== false && status.running !== false && !fs.existsSync(PAUSED());
}

const NO_ENGINE = (here: string) => `The engine couldn't be downloaded. Check that ${here} is online, then try again.`;

/** What the installer staged, gone once a box is made from it (the engine this runs on with it:
 * a running program keeps its file). */
const clearStaging = (home: string): void => fs.rmSync(path.join(home, "updates", "install"), { recursive: true, force: true });

/** Publish completion before deleting anything left in the Linux staging tree. */
function retireMachineStaging(home: string): void {
  const updates = path.join(home, "updates");
  const retired = path.join(updates, `retired-install-${crypto.randomUUID()}`);
  fs.renameSync(path.join(updates, "install"), retired);
  syncDurableFile(updates, updates);
  try {
    fs.rmSync(retired, { recursive: true, force: true });
    syncDurableFile(updates, updates);
  } catch (error) {
    console.error("Removing retired installer staging:", error);
  }
}

/** The engine the installer fetched, gone once the box that's here has that engine itself (the
 * installer runs on the box's own from then on): never two copies kept. Only where nothing is to
 * be made or moved from what's staged. */
function tidyStaging(home: string, { verdict, verb }: Judgement, release: string | undefined): void {
  if (verb !== null || !["ours", "machine", "two"].includes(verdict) || !release) return;
  // (A making's mark left beside a box that did start: it's made.)
  if (verdict !== "ours") fs.rmSync(path.join(home, MAKING), { force: true });
  try {
    const box = boxFilesFormat.read(JSON.parse(fs.readFileSync(path.join(release, "box.json"), "utf8")));
    const version = isObject(box) && typeof box.engine === "string" && /^[0-9A-Za-z][0-9A-Za-z.-]*$/.test(box.engine) ? box.engine : null;
    if (version && fs.existsSync(path.join(home, "engine", version, "opencode"))) fs.rmSync(path.join(home, "updates", "install", "engine"), { recursive: true, force: true });
  } catch {}
}

/** Why each verdict has nothing to move, in words an app shows as they are. */
const NOT_MOVED: Partial<Record<InstallVerdict, string>> = {
  fresh: "There's nothing of an earlier Kuvo Code's on this Mac to move.",
  machine: "This Mac is set up as one of your machines already: Kuvo uses what's here.",
  two: "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet.",
  back: "Kuvo Code's sessions on this Mac were moved back: they move again when you ask.",
  waits: "Another move is under way.",
};

const processAlive = (pid: number): boolean => {
  if (!Number.isInteger(pid) || pid <= 1) return false;
  try { process.kill(pid, 0); return true; } catch (error) { return (error as NodeJS.ErrnoException).code === "EPERM"; }
};

/** An earlier Kuvo Code's sessions and settings moved by the box itself (`rehome --run`, "Moving
 * this Mac's box"), started in a session of its own so that the app quitting doesn't stop it,
 * and followed: a line for each step as it's reached (`{step, title, index, of}`, the box's own
 * words), then its answer. Only a move that isn't done: one that's done is never run again, for
 * what it still owes or anything else (`judgeInstall`). */
async function moved({ home, root, bundle, release, engine }: { home: string; root: string; bundle: string; release: string; engine: string }, judged: () => Judgement, say: (value: unknown) => void): Promise<Answer> {
  const before = judged();
  if (before.verdict === "waits") return { result: "waits", verdict: "waits", message: NOT_MOVED.waits };
  if (before.verb !== "move") return { result: before.verdict === "ours" ? "nothing" : "refused", verdict: before.verdict, ...(NOT_MOVED[before.verdict] ? { message: NOT_MOVED[before.verdict] } : {}) };
  checkedRelease(release);
  const folder = rehomeFolder(home);
  fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
  // (Its answer in a file of this installer's own: two at once never read each other's.)
  const out = path.join(folder, `run.${process.pid}.out`), log = path.join(folder, "run.log");
  for (const left of fs.readdirSync(folder)) if (/^run\.(\d+\.)?out$/.test(left) && left !== path.basename(out) && !processAlive(Number(left.split(".")[1]))) fs.rmSync(path.join(folder, left), { force: true });
  const outTo = fs.openSync(out, "w", 0o600), logTo = fs.openSync(log, "a", 0o600);
  let child: ReturnType<typeof spawn>;
  try {
    child = spawn(process.execPath, [...OWN_RUNTIME, process.argv[1], "rehome", root, "--run", "--app", bundle, "--release", release, "--engine", engine], {
      cwd: home, detached: true, stdio: ["ignore", outTo, logTo], env: process.env,
    });
  } finally {
    fs.closeSync(outTo);
    fs.closeSync(logTo);
  }
  const ended = new Promise<number | null>(resolve => { child.once("exit", status => resolve(status)); child.once("error", () => resolve(null)); });
  // Polling can see "plan" again during a retry or between the journal finishing and settling.
  // Keep the furthest step shown. "clear" and "done" share a display index but have different words.
  let shown = stepIndex("plan");
  const show = () => {
    const progress = rehomeStatus(home).progress;
    if (!progress) return;
    const step = stepIndex(progress.step);
    if (step > shown) {
      shown = step;
      say(progress);
    }
  };
  say(progressOf("plan"));
  let status: number | null | undefined;
  while (status === undefined) {
    status = await Promise.race([ended, sleep(250).then(() => undefined)]);
    show();
  }
  child.unref();
  let said: unknown = null;
  try { said = JSON.parse(fs.readFileSync(out, "utf8").trim().split("\n").pop() ?? ""); } catch {}
  fs.rmSync(out, { force: true });
  const verdict = judged().verdict;
  if (!isObject(said) || typeof said.result !== "string") return { result: "failed", verdict, message: "Kuvo stopped while moving your sessions. Try again." };
  const message = typeof said.message === "string" ? { message: said.message } : {};
  const started = typeof said.started === "boolean" ? { started: said.started } : {};
  if (said.result === "done" || said.result === "nothing") return { result: said.result, verdict, ...started, ...message };
  if (said.result === "waits") return { result: "waits", verdict, ...message };
  return { result: said.result === "refused" ? "refused" : "failed", verdict, ...message };
}

interface NewMac { home: string; appFolder: string; app: string; release: string; engine: string }

/** A Mac's new box (`rehome --new`, with the installer's release and engine), then its helper
 * started by the link just put in place, as the box will run it. Made already with its helper not
 * in place (one cut short between the two): only the helper. */
async function newMac({ home, appFolder, app, release, engine }: NewMac, judged: () => Judgement): Promise<Answer> {
  const before = judged();
  if (before.verdict !== "fresh" && before.verdict !== "ours") return { result: before.verdict === "waits" ? "waits" : "refused", verdict: before.verdict, message: before.message ?? NOT_NEW[before.verdict] };
  let made = false;
  const earlier = (): string[] => { try { return fs.readdirSync(path.join(home, "trash")).filter(name => name.startsWith("earlier-box-")); } catch { return []; } };
  const asideBefore = new Set(earlier());
  const pinned = checkedRelease(release).engine;
  if (before.verdict === "fresh") {
    if (!fs.existsSync(path.join(engine, pinned, "opencode"))) throw new Said(NO_ENGINE("this Mac"));
    // (Only this release's engine goes into the box: never a download cut short, nor another version's.)
    for (const name of fs.readdirSync(engine)) if (name !== pinned) fs.rmSync(path.join(engine, name), { recursive: true, force: true });
    const outcome = await rehomeNew({ appFolder, home }, { release, engine, waiting: () => console.error("Another move is under way: this one waits for it.") });
    if (outcome.result === "waits") return { result: "waits", verdict: judged().verdict, message: outcome.message };
    if (outcome.result !== "done" && outcome.result !== "nothing") return { result: outcome.result === "refused" ? "refused" : "failed", verdict: judged().verdict, message: outcome.message };
    made = outcome.result === "done";
  }
  const after = judged();
  if (after.verdict !== "ours") return { result: "failed", verdict: after.verdict, message: "Kuvo couldn't be set up on this Mac." };
  const asideNow = earlier().filter(name => !asideBefore.has(name)).sort().pop();
  const setAside = asideNow ? { setAside: path.join(home, "trash", asideNow) } : {};
  // Started by the box's own link, on its own engine, the whole start (`helper start`): its
  // launch agent written and loaded where it isn't, started where it's loaded and not running,
  // opened where Login Items is off for it. Only for the real home, or a test's with stand-ins.
  if (!launchTools()) {
    clearStaging(home);
    return { result: made ? "done" : "nothing", verdict: "ours", started: false, message: "It's made, and starts by itself only in ~/.kuvo-code.", ...setAside };
  }
  const engines = fs.readdirSync(path.join(home, "engine")).filter(name => fs.existsSync(path.join(home, "engine", name, "opencode"))).sort();
  const version = engines.includes(pinned) ? pinned : engines.pop();
  const ran = version ? spawnSync(path.join(home, "engine", version, "opencode"), [...OWN_RUNTIME, path.join(home, "bin", "kuvo-link.js"), "helper", "start", "--app", app], {
    cwd: home, encoding: "utf8", timeout: 180_000, env: { ...process.env, KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" },
  }) : null;
  let said: unknown = null;
  try { said = JSON.parse((ran?.stdout ?? "").trim().split("\n").pop() ?? ""); } catch {}
  const started = ran?.status === 0 && isObject(said) && said.started === true;
  const message = isObject(said) && typeof said.message === "string" ? said.message : started ? undefined : `Kuvo Helper couldn't be started: ${(ran?.stderr ?? "").trim().split("\n").pop() || "it said nothing"}.`;
  // (Not started: what's staged stays, and `status` says to run `new` again.)
  if (!started) return { result: "failed", verdict: "ours", started, message, ...setAside };
  // (A move into it that owed its start owes it no more.)
  settled(home, "start");
  clearStaging(home);
  return { result: made ? "done" : "nothing", verdict: "ours", started, ...(message ? { message } : {}), ...setAside };
}

/** A Linux computer's new box, a machine's, in `~/.kuvo-code` (its keeper names no other): what's
 * there that isn't a box (one the computer that set it up removed or turned off, what a making
 * cut short left) set aside in its trash, nothing deleted, as a Mac's new box does; the release's
 * files and the engine put in place as a computer that's added gets them; then the one start
 * script that add runs there (its settings its own, how its engine starts, its `@reboot` line,
 * its id, its keeper). Under the move's lock, so two never make it at once. */
async function newMachine({ home, release, engine }: { home: string; release: string; engine: string }, judged: () => Judgement): Promise<Answer> {
  const refusal = (): Answer | null => {
    const now = judged();
    // (As a Mac's new box: nothing is made, or set aside, under something that still runs there.)
    if (now.verdict === "fresh") return runningIn(path.join(home, "run")).length ? { result: "refused", verdict: "fresh", message: "Something is running in this computer's ~/.kuvo-code: Kuvo is installed there once that's stopped." } : null;
    return { result: now.verdict === "waits" ? "waits" : "refused", verdict: now.verdict, message: now.message ?? (now.verdict === "machine" ? NOT_NEW.machine : "Kuvo on this computer is moving: that's finished first.") };
  };
  const before = refusal();
  if (before) return before;
  if (path.basename(home) !== ".kuvo-code") return { result: "refused", verdict: "fresh", message: "On Linux, Kuvo is always in ~/.kuvo-code." };
  const { version, engine: engineVersion, files } = checkedRelease(release);
  const staged = path.join(engine, engineVersion, "opencode");
  if (!fs.existsSync(staged)) throw new Said(NO_ENGINE("this computer"));
  if (!(await takeLock(home, { waiting: () => console.error("Another move is under way: this one waits for it.") }))) return { result: "waits", verdict: judged().verdict, message: "Another move is under way." };
  try {
    // (Again, holding it: another may have made it meanwhile.)
    const refused = refusal();
    if (refused) return refused;
    writePrivate(path.join(home, MAKING), JSON.stringify(installAttemptFormat.write({ at: Date.now() })));
    // What's there set aside whole, but the installer's own and the move's records, and the trash.
    const aside = path.join(home, "trash", `earlier-box-${new Date().toISOString().replace(/[:.]/g, "-")}`);
    // (Nor the desktop app's own folder there, `desktop`: its identity among your devices.)
    const kept = new Set(["updates", "trash", "desktop"]);
    const there = fs.readdirSync(home).filter(name => !kept.has(name))
      .concat(fs.readdirSync(path.join(home, "updates")).filter(name => name !== "install" && name !== "rehome").map(name => path.join("updates", name)));
    for (const name of there.sort()) {
      fs.mkdirSync(path.dirname(path.join(aside, name)), { recursive: true, mode: 0o700 });
      fs.renameSync(path.join(home, name), path.join(aside, name));
    }
    const setAside = there.length ? { setAside: aside } : {};
    // The engine first, then each file by name, the keeper's mark and the version last.
    const engineAt = path.join(home, "engine", engineVersion, "opencode");
    fs.mkdirSync(path.dirname(engineAt), { recursive: true, mode: 0o700 });
    fs.copyFileSync(staged, `${engineAt}.new`);
    fs.chmodSync(`${engineAt}.new`, 0o755);
    fs.renameSync(`${engineAt}.new`, engineAt);
    for (const [name, mode] of RELEASE_FILES) {
      const to = path.join(home, name);
      fs.mkdirSync(path.dirname(to), { recursive: true, mode: 0o700 });
      fs.copyFileSync(path.join(release, name), `${to}.new`);
      fs.chmodSync(`${to}.new`, mode);
      fs.renameSync(`${to}.new`, to);
    }
    // What it put there, for telling later whether an older Mac put its own back (`checkDrift`).
    writePrivate(path.join(home, "updates", "installed.json"), JSON.stringify(installedFormat.write({ version, files })));
    const tools = launchTools();
    if (!tools) {
      retireMachineStaging(home);
      return { result: "done", verdict: "machine", started: false, message: "It's made, and starts by itself only in ~/.kuvo-code.", ...setAside };
    }
    // (A test's stand-ins for `crontab` first on the PATH: the script names it plainly, as on a
    // computer it's sent to.)
    const standIns = path.dirname(tools.crontab);
    const ran = spawnSync("sh", ["-c", scripts.start(engineVersion, computerName(), "{}")], {
      cwd: home, input: "", encoding: "utf8", timeout: 300_000,
      env: { ...process.env, ...(path.isAbsolute(standIns) ? { PATH: `${standIns}${path.delimiter}${process.env.PATH ?? ""}` } : {}) },
    });
    const started = ran.status === 0 && /^started$/m.test(ran.stdout ?? "");
    // (Not started: its mark stays, so whatever the start left, keys or a link too, it's `fresh`
    // still, and made again by trying again.)
    if (!started) return { result: "failed", verdict: judged().verdict, started, message: `Kuvo is installed on this computer, but couldn't be started: ${(ran.stderr ?? "").trim().split("\n").pop() || "it said nothing"}. Try again.`, ...setAside };
    retireMachineStaging(home);
    return { result: "done", verdict: judged().verdict, started, ...setAside };
  } finally {
    giveLock(home);
  }
}
