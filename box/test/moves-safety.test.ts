import "./safety"
import { afterAll, beforeEach, expect, spyOn, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"
import { once } from "node:events"
import { ask } from "./route-answer"
import type { MoveRecord } from "../src/link/moves/records"

const { BOTS, RUN, ENGINE_FILE, BRIDGE_FILE } = await import("../src/link/home")
const { engineHandler } = await import("../src/link/server/router")
const { serveBox } = await import("../src/link/server/serve")
const { forgetMovesInMemory, hold, keepMoving, resumeMoves, sendMovesWith, takeMoveMail } = await import("../src/link/moves/move")
const { MOVES, forgetMove, noteEnded, readMove, saveMove } = await import("../src/link/moves/records")
const { pack } = await import("../src/link/moves/bundle")
const { queueOf, unpackQueues, WAITING_DIR } = await import("../src/link/engine/queue")

const id = "mov_Safety172"
const route = `kuvo/moves/receipt?id=${id}`
const session = "ses_Safety172"
const group = "grp_Safety172"
const peer = { key: "lab", kind: "mac" as const }
const folder = path.join(BOTS, "bot_Safety172")
const outcome = path.join(MOVES, `${id}.outcome.json`)
const held = path.join(folder, "paused")
const mail: Record<string, unknown>[] = []
const bundle = { generation: 1, hash: "a".repeat(43) }
const item = "msg_Safety172"
let exportFails = false, deletes = 0, groupActive = false, activity = 0, crashOnActivate = false
let committedBeforeActivation = false
let groupName: string | undefined
let prompts = 0, accepted = false
let beforeAcceptance: (() => void) | undefined
const syncs: string[] = []
let syncedAtActivation: string[] = []

const server = Bun.serve({
  port: 0,
  async fetch(request) {
    const endpoint = new URL(request.url).pathname
    if (endpoint.endsWith("/export")) {
      return exportFails
        ? Response.json({ message: "EIO reading /private/engine/index.sqlite" }, { status: 500 })
        : Response.json({ data: { info: { id: session }, messages: [] } })
    }
    if (endpoint.endsWith("/inbox") || endpoint === "/api/session" || endpoint === "/api/kuvo/groups") {
      return Response.json({ data: [] })
    }
    if (endpoint.endsWith(`/message/${item}`) || endpoint.endsWith(`/prompt-receipt/${item}`)) {
      return accepted ? Response.json({ data: { id: item } }) : Response.json({}, { status: 404 })
    }
    if (endpoint.endsWith("/prompt")) {
      const prompt = await request.json() as { id: string }
      if (prompt.id !== item) return Response.json({}, { status: 400 })
      prompts++
      if (beforeAcceptance) {
        beforeAcceptance()
        return new Promise<Response>(resolve => {
          request.signal.addEventListener("abort", () => resolve(new Response(null, { status: 503 })), { once: true })
        })
      }
      accepted = true
      return Response.json({})
    }
    if (endpoint.endsWith(`/${group}`)) {
      if (request.method === "PUT") {
        const body = await request.json() as { group: { name: string } }
        groupName = body.group.name
        return Response.json({})
      }
      return Response.json({}, { status: groupName ? 200 : 404 })
    }
    if (endpoint.endsWith("/activate")) {
      committedBeforeActivation = Boolean(readMove(id)?.activation)
      syncedAtActivation = [...syncs]
      if (!groupActive) activity++
      groupActive = true
      if (crashOnActivate) return Response.json({ message: "The process died after activation" }, { status: 500 })
      return Response.json({})
    }
    if (endpoint.endsWith("/release")) {
      groupActive = false
      activity = 0
      groupName = undefined
      return Response.json({})
    }
    if (request.method === "DELETE") deletes++
    return Response.json({ data: {} })
  },
})

const target = { port: server.port!, password: "pw", pid: process.pid }
const handler = engineHandler(() => target, { name: "Studio" }, () => target)
const receipt = async () => (await ask(handler, "GET", route)).json
const peerMail = (op: string, fields = {}) => takeMoveMail(peer, { op, id, ...fields })
const record = (fields: Partial<MoveRecord> = {}): MoveRecord => ({
  id, kind: "bot", thing: "bot_Safety172", name: "Mira", from: "studio", to: peer.key,
  role: "from", peer: peer.key, stage: "switching", step: "switch", sessions: [session],
  folder, wasPaused: false, safeSwitch: true, started: Date.now(), tries: 0, ...fields,
})

beforeEach(() => {
  forgetMove(id)
  fs.rmSync(outcome, { force: true })
  fs.rmSync(path.join(MOVES, "ended.json"), { force: true })
  fs.mkdirSync(folder, { recursive: true })
  fs.writeFileSync(held, "moving")
  fs.mkdirSync(RUN, { recursive: true })
  for (const file of [ENGINE_FILE, BRIDGE_FILE]) fs.writeFileSync(file, JSON.stringify(target))
  exportFails = false
  crashOnActivate = false
  committedBeforeActivation = false
  groupActive = false
  groupName = undefined
  beforeAcceptance = undefined
  accepted = false
  prompts = 0
  syncs.length = 0
  activity = deletes = 0
  mail.length = 0
  sendMovesWith((_, message) => { mail.push(message); return true })
})

afterAll(() => {
  sendMovesWith(null)
  server.stop(true)
  forgetMove(id)
  for (const file of [outcome, path.join(MOVES, "ended.json"), ENGINE_FILE, BRIDGE_FILE]) fs.rmSync(file, { force: true })
  fs.rmSync(folder, { recursive: true, force: true })
})

test("confirmed switch succeeds even across eleven failed source exports; the original stays held", async () => {
  exportFails = true
  saveMove(record())
  await peerMail("switched")
  await peerMail("dropped", { neverSwitched: true })
  for (let attempt = 0; attempt < 10; attempt++) await resumeMoves()
  expect(readMove(id)?.stage).toBe("finishing")
  expect(await receipt()).toMatchObject({ state: "done", note: expect.stringContaining("retired") })
  fs.rmSync(outcome)
  expect(await receipt()).toMatchObject({ state: "done", note: expect.stringContaining("retired") })
  expect(fs.readFileSync(held, "utf8")).toBe("moving")
  expect(deletes).toBe(0)
  exportFails = false
  await resumeMoves()
  expect(readMove(id)).toBeNull()
  expect(deletes).toBe(1)
  expect(await receipt()).toMatchObject({ state: "done" })
})

test("a removed destination that never acknowledged and an ambiguous drop cannot revive the original", async () => {
  saveMove(record())
  await peerMail("dropped")
  expect(readMove(id)?.stage).toBe("switching")
  const now = Date.now()
  keepMoving(now)
  keepMoving(now + 6 * 60_000)
  await Bun.sleep(20)
  expect(readMove(id)?.stage).toBe("switching")
  expect(fs.readFileSync(held, "utf8")).toBe("moving")
  expect(await receipt()).toMatchObject({ state: "moving" })
  // Only an explicit no-switch proof allows the parked original to go on.
  await peerMail("dropped", { neverSwitched: true })
  expect(await receipt()).toMatchObject({ state: "failed" })
  expect(fs.existsSync(held)).toBe(false)
})

test("a two-year-old switch still answers switched; an unknown move cannot prove it was dropped", async () => {
  fs.mkdirSync(MOVES, { recursive: true })
  fs.writeFileSync(path.join(MOVES, "ended.json"), JSON.stringify({ [id]: {
    as: "switched", peer: peer.key, at: Date.now() - 2 * 365 * 24 * 3600_000,
  } }))
  noteEnded("mov_OtherSafety", "dropped")
  await peerMail("switch", { safeSwitch: true })
  await peerMail("drop")
  expect(mail.map(item => item.op)).toEqual(["switched", "switched"])
  await takeMoveMail(peer, { op: "switch", id: "mov_UnknownSafety", safeSwitch: true })
  expect(mail).toHaveLength(2)
})

test("activation is journaled before a group can work, and recovery rejects a drop after a lost activation response", async () => {
  saveMove(record({ kind: "group", thing: group, role: "to", stage: "copying", step: undefined,
    from: peer.key, to: "studio", complete: true, safeSwitch: true, sessions: [], ...bundle }))
  crashOnActivate = true
  await peerMail("switch", { safeSwitch: true, ...bundle })
  expect(committedBeforeActivation).toBe(true)
  expect(groupActive).toBe(true)
  expect(mail.some(item => item.op === "switched")).toBe(false)
  forgetMovesInMemory()
  crashOnActivate = false
  await resumeMoves()
  await peerMail("drop", bundle)
  expect(groupActive).toBe(true)
  expect(activity).toBe(1)
  expect(readMove(id)).toBeNull()
  expect(mail.map(item => item.op)).toEqual(["switched", "switched"])
})

test("receipt reads a failure after the failed record was saved but before the outcome was written", async () => {
  saveMove(record({ stage: "copying", step: "drop", failed: "The destination stopped answering. Try again.", offered: true }))
  expect(await receipt()).toEqual({ state: "failed", message: "The destination stopped answering. Try again." })
  await resumeMoves()
  expect(await receipt()).toMatchObject({ state: "failed" })
})

test("desktop can reconnect after the old receipt expiry; a missing receipt is distinct from a move failure", async () => {
  saveMove(record({ folder: undefined }))
  await peerMail("switched")
  expect(readMove(id)).toBeNull()
  const yesterday = new Date(Date.now() - 24 * 3600_000)
  fs.utimesSync(outcome, yesterday, yesterday)
  keepMoving()
  expect(await receipt()).toMatchObject({ state: "done" })
  const unknown = await ask(handler, "GET", "kuvo/moves/receipt?id=mov_UnknownSafety")
  expect(unknown.status).toBe(404)
  expect(unknown.json.message).toMatch(/result isn’t available/)
})

test("stale source credentials deny receipt access without losing or changing the move result", async () => {
  saveMove(record({ kind: "group", thing: group, folder: undefined, sessions: [] }))
  await peerMail("switched")
  const yesterday = new Date(Date.now() - 24 * 3600_000)
  fs.utimesSync(outcome, yesterday, yesterday)
  keepMoving()
  const http = serveBox("new-password", handler).listen(0, "127.0.0.1")
  try {
    await once(http, "listening")
    const address = http.address() as { port: number }
    const poll = (password: string) => fetch(`http://127.0.0.1:${address.port}/${route}`, {
      headers: { authorization: `Basic ${Buffer.from(`opencode:${password}`).toString("base64")}`, "x-kuvo-from": "app", connection: "close" },
    })
    expect((await poll("old-password")).status).toBe(401)
    const allowed = await poll("new-password")
    expect(allowed.status).toBe(200)
    expect(await allowed.json()).toMatchObject({ state: "done" })
  } finally {
    await new Promise<void>((resolve, reject) => http.close(error => error ? reject(error) : resolve()))
  }
})

test("an unexpected engine error during the final source comparison never leaks into a failed receipt", async () => {
  saveMove(record({ stage: "copying", step: "arrive", offered: true }))
  exportFails = true
  await peerMail("arrived")
  expect(await receipt()).toEqual({ state: "failed", message: "The source computer couldn’t prepare the move. Your work stayed there. Try again in a moment." })
})
test("a legacy peer cannot take part in a new switch", async () => {
  saveMove(record({ stage: "copying", step: "offer", safeSwitch: false, offered: true }))
  await peerMail("ready")
  expect(await receipt()).toMatchObject({ state: "failed", message: expect.stringContaining("newer Kuvo") })
  expect(mail.some(item => item.op === "switch")).toBe(false)
  forgetMove(id)
  await peerMail("offer", { kind: "chat", thing: session, name: "Notes", hash: bundle.hash, size: 1 })
  expect(readMove(id)).toBeNull()
  expect(mail.at(-1)).toMatchObject({ op: "refused", message: expect.stringContaining("Update Kuvo") })
})

test.each(["moving", `moving:${id}`])("restarting two moves keeps the newer move's %s hold", async marker => {
  const older = "mov_OlderSafety"
  saveMove(record({ id: older, peer: "desk", to: "desk", stage: "copying", step: "drop", offered: true,
    failed: "Desk stopped answering. Try again.", pauseRecorded: true, started: Date.now() - 1000 }))
  saveMove(record({ pauseRecorded: true, ...bundle }))
  fs.writeFileSync(held, marker)
  try {
    forgetMovesInMemory()
    await resumeMoves()
    expect(readMove(older)?.failed).toBeDefined()
    expect(readMove(id)?.stage).toBe("switching")
    expect(fs.readFileSync(held, "utf8")).toBe(marker)
  } finally {
    forgetMove(older)
    fs.rmSync(path.join(MOVES, `${older}.outcome.json`), { force: true })
  }
})

test("an old offer, its bytes, and its switch cannot replace an acknowledged newer bundle", async () => {
  const prepare = async (generation: number, name: string) => {
    const seed = `mov_Replay${generation}`
    try {
      const packed = await pack(seed, { kind: "group", thing: group, name, wasPaused: false, exports: [], group: { group: { id: group, name }, messages: [] } })
      return {
        identity: { generation, hash: packed.hash },
        contents: { kind: "group", thing: group, name, size: packed.size, safeSwitch: true },
        part: { i: 0, of: 1, d: fs.readFileSync(packed.file).toString("base64url") },
      }
    } finally { forgetMove(seed) }
  }
  const first = await prepare(1, "Old conversation")
  const second = await prepare(2, "Current conversation")
  const send = async (version: typeof first) => {
    await peerMail("offer", { ...version.contents, ...version.identity })
    await peerMail("part", { ...version.part, ...version.identity })
  }
  await send(first)
  await send(second)
  expect(mail.at(-1)).toMatchObject({ op: "arrived", ...second.identity })
  await send(first)
  await peerMail("switch", { safeSwitch: true, ...first.identity })
  expect(groupActive).toBe(false)
  expect(groupName).toBe("Current conversation")
  await peerMail("switch", { safeSwitch: true, ...second.identity })
  expect(groupActive).toBe(true)
  expect(mail.at(-1)).toMatchObject({ op: "switched", ...second.identity })
  mail.length = 0
  await peerMail("switch", { safeSwitch: true, ...first.identity })
  expect(mail).toEqual([])
  await peerMail("switch", { safeSwitch: true, ...second.identity })
  expect(mail.at(-1)).toMatchObject({ op: "switched", ...second.identity })
})

test("source ignores switched acknowledgements with an old generation or a different hash", async () => {
  const current = { ...bundle, generation: 2 }
  saveMove(record({ kind: "group", thing: group, folder: undefined, ...current }))
  await peerMail("switched", bundle)
  await peerMail("switched", { ...current, hash: "b".repeat(43) })
  expect(readMove(id)?.stage).toBe("switching")
  expect(await receipt()).toMatchObject({ state: "moving" })
  await peerMail("switched", current)
  expect(await receipt()).toMatchObject({ state: "done" })
})

test("an unbound legacy switch can query a decision but cannot activate a staged copy", async () => {
  saveMove(record({ kind: "group", thing: group, role: "to", stage: "copying", step: undefined, folder: undefined,
    sessions: [], complete: true }))
  await peerMail("switch", { safeSwitch: true })
  await peerMail("decision?")
  expect(groupActive).toBe(false)
  expect(mail).toEqual([])
  expect(readMove(id)?.complete).toBe(true)
})

test.each([null, "Back after lunch\n"])("restart between the pause journal and marker preserves %j", async paused => {
  if (paused === null) fs.rmSync(held)
  else fs.writeFileSync(held, paused)
  const moving = record({ stage: "holding", step: undefined, wasPaused: true })
  saveMove(moving)
  const rename = fs.renameSync
  const stop = spyOn(fs, "renameSync").mockImplementation((from, to) => {
    if (to === held) throw new Error("Bridge killed before writing the hold")
    rename(from, to)
  })
  try { expect(() => hold(moving)).toThrow("Bridge killed") } finally { stop.mockRestore() }
  expect(readMove(id)).toMatchObject({ pauseRecorded: true, wasPaused: paused !== null })
  expect(readMove(id)?.pausedAs).toBe(paused ?? undefined)
  forgetMovesInMemory()
  await resumeMoves()
  expect(fs.existsSync(held) ? fs.readFileSync(held, "utf8") : null).toBe(paused)
})

test.each([false, true])("a killed queue handoff recovers with the engine's acceptance at %j", async engineAccepted => {
  saveMove(record({ role: "to", stage: "copying", step: undefined, folder: undefined, from: peer.key, to: "studio",
    complete: true, activation: { at: Date.now() }, queuesGoOn: [session], ...bundle }))
  const payload = path.join(WAITING_DIR, `${item}.json`)
  unpackQueues({ [session]: { paused: false, waiting: [{ shown: { id: item, text: "Do the work", at: Date.now() },
    message: { id: item, text: "Do the work", delivery: "queue" } }] } }, [session])
  let attempted!: () => void
  const atEngine = new Promise<void>(resolve => { attempted = resolve })
  beforeAcceptance = () => { accepted = engineAccepted; attempted() }
  const moveCode = new URL("../src/link/moves/move.ts", import.meta.url).href
  const child = Bun.spawn([process.execPath, "--eval", `import { resumeMoves } from ${JSON.stringify(moveCode)}; await resumeMoves()`], { stdout: "ignore", stderr: "ignore" })
  let timeout: ReturnType<typeof setTimeout> | undefined
  try {
    const limit = new Promise<never>((_, reject) => { timeout = setTimeout(() => reject(new Error("Move worker did not dispatch")), 3000) })
    await Promise.race([atEngine, limit, child.exited.then(code => { throw new Error(`Move worker exited before dispatch (${code})`) })])
    expect(queueOf(session).waiting?.map(message => message.id)).toEqual([item])
    expect(fs.existsSync(payload)).toBe(true)
    expect(readMove(id)?.activation).toBeDefined()
    expect(mail).toEqual([])
  } finally {
    clearTimeout(timeout)
    child.kill("SIGKILL")
    await child.exited
    beforeAcceptance = undefined
  }
  await resumeMoves()
  expect(prompts).toBe(engineAccepted ? 1 : 2)
  expect(accepted).toBe(true)
  expect(queueOf(session).waiting).toBeUndefined()
  expect(fs.existsSync(payload)).toBe(false)
  expect(mail.at(-1)).toMatchObject({ op: "switched", ...bundle })
})

function interceptSync(fail: (file: string) => boolean) {
  const open = fs.openSync
  const fsync = fs.fsyncSync
  const paths = new Map<number, string>()
  const openSpy = spyOn(fs, "openSync").mockImplementation((file, flags, mode) => {
    const descriptor = open(file, flags, mode)
    paths.set(descriptor, String(file))
    return descriptor
  })
  const syncSpy = spyOn(fs, "fsyncSync").mockImplementation(descriptor => {
    const file = paths.get(descriptor) ?? ""
    if (fail(file)) throw new Error("Injected sync failure")
    fsync(descriptor)
    syncs.push(file)
  })
  return () => { syncSpy.mockRestore(); openSpy.mockRestore() }
}

test("record and directory syncs gate switch mail, activation, and replayed acknowledgements", async () => {
  saveMove(record({ kind: "group", thing: group, stage: "copying", step: "arrive", folder: undefined, ...bundle }))
  let stop = interceptSync(file => file === MOVES)
  try { await peerMail("arrived", bundle) } finally { stop() }
  expect(readMove(id)?.stage).toBe("switching")
  expect(syncs.some(file => file.includes(`${id}.json.`))).toBe(true)
  expect(mail.some(item => item.op === "switch")).toBe(false)

  saveMove(record({ kind: "group", thing: group, role: "to", stage: "copying", step: undefined, folder: undefined,
    sessions: [], complete: true, ...bundle }))
  stop = interceptSync(file => file === MOVES)
  try { await expect(peerMail("switch", { safeSwitch: true, ...bundle })).rejects.toThrow("Injected sync failure") } finally { stop() }
  expect(readMove(id)?.activation).toBeDefined()
  expect(groupActive).toBe(false)

  forgetMovesInMemory()
  let decisionRenamed = false
  const rename = fs.renameSync
  const observe = spyOn(fs, "renameSync").mockImplementation((from, to) => {
    rename(from, to)
    if (to === path.join(MOVES, "ended.json")) decisionRenamed = true
  })
  syncs.length = 0
  stop = interceptSync(file => file === MOVES && decisionRenamed)
  try {
    await resumeMoves()
    expect(groupActive).toBe(true)
    expect(syncedAtActivation.some(file => file.includes(`${id}.json.`))).toBe(true)
    expect(syncedAtActivation).toContain(MOVES)
    expect(syncedAtActivation).toContain(RUN)
    expect(mail.some(item => item.op === "switched")).toBe(false)
    await peerMail("switch", { safeSwitch: true, ...bundle })
    expect(mail.some(item => item.op === "switched")).toBe(false)
  } finally { stop(); observe.mockRestore() }
  await resumeMoves()
  expect(mail.at(-1)).toMatchObject({ op: "switched", ...bundle })
  expect(activity).toBe(1)
})
