// How a signed-out provider of the engine's is signed in to, as the box says it for a session's
// model (`signInNeed`, kuvo/sessions/models), held to the cases the Mac's Accounts and KuvoClient's
// PlanSignIns are held to too (box/test/fixtures/plan-sign-ins.json).

import "./safety"
import { expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { signInNeed } from "../src/link/work/model-rule"

const vectors = JSON.parse(readFileSync(new URL("fixtures/plan-sign-ins.json", import.meta.url), "utf8"))

test.each(vectors.cases.map((each: any) => [each.provider, each]))("%s: the ways offered, what's missing, and the plan a key points to", (_, each: any) => {
  const methods = each.methods.map((method: any) => [method.type, method.id ?? ""])
  const need = signInNeed(each.provider, methods)
  expect(need.offered).toEqual(each.offered)
  expect(need.missing).toBe(each.missing)
  expect(need.plan ?? null).toEqual(each.plan)
})

test("an account reached with the computer's credentials, a key perhaps beside them, is connected, not signed in to (#734)", () => {
  // Amazon Bedrock: its AWS credentials, or a key.
  expect(signInNeed("amazon-bedrock", [["external", "aws"], ["key", ""]])).toEqual({ missing: "signIn", offered: ["external:aws", "key"], connects: true })
  // One with a sign-in page beside them is signed in to; one with only a key, given a key.
  expect(signInNeed("acme", [["oauth", "device"], ["external", "aws"]]).connects).toBeUndefined()
  expect(signInNeed("deepseek", [["key", ""]]).connects).toBeUndefined()
})
