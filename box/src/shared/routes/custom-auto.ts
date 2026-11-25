// The model-list settings kept by the agents service; running a turn on one is separate.
import { any, bool, list, number, object, string, words, type RouteShape } from "../shape";

const ENTRY = object({ providerID: string, modelID: string, "variant?": string });
const LIST = object({ id: string, name: string, entries: list(ENTRY), revision: string });
const LIMIT = object({ key: string, until: number, scope: words("provider", "model"), known: bool });
const LISTS = object({ data: object({ lists: list(LIST), limits: list(LIMIT) }) });
const READ = object({ data: object({ entries: list(ENTRY), lists: list(LIST), limits: list(LIMIT) }) });
const REFUSED = object({ message: string });

export const CUSTOM_AUTO_ROUTES: RouteShape[] = [
  { route: "GET kuvo/agents/api/kuvo/auto", feature: "auto.lists", answers: { 200: READ } },
  { route: "POST kuvo/agents/api/kuvo/auto", feature: "auto.lists", body: any, answers: { 200: LISTS, 400: REFUSED, 409: REFUSED } },
  { route: "PUT kuvo/agents/api/kuvo/auto/:id", feature: "auto.lists", body: any, answers: { 200: LISTS, 400: REFUSED, 409: REFUSED } },
  { route: "DELETE kuvo/agents/api/kuvo/auto/:id", feature: "auto.lists", body: any, answers: { 200: LISTS, 400: REFUSED, 409: REFUSED } },
  { route: "PUT kuvo/agents/api/kuvo/auto/order", feature: "auto.lists", body: any, answers: { 200: LISTS, 400: REFUSED, 409: REFUSED } },
  { route: "DELETE kuvo/agents/api/kuvo/auto/limits", feature: "auto.lists", answers: { 200: object({}) } },
];
