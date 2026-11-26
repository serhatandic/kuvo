import "../safety"
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { sideQuestionRoutes, SideQuestions } from "../../src/agents/btw"
import { EngineLink } from "../../src/agents/engine"
import { subscribe } from "../../src/agents/events"
import type { SessionPlaces } from "../../src/agents/session-places"
import { server } from "../../src/agents/groups/api"
import { branchRoutes } from "../../src/agents/branch"
import type { ServerEvent } from "../../src/agents/wire"
import { StandIn } from "./support/standin"

// Side questions and branches on the engine or with
// the coding agents, against a stand-in engine through the bridge's real link to it.

let root: string
let engine: StandIn
let link: EngineLink
let events: ServerEvent[]
let unsubscribe: () => void

const wakes = new Set<() => void>()
function beat() {
  for (const wake of [...wakes]) wake()
}
async function until(condition: () => boolean) {
  while (!condition()) await new Promise<void>((resolve) => wakes.add(() => (wakes.delete(resolve as any), resolve())))
}

/** Who's following which conversation's events, so a test can delete one. */
let following: { session: string; event(type: string, data: any): void }[] = []
const listen = (session: string, told: { event(type: string, data: any): void }) => {
  const entry = { session, event: told.event }
  following.push(entry)
  return () => void (following = following.filter((each) => each !== entry))
}
/** The conversation deleted, as its place says. */
function deleted(session: string) {
  for (const each of following.filter((each) => each.session === session)) each.event("session.deleted", { sessionID: session })
}

function places(bridge = false): SessionPlaces {
  return {
    engine: { api: link, listen },
    bridge: { api: server(engine.url, "x"), listen },
    isBridge: () => bridge,
  }
}

/** As `places`, the engine's events heard through its real link (a session under another is
 * heard through its parent's feed, as on a box). */
function linkedPlaces(): SessionPlaces {
  // As the box's own (agents/main.ts) hears it.
  return { ...places(), engine: { api: link, listen: (session, told) => link.listen(session, { event: (event) => told.event(event.type, event.data), reconnected: told.reconnected }) } }
}

/** What the conversation was told (a note or a message), which a side question never is. */
const told = (session = "ses_work") => engine.synthetic.filter((note) => note.session === session).length + engine.prompts(session).length

// git as it is, without your own settings (a global ignore file would hide what a test makes).
process.env.GIT_CONFIG_GLOBAL = "/dev/null"
process.env.GIT_CONFIG_NOSYSTEM = "1"

beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "kuvo-keywords-"))
  following = []
  engine = new StandIn(beat)
  // Its events reach those following them.
  engine.onEmit = (type, data) => {
    // A session under another is heard through its parent's feed too, as the engine's link does.
    const parent = engine.made.find((each) => each.id === data?.sessionID)?.parentID
    for (const each of following.filter((each) => each.session === data?.sessionID || (parent && each.session === parent))) each.event(type, data)
  }
  engine.session("ses_work", [
    { id: "msg_0001", type: "user", text: "Make the timer pause on blur.", time: { created: 1 } },
    { id: "msg_0002", type: "assistant", content: [{ type: "text", text: "Done: it pauses when the window loses focus." }], time: { created: 2 } },
    { id: "msg_0003", type: "idle", outcome: "succeeded", time: { created: 3 } },
  ])
  engine.titles.set("ses_work", "Pause on blur")
  link = new EngineLink()
  link.configure(engine.url, "x")
  events = []
  unsubscribe = subscribe((event) => {
    events.push(event)
    beat()
  })
})

afterEach(() => {
  unsubscribe()
  link.configure(undefined, undefined)
  engine.stop()
  rmSync(root, { recursive: true, force: true })
})

describe("a side question", () => {
  test("is answered from the conversation's own context, kept in its list, and the conversation is told nothing", async () => {
    const questions = new SideQuestions(join(root, "btw.json"), places())
    questions.onAnswered = beat
    engine.answers = ["It uses the visibilitychange event."]
    const asked = await questions.ask("ses_work", "  Which event does it listen for?  ")
    expect(asked).toMatchObject({ question: "Which event does it listen for?", state: "answering" })
    await until(() => questions.list("ses_work")[0]?.state === "answered")
    expect(questions.list("ses_work")[0].answer).toBe("It uses the visibilitychange event.")
    // From inside the conversation (its own context), the question in the prompt.
    expect(engine.checks.map((check) => [check.kind, check.session])).toEqual([["own", "ses_work"]])
    expect(engine.checks[0].prompt).toContain("<question>\nWhich event does it listen for?\n</question>")
    expect(told()).toBe(0)
    // Clients heard it asked, then answered; the list beside the conversations has it.
    expect(events.filter((event) => event.type === "kuvo.btw.updated").map((event) => (event.data as any).question.state)).toEqual(["answering", "answered"])
    expect(questions.summary()).toMatchObject([{ session: "ses_work", count: 1, latest: { answer: "It uses the visibilitychange event." } }])
  })

  test("a follow-up follows on from its side thread; a new one starts afresh; a thread goes whole", async () => {
    const questions = new SideQuestions(join(root, "btw.json"), places())
    questions.onAnswered = beat
    engine.answers = ["visibilitychange.", "Safari 14 and later.", "A regex for durations."]
    const first = await questions.ask("ses_work", "Which event?")
    expect(first.thread).toBe(first.id)
    await until(() => questions.list("ses_work")[0]?.state === "answered")
    const followUp = (await (await sideQuestionRoutes(questions, "POST", ["ses_work"], async () => ({ question: "Which browsers have it?", thread: first.id }))).json() as any).data
    expect(followUp.thread).toBe(first.id)
    await until(() => questions.list("ses_work")[1]?.state === "answered")
    expect(engine.checks[1].prompt).toContain("Q: Which event?\nA: visibilitychange.")
    // Another /btw: a thread of its own, told nothing of the first.
    const other = await questions.ask("ses_work", "What does parseDuration match?")
    await until(() => questions.list("ses_work")[2]?.state === "answered")
    expect(other.thread).toBe(other.id)
    expect(engine.checks[2].prompt).not.toContain("visibilitychange")
    expect(questions.summary()).toEqual([expect.objectContaining({ session: "ses_work", count: 3, threads: 2 })])
    // A thread that isn't there, and a whole one taken away.
    expect((await sideQuestionRoutes(questions, "POST", ["ses_work"], async () => ({ question: "And?", thread: "btw_gone" }))).status).toBe(404)
    expect((await sideQuestionRoutes(questions, "DELETE", ["ses_work", "threads", first.id], async () => ({}))).status).toBe(200)
    expect(questions.list("ses_work").map((each) => each.question)).toEqual(["What does parseDuration match?"])
    expect((await sideQuestionRoutes(questions, "DELETE", ["ses_work", "threads", first.id], async () => ({}))).status).toBe(404)
  })

  test("its side threads are the box's to group, the latest first: a follow-up joins its thread, and their events say each change as it lands", async () => {
    // (Its own clock, a tick each time it's read, so which came last is never a tie.)
    let clock = 1_000
    const questions = new SideQuestions(join(root, "btw.json"), places(), () => ++clock)
    questions.onAnswered = beat
    engine.answers = ["visibilitychange."]
    const first = await questions.ask("ses_work", "Which event?")
    await until(() => questions.list("ses_work")[0]?.state === "answered")
    const client = new AbortController()
    const stream = await sideQuestionRoutes(questions, "GET", ["ses_work", "threads", "events"], async () => ({}), client.signal)
    expect(stream.headers.get("content-type")).toBe("application/x-ndjson")
    const reader = stream.body!.getReader()
    let buffer = ""
    const next = async (): Promise<any> => {
      for (;;) {
        const end = buffer.indexOf("\n")
        if (end >= 0) {
          const line = buffer.slice(0, end)
          buffer = buffer.slice(end + 1)
          return JSON.parse(line)
        }
        const { value, done } = await reader.read()
        if (done) return null
        buffer += new TextDecoder().decode(value)
      }
    }
    const answeredFirst = questions.list("ses_work")[0]!
    expect(await next()).toEqual({ threads: [{ id: first.id, title: "Which event?", asked: first.asked, latest: answeredFirst.answered, answering: false, questions: [answeredFirst] }] })

    // A follow-up, from another client: it joins that thread, which is being answered.
    engine.holdingChecks = true
    engine.answers = ["Safari 14 and later."]
    const followUp = (await (await sideQuestionRoutes(questions, "POST", ["ses_work"], async () => ({ question: "Which browsers have it?", thread: first.id }))).json() as any).data
    const asking = await next()
    expect(asking.threads).toHaveLength(1)
    expect(asking.threads[0]).toMatchObject({ id: first.id, title: "Which event?", latest: followUp.asked, answering: true })
    expect(asking.threads[0].questions.map((each: any) => [each.question, each.state])).toEqual([["Which event?", "answered"], ["Which browsers have it?", "answering"]])
    // Its answer lands: a line says so.
    await until(() => engine.checks.length === 2)
    engine.answerChecks()
    const answered = await next()
    expect(answered.threads[0].answering).toBe(false)
    expect(answered.threads[0].questions[1]).toMatchObject({ id: followUp.id, thread: first.id, state: "answered", answer: "Safari 14 and later." })
    expect(answered.threads[0].latest).toBe(answered.threads[0].questions[1].answered)

    // A new /btw: a thread of its own, the first now, titled with its question as asked.
    engine.answers = ["A regex for durations."]
    const other = await questions.ask("ses_work", "What does parseDuration match?")
    expect((await next()).threads.map((each: any) => [each.id, each.title, each.answering])).toEqual([[other.id, "What does parseDuration match?", true], [first.id, "Which event?", false]])
    await until(() => engine.checks.length === 3)
    engine.answerChecks()
    const both = await next()
    expect(both.threads.map((each: any) => [each.id, each.questions.length, each.answering])).toEqual([[other.id, 1, false], [first.id, 2, false]])
    // The list asked for once says the same.
    expect(await (await sideQuestionRoutes(questions, "GET", ["ses_work", "threads"], async () => ({}))).json()).toEqual(both)

    // A thread taken away, whole: one line, without it.
    expect((await sideQuestionRoutes(questions, "DELETE", ["ses_work", "threads", first.id], async () => ({}))).status).toBe(200)
    expect((await next()).threads.map((each: any) => each.id)).toEqual([other.id])
    // Its client gone: the stream ends.
    client.abort()
    expect(await next()).toBeNull()
    // A conversation with none has none.
    expect(await (await sideQuestionRoutes(questions, "GET", ["ses_other", "threads"], async () => ({}))).json()).toEqual({ threads: [] })
  })

  test("a follow-up asked while the one before it is answered (from another client) still sees it", async () => {
    const questions = new SideQuestions(join(root, "btw.json"), places())
    questions.onAnswered = beat
    engine.holdingChecks = true
    const first = await questions.ask("ses_work", "Which event?")
    await until(() => engine.checks.length === 1)
    await questions.ask("ses_work", "And which browsers?", first.id)
    await until(() => engine.checks.length === 2)
    expect(engine.checks[1].prompt).toContain("Q: Which event?\nA: (not answered yet)")
    engine.answerChecks()
    questions.stopAll()
  })

  test("asked on another model, with a text file, is answered there with both; a picture is refused", async () => {
    const questions = new SideQuestions(join(root, "btw.json"), places())
    questions.onAnswered = beat
    engine.answers = ["It's the 30-second debounce."]
    const notes = join(root, "notes.md")
    writeFileSync(notes, "Debounce: 30s")
    const pasted = "data:text/plain;base64," + Buffer.from("retry after 2s").toString("base64")
    const model = { providerID: "anthropic", id: "claude-opus-5-5", variant: "high" }
    const answer = await sideQuestionRoutes(questions, "POST", ["ses_work"], async () => ({ question: "Which delay?", model, files: [{ uri: "file://" + notes }, { uri: pasted, name: "log.txt" }] }))
    const asked = ((await answer.json()) as any).data
    // Clients see the files by name and the model it's on, not the files' words.
    expect(asked).toMatchObject({ model, files: ["notes.md", "log.txt"] })
    expect(asked.attached).toBeUndefined()
    await until(() => questions.list("ses_work")[0]?.state === "answered")
    // On that model, afresh, with the conversation and the files in the prompt.
    expect(engine.checks.map((check) => [check.kind, check.model])).toEqual([["fresh", model]])
    expect(engine.checks[0].prompt).toContain("Person: Make the timer pause on blur.")
    expect(engine.checks[0].prompt).toContain('<file name="notes.md">\nDebounce: 30s\n</file>')
    expect(engine.checks[0].prompt).toContain('<file name="log.txt">\nretry after 2s\n</file>')
    expect(told()).toBe(0)
    const archive = await sideQuestionRoutes(questions, "POST", ["ses_work"], async () => ({ question: "What's this?", files: [{ uri: "data:application/zip;base64,UEsDBAA=", name: "logs.zip" }] }))
    expect(archive.status).toBe(400)
    expect(((await archive.json()) as any).message).toContain("isn't text")
    expect(questions.list("ses_work")).toHaveLength(1)
  })

  test("asked with a picture, is answered in a turn of its own that sees it, with the conversation, and nothing of it kept", async () => {
    const questions = new SideQuestions(join(root, "btw.json"), places())
    questions.onAnswered = beat
    engine.answers = ["The error shows under the wrong field."]
    const picture = "data:image/png;base64,iVBORw0KGgo="
    const answer = await sideQuestionRoutes(questions, "POST", ["ses_work"], async () => ({ question: "What's wrong here?", files: [{ uri: picture, name: "shot.png" }] }))
    expect(answer.status).toBe(200)
    expect(((await answer.json()) as any).data).toMatchObject({ files: ["shot.png"], state: "answering" })
    await until(() => questions.list("ses_work")[0]?.state === "answered")
    expect(questions.list("ses_work")[0]!.answer).toBe("The error shows under the wrong field.")
    // On the conversation's own model, the picture with the question and the conversation as a record.
    expect(engine.checks).toHaveLength(1)
    const [check] = engine.checks
    const made = engine.made.find((each) => each.id === check!.session)
    expect(made).toMatchObject({ model: { providerID: "p", id: "m" }, metadata: { kuvo: { engineFor: "ses_work" } } })
    expect(check!.files).toEqual([{ uri: picture, name: "shot.png" }])
    expect(check!.prompt).toContain("Person: Make the timer pause on blur.")
    expect(check!.prompt).toContain("<question>\nWhat's wrong here?\n</question>")
    // Its session gone after, the conversation told nothing, and the picture not kept once answered.
    expect(engine.deleted).toContain(check!.session!)
    expect(told()).toBe(0)
    expect(readFileSync(join(root, "btw.json"), "utf8")).not.toContain("iVBORw0KGgo")
    questions.stopAll()
  })

  test("about a coding agent's conversation is answered with the conversation in the question", async () => {
    const questions = new SideQuestions(join(root, "btw.json"), places(true))
    questions.onAnswered = beat
    engine.answers = ["The timer pauses."]
    await questions.ask("ses_work", "What changed?")
    await until(() => questions.list("ses_work")[0]?.state === "answered")
    expect(engine.checks[0].prompt).toContain("<conversation>")
    expect(engine.checks[0].prompt).toContain("Person: Make the timer pause on blur.")
  })

  test("waiting as the box stopped is answered once it's back; routes say what can't be", async () => {
    const before = new SideQuestions(join(root, "btw.json"), places())
    engine.holdingChecks = true
    await before.ask("ses_work", "Which event?")
    await until(() => engine.checks.length === 1)
    before.stopAll()
    engine.holdingChecks = false
    engine.answers = ["Answered after the restart."]
    const after = new SideQuestions(join(root, "btw.json"), places())
    after.onAnswered = beat
    expect(after.list("ses_work")[0].state).toBe("answering")
    after.start()
    await until(() => after.list("ses_work")[0]?.state === "answered")
    expect(after.list("ses_work")[0].answer).toBe("Answered after the restart.")
    engine.answerChecks()

    const route = (method: string, path: string, value?: unknown) => sideQuestionRoutes(after, method, path.split("/").filter(Boolean), async () => value)
    expect((await route("POST", "ses_work", { question: " " })).status).toBe(400)
    expect((await route("POST", "ses_nowhere", { question: "Hi?" })).status).toBe(404)
    expect(((await (await route("GET", "ses_work")).json()) as any).data).toHaveLength(1)
    expect((await route("DELETE", "ses_work/" + after.list("ses_work")[0].id)).status).toBe(200)
    expect(after.list("ses_work")).toHaveLength(0)
    expect((await route("DELETE", "ses_work")).status).toBe(404)
  })
})

// "Branch from here": its place makes the copy, and the box marks where it came from, so every
// client shows it under that conversation, as it does a handoff's.
describe("a branch", () => {
  const route = (session: string, before?: string, bridge = false) => branchRoutes(places(bridge), "POST", [session], async () => (before ? { before } : {}))

  test("has the words before the message, and says where it came from, on the engine and with the coding agents alike", async () => {
    for (const bridge of [false, true]) {
      const answer = await route("ses_work", "msg_0002", bridge)
      expect(answer.status).toBe(200)
      const made = ((await answer.json()) as any).data
      expect(engine.conversations.get(made.id)!.map((message) => message.id)).toEqual(["msg_0001"])
      expect(made.metadata).toEqual({ kuvo: { branchedFrom: "ses_work" } })
      expect(engine.metadata.get(made.id)).toEqual({ kuvo: { branchedFrom: "ses_work" } })
    }
  })

  test("a branch of a pinned, handed-off conversation came from that one, and starts unpinned", async () => {
    engine.metadata.set("ses_work", { "kuvo.pinned": true, color: "blue", kuvo: { handoffFrom: "ses_old", handoffTo: "ses_new", branchedFrom: "ses_older", kuvoAgent: "claude" } })
    const made = ((await (await route("ses_work", "msg_0002", true)).json()) as any).data
    expect(engine.metadata.get(made.id)).toEqual({ color: "blue", kuvo: { kuvoAgent: "claude", branchedFrom: "ses_work" } })
    expect(engine.metadata.get("ses_work").kuvo.handoffTo).toBe("ses_new")
  })

  test("a branch of a session an agent started came from the one it's branched from, and opens nowhere by itself", async () => {
    engine.metadata.set("ses_work", { kuvo: { startedBy: "ses_lead", beside: true } })
    const made = ((await (await route("ses_work", "msg_0002")).json()) as any).data
    expect(made.metadata).toEqual({ kuvo: { branchedFrom: "ses_work" } })
    expect(engine.metadata.get(made.id)).toEqual({ kuvo: { branchedFrom: "ses_work" } })
  })

  test("a subagent's, a message that isn't there, and no message at all are turned away", async () => {
    engine.session("ses_sub", [{ id: "msg_0001", type: "user", text: "Find it.", time: { created: 1 } }])
    engine.parents.set("ses_sub", "ses_work")
    expect((await route("ses_sub", "msg_0001")).status).toBe(409)
    expect((await route("ses_work", "msg_9999")).status).toBe(404)
    expect((await route("ses_work")).status).toBe(400)
    expect((await route("ses_gone", "msg_0001")).status).toBe(404)
    expect([...engine.conversations.keys()].filter((id) => id.startsWith("ses_fork"))).toHaveLength(0)
  })
})

describe("a conversation deleted", () => {
  test("takes its side questions with it", async () => {
    const questions = new SideQuestions(join(root, "side-questions.json"), places())
    questions.onAnswered = beat
    engine.answers = ["It pauses on blur."]
    await questions.ask("ses_work", "What does it do?")
    await until(() => questions.list("ses_work")[0]?.state === "answered")
    deleted("ses_work")
    expect(questions.list("ses_work")).toHaveLength(0)
    expect(following).toHaveLength(0)
  })

  test("while the box was stopped, they go once it's back", async () => {
    const question = { id: "btw_1", session: "ses_gone", question: "Why?", state: "answered", answer: "Because.", asked: 1, answered: 2 }
    writeFileSync(join(root, "side-questions.json"), JSON.stringify([question, { ...question, id: "btw_2", session: "ses_work" }]))
    const questions = new SideQuestions(join(root, "side-questions.json"), places())
    await until(() => questions.list("ses_gone").length === 0)
    expect(questions.list("ses_work")).toHaveLength(1)
  })
})
