// What a client reads of a conversation's messages, as the box forwards a page of them (PROTOCOL.md,
// "Tool steps" and "Its sessions' news"): each tool step that isn't the engine's own says what it
// did in words (never a tool's id, never "kuvo-browser"), and a note telling a lead of its sessions
// carries each one's news.

import "./safety"
import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { TOOLS } from "../src/browser/tools"
import { messageWordsPage } from "../src/shared/message-words"
import { SANDBOX_TOOLS } from "../src/link/fence/outside"
import { knownTools } from "../src/link/server/known-tools"
import { VAULT_TOOLS } from "../src/link/vault/tools"
import { stoppedLine, wakeText } from "../src/link/work/wakes"
import { QUEUE_TOOLS } from "../src/link/work-queue/tools"

// This box's engine settings: an integration (Linear), a catalog server (Notion), and one of yours.
const settings = { mcp: { servers: { "kuvo-browser": {}, linear: {}, notion: {}, "my-server": {}, "my-server-two": {} } } }
const step = (id: string, name: string, status: string, input?: Record<string, unknown>) =>
  ({ type: "tool", id, name, state: { status, ...(input ? { input } : {}) } })
const page = (...data: unknown[]) => ({ data, cursor: { next: "older" } })
const read = (value: unknown, known = knownTools(settings)) => messageWordsPage(value, known) as { data: any[]; cursor: unknown }
const steps = (value: unknown) => read(page({ type: "assistant", id: "m1", content: (value as unknown[]) })).data[0].content

describe("tool steps", () => {
  test("Kuvo Code's own say what they did, and what they do while they run", () => {
    const [started, starting, limits] = steps([
      step("c1", "kuvo-browser_start_session", "completed", { task: "Write the tests" }),
      step("c2", "kuvo-browser_start_session", "running", { task: "Write the tests" }),
      step("c3", "kuvo-browser_plan_limits", "completed", {}),
    ])
    expect([started.title, started.doing]).toEqual(["Started a session", "Starting a session"])
    expect(starting.title).toBe("Started a session")
    expect(starting.doing).toBe("Starting a session")
    expect(limits.title).toBe("Checked the plan's limits")
    // The rest of the step as the engine gave it.
    expect(started).toMatchObject(step("c1", "kuvo-browser_start_session", "completed", { task: "Write the tests" }))
  })

  test("a message's recipient is named from the call, but never by its address, nor before the call has it", () => {
    const [named, mentioned, address, streaming, reading] = steps([
      step("c1", "kuvo-browser_send_message", "completed", { to: "Mira", text: "Done?" }),
      step("c2", "kuvo-browser_send_message", "running", { to: "&Mira", text: "Done?" }),
      step("c3", "kuvo-browser_send_message", "completed", { to: "ses_abc123", text: "Done?" }),
      step("c4", "kuvo-browser_send_message", "pending"),
      step("c5", "kuvo-browser_read_conversation", "completed", { of: "Mira" }),
    ])
    expect(named.title).toBe("Sent a message to Mira")
    expect(mentioned.doing).toBe("Sending a message to Mira")
    expect(address.title).toBe("Sent a message")
    expect([streaming.title, streaming.doing]).toEqual(["Sent a message", "Sending a message"])
    expect(reading.title).toBe("Read Mira’s conversation")
  })

  test("the vault's, the sandbox's and the Work Queue's have words too", () => {
    const [vault, sandbox, queue] = steps([
      step("c1", "kuvo-browser_vault_run", "completed", { command: "deploy" }),
      step("c2", "kuvo-browser_sandbox_ask", "running", { command: "brew install jq" }),
      step("c3", "kuvo-browser_queue_add", "completed", { title: "Fix the sign-in" }),
    ])
    expect(vault.title).toBe("Ran a command with vault values")
    expect(sandbox.doing).toBe("Asking to run a command outside the sandbox")
    expect(queue.title).toBe("Added “Fix the sign-in” to the Work Queue")
  })

  test("another MCP server's are its name and the tool's, as people know them", () => {
    const [linear, notion, yours, longer, unknown, ownUnknown] = steps([
      step("c1", "linear_list_issues", "completed", {}),
      step("c2", "notion_notion-search", "running", {}),
      step("c3", "my-server_do_thing", "completed", {}),
      step("c4", "my-server-two_getIssue", "completed", {}),
      step("c5", "acme_fetch_report", "completed", {}),
      step("c6", "kuvo-browser_some_new_tool", "completed", {}),
    ])
    expect([linear.title, linear.doing]).toEqual(["Linear · List issues", "Linear · List issues"])
    expect(notion.title).toBe("Notion · Search")
    expect(yours.title).toBe("My server · Do thing")
    // The longest server name it starts with.
    expect(longer.title).toBe("My server two · Get issue")
    // A server this box doesn't know: the whole name.
    expect(unknown.title).toBe("Acme fetch report")
    // One of Kuvo Code's this box has no words for (a newer one's): its own name, not its server's.
    expect(ownUnknown.title).toBe("Some new tool")
    for (const each of [linear, notion, yours, longer, unknown, ownUnknown]) expect(`${each.title} ${each.doing}`).not.toContain("kuvo-browser")
  })

  test("the engine's own tools are left for clients to word, and so are other messages", () => {
    const content = [
      { type: "text", text: "On it." },
      step("c1", "shell", "completed", { command: "ls" }),
      step("c2", "read", "completed", { filePath: "a.ts" }),
      step("c3", "subagent", "running", { description: "Look" }),
      step("c4", "todowrite", "completed", { todos: [] }),
    ]
    const user = { type: "user", id: "m0", text: "Hi" }
    const assistant = { type: "assistant", id: "m1", content }
    expect(read(page(user, assistant))).toEqual(page(user, assistant))
    // A single message reads the same.
    expect((messageWordsPage({ data: { type: "assistant", id: "m2", content: [step("c5", "linear_list_issues", "completed", {})] } }, knownTools(settings)) as any).data.content[0].title).toBe("Linear · List issues")
  })

  test("a failed step reads as one short line, whatever agent it came from, and keeps all it said", () => {
    const failed = (id: string, name: string, input: Record<string, unknown>, error: unknown, extra: Record<string, unknown> = {}) =>
      ({ type: "tool", id, name, state: { status: "error", input, error, ...extra } })
    // Claude Code's harness refusing a command, as its bridge keeps it: tags, paths and advice.
    const refused = "<tool_use_error>Blocked: sleep 240 followed by: cd /Users/me/work scripts/heavy.sh status tail -3 /private/tmp/claude-501/tasks/b6.output. To wait for a condition, use Monitor with an until-loop (e.g. `until <check>; do sleep 2; done`). Do not chain shorter sleeps to work around this block.</tool_use_error>"
    const [blocked, declined, stopped, missing, exited, engine, mcp, printed, silent] = steps([
      failed("c1", "shell", { command: "sleep 240; cd /Users/me/work" }, { type: "tool", message: refused }),
      failed("c2", "edit", { filePath: "/Users/me/a.ts" }, { type: "tool", message: "The user doesn't want to proceed with this tool use. The tool use was rejected." }),
      failed("c3", "shell", { command: "bun test" }, { type: "aborted", message: "Stopped." }),
      failed("c4", "read", { filePath: "/Users/me/gone.ts" }, { type: "tool", message: "<tool_use_error>File does not exist. Note: your current working directory is /Users/me/work.</tool_use_error>" }),
      failed("c5", "shell", { command: "false" }, { type: "tool", message: "Exit code 1\nnpm ERR! missing script: lint" }),
      // The engine's own (opencode): an `Error:` string.
      failed("c6", "grep", { pattern: "x" }, "Error: ripgrep failed to start"),
      failed("c7", "linear_list_issues", {}, "MCP error: Error: Request timed out"),
      // Codex and Gemini say it only in what they printed.
      { type: "tool", id: "c8", name: "shell", state: { status: "failed", input: { command: "rm x" }, content: [{ type: "text", text: "rm: x: Permission denied" }] } },
      failed("c9", "webfetch", { url: "https://a.example" }, { type: "tool", message: "" }),
    ])
    expect(blocked.failure).toEqual({
      line: "Ran 1 command · blocked",
      reason: "blocked",
      error: refused.replace("<tool_use_error>", "").replace("</tool_use_error>", ""),
    })
    expect(declined.failure.line).toBe("Edited 1 file · declined")
    expect(stopped.failure.line).toBe("Ran 1 command · stopped")
    expect(missing.failure.line).toBe("Read 1 file · not found")
    expect(exited.failure.line).toBe("Ran 1 command · exit code 1")
    expect(engine.failure).toMatchObject({ line: "Searched once · failed", error: "ripgrep failed to start" })
    expect(mcp.failure).toMatchObject({ line: "Linear · List issues · timed out", error: "Request timed out" })
    expect(printed.failure.line).toBe("Ran 1 command · blocked")
    expect(silent.failure).toEqual({ line: "Read 1 web page · failed", reason: "failed", error: "It failed without saying why." })
    // Its line never carries a tag or a path, and the engine's words stay as they were.
    for (const each of [blocked, declined, stopped, missing, exited, engine, mcp, printed, silent]) {
      expect(each.failure.line).not.toMatch(/[<>/]/)
      expect(each.state.status).toMatch(/error|failed/)
    }
    // A step that didn't fail has none.
    expect(steps([step("c10", "shell", "completed", { command: "ls" })])[0].failure).toBeUndefined()
  })

  test("every one of Kuvo Code's tools has its title and its words", () => {
    const { own } = knownTools(settings)
    const listed = [...TOOLS, ...VAULT_TOOLS, ...SANDBOX_TOOLS, ...QUEUE_TOOLS] as { name: string; title?: string }[]
    for (const tool of listed) {
      const words = own.get(tool.name)
      expect(words, tool.name).toBeDefined()
      expect(tool.title, tool.name).toBe(words!.title)
      for (const said of [words!.title, words!.done, words!.doing]) expect(said.trim().length, tool.name).toBeGreaterThan(0)
      // Listed as MCP has it: its title after its name.
      expect(Object.keys(tool).slice(0, 2), tool.name).toEqual(["name", "title"])
    }
    expect(own.size).toBe(listed.length)
  })
})

describe("its sessions' news", () => {
  const session = (id: string, title: string) => ({ id, title, directory: "/home/me/web" })
  const finished = `[Session finished] "Sign-in" in web (address ses_one), after 3 min:\nThe sign-in works now.\n(Still serving: http://localhost:3000.)`
  const silent = `[Session finished] "" in web (address ses_two), after under a minute:\n(It ended without a reply.)`
  const asks = `[Session asks] "Deploy" in web (address ses_three) asks to run git push (ask per_1). Answer with answer_ask if it's yours to allow, or tell the user in a line and carry on.`
  const failed = `[Session failed] "Tests" in web (address ses_four) stopped with an error: The model is overloaded. Read it with read_conversation, then start it again, try another way, or tell the user.`
  const working = `[Session still working] "Build" in web (address ses_five):\nStill on it.`
  const notSummarized = `[Session not summarized] "Docs" in web (address ses_six): The engine refused. Its conversation and work are as they were.`
  const note = (text: string, metadata?: unknown) => ({ type: "user", id: "m1", text, ...(metadata === undefined ? {} : { metadata }) })
  const marked = { kuvo: { note: { from: "Its sessions", line: "6 updates from its sessions" } }, other: 1 }

  test("a note tells each session's news, in order, as its card shows it", () => {
    const text = wakeText([finished, silent, asks, failed, working, notSummarized, stoppedLine(session("ses_seven", "Cleanup"))])
    const [message] = read(page(note(text, marked))).data
    expect(message.metadata.kuvo.news).toEqual([
      { session: "ses_one", title: "Sign-in", status: "finished", said: "The sign-in works now." },
      { session: "ses_two", title: "New Session", status: "finished", said: "It ended without a reply." },
      { session: "ses_three", title: "Deploy", status: "asks", said: "Asks to run git push." },
      { session: "ses_four", title: "Tests", status: "stopped", said: "Stopped with an error: The model is overloaded." },
      { session: "ses_six", title: "Docs", status: "not summarized", said: "The engine refused." },
      { session: "ses_seven", title: "Cleanup", status: "stopped by you", said: "You stopped it." },
    ])
    // The rest as it was.
    expect(message.text).toBe(text)
    expect(message.metadata.kuvo.note).toEqual(marked.kuvo.note)
    expect(message.metadata.other).toBe(1)
  })

  test("one from before notes were marked is read as clients read it", () => {
    const question = `[Session asks] "Plan" (address ses_q) asks: Which database? (ask frm_1). Answer with answer_ask (answer), or tell the user in a line and carry on.`
    const [first, second] = read(page(note(finished), note(question))).data
    expect(first.metadata).toEqual({ kuvo: { news: [{ session: "ses_one", title: "Sign-in", status: "finished", said: "The sign-in works now." }] } })
    expect(second.metadata.kuvo.news).toEqual([{ session: "ses_q", title: "Plan", status: "asks", said: "Which database?" }])
  })

  test("only a session's still working tells nothing, and other messages aren't read as news", () => {
    const [still, older, goal] = read(page(
      note(working, marked),
      note(`[Session finished] "Sign-in" (address ses_one):\nDone.`),
      note("[Goal started] The user approved the plan.", { kuvo: { note: { from: "Goal", line: "Goal started" } } }),
    )).data
    expect(still.metadata.kuvo.news).toEqual([])
    expect(older.metadata.kuvo.news).toEqual([{ session: "ses_one", title: "Sign-in", status: "finished", said: "Done." }])
    expect(goal.metadata.kuvo.news).toBeUndefined()
    // A user's own message that only mentions one isn't touched.
    const plain = note(`See [Session finished] "X" (address ses_x)`)
    expect(read(page(plain)).data[0]).toEqual(plain)
  })
})

// The cases every client reads (box/test/fixtures/message-words.json), each on its own page.
const vectors = JSON.parse(readFileSync(new URL("fixtures/message-words.json", import.meta.url), "utf8"))
describe("protocol vectors", () => {
  const known = knownTools({ mcp: { servers: Object.fromEntries(vectors.servers.map((name: string) => [name, {}])) } })
  for (const each of vectors.cases) test(each.name, () => {
    expect(read({ data: [each.message] }, known).data[0]).toEqual(each.read)
  })
})

// A group chat's bot at work shows its tool step in the same words, never by the tool's id.
test("a group chat says what a bot's tool step does in words", async () => {
  const { activity } = await import("../src/agents/groups/rules");
  const step = (name: string, input: unknown) => [{ type: "assistant", id: "m1", time: { created: 10 }, content: [{ type: "tool", id: "t1", name, state: { status: "running", input } }] }];
  expect(activity(step("kuvo-browser_start_session", {}), "", 0)[0]).toMatchObject({ kind: "tool", call: { title: "Started a session", doing: "Starting a session" } });
  expect(activity(step("linear_list_issues", {}), "", 0)[0]).toMatchObject({ kind: "tool", call: { doing: "Linear · List issues" } });
  // The engine's own are worded by clients.
  expect((activity(step("read", { filePath: "/a" }), "", 0)[0] as { call: { title?: string } }).call.title).toBeUndefined();
});
