// Stand-ins for the coding agents' own programs, as the usage reader and the bridge run them:
// `codex app-server` (JSON-RPC lines) and `grok agent stdio` (ACP), each answering from the folder
// it's given (CODEX_HOME, GROK_HOME), as the real ones read their sign-in there, and `codex logout`
// / `grok logout`. What each was asked is written to `asked.jsonl` beside them.
//
// In a folder: `<folder>/auth.json` is its sign-in (Codex's `tokens.account_id` says who);
// `<folder>/usage.json` is Codex's answer to `account/rateLimits/read`, by account (`*`: any);
// `<folder>/billing.json` is Grok's answer to `_x.ai/billing`.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"

const script = String.raw`
const { appendFileSync, readFileSync, existsSync } = require("node:fs")
const { join } = require("node:path")
const [program, ...args] = process.argv.slice(2)
const log = process.env.STAND_IN_LOG_FILE
const note = (entry) => appendFileSync(log, JSON.stringify(entry) + "\n")
const home = program === "codex" ? process.env.CODEX_HOME : process.env.GROK_HOME
const read = (name) => { try { return JSON.parse(readFileSync(join(home, name), "utf8")) } catch { return undefined } }
if (args[0] === "logout") { note({ program, home, args }); process.exit(0) }
const jsonrpc = program === "grok"
let authenticated = false
const consumed = new Set()
const answer = (id, result, error) => process.stdout.write(JSON.stringify({ ...(jsonrpc ? { jsonrpc: "2.0" } : {}), id, ...(error ? { error } : { result }) }) + "\n")
let buffer = ""
process.stdin.on("data", (chunk) => {
  buffer += chunk
  let end
  while ((end = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, end); buffer = buffer.slice(end + 1)
    if (!line.trim()) continue
    const { id, method, params } = JSON.parse(line)
    if (id === undefined) continue
    const auth = read("auth.json")
    note({ program, home, method, params, ...(auth && auth.tokens && auth.tokens.account_id ? { account: auth.tokens.account_id } : {}) })
    if (method === "initialize") answer(id, jsonrpc ? { protocolVersion: 1, authMethods: [{ id: "grok.com" }] } : { userAgent: "stand-in" })
    else if (method === "authenticate") { authenticated = !!auth; auth ? answer(id, {}) : answer(id, undefined, { code: -32000, message: "Authentication required" }) }
    else if (method === "_x.ai/billing") {
      if (!auth || !authenticated) answer(id, undefined, { code: -32000, message: "Authentication required", data: "Authentication required to fetch billing data" })
      else answer(id, read("billing.json"))
    } else if (method === "account/rateLimits/read") {
      const account = auth && auth.tokens && auth.tokens.account_id
      if (!account) answer(id, undefined, { code: -32000, message: "not logged in" })
      else { const usage = read("usage.json") || {}; answer(id, { ...(usage[account] || usage["*"]), accountId: account }) }
    } else if (method === "account/rateLimitResetCredit/consume") {
      const outcome = consumed.has(params.idempotencyKey) ? "alreadyRedeemed" : "reset"
      consumed.add(params.idempotencyKey)
      answer(id, { outcome })
    } else answer(id, undefined, { code: -32601, message: "Method not found: " + method })
  }
})
`

export interface AgentPrograms {
  codex: string
  grok: string
  /** What each was asked, in order. */
  asked(): { program: string; home: string; method?: string; params?: any; args?: string[]; account?: string }[]
}

/** The stand-in programs, in `folder`. */
export function agentPrograms(folder: string): AgentPrograms {
  mkdirSync(folder, { recursive: true })
  const source = join(folder, "stand-in.cjs")
  const log = join(folder, "asked.jsonl")
  writeFileSync(source, script)
  writeFileSync(log, "")
  const program = (name: string) => {
    const path = join(folder, name)
    writeFileSync(path, `#!/bin/sh\nSTAND_IN_LOG_FILE='${log}' exec '${process.execPath}' '${source}' ${name} "$@"\n`, { mode: 0o755 })
    return path
  }
  return {
    codex: program("codex"),
    grok: program("grok"),
    asked: () => readFileSync(log, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)),
  }
}

export const keepJSON = (file: string, body: unknown) => {
  mkdirSync(join(file, ".."), { recursive: true })
  writeFileSync(file, JSON.stringify(body))
}
