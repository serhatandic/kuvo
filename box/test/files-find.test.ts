import "./safety"
import { afterAll, beforeAll, expect, test } from "bun:test"
import { spawnSync } from "node:child_process"
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { ask } from "./route-answer"

// Finding files as you type (`kuvo/files/find`): what a Files tab's filter and a new tab's "Search
// files or enter a URL" list, the box's answer drawn as it is. A real repository and a plain folder,
// each in a folder of its own; the box's real handler, with no engine (it needs none).

const { engineHandler } = await import("../src/link/server/router")
const { webAddress } = await import("../src/link/areas/files-find")

const noEngine = () => { throw new Error("No engine") }
const handler = engineHandler(noEngine as any, { name: "test" } as any, noEngine as any)

let box: string
// git as it is, without your own settings (a global ignore file would hide what a test makes).
const was = { global: process.env.GIT_CONFIG_GLOBAL, system: process.env.GIT_CONFIG_NOSYSTEM }
beforeAll(() => {
  box = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-files-find-")))
  process.env.GIT_CONFIG_GLOBAL = "/dev/null"
  process.env.GIT_CONFIG_NOSYSTEM = "1"
})
afterAll(() => {
  rmSync(box, { recursive: true, force: true })
  for (const [name, value] of [["GIT_CONFIG_GLOBAL", was.global], ["GIT_CONFIG_NOSYSTEM", was.system]] as const) {
    if (value === undefined) delete process.env[name]
    else process.env[name] = value
  }
})

const git = (folder: string, ...args: string[]) => {
  const ran = spawnSync("git", ["-c", "init.defaultBranch=main", ...args], { cwd: folder, encoding: "utf8" })
  if (ran.status !== 0) throw new Error(`git ${args.join(" ")}: ${ran.stderr}`)
}
const files = (top: string, names: string[]) => {
  for (const name of names) {
    mkdirSync(dirname(join(top, name)), { recursive: true })
    writeFileSync(join(top, name), `${name}\n`)
  }
}
/** A repository of its own, with `names` in it, and git ignoring `node_modules`. */
const repository = (name: string, names: string[]) => {
  const top = join(box, name)
  mkdirSync(top, { recursive: true })
  git(top, "init", "-q")
  files(top, names)
  writeFileSync(join(top, ".gitignore"), "node_modules/\n")
  return top
}
const find = (directory: string, q: string, more = "") => ask(handler, "GET", `kuvo/files/find?directory=${encodeURIComponent(directory)}&q=${encodeURIComponent(q)}${more}`)

test("in a repository: a name match comes before a path-only match, the shorter path first, and nothing git ignores", async () => {
  const top = repository("site", ["src/forms/signup-form.tsx", "src/signup.ts", "docs/signup/notes.md", "src/sign.ts", "node_modules/pkg/signup.js", "README.md"])
  const said = await find(top, "SignUp")
  expect(said.status).toBe(200)
  expect(said.json.url).toBeNull()
  expect(said.json.files).toEqual([
    { path: join(top, "src/signup.ts"), name: "signup.ts", folder: "src" },
    { path: join(top, "src/forms/signup-form.tsx"), name: "signup-form.tsx", folder: "src/forms" },
    { path: join(top, "docs/signup/notes.md"), name: "notes.md", folder: "docs/signup" },
  ])
  // Its letters in order, a name's before a path's; a file at the top is in "".
  expect((await find(top, "rdme")).json.files).toEqual([{ path: join(top, "README.md"), name: "README.md", folder: "" }])
  expect((await find(top, "sgnp")).json.files.map((each: { name: string }) => each.name)).toEqual(["signup.ts", "signup-form.tsx", "notes.md"])
  // At most as many as asked for; nothing typed, nothing found.
  expect((await find(top, "s", "&limit=2")).json.files).toHaveLength(2)
  expect(await find(top, "   ")).toEqual({ status: 200, json: { url: null, files: [] } })
})

test("outside a repository: a walk that skips .git, node_modules and builds", async () => {
  const top = join(box, "plain")
  files(top, ["notes/todo.txt", "node_modules/x/todo.js", "build/todo.o", "todo.txt"])
  expect((await find(top, "todo")).json.files.map((each: { path: string }) => each.path)).toEqual([join(top, "todo.txt"), join(top, "notes/todo.txt")])
})

test("refused without a folder, and as kuvo/folders refuses one it can't read", async () => {
  expect((await ask(handler, "GET", "kuvo/files/find?q=signup")).status).toBe(400)
  const gone = await find(join(box, "nowhere"), "signup")
  expect(gone).toEqual({ status: 404, json: { message: `There's no folder ${join(box, "nowhere")} here.`, reason: "No such folder" } })
  expect((await find("relative/folder", "signup")).status).toBe(400)
})

test("what's typed reads as a web address only where it is one, never a file's name", async () => {
  const top = repository("addresses", ["src/signup.ts"])
  expect((await find(top, "localhost:3000")).json.url).toBe("http://localhost:3000")
  const file = (await find(top, "signup.ts")).json
  expect(file.url).toBeNull()
  expect(file.files[0]).toEqual({ path: join(top, "src/signup.ts"), name: "signup.ts", folder: "src" })
  const reads: [string, string | null][] = [
    ["https://example.com/a?b=1", "https://example.com/a?b=1"],
    ["HTTP://Example.com", "HTTP://Example.com"],
    ["example.com", "https://example.com"],
    ["kuvo.run", "https://kuvo.run"],
    ["docs.example.dev:8080/guide", "https://docs.example.dev:8080/guide"],
    ["github.com/owner/repo", "https://github.com/owner/repo"],
    ["localhost", "http://localhost"],
    ["localhost:5173/app", "http://localhost:5173/app"],
    ["127.0.0.1:8080", "http://127.0.0.1:8080"],
    ["192.168.1.20/admin", "http://192.168.1.20/admin"],
    ["[::1]:3000", "http://[::1]:3000"],
    // An unknown ending reads as an address only with a path or a port.
    ["foo.bar", null],
    ["foo.bar/x", "https://foo.bar/x"],
    // A file's name, however it's written.
    ["signup.ts", null],
    ["notes.md", null],
    ["install.sh", null],
    ["main.go:12", null],
    ["index.html#top", null],
    ["src/main.rs", null],
    ["package.json", null],
    // Neither.
    ["", null],
    ["two words.com", null],
    ["https://", null],
    ["ftp://example.com", null],
    ["mailto:me@example.com", null],
    ["me@example.com", null],
    ["1.2.3", null],
    ["999.1.1.1", null],
    ["localhost:70000", null],
  ]
  expect(reads.map(([typed]) => [typed, webAddress(typed)])).toEqual(reads)
})

test("a question mark in what's typed or in a folder's name is part of it, as the box's server sends it", async () => {
  // (The server sends a query's `?` as it is, past the first: `canonicalPath`.)
  const top = repository("asked?here", ["src/signup.ts"])
  const said = await ask(handler, "GET", `kuvo/files/find?directory=${top}&q=https://example.com/a?b%3D1`)
  expect(said).toEqual({ status: 200, json: { url: "https://example.com/a?b=1", files: [] } })
  const listed = await ask(handler, "GET", `kuvo/folders?path=${top}`)
  expect([listed.status, listed.json.path]).toEqual([200, top])
  const tree = await ask(handler, "GET", `kuvo/folders/tree?path=${top}`)
  expect([tree.status, tree.json.path]).toEqual([200, top])
})
