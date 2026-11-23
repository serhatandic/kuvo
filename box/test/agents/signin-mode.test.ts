// Whether a coding agent's sign-in ends on a code to paste here, as the box says it
// (`api/integration/<id>/connect/oauth`, PROTOCOL.md "Signing in to a coding agent"): `mode: "code"`
// while the page in use is the one its command printed (which ends on a code), `auto` where its
// page comes back by itself; and the line beside it says which. Real: the bridge's routes, Claude
// Code's agent and the sign-in machinery. Stand-ins: Claude Code's program (a script on the PATH's
// place that prints its page, hands one to its browser, and waits for a code), and, for a page that
// comes late, an agent whose program prints its page after the attempt was handed back.
// (Antigravity's sign-in is always its printed page: its own test, antigravity-agy.test.ts, covers it.)

import "../safety"
import { afterAll, expect, test } from "bun:test"
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { Agent } from "../../src/agents/agent"
import { ClaudeAgent } from "../../src/agents/agents/claude"
import { handler } from "../../src/agents/server"
import { commandSignIn } from "../../src/agents/signin"

const root = mkdtempSync(join(tmpdir(), "kuvo-signin-mode-"))
afterAll(() => rmSync(root, { recursive: true, force: true }))

const PRINTED = "https://claude.ai/oauth/authorize?code=true&redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback"
const HANDED = "https://claude.ai/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback"

/** Claude Code as `claude auth login` runs: its printed page, the one it hands its browser, then its prompt for a code. */
const claudeProgram = () => {
  const file = join(root, "claude")
  writeFileSync(file, `#!/bin/sh
case "$1 $2" in
  "auth status") echo '{"loggedIn":false,"authMethod":"none"}'; exit 1 ;;
  "auth login")
    echo "Opening browser to sign in…"
    echo "If the browser didn't open, visit: ${PRINTED}"
    "$BROWSER" "${HANDED}"
    printf "Paste code here if prompted > "
    read code
    ;;
esac
`)
  chmodSync(file, 0o755)
  return file
}

const context = () => {
  const home = mkdtempSync(join(root, "agent-"))
  return { home, temporary: home, deps: {} as any }
}

function bridge(agent: Agent) {
  const answer = handler({ roster: { all: () => [agent], find: (id: string) => (id === agent.providerID ? agent : undefined) }, password: "pw" } as any)
  const ask = async (route: string, method = "GET", body?: unknown) => {
    const response = await answer(new Request(`http://box/api/integration/${agent.providerID}${route}`, {
      method,
      headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    }))
    return ((await response.json()) as any).data
  }
  return {
    start: (body: unknown) => ask("/connect/oauth", "POST", body),
    status: (attempt: string) => ask(`/connect/oauth/${attempt}`),
    cancel: (attempt: string) => ask(`/connect/oauth/${attempt}`, "DELETE"),
  }
}

class StandInClaude extends ClaudeAgent {
  standIn = claudeProgram()
  override executable() {
    return this.standIn
  }
}

test("Claude Code's page opened on another device ends on a code: the box says so, and how", async () => {
  const box = bridge(new StandInClaude(context()))
  const started = await box.start({ methodID: "claudeai", remote: true })
  try {
    expect(started).toMatchObject({ url: PRINTED, mode: "code", acceptsCode: true, instructions: "Sign in on Claude’s page, then paste its code here." })
    expect(await box.status(started.attemptID)).toMatchObject({ status: "pending", mode: "code", acceptsCode: true })
  } finally {
    await box.cancel(started.attemptID)
  }
})

test("Claude Code's page handed to the browser here comes back by itself: no code asked for", async () => {
  const box = bridge(new StandInClaude(context()))
  const started = await box.start({ methodID: "claudeai" })
  try {
    // (A code can still be pasted, for older clients, should its page give one.)
    expect(started).toMatchObject({ url: HANDED, mode: "auto", acceptsCode: true, instructions: "Finish signing in on Claude’s page." })
    expect(await box.status(started.attemptID)).toMatchObject({ status: "pending", mode: "auto" })
  } finally {
    await box.cancel(started.attemptID)
  }
})

test("a printed page that comes late, with nothing handed to the browser, turns the sign-in's status to a code", async () => {
  const home = mkdtempSync(join(root, "late-"))
  const go = join(home, "go")
  const script = join(home, "login")
  // Slow to start: it prints its page (never handing one to a browser) once the test says.
  writeFileSync(script, `#!/bin/sh\nwhile [ ! -f "${go}" ]; do sleep 0.05; done\necho "Visit: ${PRINTED}"\nprintf "Paste code here if prompted > "\nread code\n`)
  chmodSync(script, 0o755)
  const agent = {
    id: "claude", providerID: "claude-code", name: "Claude Code", signsInRemotely: true,
    signIn: () => commandSignIn({
      command: script, args: [], home, temporary: home, signedIn: async () => false, pageWait: 0,
      instructions: "Finish signing in on Claude’s page.",
      codeInstructions: "Sign in on Claude’s page, then paste its code here.",
      codePrompt: /Paste code here if prompted >\s*/g,
    }),
  } as unknown as Agent
  const box = bridge(agent)
  const started = await box.start({ methodID: "claudeai" })
  try {
    expect(started).toMatchObject({ url: "", mode: "auto", instructions: "Finish signing in on Claude’s page." })
    writeFileSync(go, "")
    let status = await box.status(started.attemptID)
    for (let waited = 0; waited < 15_000 && !status.url; waited += 100) {
      await Bun.sleep(100)
      status = await box.status(started.attemptID)
    }
    expect(status).toMatchObject({ status: "pending", url: PRINTED, mode: "code", acceptsCode: true, instructions: "Sign in on Claude’s page, then paste its code here." })
  } finally {
    await box.cancel(started.attemptID)
  }
})
