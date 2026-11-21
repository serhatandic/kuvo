// Setup preferences with pre-checkpoint copies. Membership, the number ledger,
// ownership assignments and release approval come from their authoritative writers.
import path from "node:path";
import { isDeepStrictEqual } from "node:util";
import { acceptedUsageUnits, unitsFrom, UsageSettings } from "../../usage/settings";
import { DATA_DECLARATIONS, SHARED_KINDS, sharedKind, sharedPolicy, type SharedKind } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { noteDocsChanged } from "../documents/changed";
import { APP_FOLDER, K } from "../home";
import { fingerprint, observe, type Choice, type Fields } from "../ownership/merge";
import { KEY, ownKey, peersOf } from "../peers/known";
import { mergeSetupPersonalities, personalityFollowers, readLocalPersonalities } from "../personalities/library";
import { retellBots } from "../personalities/take";
import type { LinkHost } from "../relay/host";
import { readLink, updateLink, type LinkState } from "../state";
import { takesForwarded } from "../work-queue/forward";
import { localLabels, setupLabels } from "../work-queue/labels";
import { mailsFor } from "../work-queue/parts";
import { localZoneForSeed, publishZoneSetting, readZoneSetting, seedZoneOf } from "../zone/setting";
import { primaryFenced } from "./lease";
import { followedSetup, usesAcceptedSaves } from "./record";
import { MAX_SHARED_BYTES, seedSources, SharedCapacity, sharedValue, stageShared, withSeedSources, type Receipt, type SeedSnapshot, type SharedValues } from "./shared";

export const SHARED_SEED_FEATURE = "setup.shared-seed";
type SeedKind = {
  [Kind in SharedKind]: typeof DATA_DECLARATIONS[Kind]["shared"] extends { seed: "member copies" } ? Kind : never
}[SharedKind];
const seedKinds = SHARED_KINDS.filter((kind): kind is SeedKind => sharedPolicy(kind).seed === "member copies");
type Copies = Partial<Record<SeedKind, unknown>>;
type Copy = { key: string; sign?: string; values: Copies; oversized?: SeedKind[] };
type Entry = NonNullable<SharedValues[SeedKind]>;
type Merge = { value: unknown; fields: Fields; choices: Choice[] };
const object = (value: unknown): Fields => isObject(value) ? value : {};
const equal = isDeepStrictEqual;

// A pre-checkpoint copy has no common accepted revision. Keep the first value of
// a disputed field visible and carry the alternatives through the offline edit API.
function choose(choices: Choice[], field: string, accepted: unknown, proposed: unknown) {
  if (equal(accepted, proposed)) return;
  const id = fingerprint(["seed", field, accepted, proposed]);
  if (!choices.some(choice => choice.id === id)) choices.push({ id, field, base: null, accepted, proposed });
}

function union<T>(
  prior: Record<string, T>, incoming: Record<string, T>, combine: (key: string, accepted: T, proposed: T) => T,
): Record<string, T> {
  const merged = { ...prior };
  for (const [key, value] of Object.entries(incoming)) {
    merged[key] = Object.hasOwn(prior, key) ? combine(key, prior[key]!, value) : value;
  }
  return merged;
}

const byID = <T extends { id: string }>(records: T[]): Record<string, T> =>
  Object.fromEntries(records.map(record => [record.id, record]));
const fieldsOf = <T extends { id: string }>(records: T[], fields: readonly (keyof T & string)[]): Fields =>
  Object.fromEntries(records.flatMap(record => fields.map(field => [record.id + ":" + field, record[field]])));

type MergeField = <T>(field: string, accepted: T, proposed: T) => T;

type SeedPolicy = {
  read: (link: LinkState) => unknown;
  merge: (prior: unknown, incoming: unknown, field: MergeField, roster: Set<string>) => Omit<Merge, "choices">;
  empty?: (value: unknown) => boolean;
};

const preferences: Record<SeedKind, SeedPolicy> = {
  queueLabels: {
    read: localLabels,
    empty: value => {
      const library = setupLabels(value);
      return !library?.labels.length && !library?.removed.length;
    },
    merge: (prior, incoming, field) => {
      const before = setupLabels(prior) ?? { labels: [], removed: [] };
      const next = setupLabels(incoming);
      const removed = new Set([...before.removed, ...(next?.removed ?? [])]);
      const live = (labels: typeof before.labels) => byID(labels.filter(label => !removed.has(label.id)));
      const labels = Object.values(union(live(before.labels), live(next?.labels ?? []),
        (id, a, b) => ({ id, name: field(id + ":name", a.name, b.name), color: field(id + ":color", a.color, b.color) })));
      return { value: { labels, removed: [...removed] }, fields: fieldsOf(labels, ["name", "color"]) };
    },
  },
  setupPersonalities: {
    read: () => readLocalPersonalities(),
    empty: value => {
      const library = mergeSetupPersonalities(object(value));
      return !library.personalities.length && !library.removed.length;
    },
    merge: (prior, incoming, field) => {
      const before = mergeSetupPersonalities(object(prior));
      const next = mergeSetupPersonalities(object(incoming));
      const removed = mergeSetupPersonalities({ removed: [...before.removed, ...next.removed] }).removed;
      const gone = new Set(removed.map(entry => entry.id));
      const live = (entries: typeof before.personalities) => byID(entries.filter(entry => !gone.has(entry.id)));
      const personalities = Object.values(union(live(before.personalities), live(next.personalities), (id, a, b) => {
        const name = field(id + ":name", a.name, b.name);
        const description = field(id + ":description", a.description, b.description);
        const symbol = field(id + ":symbol", a.symbol, b.symbol);
        return name === a.name && description === a.description && symbol === a.symbol
          ? a : { id, name, description, symbol, updated: Math.max(a.updated, b.updated) };
      }));
      return { value: { personalities, removed }, fields: fieldsOf(personalities, ["name", "description", "symbol"]) };
    },
  },
  setupNames: {
    read: link => ({
      ...(link.seedNames?.setup === link.setup?.id ? link.seedNames?.names : {}),
      ...link.peerNames,
    }),
    merge: (prior, incoming, field, roster) => {
      const names = Object.fromEntries(Object.entries(object(incoming)).flatMap(([key, value]) => {
        const name = typeof value === "string" ? plainName(value).slice(0, 100) : "";
        return KEY.test(key) && roster.has(key) && name ? [[key, name]] : [];
      }));
      const fields = union<unknown>(object(prior), names, field);
      return { value: prior !== undefined || Object.keys(fields).length ? fields : undefined, fields };
    },
  },
  setupTimeZone: {
    read: () => localZoneForSeed(),
    merge: (prior, incoming, field) => {
      const proposed = seedZoneOf(incoming);
      const zone = object(prior).zone;
      const value = proposed && (typeof zone !== "string" || field("zone", zone, proposed.zone) !== zone)
        ? proposed : prior;
      return { value, fields: value ? { zone: object(value).zone } : {} };
    },
  },
  setupUsageDisplay: {
    read: link => {
      const usage = new UsageSettings(path.join(APP_FOLDER ?? K, "agents", "usage", "settings.json"));
      if (usage.setup !== undefined && usage.setup !== link.setup?.id) return;
      return { units: usage.kept(), imported: usage.imported };
    },
    merge: (prior, incoming, field) => {
      const before = object(prior), next = object(incoming);
      const units = acceptedUsageUnits(before.units);
      for (const [key, proposed] of Object.entries(unitsFrom(next.units))) {
        const accepted = units[key];
        const value = field(key, accepted?.value, proposed.value);
        if (value !== undefined && value !== accepted?.value) units[key] = proposed;
      }
      const imported = before.imported === true || next.imported === true;
      return {
        value: prior !== undefined || Object.keys(units).length || imported ? { units, imported } : undefined,
        fields: Object.fromEntries(Object.entries(units).map(([key, unit]) => [key, unit.value])),
      };
    },
  },
};

function localSetupCopies(link: LinkState, kinds = seedKinds): Copies {
  const copies: Copies = {};
  for (const kind of kinds) {
    try {
      copies[kind] = preferences[kind].read(link) ?? null;
    } catch {
      // A newer or unreadable file can be offered again once this computer can read it.
    }
  }
  return copies;
}

export function offerSetupCopies(link: LinkState, only: unknown): { copies: Copies; oversized: SeedKind[] } {
  const copies = localSetupCopies(link, memberSeedKind(only) ? [only] : seedKinds);
  const oversized: SeedKind[] = [];
  for (const kind of seedKinds) {
    if (Object.hasOwn(copies, kind) && Buffer.byteLength(JSON.stringify(copies[kind])) > MAX_SHARED_BYTES) {
      delete copies[kind];
      oversized.push(kind);
    }
  }
  return { copies, oversized };
}

function currentCopies(link: LinkState, own: string, gathered: Copy[], only?: SeedKind): Copy[] {
  const members = peersOf(link);
  const local = offerSetupCopies(link, only);
  return [{ key: own, values: local.copies, oversized: local.oversized },
    ...gathered.filter(copy => members.some(peer => peer.key === copy.key && peer.sign === copy.sign))];
}

function mergeCopies(kind: SeedKind, before: Entry | undefined, incoming: unknown[], roster: Set<string>, baseline?: SeedSnapshot): Merge {
  const { merge } = preferences[kind];
  let current = { value: before?.value, fields: {} as Fields };
  const choices = [...(before?.offlineEdits?.choices ?? [])];
  for (const copy of incoming) {
    current = merge(current.value, copy, (field, accepted, proposed) => {
      const base = baseline?.fields[field];
      if (base !== undefined) {
        if (base === fingerprint(proposed)) return accepted;
        if (accepted === undefined) return proposed;
        if (base === fingerprint(accepted)) return proposed;
      }
      if (accepted === undefined) return proposed;
      choose(choices, field, accepted, proposed);
      return accepted;
    }, roster);
  }
  return { ...current, choices: choices.filter(choice => Object.hasOwn(current.fields, choice.field)) };
}

function history(setup: string, kind: SeedKind, prior: Entry | undefined, merged: Merge) {
  const previous = prior?.offlineEdits;
  if (previous?.versions.length || merged.choices.length) {
    return observe({
      versions: previous?.versions ?? [], receipts: [], additions: previous?.additions ?? {},
      choices: merged.choices,
    }, `${setup}:${kind}`, merged.fields, false, 8);
  }
  return previous;
}

// A content revision compares the exact local value that was gathered with the value
// still on the member after acceptance, regardless of clocks, restarts or file format.
function snapshotRevisions(copies: Copy[], kind: SeedKind, roster: Set<string>): Record<string, SeedSnapshot> {
  return Object.fromEntries(copies.flatMap(copy => {
    if (!Object.hasOwn(copy.values, kind)) return [];
    const value = copy.values[kind];
    const { fields } = preferences[kind].merge(undefined, value, (_field, accepted, proposed) =>
      accepted === undefined ? proposed : accepted, roster);
    return [[copy.key, { revision: fingerprint(value),
      fields: Object.fromEntries(Object.entries(fields).map(([field, value]) => [field, fingerprint(value)])) }]];
  }));
}

function contributions(link: LinkState, source: string) {
  const locals = localSetupCopies(link);
  return seedKinds.flatMap(kind => {
    const value = locals[kind];
    if (value === undefined || value === null || preferences[kind].empty?.(value)) return [];
    const before = sharedValue(link, kind) === undefined ? undefined : link.shared?.[kind];
    const revisions = seedSources(before?.offlineEdits);
    if (before && (!revisions || revisions[source]?.revision === fingerprint(value))) return [];
    return [{ kind, value, revision: before?.sequence ?? 0 }];
  });
}

function canSeed(link: LinkState | null, payload: Record<string, unknown>, source: string) {
  const setup = followedSetup(link);
  return link && setup?.role === "primary" && setup.id === payload.setup && setup.epoch === payload.epoch
    && usesAcceptedSaves(setup) && !link.localWipe && !primaryFenced(link)
    && (source === ownKey(link) || peersOf(link).some(peer => peer.key === source)) ? setup : null;
}

const SEED_WAIT = { status: 503, message: "The main computer could not save setup preferences." } as const;
const SEED_CAPACITY = { status: 409, message: "These existing preferences exceed the setup’s saved preference capacity." } as const;

async function applySetupCopy(source: string, position: Record<string, unknown>, kind: SeedKind, supplied?: { value: unknown; revision: number }): Promise<Receipt | "nothing" | "capacity" | null> {
  const result: { outcome: Receipt | "nothing" | "capacity" | null } = { outcome: null };
  updateLink(link => {
    const setup = canSeed(link, position, source);
    if (!link || !setup) return null;
    const own = setup.primary;
    const roster = new Set([own, ...peersOf(link).map(peer => peer.key)]);
    const before = sharedValue(link, kind) === undefined ? undefined : link.shared?.[kind];
    if (before && supplied === undefined) {
      result.outcome = "nothing";
      return null;
    }
    const revisions = seedSources(before?.offlineEdits);
    if (supplied && (supplied.revision !== (before?.sequence ?? 0) || before && !revisions
      || revisions?.[source]?.revision === fingerprint(supplied.value))) return null;
    const copies = before ? [] : currentCopies(link, own, [], kind);
    if (copies.some(copy => copy.oversized?.includes(kind))) {
      result.outcome = "capacity";
      return null;
    }
    if (supplied && !copies.some(copy => copy.key === source && Object.hasOwn(copy.values, kind))) {
      copies.push({ key: source, values: { [kind]: supplied.value } });
    }
    const merged = mergeCopies(kind, before, copies.map(copy => copy.values[kind]), roster, revisions?.[source]);
    if (merged.value === undefined || !before && preferences[kind].empty?.(merged.value)) {
      result.outcome = "nothing";
      return null;
    }
    const sources = Object.fromEntries(Object.entries(revisions ?? {}).filter(([key]) => roster.has(key)));
    const offlineEdits = withSeedSources(history(setup.id, kind, before, merged),
      { ...sources, ...snapshotRevisions(copies, kind, roster) });
    try {
      result.outcome = stageShared(link, kind, merged.value, undefined, undefined, offlineEdits, { reportCapacity: true });
    } catch (error) {
      if (!(error instanceof SharedCapacity)) throw error;
      result.outcome = "capacity";
      return null;
    }
    return result.outcome ? link : null;
  }, { durable: true });
  const saved = result.outcome;
  if (!saved || typeof saved === "string") return saved;
  noteDocsChanged();

  if (kind === "setupTimeZone") {
    const zone = readZoneSetting();
    if (zone) publishZoneSetting(zone);
  }
  if (kind === "setupPersonalities") {
    for (const tell of personalityFollowers) tell();
    const value = sharedValue(readLink(), kind);
    const library = mergeSetupPersonalities(object(value));
    retellBots([...library.personalities, ...library.removed].map(entry => entry.id)).catch(() => {});
  }
  return saved;
}

function memberSeedKind(kind: unknown): kind is SeedKind {
  return sharedKind(kind) && sharedPolicy(kind).seed === "member copies";
}

export async function takeSetupCopy(source: string, payload: Record<string, unknown>): Promise<void> {
  if (!memberSeedKind(payload.kind) || !isObject(payload.value) || !Number.isSafeInteger(payload.revision)
    || Number(payload.revision) < 0 || Buffer.byteLength(JSON.stringify(payload.value)) > MAX_SHARED_BYTES) return;
  await applySetupCopy(source, payload, payload.kind, { value: payload.value, revision: Number(payload.revision) });
}

/** Seed the main’s local preferences before the first edit. Members offer their copies in the background. */
export async function setupSeedRefusal(kind: SharedKind): Promise<typeof SEED_WAIT | typeof SEED_CAPACITY | null> {
  if (!memberSeedKind(kind)) return null;
  const link = readLink();
  const setup = followedSetup(link);
  if (setup?.role !== "primary" || !usesAcceptedSaves(setup) || sharedValue(link, kind) !== undefined) return null;
  const save = await applySetupCopy(setup.primary, { setup: setup.id, epoch: setup.epoch }, kind);
  if (save === "capacity") return SEED_CAPACITY;
  if (save) return null;
  return SEED_WAIT;
}

let nextRound = 0;
/** Reoffer on later rounds until accepted, including after a restart or a primary change. */
export async function sendSetupCopies(host: Pick<LinkHost, "send" | "welcomed"> | null | undefined, now = Date.now()): Promise<void> {
  if (now < nextRound) return;
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || !setup || !usesAcceptedSaves(setup)) return;
  const source = ownKey(link);
  if (!source) return;
  const primary = setup.role === "member" ? peersOf(link).find(peer => peer.key === setup.primary) : undefined;
  if (setup.role === "member" && (!host?.welcomed || !primary || !takesForwarded(primary.key, SHARED_SEED_FEATURE))) return;
  nextRound = now + 15_000;
  const position = { setup: setup.id, epoch: setup.epoch };
  for (const copy of contributions(link, source)) {
    const payload = { ...position, ...copy };
    if (!primary) await takeSetupCopy(source, payload);
    else for (const bytes of mailsFor(primary, { setupCopy: payload }, link, now) ?? []) host?.send(bytes);
  }
}
