// Your machines kept up to date (PROTOCOL.md, "Computers", Keeping them up to date;
// `computers.update`): the box that set a machine up over SSH brings its Kuvo up to its own once
// it's older, as a Mac's app did each time it connected before boxes (the update gate's "A Mac, its
// machines"). A machine can't take a newer release from a client over the relay before it has this
// (one from before `kuvo/box/update`, a beta GitHub's latest release never names), so this is how
// an older one is ever brought up: by itself, with no client open, only while nothing is at work
// there, never cutting a turn short (`flow.ts`'s Set Up Again leaves its running engine alone, and
// its coding agents' bridge makes way when nothing runs). Or now, on your word (Update Now), asked
// first while something is at work. Every word a client shows is here; what went wrong in detail
// goes to this box's log.
// Platform: the same on macOS and Linux, though only a Mac's box sets machines up for now (`canAdd`).

import fs from "node:fs";
import path from "node:path";
import type { ComputerAction, ComputerUpdate, ListedComputer } from "../../shared/routes/box";
import { isObject } from "../../shared/json";
import { writePrivate } from "../files";
import { RUN } from "../home";
import { readPeerAgents } from "../peers/directory";
import { isApart } from "../peers/apart";
import { peersOf } from "../peers/known";
import { readLink } from "../state";
import { boxVersion, newerVersion } from "../updates/update";
import { type Busy, beginUpdate, canAdd, lookAtWork, sshAddressOf } from "./flow";
import { type Running, underWay } from "./tries";

/** What this box knows of each machine it keeps up, by its key. */
export const KEEP_UP_FILE = path.join(RUN, "keep-up.json");

/** How often a round runs (and so how often an older machine at work is looked at again). */
export const KEEP_UP_ROUND = 15 * 60_000;
/** How long a machine found as new as this box isn't reached again for it. */
const CURRENT_FOR = 86_400_000;
/** How long it waits for nothing to be at work before it says so, and offers Update Now. */
const LONG_WAIT = 86_400_000;
/** After one that failed, an hour before it's tried again; after more, a day. */
const RETRY = 3_600_000;
const REST = 86_400_000;

/** One machine, as this box last found it: whether it's older than this box (`mine`, this box's
 * version then), when it was looked at, since when it waits for nothing to be at work, how many
 * conversations were at work there (null: it couldn't be told), and how bringing it up last failed. */
interface Kept {
  older: boolean;
  mine: string;
  checked: number;
  since?: number;
  working?: number | null;
  failed?: { at: number; tries: number };
}

function readKept(): Record<string, Kept> {
  try {
    const kept = JSON.parse(fs.readFileSync(KEEP_UP_FILE, "utf8"));
    return isObject(kept) ? (kept as Record<string, Kept>) : {};
  } catch { return {}; }
}

/** `key`'s entry changed as `change` says (none: it's forgotten), the rest as they are. */
function changeKept(key: string, change: (was: Kept | undefined) => Kept | null): void {
  const kept = readKept();
  const next = change(kept[key]);
  if (next) kept[key] = next;
  else delete kept[key];
  try { writePrivate(KEEP_UP_FILE, JSON.stringify(kept)); } catch {}
}

/** Begins bringing `key` up to date (`auto`: by this box itself), and notes how it ends: done, it's
 * forgotten; failed, it rests (an hour, then a day) and its words go to the log. */
export function bringUp(key: string, auto: boolean, clock: () => number = Date.now): Running | Busy | null {
  const begun = beginUpdate(key, auto);
  if (!begun || "busy" in begun) return begun;
  // (Asked on your word: whatever rested is tried now.)
  if (!auto) changeKept(key, was => (was ? { ...was, failed: undefined } : null));
  const started = begun.seen.id;
  void begun.over.then(() => {
    const seen = begun.seen;
    if (seen.id !== started) return;
    if (seen.state === "done") return changeKept(key, () => null);
    if (seen.state !== "failed") return;
    console.error(`Kuvo couldn't bring ${seen.name} up to date: ${seen.failure?.message ?? "it failed"}`);
    changeKept(key, was => ({ older: true, mine: boxVersion() ?? "", checked: clock(), ...(was ?? {}), failed: { at: clock(), tries: (was?.failed?.tries ?? 0) + 1 } }));
  });
  return begun;
}

let going = false;

/** One round: each machine this box set up over SSH whose Kuvo is older than its own (by what it
 * said of itself, or, where it says nothing, by looking over SSH) is brought up to date if nothing
 * is at work there now; otherwise it waits for the next round. Never two rounds at once, never one
 * under way for that machine, never while one rests after failing. */
export async function keepUpRound(clock: () => number = Date.now): Promise<void> {
  if (going || !canAdd().can) return;
  const mine = boxVersion();
  if (!mine) return;
  going = true;
  try {
    const link = readLink();
    const told = readPeerAgents();
    for (const peer of peersOf(link)) {
      const at = peer.kind === "machine" && !isApart(peer.key, link, told) ? sshAddressOf(peer.key) : null;
      if (!at || underWay(at.destination)) continue;
      const now = clock();
      const was = readKept()[peer.key];
      const said = told[peer.key]?.computer;
      // As new as this box, by its own word: nothing to do.
      if (said?.version && !newerVersion(mine, said.version)) {
        if (was) changeKept(peer.key, () => null);
        continue;
      }
      if (was?.failed && now - was.failed.at < (was.failed.tries > 1 ? REST : RETRY)) continue;
      if (was && was.mine === mine && !was.older && now - was.checked < CURRENT_FOR) continue;
      // At work, by its own word (a box that tells its peers): it waits, and isn't reached.
      if (said?.version && typeof said.working === "number" && said.working > 0) {
        const working = said.working;
        changeKept(peer.key, before => ({ ...(before ?? {}), older: true, mine, checked: now, since: before?.since ?? now, working }));
        continue;
      }
      let looked: Awaited<ReturnType<typeof lookAtWork>>;
      try {
        looked = await lookAtWork(peer.key);
      } catch (error) {
        console.error(`Kuvo couldn't look at ${at.name} to keep it up to date: ${error instanceof Error ? error.message : String(error)}`);
        changeKept(peer.key, before => ({ older: true, mine, ...(before ?? {}), checked: now, failed: { at: now, tries: (before?.failed?.tries ?? 0) + 1 } }));
        continue;
      }
      if (!looked) continue;
      if (looked.version && !newerVersion(mine, looked.version)) {
        changeKept(peer.key, () => ({ older: false, mine, checked: now }));
        continue;
      }
      if (looked.working !== 0) {
        const working = looked.working;
        changeKept(peer.key, before => ({ ...(before ?? {}), older: true, mine, checked: now, since: before?.since ?? now, working }));
        continue;
      }
      changeKept(peer.key, before => ({ ...(before ?? {}), older: true, mine, checked: now, working: 0 }));
      bringUp(peer.key, true, clock);
    }
  } finally {
    going = false;
  }
}

/** Rounds from now on, the first a minute in (a test's box runs none). */
export function keepMachinesUp(): void {
  const round = () => { keepUpRound().catch(error => console.error(`Kuvo's round keeping machines up to date failed: ${String(error)}`)); };
  setTimeout(round, 60_000).unref?.();
  setInterval(round, KEEP_UP_ROUND).unref?.();
}

const sessions = (count: number): string => `${count} ${count === 1 ? "session is" : "sessions are"}`;

/** How bringing `computer` up to date stands, in words (`update`), and Update Now (`actions`), on
 * the box that set it up over SSH: being brought up, waiting for nothing to be at work there (over
 * a day: `long`), or failed (tried again by itself). Nothing for one as new as this box, one it
 * didn't set up, and one that can't be reached now. */
export function keepingUp(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "apart" | "reach" | "version" | "working">, here: string, now = Date.now()): { update?: ComputerUpdate; actions: ComputerAction[] } {
  const none = { actions: [] };
  if (computer.here || computer.apart || computer.kind !== "machine" || !computer.key) return none;
  const mine = boxVersion();
  const at = mine ? sshAddressOf(computer.key) : null;
  if (!mine || !at || !canAdd().can) return none;
  const name = computer.name || "this machine";
  const under = underWay(at.destination);
  if (under && (under.seen.kind === "update" || under.seen.kind === "setup")) {
    return { update: { state: "updating", title: `Updating Kuvo on ${name}…`, detail: `${here} is bringing it up to date. What’s at work there keeps going.` }, actions: [] };
  }
  const was = readKept()[computer.key];
  const older = computer.version ? newerVersion(mine, computer.version) : was?.older === true && was.mine === mine;
  if (!older || computer.reach === "away") return none;
  const working = computer.version && typeof computer.working === "number" ? computer.working : was?.working ?? null;
  const confirm = working === 0 ? undefined : {
    title: `Update Kuvo on ${name} now?`,
    message: working
      ? `${sessions(working)} at work on ${name}. ${working === 1 ? "It keeps" : "They keep"} going while it updates, but may pause for a moment as Kuvo there restarts.`
      : `Something may be at work on ${name}. It keeps going while it updates, but may pause for a moment as Kuvo there restarts.`,
    label: "Update Now",
  };
  const actions: ComputerAction[] = [{ id: "update", label: "Update Now", ...(confirm ? { confirm } : {}), ...(under ? { why: under.seen.kind === "uninstall" ? "Kuvo Code is being uninstalled from it now." : "Something else is being done with it now." } : {}) }];
  if (was?.failed) {
    return { update: { state: "failed", title: `Kuvo on ${name} couldn’t be updated`, detail: `${here} tries again by itself later. Or update it now.` }, actions };
  }
  const long = was?.since !== undefined && working !== 0 && now - was.since >= LONG_WAIT;
  if (long) {
    return { update: { state: "waiting", long: true, title: `Kuvo on ${name} is waiting to update`, detail: `Something has been at work on ${name} for over a day. Update it now, or it updates by itself once nothing is.` }, actions };
  }
  if (working === 0) {
    return { update: { state: "waiting", title: `Kuvo on ${name} updates in a moment`, detail: `${here} is about to bring it up to date.` }, actions };
  }
  return { update: { state: "waiting", title: `Kuvo on ${name} updates once nothing’s at work there`, detail: working ? `${sessions(working)} at work there now. ${here} brings it up to date as soon as ${working === 1 ? "it’s" : "they’re"} done.` : `${here} brings it up to date once it can tell nothing is.` }, actions };
}
