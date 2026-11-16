// Your machines kept up to date (PROTOCOL.md, "Computers", Keeping them up to date;
// `computers.update`): the box that set a machine up over SSH brings its Kuvo up to its own once
// it's older, as a Mac's app did each time it connected before boxes (the update gate's "A Mac, its
// machines"). A machine can't take a newer release from a client over the relay before it has this
// (one from before `kuvo/box/update`, a beta GitHub's latest release never names), so this is how
// an older one is ever brought up: by itself, with no client open, whatever is at work there, as
// nothing is cut short (`flow.ts`'s Set Up Again leaves its running engine alone, and its coding
// agents' bridge makes way when nothing runs). Or now, on your word (Update Now). Every word a client shows is here, one line each; what went
// wrong in detail goes to this box's log.
// Platform: the same on macOS and Linux, though only a Mac's box sets machines up for now (`canAdd`).

import fs from "node:fs";
import path from "node:path";
import type { ComputerAction, ComputerUpdate, ListedComputer } from "../../shared/routes/box";
import { isObject } from "../../shared/json";
import { peerReach } from "../areas/computers";
import { writePrivate } from "../files";
import { IS_MAC, RUN } from "../home";
import { readPeerAgents } from "../peers/directory";
import { isApart } from "../peers/apart";
import { peersOf } from "../peers/known";
import { mainElsewhere } from "../setup/main-only";
import { readLink } from "../state";
import { boxVersion, isUpdating, newerVersion, offerChangesEngine, offerFailedAgain, offerHeld, offerWaits, updateStatus } from "../updates/update";
import { type Busy, beginUpdate, canAdd, lookAtWork, sshAddressOf } from "./flow";
import { type Running, underWay } from "./tries";

/** What this box knows of each machine it keeps up, by its key. */
export const KEEP_UP_FILE = path.join(RUN, "keep-up.json");

/** How often a round runs. */
export const KEEP_UP_ROUND = 15 * 60_000;
/** How long a machine found as new as this box isn't reached again for it. */
const CURRENT_FOR = 86_400_000;
/** After an update that failed, an hour before it's tried again; after more, a day. A stopped one
 * isn't begun again by itself for a day either. */
const RETRY = 3_600_000;
const REST = 86_400_000;

/** One machine, as this box last found it: whether it's older than this box (`mine`, this box's
 * version then), when it was looked at, since when it waits for nothing to be at work there, how
 * many conversations were at work (null: it couldn't be told) and how many looks in a row found
 * some, when it last couldn't be reached over SSH, how bringing it up last failed, and when one
 * was stopped. */
interface Kept {
  older: boolean;
  mine: string;
  checked: number;
  since?: number;
  working?: number | null;
  busyLooks?: number;
  unreached?: number;
  /** Why it couldn't be reached then, in words (what ssh said is in the log). */
  unreachedWhy?: string;
  failed?: { at: number; tries: number; why?: string };
  stopped?: number;
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
 * forgotten; failed, it rests (an hour, then a day) and its words go to the log; stopped, it isn't
 * begun again by itself for a day. Something else under way for it (Set Up Again, Uninstall) is
 * answered as busy. */
export function bringUp(key: string, auto: boolean, clock: () => number = Date.now): Running | Busy | null {
  const begun = beginUpdate(key, auto);
  if (!begun || "busy" in begun) return begun;
  if (begun.seen.kind !== "update") return { busy: "Something else is being done with it now. Try again once that’s done." };
  // (Asked on your word: whatever rested is tried now.)
  if (!auto) changeKept(key, was => (was ? { ...was, failed: undefined, stopped: undefined } : null));
  const started = begun.seen.id;
  void begun.over.then(() => {
    const seen = begun.seen;
    if (seen.id !== started) return;
    if (seen.state === "done") return changeKept(key, () => null);
    const now = clock();
    if (seen.state === "failed") {
      console.error(`Kuvo couldn't bring ${seen.name} up to date: ${seen.failure?.message ?? "it failed"}`);
      const why = seen.failure?.message?.slice(0, 400);
      return changeKept(key, was => ({ older: true, mine: boxVersion() ?? "", checked: now, ...(was ?? {}), since: undefined, failed: { at: now, tries: (was?.failed?.tries ?? 0) + 1, ...(why ? { why } : {}) } }));
    }
    // (Stopped, by you: not begun again by itself for a while.)
    changeKept(key, was => ({ older: true, mine: boxVersion() ?? "", checked: now, ...(was ?? {}), stopped: now }));
  });
  return begun;
}

let going = false;
/** Why the last round didn't run, as logged: said once each time it changes, never every round. */
let notRunning: string | null = null;

/** Logs why keeping machines up to date doesn't run now (`why`), or that it runs again (null). */
function noteRunning(why: string | null): void {
  if (why === notRunning) return;
  notRunning = why;
  console.error(why ? `Kuvo isn’t keeping your machines up to date now: ${why}` : "Kuvo keeps your machines up to date again.");
}

/** One round: each machine this box set up over SSH, that's reached now and whose Kuvo is older than
 * its own (by what it said of itself, or, where it says nothing, by looking over SSH), is brought up
 * to date, whatever is at work there (nothing is cut short). Never two rounds at once, never one under
 * way for that machine, never while one rests after failing or being stopped. */
export async function keepUpRound(clock: () => number = Date.now): Promise<void> {
  if (going) return;
  const can = canAdd();
  if (!can.can) return noteRunning(can.message);
  const mine = boxVersion();
  if (!mine) return noteRunning("its own version can’t be read (bin/box-version).");
  noteRunning(null);
  going = true;
  try {
    const link = readLink();
    const told = readPeerAgents();
    for (const peer of peersOf(link)) {
      const at = peer.kind === "machine" && !isApart(peer.key, link, told) ? sshAddressOf(peer.key) : null;
      if (!at || underWay(at.destination)) continue;
      const now = clock();
      // (Away: asleep, off or offline. Looked at again once it's back.)
      if (peerReach(peer.key, told, undefined, undefined, now).away) continue;
      const was = readKept()[peer.key];
      const said = told[peer.key]?.computer;
      // As new as this box, by its own word: nothing to do.
      if (said?.version && !newerVersion(mine, said.version)) {
        if (was) changeKept(peer.key, () => null);
        continue;
      }
      if (was?.failed && now - was.failed.at < (was.failed.tries > 1 ? REST : RETRY)) continue;
      if (was?.stopped && now - was.stopped < REST) continue;
      if (was && was.mine === mine && !was.older && now - was.checked < CURRENT_FOR) continue;
      let looked: Awaited<ReturnType<typeof lookAtWork>>;
      try {
        looked = await lookAtWork(peer.key);
      } catch (error) {
        // (Not reached over SSH: no failed update, and tried again at the next round.)
        console.error(`Kuvo couldn't reach ${at.name} over SSH to keep it up to date: ${error instanceof Error ? error.message : String(error)}`);
        const why = error instanceof Error ? error.message.slice(0, 400) : undefined;
        changeKept(peer.key, before => ({ older: true, mine, checked: now, ...(before ?? {}), unreached: now, unreachedWhy: why }));
        continue;
      }
      if (!looked) continue;
      if (looked.version && !newerVersion(mine, looked.version)) {
        changeKept(peer.key, () => ({ older: false, mine, checked: now }));
        continue;
      }
      // (What's at work there is left as it is: nothing it does is cut short.)
      changeKept(peer.key, before => ({ ...(before ?? {}), older: true, mine, checked: now, working: looked.working, since: undefined, busyLooks: undefined, unreached: undefined, unreachedWhy: undefined, failed: undefined }));
      bringUp(peer.key, true, clock);
    }
  } finally {
    going = false;
  }
}

/** Rounds from now on, the first 20 seconds in (an older machine is brought up soon after this box
 * is). (A box that set nothing up over SSH, as a test's, does nothing in them.) */
export function keepMachinesUp(): void {
  const round = () => { keepUpRound().catch(error => console.error(`Kuvo's round keeping machines up to date failed: ${String(error)}`)); };
  setTimeout(round, 20_000).unref?.();
  setInterval(round, KEEP_UP_ROUND).unref?.();
}

const sessions = (count: number): string => `${count} ${count === 1 ? "session is" : "sessions are"}`;

/** How bringing `computer` up to date stands (`update`: what's true now, its `title`, and the next
 * step where there's one to say, its `detail`), and the one action that moves it on (`actions`), on
 * the box that set it up over SSH: being brought up, about to be, not reached over SSH or failed (Try Again: it tries again by itself too). Where
 * this box can't bring it up, why, and what to do: set it up over SSH from here once (`add`), or
 * what keeps this box from it (`canAdd`'s words). Nothing for one as new as this box, one that's
 * away now, and one this box neither set up nor could. */
export function keepingUp(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "apart" | "reach" | "version" | "working">, here: string, now = Date.now()): { update?: ComputerUpdate; actions: ComputerAction[] } {
  const none = { actions: [] };
  if (computer.here) return keepingSelfUp(computer.name || here, computer.working);
  if (computer.apart || computer.kind !== "machine" || !computer.key) return none;
  const mine = boxVersion();
  if (!mine) return none;
  const name = computer.name || "this machine";
  const line = (state: ComputerUpdate["state"], title: string, detail = "", long = false): ComputerUpdate => ({ state, title, detail, ...(long ? { long: true as const } : {}) });
  const can = canAdd();
  const at = can.can ? sshAddressOf(computer.key) : null;
  // One older that this box can't bring up (nothing it set up over SSH, or something keeps it from
  // that now): said, with what to do. (Older by what it said; one that says nothing is from before
  // boxes said their version, so older, once it's heard from.)
  if (!at) {
    const older = computer.version ? newerVersion(mine, computer.version) : computer.reach === "connected";
    if (!older || computer.reach === "away") return none;
    // (Something keeps this box from it: the old app attached, no link yet, files missing. Its own
    // words say what to do; nothing to press here.)
    // (Only where it's this box's to clear, a Mac's that isn't another's member: a Linux box or a
    // member never brings machines up, so it says nothing, and a client draws what the box that
    // does says.)
    if (!can.can) return IS_MAC && !mainElsewhere(readLink()) ? { update: line("waiting", `Kuvo on ${name} needs an update`, can.message), actions: [] } : none;
    // One older that this box didn't set up over SSH (another Mac did, or its record didn't come
    // along): set up over SSH from here once, which Set Up Over SSH begins (a client opens Add a
    // Machine at its address, `id`, for you to confirm).
    return { update: line("waiting", `Kuvo on ${name} needs an update`, `${here} hasn’t set it up over SSH, so it can’t update it yet. Set it up over SSH once: what runs there stays.`), actions: [{ id: "add", label: "Set Up Over SSH…" }] };
  }
  const under = underWay(at.destination);
  if (under?.seen.kind === "update") return { update: line("updating", `Updating Kuvo on ${name}…`), actions: [] };
  const was = readKept()[computer.key];
  // (One heard from that never says its version is from before boxes said it, so older than this
  // one, unless a look over SSH found it as new; one not heard from yet waits for the look.)
  const looked = was?.mine === mine ? was.older : undefined;
  const older = computer.version ? newerVersion(mine, computer.version) : computer.reach === "connected" ? looked !== false : looked === true;
  if (!older || computer.reach === "away") return none;
  // (Nothing at work there stops, so there's nothing to wait for or ask first.)
  const action = (label: string): ComputerAction[] => [{ id: "update", label, ...(under ? { why: "Something else is being done with it now." } : {}) }];
  // (A failure or a miss says so, with Try Again: a new try replaces it. Its words where they're
  // more than "it couldn't be reached", which the title says.)
  const missed = (title: string, why: string | undefined) => {
    const own = why && !why.startsWith("Couldn’t reach ") ? sentence(why) : "Make sure it’s on and connected, then Try Again. ";
    return { update: line("failed", title, `${own}Kuvo also tries again by itself later.`), actions: action("Try Again") };
  };
  if (was?.failed) return missed(was.failed.why?.startsWith("Couldn’t reach ") ? `Couldn’t reach ${name}` : `Couldn’t update Kuvo on ${name}`, was.failed.why);
  if (was?.unreached && was.unreached >= was.checked) return missed(was.unreachedWhy?.includes("couldn’t be checked") ? `Couldn’t check ${name}` : `Couldn’t reach ${name}`, was.unreachedWhy);
  return { update: line("waiting", `Kuvo on ${name} updates in a moment`), actions: action("Update Now") };
}

/** Words ending as a sentence does, with a space after, to go before more; none for none. */
const sentence = (words: string): string => (words.trim() ? `${words.trim()}${/[.!?…]$/.test(words.trim()) ? "" : "."} ` : "");

/** How this box's own update stands, in one line (`update`), and Update Now (`update-here`, which
 * is `POST kuvo/update {now}`): the release Kuvo Code here offered, newer than this box, is taken
 * in a moment (`main.ts`), as it stops nothing; one that brings another engine, as soon as nothing's
 * at work, or now on your word, asked first (what the engine runs stops as it restarts). Nothing while none waits, or while this box can't
 * update (blocked, its engine stopped on purpose, an update on trial: `offerHeld`). */
export function keepingSelfUp(name: string, working: number | undefined, blocked?: string | null): { update?: ComputerUpdate; actions: ComputerAction[] } {
  const none = { actions: [] };
  if (!offerWaits()) return none;
  const status = updateStatus();
  if (blocked !== undefined) status.blocked = blocked;
  const line = (state: ComputerUpdate["state"], title: string): ComputerUpdate => ({ state, title, detail: "" });
  // (Looking for a release from GitHub isn't this one being taken.)
  if (isUpdating() && status.updating && status.updating !== "checking") return { update: line("updating", `Updating Kuvo on ${name}…`), actions: [] };
  if (status.blocked || offerHeld()) return none;
  // Only a release with another engine waits for nothing to be at work, and stops what the engine
  // runs when it's taken now: then Update Now asks first (what the take waits for, conversations
  // working and browser asks waiting, is more than the sessions counted at work). Any other stops
  // nothing, and is taken in a moment.
  const engine = offerChangesEngine();
  const at = working ?? 0;
  const confirm = { title: `Update Kuvo on ${name} now?`, message: at > 0 ? `${sessions(at)} at work on ${name}: what ${at === 1 ? "it’s" : "they’re"} doing stops as Kuvo restarts there.` : `Whatever is at work on ${name} stops as Kuvo restarts there.`, label: "Update Now" };
  const actions: ComputerAction[] = [{ id: "update-here", label: "Update Now", ...(engine ? { confirm } : {}) }];
  // (Failed once with nothing changed, it's tried again by itself in a while: said as a failure only
  // once that fails too, or once it was put in place and taken back.)
  if (status.offerFailed && status.offerFailed.version === status.offered) {
    if (offerFailedAgain(status.offered)) return { update: line("failed", `Kuvo on ${name} couldn’t be updated. It tries again later.`), actions };
    return { update: line("waiting", `Kuvo on ${name} tries its update again in a while.`), actions };
  }
  if (!engine) return { update: line("waiting", `Kuvo on ${name} updates in a moment.`), actions };
  return { update: line("waiting", at > 0 ? `Kuvo on ${name} updates once your sessions there finish.` : `Kuvo on ${name} updates as soon as nothing’s at work there.`), actions };
}
