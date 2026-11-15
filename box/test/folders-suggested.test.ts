// Folders worth starting in (#733, PROTOCOL.md `kuvo/folders/suggested`): the projects this box's
// agents worked in lately, newest first, then the git repositories directly in ~/Projects and
// ~/Developer; bounded on a home with many folders, and never deeper than one level. A home of the
// test's own.

import "./safety"
import { afterAll, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { CHATS } from "../src/link/home"
import { MOST_SUGGESTED, suggestedFolders } from "../src/link/areas/folders-suggested"

const home = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-suggested-")))
afterAll(() => rmSync(home, { recursive: true, force: true }))
const folder = (at: string, repository = false) => {
  mkdirSync(join(home, at), { recursive: true })
  if (repository) mkdirSync(join(home, at, ".git"))
  return join(home, at)
}

test("the projects agents worked in lately first, newest first, then the repositories in Projects and Developer", () => {
  const website = folder("Projects/website", true)
  const notes = folder("Documents/notes", true)
  const api = folder("work/api", true)
  const scratch = folder("scratch")
  folder("Projects/plain")
  folder("Projects/.hidden", true)
  const dotfiles = folder("Developer/dotfiles", true)
  folder("Projects/deep/inner", true)
  // A linked worktree's `.git` is a file: still a repository.
  const linked = folder("Developer/linked")
  writeFileSync(join(linked, ".git"), "gitdir: ../dotfiles/.git/worktrees/linked\n")
  writeFileSync(join(home, "Projects", "README.md"), "not a folder")
  // A link into a folder macOS asks about isn't followed there.
  folder("Documents/secret", true)
  symlinkSync(join(home, "Documents", "secret"), join(home, "Projects", "secret-link"))
  const box = folder(".kuvo-code")
  mkdirSync(CHATS, { recursive: true })
  const sessions = [
    { kind: "project", directory: website, updated: 300 },
    { kind: "project", directory: scratch, updated: 500 },
    { kind: "project", directory: website + "/", updated: 100 },
    // A worktree's session, in the box's folder: suggested as the project it's of.
    { kind: "project", project: api, directory: join(box, "worktree", "api-1"), updated: 450 },
    { kind: "project", directory: api, updated: 200, archived: true },
    { kind: "bot", directory: dotfiles, updated: 900 },
    { kind: "subagent", directory: dotfiles, updated: 900 },
    // A chat runs in the chats folder (on a Mac, the app's, outside the box's): never suggested.
    { kind: "chat", directory: CHATS, updated: 950 },
    { kind: "project", directory: CHATS, updated: 940 },
    { kind: "project", directory: join(box, "chats", "c1"), updated: 800 },
    { kind: "project", directory: home, updated: 700 },
    { kind: "project", directory: join(home, "gone"), updated: 600 },
    { kind: "project", directory: notes, updated: 50 },
  ]
  const suggested = suggestedFolders(home, sessions, box, "darwin")
  expect(suggested.slice(0, 4)).toEqual([
    { path: scratch, name: "scratch", seen: 500 },
    { path: api, name: "api", repository: true, seen: 450 },
    { path: website, name: "website", repository: true, seen: 300 },
    // In a folder macOS asks about first: suggested as it is, not looked into.
    { path: notes, name: "notes", seen: 50 },
  ])
  // Then the repositories found, each once (website already above), none deeper, none hidden.
  expect(suggested.slice(4).map(each => [each.name, each.repository, each.seen]).sort()).toEqual([["dotfiles", true, null], ["linked", true, null]])
  // On Linux nothing's asked about first: the same folder is looked into, and the link followed.
  const onLinux = suggestedFolders(home, sessions, box, "linux")
  expect(onLinux.find(each => each.path === notes)).toEqual({ path: notes, name: "notes", repository: true, seen: 50 })
  expect(onLinux.some(each => each.name === "secret-link")).toBe(true)
})

test("a home reached through a link: the engine's real paths are the same folders, each once", () => {
  const base = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-suggested-linked-")))
  const real = join(base, "var-home"), linkedHome = join(base, "home")
  try {
    mkdirSync(join(real, "Projects", "app", ".git"), { recursive: true })
    symlinkSync(real, linkedHome)
    const sessions = [
      { kind: "project", directory: join(real, "Projects", "app"), updated: 10 },
      { kind: "project", directory: real, updated: 20 },
      { kind: "project", directory: join(real, ".kuvo-code", "worktree", "x"), updated: 30 },
    ]
    expect(suggestedFolders(linkedHome, sessions, join(linkedHome, ".kuvo-code"), "linux")).toEqual([
      { path: join(real, "Projects", "app"), name: "app", repository: true, seen: 10 },
    ])
  } finally {
    rmSync(base, { recursive: true, force: true })
  }
})

test("a home with many folders: a bounded list, each folder looked at once and nothing deeper (no clock: the bounds are what keep it quick)", () => {
  const many = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-suggested-many-")))
  try {
    for (let at = 0; at < 3000; at++) {
      mkdirSync(join(many, "Projects", `repo-${at}`, ".git"), { recursive: true })
      // One level only: what's deeper is never read.
      if (at < 50) mkdirSync(join(many, "Projects", `repo-${at}`, "nested", ".git"), { recursive: true })
    }
    for (let at = 0; at < 200; at++) mkdirSync(join(many, "Developer", `plain-${at}`), { recursive: true })
    const sessions = Array.from({ length: 5000 }, (_, at) => ({ kind: "project", directory: join(many, "Projects", `repo-${at % 3000}`), updated: at }))
    const suggested = suggestedFolders(many, sessions, join(many, ".kuvo-code"), "linux")
    expect(suggested.length).toBe(MOST_SUGGESTED)
    // Those worked in first (the latest session's folder first), then repositories found.
    expect(suggested[0]).toEqual({ path: join(many, "Projects", "repo-1999"), name: "repo-1999", repository: true, seen: 4999 })
    expect(suggested.filter(each => each.seen !== null).length).toBe(12)
    expect(new Set(suggested.map(each => each.path)).size).toBe(MOST_SUGGESTED)
    expect(suggested.every(each => !each.path.includes("nested"))).toBe(true)
  } finally {
    rmSync(many, { recursive: true, force: true })
  }
})

test("no sessions, no Projects or Developer: nothing to suggest", () => {
  const empty = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-suggested-empty-")))
  try {
    expect(suggestedFolders(empty, [], join(empty, ".kuvo-code"), "linux")).toEqual([])
  } finally {
    rmSync(empty, { recursive: true, force: true })
  }
})

test("the route answers this computer's home and its folders", async () => {
  const { engineHandler } = await import("../src/link/server/router")
  const handler = engineHandler(() => { throw new Error("no engine") }, { name: "test" }, () => { throw new Error("none") })
  const ask = async (method: string, path: string) => {
    let status = 0, text = ""
    for await (const piece of handler({ method, path, headers: {}, body: undefined, fromApp: true }, undefined)) {
      if ("head" in piece) status = piece.head.status
      if ("body" in piece) text += Buffer.from(piece.body).toString()
    }
    return { status, body: JSON.parse(text || "{}") }
  }
  const { status, body } = await ask("GET", "kuvo/folders/suggested")
  expect(status).toBe(200)
  expect(body.home).toBe(process.env.HOME)
  expect(Array.isArray(body.folders)).toBe(true)
  expect((await ask("POST", "kuvo/folders/suggested")).status).toBe(405)
})
