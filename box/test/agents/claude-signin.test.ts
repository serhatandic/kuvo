import "../safety"
import { expect, test } from "bun:test"
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { SignInAttempt } from "../../src/agents/agent"
import { ClaudeAgent } from "../../src/agents/agents/claude"

const token = "sk-ant-oat01-" + "fixture".repeat(12)

test.each(["default", "work-123"])("setup-token captures a separately submitted code for %s", async id => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-setup-token-"))
  const gate = join(home, "token-ready")
  const executable = join(home, "claude")
  writeFileSync(executable, `#!${process.execPath}
const { existsSync } = require("node:fs")
if (process.argv[2] !== "setup-token") process.exit(1)
process.stdin.setRawMode(true)
process.stdout.write("https://sign-in.example.invalid/claude\\r\\n")
const prompt = () => process.stdout.write("\\x1b[2J\\x1b[H Paste code here if prompted\\r\\n │ │\\r")
let stage = "starting"
setTimeout(() => { stage = "prompt"; prompt() }, 500)
process.stdin.on("data", async chunk => {
  const text = chunk.toString()
  if (stage === "prompt" && /^\\x1b\\[200~(?:short|valid#state)\\x1b\\[201~$/.test(text)) {
    stage = "processing"
    await Bun.sleep(250)
    if (stage !== "processing") return
    stage = text.includes("#") ? "ready" : "incomplete"
    prompt()
    process.stdout.write("\\x1b[2C**** │\\r")
  } else if (stage === "incomplete" && text === "\\r") {
    stage = "prompt"
    prompt()
    process.stdout.write("\\r\\n✘ That doesn't look like the full code.\\r\\n   Copy all of it, including the # in the middle.\\r\\n")
  } else if (stage === "ready" && text === "\\r") {
    stage = "waiting"
    while (!existsSync(${JSON.stringify(gate)})) await Bun.sleep(20)
    process.stdout.write("\\r\\n${token}\\r\\n")
  } else {
    stage = "bad input"
    process.exit(2)
  }
})
`, { mode: 0o700 })
  const keychain = ClaudeAgent.keychain
  ClaudeAgent.keychain = true
  let attempt: SignInAttempt | undefined
  try {
    const bridge = join(home, ".kuvo-code", "agents")
    class FakeClaude extends ClaudeAgent {
      override executable() { return executable }
    }
    const claude = new FakeClaude({ home: bridge, temporary: home, deps: {} as never }, { id })
    const file = join(bridge, "claude", "tokens", `${id}.json`)
    const started = Date.now()
    const observed: { status: string | undefined; token: string }[] = []
    attempt = await claude.signIn("claudeai", {
      remote: true,
      onPlanToken: () => observed.push({ status: attempt?.status().status, token: claude.token() ?? "" }),
    })
    expect(attempt.url).toBe("https://sign-in.example.invalid/claude")
    expect(attempt.acceptsCode).toBe(true)
    await expect(attempt.complete!("short")).rejects.toThrow("Copy all of it")
    expect(attempt.status().status).toBe("pending")
    expect(existsSync(file)).toBe(false)

    await attempt.complete!("valid#state")
    expect(attempt.status().status).toBe("pending")
    expect(existsSync(file)).toBe(false)
    writeFileSync(gate, "")
    const deadline = performance.now() + 3_000
    while (attempt.status().status === "pending" && performance.now() < deadline) await Bun.sleep(25)
    expect(attempt.status()).toEqual({ status: "complete" })
    expect(observed).toEqual([{ status: "pending", token }])
    const saved = JSON.parse(readFileSync(file, "utf8"))
    expect(saved).toEqual({ token, made: expect.any(Number) })
    expect(saved.made).toBeGreaterThanOrEqual(started)
    expect(saved.made).toBeLessThanOrEqual(Date.now())
    expect(statSync(file).mode & 0o777).toBe(0o600)
  } finally {
    if (attempt?.status().status === "pending") attempt.cancel()
    ClaudeAgent.keychain = keychain
    rmSync(home, { recursive: true, force: true })
  }
}, 15_000)
