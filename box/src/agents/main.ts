// Kuvo Code's agents bridge: sessions with the coding agents you already have (Claude Code, Codex,
// Cursor, Grok, Antigravity), signed in as you, served to the app in the engine's own API.
//
// Started like the engine: it prints `{"url": …}` once it's listening, answers only with the
// password in OPENCODE_PASSWORD, and stops when its stdin closes (so it can't outlive the app).
// It keeps what it has in KUVO_AGENTS_HOME.
//
// On a box (KUVO_AGENTS_REMOTE, see machine.ts: a machine's, and a Mac's in a home of its own) its
// keeper keeps it running (KUVO_AGENTS_KEPT), with the engine's settings: it listens on
// KUVO_AGENTS_PORT with the password in KUVO_AGENTS_PASSWORD, and reaches the engine where its
// record says (`run/engine.json`), following it as the engine starts again: an engine's restart
// never ends it, nor a turn on it. (A launch from before the keeper kept it starts it beside its
// engine, with KUVO_ENGINE_PORT and OPENCODE_PASSWORD, which it takes until the record says more.)

import { boxZoneIs } from "../shared/zone"
import { existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs"
import { basename, dirname, join } from "node:path"
import { format } from "node:util"
import { Accounts, adoptOwnFolders, parseProvider } from "./accounts"
import { adoptConsoleFolder } from "./claude-compat"
import { Auto } from "./auto"
import { Deps } from "./deps"
import { EngineAgent, EngineLink } from "./engine"
import { subscribe } from "./events"
import type { SessionPlaces } from "./session-places"
import { SideQuestions } from "./btw"
import { setToolProgram } from "./kuvotools"
import { keepRelayKeys, relayPort, setToolRelayRun, usesToolRelay } from "./toolrelay"
import { dropCarriedSignIns, dropOwnPrograms, formerGroupsMemoryKey, groupsMemoryKey, machineEngine as recordedEngine, machineHome, onMachine, onMacBox, takeOwnFolders } from "./machine"
import { settingsLayers } from "./blocks"
import { Snapshots } from "./snapshots"
import { Roster } from "./roster"
import { forgetOldAntigravity } from "./agents/antigravity"
import { cancelSignIns, handler, signInsWaiting } from "./server"
import { updatesUnderWay } from "./updates"
import { launchLets, Renewal } from "./renewal"
import { keeperEngine } from "../shared/engines"
import { writeSecret } from "../shared/files"
import { leftSignIns } from "./signin"
import { commandLineOf } from "../shared/platform"
import { updateUnderWay } from "../shared/updating"
import { releaseHold, swapUnderWay, untilSwapLets } from "../shared/engine-swap"
import { spawn } from "node:child_process"
import { buildOf, bundleBuild } from "../shared/bundle"
import { bridgeRecordFormat, readRendezvous, type BridgeRecord, type RenewalRecord } from "../shared/records"
import { UnsupportedFormatVersion } from "../shared/format-version"
import { installedLinkBuild, isInstalledBundle, KUVO_FOLDER_LAYOUT, upgradeKuvoFolder } from "../shared/folder-version"
import { Sessions } from "./sessions"
import { adoptHeld, adoptTransfer, canTransfer, type HeldMeta, type HeldTurn, type TransferHolder } from "./hold/adopt"
import { controlledHolders, heldIn, heldSessions, holdFolder, HOLD_PROTO, releaseHolders, stopAllHeld, type Held } from "./hold/client"
import { runsAs } from "../shared/platform"
import { noteAgentsInside, untilSandboxDecided } from "./fence"
import { useSavedLoginPath } from "./proc"
import { setSkillSource } from "./skills"
import { setServerSource } from "./servers"
import { setSubagentSource } from "./subagents"
import { Carrying } from "./carry"
import { Store } from "./store"
import { server as requester } from "./groups/api"
import { SeatServer } from "./groups/peer"
import { LinkDirectory, PeerPost, type PeerParts } from "./groups/post"
import { GroupRunner } from "./groups/runner"
import { GroupStore } from "./groups/store"
import { queueFiles } from "../shared/queues"
import { notesBeside } from "./notes"
import { BoxServicesClient } from "./box-services"
import { servicePath } from "../shared/box-services"

import { commitTransfer, finishTransfer, prepareStandby, readTransfer, standBy, standbyFailed, TRANSFER_PROTOCOL, type Transfer } from "./bridge-transfer"

const version = "1"
// The build it runs, as its bundle names it (none from its sources): read as it starts, so a newer
// one put in its place is told from it (renewal.ts).
const script = Bun.main
const build = bundleBuild(script)
const transferLayout = `folder-layout=${KUVO_FOLDER_LAYOUT}`
const transferCapability = `${TRANSFER_PROTOCOL} ${transferLayout}`
// `selftest`: it loads and runs on this engine, and says which build it is (a bridge about to make
// way for it asks first, renewal.ts).
if (process.argv.slice(2).includes("selftest")) {
  const adopt = process.argv.includes("adopt-check") && process.argv.includes(transferLayout)
  const capability = process.argv.includes("holder-check") ? TRANSFER_PROTOCOL : adopt ? transferCapability : ""
  console.log(`ok ${build}${capability ? ` ${capability}` : ""}`)
  process.exit(0)
}
// Keep stdout's readiness record intact for launchers. Prefix each diagnostic line on stderr.
for (const level of ["warn", "error"] as const) {
  const write = console[level].bind(console)
  console[level] = (...args: unknown[]) => {
    const prefix = `${new Date().toISOString()} agents[${process.pid}]`
    write(format(...args).split("\n").map(line => `${prefix} ${line}`).join("\n"))
  }
}
const home = process.env.KUVO_AGENTS_HOME
const password = onMachine ? process.env.KUVO_AGENTS_PASSWORD : process.env.OPENCODE_PASSWORD
// Kept by its box's keeper, which starts it again whenever it ends (rather than a launch beside one
// engine): it may always make way for a newer build (renewal.ts).
const kept = process.env.KUVO_AGENTS_KEPT === "1"
// Whether it runs on one of its box's own engines (`<box>/engine/<version>/opencode`).
const ownRuntime = (() => {
  try {
    return !!machineHome && realpathSync(dirname(dirname(process.execPath))) === realpathSync(join(machineHome, "engine"))
  } catch {
    return false
  }
})()
if (!home || !password || (onMachine && (!machineHome || !Number(process.env.KUVO_AGENTS_PORT)))) {
  console.error(onMachine ? "KUVO_AGENTS_HOME, KUVO_HOME, KUVO_AGENTS_PORT and KUVO_AGENTS_PASSWORD are needed." : "KUVO_AGENTS_HOME and OPENCODE_PASSWORD are needed.")
  process.exit(2)
}
if (machineHome) {
  const folder = machineHome
  try {
    upgradeKuvoFolder(folder, undefined, () => {
      if (kept && isInstalledBundle(folder, script, "kuvo-agents.js")
        && existsSync(join(folder, "bin", "kuvo-link.js")) && installedLinkBuild(folder)?.layout !== KUVO_FOLDER_LAYOUT) {
        throw new Error("The installed Box link must open its folder before the agents bridge.")
      }
    })
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exit(1)
  }
}
const bridgeRun = machineHome ? join(machineHome, "run") : undefined
const watchingScript = kept && machineHome ? join(machineHome, "bin", "kuvo-agents.js") : script
const standby = kept && bridgeRun && process.argv.includes("handover-standby") ? await standBy(bridgeRun, build) : undefined
const recovered = kept && bridgeRun && process.argv.includes("handover-recovery") ? readTransfer(bridgeRun) : undefined
if (recovered && (recovered.from !== build || !standbyFailed(recovered))) process.exit(1)
const transfer = standby?.transfer ?? recovered
const launchEnv = kept ? { ...process.env } : undefined
// The installation replaces the pathname atomically. Keep this exact build for a failed takeover.
const original = kept && build ? readFileSync(script) : undefined
const hasOriginal = original && buildOf(original.toString("utf8")) === build

// On a box: its engine, where its record says now (or where a launch from before said), and Kuvo
// Code's tools as the engine has them.
const machineEngine = onMachine ? recordedEngine() ?? (process.env.KUVO_ENGINE_PORT ? { pid: Number(process.env.KUVO_ENGINE_PID) || undefined, url: `http://127.0.0.1:${process.env.KUVO_ENGINE_PORT}`, password: process.env.OPENCODE_PASSWORD } : undefined) : undefined
const port = onMachine ? Number(process.env.KUVO_AGENTS_PORT) : 0
if (onMachine) {
  setToolProgram(process.env.OPENCODE_CONFIG_CONTENT)
  // The bridge and its agents keep their folders in Kuvo Code's (XDG_… there); the commands the
  // agents run get the machine's own back (machine.ts).
  takeOwnFolders()
  // COMPAT(0.7 agents-runtime): sign-ins an older Mac carried over (`.from-mac`). Goes 3 months after 0.8.0.
  // Each machine signs its agents in on its own: none a Mac carried over before stays.
  if (home) dropCarriedSignIns(home)
  // Kuvo Code's commands (kuvo-wake) first, as for the engine, and where bots' and sessions'
  // wake-ups are kept.
  process.env.KUVO_BIN = join(machineHome!, "bin")
  process.env.KUVO_BOTS ??= join(machineHome!, "bots")
  process.env.KUVO_WAKES ??= join(machineHome!, "wakes")
  // Nor do they get what the engine starts with: its settings, with keys in them, nor these.
  for (const name of ["OPENCODE_CONFIG_CONTENT", "OPENCODE_CONFIG", "OPENCODE_CLIENT", "OPENCODE_PASSWORD", "KUVO_AGENTS_PASSWORD", "KUVO_AGENTS_PORT", "KUVO_ENGINE_PORT", "KUVO_ENGINE_PID", "KUVO_AGENTS_KEPT"]) delete process.env[name]
}
// A test's: how long it waits for a quiet moment before it holds new turns for a newer bridge.
const renewHoldAfter = Number(process.env.KUVO_TEST_RENEW_HOLD) || undefined
// And how long nothing must run before it goes, and how long it must have run.
const renewSettle = process.env.KUVO_TEST_RENEW_SETTLE ? Number(process.env.KUVO_TEST_RENEW_SETTLE) : undefined
const renewUptime = process.env.KUVO_TEST_RENEW_UPTIME ? Number(process.env.KUVO_TEST_RENEW_UPTIME) : undefined
for (const name of ["KUVO_TEST_RENEW_HOLD", "KUVO_TEST_RENEW_SETTLE", "KUVO_TEST_RENEW_UPTIME"]) delete process.env[name]
// Temporary files of its own and the agents' (Claude Code's task output): the app gives it a
// folder in its own temporary folder.
const temporary = process.env.TMPDIR?.replace(/\/$/, "") || join(home, "tmp")
mkdirSync(temporary, { recursive: true })
// What an agent left unpacked when the bridge last stopped without cleaning up (Antigravity's
// agent unpacks itself, about 1 GB, for each run), and half-finished downloads.
// (But for those of agents still held, which this bridge takes up below.)
const heldRuns = new Set(heldIn((machineHome && holdFolder(machineHome)) || join(temporary, "none")).map(record => record.meta.temporary))
try {
  for (const name of readdirSync(join(temporary, "antigravity"))) {
    if (!heldRuns.has(join(temporary, "antigravity", name))) rmSync(join(temporary, "antigravity", name), { recursive: true, force: true })
  }
} catch {}
// And the page of a sign-in one was in the middle of.
leftSignIns(temporary)
try {
  for (const name of readdirSync(join(home, "antigravity"))) if (name.startsWith(".install-")) rmSync(join(home, "antigravity", name), { recursive: true, force: true })
} catch {}

// The engine's binary runs this as Bun when told to: the agents it starts aren't told so.
delete process.env.BUN_BE_BUN
// A wake-up an agent's command schedules (kuvo-wake) comes back to this bridge's session.
process.env.KUVO_WAKE_VIA = "bridge"
// Nor do they get its password: every agent, and each command one runs, gets this environment,
// and an `env` there would print it, for the model to read and any process here to use.
delete process.env.OPENCODE_PASSWORD

// An SDK's promise nobody waits for, rejected (Cursor's when a shell fails to start), stops nothing.
process.on("unhandledRejection", (reason) => console.error("unhandled:", reason))
// Anything else: said, and it stops (on a machine, it's started again).
process.on("uncaughtException", (error) => {
  console.error("uncaught:", error)
  process.exit(1)
})

const deps = new Deps(join(home, "runtime"), join(home, "cache"))
if (onMachine) {
  // COMPAT(0.7 agents-runtime): the programs an older bridge installed in `agents/runtime`. Goes 3 months after 0.8.0.
  // The agents' programs are the machine's own now, found on its login shell's PATH (which a
  // command over SSH doesn't get), not copies of Kuvo Code's.
  dropOwnPrograms(join(home, "runtime"))
  useSavedLoginPath()
}
// COMPAT(0.7 agents-runtime): `cache/install`. Goes 3 months after 0.8.0.
// What an earlier bridge left of its downloads (it kept them on this Mac): nothing installs yet.
rmSync(join(home, "cache", "install"), { recursive: true, force: true })
const context = { home, temporary, deps }
// Each agent's first account is its own folder now, on Linux too: an earlier bridge's moved there.
const accounts = new Accounts(home)
adoptOwnFolders(home, accounts)
// And on a Mac, an earlier version's Anthropic Console sign-in becomes an account you added.
if (process.platform === "darwin") adoptConsoleFolder(home, accounts)
const roster = new Roster(accounts, context)
const store = new Store(join(home, "sessions"), Boolean(transfer), machineHome && join(machineHome, "attachments"))
const auto = new Auto(home)
// The engine's own models, run in sessions here (the app says where the engine is).
const link = new EngineLink()
// Blocks from the files the engine reads, while it's parked (only coding agents at work) or starting
// again, as its start names them (kuvo-launch): its settings folder, and a machine's own settings.
if (onMachine && machineHome) link.settingsLayers = (directory) => settingsLayers(directory, join(machineHome!, "config"), onMacBox ? undefined : machineHome)
else if (process.env.KUVO_ENGINE_CONFIG) link.settingsLayers = (directory) => settingsLayers(directory, process.env.KUVO_ENGINE_CONFIG)
if (machineEngine) link.configure(machineEngine.url, machineEngine.password, machineEngine.pid)
// Its engine started again (a crash, a hang ended, an update's new engine): followed where its
// record says now, every two seconds, its turns going on meanwhile (`EngineLink` reconnects).
if (onMachine) {
  setInterval(() => {
    const now = recordedEngine()
    if (now) link.configure(now.url, now.password, now.pid)
  }, 2_000).unref?.()
}
// The skills Settings › Skills lists, for the agents too (the engine lists them).
setSkillSource(link, home)
setServerSource(link)
setSubagentSource(link, home)
const engineAgents = new Map<string, EngineAgent>()
function engineAgent(providerID: string): EngineAgent | undefined {
  if (!link.ready || parseProvider(providerID)) return undefined
  let agent = engineAgents.get(providerID)
  if (!agent) engineAgents.set(providerID, (agent = new EngineAgent(link, providerID)))
  return agent
}
// Agents' processes an earlier bridge left under holders (it crashed): those with turns under way
// are taken up again, the others stopped as it would have stopped them (hold/adopt.ts).
// (Not while another bridge runs here, which has them: this one won't get its port.)
const otherBridge = (() => {
  try {
    const pid = readRendezvous<BridgeRecord>(bridgeRecordFormat, join(machineHome!, "run", "bridge.json")).pid
    return pid !== process.pid && runsAs(pid, /kuvo-agents/)
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error
    return false
  }
})()
const heldTurns = transfer ? await adoptTransfer(machineHome!, transfer.holders, Boolean(recovered)) : new Map<string, HeldTurn>()
// Every other agent held here: the one before let go of them all (or crashed).
const adopted = onMachine && (transfer || !otherBridge) ? await adoptHeld(undefined, new Set(transfer?.holders.map(holder => holder.id))) : { turns: new Map<string, HeldTurn>(), idle: [] }
for (const [session, turn] of adopted.turns) heldTurns.set(session, turn)
// On a box, the tools relay, at once where it was, with only the keys of the sessions taken up: their
// agents reach their tools as before.
if (onMachine && machineHome) {
  relayPort()
  keepRelayKeys(heldSessions())
}
if (standby && Object.keys(standby.transfer.sessions).some(id => !heldTurns.has(id))) throw new Error("A session is missing from the handover.")
let holdingRelease = false
const releaseHolding = () => {
  const holding = Boolean(bridgeRun && releaseHold(bridgeRun))
  if (holding) holdingRelease = true
  return holding
}
const sessions = new Sessions(
  store,
  roster,
  auto,
  engineAgent,
  (directory, fresh) => link.policies(directory, fresh),
  (name, directory) => link.agent(name, directory),
  new Snapshots(join(home, "snapshots")),
  (directory) => link.titleModel(directory),
  heldTurns,
  transfer?.sessions,
  home,
  releaseHolding,
)
// Tells the box's link, as it changes, whether every agent's process here runs in its sandbox
// (none started before it came on still runs): only then do agents use the vault.
// And restarts those you said to (`kuvo/sandbox/restart`): each between turns now, the rest as
// their turns end, looked at again with the note.
let restarting = false
const noteInside = () => {
  if (restarting) restarting = [sessions.restartOutside(), ...roster.all().map((agent) => agent.restartOutside?.() ?? false)].some(Boolean)
  noteAgentsInside(!sessions.outside() && !roster.all().some((agent) => agent.outside?.()))
}
const restartOutside = () => {
  console.error("Sandbox restart requested.")
  restarting = true
  noteInside()
}
noteInside()
setInterval(noteInside, 5_000).unref?.()

// The group chats that live on this computer, run here: on a machine, in its Kuvo Code folder; on
// the Mac, where the app says (KUVO_GROUPS, with its bots in KUVO_BOTS and its trash in
// KUVO_TRASH). One that fails to start leaves the rest of the bridge as it is.
let self: ReturnType<typeof requester> | undefined
// The record of what agents made, told of each chat's part given to a bot (set once it starts, below).
let groups: { runner: GroupRunner; store: GroupStore; peers: PeerParts } | undefined
const groupFolders = onMachine
  ? { root: join(machineHome!, "groups"), bots: process.env.KUVO_BOTS ?? join(machineHome!, "bots"), trash: join(machineHome!, "trash") }
  : process.env.KUVO_GROUPS && process.env.KUVO_BOTS && process.env.KUVO_TRASH
    ? { root: process.env.KUVO_GROUPS, bots: process.env.KUVO_BOTS, trash: process.env.KUVO_TRASH }
    : undefined
const groupMemoryKey = groupsMemoryKey
// What this bridge tells your other computers (its group chats' mail, its plan limits), through the
// box's link, and who they are, as the link says.
const post = new PeerPost(groupFolders ? join(groupFolders.root, "peer-outbox.json") : undefined)
const directory = new LinkDirectory(post)
const peers: PeerParts = { post, directory }
// Who there is to name with "&" in a chat: the agents the box's link lists (on the Mac, where the app says).
const agentsFile = onMachine ? join(machineHome!, "run", "agents.json") : process.env.KUVO_AGENTS_FILE || undefined
// The box's queues, where a chat's part for a conversation you stopped waits (on the Mac, the box's run folder the app says).
const runFolder = onMachine ? join(machineHome!, "run") : process.env.KUVO_RUN || undefined
setToolRelayRun(runFolder)
const boxServices = new BoxServicesClient(runFolder)
// Sessions' turns held with the box's link, in its run folder.
notesBeside(runFolder)
// What would start a turn on the engine waits while the engine is being changed (PROTOCOL.md, "A
// new engine"), bounded as that hold is: it goes to the new one.
if (runFolder) {
  // A bridge turn admitted before the release hold must be free to finish, including its next
  // Auto model. New bridge turns wait in Sessions, and other engine sends wait here.
  link.beforeTurn = (alreadyAdmitted) => untilSwapLets(runFolder, 1000, !alreadyAdmitted)
  setInterval(() => {
    if (holdingRelease && !releaseHolding()) {
      holdingRelease = false
      sessions.wakeHeld()
    }
  }, 1000).unref()
}
// Nor do the agents get these.
for (const name of ["KUVO_GROUPS", "KUVO_TRASH", "KUVO_GROUPS_MEMORY_KEY", "KUVO_AGENTS_FILE", "KUVO_ARTIFACTS", "KUVO_RUN", ...(onMachine ? [] : ["KUVO_BOTS"])]) delete process.env[name]
if (groupFolders) {
  try {
    const store = new GroupStore(groupFolders.root, groupFolders.bots, agentsFile)
    // Your zone as the box last gave it, for this bridge's clocks until it gives it again.
    boxZoneIs(store.settings()?.boxTimeZone)
    // Its bots in chats on your other computers, and theirs in chats here, through the box's link.
    const runner = new GroupRunner({
      store,
      engine: link,
      bridge: () => self,
      trash: groupFolders.trash,
      listen: (session, listener) => link.listen(session, listener),
      place: onMachine ? "this machine" : undefined,
      memoryKey: groupMemoryKey,
      formerMemoryKey: formerGroupsMemoryKey,
      // A coding agent's own account, as Settings › Accounts shows it: one with a way to sign in, not signed in.
      agentAccount: async (provider) => {
        const agent = roster.find(provider)
        if (!agent) return undefined
        const account = await agent.account()
        return { name: agent.name, signedOut: !account.signedIn && agent.methods().length > 0 }
      },
      peers: directory,
      given: (prompt, part) => boxServices.tell("api/kuvo/artifacts/given", { prompt, chat: part.chat }),
      queues: runFolder ? queueFiles(runFolder) : undefined,
    })
    const server = new SeatServer(runner.seats, post, { store, remembered: (from, memory) => runner.remembered(from, memory) })
    peers.server = server
    peers.runner = runner
    groups = { store, runner, peers }
  } catch (error) {
    console.error("group chats:", error)
  }
}

const places: SessionPlaces = {
  engine: {
    api: link,
    listen: (session, told) => link.listen(session, { event: (event) => told.event(event.type, event.data), reconnected: told.reconnected }),
  },
  bridge: {
    api: { request: (...args) => (self ? self.request(...args) : Promise.reject(new Error("The bridge isn't listening yet."))) },
    listen: (session, told) => subscribe((event) => event.data?.sessionID === session && told.event(event.type, event.data)),
  },
  isBridge: (session) => {
    try {
      sessions.get(session)
      return true
    } catch {
      return false
    }
  },
}
const sideQuestions = new SideQuestions(join(home, "side-questions.json"), places)
const usageFollow = new AbortController()
roster.observeLimits(agent => {
  if (agent.limits) boxServices.observeTurns(agent.providerID, agent.limits)
})
store.origin = () => boxServices.attribution().computer ?? directory.own()?.key ?? ""
store.usageAccount = provider => boxServices.attribution().accounts?.[provider]
// Nothing starts a coding agent before it listens (an app's first ask, a group chat), and it
// listens once the box has set up its sandbox: on this Mac the app starts it before the box, so an
// agent started then would run outside the sandbox until it stops.
await untilSandboxDecided()
// The turns it left under way go on here, before anything can be sent to their sessions; and the
// agents it left between turns are their sessions' again, as they are. (One no session here takes
// up, as for a session deleted meanwhile, is stopped.)
sessions.resumeHeld()
for (const { held, meta } of adopted.idle) if (!sessions.adoptIdle(held, meta)) held.stop()
const gates = [...new Set([...heldTurns.values()].map(turn => turn.gate))]
if (transfer) {
  let deadline: ReturnType<typeof setTimeout> | undefined
  try {
    const timeout = new Promise<never>((_, reject) => {
      deadline = setTimeout(() => reject(new Error("Timed out restoring the held sessions.")), 45_000)
    })
    await Promise.race([timeout, Promise.all(gates.map(async gate => {
      const ready = await Promise.race([gate.readyToOwn, gate.held.closed.then(() => false)])
      if (!ready || !gate.held.isOpen) throw new Error("An adapter could not restore its held sessions.")
    }))])
    if (gates.some(gate => gate.failed || !gate.held.isOpen)) throw new Error("A holder disconnected during startup.")
  } catch (error) {
    for (const gate of gates) gate.held.detach()
    console.error("Bridge handover:", error)
    process.exit(1)
  } finally {
    clearTimeout(deadline)
  }
}
const carrying = new Carrying(sessions, store, (provider) => sessions.agent(provider), (kind) => roster.all().filter((agent) => parseProvider(agent.providerID)?.kind === kind))
let server: ReturnType<typeof Bun.serve>
/** Making way for a newer bridge in its place, on a machine (below). */
let renewal: Renewal | undefined
let transferring = false
const answer = handler({ roster, sessions, auto, link, password, programHome: home, version, build, groups, peers, sideQuestions, places, restartOutside, carrying, boxServices, refreshAccounts: true }, () => transferring)
/** Requests being answered now (but the event feed's, which stays open): a newer bridge waits for them. */
let answering = 0
setInterval(() => {
  if (!answering) sessions.pruneTranscripts()
}, 30_000).unref()
const serveRequest = (request: Request) => {
  if (transferring) return answer(request)
  if (new URL(request.url).pathname === "/api/event" || servicePath(new URL(request.url).pathname)) return answer(request)
  answering++
  if (request.method !== "GET" && request.method !== "HEAD") renewal?.busied()
  return answer(request).finally(() => {
    if (--answering === 0) sessions.pruneTranscripts()
  })
}
try {
  if (standby) {
    server = standby.server
    server.reload({ fetch: serveRequest })
  } else server = Bun.serve({ hostname: "127.0.0.1", port, idleTimeout: 0, fetch: serveRequest })
} catch (error: any) {
  console.error("couldn't listen:", error?.message ?? error)
  // On a machine, its port taken: the launch gives it another.
  process.exit(onMachine && /in use|EADDRINUSE/i.test(String(error?.code ?? error?.message ?? error)) ? 98 : 1)
}
// The standalone Mac app writes bridge.json itself and does not yet add capability fields.
if (!onMachine && runFolder) {
  try { writeSecret(join(runFolder, "bridge-box-services.pid"), String(process.pid)) } catch {}
}
// On the box the readiness record is kept with the diagnostics. Local launchers receive only the URL.
console.log(JSON.stringify({
  url: `http://127.0.0.1:${server.port}`,
  ...(onMachine ? { at: new Date().toISOString(), pid: process.pid, build, mode: standby ? "transfer" : process.argv.includes("handover-recovery") ? "recovery" : "normal" } : {}),
}))
void boxServices.follow(usageFollow.signal)
self = requester(`http://127.0.0.1:${server.port}`, password)
// Once the engine's there (the app says where, a moment after this starts on the Mac): parts under
// way are followed again, and meetings held.
void link.whenReady().then(() => {
  // Sessions whose background work ended as the bridge last stopped hear so now, and what the
  // bridge before this one held for it goes.
  sessions.wakeLost()
  sessions.wakeHeld()
  sideQuestions.start()
  try {
    groups?.runner.start()
    groups?.peers.server?.start()
    // What bots are still to be told of their chats, now and each minute, as your other computers' bots become known.
    if (groups) {
      const runner = groups.runner
      // COMPAT(0.7 mac-group-chats): `pending-memory.json` from `GroupMigration`. Goes 3 months after 0.8.0.
      void runner.catchUpMemory().catch((error) => console.error("group memory:", error))
      setInterval(() => void runner.catchUpMemory().catch(() => {}), 60_000).unref?.()
    }
  } catch (error) {
    console.error("group chats:", error)
  }
})
// On a machine, where it's reached, for kuvo-wake and Kuvo Code's tools there (as the engine's
// launch writes engine.json): only this user can read it, and it goes when the bridge does. With the
// build it runs, and how it makes way for a newer one put in its place (renewal.ts), for the box to
// say (`kuvo/info`'s `pieces`).
const record = onMachine ? join(machineHome!, "run", "bridge.json") : undefined
const machineHandover = process.platform === "darwin" && kept && onMachine && Boolean(build)
const keeperHandover = () => {
  if (!machineHandover || !runFolder) return false
  const swap = swapUnderWay(runFolder)
  if (swap?.machineHelper !== true || swap.stage !== "swapping") return false
  try {
    return Number(readFileSync(join(runFolder, ".machine-handover.pid"), "utf8")) === swap.pid
  } catch {
    return false
  }
}
const writeRecord = (renewal: RenewalRecord | null, first = false) => {
  if (!record) return
  try {
    try { readRendezvous(bridgeRecordFormat, record) } catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error
    }
    writeSecret(record, JSON.stringify(bridgeRecordFormat.write({ pid: process.pid, port: server.port, password, build, renewal, boxServices: true, ...(machineHandover ? { machineHandover: true } : {}) })))
  } catch (error) {
    if (first) throw error
    if (error instanceof UnsupportedFormatVersion) console.error(error.message)
  }
}
writeRecord(null, true)
if (kept && runFolder) {
  // This folder has crossed the one-time restart from pre-holder bridges.
  try { writeSecret(join(runFolder, "agents-live-handover"), TRANSFER_PROTOCOL) } catch {}
}
// Back after a hand-over (a bridge before this one made way on purpose): no longer "restarting".
if (onMachine) rmSync(join(machineHome!, "run", "agents-renewing"), { force: true })
if (transfer) {
  finishTransfer(bridgeRun!)
  store.commit()
  for (const gate of gates) {
    gate.commit()
    gate.held.unpin()
  }
  sessions.reconcilePermissions()
}


const cleanAntigravity = () => {
  try { forgetOldAntigravity(home) } catch (error) { console.error("Antigravity versions:", error) }
}
cleanAntigravity()
setInterval(cleanAntigravity, 60_000).unref?.()

// A newer bridge put in its place on a machine (a Mac that connected brought it): it makes way at
// the first quiet moment, never cutting a turn, and the machine's launch starts the new one, which
// starts what waited for it (renewal.ts). (This Mac's own bridge takes a new version with its app,
// whose keeper starts the box again once nothing runs.)
/** Whether its launch starts it again, as last worked out (`lets`, below). */
let launchSaid: { at: number; lets: boolean } | undefined
const wholeWork = () => {
  const here = sessions.work()
  const outsideTurns = answering > 0 || (groups?.runner.busy() ?? false) || updatesUnderWay()
  const other = here.other || sideQuestions.busy() || signInsWaiting()
  return { turns: here.turns || outsideTurns, other, outsideTurns }
}
const transferBuilds = new Set<string>()
let retryTransfer = 0

function liveTransfer(newer: string): Promise<void> | undefined {
  if (!bridgeRun || !build || !hasOriginal || !launchEnv || !transferBuilds.has(newer) ||
      newer === recovered?.to || Date.now() < retryTransfer) return
  const plan = () => {
    const work = wholeWork()
    const controllers = controlledHolders()
    if (work.outsideTurns || work.other || !controllers) return
    const providers = new Map<string, string>()
    const holders: TransferHolder[] = []
    for (const held of controllers) {
      const { turns, kind, provider, version, sdk } = held.meta as Partial<HeldMeta>
      if (!turns || !Object.keys(turns).length) continue
      // An agent's sandbox names this bridge's relay port. An issued key can also refer to
      // a live MCP connection, which has to stay with its original bridge until the turn ends.
      if (!kind || !provider || held.meta.sandboxed === true ||
          (held.meta.fenced === true && kind !== "cursor" && kind !== "antigravity")) return
      for (const note of Object.values(turns)) {
        if (providers.has(note.session) || usesToolRelay(note.session) ||
            (kind === "codex" && !note.turn) ||
            (kind === "cursor" && (!note.turn || !Number.isSafeInteger(Number(note.turn)))) ||
            ((kind === "cursor" || kind === "antigravity") && note.request === undefined)) return
        providers.set(note.session, provider)
      }
      holders.push({ id: held.id, generation: held.generation, proto: HOLD_PROTO, provider, kind, turns,
        version: typeof version === "string" ? version : undefined, sdk: typeof sdk === "string" ? sdk : undefined, acked: held.seen })
    }
    if (!canTransfer(holders)) return
    const active = sessions.transfer(providers)
    if (!active) return
    return { formatVersion: 1, from: build, to: newer, old: process.pid, next: 0, holders, sessions: active } satisfies Transfer
  }
  const first = plan()
  if (!first) return
  return (async () => {
    const engine = keeperEngine(machineHome!)
    const runtime = engine ? join(machineHome!, "engine", engine, "opencode") : process.execPath
    const candidate = await prepareStandby(runtime, watchingScript, launchEnv, first)
    if (!candidate) {
      retryTransfer = Date.now() + 60_000
      return
    }
    const wasHeld = renewal?.state?.holding === true
    transferring = true
    sessions.hold(true)
    let committed = false
    const pinned = new Set<Held>()
    try {
      const cut = plan()
      if (!cut || !sameTurns(cut, first)) return
      const held = new Map(controlledHolders()?.map(controller => [controller.id, controller]))
      const ready = await Promise.all(cut.holders.map(async holder => {
        const controller = held.get(holder.id)
        if (!controller) return false
        pinned.add(controller)
        return controller.prepare(Math.min(...Object.values(holder.turns).map(turn => turn.from)))
      }))
      const final = plan()
      if (ready.includes(false) || !final || !sameTurns(final, first) || bundleBuild(watchingScript) !== newer ||
          updateUnderWay(bridgeRun) || keeperHandover() || candidate.exitCode !== null || !candidate.connected) return
      final.next = candidate.pid!
      store.flushForTransfer(Object.keys(final.sessions))
      commitTransfer(bridgeRun, final, original!)
      committed = true
      console.error("Bridge transfer committed", { build, nextBuild: newer, next: final.next, holders: final.holders.length })
      await releaseHolders()
      server.stop(true)
      process.exit(0)
    } finally {
      if (!committed) {
        for (const holder of pinned) holder.unpin()
        candidate.kill()
        transferring = false
        if (!wasHeld) sessions.hold(false)
        retryTransfer = Date.now() + 30_000
      }
    }
  })()
}

function sameTurns(a: Transfer, b: Transfer) {
  const turns = (plan: Transfer) => plan.holders.map(({ id, generation, turns }) => ({ id, generation, turns }))
  return JSON.stringify(turns(a)) === JSON.stringify(turns(b))
}

renewal =
  onMachine && build
    ? new Renewal({
        script: watchingScript,
        build,
        work: wholeWork,
        transfer: kept ? liveTransfer : undefined,
        drainNow: (next) => kept && transferBuilds.has(next),
        hold: (on) => sessions.hold(on),
        renew: () => {
          const moving = keeperHandover()
          console.error(moving ? "Kuvo Helper will start the next bridge." : "A newer bridge is in place: this one makes way for it.")
          stop(moving ? "machine helper handover" : "bundle renewal", true, !moving)
        },
        told: (state) => writeRecord(state),
        keeperHandover,
        paused: () => updateUnderWay(join(machineHome!, "run")),
        // Only on the engine its keeper starts: one a Mac brought with a newer engine it kept from
        // starting (sessions were at work) waits for that engine, which starts it.
        // (One its keeper keeps is started again on the engine the keeper starts, whichever it runs on.)
        ready: () => {
          if (kept) return true
          const engine = keeperEngine(machineHome!)
          return !engine || engine === basename(dirname(process.execPath))
        },
        // Only while the launch keeping it will start the new one (renewal.ts, `launchLets`).
        // (Its parent's command line and its launch's log, read once a minute at most.)
        lets: () => {
          if (kept) return true
          const now = Date.now()
          if (launchSaid && now - launchSaid.at < 60_000) return launchSaid.lets
          const lets = launchLets(commandLineOf(process.ppid), () => {
            try {
              return readFileSync(join(machineHome!, "run", "agents.log"), "utf8")
            } catch {
              return undefined
            }
          })
          launchSaid = { at: now, lets }
          return lets
        },
        uptime: () => performance.now(),
        longEnough: renewUptime ?? (kept ? 0 : undefined),
        // Only for a bundle that loads and runs on this engine.
        check: (newer) =>
          new Promise<boolean | undefined>((done) => {
            const env: Record<string, string> = { BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", PATH: process.env.PATH ?? "/usr/bin:/bin" }
            // (On the engine its keeper starts it on, which may be newer than the one it runs on.)
            const engine = kept ? keeperEngine(machineHome!) : null
            const runtime = engine ? join(machineHome!, "engine", engine, "opencode") : process.execPath
            const checking = spawn(runtime, ["--config=/dev/null", "--no-env-file", watchingScript, "selftest", "adopt-check", transferLayout], { env, stdio: ["ignore", "pipe", "ignore"] })
            let said = ""
            checking.stdout.on("data", (chunk) => (said += chunk))
            let late = false
            const timer = setTimeout(() => ((late = true), checking.kill("SIGKILL")), 60_000)
            // (One that couldn't start, or ran out of time, says nothing of the bundle: asked again.)
            checking.on("error", () => (clearTimeout(timer), done(undefined)))
            checking.on("close", (code, signal) => {
              clearTimeout(timer)
              // (Ended by a signal, out of memory say, or out of time: nothing said of the bundle.)
              // (Making way for its engine: the bundle in place, its own build, runs on that one.)
              const expected = `ok ${newer.startsWith("engine ") ? bundleBuild(watchingScript) : newer}`
              // The previous executable must be able to reopen the folder if the candidate dies.
              // An older self-test without a layout can still lead to a quiet restart.
              const reply = said.trim()
              const supportsTransfer = reply === `${expected} ${transferCapability}`
              if (supportsTransfer && code === 0) transferBuilds.add(newer)
              done(late || signal ? undefined : code === 0 && (supportsTransfer || reply === expected || reply === `${expected} ${TRANSFER_PROTOCOL}`))
            })
          }),
        // (One its keeper keeps, on another engine than the keeper's now: an update brought a new one.)
        // (Only where it runs on one of the box's own engines, `engine/<version>/opencode`.)
        engineBehind: () => {
          if (!kept || !ownRuntime) return undefined
          const engine = keeperEngine(machineHome!)
          return engine && engine !== basename(dirname(process.execPath)) ? engine : undefined
        },
        holdAfter: renewHoldAfter,
        settle: renewSettle,
      })
    : undefined
renewal?.start()
// The moment a turn ends may be the quiet one: a quiet moment counts from it.
const renewing = renewal
if (renewing) subscribe((event) => {
  if (event.type !== "session.idle") return
  renewing.busied()
  renewing.look()
})

let stopping = false
/** Ends this bridge. `handOver`: it makes way on purpose (Renewal), and on a box its keeper keeps it,
 * so it says so (`run/agents-renewing`, for the link to answer "restarting" and let requests wait),
 * and starts the next one itself, once its own store is written, rather than leaving it to the
 * keeper's next round (PROTOCOL.md, "The coding agents' state"). */
function stop(reason: string, handOver = false, startReplacement = true) {
  if (stopping) return
  stopping = true
  console.error("Stopping bridge", { reason, build })
  usageFollow.abort()
  const run = onMachine ? join(machineHome!, "run") : undefined
  if (handOver && run) {
    try {
      writeSecret(join(run, "agents-renewing"), JSON.stringify({ at: Date.now(), pid: process.pid }))
    } catch {}
  }
  sideQuestions.stopAll()
  cancelSignIns()
  // (Unless a newer bridge has put its own there.)
  try {
    if (record && readRendezvous<BridgeRecord>(bridgeRecordFormat, record).pid === process.pid) rmSync(record, { force: true })
  } catch {}
  const end = () => {
    server.stop(true)
    if (handOver && run && kept && startReplacement) startNext(run)
    process.exit(0)
  }
  // Making way: every agent it holds goes on, idle or at work, for the next bridge to take up, and
  // it ends once each holder has heard.
  if (handOver) return void sessions.makeWay().finally(end)
  sessions.closeAll()
  // The box stopping: every agent it holds stops with it (an app-server no session closes too).
  stopAllHeld()
  store.flush()
  end()
}

/** The next bridge, started through its keeper's own command (`kuvo-start bridge`), once this one's
 * number is no longer the one it keeps (`run/agents.pid`): in a session of its own, so this one's
 * going doesn't end it. Should it not start, the keeper's next round does. */
function startNext(run: string) {
  try {
    // (The keeper's folder is HOME/.kuvo-code: one laid out otherwise is left to its keeper.)
    if (basename(machineHome!) !== ".kuvo-code") return
    if (Number(readFileSync(join(run, "agents.pid"), "utf8").trim()) !== process.pid) return
    rmSync(join(run, "agents.pid"), { force: true })
    const start = join(machineHome!, "bin", "kuvo-start")
    const env = { HOME: dirname(machineHome!), PATH: process.env.PATH ?? "/usr/bin:/bin" }
    spawn("sh", [start, "bridge"], { detached: true, stdio: "ignore", env, cwd: machineHome! }).unref()
  } catch (error) {
    console.error("couldn't start the next bridge:", error)
  }
}
if (!onMachine) {
  process.stdin.on("end", () => stop("stdin ended"))
  process.stdin.on("close", () => stop("stdin closed"))
  process.stdin.resume()
}
// A release switch (its keeper starts the next bridge): it makes way, as for a newer build.
process.on("SIGUSR2", () => {
  if (onMachine) stop("release switch", true, false)
})
process.on("SIGTERM", () => stop("SIGTERM"))
process.on("SIGINT", () => stop("SIGINT"))
