// Where the coding agents keep your default accounts (#333): on a machine, in Kuvo Code's folder
// there; on a Mac, in your own home (`~/.claude`, `~/.codex`, `~/.grok`), whichever way its box runs:
// in its app's folder, or in a home of its own (`~/.kuvo-code`, `box/kind` saying `mac`), where its
// bridge is started as a machine's. Moving the box mustn't sign you out of them or lose their history.

import "../safety"
import { afterAll, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs"
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

test("a Mac's box in a home of its own keeps your default accounts where they are; a machine's are in its folder", () => {
  const machine = join(base, "machine", ".kuvo-code"), mac = join(base, "mac", ".kuvo-code")
  mkdirSync(join(mac, "box"), { recursive: true })
  writeFileSync(join(mac, "box", "kind"), "mac\n")
  mkdirSync(machine, { recursive: true })
  // A machine's.
  expect(homes({ KUVO_AGENTS_REMOTE: "1", KUVO_HOME: machine, AGENTS: join(machine, "agents") })).toEqual({
    claude: join(machine, "agents", "claude", "home"), codex: join(machine, "agents", "codex", "home"), grok: join(machine, "agents", "grok", "home"),
    added: "<bridge>/claude/accounts/<id>",
  })
  // A Mac's, started the same way in its home of its own.
  expect(homes({ KUVO_AGENTS_REMOTE: "1", KUVO_HOME: mac, AGENTS: join(mac, "agents") })).toEqual({ claude: "yours", codex: "yours", grok: "yours", added: "<bridge>/claude/accounts/<id>" })
  // And as its app starts it in its folder.
  expect(homes({ AGENTS: join(base, "app", "agents") })).toEqual({ claude: "yours", codex: "yours", grok: "yours", added: "<bridge>/claude/accounts/<id>" })
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
