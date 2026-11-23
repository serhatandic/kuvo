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
  // Browse: three ordered areas (apps, agents, skills) and the computer choices (`q`, `category`, `origin`).
  route("GET", "", { 200: browse }),
  // Reads the public catalog index again; the last good copy stays if it doesn't validate.
  route("POST", "/refresh", { 200: refresh }),
  // Adds your own Streamable HTTP server across the setup (`{title, url, id?, summary?, auth?}`; HTTPS).
  route("POST", "/apps/custom", { 200: data }),
  // An app's details and its actions' words.
  route("GET", "/apps/:id", { 200: data }),
  route("GET", "/apps/:id/logo", { 200: "bytes" }),
  // Adds a catalog app across the setup.
  route("POST", "/apps/:id/add", { 200: data }),
  // Removes the app from the setup.
  route("DELETE", "/apps/:id", { 200: data }),
  // Connects it here: `{token}` for a token server, else an OAuth flow to open (202: `attempt`, `opens`).
  route("POST", "/apps/:id/connect", { 200: data, 202: signIn }),
  // Removes this computer's credentials for the app.
  route("POST", "/apps/:id/disconnect", { 200: data }),
  // How an OAuth connect (`attempt=`) stands.
  route("GET", "/apps/:id/status", { 200: object({ state: string }) }),
  // Gives up on an OAuth connect (`{attempt}`).
  route("POST", "/apps/:id/cancel", { 200: object({ state: string }) }),
  // Finishes an OAuth connect from another computer with the full callback address and its `attempt`.
  route("POST", "/apps/:id/finish", { 200: data }),
  route("GET", "/agents/:id", { 200: data }),
  // What adding an agent on a computer (`computer=<key>`) offers: models, accounts, missing apps, routines.
  route("GET", "/agents/:id/options", { 200: data }),
  // Adds the agent as a bot on the chosen computer, with its skills; the same `request` on a retry.
  route("POST", "/agents/:id/add", { 201: MADE }),
  // Makes a skill of your own (`{name, description, instructions}`).
  route("POST", "/skills/create", { 200: data }),
  // Reads a skill to install (a folder, a GitHub link, or a repository path) and shows its exact files.
  route("POST", "/skills/preview", { 200: preview }),
  // Installs exactly the bytes a preview showed (`{preview, was?}`).
  route("POST", "/skills/install", { 200: data }),
  // Restores a skill from Recently Deleted (`{id}`) and lifts its setup-wide removal.
  route("POST", "/skills/restore", { 200: data }),
  // Adds a folder to Other Places, where this computer finds skills (`{place}`).
  route("POST", "/skills/places", { 200: data }),
  // Removes one of Other Places (`{index}`).
  route("DELETE", "/skills/places", { 200: data }),
  // Turns a skill on or off on this computer (`{on}`).
  route("POST", "/skills/:id/enabled", { 200: object({ enabled: bool }) }),
  // A skill's details, its actions, and an opaque `revision` for edits and removal.
  route("GET", "/skills/:id", { 200: data }),
  // Installs a catalog skill (`{version, revision, was?}`).
  route("POST", "/skills/:id/add", { 200: data }),
  // Previews an installed GitHub skill's update, to install with the previous revision.
  route("POST", "/skills/:id/update-preview", { 200: preview }),
  // Edits an authored or folder skill (`{description, instructions, was}`).
  route("PATCH", "/skills/:id", { 200: data }),
  // Removes an installed skill setup-wide (`{was}`); each computer moves its folder to Recently Deleted.
  route("DELETE", "/skills/:id", { 200: data }),
];
