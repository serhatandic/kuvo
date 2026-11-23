import path from "node:path";
import { textOf, writePrivate } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import type { ToldComputer } from "../../shared/records";
import type { FleetLine, FleetSummary, FleetUpdate, ListedComputer } from "../../shared/routes/box";
import { RUN } from "../home";
import type { RouteContext, AnswerPiece } from "../server/context";
import { askForRetry, claimRetry, requestedRetry } from "./attempt";
import { cannotFor, CANNOT_FILE, READY_FILE, retryCannot, sameRelease } from "./facts";
import { askPeerToRetry, fleetFacts, fleetTarget, HELD, pullFill, retryTransfer, type FleetTarget } from "./fleet";
import { readPeerAgents } from "../peers/directory";
import { KEY, ownKey, peersOf } from "../peers/known";
import { peerIsAway, readReach } from "../peers/reach";
import { readLink } from "../state";
import { boxVersion, newerVersion, switchPending, updateOnTrial } from "./update";

/** The setup's last update this box saw in progress, `{target, finishedAt?}`: so it says the update
 * is done for a while after it finished (`fleetSummary`), and only where it saw it under way. */
export const FINISHED_FILE = path.join(RUN, "fleet-finished.json");
/** How long the setup's update reads as done (shown) once it finished. */
export const SETTLED_SHOWN = 10 * 60_000;

export const fleetFiles = () => [CANNOT_FILE, READY_FILE, path.join(RUN, "previous"), path.join(RUN, "update-cannot"), path.join(HELD, "boxes.json"), FINISHED_FILE];

// (A release's short name: what follows its first "-", as "beta.41" of "0.8.0-beta.41".)
const shortName = (version: string) => version.includes("-") ? version.slice(version.indexOf("-") + 1) : version;
const withVersion = (version: string | null, words: string) => version ? `${shortName(version)} · ${words}` : words;

/** When a computer was last heard from, in words ("seen 2h ago"). */
export function seenWords(seen: number, now = Date.now()): string {
  const minutes = Math.floor(Math.max(0, now - seen) / 60_000);
  if (minutes < 1) return "seen just now";
  if (minutes < 60) return `seen ${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `seen ${hours}h ago`;
  const days = Math.floor(hours / 24);
  return days < 2 ? "seen yesterday" : `seen ${days} days ago`;
}

const clamped = (fill: number) => Math.min(1, Math.max(0, fill));

/** What a computer's row shows of its update, in words: the box's, so every client says the same.
 * `phase` only ever goes forward, getting ready, ready, switching, done (up to date, and only then);
 * `tone` stays as older clients read it. */
export function fleetLine(update: FleetUpdate, now = Date.now()): FleetLine {
  switch (update.state) {
    case "up-to-date": return { words: `${shortName(update.current ?? update.target)} · Up to date`, tone: "done", phase: "done" };
    case "ready": return { words: `Ready for ${shortName(update.target)}`, tone: "done", phase: "ready" };
    case "off": {
      if (update.seen === undefined) return { words: withVersion(update.current, "Can’t be reached: updates when it’s back"), tone: "off", phase: "off" };
      const seen = seenWords(update.seen, now);
      return { words: update.current ? withVersion(update.current, seen) : seen[0]!.toUpperCase() + seen.slice(1), tone: "off", phase: "off" };
    }
    // (Restarting into the release: past ready, never back to getting it ready.)
    case "updating": if (update.stage === "switching") return { words: `Switching to ${shortName(update.target)}…`, tone: "working", phase: "switching" }; break;
    case "cannot": return update.fixable
      ? { words: withVersion(update.current, update.reason), tone: "warning", short: "Can’t update" }
      : { words: `${withVersion(update.current, update.reason)} · Updates with the next release`, tone: "blocked" };
    case "failed": return { words: withVersion(update.current, update.reason), tone: "warning", short: "Can’t update" };
  }
  const fill = update.state === "getting-ready" && update.fill !== undefined ? clamped(update.fill) : undefined;
  return fill !== undefined
    ? { words: fill < 1 ? `Getting ready · ${Math.floor(fill * 100)}%` : "Getting ready…", tone: "working", phase: "getting-ready", fill }
    : { words: "Getting ready…", tone: "working", phase: "getting-ready" };
}

const lined = (update: FleetUpdate, now = Date.now()): FleetUpdate => ({ ...update, line: fleetLine(update, now) });

/** How long a computer that went quiet keeps the state it last told (a box restarting, a relay
 * reconnecting) before it reads as away: a few seconds' gap never flips a row. */
export const QUIET_GRACE = 2 * 60_000;
/** How long one that went quiet while ready for (or switching to) the target reads as switching
 * to it, before it reads as off: as long as a slow computer takes to restart into a release. */
export const SWITCH_GRACE = 10 * 60_000;

/** A computer's progress towards `target`, from what it last told (`facts`). One that's `away`,
 * for `awayFor` ms (unknown: long), keeps the state it last told for `QUIET_GRACE`; one that was
 * ready for the target, or said it was switching to it, reads as switching (`updating`,
 * `switching`) for `SWITCH_GRACE`; only after that is it `off`, with when it was last heard from
 * (`heard`, as `seen`). A box on trial of the target says it's switching itself
 * (`localFleetUpdate`), so a row never steps back to getting ready while the computer restarts
 * into the release (PROTOCOL.md, "Updates"). */
export function peerFleetUpdate(target: FleetTarget, facts: ToldComputer | undefined, away = false, awayFor = Infinity, heard?: number, now = Date.now()): FleetUpdate {
  const progress = peerProgress(target, facts, away, awayFor);
  return lined(progress.state === "off" && typeof heard === "number" && Number.isFinite(heard) ? { ...progress, seen: heard } : progress, now);
}

function peerProgress(target: FleetTarget, facts: ToldComputer | undefined, away: boolean, awayFor: number): FleetUpdate {
  const current = facts?.kuvo ?? null;
  const { version, sha256 } = target;
  const progress = { current, target: version, targetSha256: sha256, currentSha256: facts?.installedSha256, canRetry: false };
  const upToDate = current && !newerVersion(version, current);
  const ref = sha256 ? { version, sha256 } : undefined;
  const told = facts?.fleetUpdate;
  const saidSwitching = told?.state === "updating" && told.stage === "switching" && told.target === version;
  const wasReady = sameRelease(facts?.staged, ref) || saidSwitching;
  const switching: FleetUpdate = { ...progress, state: "updating", stage: "switching" };
  if (away && (!facts || !(awayFor < QUIET_GRACE))) return !upToDate && wasReady && awayFor < SWITCH_GRACE ? switching : { ...progress, state: "off" };
  if (upToDate) return { ...progress, state: "up-to-date" };
  if ((away && wasReady) || saidSwitching) return switching;
  const cannot = facts?.cannot;
  if (cannot && sameRelease(cannot, ref))
    return { ...progress, state: "cannot", reason: cannot.reason, fixable: cannot.fixable, canRetry: cannot.fixable };
  if (sameRelease(facts?.staged, ref)) return { ...progress, state: "ready" };
  // (How far it said it is in getting this release ready.)
  const fill = told?.state === "getting-ready" && told.target === version && typeof told.fill === "number" && Number.isFinite(told.fill) ? clamped(told.fill) : undefined;
  return { ...progress, state: "getting-ready", ...(fill !== undefined ? { fill } : {}) };
}

/** This box's own progress. While it switches into the target (its new release on trial, or the
 * keeper still to confirm it), it says so: its version reads as the one before until the switch is
 * confirmed, and its ready marker is spent, which would otherwise read as getting ready again.
 * Getting it ready, it says how far it is (`fill`), where it knows (`pullFill`). */
export function localFleetUpdate(target = fleetTarget(), facts = fleetFacts(), switchingNow = (switchPending() || updateOnTrial()) && boxVersion() === target?.version, fill = target ? pullFill(target.version) : undefined): FleetUpdate | undefined {
  if (!target) return;
  let result = switchingNow && facts.kuvo !== target.version
    ? peerFleetUpdate(target, { ...facts, fleetUpdate: { current: facts.kuvo ?? null, target: target.version, canRetry: false, state: "updating", stage: "switching" } })
    : peerFleetUpdate(target, facts);
  if (result.state === "getting-ready" && fill !== undefined && Number.isFinite(fill)) result = lined({ ...result, fill: clamped(fill) });
  return result.canRetry && requestedRetry()?.sha256 === result.targetSha256 ? { ...result, canRetry: false } : result;
}

const shortVersion = (version: string) => version.replace(/^.*-(beta\.\d+)$/, "$1");

type Phase = NonNullable<FleetSummary["phase"]>;

/** Whether the settled update (`version`) still shows: for `SETTLED_SHOWN` after this box first
 * found it finished, and only where it saw it in progress (`FINISHED_FILE`, written as that changes). */
function settledShows(version: string, inProgress: boolean, settled: boolean, now: number): boolean {
  const kept = parsed(textOf(FINISHED_FILE));
  const was = isObject(kept) && typeof kept.target === "string" ? { target: kept.target, finishedAt: typeof kept.finishedAt === "number" ? kept.finishedAt : undefined } : undefined;
  const keep = (value: { target: string; finishedAt?: number }) => { try { writePrivate(FINISHED_FILE, JSON.stringify(value)); } catch {} };
  if (inProgress) {
    if (was?.target !== version || was.finishedAt !== undefined) keep({ target: version });
    return false;
  }
  if (!settled || was?.target !== version) return false;
  const finishedAt = was.finishedAt ?? now;
  if (was.finishedAt === undefined) keep({ target: version, finishedAt });
  return now - finishedAt < SETTLED_SHOWN;
}

/** The setup's update in words (PROTOCOL.md, "Updates"), by where it is (`phase`): computers getting
 * it ready, all ready, switching, done but for computers that are off (`waiting`), or done. Shown
 * while it's in progress, or where one can't update for a reason you can fix (`warning`, its name and
 * reason, the one thing to do being Try Again on its row); and, done, for `SETTLED_SHOWN` after it
 * finished, on a box that saw it in progress. */
export function fleetSummary(rows: readonly Pick<ListedComputer, "fleetUpdate" | "name">[], target: FleetTarget | undefined, now = Date.now()): FleetSummary | undefined {
  if (!target) return;
  let done = 0, updating = 0, ready = 0, behind = 0, failed = 0, switching = 0;
  const fixable: { name: string; reason: string }[] = [];
  const away: string[] = [];
  // (What the ones in progress run now, and the ones not yet switched.)
  const running = new Set<string>(), still = new Set<string>();
  for (const row of rows) {
    const progress = row.fleetUpdate;
    if (progress && progress.state !== "up-to-date") still.add(progress.current ?? "");
    switch (progress?.state) {
      case "up-to-date": done++; break;
      case "ready": ready++; running.add(progress.current ?? ""); break;
      case "getting-ready": updating++; running.add(progress.current ?? ""); break;
      // (Switching: past ready, and the notifier stays up until it's done.)
      case "updating": switching++; running.add(progress.current ?? ""); break;
      case "off": behind++; away.push(row.name); break;
      case "cannot":
        failed++;
        if (progress.fixable) fixable.push({ name: row.name, reason: progress.reason });
        break;
    }
  }
  const version = shortVersion(target.version);
  const total = done + ready + switching + updating + behind + failed;
  const warning = fixable.length > 0;
  const phase: Phase | undefined = !done && !switching && updating ? "getting-ready"
    : !done && !switching && ready ? "ready"
    : switching || (done && (ready || updating)) ? "switching"
    : done && done === total ? "done"
    : done ? "waiting"
    : undefined;
  const inProgress = phase === "getting-ready" || phase === "ready" || phase === "switching";
  const settled = !warning && (phase === "waiting" || phase === "done");
  const shown = settledShows(target.version, warning || inProgress, settled, now) || warning || inProgress;
  const one = (versions: Set<string>) => { const [only] = versions; return versions.size === 1 && only ? shortVersion(only) : undefined; };
  const [title, detail] = warning
    ? [fixable.length === 1 ? `${fixable[0]!.name} can’t update to ${version}` : `${fixable.length} computers can’t update to ${version}`, `${fixable[0]!.name}: ${fixable[0]!.reason}`]
    : phase === "getting-ready" ? [ready ? `Getting ${version} ready · ${ready} of ${total}` : `Getting ${version} ready`,
      one(running) ? `Your computers stay on ${one(running)} meanwhile.` : "Your computers keep what they run meanwhile."]
    : phase === "ready" ? [`${version} is ready · ${ready} of ${total}`, "They switch next."]
    : phase === "switching" ? [`Switching to ${version} · ${done} of ${total} done`,
      one(still) ? `${total - done} still on ${one(still)}.` : `${total - done} still to switch.`]
    : phase === "done" ? [total === 1 ? `Up to date on ${version}` : `All ${total} on ${version}`, undefined]
    : [`${version} · ${done} of ${total} up to date`,
      away.length === 1 ? `${away[0]} updates when it’s back.` : away.length > 1 ? `${away.length} computers update when they’re back.` : undefined];
  return {
    target: target.version, targetSha256: target.sha256, done: done + ready + switching, updating, behind, failed,
    stage: warning ? "failed" : switching ? "switching" : updating ? "downloading" : ready ? "waiting" : "finished",
    shown, title, detail: shown ? detail : undefined, warning, dismissed: false, failures: [],
    ...(phase ? { phase } : {}),
  };
}

export function requestFleetRetry(version: string, sha256?: string): { status: 202; update: FleetUpdate } | { status: 409; message: string } {
  const target = fleetTarget();
  if (!target?.sha256 || version !== target.version || (sha256 && sha256 !== target.sha256))
    return { status: 409, message: "The setup’s target version changed." };
  const status = localFleetUpdate(target);
  if (!status?.canRetry) return { status: 409, message: "This computer has no update to retry." };
  try {
    if (!askForRetry({ version, sha256: target.sha256 })) return { status: 409, message: "A retry is already scheduled." };
  } catch (error) {
    return { status: 409, message: `Couldn’t schedule the retry: ${(error as Error).message}` };
  }
  return { status: 202, update: { ...status, canRetry: false } };
}

export function consumeFleetRetry(): void {
  const retry = claimRetry();
  if (!retry) return;
  const selected = fleetTarget();
  if (selected?.sha256 !== retry.sha256 || selected.version !== retry.version || !cannotFor(retry)?.fixable) return;
  retryCannot(retry);
  retryTransfer(retry.version);
}

/** Try Again for another of your computers, asked of this box: passed on to it, where its row
 * offers it (`canRetry`) and it can be reached; answered with how its row stands now. */
export function requestPeerRetry(key: string, version: string, sha256?: string, now = Date.now()): { status: 202; update: FleetUpdate } | { status: 404 | 409 | 503; message: string } {
  const link = readLink();
  const peer = peersOf(link).find(each => each.key === key);
  if (!peer) return { status: 404, message: "That isn’t one of your computers." };
  const target = fleetTarget();
  if (!target?.sha256 || version !== target.version || (sha256 && sha256 !== target.sha256))
    return { status: 409, message: "The setup’s target version changed." };
  const told = readPeerAgents()[key];
  const update = peerFleetUpdate(target, told?.computer, peerIsAway(key, told?.heard, readReach(), now), undefined, told?.heard, now);
  const name = peer.name || "That computer";
  if (!update.canRetry) return { status: 409, message: `${name} has no update to retry.` };
  if (!askPeerToRetry(key, { version, sha256: target.sha256 })) return { status: 503, message: `${name} can’t be reached right now. Try again once it’s connected.` };
  return { status: 202, update: { ...update, canRetry: false } };
}

export function* retryFleetRoute({ request, json }: RouteContext): Generator<AnswerPiece> {
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isObject(given) || typeof given.target !== "string" || (given.targetSha256 !== undefined && typeof given.targetSha256 !== "string")
    || (given.key !== undefined && (typeof given.key !== "string" || !KEY.test(given.key))))
    return yield* json(400, { message: "Send the setup’s target version." });
  // (This computer's own, or another's, passed on to it.)
  const link = readLink();
  const elsewhere = given.key !== undefined && (!link || given.key !== ownKey(link));
  const reply = elsewhere ? requestPeerRetry(given.key as string, given.target, given.targetSha256) : requestFleetRetry(given.target, given.targetSha256);
  return yield* json(reply.status, reply.status === 202 ? reply.update : { message: reply.message });
}
