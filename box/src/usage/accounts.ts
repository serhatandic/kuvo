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
import { chatGPTIdentity, fingerprint, LimitsProblem, xaiIdentity, type LimitExtra, type LimitsSource } from "./limits"
import { readCodexPlan, readGrok } from "./engine-limits"
import { readCodexResets, spendCodexReset, type ChatGPTSignIn } from "./codex-resets"
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
  prepare?(): Promise<void>
  cursor?: CursorClient
  cursorKeyUnreadable?(): boolean
}

/** Cache source objects across reads so backoff and per-account history survive roster edits. */
export class UsageAccounts {
  private readonly sources = new Map<string, UsageAgent>()

  constructor(private readonly root: string, private readonly managedDefaults: boolean, private readonly options: {
    claudeByToken?: boolean
    codexKeychain?: KeychainRead
    antigravity?: { fetch?: AntigravityFetch; bases: string[] }
  } = {}) {}

  all(): UsageAgent[] {
    const accounts = new Accounts(this.root, this.managedDefaults)
    const current = accounts.all().map(record => {
      const provider = providerID(record.kind, record.id)
      let agent = this.sources.get(provider)
      if (!agent) {
        agent = this.source(record.kind, provider, { id: record.id, home: accounts.home(record, false) })
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
          return own ? { token: own } : !byToken && instance.home ? credentialsIn(instance.home) : undefined
        }
        let version: { at: number; value?: string } | undefined
        const plan = new ClaudePlan({ byToken, home: instance.home, token, configHome, signIn, version: async () => {
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
        const identityOf = (auth: ChatGPTSignIn | undefined) => chatGPTIdentity(auth?.token, auth?.account)
        let kept: { identity: string; resets: LimitExtra } | undefined
        return { ...common, prepare: () => credential.prepare(), conversationRoots: () => [join(folder, "sessions"), join(folder, "archived_sessions")], limits: {
          every, signIn: () => fingerprint(signIn()?.token), account: () => fingerprint(signIn()?.refresh),
          identity: () => identityOf(signIn()),
          read: async now => {
            const auth = signIn()
            if (!auth) return null
            const identity = identityOf(auth) ?? fingerprint(auth.token)!
            return readCodexPlan(auth, now, async () => {
              const resets = await readCodexResets(auth, now)
              if (resets) kept = { identity, resets }
              return kept?.identity === identity ? kept.resets : undefined
            })
          },
          spender: async () => {
            const expected = identityOf(signIn())
            const checked = await credential.forReset()
            const live = identityOf(checked)
            return live && expected ? { same: live === expected, signIn: checked } : undefined
          },
          reset: (attempt, now) => spendCodexReset(attempt.signIn as ChatGPTSignIn | undefined, attempt, now),
        } }
      }
      case "grok": {
        const folder = grokHome(instance)
        const signIn = () => grokCredential(folder)
        return { ...common, conversationRoots: () => [join(folder, "sessions")], limits: {
          every, signIn: () => fingerprint(signIn()?.key),
          identity: () => { const auth = signIn(); return xaiIdentity(auth?.key, auth?.id_token) },
          read: async now => {
            const auth = signIn()
            if (!auth) return null
            if (Date.parse(auth.expires_at ?? auth.expiresAt ?? auth.expires ?? "") <= now)
              throw new LimitsProblem("expired", "Its sign-in has lapsed: Grok renews it the next time it runs here.")
            return readGrok(auth.key.trim(), undefined, now)
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
        const plan = new AntigravityPlan(() => profile, () => join(this.root, "usage", "antigravity", instance.id), this.options.antigravity?.fetch, async () => [], this.options.antigravity?.bases)
        return { ...common, profile, limits: plan.limits, signedOut: () => plan.signOut() }
      }
    }
  }
}
