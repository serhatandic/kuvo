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
