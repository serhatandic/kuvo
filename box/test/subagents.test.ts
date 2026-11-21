import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { ask, follow } from "./route-answer"

// A conversation's subagents, at work and done (`kuvo/work/subagents/<session>`, PROTOCOL.md "Work ›
// Background work"), as a side panel lists them: the box's own fact, so no client groups, filters or
// judges them. The box's real code; the engine and the coding agents' bridge stand-ins that list the
// sessions under one as theirs do, and the engine its models, as it names them.

const { engineHandler } = await import("../src/link/server/router")
const { backgroundFollowers, WORK_BACKGROUND } = await import("../src/link/work/background")
const { subagentsOf } = await import("../src/link/work/subagents")

type Session = { id: string; parentID?: string; title?: string; model?: object; outcome?: string; time: { created: number; updated: number; idle?: number }; metadata?: object }
let engineSessions: Session[]
let bridgeSessions: Session[]
let failing = false
const sonnet = { providerID: "anthropic", id: "claude-sonnet-4-5", variant: "high" }
beforeEach(() => {
  failing = false
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
    { id: "ses_agent_sub", parentID: "ses_agent", title: "Search the docs", outcome: "succeeded", time: { created: 10, updated: 40, idle: 40 } },
  ]
})

/** A place that lists the sessions under one (`api/session?parentID=`), as the engine and the bridge
 * do; the engine also its models, as the box reads them to name one. */
const place = (sessions: () => Session[], models = false) => Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: request => {
  const url = new URL(request.url)
  if (failing) return Response.json({ message: "Busy." }, { status: 500 })
  if (url.pathname === "/api/session" && url.searchParams.has("parentID")) return Response.json({ data: sessions().filter(each => each.parentID === url.searchParams.get("parentID")), cursor: {} })
  if (models && url.pathname === "/api/provider") return Response.json({ data: [{ id: "anthropic" }] })
  if (models && url.pathname === "/api/model") return Response.json({ data: [{ id: "claude-sonnet-4-5", providerID: "anthropic", name: "Claude Sonnet 4.5" }] })
  if (models && url.pathname === "/api/integration") return Response.json({ data: [] })
  if (models && url.pathname === "/api/model/default") return Response.json({ data: null })
  return Response.json({ message: "Not found" }, { status: 404 })
} })
const engineServer = place(() => engineSessions, true)
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
// The session lists' facts, as the box keeps them: a title, and when one began waiting on you.
const list = (facts: Record<string, Record<string, unknown>>) => ({
  epoch: "e", seq: 1, floor: 0, gone: {}, projects: {},
  sessions: Object.fromEntries(Object.entries(facts).map(([id, each]) => [id, { seq: 1, facts: { kind: "subagent", directory: "/work/site", created: 1, updated: 1, parent: "ses_main", ...each } }])),
}) as any

test("those at work, as Background work lists them, and those done, the latest first, each with how it ended and the model the box names", async () => {
  const file = join(scratch, "main.json")
  atWork(file, "ses_main", [{ id: "ses_running", since: 500 }])
  const said = await subagentsOf("ses_main", { engine, bridge: null }, undefined, { file, list: list({ ses_running: { title: "Write the migration", asked: 650 } }) })
  expect(said).toEqual({
    active: [{ session: "ses_running", title: "Write the migration", since: 500, asking: true, model: "Claude Sonnet 4.5" }],
    done: [
      { session: "ses_nested", title: "Read the failing test", started: 410, ended: 450, state: "finished" },
      { session: "ses_tests", title: "Run the tests", started: 200, ended: 400, state: "failed" },
      { session: "ses_explore", title: "Explore the auth code (@explore subagent)", started: 100, ended: 300, state: "finished", model: "Claude Sonnet 4.5" },
      { session: "ses_stopped", title: "Look for flaky tests", started: 150, ended: 250, state: "stopped" },
    ],
  })
})

test("a coding agent's subagents, as their bridge keeps them, with no model of their own to name", async () => {
  const file = join(scratch, "agent.json")
  atWork(file, "ses_agent", [])
  expect(await subagentsOf("ses_agent", { engine, bridge }, undefined, { file, list: null })).toEqual({
    active: [],
    done: [{ session: "ses_agent_sub", title: "Search the docs", started: 10, ended: 40, state: "finished" }],
  })
  // A place that runs and doesn't answer: nothing said, rather than none.
  failing = true
  expect(await subagentsOf("ses_agent", { engine, bridge }, undefined, { file, list: null })).toBeNull()
})

test("kuvo/work/subagents answers in its shape, and its events say it again as one finishes", async () => {
  // (The box's own file, as it was before, afterwards.)
  const before = existsSync(WORK_BACKGROUND) ? readFileSync(WORK_BACKGROUND) : null
  try {
    atWork(WORK_BACKGROUND, "ses_main", [{ id: "ses_running", since: 500 }])
    const handler = engineHandler((() => engine) as any, { name: "test" } as any, (() => { throw new Error("No bridge") }) as any)
    const said = await ask(handler, "GET", "kuvo/work/subagents/ses_main")
    expect(said.status).toBe(200)
    expect(said.json.active.map((each: { session: string }) => each.session)).toEqual(["ses_running"])
    expect(said.json.done.map((each: { session: string; state: string }) => [each.session, each.state])).toEqual([["ses_nested", "finished"], ["ses_tests", "failed"], ["ses_explore", "finished"], ["ses_stopped", "stopped"]])
    const events = follow(handler, "kuvo/work/subagents/ses_main/events")
    try {
      expect(await events.next()).toEqual(said.json)
      // It finishes: its place says so, Background work lets it go, and those following are told.
      Object.assign(engineSessions.find(each => each.id === "ses_running")!, { outcome: "succeeded", time: { created: 500, updated: 800, idle: 800 } })
      atWork(WORK_BACKGROUND, "ses_main", [])
      for (const tell of backgroundFollowers) tell()
      const next = await events.next()
      expect(next.active).toEqual([])
      expect(next.done[0]).toEqual({ session: "ses_running", title: "Write the migration", started: 500, ended: 800, state: "finished", model: "Claude Sonnet 4.5" })
    } finally {
      events.stop()
    }
    // Its engine not answering: said so.
    failing = true
    expect(await ask(handler, "GET", "kuvo/work/subagents/ses_main")).toMatchObject({ status: 503 })
  } finally {
    if (before) writeFileSync(WORK_BACKGROUND, before)
    else rmSync(WORK_BACKGROUND, { force: true })
  }
})
