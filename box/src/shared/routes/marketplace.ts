import { bool, list, map, number, object, string, type RouteShape } from "../shape";
import { MADE } from "./bots";

const data = map("any");
const action = object({ id: string, title: string, "form?": data });
const file = object({ path: string, data: string, sha256: string });
const computer = object({ key: string, name: string, "here?": bool, available: bool });
const browse = object({
  title: string, computers: list(computer), searchPlaceholder: string,
  areas: list(object({ id: string, title: string, "line?": string, count: number,
    categories: list(object({ title: string, count: number })), items: list(data),
    actions: list(action), "filters?": list(action), empty: string, "here?": data })),
});
const refresh = object({ refreshed: bool, "message?": string });
const preview = object({ preview: string, skill: data, installed: bool, files: list(file), action });
const signIn = object({ attempt: string, opens: string, title: string, callbackLabel: string, submit: string });

const route = (verb: string, path: string, answers: RouteShape["answers"]): RouteShape =>
  ({ route: `${verb} kuvo/marketplace${path}`, feature: "marketplace", answers });

export const MARKETPLACE_ROUTES: RouteShape[] = [
  route("GET", "", { 200: browse }),
  route("POST", "/refresh", { 200: refresh }),
  route("POST", "/apps/custom", { 200: data }),
  route("GET", "/apps/:id", { 200: data }),
  route("GET", "/apps/:id/logo", { 200: "bytes" }),
  route("POST", "/apps/:id/add", { 200: data }),
  route("DELETE", "/apps/:id", { 200: data }),
  route("POST", "/apps/:id/connect", { 200: data, 202: signIn }),
  route("POST", "/apps/:id/disconnect", { 200: data }),
  route("GET", "/apps/:id/status", { 200: object({ state: string }) }),
  route("POST", "/apps/:id/cancel", { 200: object({ state: string }) }),
  route("POST", "/apps/:id/finish", { 200: data }),
  route("GET", "/agents/:id", { 200: data }),
  route("GET", "/agents/:id/options", { 200: data }),
  route("POST", "/agents/:id/add", { 201: MADE }),
  route("POST", "/skills/create", { 200: data }),
  route("POST", "/skills/preview", { 200: preview }),
  route("POST", "/skills/install", { 200: data }),
  route("POST", "/skills/restore", { 200: data }),
  route("POST", "/skills/places", { 200: data }),
  route("DELETE", "/skills/places", { 200: data }),
  route("POST", "/skills/:id/enabled", { 200: object({ enabled: bool }) }),
  route("GET", "/skills/:id", { 200: data }),
  route("POST", "/skills/:id/add", { 200: data }),
  route("POST", "/skills/:id/update-preview", { 200: preview }),
  route("PATCH", "/skills/:id", { 200: data }),
  route("DELETE", "/skills/:id", { 200: data }),
];
