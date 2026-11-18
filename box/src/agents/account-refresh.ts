// Keep the account, its model catalog and its rings in sync for every client.

import type { Account, Agent } from "./agent"
import { emit } from "./events"
import type { Limits } from "./limits"
import { signIns } from "./sign-ins"
import type { Model } from "./wire"

const MODEL_INTERVAL = 5 * 60_000

interface Snapshot {
  account?: Account
  identity?: string
  models?: { at: number; value: Model[] }
  listing?: Promise<Model[]>
  reading?: Promise<Account>
  rereading?: Promise<Account>
}

export class AccountRefresh {
  private snapshots = new WeakMap<Agent, Snapshot>()
  private nextRound = 0

  constructor(private readonly limits?: Pick<Limits, "invalidate" | "readAgain">, private readonly now = Date.now) {}

  private snapshot(agent: Agent): Snapshot {
    let found = this.snapshots.get(agent)
    if (!found) this.snapshots.set(agent, (found = {}))
    return found
  }

  read(agent: Agent, fresh = false): Promise<Account> {
    const snapshot = this.snapshot(agent)
    return snapshot.reading ??= this.check(agent, snapshot, fresh).finally(() => { snapshot.reading = undefined })
  }

  /** Coalesce notifications, but read again when one arrives during a check. */
  changed(agent: Agent): Promise<Account> {
    const snapshot = this.snapshot(agent)
    if (!snapshot.reading) return this.read(agent, true)
    return snapshot.rereading ??= snapshot.reading.catch(() => undefined)
      .then(() => this.read(agent, true)).finally(() => { snapshot.rereading = undefined })
  }

  private async check(agent: Agent, snapshot: Snapshot, fresh: boolean): Promise<Account> {
    const account = await agent.account(fresh)
    if (this.snapshot(agent) !== snapshot) return this.read(agent, fresh)
    if (account.unchecked && snapshot.account) return { ...snapshot.account, unchecked: true }
    const previous = snapshot.account
    const identity = account.identity ?? agent.limits?.identity?.()
    const switched = snapshot.identity !== undefined && snapshot.identity !== identity
    snapshot.account = account
    snapshot.identity = identity
    const changed = previous && !account.unchecked && (previous.signedIn !== account.signedIn || previous.label !== account.label || switched)
    if (changed) {
      this.invalidate(agent, snapshot)
      this.snapshots.set(agent, { account, identity })
    }
    if (!account.unchecked) signIns.record(agent.providerID, account)
    if (changed) void this.publish(agent, account)
    return account
  }

  /** The command finished with credentials. Even an identical label can be another account. */
  async signedIn(agent: Agent): Promise<void> {
    const previous = this.snapshot(agent)
    this.invalidate(agent, previous)
    // A read begun before the sign-in finished cannot publish an account or catalog for it.
    this.snapshots.delete(agent)
    const account = await this.read(agent, true)
    await this.publish(agent, account)
  }

  private invalidate(agent: Agent, snapshot: Snapshot) {
    this.limits?.invalidate(agent.providerID)
    snapshot.models = undefined
    snapshot.listing = undefined
    agent.forgetModels?.()
  }

  private async publish(agent: Agent, account: Account) {
    emit("integration.updated", { integrationID: agent.providerID, accountRefresh: true })
    if (account.signedIn) await Promise.allSettled([this.catalog(agent, this.snapshot(agent)), this.limits?.readAgain(agent.providerID)])
    else emit("model.updated", { accountRefresh: true })
  }

  async models(agent: Agent, hidden = false): Promise<Model[]> {
    const snapshot = this.snapshot(agent)
    const account = await this.read(agent)
    if (!account.signedIn) return []
    if (this.snapshot(agent) !== snapshot) return this.models(agent, hidden)
    const value = await this.catalog(agent, snapshot)
    if (this.snapshot(agent) !== snapshot) return this.models(agent, hidden)
    return hidden ? value : value.filter(model => !model.hidden)
  }

  private catalog(agent: Agent, snapshot: Snapshot): Promise<Model[]> {
    const cached = snapshot.models
    if (cached && this.now() - cached.at < MODEL_INTERVAL) return Promise.resolve(cached.value)
    const loading = snapshot.listing ??= agent.models({ hidden: true }).then(value => {
      if (snapshot.listing !== loading || this.snapshot(agent) !== snapshot) return value
      snapshot.models = { at: this.now(), value }
      if (JSON.stringify(cached?.value) !== JSON.stringify(value)) emit("model.updated", { accountRefresh: true })
      return value
    }).catch(error => {
      if (cached) {
        if (snapshot.listing === loading) snapshot.models = { ...cached, at: this.now() }
        return cached.value
      }
      throw error
    }).finally(() => { if (snapshot.listing === loading) snapshot.listing = undefined })
    return loading
  }

  catalogChanged(agents: Agent[]) {
    for (const agent of agents) this.snapshot(agent).models = undefined
  }

  async round(agents: Agent[]): Promise<void> {
    const now = this.now()
    if (now < this.nextRound) return
    this.nextRound = now + 30_000
    await Promise.allSettled(agents.map(async agent => {
      const account = await this.read(agent, true)
      if (account.signedIn && !account.unchecked) await this.catalog(agent, this.snapshot(agent))
    }))
  }
}
