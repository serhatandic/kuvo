// Setup preferences with pre-checkpoint copies. Membership, the number ledger,
// ownership assignments and release approval come from their authoritative writers.
import path from "node:path";
import { isDeepStrictEqual } from "node:util";
import { SETTINGS_UNITS, unitsFrom, UsageSettings } from "../../agents/usage/settings";
import { DATA_DECLARATIONS, SHARED_KINDS, sharedKind, sharedPolicy, type SharedKind } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { botHere } from "../bots/here";
import { noteDocsChanged } from "../documents/changed";
import { listOf } from "../files";
import { APP_FOLDER, BOTS, K } from "../home";
import { fingerprint, observe, type Choice, type Fields } from "../ownership/merge";
import { KEY, ownKey, peersOf } from "../peers/known";
import { mergeSetupPersonalities, personalityFollowers, readLocalPersonalities } from "../personalities/library";
import { retellBots } from "../personalities/take";
import type { LinkHost } from "../relay/host";
import { readLink, updateLink, type LinkState } from "../state";
import { forwardAct, takesForwarded } from "../work-queue/forward";
import { localLabels, setupLabels } from "../work-queue/labels";
import { mailsFor } from "../work-queue/parts";
import { allItems } from "../work-queue/store";
import { localZoneForSeed, publishZoneSetting, readZoneSetting, seedZoneOf } from "../zone/setting";
import { primaryFenced } from "./lease";
import { followedSetup, usesAcceptedSaves } from "./record";
import { MAX_SHARED_BYTES, pendingShared, seedSources, SharedCapacity, sharedValue, stageShared, waitShared, withSeedSources, type Receipt, type SharedValues } from "./shared";

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
  prior: Record<string, T>, incoming: Record<string, T>, conflict: (key: string, accepted: T, proposed: T) => void,
): Record<string, T> {
  const merged = { ...prior };
  for (const [key, value] of Object.entries(incoming)) {
    if (Object.hasOwn(prior, key)) conflict(key, prior[key]!, value);
    else merged[key] = value;
  }
  return merged;
}

const byID = <T extends { id: string }>(records: T[]): Record<string, T> =>
  Object.fromEntries(records.map(record => [record.id, record]));
const fieldsOf = <T extends { id: string }>(records: T[], fields: readonly (keyof T & string)[]): Fields =>
  Object.fromEntries(records.flatMap(record => fields.map(field => [record.id + ":" + field, record[field]])));

type SeedPolicy = {
  read: (link: LinkState) => unknown;
  merge: (prior: unknown, incoming: unknown, initial: boolean, conflict: (field: string, accepted: unknown, proposed: unknown) => void, roster: Set<string>) => Omit<Merge, "choices">;
  empty?: (value: unknown) => boolean;
  recover?: (local: unknown, accepted: unknown) => unknown;
};

const preferences: Record<SeedKind, SeedPolicy> = {
  queueLabels: {
    read: () => ({ labels: localLabels(), removed: [] }),
    empty: value => !setupLabels(value)?.labels.length,
    merge: (prior, incoming, _initial, conflict) => {
      const before = setupLabels(prior) ?? { labels: [], removed: [] };
      const next = setupLabels(incoming);
      const removed = new Set(before.removed);
      const labels = Object.values(union(byID(before.labels), byID((next?.labels ?? []).filter(label => !removed.has(label.id))),
        (id, a, b) => {
          conflict(id + ":name", a.name, b.name);
          conflict(id + ":color", a.color, b.color);
        }));
      return { value: { labels, removed: before.removed }, fields: fieldsOf(labels, ["name", "color"]) };
    },
    recover: (local, accepted) => {
      const before = setupLabels(accepted);
      const unavailable = new Set([...(before?.labels.map(label => label.id) ?? []), ...(before?.removed ?? [])]);
      const missing = (setupLabels(local)?.labels ?? []).filter(label => !unavailable.has(label.id));
      if (!missing.length) return;
      const referenced = new Set(allItems().filter(item => !item.removed).flatMap(item => item.labels));
      const labels = missing.filter(label => referenced.has(label.id));
      return labels.length ? { labels, removed: [] } : undefined;
    },
  },
  setupPersonalities: {
    read: () => readLocalPersonalities(),
    empty: value => {
      const library = mergeSetupPersonalities(object(value));
      return !library.personalities.length && !library.removed.length;
    },
    merge: (prior, incoming, initial, conflict) => {
      const before = mergeSetupPersonalities(object(prior));
      const next = mergeSetupPersonalities(object(incoming));
      const active = new Set(before.personalities.map(entry => entry.id));
      const removed = mergeSetupPersonalities({ removed: [...before.removed,
        ...next.removed.filter(entry => initial || !active.has(entry.id))],
      }).removed;
      const gone = new Set(removed.map(entry => entry.id));
      const live = (entries: typeof before.personalities) => byID(entries.filter(entry => !gone.has(entry.id)));
      const personalities = Object.values(union(live(before.personalities), live(next.personalities), (id, a, b) => {
        for (const field of ["name", "description", "symbol"] as const) conflict(id + ":" + field, a[field], b[field]);
      }));
      return { value: { personalities, removed }, fields: fieldsOf(personalities, ["name", "description", "symbol"]) };
    },
    recover: (local, accepted) => {
      const before = mergeSetupPersonalities(object(accepted));
      const unavailable = new Set([...before.personalities, ...before.removed].map(entry => entry.id));
      const missing = mergeSetupPersonalities(object(local)).personalities.filter(entry => !unavailable.has(entry.id));
      if (!missing.length) return;
      const referenced = new Set(listOf(BOTS).map(id => botHere(id)?.described.personality));
      const personalities = missing.filter(entry => referenced.has(entry.id));
      return personalities.length ? { personalities, removed: [] } : undefined;
    },
  },
  setupNames: {
    read: link => ({
      ...(link.seedNames?.setup === link.setup?.id ? link.seedNames?.names : {}),
      ...link.peerNames,
    }),
    merge: (prior, incoming, _initial, conflict, roster) => {
      const names = Object.fromEntries(Object.entries(object(incoming)).flatMap(([key, value]) => {
        const name = typeof value === "string" ? plainName(value).slice(0, 100) : "";
        return KEY.test(key) && roster.has(key) && name ? [[key, name]] : [];
      }));
      const fields = union<unknown>(object(prior), names, conflict);
      return { value: prior !== undefined || Object.keys(fields).length ? fields : undefined, fields };
    },
  },
  setupTimeZone: {
    read: () => localZoneForSeed(),
    merge: (prior, incoming, _initial, conflict) => {
      const proposed = seedZoneOf(incoming);
      const accepted = prior ?? proposed;
      if (proposed && typeof object(accepted).zone === "string") conflict("zone", object(accepted).zone, proposed.zone);
      const value = accepted;
      return { value, fields: value ? { zone: object(value).zone } : {} };
    },
  },
  setupUsageDisplay: {
    read: link => {
      const usage = new UsageSettings(path.join(APP_FOLDER ?? K, "agents", "usage", "settings.json"));
      if (usage.setup !== undefined && usage.setup !== link.setup?.id) return;
      return { units: usage.kept(), imported: usage.imported };
    },
    merge: (prior, incoming, _initial, conflict) => {
      const before = object(prior), next = object(incoming);
      const units = union(unitsFrom(before.units), unitsFrom(next.units), (key, a, b) => {
        if (SETTINGS_UNITS.some(unit => unit === key)) conflict(key, a.value, b.value);
      });
      const imported = before.imported === true || next.imported === true;
      return {
        value: prior !== undefined || Object.keys(units).length || imported ? { units, imported } : undefined,
        fields: Object.fromEntries(Object.entries(units).filter(([key]) => SETTINGS_UNITS.some(unit => unit === key))
          .map(([key, unit]) => [key, unit.value])),
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

export async function gatherSetupCopies(link: LinkState, position: Record<string, unknown>, only?: SeedKind): Promise<Copy[]> {
  const signal = AbortSignal.timeout(4000);
  const peers = peersOf(link).filter(peer => peer.sign && takesForwarded(peer.key, SHARED_SEED_FEATURE));
  const copies = await Promise.all(peers.map(async peer => {
    const reply = await forwardAct(peer, "Reading setup preferences", {
      area: "setup", method: "GET", item: peer.key, body: { ...position, sharedSeed: true, only },
    }, signal, link, Date.now(), { backgroundRelay: true });
    if ("refused" in reply || reply.status !== 200 || !isObject(reply.value) || reply.value.sign !== peer.sign
      || !isObject(reply.value.copies)) return [];
    const oversized = Array.isArray(reply.value.oversized) ? reply.value.oversized.filter(memberSeedKind) : [];
    return [{ key: peer.key, sign: peer.sign, values: reply.value.copies, oversized }];
  }));
  return copies.flat();
}

function currentCopies(link: LinkState, own: string, gathered: Copy[], only?: SeedKind): Copy[] {
  const members = peersOf(link);
  const local = offerSetupCopies(link, only);
  return [{ key: own, values: local.copies, oversized: local.oversized },
    ...gathered.filter(copy => members.some(peer => peer.key === copy.key && peer.sign === copy.sign))];
}

function mergeCopies(kind: SeedKind, before: Entry | undefined, incoming: unknown[], initial: boolean, conflicts: boolean, roster: Set<string>): Merge {
  const { merge } = preferences[kind];
  let current = { value: before?.value, fields: {} as Fields };
  const choices = [...(before?.offlineEdits?.choices ?? [])];
  for (const copy of incoming) {
    current = merge(current.value, copy, initial, (field, a, b) => {
      if (conflicts) choose(choices, field, a, b);
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

/** Called under the link lock before signing the first standby checkpoint. */
export function seedSetupCopies(link: LinkState, shared: SharedValues, gathered: Copy[]): boolean {
  const own = ownKey(link);
  const setup = followedSetup(link);
  if (!own || !setup) return false;
  const copies = currentCopies(link, own, gathered);
  const roster = new Set([own, ...peersOf(link).map(peer => peer.key)]);
  for (const kind of seedKinds) {
    if (shared[kind]) continue;
    if (copies.some(copy => copy.oversized?.includes(kind))) return false;
    const merged = mergeCopies(kind, undefined, copies.map(copy => copy.values[kind]), true, true, roster);
    if (merged.value === undefined) continue;
    const offlineEdits = withSeedSources(history(setup.id, kind, undefined, merged),
      copies.filter(copy => Object.hasOwn(copy.values, kind)).map(copy => copy.key));
    shared[kind] = { sequence: 1, value: merged.value, offlineEdits };
  }
  return true;
}

function contributions(link: LinkState, source: string): Copies {
  const copies: Copies = {};
  const roster = new Set([source, ...peersOf(link).map(peer => peer.key)]);
  const locals = localSetupCopies(link);
  for (const kind of seedKinds) {
    const before = sharedValue(link, kind);
    const entry = before === undefined ? undefined : link.shared?.[kind];
    const preference = preferences[kind];
    const local = locals[kind];
    if (local === undefined || local === null) continue;
    const seeded = seedSources(entry?.offlineEdits);
    if (!entry || seeded && !seeded.includes(source)) {
      // Empty libraries are useful in the first checkpoint, but need no save on reconnect.
      if (preference.empty?.(local)) continue;
      if (mergeCopies(kind, undefined, [local], true, false, roster).value !== undefined) copies[kind] = local;
    } else if (preference.recover) {
      const missing = preference.recover(local, before);
      if (missing !== undefined) copies[kind] = missing;
    } else {
      // An older accepted entry may have missed independent fields. Only additions
      // are considered, so earlier accepted edits retain precedence.
      if (!seeded && !equal(mergeCopies(kind, entry, [local], false, false, roster).value, before)) copies[kind] = local;
    }
  }
  return copies;
}

function canSeed(link: LinkState | null, payload: Record<string, unknown>, source: string) {
  const setup = followedSetup(link);
  return link && setup?.role === "primary" && setup.id === payload.setup && setup.epoch === payload.epoch
    && usesAcceptedSaves(setup) && !link.localWipe && !primaryFenced(link)
    && (source === ownKey(link) || peersOf(link).some(peer => peer.key === source)) ? setup : null;
}

const SEED_WAIT = { status: 503, message: "Setup preferences are still syncing. Try again in a moment." } as const;
const SEED_CAPACITY = { status: 409, message: "These existing preferences exceed the setup’s saved preference capacity." } as const;

async function applySetupCopy(source: string, position: Record<string, unknown>, kind: SeedKind, supplied?: unknown): Promise<Receipt | "nothing" | "capacity" | null> {
  const beforeGather = readLink();
  const setup = canSeed(beforeGather, position, source);
  if (!beforeGather || !setup) return null;
  const gathered = sharedValue(beforeGather, kind) === undefined && !pendingShared(beforeGather, kind)
    ? await gatherSetupCopies(beforeGather, { setup: setup.id, epoch: setup.epoch }, kind) : [];
  const result: { outcome: Receipt | "nothing" | "capacity" | null } = { outcome: null };
  updateLink(link => {
    const setup = canSeed(link, position, source);
    if (!link || !setup || pendingShared(link, kind)) return null;
    const own = setup.primary;
    const roster = new Set([own, ...peersOf(link).map(peer => peer.key)]);
    const before = sharedValue(link, kind) === undefined ? undefined : link.shared?.[kind];
    if (before && supplied === undefined) {
      result.outcome = "nothing";
      return null;
    }
    const first = before ? [] : currentCopies(link, own, gathered, kind);
    if (first.some(copy => copy.oversized?.includes(kind))) {
      result.outcome = "capacity";
      return null;
    }
    const ready = first.filter(copy => Object.hasOwn(copy.values, kind));
    const incoming = before ? [supplied] : ready.map(copy => copy.values[kind]);
    if (!before && supplied !== undefined && !ready.some(copy => copy.key === source)) incoming.push(supplied);
    const seeded = seedSources(before?.offlineEdits);
    const joining = seeded && !seeded.includes(source);
    const merged = mergeCopies(kind, before, incoming, !before, Boolean(joining || !before), roster);
    const changed = !equal(merged.value, before?.value) || !equal(merged.choices, before?.offlineEdits?.choices ?? []);
    if (merged.value === undefined || !before && preferences[kind].empty?.(merged.value) || !changed && !joining) {
      result.outcome = "nothing";
      return null;
    }
    let offlineEdits = history(setup.id, kind, before, merged);
    if (!before || joining) {
      const sources = before ? seeded!.filter(key => roster.has(key)) : ready.map(copy => copy.key);
      if (supplied !== undefined && !sources.includes(source)) sources.push(source);
      offlineEdits = withSeedSources(offlineEdits, sources);
    }
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
  // A sole checkpointed primary commits without a receipt arriving through receiveDocs.
  if (saved.status === "committed") {
    if (kind === "setupTimeZone") {
      const zone = readZoneSetting();
      if (zone) publishZoneSetting(zone);
    }
    if (kind === "setupPersonalities") {
      for (const tell of personalityFollowers) tell();
      const value = sharedValue(readLink(), kind);
      retellBots(mergeSetupPersonalities(object(value)).personalities.map(entry => entry.id)).catch(() => {});
    }
  }
  return saved;
}

function memberSeedKind(kind: unknown): kind is SeedKind {
  return sharedKind(kind) && sharedPolicy(kind).seed === "member copies";
}

export async function takeSetupCopy(source: string, payload: Record<string, unknown>): Promise<void> {
  if (!memberSeedKind(payload.kind) || !isObject(payload.value) || Buffer.byteLength(JSON.stringify(payload.value)) > MAX_SHARED_BYTES) return;
  await applySetupCopy(source, payload, payload.kind, payload.value);
}

/** A first user save also passes through the seed. If there are any legacy copies,
 * accept their merge before letting the edit read its base. */
export async function setupSeedRefusal(kind: SharedKind): Promise<typeof SEED_WAIT | typeof SEED_CAPACITY | null> {
  if (!memberSeedKind(kind)) return null;
  const link = readLink();
  const setup = followedSetup(link);
  if (setup?.role !== "primary" || !usesAcceptedSaves(setup) || sharedValue(link, kind) !== undefined) return null;
  const save = await applySetupCopy(setup.primary, { setup: setup.id, epoch: setup.epoch }, kind);
  if (save === "capacity") return SEED_CAPACITY;
  if (save === "nothing" || save && (await waitShared(save)).status === "committed") return null;
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
  const copies = contributions(link, source);
  if (!Object.keys(copies).length) return;
  const position = { setup: setup.id, epoch: setup.epoch };
  for (const [kind, value] of Object.entries(copies)) {
    const payload = { ...position, kind, value };
    if (!primary) await takeSetupCopy(source, payload);
    else for (const bytes of mailsFor(primary, { setupCopy: payload }, link, now) ?? []) host?.send(bytes);
  }
}
