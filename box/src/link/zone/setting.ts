// Your time zone (PROTOCOL.md, "Your time zone"): a zone you pick (`kuvo/settings/clock`, from any
// client) is kept by the box (`box/time-zone.json`) and carried to your other computers in peers' mail
// (`zone: {zone, n, zoneinfo?}`), the pick counted highest winning. Until you pick one, each box goes by
// its own clock, as before; a machine keeps what an older Mac gave it, for itself alone. What goes by
// it: bots' and conversations' wake-ups (the `timezone` file `kuvo-wake` reads beside them), a
// `/loop`'s time of day, goals' days, the night's window, and the times limits reset.
// Platform: the same on a Mac's box and a machine's; on a Mac, bots go by its clock while no zone is
// picked (no zone file), as they always have.

import fs from "node:fs";
import path from "node:path";
import { forgetZoneRead, knownZone, ownClock, zoneFileIn } from "../../shared/zone";
import { listOf, locked, placeWhole, textOf, writePrivate } from "../files";
import { BOTS, IS_MAC, K, REAL_BOX, RUN, SESSION_WAKES } from "../home";
import { readMacKit } from "../bots/mac-kit";

/** What a box keeps of your time zone: the zone, its count (one more with each pick, whichever
 * computer it was made on: 1 and up), the zone's rules (a TZif file, base64), for a computer without
 * zone files of its own; and, on a machine, `carried` while it's only what an older Mac gave it
 * (count 0, kept here, never told, giving way to any pick), and the zone that Mac's kit last said
 * (`kitSeen`). */
export interface ZoneSetting {
  kuvo: 1;
  zone: string;
  n: number;
  zoneinfo?: string;
  carried?: true;
  kitSeen?: string;
}

/** What a box tells its peers of a pick: the zone, its count, and its rules where it has them. */
export interface ZoneMail {
  zone: string;
  n: number;
  zoneinfo?: string;
}

export const ZONE_FILE = zoneFileIn(K);
const ZONE_LOCK = path.join(RUN, ".time-zone.lock");
/** A count past this isn't taken. */
const MOST_COUNT = 2 ** 52;

/** Told as it changes here (a pick, a peer's, an older Mac's kit), so peers hear at once. */
export const zoneFollowers = new Set<() => void>();

const isSetting = (value: unknown): value is ZoneSetting => {
  const kept = value as ZoneSetting | null;
  // (A zone this runtime no longer knows by that name, renamed in the zone database, is still kept,
  // with its count.)
  return kept?.kuvo === 1 && typeof kept.zone === "string" && /^[A-Za-z0-9_+\-/]{1,64}$/.test(kept.zone) && !kept.zone.includes("..")
    && Number.isInteger(kept.n) && kept.n >= 0 && kept.n <= MOST_COUNT;
};

/** Rules given with a peer's pick (a TZif file, base64), if they're that and small; null otherwise. */
function givenRules(value: unknown): string | null {
  if (typeof value !== "string" || !value || value.length > 22_000) return null;
  const data = Buffer.from(value, "base64");
  return data.length > 4 && data.length < 16_384 && data.subarray(0, 4).toString() === "TZif" ? data.toString("base64") : null;
}

// Where this computer keeps zones' rules, as `kuvo-wake` looks for them (a test's box, one of its own:
// a Linux machine without tzdata).
const ZONEINFO = !REAL_BOX && process.env.KUVO_TEST_ZONEINFO ? process.env.KUVO_TEST_ZONEINFO : "/usr/share/zoneinfo";

/** Whether this computer has a zone's rules where `kuvo-wake`'s `date` finds them (UTC needs none). */
export const hasZoneRules = (zone: string): boolean => zone === "UTC" || (!zone.includes("..") && fs.existsSync(path.join(ZONEINFO, zone)));

/** A zone's rules as this computer has them (a TZif file), if small; null otherwise. */
function rulesOf(zone: string): string | null {
  if (zone.includes("..")) return null;
  try {
    const data = fs.readFileSync(path.join(ZONEINFO, zone));
    return data.length < 16_384 && data.subarray(0, 4).toString() === "TZif" ? data.toString("base64") : null;
  } catch {
    return null;
  }
}

/** What a machine from before kept of your zone, carried into its setting the first time it's read,
 * `carried` (kept here, never told, and giving way to any pick): what its Mac last gave it, its kit,
 * the zone above its conversations' wake-ups, its launch's `KUVO_TZ`. Null for a Mac's box (it goes
 * by its own clock, as it did) and a machine with none of them (its own clock, until a pick). */
export function zoneFromBefore(): ZoneSetting | null {
  if (IS_MAC) return null;
  // COMPAT(0.7 bot-kit): the zone a Mac of 0.7 or before gave a machine. Goes 3 months after 0.8.0.
  const kit = readMacKit();
  if (knownZone(kit?.timeZone)) return { kuvo: 1, zone: kit.timeZone, n: 0, carried: true, kitSeen: kit.timeZone, ...kitRules(kit) };
  const wakes = (textOf(path.join(SESSION_WAKES, "timezone")) ?? "").trim();
  if (knownZone(wakes)) {
    let rules: string | null = null;
    try { rules = fs.readFileSync(path.join(SESSION_WAKES, "zoneinfo")).toString("base64") || null; } catch {}
    return { kuvo: 1, zone: wakes, n: 0, carried: true, ...(rules ? { zoneinfo: rules } : {}) };
  }
  const launched = (textOf(path.join(RUN, "launch.env")) ?? "").match(/^KUVO_TZ=(.*)$/m)?.[1]?.trim() ?? process.env.KUVO_TZ;
  if (knownZone(launched)) return { kuvo: 1, zone: launched, n: 0, carried: true };
  return null;
}

const kitRules = (kit: { zoneinfo?: unknown }): { zoneinfo?: string } => {
  const rules = givenRules(kit.zoneinfo);
  return rules ? { zoneinfo: rules } : {};
};

// A Mac of 0.7 or before still connecting writes its kit again as it does, with its own clock: while
// what this machine has is only carried from it, its zone follows (for this machine alone, as that
// Mac gave it before); a pick of yours stays, the kit's zone only noted.
// COMPAT(0.7 bot-kit): a Mac of 0.7 or before's kit, followed while one still connects. Goes 3 months after 0.8.0.
function followOldKit(now: ZoneSetting): ZoneSetting {
  if (IS_MAC) return now;
  const kit = readMacKit();
  if (!knownZone(kit?.timeZone) || kit.timeZone === now.kitSeen) return now;
  const zone = kit.timeZone;
  if (!now.carried) return written({ ...now, kitSeen: zone });
  kitMoved = written({ ...now, zone, kitSeen: zone, zoneinfo: undefined, ...kitRules(kit) });
  return kitMoved;
}

// What an older Mac's kit moved your zone to just now, under the lock, for whoever took it to say
// so once out of it (`movedByKit`).
let kitMoved: ZoneSetting | null = null;
const movedByKit = (): ZoneSetting | null => {
  const moved = kitMoved;
  kitMoved = null;
  return moved;
};

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

// As kept, carried from before where there's none yet, and an older Mac's kit followed: under the lock.
function keptNow(): ZoneSetting | null {
  const now = readKept();
  if (now === null) return null;
  if (now !== undefined) return followOldKit(now);
  const before = zoneFromBefore();
  return before ? written(before) : null;
}

// Whether what's kept needs writing first: none yet (carried once), or an older Mac's kit changed.
const needsWriting = (kept: ZoneSetting | null | undefined): boolean => {
  if (kept === undefined) return true;
  if (!kept || IS_MAC) return false;
  const kit = readMacKit();
  return knownZone(kit?.timeZone) && kit.timeZone !== kept.kitSeen;
};

/** This box's setting as kept, carried from before the first time; null for none yet. */
export function readZoneSetting(): ZoneSetting | null {
  const kept = readKept();
  if (!needsWriting(kept)) return kept ?? null;
  // (No file, and nothing from before to carry: none, without the lock.)
  if (kept === undefined && !zoneFromBefore()) return null;
  // (Under the lock: another process may have just done it.)
  let made: ZoneSetting | null = null;
  try {
    made = locked(ZONE_LOCK, keptNow);
  } catch {
    return kept ?? null;
  }
  // (An older Mac's kit changed it: what goes by it here, and your other computers, follow.)
  const moved = movedByKit();
  if (moved) changed(moved);
  return made;
}

/** The zone this box goes by now, and whether it's yours (set, carried or taken) rather than its own clock. */
export function zoneHere(setting: ZoneSetting | null = readZoneSetting()): { zone: string; yours: boolean } {
  return setting ? { zone: setting.zone, yours: true } : { zone: ownClock(), yours: false };
}

/** What this box tells its peers: a pick (count 1 and up) with its rules, or nothing: never its own
 * clock, nor what an older Mac gave it before. */
export function zoneMail(): ZoneMail | null {
  const setting = readZoneSetting();
  return setting && !setting.carried && setting.n >= 1 ? { zone: setting.zone, n: setting.n, ...(setting.zoneinfo ? { zoneinfo: setting.zoneinfo } : {}) } : null;
}

function changed(setting: ZoneSetting): void {
  applyZone(setting);
  for (const told of [...zoneFollowers]) try { told(); } catch {}
}

/** A new pick's count: one more than this box has, and at least the time it's made (ms since 1970),
 * so a pick made on a box that hasn't heard your others' yet (reinstalled, just set up) still wins
 * over an older one they had. */
// (Two picks made before either box hears the other are ordered by their clocks; picking again after
// hearing always wins. A count at the cap, which only a broken box could send, stays there.)
export const pickCount = (now: Pick<ZoneSetting, "n"> | null, at = Date.now()): number => Math.min(Math.max((now?.n ?? 0) + 1, at), MOST_COUNT);

/** Your zone changed to `zone` here, from a client: counted past any this box has seen, and past now.
 * Null for a zone this computer doesn't know. */
export function setZone(zone: string): ZoneSetting | null {
  if (!knownZone(zone)) return null;
  let made: ZoneSetting | null = null;
  const setting = locked(ZONE_LOCK, () => {
    const now = keptNow();
    if (now?.zone === zone && !now.carried) return now;
    // (This computer's own rules, else the same zone's kept before.)
    const rules = rulesOf(zone) ?? (now?.zone === zone ? now.zoneinfo ?? null : null);
    made = written({ kuvo: 1, zone, n: pickCount(now), ...kept(now), ...(rules ? { zoneinfo: rules } : {}) });
    return made;
  });
  const moved = movedByKit();
  if (made || moved) changed(made ?? moved!);
  return setting;
}

// What a change keeps of the setting before it: what an older Mac's kit last said.
const kept = (now: ZoneSetting | null): Partial<ZoneSetting> => (now?.kitSeen ? { kitSeen: now.kitSeen } : {});

/** A peer's pick (`zone: {zone, n, zoneinfo?}` in its mail, count 1 and up), taken where it's
 * counted higher than this box's (at the same count, the zone whose name sorts last, so every box
 * keeps the same one), or where this box has none or only one carried from before. Its rules are
 * kept where this computer has none of its own. True if this box's changed. Who it's taken from is
 * the mail's to say (`peers/mail.ts`). */
export function takeZone(given: unknown): boolean {
  const mail = given as Partial<ZoneMail> | null;
  if (!knownZone(mail?.zone) || !Number.isInteger(mail.n) || mail.n! < 1 || mail.n! > MOST_COUNT) return false;
  const { zone, n } = mail as ZoneMail;
  const sent = givenRules(mail.zoneinfo);
  let made: ZoneSetting | null = null;
  try {
    locked(ZONE_LOCK, () => {
      const now = keptNow();
      // (The same pick told again with rules, after a first mail too big to carry them, and none here: kept.)
      if (now && !now.carried && n === now.n && zone === now.zone && !now.zoneinfo && sent && !rulesOf(zone)) {
        made = written({ ...now, zoneinfo: sent });
        return;
      }
      if (now && !now.carried && (n < now.n || (n === now.n && zone <= now.zone))) return;
      // (This computer's own rules, else those sent with it, else the same zone's kept before.)
      const rules = rulesOf(zone) ?? sent ?? (now?.zone === zone ? now.zoneinfo ?? null : null);
      made = written({ kuvo: 1, zone, n, ...kept(now), ...(rules ? { zoneinfo: rules } : {}) });
    });
  } catch {
    return false;
  }
  const moved = movedByKit();
  if (made || moved) changed(made ?? moved!);
  return Boolean(made);
}

/** What a folder of wake-ups says of its zone, for `kuvo-wake` (its `timezone` and `zoneinfo`
 * files): your zone where there's one; with none, on a machine its own clock (as before), on a Mac
 * nothing (it goes by its clock as it changes). */
export function zoneFilesFor(setting: ZoneSetting | null = readZoneSetting()): { timezone: string | null; zoneinfo: Buffer | null } {
  const here = zoneHere(setting);
  const timezone = here.yours || !IS_MAC ? here.zone : null;
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
    // (Whole, by a rename: never through a link put there, and never gone before the new one's there.)
    placeWhole(file, data, 0o644);
  };
  // (The rules first, then the name that reads them; rules no longer wanted go last.)
  if (files.zoneinfo) put("zoneinfo", files.zoneinfo);
  put("timezone", files.timezone);
  if (!files.zoneinfo) put("zoneinfo", null);
}

/** Your zone, in every folder here whose wake-ups go by it: each bot's (not one that's moving, which
 * the box it arrives at settles), and, once there's one of yours, the folder above conversations' own. */
export function applyZone(setting: ZoneSetting | null = readZoneSetting()): void {
  const files = zoneFilesFor(setting);
  for (const id of listOf(BOTS)) {
    const folder = path.join(BOTS, id);
    if (!fs.existsSync(path.join(folder, "bot.json")) || (textOf(path.join(folder, "paused")) ?? "").trim() === "moving") continue;
    try { writeZoneFiles(folder, files); } catch {}
  }
  // (Only yours: a machine's own clock written there would be read as yours from before, and told
  // to your other computers, the next time there's no setting.)
  if (setting) try { writeZoneFiles(SESSION_WAKES, files, files.timezone !== null); } catch {}
}

/** Kept so as the box runs: now, and each minute (the folders made since given it). The returned
 * function stops it. */
export function keepZone(every = 60_000): () => void {
  const round = () => {
    try { applyZone(); } catch {}
  };
  round();
  const timer = setInterval(round, every);
  timer.unref?.();
  return () => clearInterval(timer);
}
