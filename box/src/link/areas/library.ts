// The Library across your computers (PROTOCOL.md, "Library"): what was handed over to you, which
// each computer's box keeps for itself (its services, beside the artifact record), gathered here so
// a client asks one box. This box's part comes from its services; each other computer's by mail (an
// act in the `library` area), labelled with the computer it's on. A computer that's away, or whose
// Kuvo Code is older and has no Library, is listed as such, and its part waits.

import { readServices } from "../../shared/box-services";
import type { Target } from "../../shared/records";
import { isObject } from "../../shared/json";
import { IS_MAC, RUN } from "../home";
import { ownComputerName } from "../computer-name";
import { computerName } from "../peers/computer-name";
import { ownKey, peersOf, type Peer } from "../peers/known";
import { awaySince } from "../peers/reach";
import { offRelay } from "../letters/leave";
import { readLink, type LinkState } from "../state";
import { forwardAct, MEMORY_READ_FEATURE, takesForwarded, type Act, type Acted, type Forwarded } from "../work-queue/forward";
import type { Computer } from "../work-queue/view";
import type { AnswerPiece, LinkRequest, RouteContext } from "../server/context";

export const LIBRARY_FEATURE = "library";
export const LIBRARY_ROUTE = /^kuvo\/library(?:\/(?:events|lib_[0-9a-f]{24}(?:\/(?:file|thumbnail|hide))?))?$/;
/** What a peer is asked: its list, or one of its items' routes. */
const PEER_ITEM = /^(?:list|lib_[0-9a-f]{24}(?:\/(?:file|thumbnail|hide))?)$/;
/** How often your other computers' parts are asked again while a client follows the Library. */
export const PEERS_AGAIN = 20_000;
/** The most the Library lists at once, newest first, across your computers. */
const MOST = 1000;

/** A computer's part of the Library, as its services answer `kuvo/library`. */
export interface Part {
  items: Record<string, unknown>[];
  more?: boolean;
  sources?: { bots?: { id: string; name: string | null; look?: unknown }[]; you?: boolean };
}

/** Where a computer's part stands: there, away, too old for the Library, or it couldn't be read. */
export type PartState = { state: "here" | "there" } | { state: "away" | "older" | "failed"; message: string };

/** One computer's part, as it was asked for. */
export type Asked = { computer: Computer & { here?: true }; part?: Part } & PartState;

const servicesAway = "This computer’s Library can’t be read right now. Try again in a moment.";

/** This computer's services, asked `kuvo/library<tail>` as the link passes it on. */
async function services(tail: string, query: string, method: string, body: unknown, signal?: AbortSignal | null): Promise<Response | null> {
  const target: Target | undefined = readServices(RUN);
  if (!target) return null;
  try {
    return await fetch(`http://127.0.0.1:${target.port}/api/kuvo/link/library${tail}${query}`, {
      method,
      headers: { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`, ...(method === "POST" ? { "content-type": "application/json" } : {}) },
      body: method === "POST" ? JSON.stringify(body ?? {}) : undefined,
      signal: signal ?? undefined,
    });
  } catch {
    return null;
  }
}

/** This computer's answer to `tail` as a peer's act carries it: JSON, or bytes as `{type, data}`. */
async function servicesActed(tail: string, query: string, method: string, body: unknown, signal?: AbortSignal | null): Promise<Acted> {
  const response = await services(tail, query, method, body, signal);
  if (!response) return { status: 503, value: { message: servicesAway } };
  const type = response.headers.get("content-type") ?? "";
  if (response.ok && !type.includes("json")) return { status: 200, value: { type, data: Buffer.from(await response.arrayBuffer()).toString("base64") } };
  return { status: response.status, value: await response.json().catch(() => ({ message: servicesAway })) };
}

/**
 * What another of your computers asked of this one's Library (`act.item`: `list`, with the list's
 * query in `body.query`; an item, its `file` or `thumbnail`; or `hide`, with `body.hidden`).
 */
export async function libraryForPeer(act: Act): Promise<Acted> {
  if (!PEER_ITEM.test(act.item)) return { status: 404, value: { message: "No such route." } };
  const hides = act.item.endsWith("/hide");
  if (hides !== (act.method === "POST")) return { status: 405, value: { message: hides ? "Remove it with POST." : "Read it with GET." } };
  if (act.item === "list") {
    const query = typeof act.body?.query === "string" ? act.body.query.slice(0, 1000) : "";
    return servicesActed("", query ? "?" + new URLSearchParams(query).toString() : "", "GET", undefined);
  }
  return servicesActed("/" + act.item, "", act.method, hides ? { hidden: act.body?.hidden !== false } : undefined);
}

/** How this box asks another of your computers (a test's own). */
export type AskPeer = (computer: Computer, act: Omit<Act, "id" | "at">, signal?: AbortSignal | null) => Promise<Forwarded>;

const askByMail: AskPeer = (computer, act, signal) =>
  forwardAct(computer, "The Library", act, signal, readLink(), Date.now(), { memory: act.method === "GET" && takesForwarded(computer.key, MEMORY_READ_FEATURE) });

const computerOf = (peer: Peer): Computer => ({ key: peer.key, name: computerName(peer), kind: peer.kind, ...(peer.id ? { id: peer.id } : {}) });

/** This computer, as the Library labels its items. */
function own(link: LinkState | null): Computer & { here: true } {
  return { key: (link && ownKey(link)) || "here", name: ownComputerName(), kind: IS_MAC ? "mac" : "machine", here: true };
}

/** Your other computers' parts for `query`, each asked at once (one away or too old: said, not asked). */
export async function askPeers(query: string, signal?: AbortSignal | null, { peers = peersOf(readLink()), ask = askByMail, has = (key: string) => takesForwarded(key, LIBRARY_FEATURE), away = (key: string) => awaySince(key) !== null || offRelay() }: { peers?: Peer[]; ask?: AskPeer; has?: (key: string) => boolean; away?: (key: string) => boolean } = {}): Promise<Asked[]> {
  return Promise.all(peers.map(async (peer): Promise<Asked> => {
    const computer = computerOf(peer);
    if (!has(peer.key)) return { computer, state: "older", message: `${computer.name} needs a newer Kuvo Code to show its Library here.` };
    if (away(peer.key)) return { computer, state: "away", message: `${computer.name} isn’t answering now, so its part of the Library isn’t shown.` };
    const answer = await ask(computer, { area: "library", method: "GET", item: "list", body: { query } }, signal);
    if ("refused" in answer) return { computer, state: answer.status === 409 ? "older" : "away", message: answer.refused };
    if (answer.status !== 200 || !isPart(answer.value)) return { computer, state: "failed", message: `${computer.name}’s Library couldn’t be read. Try again in a moment.` };
    return { computer, state: "there", part: answer.value };
  }));
}

function isPart(value: unknown): value is Part {
  return isObject(value) && Array.isArray(value.items);
}

/**
 * The Library across your computers: every computer's items with the computer each is on, newest
 * first (the newest `MOST`); the bots with something in it, each with its computer; whether any of
 * your own sessions has; and each computer's state. The computer label is the client's to show
 * only when there's more than one computer (`computers`).
 */
export function merged(parts: Asked[]) {
  const items: Record<string, unknown>[] = [];
  const bots: { id: string; name: string | null; look?: unknown; computer: string }[] = [];
  let you = false;
  let more = false;
  for (const { computer, part } of parts) {
    if (!part) continue;
    const label = { key: computer.key, name: computer.name, ...(computer.here ? { here: true } : {}) };
    for (const item of part.items) items.push({ ...item, computer: label });
    for (const bot of part.sources?.bots ?? []) if (!bots.some(each => each.id === bot.id)) bots.push({ id: bot.id, name: bot.name ?? null, ...(isObject(bot.look) ? { look: bot.look } : {}), computer: computer.key });
    you ||= part.sources?.you === true;
    more ||= part.more === true;
  }
  items.sort((a, b) => (Number(b.at) - Number(a.at)) || String(a.id).localeCompare(String(b.id)));
  return {
    items: items.slice(0, MOST),
    more: more || items.length > MOST,
    sources: { bots, you },
    computers: parts.map(({ computer, part: _, ...state }) => ({ key: computer.key, name: computer.name, ...(computer.here ? { here: true } : {}), ...state })),
  };
}

/** This computer's part as `Asked`. */
async function ownPart(query: string, signal?: AbortSignal | null, link = readLink()): Promise<Asked> {
  const computer = own(link);
  const response = await services("", query ? "?" + query : "", "GET", undefined, signal);
  const value = response?.ok ? await response.json().catch(() => null) : null;
  return isPart(value) ? { computer, state: "here", part: value } : { computer, state: "failed", message: servicesAway };
}

/** The query a list passes on: only what a part takes. */
function listQuery(request: LinkRequest): string {
  const given = new URLSearchParams(request.path.includes("?") ? request.path.slice(request.path.indexOf("?") + 1) : "");
  const query = new URLSearchParams();
  for (const key of ["source", "kind", "q", "hidden"]) {
    const value = given.get(key);
    if (value) query.set(key, value.slice(0, 200));
  }
  return query.toString();
}

function computerParam(request: LinkRequest): string | null {
  const at = request.path.indexOf("?");
  return at < 0 ? null : new URLSearchParams(request.path.slice(at + 1)).get("computer");
}

// `kuvo/library…` (PROTOCOL.md, "Library").
export async function* libraryRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, signal, json } = context;
  const method = request.method.toUpperCase();
  const tail = route.slice("kuvo/library".length);
  if (!tail || tail === "/events") {
    if (method !== "GET") return yield* json(405, { message: "Read it with GET." });
    const query = listQuery(request);
    if (!tail) {
      const [here, there] = await Promise.all([ownPart(query, signal), askPeers(query, signal)]);
      return yield* json(200, merged([here, ...there]));
    }
    return yield* followLibrary(query, signal);
  }
  // An item's: on this computer, or on the one it's on (`computer`).
  const key = computerParam(request);
  const link = readLink();
  const hides = tail.endsWith("/hide");
  if (hides !== (method === "POST")) return yield* json(405, { message: hides ? "Remove it with POST." : "Read it with GET." });
  let body: unknown;
  if (hides) {
    try { body = JSON.parse(Buffer.from(request.body ?? []).toString("utf8") || "{}"); } catch { return yield* json(400, { message: "Say whether it's removed (hidden)." }); }
  }
  if (!key || key === "here" || (link && key === ownKey(link))) {
    const response = await services(tail, "", method, body, signal);
    if (!response) return yield* json(503, { message: servicesAway });
    const headers: Record<string, string> = { "content-type": response.headers.get("content-type") ?? "application/json" };
    const bytes = Buffer.from(await response.arrayBuffer());
    headers["content-length"] = String(bytes.length);
    yield { head: { status: response.status, headers } };
    yield { body: bytes };
    return;
  }
  const peer = peersOf(link).find(each => each.key === key);
  if (!peer) return yield* json(404, { message: "That computer isn’t one of yours any more." });
  const computer = computerOf(peer);
  if (!takesForwarded(peer.key, LIBRARY_FEATURE)) return yield* json(409, { message: `${computer.name} needs a newer Kuvo Code to show its Library here.` });
  const answer = await askByMail(computer, { area: "library", method: method as "GET" | "POST", item: tail.slice(1), ...(hides ? { body: body as Record<string, unknown> } : {}) }, signal);
  if ("refused" in answer) return yield* json(answer.status, { message: answer.refused });
  const value = answer.value;
  if (answer.status === 200 && isObject(value) && typeof value.type === "string" && typeof value.data === "string") {
    const bytes = Buffer.from(value.data, "base64");
    yield { head: { status: 200, headers: { "content-type": value.type, "content-length": String(bytes.length) } } };
    yield { body: bytes };
    return;
  }
  return yield* json(answer.status, value);
}

/**
 * `kuvo/library/events`: the Library now, then again each time this computer's part changes (its
 * services follow it) or, asked again every `PEERS_AGAIN`, another computer's does.
 */
async function* followLibrary(query: string, signal?: AbortSignal | null): AsyncGenerator<AnswerPiece> {
  const stop = new AbortController();
  const ended = () => stop.abort();
  signal?.addEventListener("abort", ended, { once: true });
  let here: Asked | undefined;
  let there: Asked[] | undefined;
  // (Told before it's waited for: not lost.)
  let dirty = false;
  let changed: (() => void) | undefined;
  const tell = () => { dirty = true; const go = changed; changed = undefined; go?.(); };
  // This computer's part, as its services follow it.
  void (async () => {
    const response = await services("/events", query ? "?" + query : "", "GET", undefined, stop.signal);
    const link = readLink();
    if (!response?.ok || !response.body) {
      here = await ownPart(query, stop.signal, link);
      return tell();
    }
    let left = "";
    try {
      for await (const chunk of response.body) {
        left += Buffer.from(chunk).toString("utf8");
        let at: number;
        while ((at = left.indexOf("\n")) >= 0) {
          const line = left.slice(0, at);
          left = left.slice(at + 1);
          let value: unknown;
          try { value = JSON.parse(line); } catch { continue; }
          if (isPart(value)) {
            here = { computer: own(link), state: "here", part: value };
            tell();
          }
        }
      }
    } catch {}
  })();
  // Your other computers' parts, asked again now and then.
  void (async () => {
    while (!stop.signal.aborted) {
      there = await askPeers(query, stop.signal);
      tell();
      await new Promise<void>(done => {
        const timer = setTimeout(done, PEERS_AGAIN);
        stop.signal.addEventListener("abort", () => { clearTimeout(timer); done(); }, { once: true });
      });
    }
  })();
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson", "cache-control": "no-cache" } } };
  let last = "";
  try {
    while (!stop.signal.aborted) {
      if (!dirty) await new Promise<void>(done => {
        changed = done;
        stop.signal.addEventListener("abort", () => done(), { once: true });
      });
      dirty = false;
      if (stop.signal.aborted || !here || !there) continue;
      const line = JSON.stringify(merged([here, ...there]));
      if (line === last) continue;
      last = line;
      yield { body: Buffer.from(line + "\n") };
    }
  } finally {
    stop.abort();
    signal?.removeEventListener("abort", ended);
  }
}
