// COMPAT(beta.30 helper-handover): a Kuvo Helper from before beta.31 handed over to the new build in its place, once. Goes once no box is from before beta.31.
//
// A Kuvo Helper from before beta.31 never leaves for a new build of itself put in its place (one
// since does, exiting 75 for launchd to start the new one: BoxHelper.swift, `renewed`), so after an
// update it runs on, old, until your next login: the menu bar item, Open Kuvo and the box's host
// interface stay its old code's. Its server, `serve`, its own child (started again on the new code
// as the update leaves it), tells (PROTOCOL.md, "This Mac's box: its helper"):
//
// - the helper that started it said nothing of renewing itself on its stdin (one since says
//   `"renews": true`), and
// - the build in place does renew itself (its Info.plist's `KuvoHelperRenews`).
//
// Then, once for each build in place (`run/helper-handover.json`, by that bundle's seal, as the
// helper's own `seal(of:)` hashes it), it hands the helper over, in a process of its own (the helper
// stopping takes `serve` with it): the old helper told to stop, as launchd and logging out tell it
// (SIGTERM: its server's stdin closed, its keeper told to stop, and the engine, the coding agents'
// bridge and their turns, detached, run on; it exits 0, which launchd doesn't start again), then,
// once it's gone, started as the app starts a stopped one (`helper start`: launchd's kickstart, or
// opened where Login Items is off for it). Nothing at work is cut, so it doesn't wait for a quiet
// moment, as the helper's own exit 75 doesn't. Never where Kuvo on this Mac was stopped (`run/paused`,
// `run/off`), before or as it goes, nor while an update is under way; a helper still the old one
// after its try is noted, never tried again for that build.

import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { writePrivate } from "../../shared/files";
import { alive, commandLineOf } from "../../shared/platform";
import { updateUnderWay } from "../../shared/updating";
import { HANDOVER_RECORD, HELPER_NAME, helperBundle, helperProgram, startHelperAgent } from "../helper";
import { K, OWN_HOME_MAC, OWN_RUNTIME, REAL_BOX, RUN, SCRIPT } from "../home";
import { LAUNCH_AGENT } from "../marks";
import { launchTools } from "../uninstall";

/** What tells one build of the helper's bundle from another, as the helper hashes it (`seal(of:)`):
 * its program, its Info.plist and its seal, hashed together; null while one is missing (a bundle
 * being swapped in). */
export function helperSeal(bundle = helperBundle()): string | null {
  const hash = crypto.createHash("sha256");
  for (const part of [path.join("Contents", "MacOS", HELPER_NAME), path.join("Contents", "Info.plist"), path.join("Contents", "_CodeSignature", "CodeResources")]) {
    try { hash.update(fs.readFileSync(path.join(bundle, part))); } catch { return null; }
  }
  return hash.digest("hex");
}

/** Whether the build in place leaves by itself for a newer one (its Info.plist says so). */
export function bundleRenews(bundle = helperBundle()): boolean {
  try {
    return /<key>KuvoHelperRenews<\/key>\s*<true\s*\/>/.test(fs.readFileSync(path.join(bundle, "Contents", "Info.plist"), "utf8"));
  } catch {
    return false;
  }
}

/** The one try for a build in place: which build, the helper it was for, when, and how it went
 * (`handing` while under way; `handed`: the new one started; `stopped`: Kuvo was stopped on this
 * Mac meanwhile, so nothing was started; `gone`: the old one had gone already, and nothing was done;
 * `stayed`: the old one didn't go; `failed`: launchd or macOS
 * didn't start the new one, `message` says why). `stillOld`: a helper found still the old one after
 * the try, noted once. */
export interface HandOver {
  v: 1;
  bundle: string;
  pid: number;
  at: number;
  outcome: "handing" | "handed" | "stopped" | "gone" | "stayed" | "failed";
  message?: string;
  stillOld?: { pid: number; at: number };
}

export function readHandOver(): HandOver | null {
  try {
    const record = JSON.parse(fs.readFileSync(HANDOVER_RECORD(), "utf8"));
    return record && typeof record === "object" && typeof record.bundle === "string" ? record : null;
  } catch {
    return null;
  }
}

const note = (record: HandOver) => writePrivate(HANDOVER_RECORD(), JSON.stringify(record));

/** Whether Kuvo on this Mac was stopped (until it next starts, or for good): a helper then is never
 * started. */
const stoppedHere = (): boolean => fs.existsSync(path.join(RUN, "paused")) || fs.existsSync(path.join(RUN, "off"));

/** Whether `pid` is this box's helper keeping it (`<bin/Kuvo Helper.app>/…/Kuvo Helper keep`; any run
 * of slashes taken as one, as the keeper takes them). */
const slashes = (text: string) => text.replace(/\/+/g, "/");
export const isOwnHelper = (pid: number): boolean => pid > 1 && alive(pid) && slashes(commandLineOf(pid) ?? "").includes(slashes(`${helperProgram()} keep`));

/** What `serve` does about the helper that started it (`parent`), which said nothing of renewing
 * itself on its stdin: `none` where there's nothing to do, ever (one that isn't this box's helper, Kuvo stopped here, the build in place no newer, or its try made already);
 * `later` while it can't be told yet (an update under way, the bundle being swapped in); `hand-over`
 * once it's due, its record noted first. */
export function considerHelper(parent: number): "none" | "later" | "hand-over" {
  if (!OWN_HOME_MAC || !launchTools() || !isOwnHelper(parent) || stoppedHere()) return "none";
  if (updateUnderWay(RUN)) return "later";
  const bundle = helperSeal();
  if (!bundle) return "later";
  if (!bundleRenews()) return "none";
  const tried = readHandOver();
  if (tried?.bundle === bundle) {
    // Still the old one after its try: said once, never tried again (it goes at your next login).
    if (!tried.stillOld && tried.outcome !== "handing") {
      note({ ...tried, stillOld: { pid: parent, at: Date.now() } });
      console.error(`${HELPER_NAME} is still the one from before the update after it was handed over once (${tried.outcome}): it moves to the new one at your next login.`);
    }
    return "none";
  }
  note({ v: 1, bundle, pid: parent, at: Date.now(), outcome: "handing" });
  return "hand-over";
}

/** `serve`'s watch of the helper that started it: looked at as it starts, then every 30 seconds (a
 * test's as often as it says) until there's nothing more to do; the hand-over, when due, started in a
 * process of its own, which outlasts this server. `renews`: what the helper said on serve's stdin
 * (undefined where serve wasn't started so: nothing is done then). */
export function watchHelperBuild(renews: boolean | undefined): void {
  if (renews === undefined || renews || !OWN_HOME_MAC) return;
  const parent = process.ppid;
  const every = !REAL_BOX && Number(process.env.KUVO_TEST_CODE_EVERY) > 0 ? Number(process.env.KUVO_TEST_CODE_EVERY) : 30_000;
  let timer: ReturnType<typeof setInterval> | undefined;
  const look = (): boolean => {
    let said: ReturnType<typeof considerHelper>;
    try { said = considerHelper(parent); } catch { said = "none"; }
    if (said === "later") return false;
    if (timer) clearInterval(timer);
    if (said === "hand-over") {
      console.error(`${HELPER_NAME} is from before the update and doesn't move to the new one by itself: it's handed over once.`);
      try {
        spawn(process.execPath, [...OWN_RUNTIME, SCRIPT, "helper", "hand-over", "--pid", String(parent)], { detached: true, stdio: "ignore", cwd: K, env: process.env }).unref();
      } catch (error) {
        const record = readHandOver();
        if (record) note({ ...record, outcome: "failed", message: (error as Error).message });
      }
    }
    return true;
  };
  if (look()) return;
  timer = setInterval(look, every);
  timer.unref?.();
}

const pause = (ms: number) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

/** The app's bundle ID the launch agent names (`AssociatedBundleIdentifiers`), for a start that has to
 * write it again. */
function appOfAgent(agents: string): string {
  try {
    const plist = fs.readFileSync(path.join(agents, `${LAUNCH_AGENT}.plist`), "utf8");
    const found = /<key>AssociatedBundleIdentifiers<\/key>\s*<array>\s*<string>([A-Za-z0-9][A-Za-z0-9.-]*)<\/string>/.exec(plist)?.[1];
    if (found) return found;
  } catch {}
  return "run.kuvo.code";
}

/** `helper hand-over --pid <helper>`: the hand-over itself (above), its outcome noted in its record
 * and said in one JSON line. 0 once the new one's started, or there was nothing to do; 1 otherwise. */
export function handOverCommand(args: string[]): number {
  const at = args.indexOf("--pid");
  const pid = at >= 0 ? Number(args[at + 1]) : NaN;
  if (!Number.isInteger(pid) || pid <= 1) {
    console.error("Usage: kuvo-link.js helper hand-over --pid <Kuvo Helper's process>");
    return 2;
  }
  const record = readHandOver();
  const done = (outcome: HandOver["outcome"], message: string): number => {
    if (record && record.pid === pid) note({ ...record, outcome, message });
    console.log(JSON.stringify({ outcome, message }));
    return outcome === "handed" ? 0 : 1;
  };
  const tools = launchTools();
  if (!tools || !OWN_HOME_MAC) return done("failed", `Only Kuvo on a Mac in ~/.kuvo-code runs under ${HELPER_NAME}.`);
  if (stoppedHere()) return done("stopped", "Kuvo on this Mac was stopped: nothing was started.");
  if (!isOwnHelper(pid)) return done("gone", `${HELPER_NAME} from before the update had gone already: nothing was done.`);
  try { process.kill(pid, "SIGTERM"); } catch {}
  // It stops its server and keeper first (three seconds at most, then a second more), and exits.
  for (let waited = 0; waited < 200 && alive(pid); waited++) pause(100);
  if (alive(pid)) return done("stayed", `${HELPER_NAME} from before the update didn't stop: it moves to the new one at your next login.`);
  // And until launchd has seen it go: a start while it still says it runs would leave it as it is.
  const domain = `gui/${process.getuid?.() ?? 0}`;
  for (let waited = 0; waited < 50 && new RegExp(`\\bpid = ${pid}\\b`).test(spawnSync(tools.launchctl, ["print", `${domain}/${LAUNCH_AGENT}`], { encoding: "utf8", timeout: 30_000, stdio: ["ignore", "pipe", "ignore"] }).stdout ?? ""); waited++) pause(100);
  // Stopped meanwhile (Stop Kuvo on this Mac as it went): it stays stopped.
  if (stoppedHere()) return done("stopped", "Kuvo on this Mac was stopped as it went: nothing was started.");
  const started = startHelperAgent({ app: appOfAgent(tools.agents) });
  return started.started ? done("handed", `${HELPER_NAME} runs the new version now.`) : done("failed", started.message);
}
