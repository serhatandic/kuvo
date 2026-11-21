// What your other computers told this one of their Work Queues (PROTOCOL.md, "The Work Queue",
// "Across your computers"): each one's items as summaries, the owners they name, their custom
// labels, and how each numbers (when it was set up, and its primary). Kept in
// `run/work-queue/peers/<key>.json`, the newest from each, so the All view and a lead's view have
// them, as of when they were told, while that computer sleeps.

import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { cut, plainName } from "../../shared/text";
import { listOf, locked, writePrivate } from "../files";
import { KEY } from "../peers/known";
import { assigneeOf, dayOf, itemNumberOf, ITEM_ID, numberAliasesOf, OWNER, WORK_QUEUE, whoOf as keptWho, type Who } from "./store";
import { PRIORITY_ID, STATUS_ID } from "./statuses";
import { LABEL_ID, labelOf, MOST_LABELS, type Label } from "./labels";
import type { ItemSummary, OwnerFacts } from "./view";

export const TOLD = path.join(WORK_QUEUE, "peers");

/** How a computer numbers: its kind, when it was set up, the highest number it used or holds, the
 * primary it numbers from (`self` for itself). */
export interface Numbering {
  kind: "mac" | "machine";
  since?: number;
  top?: number;
  primary?: { key: string; kind: "mac" | "machine"; since?: number };
}

/** An item as another computer tells it: its summary, and who handed it on. */
export type ToldItem = ItemSummary & { handedFrom?: string[]; mentionTargets?: string[] };

/** What a computer last told: as of when (its clock, `at`), when it was heard (this one's), its
 * items, the owners they name, its labels, and how it numbers. */
export interface Told extends Numbering {
  /** Highest report time heard from this sender, including across its clock resets. */
  latestAt?: number;
  id: string;
  at: number;
  heard: number;
  items: ToldItem[];
  owners: OwnerFacts[];
  labels: Label[];
}

/** How many items a computer tells at most, and of its Done ones. */
export const MOST_TOLD = 1500;
export const MOST_TOLD_DONE = 100;

const fileOf = (key: string) => path.join(TOLD, `${key}.json`);
const SESSION = /^ses_[A-Za-z0-9]{1,60}$/;
const KIND = (value: unknown): "mac" | "machine" | null => (value === "mac" || value === "machine" ? value : null);
const count = (value: unknown) => (typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : 0);
const time = (value: unknown) => (typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0);
const whole = (value: unknown, most = 999_999) => (typeof value === "number" && Number.isInteger(value) && value > 0 && value <= most ? value : undefined);
const words = (value: unknown, most: number) => (typeof value === "string" ? cut(plainName(value), most) : "");
// (As the box keeps one: an unknown shape as it came, and none at all as someone else, never you.)
const whoOf = (value: unknown): Who => keptWho(value) ?? { name: "someone else" };
const stamp = (value: unknown) => (isObject(value) ? { at: time(value.at), by: whoOf(value.by) } : { at: 0, by: whoOf(null) });

/** An item as it came from another computer, cleaned: what a summary has, nothing more; null for
 * one that isn't. */
export function toldItemOf(given: unknown, latest = Infinity): ToldItem | null {
  if (!isObject(given) || typeof given.id !== "string" || !ITEM_ID.test(given.id) || typeof given.owner !== "string" || !OWNER.test(given.owner)) return null;
  const title = words(given.title, 200);
  // (Where it stands: a status this box knows, else Todo.)
  const status = typeof given.status === "string" && STATUS_ID.test(given.status) ? given.status : "todo";
  const labels = (Array.isArray(given.labels) ? given.labels : []).filter((each): each is string => typeof each === "string" && LABEL_ID.test(each)).slice(0, 20);
  const assignee = assigneeOf(given.assignee);
  const progress = isObject(given.progress)
    ? { ...(typeof given.progress.value === "number" && given.progress.value >= 0 && given.progress.value <= 1 ? { value: given.progress.value } : {}), ...(typeof given.progress.note === "string" ? { note: words(given.progress.note, 300) } : {}) }
    : undefined;
  const sessions = (Array.isArray(given.sessions) ? given.sessions : []).filter(each => isObject(each) && typeof each.session === "string" && SESSION.test(each.session)).slice(0, 20)
    .map(each => ({ session: each.session as string, title: words(each.title, 200) || "New Session", working: each.working === true }));
  const lastNote = isObject(given.lastNote) && typeof given.lastNote.text === "string" ? { at: time(given.lastNote.at), by: whoOf(given.lastNote.by), text: cut(given.lastNote.text, 500) } : undefined;
  const source = isObject(given.source) && typeof given.source.kind === "string" && typeof given.source.ref === "string"
    ? Object.fromEntries(Object.entries(given.source).filter(([key, value]) => ["kind", "ref", "url", "host", "id", "name"].includes(key) && typeof value === "string").map(([key, value]) => [key, (value as string).slice(0, 2000)])) as unknown as ItemSummary["source"]
    : undefined;
  // (A GitHub issue's page only on GitHub: a client opens it.)
  if (source?.url && /^github/.test(source.kind) && !/^https:\/\/github\.com\//.test(source.url)) delete source.url;
  // (What GitHub said of it there, and when; with the status it keeps while that makes it Done.)
  const seen = source && isObject(given.seen) && typeof given.seen.state === "string" && /^[a-z_]{1,20}$/.test(given.seen.state)
    // (Never later than when it was told, nor than now: a clock set ahead there wins no read here.)
    ? { state: given.seen.state, at: Math.min(time(given.seen.at), latest), ...(typeof given.seen.checked === "number" ? { checked: Math.min(time(given.seen.checked), latest) } : {}), ...(given.seen.gone === true ? { gone: true } : {}) } : undefined;
  const keptIn = seen && typeof given.keptIn === "string" && STATUS_ID.test(given.keptIn) ? given.keptIn : undefined;
  return {
    id: given.id,
    ...itemNumberOf({ number: whole(given.number), source }),
    ...(Array.isArray(given.numberAliases) ? { numberAliases: numberAliasesOf(given.numberAliases) } : {}),
    ...(whole(given.wanted) ? { wanted: whole(given.wanted) } : {}),
    owner: given.owner,
    title: title || "Untitled",
    status,
    order: typeof given.order === "number" && Number.isFinite(given.order) ? given.order : 0,
    priority: typeof given.priority === "string" && PRIORITY_ID.test(given.priority) ? given.priority : "normal",
    labels,
    ...(assignee ? { assignee: { ...assignee, name: words(isObject(given.assignee) ? given.assignee.name : "", 100) || assignee.id } } : {}),
    ...(dayOf(given.due) ? { due: dayOf(given.due)! } : {}),
    ...(progress && Object.keys(progress).length ? { progress } : {}),
    sessions,
    noteCount: count(given.noteCount),
    ...(lastNote ? { lastNote } : {}),
    fileCount: count(given.fileCount),
    linkCount: count(given.linkCount),
    ...(typeof given.handedBy === "string" && OWNER.test(given.handedBy) ? { handedBy: given.handedBy } : {}),
    ...(Array.isArray(given.handedFrom) ? { handedFrom: given.handedFrom.filter((each): each is string => typeof each === "string" && OWNER.test(each)).slice(0, 20) } : {}),
    ...(Array.isArray(given.mentionTargets) ? { mentionTargets: [...new Set(given.mentionTargets.filter((id): id is string => typeof id === "string" && ITEM_ID.test(id)))] } : {}),
    ...(source ? { source } : {}),
    ...(seen ? { seen } : {}),
    ...(keptIn ? { keptIn } : {}),
    ...(isObject(given.handing) && typeof given.handing.to === "string" && OWNER.test(given.handing.to) ? { handing: { to: given.handing.to, name: words(given.handing.name, 100) } } : {}),
    created: stamp(given.created),
    updated: stamp(given.updated),
    ...(typeof given.doneAt === "number" ? { doneAt: time(given.doneAt) } : {}),
  };
}

const ownerOf = (given: unknown): OwnerFacts | null => {
  if (!isObject(given) || typeof given.owner !== "string" || !OWNER.test(given.owner)) return null;
  const kind = given.kind === "bot" || given.kind === "group" || given.kind === "you" ? given.kind : "session";
  const bot = isObject(given.bot) && typeof given.bot.id === "string" && /^bot_[A-Za-z0-9]{1,60}$/.test(given.bot.id) ? { id: given.bot.id, name: words(given.bot.name, 100) } : undefined;
  return { owner: given.owner, kind, title: typeof given.title === "string" ? words(given.title, 200) : null, ...(bot ? { bot } : {}), ...(typeof given.startedBy === "string" && SESSION.test(given.startedBy) ? { startedBy: given.startedBy } : {}) };
};

/** How a computer said it numbers, cleaned. */
export function numberingOf(given: Record<string, unknown>): Numbering {
  const kind = KIND(given.kind) ?? "machine";
  const primary = isObject(given.primary) && typeof given.primary.key === "string" && KEY.test(given.primary.key) && KIND(given.primary.kind)
    ? { key: given.primary.key, kind: KIND(given.primary.kind)!, ...(time(given.primary.since) ? { since: time(given.primary.since) } : {}) } : undefined;
  return { kind, ...(time(given.since) ? { since: time(given.since) } : {}), ...(whole(given.top) ? { top: whole(given.top) } : {}), ...(primary ? { primary } : {}) };
}

/** What a computer told, as it came, cleaned: null for what isn't. */
export function toldOf(given: unknown, now = Date.now()): Omit<Told, "heard"> | null {
  if (!isObject(given) || typeof given.id !== "string" || !/^wqt_[A-Za-z0-9_-]{8,40}$/.test(given.id) || typeof given.at !== "number" || !Array.isArray(given.items)) return null;
  // (Only from a box that keeps statuses and labels: what another says of its queue isn't read as
  // an empty list of labels, which would take a primary's off every item here.)
  if (!Array.isArray(given.labels)) return null;
  const seen = new Set<string>();
  const latest = Math.min(given.at, now);
  const items = given.items.slice(0, MOST_TOLD).map(each => toldItemOf(each, latest)).filter((each): each is ToldItem => each !== null && !seen.has(each.id) && seen.add(each.id) !== undefined);
  const owners = (Array.isArray(given.owners) ? given.owners : []).slice(0, MOST_TOLD).map(ownerOf).filter((each): each is OwnerFacts => each !== null);
  const labels = (Array.isArray(given.labels) ? given.labels : []).slice(0, MOST_LABELS).map(labelOf).filter((each): each is Label => each !== null);
  return { id: given.id, at: Math.min(given.at, now), items, owners, labels, ...numberingOf(given) };
}

const toldFormat = additiveObjectFormat("Work Queue peer summary");

/** What the computer `key` last told, or null. */
export function readTold(key: string): Told | null {
  if (!KEY.test(key)) return null;
  try {
    const kept = toldFormat.read(JSON.parse(fs.readFileSync(fileOf(key), "utf8")));
    if (typeof kept.at !== "number" || !Array.isArray(kept.items)) return null;
    if (!Array.isArray(kept.labels)) return null;
    return kept as unknown as Told;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return null;
  }
}

// (One process at a time changes what's kept: the link takes what's told, a route patches it.)
const LOCK = path.join(WORK_QUEUE, ".told.lock");

/** `told` kept as what `key` said, unless what's kept is newer: whether it was. */
export function keepTold(key: string, told: Omit<Told, "heard" | "latestAt">, now = Date.now()): boolean {
  if (!KEY.test(key)) return false;
  fs.mkdirSync(TOLD, { recursive: true, mode: 0o700 });
  return locked(LOCK, () => {
    const kept = readTold(key);
    // Older than what's kept: mail come late, unless it's a good while older (its clock was set
    // back), when it's the newer word after all.
    if (kept && kept.at > told.at && kept.at - told.at < 600_000 && kept.id !== told.id) return false;
    const latestAt = Math.max(told.at, kept?.at ?? told.at, kept?.latestAt ?? told.at);
    writePrivate(fileOf(key), JSON.stringify(toldFormat.write({ ...told, heard: now, latestAt } satisfies Told)));
    return true;
  }, { required: true });
}

/** A change you made to item `id` on the computer `key`, through this box, put in what that computer
 * last told, as its box answered it (`item`; `deleted`: taken out), until it tells again: what it
 * says of its sessions stays as it told it. Not over newer word of that item (told since). */
export function patchTold(key: string, id: string, item: unknown, deleted: boolean): void {
  if (!KEY.test(key)) return;
  const fresh = toldItemOf(item);
  // (An answer this box can't read changes nothing.)
  if (!fresh || fresh.id !== id) return;
  const removed = isObject(item) && isObject(item.removed) && typeof item.removed.at === "number" ? item.removed.at : 0;
  const answered = Math.max(fresh.updated.at, removed);
  try {
    locked(LOCK, () => {
      const kept = readTold(key);
      if (!kept) return;
      const before = kept.items.find(each => each.id === id);
      if (before && before.updated.at > answered) return;
      const others = kept.items.filter(each => each.id !== id);
      const items = deleted ? others
        : [...others, before ? { ...before, ...fresh, sessions: before.sessions, ...(before.progress ? { progress: before.progress } : {}), ...(before.handedFrom ? { handedFrom: before.handedFrom } : {}) } : fresh];
      writePrivate(fileOf(key), JSON.stringify(toldFormat.write({ ...kept, items })));
    }, { required: true });
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) console.error(error.message);
  }
}

/** The computers that told this one, by key. */
export const toldKeys = (): string[] => listOf(TOLD).filter(name => name.endsWith(".json")).map(name => name.slice(0, -5)).filter(key => KEY.test(key));

/** What a computer told, gone (one you removed). */
export const forgetTold = (key: string): void => {
  if (!KEY.test(key)) return;
  locked(LOCK, () => {
    readTold(key);
    fs.rmSync(fileOf(key), { force: true });
  }, { required: true });
};
