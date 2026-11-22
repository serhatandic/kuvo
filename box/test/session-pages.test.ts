import "./safety"
import { afterAll, expect, test } from "bun:test"
import { mkdirSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"

// A project's list a page at a time (#951) and a session's subagents under it (#950), as the box
// answers them (PROTOCOL.md, "Session lists"): its first sessions in the box's order, those at work
// or waiting on you first; "Show more" adding 10, then 20, then 40, by the cursor the box gives;
// and each subagent listed on the session that started it, saying how it takes your messages.

// (The router first: the lists alone load the link's modules in an order that doesn't start.)
const { engineHandler } = await import("../src/link/server/router")
const { LIST_FILE } = await import("../src/link/lists/sessions")

const home = process.env.KUVO_HOME!
const work = join(home, "pages-work")
mkdirSync(join(home, "box"), { recursive: true })
mkdirSync(work, { recursive: true })
writeFileSync(join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: work, folders: [], name: "Work" }] }))

// A fake engine with a project of 80 sessions, one a day; the oldest is at work.
const sessions = new Map<string, any>()
const active = new Set<string>()
const add = (id: string, updated: number, extra: Record<string, unknown> = {}) =>
  sessions.set(id, { id, location: { directory: work }, time: { created: updated, updated }, ...extra })
for (let day = 1; day <= 80; day++) add(`ses_d${String(day).padStart(2, "0")}`, day * 1000)
active.add("ses_d01")
// A conversation with two of Claude Code's subagents: one that takes messages through it, and an
// Explore agent that takes none; and one the engine's own task started.
add("ses_d79s1", 79_100, { parentID: "ses_d79", metadata: { kuvoSubagent: true, kuvo: { messages: { via: "parent" } } } })
add("ses_d79s2", 79_200, { parentID: "ses_d79", metadata: { kuvoSubagent: true, kuvo: { messages: { via: "none", note: "Claude Code's Explore agents can't take messages." } } } })
add("ses_d80s1", 80_100, { parentID: "ses_d80" })

const engine = Bun.serve({
  port: 0,
  fetch(request) {
    const url = new URL(request.url)
    if (url.pathname === "/api/session") {
      const all = [...sessions.values()].sort((a, b) => b.time.updated - a.time.updated)
      const from = Number(url.searchParams.get("cursor") ?? 0)
      const page = all.slice(from, from + Number(url.searchParams.get("limit")))
      return Response.json({ data: page, cursor: { next: from + page.length < all.length ? String(from + page.length) : null } })
    }
    if (url.pathname === "/api/session/active") return Response.json({ data: Object.fromEntries([...active].map(id => [id, { type: "busy" }])) })
    if (/^\/api\/session\/[^/]+\/(permission|form|message)$/.test(url.pathname)) return Response.json({ data: [] })
    return Response.json({ message: "Not here." }, { status: 404 })
  },
})
afterAll(() => {
  engine.stop(true)
  for (const file of [join(home, "box", "projects.json"), LIST_FILE]) rmSync(file, { force: true })
})

const handler = engineHandler(() => ({ port: engine.port, password: "pw" }) as any, { name: "test" }, () => { throw new Error("none") })
async function ask(path: string): Promise<{ status: number; body: any }> {
  let status = 0
  let text = ""
  for await (const piece of handler({ method: "GET", path, headers: {}, fromApp: true }, undefined, new AbortController().signal)) {
    if ("head" in piece) status = piece.head.status
    if ("body" in piece) text += Buffer.from(piece.body).toString()
  }
  return { status, body: JSON.parse(text) }
}
const page = (cursor?: string) => ask(`kuvo/sessions/page?project=${encodeURIComponent(work)}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`)
const day = (n: number) => `ses_d${String(n).padStart(2, "0")}`

test("a project's list shows those at work first, then the newest, and Show more adds 10, 20, then 40", async () => {
  const { status, body } = await ask("kuvo/sessions/list")
  expect(status).toBe(200)
  const project = body.projects[work]
  // The oldest, at work, before the newest five; subagents aren't counted in its list.
  expect(project.shown).toEqual(["ses_d01", day(80), day(79), day(78), day(77), day(76)])
  expect(project.total).toBe(80)
  expect(project.more.count).toBe(10)

  const first = await page(project.more.cursor)
  expect(first.status).toBe(200)
  expect(first.body.sessions).toHaveLength(16)
  expect(first.body.sessions.slice(0, 6)).toEqual(project.shown)
  expect(first.body.sessions.at(-1)).toBe(day(66))
  expect(first.body).toMatchObject({ project: work, total: 80, more: { count: 20 } })

  const second = await page(first.body.more.cursor)
  expect(second.body.sessions).toHaveLength(36)
  expect(second.body.more.count).toBe(40)
  // The last one: what's left, then nothing more.
  const third = await page(second.body.more.cursor)
  expect(third.body.sessions).toHaveLength(76)
  expect(third.body.more.count).toBe(4)
  const all = await page(third.body.more.cursor)
  expect(all.body.sessions).toHaveLength(80)
  expect(new Set(all.body.sessions).size).toBe(80)
  expect(all.body.more).toBeNull()

  // Asked again with the same cursor as the list changes: the same reach, in the order now.
  active.delete("ses_d01")
  active.add(day(30))
  const again = await page(project.more.cursor)
  expect(again.body.sessions[0]).toBe(day(30))
  expect(again.body.sessions).toHaveLength(16)
  expect(again.body.sessions).not.toContain("ses_d01")
})

test("a page names its project and a cursor the box gave", async () => {
  expect((await ask("kuvo/sessions/page")).status).toBe(400)
  expect((await page("s7.zz")).status).toBe(400)
  expect((await page()).body.sessions).toHaveLength(6)
})

test("a session lists its subagents, those at work first, and each says how it takes your messages", async () => {
  active.add("ses_d79s1")
  const { body } = await ask("kuvo/sessions/list")
  expect(body.sessions[day(79)].subagents).toEqual(["ses_d79s1", "ses_d79s2"])
  expect(body.sessions[day(80)].subagents).toEqual(["ses_d80s1"])
  expect(body.sessions.ses_d79s1).toMatchObject({ kind: "subagent", parent: day(79), messages: { via: "parent" } })
  expect(body.sessions.ses_d79s2.messages).toEqual({ via: "none", note: "Claude Code's Explore agents can't take messages." })
  // The engine's own subagent takes them directly.
  expect(body.sessions.ses_d80s1.messages).toEqual({ via: "direct" })
  expect(body.sessions[day(78)].subagents).toBeUndefined()
})
