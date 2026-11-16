// What the Work Queue looks like to an owner (PROTOCOL.md, "The Work Queue"): its own items, those
// of the sessions it oversees (started by it, or by those), and those it handed on, each living
// once, in its owner's queue; with where each stands (its status), which of its sessions are at
// work now, and what whoever asks may do to it. The box states these facts; clients draw them.

import { placeOfSession } from "../letters/letter";
import { listBots } from "../bots/list";
import { atWorkNow } from "../work/background";
import { startedByOf, under, workSessions, worksFor } from "../work/tree";
import type { Listed } from "../work/tree";
import type { Target } from "../../shared/records";
import type { Assignee, Item, ItemEvent, ItemRef, KeptFile, Link, Note, Source, Who } from "./store";
import { ITEM_ID, YOURS } from "./store";
import { isClosed } from "./statuses";
import { assigneeNamed, type Assignable } from "./assignees";
import { issueAt, sourceKey } from "./sources";
import { seenOf, withSeen, type Seen } from "./github";

/** What the box knows of the sessions here, for a view: every one an agent started (put away
 * ones too, so a lead still sees their items), and those at work now. */
export interface Facts {
  all: Listed[];
  working: Set<string>;
}

export async function factsOf(targets: Target[], signal?: AbortSignal): Promise<Facts> {
  const [all, working] = await Promise.all([workSessions(targets, signal, { archived: true }).catch(() => [] as Listed[]), atWorkNow(targets, signal).catch(() => new Set<string>())]);
  return { all, working };
}

/** The sessions `owner` oversees: those it started, theirs, and so on. */
export const overseenBy = (owner: string, all: Listed[]): Set<string> => new Set(owner.startsWith("ses_") ? under(owner, all).map(each => each.data.id) : []);

/** Whether `item` is in `owner`'s view: its own, a session's it oversees, or one it handed on. */
export const inView = (owner: string, item: Item, overseen: Set<string>): boolean =>
  item.owner === owner || overseen.has(item.owner) || item.handedBy === owner || Boolean(item.handed?.some(each => each.from === owner));

/** Whether `item` is one `owner` is working on without it being in its view: someone else's,
 * linked to it, not deleted. A view shows these too (seeing only: what its agent may change is
 * still `inView`'s). */
export const linkedTo = (owner: string, item: { sessions: { session: string }[]; removed?: unknown }): boolean =>
  !item.removed && item.sessions.some(each => each.session === owner);

/** An item named in a tool's words or a route: `#243`, `243`, its id, or the GitHub issue or pull
 * request it is (`gh:owner/repo#12`, or its page). */
export function refOf(given: unknown): ItemRef | null {
  if (typeof given === "number" && Number.isInteger(given) && given > 0 && given < 1_000_000) return { number: given };
  if (typeof given !== "string") return null;
  const text = given.trim();
  if (ITEM_ID.test(text)) return { id: text };
  const number = text.match(/^#?(\d{1,6})$/);
  if (number) return { number: Number(number[1]) };
  const at = issueAt(text);
  return at ? { source: sourceKey(at) } : null;
}

/** A session as a view names it: its title, and whether it's at work. */
export interface SessionFacts { session: string; title: string; working: boolean }

/** An owner as a view names it: what it is, its title, the bot it is, and who started it. */
export interface OwnerFacts {
  owner: string;
  kind: "bot" | "session" | "group" | "you";
  title: string | null;
  bot?: { id: string; name: string };
  startedBy?: string;
}

/** Titles and who-started-whom of the sessions named, looked up where the list doesn't have them. */
export async function sessionsNamed(ids: Iterable<string>, facts: Facts, targets: Target[], cache?: Map<string, unknown>): Promise<Map<string, { title: string; startedBy?: string; bot?: { id: string; name: string } }>> {
  const byID = new Map(facts.all.map(each => [each.data.id, each.data]));
  const bots = listBots();
  const found = new Map<string, { title: string; startedBy?: string; bot?: { id: string; name: string } }>();
  for (const id of new Set(ids)) {
    if (!id.startsWith("ses_")) continue;
    let data = (byID.get(id) ?? cache?.get(id)) as Listed["data"] | undefined;
    if (!data && targets.length && !cache?.has(id)) {
      data = (await placeOfSession(id, targets).catch(() => null))?.data as Listed["data"] | undefined;
      cache?.set(id, data);
    }
    const bot = bots.find(each => each.session === id);
    const works = worksFor(data);
    const startedBy = startedByOf(data);
    found.set(id, {
      title: bot?.name ?? (data?.title || "New Session"),
      ...(startedBy ? { startedBy } : {}),
      ...(bot ? { bot: { id: bot.id, name: bot.name } } : works ? { bot: { id: works.bot, name: works.name } } : {}),
    });
  }
  return found;
}

/** What a client shows of an item in a list: everything but its body and notes (how many, and
 * the last), with its sessions' titles and which are at work. */
export interface ItemSummary {
  id: string;
  number?: number;
  wanted?: number;
  owner: string;
  title: string;
  status: string;
  order: number;
  priority: string;
  labels: string[];
  /** Who it's for, by name; a bot of this box with its look (its face). */
  assignee?: Assignee & { name: string; look?: unknown };
  due?: string;
  /** Its due day is past (by the day it is on the box asked), and it's neither closed nor deleted. */
  overdue?: boolean;
  progress?: { value?: number; note?: string };
  sessions: SessionFacts[];
  /** What whoever asked may do to it (`canOf`): a client's view and its answers only. */
  can?: Can;
  noteCount: number;
  lastNote?: Note;
  fileCount: number;
  linkCount: number;
  handedBy?: string;
  source?: Source;
  /** Where its issue or pull request stands on GitHub, as last read (`github.ts`), and since when. */
  seen?: Seen;
  /** The status it's kept with, while GitHub's word makes it Done (`status` "done"). */
  keptIn?: string;
  /** Its title struck through: closed on GitHub without being done (not planned, or unmerged). */
  struck?: boolean;
  /** Where it's made Done (an issue's: "github"), never by a move here. */
  closesOn?: string;
  /** On its way to a conversation on another computer. */
  handing?: { to: string; name: string };
  /** The computer it's on, when that isn't the one asked: as that computer last told it. */
  computer?: Computer;
  created: { at: number; by: Who };
  updated: { at: number; by: Who };
  doneAt?: number;
  removed?: { at: number; by: Who };
}

/** Another of your computers, as the Work Queue names it: its key, its name, what it is, and its id
 * on this Mac (a machine's), for a client to reach it. */
export interface Computer { key: string; name: string; kind: "mac" | "machine"; id?: string }

/** An item whole, as `kuvo/work-queue/items/:item` and queue_show give it. */
export interface ItemWhole extends ItemSummary {
  body: string;
  notes: Note[];
  /** What happened to it, oldest first: none for an item from before the box kept them. */
  events: ItemEvent[];
  files: KeptFile[];
  links: Link[];
  handed: { from: string; to: string; at: number }[];
}

/** What a client may do to an item, so it offers nothing the box would refuse: `edit` its title
 * and description (never an issue's: GitHub's); `set` where it stands among the open statuses, its
 * priority, labels, due day and place; `assign` it to one of the view's `assignees` (those are the
 * bots of the box asked: an item another computer keeps is assigned there); `close` it (Done or Canceled: never an issue's,
 * which GitHub closes); `delete` it; `restore` it (a deleted one, and nothing else then). None
 * of them while its conversation moves to another computer (`moving`), or its computer isn't
 * answering (`away`); only `delete` while it's on its way to another computer. */
export interface Can { edit: boolean; set: boolean; assign: boolean; close: boolean; delete: boolean; restore: boolean; why?: string }

/** `why`: where something can't be done, the reason in words a client shows as they are. */
export function canOf(item: Pick<ItemSummary, "removed" | "handing" | "source" | "seen" | "keptIn" | "computer">, { moving = false, away = false, apart = false }: { moving?: boolean; away?: boolean; apart?: boolean } = {}): Can {
  const none = { edit: false, set: false, assign: false, close: false, delete: false, restore: false };
  const there = item.computer?.name ?? "Its computer";
  if (moving) return { ...none, why: "Its conversation is moving to another computer: it can be changed once it's there." };
  if (away) return { ...none, why: `${there} isn't answering now (it may be asleep, off or offline): it can be changed once it's back.` };
  // (One its computer doesn't take what a client does from another box for.)
  if (apart) return { ...none, why: `It's on ${there}: change it there.` };
  if (item.removed) return { ...none, restore: true, why: "It's deleted: bring it back to change it." };
  if (item.handing) return { ...none, delete: true, why: `It's on its way to ${item.handing.name || "a conversation"} on another computer: it can be changed there once it's arrived.` };
  // (An issue GitHub closed stays Done until it's reopened there.)
  if (item.source) return { ...none, set: item.keptIn === undefined, assign: item.keptIn === undefined && !item.computer, delete: true, why: item.keptIn === undefined ? "Its title, its description and closing it are GitHub's." : "It's closed on GitHub: it's back where it stood once it's reopened there." };
  return { edit: true, set: true, assign: !item.computer, close: true, delete: true, restore: false };
}

/** The day it is here, `YYYY-MM-DD`. */
export const todayHere = (now = new Date()): string => `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;

/** Whether an item is late: due before today (here), and still open. The box says so, so every
 * client draws the same items late. */
export const overdueOf = (item: { due?: string; status: string; removed?: unknown }, today = todayHere()): boolean =>
  Boolean(item.due) && !item.removed && !isClosed(item.status) && item.due! < today;

export function summaryOf(item: Item, working: Set<string>, titles: Map<string, { title: string }>, assignables?: Assignable[]): ItemSummary {
  return withSeen({
    id: item.id,
    ...(item.number !== undefined ? { number: item.number } : {}),
    ...(item.wanted !== undefined ? { wanted: item.wanted } : {}),
    owner: item.owner,
    title: item.title,
    status: item.status,
    order: item.order,
    priority: item.priority ?? "normal",
    labels: item.labels,
    ...(item.assignee ? { assignee: assigneeNamed(item.assignee, assignables) } : {}),
    ...(item.due ? { due: item.due } : {}),
    ...(overdueOf(item) ? { overdue: true } : {}),
    ...(item.progress ? { progress: item.progress } : {}),
    sessions: item.sessions.map(each => ({ session: each.session, title: titles.get(each.session)?.title ?? "New Session", working: !item.removed && !isClosed(item.status) && working.has(each.session) })),
    noteCount: item.notes.length,
    ...(item.notes.length ? { lastNote: item.notes[item.notes.length - 1] } : {}),
    fileCount: item.files.length,
    linkCount: item.links.length,
    ...(item.handedBy ? { handedBy: item.handedBy } : {}),
    ...(item.source ? { source: item.source } : {}),
    ...(item.handing ? { handing: { to: item.handing.to, name: item.handing.name } } : {}),
    created: item.created,
    updated: item.updated,
    ...(item.doneAt !== undefined ? { doneAt: item.doneAt } : {}),
    ...(item.removed ? { removed: item.removed } : {}),
  });
}

export function wholeOf(item: Item, working: Set<string>, titles: Map<string, { title: string }>, assignables?: Assignable[]): ItemWhole {
  return { ...summaryOf(item, working, titles, assignables), body: item.body, notes: item.notes, events: item.events ?? [], files: item.files, links: item.links, handed: item.handed ?? [] };
}

/** The owners of `items`, and `owner` itself, as a view names them. */
export async function ownersOf(owner: string, items: Item[], facts: Facts, targets: Target[], cache?: Map<string, unknown>): Promise<{ owners: OwnerFacts[]; titles: Map<string, { title: string; startedBy?: string; bot?: { id: string; name: string } }> }> {
  const keys = new Set([...(owner === "all" ? [] : [owner]), ...items.map(item => item.owner), ...items.flatMap(item => item.handedBy ? [item.handedBy] : [])]);
  const named = await sessionsNamed([...keys, ...items.flatMap(item => item.sessions.map(each => each.session))], facts, targets, cache);
  const bots = listBots();
  const owners: OwnerFacts[] = [...keys].map(key => {
    if (key.startsWith("grp_")) return { owner: key, kind: "group", title: null };
    if (key === YOURS) return { owner: key, kind: "you", title: "Your own" };
    const found = named.get(key);
    const bot = bots.find(each => each.session === key);
    return {
      owner: key,
      kind: bot ? "bot" : "session",
      title: found?.title ?? null,
      ...(found?.bot ? { bot: found.bot } : {}),
      ...(found?.startedBy ? { startedBy: found.startedBy } : {}),
    };
  });
  return { owners, titles: named };
}

/** `owners`, conversations here, by their titles (a bot's by its name), for words you read: a
 * refusal names the one that has an issue by what you call it, not its address. One that can't be
 * looked up now (gone, its engine not answering) or has no title has none: its address is said. */
export async function titlesHere(owners: Iterable<string>, targets: Target[]): Promise<(owner: string) => string | undefined> {
  const bots = listBots();
  const titles = new Map<string, string>();
  for (const owner of new Set(owners)) {
    const bot = bots.find(each => each.session === owner)?.name;
    const title = bot ?? (owner.startsWith("ses_") ? (await placeOfSession(owner, targets).catch(() => null))?.data.title : undefined);
    if (title) titles.set(owner, title);
  }
  return owner => titles.get(owner);
}

/** How long ago `at` was, in words, as tools say it. */
export function agoWords(at: number, now = Date.now()): string {
  const minutes = Math.max(0, Math.round((now - at) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

/** An item as tools name it: an outside source's by its ref (`gh:owner/repo#12 "Title"`, its
 * number never shown again), `#243 "Title"`, or `"Title" (new, no number yet)`. (An issue's
 * title is GitHub's, as last read.) */
export const nameOf = (item: Pick<Item, "number" | "title" | "wanted" | "id"> & { source?: Source }): string =>
  item.source ? `${item.source.ref} "${seenOf(item.source)?.value?.title ?? item.title}"` : item.number !== undefined ? `#${item.number} "${item.title}"` : `"${item.title}" (${item.id}, no number yet${item.wanted ? `: asked for #${item.wanted}` : ""})`;
