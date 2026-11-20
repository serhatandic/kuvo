import { activeInstallerClaim } from "../add/invite-state";
// A lease is local and disposable. Binding it to the setup and two local clocks means
// restoring a lease file or restarting a box never gives it a fresh term.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { isObject } from "../../shared/json";
import { held } from "../documents/format";
import { writePrivate } from "../files";
import { K, REAL_BOX, RUN } from "../home";
import type { LinkState } from "../state";

const file = path.join(RUN, "setup-lease.json");
export const LEASE_MS = 60_000;
export const HEARTBEAT_MS = 10_000;
export const DEFAULT_DELAY = LEASE_MS + 120_000;

/** Probes run together, so even the last successor waits at most one heartbeat for replies. */
export function takeoverProbeWindow(standby: boolean, earlierSuccessors: number, heartbeat = HEARTBEAT_MS) {
  return standby && earlierSuccessors === 0 ? 0 : heartbeat;
}

function json(file: string): unknown {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; }
}

export function standbySettings() {
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

const stamp = (link: LinkState) => [link.setup?.id, link.setup?.epoch, link.setup?.standby?.key].join(":");

export function grantLease(link: LinkState, since: ReturnType<typeof leaseClock>) {
  const authority = link.setup?.successorChain ? held(link.leadership?.[link.setup.epoch])?.h : undefined;
  writePrivate(file, JSON.stringify({ stamp: stamp(link), authority, ...since }));
}

export function fresh(since: { wall: number; uptime: number }, now: ReturnType<typeof leaseClock>, duration: number) {
  const wall = now.wall - since.wall;
  const uptime = now.uptime - since.uptime;
  return wall >= 0 && uptime >= 0 && wall < duration && uptime < duration;
}

export function primaryFenced(link: LinkState | null | undefined, allowDeparture = false): boolean {
  if (link?.setup?.role !== "primary") return false;
  if (activeInstallerClaim(link) || link.setupMerge && !allowDeparture) return true;
  if (link.leadershipPending) return true;
  const revoked = link.setupRevocations;
  if (link.setup.successorChain && revoked?.setup === link.setup.id) {
    const standby = link.setup.standby;
    if (revoked.computers.includes(link.setup.primary) || revoked.signs.includes(link.setup.sign) ||
      standby && (revoked.computers.includes(standby.key) || revoked.signs.includes(standby.sign))) return true;
  }
  // A standby's matching departure receipt can be installed after a restart even when the
  // old setup stopped renewing leases. This permission never opens other primary writes.
  if (allowDeparture && link.setupMerge) return false;
  if (!link.setup.standby) return false;
  const lease = json(file);
  const authority = link.setup.successorChain ? held(link.leadership?.[link.setup.epoch])?.h : undefined;
  return !isObject(lease) || lease.stamp !== stamp(link) || link.setup.successorChain && (!authority || lease.authority !== authority)
    || typeof lease.wall !== "number"
    || typeof lease.uptime !== "number" || !fresh({ wall: lease.wall, uptime: lease.uptime }, leaseClock(), LEASE_MS);
}

export const LEADERSHIP_WAIT = "Your primary is waiting for its standby. Try again when they can reach each other.";
