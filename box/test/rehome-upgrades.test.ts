import "./safety"
import { afterAll, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { rewriteOwnFile } from "../src/link/rehome/own"
import { PATH_REWRITER } from "../src/link/rehome/rewrite"
import { parsePlist } from "../src/link/rehome/upgrades"
import { mac07, preferences, put, readJSON, run, snapshot, type Mac } from "./rehome-run-world"

// What an earlier Kuvo Code kept in its own files and preferences, brought into the box's own
// files by the move (`rehome --run`, #713 piece 4): each as that version's app wrote it, moved by
// the real command, and read where the box reads it.
const mac = process.platform === "darwin"
const root = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-upgrades-")))
afterAll(() => rmSync(root, { recursive: true, force: true }))

const ISO = "2026-10-01T10:00:00Z"
const bot = (m: Mac, id: string, more: Record<string, unknown> = {}) => ({
  id, name: id === "bot_a1" ? "Scout" : "Mini's bot", job: "Watch the build", instructions: "Watch the build, and say when it breaks.", look: { color: "blue", shape: "round", face: "smile" },
  folder: join(m.S, "bots", id), sessionID: `ses_${id}`, created: ISO, ...more,
})

/** What Kuvo Code 0.7.2 kept beside its engine's data: its preferences, its list of bots and one
 * bot's folder, a group chat with a bot on a machine in it, a personality, a message waiting for
 * a machine that was away, and settings naming an integration's key in its temporary folder. */
function kept07(m: Mac, { described = true }: { described?: boolean } = {}) {
  preferences(m, {
    projectFolders: [m.repository, join(m.home, "Projects", "plain"), join(m.S, "bots", "bot_a1", "workspace"), "ssh://mini/srv/app"],
    projectSettings: { data: { [m.repository]: { name: "My Project", folders: [join(m.home, "extra"), m.repository], run: "npm start", terminal: "iTerm" }, [join(m.home, "Projects", "old")]: { folders: [] }, "ssh://mini/srv/app": { folders: [] } } },
    worktrees: { data: { [m.worktree]: { project: m.repository, root: m.worktree, branch: "kuvo/feature-one", base: "main" }, [join(m.home, "nowhere")]: { project: m.repository, root: join(m.home, "nowhere"), branch: "kuvo/gone" }, "ssh://mini/srv/wt": { project: "ssh://mini/srv/app", root: "ssh://mini/srv/wt", branch: "x" } } },
    removedWorktrees: { [join(m.S, "opencode", "data", "opencode", "worktree", "gone")]: m.repository },
    machines: { data: [{ destination: "mini.local", identity: "~/.ssh/id_mini", otherAddresses: ["10.0.0.5"] }] },
    "git.branchPrefix": "me/", "git.commitInstructions": " Sign off every commit. ",
    keepsAwake: false, toolApproval: "askFirst", "engine.defaultsAdded": true, favoriteModels: "anthropic/claude-opus-5-5,openai/gpt-6",
    groupMemoryWaiting: { bot_a1: ["grp_gone"] },
    composerDrafts: { data: { "ses_1": { text: "half a thought", files: [] }, "ses_2": { text: "another", files: [] } } },
    botEditsWaiting: { bot_m1: ["name"] },
    machineSignOuts: { mini: ["anthropic"] }, "machines.waiting": { mini: ["rm -rf x"], studio: ["rm -rf y"] },
    "kuvo.usage.connectedProviders.v1": ["claude", "openrouter"], showTotalSpend: false, "openusage.notifications.underTenPercent": true, "openusage.codex.fallbackModel": "gpt-5.2",
    appearance: "dark", sessionsWidth: 280, lastModelRef: "{\"id\":\"m\",\"providerID\":\"lab\"}", "notifications.sound": false, "analytics.enabled": false,
    // (0.7.2 had described each bot in its folder, and says so; 0.6 knew no such step.)
    ...(described ? { calibration: 1 } : {}),
  })
  put(join(m.S, "bots.json"), JSON.stringify([bot(m, "bot_a1", { agents: true, personality: "custom.aaaa" }), bot(m, "bot_m1", { machine: "mini", folder: "/home/u/.kuvo-code/bots/bot_m1" })], null, 2))
  put(join(m.S, "bots", "bot_a1", "MEMORY.md"), "what Scout remembers")
  put(join(m.S, "bots", "bot_a1", "workspace", "notes.md"), "notes")
  put(join(m.S, "bots", "bot_a1", "wakes", "1794000000-abc"), "check the build")
  if (described) {
    put(join(m.S, "bots", "bot_a1", "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_a1", name: "Scout", job: "Watch the build", look: {}, created: ISO, updated: ISO }))
    put(join(m.S, "bots", "bot_a1", "session"), "ses_bot_a1")
    put(join(m.S, "bots", "bot_a1", "directory"), join(m.S, "bots", "bot_a1", "workspace"))
  }
  put(join(m.S, "wakes", "ses_w1", "directory"), m.chat)
  put(join(m.S, "wakes", "ses_w1", "1794000000-xyz"), "look again")
  put(join(m.S, "groups.json"), JSON.stringify([
    { id: "grp_one", name: "Standup", members: ["bot_a1", "bot_m1"], seats: { bot_a1: "ses_seat_a", bot_m1: "ses_seat_m" }, heard: { bot_a1: "m2", bot_m1: "m1" }, agentSeats: ["ses_seat_m"], created: ISO, meeting: { next: "2026-12-01T09:00:00Z", topic: "Plans", every: "week" } },
    { id: "grp_bad", name: "Broken", members: "nobody" },
  ], null, 2))
  put(join(m.S, "groups", "grp_one.json"), JSON.stringify([
    { id: "m1", text: "Morning", time: ISO, kind: "message" },
    { id: "m2", author: "bot_a1", text: "Build is green", time: "2026-10-01T10:01:00.250Z", steps: [{ tool: "shell" }] },
    { id: "m3", author: null, text: "Thanks", time: ISO, kind: "note" },
    { id: "m4", text: "not a message the bridge reads", time: "yesterday" },
  ]))
  put(join(m.S, "personalities.json"), JSON.stringify([{ id: "custom.aaaa", name: "Dry", description: "Says little.", symbol: "moon" }]))
  put(join(m.S, "agent-messages.json"), JSON.stringify([{ name: "Scout", to: "ses_there", machine: "mini", text: "ping" }]))
  put(join(m.S, "phone-link.json"), JSON.stringify({ phones: [], machines: { mini: { name: "Mini", address: "a", key: "K".repeat(43) } } }))
  put(join(m.S, "opencode-files", "global-prompt.md"), "be brief")
  put(join(m.S, "opencode", "config", "opencode", "opencode.json"), JSON.stringify({
    instructions: [`${m.S}/opencode-files/global-prompt.md`],
    mcp: { servers: { notion: { type: "remote", url: "https://mcp.notion.com", headers: { Authorization: `Bearer {file:${m.home}/tmp/${"run.kuvo.code"}/integration-keys/KUVO_KEY_NOTION_TOKEN}` } } } },
  }))
  put(join(m.S, "Browser", "history.json"), "[]")
}

test.if(mac)("what 0.7.2 kept itself comes along: projects, worktrees, machines, Git, approval, bots, a group chat, a personality, each where the box reads it", () => {
  const m = mac07(root, m => kept07(m))
  const before = snapshot(m.S), preferencesBefore = readFileSync(join(m.home, "Library", "Preferences", "run.kuvo.code.trial713.plist"))
  const moved = run(m)
  expect(moved.status, moved.errors + JSON.stringify(moved.said)).toBe(0)
  expect(snapshot(m.S)).toEqual(before)
  expect(readFileSync(join(m.home, "Library", "Preferences", "run.kuvo.code.trial713.plist")).equals(preferencesBefore)).toBe(true)
  const { K } = m
  expect(readJSON(join(K, "folder-version.json"))).toEqual({ version: 12 })
  const worktree = join(K, "data", "opencode", "worktree", m.rootCommit.slice(0, 6), "feature one")
  // Projects, in the sidebar's order, with their names, folders and Run commands; one kept only
  // for its settings, removed as it was; never a bot's folder, nor a machine's.
  expect(readJSON(join(K, "box", "projects.json"))).toEqual({ version: 1, projects: [
    { directory: m.repository, name: "My Project", folders: [join(m.home, "extra")], run: "npm start" },
    { directory: join(m.home, "Projects", "plain"), folders: [] },
    { directory: join(m.home, "Projects", "old"), folders: [], removed: true },
  ] })
  // Worktrees' records, where they are now; one whose folder is gone isn't one; a removed one's project.
  expect(readJSON(join(K, "box", "worktrees.json"))).toEqual({
    formatVersion: 1,
    worktrees: [{ project: m.repository, root: worktree, folder: worktree, branch: "kuvo/feature-one", base: "main" }], removed: {}, restorable: [],
    handedOver: { [join(K, "data", "opencode", "worktree", "gone")]: m.repository },
  })
  expect(readJSON(join(K, "machines.json"))).toEqual([{ destination: "mini.local", otherAddresses: ["10.0.0.5"], identity: "~/.ssh/id_mini" }])
  // The engine's settings: the integration's key named where the box writes it, Settings › Git
  // taken once, and the file the settings name moved with them.
  const settings = readJSON(join(K, "config", "opencode", "opencode.json"))
  expect(settings.mcp.servers.notion.headers.Authorization).toBe("Bearer {file:~/.kuvo-code/box/keys/KUVO_KEY_NOTION_TOKEN}")
  expect(settings).toMatchObject({ branch_prefix: "me/", commit_instructions: "Sign off every commit.", instructions: [join(K, "opencode-files", "global-prompt.md")] })
  expect(readFileSync(join(K, "opencode-files", "global-prompt.md"), "utf8")).toBe("be brief")
  expect(readJSON(join(K, "box", "git.json"))).toEqual({ moved: true, formatVersion: 1 })
  expect(readJSON(join(K, "box", "work.json"))).toEqual({ keepsAwake: false })
  // Your starred models come along, for every client's picker.
  expect(readJSON(join(K, "box", "models.json"))).toEqual({ starred: [{ providerID: "anthropic", id: "claude-opus-5-5" }, { providerID: "openai", id: "gpt-6" }] })
  expect(readJSON(join(K, "box", "approval.json"))).toEqual({ last: "askFirst" })
  expect(readFileSync(join(K, "box", "defaults-added"), "utf8")).toBe("websearch\n")
  // Someone who used an earlier Kuvo Code has had the first run: they land in their sessions.
  expect(existsSync(join(K, "run", "welcomed.json"))).toBe(true)
  // "Let my devices connect" was never set on this Mac: the box starts with its own (on).
  expect(existsSync(join(K, "run", "mac-link.json"))).toBe(false)
  // This Mac's bot, as the box reads one; the list beside the bots' folder, naming where they are now.
  expect(readJSON(join(K, "bots", "bot_a1", "bot.json"))).toMatchObject({ kuvo: 1, id: "bot_a1", name: "Scout" })
  expect(readFileSync(join(K, "bots", "bot_a1", "session"), "utf8")).toBe("ses_bot_a1")
  expect(readFileSync(join(K, "bots", "bot_a1", "directory"), "utf8")).toBe(join(K, "bots", "bot_a1", "workspace"))
  expect(readFileSync(join(K, "bots", "bot_a1", "via"), "utf8")).toBe("bridge")
  expect(readJSON(join(K, "bots.json")).map((each: any) => each.folder)).toEqual([join(K, "bots", "bot_a1"), "/home/u/.kuvo-code/bots/bot_m1"])
  expect(readFileSync(join(K, "wakes", "ses_w1", "directory"), "utf8")).toBe(join(K, "chats", "c1"))
  // The group chat, as the coding agents' bridge keeps one: the machine's bot's seat taken out,
  // only messages it reads, and what a bot was still to forget.
  expect(readJSON(join(K, "groups", "grp_one", "group.json"))).toEqual({ kuvo: 1, status: "active", group: {
    id: "grp_one", name: "Standup", members: ["bot_a1", "bot_m1"], seats: { bot_a1: "ses_seat_a" }, heard: { bot_a1: "m2" }, created: ISO, agentSeats: [],
    meeting: { next: "2026-12-01T09:00:00Z", topic: "Plans", every: "week" },
  } })
  expect(readJSON(join(K, "groups", "grp_one", "messages.json")).map((each: any) => [each.id, each.kind, each.author])).toEqual([["m1", "message", undefined], ["m2", "message", "bot_a1"], ["m3", "note", undefined]])
  expect(existsSync(join(K, "groups", "grp_one.json"))).toBe(false)
  expect(readJSON(join(K, "groups", "pending-memory.json"))).toEqual({ bot_a1: ["grp_gone"] })
  expect(readJSON(join(K, "bots", "personalities.json"))).toEqual({ kuvo: 1, personalities: [{ id: "custom.aaaa", name: "Dry", description: "Says little.", symbol: "moon", updated: 0 }], removed: [] })
  // The usage panel's settings, by the box's own import.
  const usage = readJSON(join(K, "agents", "usage", "settings.json"))
  expect(usage).toMatchObject({ kuvo: 1, imported: true, codex: { fallback: "gpt-5.2" } })
  expect(usage.units.totalSpend.value).toEqual({ show: false })
  expect(usage.units.notifications.value).toEqual({ underTenPercent: true, healthyToClose: false, closeToRunningOut: false })
  // What didn't come along is said, each once, in words; and what was counted before is all there.
  const state = readJSON(join(K, "updates", "rehome", "state.json"))
  expect(state.dropped).toEqual([
    "One group chat couldn't be read, and wasn't brought along. It's still in the earlier Kuvo Code's folder (groups.json).",
    "One message in your group chats couldn't be read, and wasn't brought along.",
    "In your group chats, a bot on one of your machines takes its seat again at its next turn: the conversation it had there before stays on that machine, unused.",
    "A message one of your agents sent to an agent on a machine that was away wasn't delivered, and isn't brought along.",
    "2 messages you'd typed but not sent weren't brought along.",
    "Websites you were signed in to in Kuvo Code's browser need signing in again. Your history comes along.",
    "A change you made to a bot on a machine that was away (a name, look, personality or model) wasn't sent there. Make it again once that machine is connected.",
    "The command that opened in place of the chat for a project isn't kept.",
    "You signed out of an account while a machine was away: it may still be signed in to it. Sign out there too once it's connected.",
    "2 machines that were away still have files of something you removed here (a bot, or Kuvo Code itself): remove them from there once connected.",
    "A project on your machines may have lost its name, extra folders or Run command, which the earlier Kuvo Code kept on this Mac. Set them again in each project's settings.",
    "A worktree on your machines may show its sessions under another project: the earlier Kuvo Code kept which project it belonged to on this Mac.",
    "An API key you pasted into the usage panel (OpenRouter's or Z.ai's) isn't brought along. Paste it again in the usage panel.",
    "The model new sessions start with is chosen again in the new app.",
    "You had turned off some of Kuvo Code's notifications (or their sound or badge). Set that again in the new app.",
    "You had turned off sharing anonymous usage in the earlier Kuvo Code. That choice isn't brought along: check it in the new app's settings.",
    // (Whether your Keychain has any can't be told in a copy's home: they're said to stay there.)
    "Passwords the earlier Kuvo Code saved for websites stay in your Keychain for now: they aren't in Kuvo's vault yet.",
  ])
  expect(state.counts).toMatchObject({ sessions: 5, bots: 1, botsBefore: 1, wakeUps: 2, wakeUpsBefore: 2, groupChats: 1, groupChatsBefore: 0, worktreeRecords: 1, worktreeRecordsBefore: 0 })
  expect(moved.said!.status.dropped).toEqual(state.dropped)
}, 120_000)

test.if(mac)("a bot of 0.6, which had no description in its folder, is one the box reads after the move", () => {
  const m = mac07(root, m => kept07(m, { described: false }))
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readJSON(join(m.K, "bots", "bot_a1", "bot.json"))).toEqual({
    created: ISO, id: "bot_a1", job: "Watch the build", kuvo: 1, look: { color: "blue", shape: "round", face: "smile" }, name: "Scout", personality: "custom.aaaa", updated: ISO,
  })
  expect(readFileSync(join(m.K, "bots", "bot_a1", "session"), "utf8")).toBe("ses_bot_a1")
  expect(readFileSync(join(m.K, "bots", "bot_a1", "directory"), "utf8")).toBe(join(m.K, "bots", "bot_a1", "workspace"))
  // (0.6 described a machine's bot only on this Mac: said.)
  expect(readJSON(join(m.K, "updates", "rehome", "state.json")).dropped).toContain("A bot on your machines was described only on this Mac by the earlier Kuvo Code, and may not show there. Its work is still on that machine: make it again if it's missing.")
}, 120_000)

test.if(mac)("a Mac with no machines left gets an empty list of them, never none: no list beside what it keeps of machines it had would read as one that can't be told (#753)", () => {
  // (What it keeps of a machine it had, `phone-link.json`, stays after the machine is removed.)
  const m = mac07(root, m => { kept07(m); preferences(m, { calibration: 1 }) })
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readJSON(join(m.K, "machines.json"))).toEqual([])
}, 120_000)

test.if(mac)("\"Let my devices connect\" stays as this Mac's link had it set, with a relay of your own; a Mac that ran everything without asking isn't told anything about it (new sessions start with Auto as everywhere)", () => {
  const m = mac07(root, m => preferences(m, { phoneLink: false, "phoneLink.relay": "wss://relay.example.test/" }))
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readJSON(join(m.K, "run", "mac-link.json"))).toMatchObject({ phones: false, relay: "wss://relay.example.test", app: "" })
  expect(existsSync(join(m.K, "box", "approval.json"))).toBe(false)
  // (Only that a copy's home can't tell whether your Keychain has saved sign-ins.)
  expect(readJSON(join(m.K, "updates", "rehome", "state.json")).dropped).toEqual(["Passwords the earlier Kuvo Code saved for websites stay in your Keychain for now: they aren't in Kuvo's vault yet."])
}, 120_000)

test.if(mac)("0.8.0-beta.1's own: personalities you removed stay removed, and a letter already seen stays seen", () => {
  const now = Date.now()
  const m = mac07(root, m => {
    put(join(m.S, "personalities.json"), JSON.stringify([{ id: "custom.aaaa", name: "Dry", description: "Says little.", symbol: "moon", updated: 5 }, { id: "custom.bbbb", name: "Gone", description: "Was here.", symbol: "sun", updated: 2 }]))
    put(join(m.S, "personalities-removed.json"), JSON.stringify([{ id: "custom.bbbb", removed: 9 }]))
    put(join(m.S, "agent-letters.json"), JSON.stringify([{ id: "ltr_abcdef", to: "ses_there", toName: "x", machine: "mini", text: "hello", wait: false, at: now }, { id: "ltr_nokey1", to: "ses_x", toName: "y", machine: "other", text: "lost", wait: false, at: now }]))
    put(join(m.S, "agent-letters-seen.json"), JSON.stringify({ ltr_seen01: now - 1000, ltr_old001: now - 5 * 86_400_000 }))
    put(join(m.S, "phone-link.json"), JSON.stringify({ phones: [], machines: { mini: { name: "Mini", address: "a", key: "K".repeat(43), peers: true } } }))
  })
  expect(run(m).said).toMatchObject({ result: "done" })
  expect(readJSON(join(m.K, "bots", "personalities.json"))).toEqual({ kuvo: 1, personalities: [{ id: "custom.aaaa", name: "Dry", description: "Says little.", symbol: "moon", updated: 5 }], removed: [{ id: "custom.bbbb", removed: 9 }] })
  expect(readJSON(join(m.K, "run", "outbox", "sending", "ltr_abcdef.json"))).toEqual({ from: null, to: "ses_there", text: "hello", wait: false, at: now, peer: "K".repeat(43) })
  expect(existsSync(join(m.K, "run", "outbox", "sending", "ltr_nokey1.json"))).toBe(false)
  expect(readJSON(join(m.K, "run", "letters-seen.json"))).toEqual({ ltr_seen01: now - 1000 })
}, 120_000)

test("a property list reads as the app's preferences are kept: text, numbers, switches, lists, and what it kept as JSON", () => {
  const read = parsePlist(`<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "x">\n<plist version="1.0">\n<dict>\n\t<key>a &amp; b</key>\n\t<string>one &lt;two&gt;</string>\n\t<key>empty</key>\n\t<string></string>\n\t<key>n</key>\n\t<integer>3</integer>\n\t<key>r</key>\n\t<real>2.5</real>\n\t<key>on</key>\n\t<true/>\n\t<key>off</key>\n\t<false/>\n\t<key>list</key>\n\t<array>\n\t\t<string>x</string>\n\t\t<dict>\n\t\t\t<key>k</key>\n\t\t\t<string>v</string>\n\t\t</dict>\n\t</array>\n\t<key>none</key>\n\t<array/>\n\t<key>kept</key>\n\t<data>\n\t${Buffer.from(JSON.stringify({ deep: [1, "two"] })).toString("base64")}\n\t</data>\n\t<key>after</key>\n\t<string>still read</string>\n</dict>\n</plist>\n`)
  expect(read).toEqual({ "a & b": "one <two>", empty: "", n: 3, r: 2.5, on: true, off: false, list: ["x", { k: "v" }], none: [], kept: { deep: [1, "two"] }, after: "still read" })
}, 120_000)

test("rehome stamps its folders list without changing what the previous reader sees", () => {
  const work = mkdtempSync(join(root, "folders-"))
  const from = join(work, "app", "bots")
  const home = join(work, "box")
  const staged = join(work, "staged")
  const expected = join(from, "bot_scout", "workspace")
  mkdirSync(expected, { recursive: true })
  mkdirSync(staged)
  const move = { way: "move" as const, home, appFolder: join(work, "app"), placements: [{ from, to: join(home, "bots"), staged }], map: [], work }
  const file = join(work, "folders.json")
  writeFileSync(file, JSON.stringify([expected]))
  PATH_REWRITER.rewrite(move)
  expect(readJSON(file)).toEqual([{ formatVersion: 1 }, expected])
  expect(readJSON(file).filter((entry: unknown) => typeof entry === "string")).toEqual([expected])
  const future = JSON.stringify([{ formatVersion: 2 }, work])
  writeFileSync(file, future)
  expect(() => PATH_REWRITER.outside(move, () => { throw new Error("Unexpected recovery mutation") })).toThrow("Rehome project folders format version 2 is unsupported")
  expect(() => PATH_REWRITER.rewrite(move)).toThrow("Rehome project folders format version 2 is unsupported")
  expect(readFileSync(file, "utf8")).toBe(future)
})

test("rehome rewrites supported artifact indexes and leaves a future log byte for byte", () => {
  const work = mkdtempSync(join(root, "artifact-log-"))
  const from = join(work, "old")
  const to = join(work, "new")
  const file = join(work, "2026-11.jsonl")
  const input = { file, relative: "artifacts/log/2026-11.jsonl", kind: "lines-json" as const }
  const line = { a: { id: "art_test", path: join(from, "notes.md") }, formatVersion: 1 }
  writeFileSync(file, JSON.stringify(line) + "\n")
  expect(rewriteOwnFile(input, [{ from, to }])).toBe(true)
  expect(readJSON(file)).toEqual({ ...line, a: { ...line.a, path: join(to, "notes.md") } })
  const future = JSON.stringify(line) + "\n" + JSON.stringify({ ...line, formatVersion: 2 }) + "\n"
  writeFileSync(file, future)
  expect(() => rewriteOwnFile(input, [{ from, to }])).toThrow("Artifact index format version 2 is unsupported")
  expect(readFileSync(file, "utf8")).toBe(future)
})
