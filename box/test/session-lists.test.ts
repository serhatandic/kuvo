import "./safety"
import { afterAll, expect, jest, test } from "bun:test"
import fs, { renameSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { execFileSync } from "node:child_process"
import { Database } from "bun:sqlite"
import { dirname, join } from "node:path"

// Every client's session lists come from the box (PROTOCOL.md, "Session lists"): what each session
// is, the project it's filed under and the projects shown, as the Mac's sidebar worked them out
// (protocol/vectors/session-lists.json, which KuvoClient's tests hold the drawing to); kept
// as the engine's events come, and resumed from where a client was after its stream dropped.

// (The router first: the lists alone load the link's modules in an order that doesn't start.)
const { engineHandler } = await import("../src/link/server/router")
const { botsNow, factsOf, listedProjects, needsYou, projectOrder } = await import("../src/link/lists/rule")
const { archivedGroups, commit, keepList, keptList, listFollowers, listPlaceHeard, listRoute, noteListEvent, readList, workOut, LIST_FILE } = await import("../src/link/lists/sessions")

const vectors = JSON.parse(readFileSync(new URL("../../protocol/vectors/session-lists.json", import.meta.url), "utf8"))

test("a new conversation starts in the newest project, with sidebar order for ties, or in Chats", () => {
  const project = (name: string, updated: number) => ({ name, updated, asking: 0, working: 0 })
  expect(projectOrder({})).toEqual([])
  expect(projectOrder({
    "/old": project("First by name", 1),
    "/later": project("Project 10", 42),
    "/earlier": project("project 002", 42),
  })).toEqual(["/earlier", "/later", "/old"])
})

const placeOf = (box: any) => ({
  isChatsFolder: (folder: string) => folder === box.chats,
  botsFolder: box.bots,
  botSessions: new Set<string>(box.botSessions),
  filed: box.filed,
  projects: box.projects,
  running: new Set<string>(box.running),
  asked: new Map<string, number>(Object.entries(box.asked)),
  away: new Set<string>(box.away),
  asks: new Map<string, any>(Object.entries(box.asks ?? {})),
  botOf: new Map<string, string>(Object.entries(box.botOf ?? {})),
  thinking: new Set<string>(box.thinking ?? []),
  stopped: new Map<string, any>(Object.entries(box.stopped ?? {})),
  judging: () => false,
  retrying: new Map<string, any>(Object.entries(box.retrying ?? {})),
})

test.each(vectors.cases.map((each: any) => [each.name, each]))("%s", (_, each: any) => {
  const place = placeOf(each.box)
  const sessions = Object.fromEntries(each.sessions.map((session: any) => [session.id, factsOf(session, place)]))
  expect(archivedGroups(sessions, each.expect.projects)).toEqual(each.expect.archivedGroups)
  expect(sessions).toEqual(each.expect.sessions)
  expect(listedProjects(sessions, place)).toEqual(each.expect.projects)
  expect(needsYou(sessions, place.asks)).toEqual(each.expect.needsYou)
  expect(botsNow(sessions)).toEqual(each.expect.bots)
})

// What a client draws from the facts alone, as a client not in Swift would: the vectors' sidebar
// follows from the box's answer.
test("the sidebar the vectors expect follows from the facts alone", () => {
  for (const each of vectors.cases.filter((one: any) => one.expect.sidebar)) {
    const facts: Record<string, any> = each.expect.sessions
    const projects: Record<string, any> = each.expect.projects
    const shown = Object.entries(facts).filter(([, one]) => (one.kind === "project" || one.kind === "chat") && !one.archived)
    const newest = (ids: [string, any][]) => ids.sort((a, b) => b[1].updated - a[1].updated).map(([id]) => id)
    const name = (folder: string) => projects[folder]?.name ?? folder.split("/").pop()!
    const finder = new Intl.Collator("en", { numeric: true, sensitivity: "base" })
    const groups = Object.keys(projects).map(directory => ({ directory, sessions: newest(shown.filter(([, one]) => one.kind === "project" && one.project === directory)) }))
    const top = (group: { sessions: string[] }) => (group.sessions[0] ? facts[group.sessions[0]].updated : 0)
    groups.sort((a, b) => top(b) - top(a) || finder.compare(name(a.directory), name(b.directory)))
    expect({
      pinned: newest(shown.filter(([, one]) => one.pinned)),
      chats: newest(shown.filter(([, one]) => one.kind === "chat")),
      projects: groups,
    }).toEqual(each.expect.sidebar)
  }
})

// A fake engine: its sessions and those at work, as `api/session` pages and `api/session/active`.
const home = process.env.KUVO_HOME!
const work = join(home, "lists-work")
const chats = join(home, "chats")
mkdirSync(join(home, "box"), { recursive: true })
mkdirSync(work, { recursive: true })
writeFileSync(join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: work, folders: [], name: "Work" }] }))
const sessions = new Map<string, any>()
const messages = new Map<string, any[]>()
const active = new Set<string>()
let holdActive: (() => Promise<void>) | undefined
const failedMessages = new Map<string, number>()
const add = (id: string, directory: string, updated: number, extra: Record<string, unknown> = {}) => sessions.set(id, { id, location: { directory }, time: { created: updated, updated }, ...extra })
let listReads = 0
// Each session read (`POST …/view`) and up to where; and how many times one was read alone.
const views: [string, number][] = []
let oneReads = 0
// Held: the list's next page waits until the test lets it go, as a slow engine's.
let held: Promise<void> | null = null
let heldAsked: () => void = () => {}
// Held: the next read of one session, until the test lets it go.
const holdOne = new Map<string, { asked: () => void; until: Promise<void> }>()
function holdRead(id: string): { asked: Promise<void>; letGo: () => void } {
  let letGo: () => void = () => {}
  let asked: () => void = () => {}
  const wasAsked = new Promise<void>(done => { asked = done })
  holdOne.set(id, { asked, until: new Promise<void>(done => { letGo = done }) })
  return { asked: wasAsked, letGo }
}
const engine = Bun.serve({
  port: 0,
  async fetch(request) {
    const url = new URL(request.url)
    if (url.pathname === "/api/session") {
      // (Reads of the whole list, page after page: counted once.)
      if (!url.searchParams.get("cursor")) listReads += 1
      // Two to a page, so the list is read page after page.
      const all = [...sessions.values()].sort((a, b) => b.time.updated - a.time.updated)
      const from = Number(url.searchParams.get("cursor") ?? 0)
      const limit = Number(url.searchParams.get("limit"))
      const page = all.slice(from, from + Math.min(limit, 2))
      // (As it was when asked.)
      if (held) { heldAsked(); await held }
      return Response.json({ data: page, cursor: { next: from + page.length < all.length ? String(from + page.length) : null } })
    }
    if (url.pathname === "/api/session/active") {
      const snapshot = Response.json({ data: Object.fromEntries([...active].map(id => [id, { type: "busy" }])) })
      await holdActive?.()
      return snapshot
    }
    // A session's messages, newest first, one page; and nothing it asks.
    const said = url.pathname.match(/^\/api\/session\/([^/]+)\/message$/)
    if (said) {
      const id = said[1]!
      const failures = failedMessages.get(id) ?? 0
      if (failures > 0) {
        failedMessages.set(id, failures - 1)
        return Response.json({}, { status: 503 })
      }
      return Response.json({ data: messages.get(id) ?? [], cursor: null })
    }
    if (/^\/api\/session\/[^/]+\/(permission|form)$/.test(url.pathname)) return Response.json({ data: [] })
    // Read up to where it ended (`time.viewed`), as the engine keeps the later.
    const viewed = url.pathname.match(/^\/api\/session\/([^/]+)\/view$/)
    if (viewed && request.method === "POST" && sessions.has(viewed[1]!)) {
      const { idle } = await request.json() as { idle: number }
      const time = sessions.get(viewed[1]!).time
      views.push([viewed[1]!, idle])
      time.viewed = Math.max(time.viewed ?? 0, idle)
      return Response.json({})
    }
    const one = url.pathname.match(/^\/api\/session\/([^/]+)$/)
    if (one) oneReads += 1
    // (As it was when asked: a held read answers late with what was there then.)
    const answer = one && sessions.has(one[1]!) ? Response.json({ data: structuredClone(sessions.get(one[1]!)) }) : null
    const hold = one ? holdOne.get(one[1]!) : undefined
    if (hold) { holdOne.delete(one![1]!); hold.asked(); await hold.until }
    if (answer) return answer
    return Response.json({ message: "Not here." }, { status: 404 })
  },
})
const target = { port: engine.port, password: "pw" } as any
afterAll(() => {
  engine.stop(true)
  for (const file of [join(home, "box", "projects.json"), LIST_FILE]) rmSync(file, { force: true })
})

const handler = engineHandler(() => target, { name: "test" }, () => { throw new Error("none") })
async function ask(path: string): Promise<{ status: number; lines: any[] }> {
  const stop = new AbortController()
  let status = 0
  let text = ""
  for await (const piece of handler({ method: "GET", path, headers: {}, fromApp: true }, undefined, stop.signal)) {
    if ("head" in piece) status = piece.head.status
    if ("body" in piece) text += Buffer.from(piece.body).toString()
    // (A stream's first line is what's asked of it here.)
    if (path.includes("/events") && text.includes("\n")) { stop.abort(); break }
  }
  return { status, lines: text.trim().split("\n").filter(Boolean).map(line => JSON.parse(line)) }
}

test("a route answered in a process of its own reads the engine itself, and resumes from the list kept", async () => {
  add("ses_a", work, 100)
  add("ses_chat", chats, 50, { metadata: { kuvo: { chat: chats } } })
  // Three pages of two, each as far as the engine says it goes on.
  for (const n of [1, 2, 3]) add(`ses_page${n}`, work, 10 + n, { metadata: { kuvo: { archived: true } } })
  const { status, lines } = await ask("kuvo/sessions/list")
  expect(status).toBe(200)
  expect(lines[0].full).toBe(true)
  expect(Object.keys(lines[0].sessions).sort()).toEqual(["ses_a", "ses_chat", "ses_page1", "ses_page2", "ses_page3"])
  expect(lines[0].sessions.ses_a).toEqual({ kind: "project", project: work, directory: work, created: 100, updated: 100 })
  expect(lines[0].sessions.ses_chat.kind).toBe("chat")
  expect(lines[0].projects).toEqual({ [work]: { name: "Work", updated: 100, asking: 0, working: 0, shown: ["ses_a"], total: 1 } })
  expect(lines[0].startProject).toBe(work)
  expect(lines[0].projectOrder).toEqual([work])
  expect(lines[0].needsYou).toEqual([])
  // Kept for the next read, whichever process makes it: nothing changed since.
  const again = (await ask(`kuvo/sessions/list?since=${lines[0].at}`)).lines[0]
  expect(again).toEqual({ at: lines[0].at, sessions: {}, projects: lines[0].projects, projectOrder: [work], startProject: work, needsYou: [], archivedGroups: lines[0].archivedGroups })
  for (const n of [1, 2, 3]) sessions.delete(`ses_page${n}`)
})

test("just started, it reads its places before it answers, rather than say there's nothing", async () => {
  // (The very first time: no list saved yet.)
  rmSync(LIST_FILE, { force: true })
  const stop = keepList()
  try {
    const { status, lines } = await ask("kuvo/sessions/list/events")
    expect(status).toBe(200)
    expect(Object.keys(lines[0].sessions).sort()).toEqual(["ses_a", "ses_chat"])
  } finally {
    stop()
  }
})

test("just started with its list saved, it answers that at once, said to be as last read, and the engine's follows", async () => {
  // (Saved as the last one's process stopped: ses_a and ses_chat.)
  expect(Object.keys(readList()!.sessions).sort()).toEqual(["ses_a", "ses_chat"])
  add("ses_new", work, 700)
  let letGo: () => void = () => {}
  held = new Promise(done => { letGo = done })
  const asked = new Promise<void>(done => { heldAsked = done })
  const stop = keepList()
  const abort = new AbortController()
  const lines: any[] = []
  const said: Array<() => void> = []
  const line = (n: number) => new Promise<any>(done => { if (lines.length > n) done(lines[n]); else said[n] = () => done(lines[n]) })
  let text = ""
  const reading = (async () => {
    for await (const piece of handler({ method: "GET", path: "kuvo/sessions/list/events", headers: {}, fromApp: true }, undefined, abort.signal)) {
      if (!("body" in piece)) continue
      text += Buffer.from(piece.body).toString()
      while (text.includes("\n")) {
        const at = text.indexOf("\n")
        lines.push(JSON.parse(text.slice(0, at)))
        text = text.slice(at + 1)
        said[lines.length - 1]?.()
      }
    }
  })()
  try {
    // Its engine's sessions are being read: the saved list comes first, as last read.
    await asked
    const first = await line(0)
    expect(Object.keys(first.sessions).sort()).toEqual(["ses_a", "ses_chat"])
    expect(first.pending).toContain("engine")
    // Read: the list as the engine has it, on the same stream, no longer as last read.
    held = null
    letGo()
    const live = await line(1)
    expect(Object.keys(live.sessions)).toContain("ses_new")
    expect(live.pending).toBeUndefined()
  } finally {
    held = null
    letGo()
    abort.abort()
    await reading.catch(() => {})
    sessions.delete("ses_new")
    stop()
  }
})

test("a session deleted while its place is read whole stays gone", async () => {
  const stop = keepList()
  try {
    await listPlaceHeard("engine", target)
    add("ses_short", work, 600)
    let letGo: () => void = () => {}
    held = new Promise(done => { letGo = done })
    const asked = new Promise<void>(done => { heldAsked = done })
    const reading = listPlaceHeard("engine", target)
    // (The page it's reading has it; the event says it's gone meanwhile.)
    await asked
    sessions.delete("ses_short")
    await noteListEvent("engine", target, { type: "session.deleted", data: { sessionID: "ses_short" } })
    held = null
    letGo()
    await reading
    expect(Object.keys((await ask("kuvo/sessions/list")).lines[0].sessions)).not.toContain("ses_short")
  } finally {
    held = null
    stop()
  }
})

test("a read for an earlier event that answers after the session's deleted doesn't bring it back", async () => {
  const stop = keepList()
  try {
    add("ses_brief", work, 610)
    await listPlaceHeard("engine", target)
    const read = holdRead("ses_brief")
    const updated = noteListEvent("engine", target, { type: "session.updated", data: { sessionID: "ses_brief" } })
    await read.asked
    sessions.delete("ses_brief")
    await noteListEvent("engine", target, { type: "session.deleted", data: { sessionID: "ses_brief" } })
    read.letGo()
    await updated
    workOut()
    expect(Object.keys(keptList()!.sessions)).not.toContain("ses_brief")
  } finally {
    sessions.delete("ses_brief")
    stop()
  }
})

test("two reads of one session that answer out of order leave it as the later says", async () => {
  const stop = keepList()
  try {
    add("ses_twice", work, 620, { title: "First" })
    await listPlaceHeard("engine", target)
    const read = holdRead("ses_twice")
    const earlier = noteListEvent("engine", target, { type: "session.updated", data: { sessionID: "ses_twice" } })
    await read.asked
    sessions.get("ses_twice").title = "Second"
    await noteListEvent("engine", target, { type: "session.renamed", data: { sessionID: "ses_twice" } })
    read.letGo()
    await earlier
    workOut()
    expect(keptList()!.sessions.ses_twice!.facts.title).toBe("Second")
  } finally {
    sessions.delete("ses_twice")
    stop()
  }
})

test("what a session streams while a read for its pin is out doesn't undo that read", async () => {
  const stop = keepList()
  try {
    add("ses_streams", work, 630)
    await listPlaceHeard("engine", target)
    sessions.get("ses_streams").metadata = { "kuvo.pinned": true }
    const read = holdRead("ses_streams")
    const pinned = noteListEvent("engine", target, { type: "session.metadata.updated", data: { sessionID: "ses_streams" } })
    await read.asked
    await noteListEvent("engine", target, { type: "session.text.delta", data: { sessionID: "ses_streams" } })
    read.letGo()
    await pinned
    workOut()
    expect(keptList()!.sessions.ses_streams!.facts.pinned).toBe(true)
  } finally {
    sessions.delete("ses_streams")
    stop()
  }
})

test("kept as events come: a client resumes from where it was, and one from before the box restarted reads it all", async () => {
  const stop = keepList()
  try {
    active.add("ses_a")
    await listPlaceHeard("engine", target)
    const first = (await ask("kuvo/sessions/list")).lines[0]
    expect(first.full).toBe(true)
    expect(Object.keys(first.sessions).sort()).toEqual(["ses_a", "ses_chat"])
    expect(first.sessions.ses_a.running).toBe(true)

    // A session made, one renamed and pinned, one's run ending, one deleted: each as its event comes.
    add("ses_b", work, 300)
    await noteListEvent("engine", target, { type: "session.created", data: { sessionID: "ses_b" } })
    await noteListEvent("engine", target, { type: "session.execution.succeeded", data: { sessionID: "ses_a" } })
    sessions.get("ses_chat").title = "Ideas"
    sessions.get("ses_chat").metadata["kuvo.pinned"] = true
    await noteListEvent("engine", target, { type: "session.metadata.updated", data: { sessionID: "ses_chat" } })
    const between = (await ask(`kuvo/sessions/list?since=${first.at}`)).lines[0]
    expect(between.full).toBeUndefined()
    expect(Object.keys(between.sessions).sort()).toEqual(["ses_a", "ses_b", "ses_chat"])
    expect(between.sessions.ses_a.running).toBeUndefined()
    expect(between.sessions.ses_chat).toMatchObject({ kind: "chat", title: "Ideas", pinned: true })
    expect(between.projects[work].updated).toBe(300)

    sessions.delete("ses_b")
    await noteListEvent("engine", target, { type: "session.deleted", data: { sessionID: "ses_b" } })
    workOut()
    const after = (await ask(`kuvo/sessions/list/events?since=${between.at}`)).lines[0]
    expect(after.sessions).toEqual({})
    expect(after.gone).toEqual(["ses_b"])
    expect(after.projects[work].updated).toBe(100)
    // Nothing changed since: nothing but where it's at.
    const same = (await ask(`kuvo/sessions/list?since=${after.at}`)).lines[0]
    expect(same).toEqual({ at: after.at, sessions: {}, projects: after.projects, projectOrder: [work], startProject: work, needsYou: [], archivedGroups: after.archivedGroups })

    // A cursor of another list's (its file went), or from ahead of it: all of it, said so.
    for (const since of ["zz-1.3", `${after.at.split(".")[0]}.999`, "nonsense"]) {
      const whole = (await ask(`kuvo/sessions/list?since=${since}`)).lines[0]
      expect(whole.full).toBe(true)
      expect(Object.keys(whole.sessions).sort()).toEqual(["ses_a", "ses_chat"])
      expect(whole.gone).toBeUndefined()
    }

    // An event it can't read the session of (gone meanwhile): dropped.
    await noteListEvent("engine", target, { type: "session.renamed", data: { sessionID: "ses_nowhere" } })
    expect((await ask(`kuvo/sessions/list?since=${after.at}`)).lines[0].sessions).toEqual({})
    // Kept for a route answered in a process of its own: written a moment after a change, and
    // as it stops at the latest.
    stop()
    expect(JSON.parse(readFileSync(LIST_FILE, "utf8")).seq).toBe(Number(after.at.split(".")[1]))
  } finally {
    stop()
  }
})

test("its projects changing changes what's filed where, with no engine read", async () => {
  const stop = keepList()
  try {
    const tree = join(home, "lists-tree")
    add("ses_tree", tree, 400)
    await listPlaceHeard("engine", target)
    const before = (await ask("kuvo/sessions/list")).lines[0]
    expect(before.sessions.ses_tree.project).toBe(tree)
    const reads = listReads
    // (Written as another process would: the list hears it from the system.)
    const heard = new Promise<void>(done => {
      const look = () => { if (keptList()?.sessions.ses_tree?.facts.project === work) { listFollowers.delete(look); done() } }
      listFollowers.add(look)
    })
    writeFileSync(join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: work, folders: [tree], name: "Work" }] }))
    await heard
    const after = (await ask(`kuvo/sessions/list?since=${before.at}`)).lines[0]
    expect(Object.keys(after.sessions)).toEqual(["ses_tree"])
    expect(after.sessions.ses_tree.project).toBe(work)
    expect(listReads).toBe(reads)
  } finally {
    stop()
    sessions.delete("ses_tree")
    writeFileSync(join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: work, folders: [], name: "Work" }] }))
  }
})

test("an event while its place is first read whole says nothing's gone: the list waits for the read", async () => {
  rmSync(LIST_FILE, { force: true })
  const stop = keepList()
  try {
    let letGo: () => void = () => {}
    held = new Promise(done => { letGo = done })
    const asked = new Promise<void>(done => { heldAsked = done })
    const reading = listPlaceHeard("engine", target)
    await asked
    // (The reviewer's case: a session changing during the first read's paging.)
    await noteListEvent("engine", target, { type: "session.updated", data: { sessionID: "ses_a" } })
    workOut()
    expect(readList()).toBeNull()
    held = null
    letGo()
    await reading
    const kept = readList()!
    expect(Object.keys(kept.sessions).sort()).toEqual(["ses_a", "ses_chat"])
    expect(kept.gone).toEqual({})
  } finally {
    held = null
    stop()
  }
})

test("a place asked to be read whole again while it's read is read once more after, never twice at once", async () => {
  const stop = keepList()
  try {
    await listPlaceHeard("engine", target)
    let letGo: () => void = () => {}
    held = new Promise(done => { letGo = done })
    const asked = new Promise<void>(done => { heldAsked = done })
    const reads = listReads
    const first = listPlaceHeard("engine", target)
    await asked
    add("ses_late", work, 700)
    const second = listPlaceHeard("engine", target)
    held = null
    letGo()
    await Promise.all([first, second])
    // (The one under way, then the one asked for meanwhile, after it.)
    expect(listReads - reads).toBe(2)
    expect(Object.keys(keptList()!.sessions)).toContain("ses_late")
  } finally {
    held = null
    sessions.delete("ses_late")
    stop()
  }
})

test("a coding agents' bridge that doesn't answer holds nothing up: the engine's are listed, the bridge's as last read", async () => {
  const silent = { port: 9, password: "pw" } as any
  const pieces: any[] = []
  const context = { request: { method: "GET", path: "kuvo/sessions/list", headers: {} }, signal: undefined, readEngine: () => target, bridge: () => silent,
    json: (status: number, value: unknown) => [{ head: { status, headers: {} } }, { body: Buffer.from(JSON.stringify(value)) }] } as any
  for await (const piece of listRoute(context, "kuvo/sessions/list".match(/^kuvo\/sessions\/list(\/events)?$/)!)) pieces.push(piece)
  expect(pieces[0].head.status).toBe(200)
  const answer = JSON.parse(Buffer.from(pieces[1].body).toString())
  expect(answer.pending).toEqual(["bridge"])
  expect(Object.keys(answer.sessions)).toContain("ses_a")
  // An engine that isn't there: no list rather than one without its sessions.
  const none: any[] = []
  for await (const piece of listRoute({ ...context, readEngine: () => { throw new Error("no engine") } }, "kuvo/sessions/list".match(/^kuvo\/sessions\/list(\/events)?$/)!)) none.push(piece)
  expect((none[0] as any).head.status).toBe(503)
})

test("a stream tells each change as a line of what changed", async () => {
  const stop = keepList()
  const reading = new AbortController()
  try {
    await listPlaceHeard("engine", target)
    const stream = handler({ method: "GET", path: "kuvo/sessions/list/events", headers: {}, fromApp: true }, undefined, reading.signal)
    const line = async () => {
      for (;;) {
        const { value } = await stream.next()
        if (value && "body" in value) return JSON.parse(Buffer.from(value.body).toString())
      }
    }
    const first = await line()
    expect(first.full).toBe(true)
    add("ses_new", work, 500, { title: "Fresh" })
    const next = line()
    await noteListEvent("engine", target, { type: "session.created", data: { sessionID: "ses_new" } })
    const changed = await next
    expect(changed.full).toBeUndefined()
    expect(Object.keys(changed.sessions)).toEqual(["ses_new"])
    expect(changed.sessions.ses_new).toMatchObject({ kind: "project", project: work, title: "Fresh" })
    reading.abort()
    await stream.return(undefined)
  } finally {
    stop()
    sessions.delete("ses_new")
  }
})

test("a worktree an older Mac removed, handed over, files its sessions under its project", async () => {
  const gone = join(home, "lists-gone-tree")
  add("ses_gone_tree", gone, 450)
  try {
    expect((await ask("kuvo/sessions/list")).lines[0].sessions.ses_gone_tree.project).toBe(gone)
    const pieces: any[] = []
    for await (const piece of handler({ method: "POST", path: "kuvo/worktrees/record", headers: {}, body: Buffer.from(JSON.stringify({ removed: true, folder: gone, project: work })), fromApp: true }, undefined)) pieces.push(piece)
    expect(pieces[0].head.status).toBe(200)
    expect((await ask("kuvo/sessions/list")).lines[0].sessions.ses_gone_tree.project).toBe(work)
    // More worktrees removed here since than the 200 it keeps of them: the one handed over stays.
    const { updateWorktrees } = await import("../src/link/areas/worktree-records")
    for (let n = 0; n < 201; n++) updateWorktrees(each => each, { project: work, root: join(home, `lists-later-${n}`), folder: join(home, `lists-later-${n}`), branch: "b" } as any)
    expect((await ask("kuvo/sessions/list")).lines[0].sessions.ses_gone_tree.project).toBe(work)
  } finally {
    sessions.delete("ses_gone_tree")
    rmSync(join(home, "box", "worktrees.json"), { force: true })
  }
})

// A Mac's change over SSH (`box route`, a process of its own): the long-running process keeping the
// list sees the box's files change, and lists it, though nothing in it changed them.
test("a change another process makes to the box's projects reaches the kept list at once", async () => {
  const gone = join(home, "lists-elsewhere-tree")
  add("ses_elsewhere", gone, 460)
  const stop = keepList()
  try {
    await listPlaceHeard("engine", target)
    expect(keptList()!.sessions.ses_elsewhere!.facts.project).toBe(gone)
    const listed = new Promise<void>(done => {
      const look = () => { if (keptList()?.sessions.ses_elsewhere?.facts.project === work) { listFollowers.delete(look); done() } }
      listFollowers.add(look)
    })
    const route = Bun.spawn([process.execPath, join(import.meta.dir, "../src/link/main.ts"), "box", "route"], { env: process.env, stdin: "pipe", stdout: "pipe", stderr: "ignore" })
    route.stdin.write(JSON.stringify({ method: "POST", path: "kuvo/worktrees/record", body: { removed: true, folder: gone, project: work } }))
    await route.stdin.end()
    expect(JSON.parse(await new Response(route.stdout).text()).status).toBe(200)
    await route.exited
    await listed
  } finally {
    stop()
    sessions.delete("ses_elsewhere")
    rmSync(join(home, "box", "worktrees.json"), { force: true })
  }
}, 20_000)

// A bot's conversation named by another process (a bot made over SSH, kuvo-wake): its folder's
// files watched, not what its agent works on in it.
test("a bot another process makes is listed as a bot's at once", async () => {
  const { BOTS } = await import("../src/link/home")
  const folder = join(BOTS, "bot_listtest")
  add("ses_botty", work, 470)
  mkdirSync(folder, { recursive: true })
  const stop = keepList()
  try {
    await listPlaceHeard("engine", target)
    expect(keptList()!.sessions.ses_botty!.facts.kind).toBe("project")
    const listed = new Promise<void>(done => {
      const look = () => { if (keptList()?.sessions.ses_botty?.facts.kind === "bot") { listFollowers.delete(look); done() } }
      listFollowers.add(look)
    })
    writeFileSync(join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_listtest", name: "Lister" }))
    writeFileSync(join(folder, "session"), "ses_botty")
    await listed
  } finally {
    stop()
    sessions.delete("ses_botty")
    rmSync(folder, { recursive: true, force: true })
  }
}, 20_000)

// The box's long-running process gone before it wrote its last changes (stopped, crashed): what
// takes the list over next (it started again, under the same process id too, or a route answering
// in its place) numbers on past them, and every client from before reads it all once, so nothing
// told and not written stays with it (a session made and deleted meanwhile, say), however often
// that happens before it's back.
test("a list taken over from a process that went is read whole by every client from before", async () => {
  mkdirSync(join(LIST_FILE, ".."), { recursive: true })
  const went = (owner: number, started?: string) => {
    const kept = JSON.parse(readFileSync(LIST_FILE, "utf8"))
    writeFileSync(LIST_FILE, JSON.stringify({ ...kept, owner, started }))
  }
  writeFileSync(LIST_FILE, JSON.stringify({ epoch: "kept", seq: 10, floor: 0, sessions: {}, gone: {}, projects: {}, owner: 999999 }))
  try {
    // Another process that went: one from before, and one from what it told unwritten.
    let stop = keepList()
    await listPlaceHeard("engine", target)
    for (const since of ["kept.10", "kept.11"]) expect((await ask(`kuvo/sessions/list?since=${since}`)).lines[0].full).toBe(true)
    const first = (await ask("kuvo/sessions/list")).lines[0].at
    expect((await ask(`kuvo/sessions/list?since=${first}`)).lines[0].full).toBeUndefined()
    stop()
    // This process id again, started anew (the file says another start): taken over too.
    went(process.pid, "an-earlier-start")
    stop = keepList()
    await listPlaceHeard("engine", target)
    expect((await ask(`kuvo/sessions/list?since=${first}`)).lines[0].full).toBe(true)
    const second = (await ask("kuvo/sessions/list")).lines[0].at
    stop()
    // A route answering in place of one that went (no process keeps it now): the same.
    went(999999)
    expect((await ask(`kuvo/sessions/list?since=${second}`)).lines[0].full).toBe(true)
  } finally {
    rmSync(LIST_FILE, { force: true })
  }
})

test("a list kept by the same start, or by routes alone, resumes: no whole read each time", async () => {
  rmSync(LIST_FILE, { force: true })
  try {
    // Routes alone (no process keeps it): each answer goes on from the last.
    const first = (await ask("kuvo/sessions/list")).lines[0].at
    expect((await ask(`kuvo/sessions/list?since=${first}`)).lines[0].full).toBeUndefined()
    // The long-running process stopped and started again within one start (written as it stopped).
    let stop = keepList()
    await listPlaceHeard("engine", target)
    const kept = (await ask("kuvo/sessions/list")).lines[0].at
    stop()
    stop = keepList()
    await listPlaceHeard("engine", target)
    expect((await ask(`kuvo/sessions/list?since=${kept}`)).lines[0].full).toBeUndefined()
    stop()
  } finally {
    rmSync(LIST_FILE, { force: true })
  }
})

test("a route never writes over the list the long-running process keeps, nor takes away its mark", async () => {
  // (A process named as the long-running one is: it started as the route read the engine.)
  const link = Bun.spawn(["/bin/sh", "-c", "printf ready; while :; do sleep 1; done", "kuvo-link"], { stdin: "ignore", stdout: "pipe", stderr: "ignore" })
  try {
    // Wait for exec before relying on the shell's command line to identify the owner.
    const ready = link.stdout.getReader()
    try { expect((await ready.read()).done).toBe(false) } finally { ready.releaseLock() }
    const kept = { epoch: "kept", seq: 3, floor: 0, sessions: {}, gone: {}, projects: {}, owner: link.pid }
    mkdirSync(join(LIST_FILE, ".."), { recursive: true })
    writeFileSync(LIST_FILE, JSON.stringify(kept))
    const answered = commit({ sessions: { ses_a: { kind: "project", project: work, directory: work, created: 1, updated: 1 } }, projects: {} })
    expect(answered).toEqual(kept)
    expect(readList()).toEqual(kept)
  } finally {
    link.kill()
    await link.exited
    rmSync(LIST_FILE, { force: true })
  }
})

test("only GET", async () => {
  expect((await ask("kuvo/sessions/list")).status).toBe(200)
  const pieces: any[] = []
  for await (const piece of handler({ method: "POST", path: "kuvo/sessions/list", headers: {}, fromApp: true }, undefined)) pieces.push(piece)
  expect(pieces[0]!.head!.status).toBe(405)
})

// What waits on you (#711): a session asks, and the list says it under "Needs you" with what it
// asks, its project's sums count it, its bot says it's waiting, and `kuvo/waiting` counts each
// area; answered, all of it goes. Every client draws its badges from these, counting nothing.
test("a session that asks is in Needs you, its project's sums, its bot's now and what waits on you, until it's answered", async () => {
  const { noteAskEvent, forgetAsks } = await import("../src/link/glance/asked")
  const { BOTS } = await import("../src/link/home")
  const { WORK_QUEUE } = await import("../src/link/work-queue/store")
  const folder = join(BOTS, "bot_waits")
  // (A project of its own: other tests' sessions are in `work`.)
  const mine = join(home, "lists-waits")
  mkdirSync(folder, { recursive: true })
  writeFileSync(join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_waits", name: "Mira" }))
  writeFileSync(join(folder, "session"), "ses_mira")
  add("ses_mira", folder, 600)
  add("ses_asks", mine, 610, { title: "Rate limits" })
  add("ses_works", mine, 620, { metadata: { kuvo: { startedBy: "ses_mira", by: { bot: "bot_waits", name: "Mira" }, depth: 1 } } })
  add("ses_works_sub", mine, 625, { parentID: "ses_works" })
  active.add("ses_works")
  active.add("ses_asks")
  // An item of the Work Queue the session that will ask is working on.
  mkdirSync(join(WORK_QUEUE, "queues"), { recursive: true })
  const queueFile = join(WORK_QUEUE, "queues", "ses_asks.json")
  writeFileSync(queueFile, JSON.stringify({ owner: "ses_asks", items: [{ id: "wqi_aaaaaaaaaaaaaaaaaaaa", number: 7, title: "Limits", status: "todo", order: 1, labels: [], sessions: [{ session: "ses_asks", since: 1 }], created: { at: 1, by: { user: true } } }] }))
  const stop = keepList()
  const listed = (when: () => boolean) => new Promise<void>(done => {
    const look = () => { if (when()) { listFollowers.delete(look); done() } }
    listFollowers.add(look)
    look()
  })
  try {
    await listPlaceHeard("engine", target)
    const followersBefore = listFollowers.size
    const before = (await ask("kuvo/sessions/list")).lines[0]
    expect(before.needsYou).toEqual([])
    expect(before.projects[mine]).toMatchObject({ asking: 0, working: 2 })
    expect(before.sessions.ses_works.bot).toBe("bot_waits")
    expect((await ask("kuvo/waiting")).lines[0]).toEqual({ sessions: 0, bots: 0, queue: 0 })
    expect((await ask("kuvo/bots")).lines[0].bots.find((bot: any) => bot.id === "bot_waits").now).toEqual({ working: 1, asking: 0, state: "working", since: expect.any(Number) })

    // A client following what waits, and the bots: each says a line more as it changes.
    const follow = (path: string) => {
      const stop = new AbortController()
      const stream = handler({ method: "GET", path, headers: {}, fromApp: true }, undefined, stop.signal)
      let text = ""
      const lines: any[] = []
      // The next line that says `wanted` (lines before it are read past).
      const until = async (wanted: (line: any) => boolean) => {
        for (;;) {
          const found = lines.find(wanted)
          if (found) return found
          const { value, done } = await stream.next()
          if (done) throw new Error(`${path} ended`)
          if (!("body" in value)) continue
          text += Buffer.from(value.body).toString()
          const parts = text.split("\n")
          text = parts.pop()!
          lines.push(...parts.filter(Boolean).map(part => JSON.parse(part)))
        }
      }
      return { until, end: async () => { stop.abort(); await stream.return(undefined) } }
    }
    const waits = follow("kuvo/waiting/events"), botsFollowed = follow("kuvo/bots/events")
    await waits.until(line => line.sessions === 0)
    await botsFollowed.until(line => line.bots.some((bot: any) => bot.id === "bot_waits" && bot.now.asking === 0))

    // A project's session asks to run a command; then the bot's session's subagent asks to edit files.
    noteAskEvent({ type: "permission.asked", data: { id: "per_1", sessionID: "ses_asks", action: "bash", resources: ["git push"] } }, "engine", 1000)
    await listed(() => keptList()?.needsYou?.length === 1)
    noteAskEvent({ type: "permission.asked", data: { id: "per_2", sessionID: "ses_works_sub", action: "edit", resources: [join(mine, "a.ts"), join(mine, "b.ts")] } }, "engine", 2000)
    await listed(() => keptList()?.needsYou?.length === 2)

    const asking = (await ask(`kuvo/sessions/list?since=${before.at}`)).lines[0]
    // The one that asked last first; a subagent's ask is its conversation's.
    expect(asking.needsYou).toEqual([
      { session: "ses_works", asks: { kind: "permission", summary: "asks to edit 2 files" } },
      { session: "ses_asks", asks: { kind: "permission", summary: "asks to run a command" } },
    ])
    // (One that asks isn't counted at work.)
    expect(asking.projects[mine]).toMatchObject({ asking: 2, working: 0 })
    expect((await ask("kuvo/waiting/events")).lines[0]).toEqual({ sessions: 2, bots: 1, queue: 1 })
    expect(await waits.until(line => line.sessions === 2)).toEqual({ sessions: 2, bots: 1, queue: 1 })
    await botsFollowed.until(line => line.bots.some((bot: any) => bot.id === "bot_waits" && bot.now.asking === 1))
    // A session here working for a bot that lives on another computer is that computer's to count.
    add("ses_far", mine, 630, { metadata: { kuvo: { startedBy: "ses_x", by: { bot: "bot_elsewhere", name: "Far" }, depth: 1 } } })
    await noteListEvent("engine", target, { type: "session.created", data: { sessionID: "ses_far" } })
    noteAskEvent({ type: "permission.asked", data: { id: "per_3", sessionID: "ses_far", action: "x".repeat(300) } }, "engine", 3000)
    await listed(() => keptList()?.needsYou?.length === 3)
    expect((await ask("kuvo/waiting")).lines[0]).toEqual({ sessions: 3, bots: 1, queue: 1 })
    // (What it asks is a few words, whatever a tool names its action.)
    expect(keptList()!.needsYou![0]!.asks.summary.length).toBeLessThan(80)
    noteAskEvent({ type: "permission.replied", data: { requestID: "per_3", sessionID: "ses_far" } }, "engine")
    await listed(() => keptList()?.needsYou?.length === 2)
    expect((await ask("kuvo/bots/events")).lines[0].bots.find((bot: any) => bot.id === "bot_waits").now).toEqual({ working: 0, asking: 1, state: "needsYou", since: expect.any(Number) })

    // Answered: nothing waits, and the bot's session is at work again.
    noteAskEvent({ type: "permission.replied", data: { requestID: "per_1", sessionID: "ses_asks" } }, "engine")
    noteAskEvent({ type: "permission.replied", data: { requestID: "per_2", sessionID: "ses_works_sub" } }, "engine")
    await listed(() => keptList()?.needsYou?.length === 0)
    expect((await ask("kuvo/waiting")).lines[0]).toEqual({ sessions: 0, bots: 0, queue: 0 })
    expect((await ask("kuvo/bots")).lines[0].bots.find((bot: any) => bot.id === "bot_waits").now).toEqual({ working: 1, asking: 0, state: "working", since: expect.any(Number) })
    await waits.until(line => line.sessions === 0 && line.queue === 0)
    await waits.end()
    await botsFollowed.end()
    expect(listFollowers.size).toBe(followersBefore)

    // Its work over: the bot says when its sessions last changed.
    active.delete("ses_works")
    await noteListEvent("engine", target, { type: "session.execution.succeeded", data: { sessionID: "ses_works" } })
    await listed(() => !keptList()?.sessions.ses_works?.facts.running)
    expect((await ask("kuvo/bots")).lines[0].bots.find((bot: any) => bot.id === "bot_waits").now).toEqual({ working: 0, asking: 0, finished: 620, state: "idle", since: expect.any(Number) })

    // Its turn over, but a command it started still runs (Background work): it's at work still, in
    // the list, its project's count and its bot's, as every client's At work says; until it ends.
    const { WORK_BACKGROUND, backgroundFollowers } = await import("../src/link/work/background")
    const leftRunning = (commands: unknown[]) => {
      mkdirSync(join(WORK_BACKGROUND, ".."), { recursive: true })
      writeFileSync(WORK_BACKGROUND, JSON.stringify({ engine: { sessions: commands.length ? { ses_works: { commands, subagents: [], tasks: [] } } : {}, folders: [] } }))
      for (const tell of backgroundFollowers) tell()
    }
    try {
      leftRunning([{ id: "job_1", command: "npm test", started: Date.now() }])
      await listed(() => keptList()?.sessions.ses_works?.facts.running === true)
      expect(active.has("ses_works")).toBe(false)
      expect((await ask("kuvo/bots")).lines[0].bots.find((bot: any) => bot.id === "bot_waits").now).toEqual({ working: 1, asking: 0, state: "working", since: expect.any(Number) })
      expect((await ask("kuvo/sessions/list")).lines[0].sessions.ses_works.running).toBe(true)
      // One running past six hours no longer counts, though it may still run.
      leftRunning([{ id: "job_1", command: "npm run dev", started: Date.now() - 7 * 3_600_000 }])
      await listed(() => !keptList()?.sessions.ses_works?.facts.running)
      leftRunning([{ id: "job_2", command: "npm test", started: Date.now() }])
      await listed(() => keptList()?.sessions.ses_works?.facts.running === true)
      leftRunning([])
      await listed(() => !keptList()?.sessions.ses_works?.facts.running)
      expect((await ask("kuvo/bots")).lines[0].bots.find((bot: any) => bot.id === "bot_waits").now).toMatchObject({ working: 0, asking: 0 })
    } finally {
      rmSync(WORK_BACKGROUND, { force: true })
    }
  } finally {
    stop()
    forgetAsks()
    for (const id of ["ses_mira", "ses_asks", "ses_works", "ses_works_sub", "ses_far"]) { sessions.delete(id); active.delete(id) }
    rmSync(folder, { recursive: true, force: true })
    rmSync(queueFile, { force: true })
  }
})

// The Work Queue's count of what waits on you (#729): one rule, the box's, so the badge and the
// Needs you tab agree. An open item counts when it's in Needs you, or when a session linked to it
// asks now (whatever its status still says); each once. Closed and deleted ones never count.
test("the queue's count is its open items in Needs you and those a linked session asks for, each once", async () => {
  const { noteAskEvent, forgetAsks } = await import("../src/link/glance/asked")
  const { WORK_QUEUE } = await import("../src/link/work-queue/store")
  const mine = join(home, "lists-needs")
  add("ses_needs", mine, 640, { title: "Needs" })
  mkdirSync(join(WORK_QUEUE, "queues"), { recursive: true })
  const queueFile = join(WORK_QUEUE, "queues", "ses_needs.json")
  const item = (n: number, status: string, extra: Record<string, unknown> = {}) =>
    ({ id: `wqi_needs${n}`.padEnd(24, "a"), number: 900 + n, title: `Item ${n}`, status, order: n, labels: [], sessions: [], created: { at: 1, by: { user: true } }, ...extra })
  const linked = { sessions: [{ session: "ses_needs", since: 1 }] }
  writeFileSync(queueFile, JSON.stringify({ owner: "ses_needs", items: [
    item(1, "needs-you"),
    item(2, "needs-you", linked),
    item(3, "in-progress", linked),
    item(4, "needs-you", { removed: { at: 2, by: { user: true } } }),
    item(5, "done", linked),
    item(6, "canceled", linked),
    item(7, "todo"),
  ] }))
  const stop = keepList()
  const listed = (when: () => boolean) => new Promise<void>(done => {
    const look = () => { if (when()) { listFollowers.delete(look); done() } }
    listFollowers.add(look)
    look()
  })
  try {
    await listPlaceHeard("engine", target)
    await noteListEvent("engine", target, { type: "session.created", data: { sessionID: "ses_needs" } })
    // Nothing asks: the two open items in Needs you.
    expect((await ask("kuvo/waiting")).lines[0]).toMatchObject({ queue: 2 })
    // Its session asks: the open one it's linked to in another status too; the one in Needs you it's
    // linked to still once.
    noteAskEvent({ type: "permission.asked", data: { id: "per_needs", sessionID: "ses_needs", action: "bash", resources: ["make"] } }, "engine", 4000)
    await listed(() => keptList()?.needsYou?.some(each => each.session === "ses_needs") === true)
    expect((await ask("kuvo/waiting")).lines[0]).toMatchObject({ queue: 3 })
    noteAskEvent({ type: "permission.replied", data: { requestID: "per_needs", sessionID: "ses_needs" } }, "engine")
    await listed(() => !keptList()?.needsYou?.some(each => each.session === "ses_needs"))
    expect((await ask("kuvo/waiting")).lines[0]).toMatchObject({ queue: 2 })
  } finally {
    stop()
    forgetAsks()
    sessions.delete("ses_needs")
    rmSync(queueFile, { force: true })
  }
})

// What a client's one badge of sessions counts (#718): every session that waits on you, as its rows
// show them. A bot's own conversation that asks (its bots' area lists it, not "Needs you"), and what
// the box keeps outside the engine's asks: what an agent asks while it browses, what a bot asks
// before it does it, and a goal a bot put to you. Each session once; answered, each goes.
test("what waits counts a bot's own conversation, a browser's ask, a bot's work ask and a proposed goal, each session once", async () => {
  const { noteAskEvent, forgetAsks, followAsks, lookAtOthers } = await import("../src/link/glance/asked")
  const { BOTS, RUN } = await import("../src/link/home")
  const { askWork, WORK_ASKS } = await import("../src/link/work/asks")
  const { GOALS_FILE } = await import("../src/link/work/goals")
  const { spawn } = await import("node:child_process")
  const folder = join(BOTS, "bot_asks"), other = join(BOTS, "bot_plans")
  const mine = join(home, "lists-cards")
  for (const [at, id, name, session] of [[folder, "bot_asks", "Mira", "ses_bot_asks"], [other, "bot_plans", "Juno", "ses_bot_plans"]] as const) {
    mkdirSync(at, { recursive: true })
    writeFileSync(join(at, "bot.json"), JSON.stringify({ kuvo: 1, id, name }))
    writeFileSync(join(at, "session"), session)
  }
  add("ses_bot_asks", folder, 700)
  add("ses_bot_plans", other, 705)
  add("ses_browses", mine, 710, { title: "Billing" })
  add("ses_makes", mine, 720, { title: "New site" })
  // (A browser that runs, as its asks are only a running one's: a process named as it is.)
  const browser = spawn(process.execPath, ["-e", "setTimeout(() => {}, 120000)", "kuvo-browser"], { stdio: "ignore" })
  const browserFiles = [join(RUN, "browser.pid"), join(RUN, "browser-asks.json")]
  const stopList = keepList(), stopAsks = followAsks()
  const listed = (when: () => boolean) => new Promise<void>(done => {
    const look = () => { if (when()) { listFollowers.delete(look); done() } }
    listFollowers.add(look)
    look()
  })
  const waiting = async () => (await ask("kuvo/waiting")).lines[0]
  const asked = (id: string) => keptList()?.sessions[id]?.facts.asked !== undefined
  try {
    await listPlaceHeard("engine", target)
    expect(await waiting()).toEqual({ sessions: 0, bots: 0, queue: 0 })

    // A bot asks a permission in its own conversation: a session that waits, and its bot too.
    noteAskEvent({ type: "permission.asked", data: { id: "per_bot", sessionID: "ses_bot_asks", action: "bash" } }, "engine", 1000)
    await listed(() => asked("ses_bot_asks"))
    expect(await waiting()).toEqual({ sessions: 1, bots: 1, queue: 0 })
    // ("Needs you" lists projects' and chats' sessions: a bot's is under its bot.)
    expect(keptList()!.needsYou).toEqual([])

    // An agent asks you to sign in while it browses.
    mkdirSync(RUN, { recursive: true })
    writeFileSync(browserFiles[0]!, String(browser.pid))
    writeFileSync(browserFiles[1]!, JSON.stringify({ asks: [{ id: "bask_1", kind: "signIn", host: "github.com", session: "ses_browses", username: true, password: true }] }))
    lookAtOthers()
    await listed(() => asked("ses_browses"))
    expect(keptList()!.needsYou).toEqual([{ session: "ses_browses", asks: { kind: "other", summary: "asks you to sign in to github.com" } }])
    expect(await waiting()).toEqual({ sessions: 2, bots: 1, queue: 0 })

    // A session asks before it makes a project (told as it's kept, in this process).
    const made = askWork({ session: "ses_makes", name: "New site" }, "makeProject", { name: "Landing page" })
    await listed(() => asked("ses_makes"))
    expect(keptList()!.needsYou![0]).toEqual({ session: "ses_makes", asks: { kind: "other", summary: "asks to make a project" } })
    expect(await waiting()).toEqual({ sessions: 3, bots: 1, queue: 0 })

    // A bot puts a goal to you: it waits in the bot's own conversation.
    mkdirSync(dirname(GOALS_FILE), { recursive: true })
    writeFileSync(GOALS_FILE, JSON.stringify({ goals: [{ id: "goal_plan", bot: "bot_plans", name: "Juno", session: "ses_bot_plans", goal: "Ship it", state: "proposed", proposed: Date.now(), projects: [], steps: [], checks: [], spend: [], public: [], waiting: [], paused: [] }] }))
    lookAtOthers()
    await listed(() => asked("ses_bot_plans"))
    expect(await waiting()).toEqual({ sessions: 4, bots: 2, queue: 0 })
    expect((await ask("kuvo/bots")).lines[0].bots.find((bot: any) => bot.id === "bot_plans").now).toEqual({ working: 0, asking: 1, state: "needsYou", since: expect.any(Number) })

    // The same session asking twice is one session: its permission too.
    noteAskEvent({ type: "permission.asked", data: { id: "per_plans", sessionID: "ses_bot_plans", action: "bash" } }, "engine", 2000)
    lookAtOthers()
    expect(await waiting()).toEqual({ sessions: 4, bots: 2, queue: 0 })
    noteAskEvent({ type: "permission.replied", data: { requestID: "per_plans", sessionID: "ses_bot_plans" } }, "engine")

    // Each answered or gone: nothing waits.
    noteAskEvent({ type: "permission.replied", data: { requestID: "per_bot", sessionID: "ses_bot_asks" } }, "engine")
    writeFileSync(browserFiles[1]!, JSON.stringify({ asks: [] }))
    writeFileSync(WORK_ASKS, JSON.stringify({ asks: [] }))
    writeFileSync(GOALS_FILE, JSON.stringify({ goals: [] }))
    expect(made.id).toMatch(/^wak_/)
    lookAtOthers()
    await listed(() => !["ses_bot_asks", "ses_browses", "ses_makes", "ses_bot_plans"].some(asked))
    expect(await waiting()).toEqual({ sessions: 0, bots: 0, queue: 0 })
  } finally {
    stopAsks()
    stopList()
    browser.kill()
    forgetAsks()
    for (const file of [...browserFiles, WORK_ASKS, GOALS_FILE]) rmSync(file, { force: true })
    for (const id of ["ses_bot_asks", "ses_bot_plans", "ses_browses", "ses_makes"]) sessions.delete(id)
    rmSync(folder, { recursive: true, force: true })
    rmSync(other, { recursive: true, force: true })
  }
})

test("a list kept from before Needs you is answered whole: none waiting yet, and each project's sums", async () => {
  const { answerFrom } = await import("../src/link/lists/sessions")
  const answer = answerFrom({ epoch: "old", seq: 3, floor: 0, sessions: { ses_old: { seq: 1, facts: { kind: "project", project: "/p", directory: "/p", created: 1, updated: 2, asked: 5 } } }, gone: {}, projects: { "/p": { updated: 2 } as any } }, null)
  expect(answer.projects).toEqual({ "/p": { updated: 2, asking: 0, working: 0 } })
  expect(answer.needsYou).toEqual([{ session: "ses_old", asks: { kind: "other", summary: "waits on you" } }])
})

test("what waits on you is read with GET only", async () => {
  const pieces: any[] = []
  for await (const piece of handler({ method: "POST", path: "kuvo/waiting", headers: {}, fromApp: true }, undefined)) pieces.push(piece)
  expect(pieces[0].head.status).toBe(405)
})

 test("archived groups include bots, omit away copies, and resolve ties by id", () => {
  const facts = (kind: string, updated: number, more = {}) => ({ kind, directory: "/p", updated, archived: true, ...more }) as any;
  expect(archivedGroups({ z: facts("project", 2), a: facts("project", 2), bot: facts("bot", 3), away: facts("chat", 4, { away: true }), hidden: facts("hidden", 5) }, { "/p": { name: "Project", asking: 0, working: 0 } })).toEqual([
    { directory: "/p", title: "Project", kind: "bot", sessions: ["bot"] },
    { directory: "/p", title: "Project", kind: "project", sessions: ["a", "z"] },
  ]);
});

test("archived groups retain the saved name of a removed project with only archived sessions", () => {
  const session = { kind: "project", directory: "/old", project: "/old", updated: 1, archived: true } as any;
  expect(archivedGroups({ archived: session }, {}, [{ directory: "/old", name: "Saved Name" }])).toEqual([
    { directory: "/old", title: "Saved Name", kind: "project", sessions: ["archived"] },
  ]);
});

// MARK: A bot's one state (`bots.state`)

const clock = jest as unknown as { advanceTimersByTime(ms: number): void }

/** A bot here, `id`, whose conversation is `session` (in its folder), with these files besides. */
async function aBot(id: string, session: string, files: Record<string, string> = {}): Promise<string> {
  const { BOTS } = await import("../src/link/home")
  const folder = join(BOTS, id)
  mkdirSync(folder, { recursive: true })
  writeFileSync(join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id, name: "Ada" }))
  writeFileSync(join(folder, "session"), session)
  for (const [name, text] of Object.entries(files)) {
    mkdirSync(dirname(join(folder, name)), { recursive: true })
    writeFileSync(join(folder, name), text)
  }
  return folder
}

const nowOfBot = async (id: string) => (await ask("kuvo/bots")).lines[0].bots.find((bot: any) => bot.id === id).now
const hearing = (session: string) => (type: string, data: Record<string, unknown> = {}) => noteListEvent("engine", target, { type, data: { sessionID: session, ...data } })

/** `path`'s stream followed: `until` reads on to the first line that says what's wanted of `pick`'s. */
function following(path: string, pick: (line: any) => any) {
  const stop = new AbortController()
  const stream = handler({ method: "GET", path, headers: {}, fromApp: true }, undefined, stop.signal)
  let text = ""
  const until = async (wanted: (picked: any) => boolean) => {
    for (;;) {
      const lines = text.split("\n")
      text = lines.pop()!
      for (const [index, line] of lines.entries()) {
        if (!line || !wanted(pick(JSON.parse(line)))) continue
        text = [...lines.slice(index + 1), text].join("\n")
        return pick(JSON.parse(line))
      }
      const { value, done } = await stream.next()
      if (done) throw new Error(`${path} ended`)
      if ("body" in value) text += Buffer.from(value.body).toString()
    }
  }
  return { until, end: async () => { stop.abort(); await stream.return(undefined) } }
}

async function post(path: string, body: Record<string, unknown> = {}): Promise<{ status: number; body: any }> {
  let status = 0
  let text = ""
  for await (const piece of handler({ method: "POST", path, headers: {}, body: Buffer.from(JSON.stringify(body)), fromApp: true } as any, undefined)) {
    if ("head" in piece) status = piece.head.status
    if ("body" in piece) text += Buffer.from(piece.body).toString()
  }
  return { status, body: text ? JSON.parse(text) : null }
}

test("a bot's model reasoning shows as thinking only after two seconds of it, and goes off a moment after it calls a tool", async () => {
  const folder = await aBot("bot_thinks", "ses_thinks")
  add("ses_thinks", folder, 700)
  active.add("ses_thinks")
  const stop = keepList()
  const facts = () => keptList()!.sessions.ses_thinks!.facts
  const hear = hearing("ses_thinks")
  try {
    await listPlaceHeard("engine", target)
    expect(await nowOfBot("bot_thinks")).toEqual({ working: 1, asking: 0, state: "working", since: expect.any(Number) })
    // What it streams reads nothing again, and changes nothing listed by itself.
    const [reads, at] = [oneReads, keptList()!.seq]
    for (const type of ["session.step.started", "session.text.started", "session.tool.input.started", "session.tool.called"]) await hear(type)
    workOut()
    expect([oneReads, keptList()!.seq]).toEqual([reads, at])
    jest.useFakeTimers()
    // A short stretch of reasoning, then a tool: never thinking.
    await hear("session.reasoning.started")
    clock.advanceTimersByTime(1900)
    workOut()
    await hear("session.tool.input.started")
    clock.advanceTimersByTime(5000)
    workOut()
    expect(facts().thinking).toBeUndefined()
    expect((await nowOfBot("bot_thinks")).state).toBe("working")

    // A long one: thinking once it's gone on two seconds.
    await hear("session.reasoning.started")
    clock.advanceTimersByTime(1900)
    workOut()
    expect(facts().thinking).toBeUndefined()
    clock.advanceTimersByTime(200)
    workOut()
    expect(facts().thinking).toBe(true)
    expect(await nowOfBot("bot_thinks")).toEqual({ working: 1, asking: 0, thinking: 1, state: "thinking", since: expect.any(Number) })

    // Words between two thoughts: thinking still.
    await hear("session.text.started")
    clock.advanceTimersByTime(1000)
    await hear("session.reasoning.started")
    clock.advanceTimersByTime(3000)
    workOut()
    expect(facts().thinking).toBe(true)

    // A tool called: off a moment and a half after.
    await hear("session.tool.called")
    clock.advanceTimersByTime(1400)
    workOut()
    expect(facts().thinking).toBe(true)
    clock.advanceTimersByTime(200)
    workOut()
    expect(facts().thinking).toBeUndefined()
    expect(await nowOfBot("bot_thinks")).toMatchObject({ working: 1, state: "working" })
    expect((await nowOfBot("bot_thinks")).thinking).toBeUndefined()
  } finally {
    jest.useRealTimers()
    stop()
    sessions.delete("ses_thinks")
    active.delete("ses_thinks")
    rmSync(folder, { recursive: true, force: true })
  }
})

test("a run that fails blocks its bot until it runs again or you've seen it (a limit said as one), and seen on any client a bot that's done is idle", async () => {
  const folder = await aBot("bot_fails", "ses_fails")
  add("ses_fails", folder, 700, { time: { created: 700, updated: 700, idle: 600, viewed: 600 } })
  // Work it did for the bot, finished: a turn you haven't read.
  add("ses_fails_w", work, 710, { time: { created: 705, updated: 710, idle: 710 }, metadata: { kuvo: { startedBy: "ses_fails", by: { bot: "bot_fails", name: "Ada" }, depth: 1 } } })
  active.add("ses_fails")
  const stop = keepList()
  const hear = hearing("ses_fails")
  const ends = async (type: string, idle: number, error?: unknown) => {
    Object.assign(sessions.get("ses_fails").time, { updated: idle, idle })
    active.delete("ses_fails")
    await hear(type, error ? { error } : {})
    workOut()
  }
  const followed = following("kuvo/bots/events", line => line.bots.find((bot: any) => bot.id === "bot_fails")?.now)
  try {
    await listPlaceHeard("engine", target)
    expect(await nowOfBot("bot_fails")).toMatchObject({ working: 1, state: "working" })
    await followed.until(now => now?.state === "working")

    // Stopped on a usage limit: blocked, by a limit; a client following the bots hears it.
    await ends("session.execution.failed", 800, { type: "provider.quota", message: "You've reached your limit." })
    expect(keptList()!.sessions.ses_fails!.facts).toMatchObject({ blocked: "limit", unread: 800 })
    expect(await nowOfBot("bot_fails")).toEqual({ working: 0, asking: 0, finished: 800, blocked: 1, blockedBy: "limit", state: "blocked", since: expect.any(Number) })
    expect(await followed.until(now => now?.state === "blocked")).toMatchObject({ blockedBy: "limit" })

    // Run again: no longer blocked.
    active.add("ses_fails")
    await hear("session.execution.started")
    workOut()
    expect(keptList()!.sessions.ses_fails!.facts.blocked).toBeUndefined()
    expect(await nowOfBot("bot_fails")).toMatchObject({ working: 1, state: "working" })

    // A coding agent's limit, as the bridge says one: a limit too.
    active.add("ses_fails")
    await hear("session.execution.started")
    await ends("session.execution.failed", 850, { type: "limit", message: "You've reached your Codex usage limit for now." })
    expect(await nowOfBot("bot_fails")).toMatchObject({ blocked: 1, blockedBy: "limit", state: "blocked" })
    active.add("ses_fails")
    await hear("session.execution.started")

    // Stopped on an error; a box started again since still knows it.
    await ends("session.execution.failed", 900, { type: "provider.overloaded", message: "Overloaded." })
    expect(await nowOfBot("bot_fails")).toMatchObject({ blocked: 1, blockedBy: "error", state: "blocked" })
    stop()
    const again = keepList()
    try {
      await listPlaceHeard("engine", target)
      expect(await nowOfBot("bot_fails")).toMatchObject({ blocked: 1, blockedBy: "error", state: "blocked" })

      // Seen, on any client: neither blocked nor done, and a client following the bots hears it. Its
      // own conversation is read, as a client reads the one it shows; its work's turn, left unread
      // for you, stays new until you open it.
      views.length = 0
      expect(await post("kuvo/bots/bot_fails/seen")).toEqual({ status: 200, body: {} })
      const seenAt = Date.now()
      expect(views).toEqual([["ses_fails", 900]])
      workOut()
      expect(keptList()!.sessions.ses_fails!.facts).not.toHaveProperty("blocked")
      expect(keptList()!.sessions.ses_fails!.facts).not.toHaveProperty("unread")
      expect(keptList()!.sessions.ses_fails_w!.facts.unread).toBe(710)
      expect(sessions.get("ses_fails_w").time.viewed).toBeUndefined()
      expect(await nowOfBot("bot_fails")).toEqual({ working: 0, asking: 0, finished: 900, state: "idle", since: expect.any(Number) })
      await followed.until(now => now?.state === "idle")

      // Its run over since, unread: done, since it ended; read on another client (`session.viewed`): idle.
      active.add("ses_fails")
      await hear("session.execution.started")
      await ends("session.execution.succeeded", seenAt + 1000)
      expect(await nowOfBot("bot_fails")).toEqual({ working: 0, asking: 0, finished: seenAt + 1000, state: "done", since: seenAt + 1000 })
      await hear("session.viewed", { idle: seenAt + 1000 })
      workOut()
      expect(await nowOfBot("bot_fails")).toMatchObject({ state: "idle" })

      // Nothing for a bot that isn't here; only POST.
      expect((await post("kuvo/bots/bot_nowhere/seen")).status).toBe(404)
      expect((await ask("kuvo/bots/bot_fails/seen")).status).toBe(405)
    } finally {
      again()
    }
  } finally {
    await followed.end()
    stop()
    for (const id of ["ses_fails", "ses_fails_w"]) { sessions.delete(id); active.delete(id) }
    rmSync(folder, { recursive: true, force: true })
  }
})

test("a retry blocks its bot only once it's gone on ten seconds, and a step begun again ends it", async () => {
  const folder = await aBot("bot_retries", "ses_retries")
  add("ses_retries", folder, 700)
  active.add("ses_retries")
  const stop = keepList()
  const hear = hearing("ses_retries")
  const blocked = () => keptList()!.sessions.ses_retries!.facts.blocked
  try {
    await listPlaceHeard("engine", target)
    jest.useFakeTimers()
    // A short retry: never blocked.
    await hear("session.retry.scheduled", { error: { type: "provider.rate-limit" } })
    clock.advanceTimersByTime(9000)
    await hear("session.step.started")
    clock.advanceTimersByTime(5000)
    workOut()
    expect(blocked()).toBeUndefined()

    // A long one: blocked, by a limit, after ten seconds; retried again meanwhile.
    await hear("session.retry.scheduled", { error: { type: "provider.rate-limit" } })
    clock.advanceTimersByTime(6000)
    await hear("session.retry.scheduled", { error: { type: "provider.rate-limit" } })
    clock.advanceTimersByTime(3900)
    workOut()
    expect(blocked()).toBeUndefined()
    clock.advanceTimersByTime(200)
    workOut()
    expect(blocked()).toBe("limit")
    expect(await nowOfBot("bot_retries")).toEqual({ working: 1, asking: 0, blocked: 1, blockedBy: "limit", state: "blocked", since: expect.any(Number) })

    // Its step begun again: at work.
    await hear("session.step.started")
    workOut()
    expect(blocked()).toBeUndefined()
    expect(await nowOfBot("bot_retries")).toMatchObject({ state: "working" })
  } finally {
    jest.useRealTimers()
    stop()
    sessions.delete("ses_retries")
    active.delete("ses_retries")
    rmSync(folder, { recursive: true, force: true })
  }
})

test("the bots' events say a bot asleep is idle as its next wake-up comes within 12 hours, with nothing else changing", async () => {
  jest.useFakeTimers()
  const due = Math.floor(Date.now() / 1000) + 12 * 3600 + 60
  const folder = await aBot("bot_sleeps", "ses_sleeps", { [`wakes/${due}-abc`]: "check the build" })
  const stop = new AbortController()
  const stream = handler({ method: "GET", path: "kuvo/bots/events", headers: {}, fromApp: true }, undefined, stop.signal)
  let text = ""
  const nextLine = async () => {
    for (;;) {
      const lines = text.split("\n")
      if (lines.length > 1) { text = lines.slice(1).join("\n"); return JSON.parse(lines[0]!) }
      const { value, done } = await stream.next()
      if (done) throw new Error("ended")
      if ("body" in value) text += Buffer.from(value.body).toString()
    }
  }
  const sleeps = (line: any) => line.bots.find((bot: any) => bot.id === "bot_sleeps").now
  try {
    const asleep = sleeps(await nextLine())
    expect(asleep).toEqual({ working: 0, asking: 0, state: "asleep", since: expect.any(Number) })
    // A minute on, 12 hours to go: idle, since then (not before).
    clock.advanceTimersByTime(61_000)
    const idle = sleeps(await nextLine())
    expect(idle).toEqual({ working: 0, asking: 0, state: "idle", since: expect.any(Number) })
    expect(idle.since).toBeGreaterThan(due * 1000 - 12 * 3_600_000)
  } finally {
    stop.abort()
    await stream.return(undefined)
    jest.useRealTimers()
    rmSync(folder, { recursive: true, force: true })
  }
})

test("work the box reads for you doesn't make its bot done while the box judges whether it's news to you", async () => {
  const { keepWorkRead } = await import("../src/link/work/tree")
  const folder = await aBot("bot_leads", "ses_leads")
  add("ses_leads", folder, 700, { time: { created: 700, updated: 700, idle: 690, viewed: 690 } })
  add("ses_led", work, 710, { time: { created: 705, updated: 710, idle: 600, viewed: 600 }, metadata: { kuvo: { startedBy: "ses_leads", by: { bot: "bot_leads", name: "Ada" }, depth: 1 } } })
  active.add("ses_led")
  const stop = keepList()
  try {
    await listPlaceHeard("engine", target)
    expect(await nowOfBot("bot_leads")).toMatchObject({ state: "working" })
    // Its turn ends; the box looks at whether what it did is news to you.
    Object.assign(sessions.get("ses_led").time, { updated: 800, idle: 800 })
    active.delete("ses_led")
    const held = holdRead("ses_led")
    const judged = keepWorkRead(target, "ses_led")
    await held.asked
    await noteListEvent("engine", target, { type: "session.execution.succeeded", data: { sessionID: "ses_led" } })
    workOut()
    expect(keptList()!.sessions.ses_led!.facts.unread).toBeUndefined()
    expect(await nowOfBot("bot_leads")).toMatchObject({ state: "idle" })
    // Judged (here it couldn't read what it did: left for you), it's a turn you haven't read.
    held.letGo()
    await judged
    workOut()
    expect(keptList()!.sessions.ses_led!.facts.unread).toBe(800)
    expect(await nowOfBot("bot_leads")).toEqual({ working: 0, asking: 0, finished: 800, state: "done", since: 800 })
  } finally {
    stop()
    for (const id of ["ses_leads", "ses_led"]) { sessions.delete(id); active.delete(id) }
    rmSync(folder, { recursive: true, force: true })
  }
})

/** A turn of a bot's work, newest first, as the engine pages it: its end at `ended` (how it went),
 * begun by its lead's message, after a turn that ended at `before`. */
const workTurn = (ended: number, before: number, outcome = "succeeded") => [
  { type: "idle", outcome, time: { created: ended } },
  { type: "assistant", content: [{ type: "text", text: "Done." }], time: { created: ended - 1 } },
  { type: "user", text: "[Work from Ada] Look into it.", time: { created: before + 1 } },
  { type: "idle", outcome: "succeeded", time: { created: before } },
]

test("a slow judgement shows the work's turn as unread after half a minute, and still reads it for you once it's done", async () => {
  const { keepWorkRead, JUDGED_FOR } = await import("../src/link/work/tree")
  const folder = await aBot("bot_slow", "ses_slow")
  add("ses_slow", folder, 700, { time: { created: 700, updated: 700, idle: 690, viewed: 690 } })
  add("ses_slowled", work, 710, { time: { created: 705, updated: 710, idle: 600, viewed: 600 }, metadata: { kuvo: { startedBy: "ses_slow", by: { bot: "bot_slow", name: "Ada" }, depth: 1 } } })
  active.add("ses_slowled")
  const stop = keepList()
  try {
    await listPlaceHeard("engine", target)
    // Its turn ends with nothing that's news to you; the engine is slow to say so.
    Object.assign(sessions.get("ses_slowled").time, { updated: 800, idle: 800 })
    messages.set("ses_slowled", workTurn(800, 600))
    active.delete("ses_slowled")
    views.length = 0
    jest.useFakeTimers()
    const held = holdRead("ses_slowled")
    const judged = keepWorkRead(target, "ses_slowled")
    await held.asked
    await noteListEvent("engine", target, { type: "session.execution.succeeded", data: { sessionID: "ses_slowled" } })
    workOut()
    expect(keptList()!.sessions.ses_slowled!.facts.unread).toBeUndefined()
    // Half a minute on, unanswered: a turn you haven't read, so its bot is done.
    clock.advanceTimersByTime(JUDGED_FOR)
    workOut()
    expect(keptList()!.sessions.ses_slowled!.facts.unread).toBe(800)
    expect((await nowOfBot("bot_slow")).state).toBe("done")
    // The engine answers: judged not news, it's read for you, and on every client (`session.viewed`).
    jest.useRealTimers()
    held.letGo()
    await judged
    expect(views).toEqual([["ses_slowled", 800]])
    await noteListEvent("engine", target, { type: "session.viewed", data: { sessionID: "ses_slowled", idle: 800 } })
    workOut()
    expect(keptList()!.sessions.ses_slowled!.facts.unread).toBeUndefined()
    expect((await nowOfBot("bot_slow")).state).toBe("idle")
  } finally {
    jest.useRealTimers()
    stop()
    for (const id of ["ses_slow", "ses_slowled"]) { sessions.delete(id); messages.delete(id); active.delete(id) }
    rmSync(folder, { recursive: true, force: true })
  }
})

test("a stop you've read past doesn't block its session again when it runs and ends unheard", async () => {
  const folder = await aBot("bot_reruns", "ses_reruns")
  add("ses_reruns", folder, 700, { time: { created: 700, updated: 700, idle: 600, viewed: 600 } })
  active.add("ses_reruns")
  const stop = keepList()
  const hear = hearing("ses_reruns")
  const facts = () => keptList()!.sessions.ses_reruns!.facts
  try {
    await listPlaceHeard("engine", target)
    Object.assign(sessions.get("ses_reruns").time, { updated: 800, idle: 800 })
    active.delete("ses_reruns")
    await hear("session.execution.failed", { error: { type: "provider.overloaded" } })
    workOut()
    expect(facts()).toMatchObject({ blocked: "error", unread: 800 })
    // Read on a client: not blocked.
    sessions.get("ses_reruns").time.viewed = 800
    await hear("session.viewed", { idle: 800 })
    workOut()
    expect(facts().blocked).toBeUndefined()
    // It runs again and ends, its start and end unheard (its events dropped a while): a turn you
    // haven't read, not the stop you read.
    Object.assign(sessions.get("ses_reruns").time, { updated: 900, idle: 900 })
    await hear("session.updated")
    workOut()
    expect(facts().unread).toBe(900)
    expect(facts().blocked).toBeUndefined()
    expect((await nowOfBot("bot_reruns")).state).toBe("done")
  } finally {
    stop()
    sessions.delete("ses_reruns")
    active.delete("ses_reruns")
    rmSync(folder, { recursive: true, force: true })
  }
})

test("seeing a bot leaves the work READ_RULE kept new for you unread on every client, and a newer finish is done again", async () => {
  const { keepWorkRead } = await import("../src/link/work/tree")
  const folder = await aBot("bot_news", "ses_news")
  add("ses_news", folder, 700, { time: { created: 700, updated: 700, idle: 690, viewed: 690 } })
  add("ses_newsled", work, 710, { time: { created: 705, updated: 800, idle: 800, viewed: 600 }, metadata: { kuvo: { startedBy: "ses_news", by: { bot: "bot_news", name: "Ada" }, depth: 1 } } })
  // Its turn failed: news to you, so the box leaves it unread.
  messages.set("ses_newsled", workTurn(800, 600, "failed"))
  const stop = keepList()
  const followed = following("kuvo/bots/events", line => line.bots.find((bot: any) => bot.id === "bot_news")?.now)
  try {
    await listPlaceHeard("engine", target)
    views.length = 0
    await keepWorkRead(target, "ses_newsled")
    workOut()
    expect(views).toEqual([])
    expect(await nowOfBot("bot_news")).toMatchObject({ state: "done", since: 800 })

    // Seen: the bot isn't done on any client, and the work's session is still new.
    expect(await post("kuvo/bots/bot_news/seen")).toEqual({ status: 200, body: {} })
    const seenAt = Date.now()
    await followed.until(now => now?.state === "idle")
    workOut()
    expect(views).toEqual([])
    expect(keptList()!.sessions.ses_newsled!.facts.unread).toBe(800)
    expect(await nowOfBot("bot_news")).toMatchObject({ state: "idle" })

    // It works again and finishes after you saw the bot: done, since then.
    Object.assign(sessions.get("ses_newsled").time, { updated: seenAt + 500, idle: seenAt + 500 })
    await noteListEvent("engine", target, { type: "session.execution.succeeded", data: { sessionID: "ses_newsled" } })
    workOut()
    expect(await nowOfBot("bot_news")).toMatchObject({ state: "done", since: seenAt + 500 })
  } finally {
    await followed.end()
    stop()
    for (const id of ["ses_news", "ses_newsled"]) { sessions.delete(id); messages.delete(id); active.delete(id) }
    rmSync(folder, { recursive: true, force: true })
  }
})

test("a bot seen while the box still judges its work's turn is blocked by it once that turn proves news to you", async () => {
  const { keepWorkRead } = await import("../src/link/work/tree")
  const folder = await aBot("bot_judged", "ses_judged")
  add("ses_judged", folder, 700, { time: { created: 700, updated: 700, idle: 690, viewed: 690 } })
  add("ses_judgedled", work, 710, { time: { created: 705, updated: 710, idle: 600, viewed: 600 }, metadata: { kuvo: { startedBy: "ses_judged", by: { bot: "bot_judged", name: "Ada" }, depth: 1 } } })
  active.add("ses_judgedled")
  const stop = keepList()
  try {
    await listPlaceHeard("engine", target)
    // Its turn fails; the box looks at whether that's news to you, and you open the bot meanwhile.
    const ended = Date.now()
    Object.assign(sessions.get("ses_judgedled").time, { updated: ended, idle: ended })
    messages.set("ses_judgedled", workTurn(ended, 600, "failed"))
    active.delete("ses_judgedled")
    const held = holdRead("ses_judgedled")
    const judged = keepWorkRead(target, "ses_judgedled")
    await held.asked
    await noteListEvent("engine", target, { type: "session.execution.failed", data: { sessionID: "ses_judgedled", error: { type: "provider.overloaded" } } })
    workOut()
    expect(keptList()!.sessions.ses_judgedled!.facts.unread).toBeUndefined()
    expect(await post("kuvo/bots/bot_judged/seen")).toEqual({ status: 200, body: {} })
    // News to you (it failed): left unread, and the bot says so, though you saw it as it was judged.
    held.letGo()
    await judged
    workOut()
    expect(keptList()!.sessions.ses_judgedled!.facts).toMatchObject({ unread: ended, blocked: "error" })
    expect(await nowOfBot("bot_judged")).toMatchObject({ state: "blocked", blockedBy: "error" })
  } finally {
    stop()
    for (const id of ["ses_judged", "ses_judgedled"]) { sessions.delete(id); messages.delete(id); active.delete(id) }
    rmSync(folder, { recursive: true, force: true })
  }
})

test("a bot wakes only during its wake-up turn, including after the box starts mid-turn", async () => {
  const id = "ses_waking", bot = "bot_waking"
  const folder = await aBot(bot, id, { "wakes/1-again-1d": "Check the build" })
  add(id, folder, 700)
  active.add(id)
  const wake = { id: "msg_waking", type: "user", text: "[Wake-up] Check the build" }
  messages.set(id, [wake])
  const stop = keepList()
  const hear = hearing(id)
  const followed = following("kuvo/bots/events", line => line.bots.find((each: any) => each.id === bot))
  try {
    await listPlaceHeard("engine", target)
    expect(await nowOfBot(bot)).toMatchObject({ waking: true, working: 1 })
    await followed.until(bot => bot?.now.waking === true)

    active.delete(id)
    messages.set(id, [{ id: "msg_end", type: "idle" }, wake])
    await hear("session.execution.succeeded")
    workOut()
    const ended = await followed.until(bot => bot?.now.working === 0)
    expect(ended.now.waking).toBeUndefined()
    expect(ended.wakes).toBe(1) // A past-due timer still on disk says nothing about the turn.

    active.add(id)
    await hear("session.execution.started")
    await hear("session.inbox.delivered")
    workOut()
    expect(await nowOfBot(bot)).not.toHaveProperty("waking")
    messages.set(id, [wake, { id: "msg_end", type: "idle" }, wake])
    await hear("session.inbox.delivered")
    workOut()
    await followed.until(bot => bot?.now.waking === true)
    active.delete(id)
    await hear("session.execution.interrupted")
    workOut()
    expect((await followed.until(bot => bot?.now.working === 0)).now.waking).toBeUndefined()
  } finally {
    await followed.end()
    stop()
    active.delete(id)
    messages.delete(id)
    sessions.delete(id)
    rmSync(folder, { recursive: true, force: true })
  }
})

test("a bot's schedule stream supplies stored summaries, watch check times and stable cancellation keys", async () => {
  const id = "ses_schedule"
  const folder = await aBot("bot_schedule", id, {
    "wakes/100-first-1d": "Long instructions, which the agent still reads.",
    "summaries/first": "Check build status",
    "wakes/200-later": "Read release notes. The rest is the full note.",
    "watches/ci/check": "check",
    "watches/ci/every": "5m",
    "watches/ci/next": "300",
    "watches/ci/note": "Full CI instructions.",
    "watches/ci/summary": "Watch CI",
  })
  const stream = following(`kuvo/wakes/${id}/events`, line => line)
  try {
    const listed = await stream.until(line => line.wakes.length === 2)
    expect(listed.wakes).toMatchObject([
      { due: 100, every: "1d", summary: "Check build status", cancelKey: "first" },
      { due: 200, summary: "Read release notes.", cancelKey: "later" },
    ])
    expect(listed.watches).toMatchObject([{ letters: "ci", next: 300, every: "5m", summary: "Watch CI" }])
    // A repeat can advance while its confirmation is open.
    rmSync(join(folder, "wakes/100-first-1d"))
    writeFileSync(join(folder, "wakes/999-first-1d"), "Long instructions.")
    expect((await post(`kuvo/wakes/${id}/cancel`, { wake: "first" })).status).toBe(200)
    expect((await post(`kuvo/wakes/${id}/cancel`, { watch: "ci" })).status).toBe(200)
    const after = await stream.until(line => line.wakes.length === 1 && line.watches.length === 0)
    expect(after.wakes[0].cancelKey).toBe("later")
  } finally {
    await stream.end()
    rmSync(folder, { recursive: true, force: true })
  }
})

test("cancel waits for the keeper's lock and unlinks the renamed repeat", async () => {
  const id = "ses_cancelrace"
  const folder = await aBot("bot_cancelrace", id, { "wakes/100-repeat-1d": "Check the build" })
  const lock = join(dirname(folder), ".firing")
  mkdirSync(lock)
  try {
    let settled = false
    const cancel = post(`kuvo/wakes/${id}/cancel`, { wake: "repeat" }).finally(() => { settled = true })
    await Bun.sleep(50)
    expect(settled).toBe(false)
    renameSync(join(folder, "wakes/100-repeat-1d"), join(folder, "wakes/999-repeat-1d"))
    rmSync(lock, { recursive: true })
    expect((await cancel).status).toBe(200)
    expect((await ask(`kuvo/wakes/${id}`)).lines[0].wakes).toEqual([])
  } finally {
    rmSync(lock, { force: true, recursive: true })
    rmSync(folder, { force: true, recursive: true })
  }
})

test("the second active snapshot preserves a wake turn started while its response is held", async () => {
  const id = "ses_refreshwake"
  const folder = await aBot("bot_refreshwake", id)
  add(id, folder, 700)
  const stop = keepList()
  let releaseList = () => {}
  let releaseActive = () => {}
  try {
    await listPlaceHeard("engine", target)
    const startedList = new Promise<void>(done => { heldAsked = done })
    held = new Promise<void>(done => { releaseList = done })
    const heldSecond = new Promise<void>(asked => {
      let calls = 0
      holdActive = async () => {
        if (++calls === 2) {
          const release = new Promise<void>(done => { releaseActive = done })
          asked()
          await release
        }
      }
    })
    const refresh = listPlaceHeard("engine", target)
    await startedList
    await hearing(id)("session.updated")
    held = null
    releaseList()
    await heldSecond
    messages.set(id, [{ id: "msg_wake", type: "user", text: "[Wake-up] Check the build" }])
    active.add(id)
    await hearing(id)("session.execution.started")
    await hearing(id)("session.inbox.delivered")
    releaseActive()
    await refresh
    expect(await nowOfBot("bot_refreshwake")).toMatchObject({ waking: true, working: 1 })
  } finally {
    held = null
    holdActive = undefined
    releaseList()
    releaseActive()
    stop()
    active.delete(id)
    sessions.delete(id)
    messages.delete(id)
    rmSync(folder, { recursive: true, force: true })
  }
})

test("a wake turn retries a failed message read and keeps its confirmed state during refresh failure", async () => {
  const id = "ses_retrywake", bot = "bot_retrywake"
  const folder = await aBot(bot, id)
  add(id, folder, 700)
  active.add(id)
  messages.set(id, [{ id: "msg_wake", type: "user", text: "[Wake-up] Check the build" }])
  failedMessages.set(id, 1)
  const stop = keepList()
  try {
    await listPlaceHeard("engine", target)
    expect(await nowOfBot(bot)).not.toHaveProperty("waking")
    for (let tries = 0; tries < 30 && !(await nowOfBot(bot)).waking; tries++) await Bun.sleep(50)
    expect(await nowOfBot(bot)).toMatchObject({ waking: true })
    failedMessages.set(id, 5)
    await listPlaceHeard("engine", target)
    expect(await nowOfBot(bot)).toMatchObject({ waking: true })
    active.delete(id)
    await hearing(id)("session.execution.succeeded")
    workOut()
    expect(await nowOfBot(bot)).not.toHaveProperty("waking")
  } finally {
    stop()
    active.delete(id)
    sessions.delete(id)
    messages.delete(id)
    failedMessages.delete(id)
    rmSync(folder, { recursive: true, force: true })
  }
})

test("schedule streams share root watches and skip unrelated or superseded updates", async () => {
  const a = await aBot("bot_watchone", "ses_watchone", { "wakes/100-one": "First" })
  const b = await aBot("bot_watchtwo", "ses_watchtwo", { "wakes/100-two": "Other" })
  const { wakesChanged } = await import("../src/link/areas/wakes")
  const watch = jest.spyOn(fs, "watch")
  const read = jest.spyOn(fs, "readFileSync")
  const first = following("kuvo/wakes/ses_watchone/events", line => line)
  const second = following("kuvo/wakes/ses_watchtwo/events", line => line)
  try {
    await first.until(line => line.wakes.length === 1)
    await second.until(line => line.wakes.length === 1)
    const { BOTS, SESSION_WAKES } = await import("../src/link/home")
    const roots = [BOTS, SESSION_WAKES]
    const rootWatches = () => watch.mock.calls.filter(([path]) => roots.includes(String(path)))
    // The first snapshot comes before the filesystem subscription. Pull the next line of each
    // generator, as the server does once it has sent the snapshot.
    const nextFirst = first.until(() => true)
    const nextSecond = second.until(() => true)
    for (let tries = 0; tries < 50 && rootWatches().length < 2; tries++) await Bun.sleep(10)
    expect(rootWatches()).toHaveLength(2)
    writeFileSync(join(a, "wakes/100-one"), "From the filesystem")
    writeFileSync(join(b, "wakes/100-two"), "The other filesystem update")
    expect((await nextFirst).wakes[0].summary).toBe("From the filesystem")
    expect((await nextSecond).wakes[0].summary).toBe("The other filesystem update")
    read.mockClear()
    for (const summary of ["Second", "Third", "Last"]) {
      writeFileSync(join(a, "wakes/100-one"), summary)
      wakesChanged("ses_watchone")
    }
    expect(read.mock.calls.filter(([path]) => String(path).startsWith(`${b}/wakes/`))).toHaveLength(0)
    expect((await first.until(() => true)).wakes[0].summary).toBe("Last")
  } finally {
    await first.end()
    await second.end()
    read.mockRestore()
    watch.mockRestore()
    rmSync(a, { recursive: true, force: true })
    rmSync(b, { recursive: true, force: true })
  }
})

test("a schedule stream empties when ownership switches away from its source", async () => {
  const id = "ses_movewake", bot = "bot_movewake", move = "mov_wakemove"
  const folder = await aBot(bot, id, { "wakes/100-one": "First" })
  const { MOVES, saveMove, forgetMove, changeMove } = await import("../src/link/moves/records")
  mkdirSync(MOVES, { recursive: true })
  saveMove({ id: move, thing: bot, kind: "bot", name: "Ada", from: "here", to: "there", peer: "there",
    role: "from", stage: "copying", sessions: [id], started: Date.now(), wasPaused: false, tries: 0 })
  const stream = following(`kuvo/wakes/${id}/events`, line => line)
  try {
    await stream.until(line => line.wakes.length === 1)
    changeMove(move, record => { record.stage = "switching" })
    expect((await ask(`kuvo/wakes/${id}`)).lines[0].wakes).toEqual([])
    expect((await stream.until(() => true)).wakes).toEqual([])
    expect(fs.existsSync(join(folder, "wakes/100-one"))).toBe(true)
  } finally {
    await stream.end()
    forgetMove(move)
    rmSync(folder, { recursive: true, force: true })
  }
})

test("a Git worktree's sessions are filed under its project, including a removed worktree the engine remembers", async () => {
  const { ENGINE_DATA } = await import("../src/link/home")
  mkdirSync(ENGINE_DATA, { recursive: true })
  const file = join(ENGINE_DATA, "opencode-lists.test.db")
  const oldTree = join(home, "lists-removed-tree")
  const repo = join(home, "lists-repo")
  const tree = join(home, "lists-linked-tree")
  mkdirSync(repo)
  const git = (...args: string[]) => execFileSync("git", ["-c", "core.hooksPath=/dev/null", ...args], {
    cwd: repo, stdio: "pipe", env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" },
  })
  git("init", "-q")
  git("-c", "user.email=test@example.invalid", "-c", "user.name=Test", "commit", "--allow-empty", "-qm", "Initial")
  git("worktree", "add", "--detach", tree)
  const child = join(tree, "nested")
  mkdirSync(child)
  const projects = join(home, "box", "projects.json")
  const before = readFileSync(projects)
  try {
    const db = new Database(file)
    try {
      db.exec(`CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT, vcs TEXT);
        CREATE TABLE worktree (project_id TEXT, directory TEXT);
        CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time_created INTEGER);
        CREATE TABLE session_message (session_id TEXT, seq INTEGER, time_created INTEGER);`)
      db.query("INSERT INTO project VALUES ('project', ?, 'git')").run(repo)
      db.query("INSERT INTO worktree VALUES ('project', ?)").run(oldTree)
      db.exec("INSERT INTO session_v2 VALUES ('ses_removed', 1); INSERT INTO session_message VALUES ('ses_removed', 1, 300)")
    } finally { db.close() }
    writeFileSync(projects, JSON.stringify({ version: 1, projects: [
      { directory: repo, folders: [], name: "Repository" }, { directory: tree, folders: [] },
    ] }))
    add("ses_linked", tree, 500)
    add("ses_nested", child, 400)
    add("ses_removed", oldTree, 900)
    const { status, lines: [list] } = await ask("kuvo/sessions/list")
    expect(status).toBe(200)
    expect(list.sessions.ses_linked).toMatchObject({ directory: tree, project: repo })
    expect(list.sessions.ses_nested).toMatchObject({ directory: child, project: repo })
    expect(list.sessions.ses_removed).toMatchObject({ directory: oldTree, project: repo, updated: 300 })
    expect(list.projects[repo].shown).toEqual(["ses_linked", "ses_nested", "ses_removed"])
    expect(list.projects[tree]).toBeUndefined()
    expect(list.projects[child]).toBeUndefined()
    const stop = keepList()
    try {
      await listPlaceHeard("engine", target)
      sessions.get("ses_removed").title = "Renamed"
      sessions.get("ses_removed").time.updated = 1000
      await noteListEvent("engine", target, { type: "session.renamed", data: { sessionID: "ses_removed" } })
      const visible = (await ask("kuvo/sessions/list")).lines[0]
      expect(visible.sessions.ses_removed).toMatchObject({ title: "Renamed", updated: 300, project: repo })
    } finally { stop() }
  } finally {
    sessions.delete("ses_linked")
    sessions.delete("ses_nested")
    sessions.delete("ses_removed")
    rmSync(file, { force: true })
    writeFileSync(projects, before)
    rmSync(repo, { recursive: true })
    rmSync(tree, { recursive: true })
  }
})
