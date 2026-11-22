// Legacy migration, live checkpoints, cold pages and the link's indexes use the real stores.
import "./safety"
import { expect, test } from "bun:test"
import { Database } from "bun:sqlite"
import { fileURLToPath } from "node:url"
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Store, readSavedSessions } from "../src/agents/store"

import { transcriptDatabase, transcriptPictures } from "../src/agents/transcript"
import { Turn } from "../src/agents/turn"
import { noTokens } from "../src/agents/wire"
import type { Message, Session } from "../src/agents/wire"
import { InsightsScan } from "../src/usage/insights-scan"
import { PricingStore } from "../src/usage/pricing"
import { artifactCitations } from "../src/link/artifacts/service"

test("opening a legacy chat keeps its pictures readable and a turn saves only new or changed messages", () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-transcript-"))
  try {
    const root = join(home, "sessions")
    const id = "ses_chat"
    const folder = join(root, id)
    const attachments = join(home, "attachments")
    const model = { id: "test", providerID: "claude-code" }
    const session: Session = { id, model, title: "Screenshots", cost: 0, projectID: "project", tokens: noTokens(),
      time: { created: 1, updated: 2 }, location: { directory: home } }
    const raw = Buffer.from("a screenshot with bytes ".repeat(16_384))
    const first: Message = { id: "msg_picture", type: "user", time: { created: 1 }, text: "Review this \\" + "🌙",
      files: [{ mime: "image/png", name: "screen.png", source: { type: "uri", uri: "data:image/png;base64," + raw.toString("base64") } }] }
    mkdirSync(folder, { recursive: true })
    writeFileSync(join(folder, "session.json"), JSON.stringify(session))
    writeFileSync(join(folder, "messages.json"), JSON.stringify([first, { ...first, id: "msg_again" }], null, 2))

    const store = new Store(root, false, attachments)
    const messages = store.get(id)!.messages
    expect(messages.map(message => message.id)).toEqual(["msg_picture", "msg_again"])
    const picture = messages[0]?.type === "user" ? messages[0].files?.[0]?.source : undefined
    expect(picture?.type).toBe("uri")
    if (picture?.type !== "uri") throw new Error("The saved picture has no address")
    expect(picture.uri.startsWith("file:")).toBe(true)
    expect(readFileSync(fileURLToPath(picture.uri)).equals(raw)).toBe(true)
    expect(readdirSync(join(attachments, id, transcriptPictures))).toHaveLength(1)
    const old = join(folder, "messages.json")
    const portable = readFileSync(old, "utf8")
    expect(portable).not.toContain("data:image/")
    expect(portable).toContain("screen.png")

    // SQLite causes an attempted write of an old row to fail. A real turn still saves its
    // streamed response, including a tool result that arrives after its step closed.
    const transcript = new Database(join(folder, transcriptDatabase))
    transcript.exec(`CREATE TRIGGER old_message_is_immutable BEFORE UPDATE ON message
      WHEN old.id IN ('msg_picture','msg_again') BEGIN SELECT RAISE(ABORT, 'old message rewritten'); END;`)
    transcript.close()
    const turn = new Turn(store, id, home, model, "build")
    turn.startStep()
    turn.blockDelta("text-0", "text", "First bit.")
    turn.toolStart("call", "screenshot", {})
    turn.endStep()
    store.flush()
    turn.toolSucceeded("call", [{ type: "file", mime: "image/png", uri: "data:image/png;base64," + raw.toString("base64") }])
    turn.finish("succeeded")
    expect(readFileSync(old, "utf8")).toBe(portable)
    expect(readdirSync(join(attachments, id, transcriptPictures))).toHaveLength(1)
    const reopened = new Store(root, false, attachments)
    expect(reopened.get(id)!.messages.slice(2)).toMatchObject([
      { type: "assistant", content: [{ type: "text", text: "First bit." },
        { type: "tool", state: { status: "completed", content: [{ type: "file", uri: picture.uri }] } }] },
      { type: "idle", outcome: "succeeded" },
    ])
    // Cold paging returns the right rows after the bridge releases its working copy.
    store.prune(() => false)
    expect(store.page(id, "msg_again", 1, true)).toMatchObject({ data: [{ type: "assistant" }], more: true })
    expect(store.page(id, "msg_again", 5, false)).toMatchObject({ data: [{ id: "msg_picture", text: first.text }], more: false })
  } finally { rmSync(home, { recursive: true, force: true }) }
})

test("Insights and artifact indexing still see agent usage and shown files after migration", async () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-transcript-"))
  try {
    const root = join(home, "sessions")
    const engine = join(home, "engine")
    mkdirSync(engine)
    const sqlite = new Database(join(engine, "opencode.db"))
    sqlite.exec(`CREATE TABLE session_v2 (id TEXT PRIMARY KEY, title TEXT, parent_id TEXT, metadata TEXT);
      CREATE TABLE session_message (id TEXT, session_id TEXT, time_created INTEGER, type TEXT, data TEXT);`)
    sqlite.query("INSERT INTO session_v2 VALUES ('engine', 'Engine', NULL, NULL)").run()
    sqlite.query("INSERT INTO session_message VALUES ('call', 'engine', 10, 'assistant', ?)").run(
      JSON.stringify({ model: { providerID: "opencode", id: "engine-model" }, time: { completed: 11 },
        tokens: { ...noTokens(), input: 42 } }))
    sqlite.close()
    const providers = ["claude-code", "codex-cli", "antigravity", "grok-cli", "cursor"]
    for (const provider of providers) {
      const id = `ses_${provider}`
      const folder = join(root, id)
      mkdirSync(folder, { recursive: true })
      const model = { id: "test", providerID: provider }
      const session: Session = { id, model, title: provider, projectID: "p", cost: 0, tokens: noTokens(),
        time: { created: 1, updated: 21 }, location: { directory: home } }
      const step: Message = { type: "assistant", id: "msg_call", time: { created: 19, completed: 21 }, agent: "build",
        model, tokens: { ...noTokens(), input: 7 }, content: [
          { type: "tool", id: "skill", name: "Skill", state: { status: "completed", input: { skill: "example" }, content: [] } },
          { type: "tool", id: "cited-msg_call-1", name: "kuvo-browser_show_file",
            state: { status: "completed", input: { path: `${provider}.png` }, content: [] } },
        ] }
      writeFileSync(join(folder, "session.json"), JSON.stringify(session))
      writeFileSync(join(folder, "messages.json"), JSON.stringify([step]))
    }
    const scan = new InsightsScan({ root: home, cache: join(home, "cache"), engine, agents: () => [],
      store: () => readSavedSessions(root, "insight"), bots: () => [], computer: () => "local" })
    const pricing = new PricingStore(undefined, null).current()
    const before = await scan.scan(pricing, 0)
    const known = new Store(root, false, join(home, "attachments"))
    for (const provider of providers) expect(known.get(`ses_${provider}`)!.messages).toHaveLength(1)
    known.prune(() => false)

    const after = await scan.scan(pricing, 0)
    expect(after.records).toEqual(before.records)
    expect(after.records).toHaveLength(6)
    expect(after.records.find(record => record.session === "engine")?.tokens?.input).toBe(42)
    for (const provider of providers) {
      const call = after.records.find(record => record.session === `ses_${provider}`)
      expect(call?.skills).toEqual(["example"])
      expect(call?.tokens?.input).toBe(provider === "cursor" ? undefined : 7)
    }
    expect(artifactCitations(root).map(citation => citation.path).sort()).toEqual(
      providers.map(provider => join(home, `${provider}.png`)).sort(),
    )
  } finally { rmSync(home, { recursive: true, force: true }) }
})
