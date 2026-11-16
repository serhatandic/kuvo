// Which coding agents' accounts are signed in on this computer, as each agent last said (its
// `account()`): what Settings › Accounts shows (`api/integration`, server.ts), and so the only
// agents' accounts whose plan limits are listed (limits.ts), the rings. One record, written
// wherever an agent says it (Accounts asked, a sign-in or sign-out here, the limits' own rounds),
// read by both, so a ring is never for an account Accounts says isn't signed in.
//
// Platform: the same on macOS and Linux (each agent says its own sign-in either way).

import type { Account, Agent } from "./agent"

/** How often the limits' rounds ask every agent again (an account signed out outside Kuvo Code, in
 * Terminal): what a sign-in or sign-out here, or Accounts asked, says comes at once. */
export const SIGN_INS_AGAIN = 5 * 60_000

export class SignIns {
  private said = new Map<string, boolean>()
  private askedAt = -Infinity
  private asking?: Promise<void>

  constructor(private readonly now: () => number = Date.now) {}

  /** Told when an account's sign-in changed (signed in, or out), with the account. */
  changed?: (provider: string) => void

  /** Whether this account is signed in, as its agent last said; undefined until it has said. */
  signedIn(provider: string): boolean | undefined {
    return this.said.get(provider)
  }

  /** Whether each of `agents` has said once (until then, none counts as gone). */
  allSaid(agents: Agent[]): boolean {
    return agents.every((agent) => this.said.has(agent.providerID))
  }

  /** What an agent said of its account (one whose check failed: not signed in, as Accounts shows it). */
  record(provider: string, account: Pick<Account, "signedIn">) {
    const before = this.said.get(provider)
    this.said.set(provider, account.signedIn)
    if (before !== undefined && before !== account.signedIn) this.changed?.(provider)
  }

  /**
   * Asks every agent how its account stands (each as its own `account()` keeps it a while) and
   * records it, every `SIGN_INS_AGAIN`, or at once while one hasn't said yet: the limits' rounds.
   */
  round(agents: Agent[]): Promise<void> {
    if (this.allSaid(agents) && this.now() - this.askedAt < SIGN_INS_AGAIN) return this.asking ?? Promise.resolve()
    if (this.asking) return this.asking
    const asking: Promise<void> = Promise.all(agents.map((agent) => this.ask(agent, false)))
      .then(() => void (this.askedAt = this.now()))
      .finally(() => {
        if (this.asking === asking) this.asking = undefined
      })
    this.asking = asking
    return asking
  }

  /** Asks one agent now (`fresh`: its own answer, not what it kept) and records it: signed in or out here. */
  async ask(agent: Agent, fresh = true): Promise<void> {
    // (As `api/integration` takes it: a check that fails says not signed in.)
    const account = await agent.account(fresh).catch((): Pick<Account, "signedIn"> => ({ signedIn: false }))
    this.record(agent.providerID, account)
  }

  /** Forgets every account (a test's). */
  clear() {
    this.said.clear()
    this.askedAt = -Infinity
  }
}

/** This bridge's record. */
export const signIns = new SignIns()

/** Of `agents`, those whose accounts are signed in here as `record` has them: whose limits are listed. */
export function signedInAgents(agents: Agent[], record: SignIns = signIns): Agent[] {
  return agents.filter((agent) => record.signedIn(agent.providerID) === true)
}
