import "../safety"
import { expect, test } from "bun:test"
import { AccountRefresh } from "../../src/agents/account-refresh"
import type { Agent } from "../../src/agents/agent"
import { Limits } from "../../src/agents/limits"
import { subscribe } from "../../src/agents/events"
import type { Model } from "../../src/agents/wire"

test("account notification and timer pick up a changed plan, then update the catalog and rings", async () => {
  let plan = "Plus"
  let fresh = false
  const lists: boolean[] = []
  let time = 10_000_000
  const provider = "codex-cli@plan-test"
  const source = {
    every: 10 * 60_000,
    async read() {
      return { plan, windows: [{ id: "primary", used: plan === "Plus" ? 0.97 : 0.15, minutes: 300 }] }
    },
  }
  const limits = new Limits(() => [{ provider, service: "codex", limits: source }], undefined, () => time)
  const accounts = new AccountRefresh(limits, () => time)
  const agent = {
    providerID: provider,
    account: async (asked: boolean) => {
      fresh = asked
      return { installed: true, signedIn: true, label: `ChatGPT ${plan}` }
    },
    models: async ({ refresh }: { refresh: boolean }) => {
      lists.push(refresh)
      return [{ id: plan === "Plus" ? "gpt-6" : "gpt-6.2" }] as Model[]
    },
  } as unknown as Agent
  await limits.tick()
  await accounts.round([agent])
  expect((await accounts.models(agent))[0]?.id).toBe("gpt-6")
  expect(lists).toEqual([false])
  plan = "Pro"
  const events: string[] = []
  const stop = subscribe(event => events.push(event.type))
  try {
    time += 60_000
    expect((await accounts.changed(agent)).label).toBe("ChatGPT Pro")
    expect(fresh).toBe(true)
    expect((await accounts.models(agent))[0]?.id).toBe("gpt-6.2")
    await limits.tick()
    expect(limits.list()[0]).toMatchObject({ plan: "Pro", windows: [{ used: 0.15 }] })
    expect(events).toContain("integration.updated")
    expect(events).toContain("model.updated")
    expect(events).toContain("kuvo.limits.updated")
    expect(lists).toEqual([false, true])
    time += 5 * 60_000
    await accounts.round([agent])
    expect(lists).toEqual([false, true])
    plan = "Plus"
    time += 5 * 60_000
    await accounts.round([agent])
    expect((await accounts.models(agent))[0]?.id).toBe("gpt-6")
    expect(lists).toEqual([false, true, true])
    time += 8 * 60 * 60_000
    expect(limits.rows()[0]?.rings).toEqual({})
  } finally {
    stop()
  }
})

test("signing in again drops rings and ignores a read started under the old sign-in", async () => {
  let held = false
  let used = 0.98
  let respond!: () => void
  let started!: () => void
  const waiting = new Promise<void>(resolve => { started = resolve })
  const provider = "codex-cli@switch-test"
  const source = {
    every: 60_000,
    async read() {
      const before = used
      if (held) await new Promise<void>(resolve => {
        respond = resolve
        started()
      })
      return { windows: [{ id: "primary", used: before, minutes: 300 }] }
    },
  }
  const limits = new Limits(() => [{ provider, service: "codex", limits: source }])
  const accounts = new AccountRefresh(limits)
  const agent = {
    providerID: provider,
    account: async () => ({ installed: true, signedIn: true, label: "Same name" }),
    models: async () => [],
  } as unknown as Agent
  await accounts.read(agent)
  await limits.tick()
  held = true
  const old = limits.readAgain(provider)
  await waiting
  used = 0.02
  const next = accounts.signedIn(agent)
  expect(limits.list()[0]?.windows).toEqual([])
  held = false
  respond()
  await Promise.all([old, next])
  expect(limits.list()[0]?.windows[0]?.used).toBe(0.02)
})

test("account checks leave the catalog cached and each picker opening fetches new releases", async () => {
  let time = 10_000_000
  let provider = "one"
  let cached = provider
  const lists: boolean[] = []
  const agent = {
    id: "codex",
    providerID: "codex-cli@release-test",
    account: async () => ({ installed: true, signedIn: true }),
    models: async ({ refresh }: { refresh: boolean }) => {
      lists.push(refresh)
      if (refresh) cached = provider
      return [{ id: cached }] as Model[]
    },
  } as unknown as Agent
  const accounts = new AccountRefresh(undefined, () => time)
  const releases = async (refresh = false) => (await accounts.models(agent, false, refresh)).map(model => model.id)
  await accounts.round([agent])
  expect(lists).toEqual([])
  expect(await releases()).toEqual(["one"])
  provider = "two"
  time += 60 * 60_000
  await accounts.round([agent])
  await accounts.round([agent], true)
  expect(await releases()).toEqual(["one"])
  expect(lists).toEqual([false])
  expect(await releases(true)).toEqual(["two"])
  provider = "three"
  expect(await releases(true)).toEqual(["three"])
  expect(lists).toEqual([false, true, true])
})

test("an agent update refreshes its catalog immediately", async () => {
  let model = "one"
  const lists: boolean[] = []
  const agent = {
    id: "codex",
    providerID: "codex-cli@program-test",
    account: async () => ({ installed: true, signedIn: true }),
    models: async ({ refresh }: { refresh: boolean }) => {
      lists.push(refresh)
      return [{ id: refresh ? model : "one" }] as Model[]
    },
  } as unknown as Agent
  const accounts = new AccountRefresh()
  expect((await accounts.models(agent))[0]?.id).toBe("one")
  model = "two"
  let stop = () => {}
  const updated = new Promise<void>(resolve => {
    stop = subscribe(event => {
      if (event.type === "model.updated" && event.data?.accountRefresh === true) resolve()
    })
  })
  try {
    accounts.catalogChanged([agent], agent.id)
    await updated
    expect(lists).toEqual([false, true])
    expect((await accounts.models(agent))[0]?.id).toBe("two")
  } finally {
    stop()
  }
})

test("opening a picker during the first catalog read still fetches the provider catalog", async () => {
  let start!: () => void
  let finish!: () => void
  const started = new Promise<void>(resolve => { start = resolve })
  const finished = new Promise<void>(resolve => { finish = resolve })
  const agent = {
    providerID: "codex-cli@picker-test",
    account: async () => ({ installed: true, signedIn: true }),
    models: async ({ refresh }: { refresh: boolean }) => {
      if (!refresh) {
        start()
        await finished
      }
      return [{ id: refresh ? "new" : "cached" }] as Model[]
    },
  } as unknown as Agent
  const accounts = new AccountRefresh()
  const cached = accounts.models(agent)
  await started
  const refreshed = accounts.models(agent, false, true)
  finish()
  expect((await cached)[0]?.id).toBe("cached")
  expect((await refreshed)[0]?.id).toBe("new")
})

test("failed models are reported per account, with the last successful list of that account only", async () => {
  let fail = false
  const agent = {
    id: "fake",
    providerID: "fake@model-failure",
    account: async () => ({ installed: true, signedIn: true }),
    models: async () => {
      if (fail) throw new Error("private provider error")
      return [{ id: "one" }] as Model[]
    },
  } as unknown as Agent
  const accounts = new AccountRefresh()
  const initial = await accounts.modelStatus(agent, false, false)
  fail = true
  accounts.catalogChanged([agent])
  expect(await accounts.modelStatus(agent, false, true)).toMatchObject({ models: [{ id: "one" }], unavailable: true, generation: initial.generation })
  const restarted = await new AccountRefresh().modelStatus(agent, false, true)
  expect(restarted).toMatchObject({ models: [], unavailable: true })
  expect(restarted.generation).not.toBe(initial.generation)
  await accounts.signedIn(agent)
  const signedIn = await accounts.modelStatus(agent, false, true)
  expect(signedIn).toMatchObject({ models: [], unavailable: true })
  expect(signedIn.generation).not.toBe(initial.generation)
})

test("a model change during a read leaves the next ordinary read due", async () => {
  let provider = "old"
  let release!: () => void
  let started!: () => void
  const firstRead = new Promise<void>(resolve => { started = resolve })
  const pending = new Promise<void>(resolve => { release = resolve })
  const agent = {
    providerID: "changed-in-flight",
    account: async () => ({ installed: true, signedIn: true }),
    models: async () => {
      const id = provider
      if (id === "old") { started(); await pending }
      return [{ id }] as Model[]
    },
  } as unknown as Agent
  const accounts = new AccountRefresh()
  const first = accounts.models(agent, false, true)
  await firstRead
  provider = "new"
  accounts.catalogChanged([agent])
  release()
  expect((await first)[0]?.id).toBe("old")
  expect((await accounts.models(agent))[0]?.id).toBe("new")
})
