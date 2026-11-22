// Stand-ins for the coding agents a bridge update must not stop (bridge-update.test.ts): each says
// its pid once, as it starts (`pids`), keeps what it's sent (`sent.jsonl`), and runs each turn as a
// command would, until it's told to go on (`goOn`), then answers "done <turn>".

import { chmodSync, mkdirSync, writeFileSync } from "node:fs"
import net from "node:net"
import { join } from "node:path"
import { ClaudeAgent } from "../../../src/agents/agents/claude"
import { CodexAgent } from "../../../src/agents/agents/codex"
import { AntigravityAgent } from "../../../src/agents/agents/antigravity"
import { GrokAgent } from "../../../src/agents/agents/grok"
import { ANTIGRAVITY_ACP } from "../../../src/agents/hold/adopt"
import type { Agent } from "../../../src/agents/agent"
import { Auto } from "../../../src/agents/auto"
import type { HeldTurn } from "../../../src/agents/hold/adopt"
import type { Roster } from "../../../src/agents/roster"
import { Sessions } from "../../../src/agents/sessions"
import { Store } from "../../../src/agents/store"

/**
 * An ACP agent (Antigravity's, Grok's `grok agent stdio`), as the bridge drives it: one session, and
 * turns that run a command. Antigravity's is its program in its own folder, installed as its agent
 * finds it (`antigravity/<version>`), signed in.
 */
export function standInAcp(folder: string, kind: "antigravity" | "grok"): string {
  const script = join(folder, "acp.js")
  writeFileSync(script, shared(folder) + String.raw`
const args = process.argv.slice(2)
if (args[0] === "models") { console.log("You are logged in.\n  * grok-test (default)"); process.exit(0) }
if (args.includes("--version")) { console.log("grok 1.0.77"); process.exit(0) }
serving()
const update = (sessionId, update) => send({ jsonrpc: "2.0", method: "session/update", params: { sessionId, update } })
let turns = 0
lines(async (message) => {
  if (message.method === undefined) return
  if (message.method === "initialize") return send({ jsonrpc: "2.0", id: message.id, result: { protocolVersion: 1, agentCapabilities: { loadSession: true, sessionCapabilities: { resume: {} } }, authMethods: [{ id: "oauth-personal" }, { id: "cached_token" }] } })
  if (message.method === "session/new") return send({ jsonrpc: "2.0", id: message.id, result: { sessionId: "g1", models: { currentModelId: "acp-test", availableModels: [{ modelId: "acp-test", name: "ACP Test" }] } } })
  if (message.method !== "session/prompt") return message.id !== undefined && send({ jsonrpc: "2.0", id: message.id, result: {} })
  const turn = String(++turns)
  const sessionId = message.params.sessionId
  update(sessionId, { sessionUpdate: "tool_call", toolCallId: "c" + turn, title: "sleep 2700", kind: "execute", status: "in_progress", rawInput: { command: "sleep 2700" } })
  await goOn(turn)
  update(sessionId, { sessionUpdate: "tool_call_update", toolCallId: "c" + turn, status: "completed" })
  update(sessionId, { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "done " + turn } })
  send({ jsonrpc: "2.0", id: message.id, result: { stopReason: "end_turn" } })
})
`)
  if (kind === "grok") return launcher(folder, "grok", script)
  const installed = join(folder, "antigravity", ANTIGRAVITY_ACP)
  mkdirSync(join(folder, "antigravity", "profile", "antigravity-acp"), { recursive: true })
  writeFileSync(join(folder, "antigravity", "profile", "antigravity-acp", "acp_token.json"), "{}", { mode: 0o600 })
  mkdirSync(installed, { recursive: true })
  writeFileSync(join(installed, ".complete"), "")
  writeFileSync(join(installed, "agy_acp_server.par"), `#!/bin/sh\nexport BUN_BE_BUN=1\nexec "${process.execPath}" "${script}" "$@"\n`, { mode: 0o755 })
  return join(installed, "agy_acp_server.par")
}

/** Tells the stand-in in `folder` to go on with `turn`: done once it has heard. */
export function goOn(folder: string, turn: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const socket = net.connect(join(folder, "go.sock"), () => socket.end(`${turn}\n`))
    socket.on("close", () => resolve())
    socket.on("error", reject)
  })
}

/** A launcher in `folder` that runs `script` on this Bun. */
function launcher(folder: string, name: string, script: string): string {
  const path = join(folder, name)
  writeFileSync(path, `#!/bin/sh\nexport BUN_BE_BUN=1\nexec "${process.execPath}" "${script}" "$@"\n`)
  chmodSync(path, 0o755)
  return path
}

const shared = (folder: string) => String.raw`
const fs = require("node:fs")
const path = require("node:path")
const folder = ${JSON.stringify(folder)}
const send = (message) => process.stdout.write(JSON.stringify(message) + "\n")
/** Once it's told to go on with a turn (its number, on its socket): as a command running in the turn ends. */
const told = new Set()
const waiting = new Map()
/** As it starts to serve (not to say its version): its pid said, and it listens to be told. */
function serving() {
  fs.appendFileSync(path.join(folder, "pids"), process.pid + "\n")
  fs.rmSync(path.join(folder, "go.sock"), { force: true })
  require("node:net").createServer((socket) => {
    socket.setEncoding("utf8")
    socket.on("data", (text) => {
      for (const turn of text.split("\n").filter(Boolean)) {
        told.add(turn)
        waiting.get(turn)?.()
      }
      socket.end()
    })
  }).listen(path.join(folder, "go.sock")).unref()
}
const goOn = (turn) => new Promise((resolve) => (told.has(turn) ? resolve() : waiting.set(turn, resolve)))
let buffer = ""
process.stdin.on("end", () => process.exit(0))
function lines(each) {
  process.stdin.on("data", (chunk) => {
    buffer += chunk
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = buffer.slice(0, at)
      buffer = buffer.slice(at + 1)
      if (!line.trim()) continue
      fs.appendFileSync(path.join(folder, "sent.jsonl"), line + "\n")
      each(JSON.parse(line))
    }
  })
}
`

/** Codex's app-server, as the bridge drives it: a thread, and turns that run a command. */
export function standInCodex(folder: string): string {
  const script = join(folder, "codex.js")
  writeFileSync(script, shared(folder) + String.raw`
if (process.argv[2] !== "app-server") { console.log("codex-cli 0.200.0"); process.exit(0) }
serving()
let turns = 0
lines(async (message) => {
  if (message.method === undefined) return
  if (message.method === "thread/start" || message.method === "thread/resume") return send({ id: message.id, result: { thread: { id: "t1" } } })
  if (message.method === "model/list") return send({ id: message.id, result: { data: [{ id: "gpt-test", model: "gpt-test", displayName: "GPT Test", isDefault: true }] } })
  if (message.method !== "turn/start") return message.id !== undefined && send({ id: message.id, result: {} })
  const turn = String(++turns)
  const at = { threadId: "t1", turnId: "u" + turn }
  send({ id: message.id, result: { turn: { id: "u" + turn, status: "inProgress" } } })
  send({ method: "turn/started", params: { threadId: "t1", turn: { id: "u" + turn } } })
  send({ method: "item/started", params: { ...at, item: { type: "commandExecution", id: "c" + turn, command: "sleep 2700", cwd: folder, status: "inProgress" } } })
  await goOn(turn)
  send({ method: "item/completed", params: { ...at, item: { type: "commandExecution", id: "c" + turn, command: "sleep 2700", cwd: folder, status: "completed", exitCode: 0, aggregatedOutput: "" } } })
  send({ method: "item/completed", params: { ...at, item: { type: "agentMessage", id: "m" + turn, text: "done " + turn } } })
  send({ method: "turn/completed", params: { threadId: "t1", turn: { id: "u" + turn, status: "completed" } } })
})
`)
  return launcher(folder, "codex", script)
}

/**
 * Claude Code, as its SDK drives it (stream-json on its pipes): a second `initialize`, from a new SDK
 * client joining it, is taken as Claude Code 2.1.325 takes it (spike S1).
 */
export function standInClaude(folder: string): string {
  const script = join(folder, "claude.js")
  writeFileSync(script, shared(folder) + String.raw`
if (process.argv.includes("--version")) { console.log("2.1.300 (Claude Code)"); process.exit(0) }
serving()
const session = "00000000-0000-4000-8000-00000000c1a0"
let turns = 0
const uuid = (n) => "00000000-0000-4000-8000-" + String(n).padStart(12, "0")
lines(async (message) => {
  if (message.type === "control_request") {
    const response = message.request?.subtype === "initialize" ? { commands: [], models: [], agents: [], account: {}, pending_permission_requests: [] } : {}
    return send({ type: "control_response", response: { subtype: "success", request_id: message.request_id, response } })
  }
  if (message.type !== "user") return
  const turn = String(++turns)
  send({ type: "system", subtype: "init", session_id: session, claude_code_version: "2.1.300", cwd: folder, tools: [], mcp_servers: [], model: "stand-in", permissionMode: "default", slash_commands: [], apiKeySource: "none", output_style: "default", uuid: uuid(turn * 10) })
  await goOn(turn)
  // (Which of your messages it took, as Claude Code says from 2.1.2xx on.)
  const took = message.uuid ? [message.uuid] : []
  send({ type: "assistant", message: { id: "m" + turn, role: "assistant", type: "message", model: "stand-in", content: [{ type: "text", text: "done " + turn }], stop_reason: "end_turn", usage: { input_tokens: 1, output_tokens: 1 } }, parent_tool_use_id: null, session_id: session, uuid: uuid(turn * 10 + 1), user_message_uuids: took })
  send({ type: "result", subtype: "success", is_error: false, result: "done " + turn, num_turns: 1, duration_ms: 1, duration_api_ms: 1, total_cost_usd: 0, usage: { input_tokens: 1, output_tokens: 1 }, session_id: session, uuid: uuid(turn * 10 + 2), user_message_uuids: took })
})
`)
  return launcher(folder, "claude", script)
}


export type Kind = "codex" | "claude" | "antigravity" | "grok"

class UpdateCodex extends CodexAgent {
  constructor(readonly launcher: string, home: string) {
    super({ home, temporary: home, deps: {} as any })
  }
  override executable() {
    return this.launcher
  }
  override async account() {
    return { installed: true, signedIn: true }
  }
}

class UpdateClaude extends ClaudeAgent {
  constructor(readonly launcher: string, home: string) {
    super({ home, temporary: home, deps: {} as any })
  }
  // (The SDK the box is built with, from its own packages.)
  override async sdk(): Promise<any> {
    return import(join(import.meta.dir, "../../../node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs"))
  }
  override executable() {
    return this.launcher
  }
  override async account() {
    return { installed: true, signedIn: true }
  }
}

class UpdateGrok extends GrokAgent {
  constructor(readonly launcher: string, home: string) {
    super({ home, temporary: home, deps: {} as any })
  }
  override executable() {
    return this.launcher
  }
  override async account() {
    return { installed: true, signedIn: true }
  }
}

class UpdateAntigravity extends AntigravityAgent {
  constructor(home: string) {
    super({ home, temporary: home, deps: {} as any })
  }
  override async account() {
    return { installed: true, signedIn: true }
  }
}

/** One bridge's agent of `kind` and its sessions, over the session store in `folder`, as a bridge starts with them. */
export function bridge(kind: Kind, folder: string, launcher: string, held: Map<string, HeldTurn> = new Map()): { agent: Agent; sessions: Sessions } {
  // (Claude Code's own sign-in, as on a machine.)
  ClaudeAgent.keychain = false
  const agent: Agent = kind === "codex" ? new UpdateCodex(launcher, folder) : kind === "claude" ? new UpdateClaude(launcher, folder) : kind === "grok" ? new UpdateGrok(launcher, folder) : new UpdateAntigravity(folder)
  const roster = { find: (id: string | undefined) => (id === agent.providerID ? agent : undefined), all: () => [agent] } as unknown as Roster
  const sessions = new Sessions(new Store(join(folder, "sessions")), roster, new Auto(folder), undefined, undefined, undefined, undefined, undefined, held)
  return { agent, sessions }
}

export const modelOf = (kind: Kind, agent: Agent) => ({ providerID: agent.providerID, id: kind === "codex" ? "gpt-test" : kind === "claude" ? "default" : "acp-test" })
