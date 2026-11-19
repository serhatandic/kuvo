import { isObject, parsed } from "../../shared/json";
import { destinationOf, destinationStore } from "../backup/destination";
import { backupBefore, backupStatus, config, configure, disable, erasePoint, selectedProjects } from "../backup/engine";
import { matchReplacement, restore, restoreStatus } from "../backup/restore";
import { pendingAfterRestore, returnPending } from "../backup/pending";
import { exportVault, importVault } from "../backup/vault";
import { fromLocalApp, type AnswerPiece, type RouteContext } from "../server/context";
import { holdsVault } from "../vault/holder";

export async function* computerBackupsRoute({ request, route, json: result, signal }: RouteContext) {
  const json = (status: number, value: unknown): AnswerPiece[] => result(status, value).map(piece =>
    "head" in piece ? { head: { ...piece.head, headers: { ...piece.head.headers, "cache-control": "no-store" } } } : piece);
  // Includes recovery keys and local filesystem choices. Only the authenticated local app can act.
  if (!fromLocalApp(request)) return yield* json(403, { message: "Open backups on this computer." });
  const method = request.method.toUpperCase();
  const action = route.slice("kuvo/box/backups".length);
  if (method !== "POST" && !(method === "GET" && (!action || action === "/pending"))) return yield* json(405, {});
  const body = parsed(request.body);
  const given = isObject(body) ? body : {};
  try {
    if (method === "GET" && !action) return yield* json(200, { ...backupStatus(), restore: restoreStatus() });
    if (action === "/pending") {
      if (method === "GET") return yield* json(200, { waiting: pendingAfterRestore() });
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
