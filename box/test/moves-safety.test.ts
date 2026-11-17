import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"
import { once } from "node:events"
import { ask } from "./route-answer"
import type { MoveRecord } from "../src/link/moves/records"

const { BOTS, RUN, ENGINE_FILE, BRIDGE_FILE } = await import("../src/link/home")
const { engineHandler } = await import("../src/link/server/router")
const { serveBox } = await import("../src/link/server/serve")
const { forgetMovesInMemory, keepMoving, resumeMoves, sendMovesWith, takeMoveMail } = await import("../src/link/moves/move")
const { MOVES, forgetMove, noteEnded, readMove, saveMove } = await import("../src/link/moves/records")

const id = "mov_Safety172"
const route = `kuvo/moves/receipt?id=${id}`
const session = "ses_Safety172"
const group = "grp_Safety172"
const peer = { key: "lab", kind: "mac" as const }
const folder = path.join(BOTS, "bot_Safety172")
const outcome = path.join(MOVES, `${id}.outcome.json`)
const held = path.join(folder, "paused")
const mail: Record<string, unknown>[] = []
let exportFails = false, deletes = 0, groupActive = false, activity = 0, crashOnActivate = false
let committedBeforeActivation = false

const server = Bun.serve({
  port: 0,
  fetch(request) {
    const endpoint = new URL(request.url).pathname
    if (endpoint.endsWith("/export")) {
      return exportFails
        ? Response.json({ message: "EIO reading /private/engine/index.sqlite" }, { status: 500 })
        : Response.json({ data: { info: { id: session }, messages: [] } })
    }
    if (endpoint.endsWith("/inbox") || endpoint === "/api/session" || endpoint === "/api/kuvo/groups") {
      return Response.json({ data: [] })
    }
    if (endpoint.endsWith("/activate")) {
      committedBeforeActivation = Boolean(readMove(id)?.activation)
      if (!groupActive) activity++
      groupActive = true
      if (crashOnActivate) return Response.json({ message: "The process died after activation" }, { status: 500 })
      return Response.json({})
    }
    if (endpoint.endsWith("/release")) {
      groupActive = false
      activity = 0
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
    from: peer.key, to: "studio", complete: true, safeSwitch: true, sessions: [] }))
  crashOnActivate = true
  await peerMail("switch", { safeSwitch: true })
  expect(committedBeforeActivation).toBe(true)
  expect(groupActive).toBe(true)
  expect(mail.some(item => item.op === "switched")).toBe(false)
  forgetMovesInMemory()
  crashOnActivate = false
  await resumeMoves()
  await peerMail("drop")
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
  await peerMail("offer", { kind: "chat", thing: session, name: "Notes", hash: "a".repeat(64), size: 1 })
  expect(readMove(id)).toBeNull()
  expect(mail.at(-1)).toMatchObject({ op: "refused", message: expect.stringContaining("Update Kuvo") })
})
