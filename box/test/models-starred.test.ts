// Your starred models (PROTOCOL.md, "The models offered", `models.starred`): kept by the box, so
// every client's picker shows the same stars, in the order you starred them.

import "./safety"
import { expect, test } from "bun:test"
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { setStarred, starredFromKeys, starredModels } from "../src/link/areas/models-starred"

const file = () => join(mkdtempSync(join(tmpdir(), "kuvo-starred-")), "models.json")

test("a star is kept in the order given, once, and taken away again", () => {
  const at = file()
  expect(starredModels(at)).toEqual([])
  setStarred({ providerID: "anthropic", id: "claude-opus-5-5" }, true, at)
  setStarred({ providerID: "openai", id: "gpt-6" }, true, at)
  // Starred again (from another client at once): still once, where it was.
  expect(setStarred({ providerID: "anthropic", id: "claude-opus-5-5" }, true, at)).toEqual([
    { providerID: "anthropic", id: "claude-opus-5-5" },
    { providerID: "openai", id: "gpt-6" },
  ])
  expect(setStarred({ providerID: "openai", id: "gpt-6" }, false, at)).toEqual([{ providerID: "anthropic", id: "claude-opus-5-5" }])
  // Read again, as the next client to ask would.
  expect(starredModels(at)).toEqual([{ providerID: "anthropic", id: "claude-opus-5-5" }])
})

test("what else the file keeps stays, and a file that can't be read is no stars", () => {
  const at = file()
  writeFileSync(at, JSON.stringify({ other: 1, starred: [{ providerID: "x", id: "y" }, "nonsense", { providerID: "x", id: "y" }] }))
  expect(starredModels(at)).toEqual([{ providerID: "x", id: "y" }])
  setStarred({ providerID: "a", id: "b" }, true, at)
  expect(JSON.parse(readFileSync(at, "utf8")).other).toBe(1)
  writeFileSync(at, "{ not json")
  expect(starredModels(at)).toEqual([])
})

test("the earlier Mac app's starred models come along, by their provider and model", () => {
  expect(starredFromKeys("anthropic/claude-opus-5-5,openrouter/meta/llama-4,,bad,anthropic/claude-opus-5-5")).toEqual([
    { providerID: "anthropic", id: "claude-opus-5-5" },
    { providerID: "openrouter", id: "meta/llama-4" },
  ])
})
