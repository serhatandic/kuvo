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
