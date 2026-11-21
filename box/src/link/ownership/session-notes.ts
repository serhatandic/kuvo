import fs from "node:fs";
import path from "node:path";
import { locked, syncDurableFile, writeDurablePrivate } from "../../shared/files";
import { isObject } from "../../shared/json";
import { KEY } from "../peers/known";
import { K, RUN } from "../home";
import { MOST_NOTE, Refusal } from "../work-queue/store";
import { MUTATION_ID } from "./mutation";
import type { EditTarget, OfflineEdit } from "./edit-types";
import { editView, fingerprint, mergeEdit, observe, rememberEdit, replay, type EditHistory } from "./merge";

export const SESSION_NOTE_FEATURE = "data.session-notes";
export const NOTE_SESSION = /^ses_[A-Za-z0-9_-]{1,95}$/;
export const SESSION_NOTE_ROUTE = /^kuvo\/sessions\/(ses_[A-Za-z0-9_-]{1,95})\/notes(?:\/([A-Za-z0-9_-]{1,80}))?$/;
const ROOT = path.join(RUN, "work", "notes");
const MAX_STORE = 4 * 1024 * 1024;
const fileOf = (session: string) => path.join(ROOT, session + ".ordinary.json");
export const hasSessionNotes = (session: string) => NOTE_SESSION.test(session) && fs.existsSync(fileOf(session));
type Notes = { version: 1; move?: string; notes: Record<string, EditHistory> };
const blank = (): Notes => ({ version: 1, notes: Object.create(null) });

const key = (value: unknown): value is string => typeof value === "string" && KEY.test(value);
const text = (value: unknown): value is string => typeof value === "string" && value.length <= MOST_NOTE;
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(each => typeof each === "string");
const offset = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;

function passageHistory(value: unknown): boolean {
  if (value === undefined) return true;
  if (!isObject(value) || !key(value.base) || !isObject(value.fields) || !Array.isArray(value.fields.text)
    || Object.keys(value.fields).length !== 1) return false;
  let end = 0;
  for (const hunk of value.fields.text) {
    if (!isObject(hunk) || !offset(hunk.start) || !offset(hunk.end) || hunk.start < end || hunk.end < hunk.start
      || !strings(hunk.text) || (hunk.pieces !== undefined && (hunk.start !== hunk.end || !Array.isArray(hunk.pieces)
        || !hunk.pieces.length || hunk.pieces.some(piece => !isObject(piece) || !text(piece.text)
          || (piece.operation !== undefined && !key(piece.operation)))
        || hunk.pieces.map(piece => piece.text).join("") !== hunk.text.join("")))) return false;
    end = hunk.end;
  }
  return true;
}

function noteHistory(value: unknown): boolean {
  return isObject(value) && Array.isArray(value.versions) && value.versions.length > 0
    && value.versions.every(version => isObject(version) && key(version.revision) && typeof version.deleted === "boolean"
      && isObject(version.fields) && (version.deleted ? Object.keys(version.fields).length === 0
        : Object.keys(version.fields).length === 1 && text(version.fields.text)) && passageHistory(version.passages))
    && Array.isArray(value.choices) && value.choices.every(choice => isObject(choice) && key(choice.id) && choice.field === "text"
      && text(choice.base) && text(choice.accepted) && text(choice.proposed))
    && Array.isArray(value.receipts) && value.receipts.every(receipt => isObject(receipt) && key(receipt.digest)
      && isObject(receipt.decision) && key(receipt.decision.operation) && key(receipt.decision.revision)
      && ["committed", "choice", "deleted", "refresh"].includes(String(receipt.decision.status))
      && Array.isArray(receipt.decision.choices) && receipt.decision.choices.every(key))
    && isObject(value.additions);
}

function checked(value: unknown): Notes {
  if (!isObject(value) || value.version !== 1 || (value.move !== undefined && typeof value.move !== "string")
    || !isObject(value.notes) || Object.entries(value.notes).some(([id, note]) => !MUTATION_ID.test(id) || !noteHistory(note)))
    throw new Refusal("This computer cannot read that session's ordinary notes.");
  const notes = value as Notes;
  return { ...notes, notes: Object.assign(Object.create(null), notes.notes) };
}

function read(session: string): Notes | undefined {
  try { return checked(JSON.parse(fs.readFileSync(fileOf(session), "utf8"))); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return undefined;
  }
}

function underLock<T>(session: string, work: () => T): T {
  if (!NOTE_SESSION.test(session)) throw new Refusal("Name a session for these notes.");
  fs.mkdirSync(ROOT, { recursive: true, mode: 0o700 });
  return locked(fileOf(session) + ".lock", work, { exclusive: true });
}

function save(session: string, notes: Notes, allowTombstones = false): void {
  const text = JSON.stringify(notes);
  if (Buffer.byteLength(text) > (allowTombstones ? 2 * MAX_STORE : MAX_STORE)) throw new Refusal("This session has reached its ordinary note storage limit.");
  writeDurablePrivate(fileOf(session), text, K);
}

const record = (ref: EditTarget) => `${ref.id}/${ref.note}`;
const noNote = (ref: EditTarget) => observe(undefined, record(ref), {});
const passage = (field: string) => field === "text";

export function sessionNoteView(ref: EditTarget) {
  const notes = read(ref.id!)?.notes;
  if (ref.note) return editView(notes?.[ref.note] ?? noNote(ref));
  const ids = Object.entries(notes ?? {}).filter(([, history]) => !editView(history).deleted).map(([id]) => id).sort();
  return editView(observe(undefined, ref.id!, { ids }));
}

export function editSessionNote(edit: OfflineEdit, owns: () => boolean, authorize: () => boolean) {
  const { ref, operation } = edit;
  return underLock(ref.id!, () => {
    if (!owns()) return;
    const saved = read(ref.id!) ?? blank();
    const existing = saved.notes[ref.note!];
    let history = existing ?? noNote(ref);
    const digest = fingerprint(edit);
    const earlier = replay(history, operation, digest);
    if (Object.entries(saved.notes).some(([id, note]) => id !== ref.note && replay(note, operation, digest) !== undefined))
      return false;
    if (earlier !== undefined) return earlier;
    if (!authorize()) return;
    if (edit.create && existing && !editView(history).deleted) return false;
    if (!existing && !edit.create) return { operation, status: "refresh" as const, revision: editView(history).revision, choices: [] };
    if (edit.fields && (Object.keys(edit.fields).length !== 1 || typeof edit.fields.text !== "string"
      || !edit.fields.text.trim() || edit.fields.text.length > MOST_NOTE))
      throw new Refusal("Send the ordinary note's text.");
    if (!existing) {
      if (!edit.fields) throw new Refusal("Send the ordinary note's text.");
      history = observe(undefined, record(ref), { text: edit.fields.text });
    } else if (edit.create) {
      // A note ID is never reused, even when the original was deleted.
      return { operation, status: "deleted" as const, revision: editView(history).revision, choices: [] };
    }
    const merged = existing ? mergeEdit(history, edit, passage, true) : undefined;
    if (merged?.status === "committed" || merged?.status === "choice") {
      const fields = edit.delete ? {} : { ...editView(history).fields, ...merged.changes };
      if (typeof fields.text === "string" && fields.text.length > MOST_NOTE)
        throw new Refusal("The combined note would exceed its text limit.");
      const after = observe({ ...history, choices: merged.choices }, record(ref), fields, Boolean(edit.delete), undefined, passage, merged.passages);
      if (after.choices.some(choice => !history.choices.includes(choice)
        && typeof choice.proposed === "string" && choice.proposed.length > MOST_NOTE))
        throw new Refusal("The combined alternative would exceed the note's text limit.");
      if (edit.delete) after.versions = after.versions.slice(-1);
      history = after;
    }
    const decision = rememberEdit(history, operation, digest, merged?.status ?? "committed", merged?.added ?? []);
    saved.notes[ref.note!] = history;
    save(ref.id!, saved, Boolean(edit.delete));
    return decision;
  });
}

export function packSessionNotes(sessions: readonly string[]): Record<string, Notes> | undefined {
  const entries = sessions.filter(session => NOTE_SESSION.test(session)).flatMap(session => {
    const notes = underLock(session, () => read(session));
    return notes ? [[session, notes] as const] : [];
  });
  return entries.length ? Object.fromEntries(entries) : undefined;
}

export function placeSessionNotes(value: unknown, sessions: readonly string[], move: string): void {
  if (value === undefined) return;
  if (!isObject(value)) throw new Refusal("The ordinary session notes did not come through.");
  const entries = Object.entries(value).map(([session, notes]) => {
    if (!sessions.includes(session)) throw new Refusal("The move includes notes for another session.");
    return [session, checked(notes)] as const;
  });
  for (const [session, notes] of entries) underLock(session, () => {
    const prior = read(session);
    if (prior && prior.move !== move) throw new Refusal("Ordinary notes for that session already exist at the destination.");
    save(session, { ...notes, move }, true);
  });
}

export function forgetSessionNotes(sessions: readonly string[], move?: string): void {
  for (const session of sessions) {
    if (!hasSessionNotes(session)) continue;
    const file = fileOf(session);
    underLock(session, () => {
      if (move && read(session)?.move !== move) return;
      fs.rmSync(file, { force: true });
      syncDurableFile(ROOT);
    });
  }
}
