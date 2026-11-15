import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { spawn } from "node:child_process"
import { ask, follow } from "./route-answer"

// A box's settings as it describes them (PROTOCOL.md, "A box's settings"): its sections, the
// Permissions and Browser pages drawn from what the box keeps, a change to one row checked against
// that row's own revision, a row that can't be changed from here, what a row does, and the page
// followed as it changes. Every change is made through the box's existing routes, so a client of
// those (the phone today) finds it kept just as those routes keep it. The box's code is real, in a
// home of the test's own; there's no engine or bridge, as on a box whose engine is starting.

const { engineHandler } = await import("../src/link/server/router")
const { APPROVAL_FILE } = await import("../src/link/work/approval")
const { CONFIG_FILE } = await import("../src/link/areas/config")
const { ALLOWED_FILE } = await import("../src/link/fence/allowed")
const { BROWSER_FOLDER } = await import("../src/link/browser/history")
const { holdVault } = await import("../src/link/vault/holder")

const noEngine = () => { throw new Error("The engine isn't running.") }
const handler = engineHandler(noEngine as any, { name: "devbox" }, noEngine as any)
// The box's long-running process, which keeps Also Allowed (vault/holder.ts), keeping no
// conversation's SSH agent (there are none).
let held = holdVault(handler, () => () => {})
// A project and a folder of its, as Also Allowed keeps them.
const outside = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-settings-pages-")))
afterAll(() => {
  held.close()
  rmSync(outside, { recursive: true, force: true })
})

const files = [APPROVAL_FILE, CONFIG_FILE, ALLOWED_FILE, join(BROWSER_FOLDER, "settings.json"), join(BROWSER_FOLDER, "keeps-agent-tabs")]
const write = (file: string, value: unknown) => {
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, JSON.stringify(value))
}
beforeEach(() => {
  for (const file of files) rmSync(file, { force: true })
  // A machine's own settings, as Kuvo Code makes them once (`ownsSettings`).
  write(CONFIG_FILE, { permissions: [{ action: "shell", resource: "git push *", effect: "ask" }] })
})

const page = async (section: string) => {
  const { status, json } = await ask(handler, "GET", `kuvo/settings/${section}`)
  expect(status).toBe(200)
  return json
}
const rowOf = (shown: any, id: string): any => shown.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])])).find((row: any) => row.id === id)
const change = (section: string, row: any, value: unknown, was = row.revision) => ask(handler, "PATCH", `kuvo/settings/${section}`, { row: row.id, value, was })
const act = (section: string, row: any, action: string, was = row.revision) => ask(handler, "POST", `kuvo/settings/${section}/${encodeURIComponent(row.id)}/${action}`, { was })
const kept = (file: string) => (existsSync(file) ? readFileSync(file, "utf8") : null)

test("the box lists its sections, each with what a sidebar and search show", async () => {
  const { status, json } = await ask(handler, "GET", "kuvo/settings")
  expect(status).toBe(200)
  expect(json.sections.map((section: any) => [section.id, section.title, section.group, section.icon])).toEqual([
    ["general", "General", "General", "settings"],
    ["permissions", "Permissions", "Agents & Tools", "shield"],
    ["browser", "Browser", "System", "globe"],
  ])
  expect(json.sections[0].keywords).toContain("time zone")
  expect(json.sections[1].keywords).toContain("sandbox")
  expect((await ask(handler, "GET", "kuvo/settings/vault")).status).toBe(404)
})

test("the browser's page is its settings, and a change is kept as kuvo/browser/settings keeps it", async () => {
  let shown = await page("browser")
  const engine = rowOf(shown, "search-engine")
  expect([engine.kind, engine.value, engine.can]).toEqual(["choice", "google", true])
  expect(engine.choices.map((choice: any) => choice.title)).toEqual(["Google", "DuckDuckGo", "Bing", "Brave Search", "Kagi", "Ecosia"])
  expect([rowOf(shown, "tidies").value, rowOf(shown, "offers").value]).toEqual([true, true])
  expect(rowOf(shown, "never")).toMatchObject({ kind: "list", items: [], empty: "None yet." })

  // The same change through the route the phone uses today, then through this page: kept the same.
  await ask(handler, "PATCH", "kuvo/browser/settings", { searchEngine: "kagi", tidiesAgentTabs: false })
  const byRoute = files.map(kept)
  for (const file of files.slice(3)) rmSync(file, { force: true })
  shown = (await change("browser", engine, "kagi")).json
  shown = (await change("browser", rowOf(shown, "tidies"), false)).json
  expect(files.map(kept)).toEqual(byRoute)
  expect([rowOf(shown, "search-engine").value, rowOf(shown, "tidies").value]).toEqual(["kagi", false])
  expect((await ask(handler, "GET", "kuvo/browser/settings")).json).toMatchObject({ searchEngine: "kagi", tidiesAgentTabs: false })

  // A value that isn't one of its choices is refused, and nothing changes.
  const refused = await change("browser", rowOf(shown, "search-engine"), "altavista")
  expect(refused.status).toBe(400)
  expect(rowOf(await page("browser"), "search-engine").value).toBe("kagi")
})

test("a change from a row read before another changed it is refused with the page now, and another row's still goes", async () => {
  const first = await page("browser")
  // Someone else (another client, through the old route) changes the search engine meanwhile.
  await ask(handler, "PATCH", "kuvo/browser/settings", { searchEngine: "bing" })
  const stale = await change("browser", rowOf(first, "search-engine"), "kagi")
  expect(stale.status).toBe(409)
  expect(stale.json.message).toBe("It changed since you read it.")
  expect(rowOf(stale.json.page, "search-engine").value).toBe("bing")
  expect((await ask(handler, "GET", "kuvo/browser/settings")).json.searchEngine).toBe("bing")
  // A row that didn't change, read at the same time, isn't held up by it.
  const offers = await change("browser", rowOf(first, "offers"), false)
  expect(offers.status).toBe(200)
  expect(rowOf(offers.json, "offers").value).toBe(false)
  // And a change without the revision it was read at isn't taken at all.
  expect((await ask(handler, "PATCH", "kuvo/settings/browser", { row: "offers", value: true })).status).toBe(400)
})

test("a site never offered is offered again from its row", async () => {
  await ask(handler, "PATCH", "kuvo/browser/settings", { never: ["example.com", "localhost:3000"] })
  const shown = await page("browser")
  const site = rowOf(shown, "never-localhost:3000")
  expect(site).toMatchObject({ title: "localhost:3000", kind: "actions", actions: [{ id: "offer-again", title: "Offer Again" }] })
  const done = await act("browser", site, "offer-again")
  expect(done.status).toBe(200)
  expect(rowOf(done.json, "never").items.map((row: any) => row.title)).toEqual(["example.com"])
  expect((await ask(handler, "GET", "kuvo/browser/settings")).json.never).toEqual(["example.com"])
  // Done once already: from the same reading, it's gone.
  expect((await act("browser", site, "offer-again")).status).toBe(404)
})

test("Full Access can't be turned on from Settings, and says why; it can be turned off, as kuvo/approval turns it off", async () => {
  let shown = await page("permissions")
  const off = rowOf(shown, "full-access")
  expect([off.kind, off.value]).toEqual(["switch", false])
  expect(off.can.why).toContain("pick Full Access in its mode menu")
  const refused = await change("permissions", off, true)
  expect(refused.status).toBe(403)
  expect(refused.json.message).toBe(off.can.why)
  expect((await ask(handler, "GET", "kuvo/approval")).json.fullAccess).toBe(false)

  // Confirmed in a conversation (the route a client uses then), it can be turned off here, asked first.
  await ask(handler, "PATCH", "kuvo/approval", { fullAccess: true })
  await ask(handler, "PATCH", "kuvo/approval", { fullAccess: false })
  const byRoute = kept(APPROVAL_FILE)
  await ask(handler, "PATCH", "kuvo/approval", { fullAccess: true })
  shown = await page("permissions")
  const on = rowOf(shown, "full-access")
  expect([on.value, on.can, on.confirm.button]).toEqual([true, true, "Turn Off"])
  expect((await change("permissions", on, false)).status).toBe(200)
  expect(kept(APPROVAL_FILE)).toBe(byRoute)
})

test("letting your other computers start work here is kept as kuvo/approval keeps it", async () => {
  await ask(handler, "PATCH", "kuvo/approval", { otherComputers: false })
  const byRoute = kept(APPROVAL_FILE)
  rmSync(APPROVAL_FILE, { force: true })
  const row = rowOf(await page("permissions"), "other-computers")
  // (A machine has it on until you say.)
  expect([row.kind, row.value, row.line]).toEqual(["switch", true, "Their bots and agents can start sessions on devbox."])
  const done = await change("permissions", row, false)
  expect(rowOf(done.json, "other-computers").value).toBe(false)
  expect(kept(APPROVAL_FILE)).toBe(byRoute)
  expect(rowOf(done.json, "default-mode")).toMatchObject({ kind: "status", value: "Auto" })
})

test("what agents may do is the engine's permissions, each choice a rule for everything, the others left as they are", async () => {
  let shown = await page("permissions")
  const shell = rowOf(shown, "when-shell")
  expect([shell.kind, shell.value]).toEqual(["choice", "allow"])
  expect(shell.choices.map((choice: any) => choice.title)).toEqual(["Allow (Default)", "Ask First", "Don't Allow"])
  expect(rowOf(shown, "when-external_directory").choices.map((choice: any) => choice.title)).toEqual(["Allow", "Ask First (Default)", "Don't Allow"])

  // Written as the Mac's Settings wrote it through kuvo/config: at the front, the rest kept.
  shown = (await change("permissions", shell, "deny")).json
  expect(JSON.parse(readFileSync(CONFIG_FILE, "utf8")).permissions).toEqual([
    { action: "shell", resource: "*", effect: "deny" },
    { action: "shell", resource: "git push *", effect: "ask" },
  ])
  expect(rowOf(shown, "when-shell").value).toBe("deny")
  // Another rule changed by hand meanwhile doesn't hold up a change to this choice.
  const read = await page("permissions")
  write(CONFIG_FILE, { permissions: [{ action: "shell", resource: "*", effect: "deny" }, { action: "read", resource: "*.pem", effect: "deny" }] })
  shown = (await change("permissions", rowOf(read, "when-shell"), "allow")).json
  expect(JSON.parse(readFileSync(CONFIG_FILE, "utf8")).permissions).toEqual([{ action: "read", resource: "*.pem", effect: "deny" }])
  expect(rowOf(shown, "when-shell").value).toBe("allow")

  // This choice changed by hand since it was read: refused, with the page now; another choice read
  // at the same time still goes.
  const before = await page("permissions")
  write(CONFIG_FILE, { permissions: [{ action: "shell", resource: "*", effect: "ask" }] })
  const stale = await change("permissions", rowOf(before, "when-shell"), "deny")
  expect(stale.status).toBe(409)
  expect(rowOf(stale.json.page, "when-shell").value).toBe("ask")
  const edit = await change("permissions", rowOf(before, "when-edit"), "ask")
  expect(edit.status).toBe(200)
  expect(JSON.parse(readFileSync(CONFIG_FILE, "utf8")).permissions).toEqual([
    { action: "edit", resource: "*", effect: "ask" },
    { action: "shell", resource: "*", effect: "ask" },
  ])

  // Written by hand as something else than a list of rules: left as it is, and said why.
  write(CONFIG_FILE, { permissions: { shell: "ask" } })
  const unlistedPage = await page("permissions")
  const unlisted = rowOf(unlistedPage, "when-shell")
  // (Not shown as a default it may not be: as it is, set in the file.)
  expect([unlisted.kind, unlisted.value]).toEqual(["status", "Set in Its Settings File"])
  expect(unlistedPage.groups.find((group: any) => group.id === "when").note).toContain("aren't a list of rules")
  expect((await change("permissions", unlisted, "deny")).status).toBe(403)
  expect(JSON.parse(readFileSync(CONFIG_FILE, "utf8")).permissions).toEqual({ shell: "ask" })
})

test("picking what an action does with no rule writes none, so Plan mode's own rule (no edits) is never widened", async () => {
  // Nothing set for edits: Allow is what applies, and picking it writes nothing at all.
  const untouched = readFileSync(CONFIG_FILE, "utf8")
  const edit = rowOf(await page("permissions"), "when-edit")
  expect(edit.value).toBe("allow")
  const same = await change("permissions", edit, "allow")
  expect(same.status).toBe(200)
  expect(readFileSync(CONFIG_FILE, "utf8")).toBe(untouched)
  // A rule of yours for edits that allows them everywhere (the Mac's Settings wrote one): shown apart
  // from the default, which applies in Plan mode too; picking Allow again keeps it, the default
  // takes it away (the engine's own rules then apply), never writing one for it.
  write(CONFIG_FILE, { permissions: [{ action: "edit", resource: "*", effect: "allow" }, { action: "shell", resource: "git push *", effect: "ask" }] })
  const own = rowOf(await page("permissions"), "when-edit")
  expect(own.value).toBe("allow")
  expect(own.choices.map((choice: any) => [choice.id, choice.title])).toEqual([["allow", "Allow (Your Rule)"], ["ask", "Ask First"], ["deny", "Don't Allow"], ["default", "Allow (Default)"]])
  expect(own.revision).not.toBe(edit.revision)
  const kept = readFileSync(CONFIG_FILE, "utf8")
  await change("permissions", own, "allow")
  expect(readFileSync(CONFIG_FILE, "utf8")).toBe(kept)
  await change("permissions", rowOf(await page("permissions"), "when-edit"), "default")
  expect(JSON.parse(readFileSync(CONFIG_FILE, "utf8")).permissions).toEqual([{ action: "shell", resource: "git push *", effect: "ask" }])
  // Don't Allow, then back: the rule comes and goes.
  await change("permissions", rowOf(await page("permissions"), "when-edit"), "deny")
  expect(JSON.parse(readFileSync(CONFIG_FILE, "utf8")).permissions[0]).toEqual({ action: "edit", resource: "*", effect: "deny" })
  await change("permissions", rowOf(await page("permissions"), "when-edit"), "allow")
  expect(JSON.parse(readFileSync(CONFIG_FILE, "utf8")).permissions).toEqual([{ action: "shell", resource: "git push *", effect: "ask" }])
})

test("a row's revision is its value's: the same value set through the old route, or the computer renamed, leaves it", async () => {
  const before = await page("browser")
  await ask(handler, "PATCH", "kuvo/browser/settings", { searchEngine: "google", offersToSave: true })
  const after = await page("browser")
  for (const id of ["search-engine", "tidies", "offers", "never"]) expect(rowOf(after, id).revision).toBe(rowOf(before, id).revision)
  const permissions = await page("permissions")
  await ask(handler, "PATCH", "kuvo/approval", { otherComputers: true })
  const renamed = engineHandler(noEngine as any, { name: "devbox-2" }, noEngine as any)
  const again = (await ask(renamed, "GET", "kuvo/settings/permissions")).json
  expect(rowOf(again, "other-computers").line).toContain("devbox-2")
  expect(rowOf(again, "other-computers").revision).toBe(rowOf(permissions, "other-computers").revision)
  // And a change from the earlier reading still goes.
  expect((await change("permissions", rowOf(permissions, "other-computers"), false)).status).toBe(200)
})

test("two changes to one row from the same reading, at once: one is made, the other told it's stale", async () => {
  const read = await page("browser")
  const engine = rowOf(read, "search-engine")
  const answers = await Promise.all([change("browser", engine, "kagi"), change("browser", engine, "bing")])
  expect(answers.map(answer => answer.status).sort()).toEqual([200, 409])
  const made = answers.find(answer => answer.status === 200)!
  expect((await ask(handler, "GET", "kuvo/browser/settings")).json.searchEngine).toBe(rowOf(made.json, "search-engine").value)
})

test("a page followed is said again when its settings change through the old routes, or by hand in the file", async () => {
  const events = follow(handler, "kuvo/settings/permissions/events")
  try {
    const first = await events.next()
    expect(rowOf(first, "other-computers").value).toBe(true)
    await ask(handler, "PATCH", "kuvo/approval", { otherComputers: false })
    expect(rowOf(await events.next(), "other-computers").value).toBe(false)
    write(CONFIG_FILE, { permissions: [{ action: "webfetch", resource: "*", effect: "deny" }] })
    expect(rowOf(await events.next(), "when-webfetch").value).toBe("deny")
  } finally {
    events.stop()
  }
})

test("Also Allowed lists the folders given, and one removed from its row is gone from kuvo/sandbox/allowed", async () => {
  const project = join(outside, "web")
  const folder = join(outside, "cache")
  mkdirSync(project, { recursive: true })
  mkdirSync(folder, { recursive: true })
  // Given with Always on a stopped command's card (as Also Allowed keeps it: what it lets a folder
  // be is its own test's, fence/allowed.ts).
  const id = "al_cache"
  write(ALLOWED_FILE, { allowed: [{ id, project, kind: "write", path: folder, at: Date.now(), by: "Mac" }] })
  expect((await ask(handler, "GET", "kuvo/sandbox/allowed")).json.allowed.map((rule: any) => rule.id)).toEqual([id])
  const shown = await page("permissions")
  const list = rowOf(shown, "allowed")
  expect(list).toMatchObject({ kind: "list", title: "Also Allowed" })
  expect(list.items).toHaveLength(1)
  const rule = rowOf(shown, `allowed-${id}`)
  expect([rule.title, rule.line, rule.actions[0].title]).toEqual([folder, "Can write · for web", "Remove"])
  const done = await act("permissions", rule, "remove")
  expect(done.status).toBe(200)
  expect(rowOf(done.json, "allowed").items).toEqual([])
  expect((await ask(handler, "GET", "kuvo/sandbox/allowed")).json.allowed).toEqual([])
})

/** What a Mac asks this box over SSH (`kuvo-link.js box route`): a process of its own, which passes
 * Also Allowed to the one holding it (this test's), as it would to the box's long-running one. */
async function boxRoute(method: string, path: string, body?: unknown): Promise<{ status: number; json: any }> {
  // (The link from its source, as kuvo-link.js is built from it.)
  const script = join(import.meta.dir, "..", "src", "link", "main.ts")
  const home = process.env.KUVO_HOME!
  mkdirSync(join(home, "route-home"), { recursive: true })
  const child = spawn(process.execPath, [script, "box", "route"], { env: { PATH: process.env.PATH, BUN_BE_BUN: "1", KUVO_HOME: home, HOME: join(home, "route-home"), TMPDIR: join(home, "route-home") }, stdio: ["pipe", "pipe", "inherit"] })
  child.stdin.end(JSON.stringify({ method, path, ...(body === undefined ? {} : { body }) }))
  let out = ""
  child.stdout.setEncoding("utf8")
  for await (const chunk of child.stdout) out += chunk
  const answer = JSON.parse(out.trim().split("\n").at(-1)!)
  return { status: answer.status, json: JSON.parse(answer.body) }
}

test("over SSH, Also Allowed is read and removed through the process that holds it", async () => {
  const project = join(outside, "shop")
  mkdirSync(project, { recursive: true })
  write(ALLOWED_FILE, { allowed: [{ id: "al_ssh", project, kind: "write", path: join(outside, "shop-data"), at: Date.now() }] })
  const shown = await boxRoute("GET", "kuvo/settings/permissions")
  expect(shown.status).toBe(200)
  const rule = rowOf(shown.json, "allowed-al_ssh")
  expect(rule.title).toBe(join(outside, "shop-data"))
  const done = await boxRoute("POST", "kuvo/settings/permissions/allowed-al_ssh/remove", { was: rule.revision })
  expect(done.status).toBe(200)
  expect(rowOf(done.json, "allowed").items).toEqual([])
  expect(JSON.parse(readFileSync(ALLOWED_FILE, "utf8")).allowed).toEqual([])
})

test("over SSH, two changes to one row from the same reading go through one process: one is made, the other told it's stale", async () => {
  // Each a `box route` of its own, as the desktop over SSH and another client at once.
  const read = (await boxRoute("GET", "kuvo/settings/browser")).json
  const engine = rowOf(read, "search-engine")
  const answers = await Promise.all(["kagi", "bing"].map(value => boxRoute("PATCH", "kuvo/settings/browser", { row: engine.id, value, was: engine.revision })))
  expect(answers.map(answer => answer.status).sort()).toEqual([200, 409])
  expect(answers.find(answer => answer.status === 409)!.json.page).toBeDefined()
  // Made by the process holding the box (this test's, named devbox), never by the one asked, which
  // checks only what it alone saw: its answer is the holder's page.
  const permissions = (await boxRoute("GET", "kuvo/settings/permissions")).json
  const done = await boxRoute("PATCH", "kuvo/settings/permissions", { row: "other-computers", value: false, was: rowOf(permissions, "other-computers").revision })
  expect(done.status).toBe(200)
  expect(rowOf(done.json, "other-computers").line).toBe("Their bots and agents can start sessions on devbox.")
  // With no long-running process (a machine reached over SSH alone): made by the one asked.
  held.close()
  try {
    const engine = rowOf((await boxRoute("GET", "kuvo/settings/browser")).json, "search-engine")
    const alone = await boxRoute("PATCH", "kuvo/settings/browser", { row: engine.id, value: "ecosia", was: engine.revision })
    expect(alone.status).toBe(200)
    expect((await ask(handler, "GET", "kuvo/browser/settings")).json.searchEngine).toBe("ecosia")
  } finally {
    held = holdVault(handler, () => () => {})
  }
})

test("the sandbox's row is what kuvo/sandbox says, and its button does what that names", async () => {
  const { sandboxAction } = await import("../src/link/areas/settings-pages")
  const shown = await page("permissions")
  const sandbox = rowOf(shown, "sandbox")
  // (Not checked yet, in a test's home: Try Again, which checks it.)
  const state = (await ask(handler, "GET", "kuvo/sandbox")).json
  expect([state.on, state.route]).toEqual([false, "POST kuvo/sandbox/check"])
  expect(sandbox).toMatchObject({ kind: "actions", value: "Off", line: state.why, tone: "warning", actions: [sandboxAction(state, "devbox", true)] })
  const done = await act("permissions", sandbox, String(state.action))
  expect(done.status).toBe(200)
  // Checked, as kuvo/sandbox/check checks it: the page says what it came to.
  const now = (await ask(handler, "GET", "kuvo/sandbox")).json
  expect(rowOf(done.json, "sandbox").line ?? "").toBe(now.on ? "Agents' commands here run in it." : now.why)
  expect(shown.notes.at(-1).words).toContain("aren't on this page yet")
})

test("Restart Engine starts the box's own tool, and answers done; where the app restarts a Mac's engine, it says so", async () => {
  const { sandboxDoes } = await import("../src/link/areas/settings-pages")
  const { toolRun } = await import("../src/link/areas/engine")
  const { K, RUN } = await import("../src/link/home")
  // The machine's tool as Kuvo Code sets it up (`bin/kuvo-start`), a stand-in noting what it's asked.
  const asked = join(K, "start-asked")
  const made = [join(K, "bin", "kuvo-launch"), join(RUN, "launch.env"), join(K, "bin", "kuvo-start"), asked]
  try {
    for (const file of made.slice(0, 2)) { mkdirSync(dirname(file), { recursive: true }); writeFileSync(file, "") }
    writeFileSync(made[2]!, `echo "$1" > ${JSON.stringify(asked)}\n`)
    const context = { info: { name: "devbox" } } as any
    expect(await sandboxDoes(context, "POST kuvo/engine/restart", true)).toBeNull()
    await toolRun()
    expect(readFileSync(asked, "utf8").trim()).toBe("restart")
    rmSync(asked)
    expect(await sandboxDoes({ info: { name: "Studio" } } as any, "POST kuvo/engine/restart", false)).toEqual({ status: 403, message: "Kuvo Code on Studio restarts its engine: restart it there." })
    expect(existsSync(asked)).toBe(false)
    expect((await sandboxDoes(context, "POST kuvo/somewhere/else", true))?.status).toBe(404)
  } finally {
    // (The test home is every test file's: what a set-up machine has goes with this test.)
    for (const file of made) rmSync(file, { force: true })
  }
  // Offered where the app restarts a Mac's engine: said, never sent.
  const { sandboxAction } = await import("../src/link/areas/settings-pages")
  const restart = sandboxAction({ on: false, action: "restartEngine", actionLabel: "Restart Engine…", route: "POST kuvo/engine/restart" }, "Studio", true, false)
  expect(restart).toEqual({ id: "restartEngine", title: "Restart Engine…", can: { why: "Kuvo Code on Studio restarts its engine: restart it there." } })
})

test("the sandbox's button is the box's: a route it runs, asked first where it stops work, and this Mac's own Software Update only from the app on it", async () => {
  const { sandboxAction } = await import("../src/link/areas/settings-pages")
  const { SOFTWARE_UPDATE, SANDBOX_HELP } = await import("../src/link/fence/state")
  expect(sandboxAction({ on: false, action: "retry", actionLabel: "Try Again", route: "POST kuvo/sandbox/check" }, "devbox", false)).toEqual({ id: "retry", title: "Try Again" })
  expect(sandboxAction({ on: false, action: "restartEngine", actionLabel: "Restart Engine…", route: "POST kuvo/engine/restart" }, "devbox", false)?.confirm?.title).toBe("Restart devbox's engine?")
  expect(sandboxAction({ on: false, action: "restartEngine", actionLabel: "Restart Engine", route: "POST kuvo/engine/restart" }, "devbox", false)?.confirm).toBeUndefined()
  expect(sandboxAction({ on: false, action: "help", actionLabel: "How to Check…", opens: SANDBOX_HELP }, "devbox", false)?.opens).toBe(SANDBOX_HELP)
  const update = { on: false, action: "softwareUpdate", actionLabel: "Software Update…", opens: SOFTWARE_UPDATE, opensHere: true }
  expect(sandboxAction(update, "Studio", true)).toEqual({ id: "softwareUpdate", title: "Software Update…", opens: SOFTWARE_UPDATE })
  expect(sandboxAction(update, "Studio", false)).toEqual({ id: "softwareUpdate", title: "Software Update…", can: { why: "Open Software Update on Studio itself." } })
})

test("a page followed is said again as it changes", async () => {
  const events = follow(handler, "kuvo/settings/browser/events")
  try {
    const first = await events.next()
    expect(rowOf(first, "offers").value).toBe(true)
    await change("browser", rowOf(first, "offers"), false)
    const next = await events.next()
    expect(rowOf(next, "offers").value).toBe(false)
  } finally {
    events.stop()
  }
})

// #668: on, on a machine whose commands can still reach your login's services, the row warns with
// the box's words and the page it names, as for off; only git's files kept apart, it's a quiet On.
test("the sandbox's row warns while commands can still reach your login's services", async () => {
  const { sandboxRow } = await import("../src/link/areas/settings-pages")
  const { worded } = await import("../src/link/fence/state")
  const services = worded({ on: true, able: true, agents: true, gaps: ["services"], needs: "user namespaces" })
  expect(sandboxRow(services, "devbox", false)).toMatchObject({ value: "On", tone: "warning", line: services.why, kind: "actions", actions: [{ id: "help", title: "How to Check…", opens: "https://www.kuvo.run/code/docs/machines/#sandbox" }] })
  const git = sandboxRow(worded({ on: true, able: true, agents: true, gaps: ["git"] }), "devbox", false)
  expect(git).toMatchObject({ value: "On", kind: "status", line: "Agents' commands here run in it." })
  expect("tone" in git).toBe(false)
})
