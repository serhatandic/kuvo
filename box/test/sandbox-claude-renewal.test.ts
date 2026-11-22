import "./safety"
import { KEPT_IN_HOME, STATE_IN_HOME } from "../src/agents/agents/claude"
import { expect, setDefaultTimeout, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fenceAgent } from "../src/agents/fence"
import type { FencePolicy } from "../src/link/fence/policy"
import { canFence, realFence } from "./agents/support/fence"

// Claude Code renews its own sign-in as it runs, and in Auto it runs whole in the box's sandbox
// (claude.ts `whole`), so the renewed sign-in must be written from inside it, or it signs out when
// the old one expires. Its sign-in is a file in its settings folder (`.credentials.json`, Linux's,
// and Claude Code's fallback on a Mac), which it saves under a new name and moves into place, under
// a lock folder beside it (`.storage-write.lock`); on a Mac it's an item in your login Keychain,
// which it writes through `/usr/bin/security`. What's real: the box's policy for a home of the
// test's own, its launcher and the sandbox; Claude Code's fence options as claude.ts passes them.
// What's a stand-in: Claude Code itself (Bun saving the file as it does; `security` as it calls it)
// and the login Keychain (a keychain of the test's own, where yours is, never in your search list).

// Each test starts Bun for the box's policy and runs commands in a real sandbox.
setDefaultTimeout(30_000)

// What Claude Code writes in its folder, and what it keeps there, as claude.ts fences it.
const CLAUDE_STATE = STATE_IN_HOME
const CLAUDE_KEPT = KEPT_IN_HOME

/** The box's policy as it'd be for a home of the test's own (run as the box runs it, with HOME there). */
function policyIn(home: string): FencePolicy {
  const asStarted = Bun.spawnSync([process.execPath, "-e", `import { fencePolicy } from ${JSON.stringify(join(import.meta.dir, "..", "src", "link", "fence", "policy"))}; console.log(JSON.stringify(fencePolicy()))`],
    { env: { ...process.env as Record<string, string>, HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", XDG_CACHE_HOME: "", KUVO_OWN_XDG_CACHE_HOME: "" } })
  expect(asStarted.exitCode, asStarted.stderr.toString()).toBe(0)
  return JSON.parse(asStarted.stdout.toString())
}

/** The box's sandbox, on, for a home of the test's own (`home`), with Kuvo Code's folder (`kuvo`)
 * hidden; `claude` runs a program as Claude Code runs whole in it, with `settings` its settings folder. */
function sandboxed(root: string) {
  const kuvo = join(root, "kuvo")
  const home = join(root, "home")
  const project = join(root, "project")
  for (const folder of [kuvo, home, project]) mkdirSync(folder, { recursive: true })
  const policy = policyIn(home)
  // (Only what's in the test's home: the rest of this computer's temporary folders, which the test's
  // folder is in, aren't written.)
  const within = (each: string, top: string) => each === top || each.startsWith(top + "/")
  const mine = (paths: string[] | undefined) => [...new Set((paths ?? []).map(each => !within(each, home) && within(each, policy.home) ? home + each.slice(policy.home.length) : each).filter(each => within(each, home)))]
  const fenceFolder = realFence(kuvo, [], {
    writable: ["/dev", ...mine(policy.writable)], readOnly: mine(policy.readOnly), homeWrites: mine(policy.homeWrites), homeKept: mine(policy.homeKept),
    homeOpen: mine(policy.homeOpen), personal: mine(policy.personal), secrets: mine(policy.secrets), loginKeychains: mine(policy.loginKeychains),
  })
  const claude = (settings: string, program: string, args: string[], input?: string) => {
    process.env.KUVO_FENCE = fenceFolder
    const fence = fenceAgent(program, project, { homes: [settings], state: CLAUDE_STATE, kept: CLAUDE_KEPT, read: [process.execPath], session: "ses_renewal" })
    delete process.env.KUVO_FENCE
    expect(fence).toBeDefined()
    // (The stand-in is this test's Bun, which may be the engine's runtime, run as Bun: on Linux the launcher says so.)
    const asBun = program === process.execPath ? { KUVO_FENCE_BUN: "1", ...(process.env.BUN_BE_BUN ? { BUN_BE_BUN: process.env.BUN_BE_BUN } : {}) } : {}
    const ran = Bun.spawnSync([fence!.command, ...args], { cwd: project, stdin: input === undefined ? "ignore" : Buffer.from(input), env: { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: home, ...fence!.env, ...asBun } })
    return { status: ran.exitCode, out: ran.stdout.toString() + ran.stderr.toString() }
  }
  return { kuvo, home, claude }
}

// Saved as Claude Code saves its sign-in: under its lock folder, whole under a new name, moved into place.
const RENEW = `const fs = require("fs"), path = require("path")
const [folder, file, text] = process.argv.slice(-3)
const lock = path.join(folder, ".storage-write.lock")
fs.mkdirSync(lock)
const saving = path.join(folder, file) + ".tmp." + process.pid + "." + require("crypto").randomBytes(6).toString("hex")
fs.writeFileSync(saving, text, { mode: 0o600 })
fs.renameSync(saving, path.join(folder, file))
fs.chmodSync(path.join(folder, file), 0o600)
fs.rmdirSync(lock)`

const signIn = (token: string) => JSON.stringify({ claudeAiOauth: { accessToken: `sk-ant-oat01-${token}`, refreshToken: `sk-ant-ort01-${token}`, expiresAt: 1, scopes: ["user:inference"] } })

test.skipIf(!canFence)("Claude Code renews its sign-in in your own ~/.claude from inside the sandbox", () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-sandbox-renewal-")))
  try {
    const { home, claude } = sandboxed(root)
    const settings = join(home, ".claude")
    mkdirSync(settings)
    writeFileSync(join(settings, ".credentials.json"), signIn("old"), { mode: 0o600 })
    const ran = claude(settings, process.execPath, ["-e", RENEW, settings, ".credentials.json", signIn("renewed")])
    expect(ran.out).toBe("")
    expect(ran.status).toBe(0)
    expect(readFileSync(join(settings, ".credentials.json"), "utf8")).toBe(signIn("renewed"))
    expect(readdirSync(settings)).toEqual([".credentials.json"])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test.skipIf(!canFence)("an account you added renews its sign-in and its record in its own folder from inside the sandbox", () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-sandbox-renewal-")))
  try {
    const { kuvo, claude } = sandboxed(root)
    // (In the bridge's folder, Kuvo Code's, which the sandbox hides but for an agent's own: its CLAUDE_CONFIG_DIR.)
    const settings = join(kuvo, "agents", "claude", "accounts", "acc_1")
    mkdirSync(settings, { recursive: true })
    writeFileSync(join(settings, ".credentials.json"), signIn("old"), { mode: 0o600 })
    const renewed = claude(settings, process.execPath, ["-e", RENEW, settings, ".credentials.json", signIn("renewed")])
    expect(renewed.out).toBe("")
    expect(renewed.status).toBe(0)
    const recorded = claude(settings, process.execPath, ["-e", RENEW, settings, ".claude.json", '{"oauthAccount":{"emailAddress":"you@example.com"}}'])
    expect(recorded.out).toBe("")
    expect(recorded.status).toBe(0)
    expect(readFileSync(join(settings, ".credentials.json"), "utf8")).toBe(signIn("renewed"))
    expect(readFileSync(join(settings, ".claude.json"), "utf8")).toBe('{"oauthAccount":{"emailAddress":"you@example.com"}}')
    expect(readdirSync(settings).sort()).toEqual([".claude.json", ".credentials.json"])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

// On a Mac, Claude Code keeps its sign-in in your login Keychain, one file of all your items
// (`~/Library/Keychains/login.keychain-db`), which `security` saves anew whole beside it
// (`login.keychain-db.sb-<hex>-<random>`) and moves into place. The sandbox keeps ~/Library from
// commands whole (policy.ts `MAC_HOME_KEPT`), but for that file, written by `security` alone
// (seatbelt.ts `loginKeychains`): so Claude Code renews its sign-in there, and nothing else
// writes the file.
test.skipIf(!canFence || process.platform !== "darwin")("Claude Code renews its sign-in in your login Keychain from inside the sandbox, and nothing but `security` writes it", () => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-sandbox-renewal-")))
  const keychain = join(root, "home", "Library", "Keychains", "login.keychain-db")
  try {
    const { home, claude } = sandboxed(root)
    const settings = join(home, ".claude")
    mkdirSync(settings)
    mkdirSync(join(home, "Library", "Keychains"), { recursive: true })
    // (Made outside the sandbox, as yours is; never added to your search list.)
    const made = Bun.spawnSync(["security", "create-keychain", "-p", "kuvo-test", keychain], { stdin: "ignore" })
    expect(made.exitCode, made.stderr.toString()).toBe(0)
    const item = (token: string) => `add-generic-password -U -a "kuvo-test" -s "Kuvo test credentials" -X "${Buffer.from(signIn(token)).toString("hex")}" "${keychain}"\n`
    expect(Bun.spawnSync(["security", "-i"], { stdin: Buffer.from(item("old")) }).exitCode).toBe(0)
    // As Claude Code writes it, through `security -i`.
    const ran = claude(settings, "/usr/bin/security", ["-i"], item("renewed"))
    expect(ran.out).toBe("")
    expect(ran.status).toBe(0)
    const read = Bun.spawnSync(["security", "find-generic-password", "-a", "kuvo-test", "-s", "Kuvo test credentials", "-w", keychain], { stdin: "ignore" })
    expect(read.stdout.toString().trim()).toBe(signIn("renewed"))
    expect(readdirSync(join(home, "Library", "Keychains")).filter(name => name.includes(".sb-"))).toEqual([])
    // Any other program is kept from the file, as from the rest of ~/Library.
    const before = readFileSync(keychain)
    expect(claude(settings, "/bin/sh", ["-c", `printf x >> '${keychain}'`]).status).not.toBe(0)
    expect(claude(settings, "/bin/rm", ["-f", keychain]).status).not.toBe(0)
    expect(readFileSync(keychain).equals(before)).toBe(true)
  } finally {
    if (existsSync(keychain)) Bun.spawnSync(["security", "delete-keychain", keychain], { stdin: "ignore" })
    rmSync(root, { recursive: true, force: true })
  }
})
