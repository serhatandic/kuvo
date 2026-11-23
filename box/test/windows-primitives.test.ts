import "./safety"
import { expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { windowsProfile } from "../src/shared/account"
import { platformFacts } from "../src/shared/platform"
import { localIpc, type WindowsPipeAdapter } from "../src/shared/local-ipc.js"
import { joinWindowsPath, windowsEnvValue, windowsNativePath, windowsWirePath } from "../src/shared/windows-paths"
import { windowsProcessState } from "../src/shared/windows-process"

// Protocol vectors and the real box adapters, with Windows OS observations supplied as inputs.
const vectors = JSON.parse(readFileSync(new URL("../../protocol/vectors/file-addresses/windows.json", import.meta.url), "utf8")) as {
  paths: { native: string; wire: string }[]
  refused: string[]
}

for (const { native, wire } of vectors.paths) {
  test(`a Windows path round-trips as ${wire}`, () => {
    expect(windowsWirePath(native)).toBe(wire)
    expect(windowsNativePath(wire)).toBe(native)
  })
}

test("drive-relative, rooted, device and alternate-stream names stay out of the Windows wire format", () => {
  for (const path of vectors.refused) {
    expect(windowsWirePath(path)).toBeUndefined()
    expect(windowsNativePath(path)).toBeUndefined()
  }
})

test("Git Bash HOME cannot redirect the box away from the Windows profile", () => {
  expect(windowsProfile({ HOME: "/c/msys/home/ada", UserProfile: "D:\\Users\\Ada Lovelace" },
    { username: "Ada", homedir: "C:\\Users\\Ada" }))
    .toEqual({ name: "Ada", home: "D:\\Users\\Ada Lovelace" })
})

test("Windows PATH keeps drive colons and quoted folders with semicolons", () => {
  const path = windowsEnvValue({ Path: 'C:\\Tools;.;"D:\\Tool;Kit";C:/Tools;;C:bin' }, "PATH")
  expect(joinWindowsPath(path, "D:\\Tools\\Bin")).toBe('C:\\Tools;"D:\\Tool;Kit";D:\\Tools\\Bin')
})

const record = { pid: 4100, startIdentity: "133987324438004001" }

test("a reused Windows PID cannot be mistaken for the recorded worker", () => {
  const processTable = new Map([[4100, "133987324438004002"]])
  expect(windowsProcessState(record, pid => processTable.get(pid))).toBe("stale")
})

test("a worker that Windows won't let the box inspect cannot be claimed or cleared", () => {
  expect(windowsProcessState(record, () => undefined)).toBe("unknown")
})

test("native IPC fails closed when the SID-secured Windows provider is absent", () => {
  expect(() => localIpc("/unused.sock", { platform: "win32" }))
    .toThrow("Kuvo's secure Windows named pipes are unavailable.")
})

test("the Windows pipe policy admits only the user's SID and requests local first-instance binding", async () => {
  const sid = "S-1-5-21-48-49-50-1001"
  const adapter: WindowsPipeAdapter = {
    connect: async () => { throw new Error("This fixture only binds a listener.") },
    listen: policy => {
      if (policy.sddl !== `O:${sid}D:P(A;;GA;;;${sid})` || !policy.rejectRemote || !policy.firstInstance) {
        throw new Error("The OS cannot bind this pipe privately.")
      }
      return { ready: Promise.resolve(), close() {} }
    },
  }
  const settings = { sid, homeKey: "a1".repeat(32), channel: "ssh" } as const
  const ssh = localIpc("/unused.sock", { platform: "win32", windows: settings, adapter })
  const listener = ssh.listen(() => {})
  await listener.ready
  listener.close()
  expect(ssh.path).toMatch(/^\\\\\.\\pipe\\kuvo-[a-f0-9]{64}$/)
})

test("SSH and each holder get separate pipe names for each box home", () => {
  const adapter: WindowsPipeAdapter = {
    connect: async () => { throw new Error("This fixture only names pipes.") },
    listen: () => { throw new Error("This fixture only names pipes.") },
  }
  const name = (homeKey: string, channel: "ssh" | `holder:${string}`) => localIpc("/unused.sock", {
    platform: "win32", windows: { sid: "S-1-5-21-48-49-50-1001", homeKey, channel }, adapter,
  }).path
  const first = "a1".repeat(32)
  expect(new Set([name(first, "ssh"), name(first, "holder:000000000001"),
    name(first, "holder:000000000002"), name("b2".repeat(32), "ssh")]).size).toBe(4)
})

test("a Linux shell with a leftover WSL variable still has Linux capabilities", () => {
  const before = process.env.WSL_DISTRO_NAME
  process.env.WSL_DISTRO_NAME = "Ubuntu-24.04"
  try {
    expect(platformFacts("linux", "6.8.0-generic")).toEqual({ platform: "linux", wsl: false })
  } finally {
    if (before === undefined) delete process.env.WSL_DISTRO_NAME
    else process.env.WSL_DISTRO_NAME = before
  }
})
