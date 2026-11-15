import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { mkdirSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { ask, follow } from "./route-answer"

// What a conversation waits on you for (PROTOCOL.md, "A conversation's asks"): every client draws
// the cards from the box's words alone, and answers through it, to the session that asked (a
// subagent's too). Only the engine is a stand-in, keeping each session's asks as the engine does.

const { engineHandler } = await import("../src/link/server/router")
const { forgetAsks, noteAskEvent } = await import("../src/link/glance/asked")

const home = process.env.HOME!
const work = join(process.env.KUVO_HOME!, "asks-work")
mkdirSync(work, { recursive: true })

// The engine: its sessions, each one's permissions and forms, a subagent's messages, and what it
// was asked to do, each time.
let sessions: Record<string, any>, permissions: Record<string, any[]>, forms: Record<string, any[]>, messages: Record<string, any[]>
let got: { method: string; path: string; body: any }[], refusing: Set<string>
// An MCP server's forms, filed under no session in a folder; the folder each was asked in; whether
// listing a session's subagents fails (a busy engine).
let shared: Record<string, any[]>, folders: Record<string, string>, listingFails: boolean, active: string[]
beforeEach(() => {
  forgetAsks()
  got = []
  refusing = new Set()
  shared = {}
  folders = {}
  listingFails = false
  active = []
  sessions = {
    ses_root: { id: "ses_root", location: { directory: work }, time: { created: 1, updated: 1 } },
    ses_child: { id: "ses_child", parentID: "ses_root", location: { directory: work }, time: { created: 2, updated: 2 } },
  }
  permissions = {
    ses_root: [
      { id: "per_edit", sessionID: "ses_root", action: "edit", resources: [`${work}/src/App.swift`, `${work}/b.swift`, `${work}/c.swift`], save: ["*"],
        metadata: { files: [{ file: `${work}/src/App.swift`, patch: "@@ -1 +1 @@\n-a\n+b\n", additions: 1, deletions: 1, status: "modified" }] } },
      { id: "per_out", sessionID: "ses_root", action: "external_directory", resources: [`${home}/notes/*`] },
    ],
    ses_child: [
      { id: "per_push", sessionID: "ses_child", action: "shell", resources: ["git push origin main"], save: ["git push *"], source: { type: "tool", messageID: "msg_1", id: "call_1" }, message: "It wants to push." },
    ],
  }
  forms = {
    ses_root: [
      { id: "que_1", sessionID: "ses_root", title: "Pick", metadata: { kind: "question" }, fields: [
        { key: "color", type: "string", title: "Color", description: "Which color?", required: true, options: [{ value: "blue", label: "Blue", description: "Calm" }, { value: "red", label: "Red" }] },
        { key: "extras", type: "multiselect", title: "Extras", options: [{ value: "docs", label: "Docs" }], custom: true },
        { key: "kept", type: "string", hidden: true, default: "as before" },
      ] },
      { id: "frm_mcp", sessionID: "ses_root", title: "GitHub", metadata: { kind: "mcp-elicitation", server: "github", message: "Pick a repository." }, fields: [{ key: "repo", type: "number" }] },
    ],
  }
  messages = { ses_child: [{ id: "msg_1", type: "assistant", content: [{ type: "tool", id: "call_1", name: "shell", state: { status: "running", input: { command: "cd app && git push origin main" } } }] }] }
})

const engine = Bun.serve({
  port: 0,
  async fetch(request) {
    const url = new URL(request.url)
    const body = request.method === "GET" ? undefined : await request.json().catch(() => undefined)
    got.push({ method: request.method, path: url.pathname, body })
    const reply = (value: unknown, status = 200) => Response.json(value, { status })
    const folder = request.headers.get("x-opencode-directory")
    if (folder) folders[`${request.method} ${url.pathname}`] = decodeURIComponent(folder)
    if (url.pathname === "/api/session/active") return reply({ data: Object.fromEntries(active.map(id => [id, {}])) })
    if (url.pathname === "/api/session") {
      if (listingFails) return reply({ message: "Busy." }, 500)
      const parent = url.searchParams.get("parentID")
      return reply({ data: Object.values(sessions).filter(each => (each.parentID ?? "null") === parent) })
    }
    const [, id, kind, ask, step] = url.pathname.match(/^\/api\/session\/([^/]+)(?:\/([^/]+)(?:\/([^/]+)(?:\/([^/]+))?)?)?$/) ?? []
    if (id === "ses_broken") return reply({ message: "Busy." }, 500)
    if (id === "global" && kind === "form") {
      const here = shared[decodeURIComponent(folder ?? "")] ?? []
      if (!ask) return reply({ data: here })
      if (!here.some(each => each.id === ask)) return reply({ message: "Not asked." }, 404)
      shared[decodeURIComponent(folder ?? "")] = here.filter(each => each.id !== ask)
      return reply({ data: true })
    }
    if (!id || !sessions[id]) return reply({ message: "No such session." }, 404)
    if (!kind) return reply({ data: sessions[id] })
    if (kind === "message") return reply({ data: messages[id] ?? [] })
    const list = kind === "permission" ? permissions : kind === "form" ? forms : null
    if (!list) return reply({ message: "Not here." }, 404)
    if (!ask) return reply({ data: list[id] ?? [] })
    if (!(list[id] ?? []).some(each => each.id === ask)) return reply({ message: "Not asked." }, 404)
    if (refusing.has(ask)) return reply({ message: "It can't take that now." }, 400)
    if ((step === "reply" && request.method === "POST") || (!step && request.method === "DELETE")) {
      list[id] = list[id]!.filter(each => each.id !== ask)
      return reply({ data: true })
    }
    return reply({ message: "Not here." }, 404)
  },
})
const target = { port: engine.port, password: "pw" } as any
const handler = engineHandler(() => target, { name: "test" }, () => { throw new Error("none") })
afterAll(() => {
  engine.stop(true)
  forgetAsks()
})

const asked = (path: string) => got.filter(each => each.method !== "GET" && each.path === path)

/** The sandbox's asks, as its tool asks them (fence/outside.ts): none left from another test. */
async function sandboxBook() {
  const { listSandboxAsks, answerSandboxAsk, askSandbox } = await import("../src/link/fence/outside")
  for (const each of listSandboxAsks()) answerSandboxAsk(each.id, { answer: "no" })
  return { book: { ask: askSandbox } }
}

/** `method path` asked by something on this computer, as the app (`fromApp`) or not. */
async function askAs(fromApp: boolean, method: string, path: string, body?: unknown): Promise<{ status: number; json: any }> {
  let status = 0, text = ""
  for await (const piece of handler({ method, path, headers: {}, fromApp, ...(body !== undefined ? { body: Buffer.from(JSON.stringify(body)) } : {}) } as any, undefined)) {
    if ("head" in piece) status = piece.head.status
    if ("body" in piece) text += Buffer.from(piece.body).toString()
  }
  return { status, json: text ? JSON.parse(text) : undefined }
}

/** What a Mac asks this box over SSH (`kuvo-link.js box route`): a process of its own, which passes
 * the conversation's asks to the one keeping the box's own (this test's). */
async function boxRoute(method: string, path: string, body?: unknown): Promise<{ status: number; json: any }> {
  const script = join(import.meta.dir, "..", "src", "link", "main.ts")
  const kuvo = process.env.KUVO_HOME!
  mkdirSync(join(kuvo, "route-home"), { recursive: true })
  const child = Bun.spawn([process.execPath, script, "box", "route"], { env: { PATH: process.env.PATH, BUN_BE_BUN: "1", KUVO_HOME: kuvo, HOME: join(kuvo, "route-home"), TMPDIR: join(kuvo, "route-home") }, stdin: "pipe", stdout: "pipe", stderr: "inherit" })
  child.stdin.write(JSON.stringify({ method, path, ...(body === undefined ? {} : { body }) }))
  await child.stdin.end()
  const out = await new Response(child.stdout).text()
  await child.exited
  const answer = JSON.parse(out.trim().split("\n").at(-1)!)
  return { status: answer.status, json: JSON.parse(answer.body) }
}

test("a conversation's asks, its subagents' too, come worded as the cards show them, oldest first", async () => {
  // When each came, as the box heard them (one it didn't hear came before: first).
  noteAskEvent({ type: "form.created", data: { form: { id: "que_1", sessionID: "ses_root" } } }, "engine", 1_000)
  noteAskEvent({ type: "permission.asked", data: { id: "per_push", sessionID: "ses_child" } }, "engine", 2_000)
  noteAskEvent({ type: "permission.asked", data: { id: "per_edit", sessionID: "ses_root" } }, "engine", 3_000)
  // (An MCP server's form, asked first, but answered elsewhere: after those a client answers.)
  noteAskEvent({ type: "form.created", data: { form: { id: "frm_mcp", sessionID: "ses_root" } } }, "engine", 500)
  const { status, json } = await ask(handler, "GET", "kuvo/sessions/ses_root/asks")
  expect(status).toBe(200)
  expect(json.asks.map((each: any) => each.id)).toEqual(["per_out", "que_1", "per_push", "per_edit", "frm_mcp"])
  const [out, question, push, edit, other] = json.asks
  expect(edit).toEqual({
    id: "per_edit", kind: "permission", session: "ses_root", title: "Allow editing src/App.swift and 2 more?", message: null, command: null,
    files: [{ file: `${work}/src/App.swift`, patch: "@@ -1 +1 @@\n-a\n+b\n", additions: 1, deletions: 1, status: "modified" }],
    resources: ["src/App.swift", "b.swift", "c.swift"],
    choices: [
      { answer: "once", title: "Allow Once", named: null, placeholder: null, help: null, spoken: null },
      { answer: "always", title: "Always Allow Edits", named: null, placeholder: null, help: "Don’t ask again before changing files in this project", spoken: "Always allow edits in this project" },
      { answer: "reject", title: "Don’t Allow", named: null, placeholder: null, help: null, spoken: null },
      { answer: "instead", title: "Tell It What to Do Instead…", named: null, placeholder: "Tell it what to do instead…", help: null, spoken: null },
    ],
  })
  // A subagent's, in its own name, with the whole command line its tool call runs.
  expect(push).toMatchObject({ session: "ses_child", title: "Allow running this command?", message: "It wants to push.", command: "cd app && git push origin main", resources: ["git push origin main"] })
  // Always's words whole, for its hover text and a screen reader.
  expect(push.choices[1]).toEqual({ answer: "always", title: "Always Allow", named: "git push", placeholder: null, help: "Don’t ask again before running git push commands in this project", spoken: "Always allow git push commands" })
  // A folder outside, as people know it.
  expect(out).toMatchObject({ title: "Allow working outside the project?", resources: ["~/notes"] })
  expect(out.choices[1]).toMatchObject({ title: "Always Allow", named: "~/notes", help: "Don’t ask again before working in ~/notes", spoken: "Always allow working in Home, notes" })
  expect(question).toEqual({ id: "que_1", kind: "question", session: "ses_root", title: "Pick", questions: [
    { key: "color", title: "Color", question: "Which color?", options: [{ value: "blue", label: "Blue", description: "Calm" }, { value: "red", label: "Red", description: null }], multiple: false, custom: false, required: true },
    { key: "extras", title: "Extras", question: "Extras", options: [{ value: "docs", label: "Docs", description: null }], multiple: true, custom: true, required: false },
  ] })
  expect(other).toEqual({ id: "frm_mcp", kind: "other", session: "ses_root", title: "GitHub", message: "Pick a repository.", dismiss: true })
})

test("Always's words, whole and as they're said, are the apps' own, case for case", async () => {
  const { alwaysWords } = await import("../src/link/notices/news")
  const vectors = JSON.parse(readFileSync(join(import.meta.dir, "../../protocol/vectors/always-words.json"), "utf8"))
  for (const each of vectors.cases) {
    const words = alwaysWords(each, each.home ?? null)
    expect({ plain: words.named === null ? words.title : `${words.title} ${words.named}`, help: words.help, spoken: words.spoken }).toEqual({ plain: each.plain, help: each.help, spoken: each.spoken })
  }
  // A coding agent's Always lasts for its conversation.
  expect(alwaysWords({ action: "edit", save: ["*"] }, null, true).help).toBe("Don’t ask again before changing files in this conversation")
})

test("an answer goes to the session that asked, as the engine takes it", async () => {
  // Tell It What to Do Instead: Don't Allow, with your words.
  expect((await ask(handler, "POST", "kuvo/sessions/ses_root/asks/per_push", { answer: "instead", message: " Push a branch instead. " })).status).toBe(200)
  expect(asked("/api/session/ses_child/permission/per_push/reply")).toEqual([{ method: "POST", path: "/api/session/ses_child/permission/per_push/reply", body: { decision: "reject", message: "Push a branch instead." } }])
  expect((await ask(handler, "POST", "kuvo/sessions/ses_root/asks/per_edit", { answer: "always" })).status).toBe(200)
  expect(asked("/api/session/ses_root/permission/per_edit/reply")[0]!.body).toEqual({ decision: "always" })
  // Answered: it isn't waiting any more, and isn't listed.
  expect((await ask(handler, "POST", "kuvo/sessions/ses_root/asks/per_edit", { answer: "once" })).status).toBe(404)
  expect((await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.map((each: any) => each.id)).not.toContain("per_edit")
  // Instead without words, or an answer it hasn't: refused, nothing sent.
  expect((await ask(handler, "POST", "kuvo/sessions/ses_root/asks/per_out", { answer: "instead", message: "  " })).status).toBe(400)
  expect((await ask(handler, "POST", "kuvo/sessions/ses_root/asks/per_out", { answer: "maybe" })).status).toBe(400)
  expect(asked("/api/session/ses_root/permission/per_out/reply")).toEqual([])
})

test("a question's answers are checked against what it asks, and go with what wasn't asked", async () => {
  const send = (body: unknown) => ask(handler, "POST", "kuvo/sessions/ses_root/asks/que_1", body)
  // A required one missing, a choice it doesn't offer, a key it doesn't ask: refused.
  expect((await send({ answers: { extras: ["docs"] } })).json.message).toContain("Color")
  expect((await send({ answers: { color: "green" } })).status).toBe(400)
  expect((await send({ answers: { color: "blue", size: "big" } })).status).toBe(400)
  expect(asked("/api/session/ses_root/form/que_1/reply")).toEqual([])
  // Words of its own where it takes them; the hidden field's default with them.
  expect((await send({ answers: { color: "blue", extras: ["docs", "tests"] } })).status).toBe(200)
  expect(asked("/api/session/ses_root/form/que_1/reply")[0]!.body).toEqual({ answer: { kept: "as before", color: "blue", extras: ["docs", "tests"] } })
})

test("a form a client can't answer is dismissed from it, never answered", async () => {
  expect((await ask(handler, "POST", "kuvo/sessions/ses_root/asks/frm_mcp", { answers: { repo: 1 } })).status).toBe(400)
  expect((await ask(handler, "POST", "kuvo/sessions/ses_root/asks/frm_mcp", { dismiss: true })).status).toBe(200)
  expect(asked("/api/session/ses_root/form/frm_mcp")).toEqual([{ method: "DELETE", path: "/api/session/ses_root/form/frm_mcp", body: undefined }])
})

test("a command a subagent asks to run outside the sandbox shows with its asks, worded as its card, and is answered from here", async () => {
  const { book } = await sandboxBook()
  const asked = book.ask({ session: "ses_child", botName: "Scout", project: work, command: "npm install", why: "To add the parser.", stopped: [{ kind: "write", path: "/Users/me/.npm/_cacache/x" }], always: { kind: "write", path: "/Users/me/.npm" }, owner: { kind: "bot", id: "bot_scout", name: "Scout" } } as any)
  const listed = (await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.find((each: any) => each.kind === "sandbox")
  expect(listed).toEqual({
    id: listed.id, kind: "sandbox", session: "ses_child", title: "Scout wants to run a command outside the sandbox",
    message: "The sandbox stopped it from changing a file outside Scout’s folder.", command: "npm install",
    stopped: [{ label: "Write", detail: "~/.npm/_cacache/x" }], error: null, asker: "Scout", why: "To add the parser.",
    choices: [
      { answer: "once", title: "Allow Once", named: null, placeholder: null, help: "Runs it outside the sandbox this once", spoken: "Allow once: run it outside the sandbox" },
      { answer: "always", title: "Always Allow", named: "~/.npm", placeholder: null, help: "Runs it, and from now on lets Scout’s commands write in ~/.npm too, in the sandbox.", spoken: "Always Allow ~/.npm" },
      { answer: "reject", title: "Don’t Allow", named: null, placeholder: null, help: "It goes on without running it", spoken: null },
    ],
  })
  // (Answered by a client, never put away unanswered, nor with words it doesn't take.)
  expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${listed.id}`, { dismiss: true })).status).toBe(400)
  expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${listed.id}`, { answer: "instead", message: "no" })).status).toBe(400)
  expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${listed.id}`, { answer: "always" })).status).toBe(200)
  expect(await asked).toEqual({ answer: "always", client: "Mac" })
  // Answered meanwhile elsewhere: it isn't waiting any more.
  const stale = await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${listed.id}`, { answer: "once" })
  expect(stale).toEqual({ status: 404, json: { message: "It isn't waiting any more." } })
  expect((await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.some((each: any) => each.kind === "sandbox")).toBe(false)
})

test("a command the sandbox can't say what it stopped (Linux) shows its error, and is allowed once or not at all", async () => {
  const { book } = await sandboxBook()
  const once = book.ask({ session: "ses_root", project: work, command: "make", why: "To build it.", error: "make: cc: Permission denied", machine: "devbox" } as any)
  let listed = (await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.find((each: any) => each.kind === "sandbox")
  expect(listed).toMatchObject({ title: "An agent wants to run a command outside the sandbox", message: "On devbox, the sandbox stopped it with this error.", stopped: [], error: "make: cc: Permission denied" })
  expect(listed.choices.map((each: any) => each.answer)).toEqual(["once", "reject"])
  // Always isn't offered, so it isn't taken.
  expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${listed.id}`, { answer: "always" })).json.message).toBe("Answer once, reject.")
  expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${listed.id}`, { answer: "once" })).status).toBe(200)
  expect(await once).toEqual({ answer: "once", client: "Mac" })
  const no = book.ask({ session: "ses_root", project: work, command: "make", why: "Again." } as any)
  listed = (await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.find((each: any) => each.kind === "sandbox")
  expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${listed.id}`, { answer: "reject" })).status).toBe(200)
  expect(await no).toEqual({ answer: "no", client: "Mac" })
})

test("what a subagent asks of the vault shows with its asks, and only the vault's clients answer it", async () => {
  const { ask: askVault } = await import("../src/link/vault/asks")
  const answered = askVault({ kind: "use", session: "ses_child", bot: "bot_scout", botName: "Scout", itemName: "GitHub token", why: "To open the pull request.", site: "github.com" })
  const { status, json } = await ask(handler, "GET", "kuvo/sessions/ses_root/asks")
  expect(status).toBe(200)
  const vault = json.asks.find((each: any) => each.kind === "vault")
  expect(vault).toEqual({
    id: vault.id, kind: "vault", session: "ses_child", need: "use", title: "Scout wants to use GitHub token", message: "To open the pull request.", site: "github.com", note: null,
    choices: [
      { answer: "once", title: "Allow Once", named: null, placeholder: null, help: "Lets it use “GitHub token” this once", spoken: "Allow once: let it use “GitHub token”" },
      { answer: "always", title: "Always Allow", named: null, placeholder: null, help: "Lets Scout use “GitHub token” from now on, without asking", spoken: "Always allow Scout to use “GitHub token”", confirm: true },
      { answer: "reject", title: "Don’t Allow", named: null, placeholder: null, help: "It goes on without it", spoken: null },
    ],
  })
  // Something on this computer that isn't one of your clients (not the app, a phone, nor the Mac
  // proving itself): refused, and it still waits.
  const refused = await askAs(false, "POST", `kuvo/sessions/ses_root/asks/${vault.id}`, { answer: "always" })
  expect(refused.status).toBe(403)
  expect((await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.some((each: any) => each.id === vault.id)).toBe(true)
  expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${vault.id}`, { answer: "always" })).status).toBe(200)
  expect(await answered).toEqual({ answer: "always", client: "Mac", by: "mac" })
  expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${vault.id}`, { answer: "once" })).status).toBe(404)
})

test("an item a conversation needs from the vault is given from its card, one of the vault's items, to its clients alone", async () => {
  const { ask: askVault } = await import("../src/link/vault/asks")
  const { addItem, forgetKeys, makeVault, VAULT_FOLDER } = await import("../src/link/vault/store")
  rmSync(VAULT_FOLDER, { recursive: true, force: true })
  forgetKeys()
  try {
    makeVault()
    const item = await addItem({ kind: "key", name: "OpenAI key", values: { key: "sk-test" } })
    const answered = askVault({ kind: "item", session: "ses_root", need: "an OpenAI API key", why: "To run the evals." })
    const vault = (await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.find((each: any) => each.kind === "vault")
    expect(vault).toMatchObject({ need: "item", title: "An agent needs an OpenAI API key", message: "To run the evals.", note: null })
    expect(vault.choices).toEqual([
      { answer: "once", title: "Give", named: "OpenAI key", placeholder: null, help: "Gives it “OpenAI key” this once", spoken: "Give OpenAI key, this once", item: item.id },
      { answer: "always", title: "Always Give", named: "OpenAI key", placeholder: null, help: "Gives it “OpenAI key”, and lets this project use it from now on, without asking", spoken: "Always give OpenAI key", item: item.id, confirm: true },
      { answer: "reject", title: "Don’t Allow", named: null, placeholder: null, help: "It goes on without it", spoken: null },
    ])
    // Read by what isn't a client: no item's name, and why.
    const hidden = (await askAs(false, "GET", "kuvo/sessions/ses_root/asks")).json.asks.find((each: any) => each.kind === "vault")
    expect(hidden.choices.map((each: any) => each.answer)).toEqual(["reject"])
    expect(hidden.note).toBe("Give it an item in Kuvo Code on your Mac or iPhone.")
    // Give needs the item; one it doesn't offer isn't taken.
    expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${vault.id}`, { answer: "once" })).json.message).toBe("Say which item to give it.")
    expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${vault.id}`, { answer: "once", item: "itm_none" })).status).toBe(400)
    expect((await ask(handler, "POST", `kuvo/sessions/ses_root/asks/${vault.id}`, { answer: "once", item: item.id })).status).toBe(200)
    expect(await answered).toEqual({ answer: "once", item: item.id, client: "Mac", by: "mac" })
  } finally {
    forgetKeys()
    rmSync(VAULT_FOLDER, { recursive: true, force: true })
  }
})

test("through `box route` (a machine over SSH), the box's own asks are listed and answered by the process that keeps them", async () => {
  const { book } = await sandboxBook()
  const { holdVault } = await import("../src/link/vault/holder")
  const held = holdVault(handler, () => () => {})
  try {
    const asked = book.ask({ session: "ses_child", project: work, command: "brew install jq", why: "To read the JSON." } as any)
    const listed = await boxRoute("GET", "kuvo/sessions/ses_root/asks")
    expect(listed.status).toBe(200)
    const sandbox = listed.json.asks.find((each: any) => each.kind === "sandbox")
    expect(sandbox).toMatchObject({ session: "ses_child", command: "brew install jq" })
    // With the engine's own, as here.
    expect(listed.json.asks.map((each: any) => each.id)).toContain("per_push")
    expect((await boxRoute("POST", `kuvo/sessions/ses_root/asks/${sandbox.id}`, { answer: "once" })).status).toBe(200)
    expect(await asked).toEqual({ answer: "once", client: "Mac" })
    expect((await boxRoute("POST", `kuvo/sessions/ses_root/asks/${sandbox.id}`, { answer: "once" })).status).toBe(404)
  } finally {
    held.close()
  }
}, 30_000)

test("an MCP server's form its subagent's step waits on shows in the conversation, and goes away in its folder", async () => {
  shared[work] = [{ id: "frm_issue", sessionID: "global", title: "New issue", metadata: { kind: "mcp-elicitation", server: "github", message: "Which repository?" }, fields: [{ key: "repo", type: "number" }] }]
  // Nothing of the server's runs yet: not this conversation's.
  expect((await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.some((each: any) => each.id === "frm_issue")).toBe(false)
  messages.ses_child!.push({ id: "msg_2", type: "assistant", content: [{ type: "tool", id: "call_2", name: "github_create_issue", state: { status: "running", input: {} } }] })
  active = ["ses_child"]
  const listed = (await ask(handler, "GET", "kuvo/sessions/ses_root/asks")).json.asks.find((each: any) => each.id === "frm_issue")
  expect(listed).toEqual({ id: "frm_issue", kind: "other", session: "ses_root", title: "New issue", message: "Which repository?", dismiss: true })
  expect((await ask(handler, "POST", "kuvo/sessions/ses_root/asks/frm_issue", { dismiss: true })).status).toBe(200)
  expect(asked("/api/session/global/form/frm_issue")).toEqual([{ method: "DELETE", path: "/api/session/global/form/frm_issue", body: undefined }])
  expect(folders["DELETE /api/session/global/form/frm_issue"]).toBe(work)
})

test("an MCP server's form two conversations in a folder wait on is one conversation's only, as its notification names it", async () => {
  shared[work] = [{ id: "frm_two", sessionID: "global", title: "New issue", metadata: { kind: "mcp-elicitation", server: "github", message: "Which repository?" }, fields: [{ key: "repo", type: "number" }] }]
  sessions.ses_other = { id: "ses_other", location: { directory: work }, time: { created: 3, updated: 3 } }
  messages.ses_other = [{ id: "msg_o", type: "assistant", content: [{ type: "tool", id: "call_o", name: "github_list_repos", state: { status: "running", input: {} } }] }]
  messages.ses_child!.push({ id: "msg_3", type: "assistant", content: [{ type: "tool", id: "call_3", name: "github_create_issue", state: { status: "running", input: {} } }] })
  active = ["ses_other", "ses_child"]
  const ids = async (session: string) => (await ask(handler, "GET", `kuvo/sessions/${session}/asks`)).json.asks.map((each: any) => each.id)
  expect(await ids("ses_other")).toContain("frm_two")
  expect(await ids("ses_root")).not.toContain("frm_two")
})

test("a busy engine that couldn't list its subagents says so, rather than their asks seeming gone", async () => {
  listingFails = true
  const { status, json } = await ask(handler, "GET", "kuvo/sessions/ses_root/asks")
  expect(status).toBe(503)
  expect(json.message).toContain("isn't answering")
})

test("one the engine refuses says why; no such conversation, or one whose place doesn't answer, says so", async () => {
  refusing.add("per_out")
  const refused = await ask(handler, "POST", "kuvo/sessions/ses_root/asks/per_out", { answer: "once" })
  expect(refused).toEqual({ status: 502, json: { message: "It can't take that now." } })
  expect((await ask(handler, "GET", "kuvo/sessions/ses_none/asks")).status).toBe(404)
  expect((await ask(handler, "GET", "kuvo/sessions/ses_broken/asks")).status).toBe(503)
  const nowhere = engineHandler(() => { throw new Error("none") }, { name: "test" }, () => { throw new Error("none") })
  expect((await ask(nowhere, "GET", "kuvo/sessions/ses_root/asks")).status).toBe(503)
})

test("followed, the asks come again as one comes or is answered", async () => {
  const stream = follow(handler, "kuvo/sessions/ses_root/asks/events")
  try {
    expect((await stream.next()).asks.map((each: any) => each.id)).toEqual(["per_edit", "per_out", "que_1", "per_push", "frm_mcp"])
    // A subagent asks: the box's long-running process hears it, and every follower reads again.
    permissions.ses_child!.push({ id: "per_read", sessionID: "ses_child", action: "read", resources: [`${work}/secrets.txt`] })
    noteAskEvent({ type: "permission.asked", data: { id: "per_read", sessionID: "ses_child" } }, "engine")
    const now = (await stream.next()).asks
    // (The newest a client answers, ahead of the form answered elsewhere.)
    expect(now.map((each: any) => each.id).slice(-2)).toEqual(["per_read", "frm_mcp"])
    expect(now.at(-2)).toMatchObject({ id: "per_read", title: "Allow reading secrets.txt?", resources: ["secrets.txt"] })
    // Answered on another client: gone.
    permissions.ses_child = permissions.ses_child!.filter(each => each.id !== "per_read")
    noteAskEvent({ type: "permission.replied", data: { requestID: "per_read", sessionID: "ses_child" } }, "engine")
    expect((await stream.next()).asks.map((each: any) => each.id)).not.toContain("per_read")
  } finally {
    stream.stop()
  }
})
