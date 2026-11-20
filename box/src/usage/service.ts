import { SignIns, signedInAgents } from "./sign-ins"
import { usagePeerRoutes } from "./peer-routes"
// One usage reader for a box, kept independently of the coding agents' bridge. The same on-disk
// paths let it take over the existing readings, preferences, and reset journal.
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { readSavedSessions } from "../agents/store"
import { feed } from "../agents/events"
import { GroupStore } from "../agents/groups/store"
import { LinkDirectory, PeerPost } from "../agents/groups/post"
import { parseProvider } from "../agents/accounts"
import { isRealBridgeFolder } from "../shared/account"
import { writeSecret } from "../shared/files"
import { engineGet, readEngineFile } from "../link/engine/client"
import { UsageAccounts, type UsageAgent } from "./accounts"
import { agentAccounts, BOX_ONLY_SERVICES, Limits, limitsRoutes, type LimitWindow } from "./limits"
import { EngineAccounts, ENGINE_SERVICES } from "./engine-limits"
import { LimitsPeers } from "./limits-peers"
import { LimitResets } from "./limits-resets"
import { Insights, insightsRoutes } from "./insights"
import { InsightsDB } from "./insights-db"
import { InsightsScan } from "./insights-scan"
import { insightKey } from "./insight-records"
import { PricingStore } from "./pricing"
import { antigravitySources, claudeSources, codexSources, cursorSources, engineCodexSource, engineHostedSources, grokSources, ownComputer, Usage, usageRoutes, UsageSettings, USAGE_SERVICES } from "./usage"
import { antigravityTransport } from "./antigravity-plan"
import { cursorEndpoints } from "./cursor-api"
import { UsagePeers } from "./usage-peers"
import { SettingsPeers } from "./settings-peers"

export interface UsageOptions {
  home: string
  run: string
  box: string
  bots: string
  engineData?: string
  managedDefaults: boolean
  bundle?: string
}

export function usageReader({ home, run, box, bots, engineData, managedDefaults, bundle }: UsageOptions, fixtures?: {
  accounts: () => UsageAgent[]
  engineCredentials: () => Promise<unknown>
}) {
  const accounts = new UsageAccounts(home, managedDefaults)
  const live = process.env.KUVO_TEST_PRICE_FEEDS !== "off" && isRealBridgeFolder(home)
  const agents = fixtures?.accounts ?? (() => accounts.all())
  const reading = live || !!fixtures
  const folder = join(home, "usage")
  mkdirSync(folder, { recursive: true, mode: 0o700 })
  const post = new PeerPost(join(folder, "outbox.json"))
  const directory = new LinkDirectory(post)
  const engineAccounts = new EngineAccounts(fixtures?.engineCredentials ?? (async () => {
    if (!live) return []
    return engineGet(readEngineFile(), "api/credential")
  }))
  let said = agents()
  const signIns = new SignIns()
  const changes = new Map<string, number>()
  const available = () => reading ? signedInAgents(said, signIns) : []
  const limits = new Limits(() => [...agentAccounts(available()), ...engineAccounts.accounts()], join(home, "limits.json"),
    Date.now, async () => {
      said = agents()
      await Promise.all([engineAccounts.refresh(), reading && (async () => {
        await Promise.allSettled(said.map(agent => agent.prepare?.()))
        await signIns.round(said.map(agent => ({ ...agent, account: async () => ({ signedIn: Boolean(agent.limits?.signIn?.()) }) })))
      })()])
    }, () => engineAccounts.answered && (!reading || signIns.allSaid(said)))
  limits.covers = () => agentAccounts(said).map(account => account.service)
  limits.alsoServices = [...ENGINE_SERVICES, ...BOX_ONLY_SERVICES]
  const limitsPeers = new LimitsPeers(limits, post, directory, join(home, "limits-peers.json"))
  const resets = new LimitResets(limits, join(home, "limits-resets.json"), Date.now, post, directory)
  const settings = new UsageSettings(join(folder, "settings.json"), undefined, undefined, run)
  const settingsPeers = new SettingsPeers(settings, post, directory)
  limits.settings = settings
  limits.ownKey = () => directory.own()?.key ?? ""
  const claude = claudeSources(folder)
  const codex = codexSources(folder, () => settings.codexFallback)
  const antigravity = antigravitySources(folder)
  const grok = grokSources(folder)
  const engineCodex = engineCodexSource(engineData,
    () => engineAccounts.answered ? engineAccounts.accounts().some(account => account.provider === "openai") : undefined,
    () => usage.has("openai"))
  const hosted = engineHostedSources(engineData)
  cursorEndpoints.live = antigravityTransport.live = live
  const cursor = cursorSources(folder, (provider, identity) =>
    limits.held(provider) || (identity !== undefined && !!usage.peers?.readsWhole("cursor", identity)))
  const pricing = new PricingStore(join(folder, "pricing"), live ? undefined : null)
  const usage = new Usage(() => [...claude(said), ...codex(said), ...grok(said), ...engineCodex(), ...hosted(), ...cursor(said), ...antigravity(said)],
    pricing, provider => limits.identity(provider), join(folder, "usage.json"))
  usage.settings = settings
  engineAccounts.onAnswered = () => void usage.tick()
  if (!engineData) usage.services = USAGE_SERVICES.filter(service => service !== "opencode")
  const peers = new UsagePeers(usage, post, directory, join(folder, "peers.json"))
  const localKey = insightKey("kuvo insight computer", ownComputer()).slice(0, 32)
  const computer = () => directory.own()?.key ?? localKey
  let index: InsightsDB | undefined
  try {
    const groups = new GroupStore(join(box, "groups"), bots, undefined, false)
    const scanner = new InsightsScan({
      root: home, cache: join(folder, "insights-logs"), agents,
      store: () => readSavedSessions(join(home, "sessions")),
      bots: () => groups.allBots(), computer, engine: engineData,
    })
    index = new InsightsDB(join(folder, "insights.sqlite"), bundle)
    usage.insights = new Insights(index, scanner, pricing, directory, post, localKey)
  } catch (error) {
    console.error("Insights index:", error)
  }

  let lastAttribution = ""
  const reflect = () => {
    const value = Object.fromEntries([...agentAccounts(available()), ...engineAccounts.accounts()].flatMap(({ provider, service }) => {
      const identity = limits.identity(provider)
      return identity ? [[provider, `${service}#${identity}`]] : []
    }))
    const attribution = JSON.stringify({ computer: computer(), accounts: value })
    if (attribution !== lastAttribution) {
      writeSecret(join(run, "usage-attribution.json"), attribution)
      lastAttribution = attribution
    }
  }

  const started = () => {
    limits.start()
    usage.start()
    usage.insights?.start()
    setInterval(() => settings.followShared(), 2_000).unref()
    setInterval(() => settingsPeers.tell(), 60_000).unref()
    setInterval(() => reflect(), 5_000).unref()
    reflect()
  }

  const handle = async (request: Request) => {
    const url = new URL(request.url)
    const [, api, kuvo, route, ...rest] = url.pathname.split("/")
    const method = request.method
    const body = () => request.json().catch(() => ({}))
    if (api !== "api" || kuvo !== "kuvo") return Response.json({}, { status: 404 })
    if (route === "limits") return limitsRoutes(limits, method, rest, body)
    if (route === "usage") return usageRoutes(usage, method, rest, url.searchParams, body)
    if (route === "insights") return insightsRoutes(usage.insights, method, rest, url.searchParams)
    if (route === "usage-events" && method === "GET") return feed()
    if (route === "usage-observed" && method === "POST" && reading) {
      const given = await body() as { provider?: string; signedIn?: boolean; signIn?: string | null; window?: Omit<LimitWindow, "source" | "read"> }
      const provider = given.provider
      if (provider && parseProvider(provider)) {
        said = agents()
        const agent = said.find(agent => agent.providerID === provider)
        const source = agent?.limits
        const signIn = source?.signIn?.() ?? null
        if (given.signIn !== signIn) return Response.json({})
        const generation = changes.get(provider) ?? 0
        if (source && given.window && signIn) {
          if (!source.heard || signIns.signedIn(provider) !== true) await limits.tick()
          if ((changes.get(provider) ?? 0) === generation && signIns.signedIn(provider) && signIn === source.signIn?.())
            source.heard?.(given.window)
        } else if (typeof given.signedIn === "boolean") {
          changes.set(provider, generation + 1)
          signIns.record(provider, { signedIn: given.signedIn && !!signIn }, { signIn: signIn ?? undefined })
          if (!given.signedIn && !signIn) agent?.signedOut?.()
          limits.invalidate(provider)
          if (given.signedIn) void limits.readAgain(provider)
          else { limits.ownChanged(); void limits.tick() }
          void usage.refresh()
        }
        reflect()
      }
      return Response.json({})
    }
    if (route === "peers") {
      const response = await usagePeerRoutes({ post, directory, limits: limitsPeers, resets, insights: usage.insights, settings: settingsPeers, usage: peers }, method, rest, body, request.signal)
      if (response.ok && method === "PUT") reflect()
      return response
    }
    return Response.json({}, { status: 404 })
  }
  return { handle, started, closeInsights: () => index?.close() }
}
