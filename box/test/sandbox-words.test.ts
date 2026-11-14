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
