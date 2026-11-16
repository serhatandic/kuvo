// This box's bots: listed and followed, made, changed and deleted as the Mac's New Bot and Edit
// Bot do, brought here from another computer, and told what they are now.

import { parsed } from "../../shared/json";
import { changeBot, deleteBot } from "../bots/edit";
import { botFollowers, botHere } from "../bots/here";
import { HELD, heldLists, holdEdit } from "../bots/held";
import { botsElsewhere, elsewhereSaid, knownOf } from "../bots/elsewhere";
import { botTemplates, NO_KIT, readKit } from "../bots/kit";
import { listBots } from "../bots/list";
import { makeBot, startingModelHere } from "../bots/make";
import { freshenBots, settleBot } from "../bots/tell";
import { followed, followersOf } from "../follow";
import { listNow, noteViewed } from "../lists/sessions";
import { afterListSettles, keepOnly, nextTurn, nowOf, statesNow } from "../bots/now";
import { call } from "../engine/client";
import { botsNowHere } from "./waiting";
import { BOTS } from "../home";
import type { Target } from "../../shared/records";
import type { BotChange } from "../bots/edit";
import type { GivenBot } from "../bots/make";
import type { AnswerPiece, RouteContext } from "../server/context";

export async function* botTemplatesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, readEngine, info, json } = context;
  // Read with GET or HEAD, as every box's lists are.
  if (!["GET", "HEAD"].includes(request.method.toUpperCase())) return yield* json(405, {});
  const kit = readKit();
  if (!kit) return yield* json(409, { message: NO_KIT });
  // (By the box's one rule: no client names one here.)
  const startingModel = await startingModelHere(null, readEngine, signal);
  return yield* json(200, botTemplates(kit, info.name, startingModel));
}

// New bots, made here as New Bot makes them on the Mac, which takes them in when it next connects.
export async function* newBotRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, info, json } = context;
  const { status, value } = await makeBot(parsed(request.body) as GivenBot | null | undefined, readEngine, bridge, info.name);
  return yield* json(status, value);
}

// A bot here changed as Edit Bot changes it, or deleted as Delete Bot… deletes it.
export async function* botRoute(context: RouteContext, oneBot: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, readEngine, bridge, info, json } = context;
  let id: string;
  try {
    id = decodeURIComponent(oneBot[1]);
  } catch {
    return yield* json(404, { message: "That bot isn’t there any more." });
  }
  const deleting = request.method.toUpperCase() === "DELETE";
  // One that lives on another of your computers: held here for it, and handed on once it answers
  // ("Held for another computer"). `on` names that computer for one this box hasn't heard of yet.
  if (!botHere(id)) {
    const query = new URLSearchParams(request.path.split("?")[1] ?? "");
    const held = holdEdit(id, deleting ? { delete: true } : { change: parsed(request.body) }, query.get("on"), { removed: query.has("removed") });
    return yield* json(held.status, held.value);
  }
  const { status, value } = !deleting
    ? await changeBot(id, parsed(request.body) as BotChange | null | undefined, readEngine, bridge, info.name, signal)
    : await deleteBot(id, readEngine, bridge);
  return yield* json(status, value);
}

// A bot brought here from another computer, made this box's (Move to Computer).
export async function* arrivedRoute(context: RouteContext, arrived: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, info, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  let id: string;
  try { id = decodeURIComponent(arrived[1]); } catch { return yield* json(404, { message: "That bot isn’t there any more." }); }
  const { status, value } = await settleBot(id, readEngine, bridge, info.name);
  return yield* json(status, value);
}

// Its bots told what they are now where that changed, as its app asks after a change to its kit
// (your personalities, on a Mac).
export async function* retellRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, readBridge, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  for (const read of [readEngine, readBridge]) {
    let target: Target | null = null;
    try { target = read(); } catch {}
    if (target?.port) await freshenBots(target);
  }
  return yield* json(200, {});
}

/** Its bots, each with what it's doing now (`now`), as the session list has its sessions: its
 * conversation and those working for it, at work and waiting on you, and the one state it's in. */
function botsWithNow(machine?: string) {
  const counts = botsNowHere();
  const at = Date.now();
  const bots = listBots(machine);
  keepOnly(bots.map(bot => bot.id));
  return bots.map(bot => ({ ...bot, now: nowOf(bot.id, counts[bot.id], bot, at) }));
}

/** What `kuvo/bots` and its events say: this box's bots, what it holds for your other computers and
 * what they refused, and the bots that live on them, as each last told this box. */
function botsSaid(machine?: string) {
  const listed = botsElsewhere();
  const lists = heldLists(undefined, undefined, knownOf(listed));
  // (None of the three while it can't check what it holds: a client keeps what it last heard.)
  return { bots: botsWithNow(machine), ...lists, ...(lists.held && lists.refused ? { elsewhere: elsewhereSaid(listed, lists.held, lists.refused) } : {}) };
}

// Its bots followed: a line now, and again as they change (made, changed, deleted, a wake-up
// added or sent, what one is doing), as a client shows them.
export async function* botEventsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, info, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // (Told as the session list changes only where what a bot is doing did, or the state it's in:
  // the list changes with every step of a run, and listing the bots reads their folders. Once the
  // list settles: a run's end and what follows it a moment after are one change.)
  const nowChanged = new Set<() => void>();
  const tellAll = () => { for (const tell of nowChanged) tell(); };
  const doing = () => { const counts = botsNowHere(); return JSON.stringify([counts, statesNow(counts)]); };
  let last = doing();
  const stopFollowing = afterListSettles(() => {
    const now = doing();
    if (now === last) return;
    last = now;
    tellAll();
  });
  // A bot whose next wake-up comes within 12 hours with nothing else changing is no longer asleep:
  // told then (at the latest a day on, as each line is worked out again).
  let turning: ReturnType<typeof setTimeout> | undefined;
  const said = () => {
    const bots = botsSaid(info.name);
    clearTimeout(turning);
    const at = Date.now();
    const next = nextTurn(bots.bots, at);
    if (next !== undefined) {
      turning = setTimeout(tellAll, Math.min(next - at + 1, 24 * 3600_000));
      turning.unref?.();
    }
    return bots;
  };
  try {
    return yield* followed(said, followersOf(botFollowers, nowChanged), signal, { folders: [BOTS, HELD] });
  } finally {
    stopFollowing();
    clearTimeout(turning);
  }
}

// `POST kuvo/bots/<id>/seen`: what the bot did that you haven't read read, as a client shows it to
// you (its conversation opened, its row looked at): each of its sessions (its conversation, and
// those working for it, as its `now` counts them) with a turn you haven't read, neither at work nor
// waiting on you, read up to where it ended, as `POST api/session/<id>/view` reads one. So it's no
// longer done, nor blocked by a stop you've now seen, on every client (each hears `session.viewed`).
export async function* botSeenRoute(context: RouteContext, seen: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, readEngine, bridge, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  let id: string;
  try { id = decodeURIComponent(seen[1]!); } catch { return yield* json(404, { message: "That bot isn’t there any more." }); }
  if (!botHere(id)) return yield* json(404, { message: "That bot isn’t there any more." });
  const list = await listNow({ readEngine, bridge }, signal ?? undefined);
  if (!list) return yield* json(503, { message: "The engine isn’t running." });
  let engine: Target | null = null;
  try { engine = readEngine(); } catch {}
  const places: Record<string, Target | null> = { engine, bridge: bridge() };
  const unread = Object.entries(list.sessions).filter(([, { facts }]) =>
    facts.bot === id && !facts.archived && !facts.away && !facts.running && facts.asked === undefined && facts.unread !== undefined);
  const read = await Promise.all(unread.map(async ([session, { facts, place = "engine" }]) => {
    const target = places[place];
    // (Where it lives isn't running (the coding agents' bridge): nothing to read it on now, and
    // nothing to ask again for.)
    if (!target) return true;
    try {
      await call(target, "POST", `/api/session/${encodeURIComponent(session)}/view`, { idle: facts.unread }, AbortSignal.timeout(10_000));
      // (Taken in here at once: its `session.viewed` says the same a moment later.)
      noteViewed(place, session, facts.unread!);
      return true;
    } catch {
      return false;
    }
  }));
  // (One that couldn't be read: said, for a client to ask again.)
  if (read.includes(false)) return yield* json(502, { message: "Some of what it did couldn’t be marked as seen just now. Try again in a moment." });
  return yield* json(200, {});
}

// This machine's bots and wake-ups, from their folders.
export async function* botsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, info, text } = context;
  if (!["GET", "HEAD"].includes(request.method.toUpperCase())) return yield* text(405, "Read them with GET.");
  yield { head: { status: 200, headers: { "content-type": "application/json" } } };
  yield { body: Buffer.from(JSON.stringify(botsSaid(info.name))) };
  return;
}
