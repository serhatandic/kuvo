import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { dirname } from "node:path"
import { ask, follow } from "./route-answer"
const { engineHandler } = await import("../src/link/server/router")
const { CONFIG_FILE } = await import("../src/link/areas/config")
const { holdVault } = await import("../src/link/vault/holder")
const handler = engineHandler((() => { throw Error("offline") }) as any, { name: "test" }, (() => null) as any)
const held = holdVault(handler, () => () => {})
afterAll(() => held.close())
const save = (value: unknown) => { mkdirSync(dirname(CONFIG_FILE), { recursive: true }); writeFileSync(CONFIG_FILE, JSON.stringify(value)) }
const config = () => JSON.parse(readFileSync(CONFIG_FILE, "utf8"))
const page = async () => (await ask(handler, "GET", "kuvo/settings/commands")).json
const row = (page: any, id: string) => page.groups.flatMap((g: any) => g.rows).flatMap((r: any) => [r, ...(r.items ?? [])]).find((r: any) => r.id === id)
const values = { name: "Test command", description: "Tests", agent: "", model: "", subagent: false, template: "Test $ARGUMENTS" }
const revision = (r: any, id: string) => (r.actions ?? [r.add]).find((a: any) => a.id === id).form?.revision ?? r.revision
beforeEach(() => save({ commands: { test: { template: "Run tests", description: "Test it" }, review: { template: "Review" } } }))
test("Commands preserves the Swift page's labels, prompts and notes", async () => {
  const p = await page()
  expect(p.title).toBe("Commands")
  expect(p.groups[0].title).toBe("Your Commands")
  expect(p.groups[0].note).toBe("Type / and a command's name at the start of a message to run it. What you type after the name fills in $ARGUMENTS.")
  const list = row(p, "commands")
  expect(list.add.title).toBe("New Command…")
  const f = row(p, "command-test").actions[0].form
  expect(f.fields.map((f: any) => f.title)).toEqual(["Description", "Agent", "Model", "Run as a subagent", "Prompt"])
  expect(f.fields[3].line).toBe("In the background, without filling up the conversation.")
  expect(f.fields[4].line).toBe("What the agent gets. $ARGUMENTS is what you type after the command; $1, $2 and so on are its words. !`command` puts in a command's output.")
})
test("name check and submit share words; check changes nothing; create and remove", async () => {
  const p = await page(), r = row(p, "commands"), was = revision(r, "add")
  const checked = await ask(handler, "POST", "kuvo/settings/commands/commands/add/check", { was, values })
  expect(checked.json).toMatchObject({ ready: true, fields: [{ id: "name", words: "It'll be called /Test-command." }] })
  expect(config().commands["Test-command"]).toBeUndefined()
  const made = await ask(handler, "POST", "kuvo/settings/commands/commands/add", { was, values })
  expect(made.status).toBe(200)
  const added = row(made.json, "command-Test-command")
  expect(config().commands["Test-command"].template).toBe(values.template)
  expect((await ask(handler, "POST", "kuvo/settings/commands/command-Test-command/remove", { was: added.revision })).status).toBe(200)
  const duplicate = { ...values, name: "test" }
  const check = await ask(handler, "POST", "kuvo/settings/commands/commands/add/check", { values: duplicate })
  const refused = await ask(handler, "POST", "kuvo/settings/commands/commands/add", { was, values: duplicate })
  expect(refused.status).toBe(400)
  expect(refused.json.fields).toEqual(check.json.fields)
})
test("stale form returns 409; unrelated entry still writes; wrong types return 400", async () => {
  const r = row(await page(), "command-test"), was = revision(r, "edit")
  save({ commands: { test: { template: "Run tests", description: "Test it" }, review: { template: "Changed" } } })
  const sent = { ...values, template: "New" }
  expect((await ask(handler, "POST", "kuvo/settings/commands/command-test/edit", { was, values: sent })).status).toBe(200)
  expect((await ask(handler, "POST", "kuvo/settings/commands/command-test/edit", { was, values: sent })).status).toBe(409)
  const now = row(await page(), "command-test")
  expect((await ask(handler, "POST", "kuvo/settings/commands/command-test/edit", { was: revision(now, "edit"), values: { ...sent, template: 4 } })).status).toBe(400)
})
test("malformed command has can.why, refuses without touching file; unreadable source has no rows", async () => {
  save({ commands: { test: 42 } })
  const r = row(await page(), "command-test"), before = readFileSync(CONFIG_FILE, "utf8")
  const result = await ask(handler, "POST", "kuvo/settings/commands/command-test/edit", { was: revision(r, "edit"), values })
  expect(result.status).toBe(403)
  expect(result.json.message).toBe(r.actions[0].can.why)
  expect(readFileSync(CONFIG_FILE, "utf8")).toBe(before)
  writeFileSync(CONFIG_FILE, "broken")
  expect((await page()).groups).toEqual([])
})
test("the followed page changes after kuvo/config writes", async () => {
  const stream = follow(handler, "kuvo/settings/commands/events")
  expect(row(await stream.next(), "command-test")).toBeDefined()
  const next = stream.next()
  expect((await ask(handler, "PATCH", "kuvo/config", { changes: [{ path: ["commands", "another"], value: { template: "Another" }, was: null }] })).status).toBe(200)
  expect(row(await next, "command-another")).toBeDefined()
  stream.stop()
})
test("rewording a line leaves the revision unchanged", async () => {
  const { item } = await import("../src/link/areas/settings-kit")
  const original = row(await page(), "command-test")
  expect(item({ ...original, line: "Different explanation" }).revision).toBe(original.revision)
})
