import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import type { ToldComputer } from "../../shared/records";
import type { FleetSummary, FleetUpdate, ListedComputer } from "../../shared/routes/box";
import { RUN } from "../home";
import type { RouteContext, AnswerPiece } from "../server/context";
import { askForRetry, claimRetry, requestedRetry } from "./attempt";
import { cannotFor, CANNOT_FILE, READY_FILE, retryCannot, sameRelease } from "./facts";
import { fleetFacts, fleetTarget, HELD, retryTransfer, type FleetTarget } from "./fleet";
import { boxVersion, newerVersion, switchPending, updateOnTrial } from "./update";

export const fleetFiles = () => [CANNOT_FILE, READY_FILE, path.join(RUN, "previous"), path.join(RUN, "update-cannot"), path.join(HELD, "boxes.json")];

/** How long a computer that went quiet keeps the state it last told (a box restarting, a relay
 * reconnecting) before it reads as away: a few seconds' gap never flips a row. */
export const QUIET_GRACE = 2 * 60_000;
/** How long one that went quiet while ready for (or switching to) the target reads as switching
 * to it, before it reads as off: as long as a slow computer takes to restart into a release. */
export const SWITCH_GRACE = 10 * 60_000;

/** A computer's progress towards `target`, from what it last told (`facts`). One that's `away`,
 * for `awayFor` ms (unknown: long), keeps the state it last told for `QUIET_GRACE`; one that was
 * ready for the target, or said it was switching to it, reads as switching (`updating`,
 * `switching`) for `SWITCH_GRACE`; only after that is it `off`. A box on trial of the target says
 * it's switching itself (`localFleetUpdate`), so a row never steps back to getting ready while
 * the computer restarts into the release (PROTOCOL.md, "Updates"). */
export function peerFleetUpdate(target: FleetTarget, facts: ToldComputer | undefined, away = false, awayFor = Infinity): FleetUpdate {
  const current = facts?.kuvo ?? null;
  const { version, sha256 } = target;
  const progress = { current, target: version, targetSha256: sha256, currentSha256: facts?.installedSha256, canRetry: false };
  const upToDate = current && !newerVersion(version, current);
  const ref = sha256 ? { version, sha256 } : undefined;
  const told = facts?.fleetUpdate;
  const saidSwitching = told?.state === "updating" && told.stage === "switching" && told.target === version;
  const wasReady = sameRelease(facts?.staged, ref) || saidSwitching;
  const switching = (): FleetUpdate => ({ ...progress, state: "updating", stage: "switching", title: lineTitle("switching", progress) });
  if (away && !(awayFor < QUIET_GRACE)) {
    if (!upToDate && wasReady && awayFor < SWITCH_GRACE) return switching();
    return { ...progress, state: "off", title: lineTitle("off", progress) };
  }
  if (upToDate) return { ...progress, state: "up-to-date", title: lineTitle("up-to-date", progress) };
  if (away && wasReady) return switching();
  if (saidSwitching) return switching();
  const cannot = facts?.cannot;
  if (cannot && sameRelease(cannot, ref))
    return { ...progress, state: "cannot", reason: cannot.reason, fixable: cannot.fixable, canRetry: cannot.fixable, title: lineTitle("cannot", progress, cannot) };
  if (sameRelease(facts?.staged, ref)) return { ...progress, state: "ready", title: lineTitle("ready", progress) };
  return { ...progress, state: "getting-ready", title: lineTitle("getting-ready", progress) };
}

/** This box's own progress. While it switches into the target (its new release on trial, or the
 * keeper still to confirm it), it says so: its version reads as the one before until the switch is
 * confirmed, and its ready marker is spent, which would otherwise read as getting ready again. */
export function localFleetUpdate(target = fleetTarget(), facts = fleetFacts(), switchingNow = (switchPending() || updateOnTrial()) && boxVersion() === target?.version): FleetUpdate | undefined {
  if (!target) return;
  const result = switchingNow && facts.kuvo !== target.version
    ? peerFleetUpdate(target, { ...facts, fleetUpdate: { current: facts.kuvo ?? null, target: target.version, canRetry: false, state: "updating", stage: "switching" } })
    : peerFleetUpdate(target, facts);
  return result.canRetry && requestedRetry()?.sha256 === result.targetSha256 ? { ...result, canRetry: false } : result;
}

/** A computer's update in words, one line under its name (`title`): the box's words, drawn as
 * they are. */
export function lineTitle(state: "up-to-date" | "ready" | "getting-ready" | "switching" | "off" | "cannot",
  progress: { current: string | null; target: string }, cannot?: { reason: string; fixable: boolean }): string {
  const target = shortVersion(progress.target);
  const was = progress.current ? `${shortVersion(progress.current)} · ` : "";
  switch (state) {
    case "up-to-date": return `${shortVersion(progress.current ?? progress.target)} · Up to date`;
    case "ready": return `Ready for ${target}`;
    case "getting-ready": return `Getting ${target} ready…`;
    case "switching": return `Switching to ${target}…`;
    case "off": return `${was}Can’t be reached: updates when it’s back`;
    case "cannot": return cannot!.fixable ? `${was}${cannot!.reason}` : `${was}${cannot!.reason} · Updates with the next release`;
  }
}

const shortVersion = (version: string) => version.replace(/^.*-(beta\.\d+)$/, "$1");

/** The setup's update in words (PROTOCOL.md, "Updates"): shown only while computers get ready, or
 * where one can't update for a reason you can fix (`warning`, its name and reason, the one thing to
 * do being Try Again on its row). Off computers and ones that can't be fixed from here hide it. */
export function fleetSummary(rows: readonly Pick<ListedComputer, "fleetUpdate" | "name">[], target: FleetTarget | undefined): FleetSummary | undefined {
  if (!target) return;
  let done = 0, updating = 0, ready = 0, behind = 0, failed = 0, switching = 0;
  const fixable: { name: string; reason: string }[] = [];
  // (What the ones still to switch run now, for the line under the title.)
  const running = new Set<string>();
  for (const row of rows) {
    const progress = row.fleetUpdate;
    switch (progress?.state) {
      case "up-to-date": done++; break;
      case "ready": ready++; running.add(progress.current ?? ""); break;
      case "getting-ready": updating++; running.add(progress.current ?? ""); break;
      // (Switching: past ready, so counted with them, and the notifier stays up until it's done.)
      case "updating": switching++; running.add(progress.current ?? ""); break;
      case "off": behind++; break;
      case "cannot":
        failed++;
        if (progress.fixable) fixable.push({ name: row.name, reason: progress.reason });
        break;
    }
  }
  const version = shortVersion(target.version);
  const total = done + ready + switching + updating + failed;
  const warning = fixable.length > 0;
  const shown = warning || updating > 0 || ready > 0 || switching > 0;
  const title = !warning ? `Updating to ${version} · ${done + ready + switching} of ${total} ready`
    : fixable.length === 1 ? `${fixable[0].name} can’t update to ${version}` : `${fixable.length} computers can’t update to ${version}`;
  const [only] = running;
  const detail = warning ? `${fixable[0].name}: ${fixable[0].reason}`
    : !shown ? undefined
    : running.size === 1 && only ? `Everything works on ${shortVersion(only)} while your computers get ready.`
    : "Everything keeps working while your computers get ready.";
  return {
    target: target.version, targetSha256: target.sha256, done: done + ready + switching, updating, behind, failed,
    stage: warning ? "failed" : switching ? "switching" : updating ? "downloading" : ready ? "waiting" : "finished",
    shown, title, detail, warning, dismissed: false, failures: [],
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

export function* retryFleetRoute({ request, json }: RouteContext): Generator<AnswerPiece> {
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isObject(given) || typeof given.target !== "string" || (given.targetSha256 !== undefined && typeof given.targetSha256 !== "string"))
    return yield* json(400, { message: "Send the setup’s target version." });
  const reply = requestFleetRetry(given.target, given.targetSha256);
  return yield* json(reply.status, reply.status === 202 ? reply.update : { message: reply.message });
}
