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
const { PROJECTS_FILE } = await import("../src/link/areas/projects")
const { WORK_SETTINGS_FILE } = await import("../src/link/work/settings")
const { BRANCH_PREFIX_FILE, WORKTREES_FILE } = await import("../src/link/areas/worktree-records")
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
  for (const file of [...files, BRANCH_PREFIX_FILE, WORK_SETTINGS_FILE, WORKTREES_FILE, PROJECTS_FILE]) rmSync(file, { force: true })
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
    ["clock", "Clock", "General", "settings"],
    ["permissions", "Permissions", "Agents & Tools", "shield"],
    ["browser", "Browser", "System", "globe"],
    ["computer", process.platform === "darwin" ? "This Mac" : "This Computer", "System", "computer"],
    ["git", "Git", "Projects", "branch"],
    ["worktrees", "Worktrees", "Projects", "folder"],
    ["storage", "Storage", "System", "drive"],
    ["archived", "Archived Sessions", "System", "archive"],
  ])
  expect((await ask(handler, "GET", "kuvo/settings/general")).status).toBe(404)
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
  const { fenceFiles, forgetFence } = await import("../src/link/fence/state")
  try {
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
  } finally {
    // (The check set the sandbox up in the run's own home: every later test file would run its
    // agents in it, where the engine's own runtime is hidden.)
    forgetFence()
    rmSync(fenceFiles.folder, { recursive: true, force: true })
    rmSync(fenceFiles.launcher, { force: true })
    rmSync(fenceFiles.runner, { force: true })
    rmSync(fenceFiles.plugin, { force: true })
  }
  expect([existsSync(fenceFiles.folder), existsSync(fenceFiles.launcher), existsSync(fenceFiles.runner), existsSync(fenceFiles.plugin)]).toEqual([false, false, false, false])
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

test("Git rows wrap config, with independent revisions and stale writes refused", async () => {
  const shown = await page("git")
  expect(shown.groups.flatMap((g: any) => g.rows.map((r: any) => [r.id, r.kind, r.title, r.value]))).toEqual([
    ["branch-prefix", "text", "Branch prefix", "kuvo/"],
    ["commit-instructions", "text", "Instructions", ""],
    ["pull-request-instructions", "text", "Instructions", ""],
  ])
})

test("Git writes with independent revisions and refuses stale changes", async () => {
  const shown = await page("git")
  const prefix = rowOf(shown, "branch-prefix")
  expect((await change("git", prefix, "feature/")).status).toBe(200)
  expect((await ask(handler, "GET", "kuvo/config")).json.branch_prefix).toBe("feature/")
  expect((await change("git", prefix, "other/")).status).toBe(409)
  expect((await change("git", rowOf(shown, "commit-instructions"), "Use imperative mood")).status).toBe(200)
})

test("Git keeps an explicitly empty prefix", async () => {
  expect((await change("git", rowOf(await page("git"), "branch-prefix"), "")).status).toBe(200)
  expect((await ask(handler, "GET", "kuvo/config")).json.branch_prefix).toBe("")
})
test("Git events follow both the page and existing config route", async () => {
  const events = follow(handler, "kuvo/settings/git/events")
  try {
    const first = await events.next()
    await change("git", rowOf(first, "branch-prefix"), "session/")
    expect(rowOf(await events.next(), "branch-prefix").value).toBe("session/")
    await ask(handler, "PATCH", "kuvo/config", { changes: [{ path: ["pull_request_instructions"], value: "Include testing" }] })
    expect(rowOf(await events.next(), "pull-request-instructions").value).toBe("Include testing")
  } finally { events.stop() }
})
test("Git warns without rows when its settings cannot be read", async () => {
  writeFileSync(CONFIG_FILE, "{broken")
  const shown = await page("git")
  expect(shown.groups).toEqual([])
  expect(shown.notes[0]).toMatchObject({ tone: "warning" })
  expect(shown.notes[0].words).toContain("settings")
})

test("Git refuses a non-text setting without touching the file", async () => {
  write(CONFIG_FILE, { branch_prefix: false })
  const before = kept(CONFIG_FILE)
  const prefix = rowOf(await page("git"), "branch-prefix")
  expect(prefix.can.why).toContain("other than text")
  expect((await change("git", prefix, "feature/")).status).toBe(403)
  expect(kept(CONFIG_FILE)).toBe(before)
})

test("Git cleans the prefix and trims instructions as they are used", async () => {
  let shown = await page("git")
  shown = (await change("git", rowOf(shown, "branch-prefix"), " /bad prefix../ ")).json
  expect(rowOf(shown, "branch-prefix").value).toBe("badprefix./")
  shown = (await change("git", rowOf(shown, "commit-instructions"), "  Keep it short\n ")).json
  expect(rowOf(shown, "commit-instructions").value).toBe("Keep it short")
  expect((await ask(handler, "GET", "kuvo/config")).json).toMatchObject({ branch_prefix: "badprefix./", commit_instructions: "Keep it short" })
})

test("Git requires text", async () => {
  const answer = await change("git", rowOf(await page("git"), "branch-prefix"), false)
  expect([answer.status, answer.json.message]).toEqual([400, "Send text."])
})

test("Git wording and multi-line hints match its Swift sections", async () => {
  const shown = await page("git")
  expect(shown.groups.map((g: any) => [g.id, g.title, g.note])).toEqual([
    ["branches", "Branches", undefined],
    ["commit-messages", "Commit Messages", "Added when Suggest a Message in Changes asks the model for one."],
    ["pull-requests", "Pull Requests", "Added when Create Pull Request writes the title and description."],
  ])
  expect(shown.groups.flatMap((g: any) => g.rows.map((r: any) => [r.title, r.line, r.placeholder, r.lines]))).toEqual([
    ["Branch prefix", "How a session’s own branch in a worktree starts.", "None", undefined],
    ["Instructions", undefined, "For example: Start with the area changed, like “ui: …”, and never mention tickets.", { min: 3, max: 8 }],
    ["Instructions", undefined, "For example: Add a Testing section, and link the issue if the branch names one.", { min: 3, max: 8 }],
  ])
  const { revised } = await import("../src/link/areas/settings-pages")
  const original = rowOf(shown, "commit-instructions")
  expect(revised({ ...original, title: "Reworded", line: "Reworded", lines: { min: 4, max: 9 } }).revision).toBe(original.revision)
})

test("Git events follow the old one-time settings migration route", async () => {
  const events = follow(handler, "kuvo/settings/git/events")
  try {
    await events.next()
    const moved = await ask(handler, "POST", "kuvo/git/settings/move", { branchPrefix: "moved/", commitInstructions: "Keep it short" })
    expect(moved.status).toBe(200)
    expect(moved.json.moved).toBe(true)
    const next = await events.next()
    expect(rowOf(next, "branch-prefix").value).toBe("moved/")
    expect(rowOf(next, "commit-instructions").value).toBe("Keep it short")
  } finally { events.stop() }
})


test("Worktrees words, folders and removal confirmation match Swift", async () => {
  const shown = await page("worktrees")
  expect(shown.groups.map((g: any) => [g.id, g.title, g.note])).toEqual([
    ["new-projects", "New Projects", "Where projects your bots make go. A bot asks you before it makes one, and Kuvo Code never deletes them."],
    ["new-worktrees", "New Worktrees", undefined],
    ["session-worktrees", "Worktrees", "Removing one deletes its folder and anything in it that isn’t committed. Its branch stays if it isn’t merged yet."],
  ])
  expect(rowOf(shown, "projects-folder")).toMatchObject({ title: "Folder", kind: "folder", placeholder: "None", value: "" })
  expect(rowOf(shown, "worktrees-folder")).toMatchObject({ title: "Folder", kind: "folder", placeholder: "Kuvo Code’s Folder", value: "" })
  expect(rowOf(shown, "projects-folder").folder).toEqual({ choose: "Choose a Folder…", clear: "None", prompt: "Choose", message: "Choose where new projects go. Each one is a folder in it." })
  expect(rowOf(shown, "worktrees-folder").folder).toEqual({ choose: "Choose a Folder…", clear: "Kuvo Code’s Folder", prompt: "Choose", message: "Choose where new worktrees go. Each one is a folder in it." })
  expect(rowOf(shown, "worktrees-place").title).toBe("New worktrees go in")
  expect(rowOf(shown, "worktrees").empty).toBe("None yet. Choose Worktree in a new session’s composer to give it one.")
  const { worktreeRemovalWords, revised } = await import("../src/link/areas/settings-pages")
  expect(worktreeRemovalWords({ branch: "topic", uncommitted: true, ahead: 2 })).toBe("Its folder is deleted, with changes in it that aren’t committed. topic keeps its 2 commits not merged yet. Its sessions stay, without a folder to run in.")
  expect(worktreeRemovalWords({ branch: "topic", uncommitted: false })).toBe("Its folder is deleted. Git sees nothing in it that isn’t committed. Its sessions stay, without a folder to run in.")
  const original = rowOf(shown, "projects-folder")
  expect(revised({ ...original, line: "Reworded" }).revision).toBe(original.revision)
})

test("Worktrees writes through both routes, refuses stale revisions, and unrelated folders still write", async () => {
  const shown = await page("worktrees")
  const projects = rowOf(shown, "projects-folder")
  expect((await change("worktrees", projects, outside)).status).toBe(200)
  expect((await ask(handler, "GET", "kuvo/work/settings")).json.projectsFolder).toBe(outside)
  expect((await change("worktrees", projects, "")).status).toBe(409)
  expect((await change("worktrees", rowOf(shown, "worktrees-folder"), ` ${outside}/../${outside.split("/").at(-1)} `)).status).toBe(200)
  expect((await ask(handler, "GET", "kuvo/config")).json.worktree.directory).toBe(outside)
  const wrong = await change("worktrees", rowOf(await page("worktrees"), "projects-folder"), false)
  expect([wrong.status, wrong.json.message]).toEqual([400, "Send text."])
})

test("work settings optional was is atomic and old calls remain valid", async () => {
  expect((await ask(handler, "PATCH", "kuvo/work/settings", { projectsFolder: outside })).status).toBe(200)
  const before = kept(WORK_SETTINGS_FILE)
  const stale = await ask(handler, "PATCH", "kuvo/work/settings", { projectsFolder: null, was: { projectsFolder: null } })
  expect([stale.status, stale.json.message]).toEqual([409, "It changed since you read it."])
  expect(kept(WORK_SETTINGS_FILE)).toBe(before)
  expect((await ask(handler, "PATCH", "kuvo/work/settings", { projectsFolder: null, was: { projectsFolder: outside } })).status).toBe(200)
})

test("Worktrees explains a field it cannot replace and warns on unreadable files", async () => {
  write(CONFIG_FILE, { worktree: { directory: false } })
  const before = kept(CONFIG_FILE)
  const folder = rowOf(await page("worktrees"), "worktrees-folder")
  expect(folder.can.why).toContain("other than text")
  expect((await change("worktrees", folder, outside)).status).toBe(403)
  expect(kept(CONFIG_FILE)).toBe(before)
  for (const file of [WORK_SETTINGS_FILE, WORKTREES_FILE, PROJECTS_FILE]) {
    writeFileSync(file, "{broken")
    const shown = await page("worktrees")
    expect(shown.groups).toEqual([])
    expect(shown.notes[0].tone).toBe("warning")
    rmSync(file)
  }
})

test("Worktrees events follow config and work settings routes", async () => {
  const events = follow(handler, "kuvo/settings/worktrees/events")
  try {
    await events.next()
    await ask(handler, "PATCH", "kuvo/work/settings", { projectsFolder: outside })
    expect(rowOf(await events.next(), "projects-folder").value).toBe(outside)
    await ask(handler, "PATCH", "kuvo/config", { changes: [{ path: ["worktree", "directory"], value: outside }] })
    expect(rowOf(await events.next(), "worktrees-folder").value).toBe(outside)
    write(WORKTREES_FILE, { worktrees: [{ root: join(outside, "missing"), project: outside, branch: "topic" }] })
    const tree = rowOf(await events.next(), `tree-${join(outside, "missing")}`)
    expect(tree.actions[0]).toMatchObject({ title: "Remove…", confirm: { title: "Remove the worktree for topic?", button: "Remove" } })
    expect(tree.actions[0].can.why).toBe("The engine isn't running, so the worktree of topic stays. Try again once it's started.")
    const before = kept(WORKTREES_FILE)
    const refused = await act("worktrees", tree, "remove")
    expect([refused.status, refused.json.message]).toEqual([403, tree.actions[0].can.why])
    expect(kept(WORKTREES_FILE)).toBe(before)
  } finally { events.stop() }
})


test("Worktrees events follow old record and removal routes, with optional stale protection", async () => {
  const fake = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch() { return Response.json({ data: [] }) } })
  const target = { port: fake.port!, password: "test", pid: process.pid }
  const live = engineHandler(() => target as any, { name: "devbox" }, noEngine as any)
  const root = join(outside, "recorded")
  mkdirSync(root, { recursive: true })
  const record = { root, folder: root, project: outside, branch: "topic" }
  const events = follow(live, "kuvo/settings/worktrees/events")
  try {
    await events.next()
    expect((await ask(live, "POST", "kuvo/worktrees/record", record)).status).toBe(200)
    const shown = await events.next()
    const tree = rowOf(shown, `tree-${root}`)
    expect(tree.actions[0].confirm).toMatchObject({ title: "Remove the worktree for topic?", button: "Remove" })
    const before = kept(WORKTREES_FILE)
    const stale = await ask(live, "POST", "kuvo/worktrees/remove", { path: root, was: {} })
    expect([stale.status, stale.json.message]).toEqual([409, "It changed since you read it."])
    expect(kept(WORKTREES_FILE)).toBe(before)
    // Gone on disk: old remove call still forgets the record without needing was.
    rmSync(root, { recursive: true, force: true })
    expect((await ask(live, "POST", "kuvo/worktrees/remove", { path: root })).status).toBe(200)
    expect(rowOf(await events.next(), "worktrees").items).toEqual([])
  } finally { events.stop(); fake.stop(true) }
})

test("Worktree folder changes refuse a config edit after the page read", async () => {
  const original = rowOf(await page("worktrees"), "worktrees-folder")
  write(CONFIG_FILE, { worktree: { directory: outside } })
  const before = kept(CONFIG_FILE)
  const { worktreesChange } = await import("../src/link/areas/settings-pages")
  const context = { request: { fromApp: true }, info: {}, readEngine: noEngine, bridge: () => null,
    json: (status: number, value: unknown) => [{ head: { status, headers: {} } }, { body: Buffer.from(JSON.stringify(value)) }],
  } as any
  expect(await worktreesChange(context, original, "")).toMatchObject({ status: 409, stale: true })
  expect(await worktreesChange(context, { ...original, id: "unexpected" }, "")).toMatchObject({ status: 400 })
  expect(kept(CONFIG_FILE)).toBe(before)
})

// This computer's own page (PROTOCOL.md, "A box's settings", This computer): how it stands, its
// version and update, "Let my devices connect", and on a Mac what macOS allows Kuvo Helper. What
// only a Mac can say (launchd, its helper, its link) is the test's own here; the page, its words
// and what each row does are the box's.
const { computerFacts } = await import("../src/link/areas/settings-pages")
const realFacts = { ...computerFacts }
const fromApp = (method: string, path: string, body?: unknown) => ask(handler, method, path, body)
/** Asked as a phone or another computer's app asks (not the app beside this box). */
async function fromAway(path: string): Promise<any> {
  let text = ""
  for await (const piece of handler({ method: "GET", path, headers: {} }, undefined)) if ("body" in piece) text += Buffer.from(piece.body).toString()
  return JSON.parse(text)
}
const update = (more: Record<string, unknown> = {}) => ({ setUp: true, version: "1.0.0", engine: "2.2.6", latest: "1.0.0", last: null, updating: null, blocked: null, ...more })
const allowed = (screenRecording: boolean | null, accessibility: boolean | null) => ({
  applies: true, name: "Kuvo Helper", known: screenRecording !== null, screenRecording, accessibility, bundle: "/Users/sam/.kuvo-code/bin/Kuvo Helper.app",
  list: [
    { id: "screenRecording" as const, name: "Screen Recording", settings: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture", byHand: true, allowed: screenRecording, asks: screenRecording === false },
    { id: "accessibility" as const, name: "Accessibility", settings: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility", byHand: true, allowed: accessibility, asks: accessibility === false },
  ],
})
/** A Mac's own box, as its helper keeps it, with its link's switch kept by the test. */
function thisMac(more: Partial<typeof computerFacts> = {}) {
  const mac = { devices: true as boolean | null, restarted: 0 }
  Object.assign(computerFacts, realFacts, {
    kind: () => "ownMac", restarts: () => true, atLogin: () => true, permissions: () => allowed(true, true), update: () => update(), working: async () => false,
    restart: async () => { mac.restarted++; return { status: 202 } }, devices: () => mac.devices, letDevices: async (on: boolean) => { mac.devices = on; return true },
    ...more,
  })
  return mac
}
afterAll(() => { Object.assign(computerFacts, realFacts) })
const onMac = process.platform === "darwin"

test.if(onMac)("This Mac's page says how it stands in its rows' titles: running, its version, and your devices let in, with what macOS allows its helper", async () => {
  thisMac()
  const sections = (await fromApp("GET", "kuvo/settings")).json.sections
  expect(sections.find((section: any) => section.id === "computer").title).toBe("This Mac")
  const shown = (await fromApp("GET", "kuvo/settings/computer")).json
  expect(shown.title).toBe("This Mac")
  expect(shown.groups.map((group: any) => [group.title, group.rows.map((row: any) => [row.title, row.line, row.value])])).toEqual([
    [undefined, [
      ["Running", "Kuvo Helper keeps it running, and starts it at login.", undefined],
      ["Kuvo Code 1.0.0", "Up to date. It updates itself when nothing’s at work.", "1.0.0"],
      ["Let my devices connect", "Your phone and other computers reach this Mac, end to end encrypted.", true],
    ]],
    ["What macOS Allows Kuvo Helper", [
      ["Login Items", "Kuvo starts when you log in to this Mac.", "On"],
      ["Screen Recording", "For agents that look at the screen.", "Allowed"],
      ["Accessibility", "For agents that click and type in other apps.", "Allowed"],
      ["Files and Folders", "macOS asks when an agent first opens Desktop, Documents or Downloads.", "Asked when needed"],
    ]],
  ])
  // No word of a box or a core anywhere on it.
  expect(JSON.stringify(shown)).not.toMatch(/\bbox\b|Kuvo Core/i)
})

test.if(onMac)("what needs you on This Mac is a warning with the way to it: Login Items off, a permission not allowed, an update that's ready", async () => {
  thisMac({ atLogin: () => false, permissions: () => allowed(false, null), update: () => update({ latest: "1.0.1" }), working: async () => true })
  const shown = (await fromApp("GET", "kuvo/settings/computer")).json
  expect(rowOf(shown, "state").line).toBe("Kuvo Helper keeps it running.")
  expect(rowOf(shown, "login-items")).toMatchObject({ tone: "warning", line: "Off, so Kuvo on this Mac doesn’t start at login.", actions: [{ id: "open-login-items", title: "Open Login Items", opens: "x-apple.systempreferences:com.apple.LoginItems-Settings.extension" }] })
  expect(rowOf(shown, "allows-screenRecording")).toMatchObject({ tone: "warning", line: "Not allowed. Agents can’t look at the screen.", actions: [{ title: "Open System Settings", opens: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture" }] })
  expect(rowOf(shown, "allows-accessibility")).toMatchObject({ kind: "status", value: "Not known yet" })
  expect(rowOf(shown, "version")).toMatchObject({ title: "Kuvo Code 1.0.1 is ready", line: "It installs when nothing’s at work, or now." })
  // Work under way is said before it's stopped.
  expect(rowOf(shown, "version").actions[0]).toMatchObject({ title: "Update Now", confirm: { body: "Agents are at work there now: what they’re doing stops. A newer version, if there is one, installs now." } })
  expect(rowOf(shown, "state").actions[0].confirm).toEqual({ title: "Restart Kuvo on this Mac?", body: "Agents are at work there now: what they’re doing stops. Kuvo is back in a moment.", button: "Restart" })

  // From your phone the same page names the Mac, and what opens only on it says so instead.
  const away = await fromAway("kuvo/settings/computer")
  expect(away.title).toBe("devbox")
  expect(rowOf(away, "devices").line).toBe("Your phone and other computers reach devbox, end to end encrypted.")
  expect(rowOf(away, "login-items").actions[0]).toEqual({ id: "open-login-items", title: "Open Login Items", can: { why: "Open it on devbox itself." } })
  expect(rowOf(away, "allows-screenRecording").actions[0].opens).toBeUndefined()
})

test.if(onMac)("Let my devices connect is switched on This Mac's page, asked first as it's turned off, and Restart restarts it", async () => {
  const mac = thisMac()
  let shown = (await fromApp("GET", "kuvo/settings/computer")).json
  expect(rowOf(shown, "devices")).toMatchObject({ kind: "switch", value: true, can: true, confirm: { title: "Stop letting your devices connect?", button: "Turn Off" } })
  shown = (await ask(handler, "PATCH", "kuvo/settings/computer", { row: "devices", value: false, was: rowOf(shown, "devices").revision })).json
  expect([mac.devices, rowOf(shown, "devices").value, rowOf(shown, "devices").confirm]).toEqual([false, false, undefined])
  // (Read before it changed: refused, with the page as it is.)
  const stale = await ask(handler, "PATCH", "kuvo/settings/computer", { row: "devices", value: true, was: "before" })
  expect([stale.status, mac.devices]).toEqual([409, false])

  const restarted = await ask(handler, "POST", "kuvo/settings/computer/state/restart", { was: rowOf(shown, "state").revision })
  expect([restarted.status, mac.restarted]).toEqual([200, 1])
  // One that can't be restarted from here says why, and isn't.
  thisMac({ kind: () => "appMac", restarts: () => false, restart: async () => { mac.restarted++; return { status: 202 } } })
  shown = (await fromApp("GET", "kuvo/settings/computer")).json
  expect(rowOf(shown, "state").actions[0].can).toEqual({ why: "Kuvo Code on devbox restarts it: restart it there." })
  expect((await ask(handler, "POST", "kuvo/settings/computer/state/restart", { was: rowOf(shown, "state").revision })).status).toBe(403)
  expect(mac.restarted).toBe(1)
})

test("a computer you added has the same page without a Mac's rows: no helper, nothing of macOS, and devices always let in", async () => {
  Object.assign(computerFacts, realFacts, { update: () => update({ setUp: false }), working: async () => false })
  const shown = await fromAway("kuvo/settings/computer")
  expect(shown.groups.map((group: any) => group.id)).toEqual(["computer"])
  expect(shown.groups[0].rows.map((row: any) => [row.id, row.kind, row.line])).toEqual([
    ["state", "actions", "It runs whether or not an app is open."],
    ["version", "status", "Updates aren’t set up for devbox yet."],
    ["devices", "status", "Your phone and other computers reach devbox, end to end encrypted. A computer you added always lets them in."],
  ])
})
