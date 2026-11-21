import "./safety"
import { afterAll, beforeEach, expect, spyOn, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"
import { spawnSync } from "node:child_process"
import { once } from "node:events"
import { ask } from "./route-answer"
import { interceptSync } from "./disk-fault"
import type { MoveRecord } from "../src/link/moves/records"

const { BOTS, RUN, ENGINE_FILE, BRIDGE_FILE, LINK_FILE, K } = await import("../src/link/home")
const { engineHandler } = await import("../src/link/server/router")
const { serveBox } = await import("../src/link/server/serve")
const { forgetMovesInMemory, hold, keepMoving, resumeMoves, sendMovesWith, takeMoveMail } = await import("../src/link/moves/move")
const { MOVES, bundleOf, compactDecisionHints, endedFor, forgetMove, noteEnded, readMove, saveMove, switchedHere } = await import("../src/link/moves/records")
const { upgradeKuvoFolder } = await import("../src/shared/folder-version")
const { editSessionNote, forgetSessionNotes, packSessionNotes, sessionNoteView } = await import("../src/link/ownership/session-notes")
const { pack, unpack } = await import("../src/link/moves/bundle")
const { filesOf, WORK_QUEUE } = await import("../src/link/work-queue/store")
const { carryWorkQueue, forgetWorkQueues, packWorkQueues, placeWorkQueues } = await import("../src/link/work-queue/moves")
import type { CarriedQueues } from "../src/link/work-queue/moves"
const { newKey, publicOf, addressOf } = await import("../src/link/crypto/keys")
const { newSigningSeed, readDoc, signingKeyOf } = await import("../src/link/documents/format")
const { newSetupID } = await import("../src/link/setup/record")
const { readLink, updateLink, writeLink } = await import("../src/link/state")
const { MAX_SHARED_BYTES, pendingShared, sharedValue, takeShared } = await import("../src/link/setup/shared")
const { grantLease, leaseClock } = await import("../src/link/setup/lease")
const { settleTransfers, transferForPeer } = await import("../src/link/ownership/transfer")
const { acceptedOwner, mayActivate } = await import("../src/link/ownership/resolve")
const { handItem, removeItem } = await import("../src/link/work-queue/ops")
const { handUpkeep } = await import("../src/link/work-queue/hand")
const { allItems, changeQueues, itemOf } = await import("../src/link/work-queue/store")
const { queueOf, resumeQueue, unpackQueues, WAITING_DIR } = await import("../src/link/engine/queue")

const id = "mov_Safety172"
const route = `kuvo/moves/receipt?id=${id}`
const session = "ses_Safety172"
const group = "grp_Safety172"
const peer = { key: "lab", kind: "mac" as const }
const folder = path.join(BOTS, "bot_Safety172")
const outcome = path.join(MOVES, `${id}.outcome.json`)
const decisionFile = path.join(MOVES, "decisions", `${id}.json`)
const decisionHint = path.join(MOVES, "decision-hints", `${id}.json`)
const held = path.join(folder, "paused")
const arrivingBot = "bot_SafetyTransfer"
const arrivingSession = "ses_SafetyTransfer"
const arrivingFolder = path.join(BOTS, arrivingBot)
const arrivingHold = path.join(arrivingFolder, "paused")
const nested = path.join(arrivingFolder, "wakes", "checks", "note.txt")
const mail: Record<string, unknown>[] = []
const recordMail = (_peer: string, message: Record<string, unknown>) => { mail.push(message); return true }
const bundle = { generation: 1, hash: "a".repeat(43) }
const item = "msg_Safety172"
let exportFails = false, deletes = 0, groupActive = false, activity = 0, crashOnActivate = false
let committedBeforeActivation = false
let groupName: string | undefined
let listedHeld = false
let prompts = 0, accepted = false
let beforeAcceptance: (() => void) | undefined
let importedBot = false
const syncs: string[] = []
let syncedAtActivation: string[] = []

const server = Bun.serve({
  port: 0,
  async fetch(request) {
    const endpoint = new URL(request.url).pathname
    if (endpoint === "/api/experimental/session/import") {
      const input = await request.json() as { info: { id: string } }
      if (input.info.id === arrivingSession) importedBot = true
      return Response.json({ data: input.info })
    }
    if (endpoint === `/api/session/${arrivingSession}`) {
      if (request.method === "DELETE") importedBot = false
      return importedBot ? Response.json({ data: { id: arrivingSession } }) : Response.json({}, { status: 404 })
    }
    if (endpoint.endsWith("/export")) {
      return exportFails
        ? Response.json({ message: "EIO reading /private/engine/index.sqlite" }, { status: 500 })
        : Response.json({ data: { info: { id: session }, messages: [] } })
    }
    if (endpoint === "/api/kuvo/groups" && listedHeld) return Response.json({ data: [{ group: { id: group }, status: "held" }] })
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
  for (const file of [decisionFile, decisionHint]) fs.rmSync(file, { force: true })
  fs.rmSync(arrivingFolder, { recursive: true, force: true })
  fs.mkdirSync(folder, { recursive: true })
  fs.writeFileSync(held, "moving")
  fs.mkdirSync(RUN, { recursive: true })
  for (const file of [ENGINE_FILE, BRIDGE_FILE]) fs.writeFileSync(file, JSON.stringify(target))
  exportFails = false
  crashOnActivate = false
  committedBeforeActivation = false
  groupActive = false
  groupName = undefined
  importedBot = false
  beforeAcceptance = undefined
  accepted = false
  prompts = 0
  syncs.length = 0
  activity = deletes = 0
  mail.length = 0
  sendMovesWith(recordMail)
})

afterAll(() => {
  sendMovesWith(null)
  server.stop(true)
  forgetMove(id)
  for (const file of [outcome, path.join(MOVES, "ended.json"), decisionFile, decisionHint, ENGINE_FILE, BRIDGE_FILE]) fs.rmSync(file, { force: true })
  for (const each of [folder, arrivingFolder]) fs.rmSync(each, { recursive: true, force: true })
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

test.each(["ready", "arrived"] as const)("a peer without typed relocation cannot advance a session attachment move on %s", async op => {
  saveMove(record({ stage: "copying", step: op === "ready" ? "offer" : "arrive", offered: true,
    verifyPayloads: true, typedAttachmentPaths: true, ...bundle }))
  await peerMail(op, { safeSwitch: true, verifyPayloads: true, verifiedPayloads: true, ...bundle })
  expect(await receipt()).toMatchObject({ state: "failed", message: expect.stringContaining("newer Kuvo") })
  expect(mail.some(item => item.op === "part" || item.op === "switch")).toBe(false)
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

test("a frozen queue refuses Resume before changing its pause", async () => {
  saveMove(record())
  unpackQueues({ [session]: { paused: true, held: [], waiting: [{ shown: { id: item, text: "Wait here", at: Date.now() },
    message: { id: item, text: "Wait here", delivery: "queue" } }] } }, [session])
  const before = queueOf(session)
  expect(before.paused).toBe(true)
  await expect(resumeQueue(target, session)).rejects.toThrow("moving to another computer")
  expect(queueOf(session)).toEqual(before)
  expect(prompts).toBe(0)
})

test.each([false, true])("a killed queue handoff recovers with the engine's acceptance at %j", async engineAccepted => {
  saveMove(record({ role: "to", stage: "copying", step: undefined, folder: undefined, from: peer.key, to: "studio",
    complete: true, activation: { at: Date.now() }, queuesGoOn: [session], ...bundle }))
  const payload = path.join(WAITING_DIR, `${item}.json`)
  unpackQueues({ [session]: { paused: false, held: [], waiting: [{ shown: { id: item, text: "Do the work", at: Date.now() },
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

test("record and directory syncs gate switch mail, activation, and replayed acknowledgements", async () => {
  saveMove(record({ kind: "group", thing: group, stage: "copying", step: "arrive", folder: undefined, ...bundle }))
  let stop = interceptSync(file => file === MOVES, syncs)
  try { await peerMail("arrived", bundle) } finally { stop() }
  expect(readMove(id)?.stage).toBe("switching")
  expect(syncs.some(file => file.includes(`${id}.json.`))).toBe(true)
  expect(mail.some(item => item.op === "switch")).toBe(false)

  saveMove(record({ kind: "group", thing: group, role: "to", stage: "copying", step: undefined, folder: undefined,
    sessions: [], complete: true, ...bundle }))
  stop = interceptSync(file => file === MOVES, syncs)
  try { await expect(peerMail("switch", { safeSwitch: true, ...bundle })).rejects.toThrow("Injected sync failure") } finally { stop() }
  expect(readMove(id)?.activation).toBeDefined()
  expect(groupActive).toBe(false)

  forgetMovesInMemory()
  let decisionRenamed = false
  const rename = fs.renameSync
  const observe = spyOn(fs, "renameSync").mockImplementation((from, to) => {
    rename(from, to)
    if (to === decisionFile) decisionRenamed = true
  })
  syncs.length = 0
  stop = interceptSync(file => file === path.dirname(decisionFile) && decisionRenamed, syncs)
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

async function incomingBot(queues?: Record<string, unknown>, workQueue?: CarriedQueues, sessionNotes?: Record<string, unknown>) {
  const seed = "mov_IncomingSafety"
  fs.mkdirSync(path.dirname(nested), { recursive: true })
  fs.writeFileSync(nested, "This check must come with the bot.")
  fs.writeFileSync(path.join(arrivingFolder, "bot.json"), JSON.stringify({ kuvo: 1, id: arrivingBot, name: "Mira" }))
  fs.writeFileSync(path.join(arrivingFolder, "session"), arrivingSession)
  const packed = await pack(seed, { kind: "bot", thing: arrivingBot, name: "Mira", folder: arrivingBot, wasPaused: false,
    exports: [{ info: { id: arrivingSession }, messages: [] }], queues, workQueue, sessionNotes })
  if (workQueue) forgetWorkQueues([arrivingSession])
  const bytes = fs.readFileSync(packed.file)
  forgetMove(seed)
  fs.rmSync(arrivingFolder, { recursive: true })
  const identity = { generation: 1, hash: packed.hash }
  await peerMail("offer", { kind: "bot", thing: arrivingBot, name: "Mira", safeSwitch: true, size: bytes.length, ...(workQueue ? { verifyPayloads: true } : {}), ...identity })
  expect(mail.at(-1)).toMatchObject({ op: "ready", ...identity })
  const part = () => peerMail("part", { i: 0, of: 1, d: bytes.toString("base64url"), ...identity })
  const activate = () => peerMail("switch", { safeSwitch: true, ...identity })
  return { ...identity, part, activate }
}

for (const failure of ["attachment write", "directory sync", "index sync", "none"] as const) {
  test(`a Work Queue move waits for its attachment and index to persist (${failure})`, async () => {
    const added = handler({
      method: "POST", path: "kuvo/work-queue/items", headers: {}, fromApp: true,
      body: Buffer.from(JSON.stringify({
        operation: "keep-attachment", owner: arrivingSession, title: "Keep the attachment", files: [{ uri: "data:image/png;base64,YXR0YWNobWVudA==", name: "plan.png" }],
      })),
    }, undefined)
    expect((await added.next()).value).toMatchObject({ head: { status: 200 } })
    await added.return(undefined)
    const work = packWorkQueues([arrivingSession])!
    const attachment = path.join(filesOf(work.items[0].id), "plan.png")
    expect(fs.readFileSync(attachment, "utf8")).toBe("attachment")
    const incoming = await incomingBot(undefined, work)
    const index = path.join(WORK_QUEUE, "queues", `${arrivingSession}.json`)
    const staged = path.join(bundleOf(id), "opened", "queue-install", work.items[0].id, "plan.png")
    const synced: string[] = []
    let injected = false
    const restore = interceptSync(file => {
      const fails = (failure === "directory sync" && file === path.dirname(attachment)) ||
        (failure === "index sync" && file.startsWith(`${index}.`))
      if (fails) injected = true
      return fails
    }, synced, file => {
      if (failure !== "attachment write" || file !== staged) return false
      injected = true
      return true
    })
    const arrivalSyncs: string[][] = []
    sendMovesWith((peer, message) => {
      if (message.op === "arrived") arrivalSyncs.push([...synced])
      return recordMail(peer, message)
    })
    try { await incoming.part() } finally { restore(); sendMovesWith(recordMail) }
    if (failure !== "none") {
      expect(injected).toBe(true)
      expect(mail.some(each => each.op === "arrived")).toBe(false)
      expect(mail.at(-1)).toMatchObject({ op: "failed" })
      expect(importedBot).toBe(false)
      expect(fs.existsSync(attachment)).toBe(false)
      expect(allItems().some(item => item.id === work.items[0].id)).toBe(false)
    } else {
      expect(mail.at(-1)).toMatchObject({ op: "arrived", verifiedPayloads: true })
      expect(arrivalSyncs).toHaveLength(1)
      const syncedAtArrival = arrivalSyncs[0]!
      const fileAt = syncedAtArrival.indexOf(staged)
      const publishedAt = syncedAtArrival.indexOf(path.dirname(attachment))
      const parentAt = syncedAtArrival.indexOf(path.dirname(path.dirname(attachment)))
      const indexAt = syncedAtArrival.findIndex(file => file.startsWith(`${index}.`))
      expect(fileAt).toBeGreaterThanOrEqual(0)
      expect(publishedAt).toBeGreaterThan(fileAt)
      expect(parentAt).toBeGreaterThan(publishedAt)
      expect(indexAt).toBeGreaterThan(parentAt)
      expect(syncedAtArrival.slice(indexAt + 1)).toContain(path.dirname(index))
      expect(fs.readFileSync(attachment, "utf8")).toBe("attachment")
      await peerMail("drop", incoming)
    }
    fs.rmSync(filesOf(work.items[0].id), { recursive: true, force: true })
  })
}

test.each(["switch", "drop"] as const)("ordinary note history survives receiver placement or is removed on %s", async action => {
  const checkpoint = path.join(K, "folder-version.json")
  const previous = fs.existsSync(checkpoint) ? fs.readFileSync(checkpoint) : undefined
  upgradeKuvoFolder(K)
  const ref = { kind: "sessionNotes" as const, id: arrivingSession, note: "fixture" }
  const initial = editSessionNote({ ref, operation: "a".repeat(43), create: true, baseRevision: null, fields: { text: "Original." } }, () => true, () => true)
  if (!initial) throw new Error("The source note was not created")
  for (const [operation, text] of [["b", "First."], ["c", "Alternative."]]) {
    editSessionNote({ ref, operation: operation!.repeat(43), baseRevision: initial.revision, fields: { text } }, () => true, () => true)
  }
  const source = sessionNoteView(ref)
  expect(source.choices[0]).toMatchObject({ accepted: "First.", proposed: "Alternative." })
  const notes = packSessionNotes([arrivingSession])
  forgetSessionNotes([arrivingSession])
  try {
    const incoming = await incomingBot(undefined, undefined, notes)
    await incoming.part()
    expect(mail.at(-1)).toMatchObject({ op: "arrived", hash: incoming.hash })
    expect(mayActivate(ref)).toBe(false)
    expect(sessionNoteView(ref)).toEqual(source)
    if (action === "switch") {
      await incoming.activate()
      expect(mail.at(-1)).toMatchObject({ op: "switched", hash: incoming.hash })
      expect(mayActivate(ref)).toBe(true)
      expect(sessionNoteView(ref)).toEqual(source)
    } else {
      await peerMail("drop", incoming)
      expect(mail.at(-1)).toMatchObject({ op: "dropped", hash: incoming.hash })
      expect(packSessionNotes([arrivingSession])).toBeUndefined()
    }
  } finally {
    forgetSessionNotes([arrivingSession])
    if (previous) fs.writeFileSync(checkpoint, previous)
    else fs.rmSync(checkpoint, { force: true })
  }
})

test("moving a bot cannot report arrival until its nested wake-up files have synced", async () => {
  const incoming = await incomingBot()
  let sawNested = false
  const restore = interceptSync(file => {
    if (file !== nested) return false
    sawNested = true
    return true
  })
  try { await incoming.part() } finally { restore() }
  expect(sawNested).toBe(true)
  expect(mail.some(item => item.op === "arrived")).toBe(false)
  await incoming.activate()
  expect(mail.at(-1)).toMatchObject({ op: "dropped", neverSwitched: true, hash: incoming.hash })
  expect(importedBot).toBe(false)
})

test("replayed activation seals the already-unlinked pause before acknowledging a real import", async () => {
  const incoming = await incomingBot()
  await incoming.part()
  expect(mail.at(-1)).toMatchObject({ op: "arrived", hash: incoming.hash })
  expect(fs.readFileSync(nested, "utf8")).toBe("This check must come with the bot.")
  let attempts = 0
  const restore = interceptSync(file => {
    if (file !== arrivingFolder || fs.existsSync(arrivingHold)) return false
    attempts++
    return true
  })
  try {
    await incoming.activate()
    expect(fs.existsSync(arrivingHold)).toBe(false)
    expect(attempts).toBe(1)
    forgetMovesInMemory()
    await resumeMoves()
    expect(attempts).toBe(2)
    expect(mail.some(item => item.op === "switched")).toBe(false)
  } finally { restore() }
  await resumeMoves()
  expect(mail.at(-1)).toMatchObject({ op: "switched", generation: 1, hash: incoming.hash })
})

test("an incoming bundle missing an indexed queue payload is refused before arrival", async () => {
  const queuedId = "msg_NeverArrived"
  const incoming = await incomingBot({ [arrivingSession]: { paused: true, held: [queuedId],
    waiting: [{ shown: { id: queuedId, text: "Keep this", at: Date.now() }, message: null }] } })
  await incoming.part()
  expect(mail.at(-1)).toMatchObject({ op: "failed", message: expect.stringContaining("messages waiting to be sent") })
  expect(mail.some(item => item.op === "arrived")).toBe(false)
  await incoming.activate()
  expect(mail.at(-1)).toMatchObject({ op: "dropped", neverSwitched: true })
})

test("compacting a real move's routing hint keeps its indexed decision for an offline source", async () => {
  const incoming = await incomingBot()
  await incoming.part()
  await incoming.activate()
  expect(switchedHere().has(arrivingSession)).toBe(true)
  const receipt = fs.readFileSync(decisionFile, "utf8")
  expect(JSON.parse(receipt)).toMatchObject({ as: "switched", generation: 1, hash: incoming.hash })
  expect(fs.existsSync(path.join(MOVES, "ended.json"))).toBe(false)
  const yearsLater = Date.now() + 2 * 365 * 86400_000
  keepMoving(yearsLater, performance.now() + 7200_000)
  expect(fs.existsSync(decisionHint)).toBe(false)
  expect(fs.readFileSync(decisionFile, "utf8")).toBe(receipt)
  forgetMovesInMemory()
  mail.length = 0
  await incoming.activate()
  expect(mail.at(-1)).toMatchObject({ op: "switched", generation: 1, hash: incoming.hash })
})

test("move journals adopt the legacy shape and refuse to overwrite or discard a future owner record", () => {
  const file = path.join(MOVES, `${id}.json`)
  const original = record()
  fs.mkdirSync(MOVES, { recursive: true })
  fs.writeFileSync(file, JSON.stringify(original))
  expect(readMove(id)).toEqual(original)
  saveMove({ ...original, tries: 1 })
  expect(JSON.parse(fs.readFileSync(file, "utf8"))).toEqual({ ...original, tries: 1, formatVersion: 1 })
  const future = JSON.stringify({ ...original, formatVersion: 2, ownership: "here" })
  fs.writeFileSync(file, future)
  try {
    expect(() => readMove(id)).toThrow(/Move journal format version 2/)
    expect(() => saveMove(original)).toThrow(/Move journal format version 2/)
    expect(() => forgetMove(id)).toThrow(/Move journal format version 2/)
    expect(fs.readFileSync(file, "utf8")).toBe(future)
  } finally {
    fs.rmSync(file, { force: true })
  }
})

test("future move receipts and routing hints remain intact", () => {
  noteEnded(id, "switched", 1, [session], undefined, peer.key, bundle)
  expect(JSON.parse(fs.readFileSync(decisionFile, "utf8"))).toEqual({ as: "switched", at: 1, ...bundle, peer: peer.key, formatVersion: 1 })
  expect(JSON.parse(fs.readFileSync(decisionHint, "utf8"))).toEqual({ as: "switched", at: 1, sessions: [session], formatVersion: 1 })
  const future = JSON.stringify({ as: "switched", at: 1, formatVersion: 2 })
  fs.writeFileSync(decisionFile, future)
  fs.writeFileSync(decisionHint, future)
  expect(() => endedFor(id, peer.key)).toThrow(/Move ownership receipt format version 2/)
  expect(() => noteEnded(id, "dropped")).toThrow(/Move ownership receipt format version 2/)
  expect(() => compactDecisionHints(Date.now())).toThrow(/Move routing hint format version 2/)
  expect(fs.readFileSync(decisionFile, "utf8")).toBe(future)
  expect(fs.readFileSync(decisionHint, "utf8")).toBe(future)
})

test("a move bundle can be read without a version and refuses a future manifest", async () => {
  const contents = { kind: "group", thing: group, name: "Group", exports: [], wasPaused: false } as const
  const manifest = { ...contents, exports: [] }
  const { file } = await pack(id, manifest)
  const readManifest = spawnSync("tar", ["-xOzf", file, "move.json"], { encoding: "utf8" })
  expect(readManifest.status).toBe(0)
  expect(JSON.parse(readManifest.stdout)).toEqual({ ...manifest, formatVersion: 1 })
  expect((await unpack(id)).contents).toEqual(manifest)

  const fixture = path.join(path.dirname(file), "fixture")
  fs.mkdirSync(fixture)
  for (const formatVersion of [undefined, 4]) {
    fs.writeFileSync(path.join(fixture, "move.json"), JSON.stringify({ ...manifest, formatVersion }))
    expect(spawnSync("tar", ["-czf", file, "-C", fixture, "move.json"]).status).toBe(0)
    if (formatVersion === undefined) expect((await unpack(id)).contents).toEqual(manifest)
    else await expect(unpack(id)).rejects.toThrow(/Move bundle manifest format version 4/)
  }
})

function checkpointed(remotePrimary = false) {
  const box = () => {
    const seed = newKey(), sign = newSigningSeed()
    return { seed: seed.toString("base64url"), sign, peer: {
      key: publicOf(seed).toString("base64url"), address: addressOf(seed).toString("base64url"),
      sign: signingKeyOf(sign), name: "Desk", kind: "mac" as const,
    } }
  }
  const local = box(), remote = box(), primary = remotePrimary ? remote : local
  const setup = newSetupID()
  const link = {
    key: local.seed, sign: local.sign,
    setup: { id: setup, epoch: 1, primary: primary.peer.key, sign: primary.peer.sign,
      role: remotePrimary ? "member" as const : "primary" as const, managed: true as const, checkpointed: true as const },
    sharedSetup: setup, shared: {
      membershipAndAccess: { sequence: 1, value: { boxes: [local.peer, remote.peer], left: [] } },
      ownershipAssignments: { sequence: 1, value: {} as Record<string, unknown> },
    },
  }
  return { link, local, remote }
}

for (const reason of ["capacity", "update"]) test(`assignment refusal for ${reason} ends a staged move before switching`, async () => {
  const { link, local, remote } = checkpointed(reason === "update")
  if (reason === "capacity") link.shared.ownershipAssignments.value = {
    padding: "x".repeat(MAX_SHARED_BYTES - Buffer.byteLength(JSON.stringify(link.shared)) - 15),
  }
  writeLink(link)
  const offered = record({ kind: "group", thing: "grp_capacity", sessions: [], from: local.peer.key,
    to: remote.peer.key, peer: remote.peer.key, stage: "copying", step: "arrive", folder: undefined, offered: true, ...bundle })
  try {
    saveMove(offered)
    await takeMoveMail(remote.peer, { op: "arrived", id, ...bundle })
    expect(await receipt()).toMatchObject({ state: "failed", message: expect.stringContaining(reason === "update" ? "Update" : reason) })
    expect(readMove(id)?.stage).toBe("copying")
    expect(mail.some(each => each.op === "switch")).toBe(false)
    expect(mail.some(each => each.op === "drop")).toBe(true)
    expect(mayActivate({ kind: "groupSettings", id: offered.thing })).toBe(true)
    const drop = mail.find(each => each.op === "drop")!
    saveMove({ ...offered, role: "to", peer: local.peer.key, step: undefined, complete: true, importingGroup: true })
    await takeMoveMail(local.peer, drop)
    expect(readMove(id)).toBeNull()
    expect(mail.at(-1)).toMatchObject({ op: "dropped", neverSwitched: true })
    if (reason === "capacity") {
      const current = readLink()!;
      current.shared!.ownershipAssignments!.value = {};
      writeLink(current);
      const retry = await transferForPeer(local.peer, { body: { setup: link.sharedSetup, transfer: offered } });
      expect(retry).toMatchObject({ status: 422, value: { message: expect.stringContaining("capacity") } });
      expect(sharedValue(readLink(), "ownershipAssignments")).toEqual({});
    }
  } finally { fs.rmSync(LINK_FILE, { force: true }) }
})

test("a restart in switching releases both holds when the primary needs an update", async () => {
  const { link, local, remote } = checkpointed(true)
  const switching = record({ from: local.peer.key, to: remote.peer.key, peer: remote.peer.key, offered: true, ...bundle })
  writeLink(link)
  try {
    saveMove(switching)
    forgetMovesInMemory()
    await resumeMoves()
    expect(fs.existsSync(held)).toBe(true)
    keepMoving()
    // A later message for the same move waits for the upkeep step.
    await takeMoveMail(remote.peer, { op: "decision?", id, ...bundle })
    expect(await receipt()).toMatchObject({
      state: "failed", message: "Update Desk, the main computer, before transferring ownership.",
    })
    expect(fs.existsSync(held)).toBe(false)
    expect(mayActivate({ kind: "conversations", id: session })).toBe(true)
    expect(deletes).toBe(0)
    expect(mail.some(each => each.op === "switch")).toBe(false)
    const drop = mail.find(each => each.op === "drop")!
    expect(drop).toMatchObject(bundle)

    const failed = readMove(id)!
    writeLink({ ...link, key: remote.seed, sign: remote.sign, setup: { ...link.setup, role: "primary" } })
    saveMove({ ...switching, role: "to", peer: local.peer.key, step: undefined, complete: true, createdSessions: [session] })
    await takeMoveMail(local.peer, drop)
    expect(readMove(id)).toBeNull()
    expect(deletes).toBe(1)
    const dropped = mail.at(-1)!
    expect(dropped).toMatchObject({ op: "dropped", neverSwitched: true, ...bundle })
    writeLink(link)
    saveMove(failed)
    await takeMoveMail(remote.peer, dropped)
    expect(readMove(id)).toBeNull()
    expect(await receipt()).toMatchObject({ state: "failed" })
  } finally { fs.rmSync(LINK_FILE, { force: true }) }
})

test("a move waits for the standby acceptance before switching", async () => {
  const { link, local, remote } = checkpointed()
  const standby = { key: remote.peer.key, sign: remote.peer.sign }
  const primary = { ...link, setup: { ...link.setup, standby } }
  grantLease(primary, leaseClock())
  writeLink(primary)
  saveMove(record({ kind: "group", thing: "grp_standby", sessions: [], from: local.peer.key,
    to: remote.peer.key, peer: remote.peer.key, stage: "copying", step: "arrive", folder: undefined, offered: true, ...bundle }))
  const arrived = takeMoveMail(remote.peer, { op: "arrived", id, ...bundle })
  try {
    for (let attempt = 0; attempt < 30 && !pendingShared(readLink(), "ownershipAssignments"); attempt++) await Bun.sleep(10)
    const pending = pendingShared(readLink(), "ownershipAssignments")!
    expect(pending).toBeTruthy()
    expect(readMove(id)?.stage).toBe("copying")
    expect(mail.some(each => each.op === "switch")).toBe(false)
    const witness = { ...structuredClone(primary), key: remote.seed, sign: remote.sign,
      setup: { ...primary.setup, role: "member" as const } }
    const accepted = takeShared(witness, local.peer.key, pending)!
    expect(accepted).toBeTruthy()
    updateLink(current => {
      takeShared(current!, remote.peer.key, readDoc(accepted)!)
      return current
    }, { durable: true })
    await arrived
    expect(readMove(id)).toMatchObject({ stage: "switching", ownershipCommitted: true })
    expect(mail.at(-1)).toMatchObject({ op: "switch", ...bundle })
  } finally {
    await arrived
    fs.rmSync(LINK_FILE, { force: true })
    fs.rmSync(path.join(RUN, "setup-lease.json"), { force: true })
  }
})

test("a restart in switching honors accepted ownership even when the primary needs an update", async () => {
  const { link, local, remote } = checkpointed(true)
  const switching = record({ from: local.peer.key, to: remote.peer.key, peer: remote.peer.key, offered: true, ...bundle })
  const primary = { ...link, key: remote.seed, sign: remote.sign, setup: { ...link.setup, role: "primary" as const } }
  writeLink(primary)
  try {
    expect((await transferForPeer(local.peer, { body: { setup: link.sharedSetup, transfer: switching } })).status).toBe(200)
    writeLink({ ...readLink()!, key: local.seed, sign: local.sign, setup: link.setup })
    saveMove(switching)
    forgetMovesInMemory()
    await resumeMoves()
    keepMoving()
    await takeMoveMail(remote.peer, { op: "decision?", id, ...bundle })
    expect(readMove(id)).toMatchObject({ ownershipCommitted: true })
    expect(acceptedOwner(readLink(), { kind: "conversations", id: session })).toBe(remote.peer.key)
    expect(fs.existsSync(held)).toBe(true)
    expect(mail.some(each => each.op === "drop")).toBe(false)
    expect(mail.at(-1)).toMatchObject({ op: "switch", ...bundle })
  } finally { fs.rmSync(LINK_FILE, { force: true }) }
})

test("startup keeps a group held when accepted ownership points elsewhere", async () => {
  const { link, remote } = checkpointed()
  link.shared.ownershipAssignments.value = { ["group host:" + group]: { computer: remote.peer.key } }
  writeLink(link)
  listedHeld = true
  try {
    await resumeMoves()
    expect(groupActive).toBe(false)
  } finally {
    listedHeld = false
    fs.rmSync(LINK_FILE, { force: true })
  }
})

test("a local handoff updates accepted placement before the receiving queue moves", async () => {
  const { link, local, remote } = checkpointed()
  const id = "wqi_" + "A".repeat(20), address = "queue item:" + id
  const ref = { kind: "queueItems" as const, id, queue: "ses_b" }
  link.shared.ownershipAssignments.value = { [address]: { computer: local.peer.key, queue: "ses_a", move: "wqh_firsthand" } }
  writeLink(link)
  const checkpoint = path.join(K, "folder-version.json")
  const version = fs.existsSync(checkpoint) ? fs.readFileSync(checkpoint) : null
  fs.writeFileSync(checkpoint, '{"version":7}')
  try {
    changeQueues(book => {
      const item = itemOf({ id, title: "Hand this locally" }, "ses_a")!
      book.queue("ses_a").items.push(item)
      handItem(book, item, "ses_b", { user: true }, Date.now())
    })
    handUpkeep(readLink())
    for (let attempt = 0; attempt < 30 && allItems().find(item => item.id === id)?.owner !== "ses_b"; attempt++)
      await Bun.sleep(10)
    expect(allItems().find(item => item.id === id)?.owner).toBe("ses_b")
    expect(sharedValue(readLink(), "ownershipAssignments")).toMatchObject({ [address]: { queue: "ses_b" } })
    settleTransfers(readLink(), Date.now(), hand => !allItems().some(item => item.handing?.id === hand))
    expect(sharedValue(readLink(), "ownershipAssignments")).toEqual({
      [address]: { computer: local.peer.key, queue: "ses_b" },
    })
    const moved = await transferForPeer(local.peer, { body: { setup: link.sharedSetup,
      transfer: { id: "mov_localHand", kind: "chat", from: local.peer.key, to: remote.peer.key, thing: "ses_b", sessions: ["ses_b"] } } })
    expect(moved.status).toBe(200)
    expect(acceptedOwner(readLink(), ref)).toBe(remote.peer.key)
    expect(acceptedOwner(readLink(), { ...ref, queue: "ses_a" })).toBe(remote.peer.key)
    const destination = { ...readLink()!, key: remote.seed }
    expect(mayActivate(ref, { link: destination })).toBe(true)
    expect(mayActivate({ ...ref, queue: "ses_a" }, { link: destination })).toBe(false)
  } finally {
    changeQueues(book => {
      for (const queue of ["ses_a", "ses_b"]) book.queue(queue).items = book.queue(queue).items.filter(item => item.id !== id)
    })
    if (version) fs.writeFileSync(checkpoint, version)
    else fs.rmSync(checkpoint)
    fs.rmSync(LINK_FILE, { force: true })
  }
})

test("a carried session moves with a previously handed-off tombstone", async () => {
  const { link, local, remote } = checkpointed()
  const itemId = "wqi_" + "B".repeat(20)
  const first = "ses_tombstoneFirst", from = "ses_tombstoneFrom", to = "ses_tombstoneTo"
  const who = { user: true } as const
  const checkpoint = path.join(K, "folder-version.json")
  const version = fs.existsSync(checkpoint) ? fs.readFileSync(checkpoint) : null
  fs.writeFileSync(checkpoint, '{"version":7}')
  writeLink(link)
  try {
    changeQueues(book => {
      const item = itemOf({ id: itemId, title: "Hand this before deleting it" }, first)!
      book.queue(first).items.push(item)
      handItem(book, item, from, who, Date.now())
    })
    expect(() => packWorkQueues([from])).toThrow("A Work Queue handoff is still in progress.")
    handUpkeep(readLink())
    for (let attempt = 0; attempt < 30 && allItems().find(item => item.id === itemId)?.owner !== from; attempt++)
      await Bun.sleep(10)
    const assignments = sharedValue(readLink(), "ownershipAssignments")
    expect(assignments).toMatchObject({ ["queue item:" + itemId]: { queue: from, computer: local.peer.key } })
    const removed = { by: who, at: Date.now() }
    changeQueues(book => removeItem(book.find({ id: itemId })!, who, removed.at))

    carryWorkQueue(from, to)
    expect(packWorkQueues([from])).toBeNull()
    const carried = packWorkQueues([to])!
    expect(carried.items).toHaveLength(1)
    expect(carried.items[0]).toMatchObject({ id: itemId, owner: to, removed })
    expect(carried.items[0]!.handing).toBeUndefined()
    expect(sharedValue(readLink(), "ownershipAssignments")).toEqual(assignments)

    // A journal written by the previous carry code may still contain this handoff.
    changeQueues(book => {
      book.find({ id: itemId })!.handing = {
        id: "wqh_staleCarry", key: local.peer.key, to: from, name: from, at: removed.at, by: who, carried: true,
      }
    })
    expect(packWorkQueues([from])).toBeNull()
    const packed = packWorkQueues([to])!
    expect(packed.items[0]!.handing).toBeUndefined()
    const transfer = { id, kind: "chat" as const, from: local.peer.key, to: remote.peer.key, thing: to, sessions: [to] }
    await pack(id, { kind: "chat", thing: to, name: to, exports: [], wasPaused: false, workQueue: packed })
    const { contents, opened } = await unpack(id)
    expect((await transferForPeer(local.peer, { body: { setup: link.sharedSetup, transfer } })).status).toBe(200)
    const ref = { kind: "conversations" as const, id: to }
    expect(acceptedOwner(readLink(), ref)).toBe(remote.peer.key)
    expect(mayActivate(ref)).toBe(false)
    forgetWorkQueues([to])
    const destination = { ...readLink()!, key: remote.seed }
    expect(mayActivate(ref, { link: destination })).toBe(true)
    placeWorkQueues(contents.workQueue, [to], opened)
    expect(packWorkQueues([to])?.items[0]).toMatchObject({ id: itemId, owner: to, removed })
  } finally {
    forgetWorkQueues([first, from, to])
    if (version) fs.writeFileSync(checkpoint, version)
    else fs.rmSync(checkpoint)
    fs.rmSync(LINK_FILE, { force: true })
  }
})

test("settling a transfer compacts its receipt without losing ownership", async () => {
  const { link, local, remote } = checkpointed()
  const move = { id: "mov_compact", kind: "chat", from: local.peer.key, to: remote.peer.key, thing: "ses_compact", sessions: ["ses_compact"] }
  const ref = { kind: "conversations" as const, id: move.thing }
  writeLink(link)
  try {
    const body = { setup: link.sharedSetup, transfer: move }
    expect((await transferForPeer(local.peer, { body })).status).toBe(200)
    expect(sharedValue(readLink(), "ownershipAssignments")).toMatchObject({
      ["session computer:" + move.thing]: { move: move.id },
    })
    expect((await transferForPeer(local.peer, { body: { ...body, settled: true } })).status).toBe(200)
    expect(sharedValue(readLink(), "ownershipAssignments")).toEqual({
      ["session computer:" + move.thing]: { computer: remote.peer.key },
    })
    expect(acceptedOwner(readLink(), ref)).toBe(remote.peer.key)
    expect(mayActivate(ref)).toBe(false)
  } finally { fs.rmSync(LINK_FILE, { force: true }) }
})
