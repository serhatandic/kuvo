import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import type { ToldComputer } from "../../shared/records";
import type { FleetSummary, FleetUpdate, ListedComputer } from "../../shared/routes/box";
import { RUN } from "../home";
import type { RouteContext, AnswerPiece } from "../server/context";
import { askForRetry, claimRetry, requestedRetry } from "./attempt";
import { cannotFor, CANNOT_FILE, READY_FILE, retryCannot, sameRelease } from "./facts";
import { fleetFacts, fleetTarget, HELD, retryTransfer, type FleetTarget } from "./fleet";
import { newerVersion } from "./update";

export const fleetFiles = () => [CANNOT_FILE, READY_FILE, path.join(RUN, "previous"), path.join(RUN, "update-cannot"), path.join(HELD, "boxes.json")];

export function peerFleetUpdate(target: FleetTarget, facts: ToldComputer | undefined, away = false): FleetUpdate {
  const current = facts?.kuvo ?? null;
  const { version, sha256 } = target;
  const progress = { current, target: version, targetSha256: sha256, currentSha256: facts?.installedSha256, canRetry: false };
  const upToDate = current && !newerVersion(version, current);
  if (away) return { ...progress, state: "off" };
  if (upToDate) return { ...progress, state: "up-to-date" };
  const ref = sha256 ? { version, sha256 } : undefined;
  const cannot = facts?.cannot;
  if (cannot && sameRelease(cannot, ref))
    return { ...progress, state: "cannot", reason: cannot.reason, fixable: cannot.fixable, canRetry: cannot.fixable };
  if (sameRelease(facts?.staged, ref)) return { ...progress, state: "ready" };
  return { ...progress, state: "getting-ready" };
}

export function localFleetUpdate(target = fleetTarget(), facts = fleetFacts()): FleetUpdate | undefined {
  if (!target) return;
  const result = peerFleetUpdate(target, facts);
  return result.canRetry && requestedRetry()?.sha256 === result.targetSha256 ? { ...result, canRetry: false } : result;
}

export function fleetSummary(rows: readonly Pick<ListedComputer, "fleetUpdate">[], target: FleetTarget | undefined): FleetSummary | undefined {
  if (!target) return;
  let done = 0, updating = 0, ready = 0, behind = 0, failed = 0;
  let detail: string | undefined;
  for (const row of rows) {
    const progress = row.fleetUpdate;
    switch (progress?.state) {
      case "up-to-date": done++; break;
      case "ready": ready++; break;
      case "getting-ready": updating++; break;
      case "off": behind++; break;
      case "cannot":
        failed++;
        if (progress.fixable) detail ??= progress.reason;
        break;
    }
  }
  const version = target.version.replace(/^.*-(beta\.\d+)$/, "$1");
  const total = done + ready + updating + failed;
  const warning = detail !== undefined;
  const shown = warning || updating > 0 || ready > 0;
  const title = warning ? `Can’t update to ${version}` : `Updating to ${version} · ${done + ready} of ${total} ready`;
  return {
    target: target.version, targetSha256: target.sha256, done: done + ready, updating, behind, failed,
    stage: warning ? "failed" : updating ? "downloading" : ready ? "waiting" : "finished",
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
