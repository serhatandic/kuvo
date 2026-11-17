// Work Queue references follow the chat rule (ClientText/WorkItemRefs, #596).
import { isObject } from "../../shared/json";
import { ITEM_ID, Refusal, type Item } from "./store";

const word = /[\p{L}\p{N}_]/u;
const letters = /\p{L}/u;
const ranks = new Set("ranked rank number result results spot place position priority seller".split(" "));

export function references(text: string): { start: number; end: number; number: number }[] {
  if (!text.includes("#")) return [];
  const skipped = [...text.matchAll(/!?\[(?:\\.|[^\]\\\n])*\](?:\([^\n)]*\)|\[[^\]\n]*\])|(?:https?:\/\/|www\.)[^\s<]+|gh:[A-Za-z0-9][A-Za-z0-9_-]{0,38}\/[A-Za-z0-9_.-]{1,100}#[0-9]{1,10}/giu)]
    .map(match => [match.index, match.index + match[0].length]);
  for (const tick of text.matchAll(/`+/g)) {
    if (skipped.some(([start, end]) => start <= tick.index && tick.index < end)) continue;
    const end = text.indexOf(tick[0], tick.index + tick[0].length);
    if (end >= 0 || tick[0].length >= 3) skipped.push([tick.index, end >= 0 ? end + tick[0].length : text.length]);
  }
  const found = [];
  for (const match of text.matchAll(/#([0-9]+)/g)) {
    const start = match.index, end = start + match[0].length;
    const before = start ? text[start - 1] : "";
    if (before && !/\s|[([{“‘"'/,–-]/u.test(before)) continue;
    if (match[1].length > 6 || Number(match[1]) === 0 || word.test(String.fromCodePoint(text.codePointAt(end) ?? 0))) continue;
    if (skipped.some(([a, b]) => a < end && b > start)) continue;
    const prefix = text.slice(Math.max(0, start - 80), start).trimEnd();
    if (/(?:^|[^\p{L}\p{N}])(?:pr|mr|pull request|issue|step)$/iu.test(prefix)) continue;
    const number = Number(match[1]);
    if (number <= 10) {
      const left = prefix.match(/\p{L}+$/u)?.[0].toLowerCase() ?? "";
      const right = text.slice(end, end + 100).trimStart().toLowerCase();
      const next = right.match(/^\p{L}+/u)?.[0] ?? "";
      const following = right.split(/\s+/, 2).map(part => part.replace(/^\p{P}+|\p{P}+$/gu, ""));
      if (ranks.has(left) || ranks.has(next) || ((next === "on" || next === "in") && letters.test(following[1] ?? "")) || (next === "google" && /^results?$/.test(following[1] ?? ""))) continue;
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

// BOX-NEEDED: Exchange link identities and mention changes between the computers that keep their endpoints.
function mentionGraph(items: Item[]): Map<string, MentionLink[]> {
  const live = items.filter(item => !item.removed);
  const byNumber = new Map<number, Item>();
  for (const item of live) {
    if (item.number !== undefined && !byNumber.has(item.number)) byNumber.set(item.number, item);
  }
  const graph = new Map<string, MentionLink[]>();
  const link = (from: string, item: string, kind: MentionKind) => {
    const list = graph.get(from) ?? [];
    if (!list.length) graph.set(from, list);
    list.push({ item, kind });
  };
  for (const item of live) {
    const numbers = new Set([item.body, ...item.notes.map(note => note.text)].flatMap(text => references(text).map(ref => ref.number)));
    for (const number of numbers) {
      const other = byNumber.get(number);
      if (!other || other.id === item.id) continue;
      link(item.id, other.id, "mentions");
      link(other.id, item.id, "mentioned-in");
    }
  }
  return graph;
}

/** Keep both ends with the items under the Work Queue lock. The saved descriptions and notes
 * are authoritative if a process stopped halfway through writing two owners' files. */
export function syncMentions(items: Item[]): void {
  const graph = mentionGraph(items);
  for (const item of items) {
    const links = graph.get(item.id);
    if (links) item.mentionLinks = links;
    else delete item.mentionLinks;
  }
}

export function relationsOf(item: Item, items: Item[]): ItemRelation[] {
  const live = new Map(items.filter(other => !other.removed).map(other => [other.id, other]));
  const backward = [...live.values()].flatMap(other => (other.relations ?? [])
    .filter(link => link.item === item.id)
    .map(link => ({ item: other.id, kind: inverse[link.kind] })));
  const links = [...(item.relations ?? []), ...backward, ...(mentionGraph(items).get(item.id) ?? [])];
  return links.flatMap(({ item: id, kind }) => {
    const other = live.get(id);
    return other ? [{ id, kind, title: other.title, number: other.number }] : [];
  });
}
