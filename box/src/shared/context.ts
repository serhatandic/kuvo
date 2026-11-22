// The box's conversation setting, shared by the link and the agents bridge.
import { readFileSync } from "node:fs"
import { join } from "node:path"

export const compactionChoices = ["200000", "400000", "600000", "never"] as const
export type CompactionChoice = typeof compactionChoices[number]
export const DEFAULT_COMPACTION = "400000"

export const compactionFile = (home: string) => join(home, "box", "conversation-compaction.json")

export function isCompactionChoice(value: unknown): value is CompactionChoice {
  return compactionChoices.some(choice => choice === value)
}

export function compactionChoice(home = process.env.KUVO_HOME): CompactionChoice {
  try {
    const value: unknown = home && JSON.parse(readFileSync(compactionFile(home), "utf8"))
    if (isCompactionChoice(value)) return value
  } catch {}
  return DEFAULT_COMPACTION
}

export interface ContextSize {
  used: number
  window?: number
  /** Where it's summarized by itself, in tokens, where this box's setting decides that. */
  summarizeAt?: number
}

/** The agents that summarize at this box's "Summarize long conversations at" choice. */
const choosing = new Set(["claude", "codex", "grok"])

/**
 * Where a conversation of agent `kind`, on a model holding `window`, is summarized by itself: the
 * box's choice as each agent takes it (`never`: Claude Code under a 900k window still keeps 400k).
 * None where its agent decides instead: another agent, `never` elsewhere, or a choice so near the
 * window that the agent's own margin wins (Codex stops at 90% of it, Grok 85%, Claude Code 33k short).
 */
export function summarizeAt(kind: string | undefined, window: number | undefined, choice: CompactionChoice = compactionChoice()): number | undefined {
  if (!kind || !choosing.has(kind)) return
  const at = choice !== "never" ? Number(choice) : kind === "claude" && !(window && window >= 900_000) ? 400_000 : undefined
  if (at === undefined || (window && at > window * 0.85)) return
  return at
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
