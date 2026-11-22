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
