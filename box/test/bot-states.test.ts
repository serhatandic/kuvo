import { expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { botNow, botsNow, botStateTurns } from "../src/link/lists/rule"

// Which state a bot is in is the box's rule, one for every client (PROTOCOL.md, "What a bot is
// doing now"): held to protocol/vectors/bot-states.json, each state, their order, asleep's 12 hours,
// since when, and what's over once you've seen the bot.

const vectors = JSON.parse(readFileSync(new URL("../../protocol/vectors/bot-states.json", import.meta.url), "utf8"))

// (What its sessions come to: as given, or from their facts and when you last saw each bot.)
const countsOf = (each: any) => each.counts ?? botsNow(each.sessions, new Map(Object.entries(each.seen ?? {})))[each.bot]

test.each(vectors.cases.map((each: any) => [each.name, each]))("%s", (_, each: any) => {
  expect(botNow(countsOf(each), each.rest, each.at, each.before)).toEqual(each.expect.now)
  expect(botStateTurns(each.rest, each.at) ?? null).toBe(each.expect.turns)
})

test("every state is in the vectors", () => {
  const states = new Set(vectors.cases.map((each: any) => each.expect.now.state))
  expect([...states].sort()).toEqual(["asleep", "blocked", "done", "idle", "needsYou", "thinking", "working"])
})
