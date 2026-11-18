import "./safety"
import { expect, test } from "bun:test"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { uninstall } from "../src/link/add/scripts"

test("uninstall deletes the desktop identity even if the engine and link are already gone", () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-add-uninstall-"))
  try {
    const desktop = join(home, ".kuvo-code", "desktop")
    const sessions = join(home, ".kuvo-code", "data")
    mkdirSync(desktop, { recursive: true })
    mkdirSync(sessions)
    writeFileSync(join(desktop, "identity"), "old desktop key")

    const ran = spawnSync("sh", ["-c", uninstall("0.0.0", "mac-key", true)], {
      env: { ...process.env, HOME: home },
      encoding: "utf8",
    })
    expect(ran.status, ran.stderr).toBe(0)
    expect(existsSync(desktop)).toBe(false)
    expect(existsSync(sessions)).toBe(true)
  } finally {
    rmSync(home, { recursive: true, force: true })
  }
})

test("uninstall reports incomplete cleanup if the desktop folder cannot be deleted without an engine", () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-uninstall-incomplete-"))
  try {
    const desktop = join(home, ".kuvo-code", "desktop")
    mkdirSync(desktop, { recursive: true })
    writeFileSync(join(desktop, "identity"), "old desktop key")
    const tools = join(home, "tools")
    mkdirSync(tools)
    writeFileSync(join(tools, "rm"), "#!/bin/sh\nexit 1\n", { mode: 0o700 })

    const ran = spawnSync("sh", ["-c", uninstall("0.0.0", "mac-key", true)], {
      env: { ...process.env, HOME: home, PATH: `${tools}:${process.env.PATH}` },
      encoding: "utf8",
    })
    expect(ran.status).toBe(1)
    expect(JSON.parse(ran.stdout)).toMatchObject({ result: "incomplete", message: expect.stringContaining("desktop identity folder") })
    expect(existsSync(join(desktop, "identity"))).toBe(true)
  } finally {
    rmSync(home, { recursive: true, force: true })
  }
})

test("disconnecting a shared box reports a failed departure and keeps its identity", () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-uninstall-shared-"))
  try {
    const kuvo = join(home, ".kuvo-code")
    const engine = join(kuvo, "engine", "0.0.0")
    for (const folder of [engine, join(kuvo, "bin"), join(kuvo, "desktop")]) mkdirSync(folder, { recursive: true })
    writeFileSync(join(engine, "opencode"), "#!/bin/sh\nexit 1\n", { mode: 0o700 })
    writeFileSync(join(kuvo, "bin", "kuvo-link.js"), "")
    const identity = join(kuvo, "desktop", "identity")
    writeFileSync(identity, "keep")
    const ran = spawnSync("sh", ["-c", uninstall("0.0.0", "mac-key", false)], {
      env: { ...process.env, HOME: home }, encoding: "utf8",
    })
    expect(ran.status).toBe(1)
    expect(ran.stdout).not.toContain("kept")
    expect(existsSync(identity)).toBe(true)
  } finally {
    rmSync(home, { recursive: true, force: true })
  }
})
