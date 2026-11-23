// Live, read-only conversation follows over authenticated peer mail. The owning box serves the
// allowlisted resource. Each subscription has a lease, so a vanished subscriber leaves no stream.

import { randomUUID } from "node:crypto";
import { isObject } from "../../shared/json";
import { ownKey, mailFor, peersOf, type MailSender, type Peer } from "./known";
import { fromLocalApp, type AnswerPiece, type Asker, type RouteContext } from "../server/context";
import { readSubmitter, pairedSubmitter } from "../ownership/authority";
import type { Submitter } from "../ownership/submitter";
import { deviceConnectionsAllowed, pairedPhone } from "../phones";
import type { LinkHost } from "../relay/host";
import { liveLink, readLink } from "../state";
import type { LinkState } from "../state";
import { CONVERSATION_FEATURE, OWN_CONVERSATION } from "../lists/conversation";
import { takesForwarded } from "../work-queue/forward";
import { keepConversation, keptConversation } from "./kept-conversations";
import { peerStanding } from "./lists";
import { askReachAgain, tryingSince } from "./reach-again";
import { awaySince, unreachableFollowers } from "./reach";

const ID = /^[a-f0-9-]{36}$/;
const LEASE = 35_000;
const RENEW = 10_000;
const MAX_STREAMS = 128;
const CHUNK = 32_000;
const MAX_BUFFER = 4 * 1024 * 1024;
/** A newest page carried larger than this isn't kept. */
const MOST_KEPT_PAGE = 16 * 1024 * 1024;

type Host = Pick<LinkHost, "send" | "welcomed" | "unsent">;
type Client = Extract<Submitter, { kind: "computer" | "paired" }>;
type Response = { sequence: number; status?: number; type?: string; body?: string; end?: true; beat?: true };
type Read = { resource: string; client: Client };

function tell(host: Host, peer: Peer, link: LinkState, id: string, contents: object): boolean {
  try {
    const bytes = mailFor(peer, { at: Date.now(), sessionFollow: { id, ...contents } }, link);
    return Boolean(bytes && host.send(bytes) !== false);
  } catch { return false; }
}

function validActor(link: LinkState | null, client: Client): Asker | false | undefined {
  if (!deviceConnectionsAllowed(link)) return false;
  if (client.kind === "computer") return undefined;
  const phone = pairedPhone(link, client.key, client.pairing);
  return phone ? { key: Buffer.from(client.key, "base64url"), pairing: client.pairing, name: phone.name, kind: phone.kind === "desktop" ? "desktop" : undefined } : false;
}

function readOf(value: unknown): Read | null {
  if (!isObject(value) || typeof value.resource !== "string" || value.resource.length > 2048 ||
    !OWN_CONVERSATION.test(value.resource.split("?")[0]!)) return null;
  const client = readSubmitter(value.client);
  return !client || client.kind === "ssh" ? null : { resource: value.resource, client };
}

interface Receiving {
  peer: string;
  accept: (part: Record<string, unknown>) => void;
}
const receiving = new Map<string, Receiving>();
const serving = new Map<string, { keep(): void; stop(): void }>();

const unavailable = { message: "The conversation’s computer isn’t answering." };

/** The newest page kept of `session`, said as its computer's last copy while it's away. */
async function* lastCopy(context: RouteContext, peer: Peer, session: string): AsyncGenerator<AnswerPiece> {
  const kept = await keptConversation(peer.key, session);
  // (When it was last seen only once it's away, a minute on: switching to a new release, it's updating.)
  const { away, lastSeen, updating } = await peerStanding(peer.key);
  const seen = away ? Math.max(kept?.at ?? 0, lastSeen ?? 0) : 0;
  yield* context.json(200, { data: kept?.data ?? [], away: {
    computer: { id: peer.key, name: peer.name }, ...(seen ? { lastSeen: seen } : {}), ...(updating ? { updating } : {}),
  } });
}

export async function* peerConversation(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, json, phone, signal } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  const [, computer, id, resource] = match;
  const query = request.path.includes("?") ? "?" + request.path.split("?").slice(1).join("?") : "";
  const ownPath = `kuvo/session-read/${id}/${resource}${query}`;
  const link = readLink();
  if (computer === "here" || (link && ownKey(link) === computer))
    return yield* context.redispatch!({ ...request, path: ownPath }, signal ?? new AbortController().signal);
  const client: Client | undefined = phone?.key ? pairedSubmitter(context, link)
    : fromLocalApp(request) ? { kind: "computer" } : undefined;
  if (!client) return yield* json(403, { message: "Open the conversation from Kuvo Code." });
  const peer = peersOf(link).find(each => each.key === computer);
  if (!link || !peer) return yield* json(404, { message: "There’s no such computer in this setup." });
  if (!takesForwarded(peer.key, CONVERSATION_FEATURE))
    return yield* json(503, { message: "Update Kuvo Code on that computer to follow it from here." });
  // The newest page is kept as it's carried, and answered from there while its computer is away.
  const newest = resource === "messages" && !new URLSearchParams(query.slice(1)).has("cursor");
  const host = liveLink();
  if (newest && (!host?.welcomed || awaySince(peer.key) !== null)) {
    // (Asked again meanwhile, so the next read finds it as soon as it answers.)
    if (host?.welcomed && tryingSince(peer.key) === null) askReachAgain(peer.key);
    return yield* lastCopy(context, peer, id!);
  }
  if (!host?.welcomed || receiving.size >= MAX_STREAMS) return yield* json(503, unavailable);

  const subscription = randomUUID();
  const queue: AnswerPiece[] = [];
  let wake: (() => void) | undefined;
  let ended = false;
  let sequence = 0;
  let received = false;
  let bytes = 0;
  let heard = Date.now();
  // The newest page as it's carried, to keep once it's whole.
  let copy = null as Buffer[] | null;
  let copied = 0;
  let whole = false;
  const finish = () => { ended = true; wake?.(); };
  receiving.set(subscription, { peer: peer.key, accept(part) {
    if (ended || part.sequence !== sequence++) return finish();
    heard = Date.now();
    if (!received && typeof part.status === "number" && typeof part.type === "string") {
      // (Its engine not answering yet, as just after it starts: the same as the computer away.)
      if (newest && part.status >= 500) return finish();
      received = true;
      if (newest && part.status === 200) copy = [];
      queue.push({ head: { status: part.status, headers: { "content-type": part.type } } });
    }
    if (typeof part.body === "string") {
      const body = Buffer.from(part.body, "base64");
      bytes += body.length;
      if (bytes > MAX_BUFFER) return finish();
      if (copy && (copied += body.length) <= MOST_KEPT_PAGE) copy.push(body);
      else copy = null;
      queue.push({ body });
    }
    if (part.end === true) {
      whole = true;
      finish();
    }
    wake?.();
  } });
  // The relay found nothing there: what's kept is answered now, not when the lease runs out.
  const away = (key: string) => { if (key === peer.key && !received) finish(); };
  unreachableFollowers.add(away);
  const stillAllowed = () => {
    const current = readLink();
    return liveLink() === host && host.welcomed && current?.key === link.key &&
      current?.setup?.id === link.setup?.id && peersOf(current).some(each => each.key === peer.key) &&
      validActor(current, client) !== false;
  };
  const keep = setInterval(() => {
    if (!stillAllowed() || Date.now() - heard >= LEASE || !tell(host, peer, link, subscription, { keep: true })) finish();
  }, RENEW);
  signal?.addEventListener("abort", finish, { once: true });
  try {
    if (signal?.aborted || !tell(host, peer, link, subscription, { open: { resource: ownPath, client } })) finish();
    while (!signal?.aborted && (queue.length || !ended)) {
      if (queue.length) {
        const piece = queue.shift()!;
        if ("body" in piece) bytes -= piece.body.length;
        yield piece;
      } else {
        await new Promise<void>(done => { wake = done; });
        wake = undefined;
      }
    }
    // (Kept before the answer ends, so the next read finds it.)
    if (whole && copy) await keepConversation(peer.key, id!, Buffer.concat(copy)).catch(() => {});
    if (!received && !signal?.aborted) yield* newest ? lastCopy(context, peer, id!) : json(503, unavailable);
  } finally {
    receiving.delete(subscription);
    unreachableFollowers.delete(away);
    clearInterval(keep);
    signal?.removeEventListener("abort", finish);
    if (host.welcomed) tell(host, peer, link, subscription, { stop: true });
  }
}

export function takeSessionFollow(sender: MailSender, mail: Record<string, unknown>, host: Host | null | undefined): void {
  const { id } = mail;
  if (!host?.welcomed || typeof id !== "string" || !ID.test(id)) return;
  if (isObject(mail.part)) {
    const follow = receiving.get(id);
    if (follow?.peer === sender.key) follow.accept(mail.part);
    return;
  }
  const link = readLink();
  const peer = peersOf(link).find(each => each.key === sender.key);
  if (!link || !peer) return;
  const key = sender.key + ":" + id;
  const existing = serving.get(key);
  if (mail.stop === true) return existing?.stop();
  if (mail.keep === true) return existing?.keep();
  if (existing) return;
  const read = readOf(mail.open);
  if (!read) return;
  const actor = validActor(link, read.client);
  if (actor === false || serving.size >= MAX_STREAMS) {
    const status = actor === false ? 403 : 503;
    const body = Buffer.from(JSON.stringify({ message: "This conversation can’t be followed from here." })).toString("base64");
    tell(host, peer, link, id, { part: { sequence: 0, status, type: "application/json", body, end: true } });
    return;
  }

  const end = new AbortController();
  let renewed = Date.now();
  let sequence = 0;
  let checked = -Infinity;
  const live = () => {
    if (end.signal.aborted || !host.welcomed) return false;
    const now = performance.now();
    if (now - checked < 1000) return true;
    const current = readLink();
    if (current?.key !== link.key || current?.setup?.id !== link.setup?.id ||
      !peersOf(current).some(each => each.key === peer.key) || validActor(current, read.client) === false) {
      end.abort();
      return false;
    }
    checked = now;
    return true;
  };
  const send = (part: Omit<Response, "sequence">) => {
    if ((host.unsent?.() ?? 0) > MAX_BUFFER || !tell(host, peer, link, id, { part: { sequence: sequence++, ...part } })) end.abort();
  };
  const state = { keep: () => { renewed = Date.now(); }, stop: () => end.abort() };
  serving.set(key, state);
  const clock = setInterval(() => {
    if (!live() || Date.now() - renewed >= LEASE) end.abort();
    else send({ beat: true });
  }, RENEW);
  void (async () => {
    try {
      const { engineHandler } = await import("../server/router");
      const answer = engineHandler()({ method: "GET", path: read.resource, headers: {},
        desktop: actor === undefined || actor.kind === "desktop", fromOwnerPeer: true }, actor, end.signal);
      for await (const piece of answer) {
        if (!live()) break;
        if ("head" in piece) send({ status: piece.head.status, type: piece.head.headers["content-type"] ?? "application/json" });
        else for (let at = 0; at < piece.body.length && !end.signal.aborted; at += CHUNK)
          send({ body: piece.body.subarray(at, at + CHUNK).toString("base64") });
      }
    } catch {
      // A bad source ends this opening. The line reopens the source and signals any event gap.
    } finally {
      clearInterval(clock);
      serving.delete(key);
      if (host.welcomed) send({ end: true });
    }
  })();
}
