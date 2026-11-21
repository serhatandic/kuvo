// The owner runs this under the lock of the record being edited. Clocks are only for retention.
import { createHash } from "node:crypto";
import { isDeepStrictEqual } from "node:util";

export type Fields = Record<string, unknown>;
export type Choice = { id: string; field: string; base: unknown; accepted: unknown; proposed: unknown };
export type Outcome = "committed" | "choice" | "deleted" | "refresh";
export type Decision = { operation: string; status: Outcome; revision: string; choices: string[] };
export type Passages = { base: string; fields: Record<string, Hunk[]> };
type Snapshot = { revision: string; fields: Fields; deleted: boolean; passages?: Passages };
export type EditRecord = { digest: string; decision: Decision };
export interface EditHistory {
  versions: Snapshot[];
  choices: Choice[];
  receipts: EditRecord[];
  additions: Record<string, string>;
}

export const fingerprint = (value: unknown): string => createHash("sha256").update(JSON.stringify(value, (_key, entry) =>
  entry && typeof entry === "object" && !Array.isArray(entry)
    ? Object.fromEntries(Object.keys(entry).sort().map(key => [key, entry[key]]))
    : entry)).digest("base64url");
const current = (history: EditHistory): Snapshot => history.versions.at(-1)!;
const at = (fields: Fields, key: string): unknown => Object.hasOwn(fields, key) ? fields[key] : null;
const same = isDeepStrictEqual;

export function observe(history: EditHistory | undefined, record: string, fields: Fields, deleted = false, limit = 32, passage: (field: string) => boolean = () => false, passages?: Passages): EditHistory {
  const before = history && current(history);
  if (before && before.deleted === deleted && same(before.fields, fields)) {
    const choices = refreshChoices(history!.choices, before.revision, fields, passage);
    return choices === history!.choices ? history! : { ...history!, choices };
  }
  const revision = fingerprint([record, before?.revision, deleted, fields]);
  const versions = [...(history?.versions ?? []), { revision, fields: structuredClone(fields), deleted, ...(passages ? { passages } : {}) }].slice(-limit);
  let bytes = 0;
  // Limit history carried with an item. The latest version remains available even for a large item.
  for (let index = versions.length - 1; index >= 0; index--) {
    bytes += Buffer.byteLength(JSON.stringify(versions[index]));
    if (bytes > (limit === 8 ? 64 * 1024 : 256 * 1024)) {
      versions.splice(0, Math.min(index + 1, versions.length - 1));
      break;
    }
  }
  return {
    versions,
    choices: deleted ? [] : refreshChoices(history?.choices ?? [], revision, fields, passage),
    receipts: history?.receipts ?? [],
    additions: history?.additions ?? {},
  };
}

function refreshChoices(choices: Choice[], revision: string, fields: Fields, passage: (field: string) => boolean): Choice[] {
  if (choices.every(choice => same(choice.accepted, at(fields, choice.field)))) return choices;
  return choices.flatMap(choice => {
    const accepted = at(fields, choice.field);
    if (same(choice.accepted, accepted)) return [choice];
    let proposed = choice.proposed;
    if (passage(choice.field) && typeof accepted === "string" && typeof choice.accepted === "string" && typeof proposed === "string") {
      const combined = mergeText(choice.accepted, accepted, proposed);
      proposed = combined.proposed ?? combined.accepted;
    }
    if (same(accepted, proposed)) return [];
    return [{ id: fingerprint([choice.id, revision]), field: choice.field, base: choice.accepted, accepted, proposed }];
  });
}

export function editView(history: EditHistory) {
  const { revision, fields, deleted } = current(history);
  return { revision, fields, deleted, choices: history.choices };
}

export interface Edit {
  operation: string;
  baseRevision: string | null;
  fields?: Fields;
  delete?: true;
  resolve?: { id: string; use: "accepted" | "proposed" };
}

function baseline(history: EditHistory, edit: Edit) {
  const head = current(history);
  const index = history.versions.findIndex(version => version.revision === edit.baseRevision);
  const base = history.versions[index];
  if (head.deleted) return { status: "deleted" as const };
  if (!base || history.versions.slice(index).some(version => version.deleted)) return { status: "refresh" as const };
  if (edit.resolve && (base !== head || !history.choices.some(choice => choice.id === edit.resolve!.id && same(choice.accepted, at(head.fields, choice.field)))))
    return { status: "refresh" as const };
  return { status: "committed" as const, base, index };
}

export const unavailableBase = (history: EditHistory, edit: Edit) => {
  const { status } = baseline(history, edit);
  return status !== "committed" ? status : undefined;
};

export function mergeEdit(history: EditHistory, edit: Edit, passage: (field: string) => boolean, distinctInsertions = false) {
  const head = current(history);
  const checked = baseline(history, edit);
  const changes: Fields = Object.create(null);
  const passages: Passages = { base: edit.baseRevision ?? "", fields: Object.create(null) };
  let choices = history.choices;
  const added: string[] = [];
  let status: Outcome = checked.status;
  if (checked.status !== "committed") return { status, changes, choices, added, passages: undefined };
  const { base, index } = checked;
  if (edit.resolve) {
    const choice = choices.find(each => each.id === edit.resolve!.id)!;
    if (edit.resolve.use === "proposed") changes[choice.field] = choice.proposed;
    choices = choices.filter(each => each !== choice);
  } else if (!edit.delete) {
    for (const [field, wanted] of Object.entries(edit.fields ?? {})) {
      const original = at(base.fields, field);
      const accepted = at(head.fields, field);
      if (same(wanted, original)) continue;
      const text = passage(field) && typeof original === "string" && typeof accepted === "string" && typeof wanted === "string";
      if (same(wanted, accepted) && !(text && distinctInsertions)) continue;
      const versions = history.versions.slice(index + 1);
      const changed = versions.some(version => !same(at(version.fields, field), original));
      let kept = accepted;
      let proposed = wanted;
      if (text && (changed || distinctInsertions)) {
        const known = distinctInsertions ? versions.toReversed().flatMap(version => {
          const prior = version.passages?.base === base.revision ? version.passages.fields[field] : undefined;
          return prior ? [prior] : [];
        }) : [];
        const combined = mergePassage(original, accepted, wanted, known, distinctInsertions ? edit.operation : undefined);
        if (combined.accepted !== accepted && distinctInsertions) passages.fields[field] = combined.hunks;
        if (combined.accepted !== accepted) changes[field] = kept = combined.accepted;
        if (combined.proposed === undefined) continue;
        proposed = combined.proposed;
      } else if (!changed) {
        changes[field] = wanted;
        continue;
      }
      const id = fingerprint([edit.operation, field]);
      choices = [...choices, { id, field, base: original, accepted: kept, proposed }];
      added.push(id);
      status = "choice";
    }
  }
  return { status, changes, choices, added, passages: Object.keys(passages.fields).length ? passages : undefined };
}

export function replay(history: EditHistory, operation: string, digest: string): Decision | false | undefined {
  const earlier = history.receipts.find(each => each.decision.operation === operation);
  return earlier && (earlier.digest === digest ? earlier.decision : false);
}

export function rememberEdit(history: EditHistory, operation: string, digest: string, status: Outcome, choices: string[]): Decision {
  const decision: Decision = { operation, status, revision: current(history).revision, choices };
  history.receipts = [...history.receipts, { digest, decision }];
  return decision;
}

type Piece = { text: string; operation?: string };
interface Hunk { start: number; end: number; text: string[]; pieces?: Piece[] }

function hunks(base: string[], next: string[]): Hunk[] {
  let start = 0, end = base.length, stop = next.length;
  while (start < end && start < stop && base[start] === next[start]) start++;
  while (end > start && stop > start && base[end - 1] === next[stop - 1]) { end--; stop--; }
  if (start === end && start === stop) return [];
  const width = stop - start + 1;
  // Bound the diff work for unrelated drafts. Common prefixes and suffixes are always kept.
  if ((end - start + 1) * width > 1_000_000) return [{ start, end, text: next.slice(start, stop) }];
  const lcs = new Uint32Array((end - start + 1) * width);
  const index = (i: number, j: number) => (i - start) * width + j - start;
  for (let i = end - 1; i >= start; i--) for (let j = stop - 1; j >= start; j--) {
    lcs[index(i, j)] = base[i] === next[j] ? 1 + lcs[index(i + 1, j + 1)]
      : Math.max(lcs[index(i + 1, j)], lcs[index(i, j + 1)]);
  }
  const edits: Hunk[] = [];
  let run: Hunk | undefined;
  for (let i = start, j = start; i < end || j < stop;) {
    if (i < end && j < stop && base[i] === next[j]) { i++; j++; run = undefined; continue; }
    if (!run) edits.push(run = { start: i, end: i, text: [] });
    if (j < stop && (i === end || lcs[index(i, j + 1)] >= lcs[index(i + 1, j)])) run.text.push(next[j++]);
    else run.end = ++i;
  }
  return edits;
}

function overlaps(a: Hunk, b: Hunk): boolean {
  if (a.start === b.start) return true;
  return a.start < b.end && b.start < a.end;
}

const insertion = (edit: Hunk) => edit.start === edit.end;
const wording = (edit: Hunk) => edit.text.join("");
const sameHunk = (a: Hunk, b: Hunk) => a.start === b.start && a.end === b.end && wording(a) === wording(b);
const byText = (a: Piece, b: Piece) => a.text < b.text ? -1 : a.text > b.text ? 1 : 0;
const ordered = (edits: Hunk[]) => edits.toSorted((a, b) => a.start - b.start);
// Keep whitespace boundaries so inserting an extra newline doesn't replace an existing one.
const split = (text: string) => text.match(/[\p{L}\p{N}_]+|[^\p{L}\p{N}_]/gu) ?? [];
const offsets = (tokens: string[]) => {
  const positions = [0];
  for (const token of tokens) positions.push(positions.at(-1)! + token.length);
  return positions;
};

function apply(source: string[], edits: Hunk[]): string {
  let from = 0;
  const parts: string[] = [];
  for (const edit of edits) {
    parts.push(source.slice(from, edit.start).join(""), wording(edit));
    from = edit.end;
  }
  return parts.concat(source.slice(from).join("")).join("");
}

/** Carry a known edit through changes to otherwise untouched portions of its rendering. */
function extendHunks(source: string[], prior: Hunk[], current: string[], accepted: string): Hunk[] | undefined {
  const before = split(apply(source, prior));
  const changes = hunks(before, current);
  const baseOffsets = offsets(source);
  const baseAt = new Map(baseOffsets.map((position, index) => [position, index]));
  const beforeOffsets = offsets(before);
  // Map unchanged spans back to the base by character offset. Re-tokenizing a rendered
  // insertion can move a trailing period or whitespace across the recorded anchor.
  let read = 0, written = 0;
  const spans = [...prior, { start: source.length, end: source.length, text: [] }].map(edit => {
    const end = written + baseOffsets[edit.start]! - baseOffsets[read]!;
    const span = { start: written, end, shift: baseOffsets[read]! - written };
    read = edit.end;
    written = end + wording(edit).length;
    return span;
  });
  const lifted: Hunk[] = [];
  for (const change of changes) {
    const from = beforeOffsets[change.start]!, to = beforeOffsets[change.end]!;
    const span = spans.find(span => from >= span.start && to <= span.end);
    if (!span) return;
    const start = baseAt.get(from + span.shift), end = baseAt.get(to + span.shift);
    if (start === undefined || end === undefined) return;
    const edit = { ...change, start, end };
    if (prior.some(each => overlaps(each, edit))) return;
    lifted.push(edit);
  }
  const combined = ordered([...prior, ...lifted]);
  return apply(source, combined) === accepted ? combined : undefined;
}

export function mergeText(base: string, accepted: string, proposed: string): { accepted: string; proposed?: string } {
  const { accepted: kept, proposed: alternative } = mergePassage(base, accepted, proposed, []);
  return alternative === undefined ? { accepted: kept } : { accepted: kept, proposed: alternative };
}

function mergePassage(base: string, accepted: string, proposed: string, known: Hunk[][], operation?: string) {
  const source = split(base);
  const rendered = split(accepted);
  // Keep the original anchors while they describe the accepted text. Also try to carry
  // the latest layout through an intervening edit against a newer base.
  const exact = known.find(edits => apply(source, edits) === accepted);
  const extended = !exact && known[0] ? extendHunks(source, known[0], rendered, accepted) : undefined;
  const tagged = exact || extended ? [] : known.flat().filter(edit => edit.pieces);
  const left = exact ?? extended ?? hunks(source, rendered).map(edit =>
    tagged.find(prior => sameHunk(edit, prior)) ?? edit);
  const right = hunks(source, split(proposed));
  const shared = new Set<Hunk>();
  const joined = new Map<Hunk, Hunk>();
  for (const next of right) {
    const prior = left.find(edit => edit.start === next.start);
    if (!prior) continue;
    if (insertion(prior) && insertion(next)) {
      // Equal legacy wordings have no evidence of distinct edits. Identified wordings do.
      if (prior.pieces || !sameHunk(prior, next)) {
        const pieces = [...(prior.pieces ?? [{ text: wording(prior) }]), { text: wording(next), operation }].sort(byText);
        joined.set(prior, { ...prior, text: pieces.map(piece => piece.text), pieces });
      }
      shared.add(next);
    } else if (sameHunk(prior, next)) shared.add(next);
  }
  const onlyRight = right.filter(edit => !shared.has(edit));
  const onlyLeft = left.filter(edit => !right.some(other => sameHunk(edit, other)) && !joined.has(edit));
  const separate = (edits: Hunk[], other: Hunk[]) => edits.filter(edit => !other.some(each => overlaps(edit, each)));
  const rightExtra = separate(onlyRight, left).map(edit => insertion(edit) && operation
    ? { ...edit, pieces: [{ text: wording(edit), operation }] } : edit);
  const kept = ordered([...left.map(edit => joined.get(edit) ?? edit), ...rightExtra]);
  return {
    accepted: apply(source, kept),
    proposed: rightExtra.length === onlyRight.length ? undefined
      : apply(source, ordered([...right.filter(edit => !left.some(prior => joined.has(prior) && prior.start === edit.start)),
        ...joined.values(), ...separate(onlyLeft, right)])),
    hunks: kept,
  };
}
