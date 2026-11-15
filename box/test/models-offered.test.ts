// The models offered (`kuvo/models/offered`, PROTOCOL.md "The models offered"): each with the side a
// conversation on it runs on and its efforts, so a client offers in place only its own side's and
// draws their efforts without reading the engine's or the bridge's lists itself.

import "./safety"
import { expect, test } from "bun:test"
import { offeredAnswer } from "../src/link/areas/models-offered"
import type { Listed, Lists } from "../src/link/work/model-rule"

const listed = (providerID: string, id: string, name: string, efforts: string[]): Listed => ({
  providerID, id, name, aliases: [], efforts, free: false, provisional: false, hidden: false, retiring: false, released: 1,
})

const lists: Lists = {
  engine: { ready: new Set(["anthropic"]), models: [listed("anthropic", "claude-opus-5-5", "Claude Opus 5.5", ["low", "high", "max"])], accounts: new Map(), fallback: null },
  bridge: {
    models: [{ ...listed("claude-code", "opus", "Opus", ["low", "medium", "high"]), aliases: ["default"] }],
    accounts: new Map([["claude-code", { id: "claude-code", name: "Claude Code", methods: [], signedIn: true, installed: true, complete: true }]]),
    auto: [{ id: "auto_1", name: "Everyday", entries: 2 }],
  },
}

test("each model says its side and its efforts: the engine's, the coding agents', and Custom Auto's lists on the agents' side", () => {
  const answer = offeredAnswer(lists, null)
  expect(answer.models.map(model => [model.providerID, model.id, model.side, model.efforts])).toEqual([
    ["anthropic", "claude-opus-5-5", "engine", ["low", "high", "max"]],
    ["claude-code", "opus", "agents", ["low", "medium", "high"]],
    ["auto", "auto_1", "agents", []],
  ])
  expect(answer.known).toEqual({ engine: true, agents: true })
  // Other ids a conversation may name one by: Claude Code's `default`, the model it stands for.
  expect(answer.models[1].aliases).toEqual(["default"])
})

test("a side that can't say has none of its models, and says so", () => {
  const answer = offeredAnswer({ ...lists, bridge: null }, null)
  expect(answer.models.map(model => model.side)).toEqual(["engine"])
  expect(answer.known).toEqual({ engine: true, agents: false })
})

// Which models need no account (#733), so a client says "free models" without knowing OpenCode's
// provider: OpenCode Zen's free ones (signed out), a local provider's; never one that costs
// nothing only with its account (Copilot's), a paid one, or a coding agent's.
test("a model that needs no account says so: OpenCode Zen's free ones and a local one's, not Copilot's", () => {
  const free = (model: Listed): Listed => ({ ...model, free: true })
  const answer = offeredAnswer({
    engine: {
      ready: new Set(["anthropic", "github-copilot", "ollama"]),
      models: [
        listed("anthropic", "claude-opus-5-5", "Claude Opus 5.5", []),
        free(listed("github-copilot", "gpt-5-mini", "GPT-5 mini", [])),
        free(listed("opencode", "big-pickle", "Big Pickle", [])),
        free(listed("ollama", "qwen3", "Qwen3", [])),
      ],
      accounts: new Map([
        ["anthropic", { id: "anthropic", name: "Anthropic", methods: [["oauth", "claude"], ["key", ""]], signedIn: true, installed: true, complete: false }],
        ["github-copilot", { id: "github-copilot", name: "GitHub Copilot", methods: [["oauth", "device"]], signedIn: true, installed: true, complete: false }],
        ["opencode", { id: "opencode", name: "OpenCode Zen", methods: [["key", ""]], signedIn: false, installed: true, complete: false }],
      ]),
      fallback: null,
    },
    bridge: { ...lists.bridge!, models: [free(listed("claude-code", "opus", "Opus", []))] },
  }, null)
  expect(answer.models.map(model => [model.providerID, model.id, model.free ?? false])).toEqual([
    ["github-copilot", "gpt-5-mini", false],
    ["anthropic", "claude-opus-5-5", false],
    ["opencode", "big-pickle", true],
    ["ollama", "qwen3", true],
    ["claude-code", "opus", false],
    ["auto", "auto_1", false],
  ])
})
