// Keep the account, its model catalog and its rings in sync for every client.

import { randomUUID } from "node:crypto"
import type { Account, Agent } from "./agent"
import { emit } from "./events"
import type { Limits } from "./limits"
import { signIns } from "./sign-ins"
import type { Model } from "./wire"

interface Snapshot {
  account?: Account
  identity?: string
  models?: Model[]
  modelFailed?: boolean
  revision: number
  listedRevision?: number
  generation: string
  listing?: { refresh: boolean; pending: Promise<Model[]> }
  reading?: Promise<Account>
  rereading?: Promise<Account>
}

export class AccountRefresh {
  private snapshots = new WeakMap<Agent, Snapshot>()
  private nextRound = 0
  private nextOpen = 0

  constructor(private readonly limits?: Pick<Limits, "invalidate" | "listenToTurns" | "readAgain">, private readonly now = Date.now) {}

  private snapshot(agent: Agent): Snapshot {
    let found = this.snapshots.get(agent)
    if (!found) this.snapshots.set(agent, (found = { generation: randomUUID(), revision: 0 }))
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
      this.snapshots.set(agent, { account, identity, generation: randomUUID(), revision: 0 })
    }
    if (!account.unchecked) signIns.record(agent.providerID, account)
    if (changed) void this.publish(agent, account)
    return account
  }

  /** The command finished with credentials. Even an identical label can be another account. */
  async signedIn(agent: Agent, hasPlanToken = false): Promise<void> {
    if (!hasPlanToken) this.forgetSignIn(agent)
    const account = await this.read(agent, true)
    await this.publish(agent, account)
  }

  /** A Claude turn can use this token before the asynchronous account or catalog check finishes. */
  planTokenStored(agent: Agent) {
    this.forgetSignIn(agent)
    if (agent.limits) this.limits?.listenToTurns(agent.providerID, agent.limits)
    signIns.planTokenStored(agent.providerID)
  }

  private forgetSignIn(agent: Agent) {
    const previous = this.snapshot(agent)
    this.invalidate(agent, previous)
    // A read begun before the sign-in finished cannot publish an account or catalog for it.
    this.snapshots.set(agent, { generation: randomUUID(), revision: 0 })
  }

  private invalidate(agent: Agent, snapshot: Snapshot) {
    this.limits?.invalidate(agent.providerID)
    snapshot.models = undefined
    snapshot.listing = undefined
    agent.forgetModels?.()
  }

  private async publish(agent: Agent, account: Account) {
    emit("integration.updated", { integrationID: agent.providerID, accountRefresh: true })
    if (account.signedIn) await Promise.allSettled([this.catalog(agent, this.snapshot(agent), true), this.limits?.readAgain(agent.providerID)])
    else emit("model.updated", { accountRefresh: true })
  }

  async models(agent: Agent, hidden = false, refresh = false, picker = false): Promise<Model[]> {
    const snapshot = this.snapshot(agent)
    const account = await this.read(agent)
    if (!account.signedIn) return []
    if (this.snapshot(agent) !== snapshot) return this.models(agent, hidden, refresh, picker)
    const value = await this.catalog(agent, snapshot, refresh, picker)
    if (this.snapshot(agent) !== snapshot) return this.models(agent, hidden, refresh, picker)
    return hidden ? value : value.filter(model => !model.hidden)
  }

  async modelStatus(agent: Agent, hidden: boolean, refresh: boolean) {
    let models: Model[]
    try { models = await this.models(agent, hidden, refresh, refresh) }
    catch { return { models: [], unavailable: true, generation: this.snapshot(agent).generation } }
    const snapshot = this.snapshot(agent)
    return { models, unavailable: snapshot.account?.signedIn === true && snapshot.modelFailed === true, generation: snapshot.generation }
  }

  private catalog(agent: Agent, snapshot: Snapshot, refresh: boolean, picker = false): Promise<Model[]> {
    const cached = snapshot.models
    if (!refresh && cached && snapshot.listedRevision === snapshot.revision) return Promise.resolve(cached)
    const listing = snapshot.listing
    if (listing) {
      if (!refresh || listing.refresh) return listing.pending
      // A picker may open while the first cached read is still loading.
      return listing.pending.catch(() => []).then(value =>
        this.snapshot(agent) === snapshot ? this.catalog(agent, snapshot, true, picker) : value)
    }
    const revision = snapshot.revision
    const pending = agent.models({ hidden: true, refresh }).then(value => {
      if (snapshot.listing?.pending !== pending || this.snapshot(agent) !== snapshot) return value
      snapshot.models = value
      snapshot.modelFailed = false
      snapshot.listedRevision = revision
      if (JSON.stringify(cached) !== JSON.stringify(value)) emit("model.updated", { accountRefresh: true, ...(picker ? { pickerRefresh: true } : {}) })
      return value
    }).catch(error => {
      if (snapshot.listing?.pending === pending && this.snapshot(agent) === snapshot) snapshot.modelFailed = true
      if (cached) return cached
      throw error
    }).finally(() => { if (snapshot.listing?.pending === pending) snapshot.listing = undefined })
    snapshot.listing = { refresh, pending }
    return pending
  }

  catalogChanged(agents: Agent[], programUpdated?: string) {
    for (const agent of agents) {
      const snapshot = this.snapshot(agent)
      if (programUpdated && programUpdated === agent.id) {
        this.snapshots.set(agent, { account: snapshot.account, identity: snapshot.identity, models: snapshot.models, generation: snapshot.generation, revision: snapshot.revision + 1 })
        void this.models(agent, false, true).catch(() => {})
      } else if (!programUpdated) {
        snapshot.revision++
      }
    }
  }

  async round(agents: Agent[], opened = false): Promise<void> {
    const now = this.now()
    if (opened) {
      if (now < this.nextOpen) return
      this.nextOpen = now + 30_000
    } else {
      if (now < this.nextRound) return
      this.nextRound = now + 30_000
    }
    await Promise.allSettled(agents.map(agent => this.read(agent, true)))
  }
}
