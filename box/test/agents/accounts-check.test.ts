// An agent's sign-in as the app reads it (`/api/integration`, `/api/model`) while its check can't
// answer: on a computer too busy to run `claude auth status` in time, the account and its models
// stay as the agent last said, and only the agent's own word that you're signed out is a sign-out.
// Claude Code and Grok are fake programs on the PATH; Codex's app-server and Cursor's SDK are stand-ins.

import "../safety"
import { afterAll, afterEach, beforeAll, describe, expect, setSystemTime, test } from "bun:test"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, dirname, join } from "node:path"
import { TookTooLong, type Agent } from "../../src/agents/agent"
import { ClaudeAgent } from "../../src/agents/agents/claude"
import { CodexAgent } from "../../src/agents/agents/codex"
import { CursorAgent } from "../../src/agents/agents/cursor"
import { GrokAgent } from "../../src/agents/agents/grok"
import { subscribe } from "../../src/agents/events"
import { SignIns } from "../usage/support/sign-ins"
import { handler } from "../../src/agents/server"
import type { ServerEvent } from "../../src/agents/wire"

let root: string
let bin: string
const path = process.env.PATH
// Each agent's program check (`maintenance`), put back after: every kind of agent this file makes
// that has one (Cursor's has none).
const checks = [ClaudeAgent, CodexAgent, GrokAgent].map((kind) => [kind.prototype, kind.prototype.maintenance] as const)

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), "kuvo-accounts-check-"))
  bin = join(root, "bin")
  mkdirSync(bin)
  // The fake programs first: they're the ones found.
  process.env.PATH = [bin, path].join(delimiter)
  // Reading an account also checks its program (its version, and npm's newest), waiting at most 2 s
  // and announcing an update once a later answer comes. That's not what these tests are about: on a
  // loaded machine (the fake's `--version` slow to start, a slow registry) it came after the read and
  // fell among the changes they count. Their agents check no program (as one that has none).
  for (const [prototype] of checks) (prototype as { maintenance?: unknown }).maintenance = undefined
})

afterAll(() => {
  process.env.PATH = path
  for (const [prototype, check] of checks) prototype.maintenance = check
  rmSync(root, { recursive: true, force: true })
})

afterEach(() => {
  delete process.env.CURSOR_API_KEY
  delete process.env.XAI_API_KEY
  setSystemTime()
})

/** A program on the PATH that answers as `mode`'s file says: what it prints, or killed as it runs (a check cut short). */
function program(name: string, answers: Record<string, string>): (mode: string) => void {
  const modeFile = join(root, `${name}.mode`)
  const cases = Object.entries(answers)
    .map(([mode, script]) => `  ${mode}) ${script} ;;`)
    .join("\n")
  const file = join(bin, name)
  writeFileSync(file, `#!/bin/sh\ncase "$(cat '${modeFile}')" in\n${cases}\n  killed) kill -9 $$ ;;\nesac\n`)
  chmodSync(file, 0o755)
  return (mode) => writeFileSync(modeFile, mode)
}

/** The bridge's own routes, as the app asks them, for these agents alone. */
function bridge(agents: Agent[]) {
  const answer = handler({ roster: { all: () => agents, find: (id: string) => agents.find((agent) => agent.providerID === id) }, password: "pw" } as any)
  const ask = async (route: string) => {
    const response = await answer(new Request(`http://box/api/${route}`, { headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } }))
    return ((await response.json()) as any).data
  }
  return {
    /** The account as Settings › Accounts shows it, asked anew. */
    account: (agent: Agent) => ask(`integration/${encodeURIComponent(agent.providerID)}`),
    /** The models the picker lists, of `agent` (its account as it stands, not asked anew). */
    models: async (agent: Agent) => ((await ask("model")) as any[]).filter((model) => model.providerID === agent.providerID).map((model) => model.id),
  }
}

/** What the app was told changed meanwhile (an `integration.updated` makes it read every account and model again). */
function told(): { events: ServerEvent[]; stop: () => void } {
  const events: ServerEvent[] = []
  const stop = subscribe((event) => {
    if (event.type === "integration.updated") events.push(event)
  })
  return { events, stop }
}

const context = () => {
  const home = mkdtempSync(join(root, "agent-"))
  return { home, temporary: home, deps: {} as any }
}

/** Codex's app-server, as the test says it answers: `account/read` and `model/list`, counted. */
function codexServer() {
  const server = {
    /** How it answers now: as signed in, as signed out, too late, or held until `release`. */
    answers: "in" as "in" | "out" | "late" | "held",
    reads: 0,
    release: () => {},
    /** Called once a held check is waiting. */
    holding: () => {},
    isClosed: false,
    request: async (method: string) => {
      if (method === "account/read") server.reads++
      if (server.answers === "late") throw new TookTooLong(`${method} took too long.`)
      if (server.answers === "held") await new Promise<void>((resolve) => ((server.release = resolve), server.holding()))
      if (method === "account/read") return { account: server.answers === "out" ? null : { type: "chatgpt", email: "ada@example.com", planType: "plus" } }
      if (method === "model/list") return { data: [{ id: "gpt-6", model: "gpt-6", displayName: "GPT-6", isDefault: true }] }
      throw new Error(`No ${method}.`)
    },
  }
  return server
}

function codex(server: { request: (method: string) => Promise<unknown> } | Error, ownHome?: string) {
  class TestCodex extends CodexAgent {
    override executable() {
      return "/usr/bin/true"
    }
    override async rpc(): Promise<any> {
      if (server instanceof Error) throw server
      return server
    }
  }
  return new TestCodex(context(), ownHome ? { id: "default", home: ownHome } : undefined)
}

describe("an agent's sign-in that can't be checked just now", () => {
  test("Claude Code: still signed in as you, with its models, until Claude Code says you're signed out", async () => {
    const claude = program("claude", {
      in: `echo '{"loggedIn":true,"authMethod":"claude.ai","email":"ada@example.com","subscriptionType":"max"}'`,
      out: `echo '{"loggedIn":false,"authMethod":"none"}'; exit 1`,
      garbled: `echo 'Error: something went wrong' >&2; exit 1`,
    })
    class TestClaude extends ClaudeAgent {
      // Claude Code's own list of your models, as it says once it starts.
      override async sdk(): Promise<any> {
        return {
          query: () => ({
            initializationResult: async () => ({ models: [{ value: "opus", displayName: "Opus", description: "Opus 5.5 · Most capable" }] }),
            setModel: async () => {
              throw new Error("Unknown control request")
            },
          }),
        }
      }
    }
    const agent = new TestClaude(context())
    const box = bridge([agent])

    claude("in")
    // (Someone's sign-in isn't marked as a thing it's signed in with.)
    expect((await box.account(agent)).connections[0]).toMatchObject({ label: "ada@example.com · Max plan" })
    expect((await box.account(agent)).connections[0]).not.toHaveProperty("what")
    expect(await box.models(agent)).not.toEqual([])

    // Killed as it checks, or saying nothing it can go by.
    const changes = told()
    for (const mode of ["killed", "garbled"]) {
      claude(mode)
      const account = await box.account(agent)
      expect(account.connections[0]?.label).toBe("ada@example.com · Max plan")
      expect(account.metadata.message).toBeUndefined()
      expect(account.metadata.unchecked).toBe(true)
      expect(await box.models(agent)).not.toEqual([])
    }
    changes.stop()
    expect(changes.events).toEqual([])

    // Claude Code's own word: signed out.
    claude("out")
    const account = await box.account(agent)
    expect(account.connections).toEqual([])
    expect(account.metadata.message).toBe("Sign in to Claude Code with your Claude plan or an Anthropic Console account.")
    expect(await box.models(agent)).toEqual([])
  })

  test("never answered yet: one that took too long is taken as signed in and says it couldn't check; one that fails says why", async () => {
    const late = codexServer()
    late.answers = "late"
    const slow = codex(late)
    const account = await bridge([slow]).account(slow)
    expect(account.connections[0]?.status).toEqual({ status: "connected", message: "Couldn't check the sign-in just now" })
    expect(account.metadata.unchecked).toBe(true)

    // Its answer once it comes only names the account: nothing the app has to read again.
    const changes = told()
    late.answers = "in"
    expect((await bridge([slow]).account(slow)).connections[0]?.label).toBe("ada@example.com · ChatGPT Plus")
    changes.stop()
    expect(changes.events).toEqual([])

    // Broken for good (its app-server can't start): its own words, as before, not "couldn't check" for ever.
    const broken = codex(new Error("codex: cannot execute binary file"))
    const said = await bridge([broken]).account(broken)
    expect(said.connections).toEqual([])
    expect(said.metadata.message).toBe("codex: cannot execute binary file")
    // Its program is there: no Install, which would run its maker's installer again.
    expect(said.metadata.installed).toBe(true)

    const claude = program("claude", { crashes: `echo 'Segmentation fault' >&2; exit 139` })
    claude("crashes")
    const crashing = new ClaudeAgent(context())
    const crashed = await bridge([crashing]).account(crashing)
    expect(crashed.metadata.message).toBe("Claude Code couldn't say whether it's signed in: Segmentation fault")
    expect(crashed.metadata.installed).toBe(true)
  })

  test("Grok: its models stay listed while `grok models` can't answer", async () => {
    const grok = program("grok", {
      in: `printf 'You are logged in with grok.com.\\n\\nAvailable models:\\n  * grok-4.7 (default)\\n  - grok-4.6\\n'`,
      out: `echo 'Error: not authenticated. Run grok login first.'; exit 1`,
      silent: `exit 1`,
    })
    const agent = new GrokAgent(context())
    const box = bridge([agent])

    grok("in")
    // (What it's signed in with, not who: marked so, for the box's "Signed in with grok.com".)
    expect((await box.account(agent)).connections[0]).toMatchObject({ label: "grok.com", what: true })
    expect(await box.models(agent)).toEqual(["grok-4.7", "grok-4.6"])

    for (const mode of ["killed", "silent"]) {
      grok(mode)
      expect((await box.account(agent)).connections[0]?.label).toBe("grok.com")
      expect(await box.models(agent)).toEqual(["grok-4.7", "grok-4.6"])
    }

    grok("out")
    expect((await box.account(agent)).connections).toEqual([])
    expect(await box.models(agent)).toEqual([])
  })

  test("Codex: an app-server that doesn't answer in time keeps the account and the models it listed; one that says you're signed out signs you out", async () => {
    const server = codexServer()
    const agent = codex(server)
    const box = bridge([agent])
    const start = Date.now()
    setSystemTime(start)

    expect((await box.account(agent)).connections[0]?.label).toBe("ada@example.com · ChatGPT Plus")
    expect(await box.models(agent)).toEqual(["gpt-6"])

    // Busy past the time its models are listed again.
    server.answers = "late"
    setSystemTime(start + 31 * 60_000)
    const account = await box.account(agent)
    expect(account.metadata.installed).toBe(true)
    expect(account.connections[0]?.label).toBe("ada@example.com · ChatGPT Plus")
    expect(await box.models(agent)).toEqual(["gpt-6"])

    server.answers = "out"
    const out = await box.account(agent)
    expect(out.connections).toEqual([])
    expect(out.metadata.message).toBe("Sign in to Codex with ChatGPT or an OpenAI API key.")
    expect(await box.models(agent)).toEqual([])
  })

  test("a busy agent is asked once at a time, and not again for 30 seconds after a check that failed", async () => {
    const server = codexServer()
    const agent = codex(server)
    const box = bridge([agent])
    const start = Date.now()
    setSystemTime(start)

    // Asked twice while the first check waits: one check, both answered by it.
    server.answers = "held"
    const held = new Promise<void>((resolve) => (server.holding = resolve))
    const both = Promise.all([box.account(agent), box.account(agent)])
    await held
    server.answers = "in"
    server.release()
    for (const account of await both) expect(account.connections[0]?.label).toBe("ada@example.com · ChatGPT Plus")
    expect(server.reads).toBe(1)
    expect(await box.models(agent)).toEqual(["gpt-6"])
    expect(server.reads).toBe(1)

    // Its answer stands a while; then a check that fails…
    server.answers = "late"
    setSystemTime(start + 31_000)
    expect(await box.models(agent)).toEqual(["gpt-6"])
    expect(server.reads).toBe(2)
    // …isn't tried again for 30 seconds by the lists the app reads,
    setSystemTime(start + 50_000)
    expect(await box.models(agent)).toEqual(["gpt-6"])
    expect(server.reads).toBe(2)
    // and is then.
    server.answers = "in"
    setSystemTime(start + 62_000)
    expect(await box.models(agent)).toEqual(["gpt-6"])
    expect(server.reads).toBe(3)
  })

  test("Cursor: out of reach, still signed in as you with your models; turned down by Cursor, or signed out of here, signed out", async () => {
    process.env.CURSOR_API_KEY = "key_test"
    let reachable = true
    let rejects = false
    class AuthenticationError extends Error {}
    const sdk = {
      AuthenticationError,
      Cursor: {
        me: async () => {
          if (rejects) throw new AuthenticationError("Unauthorized")
          if (!reachable) throw new Error("fetch failed")
          return { userEmail: "ada@example.com" }
        },
        models: {
          list: async () => {
            if (!reachable) throw new Error("fetch failed")
            return [{ id: "default" }, { id: "composer-2", displayName: "Composer 2" }]
          },
        },
      },
    }
    class TestCursor extends CursorAgent {
      override async sdk(): Promise<any> {
        return sdk
      }
    }
    const agent = new TestCursor(context())
    const box = bridge([agent])
    const start = Date.now()
    setSystemTime(start)

    expect((await box.account(agent)).connections[0]?.label).toBe("ada@example.com")
    expect(await box.models(agent)).toEqual(["default", "composer-2"])

    reachable = false
    setSystemTime(start + 31 * 60_000)
    expect((await box.account(agent)).connections[0]?.label).toBe("ada@example.com")
    expect(await box.models(agent)).toEqual(["default", "composer-2"])

    reachable = true
    rejects = true
    expect((await box.account(agent)).connections).toEqual([])

    // Signed out of here, with your key still set: asked anew, not as it last said.
    rejects = false
    expect((await box.account(agent)).connections[0]?.label).toBe("ada@example.com")
    // (It says your key stays: Kuvo Code's own sign-in is all it can forget.)
    await expect(agent.signOut()).rejects.toThrow("CURSOR_API_KEY")
    rejects = true
    expect((await box.models(agent))).toEqual([])
  })
})

describe("signing out of an agent's own sign-in", () => {
  test("your own Claude sign-in: Sign Out signs it out with Claude Code's own command, in your own folder (Terminal's too, as the app says), or says why it couldn't", async () => {
    const said = join(root, "claude-logout")
    const signedIn = `echo '{"loggedIn":true,"authMethod":"claude.ai","email":"ada@example.com"}'`
    const claude = program("claude", {
      in: `if [ "$2" = logout ]; then echo "dir:$CLAUDE_CONFIG_DIR" > '${said}'; echo out > '${join(root, "claude.mode")}'; exit 0; fi; ${signedIn}`,
      locked: `if [ "$2" = logout ]; then echo 'The Keychain is locked.' >&2; exit 1; fi; ${signedIn}`,
      out: `echo '{"loggedIn":false,"authMethod":"none"}'; exit 1`,
    })
    const signOut = async (agent: Agent) => {
      const answer = handler({ roster: { all: () => [agent], find: (id: string) => (id === agent.providerID ? agent : undefined) }, password: "pw" } as any)
      const response = await answer(new Request(`http://box/api/integration/${encodeURIComponent(agent.providerID)}/disconnect`, { method: "POST", headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } }))
      return { status: response.status, said: ((await response.json()) as any)?.message as string | undefined }
    }

    claude("in")
    const yours = new ClaudeAgent(context())
    const box = bridge([yours])
    const account = await box.account(yours)
    expect(account.connections[0]?.label).toBe("ada@example.com")
    expect(account.metadata).toMatchObject({ signsOut: true, signOutShared: true })

    claude("locked")
    const refused = await signOut(yours)
    expect(refused.status).not.toBe(200)
    expect(refused.said).toContain("The Keychain is locked.")
    expect((await box.account(yours)).connections[0]?.label).toBe("ada@example.com")

    claude("in")
    expect((await signOut(yours)).status).toBe(200)
    // Claude Code's own folder: no other one named.
    expect(readFileSync(said, "utf8").trim()).toBe("dir:")
    expect((await box.account(yours)).connections).toEqual([])
  })
})

describe("Sign Out for every agent's own sign-in, Remove for one you added", () => {
  const signIns = new SignIns()
  const post = (agents: Agent[], route: string, method = "POST") => {
    const roster = { all: () => agents, find: (id: string) => agents.find((agent) => agent.providerID === id), accounts: { remove: () => {} }, sync: () => {} }
    const answer = handler({
      roster, sessions: { forgetProvider: () => {} }, password: "pw",
      boxServices: { observe: (provider: string, change: { signedIn: boolean }) => signIns.record(provider, change) },
    } as any)
    return answer(new Request(`http://box/api/${route}`, { method, headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } }))
  }
  // (Your shell's own homes for them aren't these tests'.)
  beforeAll(() => {
    delete process.env.GROK_HOME
    delete process.env.CODEX_HOME
  })
  /** A fake program that says it's signed in until its `logout`, which writes what it was run with. */
  function fake(name: string, status: { in: string; out: string }, env: string) {
    const folder = mkdtempSync(join(root, `${name}-`))
    const file = join(folder, name)
    const ran = join(folder, "logout")
    writeFileSync(file, `#!/bin/sh\nfor a in "$@"; do [ "$a" = logout ] && { echo "${env}=\${${env}:-}" > '${ran}'; exit 0; }; done\n[ -f '${ran}' ] && { ${status.out}; }\n${status.in}\n`)
    chmodSync(file, 0o755)
    return { file, ran }
  }

  test("Grok: on a Mac its own sign-in is yours, so Sign Out signs out Grok in Terminal too (and says so); on a machine, Kuvo Code's own there; the ring goes at once", async () => {
    const program = fake("grok", { in: `printf 'You are logged in with grok.com.\\n\\nAvailable models:\\n  * grok-4.7 (default)\\n'`, out: `echo 'Error: not authenticated. Run grok login first.'; exit 1` }, "GROK_HOME")
    class TestGrok extends GrokAgent {
      override executable() {
        return program.file
      }
    }
    const yours = new TestGrok(context())
    expect((await bridge([yours]).account(yours)).metadata).toMatchObject({ signsOut: true, signOutShared: true })
    signIns.record(yours.providerID, { signedIn: true })
    expect((await post([yours], `integration/${yours.providerID}/disconnect`)).status).toBe(200)
    // Its own `grok logout`, in your own GROK_HOME (none set: `~/.grok`).
    expect(readFileSync(program.ran, "utf8").trim()).toBe("GROK_HOME=")
    expect(signIns.signedIn(yours.providerID)).toBe(false)
    expect((await bridge([yours]).account(yours)).connections).toEqual([])

    rmSync(program.ran)
    const home = mkdtempSync(join(root, "machine-grok-"))
    const machine = new TestGrok(context(), { id: "default", home })
    expect((await bridge([machine]).account(machine)).metadata).toMatchObject({ signsOut: true, signOutShared: false })
    expect((await post([machine], `integration/${machine.providerID}/disconnect`)).status).toBe(200)
    expect(readFileSync(program.ran, "utf8").trim()).toBe(`GROK_HOME=${home}`)

    // Signed in with XAI_API_KEY: what Grok cached goes, and it says the key keeps it signed in.
    rmSync(program.ran)
    process.env.XAI_API_KEY = "xai-test"
    const refused = await post([yours], `integration/${yours.providerID}/disconnect`)
    expect(refused.status).not.toBe(200)
    expect(((await refused.json()) as any).message).toContain("XAI_API_KEY")
    expect(existsSync(program.ran)).toBe(true)
  })

  test("Codex: Sign Out runs its own `codex logout`, in your CODEX_HOME on a Mac (shared with Terminal) and Kuvo Code's on a machine", async () => {
    const program = fake("codex", { in: "true", out: "true" }, "CODEX_HOME")
    const server = codexServer()
    class TestCodex extends CodexAgent {
      override executable() {
        return program.file
      }
      override async rpc(): Promise<any> {
        return server
      }
    }
    const yours = new TestCodex(context())
    // (Its app-servers stop as it signs out, so its confirm says a running turn stops.)
    expect((await bridge([yours]).account(yours)).metadata).toMatchObject({ signsOut: true, signOutShared: true, signOutStops: true })
    signIns.record(yours.providerID, { signedIn: true })
    server.answers = "out"
    expect((await post([yours], `integration/${yours.providerID}/disconnect`)).status).toBe(200)
    expect(readFileSync(program.ran, "utf8").trim()).toBe("CODEX_HOME=")
    expect(signIns.signedIn(yours.providerID)).toBe(false)
    server.answers = "in"

    const home = mkdtempSync(join(root, "machine-codex-"))
    const machine = new TestCodex(context(), { id: "default", home })
    expect((await bridge([machine]).account(machine)).metadata).toMatchObject({ signsOut: true, signOutShared: false })
    expect((await post([machine], `integration/${machine.providerID}/disconnect`)).status).toBe(200)
    expect(readFileSync(program.ran, "utf8").trim()).toBe(`CODEX_HOME=${home}`)
  })

  test("Cursor: Sign Out forgets Kuvo Code's own sign-in; a key from CURSOR_API_KEY isn't its to forget, and it says so", async () => {
    const agent = new CursorAgent(context())
    const auth = join(agent.context.home, "cursor", "auth.json")
    mkdirSync(dirname(auth), { recursive: true })
    writeFileSync(auth, "{}")
    await agent.signOut()
    expect(existsSync(auth)).toBe(false)

    process.env.CURSOR_API_KEY = "key_test"
    const refused = await post([agent], `integration/${agent.providerID}/disconnect`)
    expect(refused.status).not.toBe(200)
    expect(((await refused.json()) as any).message).toContain("CURSOR_API_KEY")
  })

  test("an added account isn't signed out but removed: signed out and forgotten, its ring gone at once", async () => {
    const added = new GrokAgent(context(), { id: "1f2e3d4c", home: mkdtempSync(join(root, "added-grok-")) })
    // Removed instead (its own folder goes with it): no Sign Out of its own.
    expect(added.signOut).toBeUndefined()
    signIns.record(added.providerID, { signedIn: true })
    expect((await post([added], `kuvo/accounts/${encodeURIComponent(added.providerID)}`, "DELETE")).status).toBe(200)
    expect(signIns.signedIn(added.providerID)).toBe(false)
  })
})

describe("Claude Code's own sign-in, the same on a Mac and on Linux", () => {
  /** A Claude Code that keeps a sign-in per folder (CLAUDE_CONFIG_DIR, else your `~/.claude`), as
   * `auth login` leaves it, and notes any token it was handed. */
  function folderClaude() {
    const tokens = join(root, "claude-tokens-seen")
    writeFileSync(join(bin, "claude"), `#!/bin/sh
dir="\${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
[ -n "$CLAUDE_CODE_OAUTH_TOKEN" ] && echo "$CLAUDE_CODE_OAUTH_TOKEN" >> '${tokens}'
case "$1 $2" in
  "auth login") mkdir -p "$dir"; echo "$3" > "$dir/signed-in" ;;
  "auth logout") rm -f "$dir/signed-in" ;;
  "auth status")
    [ -f "$dir/signed-in" ] || { echo '{"loggedIn":false,"authMethod":"none"}'; exit 1; }
    grep -q console "$dir/signed-in" && echo '{"loggedIn":true,"authMethod":"console","email":"ops@example.com"}' || echo '{"loggedIn":true,"authMethod":"claude.ai","email":"ada@example.com","subscriptionType":"max"}' ;;
esac
`)
    chmodSync(join(bin, "claude"), 0o755)
    return { yours: join(process.env.HOME!, ".claude"), tokens }
  }

  test("your first account signs in with Claude Code's own sign-in, where Terminal's is; an added one in a folder of its own; neither is handed a token", async () => {
    const { yours, tokens } = folderClaude()
    rmSync(yours, { recursive: true, force: true })
    try {
      const first = new ClaudeAgent(context())
      expect((await bridge([first]).account(first)).connections).toEqual([])
      const sign = await first.signIn("claudeai", {})
      for (let waited = 0; waited < 10_000 && sign.status().status === "pending"; waited += 50) await Bun.sleep(50)
      expect(sign.status().status).toBe("complete")
      expect(readFileSync(join(yours, "signed-in"), "utf8").trim()).toBe("--claudeai")
      const account = await bridge([first]).account(first)
      expect(account.connections[0]?.label).toBe("ada@example.com · Max plan")
      // Signing out of it signs Terminal out too, and the app says so.
      expect(account.metadata).toMatchObject({ signsOut: true, signOutShared: true })

      const home = join(root, "claude-added-folder")
      const added = new ClaudeAgent(context(), { id: "work", home })
      const other = await added.signIn("console", {})
      for (let waited = 0; waited < 10_000 && other.status().status === "pending"; waited += 50) await Bun.sleep(50)
      expect(other.status().status).toBe("complete")
      expect(readFileSync(join(home, "signed-in"), "utf8").trim()).toBe("--console")
      expect((await bridge([added]).account(added)).connections[0]?.label).toBe("ops@example.com")
      // Yours is as it was.
      expect(readFileSync(join(yours, "signed-in"), "utf8").trim()).toBe("--claudeai")
      expect(existsSync(tokens)).toBe(false)
    } finally {
      rmSync(yours, { recursive: true, force: true })
    }
  })

  test("a token the box was started with never reaches Claude Code", async () => {
    const said = join(root, "claude-inherited")
    writeFileSync(join(bin, "claude"), `#!/bin/sh
echo "\${CLAUDE_CODE_OAUTH_TOKEN:-none}" >> '${said}'
echo '{"loggedIn":true,"authMethod":"claude.ai","email":"ada@example.com"}'
`)
    chmodSync(join(bin, "claude"), 0o755)
    const before = process.env.CLAUDE_CODE_OAUTH_TOKEN
    process.env.CLAUDE_CODE_OAUTH_TOKEN = "sk-ant-oat01-yours"
    try {
      await new ClaudeAgent(context()).account(true)
      expect(readFileSync(said, "utf8")).not.toContain("sk-ant")
    } finally {
      if (before === undefined) delete process.env.CLAUDE_CODE_OAUTH_TOKEN
      else process.env.CLAUDE_CODE_OAUTH_TOKEN = before
    }
  })
})

test("the picker's models leave out those an agent keeps from its own; the box asks for them too, marked, to say they still run", async () => {
  const listed = (hidden: boolean) => [
    { id: "gpt-5.5", providerID: "codex-cli" },
    ...(hidden ? [{ id: "gpt-5.1-codex-mini", providerID: "codex-cli", hidden: true }] : []),
  ]
  const agent = { providerID: "codex-cli", account: async () => ({ installed: true, signedIn: true }), models: async (options?: { hidden?: boolean }) => listed(options?.hidden === true) } as unknown as Agent
  const answer = handler({ roster: { all: () => [agent], find: () => agent }, password: "pw" } as any)
  const ask = async (route: string) => ((await (await answer(new Request(`http://box/api/${route}`, { headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } }))).json()) as any).data
  expect((await ask("model")).map((model: any) => model.id)).toEqual(["gpt-5.5"])
  expect(await ask("model?hidden=1")).toEqual(listed(true))
})

test("an account says whether its list is every model it runs: not while its agent can't say so (Claude Code's usual ones, a Codex that lists none it hides)", async () => {
  const of = (listsEveryModel?: () => boolean) =>
    ({ providerID: "codex-cli", id: "codex", name: "Codex", instance: { id: "default" }, methods: () => [], account: async () => ({ installed: true, signedIn: true }), models: async () => [], ...(listsEveryModel ? { listsEveryModel } : {}) }) as unknown as Agent
  const metadata = async (agent: Agent) => {
    const answer = handler({ roster: { all: () => [agent], find: () => agent }, password: "pw" } as any)
    const response = await answer(new Request("http://box/api/integration/codex-cli", { headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } }))
    return ((await response.json()) as any).data.metadata
  }
  expect((await metadata(of(() => false))).modelsComplete).toBe(false)
  expect((await metadata(of(() => true))).modelsComplete).toBe(true)
  expect((await metadata(of())).modelsComplete).toBe(true)
})

test("Codex Sign In Again refreshes the provider catalog even when the account stays the same", async () => {
  let released = "gpt-old"
  const calls: [string, unknown][] = []
  const agent = codex({
    async request(method: string, params?: unknown) {
      calls.push([method, params])
      if (method === "account/login/start") return { loginId: "device-login", verificationUrl: "https://auth.openai.com/codex/device", userCode: "ABCD-1234" }
      if (method === "account/read") return { account: { type: "chatgpt", email: "new@example.com", planType: "pro" } }
      if (method === "model/list") {
        if (!existsSync(cache)) writeFileSync(cache, released)
        return { data: [{ id: readFileSync(cache, "utf8") }] }
      }
      throw new Error(method)
    },
  }, mkdtempSync(join(root, "codex-login-")))
  const cache = join(agent.home, "models_cache.json")
  const answer = handler({ roster: { all: () => [agent], find: () => agent }, password: "pw" } as any)
  const ask = (path: string, method = "GET", body?: unknown) => answer(new Request(`http://box/api/integration/codex-cli${path}`, {
    method, headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  })).then(response => response.json()) as Promise<any>
  const models = () => answer(new Request("http://box/api/model", { headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } })).then(response => response.json()) as Promise<any>
  expect((await models()).data.map((model: any) => model.id)).toEqual(["gpt-old"])
  released = "gpt-new"
  const started = (await ask("/connect/oauth", "POST", { methodID: "chatgpt", remote: true })).data
  expect(started).toMatchObject({ url: "https://auth.openai.com/codex/device", userCode: "ABCD-1234" })
  expect(calls).toContainEqual(["account/login/start", { type: "chatgptDeviceCode" }])
  const status = () => ask(`/connect/oauth/${started.attemptID}`).then(answer => answer.data.status)
  expect(await status()).toBe("pending")
  ;(agent as any).notification("account/login/completed", { loginId: "device-login", success: true })
  expect(await status()).toBe("complete")
  expect((await ask("")).data.connections[0].label).toBe("new@example.com · ChatGPT Pro")
  expect((await models()).data.map((model: any) => model.id)).toEqual(["gpt-new"])
  expect(calls.map(([method]) => method)).not.toContain("account/logout")
})

test("Codex uses browser auth for a remote client that can relay the callback", async () => {
  const calls: [string, unknown][] = []
  const agent = codex({
    async request(method: string, params?: unknown) {
      calls.push([method, params])
      if (method === "account/login/start") return { loginId: "browser-login", authUrl: "https://example.test/browser" }
      throw new Error(method)
    },
  }, mkdtempSync(join(root, "codex-browser-login-")))
  const answer = handler({ roster: { all: () => [agent], find: () => agent }, password: "pw" } as any)
  const response = await answer(new Request("http://box/api/integration/codex-cli/connect/oauth", {
    method: "POST", headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json" },
    body: JSON.stringify({ methodID: "chatgpt", remote: true, browserReturn: true }),
  }))
  expect(response.status).toBe(200)
  const { data } = await response.json() as any
  expect(data.url).toBe("https://example.test/browser")
  expect(data.userCode).toBeUndefined()
  expect(calls).toContainEqual(["account/login/start", { type: "chatgpt" }])
  ;(agent as any).notification("account/login/completed", { loginId: "browser-login", success: true })
})

test("Work Queue GitHub sign-in waits for the browser’s device code authorization", async () => {
  const { GitHubSignIn } = await import("../../src/link/work-queue/github-sign-in")
  const approval = join(root, "github-approved")
  const gh = join(bin, "gh")
  writeFileSync(gh, `#!/bin/sh
case "$2" in
  status) test -f '${approval}' ;;
  login)
    printf 'First copy your one-time code: ABCD-1234\\nOpen this URL to continue in your web browser: https://github.com/login/device\\n' >&2
    while [ ! -f '${approval}' ]; do sleep 0.05; done
    ;;
  *) exit 1 ;;
esac
`, { mode: 0o700 })
  const github = new GitHubSignIn(context())
  let attempt: Awaited<ReturnType<typeof github.begin>> | undefined
  let started: ReturnType<typeof github.begin> | undefined
  try {
    expect((await github.integration())?.connections).toEqual([])
    started = github.begin("first computer")
    expect(() => github.begin("second computer")).toThrow("already open")
    attempt = await started
    expect(attempt).toMatchObject({ url: "https://github.com/login/device", userCode: "ABCD-1234" })
    expect(github.status(attempt.attemptID, "first computer").status).toBe("pending")
    expect(() => github.begin("second computer")).toThrow("already open")
    expect(github.status(attempt.attemptID, "second computer").status).toBe("expired")
    github.cancel(attempt.attemptID, "second computer")
    expect(github.status(attempt.attemptID, "first computer").status).toBe("pending")
    writeFileSync(approval, "")
    for (let retries = 0; retries < 40 && github.status(attempt.attemptID, "first computer").status === "pending"; retries++) await Bun.sleep(25)
    expect(github.status(attempt.attemptID, "first computer").status).toBe("complete")
    expect((await github.integration())?.connections.length).toBe(1)
  } finally {
    attempt ??= await started?.catch(() => undefined)
    if (attempt) github.cancel(attempt.attemptID)
    rmSync(gh, { force: true })
  }
})

test("Antigravity probe cleanup is confined to its own session folders", async () => {
  const { AntigravityAgent } = await import("../../src/agents/agents/antigravity")
  const agent = new AntigravityAgent(context(), { id: "cleanup", home: join(root, "agy-cleanup") })
  const acp = join(agent.profile, "antigravity-acp")
  const brain = join(acp, "brain")
  mkdirSync(join(brain, "valid-session"), { recursive: true })
  const protectedFolder = join(acp, "keep")
  mkdirSync(protectedFolder)
  writeFileSync(join(protectedFolder, "protected"), "still here")
  const cleanup = (id: string) => (agent as unknown as { removeSessionFiles(id: string): void }).removeSessionFiles(id)
  cleanup("../keep")
  expect(existsSync(join(protectedFolder, "protected"))).toBe(true)
  cleanup("valid-session")
  expect(existsSync(join(brain, "valid-session"))).toBe(false)
  rmSync(brain, { recursive: true })
  mkdirSync(join(protectedFolder, "valid-session"))
  symlinkSync(protectedFolder, brain)
  cleanup("valid-session")
  expect(existsSync(join(protectedFolder, "valid-session"))).toBe(true)
})

test("Antigravity reports a missing catalog after discovery and accepts an explicit empty list", async () => {
  const { AntigravityAgent } = await import("../../src/agents/agents/antigravity")
  const { AccountRefresh } = await import("../../src/agents/account-refresh")
  const agent = new AntigravityAgent(context(), { id: "discovery", home: join(root, "agy-discovery") })
  mkdirSync(join(agent.profile, "antigravity-acp"), { recursive: true })
  writeFileSync(join(agent.profile, "antigravity-acp", "acp_token.json"), JSON.stringify({ refresh_token: "discovery-test" }))
  const adapter = agent as unknown as { folder: string; modelConnection: () => unknown }
  mkdirSync(adapter.folder, { recursive: true })
  writeFileSync(join(adapter.folder, ".complete"), "")
  let reply: unknown = { models: { availableModels: [{ modelId: "old", name: "Old" }] } }
  adapter.modelConnection = () => ({
    initialize: async () => {},
    rpc: { kill() {}, request: async (method: string) => method === "session/new" ? reply : {} },
    cwd: root, launch: { signInFile: join(root, "not-requested") },
  })
  const accounts = new AccountRefresh()
  expect((await accounts.modelStatus(agent, false, true)).models.map(model => model.id)).toEqual(["old"])
  rmSync(join(agent.profile, "kuvo-models.json"))
  reply = {}
  const failed = await accounts.modelStatus(agent, false, true)
  expect(failed.unavailable).toBe(true)
  expect(failed.models.map(model => model.id)).toEqual(["old"])
  reply = { models: { availableModels: [] } }
  expect(await accounts.modelStatus(agent, false, true)).toMatchObject({ models: [], unavailable: false })
})

test("Antigravity cannot attach the previous account's disk catalog to a new sign-in", async () => {
  const { AntigravityAgent } = await import("../../src/agents/agents/antigravity")
  const { AccountRefresh } = await import("../../src/agents/account-refresh")
  const instance = { id: "catalog-switch", home: join(root, "agy-catalog-switch") }
  const agent = new AntigravityAgent(context(), instance)
  const token = join(agent.profile, "antigravity-acp", "acp_token.json")
  mkdirSync(join(agent.profile, "antigravity-acp"), { recursive: true })
  let signingIn = "account-A"
  let setup: unknown = { models: { availableModels: [{ modelId: "only-A", name: "Only A" }] } }
  const configure = (agent: InstanceType<typeof AntigravityAgent>) => {
    const adapter = agent as unknown as { folder: string; modelConnection: () => unknown }
    mkdirSync(adapter.folder, { recursive: true })
    writeFileSync(join(adapter.folder, ".complete"), "")
    adapter.modelConnection = () => ({
      initialize: async () => {},
      rpc: { kill() {}, request: async (method: string) => {
        if (method === "authenticate") writeFileSync(token, JSON.stringify({ refresh_token: signingIn }))
        return method === "session/new" ? setup : {}
      } },
      cwd: root, launch: { env: {}, signInFile: join(root, "not-requested") },
    })
  }
  configure(agent)
  const signIn = async () => {
    const attempt = await agent.signIn()
    await new Promise<void>(resolve => setImmediate(resolve))
    expect(attempt.status().status).toBe("complete")
  }
  await signIn()
  const first = new AccountRefresh()
  const before = await first.modelStatus(agent, false, false)
  expect(before.models.map(model => model.id)).toEqual(["only-A"])
  // A fresh bridge can still use a catalog with verified continuity.
  const same = new AntigravityAgent(context(), instance)
  expect((await new AccountRefresh().modelStatus(same, false, false)).models.map(model => model.id)).toEqual(["only-A"])
  signingIn = "account-B"
  setup = {}
  await signIn()
  const restarted = new AntigravityAgent(context(), instance)
  configure(restarted)
  const accounts = new AccountRefresh()
  expect(await accounts.modelStatus(restarted, false, false)).toMatchObject({ models: [], unavailable: true })
  expect(await accounts.modelStatus(restarted, false, true)).toMatchObject({ models: [], unavailable: true })
  expect(await first.modelStatus(agent, false, false)).toMatchObject({ models: [], unavailable: true })
})
