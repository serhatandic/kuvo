// The owning box's read side of a conversation addressed through another member.

import { StringDecoder } from "node:string_decoder";
import { isObject, parsed } from "../../shared/json";
import { sessionHere } from "../areas/glance";
import { acquireSessionEngine } from "../engine/lifetime";
import { targetsOf } from "../letters/tools";
import { awaySessions } from "../moves/records";
import { ownerOf, mayActivate } from "../ownership/resolve";
import { keptList } from "./sessions";
import type { AnswerPiece, RouteContext } from "../server/context";

const SESSION = "(ses_[A-Za-z0-9_-]{1,95})";
const RESOURCES = "(session|messages|events|queue(?:/events)?|asks(?:/events)?)";
export const PEER_CONVERSATION = new RegExp(`^kuvo/peer-session/([A-Za-z0-9_-]{43}|here)/${SESSION}/${RESOURCES}$`);
export const OWN_CONVERSATION = new RegExp(`^kuvo/session-read/${SESSION}/${RESOURCES}$`);
export const CONVERSATION_FEATURE = "line.peer-conversation";

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

  // An event subscription on a known runtime is passive, like the local event route. In
  // particular, reopening a stream to a parked engine must not wake it and keep it running.
  const known = keptList()?.sessions[id]?.place;
  if (resource === "events" && (known === "engine" || known === "bridge")) {
    const prefix = known === "bridge" ? "kuvo/agents/" : "";
    return yield* scopedEvents(read(prefix + "api/event"), id);
  }

  // Discover an unlisted conversation, or open it actively to read its page.
  const release = await acquireSessionEngine(id);
  let prefix: string;
  try {
    const agents = bridge();
    const targets = targetsOf(context);
    const place = await sessionHere(targets, id, stop).catch(() => undefined);
    if (place === undefined || (!place && !targets.length))
      return yield* json(503, { message: "The conversation’s engine isn’t answering." });
    if (!place) return yield* json(404, { message: "There’s no such conversation here." });
    prefix = place.target.port === agents?.port ? "kuvo/agents/" : "";
  } finally { release(); }

  if (resource === "events") return yield* scopedEvents(read(prefix + "api/event"), id);
  const base = prefix + `api/session/${id}`;
  if (resource === "session") return yield* read(base);
  const query = new URLSearchParams(request.path.split("?")[1]);
  return yield* read(base + "/message?" + query);
}
