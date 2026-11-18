import "./safety"
import { afterAll, expect, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"
import { ask } from "./route-answer"

const { BOTS } = await import("../src/link/home")
const { engineHandler } = await import("../src/link/server/router")
const { MOVES, forgetMove, saveMove, waitingFor } = await import("../src/link/moves/records")
const { peerIsAway } = await import("../src/link/peers/reach")

const noEngine = () => { throw new Error("No engine") }
const handler = engineHandler(noEngine, { name: "Studio" }, noEngine)
const id = "mov_DesktopReceipt"
const route = `kuvo/moves/receipt?id=${id}`
const outcome = path.join(MOVES, `${id}.outcome.json`)
const move = {
  id, kind: "bot" as const, thing: "bot_mira", name: "Mira", from: "studio",
  to: "lab", role: "from" as const, stage: "copying" as const, sessions: ["ses_mira"], sent: 20, size: 100,
}

afterAll(() => {
  waitingFor.delete(id)
  forgetMove(id)
  fs.rmSync(outcome, { force: true })
})

test("desktop move receipts carry source progress, waiting and terminal words", async () => {
  saveMove({ ...move, peer: "lab", wasPaused: false, tries: 0, started: Date.now() })
  const moving = await ask(handler, "GET", route)
  expect(moving.json).toMatchObject({ state: "moving", move: { ...move, progress: "Moving to the other computer…" } })
  const list = await ask(handler, "GET", "kuvo/moves")
  expect(list.json.moves.find((each: { id: string }) => each.id === id)).toEqual(moving.json.move)

  waitingFor.add(id)
  const waiting = await ask(handler, "GET", route)
  expect(waiting.json).toMatchObject({ state: "moving", move: { waiting: true, progress: "Waiting for the other computer…" } })

  forgetMove(id)
  fs.writeFileSync(outcome, JSON.stringify({ ok: true, move, note: "Mira uses the account on Lab." }))
  const done = await ask(handler, "GET", route)
  expect(done.json).toMatchObject({ state: "done", move, message: "Moved to the other computer.", note: "Mira uses the account on Lab." })
  fs.writeFileSync(outcome, JSON.stringify({ ok: false, status: 409, message: "The engine on Lab isn’t running. Try again once it is." }))
  const failed = await ask(handler, "GET", route)
  expect(failed.json).toEqual({ state: "failed", message: "The engine on Lab isn’t running. Try again once it is." })
  expect((await ask(handler, "GET", "kuvo/moves/receipt?id=..%2Flink")).status).toBe(404)
})

test("move choices use the same away facts as the computers list", () => {
  const now = 1_000_000_000
  expect(peerIsAway("lab", now, { lab: now }, now)).toBe(true)
  expect(peerIsAway("lab", now - 31 * 60_000, {}, now)).toBe(true)
  expect(peerIsAway("lab", now - 60_000, {}, now)).toBe(false)
  expect(peerIsAway("lab", undefined, {}, now)).toBe(false)
})


test("a bot appears on the destination list once the held copy is activated", async () => {
  const bot = "bot_DesktopIncoming"
  const moveId = "mov_DesktopIncoming"
  const folder = path.join(BOTS, bot)
  fs.mkdirSync(folder, { recursive: true })
  fs.writeFileSync(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Mira", look: { color: "graphite", shape: "round", face: "calm" } }))
  fs.writeFileSync(path.join(folder, "session"), "ses_incoming")
  try {
    saveMove({ ...move, id: moveId, thing: bot, role: "to", peer: "studio", wasPaused: false, tries: 0, started: Date.now() })
    const listed = async () => (await ask(handler, "GET", "kuvo/bots")).json.bots.some((each: { id: string }) => each.id === bot)
    expect(await listed()).toBe(false)
    forgetMove(moveId)
    expect(await listed()).toBe(true)
  } finally {
    forgetMove(moveId)
    fs.rmSync(folder, { recursive: true, force: true })
  }
})
