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
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { locked, writePrivate } from "../files";
import { RUN } from "../home";
import { colorBefore, fromCategory, isClosed, PARKED_LABEL } from "./statuses";

const WORK_QUEUE = path.join(RUN, "work-queue");
const QUEUES = path.join(WORK_QUEUE, "queues");
const CATEGORIES_FILE = path.join(WORK_QUEUE, "categories.json");
const LABELS_FILE = path.join(WORK_QUEUE, "labels.json");
export const LABELS_LOCK = path.join(WORK_QUEUE, ".labels.lock");
const LOCK = path.join(WORK_QUEUE, ".lock");
// What says a process is moving the items over now (a folder: made by one alone).
const MOVING = path.join(WORK_QUEUE, ".moving-to-statuses");
/** What says which way this box keeps its items: `{model: 2}` once they have statuses. */
export const MODEL_FILE = path.join(WORK_QUEUE, "model.json");
/** What was kept before, whole: the queues' files and `categories.json`. */
export const BEFORE = path.join(WORK_QUEUE, "before-statuses");
export const MODEL = 2;
// A separate file schema from the category/status migration. Old and new writers agree on fields.
export const queueFileFormat = additiveObjectFormat("Work Queue owner file");

export const labelsFormat = additiveObjectFormat("Work Queue labels");
const modelFormat = additiveObjectFormat("Work Queue status model");
const categoriesFormat = additiveObjectFormat("Work Queue legacy categories");

function modelRecord(): Record<string, unknown> | null {
  const value = jsonOf(MODEL_FILE);
  if (!isObject(value)) return null;
  const kept = modelFormat.read(value);
  if (typeof kept.model === "number" && kept.model > MODEL) {
    throw new UnsupportedFormatVersion("Work Queue status model", kept.model, MODEL);
  }
  return kept;
}

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

/** An item as it was kept under a category, said anew (in place): its status, its labels, and the
 * statuses its events name. `name` gives a custom category's name. Whether a session linked to it
 * had said it started on it (and not that it stopped): it's In progress only if that session is
 * really at work, which the box can say once its engine answers (`settleStarted`). */
export function migrateItem(item: Record<string, unknown>, name: (category: string) => string | undefined): boolean {
  if (typeof item.status === "string") return false;
  const to = (category: unknown) => fromCategory(category, typeof category === "string" ? name(category) : undefined);
  const { status, label } = to(item.category);
  const sessions = (Array.isArray(item.sessions) ? item.sessions : []).flatMap(each => (isObject(each) && typeof each.session === "string" ? [each.session] : []));
  const events = Array.isArray(item.events) ? item.events.filter(isObject) : [];
  const lastWord = (session: string) => [...events].reverse().find(each => each.kind === "done" || ((each.kind === "started" || each.kind === "stopped") && isObject(each.by) && each.by.session === session));
  const working = !item.removed && !isClosed(status) && sessions.some(session => lastWord(session)?.kind === "started");
  // (A move between categories names the statuses they became, and beside each the label one became
  // (`fromLabel`, `toLabel`), so what happened still says "from Architecture".)
  for (const each of events) {
    if (each.kind !== "moved") continue;
    for (const side of ["from", "to"] as const) {
      if (typeof each[side] !== "string") continue;
      const now = to(each[side]);
      each[side] = now.status;
      if (now.label) each[`${side}Label`] = now.label;
    }
  }
  item.status = status;
  item.labels = label ? [label] : [];
  delete item.category;
  return working;
}

// Where the old categories stood, for the order items keep within a status: Next, Later, the custom
// ones as they were listed, Parked, Done.
const placeOf = (category: unknown, custom: string[]): number =>
  category === "next" ? 0 : category === "later" ? 1 : category === "parked" ? custom.length + 2 : category === "done" ? custom.length + 3
    : typeof category === "string" && custom.includes(category) ? custom.indexOf(category) + 2 : custom.length + 2;

// A folder's entries on the disk (where the system can't say so for a folder, as it was).
const onDisk = (folder: string): void => {
  try {
    const handle = fs.openSync(folder, "r");
    try { fs.fsyncSync(handle); } finally { fs.closeSync(handle); }
  } catch {}
};

function migrate(): void {
  fs.mkdirSync(WORK_QUEUE, { recursive: true, mode: 0o700 });
  // A stale/missing status-model marker must never let an older reader rewrite a future file.
  for (const name of queueFiles(QUEUES)) {
    const queue = jsonOf(path.join(QUEUES, name));
    if (isObject(queue)) queueFileFormat.read(queue);
  }
  // What was kept, copied whole, once: made beside and put in place in one step, so a copy cut off
  // is never taken for the whole of it.
  // (A box that kept nothing yet has nothing to copy or move.)
  const labels = jsonOf(LABELS_FILE);
  const keptLabels = isObject(labels) ? labelsFormat.read(labels) : null;
  const categories = jsonOf(CATEGORIES_FILE);
  if (isObject(categories)) categoriesFormat.read(categories);
  const nothing = !queueFiles(QUEUES).length && !fs.existsSync(CATEGORIES_FILE);
  if (nothing && !fs.existsSync(BEFORE)) return void writePrivate(MODEL_FILE, JSON.stringify(modelFormat.write({ model: MODEL, at: Date.now() })));
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
    // (The folders too, then its name: after a power cut it's there whole, or not at all.)
    onDisk(path.join(making, "queues"));
    onDisk(making);
    fs.renameSync(making, BEFORE);
    onDisk(WORK_QUEUE);
  }
  // (Only from the copy: something else by its name is no copy to work from.)
  if (!fs.statSync(path.join(BEFORE, "queues"), { throwIfNoEntry: false })?.isDirectory()) throw new Error(`${BEFORE} isn't the copy of what was kept`);
  const copied = jsonOf(path.join(BEFORE, "categories.json"));
  const kept = isObject(copied) ? categoriesFormat.read(copied) : null;
  const custom = (isObject(kept) && Array.isArray(kept.custom) ? kept.custom : [])
    .filter((each: unknown): each is { id: string; name: string } => isObject(each) && typeof each.id === "string" && /^cat_[a-z0-9-]{1,40}$/.test(each.id) && typeof each.name === "string");
  const names = new Map(custom.map(each => [each.id, each.name]));
  const customIDs = custom.map(each => each.id);
  // Every item as it was, by its id: its status and labels now, and where it stood (its category's
  // place, then its own), which is the order it keeps among those of its status.
  const was: { id: string; category: unknown; order: number; said: Record<string, unknown> }[] = [];
  const started: string[] = [];
  for (const name of queueFiles(path.join(BEFORE, "queues"))) {
    const queue = jsonOf(path.join(BEFORE, "queues", name));
    if (!isObject(queue) || !Array.isArray(queue.items)) continue;
    for (const each of queue.items) {
      if (!isObject(each) || typeof each.id !== "string" || typeof each.status === "string") continue;
      const said = structuredClone(each);
      if (migrateItem(said, id => names.get(id))) started.push(each.id);
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
  // The labels: every custom category, in the colour it was drawn in (but one that's a status now:
  // "Waiting on you"), and Parked where an item was. Made once: a run that finds labels kept
  // already (cut off after making them, or run again where `model.json` was lost) adds only what an
  // item it still has to move over names, never one you removed since.
  {
    const used = new Set(was.flatMap(each => (Array.isArray(each.said.labels) ? each.said.labels as string[] : [])));
    const becameStatus = (each: { id: string; name: string }) => fromCategory(each.id, each.name).label === undefined;
    const made = [
      ...custom.filter(each => !becameStatus(each)).map(each => ({ id: each.id, name: plainName(each.name).slice(0, 40) || each.id.slice(4), color: colorBefore(each.id) })),
      // (A category an item names that the list no longer had: by its id.)
      ...[...used].filter(id => id !== PARKED_LABEL && !names.has(id)).sort().map(id => ({ id, name: id.slice(4).replace(/-/g, " "), color: colorBefore(id) })),
      ...(used.has(PARKED_LABEL) ? [{ id: PARKED_LABEL, name: "Parked", color: "graphite" }] : []),
    ];
    const kept = keptLabels;
    const there: unknown[] = isObject(kept) && Array.isArray(kept.labels) ? kept.labels : [];
    const has = new Set(there.flatMap(each => (isObject(each) && typeof each.id === "string" ? [each.id] : [])));
    const needed = new Set(queueFiles(QUEUES).flatMap(name => {
      const queue = jsonOf(path.join(QUEUES, name));
      return isObject(queue) && Array.isArray(queue.items) ? queue.items.flatMap(each => (isObject(each) && typeof each.status !== "string" ? fromCategory(each.category, typeof each.category === "string" ? names.get(each.category) : undefined).label ?? [] : [])) : [];
    }));
    const more = made.filter(each => !has.has(each.id) && (kept === null || needed.has(each.id)));
    if (more.length) writePrivate(LABELS_FILE, JSON.stringify(labelsFormat.write({ ...kept, labels: [...there, ...more] })));
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
    if (changed) writePrivate(file, JSON.stringify(queueFileFormat.write(queue)));
  }
  // (The categories are in the copy, and labels now: the file nothing reads any more goes.)
  fs.rmSync(CATEGORIES_FILE, { force: true });
  // (`started`: the items a session had said it was on, to be In progress if it's at work now.)
  writePrivate(MODEL_FILE, JSON.stringify(modelFormat.write({ model: MODEL, at: Date.now(), ...(started.length ? { started } : {}) })));
}

/** The items a session had said it was working on when they were moved over (`started`), still to
 * be looked at; none once they have been. */
export function startedThen(): string[] {
  const kept = modelRecord();
  return isObject(kept) && Array.isArray(kept.started) ? kept.started.filter((each): each is string => typeof each === "string") : [];
}

/** They've been looked at: said so, for good. */
export function forgetStarted(): void {
  const kept = modelRecord();
  if (!isObject(kept) || kept.started === undefined) return;
  const { started: _, ...rest } = kept;
  writePrivate(MODEL_FILE, JSON.stringify(modelFormat.write(rest)));
}

/** The items here kept with statuses, from now on: moved over from categories where they weren't
 * yet (see above). Called before anything reads or changes the queues; nothing once it's done. */
export function migrateOnce(): void {
  const kept = modelRecord();
  if (done) return;
  if (isObject(kept) && typeof kept.model === "number" && kept.model === MODEL) {
    done = true;
    return;
  }
  // (A box with no Work Queue yet has nothing to move, and makes no folder for it.)
  if (!fs.existsSync(WORK_QUEUE)) return;
  // (Said at once where nothing can be written there, a lock neither: never waited for.)
  fs.accessSync(WORK_QUEUE, fs.constants.W_OK);
  // One process moves them over, and only one: another that comes meanwhile leaves it to it (its
  // reads go on, its changes are refused until it's done). A mark left by one cut off goes after
  // two minutes.
  try {
    fs.mkdirSync(MOVING);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    const since = fs.statSync(MOVING, { throwIfNoEntry: false })?.mtimeMs ?? 0;
    if (Date.now() - since < 120_000) throw new Error("another process is moving them over now");
    fs.rmSync(MOVING, { recursive: true, force: true });
    fs.mkdirSync(MOVING);
  }
  try {
    locked(LOCK, () => {
      const again = modelRecord();
      if (again?.model !== MODEL) locked(LABELS_LOCK, migrate, { required: true });
    }, { stale: 60_000, wait: 10_000 });
  } finally {
    fs.rmSync(MOVING, { recursive: true, force: true });
  }
  done = true;
}
