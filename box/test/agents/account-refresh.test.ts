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
  let lists = 0
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
    models: async () => {
      lists++
      return [{ id: plan === "Plus" ? "gpt-6" : "gpt-6.2" }] as Model[]
    },
  } as unknown as Agent
  await limits.tick()
  await accounts.round([agent])
  expect((await accounts.models(agent))[0]?.id).toBe("gpt-6")
  expect(lists).toBe(1)
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
    expect(lists).toBe(2)
    time += 5 * 60_000
    await accounts.round([agent])
    expect(lists).toBe(3)
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

test("the hourly check and each picker opening fetch the provider catalog", async () => {
  let time = 10_000_000
  let provider = "one"
  let cached = ""
  let fetches = 0
  const agent = {
    id: "codex",
    providerID: "codex-cli@release-test",
    account: async () => ({ installed: true, signedIn: true }),
    models: async ({ refresh }: { refresh: boolean }) => {
      if (refresh) {
        cached = provider
        fetches++
      }
      return [{ id: cached }] as Model[]
    },
  } as unknown as Agent
  const accounts = new AccountRefresh(undefined, () => time)
  const releases = async () => (await accounts.models(agent)).map(model => model.id)
  await accounts.round([agent])
  expect(await releases()).toEqual(["one"])
  provider = "two"
  time += 59 * 60_000
  await accounts.round([agent])
  expect(await releases()).toEqual(["one"])
  time += 60_000
  await accounts.round([agent])
  expect(await releases()).toEqual(["two"])
  provider = "three"
  expect((await accounts.models(agent, false, true))[0]?.id).toBe("three")
  expect(fetches).toBe(3)
})
