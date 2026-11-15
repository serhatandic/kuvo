// Folders worth starting in (#733, PROTOCOL.md `kuvo/folders/suggested`): where this box's agents
// worked lately, newest first, then the git repositories directly in ~/Projects and ~/Developer;
// bounded and quick on a home with many folders, and never deeper than one level. A home of the
// test's own.

import "./safety"
import { afterAll, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { MOST_SUGGESTED, suggestedFolders } from "../src/link/areas/folders-suggested"

const home = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-suggested-")))
afterAll(() => rmSync(home, { recursive: true, force: true }))
const folder = (at: string, repository = false) => {
  mkdirSync(join(home, at), { recursive: true })
  if (repository) mkdirSync(join(home, at, ".git"))
  return join(home, at)
}

test("where agents worked lately first, newest first, then the repositories in Projects and Developer", () => {
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
  const box = folder(".kuvo-code")
  const sessions = [
    { kind: "project", directory: website, updated: 300 },
    { kind: "chat", directory: scratch, updated: 500 },
    { kind: "project", directory: website + "/", updated: 100 },
    { kind: "project", directory: api, updated: 200, archived: true },
    { kind: "bot", directory: api, updated: 900 },
    { kind: "subagent", directory: api, updated: 900 },
    { kind: "chat", directory: join(box, "chats", "c1"), updated: 800 },
    { kind: "chat", directory: home, updated: 700 },
    { kind: "project", directory: join(home, "gone"), updated: 600 },
    { kind: "project", directory: notes, updated: 50 },
  ]
  const suggested = suggestedFolders(home, sessions, box, "darwin")
  expect(suggested.slice(0, 3)).toEqual([
    { path: scratch, name: "scratch", seen: 500 },
    { path: website, name: "website", repository: true, seen: 300 },
    // In a folder macOS asks about first: suggested as it is, not looked into.
    { path: notes, name: "notes", seen: 50 },
  ])
  // Then the repositories found, each once (website already above), none deeper, none hidden.
  expect(suggested.slice(3).map(each => [each.name, each.repository, each.seen]).sort()).toEqual([["dotfiles", true, null], ["linked", true, null]])
  expect(suggested.some(each => each.path === dotfiles)).toBe(true)
  // On Linux nothing's asked about first: the same folder is looked into.
  expect(suggestedFolders(home, sessions, box, "linux").find(each => each.path === notes)).toEqual({ path: notes, name: "notes", repository: true, seen: 50 })
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
