import { usagePeerRoutes } from "./peer-routes"
// One usage reader for a box, kept independently of the coding agents' bridge. The same on-disk
// paths let it take over the existing readings, preferences, and reset journal.
import { mkdirSync, readFileSync } from "node:fs"
import { join } from "node:path"
import { readSavedSessions } from "../agents/store"
import { feed } from "../agents/events"
import { GroupStore } from "../agents/groups/store"
import { LinkDirectory, PeerPost } from "../agents/groups/post"
import { parseProvider } from "../agents/accounts"
import { isRealBridgeFolder } from "../shared/account"
import { writeSecret } from "../shared/files"
import { bridgeRecordFormat, readRendezvous, type BridgeRecord } from "../shared/records"
import { engineGet, readEngineFile } from "../link/engine/client"
import { UsageAccounts, type UsageAgent } from "./accounts"
import { agentAccounts, BOX_ONLY_SERVICES, Limits, limitsRoutes, type LimitWindow } from "./limits"
import { EngineAccounts, ENGINE_SERVICES } from "./engine-limits"
import { LimitsPeers } from "./limits-peers"
import { LimitResets } from "./limits-resets"
import { Insights, insightsRoutes } from "./insights"
import { KeptSummaries } from "./insights-kept"
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
  bundle?: string
  conversationHome?: (session: string, origin?: string) => string | undefined
}

export function usageReader({ home, run, box, bots, engineData, bundle, conversationHome }: UsageOptions, fixtures?: {
  accounts: () => UsageAgent[]
  engineCredentials: () => Promise<unknown>
}) {
  const accounts = new UsageAccounts(home)
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
  const signIns = new Map<string, { signedIn: boolean; signIn: string | null; revision: string }>()
  const currentBridge = () => {
    try { return readRendezvous<BridgeRecord>(bridgeRecordFormat, join(run, "bridge.json")).pid }
    catch { return undefined }
  }
  const confirmed = (agent: UsageAgent) => {
    const signIn = agent.limits?.signIn?.()
    if (!signIn) return false
    const account = signIns.get(agent.providerID)
    return account?.signIn === signIn ? account.signedIn : undefined
  }
  const available = () => reading ? said.filter(agent => confirmed(agent) === true) : []
  const limits = new Limits(() => [...agentAccounts(available()), ...engineAccounts.accounts()], join(home, "limits.json"),
    Date.now, async () => {
      said = agents()
      await Promise.all([engineAccounts.refresh(), reading && Promise.allSettled(said.map(agent => agent.prepare?.()))])
    }, () => engineAccounts.answered && (!reading || said.every(agent => confirmed(agent) !== undefined)))
  limits.covers = () => agentAccounts(said).map(account => account.service)
  limits.unconfirmed = () => reading ? agentAccounts(said.filter(agent => confirmed(agent) === undefined)) : []
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
  const fallback = insightKey("kuvo insight computer", ownComputer()).slice(0, 32)
  const localKey = () => {
    // A paired restore gives its fresh membership a distinct fallback. Old stamps in
    // portable conversations cannot become local evidence on a same-host replacement.
    try {
      const { insightsAlias } = JSON.parse(readFileSync(join(run, "..", "backup", "restore.json"), "utf8"))
      if (typeof insightsAlias === "string" && /^[0-9a-f]{32}$/.test(insightsAlias)) return insightsAlias
    } catch {}
    return fallback
  }
  const computer = () => directory.own()?.key ?? localKey()
  let index: InsightsDB | undefined
  try {
    const groups = new GroupStore(join(box, "groups"), bots, undefined, false)
    const scanner = new InsightsScan({
      root: home, cache: join(folder, "insights-logs"), agents,
      store: () => readSavedSessions(join(home, "sessions")),
      bots: () => [
        ...directory.bots().flatMap(({ id, name, session }) => session ? [{ id, name, sessionID: session }] : []),
        ...groups.allBots(),
      ],
      computer, engine: engineData,
    })
    index = new InsightsDB(join(folder, "insights.sqlite"), bundle)
    usage.insights = new Insights(index, scanner, pricing, directory, post, localKey, Date.now, conversationHome, () => groups.allBots())
    usage.insights.kept = new KeptSummaries(join(folder, "insights-kept.json"))
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
      const given = await body() as { bridge?: number; provider?: string; signedIn?: boolean; signIn?: string | null; revision?: string; window?: Omit<LimitWindow, "source" | "read"> }
      const owns = () => Number.isSafeInteger(given.bridge) && given.bridge === currentBridge()
      const retry = () => Response.json({}, { status: 409 })
      if (!owns()) return retry()
      const provider = given.provider
      if (provider && parseProvider(provider) && typeof given.revision === "string" && (given.signIn === null || typeof given.signIn === "string")) {
        said = agents()
        const agent = said.find(agent => agent.providerID === provider)
        const previous = signIns.get(provider)
        if (typeof given.signedIn === "boolean" && previous?.revision !== given.revision) {
          if (given.signIn !== (agent?.limits?.signIn?.() ?? null)) await agent?.prepare?.(true)
          const current = agent?.limits?.signIn?.() ?? null
          // The credential can change before its account check reaches this process.
          if (!owns() || (agent && given.signIn !== current)) return retry()
          signIns.set(provider, { signedIn: given.signedIn, signIn: given.signIn, revision: given.revision })
          if (!given.signedIn && given.signIn === current) agent?.signedOut?.()
          if (previous || !given.signedIn) limits.invalidate(provider)
          if (given.signedIn) void limits.readAgain(provider).catch(() => {})
          else { limits.ownChanged(); void limits.tick().catch(() => {}) }
          void usage.refresh()
        } else if (given.window) {
          if (previous?.revision !== given.revision) return retry()
          if (previous.signedIn && previous.signIn === given.signIn && agent && (confirmed(agent) !== true || !limits.hear(provider, given.window))) return retry()
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
