// Your time zone (PROTOCOL.md, "Your time zone"): one for all your own computers in a setup, kept by
// each box (`box/time-zone.json`), changed from any client (`kuvo/settings/general`), and carried box
// to box in peers' mail (`zone: {zone, n}`), the change counted highest winning. What goes by it:
// bots' and conversations' wake-ups (the `timezone` file `kuvo-wake` reads beside them), a `/loop`'s
// time of day, goals' days, the night's window, and the times limits reset.
// Platform: the same on a Mac's box and a machine's, but for one thing: a Mac's clock is where you
// are, so a Mac's box takes its own clock changing (you travelled) as a change of your zone; a
// machine's clock says nothing of where you are.

import fs from "node:fs";
import path from "node:path";
import { forgetZoneRead, knownZone, ownClock, zoneFileIn } from "../../shared/zone";
import { listOf, locked, textOf, writePrivate } from "../files";
import { BOTS, IS_MAC, K, RUN, SESSION_WAKES } from "../home";
import { readMacKit } from "../bots/mac-kit";

/** What a box keeps of your time zone: the zone, its count (one more with each change, whichever
 * computer made it), what a Mac's own clock said when last looked at, and the zone's rules (a TZif
 * file, base64) where an older Mac gave them, for a machine without zone files of its own. */
export interface ZoneSetting {
  kuvo: 1;
  zone: string;
  n: number;
  seen?: string;
  zoneinfo?: string;
}

/** What a box tells its peers of it. */
export interface ZoneMail {
  zone: string;
  n: number;
}

export const ZONE_FILE = zoneFileIn(K);
const ZONE_LOCK = path.join(RUN, ".time-zone.lock");
/** A count past this isn't taken. */
const MOST_COUNT = 2 ** 52;

/** Told as it changes here (by a client, a peer, a Mac's clock), so peers hear at once. */
export const zoneFollowers = new Set<() => void>();

const isSetting = (value: unknown): value is ZoneSetting => {
  const kept = value as ZoneSetting | null;
  return kept?.kuvo === 1 && knownZone(kept.zone) && Number.isInteger(kept.n) && kept.n >= 0 && kept.n <= MOST_COUNT;
};

/** A zone's rules as this computer has them (a TZif file), if small; null otherwise. */
function rulesOf(zone: string): string | null {
  if (zone.includes("..")) return null;
  try {
    const data = fs.readFileSync(path.join("/usr/share/zoneinfo", zone));
    return data.length < 16_384 && data.subarray(0, 4).toString() === "TZif" ? data.toString("base64") : null;
  } catch {
    return null;
  }
}

/** What a box from before kept of your zone, carried into its setting the first time it's read:
 * a Mac's own clock (n 1, so a Mac's zone wins over what a machine had of it from the Mac before);
 * a machine's from what its Mac last gave it (n 0): its kit, the zone above its conversations'
 * wake-ups, its launch's `KUVO_TZ`. Null for a box with none of them: it takes its setup's. */
export function zoneFromBefore(): ZoneSetting | null {
  if (IS_MAC) {
    const own = ownClock();
    return { kuvo: 1, zone: own, n: 1, seen: own };
  }
  // COMPAT(0.7 bot-kit): the zone a Mac of 0.7 or before gave a machine. Goes 3 months after 0.8.0.
  const kit = readMacKit();
  if (knownZone(kit?.timeZone)) return { kuvo: 1, zone: kit.timeZone, n: 0, ...(typeof kit.zoneinfo === "string" && kit.zoneinfo ? { zoneinfo: kit.zoneinfo } : {}) };
  const wakes = (textOf(path.join(SESSION_WAKES, "timezone")) ?? "").trim();
  if (knownZone(wakes)) {
    let rules: string | null = null;
    try { rules = fs.readFileSync(path.join(SESSION_WAKES, "zoneinfo")).toString("base64") || null; } catch {}
    return { kuvo: 1, zone: wakes, n: 0, ...(rules ? { zoneinfo: rules } : {}) };
  }
  const launched = (textOf(path.join(RUN, "launch.env")) ?? "").match(/^KUVO_TZ=(.*)$/m)?.[1]?.trim() ?? process.env.KUVO_TZ;
  if (knownZone(launched)) return { kuvo: 1, zone: launched, n: 0 };
  return null;
}

function written(setting: ZoneSetting): ZoneSetting {
  writePrivate(ZONE_FILE, JSON.stringify(setting));
  forgetZoneRead();
  return setting;
}

// As kept: undefined where there's no file yet, null for one that isn't a setting.
function readKept(): ZoneSetting | null | undefined {
  try {
    const kept = JSON.parse(fs.readFileSync(ZONE_FILE, "utf8"));
    return isSetting(kept) ? kept : null;
  } catch (error) {
    return (error as NodeJS.ErrnoException)?.code === "ENOENT" ? undefined : null;
  }
}

// As kept, carried from before where there's none yet: under the lock.
function keptNow(): ZoneSetting | null {
  const now = readKept();
  if (now !== undefined) return now;
  const before = zoneFromBefore();
  return before ? written(before) : null;
}

/** This box's setting as kept, carried from before the first time; null for none yet. */
export function readZoneSetting(): ZoneSetting | null {
  const kept = readKept();
  if (kept !== undefined) return kept;
  // (Carried once, under the lock: another process may have just done it.)
  try {
    return locked(ZONE_LOCK, keptNow);
  } catch {
    return null;
  }
}

/** The zone this box goes by now, and whether it's yours (set, carried or taken) rather than its own clock. */
export function zoneHere(setting: ZoneSetting | null = readZoneSetting()): { zone: string; yours: boolean } {
  return setting ? { zone: setting.zone, yours: true } : { zone: ownClock(), yours: false };
}

/** What this box tells its peers: its setting, or nothing while it has none. */
export function zoneMail(): ZoneMail | null {
  const setting = readZoneSetting();
  return setting ? { zone: setting.zone, n: setting.n } : null;
}

function changed(setting: ZoneSetting): void {
  applyZone(setting);
  for (const told of [...zoneFollowers]) try { told(); } catch {}
}

/** Your zone changed to `zone` here, from a client: counted one more than any this box has seen.
 * Null for a zone this computer doesn't know. */
export function setZone(zone: string): ZoneSetting | null {
  if (!knownZone(zone)) return null;
  let made: ZoneSetting | null = null;
  const setting = locked(ZONE_LOCK, () => {
    const now = keptNow();
    if (now?.zone === zone) return now;
    const rules = rulesOf(zone);
    made = written({ kuvo: 1, zone, n: Math.min((now?.n ?? 0) + 1, MOST_COUNT), ...(now?.seen ? { seen: now.seen } : {}), ...(rules ? { zoneinfo: rules } : {}) });
    return made;
  });
  if (made) changed(made);
  return setting;
}

/** A peer's zone (`zone: {zone, n}` in its mail), taken where it's counted higher than this box's
 * (at the same count, the zone whose name sorts last, so every box keeps the same one). True if
 * this box's changed. Who it's taken from is the mail's to say (`peers/mail.ts`). */
export function takeZone(given: unknown): boolean {
  const mail = given as Partial<ZoneMail> | null;
  if (!knownZone(mail?.zone) || !Number.isInteger(mail.n) || mail.n! < 0 || mail.n! > MOST_COUNT) return false;
  const { zone, n } = mail as ZoneMail;
  let made: ZoneSetting | null = null;
  try {
    locked(ZONE_LOCK, () => {
      const now = keptNow();
      if (now && (n < now.n || (n === now.n && zone <= now.zone))) return;
      // (The same zone counted higher keeps its rules.)
      const rules = now?.zone === zone ? now.zoneinfo ?? rulesOf(zone) : rulesOf(zone);
      made = written({ kuvo: 1, zone, n, ...(now?.seen ? { seen: now.seen } : {}), ...(rules ? { zoneinfo: rules } : {}) });
    });
  } catch {
    return false;
  }
  if (!made) return false;
  // (Only the count moved: nothing on this computer goes by it differently.)
  changed(made);
  return true;
}

/** A Mac's box: its own clock moved since it last looked (you travelled, or changed it in System
 * Settings), taken as your zone changing, counted one more. True if it did. A machine's never. */
export function followOwnClock(): boolean {
  if (!IS_MAC) return false;
  const own = ownClock();
  let made: ZoneSetting | null = null;
  try {
    locked(ZONE_LOCK, () => {
      const now = keptNow();
      if (now?.seen === own) return;
      // (A Mac's box from before kept none: it starts at its own clock, as it's carried.)
      if (!now) return void (made = written({ kuvo: 1, zone: own, n: 1, seen: own }));
      const rules = rulesOf(own);
      made = written({ kuvo: 1, zone: own, n: Math.min(now.n + 1, MOST_COUNT), seen: own, ...(rules ? { zoneinfo: rules } : {}) });
    });
  } catch {
    return false;
  }
  if (made) changed(made);
  return Boolean(made);
}

/** What a folder of wake-ups says of its zone, for `kuvo-wake` (its `timezone` and `zoneinfo`
 * files): on a machine your zone (its own clock while it has none, as before); on a Mac, nothing
 * while your zone is its own clock (it goes by it as it changes), else yours. */
export function zoneFilesFor(setting: ZoneSetting | null = readZoneSetting()): { timezone: string | null; zoneinfo: Buffer | null } {
  const here = zoneHere(setting);
  const timezone = IS_MAC ? (here.yours && here.zone !== ownClock() ? here.zone : null) : here.zone;
  const zoneinfo = timezone && setting?.zoneinfo && setting.zone === timezone ? Buffer.from(setting.zoneinfo, "base64") : null;
  return { timezone, zoneinfo };
}

/** Writes a folder's zone files as `zoneFilesFor` says, only where they differ; `make`, making the
 * folder where there's something to write. */
export function writeZoneFiles(folder: string, files = zoneFilesFor(), make = false): void {
  const put = (name: string, data: string | Buffer | null) => {
    const file = path.join(folder, name);
    if (data === null) return fs.rmSync(file, { force: true });
    let now: Buffer | null = null;
    try { now = fs.readFileSync(file); } catch {}
    if (now && now.equals(Buffer.from(data))) return;
    if (make) fs.mkdirSync(folder, { recursive: true });
    fs.writeFileSync(file, data);
  };
  put("timezone", files.timezone);
  put("zoneinfo", files.zoneinfo);
}

/** Your zone, in every folder here whose wake-ups go by it: each bot's (not one that's moving, which
 * the box it arrives at settles), and the folder above conversations' own. */
export function applyZone(setting: ZoneSetting | null = readZoneSetting()): void {
  const files = zoneFilesFor(setting);
  for (const id of listOf(BOTS)) {
    const folder = path.join(BOTS, id);
    if (!fs.existsSync(path.join(folder, "bot.json")) || (textOf(path.join(folder, "paused")) ?? "").trim() === "moving") continue;
    try { writeZoneFiles(folder, files); } catch {}
  }
  try { writeZoneFiles(SESSION_WAKES, files, files.timezone !== null); } catch {}
}

/** Kept so as the box runs: now, and each minute (a Mac's own clock looked at, the folders made
 * since given it). The returned function stops it. */
export function keepZone(every = 60_000): () => void {
  const round = () => {
    try { if (!followOwnClock()) applyZone(); } catch {}
  };
  round();
  const timer = setInterval(round, every);
  timer.unref?.();
  return () => clearInterval(timer);
}
