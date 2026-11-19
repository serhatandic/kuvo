// Your machines kept up to date (PROTOCOL.md, "Computers", Keeping them up to date;
// `computers.update`). Every computer of yours follows the newest signed release any of them holds,
// by itself, at once (`updates/fleet.ts`); this is the retry: the box that set a machine up over SSH
// brings its Kuvo up to its own when the machine doesn't follow its peers (a box from before that,
// which never says `kuvo`), or is still behind an hour after it could have. By itself, with no
// client open, whatever is at work there, as nothing is cut short (`flow.ts`'s Set Up Again leaves
// its running engine alone, and its coding agents' bridge makes way when nothing runs). Update Now
// (Try Again) is offered only where a computer can't take an update by itself: it failed twice, or
// couldn't be reached for days. Every word a client shows is here, one line each; what went wrong
// in detail goes to this box's log.
// The primary can bring its members up to date over SSH on either platform.

import fs from "node:fs";
import path from "node:path";
import type { ComputerAction, ComputerUpdate, ListedComputer } from "../../shared/routes/box";
import { isObject } from "../../shared/json";
import { swapLine, swapUnderWay } from "../../shared/engine-swap";
import { peerReach } from "../areas/computers";
import { writePrivate } from "../files";
import { IS_MAC, RUN } from "../home";
import { readPeerAgents } from "../peers/directory";
import { peersOf } from "../peers/known";
import { mainElsewhere } from "../setup/main-only";
import { readLink } from "../state";
import { boxVersion, isUpdating, newerVersion, offerChangesEngine, offerFailedTwice, offerHeld, offerWaits, offeredSince, updateStatus } from "../updates/update";
import { type Busy, beginUpdate, canAdd, lookAtWork, sshAddressOf } from "./flow";
import { type Running, underWay } from "./tries";
import { releaseSetup } from "../updates/choice";

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
/** How long a machine that follows its peers is left to take a release by itself before this box
 * brings it up over SSH. */
const FOLLOW_GRACE = 3_600_000;
/** How long a release that restarts the engine waits for nothing to be at work before Update Now. */
const ENGINE_WAIT_AT_MOST = 86_400_000;
/** How long another computer that follows is said to update in a moment before it's said that it
 * hasn't yet. */
const BEHIND_AT_MOST = 86_400_000;
/** Since when each other computer that follows has been seen behind this box's version, by key. */
const behindSeen = new Map<string, { mine: string; since: number }>();
/** How long a machine can't be reached over SSH, behind, before Try Again is offered. */
export const UNREACHED_FOR = 3 * 86_400_000;

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
  /** Since when it couldn't be reached, without a look that reached it since. */
  unreachedSince?: number;
  /** Since when this box has seen it behind while it follows its peers. */
  behind?: number;
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
      // (One you asked for that fails is said with Try Again at once: you saw it, and pressed it.)
      return changeKept(key, was => ({ older: true, mine: boxVersion() ?? "", checked: now, ...(was ?? {}), since: undefined, failed: { at: now, tries: Math.max((was?.failed?.tries ?? 0) + 1, auto ? 1 : 2), ...(why ? { why } : {}) } }));
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
    if (releaseSetup()) return;
    const told = readPeerAgents();
    for (const peer of peersOf(link)) {
      const at = peer.kind === "machine" ? sshAddressOf(peer.key) : null;
      if (!at || underWay(at.destination)) continue;
      const now = clock();
      // (Away: asleep, off or offline. Looked at again once it's back.)
      if (peerReach(peer.key, told, undefined, undefined, now).away) continue;
      const was = readKept()[peer.key];
      const said = told[peer.key]?.computer;
      // (Its Kuvo, by its own word: `kuvo` from a box that follows its peers, else its `version`.)
      const theirs = said?.kuvo ?? said?.version;
      // As new as this box, by its own word: nothing to do.
      if (theirs && !newerVersion(mine, theirs)) {
        if (was) changeKept(peer.key, () => null);
        continue;
      }
      // One that follows its peers takes it by itself, at once: brought up over SSH only once it's
      // still behind an hour on.
      if (said?.kuvo) {
        const behind: number = was?.mine === mine && was.behind ? was.behind : now;
        if (was?.behind !== behind || was.mine !== mine) changeKept(peer.key, before => ({ older: true, checked: now, ...(before ?? {}), mine, behind }));
        if (now - behind < FOLLOW_GRACE) continue;
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
        changeKept(peer.key, before => ({ older: true, mine, checked: now, ...(before ?? {}), unreached: now, unreachedSince: before?.unreachedSince ?? now, unreachedWhy: why }));
        continue;
      }
      if (!looked) continue;
      if (looked.version && !newerVersion(mine, looked.version)) {
        changeKept(peer.key, () => ({ older: false, mine, checked: now }));
        continue;
      }
      // (What's at work there is left as it is: nothing it does is cut short.)
      changeKept(peer.key, before => ({ ...(before ?? {}), older: true, mine, checked: now, working: looked.working, since: undefined, busyLooks: undefined, unreached: undefined, unreachedSince: undefined, unreachedWhy: undefined, failed: undefined }));
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
 * step where there's one to say, its `detail`), and the one action that moves it on (`actions`).
 * A computer that's behind takes the newer release by itself (it follows its peers, or this box
 * brings it up over SSH), so mostly there's a line and nothing to press: Try Again only once it
 * failed twice, or this box couldn't reach it over SSH for days. Where this box can't bring an older
 * machine up, why, and what to do: set it up over SSH from here once (`add`), or what keeps this box
 * from it (`canAdd`'s words). Nothing for one as new as this box, one that's away now, and an older
 * Mac from before computers followed each other (its own app updates it). */
export function keepingUp(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "reach" | "version" | "working"> & { kuvo?: string }, here: string, now = Date.now()): { update?: ComputerUpdate; actions: ComputerAction[] } {
  const none = { actions: [] };
  if (computer.here) return keepingSelfUp(computer.name || here, computer.working);
  const mine = boxVersion();
  if (!mine || !computer.key || computer.reach === "away") return none;
  const name = computer.name || (computer.kind === "mac" ? "this Mac" : "this machine");
  const line = (state: ComputerUpdate["state"], title: string, detail = "", long = false): ComputerUpdate => ({ state, title, detail, ...(long ? { long: true as const } : {}) });
  // One that follows its peers (it says which Kuvo it runs): behind, it takes the release by itself.
  // Still behind a day on, that's said: it hasn't, and says why on its own page.
  const follows = !!computer.kuvo;
  if (follows && !newerVersion(mine, computer.kuvo)) {
    behindSeen.delete(computer.key);
    return none;
  }
  const seen = behindSeen.get(computer.key);
  const since = seen?.mine === mine ? seen.since : now;
  if (follows && seen?.mine !== mine) behindSeen.set(computer.key, { mine, since });
  const soon = { update: follows && now - since >= BEHIND_AT_MOST ? line("waiting", `Kuvo on ${name} hasn’t updated yet`, `It updates by itself once it can.`, true) : line("waiting", `Kuvo on ${name} updates in a moment`), actions: [] };
  if (computer.kind !== "machine") return follows ? soon : none;
  const can = canAdd();
  const at = can.can ? sshAddressOf(computer.key) : null;
  // One older that this box can't bring up (nothing it set up over SSH, or something keeps it from
  // that now): said, with what to do. (Older by what it said; one that says nothing is from before
  // boxes said their version, so older, once it's heard from.)
  if (!at) {
    if (follows) return soon;
    const older = computer.version ? newerVersion(mine, computer.version) : computer.reach === "connected";
    if (!older) return none;
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
  const older = follows || (computer.version ? newerVersion(mine, computer.version) : computer.reach === "connected" ? looked !== false : looked === true);
  if (!older) return none;
  // (Nothing at work there stops, so there's nothing to wait for or ask first.)
  const action = (label: string): ComputerAction[] => [{ id: "update", label, ...(under ? { why: "Something else is being done with it now." } : {}) }];
  // (A failure or a miss it can't get past by itself says so, with Try Again: a new try replaces it.
  // Its words where they're more than "it couldn't be reached", which the title says.)
  const missed = (title: string, why: string | undefined) => {
    const own = why && !why.startsWith("Couldn’t reach ") ? sentence(why) : "Make sure it’s on and connected, then Try Again. ";
    return { update: line("failed", title, `${own}Kuvo also tries again by itself later.`), actions: action("Try Again") };
  };
  if (was?.failed) {
    // (Once: it tries again by itself in a while, as nothing needs you yet.)
    if (was.failed.tries < 2) return { update: line("waiting", `Kuvo on ${name} tries its update again in a while`), actions: [] };
    return missed(was.failed.why?.startsWith("Couldn’t reach ") ? `Couldn’t reach ${name}` : `Couldn’t update Kuvo on ${name}`, was.failed.why);
  }
  if (was?.unreached && was.unreached >= was.checked && !follows) {
    // (Not reached over SSH lately: it goes on trying by itself; only after days is that yours to
    // look at.)
    if (now - (was.unreachedSince ?? was.unreached) < UNREACHED_FOR) return { update: line("waiting", `Kuvo on ${name} updates once ${here} can reach it`), actions: [] };
    return missed(was.unreachedWhy?.includes("couldn’t be checked") ? `Couldn’t check ${name}` : `Couldn’t reach ${name}`, was.unreachedWhy);
  }
  return soon;
}

/** Words ending as a sentence does, with a space after, to go before more; none for none. */
const sentence = (words: string): string => (words.trim() ? `${words.trim()}${/[.!?…]$/.test(words.trim()) ? "" : "."} ` : "");

/** How this box's own update stands, in one line (`update`): the release Kuvo Code here offered or
 * another of your computers handed it, newer than this box, is taken in a moment (`main.ts`), as it
 * stops nothing; one that brings an engine its code wasn't checked with, as soon as nothing's at
 * work. Update Now (`update-here`, which is `POST kuvo/update {now}`) only once that release failed
 * here twice: what it can't get past by itself. Nothing while none waits, or while this box can't
 * update (blocked, its engine stopped on purpose, an update on trial: `offerHeld`). */
export function keepingSelfUp(name: string, working: number | undefined, blocked?: string | null): { update?: ComputerUpdate; actions: ComputerAction[] } {
  const none = { actions: [] };
  // A newer engine taking the running one's place (PROTOCOL.md, "A new engine"): said in its line.
  const swap = swapUnderWay(RUN);
  const status = updateStatus();
  if (blocked !== undefined) status.blocked = blocked;
  const line = (state: ComputerUpdate["state"], title: string): ComputerUpdate => ({ state, title, detail: "" });
  // A newer version that failed here twice (put back after its trial, or failed before anything
  // changed), offered or not: it isn't taken by itself again until a newer one comes, so said, with
  // Update Now (which takes the one held here, or offered).
  const failed = status.offerFailed;
  if (failed && newerVersion(failed.version, status.version) && offerFailedTwice(failed.version) && !status.blocked && !offerHeld() && !isUpdating()) {
    const at = working ?? 0;
    // (Asked first where taking it restarts the engine: what's at work stops.)
    const confirm = { title: `Update Kuvo on ${name} now?`, message: at > 0 ? `${sessions(at)} at work on ${name}: what ${at === 1 ? "it’s" : "they’re"} doing stops as Kuvo restarts there.` : `Whatever is at work on ${name} stops as Kuvo restarts there.`, label: "Update Now" };
    return { update: line("failed", `Kuvo on ${name} couldn’t be updated to ${failed.version} after 2 tries.`), actions: [{ id: "update-here", label: "Update Now", ...(offerWaits() && offerChangesEngine() ? { confirm } : {}) }] };
  }
  if (!offerWaits()) return swap ? { update: { state: swap.stage === "waiting" ? "waiting" : "updating", title: swapLine(swap)!, detail: "" }, actions: [] } : none;
  // (Looking for a release from GitHub isn't this one being taken.)
  if (isUpdating() && status.updating && status.updating !== "checking") return { update: line("updating", `Updating Kuvo on ${name}…`), actions: [] };
  if (status.blocked || offerHeld()) return none;
  // Only a release with another engine its code wasn't checked with waits for nothing to be at work,
  // and stops what the engine runs when it's taken now: then Update Now asks first (what the take
  // waits for, conversations working and browser asks waiting, is more than the sessions counted).
  const engine = offerChangesEngine();
  const at = working ?? 0;
  // (Failed once, it's tried again by itself in a while; twice is said above.)
  if (status.offerFailed && status.offerFailed.version === status.offered) return { update: line("waiting", `Kuvo on ${name} tries its update again in a while.`), actions: [] };
  if (!engine) return { update: line("waiting", `Kuvo on ${name} updates in a moment.`), actions: [] };
  const waiting = line("waiting", at > 0 ? `Kuvo on ${name} updates once your sessions there finish.` : `Kuvo on ${name} updates as soon as nothing’s at work there.`);
  // (Nothing free there for a day, a bot always at work say: it can't get past that by itself, so
  // Update Now, asked first.)
  if (Date.now() - offeredSince() < ENGINE_WAIT_AT_MOST) return { update: waiting, actions: [] };
  const confirm = { title: `Update Kuvo on ${name} now?`, message: at > 0 ? `${sessions(at)} at work on ${name}: what ${at === 1 ? "it’s" : "they’re"} doing stops as Kuvo restarts there.` : `Whatever is at work on ${name} stops as Kuvo restarts there.`, label: "Update Now" };
  return { update: { ...waiting, long: true }, actions: [{ id: "update-here", label: "Update Now", confirm }] };
}
