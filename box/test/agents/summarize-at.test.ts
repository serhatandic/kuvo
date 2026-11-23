import "../safety"
import { afterAll, expect, test } from "bun:test"
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { withSummarizeAt } from "../../src/agents/server"
import { compactionFile } from "../../src/shared/context"
import type { Session } from "../../src/agents/wire"

// A session as the bridge answers `GET api/session/<id>`: its context says where it's summarized,
// from this box's "Summarize long conversations at" setting, so a client draws the mark there.
const home = mkdtempSync(join(tmpdir(), "kuvo-summarize-at-"))
const before = process.env.KUVO_HOME
process.env.KUVO_HOME = home
mkdirSync(join(home, "box"), { recursive: true })
afterAll(() => {
  process.env.KUVO_HOME = before
  rmSync(home, { recursive: true, force: true })
})

const session = (providerID: string, window?: number) =>
  ({ id: "ses_a", model: { id: "m", providerID }, context: { used: 412_000, ...(window ? { window } : {}) } }) as unknown as Session

const choose = (value: string) => writeFileSync(compactionFile(home), JSON.stringify(value))

test("a session's context says where this box's setting summarizes it", () => {
  rmSync(compactionFile(home), { force: true })
  expect(withSummarizeAt(session("claude-code", 1_000_000)).context).toEqual({ used: 412_000, window: 1_000_000, summarizeAt: 400_000 })
  choose("200000")
  expect(withSummarizeAt(session("codex-cli", 1_000_000)).context?.summarizeAt).toBe(200_000)
  choose("600000")
  // On a smaller model the agent's own margin wins: no mark the box can't stand behind.
  expect(withSummarizeAt(session("claude-code", 200_000)).context?.summarizeAt).toBeUndefined()
  choose("never")
  expect(withSummarizeAt(session("claude-code", 1_000_000)).context?.summarizeAt).toBeUndefined()
  expect(withSummarizeAt(session("claude-code", 500_000)).context?.summarizeAt).toBe(400_000)
})

test("agents that keep their own limits say no threshold", () => {
  choose("400000")
  expect(withSummarizeAt(session("cursor", 1_000_000)).context?.summarizeAt).toBeUndefined()
  expect(withSummarizeAt(session("antigravity", 1_000_000)).context?.summarizeAt).toBeUndefined()
})

test("a conversation offers this box's setting where its agent takes it, before anything is measured too", () => {
  choose("600000")
  const unmeasured = { id: "ses_b", model: { id: "m", providerID: "claude-code" } } as unknown as Session
  expect(withSummarizeAt(unmeasured).summarize).toEqual({
    value: "600000",
    label: "Summarize at",
    choices: [{ id: "200000", title: "200k" }, { id: "400000", title: "400k", line: "Default" }, { id: "600000", title: "600k" }, { id: "never", title: "Never" }],
    line: "All conversations",
  })
  // (Nothing chosen: the default.)
  rmSync(compactionFile(home), { force: true })
  expect(withSummarizeAt(session("codex-cli", 1_000_000)).summarize?.value).toBe("400000")
  // Agents that keep their own limits offer nothing.
  expect(withSummarizeAt(session("cursor", 1_000_000)).summarize).toBeUndefined()
  expect(withSummarizeAt(session("antigravity", 1_000_000)).summarize).toBeUndefined()
})
