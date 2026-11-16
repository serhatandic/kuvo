import "./safety"
import { expect, test } from "bun:test"
import { commandLine, commandWords, serverSetup, serverSetups, webAddress } from "../src/shared/mcp-setup"

// What Settings › MCP Servers takes from what people paste and type, the same for every client: a
// server's setup from its README, a command line's words, and a web address (the retired Mac app's
// MCPSnippet and CommandWords, held to the same cases).

test("a server's setup pasted from a README, in each app's form", () => {
  expect(serverSetup(`{ "mcpServers": { "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "<YOUR_TOKEN>" } } } }`))
    .toEqual({ name: "github", command: ["npx", "-y", "@modelcontextprotocol/server-github"], environment: { GITHUB_PERSONAL_ACCESS_TOKEN: "<YOUR_TOKEN>" }, headers: {} })
  // One entry alone, with a comment and a trailing comma, as READMEs show it.
  expect(serverSetup(`"filesystem": {\n  // your folder\n  "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/Desktop",],\n},`)?.command)
    .toEqual(["npx", "-y", "@modelcontextprotocol/server-filesystem", "/Users/me/Desktop"])
  expect(serverSetup(`{"servers": {"linear": {"type": "http", "url": "https://mcp.linear.app/mcp", "headers": {"Authorization": "Bearer \${input:key}"}}}}`))
    .toEqual({ name: "linear", command: [], environment: {}, url: "https://mcp.linear.app/mcp", headers: { Authorization: "Bearer ${input:key}" } })
  expect(serverSetup(`{"mcp": {"sentry": {"type": "local", "command": ["uvx", "mcp-server-sentry"], "environment": {"SENTRY_TOKEN": "x"}}}}`)).toMatchObject({ name: "sentry", command: ["uvx", "mcp-server-sentry"], environment: { SENTRY_TOKEN: "x" } })
  expect(serverSetup(`{"command": "docker", "args": ["run", "-i", "--rm", "mcp/fetch"]}`)?.command).toEqual(["docker", "run", "-i", "--rm", "mcp/fetch"])
  expect(serverSetup(`{"deepwiki": {"serverUrl": "https://mcp.deepwiki.com/sse"}}`)?.url).toBe("https://mcp.deepwiki.com/sse")
  expect(serverSetup(`{"api": {"command": "node", "args": ["server.js", "--port", 8080], "env": {"PORT": 8080, "DEBUG": true}}}`)).toMatchObject({ command: ["node", "server.js", "--port", "8080"], environment: { PORT: "8080", DEBUG: "true" } })
})

test("Claude Code's add lines", () => {
  expect(serverSetup("claude mcp add airtable -e AIRTABLE_API_KEY=key -- npx -y airtable-mcp-server")).toEqual({ name: "airtable", command: ["npx", "-y", "airtable-mcp-server"], environment: { AIRTABLE_API_KEY: "key" }, headers: {} })
  expect(serverSetup("claude mcp add --transport http notion https://mcp.notion.com/mcp")).toMatchObject({ name: "notion", url: "https://mcp.notion.com/mcp" })
  expect(serverSetup(`claude mcp add --transport sse asana https://mcp.asana.com/sse -H "Authorization: Bearer t"`)?.headers).toEqual({ Authorization: "Bearer t" })
  expect(serverSetup("claude mcp add --transport http \\\n  notion https://mcp.notion.com/mcp")).toMatchObject({ name: "notion", url: "https://mcp.notion.com/mcp" })
  expect(serverSetup("claude mcp add --scope=user -e A=1 B=2 KEY= weather -- uvx weather-mcp")).toMatchObject({ name: "weather", command: ["uvx", "weather-mcp"], environment: { A: "1", B: "2", KEY: "" } })
  expect(serverSetup(`claude mcp add-json -s user github '{"type":"stdio","command":"npx","args":["-y","@modelcontextprotocol/server-github"]}'`)).toMatchObject({ name: "github", command: ["npx", "-y", "@modelcontextprotocol/server-github"] })
  expect(serverSetup("claude mcp add-from-claude-desktop")).toBeNull()
})

test("what isn't one server's setup", () => {
  const two = `{"mcpServers": {"b": {"command": "y"}, "a": {"command": "x"}}}`
  expect(serverSetup(two)).toBeNull()
  expect(serverSetups(two).map(each => each.name)).toEqual(["a", "b"])
  expect(serverSetup("npx -y @acme/server")).toBeNull()
  expect(serverSetup("{ not json")).toBeNull()
  expect(serverSetup(`{"name": "nothing to run"}`)).toBeNull()
})

test("a command line's words, and typed back", () => {
  expect(commandWords(`npx -y "@scope/a b" 'c d' e\\ f "g\\.h"`)).toEqual(["npx", "-y", "@scope/a b", "c d", "e f", "g\\.h"])
  expect(commandWords("a \\\n b")).toEqual(["a", "b"])
  expect(commandWords(`x ""`)).toEqual(["x", ""])
  expect(commandLine(["npx", "a b", `q"t`, ""])).toBe(`npx "a b" "q\\"t" ""`)
  expect(commandWords(commandLine(["npx", "a b", `q"t`, "\\x"]))).toEqual(["npx", "a b", `q"t`, "\\x"])
})

test("a web address as it's saved", () => {
  expect(webAddress("mcp.example.com/mcp")).toBe("https://mcp.example.com/mcp")
  expect(webAddress("localhost:3000/mcp")).toBe("http://localhost:3000/mcp")
  expect(webAddress("http://tools.lan/mcp")).toBe("http://tools.lan/mcp")
  expect(webAddress("tools")).toBeNull()
  expect(webAddress("ftp://example.com")).toBeNull()
  expect(webAddress("not an address")).toBeNull()
})
