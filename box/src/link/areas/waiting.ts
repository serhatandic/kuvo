// What waits on you in each area, across the box (PROTOCOL.md, "What waits on you", `kuvo/waiting`):
// the sessions "Needs you" lists, the bots with a session waiting, and the Work Queue's items that
// need you (`needsYouIn`). Counted from the box's session list (`lists/sessions.ts`), so
// every client's badges say the same, and none counts.

import { botsNow, waitingSessions, type BotCounts } from "../lists/rule";
import { keepsList, keptList, listFollowers, listNow, type Kept } from "../lists/sessions";
import path from "node:path";
import { botFollowers, botHere } from "../bots/here";
import { readSeen } from "../bots/seen";
import { followed, followersOf } from "../follow";
import { allItems, queueFollowers, WORK_QUEUE, type Item } from "../work-queue/store";
import { isClosed, NEEDS_YOU } from "../work-queue/statuses";
import type { SessionFacts, WaitingOnYou } from "../../shared/routes/sessions";
import type { AnswerPiece, RouteContext } from "../server/context";

export const WAITING_ROUTE = /^kuvo\/waiting(\/events)?$/;

const factsOf = (list: Kept | null): Record<string, SessionFacts> => Object.fromEntries(Object.entries(list?.sessions ?? {}).map(([id, entry]) => [id, entry.facts]));

/** What each bot here is doing now, by its id, as the session list has it (one with no session of
 * its own listed: nothing), and as you last saw each: what its sessions come to, which its state is
 * worked out from. */
export function botsNowHere(list: Kept | null = keptList(), seen: ReadonlyMap<string, number> = readSeen()): Record<string, BotCounts> {
  return botsNow(factsOf(list), seen);
}

/** What waits on you, as `list` has it: of the bots, only those that live here (a session here may
 * work for a bot on another computer, which is that computer's to count). */
export function waitingOf(list: Kept | null): WaitingOnYou {
  const facts = factsOf(list);
  // (As "Needs you" counts them: not archived, this box's own copy.)
  const asking = new Set(Object.entries(facts).filter(([, each]) => each.asked !== undefined && !each.archived && !each.away).map(([id]) => id));
  return {
    // Every session that waits, a bot's own conversation too: what a client's rows of them show.
    sessions: waitingSessions(facts),
    bots: Object.entries(botsNow(facts)).filter(([id, now]) => now.asking > 0 && botHere(id) !== null).length,
    queue: needsYouIn(allItems(), asking),
  };
}

/** How many of `items` need you (the Work Queue's one rule, PROTOCOL.md "What waits on you"): open
 * and not deleted, and either in the status Needs you (someone said so) or with a linked session
 * that `asking` has (its agent asks now, whatever its status still says). Each item once. */
export const needsYouIn = (items: readonly Item[], asking: ReadonlySet<string>): number =>
  items.filter(item => !item.removed && !isClosed(item.status) && (item.status === NEEDS_YOU || item.sessions.some(linked => asking.has(linked.session)))).length;

/** What of the list this counts from: looked at again only when it changed (the list changes with
 * every step of a run). */
const countedFrom = (list: Kept | null): string => {
  const facts = factsOf(list);
  // (And who asks at all, for the queue's count: a seat's ask is in neither of the others.)
  // (Of the bots, only who asks: what else they're doing changes often and counts for nothing here.)
  return JSON.stringify([waitingSessions(facts), Object.entries(botsNow(facts)).map(([id, now]) => [id, now.asking]), Object.keys(facts).filter(id => facts[id]!.asked !== undefined).sort()]);
};

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
  // A line as what it counts from changes: the list's "Needs you" and its bots' sessions, the
  // queues (this process's changes, and their files for another's), and the bots.
  const changed = new Set<() => void>();
  let last = countedFrom(keptList());
  const listTold = () => {
    const now = countedFrom(keptList());
    if (now === last) return;
    last = now;
    for (const tell of changed) tell();
  };
  listFollowers.add(listTold);
  try {
    return yield* followed(() => waitingOf(keptList()), followersOf(changed, queueFollowers, botFollowers), signal, { folders: [path.join(WORK_QUEUE, "queues")], recursive: false });
  } finally {
    listFollowers.delete(listTold);
  }
}
