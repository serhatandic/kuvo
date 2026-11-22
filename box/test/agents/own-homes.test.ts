// Where the coding agents keep your first accounts (docs/architecture/agent-accounts.md): in the
// agent's own folder (`~/.claude`, `~/.codex`, `~/.grok`), the one Terminal uses, on every box: a
// Mac's however its box runs, and a Linux machine's. An earlier Linux bridge kept them in its folder
// (`<agent>/home`): updating moves them over, signed in and with their conversations.

import "../safety"
import { afterAll, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { Accounts, adoptOwnFolders } from "../../src/agents/accounts"
import { tmpdir } from "node:os"
import { join } from "node:path"

const base = realpathSync(mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-own-homes-")))
afterAll(() => rmSync(base, { recursive: true, force: true }))

/** Each agent's default account's folder and an added one's, as the bridge started in `env` has them
 * ("yours" where it's the agent's own, in your home). */
function homes(env: Record<string, string>): Record<string, string> {
  const script = join(base, "homes.ts")
  writeFileSync(script, [
    `import { Accounts } from ${JSON.stringify(join(import.meta.dir, "../../src/agents/accounts"))}`,
    "const accounts = new Accounts(process.env.AGENTS!)",
    "const added = accounts.add(\"claude\", \"Work\")",
    "const said: Record<string, string> = {}",
    "for (const kind of [\"claude\", \"codex\", \"grok\"] as const) said[kind] = accounts.home({ kind, id: \"default\" }) ?? \"yours\"",
    "said.added = accounts.home(added)!.replace(process.env.AGENTS!, \"<bridge>\").replace(added.id, \"<id>\")",
    "process.stdout.write(JSON.stringify(said))",
  ].join("\n"))
  const clean = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KUVO_"))) as Record<string, string>
  const ran = Bun.spawnSync([process.execPath, script], { cwd: base, env: { ...clean, ...env }, stderr: "pipe" })
  expect(ran.stderr.toString()).toBe("")
  return JSON.parse(ran.stdout.toString())
}

test("every box's first accounts are the agents' own folders, a machine's too; an added one is in the bridge's", () => {
  const machine = join(base, "machine", ".kuvo-code"), mac = join(base, "mac", ".kuvo-code")
  mkdirSync(join(mac, "box"), { recursive: true })
  writeFileSync(join(mac, "box", "kind"), "mac\n")
  mkdirSync(machine, { recursive: true })
  const yours = { claude: "yours", codex: "yours", grok: "yours", added: "<bridge>/claude/accounts/<id>" }
  expect(homes({ KUVO_AGENTS_REMOTE: "1", KUVO_HOME: machine, AGENTS: join(machine, "agents") })).toEqual(yours)
  expect(homes({ KUVO_AGENTS_REMOTE: "1", KUVO_HOME: mac, AGENTS: join(mac, "agents") })).toEqual(yours)
  expect(homes({ AGENTS: join(base, "app", "agents") })).toEqual(yours)
})

/** A Linux box an earlier bridge ran: its first sign-ins and conversations in `<agent>/home`. */
function earlier(root: string, kind: "codex" | "grok", signIn: string) {
  const old = join(root, kind, "home")
  mkdirSync(join(old, "sessions", "2026", "11", "01"), { recursive: true })
  writeFileSync(join(old, "auth.json"), signIn)
  writeFileSync(join(old, "sessions", "2026", "11", "01", "rollout-thread-1.jsonl"), "the conversation")
  return old
}

test("updating, an earlier bridge's first sign-in and its conversations move into the agent's own folder, and nothing is left behind", () => {
  const you = join(base, "update-you"), root = join(base, "update", "agents")
  const home = process.env.HOME
  process.env.HOME = you
  try {
    const old = earlier(root, "codex", "kuvo-codex-sign-in")
    earlier(root, "grok", "kuvo-grok-sign-in")
    const accounts = new Accounts(root)
    adoptOwnFolders(root, accounts)
    expect(readFileSync(join(you, ".codex", "auth.json"), "utf8")).toBe("kuvo-codex-sign-in")
    expect(readFileSync(join(you, ".codex", "sessions", "2026", "11", "01", "rollout-thread-1.jsonl"), "utf8")).toBe("the conversation")
    expect(readFileSync(join(you, ".grok", "auth.json"), "utf8")).toBe("kuvo-grok-sign-in")
    expect(existsSync(old)).toBe(false)
    expect(existsSync(join(root, "grok", "home"))).toBe(false)
    // One account each, the agent's own: no copy of the sign-in anywhere.
    expect(accounts.all().filter(each => each.kind === "codex" || each.kind === "grok")).toEqual([{ kind: "codex", id: "default" }, { kind: "grok", id: "default" }])
  } finally {
    process.env.HOME = home
  }
})

test("updating where you're signed in to the agent already, yours stays the first account and the earlier one is an added account", () => {
  const you = join(base, "both-you"), root = join(base, "both", "agents")
  const home = process.env.HOME
  process.env.HOME = you
  try {
    mkdirSync(join(you, ".codex"), { recursive: true })
    writeFileSync(join(you, ".codex", "auth.json"), "your-terminal-sign-in")
    earlier(root, "codex", "kuvo-codex-sign-in")
    const accounts = new Accounts(root)
    adoptOwnFolders(root, accounts)
    expect(readFileSync(join(you, ".codex", "auth.json"), "utf8")).toBe("your-terminal-sign-in")
    // Its conversations go on in the first account's folder.
    expect(readFileSync(join(you, ".codex", "sessions", "2026", "11", "01", "rollout-thread-1.jsonl"), "utf8")).toBe("the conversation")
    const added = accounts.all().filter(each => each.kind === "codex" && each.id !== "default")
    expect(added.map(each => each.label)).toEqual(["Earlier sign-in"])
    expect(readFileSync(join(accounts.home(added[0]!)!, "auth.json"), "utf8")).toBe("kuvo-codex-sign-in")
    expect(existsSync(join(root, "codex", "home"))).toBe(false)
  } finally {
    process.env.HOME = home
  }
})

// A bot moved from an earlier Kuvo Code remembers this Mac's group chats in its own conversation
// under the key the Mac's app always used (`kuvo.groups`). Its box's bridge, started as a machine's
// in its home of its own, keeps them under that same key: another would leave the moved one there
// as it was, beside the new (the runner writes and removes only its own key: groups-runner.test.ts).
test("a Mac's box in a home of its own remembers its group chats under the Mac's key; a machine's, under its own", () => {
  const machine = join(base, "machine-groups", ".kuvo-code"), mac = join(base, "mac-groups", ".kuvo-code")
  mkdirSync(join(mac, "box"), { recursive: true })
  writeFileSync(join(mac, "box", "kind"), "mac\n")
  mkdirSync(machine, { recursive: true })
  const script = join(base, "memory-key.ts")
  writeFileSync(script, [
    `import { groupsMemoryKey } from ${JSON.stringify(join(import.meta.dir, "../../src/agents/machine"))}`,
    `import { memoryKey } from ${JSON.stringify(join(import.meta.dir, "../../src/agents/groups/runner"))}`,
    "process.stdout.write(groupsMemoryKey ?? memoryKey)",
  ].join("\n"))
  const clean = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KUVO_"))) as Record<string, string>
  const key = (env: Record<string, string>) => {
    const ran = Bun.spawnSync([process.execPath, script], { cwd: base, env: { ...clean, ...env }, stderr: "pipe" })
    expect(ran.stderr.toString()).toBe("")
    return ran.stdout.toString()
  }
  expect(key({ KUVO_AGENTS_REMOTE: "1", KUVO_HOME: mac })).toBe("kuvo.groups")
  expect(key({ KUVO_AGENTS_REMOTE: "1", KUVO_HOME: machine })).toBe("kuvo.groups.machine")
  // As the retired app started it in its folder, naming it.
  expect(key({ KUVO_GROUPS_MEMORY_KEY: "kuvo.groups" })).toBe("kuvo.groups")
})
