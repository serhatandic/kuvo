// Updating a Mac whose Claude Code ran on Kuvo's own sign-in (before 22 November 2026; claude-compat.ts):
// its `setup-token` token keeps working until Claude Code's own sign-in is there, which then takes
// over and the token goes; an Anthropic Console sign-in in Kuvo's folder becomes an account you
// added, in that very folder (Claude Code names its Keychain item after the folder's path). Claude
// Code is a fake program on the PATH that keeps a sign-in per folder, and notes any token it's handed.

import "../safety"
import { afterAll, afterEach, expect, test } from "bun:test"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import { Accounts, providerID } from "../../src/agents/accounts"
import { ClaudeAgent } from "../../src/agents/agents/claude"
import { adoptConsoleFolder } from "../../src/agents/claude-compat"
import { Roster } from "../../src/agents/roster"

const base = realpathSync(mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-claude-mac-update-")))
const bin = join(base, "bin")
const handed = join(base, "tokens-handed")
const path = process.env.PATH
mkdirSync(bin)
writeFileSync(join(bin, "claude"), `#!/bin/sh
dir="\${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
[ -n "$CLAUDE_CODE_OAUTH_TOKEN" ] && echo "$CLAUDE_CODE_OAUTH_TOKEN" >> '${handed}'
case "$1 $2" in
  "auth status")
    [ -f "$dir/signed-in" ] || { echo '{"loggedIn":false,"authMethod":"none"}'; exit 1; }
    echo '{"loggedIn":true,"authMethod":"claude.ai","email":"ada@example.com","subscriptionType":"max"}' ;;
esac
`)
chmodSync(join(bin, "claude"), 0o755)
process.env.PATH = [bin, path].join(delimiter)
const yours = join(process.env.HOME!, ".claude")
afterEach(() => rmSync(yours, { recursive: true, force: true }))
afterAll(() => {
  process.env.PATH = path
  rmSync(base, { recursive: true, force: true })
})

function bridgeFolder() {
  return mkdtempSync(join(base, "agents-"))
}

test("an earlier setup-token sign-in keeps Claude working until your own sign-in is there, which then takes over and the token goes", async () => {
  const home = bridgeFolder()
  mkdirSync(join(home, "claude", "tokens"), { recursive: true })
  writeFileSync(join(home, "claude", "tokens", "default.json"), JSON.stringify({ token: "sk-ant-oat01-earlier", made: 1 }))
  const agent = new ClaudeAgent({ home, temporary: home, deps: {} as any })

  // Not signed in to Claude Code yet: the earlier token still runs it, and the account says how to see its usage.
  expect(await agent.account(true)).toMatchObject({ installed: true, signedIn: true, label: "Claude plan", message: "Sign in again to see your plan's usage." })
  expect(agent.env().CLAUDE_CODE_OAUTH_TOKEN).toBe("sk-ant-oat01-earlier")

  // Signed in to Claude Code (in Terminal, say): yours is the account, and the token is gone.
  mkdirSync(yours, { recursive: true })
  writeFileSync(join(yours, "signed-in"), "")
  expect(await agent.account(true)).toMatchObject({ signedIn: true, label: "ada@example.com · Max plan" })
  expect(existsSync(join(home, "claude", "tokens"))).toBe(false)
  expect(agent.env().CLAUDE_CODE_OAUTH_TOKEN).toBeUndefined()
  // Asking whether it's signed in never hands Claude Code the token.
  expect(existsSync(handed) ? readFileSync(handed, "utf8") : "").toBe("")
})

test("an earlier Anthropic Console sign-in becomes an account you added, in the very folder its Keychain item is named after", () => {
  const home = bridgeFolder()
  const console = join(home, "claude", "console")
  mkdirSync(join(console, "projects", "-work"), { recursive: true })
  writeFileSync(join(console, ".claude.json"), "{}")
  writeFileSync(join(console, "signed-in"), "")
  const accounts = new Accounts(home)
  adoptConsoleFolder(home, accounts)
  // Once: a second start adds no other.
  adoptConsoleFolder(home, new Accounts(home))
  const added = new Accounts(home).all().filter((record) => record.kind === "claude" && record.id !== "default")
  expect(added).toHaveLength(1)
  expect(added[0]!.label).toBe("Anthropic Console")
  const roster = new Roster(new Accounts(home), { home, temporary: home, deps: {} as any } as any)
  const agent = roster.find(providerID("claude", added[0]!.id)) as ClaudeAgent
  expect(agent.env().CLAUDE_CONFIG_DIR).toBe(console)
  expect(existsSync(join(console, "projects", "-work"))).toBe(true)
  roster.all().forEach((each) => each.shutdown?.())
})

test("a Console folder Claude Code never ran in goes, and no account is added for it", () => {
  const home = bridgeFolder()
  mkdirSync(join(home, "claude", "console"), { recursive: true })
  const accounts = new Accounts(home)
  adoptConsoleFolder(home, accounts)
  expect(existsSync(join(home, "claude", "console"))).toBe(false)
  expect(accounts.all().filter((record) => record.id !== "default")).toEqual([])
})
