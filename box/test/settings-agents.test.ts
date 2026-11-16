import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { ask } from "./route-answer"

// Settings › Agents as the box describes it (PROTOCOL.md, "A box's settings"): the engine's agents,
// built-in and yours, listed, added, edited, turned off and on, reset and deleted through the
// engine's settings file, and Claude Code's agents brought over. The box's code is real, in a home of
// the test's own; the engine is a stand-in that lists its agents and models, as opencode's API does
// (box/bridge-tests/settings-agents-live.test.js runs the real one).

const { engineHandler } = await import("../src/link/server/router")
const { CONFIG_FILE } = await import("../src/link/areas/config")
const { homeFolder } = await import("../src/link/home")
const { holdVault } = await import("../src/link/vault/holder")

const engine = { agents: [] as any[] }
const fake = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(request) {
    const url = new URL(request.url)
    if (url.pathname === "/api/agent") return Response.json({ data: engine.agents })
    if (url.pathname === "/api/provider") return Response.json({ data: [{ id: "lab" }] })
    if (url.pathname === "/api/model") return Response.json({ data: [{ id: "fast", providerID: "lab", name: "Lab Fast" }, { id: "big", providerID: "lab", name: "Lab Big" }, { id: "off", providerID: "elsewhere", name: "No Account" }] })
    return Response.json({ message: "Not here." }, { status: 404 })
  },
})
let engineRuns = true
const readEngine = () => {
  if (!engineRuns) throw new Error("The engine isn't running.")
  return { port: fake.port, password: "pw" }
}
const handler = engineHandler(readEngine as any, { name: "lokum" }, (() => null) as any)
const held = holdVault(handler, () => () => {})
afterAll(() => {
  held.close()
  fake.stop(true)
})

const write = (file: string, value: unknown) => {
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, typeof value === "string" ? value : JSON.stringify(value))
}
const config = () => JSON.parse(readFileSync(CONFIG_FILE, "utf8"))
const claude = join(homeFolder(), ".claude")
const builtIns = [
  { id: "build", name: "Build", mode: "primary" },
  { id: "plan", name: "Plan", mode: "primary" },
  { id: "general", name: "General", mode: "subagent" },
  { id: "explore", name: "Explore", mode: "subagent" },
  { id: "title", name: "Title", mode: "primary", hidden: true },
]
beforeEach(() => {
  rmSync(claude, { recursive: true, force: true })
  engineRuns = true
  engine.agents = [...builtIns, { id: "reviewer", name: "reviewer", mode: "subagent" }]
  write(CONFIG_FILE, {
    default_agent: "reviewer",
    agents: {
      reviewer: { mode: "all", description: "Reviews changes\nand says what's wrong", steps: 12, permissions: [{ action: "edit", resource: "*", effect: "deny" }, { action: "shell", resource: "git diff *", effect: "allow" }] },
      plan: { system: "Plan in short steps." },
      title: { model: "lab/fast" },
    },
  })
})

const page = async (fromApp = true) => {
  const { status, json } = await ask(handler, "GET", "kuvo/settings/agents", undefined, { fromApp })
  expect(status).toBe(200)
  return json
}
const rowOf = (shown: any, id: string): any => shown.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])])).find((row: any) => row.id === id)
const act = (row: any, action: string, values?: unknown, was = row.revision) => ask(handler, "POST", `kuvo/settings/agents/${encodeURIComponent(row.id)}/${action}`, { was, ...(values === undefined ? {} : { values }) }, { fromApp: true })
const formOf = (row: any, action: string) => [...(row.actions ?? []), ...(row.add ? [row.add] : [])].find((each: any) => each.id === action)?.form
const check = (row: any, action: string, values: unknown) => ask(handler, "POST", `kuvo/settings/agents/${encodeURIComponent(row.id)}/${action}/check`, { was: formOf(row, action).revision, values })
const submit = (row: any, action: string, values?: unknown) => act(row, action, values, formOf(row, action).revision)
const change = (row: any, value: unknown, was = row.revision) => ask(handler, "PATCH", "kuvo/settings/agents", { row: row.id, value, was })
const formValues = (row: any, action: string) => Object.fromEntries(formOf(row, action).fields.map((field: any) => [field.id, field.value]))
const fieldOf = (row: any, action: string, id: string) => formOf(row, action).fields.find((field: any) => field.id === id)

test("the page lists the built-in agents, then yours, each with how it's used and what it offers", async () => {
  const { json: listed } = await ask(handler, "GET", "kuvo/settings")
  expect(listed.sections.find((each: any) => each.id === "agents")).toMatchObject({ title: "Agents", group: "Agents & Tools", icon: "bot" })
  const shown = await page()
  const agents = rowOf(shown, "agents")
  // The engine's hidden ones (titles, summaries) aren't listed.
  expect(agents.items.map((each: any) => each.title)).toEqual(["Build", "Plan", "General", "Explore", "reviewer"])
  expect(agents.add).toMatchObject({ id: "add", title: "New Agent…", form: { title: "New Agent", submit: "Create Agent" } })
  expect(rowOf(shown, "agent-build")).toMatchObject({ kind: "switch", value: true, can: true, line: "A mode · The default agent: it can change files and run commands." })
  expect(rowOf(shown, "agent-build").actions.map((each: any) => each.title)).toEqual(["Edit…"])
  // A built-in one you changed says so, and can go back to how it's built in.
  const plan = rowOf(shown, "agent-plan")
  expect(plan.badge).toEqual({ words: "Changed", tone: "quiet" })
  expect(plan.actions.map((each: any) => each.title)).toEqual(["Edit…", "Reset to Built-In…"])
  expect(plan.actions[1]).toMatchObject({ style: "destructive", confirm: { title: "Reset Plan to how it's built in?", button: "Reset" } })
  // Yours: its first line of description, and Delete.
  const reviewer = rowOf(shown, "agent-reviewer")
  expect(reviewer).toMatchObject({ line: "A mode and a subagent · Reviews changes" })
  expect(reviewer.actions[1]).toMatchObject({ id: "delete", title: "Delete Agent…", confirm: { title: "Delete reviewer?", body: `It's removed from ${process.platform === "darwin" ? "this Mac" : "this computer"}'s settings.`, button: "Delete" } })
  expect(shown.notes).toBeUndefined()
})

test("with its engine down, the agents are still listed from the settings, and the page says what it can't know", async () => {
  engineRuns = false
  const shown = await page(false)
  expect(rowOf(shown, "agents").items.map((each: any) => each.id)).toEqual(["agent-build", "agent-plan", "agent-general", "agent-explore", "agent-reviewer"])
  expect(shown.notes[0]).toMatchObject({ tone: "warning", words: expect.stringContaining("The engine isn’t running on lokum") })
  // Its model is kept as the settings name it.
  const title = fieldOf(rowOf(shown, "agent-reviewer"), "edit", "model")
  expect(title.choices.map((each: any) => each.id)).toEqual([""])
})

test("an agent is turned off and on through the engine's settings, and one turned off stops being where new sessions start", async () => {
  let shown = (await change(rowOf(await page(), "agent-reviewer"), false)).json
  expect(config().agents.reviewer.disabled).toBe(true)
  expect(config().default_agent).toBeUndefined()
  expect(rowOf(shown, "agent-reviewer").value).toBe(false)
  // Read before: refused with the page now.
  const stale = await change(rowOf(await page(), "agent-reviewer"), false, "old")
  expect(stale.status).toBe(409)
  shown = (await change(rowOf(shown, "agent-reviewer"), true)).json
  expect(config().agents.reviewer.disabled).toBeUndefined()
  // A built-in one with nothing set gets only that it's off, and loses it again.
  await change(rowOf(await page(), "agent-explore"), false)
  expect(config().agents.explore).toEqual({ disabled: true })
  await change(rowOf(await page(), "agent-explore"), true)
  expect(config().agents.explore).toBeUndefined()
})

test("New Agent's form is checked as it's typed, refused with the same words, and makes an agent the engine reads", async () => {
  const agents = rowOf(await page(), "agents")
  const opened = formValues(agents, "add")
  expect(formOf(agents, "add").fields.map((field: any) => [field.id, field.kind])).toEqual([
    ["name", "text"], ["mode", "choice"], ["description", "lines"], ["model", "choice"], ["system", "lines"],
    ["may-edit", "choice"], ["may-shell", "choice"], ["may-read", "choice"], ["may-external_directory", "choice"],
    ["may-webfetch", "choice"], ["may-websearch", "choice"], ["may-subagent", "choice"], ["may-skill", "choice"],
    ["allowed", "lines"], ["asks", "lines"], ["never", "lines"],
  ])
  expect(opened.mode).toBe("subagent")
  // The models are those there's an account for here, after Default.
  expect(fieldOf(agents, "add", "model").choices.map((each: any) => each.id)).toEqual(["", "lab/fast", "lab/big"])
  // Each kind of action opens on what applies with no choice, marked.
  expect(fieldOf(agents, "add", "may-external_directory")).toMatchObject({ value: "ask", choices: [{ id: "allow", title: "Allow" }, { id: "ask", title: "Ask First (Default)" }, { id: "deny", title: "Don't Allow" }] })

  let checked = (await check(agents, "add", { ...opened, name: "Bug  Hunter!" })).json
  expect(checked.ready).toBe(true)
  expect(checked.fields).toContainEqual({ id: "name", words: "It'll be called Bug Hunter.", tone: "quiet" })
  expect(checked.fields).toContainEqual({ id: "mode", words: expect.stringContaining("another agent hands it part of the work"), tone: "quiet" })
  checked = (await check(agents, "add", { ...opened, name: "Reviewer" })).json
  expect(checked).toMatchObject({ ready: false, fields: expect.arrayContaining([{ id: "name", words: "There's an agent called Reviewer already.", tone: "warning" }]) })
  checked = (await check(agents, "add", { ...opened, name: "plan" })).json
  expect(checked.ready).toBe(false)
  checked = (await check(agents, "add", { ...opened, name: "x", asks: "git push *" })).json
  expect(checked.fields).toContainEqual({ id: "asks", words: "“git push *” needs what it's for first, like Run commands: npm test *.", tone: "warning" })
  checked = (await check(agents, "add", { ...opened, name: "x", never: "Run commands: *" })).json
  expect(checked.fields).toContainEqual({ id: "never", words: "For all of them, set Run commands above.", tone: "warning" })

  const before = readFileSync(CONFIG_FILE, "utf8")
  const refused = await submit(agents, "add", { ...opened, name: "reviewer" })
  expect(refused.status).toBe(400)
  expect(refused.json).toMatchObject({ message: "There's an agent called reviewer already." })
  expect(readFileSync(CONFIG_FILE, "utf8")).toBe(before)

  const made = await submit(agents, "add", {
    ...opened, name: "Bug Hunter", description: "Finds bugs.", model: "lab/big", system: "Look for bugs; change nothing.",
    "may-edit": "deny", allowed: "Run commands: npm test *\nshell: bun test *", never: "open web pages: https://*.internal/*",
  })
  expect(made.status).toBe(200)
  expect(config().agents["Bug Hunter"]).toEqual({
    mode: "subagent", description: "Finds bugs.", model: "lab/big", system: "Look for bugs; change nothing.",
    permissions: [
      { action: "edit", resource: "*", effect: "deny" },
      { action: "shell", resource: "npm test *", effect: "allow" },
      { action: "shell", resource: "bun test *", effect: "allow" },
      { action: "webfetch", resource: "https://*.internal/*", effect: "deny" },
    ],
  })
  expect(rowOf(made.json, "agent-Bug Hunter")).toMatchObject({ title: "Bug Hunter", line: "A subagent · Finds bugs." })
  // Opened again, it shows what it was made with.
  const edit = formValues(rowOf(made.json, "agent-Bug Hunter"), "edit")
  expect(edit).toMatchObject({ "may-edit": "deny", allowed: "Run commands: npm test *\nRun commands: bun test *", never: "Open web pages: https://*.internal/*", on: true })
})

test("Edit… saves what's changed, keeps what it doesn't show, writes nothing for what's built in, and is stale once the agent changed", async () => {
  let reviewer = rowOf(await page(), "agent-reviewer")
  const values = formValues(reviewer, "edit")
  expect(values).toMatchObject({ mode: "all", "may-edit": "deny", allowed: "Run commands: git diff *", asks: "", never: "" })
  const saved = await submit(reviewer, "edit", { ...values, mode: "subagent", "may-edit": "allow", asks: "Edit files: *.lock" })
  expect(saved.status).toBe(200)
  // Its steps, which the form doesn't show, stay; made a subagent only, it isn't where sessions start.
  expect(config().agents.reviewer).toEqual({
    mode: "subagent", description: "Reviews changes\nand says what's wrong", steps: 12,
    // (Edit files back to its default: its own rule goes, rather than one saying the same.)
    permissions: [{ action: "shell", resource: "git diff *", effect: "allow" }, { action: "edit", resource: "*.lock", effect: "ask" }],
  })
  expect(config().default_agent).toBeUndefined()
  // The form read before is stale now.
  expect((await submit(reviewer, "edit", values)).status).toBe(409)

  // A built-in one saved as it opened writes nothing new; its instructions emptied, nothing's left of it.
  const plan = rowOf(saved.json, "agent-plan")
  expect(formValues(plan, "edit")).toMatchObject({ mode: "primary", system: "Plan in short steps.", "may-edit": "deny" })
  expect(fieldOf(plan, "edit", "may-edit").choices[2]).toEqual({ id: "deny", title: "Don't Allow (Default)" })
  expect((await submit(plan, "edit", formValues(plan, "edit"))).status).toBe(200)
  expect(config().agents.plan).toEqual({ system: "Plan in short steps." })
  reviewer = rowOf(await page(), "agent-plan")
  expect((await submit(reviewer, "edit", { ...formValues(reviewer, "edit"), system: "  " })).status).toBe(200)
  expect(config().agents.plan).toBeUndefined()
})

test("rules kept in an order the form can't show are left as they are, and said so", async () => {
  write(CONFIG_FILE, { agents: { odd: { mode: "subagent", permissions: [{ action: "shell", resource: "rm *", effect: "deny" }, { action: "shell", resource: "rm -i *", effect: "allow" }] } } })
  const odd = rowOf(await page(), "agent-odd")
  expect(fieldOf(odd, "edit", "allowed")).toMatchObject({ value: "", can: { why: "Its rules are in an order only the settings file keeps: change them there." } })
  expect((await submit(odd, "edit", { ...formValues(odd, "edit"), description: "Careful", "may-shell": "ask" })).status).toBe(200)
  expect(config().agents.odd.permissions).toEqual([{ action: "shell", resource: "*", effect: "ask" }, { action: "shell", resource: "rm *", effect: "deny" }, { action: "shell", resource: "rm -i *", effect: "allow" }])
})

test("Reset to Built-In and Delete take the agent's settings out, and a deleted one stops being where sessions start", async () => {
  let shown = (await act(rowOf(await page(), "agent-plan"), "reset")).json
  expect(config().agents.plan).toBeUndefined()
  expect(rowOf(shown, "agent-plan").actions.map((each: any) => each.id)).toEqual(["edit"])
  shown = (await act(rowOf(shown, "agent-reviewer"), "delete")).json
  expect(config().agents.reviewer).toBeUndefined()
  expect(config().default_agent).toBeUndefined()
  // Hidden ones' settings (the titles' model) stay.
  expect(config().agents.title).toEqual({ model: "lab/fast" })
  expect(rowOf(shown, "agent-reviewer")).toBeUndefined()
})

test("Claude Code's agents on this computer are offered, and one imported comes over as a subagent", async () => {
  write(join(claude, "agents", "security.md"), "---\nname: Security Auditor\ndescription: Checks for security problems\n---\nLook for injection.\n")
  write(join(claude, "agents", "plan.md"), "---\nname: plan\ndescription: Claude's planner\n---\nPlan it.\n")
  const shown = await page()
  expect(shown.groups.at(-1)).toMatchObject({ id: "claude-code", title: "From Claude Code" })
  const security = rowOf(shown, "claude-security-auditor")
  expect(security).toMatchObject({ title: "Security Auditor", line: "Checks for security problems", kind: "actions" })
  const after = (await act(security, "import")).json
  expect(config().agents["security-auditor"]).toEqual({ mode: "subagent", description: "Checks for security problems", system: "Look for injection." })
  expect(rowOf(after, "claude-security-auditor")).toMatchObject({ kind: "status", value: "Imported" })
  // One by a built-in one's name comes over beside it.
  expect(rowOf(after, "claude-plan-claude")).toMatchObject({ title: "plan" })
  expect(rowOf(after, "agent-security-auditor")).toMatchObject({ line: "A subagent · Checks for security problems" })
})

test("a settings file whose agents aren't an object is left alone, and the page says so", async () => {
  write(CONFIG_FILE, { agents: ["not", "an", "object"] })
  const shown = await page()
  expect(rowOf(shown, "agents").add).toBeUndefined()
  expect(rowOf(shown, "agent-build").can).toEqual({ why: expect.stringContaining("aren't a list Kuvo can change") })
  expect(shown.notes[0].tone).toBe("warning")
})
