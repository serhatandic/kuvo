import { servicePath } from "../shared/box-services"
import type { BoxServicesClient } from "./box-services"
import type { Feature } from "../shared/features"
import { UnsupportedFormatVersion } from "../shared/format-version"
import { ROUTE_VERSION, ROUTE_VERSION_HEADER, routeVersionError } from "../shared/route-version"
import { failureWords } from "../shared/sign-in-failure"
import { AccountRefresh } from "./account-refresh"
import { watchSignIn } from "./signin-watch"
// The bridge's HTTP API: the engine's, the parts the app uses for a session, its models and its
// sign-in, answered for outside agents. Anything else is "not here", which the app takes as it
// takes an engine without that feature.

import { kinds, parseProvider, type AgentKind } from "./accounts"
import type { Account, Agent, SignInAttempt } from "./agent"
import type { Auto, AutoList } from "./auto"
import type { EngineLink } from "./engine"
import type { Roster } from "./roster"
import { emit, feed, subscribe } from "./events"
import type { SessionPlaces } from "./session-places"
import { sideQuestionRoutes, type SideQuestions } from "./btw"
import { branchRoutes } from "./branch"
import { isOutdated, type Maintenance } from "./maintenance"
import { UpdateNotices, updateNoticesFile } from "./update-notices"
import { agentsUpdateThemselves } from "../shared/agent-updates"
import { forgetUpdate, inUpdateAll, programUpdate, resumeProgramUpdates, updateAll, updateProgram } from "./updates"
import { refreshLoginPath } from "./proc"
import { setToolServer } from "./kuvotools"
import { noteToolServer } from "./fence"
import { newID } from "./ids"
import { BadRequest, NotFound, Sessions } from "./sessions"
import { carryRoutes, type Carrying } from "./carry"
import { linkRoutes } from "./groups/link"
import { peerRoutes, type PeerParts } from "./groups/post"
import { groupRoutes } from "./groups/routes"
import type { GroupRunner } from "./groups/runner"
import type { GroupStore } from "./groups/store"
import { shell, shellOutput, shells, stopShell } from "./shells"
import type { Integration, Model, Session } from "./wire"
import { compactionChoice, summarizeAt, summarizeSetting, type SummarizeSetting } from "../shared/context"

export interface Context {
  roster: Roster
  sessions: Sessions
  auto: Auto
  link: EngineLink
  password: string
  /** Where accepted program updates wait across a bridge restart. */
  programHome?: string
  version: string
  /** The build it runs, as its bundle names it (none run from its sources). */
  build?: string | null
  /** The group chats that live on this computer (groups/runner.ts), and its part in those on your others; none where they don't run here. */
  groups?: { runner: GroupRunner; store: GroupStore; peers?: PeerParts }
  /** What this bridge tells your other computers and hears from them, through the box's link (groups/post.ts): its group chats' mail, its plan limits. */
  peers?: PeerParts
  /** Restarts the agents' processes started before the box's sandbox was on, as you said to (main.ts). */
  restartOutside?: () => void
  /** Side questions (btw.ts). */
  sideQuestions?: SideQuestions
  /** Where conversations are, for branching one (branch.ts). */
  places?: SessionPlaces
  /** Conversations on their way to and from your other computers (carry.ts). */
  carrying?: Carrying
  boxServices?: BoxServicesClient
  /** Recheck signed-in accounts even while no client is looking. */
  refreshAccounts?: boolean
}

const attempts = new Map<string, { agent: Agent; attempt: SignInAttempt; created: number; expires: number; stop: () => void; finish: () => void }>()

/** Sign-ins still waiting, given up on (the bridge is stopping). */
export function cancelSignIns() {
  for (const pending of attempts.values()) { pending.stop(); pending.attempt.cancel() }
  attempts.clear()
}

/** Whether a sign-in waits for you to finish it (a newer bridge waits for it: renewal.ts). */
export function signInsWaiting(): boolean {
  const now = Date.now()
  return [...attempts.values()].some((pending) => now < pending.expires && pending.attempt.status().status === "pending")
}

// Sign-ins left waiting past their time: stopped, with what they started.
setInterval(() => {
  for (const [id, pending] of attempts) {
    if (Date.now() < pending.expires + 60_000) continue
    pending.stop()
    pending.attempt.cancel()
    attempts.delete(id)
  }
}, 60_000).unref?.()

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { "Content-Type": "application/json" } })
const located = (value: unknown) => json({ location: { directory: process.cwd() }, data: value })
const error = (status: number, message: string) => json({ message }, status)

/** What each agent's account looked like when last asked: a change means other models. */
const lastSeen = new Map<string, { installed: boolean; signedIn: boolean; label?: string }>()
/** Each agent's program's version and the newest, as last known, by program (`agent.id`: one
 * program serves every account with it). */
const lastMaintained = new Map<string, Maintenance>()

/**
 * Its version and the newest, if they come soon (checking may ask Homebrew or npm): what was
 * known before otherwise, and the account is said again once they do.
 */
/** Each program as it's read, for its update notices (the bridge's handler sets it, update-notices.ts). */
let noticeUpdates: ((agent: Agent, value: Maintenance) => void) | undefined

/** What `agent`'s program is now, as just read; whether that's news. */
function maintainedNow(agent: Agent, value: Maintenance, matching: Agent[] = []): boolean {
  noticeUpdates?.(agent, value)
  const before = lastMaintained.get(agent.id)
  const changed = JSON.stringify(before) !== JSON.stringify(value)
  lastMaintained.set(agent.id, value)
  if (matching.length && before && before.installed !== value.installed) {
    for (const each of matching) each.programUpdated?.()
    emit("model.updated", { program: agent.id })
  }
  return changed
}

async function maintainedSoon(agent: Agent, fresh: boolean, note: typeof maintainedNow): Promise<Maintenance | undefined> {
  if (!agent.maintenance) return undefined
  let answered = false
  const asked = agent.maintenance(fresh).then(
    (value) => {
      const changed = note(agent, value)
      if (answered && changed) emit("integration.updated", { integrationID: agent.providerID })
      return value
    },
    () => lastMaintained.get(agent.id),
  )
  const value = await Promise.race([asked, Bun.sleep(2_000).then(() => lastMaintained.get(agent.id))])
  answered = true
  return value
}

export async function integration(agent: Agent, accounts: AccountRefresh, fresh = false, auto?: Auto, note = maintainedNow): Promise<Integration> {
  const [account, maintained] = await Promise.all([
    // A check that fails is its program's (each agent says itself when it isn't installed): it's
    // there, and says why, rather than offering its installer again.
    accounts.read(agent, fresh).then(
      (account) => account,
      (error): Account => ({ installed: true, signedIn: false, message: String(error?.message ?? error) }),
    ),
    maintainedSoon(agent, fresh, note),
  ])
  const before = lastSeen.get(agent.providerID)
  // Who's signed in, as last said (one not said yet, or not now, is no other account).
  const label = account.signedIn ? (account.label ?? before?.label) : undefined
  lastSeen.set(agent.providerID, { installed: account.installed, signedIn: account.signedIn, label })
  const changed = before && (before.installed !== account.installed || before.signedIn !== account.signedIn || (before.label !== undefined && before.label !== label))
  if (changed) {
    // Signed in anew (or as someone else): Custom Auto tries it again.
    if (account.signedIn && auto?.release(agent.providerID)) emit("kuvo.auto.updated", {})
  }
  return {
    id: agent.providerID,
    name: agent.name,
    metadata: {
      source: "agent",
      agent: agent.id,
      addedAccounts: kinds.includes(agent.id as AgentKind),
      // Which of your accounts with it: `default` is the agent's own sign-in.
      accountID: agent.instance.id,
      accountLabel: agent.instance.label,
      installed: account.installed,
      message: account.message,
      signInEnded: account.ended ?? false,
      // Its sign-in couldn't be checked just now, and is as it last said.
      unchecked: account.unchecked ?? false,
      account: account.label,
      install: agent.install,
      signsOut: Boolean(agent.signOut),
      // Signing out signs it out in Terminal too (its own sign-in is yours).
      signOutShared: Boolean(agent.signOut && agent.signOutShared),
      // Signing out stops a turn running on it.
      signOutStops: Boolean(agent.signOut && agent.signOutStops),
      // Its program's version, and the newest (an old Claude Code lists old models).
      version: maintained?.installed,
      latestVersion: maintained?.latest,
      outdated: maintained ? isOutdated(maintained) : false,
      updateCommand: maintained?.shown,
      updates: Boolean(agent.update && maintained?.command),
      // How its update stands, the same for every account with its program, and whether Update
      // All updates it (said of the agent's own sign-in, once for its program): the box's to say,
      // so every client shows the same (updates.ts).
      programUpdate: programUpdate(agent.id, maintained?.installed),
      updateAll: agent.instance.id === "default" && inUpdateAll(agent, maintained),
      // Whether its sign-in can finish on another device (`remote: true`).
      remoteSignIn: Boolean(agent.signIn && agent.signsInRemotely),
      // Its list (`api/model?hidden=1`) is every model it runs: one missing from it is gone.
      modelsComplete: agent.listsEveryModel ? agent.listsEveryModel() : true,
    },
    methods: agent.methods(),
    connections: account.signedIn
      ? [{ type: "agent", label: account.label, method: "oauth", ...(account.what && account.label ? { what: true as const } : {}), status: { status: "connected", message: account.label ? `Signed in as ${account.label}` : account.unchecked ? "Couldn't check the sign-in just now" : "Signed in" } }]
      : [],
  }
}

/** A session as the app reads it: its context with where it's summarized by itself, and the box's
 * setting that decides it, to offer on the conversation (`summarize`), where its agent takes it
 * (PROTOCOL.md, "Session context"). */
export function withSummarizeAt(session: Session): Session & { summarize?: SummarizeSetting } {
  const kind = parseProvider((session.contextModel ?? session.model)?.providerID ?? "")?.kind
  const choice = compactionChoice()
  const summarize = summarizeSetting(kind, choice)
  const context = session.context
  const at = context && summarizeAt(kind, context.window, choice)
  const measured = context && at !== undefined ? { ...session, context: { ...context, summarizeAt: at } } : session
  return summarize ? { ...measured, summarize } : measured
}

function page<T extends { id: string }>(items: T[], url: URL): { data: T[]; cursor: { next?: string; previous?: string } } {
  // Items come newest first unless `order=asc`; the cursor is the last one sent.
  const limit = Number(url.searchParams.get("limit") ?? 100) || 100
  const cursor = url.searchParams.get("cursor")
  let start = 0
  if (cursor) {
    const after = cursor.split(":")[1]
    const index = items.findIndex((item) => item.id === after)
    start = index >= 0 ? index + 1 : items.length
  }
  const data = items.slice(start, start + limit)
  const order = cursor?.split(":", 1)[0] ?? url.searchParams.get("order") ?? "desc"
  const filter = url.searchParams.get("parentID") ?? cursor?.split(":")[2]
  const next = start + limit < items.length && data.length ? `${order}:${data[data.length - 1].id}${filter ? `:${filter}` : ""}` : undefined
  return { data, cursor: { next } }
}

// The bridge extensions behind kuvo/agents/api/kuvo. A new extension needs a
// feature advertised by the Box. Internal operations share the bridge's name.
const BRIDGE_EXTENSIONS = {
  accounts: "agents.accounts",
  artifacts: "agents",
  auto: "auto.lists",
  branch: "branch",
  btw: "btw",
  carry: "moves.agents",
  engine: "agents",
  groups: "agents",
  insights: "usage.insights",
  limits: "limits",
  link: "agents",
  peers: "agents",
  sandbox: "sandbox",
  tools: "agents",
  updates: "agents.updates",
  usage: "usage.history",
  voice: "agents.voice",
} as const satisfies Record<string, Feature>

function bridgeExtension(name: string | undefined): name is keyof typeof BRIDGE_EXTENSIONS {
  return name !== undefined && Object.hasOwn(BRIDGE_EXTENSIONS, name)
}

const PROGRAM_RELEASE_INTERVAL = 60 * 60_000

export function handler(context: Context, transferring?: () => boolean) {
  const expected = "Basic " + Buffer.from(`opencode:${context.password}`).toString("base64")
  const { sessions, roster, auto, link } = context
  const agentFor = (id: string) => roster.find(id)
  const note = (agent: Agent, value: Maintenance) => maintainedNow(agent, value, roster.all().filter(each => each.id === agent.id))
  const programs = { home: context.programHome, hold: sessions?.holdProgram?.bind(sessions) }
  resumeProgramUpdates(() => roster.all(), note, programs)
  // The update notices, decided as each program is read (and every hour, by the running bridge).
  const updateNotices = context.programHome ? new UpdateNotices(updateNoticesFile(context.programHome)) : undefined
  noticeUpdates = updateNotices && ((agent, value) => {
    const own = roster.all().find((each) => each.id === agent.id && each.instance.id === "default") ?? agent
    const decided = updateNotices.note({ program: agent.id, name: own.name, provider: own.providerID, maintenance: value, updates: Boolean(own.update && value.command) }, agentsUpdateThemselves())
    if (decided.told) emit("kuvo.updates.notice", { notice: decided.told })
    if (decided.changed) emit("kuvo.updates.notices.updated", {})
  })
  if (updateNotices && context.refreshAccounts) {
    // Each program's newest looked up (npm, Homebrew) once soon after starting, then every hour.
    const look = () => {
      const programs = new Map<string, Agent>()
      for (const agent of roster.all()) if (agent.maintenance && !programs.has(agent.id)) programs.set(agent.id, agent)
      for (const agent of programs.values()) void agent.maintenance!(true).then((value) => note(agent, value), () => {})
    }
    setTimeout(look, 60_000).unref?.()
    setInterval(look, 60 * 60_000).unref?.()
  }
  const accounts = new AccountRefresh(undefined, Date.now, (provider, signedIn, signIn) => context.boxServices?.observe(provider, { signedIn, signIn }))
  const signedIn = (agent: Agent) => void accounts.signedIn(agent).catch(() => {})
  if (context.refreshAccounts) {
    const checkPrograms = async () => {
      const agents = roster.all()
      await Promise.allSettled(agents.filter(agent => agent.instance.id === "default" && agent.maintenance).map(async agent => {
        const value = await agent.maintenance!()
        if (!note(agent, value)) return
        for (const each of agents) if (each.id === agent.id) emit("integration.updated", { integrationID: each.providerID, programMaintenance: true })
      }))
    }
    setInterval(() => void checkPrograms(), PROGRAM_RELEASE_INTERVAL).unref?.()
    void checkPrograms()
    setInterval(() => void accounts.round(roster.all()), 60_000).unref?.()
    subscribe(event => {
      if (event.type === "model.updated" && event.data?.accountRefresh !== true) accounts.catalogChanged(roster.all(), typeof event.data?.program === "string" ? event.data.program : undefined)
      const changed = event.type === "kuvo.agent.account.changed" || (event.type === "integration.updated" && event.data?.accountRefresh !== true && event.data?.programMaintenance !== true)
      if (!changed) return
      const id = event.data?.integrationID
      const agent = typeof id === "string" ? agentFor(id) : undefined
      if (agent) void accounts.changed(agent).catch(() => {})
    })
  }

  const answer = async (request: Request): Promise<Response> => {
    const url = new URL(request.url)
    const parts = url.pathname.split("/").filter(Boolean).map(decodeURIComponent)
    const method = request.method
    const body = async (): Promise<any> => (request.headers.get("content-type")?.includes("json") ? await request.json().catch(() => ({})) : {})
    try {
      if (parts[0] !== "api") return error(404, "Not found")
      const [, area, a, b, c, d, e] = parts

      if (area === "info") return json({ version: context.version, pid: process.pid, build: context.build ?? null })
      if (area === "event") {
        if (context.refreshAccounts && url.searchParams.get("passive") !== "1") void accounts.round(roster.all(), true)
        return feed()
      }

      if (servicePath(url.pathname)) {
        return context.boxServices?.forward(request) ?? error(503, "The box's services are starting. Try again in a moment.")
      }

      // Sign-in
      if (area === "integration") {
        if (!a) {
          // `fresh`: asked anew (you refreshed, or came back to the app), with your shell's PATH read again.
          const fresh = url.searchParams.get("fresh") === "1"
          if (fresh) await refreshLoginPath()
          return located(await Promise.all(roster.all().map((agent) => integration(agent, accounts, fresh, auto, note))))
        }
        const agent = agentFor(a)
        if (!agent) return error(404, `No agent ${a}.`)
        if (!b) return located(await integration(agent, accounts, true, auto, note))
        if (b === "connect" && c === "oauth") {
          if (!d && method === "POST") {
            const { methodID, remote, browserReturn } = await body()
            if (!agent.signIn) return error(400, `${agent.name} signs in another way.`)
            // Its page opened on another device (a phone): only where that can finish.
            if (remote === true && !agent.signsInRemotely) return error(400, `Sign in to ${agent.name} on the computer it runs on: its page comes back there.`)
            const attempt = await agent.signIn(methodID, {
              remote: remote === true,
              browserReturn: remote === true && browserReturn === true,
            })
            const id = newID("att")
            const created = Date.now()
            // Long enough for a download first (Antigravity's agent) on a slow connection.
            const expires = created + 30 * 60_000
            let completed = false
            const finish = () => {
              if (completed) return
              completed = true
              signedIn(agent)
            }
            const stop = watchSignIn(attempt, expires, finish)
            attempts.set(id, { agent, attempt, created, expires, stop, finish })
            return located({ attemptID: id, url: attempt.url, instructions: attempt.instructions, mode: attempt.mode, acceptsCode: attempt.acceptsCode || undefined, userCode: attempt.userCode, time: { created, expires } })
          }
          const pending = d ? attempts.get(d) : undefined
          if (!pending || pending.agent !== agent) return error(404, "That sign-in is over. Start it again.")
          if (method === "GET" && !e) {
            const expired = Date.now() > pending.expires
            const status = expired ? { status: "expired" as const } : pending.attempt.status()
            if (expired) { pending.stop(); pending.attempt.cancel() }
            if (status.status === "complete") pending.finish()
            if (status.status !== "pending") { pending.stop(); attempts.delete(d!) }
            if (status.status === "failed") return located({ status: "failed", message: failureWords("failed", status.message) })
            // (`mode` as it stands now: a page that came late decides it.)
            return located(status.status === "pending" ? { ...status, acceptsCode: pending.attempt.acceptsCode, mode: pending.attempt.mode } : status)
          }
          if (method === "POST" && e === "complete") {
            const { code } = await body()
            if (!pending.attempt.complete) return error(400, "This sign-in finishes in the browser.")
            await pending.attempt.complete(String(code ?? ""))
            return json({})
          }
          if (method === "DELETE") {
            pending.stop()
            pending.attempt.cancel()
            attempts.delete(d!)
            return json({})
          }
        }
        if (b === "connect" && c === "key" && method === "POST") {
          const { key } = await body()
          if (!agent.signInWithKey) return error(400, `${agent.name} doesn't take a key here.`)
          await agent.signInWithKey(String(key ?? "").trim())
          signedIn(agent)
          return json({})
        }
        // Its program installed with its maker's installer (the command `metadata.install` shows),
        // as you'd run it in Terminal; then what it lists.
        if (b === "install" && method === "POST") {
          if (!agent.installProgram) return error(400, `Install ${agent.name} the way its maker says.`)
          const result = await agent.installProgram()
          if (result.installed) {
            // How an update went before is no longer news.
            forgetUpdate(agent.id)
            for (const each of roster.all()) if (each.id === agent.id) each.programUpdated?.()
            emit("integration.updated", { integrationID: agent.providerID })
            emit("model.updated", { program: agent.id })
          }
          return json({ data: result })
        }
        if (b === "disconnect" && method === "POST") {
          if (!agent.signOut) return error(400, `Sign out of ${agent.name} where you signed in.`)
          await agent.signOut()
          // Gone from the rings at once, with what was read of it (sign-ins.ts).
          await accounts.signedIn(agent)
          return json({})
        }
      }

      // Catalog
      if (area === "model") {
        if (a === "default") return located(null)
        // `hidden=1`: also those an agent still runs but keeps out of its picker (the box's
        // `kuvo/sessions/models` reads them; pickers don't).
        const hidden = url.searchParams.get("hidden") === "1"
        const refresh = url.searchParams.get("refresh") === "1"
        const results = await Promise.all(roster.all().map(async agent => ({ id: agent.providerID, ...await accounts.modelStatus(agent, hidden, refresh) })))
        const models = results.flatMap(each => each.models)
        if (url.searchParams.get("details") === "1") {
          return located({ models, generations: Object.fromEntries(results.map(({ id, generation }) => [id, generation])), unavailable: results.filter(each => each.unavailable).map(({ id }) => ({ id })) })
        }
        return located(models)
      }
      if (area === "agent" || area === "command" || area === "skill" || area === "mcp") return located([])
      if (area === "provider") return located([])

      // Commands agents left running in the background (the bar over the composer)
      if (area === "shell") {
        const header = request.headers.get("x-opencode-directory")
        const directory = url.searchParams.get("directory") ?? (header ? decodeURIComponent(header) : undefined)
        if (!a && method === "GET") return located(shells(directory))
        if (a && !b && method === "GET") {
          const job = shell(a)
          return job ? located(job) : error(404, "No such command.")
        }
        if (a && b === "output" && method === "GET") {
          const limit = url.searchParams.get("limit")
          const read = await shellOutput(a, Number(url.searchParams.get("cursor") ?? 0) || 0, limit ? Number(limit) : undefined)
          return read ? located(read) : error(404, "No such command.")
        }
        if (a && !b && method === "DELETE") return (await stopShell(a)) ? json({}) : error(404, "No such command.")
      }

      // Sessions
      if (area === "session") {
        if (!a) {
          if (method === "POST") {
            const session = sessions.create(await body())
            return json({ data: await sessions.withContextWindow(session.id) })
          }
          const directory = url.searchParams.get("directory") ?? undefined
          // Later pages carry the first's filter in their cursor ("<order>:<id>:<parentID>").
          const under = url.searchParams.get("parentID") ?? url.searchParams.get("cursor")?.split(":")[2] ?? null
          const roots = sessions.list(directory, under === null || under === "" ? undefined : under === "null" ? null : under)
          const ordered = (url.searchParams.get("cursor")?.startsWith("asc") || url.searchParams.get("order") === "asc") ? [...roots].reverse() : roots
          return json(page(ordered, url))
        }
        if (a === "active") return json({
          data: Object.fromEntries(sessions.active().map((id) => [id, { type: "busy" }])),
          ...(url.searchParams.has("release-switch") ? { engineTurns: sessions.engineTurns() } : {}),
        })
        if (a === "background") return json({ data: sessions.background(true) })
        // A conversation from the engine, carried on here.
        if (a === "import" && method === "POST") return json({ data: sessions.importSession(await body()) })
        const id = a
        if (!b) {
          if (method === "GET") {
            const session = url.searchParams.get("context") === "0" ? sessions.get(id) : await sessions.withContextWindow(id)
            return json({ data: withSummarizeAt(session) })
          }
          if (method === "PATCH") {
            sessions.update(id, await body())
            return json({ data: sessions.get(id) })
          }
          if (method === "DELETE") {
            await sessions.remove(id)
            return json({})
          }
        }
        switch (b) {
          case "compact":
            if (method === "POST") return json({ data: sessions.compact(id) })
            break
          case "background":
            if (method === "DELETE") {
              await sessions.stopBackground(id)
              return json({})
            }
            break
          case "message": {
            const cursor = url.searchParams.get("cursor")
            const asc = url.searchParams.get("order") === "asc" || cursor?.startsWith("asc") === true
            const limit = Number(url.searchParams.get("limit") ?? 100) || 100
            if (!c && Number.isSafeInteger(limit) && limit > 0 && limit < Number.MAX_SAFE_INTEGER && (!cursor || cursor.split(":")[1])) {
              const saved = sessions.messagePage(id, cursor?.split(":")[1], limit, asc)
              if (saved) {
                const order = cursor?.split(":", 1)[0] ?? url.searchParams.get("order") ?? "desc"
                const filter = url.searchParams.get("parentID") ?? cursor?.split(":")[2]
                const next = saved.more && saved.data.length ? `${order}:${saved.data.at(-1)!.id}${filter ? `:${filter}` : ""}` : undefined
                return json({ data: saved.data, cursor: { next } })
              }
            }
            const messages = sessions.messages(id)
            if (c) {
              const found = messages.find((message) => message.id === c)
              return found ? json({ data: found }) : error(404, "No such message.")
            }
            return json(page(asc ? messages : [...messages].reverse(), url))
          }
          case "inbox":
            if (method === "DELETE" && c) {
              const outcome = sessions.cancelInput(id, c, false)
              if (outcome === "busy") return error(409, "Kuvo is checking whether the agent took this message.")
              if (outcome === "missing") return error(404, "That message isn't waiting any more.")
              return json({})
            }
            if (method === "PATCH" && c) {
              const asked = await body()
              sessions.setDelivery(id, c, asked.delivery, asked.into === true)
              return json({})
            }
            return json({ data: sessions.inbox(id) })
          case "prompt": {
            const input = await body()
            sessions.prompt(id, { id: input.id ?? newID("msg"), text: String(input.text ?? ""), files: input.files ?? [], delivery: input.delivery ?? "steer", metadata: input.metadata && typeof input.metadata === "object" && !Array.isArray(input.metadata) ? input.metadata : undefined }, typeof input.handoffToken === "string" ? input.handoffToken : undefined)
            return json({})
          }
          case "prompt-receipt":
            if (method === "POST" && c && d === "cancel") {
              const { token } = await body()
              if (typeof token !== "string" || !/^[a-f0-9-]{36}$/.test(token)) return error(400, "Give the handoff token.")
              return sessions.cancelHandoff(id, c, token) ? json({}) : error(409, "The bridge already has that message.")
            }
            if (method === "GET" && c) return sessions.confirmPrompt(id, c) ? json({ data: { id: c } }) : error(404, "No such message.")
            break
          case "fork":
            return json({ data: sessions.fork(id, String((await body()).before ?? "")) })
          case "revert":
            if (c === "stage" && method === "POST") return json({ data: await sessions.stageRevert(id, String((await body()).messageID ?? "")) })
            if (!c && method === "DELETE") {
              await sessions.clearRevert(id)
              return json({})
            }
            break
          case "synthetic": {
            const input = await body()
            sessions.synthetic(id, String(input.text ?? ""), typeof input.description === "string" ? input.description : undefined)
            return json({})
          }
          case "generate": {
            const input = await body()
            const prompt = String(input.prompt ?? "")
            if (!prompt.trim()) throw new BadRequest("Say what to write.")
            // `orAnother`: another agent you're signed in to may answer, where this one can't.
            // `model`: on that model of the session's own maker, with its effort (Auto's reviewer, on
            // its small one; a side question asked on another).
            const model = input.model && typeof input.model.providerID === "string" && typeof input.model.id === "string" ? { providerID: input.model.providerID, id: input.model.id, ...(typeof input.model.variant === "string" && input.model.variant ? { variant: input.model.variant } : {}) } : undefined
            return json({ data: { text: await sessions.generate(id, prompt, { orAnother: input.orAnother === true, model }) } })
          }
          case "interrupt":
            return json({ interrupted: await sessions.interrupt(id, url.searchParams.get("resume") === "true") })
          case "model":
            sessions.selectModel(id, (await body()).model)
            return json({})
          case "agent":
            sessions.selectAgent(id, (await body()).agent)
            return json({})
          case "permission":
            // What the engine asks for everyone ("global": its MCP servers') isn't asked here.
            if (method === "GET" && !c) {
              if (url.searchParams.get("live") !== "1" && !(await sessions.permissionsReady(id, request.signal))) return json({ message: "Kuvo is restoring this session’s approvals. Try again in a moment." }, 503)
              return json({ data: id === "global" ? [] : sessions.permissions(id) })
            }
            if (method === "POST" && c && d === "reply" && parts.length === 6) {
              const { decision, message } = await body()
              const refused = await sessions.replyPermissionLive(id, c, decision, typeof message === "string" ? message : undefined, request.signal)
              if (refused) return error(refused.status, refused.message)
              return json({})
            }
            break
          case "form":
            if (method === "GET" && !c) return json({ data: id === "global" ? [] : sessions.forms(id) })
            if (method === "POST" && c && d === "reply" && parts.length === 6) {
              const { answer } = await body()
              request.signal.throwIfAborted()
              sessions.replyForm(id, c, answer ?? {})
              return json({})
            }
            if (method === "DELETE" && c && parts.length === 5) {
              request.signal.throwIfAborted()
              sessions.replyForm(id, c, null, url.searchParams.get("message") ?? undefined)
              return json({})
            }
            break
          case "view": {
            const { idle } = await body()
            sessions.view(id, Number(idle) || Date.now())
            return json({})
          }
          case "diff":
            return json({ data: [] })
        }
      }

      // Your accounts with the agents: adding one (it then signs in as any does), naming one, removing one.
      const extension = area === "kuvo" && bridgeExtension(a) ? a : undefined

      if (extension === "accounts") {
        if (method === "POST" && !b) {
          const { agent: givenAgent, provider, label } = await body()
          const agent = givenAgent ?? (typeof provider === "string" ? parseProvider(provider)?.kind : undefined)
          if (!kinds.includes(agent)) return error(400, "That isn't one of the coding agents.")
          const record = roster.accounts.add(agent as AgentKind, label)
          roster.sync()
          const added = roster.all().find((each) => each.instance.id === record.id && each.id === agent)!
          emit("integration.updated", { integrationID: added.providerID })
          return json({ data: { providerID: added.providerID } })
        }
        const target = b ? roster.find(b) : undefined
        if (!target) return error(404, "No such account.")
        if (target.instance.id === "default") return error(400, "That's the agent's own sign-in: sign out of it where you signed in.")
        if (method === "PATCH") {
          roster.accounts.rename(target.providerID, (await body()).label)
          roster.sync()
          emit("integration.updated", { integrationID: target.providerID })
          return json({})
        }
        if (method === "DELETE") {
          if (url.searchParams.get("onlyUnsigned") === "1" && (await target.account(true)).signedIn) return json({})
          sessions.forgetProvider(target.providerID)
          await target.removeAccount?.().catch(() => {})
          target.shutdown?.()
          // Signed out as it goes: gone from the rings at once (sign-ins.ts).
          roster.accounts.remove(target.providerID)
          await context.boxServices?.observe(target.providerID, { signedIn: false, signIn: null })
          roster.sync()
          emit("integration.updated", { integrationID: target.providerID })
          emit("model.updated", {})
          return json({})
        }
      }

      // Updating the agents' programs, answered as it starts: every account with the program says
      // how it goes (`metadata.programUpdate`). `POST kuvo/updates`: Update All, each program it
      // picks (`metadata.updateAll`); `POST kuvo/updates/<provider>`: that account's program;
      // `DELETE kuvo/updates/<provider>?at=…`: how that one went, seen, for every client.
      // The update notices (update-notices.ts): what's served now, and one seen, for every client.
      if (extension === "updates" && b === "notices") {
        if (!c && method === "GET") return json({ data: { notices: updateNotices?.list() ?? [] } })
        if (c && !d && method === "DELETE") {
          if (updateNotices?.seen(c)) emit("kuvo.updates.notices.updated", {})
          return json({})
        }
        return error(405, "Not allowed.")
      }
      if (extension === "updates" && !c) {
        if (!b) {
          if (method !== "POST") return error(405, "Not allowed.")
          return json({ data: { updating: await updateAll(roster.all(), () => roster.all(), maintainedNow, (agent) => lastMaintained.get(agent.id), programs) } })
        }
        const agent = agentFor(b)
        if (!agent) return error(404, `No agent ${b}.`)
        if (method === "DELETE") {
          const at = url.searchParams.get("at")
          if (forgetUpdate(agent.id, at === null ? undefined : Number(at))) {
            for (const each of roster.all()) if (each.id === agent.id) emit("integration.updated", { integrationID: each.providerID })
          }
          return json({})
        }
        if (method !== "POST") return error(405, "Not allowed.")
        if (!agent.update || !agent.maintenance) return error(400, `Update ${agent.name} the way you installed it.`)
        void updateProgram(agent, () => roster.all(), maintainedNow, programs)
        return json({ data: { updating: [agent.id] } })
      }

      // Where Kuvo Code's own tools are (the app says each time they start), for every agent here.
      // The group chats that live on this machine.
      // You said to restart the agents started before the box's sandbox was on (`kuvo/sandbox/restart`, from the box's link).
      if (extension === "sandbox" && b === "restart" && !c && method === "POST") {
        context.restartOutside?.()
        return json({})
      }
      if (extension === "carry") return await carryRoutes(context.carrying, method, parts.slice(3), body)
      if (extension === "btw") return await sideQuestionRoutes(context.sideQuestions, method, parts.slice(3), body, request.signal)
      if (extension === "branch") return await branchRoutes(context.places, method, parts.slice(3), body)
      if (extension === "groups") return await groupRoutes(context.groups?.runner, context.groups?.store, method, parts.slice(3), body)
      // The same, as a client reads and changes them (`kuvo/groups`, which the box's link passes on).
      if (extension === "link" && b === "groups") return await linkRoutes(context.groups?.runner, context.groups?.store, method, parts.slice(4), body, request.signal)
      // Group chats and plan limits across your computers, as the box's link carries their mail (groups/post.ts).
      if (extension === "peers") return await peerRoutes(context.peers ?? context.groups?.peers, method, parts.slice(3), body, request.signal)

      if (extension === "tools" && method === "PUT") {
        const { url: address, token, timeout } = await body()
        setToolServer(typeof address === "string" && address && typeof token === "string" ? { url: address, token, timeout: Number(timeout) || 960_000 } : undefined)
        noteToolServer(typeof address === "string" && typeof token === "string" ? address : undefined)
        return json({})
      }

      // Where the engine is (the app says each time it starts), for its models in sessions here.
      if (extension === "engine" && method === "PUT") {
        const { url: address, password } = await body()
        link.configure(typeof address === "string" && address ? address : undefined, typeof password === "string" ? password : undefined)
        return json({})
      }

      // Auto: your list of models, and the ones held back at a limit until it resets.
      if (extension === "auto") {
        if (b === "limits" && method === "DELETE") {
          // One account's (you signed in to it again, or switched to another of it), or all.
          const provider = url.searchParams.get("provider")
          if (provider) auto.release(provider)
          else auto.clearLimits()
          emit("kuvo.auto.updated", {})
          return json({})
        }
        if (!c && ((!b && method === "POST") || (b && b !== "limits" && (b !== "order" || method === "PUT") && ["PUT", "DELETE"].includes(method)))) {
          const given = await body()
          try {
            let changed: boolean
            if (b === "order") changed = auto.reorder(given.ids, given.was)
            else if (method === "DELETE") changed = auto.remove(b!, given.revision)
            else {
              const id = b ?? given.id
              if (typeof id !== "string" || !/^[\w-]{1,64}$/.test(id) || ["order", "limits"].includes(id)) return error(400, "Invalid list id.")
              if (given.name !== undefined && (typeof given.name !== "string" || !given.name.trim())) return error(400, "Send a name.")
              if (given.entries !== undefined && !Array.isArray(given.entries)) return error(400, "Send entries.")
              const patch = { ...(given.name !== undefined ? { name: given.name } : {}), ...(given.entries !== undefined ? { entries: ((await sessions.asKept([{ id, entries: given.entries }]))[0] as AutoList).entries } : {}) }
              changed = auto.change(id, given.revision, patch, method === "POST")
            }
            if (!changed) return error(409, "This list changed. Read it again before saving.")
          } catch (problem) { return error(400, (problem as Error).message) }
          emit("kuvo.auto.updated", {})
          return json({ data: { lists: await sessions.autoLists(), limits: auto.limits() } })
        }
        if (!b && method === "GET") return json({ data: { entries: await sessions.autoEntries(), lists: await sessions.autoLists(), limits: auto.limits() } })
      }

      if (area === "experimental" && a === "session" && b) {
        const id = b
        // Its instruction entries, as the engine lists them; one changed or taken away.
        if (c === "instructions" && d === "entries" && !e && method === "GET") return json({ data: sessions.instructionEntries(id) })
        if (c === "instructions" && d === "entries" && e && (method === "PUT" || method === "DELETE")) {
          sessions.setInstruction(id, e, method === "DELETE" ? undefined : String((await body()).value ?? ""))
          return json({})
        }
        if (c === "wait") {
          while (sessions.active().includes(id)) await Bun.sleep(200)
          return json({})
        }
      }

      return error(404, "Not available for this agent.")
    } catch (caught) {
      if (caught instanceof UnsupportedFormatVersion) return Response.json({ code: "unsupportedFormatVersion", message: caught.message }, { status: 409 })
      if (/^\/api\/integration\/[^/]+\/connect\/oauth(?:\/|$)/.test(url.pathname)) {
        return error(502, "The sign-in couldn’t continue. Try again.")
      }
      if (caught instanceof NotFound) return error(404, caught.message)
      if (caught instanceof BadRequest) return error(400, caught.message)
      return error(500, caught instanceof Error ? caught.message : String(caught))
    }
  }

  return async (request: Request): Promise<Response> => {
    if (request.headers.get("authorization") !== expected) return error(401, "Unauthorized")
    const refusal = routeVersionError(request.headers.get(ROUTE_VERSION_HEADER) ?? undefined)
    if (refusal) return Response.json(refusal, { status: 400, headers: { [ROUTE_VERSION_HEADER]: ROUTE_VERSION } })
    if (transferring?.()) return new Response(null, { status: 503, headers: {
      [ROUTE_VERSION_HEADER]: ROUTE_VERSION, "retry-after": "1", "x-kuvo-bridge-handover": "1",
    } })
    const response = await answer(request)
    const headers = new Headers(response.headers)
    headers.set(ROUTE_VERSION_HEADER, ROUTE_VERSION)
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers })
  }
}
