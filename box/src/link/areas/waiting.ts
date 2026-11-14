// What waits on you in each area, across the box (PROTOCOL.md, "What waits on you", `kuvo/waiting`):
// the sessions "Needs you" lists, the bots with a session waiting, and the Work Queue's items a
// session linked to them waits for. Counted from the box's session list (`lists/sessions.ts`), so
// every client's badges say the same, and none counts.

import { botsNow } from "../lists/rule";
import { keepsList, keptList, listFollowers, listNow, type Kept } from "../lists/sessions";
import { botFollowers } from "../bots/here";
import { followed, followersOf } from "../follow";
import { allItems, queueFollowers } from "../work-queue/store";
import type { BotNow, SessionFacts, WaitingOnYou } from "../../shared/routes/sessions";
import type { AnswerPiece, RouteContext } from "../server/context";

export const WAITING_ROUTE = /^kuvo\/waiting(\/events)?$/;

const factsOf = (list: Kept | null): Record<string, SessionFacts> => Object.fromEntries(Object.entries(list?.sessions ?? {}).map(([id, entry]) => [id, entry.facts]));

/** What each bot here is doing now, by its id, as the session list has it (one with no session of
 * its own listed: nothing). */
export function botsNowHere(list: Kept | null = keptList()): Record<string, BotNow> {
  return botsNow(factsOf(list));
}

/** What waits on you, as `list` has it. */
export function waitingOf(list: Kept | null): WaitingOnYou {
  const facts = factsOf(list);
  const asking = new Set(Object.entries(facts).filter(([, each]) => each.asked !== undefined).map(([id]) => id));
  return {
    sessions: (list?.needsYou ?? []).length,
    bots: Object.values(botsNow(facts)).filter(now => now.asking > 0).length,
    // (The queues are read only while something waits at all: this is asked as the list changes.)
    queue: asking.size ? allItems().filter(item => !item.removed && item.category !== "done" && item.sessions.some(linked => asking.has(linked.session))).length : 0,
  };
}

export async function* waitingRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  // (The list read first where this process hasn't yet; none while the engine can't be read: zeros
  // then, as nothing is known to wait.)
  const list = await listNow(context, signal ?? undefined);
  if (!match[1]) return yield* json(200, waitingOf(list));
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // (Followed only where the list is kept: elsewhere its first line, as `box route` answers.)
  if (!keepsList()) return yield { body: Buffer.from(JSON.stringify(waitingOf(list)) + "\n") };
  return yield* followed(() => waitingOf(keptList()), followersOf(listFollowers, queueFollowers, botFollowers), signal);
}
