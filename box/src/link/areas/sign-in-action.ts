// The remaining same-box sign-in actions. Forward them as the existing integration routes do,
// preserving their refusals, access checks, and bridge handovers.
import { GITHUB_WORK_QUEUE } from "../../shared/github";
import { isObject, parsed } from "../../shared/json";
import type { AnswerPiece, RouteContext } from "../server/context";
import { AGENTS, agentsRoute, engineRoute } from "../server/forward";
import { workQueueSignInRoute } from "./sign-in";

export const SIGN_IN_ACTION_ROUTE = "kuvo/sign-in/action";

export async function* signInActionRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const mark = request.path.indexOf("?");
  const query = new URLSearchParams(mark === -1 ? "" : request.path.slice(mark + 1));
  const account = query.get("integration");
  const workQueue = account === GITHUB_WORK_QUEUE;
  const onBridge = !workQueue && query.get("agents") === "1";
  const given = parsed(request.body);
  if (!account || !isObject(given)) return yield* json(400, { message: "Name the account and what to do." });
  const base = `${onBridge ? AGENTS : ""}api/integration/${encodeURIComponent(account)}/connect`;
  const answer = isObject(given.answer) ? { answer: given.answer } : {};
  const to = (path: string, method: string, body?: unknown) => ({
    ...context, route: path,
    request: {
      ...request, path, method,
      ...(body === undefined
        ? { body: undefined }
        : { headers: { ...request.headers, "content-type": "application/json" }, body: Buffer.from(JSON.stringify(body)) }),
    },
  });
  const forward = onBridge ? agentsRoute : engineRoute;
  switch (given.action) {
    case "cancel":
    case "complete": {
      if (typeof given.attempt !== "string" || !given.attempt) break;
      const attempt = `${base}/oauth/${encodeURIComponent(given.attempt)}`;
      if (given.action === "cancel") {
        // GitHub for Work Queue is listed with the engine providers, but its sign-in is the bridge's.
        if (workQueue) {
          if (!/^att_[a-zA-Z0-9]+$/.test(given.attempt)) break;
          return yield* workQueueSignInRoute(to(attempt, "DELETE"));
        }
        return yield* forward(to(attempt, "DELETE"));
      }
      if (typeof given.code === "string") return yield* forward(to(`${attempt}/complete`, "POST", { code: given.code }));
      break;
    }
    case "addKey":
      if (typeof given.key === "string") return yield* forward(to(`${base}/key`, "POST", { key: given.key, ...answer }));
      break;
    case "external":
      if (typeof given.methodID === "string") return yield* forward(to(`${base}/external`, "POST", { methodID: given.methodID, ...answer }));
      break;
  }
  return yield* json(400, { message: "Name the sign-in action and what it needs." });
}
