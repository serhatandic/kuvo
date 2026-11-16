// What each bot here is doing now, as `kuvo/bots` and its events say it (`bots.now`, `bots.state`;
// PROTOCOL.md, "What a bot is doing now"): what its sessions come to (`botsNow`), the one state it's
// in (`botState`, the box's rule) and since when, as this process saw it change. The box's
// long-running process notes each bot's state as its sessions change (`followBotStates`), so
// `since` is when it changed, not when a client next asked.

import { botNow, botStateTurns, type BotCounts, type BotRest } from "../lists/rule";
import { listFollowers } from "../lists/sessions";
import { botsNowHere } from "../areas/waiting";
import { listBots } from "./list";
import { followSeen } from "./seen";
import type { BotNow, BotState } from "../../shared/routes/sessions";

/** How long the list stays as it is before a bot's state is noted from it (and at most how long
 * changes put that off): a run's end, its read and the box's own look at a bot's work come a moment
 * apart, and a state in between isn't one. */
export const STATE_SETTLES = 250;
export const STATE_SETTLES_AT_MOST = 1000;

const NOTHING: BotCounts = { working: 0, asking: 0 };

/** Each bot's state as last said here, since when, and what it was worked out with beside its
 * sessions (to say it again with no folder read). */
const told = new Map<string, { state: BotState; since: number; rest: BotRest }>();

/** `id`'s `now` at `at`, from what its sessions come to (`counts`) and `rest`; noted. */
export function nowOf(id: string, counts: BotCounts | undefined, rest: BotRest, at = Date.now()): BotNow {
  const now = botNow(counts ?? NOTHING, rest, at, told.get(id));
  told.set(id, { state: now.state, since: now.since, rest: { paused: rest.paused, ...(rest.wakes !== undefined ? { wakes: rest.wakes } : {}) } });
  return now;
}

/** Only `ids` kept (those listed now: a bot deleted is let go). */
export function keepOnly(ids: Iterable<string>): void {
  const kept = new Set(ids);
  for (const id of told.keys()) if (!kept.has(id)) told.delete(id);
}

/** Each bot said here before, as it is now from the session list (no folder read: as it was
 * besides when last listed). */
export function statesNow(counts: Record<string, BotCounts> = botsNowHere(), at = Date.now()): Record<string, BotNow> {
  const states: Record<string, BotNow> = {};
  for (const [id, { rest }] of told) states[id] = nowOf(id, counts[id], rest, at);
  return states;
}

/** When the first of `bots` may change state with nothing else changing, after `at` (ms). */
export function nextTurn(bots: Iterable<BotRest>, at = Date.now()): number | undefined {
  let next: number | undefined;
  for (const bot of bots) {
    const turns = botStateTurns(bot, at);
    if (turns !== undefined && (next === undefined || turns < next)) next = turns;
  }
  return next;
}

/** `then`, once the session list has stayed as it is a moment after it changes (`STATE_SETTLES`;
 * a second at most after the first change, however often it changes meanwhile). How to stop. */
export function afterListSettles(then: () => void): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let first: number | undefined;
  const settled = () => { timer = undefined; first = undefined; then(); };
  const told = () => {
    const now = performance.now();
    first ??= now;
    clearTimeout(timer);
    timer = setTimeout(settled, Math.max(0, Math.min(STATE_SETTLES, first + STATE_SETTLES_AT_MOST - now)));
    timer.unref?.();
  };
  listFollowers.add(told);
  return () => {
    listFollowers.delete(told);
    clearTimeout(timer);
  };
}

/** Each bot's state noted as the session list changes and as a bot is seen (the box's long-running
 * process), from its start: its bots read once now, so `since` is when each changed, whether or not
 * a client has asked yet. How to stop. */
export function followBotStates(): () => void {
  try {
    const counts = botsNowHere(), at = Date.now();
    for (const bot of listBots()) nowOf(bot.id, counts[bot.id], bot, at);
  } catch {}
  const note = () => { try { statesNow(); } catch {} };
  const stopList = afterListSettles(note);
  const stopSeen = followSeen(note);
  return () => { stopList(); stopSeen(); };
}
