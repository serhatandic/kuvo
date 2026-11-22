// The box's conversation setting, shared by the link and the agents bridge.
import { readFileSync } from "node:fs"
import { join } from "node:path"

export const compactionChoices = ["200000", "400000", "600000", "never"] as const
export type CompactionChoice = typeof compactionChoices[number]

export const compactionFile = (home: string) => join(home, "box", "conversation-compaction.json")

export function isCompactionChoice(value: unknown): value is CompactionChoice {
  return compactionChoices.some(choice => choice === value)
}

export function compactionChoice(home = process.env.KUVO_HOME): CompactionChoice {
  try {
    const value: unknown = home && JSON.parse(readFileSync(compactionFile(home), "utf8"))
    if (isCompactionChoice(value)) return value
  } catch {}
  return "400000"
}

export interface ContextSize {
  used: number
  window?: number
}

export const tokenCount = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0

/** Occupancy after one model request. These buckets are disjoint in Kuvo's wire format. */
export function stepContext(usage: unknown): number | undefined {
  const tokens = usage as { input?: unknown; output?: unknown; reasoning?: unknown; cache?: { read?: unknown; write?: unknown } } | undefined
  if (!tokenCount(tokens?.input)) return
  const values = [tokens.input, tokens.output ?? 0, tokens.reasoning ?? 0, tokens.cache?.read ?? 0, tokens.cache?.write ?? 0]
  if (!values.every(tokenCount)) return
  const used = values.reduce((sum, value) => sum + value, 0)
  return used || undefined
}
