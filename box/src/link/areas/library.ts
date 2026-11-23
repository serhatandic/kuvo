// The Library across your computers (PROTOCOL.md, "Library"): what was handed over to you, which
// each computer's box keeps for itself (its services, beside the artifact record), gathered here so
// a client asks one box. This box's part comes from its services; each other computer's by mail (an
// act in the `library` area), labelled with the computer it's on. A computer that's away, or whose
// Kuvo Code is older and has no Library, is listed as such, and its part waits. Each other
// computer's part as last read is kept (`run/library/peers/<key>.json`), so the Library lists it at
// once, as of when it was seen, while that computer is asked or away (none only the very first time).

import fs from "node:fs";
import path from "node:path";
import { writePrivate } from "../../shared/files";

import { LIBRARY_COPY } from "../../shared/routes/library";
import { fitsShape, object, type Infer } from "../../shared/shape";
import { followedLive } from "../follow";
import { readPeerAgents } from "../peers/directory";
import { pendingComputers } from "../peers/pending";
import { peerSnapshots } from "../peers/lists";
import { filterOf } from "../library/routes";
import { matchesLibrary } from "../library/store";
import type { LinkHost } from "../relay/host";

import { readServices } from "../../shared/box-services";
import type { Target } from "../../shared/records";
import { isObject } from "../../shared/json";
import { IS_MAC, RUN } from "../home";
import { ownComputerName } from "../computer-name";
import { computerName } from "../peers/computer-name";
import { KEY, ownKey, peersOf, removedBoxes, type Peer } from "../peers/known";
import { standingOf } from "../peers/lists";
import { awaySince, readReach } from "../peers/reach";
import { offRelay } from "../letters/leave";
import { readLink, type LinkState } from "../state";
import { forwardAct, MEMORY_READ_FEATURE, takesForwarded, type Act, type Acted, type Forwarded } from "../work-queue/forward";
import type { Computer } from "../work-queue/view";
import type { AnswerPiece, LinkRequest, RouteContext } from "../server/context";

export const LIBRARY_FEATURE = "library";
const MERGED_LIBRARY_FEATURE = "line.merged-library";
export const MERGED_LIBRARY_ROUTE = "kuvo/setup/library/events";
const copyShape = object(LIBRARY_COPY);
type LibraryCopy = Infer<typeof copyShape>;
export const libraryCopies = peerSnapshots<LibraryCopy>("libraryCopy", "library-v1.json", MERGED_LIBRARY_FEATURE, LIBRARY_COPY);

let publishing = false;
export async function shareLibrary(host: Pick<LinkHost, "send">): Promise<void> {
  if (publishing) return;
  publishing = true;
  try {
    await libraryCopies.push(host, async () => {
      const reply = await services("/peer-copy", "", "GET", undefined, AbortSignal.timeout(5_000));
      const value: unknown = await reply?.json().catch(() => null);
      return fitsShape(copyShape, value) ? value as LibraryCopy : null;
    });
  } finally { publishing = false; }
}
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

/** One computer's part, as it was asked for; `seen` (ms): it's the copy last read then. */
export type Asked = { computer: Computer & { here?: true }; part?: Part; seen?: number } & PartState;

// MARK: Your other computers' parts as last read

/** Where your other computers' parts are kept as last read, one file each. */
export const LIBRARY_PEERS = path.join(RUN, "library", "peers");
/** How many lists a computer's copy keeps (the latest asked; a search with words is never kept). */
const MOST_KEPT = 8;
type Kept = { query: string; seen: number; part: Part };
const keptFile = (key: string) => path.join(LIBRARY_PEERS, `${key}.json`);

function keptOf(key: string): Kept[] {
  if (!KEY.test(key)) return [];
  try {
    const value: unknown = JSON.parse(fs.readFileSync(keptFile(key), "utf8"));
    return isObject(value) && Array.isArray(value.lists)
      ? value.lists.filter((each): each is Kept => isObject(each) && typeof each.query === "string" && typeof each.seen === "number" && isPart(each.part))
      : [];
  } catch { return []; }
}

/** A computer's part for `query` as last read, with when. */
export function keptPart(key: string, query: string): Kept | undefined {
  return keptOf(key).find(each => each.query === query);
}

function keep(key: string, query: string, part: Part, seen: number): void {
  if (!KEY.test(key) || new URLSearchParams(query).has("q")) return;
  const lists = [{ query, seen, part }, ...keptOf(key).filter(each => each.query !== query)].slice(0, MOST_KEPT);
  try { writePrivate(keptFile(key), JSON.stringify({ lists })); } catch {}
}

/** Forget the copies of computers removed from your setup. */
function forgetRemoved(link = readLink()): void {
  if (!link) return;
  for (const key of removedBoxes(link)) if (KEY.test(key)) try { fs.rmSync(keptFile(key), { force: true }); } catch {}
}

const lastSeen = (name: string) => `${name} isn’t answering now, so its part of the Library is as it was when last seen.`;

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
/** Whom `askPeers` asks, and how (a test's own). */
export type Asking = { peers?: Peer[]; ask?: AskPeer; has?: (key: string) => boolean; away?: (key: string) => boolean; now?: () => number };

const peersAsking = ({ peers = peersOf(readLink()), ask = askByMail, has = (key: string) => takesForwarded(key, LIBRARY_FEATURE), away = (key: string) => awaySince(key) !== null || offRelay(), now = Date.now }: Asking) => ({ peers, ask, has, away, now });

/** Your other computers' parts for `query`, each asked at once (one away or too old: said, not
 * asked). One away or not answering is its part as last read, with when (`seen`), where there is one. */
export async function askPeers(query: string, signal?: AbortSignal | null, asking: Asking = {}): Promise<Asked[]> {
  const { peers, ask, has, away, now } = peersAsking(asking);
  if (!asking.peers) forgetRemoved();
  return Promise.all(peers.map(async (peer): Promise<Asked> => {
    const computer = computerOf(peer);
    if (!has(peer.key)) return { computer, state: "older", message: `${computer.name} needs a newer Kuvo Code to show its Library here.` };
    const asKept = (state: "away" | "failed", message: string): Asked => {
      const kept = keptPart(peer.key, query);
      return kept ? { computer, state, message: lastSeen(computer.name), part: kept.part, seen: kept.seen } : { computer, state, message };
    };
    if (away(peer.key)) return asKept("away", `${computer.name} isn’t answering now, so its part of the Library isn’t shown.`);
    const answer = await ask(computer, { area: "library", method: "GET", item: "list", body: { query } }, signal);
    if ("refused" in answer) return answer.status === 409 ? { computer, state: "older", message: answer.refused } : asKept("away", answer.refused);
    if (answer.status !== 200 || !isPart(answer.value)) return asKept("failed", `${computer.name}’s Library couldn’t be read. Try again in a moment.`);
    keep(peer.key, query, answer.value, now());
    return { computer, state: "there", part: answer.value };
  }));
}

/** Your other computers' parts for `query` before they're asked: each as last read, with when
 * (`seen`); one away, said so; one too old, said so; one never read, left out until it answers. */
export function keptPeers(query: string, asking: Asking = {}): Asked[] {
  const { peers, has, away } = peersAsking(asking);
  return peers.flatMap((peer): Asked[] => {
    const computer = computerOf(peer);
    if (!has(peer.key)) return [{ computer, state: "older", message: `${computer.name} needs a newer Kuvo Code to show its Library here.` }];
    const kept = keptPart(peer.key, query);
    if (away(peer.key)) return [kept ? { computer, state: "away", message: lastSeen(computer.name), part: kept.part, seen: kept.seen } : { computer, state: "away", message: `${computer.name} isn’t answering now, so its part of the Library isn’t shown.` }];
    return kept ? [{ computer, state: "there", part: kept.part, seen: kept.seen }] : [];
  });
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

/** The desktop's topic reads peer copies before publishing any state. No mail round is needed
 * to open another tab, switch bots or search. The legacy cache can seed queries it already knew. */
export async function* mergedLibraryRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Read it with GET." });
  const query = listQuery(request);
  const filter = filterOf(new URLSearchParams(query));
  if (typeof filter === "string") return yield* json(400, { message: filter });
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  yield* followedLive(async () => {
    const link = readLink();
    const agents = readPeerAgents();
    const reach = readReach();
    const [here, remote] = await Promise.all([
      ownPart(query, signal, link),
      Promise.all(peersOf(link).map(async (peer): Promise<Asked | undefined> => {
        const computer = computerOf(peer);
        const copy = await libraryCopies.read(peer.key);
        const legacy = copy ? undefined : keptPart(peer.key, query);
        const kept = copy ?? legacy;
        const now = Date.now();
        const aged = copy && now - copy.seen > 180_000;
        // (Away as its rows say it, `standingOf`: not under a minute, nor while it only restarts
        // into a new release; its part as last read meanwhile.)
        const quiet = awaySince(peer.key, reach) !== null || Boolean(kept && offRelay()) || Boolean(aged);
        const away = standingOf(peer.key, quiet, kept?.seen, reach, agents, now).away;
        if (!kept) {
          const features = agents[peer.key]?.features;
          if (away) return { computer, state: "away", message: `${computer.name} isn’t answering now, so its part of the Library isn’t shown.` };
          if (features && !features.includes(MERGED_LIBRARY_FEATURE)) return { computer, state: "older", message: `${computer.name} needs a newer Kuvo Code to show its Library here.` };
          return undefined;
        }
        const part: Part | undefined = copy ? {
          items: copy.items.filter(matchesLibrary(filter, id => copy.botNames[id], computer.name)),
          sources: { bots: [...copy.sources.bots], you: copy.sources.you },
        } : legacy?.part;
        if (!part) return undefined;
        if (away) return { computer, state: "away", seen: kept.seen, message: lastSeen(computer.name),
          part: { ...part, items: part.items.map(item => ({ ...item, there: false, thumbnail: false })) } };
        return { computer, state: "there", part };
      })),
    ]);
    const known = remote.filter(part => part !== undefined);
    const pending = pendingComputers(MERGED_LIBRARY_FEATURE, key => known.some(part => part.computer.key === key), link);
    const firstPeerCopy = known.some(part => part.part);
    if (!pending.length && !here.part && !firstPeerCopy) return undefined;
    const state = merged([here, ...known]);
    return { ...state, ...(pending.length && !firstPeerCopy ? { items: [], more: false } : {}), pendingComputers: pending };
  }, tell => {
    libraryCopies.followers.add(tell);
    return () => { libraryCopies.followers.delete(tell); };
  }, signal, { every: 3_000 });
}

/**
 * `kuvo/library/events`: the Library now, then again each time this computer's part changes (its
 * services follow it) or, asked again every `PEERS_AGAIN`, another computer's does.
 */
export async function* followLibrary(query: string, signal?: AbortSignal | null, asking: Asking = {}): AsyncGenerator<AnswerPiece> {
  const stop = new AbortController();
  const ended = () => stop.abort();
  signal?.addEventListener("abort", ended, { once: true });
  let here: Asked | undefined;
  // (Your other computers' parts as last read, at once, while they're asked.)
  let there: Asked[] | undefined = keptPeers(query, asking);
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
      there = await askPeers(query, stop.signal, asking);
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
