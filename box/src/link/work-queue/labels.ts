// The Work Queue's labels (PROTOCOL.md, "The Work Queue", "Labels"): what kind of work an item is,
// several on one item, one set for every queue. None is built in: you and your agents make, rename,
// recolour and remove them. Kept in `run/work-queue/labels.json`.

import path from "node:path";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { locked, textOf, writePrivate } from "../files";
import { migratedToChange, noteQueuesChanged, Refusal, WORK_QUEUE } from "./store";

export const LABELS_FILE = path.join(WORK_QUEUE, "labels.json");
const LOCK = path.join(WORK_QUEUE, ".labels.lock");
export const MOST_LABELS = 60;
/** How many labels one item carries at most. */
export const MOST_LABELS_EACH = 10;
const MOST_NAME = 40;

/** A label's colours, by name (the looks' names, as clients draw them). */
export const LABEL_COLORS = ["coral", "orange", "amber", "lime", "mint", "sky", "indigo", "violet", "pink", "graphite"] as const;
const COLOR = new Set<string>(LABEL_COLORS);

export interface Label { id: string; name: string; color: string }

/** A label's id: one made here (`lbl_…`), or a custom category's from before labels (`cat_…`). */
export const LABEL_ID = /^(?:lbl|cat)_[a-z0-9-]{1,40}$/;

/** A label as it was kept or came from another computer, or null for what isn't one. */
export function labelOf(given: unknown): Label | null {
  if (!isObject(given) || typeof given.id !== "string" || !LABEL_ID.test(given.id) || typeof given.name !== "string") return null;
  const name = plainName(given.name).slice(0, MOST_NAME) || given.id.slice(4);
  return { id: given.id, name, color: typeof given.color === "string" && COLOR.has(given.color) ? given.color : "graphite" };
}

// The labels as they're kept, in order; and which of them came from the primary
// (`takePrimaryLabels`).
function readKept(): { labels: Label[]; fromPrimary: string[] } {
  try {
    const kept = JSON.parse(textOf(LABELS_FILE) ?? "null");
    const seen = new Set<string>();
    const labels = (isObject(kept) && Array.isArray(kept.labels) ? kept.labels : []).map(labelOf).filter((each: Label | null): each is Label => each !== null && !seen.has(each.id) && seen.add(each.id) !== undefined);
    const fromPrimary = isObject(kept) && Array.isArray(kept.fromPrimary) ? kept.fromPrimary.filter((each: unknown): each is string => typeof each === "string" && LABEL_ID.test(each)) : [];
    return { labels, fromPrimary };
  } catch {
    return { labels: [], fromPrimary: [] };
  }
}

/** Every label, in the order clients show them (as they were made). */
export const labels = (): Label[] => readKept().labels;

/** The label `given` names (its id or its name, any case), or null for none. */
export function labelNamed(given: unknown, among: Label[] = labels()): Label | null {
  if (typeof given !== "string" || !given.trim()) return null;
  const wanted = given.trim().toLowerCase();
  return among.find(each => each.id === wanted) ?? among.find(each => each.name.toLowerCase() === wanted) ?? null;
}

const slugOf = (name: string) => name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 40);

/** The colour `given` names, or why not. */
function colorFor(given: unknown): string | { refused: string } {
  const wanted = typeof given === "string" ? given.trim().toLowerCase() : "";
  if (wanted === "gray" || wanted === "grey") return "graphite";
  return COLOR.has(wanted) ? wanted : { refused: `${String(given)} isn't a label's colour: use ${LABEL_COLORS.join(", ")}.` };
}

/** What a change says: done (the label now), or why not. */
export type LabelOutcome = { done: Label } | { refused: string };

const change = (work: (kept: { labels: Label[]; fromPrimary: string[] }) => LabelOutcome): LabelOutcome => {
  // (Not before the categories from before statuses are labels: `migrate.ts` makes them.)
  try {
    migratedToChange();
  } catch (error) {
    if (error instanceof Refusal) return { refused: error.message };
    throw error;
  }
  const outcome = locked(LOCK, () => {
    const kept = readKept();
    const outcome = work(kept);
    const fromPrimary = kept.fromPrimary.filter(id => kept.labels.some(each => each.id === id));
    if ("done" in outcome) writePrivate(LABELS_FILE, JSON.stringify({ labels: kept.labels, ...(fromPrimary.length ? { fromPrimary } : {}) }));
    return outcome;
  });
  // (Every queue's view names them.)
  if ("done" in outcome) noteQueuesChanged();
  return outcome;
};

/** A label made: in the colour given, else the one the fewest labels have. */
export function addLabel(name: unknown, color?: unknown): LabelOutcome {
  const named = plainName(name).slice(0, MOST_NAME);
  if (!named) return { refused: "Say what the label is called." };
  const slug = slugOf(named);
  if (!slug) return { refused: `${named} can't be a label's name: use letters or numbers.` };
  const wanted = color === undefined || color === null || color === "" ? null : colorFor(color);
  if (wanted && typeof wanted !== "string") return wanted;
  return change(kept => {
    if (labelNamed(named, kept.labels)) return { refused: `There's a label called ${named} already.` };
    if (kept.labels.length >= MOST_LABELS) return { refused: `There are ${MOST_LABELS} labels already: remove one first.` };
    let id = `lbl_${slug}`;
    for (let n = 2; kept.labels.some(each => each.id === id); n++) id = `lbl_${slug.slice(0, 36)}-${n}`;
    const used = (each: string) => kept.labels.filter(label => label.color === each).length;
    const picked = wanted ?? [...LABEL_COLORS.slice(0, -1)].sort((a, b) => used(a) - used(b))[0];
    const made = { id, name: named, color: picked };
    kept.labels.push(made);
    return { done: made };
  });
}

/** A label renamed, recoloured, or both. */
export function changeLabel(which: unknown, { name, color }: { name?: unknown; color?: unknown }): LabelOutcome {
  const named = name === undefined ? undefined : plainName(name).slice(0, MOST_NAME);
  if (name !== undefined && !named) return { refused: "A label's name can't be empty." };
  if (named !== undefined && !slugOf(named)) return { refused: `${named} can't be a label's name: use letters or numbers.` };
  const wanted = color === undefined ? undefined : colorFor(color);
  if (wanted !== undefined && typeof wanted !== "string") return wanted;
  if (named === undefined && wanted === undefined) return { refused: "Say its new name (name), its new colour (color), or both." };
  return change(kept => {
    const found = labelNamed(which, kept.labels);
    if (!found) return { refused: `There's no label ${String(which ?? "")}.` };
    if (kept.fromPrimary.includes(found.id)) return { refused: thePrimarys(found.name, "Change") };
    const taken = named !== undefined ? labelNamed(named, kept.labels) : null;
    if (taken && taken.id !== found.id) return { refused: `There's a label called ${named} already.` };
    if (named !== undefined) found.name = named;
    if (wanted !== undefined) found.color = wanted;
    return { done: found };
  });
}

// A label this box took from the computer it numbers from: that one's to change or remove, since
// its list comes back here as it tells it (a change made here would be undone at its next word).
const thePrimarys = (name: string, change: "Change" | "Remove"): string =>
  `${name} is one of the labels this computer takes from your first computer (the one that gives the Work Queue its numbers). ${change} it there, and it's ${change === "Remove" ? "gone" : "changed"} here too.`;

/** A label removed: taken off every item here first (`strip`, by its id), then off the list. Two
 * steps, under two locks (the queues', then the labels'): one cut off between them leaves a label
 * no item has, still listed, which removing it again finishes; never an item with a label that's
 * gone. */
export function removeLabel(which: unknown, strip: (id: string) => void): LabelOutcome {
  const found = labelNamed(which);
  if (!found) return { refused: `There's no label ${String(which ?? "")}.` };
  if (readKept().fromPrimary.includes(found.id)) return { refused: thePrimarys(found.name, "Remove") };
  try {
    strip(found.id);
  } catch (error) {
    if (error instanceof Refusal) return { refused: error.message };
    throw error;
  }
  return change(kept => {
    const index = kept.labels.findIndex(each => each.id === found.id);
    if (index < 0) return { refused: `There's no label ${found.name}.` };
    kept.labels.splice(index, 1);
    return { done: found };
  });
}

/** One an item brought (a hand-off, a move) that this box doesn't have yet: added under its own id.
 * Whether it's here now (one more than this box keeps isn't). */
export function keepLabel(given: unknown): boolean {
  const label = labelOf(given);
  if (!label) return false;
  if (labels().some(each => each.id === label.id)) return true;
  return "done" in change(kept => {
    const here = kept.labels.find(each => each.id === label.id);
    if (here) return { done: here };
    if (kept.labels.length >= MOST_LABELS) return { refused: "" };
    kept.labels.push(label);
    return { done: label };
  });
}

/** The labels of the primary this box numbers from, as it told them: its list, in its order, names,
 * colours and all, is this box's; one it removed goes here too, taken off every item here first
 * (`strip`), as it went there. Those only this box has (made here) stay, after them (`used`: how
 * many items have one, for which of them go past the most there may be). */
export function takePrimaryLabels(given: Label[], used: (id: string) => number, strip: (id: string) => void): void {
  const theirs = given.slice(0, MOST_LABELS);
  const ids = new Set(theirs.map(each => each.id));
  const worked = (kept: { labels: Label[]; fromPrimary: string[] }) => {
    const left = kept.labels.filter(each => !ids.has(each.id) && !kept.fromPrimary.includes(each.id));
    // (Past the most there may be, this box's own unused ones go, the last first; never one in use.)
    const all = [...theirs, ...left];
    for (let index = all.length - 1; index >= theirs.length && all.length > MOST_LABELS; index--) if (!used(all[index].id)) all.splice(index, 1);
    return { labels: all, fromPrimary: theirs.map(each => each.id) };
  };
  const same = (a: { labels: Label[]; fromPrimary: string[] }, b: { labels: Label[]; fromPrimary: string[] }) => JSON.stringify(a) === JSON.stringify(b);
  const before = readKept();
  if (same(worked(before), before)) return;
  // (Off the items first, as a removal here goes: never an item with a label that's gone.)
  try {
    for (const id of before.fromPrimary) if (!ids.has(id)) strip(id);
  } catch (error) {
    if (error instanceof Refusal) return;
    throw error;
  }
  change(kept => {
    const now = worked(kept);
    if (same(now, kept)) return { refused: "" };
    kept.labels.splice(0, kept.labels.length, ...now.labels);
    kept.fromPrimary = now.fromPrimary;
    return { done: now.labels[0] ?? { id: "lbl_none", name: "", color: "graphite" } };
  });
}
