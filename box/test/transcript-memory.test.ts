// Listing saved coding-agent conversations reads the bridge's real session store.
import "./safety"
import { expect, test } from "bun:test"
import { closeSync, mkdirSync, mkdtempSync, openSync, rmSync, writeFileSync, writeSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Store } from "../src/agents/store"

test("listing conversations does not parse their saved pictures", () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-transcript-"))
  try {
    const root = join(home, "sessions")
    const picture = "QUJD".repeat(512 * 1024)
    for (let n = 0; n < 64; n++) {
      const folder = join(root, "ses_" + n)
      mkdirSync(folder, { recursive: true })
      writeFileSync(join(folder, "session.json"), JSON.stringify({ id: "ses_" + n, title: "Conversation " + n }))
      const transcript = openSync(join(folder, "messages.json"), "w")
      try {
        writeSync(transcript, "[" + JSON.stringify({
          id: "msg_" + n, type: "user", time: { created: n }, text: "A picture",
          files: [{ mime: "image/png", source: { type: "uri", uri: "data:image/png;base64," + picture } }],
        }) + "]")
      } finally { closeSync(transcript) }
    }
    Bun.gc(true)
    const before = process.memoryUsage().rss
    const store = new Store(root)
    expect(store.all().map(session => session.title).sort()).toEqual(
      Array.from({ length: 64 }, (_, n) => "Conversation " + n).sort(),
    )
    Bun.gc(true)
    const growth = process.memoryUsage().rss - before
    console.log("Listing RSS growth (MiB)", Math.round(growth / 1024 ** 2))
    expect(growth).toBeLessThan(64 * 1024 ** 2)
  } finally { rmSync(home, { recursive: true, force: true }) }
})

test("Insights reads usage in a saved reply even when a tool returned a large document", () => {
  const folder = mkdtempSync(join(tmpdir(), "kuvo-usage-memory-"))
  try {
    const file = openSync(join(folder, "messages.json"), "w")
    const block = "a".repeat(1024 ** 2)
    try {
      writeSync(file, '[{"type":"assistant","id":"msg_usage","agent":"ada","time":{"created":1,"completed":2},' +
        '"model":{"id":"claude-test","providerID":"claude-code"},"tokens":{"input":12,"output":3},' +
        '"content":[{"type":"tool","id":"large-result","name":"shell","state":{"status":"completed","content":[{"type":"text","text":"')
      for (let n = 0; n < 48; n++) writeSync(file, block)
      writeSync(file, '"}]}},{"type":"tool","id":"skill","name":"skill","state":{"status":"completed",' +
        '"input":{"skill":"paper \\"résumé\\""},"content":[]}}]}]')
    } finally { closeSync(file) }

    const result = Bun.spawnSync({
      cmd: [process.execPath, "--config=/dev/null", "--no-env-file", "-e", `
        import { savedMessages } from ${JSON.stringify(join(import.meta.dir, "../src/agents/transcript.ts"))}
        const before = process.resourceUsage().maxRSS
        const [reply] = savedMessages(process.argv.at(-1), "insight")
        console.log(JSON.stringify({
          growth: process.resourceUsage().maxRSS - before,
          tokens: reply.tokens,
          tools: reply.content.map(part => [part.name, part.state.input])
        }))
      `, folder],
      stdout: "pipe", stderr: "pipe",
    })
    expect(result.exitCode).toBe(0)
    const found = JSON.parse(result.stdout.toString())
    expect(found.tokens).toEqual({ input: 12, output: 3 })
    expect(found.tools).toEqual([["skill", { skill: 'paper "résumé"' }]])
    expect(found.growth).toBeLessThan(32 * 1024)
  } finally { rmSync(folder, { recursive: true, force: true }) }
})
