import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { mkdtempSync, realpathSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { spawn } from "node:child_process"
import { ask, follow } from "./route-answer"
import { ArtifactStore } from "../src/agents/artifacts/store"
import { artifactRoutes } from "../src/agents/artifacts/routes"

const { engineHandler } = await import("../src/link/server/router")
const root = realpathSync(mkdtempSync(join(tmpdir(), "settings-storage-")))
let store: ArtifactStore
let disk = { free: 500 * 1024 ** 3, total: 1000 * 1024 ** 3 }
let serial = 0
const processOfBridge = spawn(process.execPath, ["-e", "setInterval(() => {}, 1e9)", "kuvo-agents"], { stdio: "ignore" })
const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch(request) {
  const url = new URL(request.url)
  return artifactRoutes(store, request.method, url.pathname.split("/artifacts")[1]!.split("/").filter(Boolean), url.searchParams, () => request.json())
} })
const noEngine = () => { throw new Error("No engine") }
const handler = engineHandler(noEngine as any, { name: "test" }, () => ({ port: server.port, password: "test", pid: processOfBridge.pid }) as any)
afterAll(() => { server.stop(true); processOfBridge.kill(); rmSync(root, { recursive: true, force: true }) })
beforeEach(() => {
  disk = { free: 500 * 1024 ** 3, total: 1000 * 1024 ** 3 }
  store = new ArtifactStore(join(root, `store-${++serial}`), { budget: 1024 ** 3, disk: () => disk })
})
const shown = async () => (await ask(handler, "GET", "kuvo/settings/storage")).json
const row = (page: any, id: string) => page.groups.flatMap((g: any) => g.rows).find((r: any) => r.id === id)
const change = (at: any, value: unknown) => ask(handler, "PATCH", "kuvo/settings/storage", { row: at.id, was: at.revision, value })
const older = async () => {
  const file = join(root, "made.txt")
  writeFileSync(file, "first version\n")
  await store.change(file, { by: { kind: "agent" }, via: "test", turn: { session: "ses_test", message: "first" } })
  writeFileSync(file, "second version\n")
  await store.change(file, { by: { kind: "agent" }, via: "test", turn: { session: "ses_test", message: "second" } })
  expect(store.storage().older).toBeGreaterThan(0)
}

test("Storage preserves the Swift box group's words, budgets, units and confirmations", async () => {
  const page = await shown()
  expect(page.groups[0].title).toBe("What Agents Made")
  expect(page.groups[0].rows.map((r: any) => [r.id, r.title, r.line])).toEqual([
    ["copies", "Copies of what agents made", "Each version of the files agents write or change, so you can see what changed and put one back. Never what looks like a secret, and only the changes for code git keeps."],
    ["older-budget", "Older versions keep up to", "Past it, the oldest go first. The newest version of each file, memory, skills and what’s kept always stay."],
    ["older-copies", "Older versions", "None kept."],
  ])
  const budget = row(page, "older-budget")
  expect([budget.kind, budget.unit, budget.min]).toEqual(["number", "bytes", 0])
  expect(budget.suggestions.map((s: any) => s.value)).toEqual([256, 512, 1024, 2048, 5120, 10240].map(n => n * 1024 ** 2))
  expect(budget.confirmBelow.confirm.title).toBe("Keep less of older versions?")
  expect(budget.confirmBelow.confirm.body).toBe("Older versions take 0 bytes now, so the oldest of their copies go until they fit. The newest version of each file, memory, skills and what’s kept stay.")
  expect(row(page, "older-copies").actions[0]).toMatchObject({ title: "Clear…", confirm: { title: "Clear older versions?", body: "Their copies go; each stays listed, without one. The newest version of each file, memory, skills and what’s kept stay.", button: "Clear" } })
  const { revised } = await import("../src/link/areas/settings-pages")
  expect(revised({ ...budget, line: "Reworded" }).revision).toBe(budget.revision)
})

test("Storage floors budget writes, refuses stale and wrong types; disk changes do not conflict", async () => {
  const budget = row(await shown(), "older-budget")
  disk.free -= 100
  expect((await change(budget, 1000.9)).status).toBe(200)
  expect(store.storage().budget).toBe(1000)
  expect((await change(budget, 2000)).status).toBe(409)
  expect((await change(row(await shown(), "older-budget"), "1000")).status).toBe(400)
})

test("Storage route optional was is atomic; calls without it still work", async () => {
  expect((await ask(handler, "POST", "kuvo/artifacts/storage", { budget: 3000 })).status).toBe(200)
  const file = join(root, `store-${serial}`, "settings.json")
  const before = readFileSync(file, "utf8")
  expect((await ask(handler, "POST", "kuvo/artifacts/storage", { budget: 4000, was: 2000 })).status).toBe(409)
  expect(readFileSync(file, "utf8")).toBe(before)
  const answers = await Promise.all([5000, 6000].map(budget => ask(handler, "POST", "kuvo/artifacts/storage", { budget, was: 3000 })))
  expect(answers.map(a => a.status).sort()).toEqual([200, 409])
})

test("Storage refuses an unavailable Clear with its reason and touches no settings", async () => {
  store.setBudget(1024)
  const file = join(root, `store-${serial}`, "settings.json")
  const contents = readFileSync(file, "utf8")
  const before = store.storage()
  const at = row(await shown(), "older-copies")
  expect(at.actions[0].can.why).toBe("None kept.")
  const answer = await ask(handler, "POST", "kuvo/settings/storage/older-copies/clear", { was: at.revision })
  expect([answer.status, answer.json.message]).toEqual([403, "None kept."])
  expect(store.storage()).toEqual(before)
  expect(readFileSync(file, "utf8")).toBe(contents)
})

test("Storage events follow the page, old budget route and clear route", async () => {
  const events = follow(handler, "kuvo/settings/storage/events")
  try {
    let page = await events.next()
    expect((await change(row(page, "older-budget"), 2000)).status).toBe(200)
    expect(row(await events.next(), "older-budget").value).toBe(2000)
    await ask(handler, "POST", "kuvo/artifacts/storage", { budget: 3000 })
    expect(row(await events.next(), "older-budget").value).toBe(3000)
    await older()
    page = await events.next()
    const bytes = row(page, "older-copies").value
    expect((await ask(handler, "POST", "kuvo/artifacts/storage/clear", { was: bytes + 1 })).status).toBe(409)
    expect(store.storage().older).toBe(bytes)
    await ask(handler, "POST", "kuvo/artifacts/storage/clear", {})
    expect(row(await events.next(), "older-copies").value).toBe(0)
  } finally { events.stop() }
})

test("Storage does not invent rows when the bridge is unavailable", async () => {
  const offline = engineHandler(noEngine as any, { name: "test" }, noEngine as any)
  const page = (await ask(offline, "GET", "kuvo/settings/storage")).json
  expect(page.groups).toEqual([])
  expect(page.notes[0].tone).toBe("warning")
})
