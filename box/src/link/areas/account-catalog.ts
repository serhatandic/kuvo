// The Accounts catalog: one read of each source's integration list, with the box's account facts.
import { isObject } from "../../shared/json";
import { computerInSentence } from "../computer-name";
import type { AnswerPiece, RouteContext } from "../server/context";
import { agentsRoute, engineRoute } from "../server/forward";
import { asked } from "./settings-kit";
import { describeAccounts } from "./sign-in";

export const ACCOUNT_CATALOG_ROUTE = "kuvo/accounts/catalog";

/** Only display fields from the underlying integration. Its connections and metadata can carry
 * fields the catalog doesn't need. */
function catalogMetadata(value: unknown) {
  if (!isObject(value)) return {};
  const { accountLabel, version, programUpdate } = value;
  return {
    ...(typeof accountLabel === "string" ? { accountLabel } : {}),
    ...(typeof version === "string" ? { version } : {}),
    ...(isObject(programUpdate) && typeof programUpdate.state === "string" ? { programUpdate: {
      state: programUpdate.state,
      ...(typeof programUpdate.version === "string" ? { version: programUpdate.version } : {}),
      ...(typeof programUpdate.message === "string" ? { message: programUpdate.message } : {}),
    } } : {}),
  };
}

function unavailable(value: unknown) {
  const { message, reason, action } = isObject(value) ? value : {};
  return {
    message: typeof message === "string" ? message : "It said no (503).",
    ...(typeof reason === "string" ? { reason } : {}),
    ...(isObject(action) && typeof action.label === "string" && typeof action.route === "string" ? { action: { label: action.label, route: action.route } } : {}),
  };
}

/** One request for Settings and first run. The box assigns the Settings section without changing
 * either source's order (first run also uses that order). */
export async function* accountCatalogRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  // Read exactly what the integration routes offer, including their bridge handover and the
  // GitHub account the engine's list adds. Each source is read once.
  const [engine, agents] = await Promise.all([
    asked(context, engineRoute, "GET", "api/integration"),
    asked(context, agentsRoute, "GET", "kuvo/agents/api/integration"),
  ]);
  for (const { status, value } of [engine, agents]) {
    if (status === 503) continue;
    if (status !== 200) return yield* json(status, value);
    if (!Array.isArray(value?.data)) return yield* json(502, { message: "The accounts couldn’t be read. Try again." });
  }
  const computer = computerInSentence(request);
  const rows = ({ status, value }: typeof engine, agents: boolean) =>
    describeAccounts(status === 200 ? value.data : [], agents, computer).flatMap(({ account, facts }) => {
      if (!facts.listed || typeof account.name !== "string") return [];
      const section = agents ? "codingAgents" : facts.state === "signedOut" ? "addProvider" : "providers";
      return [{ ...facts, name: account.name, metadata: catalogMetadata(account.metadata), section }];
    });
  return yield* json(200, {
    engine: rows(engine, false),
    agents: rows(agents, true),
    ...(engine.status === 503 ? { engineDown: unavailable(engine.value) } : {}),
    ...(agents.status === 503 ? { agentsDown: unavailable(agents.value) } : {}),
  });
}
