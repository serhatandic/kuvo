// The Work Queue (PROTOCOL.md, "The Work Queue"): an owner's view and its events, one item whole,
// what `#n` refs name, the statuses, priorities and labels, and what you do from a client. Times
// are milliseconds since 1970.

import { any, bool, BYTES, type Infer, list, map, nullable, number, object, oneOf, type RouteShape, string, words } from "../shape";

// (You, an agent, or another shape kept as it came: an outside tracker's change, later.)
const WHO = oneOf(object({ user: bool }), object({ session: string, name: string }), map(any));
const STAMP = object({ at: number, by: WHO });
const NOTE = object({ at: number, by: WHO, text: string });
/** Something that happened to an item: `started`, `stopped`, `moved` (`from`, `to` statuses, `top`), `done`; a
 * newer box may say other kinds, which a client leaves out. */
const EVENT = object({ at: number, by: WHO, kind: string, "from?": string, "to?": string, "top?": bool });
const PROGRESS = object({ "value?": number, "note?": string });

/** An outside source's item (an issue or a pull request), named by its ref, never numbered; `name`
 * is what a client shows (`kuvo-ai#12`). */
const SOURCE = object({ kind: string, ref: string, "url?": string, "host?": string, "id?": string, "name?": string });
/** Where its issue or pull request stands on GitHub, as last read, and since when: open, draft,
 * completed, not_planned, merged or closed (a newer box may say more); `gone` when it couldn't be
 * read since (Done then, its mark drawn dimmed); `checked`, when another computer last asked GitHub
 * of it. */
const SEEN = object({ state: string, at: number, "checked?": number, "gone?": bool });
/** Another of your computers, the one an item is on. */
const COMPUTER = object({ key: string, name: string, kind: words("mac", "machine"), "id?": string });

/** A status, as the box lists them, in order; `closed` for Done and Canceled. */
export const QUEUE_STATUS = object({ id: string, name: string, closed: bool });
export const QUEUE_PRIORITY = object({ id: string, name: string });
/** A label; `color` one of the looks' names (graphite for one a client doesn't know). */
export const QUEUE_LABEL = object({ id: string, name: string, color: string });
/** Who an item is, or may be, for: a bot (`look`, its face, in the list of those it may be for) or a
 * coding agent. */
const ASSIGNEE = object({ kind: words("bot", "agent"), id: string, name: string, "look?": any });
/** What whoever asked may do to an item (a client's view and answers; not what's told between
 * computers): change its title and description; where it stands among the open statuses, its
 * priority, labels, assignee, due day and place; close it (Done, Canceled); delete it; bring it back. */
const CAN = object({ edit: bool, set: bool, assign: bool, close: bool, delete: bool, restore: bool, "why?": string });

/** An item as a list shows it: everything but its body and notes (how many, and the last). */
export const QUEUE_ITEM = object({
  id: string,
  "number?": number,
  "wanted?": number,
  owner: string,
  title: string,
  status: string,
  order: number,
  priority: string,
  labels: list(string),
  "assignee?": ASSIGNEE,
  /** The day it's due, `YYYY-MM-DD`. */
  "due?": string,
  /** Its due day is past, as the box asked counts days, and it's open. */
  "overdue?": bool,
  "can?": CAN,
  "progress?": PROGRESS,
  sessions: list(object({ session: string, title: string, working: bool })),
  noteCount: number,
  "lastNote?": NOTE,
  fileCount: number,
  linkCount: number,
  "handedBy?": string,
  "source?": SOURCE,
  "seen?": SEEN,
  "keptIn?": string,
  "struck?": bool,
  "closesOn?": string,
  /** What a client shows in place of its status while GitHub makes it Done ("Merged on GitHub"). */
  "doneAs?": string,
  "handing?": object({ to: string, name: string }),
  "computer?": COMPUTER,
  created: STAMP,
  updated: STAMP,
  "doneAt?": number,
  "removed?": STAMP,
});

/** An item whole. One on another of your computers says `told`: false, read whole there (its box
 * answered the box asked); true, as that computer last told it, without what only it keeps. */
export const QUEUE_ITEM_WHOLE = object({
  ...QUEUE_ITEM.object,
  "told?": bool,
  body: string,
  notes: list(NOTE),
  events: list(EVENT),
  files: list(object({ name: string, size: number, type: string, at: number })),
  links: list(object({ url: string, "title?": string, "kind?": string, "ref?": string })),
  handed: list(object({ from: string, to: string, at: number })),
});

/** An owner in a view: what it is, its title, the bot it is, and who started it. */
export const QUEUE_OWNER = object({ owner: string, kind: words("bot", "session", "group", "you"), title: nullable(string), "bot?": object({ id: string, name: string }), "startedBy?": string });

/** A view's counts (`counts=1`), of the items it lists (those a search left): how many are open,
 * of each status, priority and label (by its id, every one of the view's), from each outside source
 * (by its kind), on each computer (by its key, this box's own too), and how many are deleted. */
export const QUEUE_COUNTS = object({ open: number, statuses: map(number), priorities: map(number), labels: map(number), sources: map(number), computers: map(number), removed: number });

/** An owner's view; with `counts=1`, its counts too; with `q`, only what the words are in. */
export const QUEUE_VIEW = object({ owner: string, statuses: list(QUEUE_STATUS), priorities: list(QUEUE_PRIORITY), labels: list(QUEUE_LABEL), assignees: list(ASSIGNEE),
  /** What an item added with nothing said starts as. */
  new: object({ status: string, priority: string }),
  /** The day it is on the box asked (`YYYY-MM-DD`): what `overdue` goes by, and what a client counts days from. */
  today: string,
  /** The filters it was asked with, as taken. */
  "filters?": object({ "computers?": list(string), "sources?": list(string), "owners?": list(string), "labels?": list(string) }),
  items: list(QUEUE_ITEM), owners: list(QUEUE_OWNER), removed: list(QUEUE_ITEM),
  computers: list(object({ ...COMPUTER.object, at: number, "away?": number })), "counts?": QUEUE_COUNTS, "q?": string });

const REF = nullable(object({ id: string, "number?": number, "source?": SOURCE, "seen?": SEEN, "struck?": bool, "closesOn?": string, title: string, status: string, owner: string, "removed?": bool, "computer?": COMPUTER }));

// An item on another of your computers that couldn't be changed there now: 409, a computer that
// doesn't take it from another box (changed there); 503, one that isn't answering.
const ON_ANOTHER = object({ message: string, computer: COMPUTER });

export const WORK_QUEUE_ROUTES: RouteShape[] = [
  { route: "GET kuvo/work-queue", answers: { 200: QUEUE_VIEW } },
  { route: "GET kuvo/work-queue/events", answers: { 200: QUEUE_VIEW } },
  { route: "GET kuvo/work-queue/tools", answers: { 200: object({ tools: list(any), instructions: string }) } },
  { route: "POST kuvo/work-queue/tools", answers: { 200: object({ text: string, isError: bool }) } },
  { route: "GET kuvo/work-queue/refs", answers: { 200: object({ refs: map(REF) }) } },
  { route: "POST kuvo/work-queue/refs", answers: { 200: object({ refs: map(REF) }) } },
  { route: "GET kuvo/work-queue/labels", answers: { 200: object({ labels: list(QUEUE_LABEL) }) } },
  { route: "POST kuvo/work-queue/labels", answers: { 200: object({ labels: list(QUEUE_LABEL), "label?": QUEUE_LABEL }) } },
  { route: "POST kuvo/work-queue/items", answers: { 200: object({ item: QUEUE_ITEM }) } },
  { route: "POST kuvo/work-queue/import", answers: { 200: object({ added: number, byStatus: map(number), waiting: number, skipped: list(string), said: string }) } },
  { route: "GET kuvo/work-queue/items/:item", answers: { 200: object({ item: QUEUE_ITEM_WHOLE }) } },
  // (A file of an item on another computer: from there, 4 MB at most, else 413.)
  { route: "GET kuvo/work-queue/items/:item/files/:name", answers: { 200: BYTES, 409: ON_ANOTHER, 413: object({ message: string, "computer?": COMPUTER }), 502: ON_ANOTHER, 503: ON_ANOTHER } },
  { route: "PATCH kuvo/work-queue/items/:item", answers: { 200: object({ item: QUEUE_ITEM }), 409: ON_ANOTHER, 503: ON_ANOTHER } },
  { route: "POST kuvo/work-queue/items/:item/restore", answers: { 200: object({ item: QUEUE_ITEM }), 409: ON_ANOTHER, 503: ON_ANOTHER } },
  { route: "DELETE kuvo/work-queue/items/:item", answers: { 200: object({ item: QUEUE_ITEM }), 409: ON_ANOTHER, 503: ON_ANOTHER } },
];



export type QueueCounts = Infer<typeof QUEUE_COUNTS>;
