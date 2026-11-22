// Cursor through a bridge update (bridge-update-cursor.test.ts): a stand-in for its SDK on disk, where
// the bridge installs it, which its worker loads as it loads the real one; the bridge's Cursor and
// sessions over a test's folder; and, run as a process of its own, the bridge before the update,
// which leaves its worker at work in a run (`working`) or between runs (`idle`) and, told to, makes
// way as a bridge does, and ends.
//
// usage: bun update-cursor.ts <folder> <runtime> <working|idle>
// (Not ../../safety, which gives a process a home of its own: it runs in its test's, as it's given.)

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import net from "node:net"
import { join } from "node:path"
import { CursorAgent } from "../../../src/agents/agents/cursor"
import { Auto } from "../../../src/agents/auto"
import { agentHoldFolder, type HeldTurn } from "../../../src/agents/hold/adopt"
import { heldIn } from "../../../src/agents/hold/client"
import type { Roster } from "../../../src/agents/roster"
import { Sessions } from "../../../src/agents/sessions"
import { Store } from "../../../src/agents/store"

/**
 * Cursor's SDK as a package in `runtime`, as its worker loads it: the worker says its pid once
 * (`pids` in `folder`), keeps each message it's sent (`sent.jsonl`), and runs each until it's told
 * to go on (`goOn`), then says "done <run>". (Loaded in the bridge too, which only reads its types.)
 */
export function standInCursor(folder: string, runtime: string) {
  const sdk = join(runtime, "node_modules", "@cursor", "sdk")
  mkdirSync(sdk, { recursive: true })
  writeFileSync(join(sdk, "package.json"), JSON.stringify({ name: "@cursor/sdk", version: "1.0.35", type: "module", main: "index.js" }))
  writeFileSync(
    join(sdk, "index.js"),
    String.raw`import { appendFileSync, rmSync } from "node:fs"
import { createServer } from "node:net"
import { join } from "node:path"
const folder = ${JSON.stringify(folder)}
const told = new Set()
const waiting = new Map()
// (Its worker's: as the bridge itself loads it, nothing more.)
if (process.argv.some((arg) => arg.endsWith("kuvo-cursor-worker.mjs"))) {
  appendFileSync(join(folder, "pids"), process.pid + "\n")
  rmSync(join(folder, "go.sock"), { force: true })
  createServer((socket) => {
    socket.setEncoding("utf8")
    socket.on("data", (text) => {
      for (const run of text.split("\n").filter(Boolean)) {
        told.add(run)
        waiting.get(run)?.()
      }
      socket.end()
    })
  }).listen(join(folder, "go.sock")).unref()
}
const goOn = (run) => new Promise((resolve) => (told.has(run) ? resolve() : waiting.set(run, resolve)))
export class JsonlLocalAgentStore {
  constructor(root) { this.root = root }
}
let runs = 0
const agent = (agentId) => ({
  agentId,
  close() {},
  async send(message, { onDelta }) {
    const run = String(++runs)
    appendFileSync(join(folder, "sent.jsonl"), JSON.stringify({ agentId, run, message }) + "\n")
    const ended = goOn(run).then(async () => {
      await onDelta({ update: { type: "text-delta", text: "done " + run } })
      return { status: "finished" }
    })
    return { wait: () => ended, cancel: async () => {} }
  },
})
let next = 1
export const Agent = {
  create: async () => agent("agent-" + next++),
  resume: async (id) => agent(id),
}
export const createAgentPlatform = async () => ({ prewarmLocalWorkspace: async () => undefined })
`,
  )
}

/** Tells Cursor's stand-in worker in `folder` to go on with its `run`: done once it has heard. */
export function goOn(folder: string, run: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const socket = net.connect(join(folder, "go.sock"), () => socket.end(`${run}\n`))
    socket.on("close", () => resolve())
    socket.on("error", reject)
  })
}

class UpdateCursor extends CursorAgent {
  override async account() {
    return { installed: true, signedIn: true }
  }
}

/** One bridge's Cursor and its sessions, over the session store in `folder`, with its SDK in `runtime`. */
export function cursorBridge(folder: string, runtime: string, held: Map<string, HeldTurn> = new Map()) {
  const agent = new UpdateCursor({ home: folder, temporary: folder, deps: { ensure: async () => runtime, load: async (_: string, name: string) => import(Bun.resolveSync(name, runtime)) } as any })
  const roster = { find: (id: string | undefined) => (id === agent.providerID ? agent : undefined), all: () => [agent] } as unknown as Roster
  const sessions = new Sessions(new Store(join(folder, "sessions")), roster, new Auto(folder), undefined, undefined, undefined, undefined, undefined, held)
  return { agent, sessions }
}

export const sentIn = (folder: string): any[] => (existsSync(join(folder, "sent.jsonl")) ? readFileSync(join(folder, "sent.jsonl"), "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line)) : [])

if (import.meta.main) {
  const [folder, runtime, scenario] = process.argv.slice(2) as [string, string, "working" | "idle"]
  const { agent, sessions } = cursorBridge(folder, runtime)
  const hold = agentHoldFolder("cursor")!
  const id = sessions.create({ title: "Update", model: { providerID: agent.providerID, id: "composer-2" }, location: { directory: folder } }).id
  sessions.prompt(id, { id: "msg_update_cursor_00001", text: "run the command", files: [], delivery: "steer" })
  const until = async (condition: () => boolean) => {
    while (!condition()) await new Promise((resolve) => setTimeout(resolve, 10))
  }
  // Under way in its worker: its run noted in the holder's record, and the message with its SDK.
  const turnNoted = () => heldIn(hold).some((record) => Object.values((record.meta.turns ?? {}) as Record<string, { session: string; turn?: string }>).some((note) => note.session === id && note.turn !== undefined))
  await until(() => sentIn(folder).length === 1 && turnNoted())
  if (scenario === "idle") {
    await goOn(folder, 1)
    await until(() => sessions.messages(id).some((message) => message.type === "idle") && !turnNoted())
  }
  console.log(JSON.stringify({ session: id }))
  process.stdin.once("data", () => void sessions.makeWay().finally(() => process.exit(0)))
}
