import "../safety"
import { peerRoutes as groupPeerRoutes } from "../../src/agents/groups/post"
import { usagePeerRoutes as peerRoutes, type UsagePeerParts as PeerParts } from "../../src/usage/peer-routes"
// Your plans' limits across your computers (PROTOCOL.md, "Plan limits", "Across your computers"):
// each box tells its peers what it read, an account a peer read lately isn't read again, and every
// box lists every account across them, one row each, with the computers it's signed in on. Here,
// three in-memory computers, each with its real bridge parts (its limits, its post, its directory of
// your other computers, its peer routes), on a stand-in relay that plays each box's link; the maker
// (Anthropic, say) is a stand-in that counts the reads it gets.

import { afterAll, beforeEach, describe, expect, test } from "bun:test"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { LinkDirectory, PeerPost } from "../../src/agents/groups/post"
import type { Clock } from "../../src/agents/groups/peer"
import { Limits, LimitsProblem, identityOf, limitsRoutes, type LimitedAccount, type LimitsSource } from "../../src/usage/limits"
import { LimitsPeers, PEER_AWAY, TELL_AGAIN, type LimitsMail } from "../../src/usage/limits-peers"
import type { LimitRow } from "../../src/usage/limits-rows"
import { subscribe } from "../../src/agents/events"

const base = mkdtempSync(join(process.env.KUVO_TEST_ROOT ?? tmpdir(), "kuvo-limits-peers-"))
afterAll(() => rmSync(base, { recursive: true, force: true }))

const MINUTE = 60_000
/** Claude's time between reads. */
const EVERY = 15 * MINUTE

/** Waits that look again only when mail came. */
const wakes = new Set<() => void>()
const beat = () => {
  for (const wake of [...wakes]) wake()
}
async function until(condition: () => boolean) {
  while (!condition()) {
    await new Promise<void>((resolve) => {
      const wake = () => {
        wakes.delete(wake)
        resolve()
      }
      wakes.add(wake)
    })
  }
}

/** The clock every box here reads (each may be off from it by `off`), moved on by hand. */
let time = Date.parse("2026-11-12T09:00:00.000Z")
const pass = (ms: number) => (time += ms)
/** The post's heartbeat never goes in a test. */
const still: Clock = { setTimeout: () => undefined, clearTimeout: () => {} }

/** The maker: what it answers each account (by who it is), and each read it got, by the computer that asked. */
class Maker {
  reads: { box: string; who: string }[] = []
  used = new Map<string, number>()
  /** Computers it turns away now (a 429), and until when. */
  refuses = new Map<string, number>()
  /** An account's sign-in on `box`: read with its own token there, the same account everywhere. */
  source(box: string, who: string): LimitsSource {
    return {
      every: EVERY,
      identity: () => who,
      read: async (now) => {
        this.reads.push({ box, who })
        const until = this.refuses.get(box)
        if (until) throw new LimitsProblem("rate-limited", "Anthropic turned the read away (429).", until)
        return { plan: "Max 20x", windows: [{ id: "five_hour", used: this.used.get(who) ?? 0.1, minutes: 300, resetsAt: now + 3 * 60 * MINUTE }] }
      },
    }
  }
}

/** The relay, and each box's link on it: what a box's post sends goes to the box it's for, unless one of them is off. */
class Relay {
  boxes = new Map<string, Box>()
  off = new Set<string>()
  sent: { from: string; to: string; mail: LimitsMail }[] = []
  /** Mail that's been given to its box, or lost to one that's off. */
  done = 0
  carry(from: string, to: string, mail: LimitsMail) {
    this.sent.push({ from, to, mail: structuredClone(mail) })
    if (this.off.has(from) || this.off.has(to)) {
      this.done++
      return beat()
    }
    queueMicrotask(async () => {
      const target = this.boxes.get(to)
      if (target) await peerRoutes(target.parts, "POST", ["mail"], async () => ({ from, mail: structuredClone(mail) }))
      this.done++
      beat()
    })
  }

  /** What the boxes' bridges sent. */
  posted = 0
  /** Once every mail any box's bridge sent is where it went. */
  settled() {
    return until(() => this.done === this.posted)
  }
}

/** A box's real post, counting what its bridge sends. */
class Post extends PeerPost {
  constructor(private readonly counted: () => void) {
    super(undefined, still)
  }
  override send(...given: Parameters<PeerPost["send"]>) {
    this.counted()
    super.send(...given)
  }
}

/** One of your computers: its accounts' sign-ins, and its bridge's limits shared through its link. */
class Box {
  readonly root: string
  post!: Post
  directory!: LinkDirectory
  limits!: Limits
  peers!: LimitsPeers
  parts!: PeerParts
  private following = new AbortController()

  constructor(
    readonly key: string,
    readonly name: string,
    readonly relay: Relay,
    readonly accounts: LimitedAccount[],
    /** How far its clock is off. */
    readonly off = 0,
    root?: string,
  ) {
    this.root = root ?? mkdtempSync(join(base, `${name}-`))
    this.start()
  }

  /** Its bridge started (again, from what it kept). */
  start() {
    this.following.abort()
    this.following = new AbortController()
    const now = () => time + this.off
    this.post = new Post(() => this.relay.posted++)
    this.directory = new LinkDirectory(this.post, still)
    this.limits = new Limits(() => this.accounts, join(this.root, "limits.json"), now)
    this.peers = new LimitsPeers(this.limits, this.post, this.directory, join(this.root, "limits-peers.json"), now)
    this.parts = { post: this.post, directory: this.directory, limits: this.peers }
    this.relay.boxes.set(this.key, this)
    void this.link()
  }

  /** The box's link: follows what its post sends and carries it on the relay. */
  private async link() {
    const reader = this.post.follow(this.following.signal).body!.getReader()
    const decoder = new TextDecoder()
    let buffer = ""
    for (;;) {
      const read = await reader.read().catch(() => ({ done: true as const, value: undefined }))
      if (read.done) return
      buffer += decoder.decode(read.value, { stream: true })
      let end: number
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, end)
        buffer = buffer.slice(end + 1)
        if (!line) continue
        const given = JSON.parse(line)
        this.relay.carry(this.key, given.to, given.mail)
      }
    }
  }

  /** Its link starts again (its bridge doesn't): it follows what the bridge sends anew, and says who your computers are. */
  async relink(others: Box[]) {
    this.following.abort()
    this.following = new AbortController()
    void this.link()
    await this.meet(others)
  }

  /**
   * Supplies membership to the bridge. Older links also sent `also`.
   */
  async meet(others: Box[], legacyAlso: Pick<Box, "key" | "name">[] = []) {
    await peerRoutes(this.parts, "PUT", [], async () => ({ self: { key: this.key, name: this.name }, boxes: others.map((box) => ({ key: box.key, name: box.name, seats: true })), also: legacyAlso.map((box) => ({ key: box.key, name: box.name })), bots: [] }))
  }

  async route(method = "GET", rest: string[] = []) {
    return ((await (await limitsRoutes(this.limits, method, rest)).json()) as any).data as { accounts: any[]; services: string[]; rows: LimitRow[] }
  }

  rows() {
    return this.limits.rows()
  }

  stop() {
    this.following.abort()
  }
}

const claude = (maker: Maker, box: string, who: string, provider = "claude-code"): LimitedAccount => ({
  provider,
  service: "claude",
  agent: { kind: "claude", account: provider === "claude-code" ? "default" : provider.split("@")[1]! },
  limits: maker.source(box, who),
})

let maker: Maker
let relay: Relay

beforeEach(() => {
  maker = new Maker()
  relay = new Relay()
})

/** Three computers, each signed in to `work`; lokum to `home` too. */
async function threeComputers() {
  const mac = new Box("key-mac", "MacBook Pro", relay, [claude(maker, "mac", "anthropic:work|org")])
  const hetzner = new Box("key-hetzner", "hetzner", relay, [claude(maker, "hetzner", "anthropic:work|org")])
  const lokum = new Box("key-lokum", "lokum", relay, [claude(maker, "lokum", "anthropic:work|org"), claude(maker, "lokum", "anthropic:home|org", "claude-code@home0001")])
  const all = [mac, hetzner, lokum]
  for (const box of all) await box.meet(all.filter((other) => other !== box))
  return { mac, hetzner, lokum, all }
}

/** Two computers signed in to nothing (every account signed in is listed, read or not): only what one tells the other shows. */
async function twoUnsignedComputers() {
  const mac = new Box("key-mac", "MacBook Pro", relay, [])
  const hetzner = new Box("key-hetzner", "hetzner", relay, [])
  await mac.meet([hetzner])
  await hetzner.meet([mac])
  return { mac, hetzner }
}

describe("plan limits across your computers", () => {
  test("an account signed in on three computers is read from its maker once each round, by one of them, and every one lists it once with its newest reading", async () => {
    const { mac, hetzner, lokum } = await threeComputers()
    maker.used.set("anthropic:work|org", 0.42)
    await mac.limits.tick()
    await relay.settled()
    await hetzner.limits.tick()
    await lokum.limits.tick()
    // The work account read once (by the Mac), and lokum's own other account by lokum.
    expect(maker.reads.filter((read) => read.who === "anthropic:work|org")).toEqual([{ box: "mac", who: "anthropic:work|org" }])

    // Round after round: the Mac reads it when it's due, and the others, whose minutes come round
    // a little later, don't.
    for (let round = 1; round <= 3; round++) {
      pass(round === 1 ? EVERY : EVERY - 5 * MINUTE)
      await mac.limits.tick()
      await relay.settled()
      pass(5 * MINUTE)
      await hetzner.limits.tick()
      await lokum.limits.tick()
      expect(maker.reads.filter((read) => read.who === "anthropic:work|org").map((read) => read.box)).toEqual(Array(round + 1).fill("mac"))
    }

    // Every box lists it once, with the Mac's reading, and the computers it's signed in on.
    for (const box of [mac, hetzner, lokum]) {
      const work = box.rows().filter((row) => row.windows[0]?.used === 0.42)
      expect(work).toHaveLength(1)
      expect(work[0]!.plan).toBe("Max 20x")
      expect(work[0]!.boxes.map((each) => each.name).sort()).toEqual(["MacBook Pro", "hetzner", "lokum"])
      expect(work[0]!.boxes.filter((each) => each.here).map((each) => each.name)).toEqual([box.name])
      // This box's first.
      expect(work[0]!.boxes[0]!.name).toBe(box.name)
    }
    // lokum's other account: one row, on lokum only, on every box.
    for (const box of [mac, hetzner]) {
      const home = box.rows().filter((row) => row.boxes.length === 1 && row.boxes[0]!.name === "lokum")
      expect(home).toHaveLength(1)
      expect(home[0]!.label).toBeUndefined()
      expect(home[0]!.windows).toHaveLength(1)
    }
    // The route says the same, and keeps what older clients read: each box's own accounts.
    const said = await hetzner.route()
    expect(said.rows.map(({ usage: _, card: __, ...row }) => row)).toEqual(JSON.parse(JSON.stringify(hetzner.rows())))
    expect(said.rows[0]!.usage?.computers).toEqual([{ name: "MacBook Pro", here: false }])
    expect(said.accounts.map((account) => account.provider)).toEqual(["claude-code"])
    // hetzner never read it: what it lists as its own (for a client from before rows) has the Mac's numbers.
    expect(said.accounts[0].windows[0].used).toBe(0.42)
    expect(said.accounts[0].checked).toBeUndefined()
    // Which computer read it, in its row: the Mac, not hetzner.
    expect(said.rows[0]!.boxes.find((each) => each.here)?.checked).toBeUndefined()
    expect(said.rows).toHaveLength(2)
  })

  test("computers that read at the same moment settle on one reader by the next round", async () => {
    const { mac, hetzner, lokum, all } = await threeComputers()
    // Nothing heard yet: each reads it.
    await Promise.all(all.map((box) => box.limits.tick()))
    await relay.settled()
    const work = () => maker.reads.filter((read) => read.who === "anthropic:work|org").map((read) => read.box)
    expect(work().sort()).toEqual(["hetzner", "lokum", "mac"])
    // A round later, as their minutes come round: the first reads it, and the others hear it did.
    pass(EVERY)
    await hetzner.limits.tick()
    await relay.settled()
    await lokum.limits.tick()
    await mac.limits.tick()
    expect(work().slice(3)).toEqual(["hetzner"])
  })

  test("a refresh asked of one computer reads there only what's due, and asks the computer an account is on for one it doesn't have", async () => {
    const { mac, hetzner, lokum } = await threeComputers()
    await lokum.limits.tick()
    await relay.settled()
    maker.used.set("anthropic:home|org", 0.2)
    const before = maker.reads.length
    // Read a minute ago on lokum: a refresh on the Mac doesn't ask Anthropic again.
    pass(MINUTE)
    await mac.route("POST", ["refresh"])
    // (Once lokum, asked, has looked.)
    await relay.settled()
    await lokum.limits.tick()
    expect(maker.reads.length).toBe(before)

    // Long after: the Mac reads its own (the work account), and lokum, asked, reads its home account.
    pass(EVERY + MINUTE)
    maker.used.set("anthropic:home|org", 0.77)
    await mac.route("POST", ["refresh"])
    await until(() => mac.rows().some((row) => row.windows[0]?.used === 0.77))
    // The work account once: by the Mac, which lokum heard of before it was asked.
    expect(maker.reads.slice(before).map((read) => `${read.box} ${read.who}`)).toEqual(["mac anthropic:work|org", "lokum anthropic:home|org"])
  })

  test("a computer that's off keeps its last reading, listed as of when it was read and away, through a restart; what it read is read elsewhere once it's due", async () => {
    const { mac, hetzner, lokum } = await threeComputers()
    maker.used.set("anthropic:home|org", 0.3)
    await lokum.limits.tick()
    const read = time
    await relay.settled()
    relay.off.add("key-lokum")
    lokum.stop()

    // Not heard from past PEER_AWAY (it would have told again each ten minutes).
    pass(PEER_AWAY + MINUTE)
    const home = () => hetzner.rows().find((row) => row.boxes.some((each) => each.provider === "claude-code@home0001"))
    expect(home()?.away).toBe(true)
    expect(home()?.boxes[0]?.away).toBe(true)
    expect(home()?.windows[0]?.read).toBe(read)
    expect(home()?.windows[0]?.used).toBe(0.3)
    // The work account, signed in here too, isn't away.
    expect(hetzner.rows().find((row) => row.boxes.some((each) => each.here))?.away).toBeUndefined()

    // Through a restart of hetzner's bridge.
    hetzner.stop()
    hetzner.start()
    await hetzner.meet([mac, lokum])
    expect(home()?.away).toBe(true)
    expect(home()?.windows[0]?.read).toBe(read)

    // lokum read the work account last: past its next read and a little, the others take it on.
    const before = maker.reads.length
    await hetzner.limits.tick()
    expect(maker.reads.slice(before)).toEqual([{ box: "hetzner", who: "anthropic:work|org" }])
  })

  test("a maker that turns one computer away holds that one only: another reads the account, and a read that failed never stands in for one", async () => {
    const mac = new Box("key-mac", "MacBook Pro", relay, [claude(maker, "mac", "anthropic:work|org")])
    const hetzner = new Box("key-hetzner", "hetzner", relay, [claude(maker, "hetzner", "anthropic:work|org")])
    await mac.meet([hetzner])
    await hetzner.meet([mac])
    maker.used.set("anthropic:work|org", 0.5)
    await mac.limits.tick()
    await relay.settled()
    await hetzner.limits.tick()
    // Due again, the Mac is turned away for an hour.
    pass(EVERY)
    maker.refuses.set("mac", time + 60 * MINUTE)
    await mac.limits.tick()
    await relay.settled()
    // hetzner's minute comes round: the Mac's last good reading is a round old, so it reads.
    pass(3 * MINUTE)
    maker.used.set("anthropic:work|org", 0.6)
    await hetzner.limits.tick()
    await relay.settled()
    // The Mac, held, asks nothing; its hold isn't hetzner's.
    pass(10 * MINUTE)
    await mac.limits.tick()
    expect(maker.reads.map((read) => read.box)).toEqual(["mac", "mac", "hetzner"])
    // Read on hetzner: listed with that, and no problem (one box could read it).
    for (const box of [mac, hetzner]) {
      const [row] = box.rows()
      expect(row!.windows[0]!.used).toBe(0.6)
      expect(row!.problem).toBeUndefined()
      expect(row!.boxes.find((each) => each.key === "key-mac")).toBeDefined()
    }
  })

  test("a computer signed in to another account since, whose sign-in doesn't say who it is (Copilot's), reads it rather than taking a peer's reading of the one before", async () => {
    // Copilot's sign-in doesn't say whose it is: GitHub's answer does. Its short token is renewed
    // often; its refresh token lasts until it's signed in again.
    const logins = new Map<string, string>([["mac", "octocat"], ["hetzner", "octocat"]])
    let renewals = 0
    const copilot = (box: string): LimitedAccount => ({
      provider: "github-copilot",
      service: "copilot",
      limits: {
        every: 10 * MINUTE,
        signIn: () => `token-of-${logins.get(box)}-${renewals}`,
        account: () => `refresh-of-${logins.get(box)}`,
        read: async () => {
          const login = logins.get(box)!
          maker.reads.push({ box, who: login })
          return { identity: `github:${login}`, windows: [{ id: "premium_interactions", used: login === "octocat" ? 0.4 : 0.1, minutes: 43_200 }] }
        },
      },
    })
    const mac = new Box("key-mac", "MacBook Pro", relay, [copilot("mac")])
    const hetzner = new Box("key-hetzner", "hetzner", relay, [copilot("hetzner")])
    await mac.meet([hetzner])
    await hetzner.meet([mac])
    // Each reads it once to learn who it is; then hetzner takes the Mac's readings.
    await mac.limits.tick()
    await relay.settled()
    await hetzner.limits.tick()
    await relay.settled()
    pass(10 * MINUTE)
    await mac.limits.tick()
    await relay.settled()
    pass(MINUTE)
    await hetzner.limits.tick()
    expect(maker.reads.map((read) => read.box)).toEqual(["mac", "hetzner", "mac"])
    // Its token renewed, the same login: still the Mac's reading.
    renewals++
    pass(10 * MINUTE)
    await mac.limits.tick()
    await relay.settled()
    pass(3 * MINUTE)
    await hetzner.limits.tick()
    expect(maker.reads.map((read) => read.box)).toEqual(["mac", "hetzner", "mac", "mac"])

    // hetzner signs in as someone else: read at once, though the Mac read octocat's just now.
    logins.set("hetzner", "hubot")
    await hetzner.limits.tick()
    await relay.settled()
    expect(maker.reads.slice(4)).toEqual([{ box: "hetzner", who: "hubot" }])
    // Two rows on each: octocat's on the Mac, hubot's on hetzner.
    for (const box of [mac, hetzner]) expect(box.rows().map((row) => `${row.windows[0]?.used} on ${row.boxes.map((each) => each.name).join(",")}`).sort()).toEqual(["0.1 on hetzner", "0.4 on MacBook Pro"])
  })

  test("Refresh on a Mac, whose Claude only its turns tell of, asks the computer that reads that account", async () => {
    // The Mac's Claude: no read of Anthropic there, only what its turns hear.
    const turns: LimitsSource = { every: EVERY, identity: () => "anthropic:work|org" }
    const mac = new Box("key-mac", "MacBook Pro", relay, [{ provider: "claude-code", service: "claude", agent: { kind: "claude", account: "default" }, limits: turns }])
    const hetzner = new Box("key-hetzner", "hetzner", relay, [claude(maker, "hetzner", "anthropic:work|org")])
    await mac.meet([hetzner])
    await hetzner.meet([mac])
    await mac.limits.tick()
    turns.heard!({ id: "five_hour", used: 0.3, minutes: 300 })
    await hetzner.limits.tick()
    await relay.settled()
    expect(maker.reads.map((read) => read.box)).toEqual(["hetzner"])
    // A while on, Refresh on the Mac: hetzner, asked, reads it now.
    pass(EVERY + MINUTE)
    maker.used.set("anthropic:work|org", 0.66)
    await mac.route("POST", ["refresh"])
    await until(() => mac.rows().some((row) => row.windows.some((window) => window.used === 0.66)))
    expect(maker.reads.map((read) => read.box)).toEqual(["hetzner", "hetzner"])
  })

  test("an agent's plan_limits says the numbers its computer's accounts were read with anywhere, and the accounts on your other computers, where", async () => {
    // (The tool tells how long until a reset by the computer's own clock.)
    time = Date.now()
    const { mac, hetzner } = await threeComputers()
    maker.used.set("anthropic:work|org", 0.42)
    maker.used.set("anthropic:home|org", 0.77)
    await mac.limits.tick()
    await relay.settled()
    const lokum = relay.boxes.get("key-lokum")!
    await lokum.limits.tick()
    await relay.settled()
    await hetzner.limits.tick()
    // The bridge as the link reaches it, on hetzner, which never read either.
    const bridge = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: (request) => limitsRoutes(hetzner.limits, request.method, new URL(request.url).pathname.split("/").slice(4)) })
    try {
      const { planLimitsTool } = await import("../../src/link/work/plan-limits")
      const { text } = await planLimitsTool({ port: bridge.port, password: "x" } as never)
      expect(text).toContain("- Claude Code, Max 20x, provider claude-code:\n  - 5-hour window: 42% used")
      expect(text).toContain("- Claude Code, Max 20x, signed in on lokum:\n  - 5-hour window: 77% used")
    } finally {
      bridge.stop(true)
    }
  })

  test("a box whose group chats didn't start takes plan limits, and says not now to group chat mail, so what mustn't be lost comes again", async () => {
    const box = new Box("key-mac", "MacBook Pro", relay, [])
    const post = async (route: string, body: any) => {
      const handler = body.mail?.kind === "limits" ? peerRoutes : groupPeerRoutes
      return (await handler(box.parts, "POST", [route], async () => body)).status
    }
    expect(await post("mail", { from: "key-other", mail: { kind: "limits", at: time, accounts: [] } })).toBe(200)
    expect(await post("mail", { from: "key-other", mail: { kind: "work", work: { task: "gt_1" } } })).toBe(404)
    expect(await post("gaveup", { to: "key-other", mail: { kind: "work", work: { task: "gt_1" } } })).toBe(404)
  })

  test("the link saying who your computers are again tells no one again, and an ask is answered once a minute at most", async () => {
    const { mac, hetzner, lokum, all } = await threeComputers()
    await relay.settled()
    const before = relay.sent.length
    // (The link says it each time a peer's bots change.)
    for (const box of all) await box.meet(all.filter((other) => other !== box))
    await relay.settled()
    expect(relay.sent.length).toBe(before)
    // A minute on, lokum asks twice within the minute: the Mac answers once.
    pass(MINUTE)
    const ask = { kind: "limits", at: time, accounts: [], ask: true }
    await peerRoutes(mac.parts, "POST", ["mail"], async () => ({ from: lokum.key, mail: ask }))
    await peerRoutes(mac.parts, "POST", ["mail"], async () => ({ from: lokum.key, mail: ask }))
    await relay.settled()
    expect(relay.sent.slice(before).map((each) => `${each.from} ${each.to}`)).toEqual(["key-mac key-lokum"])
    void hetzner
  })

  test("a tell lost while a computer's link was off is made up once that link carries mail again", async () => {
    const mac = new Box("key-mac", "MacBook Pro", relay, [claude(maker, "mac", "anthropic:work|org")])
    const lokum = new Box("key-lokum", "lokum", relay, [])
    await mac.meet([lokum])
    await lokum.meet([mac])
    await relay.settled()
    pass(2 * MINUTE)
    // lokum's link is off as the Mac reads and tells it: lost.
    relay.off.add("key-lokum")
    maker.used.set("anthropic:work|org", 0.61)
    await mac.limits.tick()
    await relay.settled()
    // (The Mac's account is listed there as signed in, before any reading: none reached it.)
    expect(lokum.rows().map((row) => ({ id: row.id, boxes: row.boxes.map((box) => box.key), windows: row.windows }))).toEqual([{ id: "key-mac/claude-code", boxes: ["key-mac"], windows: [] }])
    // Half a minute on, its link starts again (its bridge didn't): it asks, and the Mac answers.
    pass(30_000)
    relay.off.delete("key-lokum")
    await lokum.relink([mac])
    await relay.settled()
    expect(lokum.rows().map((row) => row.windows[0]?.used)).toEqual([0.61])
  })

  test("an account of a service a newer box knows and this one doesn't is listed by its name", async () => {
    const { mac, hetzner } = await twoUnsignedComputers()
    await peerRoutes(mac.parts, "POST", ["mail"], async () => ({
      from: hetzner.key,
      mail: { kind: "limits", at: time, accounts: [{ provider: "kimi", service: "kimi-code", identity: "abcdefabcdefabcd", windows: [{ id: "week", used: 0.25, minutes: 10_080, read: time, source: "usage" }], checked: time }] },
    }))
    expect(mac.rows().map((row) => `${row.service} ${row.windows[0]?.used}`)).toEqual(["kimi-code 0.25"])
    // What isn't a service's name isn't taken.
    await peerRoutes(mac.parts, "POST", ["mail"], async () => ({ from: hetzner.key, mail: { kind: "limits", at: time, accounts: [{ provider: "x", service: "Not A Name!", windows: [] }] } }))
    expect(mac.rows()).toEqual([])
  })

  test("what a peer's account has beyond its windows (`extras`) is kept as told, and only what one is", async () => {
    const { mac, hetzner } = await twoUnsignedComputers()
    await peerRoutes(mac.parts, "POST", ["mail"], async () => ({
      from: hetzner.key,
      mail: {
        kind: "limits",
        at: time,
        accounts: [
          {
            provider: "cursor",
            service: "cursor",
            identity: "abcdefabcdefabcd",
            windows: [{ id: "total", used: 1, read: time, source: "usage" }],
            extras: [
              { id: "on_demand", name: "On-demand", kind: "spent", amount: 43.21, unit: "usd", secret: "x" },
              { id: "x", name: "X", kind: "grants", amount: 1, unit: "usd" },
              "nope",
              // (Over 1 KB as it would be passed on, its ID cut to length included: not passed on.)
              { id: "i".repeat(300), name: "Big", kind: "grants", a: "p".repeat(190), b: "p".repeat(190), c: "p".repeat(190), d: "p".repeat(190) },
            ],
            checked: time,
          },
        ],
      },
    }))
    // (A kind this box doesn't know yet, a newer box's, is passed on as told: only its plain fields.)
    expect(mac.rows()[0]!.extras).toEqual([
      { id: "on_demand", name: "On-demand", kind: "spent", amount: 43.21, unit: "usd" },
      { id: "x", name: "X", kind: "grants", amount: 1, unit: "usd" },
    ])
  })

  test("a computer you removed goes from the list, with its accounts", async () => {
    const { mac, hetzner, lokum } = await threeComputers()
    await lokum.limits.tick()
    await relay.settled()
    expect(hetzner.rows().some((row) => row.boxes.some((each) => each.name === "lokum"))).toBe(true)
    await hetzner.meet([mac])
    expect(hetzner.rows().some((row) => row.boxes.some((each) => each.name === "lokum"))).toBe(false)
  })

  test("a peer whose clock is an hour ahead holds this box back no longer than one whose clock is right", async () => {
    const mac = new Box("key-mac", "MacBook Pro", relay, [claude(maker, "mac", "anthropic:work|org")])
    const ahead = new Box("key-ahead", "ahead", relay, [claude(maker, "ahead", "anthropic:work|org")], 60 * MINUTE)
    await mac.meet([ahead])
    await ahead.meet([mac])
    await ahead.limits.tick()
    await relay.settled()
    // Its reading is as of now here, not an hour on. (The Mac's own, signed in and not read yet, is a row of its own.)
    const theirs = mac.rows().find((row) => row.boxes.some((box) => box.key === "key-ahead"))!
    expect(Math.max(...theirs.windows.map((window) => window.read))).toBe(time)
    pass(EVERY + 3 * MINUTE)
    const before = maker.reads.length
    await mac.limits.tick()
    expect(maker.reads.slice(before)).toEqual([{ box: "mac", who: "anthropic:work|org" }])
  })

  test("what goes between them is the readings, never a sign-in or who the account is at its maker", async () => {
    const { mac, hetzner } = await threeComputers()
    await mac.limits.tick()
    await relay.settled()
    const mails = JSON.stringify(relay.sent)
    expect(mails).toContain("five_hour")
    expect(mails).not.toContain("anthropic:work")
    // Told again each ten minutes though nothing changed (the relay keeps nothing).
    const told = relay.sent.filter((each) => each.from === "key-mac" && each.to === "key-hetzner").length
    pass(TELL_AGAIN)
    await mac.limits.tick()
    expect(relay.sent.filter((each) => each.from === "key-mac" && each.to === "key-hetzner").length).toBe(told + 1)
  })
})

describe("restoring peer readings", () => {
  /** An account with credits left (as OpenRouter's is): what only your own computers are told. */
  const credits = (): LimitedAccount => ({ provider: "openrouter", service: "openrouter", limits: { every: 600_000, identity: () => "key:sk-or-test", read: async () => ({ windows: [], balance: { left: 7.7, limit: 50, unit: "usd" as const } }) } })
  const balance = (box: Box) => box.rows().find((row) => row.service === "openrouter")?.balance

  test("what your own machine last told is kept whole through a restart: shown once the link confirms the computer is a peer", async () => {
    const mac = new Box("key-mac", "MacBook Pro", relay, [])
    const hetzner = new Box("key-hetzner", "hetzner", relay, [credits()])
    await mac.meet([hetzner])
    await hetzner.meet([mac])
    await hetzner.limits.tick()
    await relay.settled()
    expect(balance(mac)).toEqual({ left: 7.7, limit: 50, unit: "usd" })
    // The Mac's bridge restarts while hetzner is off (it tells nothing now).
    relay.off.add("key-hetzner")
    const peersFile = join(mac.root, "limits-peers.json")
    const saved = JSON.parse(readFileSync(peersFile, "utf8"))
    saved.peers["old-key"] = { ...saved.peers["key-hetzner"], name: "Old entry" }
    writeFileSync(peersFile, JSON.stringify(saved))
    mac.start()
    expect(balance(mac)).toBeUndefined()
    expect(mac.rows().find((row) => row.service === "openrouter")).toBeUndefined()
    // Clients hear it as soon as the link supplies membership.
    const heard: any[] = []
    const stop = subscribe((event) => event.type === "kuvo.limits.updated" && heard.push(event.data))
    await mac.meet([hetzner], [{ key: "old-key", name: "Old entry" }])
    stop()
    expect(mac.rows().flatMap(row => row.boxes.map(box => box.key))).toEqual(["key-hetzner"])
    expect(balance(mac)).toEqual({ left: 7.7, limit: 50, unit: "usd" })
    expect(heard.at(-1)?.rows.find((row: LimitRow) => row.service === "openrouter")?.balance).toEqual({ left: 7.7, limit: 50, unit: "usd" })
  })

})

test("a weekly forecast combines both computers' readings and survives a fallback to an older reading", async () => {
  const who = "anthropic:shared"
  const account = claude(maker, "mac", who)
  delete account.limits.read // On this Mac, Claude tells its limits through turns.
  const mac = new Box("key-mac", "MacBook", relay, [account])
  const other = new Box("key-mini", "Mini", relay, [])
  const start = time
  const resetsAt = start + 7 * 24 * 60 * MINUTE
  const window = { id: "seven_day", used: 0.42, minutes: 10_080, resetsAt }
  const readPeer = async (windows = [{ ...window, read: time, source: "turn" as const }]) => {
    const mail = { kind: "limits", at: time, accounts: [{ provider: "claude-code", agent: "claude", service: "claude", identity: identityOf(who), windows }] }
    await peerRoutes(mac.parts, "POST", ["mail"], async () => ({ from: other.key, mail }))
  }
  try {
    await mac.meet([other])
    await mac.limits.tick()
    mac.limits.hear("claude-code", window)
    pass(2 * 60 * MINUTE)
    await readPeer()
    pass(2 * 60 * MINUTE)
    mac.limits.hear("claude-code", { ...window, used: 0.38 })
    pass(2 * 60 * MINUTE)
    await readPeer()
    const forecast = async () => (await mac.route()).rows.map(row => row.card!.lines[0]!.words.map(word => word.text).join(""))
    expect(await forecast()).toEqual(["58% left · lasts until reset"])
    await readPeer([])
    pass(MINUTE)
    await readPeer()
    expect(await forecast()).toEqual(["58% left · lasts until reset"])
  } finally {
    mac.stop()
    other.stop()
  }
})

test("a box still shows the weekly forecast it learned from a peer after it restarts", async () => {
  const mac = new Box("key-mac", "MacBook", relay, [])
  const other = new Box("key-mini", "Mini", relay, [])
  const resetsAt = time + 7 * 24 * 60 * MINUTE
  try {
    await mac.meet([other])
    for (let reading = 0; reading < 4; reading++) {
      const window = { id: "seven_day", used: 0.42, minutes: 10_080, resetsAt, read: time, source: "turn" }
      const mail = { kind: "limits", at: time, accounts: [{ provider: "claude-code", agent: "claude", service: "claude", identity: identityOf("anthropic:shared"), windows: [window] }] }
      await peerRoutes(mac.parts, "POST", ["mail"], async () => ({ from: other.key, mail }))
      if (reading < 3) pass(2 * 60 * MINUTE)
    }
    mac.start()
    await mac.meet([other])
    const { rows } = await mac.route()
    expect(rows.map(row => row.card!.lines[0]!.words.map(word => word.text).join(""))).toEqual(["58% left · lasts until reset"])
  } finally {
    mac.stop()
    other.stop()
  }
})

test("usage names a reachable computer while its first limits snapshot is in flight", async () => {
  const relay = new Relay()
  const near = new Box("near", "MacBook", relay, [])
  const far = new Box("far", "Mac mini", relay, [])
  relay.off.add(far.key)
  const roster = (limits: "connected" | "away") => ({
    self: { key: near.key, name: near.name },
    boxes: [{ key: far.key, name: far.name, seats: true, limits }], bots: [],
  })
  const current = async () => (await limitsRoutes(near.limits, "GET", [])).json()
  try {
    near.directory.set(roster("connected"))
    expect(await current()).toMatchObject({ data: { pendingComputers: [{ id: far.key, name: far.name }] } })
    near.directory.set(roster("away"))
    expect(await current()).toMatchObject({ data: { pendingComputers: [] } })
  } finally {
    near.stop()
    far.stop()
  }
})
