// Group chats that live here

import fs from "node:fs";
import path from "node:path";
import { mayActivate } from "../ownership/resolve";
import { IS_MAC, RUN } from "../home";
import { writePrivate } from "../files";
import { agentsAway, type AgentsAway, AGENTS_RESTARTING, bridgeBack } from "./agents-down";
import { listedAgents, nameMissing, refreshOwnShared } from "../server/naming";
import type { Target } from "../../shared/records";
import type { Groups, ListedGroup } from "../../shared/routes/groups";
import { fromLocalApp, type AnswerPiece, type LinkRequest, type RouteContext } from "../server/context";

// What a client asks of them (PROTOCOL.md, `kuvo/groups`): the list and one chat (read, or followed
// as they change), who can be in one, and what it does (makes, changes or deletes one, writes to it,
// stops it, lets it keep going, looks at it, takes a message back, starts the work a wrap-up split
// up). The coding agents' bridge that runs them answers in the shapes a client takes, with every
// rule of what can be: passed on as it answers.
export const GROUP_ROUTE = /^kuvo\/groups(?:\/(events|joinable|grp_[a-z0-9]+(?:\/(?:events|stop|keep-going|viewed|undo|start|restore-review))?))?$/;

// What the bridge's own group chat API (`/api/kuvo/groups…`, src/agents/groups/routes.ts) answers
// is the asker's to say (`Value`), with its status as it came; anything that isn't JSON comes as {}.
export async function askGroups<Value = unknown>(target: Target, method: string, route: string, body: unknown, signal: AbortSignal | undefined): Promise<{ status: number; value: Value }> {
  const response = await fetch(`http://127.0.0.1:${target.port}/api/kuvo/groups${route}`, {
    method,
    headers: { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`, ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal,
  });
  const value = await response.json().catch(() => ({})) as Value;
  return { status: response.status, value };
}

// The chats that live here, as a client lists them (`kuvo/groups`), from the bridge that runs
// them; null while it doesn't answer.
export async function listedGroups(target: Target, signal: AbortSignal | undefined): Promise<ListedGroup[] | null> {
  try {
    const response = await fetch(`http://127.0.0.1:${target.port}/api/kuvo/link/groups`, {
      headers: { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}` },
      signal,
    });
    if (!response.ok) return null;
    const groups = ((await response.json()) as Partial<Groups> | null)?.groups;
    return Array.isArray(groups) ? groups : null;
  } catch {
    return null;
  }
}

// A client's group chat route, to the bridge that runs them and back as it answers (a followed
// one as its lines come). Not answering now: 503, to ask again, never taken as having none.
export async function* forwardGroups(target: Target, route: string, request: LinkRequest, signal: AbortSignal | undefined): AsyncGenerator<AnswerPiece> {
  const method = request.method.toUpperCase();
  const sends = !["GET", "HEAD", "DELETE"].includes(method);
  let response: Response;
  try {
    response = await fetch(`http://127.0.0.1:${target.port}/api/kuvo/link/groups${route.slice("kuvo/groups".length)}`, {
      method,
      headers: { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`, ...(sends ? { "content-type": "application/json" } : {}) },
      body: sends ? Buffer.from(request.body ?? []) : undefined,
      signal,
    });
  } catch {
    if (signal?.aborted) return;
    return yield* groupJSON(503, { message: groupsNotAnswering() });
  }
  if (response.status >= 500) {
    await response.body?.cancel().catch(() => {});
    return yield* groupJSON(503, { message: groupsNotAnswering() });
  }
  yield { head: { status: response.status, headers: { "content-type": response.headers.get("content-type") ?? "application/json" } } };
  try {
    for await (const chunk of response.body ?? []) yield { body: Buffer.from(chunk) };
  } catch {}
}

// MARK: The list as last read

/** The chats' list as the bridge last answered it, kept so a client draws them at once after the
 * box or its bridge starts again (only none the very first time). */
export const GROUPS_FILE = path.join(RUN, "lists", "groups.json");

function savedGroups(): string | null {
  try {
    const text = fs.readFileSync(GROUPS_FILE, "utf8");
    return Array.isArray((JSON.parse(text) as Partial<Groups>)?.groups) ? text : null;
  } catch {
    return null;
  }
}

function saveGroups(text: string): void {
  try {
    if (!Array.isArray((JSON.parse(text) as Partial<Groups>)?.groups)) return;
    if (savedGroups() !== text) writePrivate(GROUPS_FILE, text);
  } catch {}
}

/** The list's answer passed on as it comes, each whole state in it kept. */
async function* keepingGroups(pieces: AsyncGenerator<AnswerPiece>, events: boolean, skipHead = false): AsyncGenerator<AnswerPiece> {
  let ok = false;
  let text = "";
  for await (const piece of pieces) {
    if ("head" in piece) {
      ok = piece.head.status === 200;
      if (skipHead) {
        // (Its head went with the saved list: a refusal now ends the stream.)
        if (!ok) return;
        continue;
      }
    }
    else if (ok && "body" in piece) {
      text += Buffer.from(piece.body).toString();
      if (events) {
        for (let at = text.indexOf("\n"); at >= 0; at = text.indexOf("\n")) {
          const line = text.slice(0, at).trim();
          text = text.slice(at + 1);
          if (line) saveGroups(line);
        }
      }
    }
    yield piece;
  }
  if (ok && !events && text) saveGroups(text.trim());
}

/** Why there are no group chats to ask now: the bridge that runs them isn't running (its state). */
export const groupsAbsent = (): AgentsAway => agentsAway() ?? { message: AGENTS_RESTARTING, reason: "agentsRestarting" };
export const groupsNotAnswering = () => (IS_MAC ? "Group chats on this Mac aren’t answering. Try again in a moment." : "The coding agents’ bridge here isn’t answering. Try again in a moment.");

export function* groupJSON(status: number, value: { message: string } | AgentsAway): Generator<AnswerPiece> {
  yield { head: { status, headers: { "content-type": "application/json" } } };
  yield { body: Buffer.from(JSON.stringify(value)) };
}

// The group chats that live here (`kuvo/groups`), from the coding agents' bridge, which runs them.
// What it answers is the bridge's, passed on as it comes, in the shapes of shared/routes/groups.ts.
export async function* groupsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, route, readEngine, bridge } = context;
  if (route.endsWith("/restore-review")) {
    if (!fromLocalApp(request)) return yield* groupJSON(403, { message: "Review recovered group work on this computer." });
    if (!mayActivate({ kind: "groupSettings", id: route.split("/")[2] }))
      return yield* groupJSON(409, { message: "This group belongs to another computer or is moving." });
  }
  // The list, while the bridge that runs them is starting: as last read, at once; then, followed,
  // as the bridge says it once it's back.
  const method = request.method.toUpperCase();
  const listing = method === "GET" && (route === "kuvo/groups" || route === "kuvo/groups/events");
  const events = route === "kuvo/groups/events";
  const saved = listing && !bridge() ? savedGroups() : null;
  if (saved) {
    if (!events) return yield* groupJSON(200, JSON.parse(saved));
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    yield { body: Buffer.from(saved + "\n") };
    const back = await bridgeBack(bridge, signal);
    if (!back) return;
    return yield* keepingGroups(forwardGroups(back, route, request, signal), true, true);
  }
  const target = await bridgeBack(bridge, signal);
  if (!target) return yield* groupJSON(503, groupsAbsent());
  // Who a message names with "&" is worked out there, from the list as it is now: worked out
  // again meanwhile, and waited for (two seconds at most) only when a name after "&" isn't
  // there yet, as a message from a phone (`namingHere`).
  if (request.method.toUpperCase() === "POST" && route.split("/").length === 3 && Buffer.from(request.body ?? []).includes("&")) {
    let text: unknown = "";
    try { text = JSON.parse(Buffer.from(request.body!).toString())?.text; } catch {}
    const working = refreshOwnShared(readEngine);
    if (typeof text === "string" && nameMissing(text, listedAgents().agents)) {
      await Promise.race([working, new Promise(resolve => setTimeout(resolve, 2000).unref?.())]);
    }
  }
  const veto = context.admitQueued?.();
  if (veto) {
    const pieces = context.json(veto.status, veto.value);
    for (const piece of pieces) yield "head" in piece && veto.notAdmitted ? { head: { ...piece.head, notAdmitted: true } } : piece;
    return;
  }
  if (listing) return yield* keepingGroups(forwardGroups(target, route, request, signal), events);
  return yield* forwardGroups(target, route, request, signal);
}
