// Availability samples are hints for ordering computers. Authority still comes from the epoch certificate.
import { successorRankLayoutReady } from "../../shared/folder-version";
import { K } from "../home";
import type { Peer } from "../peers/known";
import { fresh, HEARTBEAT_MS, leaseClock, testSetupClock } from "./lease";

type Clock = ReturnType<typeof leaseClock>;
const availability = new Map<string, { at: Clock; since: Clock; preferred: boolean; reason: string }>();
export const RANK_INTERVAL = 6 * HEARTBEAT_MS;

export function resetAvailability() {
  availability.clear();
}

export function observeAvailability(key: string, now: Clock, preferred: boolean, reason: string) {
  const last = availability.get(key);
  const since = last && fresh(last.at, now, 3 * HEARTBEAT_MS) ? last.since : now;
  availability.set(key, { at: now, since, preferred, reason });
}

export function availableComputer(key: string, now: Clock) {
  const sample = availability.get(key);
  return sample && fresh(sample.at, now, testSetupClock() ? 10_000 : 3 * HEARTBEAT_MS) ? sample : undefined;
}

export function observedUptime(since: Clock, now: Clock) {
  return Math.floor((now.uptime - since.uptime) / RANK_INTERVAL);
}

/** All computers, in reliability order. Keep the accepted order when observations tie. */
export function rankComputers(boxes: Peer[], previous?: string[], now = leaseClock()): string[] | undefined {
  if (!successorRankLayoutReady(K)) return previous;
  const places = new Map(previous?.map((key, rank) => [key, rank]));
  return boxes.map(peer => {
    const sample = availableComputer(peer.key, now);
    const preference = sample?.preferred ? 2 : !sample && peer.kind === "machine" ? 1 : 0;
    return { key: peer.key, uptime: sample ? observedUptime(sample.since, now) : 0,
      preference, place: places.get(peer.key) ?? boxes.length };
  }).sort((a, b) => b.uptime - a.uptime
    || a.place - b.place || b.preference - a.preference || a.key.localeCompare(b.key))
    .map(box => box.key);
}
