import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { ask, follow } from "./route-answer"
import { Auto } from "../src/agents/auto"
const { engineHandler } = await import("../src/link/server/router")
const { holdVault } = await import("../src/link/vault/holder")
const root = mkdtempSync(join(tmpdir(), "settings-auto-"))
let auto: Auto
let down = false
let signedIn = true
const defaultModels = [{ id: "fast", name: "Fast", providerID: "codex-cli", variants: [{ id: "high" }] }, { id: "other", name: "Other", providerID: "codex-cli", variants: [] }]
let modelCatalog = defaultModels
const { forgetModelLists } = await import("../src/link/work/model-rule")
const server = Bun.serve({ port: 0, hostname: "127.0.0.1", async fetch(request) {
  const url = new URL(request.url), parts = url.pathname.split("/"), id = parts[4]
  if (url.pathname === "/api/model") return Response.json({ data: modelCatalog })
  if (url.pathname === "/api/integration") return Response.json({ data: [{ id: "codex-cli", connections: signedIn ? [{}] : [], metadata: { installed: true, modelsComplete: true } }] })
  if (!url.pathname.startsWith("/api/kuvo/auto")) return Response.json({ data: [] })
  if (request.method === "GET") return Response.json({ data: { lists: auto.all(), limits: auto.limits() } })
  if (request.method === "GET") { if (new URL(request.url).pathname === "/api/kuvo/auto") return Response.json({data:{lists:auto.all(),limits:[]}}); return Response.json({data:[]}) }
  const body = await request.json() as any
  let changed: boolean
  try {
    if (id === "order") changed = auto.reorder(body.ids, body.was)
    else if (request.method === "DELETE") changed = auto.remove(id!, body.revision)
    else changed = auto.change(id ?? body.id, body.revision, { ...(body.name === undefined ? {} : { name: body.name }), ...(body.entries === undefined ? {} : { entries: body.entries }) }, request.method === "POST")
  } catch (error) { return Response.json({ message: String(error) }, { status: 400 }) }
  return Response.json({ data: { lists: auto.all(), limits: [] } }, { status: changed ? 200 : 409 })
} })
const processOfBridge = spawn(process.execPath, ["-e", "setInterval(() => {}, 1e9)", "kuvo-agents"], { stdio: "ignore" })
const handler = engineHandler((() => { throw Error("No engine") }) as any, { name: "test" }, (() => down ? null : { port: server.port, password: "pw", pid: processOfBridge.pid }) as any)
const held = holdVault(handler, () => () => {})
afterAll(() => { held.close(); server.stop(true); processOfBridge.kill(); rmSync(root, { recursive: true, force: true }) })
beforeEach(() => { signedIn = true; modelCatalog = defaultModels; forgetModelLists(); auto = new Auto(root); auto.setLists([{ id: "first", name: "Features", entries: [{ providerID: "codex-cli", modelID: "fast" }] }, { id: "second", name: "Bugs", entries: [] }]); down = false })
const page = async () => (await ask(handler, "GET", "kuvo/settings/custom-auto")).json
const rowOf = (page: any, id: string): any => page.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])])).find((row: any) => row.id === id)
const actionOf = (row: any, id: string) => [...(row.actions ?? []), ...(row.add ? [row.add] : [])].find((action: any) => action.id === id)
const act = (row: any, id: string, values?: unknown) => ask(handler, "POST", `kuvo/settings/custom-auto/${encodeURIComponent(row.id)}/${id}`, { was: actionOf(row,id).form?.revision ?? row.revision, ...(values === undefined ? {} : { values }) })
test("Custom Auto lists, ordered models, exact Swift words and ordinary forms", async () => {
 const shown = await page()
 if (process.env.AUTO_SETTINGS_FIXTURE) writeFileSync(process.env.AUTO_SETTINGS_FIXTURE, JSON.stringify(shown, null, 2) + "\n")
 expect(shown.title).toBe("Custom Auto")
 expect(shown.groups[0].note).toBe("A model at its limit is skipped until the limit resets; an account at its limit skips all its models.")
 expect(rowOf(shown,"models-second").empty).toBe("Add the models to go through, in the order you want them. Pick this list in a session's model picker to use it.")
 expect(rowOf(shown,"lists").items.map((row:any)=>row.title)).toEqual(["Features","Bugs"])
 expect(actionOf(rowOf(shown,"lists"),"add").form.fields[0]).toMatchObject({title:"Name",line:"Its name, as the model picker shows it"})
 expect(rowOf(shown,"list-first").actions.map((action:any)=>action.title)).toEqual(["Edit…","Move Down","Delete List"])
})
test("name check and refused submit have the same words; wrong types never write", async () => {
 const row = rowOf(await page(),"lists"), form = actionOf(row,"add").form
 const checked = await ask(handler,"POST","kuvo/settings/custom-auto/lists/add/check",{was:form.revision,values:{name:42}})
 const refused = await act(row,"add",{name:42})
 expect(checked.json.ready).toBe(false); expect(refused.status).toBe(400); expect(refused.json.fields).toEqual(checked.json.fields); expect(auto.all().length).toBe(2)
})
test("add and rename; unrelated list changes do not stale a form; changed form returns 409", async () => {
 const original = rowOf(await page(),"list-first")
 expect((await act(rowOf(await page(),"lists"),"add",{name:"More"})).status).toBe(200)
 expect((await act(original,"edit",{name:"Build"})).status).toBe(200)
 const stale = await act(original,"edit",{name:"Old"}); expect(stale.status).toBe(409); expect(stale.json.page.title).toBe("Custom Auto")
})
test("ordered model edits and list reordering use the existing route", async () => {
 let shown = await page()
 expect((await act(rowOf(shown,"models-first"),"add-model",{model:JSON.stringify(["codex-cli","other"]),variant:""})).status).toBe(200)
 shown = await page(); expect((await act(rowOf(shown,"entry-first-1"),"up")).status).toBe(200)
 expect(auto.all()[0]!.entries.map(entry=>entry.modelID)).toEqual(["other","fast"])
 expect((await act(rowOf(await page(),"list-second"),"up")).status).toBe(200)
 expect(auto.all().map(list=>list.id)).toEqual(["second","first"])
})
test("can.why returns 403 with storage untouched", async () => {
 auto.setLists([{id:"first",name:"Only",entries:[]}]); const before = auto.all()
 const refused = await act(rowOf(await page(),"list-first"),"remove")
 expect(refused.status).toBe(403); expect(auto.all()).toEqual(before)
})
test("unavailable source has no invented defaults", async () => {
 down = true; const shown = await page(); expect(shown.groups).toEqual([]); expect(shown.notes[0].tone).toBe("warning")
})

test("external changes and every mutation route update the events page", async () => {
 const stream = follow(handler,"kuvo/settings/custom-auto/events")
 try {
   await stream.next()
   const changed = stream.next()
   expect((await act(rowOf(await page(),"lists"),"add",{name:"Live"})).status).toBe(200)
   expect(rowOf(await changed,"lists").items.map((row:any)=>row.title)).toContain("Live")
   const external = stream.next()
   auto.change("second", auto.all()[1]!.revision, {name:"Outside"})
   expect(rowOf(await external,"list-second").title).toBe("Outside")
 } finally { stream.stop() }
})

test("model forms have bounded fields and unsupported efforts never write", async () => {
 const row = rowOf(await page(), "models-first"), form = actionOf(row, "add-model").form
 expect(form.fields.map((field:any) => field.id)).toEqual(["model", "variant"])
 const before = auto.all()
 const values = {model:JSON.stringify(["codex-cli","other"]),variant:"high"}
 const checked = await ask(handler,"POST","kuvo/settings/custom-auto/models-first/add-model/check",{values})
 const refused = await act(row,"add-model",values)
 expect(checked.json.ready).toBe(false)
 expect(refused.status).toBe(400)
 expect(refused.json.fields).toEqual(checked.json.fields)
 expect(auto.all()).toEqual(before)
})

test("first and last entries omit impossible moves", async () => {
 const shown = await page()
 expect(rowOf(shown, "list-first").actions.some((action:any) => action.id === "up")).toBe(false)
 expect(rowOf(shown, "list-second").actions.some((action:any) => action.id === "down")).toBe(false)
 expect(rowOf(shown, "entry-first-0").actions.some((action:any) => ["up", "down"].includes(action.id))).toBe(false)
 expect(JSON.stringify(shown)).not.toContain("Already first.")
 expect(JSON.stringify(shown)).not.toContain("Already last.")
})

test("a large catalog keeps two fields per form and only the edited model's efforts", async () => {
 modelCatalog = Array.from({length: 1000}, (_, index) => ({ id: `model-${index}`, name: `Model ${index}`, providerID: "codex-cli", variants: [{id: `effort-${index}`}] }))
 auto.setLists([{id:"first",name:"Large",entries:[{providerID:"codex-cli",modelID:"model-0"},{providerID:"codex-cli",modelID:"model-1"}]}])
 forgetModelLists()
 const shown = await page()
 expect(actionOf(rowOf(shown, "models-first"), "add-model").form.fields.length).toBe(2)
 for (const index of [0,1]) {
   const form = actionOf(rowOf(shown, `entry-first-${index}`), "edit-model").form
   expect(form.fields.length).toBe(2)
   expect(form.fields.find((field:any) => field.id === "variant")).toMatchObject({kind:"text",line:`Supported efforts: effort-${index}. Leave empty for the default.`})
 }
})

test("signed-out accounts say why Custom Auto passes their models by", async () => {
 signedIn = false
 forgetModelLists()
 expect(rowOf(await page(), "entry-first-0").notice.words).toBe("Not signed in: Custom Auto passes it by")
})

test("limit resets use the computer's configured zone and include a date only outside today", async () => {
 const { autoResetWords } = await import("../src/link/areas/settings-custom-auto")
 const { setZone, ZONE_FILE } = await import("../src/link/zone/setting")
 const { forgetZoneRead } = await import("../src/shared/zone")
 let previous: string | undefined
 try { previous = readFileSync(ZONE_FILE, "utf8") } catch {}
 try {
   setZone("Europe/Istanbul")
   const now = Date.parse("2026-11-16T08:00:00Z")
   expect(autoResetWords(Date.parse("2026-11-16T12:45:00Z"), true, now)).toBe("until 3:45 PM")
   expect(autoResetWords(Date.parse("2026-11-17T12:45:00Z"), false, now)).toContain("Nov 17, 2026")
   expect(autoResetWords(Date.parse("2026-11-17T12:45:00Z"), false, now)).toContain("3:45 PM")
   expect(autoResetWords(Date.parse("2026-11-16T22:00:00Z"), true, now)).toContain("Nov 17, 2026")
 } finally {
   if (previous === undefined) rmSync(ZONE_FILE, {force:true}); else writeFileSync(ZONE_FILE, previous)
   forgetZoneRead()
 }
})


test("editing a model and its different supported effort uses a single editable field", async () => {
 modelCatalog = defaultModels.map(model => model.id === "other" ? {...model, variants:[{id:"low"}]} : model)
 auto.change("first", auto.all()[0]!.revision, {entries:[{providerID:"codex-cli",modelID:"fast",variant:"high"}]})
 forgetModelLists()
 const entry = rowOf(await page(), "entry-first-0"), form = actionOf(entry,"edit-model").form
 expect(form.fields.find((field:any) => field.id === "variant")).toMatchObject({kind:"text",value:"high"})
 const values = {model:JSON.stringify(["codex-cli","other"]),variant:"low"}
 const checked = await ask(handler,"POST","kuvo/settings/custom-auto/entry-first-0/edit-model/check",{values})
 expect(checked.json.ready).toBe(true)
 expect(checked.json.fields.find((field:any) => field.id === "variant").words).toBe("Supported efforts: low. Leave empty for the default.")
 expect((await act(entry,"edit-model",values)).status).toBe(200)
 expect(auto.all()[0]!.entries).toEqual([{providerID:"codex-cli",modelID:"other",variant:"low"}])
})
