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
import { listFollowers } from "../lists/sessions";
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
  const startingModel = await startingModelHere(kit.startingModel ?? null, readEngine, signal);
  return yield* json(200, botTemplates({ ...kit, startingModel }, info.name));
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
 * conversation and those working for it, at work and waiting on you. */
function botsWithNow(machine?: string) {
  const now = botsNowHere();
  return listBots(machine).map(bot => ({ ...bot, now: now[bot.id] ?? { working: 0, asking: 0 } }));
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
// added or sent), as a client shows them.
export async function* botEventsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, info, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // (Told as the session list changes only where what a bot is doing did: the list changes with
  // every step of a run, and listing the bots reads their folders.)
  const nowChanged = new Set<() => void>();
  let last = JSON.stringify(botsNowHere());
  const listTold = () => {
    const now = JSON.stringify(botsNowHere());
    if (now === last) return;
    last = now;
    for (const tell of nowChanged) tell();
  };
  listFollowers.add(listTold);
  try {
    return yield* followed(() => botsSaid(info.name), followersOf(botFollowers, nowChanged), signal, { folders: [BOTS, HELD] });
  } finally {
    listFollowers.delete(listTold);
  }
}

// This machine's bots and wake-ups, from their folders.
export async function* botsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, info, text } = context;
  if (!["GET", "HEAD"].includes(request.method.toUpperCase())) return yield* text(405, "Read them with GET.");
  yield { head: { status: 200, headers: { "content-type": "application/json" } } };
  yield { body: Buffer.from(JSON.stringify(botsSaid(info.name))) };
  return;
}
