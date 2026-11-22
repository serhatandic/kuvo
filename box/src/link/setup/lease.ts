import { activeInstallerClaim } from "../add/invite-state";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { isObject } from "../../shared/json";
import { locked, writePrivate } from "../files";
import { K, REAL_BOX, RUN } from "../home";
import { peersOf } from "../peers/known";
import type { LinkState } from "../state";

const file = path.join(RUN, "setup-relay-lease.json");
export const HEARTBEAT_MS = 10_000;

function json(file: string): unknown {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; }
}

/** Whether this computer is always on: it claims a lapsed lease first (`home/setup-takeover.json`, else Linux). */
export function alwaysOnSettings() {
  const settings = json(path.join(K, "setup-takeover.json"));
  const alwaysOn = isObject(settings) && typeof settings.alwaysOn === "boolean" ? settings.alwaysOn : undefined;
  return {
    alwaysOn: alwaysOn ?? process.platform === "linux",
    reason: alwaysOn !== undefined ? "box configuration" : process.platform === "linux" ? "Linux host" : "non-Linux host",
  };
}

export function leaseClock() {
  const test = REAL_BOX ? null : json(path.join(RUN, "test-setup-clock.json"));
  const offset = isObject(test) && typeof test.offset === "number" ? test.offset : 0;
  return { wall: Date.now() + offset, uptime: os.uptime() * 1000 + offset };
}

export function testSetupClock() {
  return !REAL_BOX && fs.existsSync(path.join(RUN, "test-setup-clock.json"));
}

export type LeaseClock = ReturnType<typeof leaseClock>;
export type RelayLease = {
  setup: string; holder: string; sign: string; term: number; expires: number; high: number; numberFloor?: number;
  relayHost: boolean; since: LeaseClock; validAfter: LeaseClock; duration: number;
};

export function relayLease(link: LinkState | null | undefined): RelayLease | null {
  const saved = json(file) as RelayLease | null;
  return saved && saved.setup === link?.setup?.id ? saved : null;
}

export function rememberRelayLease(lease: RelayLease, upgradeNumberFloor = 0): boolean {
  return locked(file + ".lock", () => {
    const saved = json(file) as RelayLease | null;
    if (saved?.setup === lease.setup && (saved.term > lease.term ||
      saved.term === lease.term && (saved.expires > lease.expires || saved.since.wall > lease.since.wall))) return false;
    const numberFloor = saved?.setup === lease.setup && saved.term === lease.term
      ? saved.numberFloor ?? Math.max(saved.high, upgradeNumberFloor) : lease.high;
    writePrivate(file, JSON.stringify({ ...lease, numberFloor }));
    return true;
  }, { exclusive: true });
}

export function fresh(since: LeaseClock, now: LeaseClock, duration: number) {
  const wall = now.wall - since.wall;
  const uptime = now.uptime - since.uptime;
  return wall >= 0 && uptime >= 0 && wall < duration && uptime < duration;
}

export function holdsRelayLease(link: LinkState | null | undefined): boolean {
  const lease = relayLease(link);
  const now = leaseClock();
  return !!lease && lease.holder === link?.setup?.primary && lease.sign === link.setup.sign &&
    fresh(lease.since, now, lease.duration) && fresh(lease.validAfter, now, lease.duration);
}

export function primaryFenced(link: LinkState | null | undefined, allowDeparture = false): boolean {
  if (link?.macRotation) return true;
  if (link?.setup?.role !== "primary" || link.setup.aside) return false;
  if (activeInstallerClaim(link) || link.setupMerge && !allowDeparture) return true;
  // With no other computer in the setup, none can take over: the lease guards nothing. (Adding one
  // needs the relay.)
  return peersOf(link).length > 0 && !holdsRelayLease(link);
}

export const LEADERSHIP_WAIT = "Kuvo's relay can't be reached. Setup changes will resume when it reconnects.";
export const RELAY_HOST_WARNING = "The main computer also runs Kuvo's relay. If this computer goes offline, another computer can't take over.";
