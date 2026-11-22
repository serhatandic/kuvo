// The coding agents' update notices, as a client reads them (`kuvo/agents/api/kuvo/updates/notices`)
// and as the box tells them once to macOS and your phones (the bridge's `kuvo.updates.notice`
// event, which the box's link passes on). With "Update coding agents automatically" off: "X 2.0.0
// is available" as soon as a newer version is known, with Update where Kuvo Code can update it;
// on: "X updated to 2.0.0" once it moved on. One per version, across restarts, gone once updated
// or seen. Real: the bridge's routes, its notices file and the setting's file. Stand-ins: the
// agents' programs (their version and the newest, as npm or Homebrew would say).

import "../safety"
import { afterEach, describe, expect, test } from "bun:test"
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { Agent } from "../../src/agents/agent"
import { subscribe } from "../../src/agents/events"
import type { Maintenance } from "../../src/agents/maintenance"
import { handler } from "../../src/agents/server"
import { agentUpdatesFile } from "../../src/shared/agent-updates"
import { updateNotice } from "../../src/link/notices/news"

const folders: string[] = []
afterEach(() => {
  rmSync(agentUpdatesFile(process.env.KUVO_HOME!), { force: true })
  for (const folder of folders.splice(0)) rmSync(folder, { recursive: true, force: true })
})

/** The box's setting, as Settings › Computer saves it. */
function updateThemselves(on: boolean) {
  mkdirSync(join(process.env.KUVO_HOME!, "box"), { recursive: true })
  writeFileSync(agentUpdatesFile(process.env.KUVO_HOME!), JSON.stringify(on))
}

/** A stand-in agent's program: what's installed, the newest out, and whether Kuvo Code can update it. */
function program(kind: string, installed: string, latest: string, updatable = true) {
  const state = { installed, latest }
  const agent = {
    id: kind,
    providerID: `${kind}-cli`,
    name: `${kind} Agent`,
    instance: { id: "default" },
    methods: () => [],
    account: async () => ({ installed: true, signedIn: true, label: "me@example.com" }),
    models: async () => [],
    maintenance: async (): Promise<Maintenance> => ({ installed: state.installed, latest: state.latest, ...(updatable ? { command: [kind, "update"], shown: `${kind} update` } : { shown: undefined }) }),
    update: async () => ({ updated: false }),
  } as unknown as Agent
  return { state, agent }
}

/** A bridge on `home` (a restart is another on the same one), and every notice it tells. */
function bridge(agents: Agent[], home: string) {
  const answer = handler({ roster: { all: () => agents, find: (id: string) => agents.find((agent) => agent.providerID === id) }, password: "pw", programHome: home } as any)
  const ask = async (route: string, method = "GET") => {
    const response = await answer(new Request(`http://box/api/${route}`, { method, headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64") } }))
    return { status: response.status, value: (await response.json()) as any }
  }
  return {
    ask,
    /** Settings › Accounts read, as a client does: each program read, and its notices decided. */
    read: () => ask("integration"),
    notices: async () => (await ask("kuvo/updates/notices")).value.data.notices as any[],
  }
}

function told() {
  const notices: any[] = []
  const stop = subscribe((event) => {
    if (event.type === "kuvo.updates.notice") notices.push(event.data.notice)
  })
  return { notices, stop }
}

function home() {
  const folder = mkdtempSync(join(tmpdir(), "update-notices-"))
  folders.push(folder)
  return folder
}

describe("update notices", () => {
  test("off: a newer version is available, with Update, told once, and not again after a restart or once seen", async () => {
    updateThemselves(false)
    const folder = home()
    const codex = program("codex-n1", "1.0.0", "2.0.0")
    const heard = told()
    const box = bridge([codex.agent], folder)
    await box.read()
    await box.read()
    expect(await box.notices()).toEqual([
      { id: "codex-n1-available-2.0.0", program: "codex-n1", provider: "codex-n1-cli", kind: "available", version: "2.0.0", title: "codex-n1 Agent 2.0.0 is available", body: "You have 1.0.0.", at: expect.any(Number), action: { id: "update", label: "Update" } },
    ])
    // Told to macOS and your phones once, as the link shows it.
    expect(heard.notices.map((each) => updateNotice(each))).toEqual([
      { category: "update", title: "codex-n1 Agent 2.0.0 is available", body: "You have 1.0.0.", thread: "update:codex-n1", data: { notice: "codex-n1-available-2.0.0" } },
    ])

    // The bridge restarts: still served, not told again.
    const again = bridge([codex.agent], folder)
    await again.read()
    expect((await again.notices()).map((each) => each.id)).toEqual(["codex-n1-available-2.0.0"])
    expect(heard.notices).toHaveLength(1)

    // Seen on one client: gone for every client, and never raised again for 2.0.0.
    expect((await again.ask("kuvo/updates/notices/codex-n1-available-2.0.0", "DELETE")).status).toBe(200)
    await again.read()
    expect(await again.notices()).toEqual([])
    expect(heard.notices).toHaveLength(1)

    // A newer one is out: raised for it.
    codex.state.latest = "2.1.0"
    await again.read()
    expect((await again.notices()).map((each) => each.title)).toEqual(["codex-n1 Agent 2.1.0 is available"])
    expect(heard.notices.map((each) => each.version)).toEqual(["2.0.0", "2.1.0"])
    heard.stop()
  })

  test("off: once it's updated (here or in Terminal), its notice goes", async () => {
    updateThemselves(false)
    const grok = program("grok-n2", "1.0.0", "1.2.0")
    const box = bridge([grok.agent], home())
    await box.read()
    expect(await box.notices()).toHaveLength(1)
    grok.state.installed = "1.2.0"
    await box.read()
    expect(await box.notices()).toEqual([])
  })

  test("off: one Kuvo Code can't update says how to, with no Update", async () => {
    updateThemselves(false)
    const claude = program("claude-n3", "1.0.0", "1.1.0", false)
    const box = bridge([claude.agent], home())
    await box.read()
    const [notice] = await box.notices()
    expect(notice.title).toBe("claude-n3 Agent 1.1.0 is available")
    expect(notice.body).toBe("You have 1.0.0. Update claude-n3 Agent the way you installed it.")
    expect(notice.action).toBeUndefined()
  })

  test("on: nothing is raised for what's out; once it updated itself, it says so, once", async () => {
    updateThemselves(true)
    const codex = program("codex-n4", "1.0.0", "1.1.0")
    const heard = told()
    const box = bridge([codex.agent], home())
    await box.read()
    expect(await box.notices()).toEqual([])
    codex.state.installed = "1.1.0"
    await box.read()
    await box.read()
    const notices = await box.notices()
    expect(notices.map((each) => [each.title, each.body, each.action])).toEqual([["codex-n4 Agent updated to 1.1.0", "It was 1.0.0. It updates itself, as set in Settings › Computer.", undefined]])
    expect(heard.notices.map((each) => each.id)).toEqual(["codex-n4-updated-1.1.0"])
    await box.ask(`kuvo/updates/notices/${notices[0].id}`, "DELETE")
    expect(await box.notices()).toEqual([])
    heard.stop()
  })
})
