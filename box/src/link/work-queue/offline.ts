import { plainName } from "../../shared/text";
import { isObject } from "../../shared/json";
import { editView, fingerprint, mergeEdit, observe, rememberEdit, replay, unavailableBase, type Fields as Values } from "../ownership/merge";
import type { EditTarget, OfflineEdit } from "../ownership/edit-types";
import { assigneeFor, botSession } from "./assignees";
import { tellQueuesAgain } from "./across";
import { numberWaiting, primaryOf } from "./numbers";
import { addItem, applyFileOps, changeItem, handItem, labelsFor, priorityFor, refuseClosedThere, refuseMoving, removeItem, stageClientFiles, statusFor, type Acting, type Fields } from "./ops";
import { queueField, queuePassage, observeQueueItem } from "./offline-fields";
import { changeQueues, dayOf, deletionFor, departureFor, MOST_BODY, MOST_NOTE, MOST_NOTES, MOST_TITLE, Refusal, YOURS } from "./store";

const YOU = { user: true } as const;
const empty = (ref: EditTarget, deleted: boolean) => observe(undefined, ref.id!, {}, deleted);
const absent = (ref: EditTarget) => editView(empty(ref, Boolean(deletionFor(ref.id!))));

export function queueEditView(ref: EditTarget, owns: (ref: EditTarget) => boolean) {
  return changeQueues(book => {
    const item = book.find({ id: ref.id! });
    if (!item) return owns({ ...ref, queue: deletionFor(ref.id!)?.queue ?? ref.queue }) ? absent(ref) : null;
    return owns({ ...ref, queue: item.owner }) ? editView(observeQueueItem(item)) : null;
  }, Date.now(), { durable: true });
}

function queueFields(fields: Values): Fields {
  const result: Fields = Object.fromEntries(Object.entries(fields).filter(([key]) => !key.startsWith("note:") && key !== "order"));
  if (isObject(fields.order)) {
    for (const key of ["before", "after", "top"] as const) if (Object.hasOwn(fields.order, key)) result[key] = fields.order[key];
  }
  return result;
}

function normalizedFields(edit: OfflineEdit): Values {
  const fields: Values = Object.create(null);
  for (const [key, value] of Object.entries(edit.fields ?? {})) {
    if (!queueField(key) || (edit.create && key.startsWith("note:")) || (key === "body" && (typeof value !== "string" || value.length > MOST_BODY))
      || (key.startsWith("note:") && (typeof value !== "string" || !value.trim() || value.length > MOST_NOTE))
      || (key === "order" && (!isObject(value) || Object.keys(value).length !== 1
        || !(value.top === true || typeof value.before === "string" || typeof value.after === "string"))))
      throw new Refusal("That is not an editable Work Queue field or value.");
    switch (key) {
      case "title": {
        const title = typeof value === "string" && value.length <= MOST_TITLE * 4 ? plainName(value).slice(0, MOST_TITLE) : "";
        if (!title) throw new Refusal("Say a title in one line.");
        fields.title = title;
        break;
      }
      case "status": fields.status = statusFor(value); break;
      case "priority": fields.priority = priorityFor(value) ?? null; break;
      case "labels": fields.labels = labelsFor(value); break;
      case "assignee": fields.assignee = assigneeFor(value); break;
      case "due": {
        const due = dayOf(value);
        if (!due && value !== null && value !== "") throw new Refusal("Give the due day as YYYY-MM-DD.");
        fields.due = due;
        break;
      }
      default: fields[key] = value;
    }
  }
  return fields;
}

export function editQueue(edit: OfflineEdit, owns: (ref: EditTarget) => boolean) {
  const { ref, operation } = edit;
  const digest = fingerprint(edit);
  const now = Date.now();
  const primary = primaryOf();
  const result = changeQueues(book => {
    let item = book.find({ id: ref.id! });
    const target = { ...ref, queue: item?.owner ?? deletionFor(ref.id!)?.queue ?? ref.queue };
    if (!owns(target) || item?.handing) return;
    const deleted = !item && Boolean(deletionFor(ref.id!));
    let history = item ? observeQueueItem(item) : empty(ref, deleted);
    const earlier = replay(history, operation, digest);
    if (earlier !== undefined) return earlier;
    if (!item && (deleted || !edit.create || departureFor(ref.id!))) return { operation, status: "refresh" as const, revision: editView(history).revision, choices: [] };
    if (edit.create && item) return false;
    // Check the age and deletion fence before validating values against today's labels or bots.
    const unavailable = item && unavailableBase(history, edit);
    if (unavailable) return rememberEdit(history, operation, digest, unavailable, []);
    const fields = normalizedFields(edit);
    const merged = item ? mergeEdit(history, { ...edit, fields }, queuePassage)
      : { status: "committed" as const, changes: fields, choices: [], added: [] };
    const notes: NonNullable<OfflineEdit["notes"]> = [];
    const attachments: NonNullable<OfflineEdit["attachments"]> = [];
    const additions = { ...history.additions };
    const append = <T extends { id: string }>(kind: string, given: T[], fresh: T[]) => {
      for (const entry of given) {
        const key = `${kind}:${entry.id}`;
        const hash = fingerprint(entry);
        if (Object.hasOwn(additions, key)) {
          if (additions[key] !== hash) throw new Refusal("That addition id was used with different content.");
          continue;
        }
        if (kind === "note" && item?.notes.some(note => note.id === entry.id)) throw new Refusal("That note id is already in use.");
        additions[key] = hash;
        fresh.push(entry);
      }
    };
    append("note", edit.notes ?? [], notes);
    append("attachment", edit.attachments ?? [], attachments);
    if ((item?.notes.length ?? 0) + notes.length > MOST_NOTES) throw new Refusal("This item has reached its note limit.");
    const files = attachments.length ? stageClientFiles(attachments.map(each => each.file), false) : undefined;
    try {
      const fields = queueFields(merged.changes);
      if (files) {
        fields.files = attachments;
        files.staged.forEach((file, index) => { file.id = attachments[index]!.id; });
      }
      const acting: Acting = { who: YOU, now, staged: files?.staged, fileOps: [] };
      if (!item) item = addItem(book, target.queue!, fields, acting, primary, undefined, ref.id);
      else if (edit.delete) removeItem(item, YOU, now);
      else if (Object.keys(merged.changes).length || files) {
        refuseClosedThere(item);
        if (item.source && (fields.body !== undefined || fields.title !== undefined || files))
          throw new Refusal("Change that item's title, description or attachments at its source.");
        for (const [field, text] of Object.entries(merged.changes)) {
          if (!field.startsWith("note:")) continue;
          const note = item.notes.find(each => each.id === field.slice(5));
          if (!note || typeof text !== "string" || text.length > MOST_NOTE) throw new Refusal("That note could not be updated.");
          note.text = text;
        }
        changeItem(book, item, fields, acting);
        const assignTo = item.owner === YOURS && fields.assignee !== undefined ? botSession(item.assignee) : null;
        if (assignTo) {
          refuseMoving([assignTo]);
          handItem(book, item, assignTo, YOU, now);
        }
      }
      item.notes.push(...notes.map(note => ({ ...note, by: YOU, at: now })));
      applyFileOps(acting);
      numberWaiting(book, primary, null);
      item.offlineEdits = { ...history, choices: merged.choices };
      history = observeQueueItem(item);
      history.additions = additions;
      return rememberEdit(history, operation, digest, merged.status, merged.added);
    } finally { files?.clear(); }
  }, now, { durable: true });
  if (result && (result.status === "committed" || result.status === "choice")) tellQueuesAgain();
  return result;
}
