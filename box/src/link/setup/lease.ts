// A lease is local and disposable. Binding it to the setup and two local clocks means
// restoring a lease file or restarting a box never gives it a fresh term.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { isObject } from "../../shared/json";
import { writePrivate } from "../files";
import { K, REAL_BOX, RUN } from "../home";
import type { LinkState } from "../state";

const file = path.join(RUN, "setup-lease.json");
export const LEASE_MS = 60_000;
export const HEARTBEAT_MS = 10_000;
export const MIN_DELAY = 60 * 60_000;
export const DEFAULT_DELAY = 3 * MIN_DELAY;

function json(file: string): unknown {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; }
}

export function standbySettings() {
  const settings = json(path.join(K, "setup-takeover.json"));
  const minutes = isObject(settings) ? settings.delayMinutes : null;
  const alwaysOn = isObject(settings) && typeof settings.alwaysOn === "boolean" ? settings.alwaysOn : undefined;
  return {
    delay: typeof minutes === "number" && Number.isSafeInteger(minutes) && minutes >= 60 && minutes <= 10_080 ? minutes * 60_000 : DEFAULT_DELAY,
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
  writePrivate(file, JSON.stringify({ stamp: stamp(link), ...since }));
}

export function fresh(since: { wall: number; uptime: number }, now: ReturnType<typeof leaseClock>, duration: number) {
  const wall = now.wall - since.wall;
  const uptime = now.uptime - since.uptime;
  return wall >= 0 && uptime >= 0 && wall < duration && uptime < duration;
}

export function primaryFenced(link: LinkState | null | undefined): boolean {
  if (link?.setup?.role !== "primary" || !link.setup.standby) return false;
  if (link.leadershipPending) return true;
  const lease = json(file);
  return !isObject(lease) || lease.stamp !== stamp(link) || typeof lease.wall !== "number"
    || typeof lease.uptime !== "number" || !fresh({ wall: lease.wall, uptime: lease.uptime }, leaseClock(), LEASE_MS);
}

export const LEADERSHIP_WAIT = "Your primary is waiting for its standby. Try again when they can reach each other.";
