import "./safety"
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { pathToFileURL } from "node:url"
import { ArtifactStore } from "../src/link/artifacts/store"
import { Recorder, type Event, type Source } from "../src/link/artifacts/recorder"
import { libraryService } from "../src/link/library/service"
import { askPeers, libraryForPeer, merged, type Asked } from "../src/link/areas/library"
import type { Peer } from "../src/link/peers/known"

// The Library (`kuvo/library`): what was handed over to you on a computer (a file an agent showed,
// a picture it generated, a file you attached), never what it merely touched; filtered by whose and
// what kind; removed from it without the file going; filled from what came before it; and gathered
// across your computers, each item with the computer it's on.

let root: string
let bots: string
let project: string
let store: ArtifactStore
let library: ReturnType<typeof libraryService>
let recorder: Recorder
const sessions = new Map<string, any>()
/** The engine, as far as the record asks it: its conversations. */
const source: Source = { name: "engine", session: async (id) => sessions.get(id), agent: () => "engine" }

function session(id: string, directory: string, extra: Record<string, any> = {}) {
  sessions.set(id, { id, location: { directory }, ...extra })
}

function tool(sessionID: string, message: string, id: string, name: string, input: Record<string, unknown>, content: unknown[] = [{ type: "text", text: "done" }], metadata: unknown = {}): Event[] {
  return [
    { type: "session.tool.input.started", data: { sessionID, assistantMessageID: message, id, name } },
    { type: "session.tool.called", data: { sessionID, assistantMessageID: message, id, input } },
    { type: "session.tool.success", data: { sessionID, assistantMessageID: message, id, content, metadata } },
  ]
}

function turn(sessionID: string, message: string, events: Event[]): Event[] {
  return [
    { type: "session.execution.started", data: { sessionID } },
    { type: "session.inbox.delivered", data: { sessionID, inboxID: message } },
    ...events,
    { type: "session.execution.succeeded", data: { sessionID } },
  ]
}

/** A message of yours as it arrives, with files. */
function yours(sessionID: string, message: string, text: string, files: { uri: string; name: string }[]): Event {
  return { type: "session.inbox.enqueued", data: { sessionID, inboxID: message, item: { type: "user", payload: { text, files } } } }
}

async function hear(events: Event[]) {
  for (const event of events) await recorder.heard(source, event)
}

function file(path: string, bytes: string | Buffer = "x") {
  mkdirSync(join(path, ".."), { recursive: true })
  writeFileSync(path, bytes)
  return path
}

async function route(method: string, path: string, body: unknown = {}): Promise<{ status: number; value: any }> {
  const request = new Request("http://box/api/kuvo/link/" + path, { method, ...(method === "POST" ? { body: JSON.stringify(body) } : {}) })
  const response = await library.handle(request)
  const type = response.headers.get("content-type") ?? ""
  return { status: response.status, value: type.includes("json") ? await response.json() : Buffer.from(await response.arrayBuffer()).toString() }
}

const names = async (query = "") => (await route("GET", "library" + query)).value.items.map((item: any) => item.name)

beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "kuvo-library-"))
  bots = join(root, "bots")
  project = join(root, "project")
  mkdirSync(project, { recursive: true })
  mkdirSync(join(bots, "bot_ada", "workspace"), { recursive: true })
  writeFileSync(join(bots, "bot_ada", "bot.json"), JSON.stringify({ name: "Ada", look: { color: "mint", shape: "gem", face: "calm" } }))
  sessions.clear()
  store = new ArtifactStore(join(root, "artifacts"), { bots, disk: () => ({ free: 500 * 1024 ** 3, total: 1000 * 1024 ** 3 }) })
  library = libraryService(join(root, "library"), { bots, sessions: join(root, "sessions"), artifacts: store, computer: () => "Studio", platform: "linux" })
  recorder = new Recorder(store, { bots, handed: library.handed })
})

afterEach(() => {
  rmSync(root, { recursive: true, force: true })
})

describe("what counts as handed over", () => {
  test("a file an agent shows, a picture it generates and a file you attach are in the Library, each where it was handed over", async () => {
    session("ses_code", project)
    const report = file(join(project, "Q4 Board Brief.pdf"), "%PDF-1.4")
    const chart = file(join(project, "out", "chart.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47]))
    const brief = file(join(root, "Desktop", "brief.pdf"), "%PDF brief")
    await hear([yours("ses_code", "msg_you", "Here's the brief.", [{ uri: pathToFileURL(brief).href, name: "brief.pdf" }])])
    await hear(turn("ses_code", "msg_you", [
      ...tool("ses_code", "msg_a1", "c1", "kuvo-browser_show_file", { path: "Q4 Board Brief.pdf" }),
      ...tool("ses_code", "msg_a1", "c2", "generate_image", { prompt: "a chart" }, [{ type: "file", uri: pathToFileURL(chart).href, mime: "image/png" }]),
    ]))
    const { value } = await route("GET", "library")
    expect(value.items.map((item: any) => [item.name, item.kind, item.how]).sort()).toEqual([
      ["Q4 Board Brief.pdf", "document", "shown"],
      ["brief.pdf", "document", "attached"],
      ["chart.png", "image", "generated"],
    ])
    const shown = value.items.find((item: any) => item.how === "shown")
    expect(shown).toMatchObject({ session: "ses_code", message: "msg_a1", part: "c1", path: report, there: true })
    expect(shown.bot).toBeUndefined()
    expect(value.items.find((item: any) => item.how === "attached")).toMatchObject({ session: "ses_code", message: "msg_you" })
    expect(value.sources).toEqual({ bots: [], you: true })
  })

  test("its code edits, writes, a bot's notes, what it opens in the panel, logs and its own screenshots aren't", async () => {
    session("ses_bot", join(bots, "bot_ada", "workspace"), { metadata: { kuvo: { bot: "bot_ada" } } })
    const code = file(join(project, "src", "main.rs"), "fn main() {}\n")
    const memory = file(join(bots, "bot_ada", "MEMORY.md"), "# Notes\n")
    const log = file(join(project, "build.log"), "ok\n")
    const panel = file(join(project, "preview.html"), "<title>Preview</title>")
    const shot = file(join(root, "shots", "page.png"), Buffer.from([0x89, 0x50]))
    const secret = file(join(project, ".env"), "KEY=1")
    await hear(turn("ses_bot", "msg_1", [
      ...tool("ses_bot", "msg_a", "c1", "edit", { filePath: code }, undefined, { files: [{ file: code, status: "modified", patch: "" }] }),
      ...tool("ses_bot", "msg_a", "c2", "write", { filePath: join(project, "draft.md"), content: "x" }),
      ...tool("ses_bot", "msg_a", "c3", "show_file", { path: memory }),
      ...tool("ses_bot", "msg_a", "c4", "show_file", { path: log }),
      ...tool("ses_bot", "msg_a", "c5", "show_in_panel", { path: panel }),
      ...tool("ses_bot", "msg_a", "c6", "browser_screenshot", {}, [{ type: "file", uri: pathToFileURL(shot).href, mime: "image/png" }]),
      ...tool("ses_bot", "msg_a", "c7", "show_file", { path: secret }),
    ]))
    expect(await names()).toEqual([])
    // (The record still has what it made: the Library is what it handed over.)
    expect(store.at(code)).toBeDefined()
  })

  for (const folder of [".agents", ".codex", ".cursor", ".gemini", ".grok"]) {
    test(`files inside ${folder}/skills stay out of the Library`, async () => {
      session("ses_code", project)
      const notes = file(join(project, folder, "skills", "review", "references", "skills", "notes.md"), "# References")
      const report = file(join(project, "skills", "report.md"), "# Report")
      await hear(turn("ses_code", "msg_1", [
        ...tool("ses_code", "msg_a", "c1", "show_file", { path: notes }),
        ...tool("ses_code", "msg_a", "c2", "show_file", { path: report }),
      ]))
      expect(await names()).toEqual(["report.md"])
    })
  }

  test("a page it built is named by its title, and a subagent's is its parent's, at the turn", async () => {
    session("ses_code", project)
    session("ses_child", project, { parentID: "ses_code" })
    file(join(project, "pricing.html"), "<html><head><title>Pricing &amp; Seats</title></head></html>")
    await hear(turn("ses_code", "msg_ask", tool("ses_child", "msg_sub", "c1", "show_file", { path: "pricing.html" })))
    const { value } = await route("GET", "library")
    expect(value.items).toHaveLength(1)
    expect(value.items[0]).toMatchObject({ name: "Pricing & Seats", kind: "page", session: "ses_code", message: "msg_ask" })
    expect(value.items[0].part).toBeUndefined()
  })
})

describe("finding things", () => {
  beforeEach(() => {
    const at = (minutes: number) => new Date(Date.UTC(2026, 10, 21, 12, minutes))
    library.store.hand({ path: file(join(project, "hero.png")), how: "generated", bot: "bot_ada", session: "ses_bot", at: at(5) })
    library.store.hand({ path: file(join(project, "brief.pdf")), how: "shown", bot: "bot_ada", session: "ses_bot", at: at(3) })
    library.store.hand({ path: file(join(project, "terms.pdf")), how: "attached", session: "ses_you", at: at(4) })
    library.store.hand({ path: file(join(project, "export.zip")), how: "shown", session: "ses_you", at: at(1) })
  })

  test("newest first, by whose, by kind and by words in its name, its bot's or this computer's", async () => {
    expect(await names()).toEqual(["hero.png", "terms.pdf", "brief.pdf", "export.zip"])
    expect(await names("?source=bot_ada")).toEqual(["hero.png", "brief.pdf"])
    expect(await names("?source=you")).toEqual(["terms.pdf", "export.zip"])
    expect(await names("?kind=document")).toEqual(["terms.pdf", "brief.pdf"])
    expect(await names("?kind=other")).toEqual(["export.zip"])
    expect(await names("?q=TERMS")).toEqual(["terms.pdf"])
    expect(await names("?q=ada")).toEqual(["hero.png", "brief.pdf"])
    expect(await names("?q=studio%20hero")).toEqual(["hero.png"])
    expect((await route("GET", "library?kind=video")).status).toBe(400)
    expect((await route("GET", "library")).value.sources).toEqual({ bots: [{ id: "bot_ada", name: "Ada", look: { color: "mint", shape: "gem", face: "calm" } }], you: true })
  })

  test("Remove from Library hides it, the file stays, and it's still hidden after a restart", async () => {
    const id = (await route("GET", "library?q=brief")).value.items[0].id
    const removed = await route("POST", `library/${id}/hide`, {})
    expect(removed.value.hidden).toBeNumber()
    expect(await names()).toEqual(["hero.png", "terms.pdf", "export.zip"])
    expect(readFileSync(join(project, "brief.pdf"), "utf8")).toBe("x")
    expect(await names("?hidden=1")).toEqual(["brief.pdf"])
    library = libraryService(join(root, "library"), { bots, sessions: join(root, "sessions"), artifacts: store, platform: "linux" })
    expect(await names()).toEqual(["hero.png", "terms.pdf", "export.zip"])
    // Handed over again: back.
    library.store.hand({ path: join(project, "brief.pdf"), how: "shown", bot: "bot_ada", session: "ses_bot" })
    expect((await names())[0]).toBe("brief.pdf")
  })
})

test("a picture whose file is gone opens from the record's copy; one with nothing kept says so", async () => {
  session("ses_code", project)
  const chart = file(join(project, "tmp-chart.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47, 1, 2]))
  await hear(turn("ses_code", "msg_1", tool("ses_code", "msg_a", "c1", "generate_image", { prompt: "chart" }, [{ type: "file", uri: pathToFileURL(chart).href, mime: "image/png" }])))
  const id = (await route("GET", "library")).value.items[0].id
  rmSync(chart)
  const item = (await route("GET", `library/${id}`)).value
  expect(item).toMatchObject({ there: true, thumbnail: true })
  expect((await route("GET", `library/${id}/file`)).value).toBe(Buffer.from([0x89, 0x50, 0x4e, 0x47, 1, 2]).toString())
  expect((await route("GET", `library/${id}/thumbnail`)).status).toBe(200)
  const attached = file(join(root, "gone.pdf"), "%PDF")
  library.store.hand({ path: attached, how: "attached", session: "ses_code" })
  const other = (await route("GET", "library?q=gone")).value.items[0]
  rmSync(attached)
  expect((await route("GET", `library/${other.id}`)).value).toMatchObject({ there: false, thumbnail: false })
  expect((await route("GET", `library/${other.id}/file`)).status).toBe(410)
})

test("filled once from what came before it: saved conversations' shown files, pictures and your files", async () => {
  const saved = join(root, "sessions", "ses_old")
  mkdirSync(saved, { recursive: true })
  const deck = file(join(project, "deck.pdf"), "%PDF")
  const picture = file(join(project, "art.png"), Buffer.from([0x89, 0x50]))
  const attached = file(join(root, "Desktop", "notes.txt"), "notes")
  writeFileSync(join(saved, "session.json"), JSON.stringify({ id: "ses_old", title: "Old", location: { directory: project }, time: { created: 1, updated: 2 } }))
  writeFileSync(join(saved, "messages.json"), JSON.stringify([
    { id: "msg_u", type: "user", text: "see this", files: [{ mime: "text/plain", name: "notes.txt", source: { type: "uri", uri: pathToFileURL(attached).href } }], time: { created: 1_000 } },
    { id: "msg_a", type: "assistant", time: { created: 2_000, completed: 3_000 }, content: [
      { type: "tool", id: "t1", name: "kuvo-browser_show_file", state: { status: "completed", input: { path: "deck.pdf" } } },
      { type: "tool", id: "t2", name: "generate_image", state: { status: "completed", input: { prompt: "art" }, content: [{ type: "file", uri: pathToFileURL(picture).href, mime: "image/png" }] } },
      { type: "tool", id: "t3", name: "edit", state: { status: "completed", input: { filePath: join(project, "x.ts") } } },
    ] },
  ]))
  library.backfill()
  const { value } = await route("GET", "library")
  // (Handed over in the same message: as old as each other, so in no order of their own.)
  expect(value.items.map((item: any) => [item.name, item.how, item.message]).sort()).toEqual([
    ["art.png", "generated", "msg_a"],
    ["deck.pdf", "shown", "msg_a"],
    ["notes.txt", "attached", "msg_u"],
  ])
  expect(value.items.at(-1).name).toBe("notes.txt")
  expect(value.items.find((item: any) => item.name === "deck.pdf")).toMatchObject({ session: "ses_old", part: "t1", path: deck })
  // Once: removed since, it stays removed.
  await route("POST", `library/${value.items.find((item: any) => item.name === "art.png").id}/hide`, {})
  library.backfill()
  expect(await names()).toEqual(["deck.pdf", "notes.txt"])
})

describe("across your computers", () => {
  const peer = (key: string, name: string): Peer => ({ key, address: "x", kind: "mac", name } as Peer)
  const item = (id: string, at: number, extra: Record<string, unknown> = {}) => ({ id, name: id + ".png", kind: "image", type: "image/png", how: "generated", path: "/x/" + id, at, there: true, thumbnail: true, ...extra })

  test("each computer's items with the computer they're on, newest first; one away or too old is said, not waited for", async () => {
    const asked: string[] = []
    const peers = [peer("k_studio", "Mac Studio"), peer("k_old", "Old Mac"), peer("k_away", "Laptop")]
    const there = await askPeers("kind=image", null, {
      peers,
      has: (key) => key !== "k_old",
      away: (key) => key === "k_away",
      ask: async (computer, act) => {
        asked.push(`${computer.key} ${act.method} ${act.item} ${JSON.stringify(act.body)}`)
        return { status: 200, value: { items: [item("lib_b", 20, { bot: "bot_mira" }), item("lib_d", 5)], more: false, sources: { bots: [{ id: "bot_mira", name: "Mira" }], you: true } } }
      },
    })
    expect(asked).toEqual(['k_studio GET list {"query":"kind=image"}'])
    const here: Asked = { computer: { key: "k_here", name: "MacBook Pro", kind: "mac", here: true }, state: "here", part: { items: [item("lib_a", 30, { bot: "bot_ada" }), item("lib_c", 10)], sources: { bots: [{ id: "bot_ada", name: "Ada" }], you: false } } }
    const library = merged([here, ...there])
    expect(library.items.map((each: any) => [each.id, each.computer.name])).toEqual([["lib_a", "MacBook Pro"], ["lib_b", "Mac Studio"], ["lib_c", "MacBook Pro"], ["lib_d", "Mac Studio"]])
    expect(library.items[0].computer).toEqual({ key: "k_here", name: "MacBook Pro", here: true })
    expect(library.sources).toEqual({ bots: [{ id: "bot_ada", name: "Ada", computer: "k_here" }, { id: "bot_mira", name: "Mira", computer: "k_studio" }], you: true })
    expect(library.computers.map((each: any) => [each.name, each.state])).toEqual([["MacBook Pro", "here"], ["Mac Studio", "there"], ["Old Mac", "older"], ["Laptop", "away"]])
    expect((library.computers.find((each: any) => each.state === "older") as any).message).toContain("needs a newer Kuvo Code")
  })

  test("one that can't be read is said, and the rest still shown", async () => {
    const there = await askPeers("", null, { peers: [peer("k_studio", "Mac Studio")], has: () => true, away: () => false, ask: async () => ({ status: 503, refused: "Mac Studio stopped answering." }) })
    expect(there[0]).toMatchObject({ state: "away", message: "Mac Studio stopped answering." })
    expect(merged(there).items).toEqual([])
  })

  test("another computer asks only for the list, an item, its file or thumbnail, or removing one", async () => {
    const act = (item: string, method: "GET" | "POST" = "GET") => ({ id: "wqa_" + "x".repeat(20), area: "library" as const, method, item, at: Date.now() })
    expect((await libraryForPeer(act("../../etc/passwd"))).status).toBe(404)
    expect((await libraryForPeer(act("lib_" + "0".repeat(24) + "/hide"))).status).toBe(405)
    expect((await libraryForPeer(act("list", "POST"))).status).toBe(405)
  })
})
