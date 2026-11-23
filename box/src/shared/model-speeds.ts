// The speed you picked for each provider (PROTOCOL.md, "The models offered", `models.speed`),
// shared by the link, which keeps it as `kuvo/models/speed` says, and the agents bridge, which runs
// each of Codex's turns at it. Kept in `box/models.json` beside your starred models: `speeds`, by
// provider (`codex-cli`, `codex-cli@work`), a speed's id (Codex's `priority`, its "Fast"), or null
// for Standard picked yourself. A provider with none there was never picked for: its agent's own
// settings decide.
import { readFileSync } from "node:fs"
import { join } from "node:path"
import { isObject } from "./json"

/** A speed a model runs at besides its standard one, as its agent lists it: Codex's service tiers
 * (`{id: "priority", name: "Fast", description: "1.5x speed"}`). */
export interface Speed { id: string; name: string; description?: string }

export const modelsFile = (home: string) => join(home, "box", "models.json")

/** The longest provider id kept. */
const LONGEST = 300

export const isSpeedProvider = (value: unknown): value is string => typeof value === "string" && value !== "" && value.length <= LONGEST

/** A speed's id as one is kept: short, plain letters, digits, `-`, `_` and `.`. */
export const isSpeedID = (value: unknown): value is string => typeof value === "string" && /^[\w.-]{1,64}$/.test(value)

/** The speeds picked, by provider (none for a file that's missing or can't be read). */
export function pickedSpeeds(file: string | undefined): Record<string, string | null> {
  const out: Record<string, string | null> = {}
  if (!file) return out
  try {
    const kept: unknown = JSON.parse(readFileSync(file, "utf8"))
    if (!isObject(kept) || !isObject(kept.speeds)) return out
    for (const [provider, speed] of Object.entries(kept.speeds)) {
      if (isSpeedProvider(provider) && (speed === null || isSpeedID(speed))) out[provider] = speed
    }
  } catch {}
  return out
}

/** The speed picked for `provider` on this box (its folder: `KUVO_HOME`): a speed's id, null for
 * Standard picked yourself, undefined for none picked. */
export function pickedSpeed(provider: string, home = process.env.KUVO_HOME): string | null | undefined {
  if (!home) return undefined
  const speeds = pickedSpeeds(modelsFile(home))
  return Object.hasOwn(speeds, provider) ? speeds[provider] : undefined
}

/** The speeds a model lists, as kept: each with an id and a name, once; a description where it has one. */
export function speedsIn(listed: unknown): Speed[] {
  if (!Array.isArray(listed)) return []
  const out: Speed[] = []
  for (const each of listed) {
    if (!isObject(each) || !isSpeedID(each.id) || out.some(other => other.id === each.id)) continue
    const name = typeof each.name === "string" && each.name.trim() ? each.name.trim() : each.id
    const description = typeof each.description === "string" && each.description.trim() ? each.description.trim() : undefined
    out.push({ id: each.id, name, ...(description ? { description } : {}) })
  }
  return out
}
