// Plan usage asked of the coding agent itself, in the account's own folder, as T3 Code and OpenBot
// do: Codex's app-server (`account/rateLimits/read`, and `account/rateLimitResetCredit/consume` to
// spend a reset credit) and Grok's ACP (`_x.ai/billing`). The agent reads and renews its own
// sign-in as it answers; Kuvo never sends its token anywhere. Each ask starts the agent's program
// for that one answer and stops it.

import { RPC, RPCError } from "../agents/rpc"
import { codexCredits, codexResets } from "./codex-resets"
import { grokBilling } from "./engine-limits"
import { LimitsProblem, type LimitExtra, type LimitReading, type ResetAttempt, type ResetOutcome } from "./limits"

const ANSWER = 30_000

/** Runs `program` with the account's folder, asks it through `ask`, and stops it either way. */
async function asking<T>(argv: string[], env: Record<string, string>, cwd: string, jsonrpc: boolean, ask: (rpc: RPC) => Promise<T>): Promise<T> {
  const rpc = new RPC(argv, { cwd, env, jsonrpc })
  rpc.start()
  try {
    return await ask(rpc)
  } finally {
    rpc.kill()
  }
}

export interface CodexProgram {
  executable: string
  /** The account's folder (CODEX_HOME). */
  home: string
}

const codexEnv = (home: string) => ({ CODEX_HOME: home, CODEX_INTERNAL_ORIGINATOR_OVERRIDE: process.env.CODEX_INTERNAL_ORIGINATOR_OVERRIDE ?? "kuvo-code" })

function codexServer<T>(program: CodexProgram, ask: (rpc: RPC) => Promise<T>): Promise<T> {
  return asking([program.executable, "app-server"], codexEnv(program.home), program.home, false, async (rpc) => {
    await rpc.request("initialize", { clientInfo: { name: "kuvo-code", title: "Kuvo Code", version: "1" }, capabilities: { experimentalApi: true } }, ANSWER)
    rpc.notify("initialized")
    return ask(rpc)
  })
}

/** What Codex said, as a problem the usage rows show. */
function codexProblem(error: unknown): LimitsProblem {
  const message = error instanceof Error ? error.message : String(error)
  if (/sign ?in|log ?in|logged out|not authenticated|unauthori[sz]ed|401|refresh token|token (?:is )?(?:expired|invalid)/i.test(message))
    return new LimitsProblem("expired", "Codex isn't signed in here now. Sign in again.")
  return new LimitsProblem("failed", error instanceof RPCError ? `Codex couldn't read its plan: ${message.slice(0, 200)}` : "Codex didn't answer about its plan.")
}

const title = (value: unknown) =>
  typeof value === "string" && value.trim() ? value.trim().split(/[_\s]+/).map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase()).join(" ") : undefined

/** One of Codex's snapshots' windows, under the IDs the rows have always used. */
function snapshotWindows(snapshot: any, prefix: string, model: string | undefined): LimitReading["windows"] {
  return (["primary", "secondary"] as const).flatMap((which) => {
    const window = snapshot?.[which]
    const used = typeof window?.usedPercent === "number" ? window.usedPercent : undefined
    if (!window || used === undefined || !Number.isFinite(used)) return []
    const minutes = typeof window.windowDurationMins === "number" && window.windowDurationMins > 0 ? window.windowDurationMins : undefined
    const resetsAt = typeof window.resetsAt === "number" && window.resetsAt > 0 ? window.resetsAt * 1000 : undefined
    return [{ id: prefix + which, used: used / 100, ...(minutes ? { minutes } : {}), ...(resetsAt ? { resetsAt } : {}), ...(model ? { model } : {}) }]
  })
}

/** Codex's answer (`GetAccountRateLimitsResponse`) as a reading: its plan's windows, each other limit's (Spark's), its credits and reset credits. */
export function codexReading(answer: any, now: number): LimitReading {
  const byID: Record<string, any> = answer?.rateLimitsByLimitId && typeof answer.rateLimitsByLimitId === "object" ? answer.rateLimitsByLimitId : {}
  const main = byID.codex ?? answer?.rateLimits
  const windows = snapshotWindows(main, "", undefined)
  for (const [id, snapshot] of Object.entries(byID)) {
    if (id === "codex" || !snapshot) continue
    windows.push(...snapshotWindows(snapshot, `${id}.`, snapshot.limitName ?? undefined))
  }
  const plan = title(main?.planType)
  const summary = answer?.rateLimitResetCredits
  const resets = summary
    ? codexResets({
        available_count: Number(summary.availableCount),
        credits: Array.isArray(summary.credits) ? summary.credits.map((credit: any) => ({ id: credit.id, status: credit.status, expires_at: credit.expiresAt })) : [],
      }, undefined, now)
    : undefined
  const extras = [codexCredits(main?.credits), resets].filter((extra): extra is LimitExtra => !!extra)
  return { windows, ...(plan ? { plan } : {}), ...(extras.length ? { extras } : {}) }
}

/** The plan's usage, as Codex says it for this account. */
export async function readCodexNative(program: CodexProgram, now: number): Promise<LimitReading> {
  try {
    return codexReading(await codexServer(program, (rpc) => rpc.request("account/rateLimits/read", {}, ANSWER)), now)
  } catch (error) {
    throw codexProblem(error)
  }
}

/** The ChatGPT account Codex is signed in to here, as it says right now (undefined: it couldn't say). */
export async function codexAccountNow(program: CodexProgram): Promise<string | undefined> {
  try {
    const answer: any = await codexServer(program, (rpc) => rpc.request("account/rateLimits/read", { excludeResetCreditDetails: true }, ANSWER))
    return typeof answer?.accountId === "string" && answer.accountId ? answer.accountId : undefined
  } catch {
    return undefined
  }
}

/**
 * Spends one reset credit through Codex: the one `attempt` already chose, else the soonest to
 * expire, kept in the journal before it's spent. The journal's request is Codex's idempotency key,
 * so asked again it's the same request.
 */
export async function spendCodexResetNative(program: CodexProgram, attempt: ResetAttempt, now: number): Promise<{ outcome: ResetOutcome; already?: true }> {
  let sent = false
  try {
    return await codexServer(program, async (rpc) => {
      let credit = attempt.target
      if (!credit) {
        const answer: any = await rpc.request("account/rateLimits/read", {}, ANSWER)
        const credits = (Array.isArray(answer?.rateLimitResetCredits?.credits) ? answer.rateLimitResetCredits.credits : [])
          .filter((each: any) => (each?.status ?? "available") === "available" && typeof each.id === "string" && /^[A-Za-z0-9_-]{1,100}$/.test(each.id) && (each.expiresAt ? each.expiresAt * 1000 : Infinity) > now)
          .sort((a: any, b: any) => (a.expiresAt ?? Infinity) - (b.expiresAt ?? Infinity))
        if (!credits.length) return { outcome: "no-reset" as const }
        credit = String(credits[0].id)
        attempt.keep(credit)
      }
      sent = true
      const answer: any = await rpc.request("account/rateLimitResetCredit/consume", { idempotencyKey: attempt.request, creditId: credit }, ANSWER)
      switch (answer?.outcome) {
        case "reset": return { outcome: "reset" as const }
        case "alreadyRedeemed": return { outcome: "reset" as const, already: true as const }
        case "nothingToReset": return { outcome: "nothing-to-reset" as const }
        case "noCredit": return { outcome: "no-reset" as const }
        default: return { outcome: "unknown" as const }
      }
    })
  } catch {
    // Before the spend was asked, nothing was spent; after, whether it went isn't said.
    return { outcome: sent ? "unknown" : "failed" }
  }
}

export interface GrokProgram {
  executable: string
  /** The account's folder (GROK_HOME). */
  home: string
}

/** SuperGrok's billing period, as Grok says it for this account (its ACP `_x.ai/billing`). */
export async function readGrokNative(program: GrokProgram, now: number): Promise<LimitReading> {
  const env = { GROK_HOME: program.home, GROK_DISABLE_AUTOUPDATER: "1" }
  let body: any
  try {
    body = await asking([program.executable, "agent", "--no-leader", "stdio"], env, program.home, true, async (rpc) => {
      await rpc.request("initialize", { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false }, clientInfo: { name: "kuvo-code", title: "Kuvo Code", version: "1" }, _meta: { clientType: "extension" } }, ANSWER)
      try {
        return await rpc.request("_x.ai/billing", {}, ANSWER)
      } catch (error) {
        if (!(error instanceof RPCError) || !/auth/i.test(`${error.message} ${error.data ?? ""}`)) throw error
        // Not signed in for this process yet: with the sign-in it keeps, as the bridge's sessions do.
        await rpc.request("authenticate", { methodId: "cached_token" }, ANSWER)
        return await rpc.request("_x.ai/billing", {}, ANSWER)
      }
    })
  } catch (error) {
    if (error instanceof RPCError && /auth/i.test(`${error.message} ${error.data ?? ""}`)) throw new LimitsProblem("expired", "Grok isn't signed in here now. Sign in again.")
    throw new LimitsProblem("failed", "Grok didn't answer about its plan.")
  }
  return grokBilling(body, now)
}

export interface ClaudeProgram {
  executable: string
  /** An added account's folder (CLAUDE_CONFIG_DIR); none for your own sign-in, where Claude Code keeps it. */
  home?: string
  /** Where it runs (a folder of the box's own, never a project). */
  cwd: string
}

/**
 * Claude Code's own `/usage`, asked as the Agent SDK asks it (its control request `get_usage`,
 * without the scan of conversations: plan limits only), of a Claude Code started for that one
 * answer with the account's sign-in. It renews that sign-in itself as it answers.
 */
async function askClaude(program: ClaudeProgram): Promise<any> {
  const { spawn } = await import("node:child_process")
  const env: Record<string, string | undefined> = { ...process.env, CLAUDE_CODE_OAUTH_TOKEN: undefined, DISABLE_AUTOUPDATER: "1" }
  if (program.home) env.CLAUDE_CONFIG_DIR = program.home
  const child = spawn(program.executable, [
    "--output-format", "stream-json", "--verbose", "--input-format", "stream-json",
    "--no-session-persistence", "--strict-mcp-config", "--settings", JSON.stringify({ disableAllHooks: true }),
  ], { cwd: program.cwd, env: env as NodeJS.ProcessEnv, stdio: ["pipe", "pipe", "ignore"] })
  const waiting = new Map<string, { resolve: (value: any) => void; reject: (error: Error) => void }>()
  let ended: Error | undefined
  const end = (error: Error) => {
    ended ??= error
    for (const each of waiting.values()) each.reject(ended)
    waiting.clear()
  }
  child.on("error", (error) => end(error))
  child.on("exit", () => end(new Error("Claude Code stopped before it answered.")))
  let buffer = ""
  child.stdout!.setEncoding("utf8")
  child.stdout!.on("data", (chunk: string) => {
    buffer += chunk
    let at
    while ((at = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, at)
      buffer = buffer.slice(at + 1)
      let message: any
      try { message = JSON.parse(line) } catch { continue }
      if (message?.type !== "control_response") continue
      const answer = message.response
      const asked = waiting.get(answer?.request_id)
      if (!asked) continue
      waiting.delete(answer.request_id)
      if (answer.subtype === "success") asked.resolve(answer.response)
      else asked.reject(new ClaudeRefused(typeof answer.error === "string" ? answer.error : JSON.stringify(answer.error ?? "")))
    }
  })
  let next = 0
  const request = (body: Record<string, unknown>) => new Promise<any>((resolve, reject) => {
    if (ended) return reject(ended)
    const id = `kuvo-${++next}`
    const timer = setTimeout(() => { waiting.delete(id); reject(new Error("Claude Code took too long to answer.")) }, ANSWER)
    waiting.set(id, { resolve: (value) => { clearTimeout(timer); resolve(value) }, reject: (error) => { clearTimeout(timer); reject(error) } })
    child.stdin!.write(JSON.stringify({ type: "control_request", request_id: id, request: body }) + "\n")
  })
  try {
    await request({ subtype: "initialize" })
    return await request({ subtype: "get_usage", skip_behaviors: true })
  } finally {
    child.stdin!.end()
    child.kill()
  }
}

class ClaudeRefused extends Error {}

/** The plan's usage, as Claude Code says it for this account: its `get_usage` answer (claude-limits.ts reads it). */
export async function askClaudeUsage(program: ClaudeProgram): Promise<any> {
  let answer: any
  try {
    answer = await askClaude(program)
  } catch (error) {
    if (error instanceof ClaudeRefused && /log ?in|sign ?in|auth|401|token/i.test(error.message)) throw new LimitsProblem("expired", "Claude Code isn't signed in here now. Sign in again.")
    if (error instanceof ClaudeRefused) throw new LimitsProblem("failed", "This Claude Code can't say its plan's usage: update it.")
    throw new LimitsProblem("failed", "Claude Code didn't answer about its plan.")
  }
  // An API key, Bedrock or Vertex: no plan, so no limits to show.
  if (answer?.rate_limits_available === false) throw new LimitsProblem("none", "Not signed in to a Claude plan here.", undefined, false)
  return answer
}
