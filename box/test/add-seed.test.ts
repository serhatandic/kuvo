import "./safety"
import { afterAll, expect, test } from "bun:test"
import { rmSync } from "node:fs"
import { createServer } from "node:http"
import type { AddressInfo } from "node:net"
import { SEEDED_FILE, credentialSelection, gatherSeed, safeStartingSettings, renewsItself, seedCredentials, seedSettings, takeSeed } from "../src/link/add/seed"
import { readSettings, valueAt } from "../src/link/areas/config"
import { deleteKey, readKey, writeKey } from "../src/link/keys"

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
    expect(await takeSeed(JSON.stringify({ formatVersion: 90, credentials: [
      { integrationID: "future", value: { type: "api", key: "unknown-layout" } },
    ] }), target)).toMatchObject({ status: 400 })
    expect(kept).toHaveLength(1)
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
    expect(await takeSeed(JSON.stringify({ formatVersion: 1, credentials: [{ integrationID: "poe", label: "", value: { type: "api", key: "k2" } }] }), target)).toEqual({ status: 200, taken: 0 })
    expect(kept.length).toBe(2)
  } finally {
    rmSync(SEEDED_FILE, { force: true })
  }
})


test("no selection copies no sign-ins or keys and settings do not carry inline secrets", async () => {
  expect(credentialSelection(undefined)).toEqual([])
  expect(credentialSelection([])).toEqual([])
  expect(credentialSelection(["*"])).toBeNull()
  let engineReads = 0
  expect(await gatherSeed([], () => {
    engineReads++
    throw new Error("omitted selection must not read engine credentials")
  })).toMatchObject({ keys: {}, credentials: [] })
  expect(engineReads).toBe(0)
  const settings = {
    model: "test/model",
    headers: { Authorization: "Bearer secret" },
    mcp: {
      inline: { type: "remote", url: "https://example.invalid", headers: { Authorization: "Bearer secret" } },
      mixed: { type: "local", command: ["test", "--secret=token"], environment: { TOKEN: "secret" } },
      key: { type: "local", command: ["test"], environment: { TOKEN: "{file:/somewhere/KUVO_KEY_TEST_TOKEN}" } },
    },
  }
  expect(safeStartingSettings(settings, new Set())).toEqual({ model: "test/model" })
  expect(safeStartingSettings(settings, new Set(["KUVO_KEY_TEST_TOKEN"]))).toEqual({
    model: "test/model",
    mcp: { key: { type: "local", command: ["test"], environment: { TOKEN: "{file:~/.kuvo-code/box/keys/KUVO_KEY_TEST_TOKEN}" } } },
  })
})


test("a selected catalog key uses the trusted integration configuration", () => {
  const source = { mcp: { github: { type: "remote", disabled: true, url: "https://custom.invalid", headers: { Authorization: "secret" } } } }
  expect(safeStartingSettings(source, new Set())).toEqual({})
  expect(safeStartingSettings(source, new Set(["KUVO_KEY_GITHUB_TOKEN"]))).toEqual({
    mcp: { github: {
      type: "remote",
      url: "https://api.githubcopilot.com/mcp/",
      oauth: false,
      headers: { Authorization: "Bearer {file:~/.kuvo-code/box/keys/KUVO_KEY_GITHUB_TOKEN}" },
      codemode: false,
      disabled: true,
    } },
  })
})

test("an SSH seed retry can configure its selected integration after adoption already saved settings", async () => {
  const given = { credentials: [], keys: { KUVO_KEY_GITHUB_TOKEN: "test-token" }, integrations: [{ id: "github", disabled: true }] }
  const target = () => { throw new Error("key-only retry must not call the engine") }
  const key = "KUVO_KEY_GITHUB_TOKEN"
  const before = await readKey(key)
  await deleteKey(key)
  try {
    expect(await takeSeed(JSON.stringify(given), target)).toEqual({ status: 200, taken: 0, keys: 1 })
    expect(valueAt(readSettings().settings, ["mcp", "github"])).toMatchObject({ disabled: true, url: "https://api.githubcopilot.com/mcp/" })
    given.keys.KUVO_KEY_GITHUB_TOKEN = "other-test-token"
    expect(await takeSeed(JSON.stringify(given), target)).toEqual({ status: 200, taken: 0, keys: 1 })
    expect(await readKey("KUVO_KEY_GITHUB_TOKEN")).toBe("test-token")
  } finally {
    if (before === null) await deleteKey(key)
    else await writeKey(key, before)
  }
})
