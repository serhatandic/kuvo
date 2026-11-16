import { expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { botNow, botStateTurns } from "../src/link/lists/rule"

// Which state a bot is in is the box's rule, one for every client (PROTOCOL.md, "What a bot is
// doing now"): held to protocol/vectors/bot-states.json, each state, their order, asleep's 12 hours
// and since when.

const vectors = JSON.parse(readFileSync(new URL("../../protocol/vectors/bot-states.json", import.meta.url), "utf8"))

test.each(vectors.cases.map((each: any) => [each.name, each]))("%s", (_, each: any) => {
  expect(botNow(each.counts, each.rest, each.at, each.before)).toEqual(each.expect.now)
  expect(botStateTurns(each.rest, each.at) ?? null).toBe(each.expect.turns)
})

test("every state is in the vectors", () => {
  const states = new Set(vectors.cases.map((each: any) => each.expect.now.state))
  expect([...states].sort()).toEqual(["asleep", "blocked", "done", "idle", "needsYou", "thinking", "working"])
})
