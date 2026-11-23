// The owning box's read side of a conversation addressed through another member.

import { StringDecoder } from "node:string_decoder";
import { isObject, parsed } from "../../shared/json";
import { sessionHere } from "../areas/glance";
import { acquireSessionEngine } from "../engine/lifetime";
import { targetsOf } from "../letters/tools";
import { awaySessions } from "../moves/records";
import { ownerOf, mayActivate } from "../ownership/resolve";
import { createdRuntime, keptList } from "./sessions";
import { movedOf, ownKey, peersOf } from "../peers/known";
import { peerSetupLists } from "../peers/lists";
import { readLink } from "../state";
import type { AnswerPiece, RouteContext } from "../server/context";

const SESSION = "(ses_[A-Za-z0-9_-]{1,95})";
const RESOURCES = "(location|session|messages|events|queue(?:/events)?|asks(?:/events)?)";
export const PEER_CONVERSATION = new RegExp(`^kuvo/peer-session/([A-Za-z0-9_-]{43}|here)/${SESSION}/${RESOURCES}$`);
export const CONVERSATION_LOCATION = new RegExp(`^kuvo/sessions/${SESSION}/location$`);
export const OWN_CONVERSATION = new RegExp(`^kuvo/session-read/${SESSION}/${RESOURCES}$`);
export const CONVERSATION_FEATURE = "line.peer-conversation";

const gone = { message: "That conversation isn’t there any more." };
const notReady = { message: "The conversation’s engine isn’t answering." };

function localLocation(list: ReturnType<typeof keptList>, id: string) {
  const entry = list?.sessions[id];
  if (!entry) return list?.gone[id] ? undefined : createdRuntime(id);
  if (entry.facts.away) return;
  if (entry.place !== "engine" && entry.place !== "bridge") return;
  return { place: entry.place === "bridge" ? "agents" : "engine", running: entry.facts.running === true };
}

/** Locate a conversation from the local list and the lists already received over peer mail. */
export async function* conversationLocation(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  const id = match[1]!;
  const link = readLink();
  const journal = movedOf(link).get(id);
  const owner = ownerOf({ kind: "conversations", id }, { link, name: context.info.name });
  const list = keptList();
  const here = localLocation(list, id);
  if (here && owner?.local !== false && (!journal || (link && journal === ownKey(link))) && mayActivate({ kind: "conversations", id }, { link }))
    return yield* json(200, here);

  const peers = await peerSetupLists(link);
  const destination = journal ?? owner?.key;
  for (const { saved, origin } of peers) {
    if (destination && destination !== origin.computer.id) continue;
    const row = saved.sessions.find(row => row.id === id);
    if (!row) continue;
    const { computer, away, ...details } = origin;
    return yield* json(200, { on: { ...computer, away, ...details, actions: away ? [] : row.actions } });
  }

  // A move or an ownership announcement can arrive before its destination's list.
  const peer = peersOf(link).find(peer => peer.key === destination);
  if (peer) {
    const saved = peers.find(({ origin }) => origin.computer.id === peer.key)?.origin;
    const away = saved?.away ?? (owner?.key === peer.key ? !owner.reachable : true);
    return yield* json(200, { on: { id: peer.key, name: peer.name, away, actions: away ? [] : ["open"] } });
  }
  if (!list || list.pending?.length || list.sessions[id] || owner || journal) return yield* json(503, notReady);
  return yield* json(404, gone);
}

function eventFor(block: string, session: string): boolean {
  const lines = block.split(/\r?\n/);
  const body = lines.filter(line => line.startsWith("data:")).map(line => line.slice(5).trimStart()).join("\n");
  if (!body) return lines.every(line => line.startsWith(":"));
  const event = parsed(Buffer.from(body));
  if (!isObject(event)) return false;
  const data = isObject(event.data) ? event.data : isObject(event.properties) ? event.properties : {};
  const info = isObject(data.info) ? data.info : {};
  return (data.sessionID ?? info.id) === session;
}

async function* scopedEvents(stream: AsyncIterable<AnswerPiece>, session: string): AsyncGenerator<AnswerPiece> {
  const decoder = new StringDecoder("utf8");
  let success = false;
  let rest = "";
  for await (const piece of stream) {
    if ("head" in piece) {
      success = piece.head.status === 200;
      yield piece;
      continue;
    }
    if (!success) {
      yield piece;
      continue;
    }
    rest += decoder.write(piece.body);
    let end: RegExpExecArray | null;
    while ((end = /\r?\n\r?\n/.exec(rest))) {
      const block = rest.slice(0, end.index);
      rest = rest.slice(end.index + end[0].length);
      if (eventFor(block, session)) yield { body: Buffer.from(block + "\n\n") };
    }
    if (rest.length > 4 * 1024 * 1024) throw new Error("The conversation event is too large.");
  }
}

export async function* ownConversation(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { json, request, redispatch, signal, bridge } = context;
  const id = match[1]!, resource = match[2]!;
  if (request.method.toUpperCase() !== "GET" || !redispatch) return yield* json(405, { message: "Only GET." });
  const ref = { kind: "conversations" as const, id };
  const owner = ownerOf(ref);
  if (owner?.local === false || awaySessions().includes(id) || !mayActivate(ref))
    return yield* json(409, { message: "Open the conversation on its current computer." });
  const stop = signal ?? new AbortController().signal;
  const read = (path: string) => redispatch({ ...request, path }, stop);
  if (resource.startsWith("queue") || resource.startsWith("asks")) {
    return yield* read(`kuvo/sessions/${id}/${resource}${resource === "queue" ? "?items=1" : ""}`);
  }

  const list = keptList();
  const known = localLocation(list, id);
  if (resource === "location") {
    if (known) return yield* json(200, known);
    if (!list || list.pending?.length || list.sessions[id]) return yield* json(503, notReady);
    return yield* json(404, gone);
  }

  // Known conversations use the same runtime for pages and events. An event subscription is
  // passive, so reopening it to a parked engine does not wake it up.
  let prefix = known?.place === "agents" ? "kuvo/agents/" : "";
  if (!known) {
    const release = await acquireSessionEngine(id);
    try {
      const agents = bridge();
      const targets = targetsOf(context);
      const place = await sessionHere(targets, id, stop).catch(() => undefined);
      if (place === undefined || (!place && !targets.length)) return yield* json(503, notReady);
      if (!place) return yield* json(404, gone);
      prefix = place.target.port === agents?.port ? "kuvo/agents/" : "";
    } finally { release(); }
  }

  if (resource === "events") return yield* scopedEvents(read(prefix + "api/event"), id);
  const base = prefix + `api/session/${id}`;
  if (resource === "session") return yield* read(base);
  const query = new URLSearchParams(request.path.split("?")[1]);
  return yield* read(base + "/message?" + query);
}
