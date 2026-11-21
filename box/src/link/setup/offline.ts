import path from "node:path";
import { acceptedUsageUnits, checked, SETTINGS_UNITS, UsageSettings, type SettingsUnit } from "../../usage/settings";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { noteDocsChanged } from "../documents/changed";
import { APP_FOLDER, K } from "../home";
import { editView, fingerprint, mergeEdit, observe, rememberEdit, replay, type Choice, type Fields } from "../ownership/merge";
import type { EditReceipt, EditTarget, OfflineEdit } from "../ownership/edit-types";
import { ownKey, peersOf, KEY } from "../peers/known";
import { keptPersonality, fitsLibrary, mergePersonalities, readPersonalities } from "../personalities/library";
import { retellBots } from "../personalities/take";
import { readLink, updateLink, type LinkState } from "../state";
import { LABEL_COLORS, labelOf, localLabels, MOST_LABELS, setupLabels } from "../work-queue/labels";
import { noteQueuesChanged, Refusal } from "../work-queue/store";
import { publishZoneSetting, readZoneSetting, zoneChoice } from "../zone/setting";
import { primaryFenced } from "./lease";
import { MergeRefusal } from "./merge-data";
import { followedSetup, usesAcceptedSaves } from "./record";
import { operationOutcome, pendingShared, sharedValue, stageShared, type CompanionChanges, type Receipt } from "./shared";

type Kind = Exclude<EditTarget["kind"], "queueItems" | "botSettings" | "sessionNotes">;
const object = (value: unknown): Fields => isObject(value) ? value : {};
const usage = (field: string): field is SettingsUnit => SETTINGS_UNITS.some(unit => unit === field);

export type MergePreferences = Partial<Record<Kind, unknown>>;
export type MergePreferenceChoices = Partial<Record<Kind, Choice[]>>;
const preferenceKinds = ["setupNames", "setupTimeZone", "setupUsageDisplay", "setupPersonalities", "queueLabels"] as const;

export function mergePreferenceChoices(link: LinkState): MergePreferenceChoices {
  return Object.fromEntries(preferenceKinds.flatMap(kind => {
    const choices = link.sharedSetup === link.setup?.id ? link.shared?.[kind]?.offlineEdits?.choices : undefined;
    return choices?.length ? [[kind, choices]] : [];
  }));
}

/** Read the same settings the old libraries exposed before accepted saves existed. */
export function followMergeValues(link: LinkState): MergePreferences {
  const settings = new UsageSettings(path.join(APP_FOLDER ?? K, "agents", "usage", "settings.json"));
  const units = settings.kept();
  const zone = readZoneSetting();
  return {
    setupNames: sharedValue(link, "setupNames") ?? link.peerNames ?? {},
    ...(zone ? { setupTimeZone: zone } : {}),
    setupPersonalities: readPersonalities(),
    queueLabels: setupLabels(sharedValue(link, "queueLabels")) ?? localLabels(),
    ...(sharedValue(link, "setupUsageDisplay") !== undefined
      ? { setupUsageDisplay: sharedValue(link, "setupUsageDisplay") }
      : settings.imported || Object.keys(units).length ? { setupUsageDisplay: { units, imported: settings.imported } } : {}),
  };
}

function portableFields(kind: Kind, value: unknown): Fields {
  if (kind === "setupNames") return object(value);
  if (kind === "setupTimeZone") return typeof object(value).zone === "string" ? { zone: object(value).zone } : {};
  if (kind === "setupUsageDisplay") return Object.fromEntries(Object.entries(object(object(value).units))
    .filter(([key]) => usage(key)).map(([key, entry]) => [key, object(entry).value]));
  if (kind === "queueLabels") return Object.fromEntries((setupLabels(value)?.labels ?? []).flatMap(entry =>
    [[entry.id + ":name", entry.name], [entry.id + ":color", entry.color]]));
  return Object.fromEntries(mergePersonalities(object(value)).personalities.flatMap(entry =>
    (["name", "description", "symbol"] as const).map(field => [entry.id + ":" + field, entry[field]])));
}

/** With unrelated origins, a field present on both sides has no common prior value.
 * Use the same conflict record as two offline edits. Distinct fields and deletions still merge. */
export function mergeSetupPreferences(link: LinkState, incoming: MergePreferences, operation: string, earlier: MergePreferenceChoices): CompanionChanges {
  const local = followMergeValues(link);
  const out: CompanionChanges = {};
  const names = Object.fromEntries(Object.entries(object(incoming.setupNames)).filter(([key, value]) => KEY.test(key) && typeof value === "string"));
  if (Object.keys(names).length) out.setupNames = { value: { ...names, ...object(local.setupNames) } };

  const zone = object(incoming.setupTimeZone).zone;
  if (typeof zone === "string" && zoneChoice(zone)) out.setupTimeZone = { value: local.setupTimeZone ?? zoneChoice(zone) };

  if (isObject(incoming.setupUsageDisplay)) {
    const remote = Object.fromEntries(Object.entries(object(incoming.setupUsageDisplay.units))
      .filter(([key, entry]) => usage(key) && isObject(entry) && checked(key, entry.value) !== undefined && isObject(entry.stamp)));
    if (Object.keys(remote).length) {
      const kept = object(local.setupUsageDisplay);
      out.setupUsageDisplay = { value: { ...kept, imported: kept.imported === true || incoming.setupUsageDisplay.imported === true,
        units: { ...remote, ...object(kept.units) } } };
    }
  }

  const localLibrary = mergePersonalities(object(local.setupPersonalities));
  const remoteLibrary = mergePersonalities(object(incoming.setupPersonalities));
  if (remoteLibrary.personalities.length || remoteLibrary.removed.length) {
    const combined = mergePersonalities(localLibrary, remoteLibrary);
    const existing = new Map(localLibrary.personalities.map(entry => [entry.id, entry]));
    out.setupPersonalities = { value: { ...combined, personalities: combined.personalities.map(entry => existing.get(entry.id) ?? entry) } };
  }

  const keptLabels = setupLabels(local.queueLabels)!;
  const remoteLabels = setupLabels(incoming.queueLabels);
  if (remoteLabels?.labels.length || remoteLabels?.removed.length) {
    const removed = [...new Set([...keptLabels.removed, ...remoteLabels.removed])];
    const left = keptLabels.labels.filter(entry => !removed.includes(entry.id));
    const ids = new Set(left.map(entry => entry.id));
    const names = new Set(left.map(entry => entry.name.toLowerCase()));
    const right = remoteLabels.labels.flatMap(entry => {
      if (removed.includes(entry.id) || ids.has(entry.id)) return [];
      ids.add(entry.id);
      let name = entry.name;
      for (let number = 2; names.has(name.toLowerCase()); number++) {
        const suffix = " (" + number + ")";
        name = entry.name.slice(0, 40 - suffix.length) + suffix;
      }
      names.add(name.toLowerCase());
      return [{ ...entry, name }];
    });
    const labels = [...left, ...right];
    if (labels.length > MOST_LABELS) throw new MergeRefusal("This merge would exceed the setup’s Work Queue label capacity.");
    out.queueLabels = { value: { labels, removed } };
  }

  for (const kind of Object.keys(out) as Kind[]) {
    const fields = portableFields(kind, out[kind]!.value);
    const current = portableFields(kind, local[kind]);
    const remote = portableFields(kind, incoming[kind]);
    const collisions = Object.fromEntries(Object.entries(remote).filter(([key, value]) =>
      value !== null && value !== undefined && current[key] !== null && current[key] !== undefined && Object.hasOwn(fields, key)));
    const record = `${link.setup?.id}:${kind}`;
    const base = observe(undefined, record, {});
    const outline = observe(base, record, fields, false, 8);
    const merged = mergeEdit(outline, { operation, baseRevision: editView(base).revision, fields: collisions }, () => false);
    const history = historyOf(link, kind, fields);
    const joining = Array.isArray(earlier[kind]) ? earlier[kind]!.filter(choice => isObject(choice) && typeof choice.id === "string"
      && typeof choice.field === "string" && Object.hasOwn(fields, choice.field) && normalize(kind, fields, choice.field, choice.proposed) !== undefined)
      .map(choice => ({ ...choice, accepted: fields[choice.field] })) : [];
    const choices = [...new Map([...history.choices, ...merged.choices, ...joining].map(choice => [choice.id, choice])).values()];
    if (choices.length) out[kind]!.offlineEdits = { ...history, choices };
  }
  return out;
}

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
      return { fields: Object.fromEntries(Object.entries(acceptedUsageUnits(units)).map(([key, entry]) => [key, entry.value])), units };
    }
    case "setupPersonalities": {
      const library = readPersonalities();
      return { fields: Object.fromEntries(library.personalities.flatMap(entry =>
        ["name", "description", "symbol"].map(field => [`${entry.id}:${field}`, entry[field as keyof typeof entry]]))), library };
    }
    case "queueLabels": {
      const library = setupLabels(kept) ?? localLabels();
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
      const labels = library.labels.map(entry => {
        return labelOf({ ...entry, name: changes[`${entry.id}:name`] ?? entry.name, color: changes[`${entry.id}:color`] ?? entry.color })!;
      });
      const used = new Map<string, number>();
      for (const label of labels) {
        const name = label.name.toLowerCase();
        used.set(name, (used.get(name) ?? 0) + 1);
      }
      if (labels.some((label, index) => {
        const name = label.name.toLowerCase();
        return name !== library.labels[index]!.name.toLowerCase() && used.get(name)! > 1;
      })) throw new Refusal("There is already a label with that name.");
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

export function editSetup(edit: OfflineEdit, owns: (link: LinkState) => boolean, authorize: (link: LinkState) => boolean): EditReceipt | false | undefined {
  const { ref, operation } = edit;
  const kind = ref.kind as Kind;
  const digest = fingerprint(edit);
  let decision: EditReceipt | false | undefined;
  let save: Receipt | null = null;
  updateLink(link => {
    const setup = followedSetup(link);
    if (!link || setup?.role !== "primary" || !usesAcceptedSaves(setup) || !owns(link) || primaryFenced(link)) return null;
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
      decision = { operation, status: "interrupted", message: "The main computer changed before this edit committed." };
      return null;
    }
    if (!authorize(link)) return null;
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
    const after = observe({ ...history, choices: merged.choices }, `${setup.id}:${kind}`, { ...next.fields, ...merged.changes }, false, 8);
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
