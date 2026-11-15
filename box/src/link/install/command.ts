// `kuvo-link.js install --status | --new | --move` (PROTOCOL.md, "Installing this computer's box";
// #713 §4.1): what `kuvo-install` hands over to, from the release an app carries, on the engine it
// took: this process's `KUVO_HOME` is the box's home, and its script is the release's, unpacked in
// `<home>/updates/install/release`.
//
//   install --status --root <name> [--app <bundle>]                       what this computer holds
//   install --new --root <name> --release <folder> --engine <folder> [--app <bundle>]   a new box, made and started
//   install --move …                                                      the old app's box moved (not built yet)
//
// One JSON line on stdout. Exit: 0 done; 1 refused or failed, in words; 2 not asked as above; 4
// another move holds the lock.
// Platform: a Mac's new box is its own (`box/kind` `mac`, started by Kuvo Helper, `rehome --new`);
// on Linux it's a machine's, in `~/.kuvo-code`, started as a computer that's added is (the one
// start script, `add/scripts.ts`), with its `@reboot` line and no helper.

import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { isObject } from "../../shared/json";
import { RELEASE_FILES } from "../add/files";
import * as scripts from "../add/scripts";
import { writePrivate } from "../files";
import { helperStatus } from "../helper";
import { accountHome, homeFolder, K, located, OWN_RUNTIME } from "../home";
import { homeOfRoot, rehomeNew } from "../rehome/move";
import { launchTools } from "../uninstall";
import { judgeInstall, MAKING, type InstallVerdict, type Judgement } from "./verdict";

const USAGE = "Usage: kuvo-link.js install --status | --new | --move, with --root <name> [--app <bundle>] [--release <folder> --engine <folder>]";
/** The app whose folder is the real one, and the only one that installs into the real `~/.kuvo-code`. */
const REAL_APP = "run.kuvo.code";

interface Answer {
  result: "done" | "nothing" | "refused" | "failed" | "waits";
  verdict: InstallVerdict;
  message?: string;
  /** Whether what was made runs now (its helper, or a machine's keeper). */
  started?: boolean;
}
const EXIT: Record<Answer["result"], number> = { done: 0, nothing: 0, refused: 1, failed: 1, waits: 4 };

/** Why each verdict gets no new box, in words an app shows as they are. */
const NOT_NEW: Record<Exclude<InstallVerdict, "fresh" | "ours">, string> = {
  "app-07": "Kuvo Code has your sessions and settings on this Mac already: they move into their new place instead.",
  "app-box": "Kuvo Code has your sessions and settings on this Mac already: they move into their new place instead.",
  moving: "Kuvo Code is moving your sessions to their new place on this Mac: that's finished first.",
  stuck: "Kuvo Code was moving your sessions to their new place on this Mac and stopped part way: that's finished first.",
  waits: "Another move is under way.",
  back: "Kuvo Code's sessions on this Mac were moved back: they move again when you ask.",
  machine: "This computer is set up as one of your machines already: Kuvo Code uses the box that's here.",
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
  const damaged = new Error("This copy of Kuvo Code is damaged. Download it again, then try again.");
  let box: unknown;
  try { box = JSON.parse(fs.readFileSync(path.join(release, "box.json"), "utf8")); } catch { throw damaged; }
  if (!isObject(box) || typeof box.version !== "string" || typeof box.engine !== "string" || !/^[0-9A-Za-z][0-9A-Za-z.-]*$/.test(box.engine) || !isObject(box.files)) throw damaged;
  for (const [name, sum] of Object.entries(box.files)) {
    const file = path.join(release, name);
    if (name.split("/").includes("..") || path.isAbsolute(name) || fs.lstatSync(file, { throwIfNoEntry: false })?.isFile() !== true || sha256(file) !== sum) throw damaged;
  }
  for (const [name] of RELEASE_FILES) if (!Object.hasOwn(box.files, name)) throw damaged;
  return { version: box.version, engine: box.engine, files: box.files as Record<string, string> };
}

/** This computer's name, as you named it (a Mac's, in System Settings), else its host's. */
const computerName = (): string => os.hostname().replace(/\.(local|lan|home)$/i, "") || "computer";

export async function installCommand(args: string[]): Promise<number> {
  // (Whether something runs is told by its process, never a stale record: as `rehome`.)
  delete process.env.KUVO_BOX;
  const option = (name: string): string | undefined => (args.indexOf(name) >= 0 ? args[args.indexOf(name) + 1] : undefined);
  const say = (value: unknown) => console.log(JSON.stringify(value));
  const verbs = ["--status", "--new", "--move"].filter(verb => args.includes(verb));
  let home: string;
  try {
    if (verbs.length !== 1) throw new Error("Say one of --status, --new and --move.");
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
  const judged = (): Judgement => judgeInstall(home, appFolder, { helperInPlace: () => (mac && launchTools() ? helperStatus().agent : null) });
  try {
    if (mac && !app) {
      say({ result: "refused", message: "Kuvo Code's installer needs the app it's run from (--app <bundle>).", ...judgeInstall(home, null) });
      return EXIT.refused;
    }
    // (A copy of the app, a demo's or a test's, never judges or makes the real one's box, nor the
    // real app a copy's: each has its own folder and its own home.)
    if (mac && real !== (app === REAL_APP)) {
      say({ result: "refused", message: real ? "Only Kuvo Code itself installs its box in ~/.kuvo-code: a copy installs it in a home of its own." : "Kuvo Code itself installs its box in ~/.kuvo-code only.", ...judgeInstall(home, null) });
      return EXIT.refused;
    }
    if (verbs[0] === "--status") {
      const judgement = judged();
      tidyStaging(home, judgement, option("--release"));
      say(judgement);
      return 0;
    }
    if (verbs[0] === "--move") {
      // BOX-NEEDED (#713 piece 4): `rehome --run`, the old app's box and data moved by the box itself.
      say({ result: "refused", message: "Moving an earlier Kuvo Code's sessions into their new place isn't in this version yet.", ...judged() });
      return EXIT.refused;
    }
    const release = option("--release"), engine = option("--engine");
    if (!release || !engine || !path.isAbsolute(release) || !path.isAbsolute(engine)) {
      console.error(USAGE);
      return 2;
    }
    const answer = mac ? await newMac({ home, appFolder: appFolder!, app: app!, release, engine }, judged) : newMachine({ home, release, engine }, judged);
    say({ ...answer, status: judged().status });
    return EXIT[answer.result];
  } catch (error) {
    say({ result: "failed", message: error instanceof Error ? error.message : String(error), ...judged() });
    return EXIT.failed;
  }
}

/** What the installer staged, gone once a box is made from it (the engine this runs on with it:
 * a running program keeps its file). */
const clearStaging = (home: string): void => fs.rmSync(path.join(home, "updates", "install"), { recursive: true, force: true });

/** The engine the installer fetched, gone once the box that's here has that engine itself (the
 * installer runs on the box's own from then on): never two copies kept. Only where nothing is to
 * be made or moved from what's staged. */
function tidyStaging(home: string, { verdict, verb }: Judgement, release: string | undefined): void {
  if (verb !== null || !["ours", "machine", "two"].includes(verdict) || !release) return;
  try {
    const box: unknown = JSON.parse(fs.readFileSync(path.join(release, "box.json"), "utf8"));
    const version = isObject(box) && typeof box.engine === "string" && /^[0-9A-Za-z][0-9A-Za-z.-]*$/.test(box.engine) ? box.engine : null;
    if (version && fs.existsSync(path.join(home, "engine", version, "opencode"))) fs.rmSync(path.join(home, "updates", "install", "engine"), { recursive: true, force: true });
  } catch {}
}

interface NewMac { home: string; appFolder: string; app: string; release: string; engine: string }

/** A Mac's new box (`rehome --new`, with the installer's release and engine), then its helper
 * started by the link just put in place, as the box will run it. Made already with its helper not
 * in place (one cut short between the two): only the helper. */
async function newMac({ home, appFolder, app, release, engine }: NewMac, judged: () => Judgement): Promise<Answer> {
  const before = judged();
  if (before.verdict !== "fresh" && before.verdict !== "ours") return { result: before.verdict === "waits" ? "waits" : "refused", verdict: before.verdict, message: NOT_NEW[before.verdict] };
  let made = false;
  if (before.verdict === "fresh") {
    const { engine: version } = checkedRelease(release);
    if (!fs.existsSync(path.join(engine, version, "opencode"))) throw new Error("The engine isn't downloaded yet: this Mac's box is made once it is.");
    const outcome = await rehomeNew({ appFolder, home }, { release, engine, waiting: () => console.error("Another move is under way: this one waits for it.") });
    if (outcome.result === "waits") return { result: "waits", verdict: judged().verdict, message: outcome.message };
    if (outcome.result !== "done" && outcome.result !== "nothing") return { result: outcome.result === "refused" ? "refused" : "failed", verdict: judged().verdict, message: outcome.message };
    made = outcome.result === "done";
  }
  const after = judged();
  if (after.verdict !== "ours") return { result: "failed", verdict: after.verdict, message: "This Mac's box couldn't be made." };
  // Started by the box's own link, on its own engine: its launch agent written and loaded (opened
  // where Login Items is off for it). Only for the real home, or a test's with stand-ins.
  let started = false, message: string | undefined;
  if (!launchTools()) message = "It's made, and starts by itself only in ~/.kuvo-code.";
  else {
    const version = fs.readdirSync(path.join(home, "engine")).filter(name => fs.existsSync(path.join(home, "engine", name, "opencode"))).sort().pop();
    const ran = version ? spawnSync(path.join(home, "engine", version, "opencode"), [...OWN_RUNTIME, path.join(home, "bin", "kuvo-link.js"), "helper", "start", "--if-missing", "--app", app], {
      cwd: home, encoding: "utf8", timeout: 180_000, env: { ...process.env, KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" },
    }) : null;
    let said: unknown = null;
    try { said = JSON.parse((ran?.stdout ?? "").trim().split("\n").pop() ?? ""); } catch {}
    started = ran?.status === 0 && isObject(said) && (said.started === true || said.left === true);
    message = isObject(said) && typeof said.message === "string" ? said.message : started ? undefined : `Kuvo Helper couldn't be started: ${(ran?.stderr ?? "").trim().split("\n").pop() || "it said nothing"}.`;
  }
  if (started || !launchTools()) clearStaging(home);
  if (!started && launchTools()) return { result: "failed", verdict: "ours", started, message };
  return { result: made ? "done" : "nothing", verdict: "ours", started, ...(message ? { message } : {}) };
}

/** A Linux computer's new box, a machine's, in `~/.kuvo-code` (its keeper names no other): the
 * release's files and the engine put in place as a computer that's added gets them, then the one
 * start script that add runs there (its settings its own, how its engine starts, its `@reboot`
 * line, its id, its keeper). Nothing of it there yet: a box that's there is never written over. */
function newMachine({ home, release, engine }: { home: string; release: string; engine: string }, judged: () => Judgement): Answer {
  const before = judged();
  if (before.verdict !== "fresh") return { result: before.verdict === "waits" ? "waits" : "refused", verdict: before.verdict, message: before.verdict === "machine" ? NOT_NEW.machine : "A move is under way in this computer's box: that's finished first." };
  if (path.basename(home) !== ".kuvo-code") return { result: "refused", verdict: before.verdict, message: "On Linux, Kuvo Code's box is always in ~/.kuvo-code." };
  const { version, engine: engineVersion, files } = checkedRelease(release);
  const staged = path.join(engine, engineVersion, "opencode");
  if (!fs.existsSync(staged)) throw new Error("The engine isn't downloaded yet: this computer's box is made once it is.");
  // The engine first, then each file by name, the keeper's mark and the version last (one cut
  // short is seen as not done, and made again: nothing here says a box is there until they are).
  writePrivate(path.join(home, MAKING), `${Date.now()}\n`);
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
  writePrivate(path.join(home, "updates", "installed.json"), JSON.stringify({ version, files }));
  const tools = launchTools();
  if (!tools) {
    clearStaging(home);
    return { result: "done", verdict: "machine", started: false, message: "It's made, and starts by itself only in ~/.kuvo-code." };
  }
  // (A test's stand-ins for `crontab` first on the PATH: the script names it plainly, as on a
  // computer it's sent to.)
  const standIns = path.dirname(tools.crontab);
  const ran = spawnSync("sh", ["-c", scripts.start(engineVersion, computerName(), "{}")], {
    cwd: home, input: "", encoding: "utf8", timeout: 300_000,
    env: { ...process.env, ...(path.isAbsolute(standIns) ? { PATH: `${standIns}${path.delimiter}${process.env.PATH ?? ""}` } : {}) },
  });
  const started = ran.status === 0 && /^started$/m.test(ran.stdout ?? "");
  if (!started) return { result: "failed", verdict: judged().verdict, started, message: `Kuvo Code is installed on this computer, but couldn't be started: ${(ran.stderr ?? "").trim().split("\n").pop() || "it said nothing"}. Try again.` };
  clearStaging(home);
  return { result: "done", verdict: judged().verdict, started };
}
