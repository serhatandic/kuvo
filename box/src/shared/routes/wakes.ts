// Wake-ups and watches (`kuvo/wakes`): a conversation's or a bot's, as the bar over
// the Mac's composer shows them.

import { bool, type Infer, list, NOTHING, number, object, type RouteShape, string } from "../shape";

/** A wake-up waiting: when it's due (seconds since 1970), what it says, and how often it comes
 * back (left out for one that doesn't). */
export const WAKE = object({ id: string, due: number, note: string, "every?": string, "summary?": string, "cancelKey?": string });
/** A watch: its letters, how often it looks, the check it runs and what to do when that changes. */
export const WATCH = object({ letters: string, every: string, check: string, note: string, "summary?": string, "next?": number });
/** A conversation's wake-ups (soonest first), its watches, and whether you paused them. */
export const WAKES = object({ wakes: list(WAKE), watches: list(WATCH), paused: bool });
/** Each conversation here with some: its soonest wake-up, and whether it watches; and, from a
 * newer box, its wake-ups, watches and pause as `kuvo/wakes/:session` says them, so a client
 * reads them all at once (an older box leaves them out). */
export const WAKE_LIST = object({
  sessions: list(object({ session: string, "next?": number, watching: bool, "wakes?": list(WAKE), "watches?": list(WATCH), "paused?": bool })),
});

export type Wake = Infer<typeof WAKE>;
export type Watch = Infer<typeof WATCH>;
export type Wakes = Infer<typeof WAKES>;
export type WakeList = Infer<typeof WAKE_LIST>;

export const WAKE_ROUTES: RouteShape[] = [
  // Every conversation here with wake-ups or watches: its soonest, and its wake-ups, watches and pause.
  { route: "GET kuvo/wakes", feature: "wakes", answers: { 200: WAKE_LIST } },
  { route: "GET kuvo/wakes/events", feature: "wakes.events", answers: { 200: WAKE_LIST } },
  { route: "GET kuvo/wakes/:session", feature: "wakes", answers: { 200: WAKES } },
  { route: "GET kuvo/wakes/:session/events", feature: "bots.wake-ups", answers: { 200: WAKES } },
  // Add a wake-up (`due`, `note`, `every?`), at least a minute away.
  { route: "POST kuvo/wakes/:session", feature: "wakes.edit", answers: { 200: WAKES } },
  // Make a watch: a shell `check` run every so often, and a `note` for when its output changes.
  { route: "POST kuvo/wakes/:session/watch", feature: "wakes.events", answers: { 200: WAKES } },
  // Pause or let go of a conversation's wake-ups (`paused`), or hold them as archiving does.
  { route: "POST kuvo/wakes/:session/pause", feature: "wakes.edit", answers: { 200: WAKES } },
  // Cancel one wake-up (`wake`) or watch (`watch`).
  { route: "POST kuvo/wakes/:session/cancel", feature: "wakes.edit", answers: { 200: NOTHING } },
  // Send one wake-up now; a repeating one still comes back at its time.
  { route: "POST kuvo/wakes/:session/now", feature: "wakes.edit", answers: { 200: NOTHING } },
];
