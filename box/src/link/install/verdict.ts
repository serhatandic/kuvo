// What this computer holds, as the box's installer judges it (PROTOCOL.md, "Installing this
// computer's box"; #713 §4.1): one verdict from the folders alone, for the app that carries the
// release to draw, and the installer's verb to run next. `home` is the box's home (`~/.kuvo-code`,
// or a demo copy's own), `appFolder` the old Mac app's folder (`~/Library/Application
// Support/run.kuvo.code`), which only a Mac has: null on Linux.
// Platform: the same on macOS and Linux; Linux has no app folder, so only `fresh`, `machine` and
// the move's own (`waits`, `moving`, `stuck`) can be said there.

import fs from "node:fs";
import path from "node:path";
import { recordedKind } from "../home";
import { makingCutShort, runningIn } from "../rehome/move";
import { forwardOnly, heldByAnother, readJournal, readRecord, rehomeStatus } from "../rehome/state";
import { foundAt, OWN_BOX_RAN, runsAt } from "../setup/judge";
import type { RehomeStatus } from "../../shared/routes/box";

export type InstallVerdict = "fresh" | "ours" | "app-07" | "app-box" | "moving" | "stuck" | "waits" | "back" | "machine" | "two";
/** What the installer is run with next: a new box made, the old one moved, or nothing (the app
 * attaches, waits, or says how things stand). */
export type InstallVerb = "new" | "move" | null;

/** What's in the old app's folder that isn't anyone's data: its engine's download and caches, the
 * desktop app's own folder there (window state, `gpui/kuvo`), and its box's folder, judged apart. */
const NOT_DATA = new Set(["engine", "box", "desktop", "Caches", "cache", "tmp", "logs", "Logs"]);
const listOf = (folder: string): string[] => { try { return fs.readdirSync(folder); } catch { return []; } };
const exists = (file: string): boolean => { try { fs.lstatSync(file); return true; } catch { return false; } };

/** Whether a box of the old app's own ran in its folder (`<app folder>/box`, a keeper build's). */
export const appBoxThere = (appFolder: string): boolean => OWN_BOX_RAN.some(file => exists(path.join(appFolder, "box", "run", file)));

/** Whether the old app left anything of yours in its folder: any entry but what's never data. The
 * engine's own folder (`opencode`) counts only for more than its cache. Broad on purpose: taking a
 * Mac with data for a new one would start a box beside it and leave the data behind. */
export function appDataThere(appFolder: string): boolean {
  for (const name of listOf(appFolder)) {
    if (name.startsWith(".") || NOT_DATA.has(name)) continue;
    if (name === "opencode") {
      if (listOf(path.join(appFolder, name)).some(inner => !inner.startsWith(".") && inner !== "cache")) return true;
      continue;
    }
    return true;
  }
  // (What the app kept in its box's folder with no box of its own running there: its lists.)
  return listOf(path.join(appFolder, "box")).some(name => !name.startsWith(".") && name !== "run" && name !== "bin");
}

/** The installer's mark while it makes a machine's box on Linux (in what it staged, gone with it
 * once the box has started): what's there under it was cut short, and is made again. */
export const MAKING = path.join("updates", "install", "making");

/** Whether a machine's box is in `home`: one the computer that set it up hasn't removed (`runsAt`:
 * `gone`, with no way to start or turned off, is no box), with a card or a link, keys of its own,
 * or its kind said. On Linux, where no box is a Mac's own, its start script there is enough; and
 * under the installer's mark (`MAKING`) whatever is there is a making that never finished (its
 * keys and link too, where its start failed after writing them), unless it runs: then it started,
 * and only the mark was left. */
export function machineBoxThere(home: string, linux = process.platform !== "darwin"): boolean {
  const kind = recordedKind(path.join(home, "box", "kind"));
  if (kind === "mac" || runsAt(home) === "gone") return false;
  if (linux && exists(path.join(home, MAKING))) return runningIn(path.join(home, "run")).length > 0;
  if (foundAt(home, false) !== null || kind === "machine" || exists(path.join(home, "box", "keys"))) return true;
  return linux;
}

export interface Judgement {
  verdict: InstallVerdict;
  verb: InstallVerb;
  /** How a move stands (`kuvo/box/rehome` says the same), for its step to be drawn. */
  status: RehomeStatus;
}

/** The verdict. `helperInPlace`: whether what starts a Mac's own box at login is there (null where
 * that can't be told, a demo copy's home; asked only of a Mac's own box): a box made whose start
 * isn't in place is finished by `new`. */
export function judgeInstall(home: string, appFolder: string | null, { helperInPlace = (): boolean | null => null } = {}): Judgement {
  const status = rehomeStatus(home);
  const say = (verdict: InstallVerdict, verb: InstallVerb = null): Judgement => ({ verdict, verb, status });
  if (heldByAnother(home)) return say("waits");
  let journal: ReturnType<typeof readJournal> = null;
  let unreadable = false;
  try { journal = readJournal(home); } catch { unreadable = true; }
  if (unreadable || (journal && journal.step !== "done" && forwardOnly(journal.step))) return say("stuck", "move");
  if (journal && journal.step !== "done") return say("moving", "move");
  // (A move the box ran itself that still owes something, its start, the Keychain's originals or
  // an earlier Kuvo Code's saved sign-ins, is finished by running it again.)
  if (recordedKind(path.join(home, "box", "kind")) === "mac" && !makingCutShort(home)) {
    const record = readRecord(home);
    const owes = record.state === "done" && record.app !== undefined && (record.owed ?? []).length > 0;
    return say("ours", owes ? "move" : helperInPlace() === false ? "new" : null);
  }
  if (readRecord(home).movedBack === true) return say("back");
  const data = appFolder !== null && (appBoxThere(appFolder) || appDataThere(appFolder));
  if (machineBoxThere(home)) return say(data ? "two" : "machine");
  if (appFolder !== null && appBoxThere(appFolder)) return say("app-box", "move");
  if (data) return say("app-07", "move");
  return say("fresh", "new");
}
