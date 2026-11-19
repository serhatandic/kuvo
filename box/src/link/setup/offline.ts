import { checked, SETTINGS_UNITS, type SettingsUnit } from "../../agents/usage/settings";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { noteDocsChanged } from "../documents/changed";
import { editView, fingerprint, mergeEdit, observe, rememberEdit, replay, type Fields } from "../ownership/merge";
import type { EditReceipt, EditTarget, OfflineEdit } from "../ownership/edit-types";
import { ownKey, peersOf, KEY } from "../peers/known";
import { keptPersonality, fitsLibrary, readPersonalities } from "../personalities/library";
import { retellBots } from "../personalities/take";
import { readLink, updateLink, type LinkState } from "../state";
import { LABEL_COLORS, labelOf, labels, setupLabels } from "../work-queue/labels";
import { noteQueuesChanged, Refusal } from "../work-queue/store";
import { publishZoneSetting, readZoneSetting, zoneChoice } from "../zone/setting";
import { primaryFenced } from "./lease";
import { followedSetup, usesAcceptedSaves } from "./record";
import { operationOutcome, pendingShared, sharedValue, stageShared, type Receipt } from "./shared";

type Kind = Exclude<EditTarget["kind"], "queueItems" | "botSettings">;
const object = (value: unknown): Fields => isObject(value) ? value : {};
const usage = (field: string): field is SettingsUnit => SETTINGS_UNITS.some(unit => unit === field);

function projection(link: LinkState, kind: Kind) {
  const kept = sharedValue(link, kind);
  switch (kind) {
    case "setupTimeZone":
      return { fields: { zone: readZoneSetting()?.zone ?? null } };
    case "setupNames": {
      const names = isObject(kept) ? kept : link.peerNames ?? {};
      const boxes = [ownKey(link), ...peersOf(link).map(peer => peer.key)];
      return { fields: Object.fromEntries(boxes.filter((key): key is string => Boolean(key)).map(key => [key, names[key] ?? null])) };
    }
    case "setupUsageDisplay": {
      const units = object(object(kept).units);
      return { fields: Object.fromEntries(Object.entries(units).filter(([key]) => usage(key)).map(([key, entry]) => [key, object(entry).value])), units };
    }
    case "setupPersonalities": {
      const library = readPersonalities();
      return { fields: Object.fromEntries(library.personalities.flatMap(entry =>
        ["name", "description", "symbol"].map(field => [`${entry.id}:${field}`, entry[field as keyof typeof entry]]))), library };
    }
    case "queueLabels": {
      const library = setupLabels(kept) ?? { labels: labels(), removed: [] };
      return { fields: Object.fromEntries(library.labels.flatMap(entry =>
        ["name", "color"].map(field => [`${entry.id}:${field}`, entry[field as "name" | "color"]]))), labelLibrary: library };
    }
  }
}

function historyOf(link: LinkState, kind: Kind, fields: Fields) {
  const prior = link.sharedSetup === link.setup?.id ? link.shared?.[kind]?.offlineEdits : undefined;
  return observe(prior, `${link.setup?.id}:${kind}`, fields, false, 8);
}

export function setupEditView(ref: EditTarget, link = readLink()) {
  if (!link || !usesAcceptedSaves(followedSetup(link))) return null;
  const kind = ref.kind as Kind;
  return editView(historyOf(link, kind, projection(link, kind).fields));
}

function changesOf(kind: Kind, link: LinkState, next: ReturnType<typeof projection>, changes: Fields): unknown {
  const kept = sharedValue(link, kind);
  switch (kind) {
    case "setupTimeZone":
      return Object.hasOwn(changes, "zone") ? zoneChoice(String(changes.zone)) : readZoneSetting();
    case "setupNames":
      return { ...next.fields, ...changes };
    case "setupUsageDisplay": {
      const units = { ...next.units };
      for (const [field, value] of Object.entries(changes)) {
        const count = Number(object(object(units[field]).stamp).v ?? 0) + 1;
        units[field] = { value, stamp: { v: count, by: ownKey(link) } };
      }
      return { ...object(kept), units, imported: object(kept).imported === true };
    }
    case "setupPersonalities": {
      const library = next.library!;
      const personalities = library.personalities.map(entry => {
        const after = { ...entry };
        for (const field of ["name", "description", "symbol"] as const) {
          const key = `${entry.id}:${field}`;
          if (Object.hasOwn(changes, key)) after[field] = String(changes[key]);
        }
        return after.name === entry.name && after.description === entry.description && after.symbol === entry.symbol
          ? entry : { ...after, updated: entry.updated + 1 };
      });
      const value = { ...library, personalities };
      if (!fitsLibrary(value)) throw new Refusal("The personality library has reached its size limit.");
      return value;
    }
    case "queueLabels": {
      const library = next.labelLibrary!;
      const names = new Set<string>();
      const labels = library.labels.map(entry => {
        const after = labelOf({ ...entry, name: changes[`${entry.id}:name`] ?? entry.name, color: changes[`${entry.id}:color`] ?? entry.color })!;
        const name = after.name.toLowerCase();
        if (names.has(name)) throw new Refusal("There is already a label with that name.");
        names.add(name);
        return after;
      });
      return { ...library, labels };
    }
  }
}

function normalize(kind: Kind, fields: Fields, key: string, value: unknown) {
  if (kind === "setupTimeZone") return key === "zone" && typeof value === "string" && zoneChoice(value) ? value : undefined;
  if (kind === "setupNames") return KEY.test(key) && Object.hasOwn(fields, key) ? plainName(value).slice(0, 100) || undefined : undefined;
  if (kind === "setupUsageDisplay") return usage(key) ? checked(key, value) : undefined;
  if (!Object.hasOwn(fields, key) || typeof value !== "string" || !value.trim()) return;
  const [id, field] = key.split(":");
  if (kind === "queueLabels") {
    if (field === "color") return LABEL_COLORS.some(color => color === value) ? value : undefined;
    return labelOf({ id, name: value })?.name;
  }
  const entry = keptPersonality({ id, name: "name", description: "description", [field]: value });
  return entry && "symbol" in entry ? entry[field as "name" | "description" | "symbol"] : undefined;
}

export function editSetup(edit: OfflineEdit, authorize: (link: LinkState) => boolean): EditReceipt | false | undefined {
  const { ref, operation } = edit;
  const kind = ref.kind as Kind;
  const digest = fingerprint(edit);
  let decision: EditReceipt | false | undefined;
  let save: Receipt | null = null;
  updateLink(link => {
    const setup = followedSetup(link);
    if (!link || setup?.role !== "primary" || !usesAcceptedSaves(setup) || !authorize(link) || primaryFenced(link)) return null;
    const next = projection(link, kind);
    const history = structuredClone(historyOf(link, kind, next.fields));
    decision = replay(history, operation, digest);
    if (decision !== undefined || pendingShared(link, kind)) return null;
    const outcome = operationOutcome(link, setup.id, operation);
    if (outcome?.status === "committed" && "offlineEdit" in outcome && outcome.offlineEdit) {
      decision = outcome.offlineEdit.digest === digest ? outcome.offlineEdit.decision : false;
      return null;
    }
    if (outcome?.status === "interrupted") {
      decision = { operation, status: "interrupted", message: "The primary changed before this edit committed." };
      return null;
    }
    const fields: Fields = Object.create(null);
    for (const [field, value] of Object.entries(edit.fields ?? {})) {
      const normalized = normalize(kind, next.fields, field, value);
      if (normalized === undefined) throw new Refusal("That is not an editable setup preference or value.");
      fields[field] = normalized;
    }
    const merged = mergeEdit(history, { ...edit, fields }, () => false);
    if (merged.status === "refresh") {
      decision = rememberEdit(history, operation, digest, "refresh", []);
      return null;
    }
    const changed = changesOf(kind, link, next, merged.changes);
    const after = observe(history, `${setup.id}:${kind}`, { ...next.fields, ...merged.changes }, false, 8);
    after.choices = merged.choices;
    decision = rememberEdit(after, operation, digest, merged.status, merged.added);
    save = stageShared(link, kind, changed, undefined, operation, after);
    if (!save || save.status === "pending") decision = undefined;
    return save ? link : null;
  }, { durable: true });
  if (save) noteDocsChanged();
  if (decision && (decision.status === "committed" || decision.status === "choice")) {
    if (kind === "setupTimeZone") {
      const zone = readZoneSetting();
      if (zone) publishZoneSetting(zone);
    }
    if (kind === "queueLabels") noteQueuesChanged();
    if (kind === "setupPersonalities") {
      const fields = Object.keys(edit.fields ?? {});
      if (edit.resolve) fields.push(...(setupEditView(ref)?.fields ? Object.keys(setupEditView(ref)!.fields) : []));
      retellBots([...new Set(fields.map(field => field.split(":")[0]))]).catch(() => {});
    }
  }
  return decision;
}
