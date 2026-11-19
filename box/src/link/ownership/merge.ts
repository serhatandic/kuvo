// The owner runs this under the lock of the record being edited. Clocks are only for retention.
import { createHash } from "node:crypto";
import { isDeepStrictEqual } from "node:util";

export type Fields = Record<string, unknown>;
export type Choice = { id: string; field: string; base: unknown; accepted: unknown; proposed: unknown };
export type Outcome = "committed" | "choice" | "deleted" | "refresh";
export type Decision = { operation: string; status: Outcome; revision: string; choices: string[] };
type Snapshot = { revision: string; fields: Fields; deleted: boolean };
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

export function observe(history: EditHistory | undefined, record: string, fields: Fields, deleted = false, limit = 32, passage: (field: string) => boolean = () => false): EditHistory {
  const before = history && current(history);
  if (before && before.deleted === deleted && same(before.fields, fields)) {
    const choices = refreshChoices(history!.choices, before.revision, fields, passage);
    return choices === history!.choices ? history! : { ...history!, choices };
  }
  const revision = fingerprint([record, before?.revision, deleted, fields]);
  const versions = [...(history?.versions ?? []), { revision, fields: structuredClone(fields), deleted }].slice(-limit);
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

export function mergeEdit(history: EditHistory, edit: Edit, passage: (field: string) => boolean) {
  const head = current(history);
  const checked = baseline(history, edit);
  const changes: Fields = Object.create(null);
  let choices = history.choices;
  const added: string[] = [];
  let status: Outcome = checked.status;
  if (checked.status !== "committed") return { status, changes, choices, added };
  const { base, index } = checked;
  if (edit.resolve) {
    const choice = choices.find(each => each.id === edit.resolve!.id)!;
    if (edit.resolve.use === "proposed") changes[choice.field] = choice.proposed;
    choices = choices.filter(each => each !== choice);
  } else if (!edit.delete) {
    for (const [field, wanted] of Object.entries(edit.fields ?? {})) {
      const original = at(base.fields, field);
      const accepted = at(head.fields, field);
      if (same(wanted, original) || same(wanted, accepted)) continue;
      const changed = history.versions.slice(index + 1).some(version => !same(at(version.fields, field), original));
      if (!changed) {
        changes[field] = wanted;
        continue;
      }
      let kept = accepted;
      let proposed = wanted;
      if (passage(field) && typeof original === "string" && typeof accepted === "string" && typeof wanted === "string") {
        const combined = mergeText(original, accepted, wanted);
        changes[field] = kept = combined.accepted;
        if (combined.proposed === undefined) continue;
        proposed = combined.proposed;
      }
      const id = fingerprint([edit.operation, field]);
      choices = [...choices, { id, field, base: original, accepted: kept, proposed }];
      added.push(id);
      status = "choice";
    }
  }
  return { status, changes, choices, added };
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

interface Hunk { start: number; end: number; text: string[] }

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

export function mergeText(base: string, accepted: string, proposed: string): { accepted: string; proposed?: string } {
  const split = (text: string) => text.match(/\s+|[\p{L}\p{N}_]+|[^\s\p{L}\p{N}_]/gu) ?? [];
  const source = split(base);
  const left = hunks(source, split(accepted));
  const right = hunks(source, split(proposed));
  const onlyRight = right.filter(edit => !left.some(other => same(edit, other)));
  const onlyLeft = left.filter(edit => !right.some(other => same(edit, other)));
  const separate = (edits: Hunk[], other: Hunk[]) => edits.filter(edit => !other.some(each => overlaps(edit, each)));
  const rightExtra = separate(onlyRight, left);
  const apply = (edits: Hunk[]) => {
    let from = 0;
    const parts: string[] = [];
    for (const edit of edits.sort((a, b) => a.start - b.start)) {
      parts.push(source.slice(from, edit.start).join(""), edit.text.join(""));
      from = edit.end;
    }
    return parts.concat(source.slice(from).join("")).join("");
  };
  const kept = apply([...left, ...rightExtra]);
  return rightExtra.length === onlyRight.length ? { accepted: kept }
    : { accepted: kept, proposed: apply([...right, ...separate(onlyLeft, right)]) };
}
