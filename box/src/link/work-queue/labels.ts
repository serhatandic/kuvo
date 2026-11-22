// The Work Queue's labels (PROTOCOL.md, "The Work Queue", "Labels"): what kind of work an item is,
// several on one item, one set for every queue. None is built in: you and your agents make, rename,
// recolour and remove them. Kept in `run/work-queue/labels.json`.

import path from "node:path";
import { UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { locked, textOf, writePrivate } from "../files";
import { LEADERSHIP_WAIT, primaryFenced } from "../setup/lease";
import { changeShared, sharedValue, type Receipt } from "../setup/shared";
import { followedSetup, usesAcceptedSaves } from "../setup/record";
import { readLink, type LinkState } from "../state";
import { primaryOf } from "./numbers";
import { LABELS_LOCK, labelsFormat } from "./migrate";
import { migratedToChange, noteQueuesChanged, Refusal, WORK_QUEUE } from "./store";

export const LABELS_FILE = path.join(WORK_QUEUE, "labels.json");
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
interface Kept { labels: Label[]; removed: string[]; fromPrimary: string[]; primary?: string; primaryAt?: number }

const removedIds = (given: unknown): string[] => Array.isArray(given)
  ? given.filter((id): id is string => typeof id === "string" && LABEL_ID.test(id)) : [];
const labelSetup = (): string | null => followedSetup(readLink())?.id ?? null;

// This computer follows another primary's labels while it numbers from that primary.
const follows = () => !("self" in primaryOf());

function readKept(setup = labelSetup()): Kept {
  try {
    const value: unknown = JSON.parse(textOf(LABELS_FILE) ?? "null");
    const kept = isObject(value) ? labelsFormat.read(value) : {};
    const removed = kept.removedSetup === setup ? removedIds(kept.removed) : [];
    const seen = new Set(removed);
    const labels = (Array.isArray(kept.labels) ? kept.labels : []).map(labelOf).filter((each: Label | null): each is Label => each !== null && !seen.has(each.id) && seen.add(each.id) !== undefined);
    const fromPrimary = Array.isArray(kept.fromPrimary) ? kept.fromPrimary.filter((each: unknown): each is string => typeof each === "string" && LABEL_ID.test(each)) : [];
    return {
      labels, removed, fromPrimary,
      ...(typeof kept.primary === "string" ? { primary: kept.primary } : {}),
      ...(typeof kept.primaryAt === "number" ? { primaryAt: kept.primaryAt } : {}),
    };
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return { labels: [], removed: [], fromPrimary: [] };
  }
}

/** Every label, in the order clients show them (as they were made). */
export const labels = (): Label[] => setupLabels(sharedValue(readLink(), "queueLabels"))?.labels ?? readKept().labels;

/** The library underneath the accepted view, including ids from the category migration. */
export function localLabels() {
  const { labels, removed } = readKept();
  return { labels, removed };
}

export function setupLabels(value: unknown) {
  if (!isObject(value) || !Array.isArray(value.labels) || !Array.isArray(value.removed)) return null;
  return {
    labels: value.labels.map(labelOf).filter((each): each is Label => each !== null),
    removed: removedIds(value.removed),
  };
}

type RestoreResult = { restored: number; alreadyPresent: number; conflicts: number; details: { id: string; reason: string }[]; provisional?: true };
type RestoreOutcome = { result: RestoreResult; save?: Receipt } | { status: 400 | 409; message: string };

/** Restore definitions only. A saved library's removal list has no effect on the current one. */
export function restoreLabels(library: unknown, dryRun: boolean, force: boolean): RestoreOutcome {
  const entries = isObject(library) ? library.labels : library;
  if (!Array.isArray(entries) || entries.length > MOST_LABELS) {
    return { status: 400, message: `Give a saved list of up to ${MOST_LABELS} label definitions.` };
  }
  const wanted = new Map<string, Label>();
  for (const entry of entries) {
    const label = labelOf(entry);
    if (!label || label.name !== entry.name || label.color !== entry.color || wanted.has(label.id)) {
      return { status: 400, message: `Invalid or duplicate label at position ${wanted.size + 1} in the saved list.` };
    }
    wanted.set(label.id, label);
  }

  const plan = (value: unknown, link: LinkState | null) => {
    const setup = followedSetup(link);
    if (setup?.role !== "primary" || primaryFenced(link)) return { message: LEADERSHIP_WAIT };
    const next = setupLabels(value) ?? localLabels();
    const removed = new Set(next.removed);
    const result: RestoreResult = { restored: 0, alreadyPresent: 0, conflicts: 0, details: [] };
    for (const label of wanted.values()) {
      const existing = next.labels.find(each => each.id === label.id);
      if (existing?.name === label.name && existing.color === label.color) {
        result.alreadyPresent++;
        continue;
      }
      const reason = existing ? "This ID already has a different name or color."
        : removed.has(label.id) && !force ? "This label was removed. Use --force to restore it."
        : labelNamed(label.name, next.labels) ? "Another label already uses this name."
        : next.labels.length >= MOST_LABELS ? "The label library is full."
        : null;
      if (reason) {
        result.details.push({ id: label.id, reason });
        continue;
      }
      next.labels.push(label);
      removed.delete(label.id);
      result.restored++;
    }
    next.removed = [...removed];
    result.conflicts = result.details.length;
    return { result, next };
  };

  if (dryRun) {
    const link = readLink();
    const accepted = sharedValue(link, "queueLabels");
    const preview = plan(accepted, link);
    if (!preview.result) return { status: 409, message: preview.message };
    return { result: { ...preview.result, ...(accepted === undefined ? { provisional: true } : {}) } };
  }
  const proposed: { outcome?: ReturnType<typeof plan> } = {};
  const save = changeShared("queueLabels", (value, link) => {
    const outcome = proposed.outcome = plan(value, link);
    return outcome.result?.restored ? outcome.next : undefined;
  });
  const outcome = proposed.outcome;
  if (!outcome?.result) return { status: 409, message: outcome?.message ?? LEADERSHIP_WAIT };
  if (save) return { result: outcome.result, save };
  return outcome.result.restored ? { status: 409, message: "The label library couldn't be saved." } : { result: outcome.result };
}

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
type LocalLabelOutcome = { done: Label } | { refused: string };
export type LabelOutcome = LocalLabelOutcome | { done: Label; save: Receipt };

function editLabels(work: (kept: Kept) => LocalLabelOutcome): LabelOutcome {
  const setup = readLink()?.setup;
  if (!usesAcceptedSaves(setup)) return change(work);
  if (setup?.role !== "primary") return { refused: "Change your setup's labels on its main computer." };
  migratedToChange();
  const result: { outcome: LocalLabelOutcome } = { outcome: { refused: "The main computer could not save the label change." } };
  const save = changeShared("queueLabels", value => {
    const before = setupLabels(value) ?? localLabels();
    result.outcome = work({ ...before, fromPrimary: [] });
    return "done" in result.outcome ? before : undefined;
  });
  if ("refused" in result.outcome) return result.outcome;
  return save ? { done: result.outcome.done, save } : { refused: "The main computer could not save the label change." };
}

const change = (work: (kept: Kept) => LabelOutcome): LabelOutcome => {
  // (Not before the categories from before statuses are labels: `migrate.ts` makes them.)
  try {
    migratedToChange();
  } catch (error) {
    if (error instanceof Refusal) return { refused: error.message };
    throw error;
  }
  const outcome = locked(LABELS_LOCK, () => {
    const setup = labelSetup();
    const kept = readKept(setup);
    const outcome = work(kept);
    const fromPrimary = kept.fromPrimary.filter(id => kept.labels.some(each => each.id === id));
    if ("done" in outcome) writePrivate(LABELS_FILE, JSON.stringify(labelsFormat.write({
      labels: kept.labels, ...(kept.removed.length ? { removed: kept.removed, removedSetup: setup } : {}),
      ...(fromPrimary.length ? { fromPrimary } : {}), ...(kept.primary ? { primary: kept.primary } : {}),
      ...(kept.primaryAt !== undefined ? { primaryAt: kept.primaryAt } : {}),
    })));
    return outcome;
  }, { required: true });
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
  return editLabels(kept => {
    if (labelNamed(named, kept.labels)) return { refused: `There's a label called ${named} already.` };
    if (kept.labels.length >= MOST_LABELS) return { refused: `There are ${MOST_LABELS} labels already: remove one first.` };
    let id = `lbl_${slug}`;
    for (let n = 2; (kept.removed.includes(id) || kept.labels.some(each => each.id === id)); n++) id = `lbl_${slug.slice(0, 36)}-${n}`;
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
  return editLabels(kept => {
    const found = labelNamed(which, kept.labels);
    if (!found) return { refused: `There's no label ${String(which ?? "")}.` };
    if (kept.fromPrimary.includes(found.id) && follows()) return { refused: thePrimarys(found.name, "Change") };
    const taken = named !== undefined ? labelNamed(named, kept.labels) : null;
    if (taken && taken.id !== found.id) return { refused: `There's a label called ${named} already.` };
    if (named !== undefined) found.name = named;
    if (wanted !== undefined) found.color = wanted;
    return { done: found };
  });
}

// A label this box took from the computer it numbers from: that one's to change or remove, since
// its list comes back here as it tells it (a change made here would be undone at its next word).
// Only while this box numbers from another: once it's the first computer itself, they're its own.
const thePrimarys = (name: string, change: "Change" | "Remove"): string =>
  `${name} is one of the labels this computer takes from your first computer (the one that gives the Work Queue its numbers). ${change} it there, and it's ${change === "Remove" ? "gone" : "changed"} here too.`;

/** A label removed: taken off every item here first (`strip`, by its id), then off the list. Two
 * steps, under two locks (the queues', then the labels'): one cut off between them leaves a label
 * no item has, still listed, which removing it again finishes. (An agent that gives an item the
 * label between the two leaves its id on that item, which no view lists once the label's gone.) */
export function removeLabel(which: unknown, strip: (id: string) => void): LabelOutcome {
  const found = labelNamed(which);
  if (!found) return { refused: `There's no label ${String(which ?? "")}.` };
  const remove = (kept: Kept): LocalLabelOutcome => {
    const index = kept.labels.findIndex(each => each.id === found.id);
    if (index < 0) return { refused: `There's no label ${found.name}.` };
    kept.labels.splice(index, 1);
    if (!kept.removed.includes(found.id)) kept.removed.push(found.id);
    return { done: found };
  };
  if (usesAcceptedSaves(readLink()?.setup)) return editLabels(remove);
  if (readKept().fromPrimary.includes(found.id) && follows()) return { refused: thePrimarys(found.name, "Remove") };
  try {
    strip(found.id);
  } catch (error) {
    if (error instanceof Refusal) return { refused: error.message };
    throw error;
  }
  return change(remove);
}

/** One an item brought (a hand-off, a move) that this box doesn't have yet: added under its own id.
 * Whether it's here now (one more than this box keeps isn't). */
export function keepLabel(given: unknown): boolean {
  const label = labelOf(given);
  if (!label) return false;
  const accepted = setupLabels(sharedValue(readLink(), "queueLabels"));
  if (accepted?.removed.includes(label.id)) return false;
  if ((accepted?.labels ?? readKept().labels).some(each => each.id === label.id)) return true;
  return "done" in change(kept => {
    if (kept.removed.includes(label.id)) return { refused: "" };
    const here = kept.labels.find(each => each.id === label.id);
    if (here) return { done: here };
    if (kept.labels.length >= MOST_LABELS) return { refused: "" };
    kept.labels.push(label);
    return { done: label };
  });
}

/** The labels of the primary this box numbers from (`key`), as it told them: its list, in its
 * order, names, colours and all, is this box's (one of the same id made here is the primary's
 * from then on: one set for every queue); one it removed goes here too, taken off every item here
 * first (`strip`), as it went there. Only the same primary's word removes one: where the primary
 * is another computer than before, or tells no labels at all (a list it lost, say), those it no
 * longer names stay, as this box's own. Those only this box has stay, after the primary's (`used`:
 * how many items have one, for which of them go past the most there may be). */
export function takePrimaryLabels(given: Label[], used: (id: string) => number, strip: (id: string) => void,
  { key, at, previousAt, complete }: { key: string; at: number; previousAt?: number; complete: boolean }): void {
  const theirs = given.slice(0, MOST_LABELS);
  const ids = new Set(theirs.map(each => each.id));
  // A peer summary may move its clock backwards. Only advancing label reports can
  // establish a deletion, even if the summary cache has already accepted a reset.
  const samePrimary = (kept: Kept) => kept.primary === undefined || kept.primary === key;
  const watermark = (kept: Kept) => Math.max(samePrimary(kept) ? kept.primaryAt ?? -Infinity : -Infinity, previousAt ?? -Infinity);
  const trusted = (kept: Kept) => complete && at > watermark(kept);
  const removes = (kept: Kept) => theirs.length > 0 && trusted(kept) && samePrimary(kept)
    ? kept.fromPrimary.filter(id => !ids.has(id)) : [];
  const worked = (kept: Kept): Kept => {
    if (!trusted(kept)) {
      // Older label files have no watermark. Remember the previous report even
      // when the summary cache replaces it with a clock-reset report.
      const primaryAt = watermark(kept);
      return samePrimary(kept) && Number.isFinite(primaryAt) ? { ...kept, primary: key, primaryAt } : kept;
    }
    const gone = new Set(removes(kept));
    const left = kept.labels.filter(each => !ids.has(each.id) && !gone.has(each.id));
    // (Past the most there may be, this box's own unused ones go, the last first; never one in use.)
    const removed = new Set([...kept.removed, ...gone]);
    const takes = theirs.filter(label => !removed.has(label.id));
    const all = [...takes, ...left];
    for (let index = all.length - 1; index >= takes.length && all.length > MOST_LABELS; index--) if (!used(all[index].id)) all.splice(index, 1);
    const primaryAt = theirs.length ? at : kept.primary === key ? kept.primaryAt : undefined;
    return {
      labels: all, removed: [...removed], fromPrimary: takes.map(each => each.id), primary: key,
      ...(primaryAt !== undefined ? { primaryAt } : {}),
    };
  };
  const same = (a: Kept, b: Kept) => JSON.stringify(a) === JSON.stringify(b);
  const before = readKept();
  if (same(worked(before), before)) return;
  // (Off the items first, as a removal here goes.)
  try {
    for (const id of removes(before)) strip(id);
  } catch (error) {
    if (error instanceof Refusal) return;
    throw error;
  }
  change(kept => {
    const now = worked(kept);
    if (same(now, kept)) return { refused: "" };
    kept.labels.splice(0, kept.labels.length, ...now.labels);
    kept.removed = now.removed;
    kept.fromPrimary = now.fromPrimary;
    kept.primary = key;
    kept.primaryAt = now.primaryAt;
    return { done: now.labels[0] ?? { id: "lbl_none", name: "", color: "graphite" } };
  });
}
