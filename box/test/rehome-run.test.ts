import "./safety"
import { afterAll, expect, test } from "bun:test"
import { Database } from "bun:sqlite"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, readlinkSync, realpathSync, rmSync, symlinkSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { spawn, spawnSync } from "node:child_process"
import { judgeInstall } from "../src/link/install/verdict"
import { databaseWithLog, git, mac07, preferences, put, readJSON, REAL_ENGINE, recorded, rehome, rows, run, sha, snapshot } from "./rehome-run-world"

// An earlier Kuvo Code's sessions and settings moved into this Mac's box by the box itself
// (`kuvo-link.js rehome --run`, #713 piece 4), run as the installer runs it: a process of its
// own, in a home folder of the test's own. The earlier Kuvo Code's folder is never changed.
const mac = process.platform === "darwin"
// (Room for the real engine, where a run names it: it takes seconds to start, a stand-in none.)
const LIMIT = REAL_ENGINE ? 120_000 : 30_000
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
  // The engine's plugin is the release's, never the one 0.7.2 wrote for its own engine.
  expect(read("config/opencode/plugins/kuvo-code.js")).toBe("the release's config/opencode/plugins/kuvo-code.js\n")
  // Its sessions, in the pinned engine's database, each in its new folder.
  const db = rows(join(m.K, "data", "opencode", "opencode.db"))
  const chat = join(m.K, "chats", "c1"), worktree = join(m.K, "data", "opencode", "worktree", m.rootCommit.slice(0, 6), "feature one")
  expect(db.migration.length).toBe(59)
  expect(db.session_v2.map(each => `${each.title}: ${each.directory}`).sort()).toEqual([
    `chat (fork #1): ${chat}`, `chat: ${chat}`, `git edit: ${m.repository}`, `in worktree: ${worktree}`, `plain: ${join(m.home, "Projects", "plain")}`,
  ])
  expect(db.session_message.length).toBe(13)
  // A provider's sign-in, an account and a kept rule come through the engine's update, the rule's folder where it is now.
  expect(db.credential.map(each => [each.id, each.value])).toEqual([["crd_1", "{\"type\":\"api\",\"key\":\"sk-test\"}"]])
  expect(db.account.map(each => each.email)).toEqual(["you@example.com"])
  expect(db.permission.map(each => [each.id, each.resource])).toEqual([["psv_kept1", `${chat}/*`]])
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
  // This Mac's keys: copied for the helper the release brings, read back, then the app's own taken
  // away. Its saved sign-ins are never read by the move (macOS may ask about each): they wait for you.
  expect(readFileSync(join(m.home, "keys-asked"), "utf8").trim().split("\n").map(line => line.trim())).toEqual(["copy Kuvo Helper.app", "delete-originals"])
  expect(readdirSync(join(m.keychain, "helper")).sort()).toEqual(["integrations.KUVO_KEY_NOTION", "phone-link.key"])
  expect(readdirSync(join(m.keychain, "app"))).toEqual([])
  // Nothing of the move's left over: no staging, no kept copy, nothing the installer staged.
  expect(existsSync(join(m.K, "updates", "rehome", "staging"))).toBe(false)
  expect(existsSync(join(m.K, "updates", "rehome", "kept"))).toBe(false)
  expect(existsSync(join(m.K, "updates", "install"))).toBe(false)
  // (Whether your Keychain has any can't be told in a copy's home: they're owed.)
  expect(readJSON(join(m.K, "updates", "rehome", "state.json"))).toMatchObject({ state: "done", owed: ["passwords"], counts: { sessions: 5, sessionsBefore: 5, projects: 3, messages: 13 } })
}, LIMIT)

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
// (The move's own records, and what the engine writes of its own as it's started: its log; and,
// with the real engine, its database's bytes, which hold when it ran. Its rows are held elsewhere.)
const moveOwn = (relative: string) => relative === "updates/rehome" || relative.startsWith("data/opencode/log") || (REAL_ENGINE !== null && relative.startsWith("data/opencode/opencode.db"))

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
}, 600_000)

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
}, 300_000)

test.if(mac)("an earlier Kuvo Code that still has its database open refuses the move, saying to quit it; once it's quit, the move goes ahead", async () => {
  const m = mac07(root)
  const before = snapshot(m.S)
  // (Something that holds the database open to write in it, as its engine does: never by opening it as a database.)
  const holder = spawn("/bin/sh", ["-c", 'exec 3<>"$1"; echo ready; read line', "sh", join(m.S, "opencode", "data", "opencode", "opencode.db")], { stdio: ["pipe", "pipe", "ignore"] })
  try {
    await new Promise<void>(resolve => holder.stdout.once("data", () => resolve()))
    const refused = run(m)
    expect(refused.status).toBe(1)
    expect(refused.said).toMatchObject({ result: "refused", status: { state: "failed", tries: 1, nextTry: null } })
    // (Named as you know it, never by a path.)
    expect(refused.said!.message).toMatch(/^“\w+” is still using your earlier Kuvo Code's files\. Quit it, then try again\.$/)
    expect(existsSync(join(m.K, "box"))).toBe(false)
    expect(snapshot(m.S)).toEqual(before)
  } finally {
    holder.stdin.end()
    await new Promise(resolve => holder.once("exit", resolve))
  }
  // Tried again at once: no wait between tries. Something only reading it (a backup, Spotlight) holds nothing back.
  const reader = spawn("/bin/sh", ["-c", 'exec 3<"$1"; echo ready; read line', "sh", join(m.S, "opencode", "data", "opencode", "opencode.db")], { stdio: ["pipe", "pipe", "ignore"] })
  try {
    await new Promise<void>(resolve => reader.stdout.once("data", () => resolve()))
    const moved = run(m)
    expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
    expect(snapshot(m.S)).toEqual(before)
  } finally {
    reader.stdin.end()
    await new Promise(resolve => reader.once("exit", resolve))
  }
}, LIMIT)

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
}, LIMIT)

test.if(mac)("a Keychain that keeps this Mac's keys from Kuvo without being locked never says it's locked: you're asked for your OK, or told to try again, and only the move's log names the key and what macOS said", () => {
  for (const [file, shown, logged] of [
    ["keychain-not-allowed", "Kuvo needs your OK to take this Mac's keys along. Try again: macOS asks once for each key whether kuvo-keys may use it. Each time, enter your Mac's password and click Always Allow.", "kuvo-keys copy: not-allowed (OSStatus -25308): Your Keychain didn’t give out phone-link/key (OSStatus -25308)."],
    ["keychain-refuses", "Kuvo couldn't take this Mac's keys along, so nothing was moved. Try again.", "kuvo-keys copy: refused (OSStatus -25291): Your Keychain didn’t give out run.kuvo.code.vault (OSStatus -25291)."],
  ]) {
    const m = mac07(root, m => put(join(m.home, file), ""))
    const before = snapshot(m.S)
    const stopped = run(m)
    expect(stopped.status, file).toBe(1)
    expect(stopped.said, file).toMatchObject({ result: "failed", status: { state: "failed", failed: { step: "keys" } } })
    expect(stopped.said!.message, file).toBe(shown)
    // (No number, no key's name on screen: those are for support, in the move's log.)
    expect(stopped.said!.message, file).not.toMatch(/-?\d{3,}|phone-link|run\.kuvo/)
    // (Its log: what the move writes to its errors, which the installer keeps in `run.log`.)
    expect(stopped.errors, file).toContain(logged)
    expect(existsSync(join(m.K, "box")), file).toBe(false)
    expect(snapshot(m.S), file).toEqual(before)
    // Allowed, trying again moves everything.
    rmSync(join(m.home, file))
    expect(run(m).said, file).toMatchObject({ result: "done" })
    rmSync(m.home, { recursive: true, force: true })
  }
}, LIMIT * 2)

test.if(mac)("keys the helper can't read back are taken away again and nothing moves: the app's own stay where they were", () => {
  const m = mac07(root, m => put(join(m.home, "helper-cannot-read"), ""))
  const stopped = run(m)
  expect(stopped.status).toBe(1)
  expect(stopped.said!.message).toBe("Kuvo couldn't read one of this Mac's keys back after copying it. Try again.")
  expect(existsSync(join(m.keychain, "helper"))).toBe(false)
  expect(readdirSync(join(m.keychain, "app")).length).toBe(2)
  expect(existsSync(join(m.K, "updates", "rehome", "keychain.json"))).toBe(false)
  expect(existsSync(join(m.K, "box"))).toBe(false)
}, LIMIT)

test.if(mac)("copies of this Mac's keys an earlier try left are taken away first, and the move goes on", () => {
  const m = mac07(root, m => put(join(m.keychain, "helper", "phone-link.key"), "an earlier box's"))
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readFileSync(join(m.keychain, "helper", "phone-link.key"), "utf8")).toBe("the link's key")
  expect(readFileSync(join(m.home, "helper-asked"), "utf8")).toContain("keys clear")
}, LIMIT)

test.if(mac)("with no engine fetched (offline), the move says to go online and changes nothing", () => {
  const m = mac07(root, m => rmSync(m.engine, { recursive: true }))
  const refused = run(m)
  expect(refused.status).toBe(1)
  expect(refused.said).toMatchObject({ result: "refused", message: "Kuvo's engine couldn't be downloaded. Check that this Mac is online, then try again." })
  expect(existsSync(join(m.K, "box"))).toBe(false)
}, LIMIT)

test.if(mac)("sessions last used by an engine nobody tried are refused before anything's copied, the database never opened where it is", () => {
  const m = mac07(root, m => {
    const file = join(m.S, "opencode", "data", "opencode", "opencode.db")
    for (const suffix of ["", "-wal", "-shm"]) rmSync(file + suffix, { force: true })
    databaseWithLog(file, recorded("engine-2.0.32", m) + "\nINSERT INTO migration VALUES('20270101000000_later', 1);", join(m.home, "tmp", "make-db"))
  })
  const before = snapshot(m.S)
  const refused = run(m)
  expect(refused.said).toMatchObject({ result: "refused", message: "Your sessions were last used by a version of Kuvo Code that this one doesn't know how to bring along (51 20270101000000_later). Nothing was changed. Update Kuvo if a newer version is out, then try again." })
  expect(snapshot(m.S)).toEqual(before)
  expect(existsSync(join(m.K, "updates", "rehome", "plan-database"))).toBe(false)
}, LIMIT)

test.if(mac && !REAL_ENGINE)("an engine that can't bring the sessions up to date stops the move, which goes back", () => {
  const m = mac07(root, m => put(join(m.home, "world.json"), JSON.stringify({ S: m.S, home: m.home, rootCommit: m.rootCommit, cannotMigrate: true })))
  const before = snapshot(m.S)
  const stopped = run(m)
  expect(stopped.status).toBe(1)
  // (Nothing of the person's to act on: said in one plain sentence, never the move's workings.)
  expect(stopped.said).toMatchObject({ result: "failed", message: "Kuvo couldn't move your sessions this time, and left everything as it was. Try again.", status: { state: "failed", failed: { step: "database" } } })
  expect(stopped.errors).toContain("The move stopped at database:")
  expect(snapshot(m.S)).toEqual(before)
  expect(existsSync(join(m.K, "box"))).toBe(false)
  expect(existsSync(join(m.keychain, "helper"))).toBe(false)
}, LIMIT)

test.if(mac)("something written once in the earlier Kuvo Code's folder while it's copied: the move goes back and runs again by itself, taking what was written", () => {
  const m = mac07(root)
  const written = join(m.S, "Chats", "c1", "written-meanwhile.md")
  const moved = run(m, { KUVO_TEST_REHOME_WRITE_AT: "after-check", KUVO_TEST_REHOME_WRITE_FILE: written, KUVO_TEST_REHOME_WRITE_TEXT: "new", KUVO_TEST_REHOME_WRITE_ONCE: "1" })
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(moved.said).toMatchObject({ result: "done" })
  expect(readFileSync(join(m.K, "chats", "c1", "written-meanwhile.md"), "utf8")).toBe("new")
  // (Said for support in the move's log alone.)
  expect(moved.errors).toContain(`Changed in the earlier Kuvo Code's folder while it was copied: ${join(m.S, "Chats", "c1")}`)
  expect(moved.errors).toContain("Running the move again (2 of 3)")
}, LIMIT * 2)

test.if(mac)("something that goes on writing in the earlier Kuvo Code's folder stops the move after a few rounds, in plain words, and the next try takes what was written", () => {
  const m = mac07(root)
  const written = join(m.S, "Chats", "c1", "written-meanwhile.md")
  const stopped = run(m, { KUVO_TEST_REHOME_WRITE_AT: "after-check", KUVO_TEST_REHOME_WRITE_FILE: written, KUVO_TEST_REHOME_WRITE_TEXT: "new" })
  expect(stopped.status).toBe(1)
  expect(stopped.said!.message).toBe("Something kept changing your sessions while Kuvo copied them, so nothing was moved. Quit the earlier Kuvo Code wherever it's open, then try again.")
  expect(stopped.errors).toContain("Running the move again (3 of 3)")
  expect(existsSync(join(m.K, "box"))).toBe(false)
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readFileSync(join(m.K, "chats", "c1", "written-meanwhile.md"), "utf8")).toBe("new")
}, LIMIT * 4)

test.if(mac)("a program that only reads the earlier Kuvo Code's database while it's copied (its shared-memory index rewritten) never stops the move", () => {
  const shm = (m: { S: string }) => join(m.S, "opencode", "data", "opencode", "opencode.db-shm")
  const m = mac07(root, m => put(shm(m), "index"))
  const moved = run(m, { KUVO_TEST_REHOME_WRITE_AT: "after-check", KUVO_TEST_REHOME_WRITE_FILE: shm(m), KUVO_TEST_REHOME_WRITE_TEXT: "index, read again" })
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(moved.errors).not.toContain("Running the move again")
}, LIMIT)

test.if(mac)("a program that opens and closes the earlier Kuvo Code's database to read it while it's copied (SQLite's files beside it made and taken away) never stops the move", () => {
  const m = mac07(root)
  // (Its log, empty as a reader leaves it, and its index: there now, none when it was cloned.)
  const moved = run(m, { KUVO_TEST_REHOME_WRITE_AT: "after-check", KUVO_TEST_REHOME_WRITE_FILE: join(m.S, "opencode", "data", "opencode", "opencode.db-wal"), KUVO_TEST_REHOME_WRITE_TEXT: "" })
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(moved.errors).not.toContain("Running the move again")
}, LIMIT)

test.if(mac)("the earlier app still open is stopped, but programs of yours run from its folder (a chat's build) and another copy of Kuvo itself never are", async () => {
  const m = mac07(root)
  const sign = (file: string) => expect(spawnSync("/usr/bin/codesign", ["-f", "-s", "-", file]).status).toBe(0)
  // A program built in a chat's folder there, running.
  const built = join(m.S, "Chats", "c1", "build", "server")
  mkdirSync(join(m.S, "Chats", "c1", "build"), { recursive: true })
  spawnSync("/bin/cp", ["/bin/zsh", built]); sign(built)
  // Another copy of the app moving it (its ID, and its program's name), opened from elsewhere.
  const copy = join(m.home, "Downloads", "Kuvo Code.app")
  mkdirSync(join(copy, "Contents", "MacOS"), { recursive: true })
  spawnSync("/bin/cp", [join(m.bundle, "Contents", "Info.plist"), join(copy, "Contents", "Info.plist")])
  const program = join(copy, "Contents", "MacOS", "kuvo")
  spawnSync("/bin/cp", ["/bin/zsh", program]); sign(program)
  // And the earlier app itself, still open: its ID, a program of another name.
  const earlier = join(m.home, "Applications", "Earlier.app")
  mkdirSync(join(earlier, "Contents", "MacOS"), { recursive: true })
  put(join(earlier, "Contents", "Info.plist"), readFileSync(join(m.bundle, "Contents", "Info.plist"), "utf8").replace("<string>kuvo</string>", "<string>Kuvo Code</string>"))
  const earlierProgram = join(earlier, "Contents", "MacOS", "Kuvo Code")
  spawnSync("/bin/cp", ["/bin/zsh", earlierProgram]); sign(earlierProgram)
  const running = [built, program, earlierProgram].map(file => spawn(file, ["-c", "read line"], { stdio: ["pipe", "ignore", "ignore"] }))
  const earlierEnded = new Promise<string | null>(resolve => running[2].once("exit", (_, signal) => resolve(signal)))
  try {
    await new Promise(resolve => setTimeout(resolve, 0))
    const moved = run(m)
    expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
    expect(moved.errors).toContain(`Stopping what still runs of the earlier Kuvo Code: ${running[2].pid} ${earlierProgram}`)
    expect(moved.errors).not.toContain(`${running[0].pid} `)
    expect(moved.errors).not.toContain(`${running[1].pid} `)
    expect(await earlierEnded).toBe("SIGTERM")
    for (const each of running.slice(0, 2)) expect(each.exitCode === null && each.signalCode === null).toBe(true)
  } finally {
    for (const each of running) { try { each.stdin!.end() } catch {} ; if (each.exitCode === null && each.signalCode === null) await new Promise(resolve => each.once("exit", resolve)) }
  }
}, LIMIT)

test.if(mac)("an earlier Kuvo Code's program still running from its folder and writing there is found and stopped before anything's copied, and the move finishes without you", async () => {
  const m = mac07(root)
  // (Its engine's program, as a stand-in: a shell, run from the earlier Kuvo Code's own folder, as
  // its coding agents' bridge runs its engine; it writes in its folder all the while.)
  const program = join(m.S, "engine", "2.0.32", "opencode")
  rmSync(program)
  spawnSync("/bin/cp", ["/bin/zsh", program])
  // (Signed anew, ad hoc: macOS runs a system program only from where the system keeps it.)
  expect(spawnSync("/usr/bin/codesign", ["-f", "-s", "-", program]).status).toBe(0)
  const log = join(m.S, "Chats", "c1", "activity.jsonl")
  // (Never for good, should the test itself be stopped part way.)
  const writer = spawn(program, ["-c", 'for i in {1..600}; do echo x >> "$1"; /bin/sleep 0.1; done', "sh", log], { stdio: "ignore" })
  const ended = new Promise<string | null>(resolve => writer.once("exit", (_, signal) => resolve(signal)))
  try {
    await new Promise<void>(resolve => { const look = () => existsSync(log) ? resolve() : setTimeout(look, 20); look() })
    const moved = run(m)
    expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
    expect(moved.said).toMatchObject({ result: "done" })
    expect(moved.errors).toContain(`Stopping what still runs of the earlier Kuvo Code: ${writer.pid} ${program}`)
    // (Ended by the move, before it copied anything: the test only hears of it now.)
    expect(await ended).toBe("SIGTERM")
  } finally {
    try { process.kill(writer.pid!, "SIGKILL") } catch {}
  }
}, LIMIT)

test.if(mac)("another app writing in the earlier Kuvo Code's folder is named in words, never stopped", async () => {
  const m = mac07(root)
  const log = join(m.S, "Chats", "c1", "open.log")
  // (A program that isn't Kuvo's, holding a file there open for writing.)
  const other = spawn("/bin/zsh", ["-c", 'exec 3>>"$1"; echo ready; read line', "zsh", log], { stdio: ["pipe", "pipe", "ignore"] })
  try {
    await new Promise<void>(resolve => other.stdout.once("data", () => resolve()))
    const refused = run(m)
    expect(refused.status).toBe(1)
    expect(refused.said!.message).toBe("“zsh” is still using your earlier Kuvo Code's files. Quit it, then try again.")
    expect(refused.said!.message).not.toContain("/")
    expect(refused.errors).toContain(`${other.pid} /bin/zsh: ${log}`)
    expect(other.exitCode).toBe(null)
  } finally {
    other.stdin.end()
    await new Promise(resolve => other.once("exit", resolve))
  }
  expect(run(m).said).toMatchObject({ result: "done" })
}, LIMIT)

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
}, LIMIT)

test.if(mac)("run again once it's moved, nothing moves twice, and Move Back says there's nothing to put back", () => {
  const m = mac07(root)
  expect(run(m).said).toMatchObject({ result: "done" })
  const K = snapshot(m.K, moveOwn)
  const again = run(m)
  expect(again.status).toBe(0)
  expect(again.said).toMatchObject({ result: "done", status: { state: "done" } })
  expect(snapshot(m.K, moveOwn)).toEqual(K)
  expect(rehome(m, ["--back"]).said).toMatchObject({ result: "refused", message: "Kuvo on this Mac was moved from an earlier Kuvo Code, whose own folder is as it was: there's nothing to move back." })
}, LIMIT)

test.if(mac)("saved sign-ins are never read by the move, nor by opening or updating Kuvo after it: kuvo-keys is never asked for them, even where it would wait on macOS", () => {
  // (kuvo-keys here waits on macOS's question for as long as no one answers it, as it did at every
  // update before: asked, it's noted, and it answers only once this test's folder is gone.)
  const m = mac07(root, m => put(join(m.bundle, "Contents", "MacOS", "kuvo-keys"), readFileSync(join(m.bundle, "Contents", "MacOS", "kuvo-keys"), "utf8").replace("passwords)\n", "passwords)\n    n=0; while [ -d \"$HOME\" ] && [ $n -lt 300 ]; do sleep 1; n=$((n+1)); done; exit 1 ;;\nnever)\n"), 0o755))
  const moved = run(m)
  expect(moved.status, moved.errors).toBe(0)
  expect(moved.said).toMatchObject({ result: "done", status: { state: "done", owed: ["passwords"] } })
  // (Nothing's said of them here: Kuvo asks you about them when you can answer.)
  const said = moved.said
  if (said === null) throw new Error(`the move said nothing it could read: ${moved.errors}`)
  expect(said.message ?? "").not.toContain("sign-ins")
  // Opening Kuvo again, an update or not: this Mac's own, nothing to run, and the move, asked
  // again all the same (an older app), runs nothing and asks nothing.
  expect(judgeInstall(m.K, m.S)).toMatchObject({ verdict: "ours", verb: null })
  const again = run(m)
  expect(again.status, again.errors).toBe(0)
  expect(again.said).toMatchObject({ result: "done", status: { state: "done", owed: ["passwords"] } })
  expect(readFileSync(join(m.home, "keys-asked"), "utf8")).not.toContain("passwords")
  // Nor does what the box settles as it runs: what's owed that asks nothing.
  const settled = rehome(m, ["--settle"])
  expect(settled.status, settled.errors).toBe(0)
  expect(settled.said).toMatchObject({ status: { owed: ["passwords"] } })
  expect(readFileSync(join(m.home, "keys-asked"), "utf8")).not.toContain("passwords")
}, LIMIT)

test.if(mac)("what a move that's done still owes that asks nothing is settled by the box as it runs, never the saved sign-ins", () => {
  const m = mac07(root)
  const moved = run(m)
  expect(moved.status, moved.errors).toBe(0)
  // (As a move that couldn't take the Keychain's originals away left it.)
  const state = join(m.K, "updates", "rehome", "state.json")
  put(state, JSON.stringify({ ...readJSON(state), owed: ["keychain", "passwords"] }))
  put(join(m.keychain, "app", "phone-link.key"), "the link's key")
  const settled = rehome(m, ["--settle"])
  expect(settled.status, settled.errors).toBe(0)
  expect(settled.said).toMatchObject({ result: "done", status: { owed: ["passwords"] } })
  expect(readdirSync(join(m.keychain, "app"))).toEqual([])
  expect(readFileSync(join(m.home, "keys-asked"), "utf8").trim().split("\n").slice(-1)).toEqual(["delete-originals"])
}, LIMIT)

test.if(mac)("a Mac moved before, still owing saved sign-ins its earlier Kuvo Code had brought into a vault itself, owes them no more once the box runs: its preferences say so, no Keychain asked", () => {
  const m = mac07(root)
  expect(run(m).said).toMatchObject({ result: "done", status: { owed: ["passwords"] } })
  preferences(m, { "passwords.movedToVault": true })
  const settled = rehome(m, ["--settle"])
  expect(settled.status, settled.errors).toBe(0)
  expect(settled.said).toMatchObject({ result: "done", status: { owed: [] } })
  expect(readFileSync(join(m.home, "keys-asked"), "utf8")).not.toContain("passwords")
}, LIMIT)

/** A Mac that ran a keeper build: a box of the app's own in its folder (`S/box`), its engine's
 * database already the pinned engine's, its lists the box's own. */
function keeperBuild(m: ReturnType<typeof mac07>) {
  const file = join(m.S, "opencode", "data", "opencode", "opencode.db")
  for (const suffix of ["", "-wal", "-shm"]) rmSync(file + suffix, { force: true })
  databaseWithLog(file, recorded("engine-2.2.6", m), join(m.home, "tmp", "make-db"))
  put(join(m.S, "box", "run", "setup-card.json"), JSON.stringify({ self: { key: "k", name: "Mac", kind: "mac" }, app: true }))
  put(join(m.S, "box", "run", "link.json"), JSON.stringify({ name: "Mac", phones: [] }))
  put(join(m.S, "box", "run", "mac-link.json"), JSON.stringify({ phones: false, relay: "wss://relay.example.test", name: "Mac", app: "0.8.0" }))
  put(join(m.S, "box", "run", "serve.json"), JSON.stringify({ pid: 999_998, port: 1, password: "x" }))
  put(join(m.S, "box", "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: m.repository, name: "The box's name", folders: [] }] }))
  put(join(m.S, "box", "attachments", "ses_a", "b.png"), "the box's own b")
  put(join(m.S, "box", "bin", "mcp-catalog.json"), "{\"the app's\":true}")
  preferences(m, { projectFolders: [m.repository, join(m.home, "Projects", "plain")], projectSettings: { data: { [m.repository]: { name: "The app's old name", folders: [] } } }, phoneLink: true })
}

test.if(mac)("a keeper build's own box moves the same way: its lists kept and what the app still held added, its link as it was set, its old keeper stopped first", async () => {
  const m = mac07(root, keeperBuild)
  // Its keeper still running, answering on its control as the app's did (`POST stop`).
  const keeper = join(m.home, "keeper.ts")
  put(keeper, `const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request) { if (request.method === "POST" && new URL(request.url).pathname === "/stop" && request.headers.get("authorization") === "Bearer pw") { setTimeout(() => process.exit(0), 50); return Response.json({}) } return new Response("", { status: 404 }) } })\nconsole.log(server.port)\n`)
  const running = spawn(process.execPath, [keeper, "--box"], { stdio: ["ignore", "pipe", "ignore"] })
  try {
    const port = await new Promise<string>(resolve => running.stdout.once("data", chunk => resolve(String(chunk).trim())))
    put(join(m.S, "box", "run", "keeper.json"), JSON.stringify({ pid: running.pid, url: `http://127.0.0.1:${port}`, password: "pw", started: 1 }))
    const before = snapshot(m.S)
    const moved = run(m)
    expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
    expect(moved.said).toMatchObject({ result: "done", status: { state: "done", owed: ["passwords"] } })
    // (Asked to stop on its control, it went by itself.)
    expect(await new Promise<number | null>(resolve => (running.exitCode !== null ? resolve(running.exitCode) : running.once("exit", status => resolve(status))))).toBe(0)
    expect(snapshot(m.S)).toEqual(before)
  } finally {
    if (running.exitCode === null) running.kill()
  }
  // The box's own list first, as it was, then what only the app's preferences had.
  expect(readJSON(join(m.K, "box", "projects.json")).projects).toEqual([{ directory: m.repository, name: "The box's name", folders: [] }, { directory: join(m.home, "Projects", "plain"), folders: [] }])
  // Its identity and its link's settings as they were; none of its processes' records.
  expect(readJSON(join(m.K, "run", "link.json"))).toEqual({ name: "Mac", phones: [] })
  expect(readJSON(join(m.K, "run", "mac-link.json"))).toMatchObject({ phones: false, relay: "wss://relay.example.test" })
  expect(existsSync(join(m.K, "run", "serve.json"))).toBe(false)
  expect(existsSync(join(m.K, "run", "keeper.json"))).toBe(false)
  // A file sent that both kept under one name: both kept, and the message names the one it sent.
  expect(readFileSync(join(m.K, "attachments", "ses_a", "b.png"), "utf8")).toBe("the box's own b")
  expect(readFileSync(join(m.K, "attachments", "ses_a", "b 2.png"), "utf8")).toBe("PNGDATA")
  expect(readFileSync(join(m.K, "bin", "mcp-catalog.json"), "utf8")).toBe("the release's bin/mcp-catalog.json\n")
  expect(rows(join(m.K, "data", "opencode", "opencode.db")).session_v2.length).toBe(5)
}, LIMIT)

test.if(!mac)("on Linux there's nothing to move: `--run` says so and makes nothing", () => {
  const home = join(root, "linux")
  const bundle = join(home, "Kuvo Code.app")
  put(join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.code.trial713</string></dict></plist>\n`)
  const K = join(home, ".kuvo-code")
  const refused = rehome({ home, K } as Parameters<typeof rehome>[0], ["--run", "--app", bundle, "--release", join(K, "updates", "install", "release"), "--engine", join(K, "updates", "install", "engine")])
  expect(refused.status).toBe(1)
  expect(refused.said).toMatchObject({ result: "refused", message: "Kuvo on this machine is in its own home already: there's nothing to move." })
  expect(existsSync(join(K, "bin"))).toBe(false)
  expect(existsSync(join(K, "box"))).toBe(false)
}, LIMIT)

test.if(mac)("an account added to Claude Code comes along: its folder, its history of each folder that moved, and its sign-in under its new name", () => {
  // The Keychain its sign-in is in: a test's own file, never in your search list, nor your login keychain.
  const keychain = join(root, "run-test.keychain-db")
  spawnSync("/usr/bin/security", ["create-keychain", "-p", "test", keychain])
  spawnSync("/usr/bin/security", ["unlock-keychain", "-p", "test", keychain])
  const item = (folder: string) => `Claude Code-credentials-${sha("sha256", folder).slice(0, 8)}`
  const valueOf = (service: string) => {
    const found = spawnSync("/usr/bin/security", ["find-generic-password", "-s", service, "-w", keychain], { encoding: "utf8" })
    return found.status === 0 ? found.stdout.replace(/\n$/, "") : null
  }
  try {
    const slug = (folder: string) => folder.replace(/[^a-zA-Z0-9]/g, "-")
    const m = mac07(root, m => {
      const account = join(m.S, "agents", "claude", "accounts", "ab12cd34")
      put(join(account, "projects", slug(m.chat), "c.jsonl"), "{}\n")
      put(join(m.S, "agents", "store.json"), "{}")
      spawnSync("/usr/bin/security", ["add-generic-password", "-U", "-a", "you", "-s", item(account), "-X", Buffer.from("{\"token\":\"t\"}").toString("hex"), keychain])
    })
    const before = snapshot(m.S)
    const moved = run(m, { KUVO_TEST_REHOME_KEYCHAIN: keychain })
    expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
    expect(snapshot(m.S)).toEqual(before)
    const account = join(m.K, "agents", "claude", "accounts", "ab12cd34")
    expect(existsSync(join(account, "projects", slug(join(m.K, "chats", "c1")), "c.jsonl"))).toBe(true)
    expect(valueOf(item(account))).toBe("{\"token\":\"t\"}")
    expect(readJSON(join(m.K, "updates", "rehome", "state.json")).counts).toMatchObject({ accounts: 1, accountsBefore: 1 })
  } finally {
    spawnSync("/usr/bin/security", ["delete-keychain", keychain])
  }
}, LIMIT)

test.if(mac && REAL_ENGINE !== null)("the real pinned engine makes of 0.7.2's database what the recording says it does: the same migrations, sessions and messages", () => {
  const m = mac07(root)
  expect(run(m).said).toMatchObject({ result: "done" })
  const real = rows(join(m.K, "data", "opencode", "opencode.db"))
  const file = join(m.home, "tmp", "recorded.db")
  const made = new Database(file)
  made.exec(recorded("engine-2.0.32-migrated", m))
  made.close()
  const told = rows(file)
  expect(real.migration.map(each => each.id)).toEqual(told.migration.map(each => each.id))
  expect(Object.keys(real).sort()).toEqual(Object.keys(told).sort())
  expect(real.session_v2.map(each => [each.id, each.title, each.project_id.length]).sort()).toEqual(told.session_v2.map(each => [each.id, each.title, each.project_id.length]).sort())
  expect(real.session_message.map(each => [each.id, each.session_id, each.type, each.seq])).toEqual(told.session_message.map(each => [each.id, each.session_id, each.type, each.seq]))
  expect(real.session_inbox.map(each => each.id)).toEqual(told.session_inbox.map(each => each.id))
}, LIMIT)

test.if(mac)("a Mac that's one of your machines already, with an earlier Kuvo Code's sessions too, isn't put together yet: said plainly, with both left exactly as they are", () => {
  const m = mac07(root, m => {
    // A machine's box in its home, as another Mac set it up: its keeper's script, its link, its own sessions' folder.
    put(join(m.K, "bin", "kuvo-start"), "#!/bin/sh\n", 0o700)
    put(join(m.K, "run", "link.json"), JSON.stringify({ mac: "A".repeat(43), macs: ["A".repeat(43)], phones: [] }))
    put(join(m.K, "run", "setup-card.json"), JSON.stringify({ self: { key: "k", name: "Mini", kind: "machine" } }))
    put(join(m.K, "chats", "theirs", "notes.md"), "the machine's own")
  })
  expect(judgeInstall(m.K, m.S)).toMatchObject({ verdict: "two", verb: null })
  const before = { S: snapshot(m.S), K: snapshot(m.K) }
  const refused = run(m)
  expect(refused.status).toBe(1)
  expect(refused.said).toMatchObject({ result: "refused", message: "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet." })
  // (Nothing noted in that box's home either: only the move's own empty folder is new.)
  expect({ S: snapshot(m.S), K: snapshot(m.K, relative => relative === "updates/rehome") }).toEqual({ ...before, K: Object.fromEntries(Object.entries(before.K).filter(([name]) => name !== "updates/rehome" && !name.startsWith("updates/rehome/"))) })
  expect(readdirSync(join(m.K, "updates", "rehome"))).toEqual([])
  expect(existsSync(join(m.keychain, "helper"))).toBe(false)
  expect(judgeInstall(m.K, m.S)).toMatchObject({ verdict: "two", verb: null })
}, LIMIT)

test.if(mac)("folders the earlier Kuvo Code left empty (its last bot deleted, no group chat left) come along empty: they never stop the move", () => {
  const m = mac07(root, m => { for (const name of ["bots", "groups", "agents", "wakes"]) mkdirSync(join(m.S, name), { recursive: true }) })
  const before = snapshot(m.S)
  const moved = run(m)
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(moved.said).toMatchObject({ result: "done", status: { tries: 0 } })
  for (const name of ["bots", "groups", "agents", "wakes"]) expect(readdirSync(join(m.K, name)), name).toEqual([])
  expect(snapshot(m.S)).toEqual(before)
}, LIMIT)

test.if(mac)("settings of the earlier Kuvo Code that are there and can't be read stop the move, which goes back: never moved without them", () => {
  const m = mac07(root, m => put(join(m.home, "Library", "Preferences", "run.kuvo.code.trial713.plist"), "bplist00 cut short"))
  const stopped = run(m)
  expect(stopped.status).toBe(1)
  expect(stopped.said).toMatchObject({ result: "failed", message: "Kuvo couldn't read the earlier Kuvo Code's settings, so nothing was moved. Try again.", status: { state: "failed", failed: { step: "upgrade" } } })
  expect(existsSync(join(m.K, "box"))).toBe(false)
  expect(existsSync(join(m.keychain, "helper"))).toBe(false)
  // Readable again (none at all is none), it moves.
  rmSync(join(m.home, "Library", "Preferences", "run.kuvo.code.trial713.plist"))
  expect(run(m).said).toMatchObject({ result: "done" })
}, LIMIT)

test.if(mac)("a message still waiting on a session in the earlier engine's own table, which the pinned engine lets go of, is said: never dropped unsaid", () => {
  const m = mac07(root, m => {
    const file = join(m.S, "opencode", "data", "opencode", "opencode.db")
    for (const suffix of ["", "-wal", "-shm"]) rmSync(file + suffix, { force: true })
    const sql = recorded("engine-2.0.32", m)
    const session = /INSERT INTO session_v2 VALUES\('(ses_[A-Za-z0-9]+)'/.exec(sql)![1]
    databaseWithLog(file, sql + `\nINSERT INTO session_pending VALUES('msg_waiting0000000000000001', '${session}', 'user', '{"text":"still to send"}', 'queue', 1, 1);`, join(m.home, "tmp", "make-db"))
  })
  const moved = run(m)
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(moved.said!.status.dropped).toContain("A message still waiting to reach an agent when the earlier Kuvo Code last quit wasn't kept: send it again.")
}, LIMIT)

test.if(mac)("a link in the earlier Kuvo Code's folder that names another place there by its whole path points at where that is now; one by a path from its own place stays", () => {
  const m = mac07(root, m => {
    symlinkSync(join(m.S, "Chats", "c1", "notes.md"), join(m.S, "Chats", "c1", "whole-link"))
    symlinkSync("notes.md", join(m.S, "Chats", "c1", "near-link"))
    symlinkSync(join(m.home, "Projects", "plain", "p.txt"), join(m.S, "Chats", "c1", "outside-link"))
  })
  const before = snapshot(m.S)
  expect(run(m).said).toMatchObject({ result: "done" })
  const chat = join(m.K, "chats", "c1")
  expect(readlinkSync(join(chat, "whole-link"))).toBe(join(chat, "notes.md"))
  expect(readlinkSync(join(chat, "near-link"))).toBe("notes.md")
  expect(readlinkSync(join(chat, "outside-link"))).toBe(join(m.home, "Projects", "plain", "p.txt"))
  expect(snapshot(m.S)).toEqual(before)
}, LIMIT)

test.if(mac)("this Mac's own Kuvo, made here already, is never moved over: whoever asks", () => {
  const m = mac07(root, m => put(join(m.K, "box", "kind"), "mac\n"))
  const before = snapshot(m.K)
  const refused = run(m)
  expect(refused.status).toBe(1)
  expect(refused.said).toMatchObject({ result: "refused", message: "Kuvo is set up on this Mac already, so nothing was moved over it. An earlier Kuvo Code's sessions are still in its own folder." })
  expect(snapshot(m.K, relative => relative === "updates/rehome")).toEqual(Object.fromEntries(Object.entries(before).filter(([name]) => !name.startsWith("updates/rehome"))))
}, LIMIT)

test.if(mac)("a move whose own record can't be read says so in words with what to do, never by its file's name alone, and changes nothing", () => {
  const m = mac07(root, m => put(join(m.K, "updates", "rehome", "journal.json"), "{\"v\": 1, \"step\": \"cle"))
  const before = snapshot(m.S)
  const stuck = run(m)
  expect(stuck.status).toBe(3)
  const message = "Kuvo was stopped part way through moving your sessions and can't tell how far it got, so it's changing nothing more. Try again; if this stays, send ~/.kuvo-code/updates/rehome/run.log to Kuvo's support."
  expect(stuck.said).toMatchObject({ result: "stuck", message, status: { failed: { message } } })
  expect(existsSync(join(m.K, "box"))).toBe(false)
  expect(snapshot(m.S)).toEqual(before)
}, LIMIT)

test.if(mac)("while a move is at work outside its journal, checking first or starting Kuvo after, its progress still says so; a mark left by one that was killed says nothing", async () => {
  const m = mac07(root)
  // (A move at work: a process that holds the move's lock, named as one is.)
  const holder = spawn("/bin/sh", ["-c", "echo ready; read line", "rehome"], { stdio: ["pipe", "pipe", "ignore"] })
  try {
    await new Promise<void>(resolve => holder.stdout.once("data", () => resolve()))
    put(join(m.K, "updates", "rehome", "lock", "pid"), String(holder.pid))
    put(join(m.K, "updates", "rehome", "run.phase"), "plan")
    expect(rehome(m, ["--status"]).said!.progress).toEqual({ step: "plan", title: "Checking this Mac", index: 1, of: 11 })
    put(join(m.K, "updates", "rehome", "run.phase"), "settle")
    expect(rehome(m, ["--status"]).said!.progress).toEqual({ step: "done", title: "Starting Kuvo on this Mac", index: 11, of: 11 })
  } finally {
    holder.stdin.end()
    await new Promise(resolve => holder.once("exit", resolve))
  }
  expect(rehome(m, ["--status"]).said!.progress).toBe(null)
  // And a move run to its end leaves no mark.
  rmSync(join(m.K, "updates", "rehome", "lock"), { recursive: true })
  expect(run(m).said).toMatchObject({ result: "done", status: { progress: null } })
  expect(existsSync(join(m.K, "updates", "rehome", "run.phase"))).toBe(false)
}, LIMIT)
