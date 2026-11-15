import "./safety"
import { expect, test } from "bun:test"

// The sandbox's row in Settings is worded by the box (PROTOCOL.md, "Sandbox", `sandbox.words`): the
// button for each thing you can do says the same on every client.
test("each thing you can do about the sandbox has its button's words", async () => {
  const { actionLabel } = await import("../src/link/fence/state")
  expect([actionLabel("retry", "checkFailed"), actionLabel("retry", "checking"), actionLabel("retry", "engineMissing")]).toEqual(["Try Again", "Try Again", "Check Again"])
  expect(actionLabel("fix", "plugin")).toBe("Fix It")
  // A restart stops what agents are doing, so a client asks first; with no engine running there's nothing to stop.
  expect([actionLabel("restartEngine", "waiting"), actionLabel("restartEngine", "engineDown")]).toEqual(["Restart Engine…", "Restart Engine"])
  expect([actionLabel("help", "kernel"), actionLabel("softwareUpdate", "macos")]).toEqual(["How to Check…", "Software Update…"])
})

// What a button with no route opens is the box's to say too (`sandbox.opens`): no client keeps the
// docs' address, or picks a system pane for a computer that may not be the one it runs on.
test("a button that sends no route says what it opens, and whether that's this computer's own", async () => {
  const { actionOpens } = await import("../src/link/fence/state")
  expect(actionOpens("help")).toEqual({ opens: "https://www.kuvo.run/code/docs/machines/#sandbox" })
  expect(actionOpens("softwareUpdate")).toEqual({ opens: "x-apple.systempreferences:com.apple.Software-Update-Settings.extension", opensHere: true })
  for (const action of ["retry", "fix", "restartEngine"] as const) expect(actionOpens(action)).toBeUndefined()
})

// Every reason's sentence is shown to people as it is: none names the machinery ("plugin").
test("no reason's sentence says plugin: a changed setup is Kuvo's part in the engine", async () => {
  const { WHY } = await import("../src/link/fence/state")
  expect(WHY.plugin).toBe("Kuvo’s part in the engine was changed or removed.")
  for (const sentence of Object.values(WHY)) expect(sentence.toLowerCase()).not.toContain("plugin")
})

// #668 5c: on, but where commands can still reach your login's services (a Linux machine without
// user namespaces), the box says why and names the docs' page; and once the check has failed twice
// in a row, what a report of it says. No client keeps either sentence or the address.
test("on with commands reaching your login's services: why, and the page on what the machine needs", async () => {
  const { worded } = await import("../src/link/fence/state")
  const services = worded({ on: true, able: true, agents: true, gaps: ["services"], needs: "util-linux" })
  expect(services).toMatchObject({ on: true, action: "help", actionLabel: "How to Check…", opens: "https://www.kuvo.run/code/docs/machines/#sandbox" })
  expect(services.why).toContain("Until it has util-linux, agents don’t use your vault here and goals don’t run on their own.")
  expect(worded({ on: true, able: true, agents: true, gaps: ["services"], needs: "user namespaces" }).why).toContain("Until it allows user namespaces,")
  // On and whole, or with only git's files kept apart: nothing to say.
  expect(worded({ on: true, able: true, agents: true })).toEqual({ on: true, able: true, agents: true })
  expect(worded({ on: true, able: true, agents: true, gaps: ["git"] })).toEqual({ on: true, able: true, agents: true, gaps: ["git"] })
})

test("a check that failed twice in a row says what a report of it says; once, it doesn't", async () => {
  const { worded } = await import("../src/link/fence/state")
  const failed = (failures: number) => worded({ on: false, able: true, agents: false, reason: "checkFailed", action: "retry", route: "POST kuvo/sandbox/check", check: "readsOwn", failures })
  expect(failed(1).report).toBeUndefined()
  expect(failed(2).report).toBe("The sandbox’s check failed 2 times in a row: Its check failed. (check: readsOwn)")
  expect(failed(3)).toMatchObject({ why: "Its check failed.", actionLabel: "Try Again" })
})
