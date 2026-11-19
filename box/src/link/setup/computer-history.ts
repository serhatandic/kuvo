import { isObject } from "../../shared/json";
import { SIGN } from "../documents/format";
import { KEY, type Peer } from "../peers/known";
import type { LinkState } from "../state";

/** A list position follows the computer. Its credentials belong to individual memberships. */
export interface ComputerHistory {
  setup: string;
  slots: { key: string; fingerprint?: string; fingerprintSource?: FingerprintSource; previous?: string[] }[];
  retiredSigns: string[];
}

export const fingerprintOf = (value: unknown) =>
  typeof value === "string" && /^[0-9a-f]{64}$/.test(value) ? value : undefined;

export type FingerprintSource = "platform-uuid" | "dmi-uuid" | "os-id";
export const fingerprintSourceOf = (value: unknown): FingerprintSource | undefined =>
  value === "platform-uuid" || value === "dmi-uuid" || value === "os-id" ? value : undefined;

const keyOf = (value: unknown): value is string => typeof value === "string" && KEY.test(value);

export function readComputerHistory(value: unknown, keys: Set<string>, retired: Set<string>): ComputerHistory | undefined {
  if (!isObject(value) || typeof value.setup !== "string" || !Array.isArray(value.slots)) return;
  const seen = new Set<string>();
  const slots = value.slots.flatMap(slot => {
    if (!isObject(slot) || !keyOf(slot.key) || !keys.has(slot.key) || seen.has(slot.key)) return [];
    seen.add(slot.key);
    const fingerprint = fingerprintOf(slot.fingerprint);
    const fingerprintSource = fingerprint && fingerprintSourceOf(slot.fingerprintSource);
    const previous = (Array.isArray(slot.previous) ? slot.previous : []).filter(key => keyOf(key) && retired.has(key) && !seen.has(key));
    for (const key of previous) seen.add(key);
    return [{ key: slot.key, ...(fingerprint ? { fingerprint } : {}), ...(fingerprintSource ? { fingerprintSource } : {}), ...(previous.length ? { previous } : {}) }];
  });
  const retiredSigns = [...new Set((Array.isArray(value.retiredSigns) ? value.retiredSigns : [])
    .filter((sign): sign is string => typeof sign === "string" && SIGN.test(sign)))];
  return { setup: value.setup, slots, retiredSigns };
}

export function computerHistory(link: LinkState | null | undefined): ComputerHistory | undefined {
  const history = link?.computerHistory;
  return history && history.setup === link?.setup?.id ? history : undefined;
}

export function historyFor(link: LinkState, roster: Peer[]): ComputerHistory | undefined {
  if (!link.setup) return;
  const history = computerHistory(link) ?? { setup: link.setup.id, slots: [], retiredSigns: [] };
  const known = new Set(history.slots.map(slot => slot.key));
  return { ...history, slots: [...history.slots, ...roster.filter(peer => !known.has(peer.key)).map(({ key }) => ({ key }))] };
}

export function rememberEnrollment(history: ComputerHistory, key: string, fingerprint?: string, previous?: string, source?: FingerprintSource): void {
  const index = history.slots.findIndex(slot => slot.key === (previous ?? key));
  const slot = history.slots[index];
  const kept = fingerprint ?? slot?.fingerprint;
  const prior = previous && previous !== key ? [...(slot?.previous ?? []), previous] : slot?.previous;
  const fingerprintSource = source ?? (kept === slot?.fingerprint ? slot?.fingerprintSource : undefined);
  const entry = { key, ...(kept && fingerprintSource ? { fingerprintSource } : {}), ...(kept ? { fingerprint: kept } : {}), ...(prior ? { previous: prior } : {}) };
  if (index < 0) history.slots.push(entry);
  else history.slots[index] = entry;
}

export function orderComputers(history: ComputerHistory | undefined, roster: Peer[]): Peer[] {
  if (!history) return roster;
  const order = new Map(history.slots.map(({ key }, index) => [key, index]));
  return [...roster].sort((a, b) => (order.get(a.key) ?? Infinity) - (order.get(b.key) ?? Infinity));
}

export function rememberRetiredSigns(link: LinkState, roster: Peer[], gone: Peer[]): void {
  if (!gone.length) return;
  const history = historyFor(link, roster);
  if (!history) return;
  history.retiredSigns = [...new Set([...history.retiredSigns, ...gone.flatMap(peer => peer.sign ? [peer.sign] : [])])];
  link.computerHistory = history;
}

export function historySuccessor(history: ComputerHistory | undefined, previous: string): string | undefined {
  return history?.slots.find(slot => slot.previous?.includes(previous))?.key;
}

/** An explicit choice and automatic evidence must point to the same list entry. */
export function enrollmentPredecessor(history: ComputerHistory | undefined, fingerprint?: string, chosen?: string): string | undefined {
  const candidates = fingerprint ? history?.slots.filter(slot => slot.fingerprint === fingerprint) ?? [] : [];
  if (chosen) {
    if (historySuccessor(history, chosen) || candidates.some(each => each.key !== chosen)) {
      throw new Error("The computer identity conflicts with the selected computer.");
    }
    return chosen;
  }
  return candidates.length === 1 ? candidates[0]?.key : undefined;
}

/** A cloned OS image is evidence for a suggestion, never authorization to replace a member. */
export function requiresReplacementConfirmation(history: ComputerHistory | undefined, predecessor: string, source?: FingerprintSource): boolean {
  const recorded = history?.slots.find(slot => slot.key === predecessor)?.fingerprintSource;
  return source !== "platform-uuid" && source !== "dmi-uuid" || recorded === "os-id";
}

/** Restore can approve an unrecorded replacement explicitly, or confirm the same
 * successor recorded by enrollment. It cannot contradict either computer's history. */
export function permitsReplacement(history: ComputerHistory | undefined, previous: string, replacement: string): boolean {
  const successor = historySuccessor(history, previous);
  const slot = history?.slots.find(each => each.key === replacement);
  return (!successor || successor === replacement) && (!slot?.previous?.length || slot.previous.includes(previous));
}
