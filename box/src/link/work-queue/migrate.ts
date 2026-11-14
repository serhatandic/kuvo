// The Work Queue's move from categories to statuses, priorities and labels (PROTOCOL.md, "The Work
// Queue", "From categories"): once, one way, on the box that keeps the items, before anything else
// reads or changes them. Nothing an item holds is dropped: only where it stands is said anew.
//
// - What was kept is copied whole first, to `run/work-queue/before-statuses/` (the queues' files
//   and `categories.json`), and never touched again: it's what this works from, so a run cut off
//   midway is carried on by the next, to the same end.
// - Each queue's file is written whole, one at a time; an item that has a status already is left.
// - `categories.json` goes (it's in the copy), and `model.json` says it's done, written last.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { locked, writePrivate } from "../files";
import { RUN } from "../home";
import { categoryBefore, fromCategory, isClosed, PARKED_LABEL } from "./statuses";

const WORK_QUEUE = path.join(RUN, "work-queue");
const QUEUES = path.join(WORK_QUEUE, "queues");
const CATEGORIES_FILE = path.join(WORK_QUEUE, "categories.json");
const LABELS_FILE = path.join(WORK_QUEUE, "labels.json");
const LOCK = path.join(WORK_QUEUE, ".lock");
/** What says which way this box keeps its items: `{model: 2}` once they have statuses. */
export const MODEL_FILE = path.join(WORK_QUEUE, "model.json");
/** What was kept before, whole: the queues' files and `categories.json`. */
export const BEFORE = path.join(WORK_QUEUE, "before-statuses");
export const MODEL = 2;

let done = false;

/** For a test: as a box just started. */
export function forgetMigrated(): void {
  done = false;
}

const jsonOf = (file: string): unknown => {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; }
};
const queueFiles = (folder: string): string[] => {
  try { return fs.readdirSync(folder).filter(name => name.endsWith(".json")).sort(); } catch { return []; }
};

// A custom category's colour as the apps drew it before (by its id), in a label's colours.
const colorBefore = (id: string): string =>
  ["orange", "pink", "indigo", "mint", "coral", "amber"][[...id].reduce((sum, each) => (sum * 31 + (each.codePointAt(0) ?? 0)) & 0xFFFF, 0) % 6];

/** An item as it was kept under a category, said anew (in place): its status, its labels, and the
 * statuses its events name. One marked as being worked on (a session linked to it whose last word
 * was that it started) is In progress. `name` gives a custom category's name. */
export function migrateItem(item: Record<string, unknown>, name: (category: string) => string | undefined): void {
  if (typeof item.status === "string") return;
  const to = (category: unknown) => fromCategory(category, typeof category === "string" ? name(category) : undefined);
  const { status, label } = to(item.category);
  const sessions = (Array.isArray(item.sessions) ? item.sessions : []).flatMap(each => (isObject(each) && typeof each.session === "string" ? [each.session] : []));
  const events = Array.isArray(item.events) ? item.events.filter(isObject) : [];
  const lastWord = (session: string) => [...events].reverse().find(each => each.kind === "done" || ((each.kind === "started" || each.kind === "stopped") && isObject(each.by) && each.by.session === session));
  const working = !item.removed && !isClosed(status) && sessions.some(session => lastWord(session)?.kind === "started");
  for (const each of events) {
    if (each.kind !== "moved") continue;
    if (typeof each.from === "string") each.from = to(each.from).status;
    if (typeof each.to === "string") each.to = to(each.to).status;
  }
  item.status = working ? "in-progress" : status;
  item.labels = label ? [label] : [];
  // (What a box from before statuses still reads it under: `asKept`, store.ts.)
  item.category = categoryBefore({ status: item.status as string, labels: item.labels as string[] });
}

// Where the old categories stood, for the order items keep within a status: Next, Later, the custom
// ones as they were listed, Parked, Done.
const placeOf = (category: unknown, custom: string[]): number =>
  category === "next" ? 0 : category === "later" ? 1 : category === "parked" ? custom.length + 2 : category === "done" ? custom.length + 3
    : typeof category === "string" && custom.includes(category) ? custom.indexOf(category) + 2 : custom.length + 2;

function migrate(): void {
  fs.mkdirSync(WORK_QUEUE, { recursive: true, mode: 0o700 });
  // What was kept, copied whole, once: made beside and put in place in one step, so a copy cut off
  // is never taken for the whole of it.
  // (A box that kept nothing yet has nothing to copy or move.)
  const nothing = !queueFiles(QUEUES).length && !fs.existsSync(CATEGORIES_FILE);
  if (nothing && !fs.existsSync(BEFORE)) return void writePrivate(MODEL_FILE, JSON.stringify({ model: MODEL, at: Date.now() }));
  if (!fs.existsSync(BEFORE)) {
    for (const name of fs.readdirSync(WORK_QUEUE)) if (name.startsWith("before-statuses.")) fs.rmSync(path.join(WORK_QUEUE, name), { recursive: true, force: true });
    const making = path.join(WORK_QUEUE, `before-statuses.${crypto.randomBytes(6).toString("hex")}`);
    fs.mkdirSync(path.join(making, "queues"), { recursive: true, mode: 0o700 });
    // (Each on the disk before it counts as the copy: the files it's of are written over next.)
    const copy = (from: string, to: string) => {
      fs.copyFileSync(from, to);
      const handle = fs.openSync(to, "r");
      try { fs.fsyncSync(handle); } finally { fs.closeSync(handle); }
    };
    for (const name of queueFiles(QUEUES)) copy(path.join(QUEUES, name), path.join(making, "queues", name));
    if (fs.existsSync(CATEGORIES_FILE)) copy(CATEGORIES_FILE, path.join(making, "categories.json"));
    try {
      fs.renameSync(making, BEFORE);
    } catch (error) {
      // (Another process put its copy there meanwhile: the same.)
      fs.rmSync(making, { recursive: true, force: true });
      if (!fs.existsSync(BEFORE)) throw error;
    }
  }
  // (Only from the copy: something else by its name is no copy to work from.)
  if (!fs.statSync(path.join(BEFORE, "queues"), { throwIfNoEntry: false })?.isDirectory()) throw new Error(`${BEFORE} isn't the copy of what was kept`);
  const kept = jsonOf(path.join(BEFORE, "categories.json"));
  const custom = (isObject(kept) && Array.isArray(kept.custom) ? kept.custom : [])
    .filter((each: unknown): each is { id: string; name: string } => isObject(each) && typeof each.id === "string" && /^cat_[a-z0-9-]{1,40}$/.test(each.id) && typeof each.name === "string");
  const names = new Map(custom.map(each => [each.id, each.name]));
  const customIDs = custom.map(each => each.id);
  // Every item as it was, by its id: its status and labels now, and where it stood (its category's
  // place, then its own), which is the order it keeps among those of its status.
  const was: { id: string; category: unknown; order: number; said: Record<string, unknown> }[] = [];
  for (const name of queueFiles(path.join(BEFORE, "queues"))) {
    const queue = jsonOf(path.join(BEFORE, "queues", name));
    if (!isObject(queue) || !Array.isArray(queue.items)) continue;
    for (const each of queue.items) {
      if (!isObject(each) || typeof each.id !== "string" || typeof each.status === "string") continue;
      const said = structuredClone(each);
      migrateItem(said, id => names.get(id));
      was.push({ id: each.id, category: each.category, order: typeof each.order === "number" && Number.isFinite(each.order) ? each.order : 0, said });
    }
  }
  const order = new Map<string, number>();
  const byStatus = new Map<string, typeof was>();
  for (const each of was) byStatus.set(String(each.said.status), [...(byStatus.get(String(each.said.status)) ?? []), each]);
  for (const group of byStatus.values()) {
    group.sort((a, b) => placeOf(a.category, customIDs) - placeOf(b.category, customIDs) || a.order - b.order || a.id.localeCompare(b.id));
    group.forEach((each, index) => order.set(each.id, index));
  }
  // The labels: each custom category an item was in, in the colour it was drawn in, and Parked.
  // (Not one that's a status now, nor one nothing was in: no label is made that nothing has.)
  {
    const used = new Set(was.flatMap(each => (Array.isArray(each.said.labels) ? each.said.labels as string[] : [])));
    const made = [
      ...custom.filter(each => used.has(each.id)).map(each => ({ id: each.id, name: plainName(each.name).slice(0, 40) || each.id.slice(4), color: colorBefore(each.id) })),
      // (A category an item names that the list no longer had: by its id.)
      ...[...used].filter(id => id !== PARKED_LABEL && !names.has(id)).sort().map(id => ({ id, name: id.slice(4).replace(/-/g, " "), color: colorBefore(id) })),
      ...(used.has(PARKED_LABEL) ? [{ id: PARKED_LABEL, name: "Parked", color: "graphite" }] : []),
    ];
    // (Beside any already there, a run cut off having made some: each once, as it was first made.)
    const kept = jsonOf(LABELS_FILE);
    const there: unknown[] = isObject(kept) && Array.isArray(kept.labels) ? kept.labels : [];
    const has = new Set(there.flatMap(each => (isObject(each) && typeof each.id === "string" ? [each.id] : [])));
    const more = made.filter(each => !has.has(each.id));
    if (more.length) writePrivate(LABELS_FILE, JSON.stringify({ ...(isObject(kept) ? kept : {}), labels: [...there, ...more] }));
  }
  // Each queue as it's kept now: every item still under a category said anew, as worked out from
  // what was kept before (one added since, by an older box, on its own).
  for (const name of queueFiles(QUEUES)) {
    const file = path.join(QUEUES, name);
    const queue = jsonOf(file);
    if (!isObject(queue) || !Array.isArray(queue.items)) continue;
    let changed = false;
    queue.items = queue.items.map(each => {
      if (!isObject(each) || typeof each.status === "string") return each;
      changed = true;
      // (Said anew from what it holds now; its place, from where everything stood before.)
      migrateItem(each, id => names.get(id));
      const place = typeof each.id === "string" ? order.get(each.id) : undefined;
      if (place !== undefined) each.order = place;
      return each;
    });
    if (changed) writePrivate(file, JSON.stringify(queue));
  }
  // (The categories are in the copy, and labels now: the file nothing reads any more goes.)
  fs.rmSync(CATEGORIES_FILE, { force: true });
  writePrivate(MODEL_FILE, JSON.stringify({ model: MODEL, at: Date.now() }));
}

/** The items here kept with statuses, from now on: moved over from categories where they weren't
 * yet (see above). Called before anything reads or changes the queues; nothing once it's done. */
export function migrateOnce(): void {
  if (done) return;
  const kept = jsonOf(MODEL_FILE);
  if (isObject(kept) && typeof kept.model === "number" && kept.model >= MODEL) {
    done = true;
    return;
  }
  // (A box with no Work Queue yet has nothing to move, and makes no folder for it.)
  if (!fs.existsSync(WORK_QUEUE)) return;
  // (Said at once where nothing can be written there, a lock neither: never waited for.)
  fs.accessSync(WORK_QUEUE, fs.constants.W_OK);
  locked(LOCK, () => {
    const again = jsonOf(MODEL_FILE);
    if (!(isObject(again) && typeof again.model === "number" && again.model >= MODEL)) migrate();
  }, { stale: 60_000, wait: 10_000 });
  done = true;
}
