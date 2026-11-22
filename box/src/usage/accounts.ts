import { CodexCredentials, type KeychainRead } from "./codex-credentials"
// Read the credentials and log locations belonging to this box. Provider turns, install, login
// and token renewal are the agents' responsibility.
import { claudeToken, grokCredential, grokHome } from "../shared/agent-credentials"
import { join } from "node:path"
import { Accounts, providerID, type AgentAccount, type AgentKind } from "../agents/accounts"
import { ClaudePlan, credentialsIn } from "./claude-limits"
import { installedVersion } from "../agents/maintenance"
import { which } from "../agents/proc"
import { homeFolder } from "../shared/account"
import { chatGPTIdentity, fingerprint, LimitsProblem, xaiIdentity, type LimitsSource } from "./limits"
import { codexAccountNow, readCodexNative, readGrokNative, spendCodexResetNative } from "./native-usage"
import { codexProgram, grokProgram } from "../shared/agent-programs"
import { AntigravityPlan, type AntigravityFetch } from "./antigravity-plan"
import { CursorClient, cursorKey, cursorKeyUnreadable } from "./cursor-api"
import { readCursorPlan, readingOf } from "./cursor-plan"

export interface UsageAgent {
  providerID: string
  instance: AgentAccount
  limits?: LimitsSource
  conversationRoots?(): string[]
  profile?: string
  signedOut?(): void
  prepare?(fresh?: boolean): Promise<void>
  cursor?: CursorClient
  cursorKeyUnreadable?(): boolean
}

/** Cache source objects across reads so backoff and per-account history survive roster edits. */
export class UsageAccounts {
  private readonly sources = new Map<string, UsageAgent>()

  constructor(private readonly root: string, private readonly options: {
    claudeByToken?: boolean
    codexKeychain?: KeychainRead
    /** The agents' programs (a test's stand-ins); else each one's own, where its installer puts it. */
    programs?: { codex?: string; grok?: string }
    antigravity?: { fetch?: AntigravityFetch; bases: string[] }
  } = {}) {}

  all(): UsageAgent[] {
    const accounts = new Accounts(this.root)
    const current = accounts.all().map(record => {
      const provider = providerID(record.kind, record.id)
      let agent = this.sources.get(provider)
      if (!agent) {
        agent = this.source(record.kind, provider, { id: record.id, home: accounts.home(record) })
        this.sources.set(provider, agent)
      }
      agent.instance.label = record.label
      return agent
    })
    const present = new Set(current.map(agent => agent.providerID))
    for (const provider of this.sources.keys()) if (!present.has(provider)) this.sources.delete(provider)
    return current
  }

  source(kind: AgentKind, providerID: string, instance: AgentAccount): UsageAgent {
    const common = { providerID, instance }
    const home = homeFolder()
    const every = 10 * 60_000
    switch (kind) {
      case "claude": {
        const byToken = this.options.claudeByToken ?? (process.platform === "darwin")
        const token = () => claudeToken(join(this.root, "claude", "tokens", instance.id + ".json"))
        const standard = process.env.CLAUDE_CONFIG_DIR ?? join(home, ".claude")
        const consoleHome = join(this.root, "claude", "console")
        const configHome = () => instance.home ?? (byToken && !token() ? consoleHome : standard)
        const signIn = () => {
          const own = byToken ? token() : undefined
          return own ? { token: own } : !byToken ? credentialsIn(configHome()) : undefined
        }
        let version: { at: number; value?: string } | undefined
        // (Where Claude Code keeps who's signed in, `.claude.json`: its folder when given one, else your home.)
        const recordHome = instance.home ?? (byToken ? undefined : process.env.CLAUDE_CONFIG_DIR ?? home)
        const plan = new ClaudePlan({ byToken, home: recordHome, token, configHome, signIn, version: async () => {
          if (version && Date.now() - version.at < 3_600_000) return version.value
          const executable = which("claude", [join(home, ".local/bin"), "/opt/homebrew/bin", "/usr/local/bin"])
          const value = executable ? await installedVersion(executable).catch(() => undefined) : undefined
          version = { at: Date.now(), value }
          return value
        } })
        return { ...common, limits: plan.limits, conversationRoots: () => [...new Set([configHome(), ...(!instance.home ? [consoleHome, standard] : [])])].map(folder => join(folder, "projects")) }
      }
      case "codex": {
        const folder = instance.home ?? process.env.CODEX_HOME ?? join(home, ".codex")
        const credential = new CodexCredentials(folder, this.options.codexKeychain)
        const signIn = () => credential.current()
        const identity = () => { const auth = signIn(); return chatGPTIdentity(auth?.token, auth?.account) }
        // Asked of Codex itself, in this account's folder: it renews its own sign-in as it answers.
        const program = () => { const executable = this.options.programs?.codex ?? codexProgram(); return executable ? { executable, home: folder } : undefined }
        return { ...common, prepare: fresh => credential.prepare(fresh), conversationRoots: () => [join(folder, "sessions"), join(folder, "archived_sessions")], limits: {
          every, signIn: () => fingerprint(signIn()?.token), account: () => fingerprint(signIn()?.refresh), identity,
          read: async now => {
            if (!signIn()) return null
            const codex = program()
            if (!codex) throw new LimitsProblem("failed", "Codex isn't installed here.")
            return readCodexNative(codex, now)
          },
          spender: async () => {
            const codex = program()
            const expected = signIn()?.account
            const live = codex ? await codexAccountNow(codex) : undefined
            return live && expected ? { same: live === expected } : undefined
          },
          reset: async (attempt, now) => {
            const codex = program()
            return codex ? spendCodexResetNative(codex, attempt, now) : { outcome: "failed" }
          },
        } }
      }
      case "grok": {
        const folder = grokHome(instance)
        const signIn = () => grokCredential(folder)
        const program = () => { const executable = this.options.programs?.grok ?? grokProgram(); return executable ? { executable, home: folder } : undefined }
        return { ...common, conversationRoots: () => [join(folder, "sessions")], limits: {
          every, signIn: () => fingerprint(signIn()?.key),
          identity: () => { const auth = signIn(); return xaiIdentity(auth?.key, auth?.id_token) },
          // Asked of Grok itself (its ACP `_x.ai/billing`), which renews a lapsed sign-in as it answers.
          read: async now => {
            if (!signIn()) return null
            const grok = program()
            if (!grok) throw new LimitsProblem("failed", "Grok's CLI isn't installed here.")
            return readGrokNative(grok, now)
          },
        } }
      }
      case "cursor": {
        const folder = instance.home ?? join(this.root, "cursor")
        const unreadable = () => cursorKeyUnreadable(folder)
        const cursor = new CursorClient(() => cursorKey(folder, !instance.home), Date.now, unreadable)
        return { ...common, cursor, cursorKeyUnreadable: unreadable, limits: {
          every, identity: () => cursor.identity(), signIn: () => cursor.signIn(), account: () => cursor.signIn(),
          read: async () => { const { mapped, identity } = await readCursorPlan(cursor); return readingOf(mapped, identity) },
        } }
      }
      case "antigravity": {
        const profile = join(instance.home ?? join(this.root, "antigravity"), "profile")
        const plan = new AntigravityPlan(() => profile, () => join(this.root, "usage", "antigravity", instance.id), this.options.antigravity?.fetch, undefined, this.options.antigravity?.bases)
        return { ...common, profile, limits: plan.limits, signedOut: () => plan.signOut() }
      }
    }
  }
}
