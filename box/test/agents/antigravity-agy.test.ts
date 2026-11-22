// Antigravity on Google's `agy` CLI (docs/architecture/agent-accounts.md): installed with Google's
// installer, signed in with `agy acp --login` (Google's page, its code pasted back) in a home of
// Kuvo Code's own, checked by `agy` itself before it counts, its Claude and GPT models named apart
// from Claude Code's and Codex's, signed out by `agy` itself, its version and the newest from
// Google's feed, and an account signed in before `agy` still running until it's signed in again.
// Real: the agent, its sign-in flow and the box's maintenance. Stand-ins: `agy` (a script that
// behaves as Google's 1.5 does, as seen on a Linux test machine: its `--login` prompt, its ACP
// `initialize`, its refusal when it has no sign-in) and Google's release feed.

import "../safety"
import { afterAll, afterEach, beforeAll, describe, expect, test } from "bun:test"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { AntigravityAgent, antigravityFeed } from "../../src/agents/agents/antigravity"

let root: string
let feed: ReturnType<typeof Bun.serve>
const feedBase = antigravityFeed.base
beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), "agy-"))
  feed = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: (request) => (new URL(request.url).pathname.endsWith(".json") ? Response.json({ version: "1.5.20", url: "https://example.invalid/agy.tar.gz", sha512: "0".repeat(128) }) : new Response(null, { status: 404 })) })
  antigravityFeed.base = `${feed.url.origin}/manifests`
})
afterAll(() => {
  antigravityFeed.base = feedBase
  feed.stop(true)
  rmSync(root, { recursive: true, force: true })
})
afterEach(() => {
  delete process.env.AGY_STANDIN_LOG
  delete process.env.GEMINI_API_KEY
})

/**
 * Google's `agy` 1.5, as a program. `--login` prints Google's page and its prompt; a code
 * `good-<who>` signs `<who>` in, in a file of its home (as it does when it thinks it runs over
 * SSH). Its ACP: `authenticate` refused with no sign-in, as Google's is; `session/new` lists the
 * models `models.json` beside it says for that account; `logout` forgets the sign-in. Each run
 * notes its arguments and environment in AGY_STANDIN_LOG.
 */
const STANDIN = String.raw`
const fs = require("node:fs"), path = require("node:path")
const args = process.argv.slice(2)
const home = process.env.GEMINI_HOME
const token = home && path.join(home, "antigravity-acp", "antigravity-oauth-token")
if (process.env.AGY_STANDIN_LOG) fs.appendFileSync(process.env.AGY_STANDIN_LOG, JSON.stringify({ args, env: { GEMINI_HOME: home, SSH_CONNECTION: process.env.SSH_CONNECTION, AGY_CLI_DISABLE_AUTO_UPDATE: process.env.AGY_CLI_DISABLE_AUTO_UPDATE, GEMINI_API_KEY: process.env.GEMINI_API_KEY, DISPLAY: process.env.DISPLAY } }) + "\n")
if (args[0] === "--version") { console.log("1.5.14"); process.exit(0) }
if (args.includes("--login")) {
  console.log("Open this link in a browser to log in with Google:\n\n  https://accounts.google.com/o/oauth2/auth?response_type=code&state=s\n")
  process.stdout.write("Then paste the authorization code, or the full URL of the page you land on, and press Enter: ")
  let typed = ""
  process.stdin.on("data", (chunk) => {
    typed += chunk
    if (!typed.includes("\n")) return
    const code = typed.trim()
    if (!code.startsWith("good-")) { console.error('Error: failed to exchange the code: oauth2: "invalid_grant" "Bad Request"'); process.exit(1) }
    fs.mkdirSync(path.dirname(token), { recursive: true })
    fs.writeFileSync(token, code.slice(5))
    process.exit(0)
  })
} else if (args[0] === "acp") {
  const models = JSON.parse(fs.readFileSync(path.join(__dirname, "models.json"), "utf8"))
  const send = (message) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n")
  let buffer = ""
  process.stdin.on("data", (chunk) => {
    buffer += chunk
    let at
    while ((at = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, at); buffer = buffer.slice(at + 1)
      if (!line.trim()) continue
      const { id, method } = JSON.parse(line)
      const who = fs.existsSync(token) ? fs.readFileSync(token, "utf8") : undefined
      if (method === "initialize") send({ id, result: { protocolVersion: 1, agentCapabilities: { auth: { logout: {} } }, agentInfo: { name: "Antigravity ACP Server", version: "1.5.14" }, authMethods: [{ id: "oauth-personal", name: "Log in with Google" }] } })
      else if (method === "authenticate") who ? send({ id, result: {} }) : send({ id, error: { code: -32603, message: "No browser is available here (this is a remote (SSH) session, so the browser's sign-in redirect can't reach this machine)." } })
      else if (method === "session/new") send({ id, result: { sessionId: "probe", ...(models[who] ? { models: { availableModels: models[who] } } : {}) } })
      else if (method === "logout") { fs.rmSync(token, { force: true }); send({ id, result: {} }) }
      else send({ id, result: {} })
    }
  })
}
`

/** Antigravity with the stand-in as its `agy` (in a folder of its own, named `agy`), on a box home of its own. */
class StandInAntigravity extends AntigravityAgent {
  readonly agy: string
  readonly log: string
  constructor(name: string, installed = true) {
    const home = join(root, name)
    super({ home, temporary: join(home, "temporary"), deps: {} as any })
    mkdirSync(join(home, "temporary"), { recursive: true })
    const bin = join(home, "bin")
    mkdirSync(bin, { recursive: true })
    writeFileSync(join(bin, "agy.js"), STANDIN)
    writeFileSync(join(bin, "models.json"), JSON.stringify({
      ada: [{ modelId: "gemini-3.8-flash", name: "Gemini 3.8 Flash" }, { modelId: "claude-opus-5-5", name: "Claude Opus 5.5" }, { modelId: "gpt-6.2-sol", name: "GPT-6.2 Sol" }],
    }))
    this.agy = join(bin, "agy")
    writeFileSync(this.agy, `#!/bin/sh\nexport BUN_BE_BUN=1\nexec "${process.execPath}" "${join(bin, "agy.js")}" "$@"\n`)
    chmodSync(this.agy, 0o755)
    if (!installed) rmSync(this.agy)
    this.log = join(home, "runs.jsonl")
  }
  override executable() {
    return existsSync(this.agy) ? this.agy : undefined
  }
  runs(): { args: string[]; env: Record<string, string | undefined> }[] {
    try {
      return readFileSync(this.log, "utf8").trim().split("\n").map((line) => JSON.parse(line))
    } catch {
      return []
    }
  }
}

/** Signs in with `code` pasted back from Google's page: how the attempt ended, and the page it gave. */
async function signIn(agent: StandInAntigravity, code: string) {
  process.env.AGY_STANDIN_LOG = agent.log
  const attempt = await agent.signIn()
  expect(attempt.acceptsCode).toBe(true)
  await attempt.complete!(code)
  for (let waited = 0; attempt.status().status === "pending"; waited += 50) {
    if (waited > 30_000) throw new Error("The sign-in didn't end.")
    await Bun.sleep(50)
  }
  return { url: attempt.url, status: attempt.status() }
}

describe("Antigravity on agy", () => {
  test("signing in: Google's page, its code pasted back, in a home of Kuvo Code's own; then its models, Claude's and GPT's named apart", async () => {
    const agent = new StandInAntigravity("signs-in")
    process.env.GEMINI_API_KEY = "your-other-google-key"
    expect(await agent.account()).toEqual({ installed: true, signedIn: false, message: "Sign in with your Google account." })

    const { url, status } = await signIn(agent, "good-ada")
    expect(url).toBe("https://accounts.google.com/o/oauth2/auth?response_type=code&state=s")
    expect(status).toEqual({ status: "complete" })
    expect(await agent.account()).toMatchObject({ installed: true, signedIn: true, label: "Google account" })
    expect((await agent.models()).map((model) => model.name)).toEqual(["Gemini 3.8 Flash", "Claude Opus 5.5 (Antigravity)", "GPT-6.2 Sol (Antigravity)"])

    // Each run in Kuvo Code's own home for the account (never your ~/.gemini), keeping its sign-in
    // there rather than in the keyring every home shares, not updating itself, without your other key.
    const runs = agent.runs()
    expect(runs.map((run) => run.args.includes("--login"))).toEqual([true, false])
    for (const run of runs) {
      expect(run.args.slice(0, 5)).toEqual(["acp", "--gemini-dir", agent.profile, "--app-data-dir", "antigravity-acp"])
      expect(run.env).toEqual({ GEMINI_HOME: agent.profile, SSH_CONNECTION: "127.0.0.1 0 127.0.0.1 0", AGY_CLI_DISABLE_AUTO_UPDATE: "1" })
    }
  })

  test("a code Google's page didn't give: not signed in, and it says so", async () => {
    const agent = new StandInAntigravity("wrong-code")
    const { status } = await signIn(agent, "not-a-code")
    expect(status).toEqual({ status: "failed", message: "That code didn't work. Start again, and paste the code from the page this sign-in opens." })
    expect((await agent.account()).signedIn).toBe(false)
  })

  test("signed in as someone else, the earlier account's models aren't shown as theirs", async () => {
    const agent = new StandInAntigravity("someone-else")
    await signIn(agent, "good-ada")
    expect((await agent.models()).length).toBe(3)
    // Bea's account lists no models (her session says none).
    expect((await signIn(agent, "good-bea")).status).toEqual({ status: "complete" })
    await expect(agent.models()).rejects.toThrow("Antigravity’s model list couldn’t be read.")
  })

  test("signing out: agy forgets the sign-in itself, and it's signed out here", async () => {
    const agent = new StandInAntigravity("signs-out")
    await signIn(agent, "good-ada")
    await agent.signOut()
    expect(existsSync(join(agent.profile, "antigravity-acp", "antigravity-oauth-token"))).toBe(false)
    expect(await agent.account()).toEqual({ installed: true, signedIn: false, message: "Sign in with your Google account." })
  })

  test("not installed: it says so, and Install runs Google's installer", async () => {
    const agent = new StandInAntigravity("not-installed", false)
    expect(await agent.account()).toEqual({ installed: false, signedIn: false, message: "Install Antigravity's CLI, then sign in here." })
    expect(agent.install).toBe("curl -fsSL https://antigravity.google/cli/install.sh | bash")
    await expect(agent.signIn()).rejects.toThrow("Install Antigravity's CLI first")
  })

  test("its version, the newest from Google's feed, and Update runs agy update", async () => {
    const agent = new StandInAntigravity("versions")
    expect(await agent.maintenance(true)).toMatchObject({ installed: "1.5.14", latest: "1.5.20", shown: "agy update", command: [agent.agy, "update"] })
  })

  test("an account signed in before agy keeps running on Google's earlier agent and asks to be signed in again; once it is, it's on agy", async () => {
    const agent = new StandInAntigravity("earlier")
    // Google's earlier agent, downloaded then, and its sign-in file.
    const earlier = join(root, "earlier", "antigravity", "1.3.0")
    mkdirSync(earlier, { recursive: true })
    writeFileSync(join(earlier, ".complete"), "{}")
    mkdirSync(join(agent.profile, "antigravity-acp"), { recursive: true })
    writeFileSync(join(agent.profile, "antigravity-acp", "acp_token.json"), JSON.stringify({ refresh_token: "earlier" }))
    expect(await agent.account()).toMatchObject({ installed: true, signedIn: true, message: "Sign in again to update Antigravity." })

    await signIn(agent, "good-ada")
    expect(await agent.account()).toEqual(expect.not.objectContaining({ message: expect.anything() }))
    expect((await agent.account()).signedIn).toBe(true)
    expect(existsSync(join(agent.profile, "antigravity-acp", "acp_token.json"))).toBe(false)
  })
})
