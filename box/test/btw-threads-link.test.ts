import "./safety"
import { afterAll, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { follow } from "./route-answer"

// A conversation's side threads followed through the box's link (`kuvo/agents/api/kuvo/btw/<session>/
// threads/events`, PROTOCOL.md "Side questions"): the coding agents' bridge streams them, and the
// link passes each line on as it comes, so a client sees an answer land without asking again. The
// box's real link; the bridge a stand-in that streams as the real one does (test/agents/keywords.test.ts
// holds the bridge's own side).

const { engineHandler } = await import("../src/link/server/router")

const thread = (answering: boolean) => ({
  threads: [{
    id: "btw_1", title: "Which event?", asked: 1, latest: answering ? 1 : 2, answering,
    questions: [{ id: "btw_1", session: "ses_work", thread: "btw_1", question: "Which event?", state: answering ? "answering" : "answered", asked: 1, ...(answering ? {} : { answer: "visibilitychange.", answered: 2 }) }],
  }],
})

let send: ((line: unknown) => void) | undefined
let asked = ""
const bridge = Bun.serve({ port: 0, hostname: "127.0.0.1", idleTimeout: 0, fetch: request => {
  asked = new URL(request.url).pathname
  if (asked !== "/api/kuvo/btw/ses_work/threads/events") return Response.json({ message: "Not available for this agent." }, { status: 404 })
  return new Response(new ReadableStream<Uint8Array>({
    start(controller) {
      send = line => controller.enqueue(new TextEncoder().encode(JSON.stringify(line) + "\n"))
      send(thread(true))
    },
  }), { headers: { "content-type": "application/x-ndjson" } })
} })
// (The bridge's record names a process that's it: one of this test's own, stopped at its end.)
const bridgeProcess = spawn(process.execPath, ["-e", "setTimeout(() => {}, 120_000)", "kuvo-agents"], { stdio: "ignore" })
afterAll(() => { bridge.stop(true); bridgeProcess.kill() })

const noEngine = () => { throw new Error("No engine") }
const handler = engineHandler(noEngine as any, { name: "test" } as any, () => ({ port: bridge.port, password: "test", pid: bridgeProcess.pid }) as any)

test("side threads followed through the link: each line the bridge sends, as it sends it", async () => {
  const events = follow(handler, "kuvo/agents/api/kuvo/btw/ses_work/threads/events")
  try {
    expect(await events.next()).toEqual(thread(true))
    expect(asked).toBe("/api/kuvo/btw/ses_work/threads/events")
    // The answer lands on the bridge: the client hears it on the same stream.
    send!(thread(false))
    expect(await events.next()).toEqual(thread(false))
  } finally {
    events.stop()
  }
})
