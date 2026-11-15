import "./safety"
import { afterAll, expect, test } from "bun:test"
import { existsSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { spawn } from "node:child_process"
import { databaseWithLog, git, mac07, put, readJSON, REAL_ENGINE, recorded, rehome, rows, run, sha, snapshot } from "./rehome-run-world"

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

// What a clean move leaves, to hold the others to: made where the world held to it will be (what's
// rewritten names its folders), then cleared away for it.
function cleanMove(at: string) {
  const m = mac07(root, () => {}, { at })
  const moved = run(m)
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  const result = { K: snapshot(m.K, moveOwn), keychain: snapshot(m.keychain) }
  rmSync(m.home, { recursive: true, force: true })
  return result
}
// (The move's own records, and what the engine writes of its own as it's started: its log.)
const moveOwn = (relative: string) => relative === "updates/rehome" || relative.startsWith("data/opencode/log")

const POINTS_BACK = ["after-plan", "after-install", "after-keys", "after-clone", "after-upgrade", "after-database", "after-rewrite", "after-check", "in-place", "after-place", "after-fix"]

test.if(mac)("stopped at any step before it's done, the move leaves both folders as they were once it's run again, and that run moves everything", () => {
  for (const point of POINTS_BACK) {
    const at = `stop-${point}`
    const clean = cleanMove(at)
    const m = mac07(root, () => {}, { at })
    const before = snapshot(m.S), keys = snapshot(join(m.keychain, "app"))
    const stopped = run(m, { KUVO_TEST_REHOME_STOP: point })
    expect(stopped.signal, `${point}: ${stopped.errors}`).toBe("SIGKILL")
    // Killed where it stood: the earlier Kuvo Code's folder and its keys as they were, and what
    // the installer says is a move to run again.
    expect(snapshot(m.S), point).toEqual(before)
    expect(snapshot(join(m.keychain, "app")), point).toEqual(keys)
    expect(rehome(m, ["--status"]).said, point).toMatchObject({ state: "moving", progress: { of: 11 } })
    const again = run(m)
    expect(again.status, `${point}: ${again.errors}${JSON.stringify(again.said)}`).toBe(0)
    expect(again.said, point).toMatchObject({ result: "done", status: { state: "done" } })
    expect(snapshot(m.S), point).toEqual(before)
    expect(snapshot(m.K, moveOwn), point).toEqual(clean.K)
    expect(snapshot(m.keychain), point).toEqual(clean.keychain)
    rmSync(m.home, { recursive: true, force: true })
  }
}, 240_000)

test.if(mac)("stopped once it can only go forward, running it again finishes it: started, the app's own keys taken away", () => {
  for (const point of ["after-clear", "after-start"]) {
    const at = `forward-${point}`
    const clean = cleanMove(at)
    const m = mac07(root, () => {}, { at })
    const before = snapshot(m.S)
    const stopped = run(m, { KUVO_TEST_REHOME_STOP: point })
    expect(stopped.signal, `${point}: ${stopped.errors}`).toBe("SIGKILL")
    const again = run(m)
    expect(again.status, `${point}: ${again.errors}${JSON.stringify(again.said)}`).toBe(0)
    expect(again.said, point).toMatchObject({ result: "done", status: { state: "done", owed: ["passwords"] } })
    expect(snapshot(m.S), point).toEqual(before)
    expect(snapshot(m.K, moveOwn), point).toEqual(clean.K)
    expect(snapshot(m.keychain), point).toEqual(clean.keychain)
    rmSync(m.home, { recursive: true, force: true })
  }
}, 120_000)

test.if(mac)("an earlier Kuvo Code that still has its database open refuses the move, saying to quit it; once it's quit, the move goes ahead", async () => {
  const m = mac07(root)
  const before = snapshot(m.S)
  // (Something that holds the database open, as its engine does: never by opening it as a database.)
  const holder = spawn("/bin/sh", ["-c", 'exec 3<"$1"; echo ready; read line', "sh", join(m.S, "opencode", "data", "opencode", "opencode.db")], { stdio: ["pipe", "pipe", "ignore"] })
  try {
    await new Promise<void>(resolve => holder.stdout.once("data", () => resolve()))
    const refused = run(m)
    expect(refused.status).toBe(1)
    expect(refused.said).toMatchObject({ result: "refused", status: { state: "failed", tries: 1, nextTry: null } })
    expect(refused.said!.message).toMatch(/^An earlier Kuvo Code is still running on this Mac \(.+\)\. Quit Kuvo Code wherever it's open, then try again\.$/)
    expect(existsSync(join(m.K, "box"))).toBe(false)
    expect(snapshot(m.S)).toEqual(before)
  } finally {
    holder.stdin.end()
    await new Promise(resolve => holder.once("exit", resolve))
  }
  // Tried again at once: no wait between tries.
  const moved = run(m)
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(snapshot(m.S)).toEqual(before)
})

test.if(mac)("a locked Keychain stops the move before anything's copied, saying to unlock it; unlocked, trying again moves everything", () => {
  const m = mac07(root, m => put(join(m.home, "keychain-locked"), ""))
  const before = snapshot(m.S)
  const stopped = run(m)
  expect(stopped.status).toBe(1)
  expect(stopped.said).toMatchObject({ result: "failed", status: { state: "failed", tries: 1, nextTry: null, failed: { step: "keys" } } })
  expect(stopped.said!.message).toBe("Your Keychain is locked, so Kuvo can't take this Mac's keys along. Unlock it (log out and back in, or open Keychain Access), then try again.")
  expect(existsSync(join(m.K, "box"))).toBe(false)
  expect(existsSync(join(m.K, "bin"))).toBe(false)
  expect(snapshot(m.S)).toEqual(before)
  rmSync(join(m.home, "keychain-locked"))
  expect(run(m).said).toMatchObject({ result: "done", status: { tries: 0 } })
  expect(readdirSync(join(m.keychain, "helper")).length).toBe(2)
})

test.if(mac)("keys the helper can't read back are taken away again and nothing moves: the app's own stay where they were", () => {
  const m = mac07(root, m => put(join(m.home, "helper-cannot-read"), ""))
  const stopped = run(m)
  expect(stopped.status).toBe(1)
  expect(stopped.said!.message).toBe("Kuvo couldn't read one of this Mac's keys back after copying it. Try again.")
  expect(existsSync(join(m.keychain, "helper"))).toBe(false)
  expect(readdirSync(join(m.keychain, "app")).length).toBe(2)
  expect(existsSync(join(m.K, "updates", "rehome", "keychain.json"))).toBe(false)
  expect(existsSync(join(m.K, "box"))).toBe(false)
})

test.if(mac)("copies of this Mac's keys an earlier try left are taken away first, and the move goes on", () => {
  const m = mac07(root, m => put(join(m.keychain, "helper", "phone-link.key"), "an earlier box's"))
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readFileSync(join(m.keychain, "helper", "phone-link.key"), "utf8")).toBe("the link's key")
  expect(readFileSync(join(m.home, "helper-asked"), "utf8")).toContain("keys clear")
})

test.if(mac)("with no engine fetched (offline), the move says to go online and changes nothing", () => {
  const m = mac07(root, m => rmSync(m.engine, { recursive: true }))
  const refused = run(m)
  expect(refused.status).toBe(1)
  expect(refused.said).toMatchObject({ result: "refused", message: "Kuvo's engine couldn't be downloaded. Check that this Mac is online, then try again." })
  expect(existsSync(join(m.K, "box"))).toBe(false)
})

test.if(mac)("sessions last used by an engine nobody tried are refused before anything's copied, the database never opened where it is", () => {
  const m = mac07(root, m => {
    const file = join(m.S, "opencode", "data", "opencode", "opencode.db")
    for (const suffix of ["", "-wal", "-shm"]) rmSync(file + suffix, { force: true })
    databaseWithLog(file, recorded("engine-2.0.32", m) + "\nINSERT INTO migration VALUES('20270101000000_later', 1);", join(m.home, "tmp", "make-db"))
  })
  const before = snapshot(m.S)
  const refused = run(m)
  expect(refused.said).toMatchObject({ result: "refused", message: "Your sessions were last used by a version of Kuvo Code that this one can't bring along yet (51 20270101000000_later). Update Kuvo Code, then try again." })
  expect(snapshot(m.S)).toEqual(before)
  expect(existsSync(join(m.K, "updates", "rehome", "plan-database"))).toBe(false)
})

test.if(mac && !REAL_ENGINE)("an engine that can't bring the sessions up to date stops the move, which goes back", () => {
  const m = mac07(root, m => put(join(m.home, "world.json"), JSON.stringify({ S: m.S, home: m.home, rootCommit: m.rootCommit, cannotMigrate: true })))
  const before = snapshot(m.S)
  const stopped = run(m)
  expect(stopped.status).toBe(1)
  expect(stopped.said).toMatchObject({ result: "failed", status: { state: "failed", failed: { step: "database" } } })
  expect(snapshot(m.S)).toEqual(before)
  expect(existsSync(join(m.K, "box"))).toBe(false)
  expect(existsSync(join(m.keychain, "helper"))).toBe(false)
})

test.if(mac)("something written in the earlier Kuvo Code's folder while it's copied sends the move back: nothing is left behind unsaid", () => {
  const m = mac07(root)
  const written = join(m.S, "Chats", "c1", "written-meanwhile.md")
  const stopped = run(m, { KUVO_TEST_REHOME_WRITE_AT: "after-check", KUVO_TEST_REHOME_WRITE_FILE: written, KUVO_TEST_REHOME_WRITE_TEXT: "new" })
  expect(stopped.status).toBe(1)
  expect(stopped.said!.message).toBe(`${join(m.S, "Chats", "c1")} changed while your sessions were being copied. Quit Kuvo Code wherever it's still open, then try again.`)
  expect(existsSync(join(m.K, "box"))).toBe(false)
  // The next try takes what was written too.
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readFileSync(join(m.K, "chats", "c1", "written-meanwhile.md"), "utf8")).toBe("new")
})

test.if(mac)("a Mac with nothing of an earlier Kuvo Code's but its browser's data still gets its box, with nothing to bring", () => {
  const m = mac07(root, m => {
    for (const name of readdirSync(m.S)) if (name !== "Browser") rmSync(join(m.S, name), { recursive: true })
    rmSync(m.keychain, { recursive: true })
  })
  const before = snapshot(m.S)
  const moved = run(m)
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(readFileSync(join(m.K, "box", "kind"), "utf8")).toBe("mac\n")
  expect(existsSync(join(m.K, "data"))).toBe(false)
  expect(snapshot(m.S)).toEqual(before)
})

test.if(mac)("run again once it's moved, nothing moves twice, and Move Back says there's nothing to put back", () => {
  const m = mac07(root)
  expect(run(m).said).toMatchObject({ result: "done" })
  const K = snapshot(m.K, moveOwn)
  const again = run(m)
  expect(again.status).toBe(0)
  expect(again.said).toMatchObject({ result: "done", status: { state: "done" } })
  expect(snapshot(m.K, moveOwn)).toEqual(K)
  expect(rehome(m, ["--back"]).said).toMatchObject({ result: "refused", message: "Kuvo on this Mac was moved from an earlier Kuvo Code, whose own folder is as it was: there's nothing to move back." })
})
