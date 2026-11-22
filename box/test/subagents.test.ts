import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { ask, follow } from "./route-answer"

// A conversation's subagents, at work and done (`kuvo/work/subagents/<session>`, PROTOCOL.md "Work ›
// Background work"), as a side panel lists them: the box's own fact, so no client groups, filters or
// judges them. The stand-ins feed the session list and can go away once it has their facts.

const { engineHandler } = await import("../src/link/server/router")
const { backgroundFollowers, WORK_BACKGROUND } = await import("../src/link/work/background")
const { subagentsOf } = await import("../src/link/work/subagents")
const { LIST_FILE, keepList, listPlaceHeard, noteListEvent, keptList, workOut } = await import("../src/link/lists/sessions")

type Session = { id: string; parentID?: string; title?: string; model?: object; outcome?: string; time: { created: number; updated: number; idle?: number }; metadata?: object }
let engineSessions: Session[]
let bridgeSessions: Session[]
let reads = 0
let failing = false
const sonnet = { providerID: "anthropic", id: "claude-sonnet-4-5", variant: "high" }
beforeEach(() => {
  failing = false
  reads = 0
  engineSessions = [
    { id: "ses_main", title: "Sign-up form", time: { created: 1, updated: 900 } },
    { id: "ses_explore", parentID: "ses_main", title: "Explore the auth code (@explore subagent)", model: sonnet, outcome: "succeeded", time: { created: 100, updated: 300, idle: 300 } },
    { id: "ses_stopped", parentID: "ses_main", title: "Look for flaky tests", outcome: "interrupted", time: { created: 150, updated: 250, idle: 250 } },
    { id: "ses_tests", parentID: "ses_main", title: "Run the tests", outcome: "failed", time: { created: 200, updated: 400, idle: 400 } },
    // A subagent's own: listed too.
    { id: "ses_nested", parentID: "ses_tests", title: "Read the failing test", outcome: "succeeded", time: { created: 410, updated: 450, idle: 450 } },
    { id: "ses_running", parentID: "ses_main", title: "Write the migration", model: sonnet, time: { created: 500, updated: 600 } },
    // Not listed: a review's part (its review shows it), and one made just now that hasn't run.
    { id: "ses_part", parentID: "ses_main", title: "Review part", outcome: "succeeded", metadata: { kuvo: { reviewPart: { review: "rev_1" } } }, time: { created: 120, updated: 130, idle: 130 } },
    { id: "ses_new", parentID: "ses_main", title: "Starting", time: { created: 700, updated: 700 } },
    // Another conversation's.
    { id: "ses_other", parentID: "ses_elsewhere", title: "Theirs", outcome: "succeeded", time: { created: 1, updated: 2, idle: 2 } },
  ]
  bridgeSessions = [
    { id: "ses_agent", title: "Claude Code conversation", time: { created: 1, updated: 50 } },
    { id: "ses_agent_sub", parentID: "ses_agent", title: "Search the docs", metadata: { kuvo: { requestedModel: "gemini-flash" } }, outcome: "succeeded", time: { created: 10, updated: 40, idle: 40 } },
  ]
})

/** A source for the session list keeper, as the engine and bridge each enumerate their sessions. */
const place = (sessions: () => Session[]) => Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: request => {
  reads++
  const url = new URL(request.url)
  if (failing) return Response.json({ message: "Busy." }, { status: 500 })
  if (url.pathname === "/api/session") return Response.json({ data: sessions(), cursor: {} })
  if (url.pathname === "/api/session/active") return Response.json({ data: [] })
  const found = sessions().find(each => url.pathname === `/api/session/${each.id}`)
  if (found) return Response.json({ data: found })
  return Response.json({ message: "Not found" }, { status: 404 })
} })
const engineServer = place(() => engineSessions)
const bridgeServer = place(() => bridgeSessions)
const engine = { port: engineServer.port, password: "test" } as any
const bridge = { port: bridgeServer.port, password: "test" } as any
afterAll(() => { engineServer.stop(true); bridgeServer.stop(true) })

const scratch = mkdtempSync(join(tmpdir(), "kuvo-subagents-"))
afterAll(() => rmSync(scratch, { recursive: true, force: true }))
/** What the box keeps of what's at work under a session (`run/work/background.json`), as its watcher writes it. */
const atWork = (file: string, session: string, subagents: { id: string; since: number }[]) => {
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, JSON.stringify({ engine: { sessions: { [session]: { commands: [], subagents, tasks: [] } }, folders: [] } }))
}
async function kept(run: () => Promise<void>) {
  const saved = [LIST_FILE, WORK_BACKGROUND].map(file => [file, existsSync(file) ? readFileSync(file) : undefined] as const)
  const stop = keepList()
  try {
    await listPlaceHeard("engine", engine)
    await listPlaceHeard("bridge", bridge)
    await run()
  } finally {
    stop()
    for (const [file, buffer] of saved) {
      if (buffer) writeFileSync(file, buffer)
      else rmSync(file, { force: true })
    }
  }
}

test("those at work and those done are listed with their titles, models and outcomes", () => kept(async () => {
  const file = join(scratch, "main.json")
  atWork(file, "ses_main", [{ id: "ses_running", since: 500 }])
  const list = keptList()!
  list.sessions.ses_running.facts = { ...list.sessions.ses_running.facts, asked: 650 }
  expect(subagentsOf("ses_main", { file, list })).toEqual({
    active: [{ session: "ses_running", title: "Write the migration", since: 500, asking: true, model: "Claude Sonnet 4.5" }],
    done: [
      { session: "ses_nested", title: "Read the failing test", started: 410, ended: 450, state: "finished" },
      { session: "ses_tests", title: "Run the tests", started: 200, ended: 400, state: "failed" },
      { session: "ses_explore", title: "Explore the auth code (@explore subagent)", started: 100, ended: 300, state: "finished", model: "Claude Sonnet 4.5" },
      { session: "ses_stopped", title: "Look for flaky tests", started: 150, ended: 250, state: "stopped" },
    ],
  })
  expect(subagentsOf("ses_agent", { file })).toEqual({
    active: [],
    done: [{ session: "ses_agent_sub", title: "Search the docs", started: 10, ended: 40, state: "finished", model: expect.stringMatching(/Flash.*\(requested\)$/i) }],
  })
}))

test("subagent routes say what is kept during an outage, then follow the next turn end", () => kept(async () => {
  atWork(WORK_BACKGROUND, "ses_main", [{ id: "ses_running", since: 500 }])
  const handler = engineHandler((() => engine) as any, { name: "test" } as any, (() => { throw new Error("No bridge") }) as any)
  reads = 0
  failing = true
  const said = await ask(handler, "GET", "kuvo/work/subagents/ses_main")
  expect(said.status).toBe(200)
  expect(said.json.active.map((each: { session: string }) => each.session)).toEqual(["ses_running"])
  expect(said.json.done.map((each: { session: string }) => each.session)).toEqual(["ses_nested", "ses_tests", "ses_explore", "ses_stopped"])
  const events = follow(handler, "kuvo/work/subagents/ses_main/events")
  try {
    expect(await events.next()).toEqual(said.json)
    expect(reads).toBe(0)
    failing = false
    Object.assign(engineSessions.find(each => each.id === "ses_running")!, { outcome: "succeeded", time: { created: 500, updated: 800, idle: 800 } })
    atWork(WORK_BACKGROUND, "ses_main", [])
    await noteListEvent("engine", engine, { type: "session.execution.succeeded", data: { sessionID: "ses_running" } })
    workOut()
    for (const tell of backgroundFollowers) tell()
    const next = await events.next()
    expect(next.active).toEqual([])
    expect(next.done[0]).toEqual({ session: "ses_running", title: "Write the migration", started: 500, ended: 800, state: "finished", model: "Claude Sonnet 4.5" })
  } finally {
    events.stop()
  }
}))
