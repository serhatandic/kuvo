import "./safety"
import { afterAll, expect, test } from "bun:test"
import { existsSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { git, mac07, readJSON, rows, run, sha, snapshot } from "./rehome-run-world"

// An earlier Kuvo Code's sessions and settings moved into this Mac's box by the box itself
// (`kuvo-link.js rehome --run`, #713 piece 4), run as the installer runs it: a process of its
// own, in a home folder of the test's own. The earlier Kuvo Code's folder is never changed.
const mac = process.platform === "darwin"
const root = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-run-")))
afterAll(() => rmSync(root, { recursive: true, force: true }))

test.if(mac)("a Mac of 0.7.2 moves whole: its sessions brought up to date and into ~/.kuvo-code, its keys handed over, and its own folder left byte for byte as it was", () => {
  const m = mac07(root)
  const before = snapshot(m.S)
  const moved = run(m)
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(moved.said).toMatchObject({ result: "done", status: { state: "done", way: "move", failed: null, tries: 0 } })
  // The earlier Kuvo Code's folder: every file's bytes as they were, its database's log too.
  expect(snapshot(m.S)).toEqual(before)
  // A Mac's own box, with the release's files and the engine its installer fetched (never 0.7.2's).
  const read = (file: string) => readFileSync(join(m.K, file), "utf8")
  expect(read("box/kind")).toBe("mac\n")
  expect(read("bin/kuvo-start")).toContain("V=2.2.6")
  expect(readdirSync(join(m.K, "engine"))).toEqual(["2.2.6"])
  // Its sessions, in the pinned engine's database, each in its new folder.
  const db = rows(join(m.K, "data", "opencode", "opencode.db"))
  const chat = join(m.K, "chats", "c1"), worktree = join(m.K, "data", "opencode", "worktree", m.rootCommit.slice(0, 6), "feature one")
  expect(db.migration.length).toBe(59)
  expect(db.session_v2.map(each => `${each.title}: ${each.directory}`).sort()).toEqual([
    `chat (fork #1): ${chat}`, `chat: ${chat}`, `git edit: ${m.repository}`, `in worktree: ${worktree}`, `plain: ${join(m.home, "Projects", "plain")}`,
  ])
  expect(db.session_message.length).toBe(13)
  const chatID = sha("sha1", `directory:${chat}`)
  expect(db.project.map(each => [each.id, each.worktree]).sort()).toEqual([[m.rootCommit, m.repository], [chatID, chat], [sha("sha1", `directory:${join(m.home, "Projects", "plain")}`), join(m.home, "Projects", "plain")]].sort())
  // A file sent names where it is now, written as it was (plainly, or with `%20`).
  const sent = JSON.parse(db.session_message.find(each => each.data.includes("see attached"))!.data).files.map((file: any) => file.source.uri)
  expect(sent).toEqual([`file://${join(m.K, "attachments", "ses_a", "b.png")}`, `file://${join(m.K, "attachments", "ses_a")}/with%20space.png`])
  expect(read("attachments/ses_a/with space.png")).toBe("PNGDATA")
  expect(read("chats/c1/notes.md")).toBe("chat")
  // The worktree, linked from your repository to where it is now.
  const [linked] = readdirSync(join(m.repository, ".git", "worktrees"))
  expect(readFileSync(join(m.repository, ".git", "worktrees", linked, "gitdir"), "utf8").trim()).toBe(join(worktree, ".git"))
  expect(git(worktree, ["status", "--porcelain"])).toBe("")
  // This Mac's keys: copied for the helper the release brings, read back, then the app's own taken away.
  expect(readFileSync(join(m.home, "keys-asked"), "utf8").trim().split("\n")).toEqual(["copy Kuvo Helper.app", "delete-originals"])
  expect(readdirSync(join(m.keychain, "helper")).sort()).toEqual(["integrations.KUVO_KEY_NOTION", "phone-link.key"])
  expect(readdirSync(join(m.keychain, "app"))).toEqual([])
  // Nothing of the move's left over: no staging, no kept copy, nothing the installer staged.
  expect(existsSync(join(m.K, "updates", "rehome", "staging"))).toBe(false)
  expect(existsSync(join(m.K, "updates", "rehome", "kept"))).toBe(false)
  expect(existsSync(join(m.K, "updates", "install"))).toBe(false)
  expect(readJSON(join(m.K, "updates", "rehome", "state.json"))).toMatchObject({ state: "done", owed: ["passwords"], counts: { sessions: 5, sessionsBefore: 5, projects: 3, messages: 13 } })
})
