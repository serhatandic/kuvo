// The smallest usage observation Insights keeps or shares. It contains no prompt or reply text.
import { createHash } from "node:crypto"
import type { Tokens } from "./pricing"

export interface InsightRecord {
  key: string
  supersedes?: string
  attributed?: true
  observedAt?: number
  at: number
  session: string
  nativeSession?: string
  call: string
  computer: string
  provider: string
  account?: string
  /** The account was known when the call ran. */
  accountObserved?: true
  model?: string
  pricingModels?: string[]
  bot?: string
  botName?: string
  agent?: string
  title?: string
  skills?: string[]
  tokens?: TokenBuckets
  /** Processed tokens when a source supplied a total but omitted one or more buckets. */
  reportedTotal?: number
  human?: true
  /** Display this call, but don't claim the tokens for a partially reported turn. */
  partial?: true
}

export type TokenBuckets = { [K in keyof Omit<Tokens, "fast" | "tier">]: number | null } & { fast?: boolean; ultrafast?: boolean }

export const insightKey = (...parts: unknown[]) => createHash("sha256").update(JSON.stringify(parts)).digest("hex")

export interface NativeUsage {
  at: number
  session: string
  call: string
  model?: string
  pricingModels?: string[]
  tokens: TokenBuckets
}
