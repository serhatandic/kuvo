import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { isObject, parsed } from "../../shared/json";
import type { FleetSummary, FleetUpdate, ListedComputer } from "../../shared/routes/box";
import { locked, textOf, writePrivate } from "../files";
import { K, RUN } from "../home";
import type { RouteContext, AnswerPiece } from "../server/context";
import { swapUnderWay } from "../../shared/engine-swap";
import { bundleBuild } from "../../shared/bundle";
import { readBridgeFile } from "../engine/client";
import { askForRetry, requestedRetry, transferAttempt, STUCK_AFTER } from "./attempt";
import { releaseChoice } from "./choice";
import {
  boxVersion, installedPack, isUpdating, newerVersion, noteTrialEnded, offerChangesEngine, offerHeld, offerRetryAt, sameVersion,
  offeredPack, offeredVersion, updateOnTrial, updateState, updateStatusHere, updatesBlocked,
} from "./update";

const fleetDismissals = path.join(RUN, "fleet-dismissals.json");
const updateFacts = path.join(RUN, "update.json");
const transferFacts = path.join(RUN, "fleet-transfer.json");
export const fleetFiles = [fleetDismissals, updateFacts, transferFacts];

/** When this box first observed a given target through accepted setup state. */
function learnedAt(target: string, sha256: string | undefined, now: number): number {
  const file = path.join(RUN, "fleet-target.json");
  const known = parsed(textOf(file));
  if (isObject(known) && known.target === target && known.sha256 === sha256 && typeof known.at === "number") return known.at;
  try { writePrivate(file, JSON.stringify({ target, sha256, at: now })); } catch {}
  return now;
}

export function localFleetUpdate(target = releaseChoice()?.version, runningTurns: number | null = null, now = Date.now(), choice = releaseChoice()): FleetUpdate | undefined {
  if (!target) return undefined;
  const installed = boxVersion();
  try { noteTrialEnded(); } catch {}
  const update = updateState();
  const trial = updateOnTrial();
  const targetSha256 = choice?.version === target ? choice.sha256 : undefined;
  const currentSha256 = trial ? undefined : installedPack() ?? undefined;
  const installedTarget = installed === target && (!targetSha256 || currentSha256 === targetSha256);
  const current = trial ? update.trial?.from ?? installed : installed;
  const attempt = transferAttempt();
  const transfer = attempt?.version === target && attempt.sha256 === targetSha256 ? attempt : null;
  const failed = update.offerFailed;
  const failedAt = Math.max(transfer?.version === target ? transfer.lastFailedAt ?? 0 : 0,
    update.fleetFailedAt?.version === target && (!update.fleetFailedAt.sha256 || update.fleetFailedAt.sha256 === targetSha256) ? update.fleetFailedAt.at : failed?.version === target && (!failed.sha256 || failed.sha256 === targetSha256) ? failed.at : 0);
  const needsUpdate = current ? newerVersion(target, current) || !!targetSha256 && sameVersion(target, current) && targetSha256 !== currentSha256 : undefined;
  const progress = { current, target, targetSha256, currentSha256, requiresUpdate: choice?.clientProtocol ? needsUpdate : false, canRetry: false, ...(failedAt ? { lastFailureAt: failedAt } : {}) };
  if (trial) return { ...progress, state: "updating", stage: "switching" };
  const working = isUpdating();
  if (working && !installedTarget) {
    const stage = update.running?.startsWith("downloading") ? "downloading" : "switching";
    return { ...progress, state: "updating", stage };
  }
  if (installedTarget) {
    const swap = swapUnderWay(RUN);
    if (swap) return swap.stage === "waiting" || swap.stage === "holding"
      ? { ...progress, state: "updating", stage: "waiting", runningTurns }
      : { ...progress, state: "updating", stage: "switching" };
    try {
      const bridge = readBridgeFile();
      const expected = bundleBuild(path.join(K, "bin", "kuvo-agents.js"));
      if (bridge?.build && expected && bridge.build !== expected) {
        return bridge.renewal?.waits === "quiet" || bridge.renewal?.waits === "holding"
          ? { ...progress, state: "updating", stage: "waiting", runningTurns }
          : { ...progress, state: "updating", stage: "switching" };
      }
    } catch {}
    return { ...progress, state: "up-to-date" };
  }
  const canRetry = !!needsUpdate && !(updatesBlocked() ?? offerHeld()) && (!choice || requestedRetry()?.sha256 !== choice.sha256);
  if (transfer?.version === target && !transfer.reason && now - transfer.at < STUCK_AFTER && transfer.step !== "offer accepted")
    return { ...progress, state: "updating", stage: "downloading" };
  if (failed?.version === target && (!failed.sha256 || failed.sha256 === targetSha256) && (!transfer || transfer.version !== target || transfer.step === "offer accepted" || failed.at >= transfer.at)) {
    return { ...progress, state: "failed", canRetry, reason: failed.message ?? "Kuvo couldn’t install the release.",
      rolledBackTo: failed.takenBack ? current : null, nextRetryAt: failed.retryNow ? failed.at : offerRetryAt(failed), failedAt: failed.at };
  }
  if (transfer?.version === target && transfer.reason) {
    return { ...progress, state: "failed", canRetry, reason: transfer.reason, rolledBackTo: null,
      nextRetryAt: transfer.retryAt ?? null, failedAt: transfer.at };
  }
  const stopped = offerHeld();
  const blocked = updatesBlocked() ?? stopped;
  if (blocked) return { ...progress, state: "behind", reason: blocked };
  if (offeredVersion() === target && (!targetSha256 || offeredPack() === targetSha256) && offerChangesEngine() && runningTurns !== 0)
    return { ...progress, state: "updating", stage: "waiting", runningTurns };
  const since = transfer?.version === target ? transfer.at : learnedAt(target, targetSha256, now);
  return { ...progress, state: "behind", canRetry: canRetry && now - since >= STUCK_AFTER,
    ...(newerVersion(current, target) ? { reason: "This computer already runs a newer version." } : {}) };
}

export async function fleetTurnCount(status?: FleetUpdate): Promise<FleetUpdate | undefined> {
  if (status?.state !== "updating" || status.stage !== "waiting") return status;
  const live = await updateStatusHere();
  return { ...status, runningTurns: live.waiting?.sessionsAtWork ?? null };
}

export function peerFleetUpdate(target: string, current: string | undefined, said?: FleetUpdate, away = false, targetSha256?: string, currentSha256?: string, gated = false): FleetUpdate {
  const requiresUpdate = !gated ? false : current ? newerVersion(target, current) || !!targetSha256 && sameVersion(target, current) && targetSha256 !== currentSha256 : undefined;
  if (said?.target === target && said.targetSha256 === targetSha256) {
    if (!away) return said;
    if (said.state === "updating") return { current: said.current, target, targetSha256, currentSha256, requiresUpdate: said.requiresUpdate ?? requiresUpdate, state: "behind", canRetry: false,
      reason: "This computer can’t be reached.", ...(said.lastFailureAt ? { lastFailureAt: said.lastFailureAt } : {}) };
    return { ...said, canRetry: false };
  }
  return { current: current ?? null, target, targetSha256, currentSha256, requiresUpdate, canRetry: false, state: current === target && (!targetSha256 || currentSha256 === targetSha256) ? "up-to-date" : "behind" };
}

type UpdateRow = Pick<ListedComputer, "key" | "fleetUpdate">;
function failureKeys(rows: readonly UpdateRow[]): string[] {
  return rows.flatMap(row => {
    const failure = row.fleetUpdate?.lastFailureAt;
    return failure ? [createHash("sha256").update(JSON.stringify([row.key, failure])).digest("hex")] : [];
  }).sort();
}

type Dismissal = { target: string; sha256?: string; failures: string[] };
function dismissals(): Record<string, Dismissal> {
  const kept = parsed(textOf(fleetDismissals));
  return isObject(kept) ? kept as Record<string, Dismissal> : {};
}

export function fleetClient(context: RouteContext): string | null {
  if (context.phone?.key) return context.phone.key.toString("base64url");
  if (!context.request.served && !context.request.fromMac) return null;
  const given = context.request.headers["x-kuvo-client-id"];
  if (typeof given === "string" && /^[a-f0-9]{32}$/.test(given)) return `local-${given}`;
  return context.request.served ? "local" : null;
}

export function fleetSummary(rows: readonly UpdateRow[], target: string | undefined, client: string | null, targetSha256 = releaseChoice()?.sha256): FleetSummary | undefined {
  if (!target) return undefined;
  const counts = { target, targetSha256, done: 0, updating: 0, behind: 0, failed: 0 };
  const stages = new Set<string>();
  for (const row of rows) {
    const state = row.fleetUpdate?.target === target && row.fleetUpdate.targetSha256 === targetSha256 ? row.fleetUpdate : undefined;
    if (state?.state === "up-to-date") counts.done++;
    else if (state?.state === "updating") { counts.updating++; stages.add(state.stage); }
    else if (state?.state === "failed") counts.failed++;
    else counts.behind++;
  }
  const stage = counts.failed ? "failed" : stages.has("switching") ? "switching" : stages.has("downloading") ? "downloading"
    : stages.has("waiting") ? "waiting" : counts.behind ? "behind" : "finished";
  const dismissal = client ? dismissals()[client] : undefined;
  const failures = failureKeys(rows);
  const dismissed = dismissal?.target === target && dismissal.sha256 === targetSha256 && failures.every(key => dismissal.failures.includes(key));
  return { ...counts, stage, dismissed, failures };
}

export function dismissFleetUpdate(rows: readonly UpdateRow[], target: string, client: string, targetSha256 = releaseChoice()?.sha256): FleetSummary {
  locked(`${fleetDismissals}.lock`, () => {
    const all = dismissals();
    delete all[client];
    all[client] = { target, sha256: targetSha256, failures: failureKeys(rows) };
    writePrivate(fleetDismissals, JSON.stringify(Object.fromEntries(Object.entries(all).slice(-128))));
  }, { required: true });
  return fleetSummary(rows, target, client, targetSha256)!;
}

export function requestFleetRetry(target: string, targetSha256?: string): { status: 202; update: FleetUpdate } | { status: 409; message: string } {
  const choice = releaseChoice();
  if (!choice || target !== choice.version || (targetSha256 && targetSha256 !== choice.sha256)) return { status: 409, message: "The setup’s target version changed." };
  const status = localFleetUpdate(choice.version);
  if (!status?.canRetry || requestedRetry()?.sha256 === choice.sha256)
    return { status: 409, message: "This computer is up to date, is making progress, or cannot retry yet." };
  try {
    if (!askForRetry(choice)) return { status: 409, message: "A retry is already scheduled for this release." };
  } catch (error) {
    return { status: 409, message: `Couldn’t schedule the retry: ${(error as Error).message}` };
  }
  return { status: 202, update: { ...status, canRetry: false } };
}

export function* retryFleetRoute({ request, json }: RouteContext): Generator<AnswerPiece> {
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isObject(given) || typeof given.target !== "string" || (given.targetSha256 !== undefined && typeof given.targetSha256 !== "string")) return yield* json(400, { message: "Send the setup’s target version." });
  const reply = requestFleetRetry(given.target, given.targetSha256);
  return yield* json(reply.status, reply.status === 202 ? reply.update : { message: reply.message });
}
