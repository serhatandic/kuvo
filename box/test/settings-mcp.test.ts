import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { ask, follow } from "./route-answer"

// Settings › MCP Servers as the box describes it (PROTOCOL.md, "A box's settings"): the first flow
// section, a list with forms to add and edit a server, its popular servers, and a server's sign-in
// the box leads. The box's code is real, in a home of the test's own; the engine is a stand-in that
// lists servers, signs one in and takes its connections again, as opencode's API does.

const { engineHandler } = await import("../src/link/server/router")
const { CONFIG_FILE } = await import("../src/link/areas/config")
const { CATALOG_FILE } = await import("../src/link/areas/mcp-catalog")
const { FLOWS_FILE, flowClock, ENDED_SHOWN } = await import("../src/link/areas/settings-flows")
const { holdVault } = await import("../src/link/vault/holder")

/** The stand-in engine: its servers' statuses, the sign-ins started, and what it was asked. */
const engine = {
  servers: [] as any[],
  attempt: "pending" as string,
  asked: [] as string[],
  returned: [] as string[],
}
const fake = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(request) {
    const url = new URL(request.url)
    engine.asked.push(`${request.method} ${url.pathname}`)
    if (url.pathname === "/api/mcp") return Response.json(engine.servers)
    if (url.pathname === "/api/integration") return Response.json({ data: [{ id: "int_tracker", name: "tracker", metadata: { source: "mcp" }, methods: [{ type: "oauth", id: "mcp", label: "Sign in" }], connections: [] }] })
    if (url.pathname === "/api/integration/int_tracker/connect/oauth" && request.method === "POST") return Response.json({ data: { attemptID: "att_1", url: "https://tracker.example.com/authorize?state=1" } })
    if (url.pathname === "/api/integration/int_tracker/connect/oauth/att_1") return Response.json({ data: { status: engine.attempt, ...(engine.attempt === "failed" ? { message: "access_denied" } : {}) } })
    if (url.pathname === "/mcp/oauth/callback") {
      engine.returned.push(url.search)
      engine.attempt = "complete"
      return new Response("Signed in.")
    }
    if (url.pathname === "/api/debug/location") return Response.json([])
    if (url.pathname.startsWith("/api/experimental/mcp/")) return Response.json({})
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
  writeFileSync(file, JSON.stringify(value))
}
const config = () => JSON.parse(readFileSync(CONFIG_FILE, "utf8"))
beforeEach(() => {
  rmSync(FLOWS_FILE, { force: true })
  rmSync(CATALOG_FILE, { force: true })
  engineRuns = true
  engine.servers = []
  engine.attempt = "pending"
  engine.asked = []
  engine.returned = []
  flowClock.now = () => Date.now()
  write(CONFIG_FILE, {
    mcp: {
      servers: {
        files: { type: "local", command: ["sh", "-c", "serve files"], environment: { ROOT: "/srv" }, codemode: false },
        tracker: { type: "remote", url: "https://tracker.example.com/mcp" },
        "kuvo-browser": { type: "local", command: ["kuvo-browser"] },
      },
    },
  })
})

/** This computer, as the box words it to the app beside it (as `ask` asks). */
const here = process.platform === "darwin" ? "this Mac" : "this computer"
const page = async (fromApp = true) => {
  const { status, json } = await ask(handler, "GET", "kuvo/settings/mcp", undefined, { fromApp, served: fromApp })
  expect(status).toBe(200)
  return json
}
const rowOf = (shown: any, id: string): any => shown.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])])).find((row: any) => row.id === id)
const act = (row: any, action: string, values?: unknown, was = row.revision, fromApp = true) => ask(handler, "POST", `kuvo/settings/mcp/${encodeURIComponent(row.id)}/${action}`, { was, ...(values === undefined ? {} : { values }) }, { fromApp, served: fromApp })
/** A form's own revision, which its values and checks are sent from. */
const formOf = (row: any, action: string) => [...(row.actions ?? []), ...(row.add ? [row.add] : [])].find((each: any) => each.id === action)?.form
const check = (row: any, action: string, values: unknown) => ask(handler, "POST", `kuvo/settings/mcp/${encodeURIComponent(row.id)}/${action}/check`, { was: formOf(row, action).revision, values })
const submit = (row: any, action: string, values?: unknown) => act(row, action, values, formOf(row, action).revision)
const change = (row: any, value: unknown, was = row.revision) => ask(handler, "PATCH", "kuvo/settings/mcp", { row: row.id, value, was })
/** A form's values as it opens, by field. */
const formValues = (action: any) => Object.fromEntries(action.form.fields.map((field: any) => [field.id, field.value]))

test("the page lists your servers with how each is doing, in the box's words", async () => {
  engine.servers = [
    { name: "files", status: { status: "failed", error: "spawn serve ENOENT" } },
    { name: "tracker", status: { status: "needs_auth" }, integrationID: "int_tracker" },
  ]
  const { json: listed } = await ask(handler, "GET", "kuvo/settings")
  expect(listed.sections.find((each: any) => each.id === "mcp")).toMatchObject({ title: "MCP Servers", group: "Agents & Tools", icon: "sliders" })
  const shown = await page()
  const servers = rowOf(shown, "servers")
  // Kuvo Code's own browser tools aren't yours to change, so they aren't listed.
  expect(servers.items.map((each: any) => each.id)).toEqual(["server-files", "server-tracker"])
  expect(shown.groups[0]).toMatchObject({ title: "Servers", note: `A server on ${here} runs a command; one on the web is a URL.` })
  expect(servers.add).toMatchObject({ id: "add", title: "Add Server…", form: { title: "Add Server", submit: "Add Server" } })

  const files = rowOf(shown, "server-files")
  expect(files).toMatchObject({ kind: "switch", value: true, can: true, line: "sh -c \"serve files\"", badge: { words: "Couldn’t connect", tone: "failed" }, notice: { words: "spawn serve ENOENT", tone: "warning" } })
  expect(files.actions.map((each: any) => each.title)).toEqual(["Try Again", "Edit…", "Remove…"])
  expect(files.actions[2]).toMatchObject({ style: "destructive", confirm: { title: "Remove files?", body: "The agent loses its tools.", button: "Remove" } })
  const tracker = rowOf(shown, "server-tracker")
  expect(tracker).toMatchObject({ line: "https://tracker.example.com/mcp", badge: { words: "Needs sign-in", tone: "warning" } })
  expect(tracker.actions.map((each: any) => each.title)).toEqual(["Sign In…", "Edit…", "Remove…"])
  expect(rowOf(shown, "timeout-startup")).toMatchObject({ title: "Time to start, in milliseconds", kind: "number", placeholder: "30000", can: true })
  expect(shown.groups.at(-1)).toMatchObject({ title: "Timeouts", note: "For every server, unless one sets its own." })

  // How a server is doing changes from moment to moment: it never makes a reading stale.
  engine.servers[0].status = { status: "connected" }
  const now = rowOf(await page(), "server-files")
  expect(now.badge).toEqual({ words: "Connected", tone: "good" })
  expect(now.revision).toBe(files.revision)
})

test("with its engine down, the servers are still listed, and the page says why it can't say how they are", async () => {
  engineRuns = false
  const shown = await page()
  expect(rowOf(shown, "server-files").badge).toBeUndefined()
  expect(shown.notes).toEqual([{ words: `The engine isn’t running on ${here}, so how each server is doing isn’t known. They’re listed as the settings have them.`, tone: "warning" }])
})

test("a server is turned off and on through the engine's settings, and a stale switch is refused with the page now", async () => {
  let shown = await page()
  const files = rowOf(shown, "server-files")
  shown = (await change(files, false)).json
  expect(config().mcp.servers.files.disabled).toBe(true)
  expect(rowOf(shown, "server-files")).toMatchObject({ value: false, badge: { words: "Off", tone: "quiet" } })
  // Read before it was turned off: refused with the page now, the file as it is.
  const stale = await change(files, false)
  expect(stale.status).toBe(409)
  expect(stale.json.message).toBe("It changed since you read it.")
  expect(rowOf(stale.json.page, "server-files").value).toBe(false)
  // Another row read at the same time still changes.
  const tracker = rowOf(await page(), "server-tracker")
  expect((await change(tracker, false)).status).toBe(200)
  expect((await change(rowOf(await page(), "server-files"), "off")).status).toBe(400)
  shown = (await change(rowOf(await page(), "server-files"), true)).json
  expect(config().mcp.servers.files.disabled).toBeUndefined()
})

test("Add Server's form is checked as it's typed, in the box's words, and adds a server", async () => {
  const servers = rowOf(await page(), "servers")
  const opened = formValues(servers.add)
  expect(servers.add.form.fields.map((field: any) => [field.id, field.kind, field.shown?.is ?? null])).toEqual([
    ["name", "text", null], ["runs", "choice", null], ["command", "lines", ["local"]], ["cwd", "folder", ["local"]],
    ["environment", "pairs", ["local"]], ["url", "text", ["remote"]], ["oauth", "switch", ["remote"]], ["headers", "pairs", ["remote"]], ["codemode", "switch", null],
  ])
  expect(servers.add.form.fields[1].choices).toEqual([{ id: "local", title: `On ${here}` }, { id: "remote", title: "On the web" }])

  let checked = (await check(servers, "add", { ...opened, name: "my files" })).json
  expect(checked.ready).toBe(false)
  expect(checked.fields).toContainEqual({ id: "name", words: "It'll be called my-files.", tone: "quiet" })
  checked = (await check(servers, "add", { ...opened, name: "files", command: "sh -c 'x'" })).json
  expect(checked).toMatchObject({ ready: false })
  expect(checked.fields).toContainEqual({ id: "name", words: "There's a server called files already.", tone: "warning" })
  expect(checked.fields).toContainEqual({ id: "command", words: `sh is on ${here}.`, tone: "good" })
  checked = (await check(servers, "add", { ...opened, name: "kuvo-browser", command: "no-such-program-here --flag" })).json
  expect(checked.fields).toContainEqual({ id: "name", words: "Kuvo Code uses that name for its own tools.", tone: "warning" })
  expect(checked.fields).toContainEqual({ id: "command", words: `no-such-program-here isn’t on ${here}. Install it there first, or use a command it has.`, tone: "warning" })
  checked = (await check(servers, "add", { ...opened, name: "web", runs: "remote", url: "not an address" })).json
  expect(checked.fields).toContainEqual({ id: "url", words: "That isn’t a web address. Give one like https://example.com/mcp.", tone: "warning" })

  // Submitting what isn't ready is refused with the same words, and nothing is written.
  const before = readFileSync(CONFIG_FILE, "utf8")
  const refused = await submit(servers, "add", { ...opened, name: "files", command: "sh" })
  expect(refused.status).toBe(400)
  expect(refused.json).toMatchObject({ message: "There's a server called files already.", fields: expect.arrayContaining([{ id: "name", words: "There's a server called files already.", tone: "warning" }]) })
  expect(readFileSync(CONFIG_FILE, "utf8")).toBe(before)
  // Without its values: refused.
  expect((await submit(servers, "add")).status).toBe(400)

  // Another server changed meanwhile (on a phone): the form read before still goes.
  await change(rowOf(await page(), "server-tracker"), false)
  const added = await submit(servers, "add", { ...opened, name: "docs", command: "npx -y docs-server ~/Docs", environment: [{ key: "TOKEN", value: "{env:DOCS}" }], codemode: true })
  expect(added.status).toBe(200)
  expect(config().mcp.servers.docs).toEqual({ type: "local", command: ["npx", "-y", "docs-server", "~/Docs"], environment: { TOKEN: "{env:DOCS}" }, codemode: true })
  expect(rowOf(added.json, "server-docs")).toMatchObject({ value: true, line: "npx -y docs-server ~/Docs" })
})

test("a server's setup pasted into its name fills the form in", async () => {
  const servers = rowOf(await page(), "servers")
  const pasted = JSON.stringify({ mcpServers: { github: { command: "npx", args: ["-y", "@github/mcp"], env: { GITHUB_TOKEN: "x" } } } })
  const checked = (await check(servers, "add", { ...formValues(servers.add), name: pasted })).json
  expect(checked.values).toEqual({ name: "github", runs: "local", command: "npx -y @github/mcp", environment: [{ key: "GITHUB_TOKEN", value: "x" }] })
  expect(checked.fields).toContainEqual({ id: "command", words: "Filled in from the setup you pasted.", tone: "good" })
  const remote = (await check(servers, "add", { ...formValues(servers.add), name: "claude mcp add --transport http linear https://mcp.linear.app/mcp" })).json
  expect(remote.values).toMatchObject({ name: "linear", runs: "remote", url: "https://mcp.linear.app/mcp" })
  const several = (await check(servers, "add", { ...formValues(servers.add), name: JSON.stringify({ mcpServers: { a: { command: "a" }, b: { command: "b" } } }) })).json
  expect(several.fields).toContainEqual({ id: "name", words: "This sets up 2 servers (a, b). Paste one server’s part at a time.", tone: "warning" })
})

test("Edit… opens with the server's settings, saves what's changed, keeps what it doesn't show, and is stale once the server changed", async () => {
  write(CONFIG_FILE, { mcp: { servers: { files: { type: "local", command: ["sh"], timeout: 9000, codemode: true } } } })
  const files = rowOf(await page(), "server-files")
  const edit = files.actions.find((each: any) => each.id === "edit")
  expect(edit.form).toMatchObject({ title: "files", submit: "Save" })
  expect(formValues(edit)).toMatchObject({ runs: "local", command: "sh", codemode: true })
  // Switched to the web: its command goes, what the form doesn't show (its timeout) stays.
  const saved = await submit(files, "edit", { ...formValues(edit), runs: "remote", url: "mcp.example.com/mcp", oauth: false })
  expect(saved.status).toBe(200)
  expect(config().mcp.servers.files).toEqual({ type: "remote", url: "https://mcp.example.com/mcp", oauth: false, timeout: 9000, codemode: true })
  // The form read before that change is stale now.
  const stale = await submit(files, "edit", { ...formValues(edit), command: "sh -x" })
  expect(stale.status).toBe(409)
  expect(config().mcp.servers.files.type).toBe("remote")
})

test("Remove… takes the server out of the settings, and the rest stay", async () => {
  const shown = (await act(rowOf(await page(), "server-files"), "remove")).json
  expect(Object.keys(config().mcp.servers).sort()).toEqual(["kuvo-browser", "tracker"])
  expect(rowOf(shown, "server-files")).toBeUndefined()
})

test("a server's sign-in is led by the box: started, followed, done, and its server connected again", async () => {
  engine.servers = [{ name: "tracker", status: { status: "needs_auth" }, integrationID: "int_tracker" }]
  let tracker = rowOf(await page(), "server-tracker")
  let shown = (await act(tracker, "sign-in")).json
  tracker = rowOf(shown, "server-tracker")
  expect(tracker.flow).toMatchObject({ state: "waiting", title: "Signing in to tracker", line: "Finish signing in on its page in your browser.", opens: "https://tracker.example.com/authorize?state=1" })
  // The page ends at an address on the box's computer: the app beside it has nothing to paste;
  // any other client (a phone, another computer) pastes the address back.
  expect(tracker.flow.code).toBeUndefined()
  expect(tracker.flow.actions.map((each: any) => each.id)).toEqual(["open-sign-in", "cancel"])
  tracker = rowOf(await page(false), "server-tracker")
  expect(tracker.flow.actions.map((each: any) => each.id)).toEqual(["open-sign-in", "finish", "cancel"])
  expect(tracker.flow.code).toEqual({ title: "Or paste the address the page ended on", placeholder: "http://127.0.0.1:19876/mcp/oauth/callback?code=…", action: "finish" })
  // Sign In… isn't offered twice.
  expect(tracker.actions.map((each: any) => each.id)).toEqual(["edit", "remove"])
  // Open Sign-In Page is the client's to open.
  expect((await act(tracker, "open-sign-in", undefined, tracker.revision, false)).status).toBe(400)

  // The address the page ended on, pasted: loaded on this computer, and the engine signs it in.
  // An address its page doesn't take is said, and the sign-in still waits.
  const turnedDown = await act(tracker, "finish", { code: `http://127.0.0.1:${fake.port}/elsewhere?code=old` }, tracker.revision, false)
  expect(turnedDown.status).toBe(400)
  expect(turnedDown.json.message).toBe("The sign-in didn't take that address. Paste the one its page ended on for this sign-in, or start it again.")
  const pasted = `http://127.0.0.1:${fake.port}/mcp/oauth/callback?code=abc&state=1`
  expect((await act(tracker, "finish", { code: pasted }, tracker.revision, false)).status).toBe(200)
  expect(engine.returned).toEqual(["?code=abc&state=1"])
  engine.servers[0].status = { status: "connected" }
  shown = await page()
  tracker = rowOf(shown, "server-tracker")
  expect(tracker.flow).toMatchObject({ state: "done", line: "Signed in. The agent can use its tools now.", actions: [{ id: "dismiss", title: "OK" }] })
  expect(tracker.badge).toEqual({ words: "Connected", tone: "good" })
  expect(engine.asked.filter(each => each === "POST /api/experimental/mcp/tracker/connect")).toHaveLength(1)
  // Read again, it's connected once only.
  await page()
  expect(engine.asked.filter(each => each === "POST /api/experimental/mcp/tracker/connect")).toHaveLength(1)
  // Seen: put away. One that ended a while ago goes by itself.
  expect(rowOf((await act(tracker, "dismiss")).json, "server-tracker").flow).toBeUndefined()
})

test("a sign-in turned down says so in the box's words, and can be tried again; one that ended long ago is gone", async () => {
  engine.servers = [{ name: "tracker", status: { status: "needs_auth" }, integrationID: "int_tracker" }]
  await act(rowOf(await page(), "server-tracker"), "sign-in")
  engine.attempt = "failed"
  let tracker = rowOf(await page(), "server-tracker")
  expect(tracker.flow).toMatchObject({ state: "failed", line: "The sign-in was turned down on its page. Try again to sign in." })
  expect(tracker.flow.actions.map((each: any) => each.id)).toEqual(["sign-in", "dismiss"])
  engine.attempt = "pending"
  tracker = rowOf((await act(tracker, "sign-in")).json, "server-tracker")
  expect(tracker.flow.state).toBe("waiting")
  // Cancelled: gone.
  tracker = rowOf((await act(tracker, "cancel")).json, "server-tracker")
  expect(tracker.flow).toBeUndefined()
  // Ended a while ago: no longer shown.
  await act(tracker, "sign-in")
  engine.attempt = "failed"
  await page()
  const later = Date.now() + ENDED_SHOWN + 1
  flowClock.now = () => later
  expect(rowOf(await page(), "server-tracker").flow).toBeUndefined()
  expect(existsSync(FLOWS_FILE)).toBe(false)
})

test("a popular server is added from its row, and shows as added", async () => {
  write(CATALOG_FILE, { servers: [{ id: "context7", title: "Context7", summary: "Docs for libraries.", access: "open", settings: { type: "remote", url: "https://mcp.context7.com/mcp" } }] })
  const shown = await page()
  expect(shown.groups.find((group: any) => group.id === "popular-open")).toMatchObject({ title: "No Account Needed", note: expect.stringContaining("official MCP Registry") })
  const added = (await act(rowOf(shown, "popular-context7"), "add")).json
  expect(config().mcp.servers.context7).toEqual({ codemode: false, type: "remote", url: "https://mcp.context7.com/mcp" })
  expect(rowOf(added, "popular-context7")).toMatchObject({ kind: "status", value: "Added" })
  expect(rowOf(added, "server-context7")).toBeDefined()
})

test("a timeout is set as a number, and one written by hand as something else says why it can't be changed here", async () => {
  const startup = rowOf(await page(), "timeout-startup")
  expect((await change(startup, 45000)).status).toBe(200)
  expect(config().mcp.timeout.startup).toBe(45000)
  write(CONFIG_FILE, { mcp: { timeout: { startup: "slow" } } })
  const handwritten = rowOf(await page(), "timeout-startup")
  expect(handwritten.can).toEqual({ why: "This is set in the settings file as something other than a number. Change it there." })
  const refused = await change(handwritten, 1000)
  expect(refused.status).toBe(403)
  expect(config().mcp.timeout.startup).toBe("slow")
})

test("the page followed is sent again as the settings change elsewhere", async () => {
  const lines = follow(handler, "kuvo/settings/mcp/events")
  const first = await lines.next()
  expect(rowOf(first, "server-files")).toBeDefined()
  write(CONFIG_FILE, { mcp: { servers: { other: { type: "local", command: ["sh"] } } } })
  const next = await lines.next()
  expect(rowOf(next, "server-other")).toBeDefined()
  expect(rowOf(next, "server-files")).toBeUndefined()
  lines.stop()
})
