import "./safety"
import confirmationVectors from "./fixtures/settings-confirmation.json"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { mkdtempSync, realpathSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { spawn } from "node:child_process"
import { ask, follow } from "./route-answer"
import { ArtifactStore } from "../src/link/artifacts/store"
import { artifactRoutes } from "../src/link/artifacts/routes"

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

test("Storage describes decimal units and suggestions while preserving words and confirmations", async () => {
  const page = await shown()
  const made = page.groups.find((g: any) => g.id === "agents-made")
  expect(made.title).toBe("What Agents Made")
  expect(made.rows.map((r: any) => [r.id, r.title, r.line])).toEqual([
    ["copies", "Copies of what agents made", "Each version of the files agents write or change, so you can see what changed and put one back. Never what looks like a secret, and only the changes for code git keeps."],
    ["older-budget", "Older versions keep up to", "Past it, the oldest go first. The newest version of each file, memory, skills and what’s kept always stay."],
    ["older-copies", "Older versions", "None kept."],
  ])
  const budget = row(page, "older-budget")
  expect([budget.kind, budget.unit, budget.min]).toEqual(["number", { name: "GB", factor: 1_000_000_000, decimals: 2, rounding: "nearest" }, 0])
  expect(budget.suggestions.map((s: any) => s.value)).toEqual([0.25, 0.5, 1, 2, 5, 10].map(n => n * 1_000_000_000))
  expect(budget.confirmBelow.confirm.title).toBe("Keep less of older versions?")
  expect(budget.confirmBelow.confirm.body).toBe("Older versions take 0 bytes now, so the oldest of their copies go until they fit. The newest version of each file, memory, skills and what’s kept stay.")
  expect(row(page, "older-copies").actions[0]).toMatchObject({ title: "Clear…", confirm: { title: "Clear older versions?", body: "Their copies go; each stays listed, without one. The newest version of each file, memory, skills and what’s kept stay.", button: "Clear" } })
  const { revised } = await import("../src/link/areas/settings-pages")
  expect(revised({ ...budget, line: "Reworded" }).revision).toBe(budget.revision)
  expect(revised({ ...budget, confirmBelow: { ...budget.confirmBelow, confirm: { ...budget.confirmBelow.confirm, body: "A changed warning" } } }).revision).not.toBe(budget.revision)
  disk.free = 0
  expect((await shown()).notes[0].words).toBe("The disk is nearly full, so only the newest version of each file keeps a copy until 10.7 GB is free again.")
})

test("Storage floors budget writes, refuses stale and wrong types; disk changes do not conflict", async () => {
  const budget = row(await shown(), "older-budget")
  const file = join(root, "new-copy.txt")
  writeFileSync(file, "A new copy")
  await store.change(file, { by: { kind: "outside" }, via: "outside" })
  expect(row(await shown(), "copies").value).toBeGreaterThan(0)
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
  // (Measured first, so the measure coming in isn't one of the changes followed.)
  await (await import("../src/link/areas/storage-sizes")).measureSizes()
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

test("Storage does not invent what agents made when the bridge is unavailable, and shows the rest", async () => {
  const offline = engineHandler(noEngine as any, { name: "test" }, noEngine as any)
  const page = (await ask(offline, "GET", "kuvo/settings/storage")).json
  expect(page.groups.map((g: any) => g.id)).toEqual(["computer", "folders", "manage"])
  expect(page.notes[0].tone).toBe("warning")
  expect(page.notes[0].words).toStartWith("What agents made couldn’t be read")
})


test("Storage requires explicit confirmation below the threshold before deleting copies", async () => {
  await older()
  const at = row(await shown(), "older-budget")
  const before = store.storage()
  for (const confirmed of [undefined, ...confirmationVectors.cases.filter(each => each.status === 400).map(each => each.confirmed)]) {
    const answer = await ask(handler, "PATCH", "kuvo/settings/storage", { row: at.id, was: at.revision, value: 0, confirmed })
    expect(answer.status).toBe(400)
    if (confirmed === undefined || confirmed === false) expect(answer.json.message).toBe(at.confirmBelow.confirm.body)
    else expect(typeof answer.json.message).toBe("string")
    expect(store.storage()).toEqual(before)
  }
  expect((await change(at, at.confirmBelow.value + 100)).status).toBe(200)
  expect((await change(row(await shown(), "older-budget"), at.confirmBelow.value)).status).toBe(200)
  expect(store.storage().older).toBe(before.older)
  const current = row(await shown(), "older-budget")
  expect((await ask(handler, "PATCH", "kuvo/settings/storage", { row: current.id, was: current.revision, value: 0, confirmed: true })).status).toBe(200)
  expect(store.storage().older).toBe(0)
})


test("Storage atomically refuses budget changes when the warning's older count changed", async () => {
  const read = store.storage()
  await older()
  const before = store.storage()
  const answer = await ask(handler, "POST", "kuvo/artifacts/storage", { budget: 0, was: read.budget, wasOlder: read.older })
  expect(answer.status).toBe(409)
  expect(store.storage()).toEqual(before)
})

test("The artifact storage route requires confirmation before any shrinking write deletes copies", async () => {
  await older()
  const before = store.storage()
  for (const confirmed of [undefined, null, false, "true", 1]) {
    const answer = await ask(handler, "POST", "kuvo/artifacts/storage", { budget: 0, confirmed })
    expect(answer.status).toBe(400)
    expect(answer.json.message).toBe(row(await shown(), "older-budget").confirmBelow.confirm.body)
    expect(store.storage()).toEqual(before)
  }
  expect((await ask(handler, "POST", "kuvo/artifacts/storage", { budget: before.older })).status).toBe(200)
  expect(store.storage().older).toBe(before.older)
  expect((await ask(handler, "POST", "kuvo/artifacts/storage", { budget: 0, confirmed: true })).status).toBe(200)
  expect(store.storage().older).toBe(0)
})

test("The store itself refuses an unconfirmed shrink before writing settings or deleting copies", async () => {
  store.setBudget(1_000_000_000)
  const settings = join(root, `store-${serial}`, "settings.json")
  const saved = readFileSync(settings, "utf8")
  await older()
  const before = store.storage()
  expect(store.setBudget(0)).toBe(row(await shown(), "older-budget").confirmBelow.confirm.body)
  expect(store.storage()).toEqual(before)
  expect(readFileSync(settings, "utf8")).toBe(saved)
  expect(store.setBudget(0, true)).toBeNull()
  expect(store.storage().older).toBe(0)
})


test("New storage budgets preserve the existing binary default and use the same base as the box's unit and suggestions", () => {
  const fresh = new ArtifactStore(join(root, `default-${serial}`), { disk: () => disk })
  expect(fresh.storage().budget).toBe(1024 ** 3)
})

 test("The binary default displays as 1.07 GB and saves untouched with exactly its original bytes", async () => {
  store = new ArtifactStore(join(root, `untouched-${serial}`), { disk: () => disk })
  const budget = row(await shown(), "older-budget")
  expect(budget.value).toBe(1_073_741_824)
  expect(new Intl.NumberFormat("en", { maximumFractionDigits: budget.unit.decimals }).format(budget.value / budget.unit.factor)).toBe("1.07")
  expect(budget.unit.name).toBe("GB")
  expect((await change(budget, budget.value)).status).toBe(200)
  expect(store.storage().budget).toBe(1_073_741_824)
  store.setBudget(4_000_000)
  const small = row(await shown(), "older-budget")
  expect(small.unit).toEqual({ name: "MB", factor: 1_000_000, decimals: 2, rounding: "nearest" })
  expect(small.value / small.unit.factor).toBe(4)
  expect((await change(small, small.value)).status).toBe(200)
  expect(store.storage().budget).toBe(4_000_000)
})

test("Storage measures what Kuvo keeps: the whole as parts, Kuvo's folder, Refresh, and the pages that clear the rest", async () => {
  const { measureSizes, SIZES_FILE } = await import("../src/link/areas/storage-sizes")
  const { K, ENGINE_DATA } = await import("../src/link/home")
  // (Any measure begun by an earlier read finished first, so this one starts from none.)
  await measureSizes()
  rmSync(SIZES_FILE, { force: true })
  const { mkdirSync } = await import("node:fs")
  mkdirSync(join(ENGINE_DATA, "worktree", "tree"), { recursive: true })
  writeFileSync(join(ENGINE_DATA, "worktree", "tree", "big"), Buffer.alloc(300_000))
  mkdirSync(join(K, "browser"), { recursive: true })
  mkdirSync(join(K, "browser", "profile"), { recursive: true })
  writeFileSync(join(K, "browser", "profile", "storage-test-blob"), Buffer.alloc(100_000))
  mkdirSync(join(K, "engine"), { recursive: true })
  writeFileSync(join(K, "engine", "opencode"), Buffer.alloc(200_000))
  // As it's first read: measuring, said, with no parts yet; the page follows once it's in.
  let page = await shown()
  let used = row(page, "used")
  expect([used.kind, used.title, used.busy, used.parts]).toEqual(["meter", "Measuring what Kuvo keeps…", "Measuring…", undefined])
  // (Asked as the app beside its box asks: "This Mac", or "This Computer" on Linux.)
  const mac = process.platform === "darwin"
  expect(page.groups[0]).toMatchObject({ id: "computer", title: mac ? "This Mac" : "This Computer", note: `What Kuvo keeps on ${mac ? "this Mac" : "this computer"}.` })
  await measureSizes()
  page = await shown()
  used = row(page, "used")
  expect(used.title).toMatch(/^[\d.]+ (KB|MB) used by Kuvo$/)
  expect(used.busy).toBeUndefined()
  expect(used.actions.map((a: any) => [a.id, a.title])).toEqual([["refresh", "Refresh"]])
  const part = (id: string) => used.parts.find((p: any) => p.id === id)
  expect(used.parts.map((p: any) => p.id)).toEqual(["sessions", "worktrees", "made", "browser", "other"])
  expect(part("worktrees").value).toBeGreaterThanOrEqual(300_000)
  expect(part("browser").value).toBeGreaterThanOrEqual(100_000)
  // (Worktrees in the engine's data are counted once, as worktrees, not as sessions too.)
  expect(part("sessions").value).toBeLessThan(300_000)
  expect(part("other").parts.find((p: any) => p.title === "The engine").value).toBeGreaterThanOrEqual(200_000)
  expect(used.parts.every((p: any) => typeof p.words === "string")).toBe(true)
  // Kuvo's folder with its size; Show in Finder only to the app beside a Mac's box.
  const { IS_MAC } = await import("../src/link/home")
  const home = row(page, "home-folder")
  expect([home.title, home.kind, home.value]).toEqual(["Kuvo’s folder", IS_MAC ? "actions" : "status", (await import("../src/link/areas/storage-sizes")).readSizes()!.home])
  if (IS_MAC) expect(home.actions[0]).toMatchObject({ title: "Show in Finder", opens: `file://${encodeURI(K)}` })
  expect(row(page, "manage-worktrees")).toMatchObject({ kind: "open", section: "worktrees" })
  expect(row(page, "manage-archived")).toMatchObject({ kind: "open", section: "archived" })
  // Refresh measures again: answered at once, the page saying it's measuring.
  writeFileSync(join(K, "browser", "more"), Buffer.alloc(400_000))
  const refreshed = await ask(handler, "POST", "kuvo/settings/storage/used/refresh", { was: used.revision })
  expect(refreshed.status).toBe(200)
  expect(row(refreshed.json, "used").busy).toBe("Measuring…")
  await measureSizes()
  expect(row(await shown(), "used").parts.find((p: any) => p.id === "browser").value).toBeGreaterThanOrEqual(500_000)
})

test("Storage counts a worktree kept outside Kuvo's folder by itself, never the rest of the folder it's in", async () => {
  const { measureSizes, SIZES_FILE } = await import("../src/link/areas/storage-sizes")
  const { WORKTREES_FILE } = await import("../src/link/areas/worktree-records")
  const { mkdirSync } = await import("node:fs")
  const projects = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-projects-")))
  try {
    mkdirSync(join(projects, "tree"), { recursive: true })
    writeFileSync(join(projects, "tree", "work"), Buffer.alloc(200_000))
    // Your own project beside it, in the same folder: not Kuvo's.
    mkdirSync(join(projects, "mine"), { recursive: true })
    writeFileSync(join(projects, "mine", "big"), Buffer.alloc(3_000_000))
    mkdirSync(join(WORKTREES_FILE, ".."), { recursive: true })
    writeFileSync(WORKTREES_FILE, JSON.stringify({ worktrees: [{ root: join(projects, "tree"), project: projects, branch: "topic" }] }))
    await measureSizes()
    rmSync(SIZES_FILE, { force: true })
    await measureSizes(true)
    const page = await shown()
    const outside = row(page, "worktree-folder-0")
    expect([outside.title, outside.line]).toEqual(["1 worktree in", projects])
    expect(outside.value).toBeGreaterThanOrEqual(200_000)
    expect(outside.value).toBeLessThan(1_000_000)
    expect(row(page, "used").parts.find((p: any) => p.id === "worktrees").value).toBeLessThan(3_000_000 + 1_000_000)
  } finally {
    rmSync(WORKTREES_FILE, { force: true })
    rmSync(projects, { recursive: true, force: true })
  }
})

test("Storage tries a measure that failed again only when it's old, or Refresh is pressed", async () => {
  const { measureSizes, measureProblem, readSizes, SIZES_FILE } = await import("../src/link/areas/storage-sizes")
  const { K } = await import("../src/link/home")
  await measureSizes()
  rmSync(SIZES_FILE, { force: true })
  let asked = 0
  await measureSizes(true, K, () => { asked += 1; throw new Error("The records couldn't be read.") })
  expect([asked, measureProblem()]).toEqual([1, "The records couldn't be read."])
  // Read again (the page, each second): not begun again, and the problem stays said.
  await measureSizes(false, K, () => { asked += 1; return [] })
  expect([asked, readSizes()]).toEqual([1, null])
  expect((await shown()).groups[0].rows[0].line).toBe("It couldn’t be measured just now: The records couldn't be read.")
  // Refresh begins it again.
  await measureSizes(true, K, () => { asked += 1; return [] })
  expect([asked, measureProblem(), readSizes() !== null]).toEqual([2, null, true])
})
