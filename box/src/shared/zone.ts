// Your time zone, as every part of a box reads it (PROTOCOL.md, "Your time zone"): the one its
// setting keeps (`box/time-zone.json` in its folder, which your computers carry to each other),
// else `KUVO_TZ` (what an older Mac's launch gave a machine), else none: the computer's own clock.
// Shared with the coding agents' bridge, which finds the box's folder in `KUVO_HOME` (a machine's,
// and a Mac's box in a home of its own); one without it goes by `KUVO_TZ` alone.

import fs from "node:fs"
import path from "node:path"
import { isObject } from "./json"
import { localSetupValue } from "./setup-local"

/** Where a box keeps its time zone, under its folder. */
export const zoneFileIn = (home: string): string => path.join(home, "box", "time-zone.json")

/** Whether this runtime knows the zone `name` (an IANA name, "Europe/Istanbul"). */
export function knownZone(name: unknown): name is string {
  if (typeof name !== "string" || !name || name.length > 64) return false
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: name })
    return true
  } catch {
    return false
  }
}

/** This computer's own clock's zone, as its system says now: `TZ` where it names one, else what
 * `/etc/localtime` links to (read each time: a runtime keeps the zone it started in), else the
 * runtime's. */
export function ownClock(): string {
  if (knownZone(process.env.TZ)) return process.env.TZ
  try {
    const linked = fs.readlinkSync("/etc/localtime").split("/zoneinfo/")[1]
    if (knownZone(linked)) return linked
  } catch {}
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC"
  } catch {
    return "UTC"
  }
}

// (Read again only when the file changed: it's read for every time worked out.)
let cached: { file: string; mtime: number; zone: string | undefined } | null = null

/** Forgets what was read, after the box wrote its setting (two writes can share a time). */
export function forgetZoneRead(): void {
  cached = null
}

/** The zone a box's setting keeps in `file`, if it names one this runtime knows. */
export function settingZone(file: string): string | undefined {
  let mtime = 0
  try { mtime = fs.statSync(file).mtimeMs } catch { return undefined }
  if (cached && cached.file === file && cached.mtime === mtime) return cached.zone
  let zone: string | undefined
  try {
    const kept = JSON.parse(fs.readFileSync(file, "utf8"))
    if (knownZone(kept?.zone)) zone = kept.zone
  } catch {}
  cached = { file, mtime, zone }
  return zone
}

// The box's folder: `KUVO_HOME`, or the link's own (`boxHomeIs`, which its home module says).
let boxHome: string | undefined = process.env.KUVO_HOME

/** The box's folder, as the link has it (its `K`), for `yourZone`. */
export function boxHomeIs(home: string): void {
  boxHome = home
}

// Your zone as the box gave it (the coding agents' bridge, `box-settings`), for a bridge that can't
// read the box's folder.
let boxGiven: string | undefined

/** Your zone as the box gave it to this process (the bridge's group chats' settings). */
export function boxZoneIs(zone: unknown): void {
  boxGiven = knownZone(zone) ? zone : undefined
}

/** Your time zone: the box's setting (its file, else as the box gave it), else `KUVO_TZ`, else
 * undefined (this computer's clock). */
export function yourZone(home: string | undefined = boxHome): string | undefined {
  const shared = home ? localSetupValue(path.join(home, "run"), "setupTimeZone")?.value : null
  if (isObject(shared) && knownZone(shared.zone)) return shared.zone
  const set = (home ? settingZone(zoneFileIn(home)) : undefined) ?? boxGiven
  if (set) return set
  // COMPAT(0.7 bot-kit): the zone a Mac of 0.7 or before started a machine's engine with. Goes 3 months after 0.8.0.
  const given = process.env.KUVO_TZ
  return knownZone(given) ? given : undefined
}
