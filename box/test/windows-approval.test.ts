import "./safety"
import { expect, spyOn, test } from "bun:test"
import os from "node:os"
import { createBoxDispatcher } from "../src/link/server/router"
import type { ApprovalHere } from "../src/shared/routes/approval"
import type { SandboxState } from "../src/shared/routes/sandbox"

// The real desktop routes with only the OS platform and kernel release faked.
const request = createBoxDispatcher().entry()

async function onPlatform(platform: NodeJS.Platform, release: string, run: () => Promise<void>) {
  const original = Object.getOwnPropertyDescriptor(process, "platform")!
  const kernel = spyOn(os, "release").mockReturnValue(release)
  Object.defineProperty(process, "platform", { ...original, value: platform })
  try {
    await run()
  } finally {
    Object.defineProperty(process, "platform", original)
    kernel.mockRestore()
  }
}

async function get<T>(path: string): Promise<T> {
  const pieces = await Array.fromAsync(request({ path, method: "GET", headers: {}, fromApp: true }, undefined))
  const head = pieces[0]
  expect(head && "head" in head ? head.head.status : undefined).toBe(200)
  return JSON.parse(Buffer.concat(pieces.flatMap(piece => "body" in piece ? [piece.body] : [])).toString())
}

for (const [name, platform, release] of [
  ["native Windows", "win32", "10.0.26100"],
  ["WSL2", "linux", "6.6.87.2-microsoft-standard-WSL2"],
] as const) {
  test(`the mode picker shows Auto as coming soon on ${name}`, () => onPlatform(platform, release, async () => {
    const approval = await get<ApprovalHere>("kuvo/approval")
    for (const options of [approval.options?.engine, approval.options?.agents]) {
      expect(options?.find(option => option.approval === "auto")).toEqual({
        approval: "auto", title: "Auto", detail: "Works on its own inside the sandbox; asks before anything beyond it.",
        unavailable: { reason: "windows", why: "Coming soon on Windows." },
      })
    }
  }))

  test(`a new conversation starts in Ask First on ${name}`, () => onPlatform(platform, release, async () => {
    expect((await get<ApprovalHere>("kuvo/approval")).default).toBe("askFirst")
  }))

  test(`the sandbox reports unavailable on ${name} before a check`, () => onPlatform(platform, release, async () => {
    const { on, able, reason, action } = await get<SandboxState>("kuvo/sandbox")
    expect({ on, able, reason, action }).toEqual({ on: false, able: false, reason: "windows", action: undefined })
  }))
}
