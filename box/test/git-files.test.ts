import "./safety"
import { afterAll, beforeAll, expect, test } from "bun:test"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, utimesSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { ask, follow } from "./route-answer"

// A Changes panel's list, as every client draws it (PROTOCOL.md, "Changes", `git.files`): what git
// says changed in a session's folder, grouped (conflicts, staged, unstaged, new), a file partly
// staged in both; one file's diff alone (`git.changes.path`), a new file's too; and where a
// worktree's branch goes back (`git.branches.worktree`). Real repositories, in folders of their own.

const { engineHandler } = await import("../src/link/server/router")
const handler = engineHandler(() => { throw new Error("no engine") }, { name: "test" }, () => { throw new Error("none") })

let box: string
beforeAll(() => {
  box = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-git-files-")))
})
afterAll(() => rmSync(box, { recursive: true, force: true }))

const git = (folder: string, ...args: string[]) => {
  const ran = spawnSync("git", ["-c", "init.defaultBranch=main", "-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgSign=false", ...args], {
    cwd: folder, env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" }, encoding: "utf8",
  })
  if (ran.status !== 0) throw new Error(`git ${args.join(" ")}: ${ran.stderr}`)
  return ran.stdout
}

/** A repository with one commit: `app/Form.tsx` (three lines), `app/old.ts`, `README.md`. */
function repository(name: string) {
  const top = join(box, name)
  mkdirSync(join(top, "app"), { recursive: true })
  git(top, "init", "-q")
  writeFileSync(join(top, "app", "Form.tsx"), "one\ntwo\nthree\n")
  writeFileSync(join(top, "app", "old.ts"), "export const old = 1\n")
  writeFileSync(join(top, "README.md"), "# App\n")
  git(top, "add", "-A")
  git(top, "commit", "-q", "-m", "First")
  return top
}

const files = async (directory: string) => (await ask(handler, "GET", `kuvo/git/files?directory=${encodeURIComponent(directory)}`)).json

test("a folder's changes come grouped, a file partly staged in both groups, each with its scope and its lines", async () => {
  const top = repository("grouped")
  // Staged, then changed again: in both groups.
  writeFileSync(join(top, "app", "Form.tsx"), "one\n2\nthree\n")
  git(top, "add", "app/Form.tsx")
  writeFileSync(join(top, "app", "Form.tsx"), "one\n2\nthree\nfour\n")
  // Moved, staged.
  git(top, "mv", "app/old.ts", "app/new.ts")
  // Deleted, not staged.
  rmSync(join(top, "README.md"))
  // New, in a folder of its own.
  mkdirSync(join(top, "docs"))
  writeFileSync(join(top, "docs", "Plan 2.md"), "plan\n")

  const listed = await files(top)
  // A stamp for each that can be discarded, which a discard sends back.
  for (const file of listed.files) expect(typeof file.stamp).toBe("string")
  for (const file of listed.files) delete file.stamp
  expect(listed).toEqual({
    repository: true, root: top, branch: "main", uncommitted: true, commits: "staged", stagedElsewhere: 0, takes: 2,
    files: [
      { path: "app/Form.tsx", group: "staged", status: "modified", scope: "staged", stage: "unstage", additions: 1, deletions: 1, discard: "head", discards: { kind: "changes", undoable: false } },
      { path: "app/new.ts", oldPath: "app/old.ts", group: "staged", status: "renamed", scope: "staged", stage: "unstage", additions: 0, deletions: 0, discard: "head", discards: { kind: "undoMove", undoable: true } },
      { path: "app/Form.tsx", group: "unstaged", status: "modified", scope: "unstaged", stage: "stage", additions: 1, deletions: 0, discard: "unstaged", discards: { kind: "changes", undoable: false } },
      { path: "README.md", group: "unstaged", status: "deleted", scope: "unstaged", stage: "stage", additions: 0, deletions: 1, discard: "unstaged", discards: { kind: "restore", undoable: true } },
      { path: "docs/Plan 2.md", group: "untracked", status: "added", scope: "unstaged", stage: "stage", discard: "trash", discards: { kind: "trash", undoable: true } },
    ],
  })

  // From a folder in it: only that folder's, by their paths in the repository.
  const inApp = await files(join(top, "app"))
  expect(inApp.files.map((file: any) => `${file.group} ${file.path}`)).toEqual(["staged app/Form.tsx", "staged app/new.ts", "unstaged app/Form.tsx"])

  // Nothing changed: an empty list, nothing uncommitted; outside a repository, none.
  const clean = repository("clean")
  expect(await files(clean)).toEqual({ repository: true, root: clean, branch: "main", files: [], uncommitted: false, commits: "all", stagedElsewhere: 0, takes: 0 })
  mkdirSync(join(box, "plain"))
  expect(await files(join(box, "plain"))).toEqual({ repository: false })
  expect((await ask(handler, "GET", `kuvo/git/files?directory=${encodeURIComponent(join(box, "gone"))}`)).status).toBe(404)
})

test("a file both sides changed in a merge is a conflict, once, and nothing else lists it", async () => {
  const top = repository("conflict")
  git(top, "switch", "-q", "-c", "other")
  writeFileSync(join(top, "app", "Form.tsx"), "one\nTWO\nthree\n")
  git(top, "commit", "-q", "-am", "Other")
  git(top, "switch", "-q", "main")
  writeFileSync(join(top, "app", "Form.tsx"), "one\nzwei\nthree\n")
  git(top, "commit", "-q", "-am", "Main")
  spawnSync("git", ["-c", "user.name=Test", "-c", "user.email=test@example.com", "merge", "other"], { cwd: top, env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" } })

  const listed = await files(top)
  // Its button marks it resolved; a conflict isn't discarded, so nothing is said of one.
  expect(listed.files).toEqual([{ path: "app/Form.tsx", group: "conflicted", status: "modified", scope: "uncommitted", stage: "resolve" }])
  expect(listed.underway).toBe("merging")
})

test("names with spaces are read whole, a move's both sides; a new file staged goes to the Trash; a repository inside says so", async () => {
  const top = repository("spaces")
  writeFileSync(join(top, "app", "My Form.tsx"), "a\n")
  git(top, "add", "-A")
  git(top, "commit", "-q", "-m", "Spaces")
  git(top, "mv", "app/My Form.tsx", "app/Your  Form.tsx")
  writeFileSync(join(top, "a b.ts"), "x\n")
  git(top, "add", "a b.ts")
  mkdirSync(join(top, "web"))
  git(join(top, "web"), "init", "-q")
  writeFileSync(join(top, "web", "index.html"), "<p>hi</p>\n")
  git(join(top, "web"), "add", "-A")
  git(join(top, "web"), "commit", "-q", "-m", "Web")

  const listed = await files(top)
  for (const file of listed.files) delete file.stamp
  expect(listed.files).toEqual([
    { path: "a b.ts", group: "staged", status: "added", scope: "staged", stage: "unstage", additions: 1, deletions: 0, discard: "trash", discards: { kind: "trash", undoable: true } },
    { path: "app/Your  Form.tsx", oldPath: "app/My Form.tsx", group: "staged", status: "renamed", scope: "staged", stage: "unstage", additions: 0, deletions: 0, discard: "head", discards: { kind: "undoMove", undoable: true } },
    { path: "web/", group: "untracked", status: "added", scope: "unstaged", stage: "stage", discard: "trash", discards: { kind: "trash", undoable: true }, repository: true },
  ])
})

test("one file's diff comes alone, a move with where it was, and a new file as all its lines", async () => {
  const top = repository("one-file")
  writeFileSync(join(top, "app", "Form.tsx"), "one\n2\nthree\n")
  writeFileSync(join(top, "README.md"), "# App\nMore.\n")
  git(top, "mv", "app/old.ts", "app/new.ts")
  writeFileSync(join(top, "notes.txt"), "first\nsecond\n")

  const diff = async (body: Record<string, unknown>) => (await ask(handler, "POST", "kuvo/git/changes", { directory: top, ...body })).json
  const form = await diff({ scope: "unstaged", path: "app/Form.tsx" })
  expect(form.patch).toContain("diff --git a/app/Form.tsx b/app/Form.tsx")
  expect(form.patch).toContain("-two\n+2\n")
  expect(form.patch).not.toContain("README.md")
  expect(form.untracked).toEqual([])

  const moved = await diff({ scope: "staged", path: "app/new.ts", oldPath: "app/old.ts" })
  expect(moved.patch).toContain("rename from app/old.ts")
  expect(moved.patch).toContain("rename to app/new.ts")

  const added = await diff({ scope: "unstaged", path: "notes.txt" })
  expect(added.patch).toContain("new file mode")
  expect(added.patch).toContain("+first\n+second\n")

  // Binary, and too big to show.
  writeFileSync(join(top, "logo.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 13, 0, 1]))
  const logo = await diff({ scope: "unstaged", path: "logo.png" })
  expect(logo.patch).toContain("Binary files")
  // The box says it's binary: no client reads git's words for it. Never of a text file, even one
  // whose own lines say "Binary files … differ".
  expect(logo.binary).toBe(true)
  git(top, "add", "logo.png")
  git(top, "commit", "-q", "-m", "Logo")
  writeFileSync(join(top, "logo.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 13, 0, 2]))
  expect((await diff({ scope: "unstaged", path: "logo.png" })).binary).toBe(true)
  expect(form.binary).toBeUndefined()
  expect(added.binary).toBeUndefined()
  writeFileSync(join(top, "said.txt"), "Binary files a/x and b/x differ\n")
  expect((await diff({ scope: "unstaged", path: "said.txt" })).binary).toBeUndefined()
  // Nor one whose line says so after a bare return (a captured log) or a line separator.
  writeFileSync(join(top, "log.txt"), "Comparing...\rBinary files a/x and b/x differ\nline\u2028Binary files a and b differ\u2028\n")
  const log = await diff({ scope: "unstaged", path: "log.txt" })
  expect(log.patch).toContain("Binary files a/x and b/x differ")
  expect(log.binary).toBeUndefined()
  writeFileSync(join(top, "huge.txt"), "x".repeat(5 * 1024 * 1024))
  expect(await diff({ scope: "unstaged", path: "huge.txt" })).toMatchObject({ patch: "", big: true })

  // Not one in the repository: refused, saying so.
  expect((await ask(handler, "POST", "kuvo/git/changes", { directory: top, path: "../elsewhere" })).status).toBe(400)
})

test("a worktree's branches say where Merge into… takes it; the project's own folder says nothing of one", async () => {
  const top = repository("project")
  const tree = join(box, "project-tree")
  git(top, "worktree", "add", "-q", "-b", "kuvo/fix-form", tree)

  const branches = async (directory: string) => (await ask(handler, "POST", "kuvo/git/branches", { directory })).json
  const inTree = await branches(tree)
  expect(inTree.current).toBe("kuvo/fix-form")
  expect(inTree.worktree).toEqual({ into: "main" })
  const inProject = await branches(top)
  expect(inProject.current).toBe("main")
  expect(inProject.worktree).toBeUndefined()
})

test("a pull request is offered only where one can be made: from a branch of its own, with somewhere to push", async () => {
  const top = repository("offered")
  const branches = async (directory: string) => (await ask(handler, "POST", "kuvo/git/branches", { directory })).json
  git(top, "switch", "-q", "-c", "fix-form")
  // Nowhere to push yet: none offered, and one asked for anyway is refused saying so.
  expect((await branches(top)).pullRequest).toBe(false)
  const nowhere = await ask(handler, "POST", "kuvo/git/pull-request/draft", { directory: top })
  expect([nowhere.status, nowhere.json.message]).toEqual([409, "This repository has no origin to push to. Add a remote called origin first."])
  git(top, "remote", "add", "origin", "git@github.com:me/app.git")
  expect((await branches(top)).pullRequest).toBe(true)
  // Work not committed yet doesn't take the offer away: its confirmation says to commit first.
  writeFileSync(join(top, "README.md"), "# App\nMore.\n")
  expect((await branches(top)).pullRequest).toBe(true)
  git(top, "commit", "-q", "-am", "More")
  // The branch pull requests go into, and no branch at all: none from there.
  git(top, "switch", "-q", "main")
  expect((await branches(top)).pullRequest).toBe(false)
  git(top, "switch", "-q", "--detach", "fix-form")
  expect((await branches(top)).pullRequest).toBe(false)
  // A worktree's goes into the branch it was made from, as its draft has it: never from that one.
  const tree = join(box, "offered-tree")
  git(top, "worktree", "add", "-q", "-b", "kuvo/side", tree, "fix-form")
  expect((await branches(tree)).pullRequest).toBe(true)
  // On the branch they go into, Create Pull Request's confirmation refuses too.
  git(top, "switch", "-q", "main")
  expect((await ask(handler, "POST", "kuvo/git/pull-request/draft", { directory: top })).status).toBe(409)
})

test("followed, the list comes again as soon as something is staged", async () => {
  const top = repository("followed")
  writeFileSync(join(top, "README.md"), "# App\nMore.\n")
  const stream = follow(handler, `kuvo/git/files/events?directory=${encodeURIComponent(top)}`)
  try {
    expect((await stream.next()).files.map((file: any) => file.group)).toEqual(["unstaged"])
    git(top, "add", "README.md")
    expect((await stream.next()).files.map((file: any) => file.group)).toEqual(["staged"])
  } finally {
    stream.stop()
  }
})

test("a Files panel's folder leaves out version control's own folder, and says why one can't be read", async () => {
  const top = repository("listed")
  writeFileSync(join(top, ".env"), "X=1\n")
  const { json } = await ask(handler, "GET", `kuvo/folders?path=${encodeURIComponent(top)}&files=1&hidden=1`)
  expect(json.folders).toEqual([{ name: "app", path: join(top, "app") }])
  expect(json.files).toEqual([".env", "README.md"])
  // Mercurial's and Subversion's too, as a folder or a file.
  mkdirSync(join(top, ".hg"))
  writeFileSync(join(top, ".svn"), "")
  const others = await ask(handler, "GET", `kuvo/folders?path=${encodeURIComponent(top)}&files=1&hidden=1`)
  expect([others.json.folders, others.json.files]).toEqual([[{ name: "app", path: join(top, "app") }], [".env", "README.md"]])
  rmSync(join(top, ".hg"), { recursive: true })
  rmSync(join(top, ".svn"))
  // Asked for folders alone (a folder to choose), `.git` is still there.
  const chosen = await ask(handler, "GET", `kuvo/folders?path=${encodeURIComponent(top)}&hidden=1`)
  expect(chosen.json.folders.map((folder: any) => folder.name)).toEqual([".git", "app"])
  const gone = await ask(handler, "GET", `kuvo/folders?path=${encodeURIComponent(join(top, "nowhere"))}&files=1`)
  expect(gone.status).toBe(404)
  expect(gone.json.reason).toBe("No such folder")
})

const discard = async (file: any) =>
  ask(handler, "POST", "kuvo/git/discard", { root: file.root, path: file.path, status: file.status, scope: file.scope, stamp: file.stamp })

test("what a commit takes is said for the whole repository, a staged file outside the folder counted", async () => {
  const top = repository("inside")
  mkdirSync(join(top, "other"))
  writeFileSync(join(top, "other", "x.ts"), "x\n")
  git(top, "add", "other/x.ts")
  writeFileSync(join(top, "app", "Form.tsx"), "changed\n")
  // Only the folder's own file changed, but another folder's is staged: a commit takes that one.
  const app = await files(join(top, "app"))
  expect(app.files.map((file: any) => file.group)).toEqual(["unstaged"])
  expect(app.commits).toBe("staged")
  expect(app.stagedElsewhere).toBe(1)
  expect(app.takes).toBe(1)
  // Staged in both: it takes both, one of them outside.
  git(top, "add", "app/Form.tsx")
  const both = await files(join(top, "app"))
  expect([both.commits, both.stagedElsewhere, both.files.length, both.takes]).toEqual(["staged", 1, 1, 2])
  // Nothing staged: everything in the folder, a file once, as the commit itself then takes it.
  git(top, "restore", "--staged", ".")
  writeFileSync(join(top, "app", "new.ts"), "n\n")
  const all = await files(join(top, "app"))
  expect([all.commits, all.stagedElsewhere, all.takes]).toEqual(["all", 0, 2])
  // (Its author sent along, as a client's settings would: no computer's own git identity is counted on.)
  const author = { GIT_AUTHOR_NAME: "Test", GIT_AUTHOR_EMAIL: "test@example.com", GIT_COMMITTER_NAME: "Test", GIT_COMMITTER_EMAIL: "test@example.com" }
  expect((await ask(handler, "POST", "kuvo/git/commit", { directory: join(top, "app"), message: "Both", author })).status).toBe(200)
  expect(git(top, "show", "--name-only", "--format=", "HEAD").trim().split("\n").sort()).toEqual(["app/Form.tsx", "app/new.ts"])
})

test("a new file staged and then gone from disk can't be had back from the Trash, and its discard says so", async () => {
  const top = repository("staged-gone")
  writeFileSync(join(top, "draft.md"), "draft\n")
  writeFileSync(join(top, "kept.md"), "kept\n")
  git(top, "add", "draft.md", "kept.md")
  rmSync(join(top, "draft.md"))
  const staged = (await files(top)).files.filter((file: any) => file.group === "staged")
  expect(staged.map((file: any) => [file.path, file.discards])).toEqual([
    // (Said in a sentence of the box's, for the confirm: its usual words say only "to the Trash".)
    ["draft.md", { kind: "trash", undoable: false, loses: "What you staged of it isn’t kept anywhere, so this can’t be undone." }],
    ["kept.md", { kind: "trash", undoable: true }],
  ])
})

test("a discard that would lose a staged version for good never says it can be undone", async () => {
  const top = repository("staged-then-changed")
  const trash = join(box, "trash-changed")
  process.env.KUVO_CHANGES_TRASH = trash
  try {
    // A new file staged, then written again: its discard trashes what's on disk, and the staged one goes.
    writeFileSync(join(top, "new.md"), "v1\n")
    git(top, "add", "new.md")
    writeFileSync(join(top, "new.md"), "v2\n")
    // A move staged with an edit, then edited again; and one staged, then gone from disk.
    git(top, "mv", "app/old.ts", "app/new.ts")
    writeFileSync(join(top, "app", "new.ts"), "export const old = 1\n// staged\n")
    git(top, "add", "app/new.ts")
    writeFileSync(join(top, "app", "new.ts"), "export const old = 1\n// staged\n// later\n")
    git(top, "mv", "README.md", "READ.md")
    rmSync(join(top, "READ.md"))
    // One moved and left as staged can be had back whole.
    git(top, "mv", "app/Form.tsx", "app/Sheet.tsx")
    const staged = (await files(top)).files.filter((file: any) => file.group === "staged")
    expect(staged.map((file: any) => [file.path, file.discards])).toEqual([
      ["app/new.ts", { kind: "undoMove", undoable: false, loses: "What you staged of it isn’t kept anywhere, so this can’t be undone." }],
      ["app/Sheet.tsx", { kind: "undoMove", undoable: true }],
      ["new.md", { kind: "trash", undoable: false, loses: "What you staged of it isn’t kept anywhere, so this can’t be undone." }],
      ["READ.md", { kind: "undoMove", undoable: false, loses: "What you staged of it isn’t kept anywhere, so this can’t be undone." }],
    ])
    // As the discard itself goes: v2 is in the Trash, v1 nowhere.
    const row = { ...staged.find((file: any) => file.path === "new.md"), root: top }
    expect((await discard(row)).status).toBe(200)
    expect(spawnSync("grep", ["-rl", "v2", trash], { encoding: "utf8" }).stdout.trim()).not.toBe("")
    expect(spawnSync("grep", ["-rlx", "v1", trash], { encoding: "utf8" }).stdout.trim()).toBe("")
  } finally {
    delete process.env.KUVO_CHANGES_TRASH
  }
})

test("a commit takes the same number of files from every folder, a move across a folder's edge as one", async () => {
  const top = repository("moves")
  mkdirSync(join(top, "out"))
  writeFileSync(join(top, "out", "in.ts"), "into the folder\n")
  writeFileSync(join(top, "app", "away.ts"), "out of the folder\n")
  git(top, "add", "-A")
  git(top, "commit", "-q", "-m", "More")
  git(top, "mv", "app/old.ts", "app/renamed.ts")
  git(top, "mv", "out/in.ts", "app/in.ts")
  git(top, "mv", "app/away.ts", "out/away.ts")
  const [atTop, inApp] = [await files(top), await files(join(top, "app"))]
  expect([atTop.commits, atTop.takes, inApp.takes]).toEqual(["staged", 3, 3])
  const author = { GIT_AUTHOR_NAME: "Test", GIT_AUTHOR_EMAIL: "test@example.com", GIT_COMMITTER_NAME: "Test", GIT_COMMITTER_EMAIL: "test@example.com" }
  expect((await ask(handler, "POST", "kuvo/git/commit", { directory: join(top, "app"), message: "Moves", author })).status).toBe(200)
  expect(git(top, "show", "--name-status", "--find-renames", "--format=", "HEAD").trim().split("\n").length).toBe(3)
})

test("a file opens as its text where it's text, and says it isn't where it isn't", async () => {
  const folder = join(box, "viewer")
  mkdirSync(folder)
  const read = async (name: string, more = "") => ask(handler, "GET", `kuvo/file/text?uri=${encodeURIComponent(`file://${folder}/${name}`)}${more}`)
  writeFileSync(join(folder, "notes.md"), "# Café\nline two\n")
  expect((await read("notes.md")).json).toEqual({ text: true, encoding: "utf-8", content: "# Café\nline two\n" })
  writeFileSync(join(folder, "empty.txt"), "")
  expect((await read("empty.txt")).json).toEqual({ text: true, encoding: "utf-8", content: "" })
  // A mark at its start saying it's UTF-8 isn't part of its text.
  writeFileSync(join(folder, "marked.txt"), "\ufeffhi\n")
  expect((await read("marked.txt")).json).toEqual({ text: true, encoding: "utf-8", content: "hi\n" })
  // All of a file as big as the most asked for, and none of one a byte bigger.
  expect((await read("marked.txt", "&max=6")).json.content).toBe("hi\n")
  expect((await read("marked.txt", "&max=5")).status).toBe(413)
  // A size that isn't a whole number is taken as the whole number under it, never as a file that can't be opened.
  expect((await read("empty.txt", "&max=1.5")).json).toEqual({ text: true, encoding: "utf-8", content: "" })
  expect((await read("marked.txt", "&max=6.9")).json.content).toBe("hi\n")
  // Past the most text one answer carries, whatever was asked for: too big, not read.
  writeFileSync(join(folder, "long.txt"), "x".repeat(4 * 1024 * 1024 + 1))
  expect((await read("long.txt")).status).toBe(413)
  expect((await read("long.txt", `&max=${50 * 1024 * 1024}`)).status).toBe(413)
  // A picture (a NUL in it), and bytes that aren't UTF-8 (Latin-1's é, a UTF-16 file): not text.
  writeFileSync(join(folder, "logo.png"), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 13]))
  expect((await read("logo.png")).json).toEqual({ text: false })
  writeFileSync(join(folder, "latin.txt"), Buffer.from([0x63, 0x61, 0x66, 0xe9]))
  expect((await read("latin.txt")).json).toEqual({ text: false })
  writeFileSync(join(folder, "wide.txt"), Buffer.from("\ufeffhi", "utf16le"))
  expect((await read("wide.txt")).json).toEqual({ text: false })
  // Refused as the file's bytes are: too big to show, not there, not a file's address.
  expect((await read("notes.md", "&max=4")).status).toBe(413)
  expect((await read("gone.md")).status).toBe(404)
  expect((await ask(handler, "GET", "kuvo/file/text?uri=https%3A%2F%2Fexample.com%2Fa.md")).status).toBe(400)
})

test("a discard is refused once its file changed since it was listed, and a new file committed since isn't trashed", async () => {
  const top = repository("stale")
  writeFileSync(join(top, "notes.txt"), "mine\n")
  writeFileSync(join(top, "README.md"), "# App\nOne edit.\n")
  const listed = await files(top)
  const note = { ...listed.files.find((file: any) => file.path === "notes.txt"), root: top }
  const readme = { ...listed.files.find((file: any) => file.path === "README.md"), root: top }

  // An agent commits the new file, then edits the README again.
  git(top, "add", "notes.txt")
  git(top, "commit", "-q", "-m", "Notes")
  writeFileSync(join(top, "README.md"), "# App\nOne edit.\nThe agent's later edit.\n")
  const trashed = await discard(note)
  expect(trashed.status).toBe(409)
  expect(trashed.json.message).toContain("is in the last commit now")
  expect(readFileSync(join(top, "notes.txt"), "utf8")).toBe("mine\n")
  const edited = await discard(readme)
  expect(edited.status).toBe(409)
  expect(readFileSync(join(top, "README.md"), "utf8")).toContain("The agent's later edit.")

  // Listed again, as it is now: it goes.
  const again = { ...(await files(top)).files.find((file: any) => file.path === "README.md"), root: top }
  expect((await discard(again)).status).toBe(200)
  expect(readFileSync(join(top, "README.md"), "utf8")).toBe("# App\n")
})

test("a file where a committed file's folder was: the deleted file is listed, its diff shows, and Discard brings it back", async () => {
  const top = repository("file-for-folder")
  rmSync(join(top, "app"), { recursive: true })
  writeFileSync(join(top, "app"), "a file in the folder's place\n")
  const listed = await files(top)
  const form = listed.files.find((file: any) => file.path === "app/Form.tsx")
  expect(form).toMatchObject({ status: "deleted" })
  const diff = await ask(handler, "POST", "kuvo/git/changes", { directory: top, scope: "unstaged", path: "app/Form.tsx" })
  expect(diff.status).toBe(200)
  expect(diff.json.patch).toContain("-one\n-two\n-three\n")
  expect((await discard({ ...form, root: top })).status).toBe(200)
  expect(readFileSync(join(top, "app", "Form.tsx"), "utf8")).toBe("one\ntwo\nthree\n")
})

test("discarding a staged deletion of a file still there keeps what was on disk in the Trash", async () => {
  const top = repository("deleted")
  const trash = join(box, "trash")
  process.env.KUVO_CHANGES_TRASH = trash
  try {
    git(top, "rm", "-q", "--cached", "README.md")
    writeFileSync(join(top, "README.md"), "# App\nEdits of mine.\n")
    const row = { ...(await files(top)).files.find((file: any) => file.group === "staged"), root: top }
    expect([row.status, row.discard, row.discards]).toEqual(["deleted", "head", { kind: "restore", undoable: true }])
    expect((await discard(row)).status).toBe(200)
    expect(readFileSync(join(top, "README.md"), "utf8")).toBe("# App\n")
    const kept = spawnSync("grep", ["-rl", "Edits of mine.", trash], { encoding: "utf8" }).stdout.trim()
    expect(kept).not.toBe("")
  } finally {
    delete process.env.KUVO_CHANGES_TRASH
  }
})

test("new files past the most a list names are counted, and a big file's diff isn't read", async () => {
  const top = repository("many")
  mkdirSync(join(top, "out"))
  for (let at = 0; at < 1_005; at++) writeFileSync(join(top, "out", `f${at}.txt`), "x\n")
  const listed = await files(top)
  expect(listed.files.length).toBe(1_000)
  expect(listed.more).toBe(5)
  // A commit takes those not listed too.
  expect([listed.commits, listed.takes]).toEqual(["all", 1_005])
  rmSync(join(top, "out"), { recursive: true, force: true })
  writeFileSync(join(top, "big.txt"), "x\n".repeat(3 * 1024 * 1024))
  git(top, "add", "big.txt")
  git(top, "commit", "-q", "-m", "Big")
  writeFileSync(join(top, "big.txt"), "y\n".repeat(3 * 1024 * 1024))
  const diff = (await ask(handler, "POST", "kuvo/git/changes", { directory: top, scope: "unstaged", path: "big.txt" })).json
  expect(diff).toMatchObject({ patch: "", big: true })
})

test("a linked worktree's .git file isn't listed in its Files", async () => {
  const top = repository("linked")
  const tree = join(box, "linked-tree")
  git(top, "worktree", "add", "-q", "-b", "side", tree)
  const { json } = await ask(handler, "GET", `kuvo/folders?path=${encodeURIComponent(tree)}&files=1&hidden=1`)
  expect(json.files).toEqual(["README.md"])
})

test("followers share a read only when it started after the change they heard of", async () => {
  const { sharedFiles } = await import("../src/link/areas/changes")
  const top = repository("shared")
  const before = Date.now()
  const first = await sharedFiles(top, before)
  expect(first!.repository && first!.files).toEqual([])
  // A second follower, told of nothing new, gets the same read.
  expect(await sharedFiles(top, before)).toBe(first)
  // A change, then a follower told of it: a new read, never the one from before it.
  writeFileSync(join(top, "README.md"), "# App\nChanged.\n")
  const after = await sharedFiles(top, Date.now())
  expect(after!.repository && after!.files.map((file: any) => file.path)).toEqual(["README.md"])
})

test("discarding a staged move never overwrites a file written at its old path since", async () => {
  const top = repository("moved")
  const trash = join(box, "trash-moved")
  process.env.KUVO_CHANGES_TRASH = trash
  try {
    git(top, "mv", "app/old.ts", "app/new.ts")
    const listed = { ...(await files(top)).files.find((file: any) => file.status === "renamed"), root: top }
    expect([listed.path, listed.oldPath]).toEqual(["app/new.ts", "app/old.ts"])
    // An agent writes a new file where the old one was.
    writeFileSync(join(top, "app", "old.ts"), "agent work\n")
    // With what it was shown: refused, nothing touched.
    const refused = await ask(handler, "POST", "kuvo/git/discard", { root: top, path: listed.path, oldPath: listed.oldPath, status: "renamed", scope: "staged", stamp: listed.stamp })
    expect(refused.status).toBe(409)
    expect(readFileSync(join(top, "app", "old.ts"), "utf8")).toBe("agent work\n")
    // Listed again, the new file seen: the move goes back, and the new file goes to the Trash first.
    const seen = (await files(top)).files.find((file: any) => file.status === "renamed")
    expect((await ask(handler, "POST", "kuvo/git/discard", { root: top, path: listed.path, oldPath: listed.oldPath, status: "renamed", scope: "staged", stamp: seen.stamp })).status).toBe(200)
    expect(readFileSync(join(top, "app", "old.ts"), "utf8")).toBe("export const old = 1\n")
    expect(spawnSync("grep", ["-rl", "agent work", trash], { encoding: "utf8" }).stdout.trim()).not.toBe("")
  } finally {
    delete process.env.KUVO_CHANGES_TRASH
  }
})

test("a move into the folder from outside it counts the side outside", async () => {
  const top = repository("into")
  mkdirSync(join(top, "other"))
  writeFileSync(join(top, "other", "x.ts"), "x\n")
  git(top, "add", "-A")
  git(top, "commit", "-q", "-m", "Other")
  git(top, "mv", "other/x.ts", "app/x.ts")
  const app = await files(join(top, "app"))
  // (One file, whichever folder it's seen from.)
  expect([app.commits, app.stagedElsewhere, app.takes]).toEqual(["staged", 1, 1])
})

test("a staged deletion of a file just as it was committed leaves nothing in the Trash", async () => {
  const top = repository("unchanged")
  const trash = join(box, "trash-unchanged")
  process.env.KUVO_CHANGES_TRASH = trash
  try {
    git(top, "rm", "-q", "--cached", "README.md")
    const row = { ...(await files(top)).files.find((file: any) => file.group === "staged"), root: top }
    expect((await discard(row)).status).toBe(200)
    expect(readFileSync(join(top, "README.md"), "utf8")).toBe("# App\n")
    expect(spawnSync("ls", ["-A", trash], { encoding: "utf8" }).stdout.trim()).toBe("")
  } finally {
    delete process.env.KUVO_CHANGES_TRASH
  }
})

test("a file only noted with git add -N can be discarded with the stamp it was listed with", async () => {
  const top = repository("noted")
  process.env.KUVO_CHANGES_TRASH = join(box, "trash-noted")
  try {
    writeFileSync(join(top, "later.ts"), "later\n")
    git(top, "add", "-N", "later.ts")
    const row = { ...(await files(top)).files.find((file: any) => file.path === "later.ts"), root: top }
    expect((await discard(row)).status).toBe(200)
  } finally {
    delete process.env.KUVO_CHANGES_TRASH
  }
})

test("a copy left after git rm --cached is a new file to discard, not one committed since", async () => {
  const top = repository("uncached")
  process.env.KUVO_CHANGES_TRASH = join(box, "trash-uncached")
  try {
    git(top, "rm", "-q", "--cached", "README.md")
    const listed = (await files(top)).files
    const copy = { ...listed.find((file: any) => file.group === "untracked"), root: top }
    expect([copy.path, copy.discard]).toEqual(["README.md", "trash"])
    expect((await discard(copy)).status).toBe(200)
  } finally {
    delete process.env.KUVO_CHANGES_TRASH
  }
})

test("a discard that doesn't say what the file was when it was listed is refused, and nothing is touched", async () => {
  const top = repository("no-stamp")
  process.env.KUVO_CHANGES_TRASH = join(box, "trash-no-stamp")
  try {
    writeFileSync(join(top, "README.md"), "# App\nThe agent's edit.\n")
    writeFileSync(join(top, "notes.txt"), "mine\n")
    const listed = (await files(top)).files.map((file: any) => ({ ...file, root: top }))
    for (const row of listed) {
      for (const stamp of [undefined, "", 7]) {
        const refused = await ask(handler, "POST", "kuvo/git/discard", { root: top, path: row.path, status: row.status, scope: row.scope, ...(stamp === undefined ? {} : { stamp }) })
        expect(refused.status).toBe(409)
        expect(refused.json.message).toBe(`${row.path} wasn’t discarded: this app didn’t say what the file was when it listed it, so changes made since could be lost. Update the app, then try again.`)
      }
    }
    expect(readFileSync(join(top, "README.md"), "utf8")).toBe("# App\nThe agent's edit.\n")
    expect(readFileSync(join(top, "notes.txt"), "utf8")).toBe("mine\n")
    expect(existsSync(join(box, "trash-no-stamp"))).toBe(false)
    // With the stamp each was listed with, they go.
    for (const row of listed) expect((await discard(row)).status).toBe(200)
    expect(readFileSync(join(top, "README.md"), "utf8")).toBe("# App\n")
    expect(existsSync(join(top, "notes.txt"))).toBe(false)
  } finally {
    delete process.env.KUVO_CHANGES_TRASH
  }
})

// A repository of its own inside this one ("web/") goes to the Trash whole, so its stamp is what its
// own git says: an edit or a commit an agent makes inside it after it was listed is seen, and the
// discard refused, where the folder's own size and time said nothing had changed.
test("a repository inside this one isn't discarded once anything in it changed since it was listed", async () => {
  const top = repository("nested")
  const trash = join(box, "trash-nested")
  process.env.KUVO_CHANGES_TRASH = trash
  try {
    const web = join(top, "web")
    mkdirSync(web)
    git(web, "init", "-q", "-b", "main")
    writeFileSync(join(web, "index.html"), "<p>hi</p>\n")
    git(web, "add", "-A")
    git(web, "-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "-q", "-m", "Web")
    const row = async () => ({ ...(await files(top)).files.find((file: any) => file.path === "web/"), root: top })
    const listed = await row()
    expect([listed.repository, listed.discard]).toEqual([true, "trash"])
    // Listed again with nothing changed: the same stamp.
    expect((await row()).stamp).toBe(listed.stamp)

    // An agent edits a file in it (the folder itself is as it was).
    writeFileSync(join(web, "index.html"), "<p>the agent's newer page</p>\n")
    const edited = await discard(listed)
    expect(edited.status).toBe(409)
    expect(edited.json.message).toBe("web changed since it was listed. Look at it again before you discard it.")
    expect(readFileSync(join(web, "index.html"), "utf8")).toBe("<p>the agent's newer page</p>\n")

    // It commits there: nothing uncommitted again, but another commit.
    const seen = await row()
    expect(seen.stamp).not.toBe(listed.stamp)
    git(web, "-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "-q", "-am", "Newer")
    expect((await discard(seen)).status).toBe(409)
    expect((await discard(listed)).status).toBe(409)
    expect(existsSync(web)).toBe(true)
    expect(existsSync(trash)).toBe(false)

    // Listed again, as it is now: it goes to the Trash, whole.
    expect((await discard(await row())).status).toBe(200)
    expect(existsSync(web)).toBe(false)
    expect(spawnSync("grep", ["-rl", "the agent's newer page", trash], { encoding: "utf8" }).stdout.trim()).not.toBe("")
  } finally {
    delete process.env.KUVO_CHANGES_TRASH
  }
})

test("a file changed while the list was being read has a stamp no discard matches", async () => {
  const top = repository("racing")
  writeFileSync(join(top, "README.md"), "# App\nEdited.\n")
  // Its time after the list began to be read, as an edit landing meanwhile would have.
  const later = new Date(Date.now() + 60_000)
  utimesSync(join(top, "README.md"), later, later)
  const row = { ...(await files(top)).files.find((file: any) => file.path === "README.md"), root: top }
  expect((await discard(row)).status).toBe(409)
  expect(readFileSync(join(top, "README.md"), "utf8")).toBe("# App\nEdited.\n")
})

test("a follower hears of a change as it's seen, and of a reset that moves only the branch", async () => {
  const { changeClock, gitLook } = await import("../src/link/areas/changes")
  let seen = "a"
  const clock = changeClock(() => seen)
  seen = "b"
  const at = Date.now()
  clock.look()
  expect(clock.changed()).toBeGreaterThanOrEqual(at)

  const top = repository("reset")
  writeFileSync(join(top, "README.md"), "# App\nTwo.\n")
  git(top, "commit", "-q", "-am", "Two")
  const gitDir = join(top, ".git")
  const past = new Date(Date.now() - 60_000)
  for (const file of ["index", "HEAD", "logs/HEAD"]) utimesSync(join(gitDir, file), past, past)
  const before = gitLook(gitDir)
  git(top, "reset", "-q", "--soft", "HEAD~1")
  expect(gitLook(gitDir)).not.toBe(before)
})
