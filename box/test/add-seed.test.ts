import "./safety"
import { afterAll, expect, test } from "bun:test"
import { rmSync } from "node:fs"
import { createServer } from "node:http"
import type { AddressInfo } from "node:net"
import { SEEDED_FILE, renewsItself, seedCredentials, seedSettings, takeSeed } from "../src/link/add/seed"

// What a computer newly added starts with (PROTOCOL.md, "Computers", Adding one): which of a box's
// sign-ins are given, and how the computer added takes them. The engine is a stand-in that keeps
// what it's given.

test("a sign-in that renews itself is never one to give; a key and GitHub Copilot's are", () => {
  expect(renewsItself({ integrationID: "openai", value: { type: "oauth", refresh: "rt", access: "at" } })).toBe(true)
  expect(renewsItself({ integrationID: "gitlab", value: { type: "oauth", refresh: "rt" } })).toBe(true)
  expect(renewsItself({ integrationID: "github-copilot", value: { type: "oauth", refresh: "gho", access: "gho" } })).toBe(false)
  expect(renewsItself({ integrationID: "anthropic", value: { type: "api", key: "sk" } })).toBe(false)
  expect(renewsItself({ integrationID: "poe", value: { type: "oauth", refresh: "", access: "at" } })).toBe(false)
})

test("of a box's sign-ins, a new computer gets each account's one in use that stays the same, never an integration's own", () => {
  const given = seedCredentials([
    { id: "1", integrationID: "anthropic", label: "Work", active: true, value: { type: "api", key: "sk-1" } },
    { id: "2", integrationID: "anthropic", label: "Old", active: false, value: { type: "api", key: "sk-0" } },
    { id: "3", integrationID: "openai", label: "me", active: true, value: { type: "oauth", refresh: "rt" } },
    { id: "4", integrationID: "notion-mcp", label: "Notion", active: true, value: { type: "oauth", access: "x" } },
    { id: "5", integrationID: "github-copilot", label: "me", active: true, value: { type: "oauth", refresh: "gho", access: "gho" } },
  ], new Set(["anthropic", "openai", "github-copilot"]))
  expect(given.credentials.map(each => [each.integrationID, each.label])).toEqual([["anthropic", "Work"], ["github-copilot", "me"]])
  expect(given.renewing).toBe(1)
})

test("settings go without what names this computer alone, with keys' files named as the other keeps them", () => {
  expect(seedSettings({
    $schema: "x", shell: "/bin/zsh", worktree: "/Users/me/trees", model: "a/b",
    mcp: { notion: { environment: { TOKEN: "{file:/Users/me/Library/keys/KUVO_KEY_NOTION_TOKEN}" } }, "chrome-devtools": {}, "apple-calendar": {} },
  })).toEqual({ model: "a/b", mcp: { notion: { environment: { TOKEN: "{file:~/.kuvo-code/box/keys/KUVO_KEY_NOTION_TOKEN}" } } } })
  expect(seedSettings(null)).toEqual({})
})

test("the computer added takes them once: its own sign-in for an account stays, one that renews itself is refused, and it says how many, never what", async () => {
  const kept: { integrationID: string; value: unknown }[] = [{ integrationID: "anthropic", value: { type: "api", key: "its-own" } }]
  const engine = createServer((request, response) => {
    const chunks: Buffer[] = []
    request.on("data", chunk => chunks.push(chunk))
    request.on("end", () => {
      if (request.method === "POST") kept.push(JSON.parse(Buffer.concat(chunks).toString()))
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ data: kept }))
    })
  })
  await new Promise<void>(resolve => engine.listen(0, "127.0.0.1", resolve))
  afterAll(() => { engine.close() })
  const target = () => ({ port: (engine.address() as AddressInfo).port, password: "pw" })
  rmSync(SEEDED_FILE, { force: true })
  try {
    expect(await takeSeed("not json", target)).toMatchObject({ status: 400 })
    const said = await takeSeed(JSON.stringify({ credentials: [
      { integrationID: "anthropic", label: "Work", value: { type: "api", key: "sk-given" } },
      { integrationID: "github-copilot", label: "me", value: { type: "oauth", refresh: "gho", access: "gho" } },
      { integrationID: "openai", label: "me", value: { type: "oauth", refresh: "rt-renews" } },
      { integrationID: "../etc", label: "x", value: { type: "api", key: "k" } },
    ] }), target)
    expect(said).toEqual({ status: 200, taken: 1 })
    expect(kept.map(each => each.integrationID)).toEqual(["anthropic", "github-copilot"])
    expect(kept[0]!.value).toEqual({ type: "api", key: "its-own" })
    // Asked again (it's set up again, by this box or another): nothing more.
    expect(await takeSeed(JSON.stringify({ credentials: [{ integrationID: "poe", label: "", value: { type: "api", key: "k2" } }] }), target)).toEqual({ status: 200, taken: 0 })
    expect(kept.length).toBe(2)
  } finally {
    rmSync(SEEDED_FILE, { force: true })
  }
})
