// In-process account probes for Limits fixtures. A running box instead follows the
// account checks published by its agents bridge.

import type { Account } from "../../../src/agents/agent"
import type { UsageAgent } from "../../../src/usage/accounts"

type Probe = UsageAgent & { account(fresh?: boolean): Promise<Pick<Account, "signedIn">> }

/** The longest an agent is waited on to say how its account stands. */
const ASK_WITHIN = 20_000

export class SignIns {
  private said = new Map<string, boolean>()
  /** Each account's sign-in (`LimitsSource.signIn`) when its agent was last asked by a round. */
  private seen = new Map<string, string | undefined>()
  /** Accounts with no sign-in to read their limits with now: none listed. */
  private none = new Set<string>()

  /** `within`: the longest an agent is waited on to say (a test's shorter). */
  constructor(private readonly within = ASK_WITHIN) {}

  /** Told when an account's sign-in changed (signed in, or out), with the account. */
  changed?: (provider: string) => void

  /** Whether this account is signed in, as its agent last said; undefined until it has said. */
  signedIn(provider: string): boolean | undefined {
    return this.none.has(provider) ? false : this.said.get(provider)
  }

  /** Whether each of `agents` has said once, or has no sign-in to say of (until then, none counts as gone). */
  allSaid(agents: UsageAgent[]): boolean {
    return agents.every((agent) => this.signedIn(agent.providerID) !== undefined)
  }

  /** What an agent said of its account: told where that changed it. */
  record(provider: string, account: Pick<Account, "signedIn">, observed?: { signIn?: string }) {
    this.telling(provider, () => {
      this.said.set(provider, account.signedIn)
      if (observed) {
        this.seen.set(provider, observed.signIn)
        if (observed.signIn) this.none.delete(provider)
      }
    })
  }

  /** Does `change`, telling `changed` where it changed whether the account counts as signed in. */
  private telling(provider: string, change: () => void) {
    const before = this.signedIn(provider)
    change()
    if (before !== undefined && before !== this.signedIn(provider)) this.changed?.(provider)
  }

  /**
   * The limits' round: each agent whose sign-in (as its limits read it) changed since it last said,
   * or that hasn't said yet, is asked; one with no such sign-in now has none listed, unasked.
   */
  async round(agents: Probe[]): Promise<void> {
    await Promise.all(
      agents.map(async (agent) => {
        const provider = agent.providerID
        const source = agent.limits
        const signIn = source?.signIn?.()
        if (source?.signIn && signIn === undefined) return this.telling(provider, () => this.none.add(provider))
        if (this.said.has(provider) && !this.none.has(provider) && this.seen.get(provider) === signIn) return
        this.seen.set(provider, signIn)
        const account = await this.answer(agent, false)
        this.telling(provider, () => {
          this.none.delete(provider)
          if (account) this.said.set(provider, account.signedIn)
        })
      }),
    )
  }

  /** Asks one agent now (`fresh`: its own answer, not what it kept) and records it: signed in or out here. */
  async ask(agent: Probe, fresh = true): Promise<void> {
    const account = await this.answer(agent, fresh)
    if (account) this.record(agent.providerID, account)
  }

  /**
   * What an agent says of its account: none where its check failed or didn't answer in time, so
   * it stays as last said (a computer offline isn't signed out, and no round waits on one long).
   */
  private async answer(agent: Probe, fresh: boolean): Promise<Pick<Account, "signedIn"> | undefined> {
    let tooLate: () => void = () => {}
    const waited = new Promise<undefined>((resolve) => (tooLate = () => resolve(undefined)))
    const late = setTimeout(() => tooLate(), this.within)
    late.unref?.()
    return Promise.race([
      agent.account(fresh).then(
        (account) => account,
        () => undefined,
      ),
      waited,
    ]).finally(() => clearTimeout(late))
  }
}

/** Of `agents`, those whose accounts are signed in here as `record` has them: whose limits are listed. */
export function signedInAgents<Source extends UsageAgent>(agents: Source[], record: SignIns): Source[] {
  return agents.filter((agent) => record.signedIn(agent.providerID) === true)
}
