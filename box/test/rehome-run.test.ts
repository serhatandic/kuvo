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
  // This Mac's keys: copied for the helper the release brings, read back, then the app's own taken away.
  expect(readFileSync(join(m.home, "keys-asked"), "utf8").trim().split("\n").map(line => line.trim())).toEqual(["copy Kuvo Helper.app", "delete-originals", "passwords"])
  expect(readdirSync(join(m.keychain, "helper")).sort()).toEqual(["integrations.KUVO_KEY_NOTION", "phone-link.key"])
  expect(readdirSync(join(m.keychain, "app"))).toEqual([])
  // Nothing of the move's left over: no staging, no kept copy, nothing the installer staged.
  expect(existsSync(join(m.K, "updates", "rehome", "staging"))).toBe(false)
  expect(existsSync(join(m.K, "updates", "rehome", "kept"))).toBe(false)
  expect(existsSync(join(m.K, "updates", "install"))).toBe(false)
  expect(readJSON(join(m.K, "updates", "rehome", "state.json"))).toMatchObject({ state: "done", owed: [], counts: { sessions: 5, sessionsBefore: 5, projects: 3, messages: 13 } })
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
    expect(again.said, point).toMatchObject({ result: "done", status: { state: "done", owed: [] } })
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
    expect(refused.said!.message).toMatch(/^Your sessions are still open in another program on this Mac \(.+\)\. Quit Kuvo Code wherever it's open, then try again\.$/)
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
  expect(refused.said).toMatchObject({ result: "refused", message: "Your sessions were last used by a version of Kuvo Code that this one doesn't know how to bring along (51 20270101000000_later). Nothing was changed. Update Kuvo Code if a newer version is out, then try again." })
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

test.if(mac)("something written in the earlier Kuvo Code's folder while it's copied sends the move back: nothing is left behind unsaid", () => {
  const m = mac07(root)
  const written = join(m.S, "Chats", "c1", "written-meanwhile.md")
  const stopped = run(m, { KUVO_TEST_REHOME_WRITE_AT: "after-check", KUVO_TEST_REHOME_WRITE_FILE: written, KUVO_TEST_REHOME_WRITE_TEXT: "new" })
  expect(stopped.status).toBe(1)
  expect(stopped.said!.message).toBe(`Something changed in ${join(m.S, "Chats", "c1")} while your sessions were being copied. Close what's using that folder (Kuvo Code, an editor, a terminal), then try again.`)
  expect(existsSync(join(m.K, "box"))).toBe(false)
  // The next try takes what was written too.
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readFileSync(join(m.K, "chats", "c1", "written-meanwhile.md"), "utf8")).toBe("new")
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

test.if(mac)("saved sign-ins wait for Kuvo to answer on this Mac: until then they're still owed, never written anywhere, and the installer says to run the move again", () => {
  const m = mac07(root, m => put(join(m.home, "keychain-passwords.json"), JSON.stringify({ v: 1, logins: [{ host: "example.com", account: "me", password: "hunter22" }] })))
  const moved = run(m)
  expect(moved.status, moved.errors).toBe(0)
  expect(moved.said).toMatchObject({ result: "done", status: { state: "done", owed: ["passwords"] } })
  expect(moved.errors).toContain("Your saved sign-ins couldn't be brought into the vault yet: Kuvo isn't answering on this Mac yet")
  // Nowhere in the box's home: not in a file, not in the move's own records.
  const everything = Object.keys(snapshot(m.K)).filter(name => !name.startsWith("engine/")).map(name => { try { return readFileSync(join(m.K, name), "utf8") } catch { return "" } }).join("\n")
  expect(everything).not.toContain("hunter22")
  expect(judgeInstall(m.K, m.S)).toMatchObject({ verdict: "ours", verb: "move" })
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
    expect(moved.said).toMatchObject({ result: "done", status: { state: "done", owed: [] } })
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
  expect(refused.said).toMatchObject({ result: "refused", message: "This machine's box is in its own home already: there's nothing to move." })
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
  expect(stuck.said).toMatchObject({ result: "stuck", message: "Kuvo was stopped part way through moving your sessions and couldn't finish by itself, so it's changing nothing more. The earlier Kuvo Code's own folder is as it was. Try again; if this stays, send ~/.kuvo-code/updates/rehome/run.log to Kuvo's support." })
  expect(stuck.said!.status.failed.message).not.toContain("journal")
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
