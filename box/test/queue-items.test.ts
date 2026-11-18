import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"
import { ask, follow } from "./route-answer"
import { interceptSync } from "./disk-fault"

// The queue as one list every client draws (PROTOCOL.md, "The queue", `queue.items`): what the
// engine holds and what waits on the box, in the order they go, each as it was written; and Edit
// taking back one the engine holds, with what the composer needs. Only the engine is a stand-in,
// keeping each session's inbox and messages as the engine does.

const { engineHandler } = await import("../src/link/server/router")
const { forgetQueue, queueOf, queueWatcher, releaseWaiting, QUEUES_FILE, WAITING_DIR } = await import("../src/link/engine/queue")

let activeFails: boolean, acceptsPrompt: boolean
let beforeNextInbox: (() => Promise<void>) | undefined
let inboxes: Record<string, any[]>, delivered: Record<string, string[]>, keeps: Set<string>, got: { method: string; path: string }[]
beforeEach(() => {
  for (const session of ["ses_q", "ses_late"]) forgetQueue(session)
  activeFails = false
  acceptsPrompt = true
  beforeNextInbox = undefined
  got = []
  keeps = new Set()
  delivered = {}
  inboxes = {
    ses_q: [
      // Yours, its parts kept with it: what you wrote, and the keyword, not what the agent reads.
      { id: "msg_steer", sessionID: "ses_q", type: "user", delivery: "steer", payload: {
        text: "Use the blue one.\n\n<attached-files>\n- /tmp/shot.png\n</attached-files>",
        files: [{ mime: "image/png", name: "shot.png", data: "iVBORw0K", source: { type: "uri", uri: "file:///Users/me/shot.png" } }],
        metadata: { kuvo: { parts: { words: "Use the blue one.", keyword: "/goal" } } },
      } },
      // Another agent's letter, in its own words.
      { id: "msg_mail", sessionID: "ses_q", type: "user", delivery: "queue", payload: { text: "[Message from Mira · ses_mira] (Answer with send_message.)\n\nCan you check the build?" } },
      // A summary waiting is no message of yours.
      { id: "msg_sum", sessionID: "ses_q", type: "compaction", delivery: "queue", payload: {} },
    ],
    ses_late: [{ id: "msg_late", sessionID: "ses_late", type: "user", delivery: "queue", payload: { text: "Too late." } }],
  }
})

const engine = Bun.serve({
  port: 0,
  async fetch(request) {
    const url = new URL(request.url)
    got.push({ method: request.method, path: url.pathname })
    const reply = (value: unknown, status = 200) => Response.json(value, { status })
    if (url.pathname === "/api/session/active") return activeFails ? reply({}, 503) : reply({ data: {} })
    const [, id, kind, item] = url.pathname.match(/^\/api\/session\/([^/]+)(?:\/([^/]+)(?:\/([^/]+))?)?$/) ?? []
    if (id === "ses_broken") return reply({ message: "Busy." }, 500)
    if (!id || !inboxes[id]) return reply({ message: "No such session." }, 404)
    if (!kind) return reply({ data: { id, location: { directory: "/w" }, time: { created: 1, updated: 1 } } })
    if (kind === "message") {
      if (item) return (delivered[id] ?? []).includes(item) ? reply({ data: { id: item } }) : reply({}, 404)
      return reply({ data: (delivered[id] ?? []).map(each => ({ id: each, type: "user" })) })
    }
    if (kind === "prompt") {
      const prompt = await request.json() as { id: string }
      if (acceptsPrompt) (delivered[id] ??= []).push(prompt.id)
      return reply({ data: { id: prompt.id } })
    }
    if (kind === "interrupt") return reply({ data: true })
    if (kind !== "inbox") return reply({ message: "Not here." }, 404)
    if (!item) {
      const before = beforeNextInbox
      beforeNextInbox = undefined
      await before?.()
      return reply({ data: inboxes[id] })
    }
    const entry = inboxes[id]!.find(each => each.id === item)
    if (!entry) return reply({ message: "Not in the inbox." }, 404)
    if (request.method === "PATCH") {
      entry.delivery = (await request.json() as { delivery: string }).delivery
      return reply({ data: entry })
    }
    if (request.method === "DELETE") {
      // (One the agent took first: the cancel comes too late, and says nothing.)
      if (id === "ses_late") delivered[id] = [...(delivered[id] ?? []), item]
      if (!keeps.has(item)) inboxes[id] = inboxes[id]!.filter(each => each.id !== item)
      return reply({ data: true })
    }
    return reply({ message: "Not here." }, 404)
  },
})
const target = { port: engine.port, password: "pw" } as any
const handler = engineHandler(() => target, { name: "test" }, () => { throw new Error("none") })
afterAll(() => {
  engine.stop(true)
  for (const session of ["ses_q", "ses_late"]) forgetQueue(session)
})

// Stopped: what was for its next step held, the queue paused; then one of yours put back while it's
// paused (a new id each time: one sent again under its id is added once).
let adds = 0
async function stopAndAdd(): Promise<string> {
  const id = `msg_box${++adds}`
  expect((await ask(handler, "POST", "kuvo/sessions/ses_q/queue/hold")).status).toBe(200)
  const added = await ask(handler, "POST", "kuvo/sessions/ses_q/queue/add", { id, text: "Also the docs.", kept: true, words: "Also the docs.", files: [{ uri: "file:///Users/me/notes.md", name: "notes.md" }] })
  expect(added.json.waits).toBe(true)
  return id
}

test("Send Now and engine-swap release keep waiting payloads until the engine confirms their ID", async () => {
  const one = await stopAndAdd()
  const payload = path.join(WAITING_DIR, `${one}.json`)
  const sendNow = () => ask(handler, "POST", `kuvo/sessions/ses_q/queue/${one}/now`)
  activeFails = true
  expect((await sendNow()).status).toBe(502)
  expect(queueOf("ses_q").paused).toBe(true)
  expect(queueOf("ses_q").waiting?.map(item => item.id)).toContain(one)
  expect(fs.existsSync(payload)).toBe(true)

  // The existing engine items go first when the queue is resumed. This queued box prompt
  // remains for the engine-swap release pump to send.
  activeFails = false
  expect((await ask(handler, "POST", "kuvo/sessions/ses_q/queue/resume")).status).toBe(200)
  acceptsPrompt = false
  expect(await releaseWaiting(target)).toBe(0)
  expect(queueOf("ses_q").waiting?.map(item => item.id)).toContain(one)
  expect(fs.existsSync(payload)).toBe(true)
  acceptsPrompt = true
  expect(await releaseWaiting(target)).toBe(1)
  expect(delivered.ses_q).toEqual([one])
  expect(fs.existsSync(payload)).toBe(false)
})

test("paused send and queue add reject IDs that cannot be kept in the queue", async () => {
  expect((await ask(handler, "POST", "kuvo/sessions/ses_q/queue/hold")).status).toBe(200)
  for (const route of ["send", "queue/add"]) {
    const answer = await ask(handler, "POST", `kuvo/sessions/ses_q/${route}`, { id: "msg_bad_id", text: "Keep it.", kept: true, delivery: "queue" })
    expect(answer.status).toBe(400)
    expect(answer.json.message).toContain("Send it again as a new message.")
  }
  expect(queueOf("ses_q").waiting ?? []).toEqual([])
  expect(fs.existsSync(path.join(WAITING_DIR, "msg_bad_id.json"))).toBe(false)
})

test("a paused enqueue gets success only after its payload and index sync", async () => {
  const id = "msg_SaveFault"
  const payload = path.join(WAITING_DIR, `${id}.json`)
  const add = () => ask(handler, "POST", "kuvo/sessions/ses_q/queue/add", { id, text: "Keep this message.", kept: true })
  expect((await ask(handler, "POST", "kuvo/sessions/ses_q/queue/hold")).status).toBe(200)

  let restore = interceptSync(file => file.startsWith(`${payload}.`))
  try { expect((await add()).status).toBe(500) } finally { restore() }
  expect(fs.existsSync(payload)).toBe(false)

  const synced: string[] = []
  restore = interceptSync(file => file === path.dirname(QUEUES_FILE) && synced.some(each => each.startsWith(`${QUEUES_FILE}.`)), synced)
  try { expect((await add()).status).toBe(500) } finally { restore() }
  expect(synced.findIndex(file => file.startsWith(`${payload}.`))).toBeLessThan(synced.findIndex(file => file.startsWith(`${QUEUES_FILE}.`)))

  synced.length = 0
  restore = interceptSync(() => false, synced)
  try { expect((await add()).json).toMatchObject({ waits: true }) } finally { restore() }
  expect(synced.at(-1)).toBe(path.dirname(QUEUES_FILE))
  const saved = JSON.parse(fs.readFileSync(QUEUES_FILE, "utf8"))
  expect(saved.ses_q.waiting.map((each: { id: string }) => each.id)).toEqual([id])
  expect(JSON.parse(fs.readFileSync(payload, "utf8"))).toMatchObject({ id, text: "Keep this message." })
})

test("the queue lists every message waiting, in the order they go, as each was written", async () => {
  const box = await stopAndAdd()
  const { status, json } = await ask(handler, "GET", "kuvo/sessions/ses_q/queue?items=1")
  expect(status).toBe(200)
  expect(json.paused).toBe(true)
  expect(json.held).toEqual(["msg_steer"])
  expect(json.items).toEqual([
    { id: "msg_steer", words: "Use the blue one.", files: ["shot.png"], keyword: "/goal", from: null, on: "engine", delivery: "queue", held: true },
    { id: "msg_mail", words: "Can you check the build?", files: [], keyword: null, from: "Mira", on: "engine", delivery: "queue", held: false },
    { id: box, words: "Also the docs.", files: ["notes.md"], keyword: null, from: null, on: "box", delivery: "queue", held: false },
  ])
})

test("Edit takes back one the engine holds, with its words and files for the composer, and what came after it", async () => {
  const box = await stopAndAdd()
  const { status, json } = await ask(handler, "DELETE", "kuvo/sessions/ses_q/queue/msg_steer")
  expect(status).toBe(200)
  expect(json.removed).toEqual({
    id: "msg_steer", text: "Use the blue one.\n\n<attached-files>\n- /tmp/shot.png\n</attached-files>",
    files: [{ uri: "file:///Users/me/shot.png", name: "shot.png" }], delivery: "queue", words: "Use the blue one.",
    // (A goal: Edit puts it back as one, as sending it again asks for the goal again.)
    keyword: "/goal", draft: "/goal Use the blue one.", before: "msg_mail",
  })
  expect(got).toContainEqual({ method: "DELETE", path: "/api/session/ses_q/inbox/msg_steer" })
  // Gone from the list, and from what's held.
  const now = (await ask(handler, "GET", "kuvo/sessions/ses_q/queue?items=1")).json
  expect(now.held).toEqual([])
  expect(now.items.map((each: any) => each.id)).toEqual(["msg_mail", box])
  // Not waiting anywhere: 404.
  expect((await ask(handler, "DELETE", "kuvo/sessions/ses_q/queue/msg_none")).status).toBe(404)
})

test("one the agent took first isn't taken back, and one the engine keeps says so", async () => {
  expect((await ask(handler, "DELETE", "kuvo/sessions/ses_late/queue/msg_late")).status).toBe(404)
  keeps.add("msg_mail")
  expect((await ask(handler, "DELETE", "kuvo/sessions/ses_q/queue/msg_mail")).status).toBe(502)
})

test("while the engine can't be read the queue says what waits on the box, and that its items can't be said", async () => {
  const { json } = await ask(handler, "GET", "kuvo/sessions/ses_broken/queue?items=1")
  expect(json).toEqual({ paused: false, held: [], items: null })
  // A session no place has: nothing waits for it.
  expect((await ask(handler, "GET", "kuvo/sessions/ses_none/queue?items=1")).json.items).toEqual([])
  // Unasked, the box's own state at once, the engine not asked (the Mac's and the phone's reads).
  const before = got.length
  expect((await ask(handler, "GET", "kuvo/sessions/ses_q/queue")).json).toEqual({ paused: false, held: [] })
  expect(got.length).toBe(before)
})

test("followed, a session's queue comes again as the engine's inbox for it changes", async () => {
  const stream = follow(handler, "kuvo/sessions/ses_q/queue/events")
  try {
    expect((await stream.next()).items.map((each: any) => each.id)).toEqual(["msg_steer", "msg_mail"])
    // A message comes: the engine says so, as the box's long-running process hears it.
    inboxes.ses_q!.push({ id: "msg_new", sessionID: "ses_q", type: "user", delivery: "queue", payload: { text: "One more thing." } })
    queueWatcher(target, { schedule: () => 0, cancel: () => {} })({ type: "session.inbox.enqueued", data: { sessionID: "ses_q", inboxID: "msg_new", item: { type: "user", delivery: "queue", payload: { text: "One more thing." } } } })
    expect((await stream.next()).items.at(-1)).toMatchObject({ id: "msg_new", words: "One more thing.", on: "engine" })
  } finally {
    stream.stop()
  }
})

test("wake-ups waiting in the engine and on a stopped queue use the stored summary", async () => {
  await ask(handler, "POST", "kuvo/sessions/ses_q/queue/hold")
  const text = "[Wake-up] Check everything. More instructions here."
  const metadata = { kuvo: { wake: { summary: "Check the release" } } }
  inboxes.ses_q!.push({ id: "msg_engwake", type: "user", delivery: "queue", payload: { text, metadata } })
  const added = await ask(handler, "POST", "kuvo/sessions/ses_q/queue/add", { id: "msg_boxwake", text, metadata, kept: true, from: "Wake-up", words: text.slice("[Wake-up] ".length) })
  expect(added.json.waits).toBe(true)
  const { json } = await ask(handler, "GET", "kuvo/sessions/ses_q/queue?items=1")
  expect(json.items.filter((each: any) => each.from === "Wake-up").map((each: any) => [each.on, each.words])).toEqual([
    ["engine", "Check the release"], ["box", "Check the release"],
  ])
})

test("reordering persists across queue reads and the next drain follows it", async () => {
  // The already-steered message keeps its slot. Swap two messages waiting for the run to end.
  inboxes.ses_q!.push({ id: "msg_second", type: "user", delivery: "queue", payload: { text: "Second." } })
  const path = "kuvo/sessions/ses_q/queue"
  for (const _ of [1, 2]) {
    expect((await ask(handler, "POST", `${path}/reorder`, { item: "msg_second", anchor: "msg_mail", before: true })).status).toBe(200)
  }
  const { readQueues, drain } = await import("../src/link/engine/queue")
  expect(readQueues().ses_q?.order).toEqual(["msg_second", "msg_mail"])
  const reopened = await ask(handler, "GET", `${path}?items=1`)
  expect(reopened.json.items.map((row: { id: string }) => row.id)).toEqual(["msg_steer", "msg_second", "msg_mail"])
  inboxes.ses_q = inboxes.ses_q!.filter(row => row.id !== "msg_steer")
  await drain(target, "ses_q")
  expect(inboxes.ses_q.find(row => row.delivery === "steer")?.id).toBe("msg_second")
})

test("reorder interleaved with enqueue and Send Now delivers each message once in the chosen order", async () => {
  inboxes.ses_q = []
  const route = "kuvo/sessions/ses_q/queue"
  const add = (id: string) => ask(handler, "POST", `${route}/add`, { id, text: id, kept: true })
  const reorder = () => ask(handler, "POST", `${route}/reorder`, { item: "msg_c", anchor: "msg_b", before: true })
  const sendNow = () => ask(handler, "POST", `${route}/msg_a/now`)
  const listed = async () => (await ask(handler, "GET", `${route}?items=1`)).json.items.map((row: { id: string }) => row.id)
  expect((await ask(handler, "POST", "kuvo/sessions/ses_q/stop")).status).toBe(200)
  for (const id of ["msg_a", "msg_b", "msg_c"]) expect((await add(id)).json.waits).toBe(true)

  let restore = interceptSync(file => file.startsWith(`${QUEUES_FILE}.`))
  try { expect((await reorder()).status).toBe(502) } finally { restore() }
  expect(await listed()).toEqual(["msg_a", "msg_b", "msg_c"])

  const readingInbox = Promise.withResolvers<void>()
  const finishReading = Promise.withResolvers<void>()
  beforeNextInbox = () => {
    readingInbox.resolve()
    return finishReading.promise
  }
  const reordering = reorder()
  await readingInbox.promise
  try {
    expect((await add("msg_d")).json.waits).toBe(true)
    // The engine takes A, but the box can't seal the removal yet. Retrying must reuse the receipt.
    restore = interceptSync(file => file.startsWith(`${QUEUES_FILE}.`))
    try { expect((await sendNow()).status).toBe(502) } finally { restore() }
    expect((await sendNow()).status).toBe(200)
  } finally {
    finishReading.resolve()
  }
  expect((await reordering).status).toBe(200)
  expect(await listed()).toEqual(["msg_c", "msg_b", "msg_d"])
  expect(await releaseWaiting(target)).toBe(3)
  expect(delivered.ses_q).toEqual(["msg_a", "msg_c", "msg_b", "msg_d"])
  expect(await listed()).toEqual([])
})
