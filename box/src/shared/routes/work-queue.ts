import { PENDING_COMPUTERS } from "./list-computers";
import { OFFLINE_EDIT_ANSWERS } from "./ownership";
// The Work Queue (PROTOCOL.md, "Per area"): an owner's view and its events, one item whole,
// what `#n` refs name, the statuses, priorities and labels, and what you do from a client. Times
// are milliseconds since 1970.

import { any, bool, BYTES, type Infer, list, map, nullable, number, object, oneOf, type RouteShape, string, words } from "../shape";
import { SAVE_RECEIPT } from "./setup-save";

export const LABEL_RESTORE_ROUTE = "kuvo/work-queue/labels/restore";
export const LABEL_RESTORE_FEATURE = "work-queue.labels.restore";
export const LABEL_RESTORE_PREVIEW_FEATURE = "work-queue.labels.restore.preview";
export const LABEL_RESTORE_RESULT = object({
  restored: number, alreadyPresent: number, conflicts: number,
  details: list(object({ id: string, reason: string })), "save?": SAVE_RECEIPT, "provisional?": bool,
});
const MESSAGE = object({ message: string });

// (You, an agent, or another shape kept as it came: an outside tracker's change, later.)
const WHO = oneOf(object({ user: bool }), object({ session: string, name: string }), map(any));
const STAMP = object({ at: number, by: WHO });
const NOTE = object({ at: number, by: WHO, text: string, "id?": string });
const MENTION = object({ start: number, end: number, number, item: string });
const ITEM_RELATION = object({ id: string, kind: string, title: string, "number?": number });
/** Activity history. An edit can name its fields and a stop can name the affected session. */
const EVENT = object({ at: number, by: WHO, kind: string, "from?": string, "to?": string, "top?": bool, "fields?": list(string), "session?": string, "fromLabel?": string, "toLabel?": string });
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
const CAN = object({ edit: bool, set: bool, assign: bool, close: bool, delete: bool, restore: bool, "attach?": bool, "why?": string });

/** An item as a list shows it: everything but its body and notes (how many, and the last). */
const NUMBER_ALIASES = list(object({ setup: string, name: string, number }));

export const QUEUE_ITEM = object({
  id: string,
  "number?": number,
  "numberPending?": bool,
  "numberAliases?": NUMBER_ALIASES,
  "mentionTargets?": list(string),
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
  notes: list(object({ ...NOTE.object, "mentions?": list(MENTION) })),
  "bodyMentions?": list(MENTION),
  "itemRelations?": list(ITEM_RELATION),
  "canLinkItems?": bool,
  events: list(EVENT),
  files: list(object({ name: string, size: number, type: string, at: number, "id?": string, "anchor?": string, "sha256?": string })),
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
export const QUEUE_VIEW = object({ ...PENDING_COMPUTERS, owner: string, statuses: list(QUEUE_STATUS), priorities: list(QUEUE_PRIORITY), labels: list(QUEUE_LABEL),
  /** The colours a label may be (the looks' names), in the order a picker offers them. */
  colors: list(string),
  assignees: list(ASSIGNEE),
  /** What an item added with nothing said starts as. */
  new: object({ status: string, priority: string }),
  /** The day it is on the box asked (`YYYY-MM-DD`): what `overdue` goes by, and what a client counts days from. */
  today: string,
  /** The filters it was asked with, as taken. */
  "filters?": object({ "computers?": list(string), "sources?": list(string), "owners?": list(string), "labels?": list(string) }),
  items: list(QUEUE_ITEM), owners: list(QUEUE_OWNER), removed: list(QUEUE_ITEM),
  computers: list(object({ ...COMPUTER.object, at: number, "away?": number })), "counts?": QUEUE_COUNTS, "q?": string });

const REF_FACTS = object({ id: string, "number?": number, "numberPending?": bool, "numberAliases?": NUMBER_ALIASES, "source?": SOURCE, "seen?": SEEN, "struck?": bool, "closesOn?": string, title: string, status: string, owner: string, "removed?": bool, "computer?": COMPUTER });
const REFS = object({ refs: map(nullable(REF_FACTS)), "aliases?": map(list(REF_FACTS)) });

// An item on another of your computers that couldn't be changed there now: 409, a computer that
// doesn't take it from another box (changed there); 503, one that isn't answering.
const ON_ANOTHER = object({ message: string, computer: COMPUTER });

export const WORK_QUEUE_ROUTES: RouteShape[] = [
  // An owner's view (`owner=ses_…|grp_…|usr_you|all`), across your computers; `q`, filters and `counts=1`.
  { route: "GET kuvo/work-queue", feature: "work-queue", answers: { 200: QUEUE_VIEW } },
  { route: "GET kuvo/work-queue/events", feature: "work-queue", answers: { 200: QUEUE_VIEW } },
  // The agents' `queue_…` tools (MCP definitions) and their instructions, for a tool server to list.
  { route: "GET kuvo/work-queue/tools", feature: "work-queue", answers: { 200: object({ tools: list(any), instructions: string }) } },
  // Runs an agent's `queue_…` call (`{name, arguments, session}`), only while that session is making it.
  { route: "POST kuvo/work-queue/tools", feature: "work-queue", answers: { 200: object({ text: string, isError: bool, "save?": SAVE_RECEIPT }) } },
  // What each ref names (`refs=243,12`: a `#n`, an id or a `gh:` issue), for chips; null for one unknown.
  { route: "GET kuvo/work-queue/refs", feature: "work-queue", answers: { 200: REFS } },
  // Refs as a list (`{refs}`), or `{text}` → the `#n` mentions in a description (UTF-16 ranges).
  { route: "POST kuvo/work-queue/refs", feature: "work-queue", answers: { 200: REFS } },
  { route: "GET kuvo/work-queue/labels", feature: "work-queue.edit", answers: { 200: object({ labels: list(QUEUE_LABEL) }) } },
  // Adds (`{add, color?}`), changes (`{change, name?, color?}`) or removes (`{remove}`) a label.
  { route: "POST kuvo/work-queue/labels", feature: "work-queue.edit", answers: { 200: object({ labels: list(QUEUE_LABEL), "label?": QUEUE_LABEL }) } },
  // Restores saved label definitions with their ids through the main computer (`{library, force?}`).
  { route: `POST ${LABEL_RESTORE_ROUTE}`, feature: LABEL_RESTORE_FEATURE, answers: {
    200: LABEL_RESTORE_RESULT, 400: MESSAGE, 403: MESSAGE, 409: MESSAGE, 503: MESSAGE,
  } },
  // Previews a label restore without saving it; answered by the main computer.
  { route: `GET ${LABEL_RESTORE_ROUTE}`, feature: LABEL_RESTORE_PREVIEW_FEATURE, answers: {
    200: LABEL_RESTORE_RESULT, 400: MESSAGE, 403: MESSAGE, 409: MESSAGE, 503: MESSAGE,
  } },
  // Adds an item; send a client operation id (`x-kuvo-operation`) and reuse it on a retry.
  { route: "POST kuvo/work-queue/items", feature: "work-queue.edit", answers: { 200: object({ item: QUEUE_ITEM }) } },
  // Imports items from Markdown backlog files on the box (`{owner, files}`), keeping their numbers where free.
  { route: "POST kuvo/work-queue/import", feature: "work-queue", answers: { 200: object({ added: number, byStatus: map(number), waiting: number, skipped: list(string), said: string }) } },
  // One item whole, by id or number; one on another computer is read from there (`told: false`) when it can be.
  { route: "GET kuvo/work-queue/items/:item", feature: "work-queue", answers: { 200: object({ item: QUEUE_ITEM_WHOLE }) } },
  // An item's file; one on another computer comes from there, 4 MB at most, else 413.
  { route: "GET kuvo/work-queue/items/:item/files/:name", feature: "work-queue.attachments", answers: { 200: BYTES, 409: ON_ANOTHER, 413: object({ message: string, "computer?": COMPUTER }), 502: ON_ANOTHER, 503: ON_ANOTHER } },
  // Changes only what's sent; one kept on another computer is changed there by its box.
  { route: "PATCH kuvo/work-queue/items/:item", feature: "work-queue.edit", answers: { ...OFFLINE_EDIT_ANSWERS, 200: oneOf(object({ item: QUEUE_ITEM }), OFFLINE_EDIT_ANSWERS[200]), 409: oneOf(ON_ANOTHER, OFFLINE_EDIT_ANSWERS[409]), 503: ON_ANOTHER } },
  // Brings a deleted item back (kept 30 days).
  { route: "POST kuvo/work-queue/items/:item/restore", feature: "work-queue.edit", answers: { 200: object({ item: QUEUE_ITEM }), 409: ON_ANOTHER, 503: ON_ANOTHER } },
  // Deletes an item (kept 30 days); one on its way to another computer has its hand-off called off.
  { route: "DELETE kuvo/work-queue/items/:item", feature: "work-queue.edit", answers: { 200: object({ item: QUEUE_ITEM }), 409: ON_ANOTHER, 503: ON_ANOTHER } },
];



export type QueueCounts = Infer<typeof QUEUE_COUNTS>;
