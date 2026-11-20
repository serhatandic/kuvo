// Work Queue references follow the chat rule (ClientText/WorkItemRefs, #596).
import { protectedRanges } from "./markdown";
import { isObject } from "../../shared/json";
import { ITEM_ID, Refusal, type Item } from "./store";

const word = /[\p{L}\p{N}_]/u;
const letters = /\p{L}/u;
const ranks = new Set("ranked rank number result results spot place position priority seller".split(" "));

export function references(text: string, { task = false } = {}): MentionRange[] {
  if (!text.includes("#")) return [];
  const skipped = protectedRanges(text).map(({ start, end }) => ({ start, end }));
  for (const match of text.matchAll(/(?:https?:\/\/|www\.)[^\s<]+|gh:[A-Za-z0-9][A-Za-z0-9_-]{0,38}\/[A-Za-z0-9_.-]{1,100}#[0-9]{1,10}/giu)) {
    skipped.push({ start: match.index, end: match.index + match[0].length });
  }
  skipped.sort((a, b) => a.start - b.start);
  let at = 0;
  const found = [];
  for (const match of text.matchAll(/#([0-9]+)/g)) {
    const start = match.index, end = start + match[0].length;
    const before = start ? text[start - 1] : "";
    if (before && !/\s|[([{“‘"'/,–-]/u.test(before)) continue;
    if (match[1].length > 6 || Number(match[1]) === 0 || word.test(String.fromCodePoint(text.codePointAt(end) ?? 0))) continue;
    while (at < skipped.length && skipped[at].end <= start) at++;
    if (skipped[at]?.start < end) continue;
    const prefix = text.slice(Math.max(0, start - 80), start).trimEnd();
    if (/(?:^|[^\p{L}\p{N}])(?:pr|mr|pull request|issue|step)$/iu.test(prefix)) continue;
    const number = Number(match[1]);
    if (number <= 10) {
      const left = prefix.match(/\p{L}+$/u)?.[0].toLowerCase() ?? "";
      const right = text.slice(end, end + 100).trimStart().toLowerCase();
      const next = right.match(/^\p{L}+/u)?.[0] ?? "";
      const following = right.split(/\s+/, 2).map(part => part.replace(/^\p{P}+|\p{P}+$/gu, ""));
      if (ranks.has(left) || ranks.has(next) || (!task && (next === "on" || next === "in") && letters.test(following[1] ?? "")) || (next === "google" && /^results?$/.test(following[1] ?? ""))) continue;
    }
    found.push({ start, end, number });
  }
  return found;
}

export type RelationKind = "related" | "blocks" | "blocked-by" | "duplicate-of" | "duplicated-by";
const inverse = { related: "related", blocks: "blocked-by", "blocked-by": "blocks", "duplicate-of": "duplicated-by", "duplicated-by": "duplicate-of" } as const;
export interface Relation { item: string; kind: RelationKind }

export function relationOf(value: unknown): value is Relation {
  return isObject(value) && typeof value.item === "string" && ITEM_ID.test(value.item) &&
    typeof value.kind === "string" && Object.hasOwn(inverse, value.kind);
}

export function setRelation(from: Item, to: Item, kind: unknown): void {
  if (from.id === to.id) throw new Refusal("Choose a different item.");
  if (kind !== null && !["related", "blocks", "blocked-by", "duplicate-of"].some(each => each === kind)) {
    throw new Refusal("Choose a kind of link and try again.");
  }
  const kept = kind as RelationKind | null;
  // One record per pair keeps a link edit within a single queue write, even across owners.
  const [first, other] = from.id < to.id ? [from, to] : [to, from];
  const list = (first.relations ?? []).filter(each => each.item !== other.id);
  if (kept !== null) list.push({ item: other.id, kind: first === from ? kept : inverse[kept] });
  if (list.length) first.relations = list;
  else delete first.relations;
}

type Named = Pick<Item, "id" | "title" | "number">;
export type MentionKind = "mentions" | "mentioned-in";
export interface MentionLink { item: string; kind: MentionKind }
export type ItemRelation = Named & { kind: RelationKind | MentionKind };

export function mentionLinkOf(value: unknown): value is MentionLink {
  return isObject(value) && typeof value.item === "string" && ITEM_ID.test(value.item)
    && (value.kind === "mentions" || value.kind === "mentioned-in");
}

export interface MentionRange { start: number; end: number; number: number }
export interface BoundMention extends MentionRange { item?: string }
export interface MentionIndex { body: BoundMention[]; notes: BoundMention[][]; targets: string[] }

const isItemId = (value: unknown): value is string => typeof value === "string" && ITEM_ID.test(value);
const rangesOf = (given: unknown, text: string): BoundMention[] | null => {
  if (!Array.isArray(given)) return null;
  let end = 0;
  for (const range of given) {
    if (!isObject(range) || typeof range.start !== "number" || !Number.isSafeInteger(range.start) || range.start < end ||
        typeof range.end !== "number" || !Number.isSafeInteger(range.end) || range.end > text.length ||
        typeof range.number !== "number" || !/^#[0-9]{1,6}$/.test(text.slice(range.start, range.end)) ||
        range.number !== Number(text.slice(range.start + 1, range.end)) ||
        (range.item !== undefined && !isItemId(range.item))) return null;
    end = range.end;
  }
  return given as BoundMention[];
};

export function mentionIndexOf(given: unknown, body: string, notes: Item["notes"]): MentionIndex | undefined {
  if (!isObject(given) || !Array.isArray(given.notes) || given.notes.length !== notes.length ||
      !Array.isArray(given.targets) || !given.targets.every(isItemId)) return;
  const ranges = rangesOf(given.body, body);
  const noteRanges = given.notes.map((ranges, at) => rangesOf(ranges, notes[at].text));
  return ranges && noteRanges.every(ranges => ranges !== null) ? { body: ranges, notes: noteRanges, targets: given.targets } : undefined;
}

function targetsOf(body: BoundMention[], notes: BoundMention[][]): string[] {
  return [...new Set([body, ...notes].flatMap(ranges => ranges.flatMap(range => range.item ? [range.item] : [])))];
}

type Indexed = Pick<Item, "body" | "notes" | "mentionIndex" | "mentionLinks">;

export interface MentionSnapshot extends Indexed {
  id: string;
  number?: number;
  removed?: Item["removed"];
}

export function mentionSnapshot(item: Item): MentionSnapshot {
  // A few writers append to the notes array in place.
  return { id: item.id, number: item.number, removed: item.removed, body: item.body, notes: item.notes.slice(), mentionIndex: item.mentionIndex, mentionLinks: item.mentionLinks };
}

/** Update changed text only. The saved ranges travel with the item, with the original binding even
 * if its target is now elsewhere or the number is used for a different item. With no previous
 * snapshot (a legacy file or an import), build missing indexes from their source text. */
export function syncMentions(items: Item[], previous = new Map<string, MentionSnapshot>()): void {
  const byNumber = new Map<number, string>();
  const byId = new Map(items.map(item => [item.id, item]));
  for (const item of items) {
    if (!item.removed && item.number !== undefined && !byNumber.has(item.number)) byNumber.set(item.number, item.id);
  }
  const newlyAvailable = new Set<number>();
  for (const [number, id] of byNumber) {
    const before = previous.get(id);
    if (!before || before.removed || before.number !== number) newlyAvailable.add(number);
  }
  for (const item of items) {
    const saved = previous.get(item.id);
    const before = item.mentionIndex && item.mentionIndex !== saved?.mentionIndex ? item : saved ?? item;
    const index = before.mentionIndex;
    const unchanged = index && before.body === item.body && before.notes.length === item.notes.length &&
      before.notes.every((note, at) => note.text === item.notes[at].text);
    if (unchanged && (!newlyAvailable.size || [index.body, ...index.notes].every(ranges =>
      ranges.every(range => range.item || !newlyAvailable.has(range.number) || byNumber.get(range.number) === item.id)))) continue;

    const bindings = new Map<number, string>();
    for (const range of [index?.body ?? [], ...(index?.notes ?? [])].flat()) {
      if (range.item) bindings.set(range.number, range.item);
    }
    for (const link of before.mentionLinks ?? []) {
      const number = byId.get(link.item)?.number;
      if (link.kind === "mentions" && number !== undefined && !bindings.has(number)) bindings.set(number, link.item);
    }
    const bind = (ranges: BoundMention[]) => ranges.map(range => {
      const id = range.item ?? bindings.get(range.number) ?? byNumber.get(range.number);
      return id && id !== item.id && id !== range.item ? { ...range, item: id } : range;
    });
    const body = bind(index && before.body === item.body ? index.body : references(item.body));
    const keptNotes = new Map(before.notes.map((note, at) => [note.text, index?.notes[at]]));
    const notes = item.notes.map(note => bind(keptNotes.get(note.text) ?? references(note.text)));
    item.mentionIndex = { body, notes, targets: targetsOf(body, notes) };
    delete item.mentionLinks;
  }
}

type Endpoint = Named & { removed?: unknown; relations?: Relation[]; mentionIndex?: MentionIndex; mentionTargets?: string[] };
const outgoing = (item: Endpoint) => item.mentionIndex?.targets ?? item.mentionTargets ?? [];

export function relationsOf(item: Endpoint, items: Endpoint[]): ItemRelation[] {
  if (item.removed) return [];
  const live = new Map(items.filter(other => !other.removed).map(other => [other.id, other]));
  const backward: (Relation | MentionLink)[] = [];
  for (const other of live.values()) {
    if (other.id === item.id) continue;
    for (const link of other.relations ?? []) {
      if (link.item === item.id) backward.push({ item: other.id, kind: inverse[link.kind] });
    }
    if (outgoing(other).includes(item.id)) backward.push({ item: other.id, kind: "mentioned-in" });
  }
  const links: (Relation | MentionLink)[] = [
    ...(item.relations ?? []),
    ...outgoing(item).map((id): MentionLink => ({ item: id, kind: "mentions" })),
    ...backward,
  ];
  return links.flatMap(({ item: id, kind }) => {
    const other = live.get(id);
    return other ? [{ id, kind, title: other.title, number: other.number }] : [];
  });
}

/** Manual links are local. A permanently removed or transferred endpoint leaves neither copy. */
export function pruneRelations(items: Item[], gone: ReadonlySet<string>): void {
  if (!gone.size) return;
  for (const item of items) {
    if (!item.relations) continue;
    const kept = item.relations.filter(link => !gone.has(link.item));
    if (kept.length) item.relations = kept;
    else delete item.relations;
  }
}
