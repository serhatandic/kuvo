import { isObject, parsed } from "../../shared/json";
import { destinationOf, destinationStore } from "../backup/destination";
import { backupBefore, backupStatus, config, configure, disable, erasePoint, selectedProjects } from "../backup/engine";
import { matchReplacement, restore, restoreStatus } from "../backup/restore";
import { pendingAfterRestore, pendingQueueItems, returnPending, settleQueueItem } from "../backup/pending";
import { exportVault, importVault } from "../backup/vault";
import { fromLocalApp, type AnswerPiece, type RouteContext } from "../server/context";
import { holdsVault } from "../vault/holder";
import { pairedSubmitter } from "../ownership/authority";
import { readLink } from "../state";

export async function* computerBackupsRoute(context: RouteContext) {
  const { request, route, json: result, signal } = context;
  const json = (status: number, value: unknown): AnswerPiece[] => result(status, value).map(piece =>
    "head" in piece ? { head: { ...piece.head, headers: { ...piece.head.headers, "cache-control": "no-store" } } } : piece);
  const method = request.method.toUpperCase();
  const action = route.slice("kuvo/box/backups".length);
  if (action === "/status") {
    if (!fromLocalApp(request) && !pairedSubmitter(context, readLink()))
      return yield* json(403, { message: "Open backup status from a paired Kuvo Code client." });
    if (method !== "GET") return yield* json(405, {});
    try {
      const { enabled, destination, destinationAvailable, lastVerifiedRestorePoint, error } = backupStatus();
      return yield* json(200, { enabled, destinationKind: destination?.kind ?? null, destinationAvailable,
        lastVerifiedAt: lastVerifiedRestorePoint?.at ?? null, attention: Boolean(error) });
    } catch {
      return yield* json(503, { message: "Backup status is unavailable." });
    }
  }
  // Recovery keys, destinations, paths and recovery operations require the authenticated local app.
  if (!fromLocalApp(request)) return yield* json(403, { message: "Open backups on this computer." });
  if (method !== "POST" && !(method === "GET" && (!action || action === "/pending"))) return yield* json(405, {});
  const body = parsed(request.body);
  const given = isObject(body) ? body : {};
  try {
    if (method === "GET" && !action) return yield* json(200, { ...backupStatus(), restore: restoreStatus() });
    if (action === "/pending") {
      if (method === "GET") return yield* json(200, { waiting: pendingAfterRestore(), queueItems: pendingQueueItems() });
      if (given.kind === "queueItem") {
        return yield* json(200, await settleQueueItem(String(given.id ?? ""), String(given.operation ?? ""), given.decision));
      }
      if (given.confirmStillDue !== true) return yield* json(409, { message: "Check whether this work already ran, then confirm it’s still due." });
      return yield* json(200, await returnPending(String(given.session ?? ""), String(given.id ?? "")));
    }
    if (action === "/destination") {
      if (given.enabled === false) { await disable(); return yield* json(200, backupStatus()); }
      const dest = destinationOf(given.destination);
      if (given.confirm !== true) return yield* json(200, { destination: dest, current: backupStatus() });
      const recovery = await configure(dest, selectedProjects(given.projects));
      // Keys are only returned here, over the local authenticated route.
      return yield* json(200, { ...backupStatus(), ...recovery });
    }
    if (action === "/now") {
      return yield* json(200, { point: await backupBefore("daily") });
    }
    if (action === "/match") {
      const save = await matchReplacement(String(given.source ?? ""), String(given.previous ?? ""), String(given.replacement ?? ""));
      return yield* json(save?.status === "pending" ? 202 : save?.status === "interrupted" ? 409 : 200, { matched: !save || save.status === "committed", ...(save ? { save } : {}) });
    }

    const kept = config();
    const destination = destinationOf(given.destination ?? kept?.dest);
    const source = typeof given.source === "string" ? given.source : kept?.source ?? "";
    if (action === "/points") return yield* json(200, { points: await destinationStore(destination, source).list() });
    if (given.confirm !== true) return yield* json(400, { message: "Confirm this backup operation first." });
    const id = typeof given.id === "string" ? given.id : "";
    if (action === "/erase") {
      await erasePoint(destination, source, id);
      return yield* json(200, { erased: id });
    }
    if (action === "/vault" || action === "/vault/import") {
      if (!holdsVault()) return yield* json(503, { message: "Open the vault on this computer first." });
      if (action === "/vault") {
        if (!kept?.enabled || JSON.stringify(destination) !== JSON.stringify(kept.dest) || source !== kept.source) {
          return yield* json(409, { message: "Choose a separate backup destination before exporting the vault." });
        }
        return yield* json(200, await exportVault(destination, source));
      }
      if (typeof given.recoveryKey === "string") return yield* json(200, await importVault(destination, source, id, given.recoveryKey));
    }
    if (action === "/restore" && typeof given.recoveryKey === "string") {
      return yield* json(200, await restore(destination, source, id, given.recoveryKey, signal));
    }
    return yield* json(400, { message: "Choose a restore point and enter its recovery key." });
  } catch (error) {
    const message = (error as Error).message;
    return yield* json(409, { message: /authenticate data|unable to authenticate/i.test(message) ? "Backup verification failed. Check the recovery key and archive." : message });
  }
}
