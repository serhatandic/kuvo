import "./safety"
import { expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { addressProblem, cleanAddress, configHosts, suggestedHosts } from "../src/link/add/address"
import { RELEASE_FILES, tarOf } from "../src/link/add/files"
import { boxFiles } from "../src/release/files"

// What Add a Machine takes as an address and suggests (PROTOCOL.md, "Computers", Adding one), in a
// home of the test's own: never your `~/.ssh`.

test("an address is a host's name, user@host or a Host from ~/.ssh/config; anything ssh would read otherwise is refused in words", () => {
  for (const address of ["studio", "me@studio", "10.0.0.5", "build-box.internal", "me@host_1"]) expect(addressProblem(address)).toBeNull()
  expect(cleanAddress("  me@studio \t")).toBe("me@studio")
  expect(cleanAddress(12)).toBe("")
  const port = "Give a host name, or user@host. For a port or an IPv6 address, add a Host for the machine to ~/.ssh/config and give its name."
  for (const address of ["-oProxyCommand=evil", "host:2222", "ssh://host/path", "::1", "two words", "a\nb", "me@you@host", "@host", "me@", "$(reboot)", "a;b", "a'b"]) expect(addressProblem(address)).toBe(port)
  expect(addressProblem("")).toBe("Give a host name, or user@host.")
  expect(addressProblem("This-Mac.localhost")).toBe("That’s this computer.")
  expect(addressProblem("a".repeat(400))).toBe("That's too long for a host's name.")
})

test("the hosts suggested are ~/.ssh/config's Hosts and those of the files it includes, never a pattern, nor one already added", () => {
  const home = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-add-address-")))
  try {
    mkdirSync(join(home, ".ssh", "conf.d"), { recursive: true })
    writeFileSync(join(home, ".ssh", "config"), [
      "# my machines", "Host studio mini", "  HostName 10.0.0.5", "host=gpu # the big one", "Host *.internal !bastion web-?", "Include conf.d/*.conf", "Include ~/elsewhere", "Include /nowhere/at/all",
      "Host studio", "Host bad:port",
    ].join("\n"))
    writeFileSync(join(home, ".ssh", "conf.d", "b.conf"), "Host late\n")
    writeFileSync(join(home, ".ssh", "conf.d", "a.conf"), "Host early\nInclude conf.d/a.conf\n")
    writeFileSync(join(home, ".ssh", "conf.d", "skipped.txt"), "Host never\n")
    writeFileSync(join(home, "elsewhere"), "Host \"quoted\"\n")
    const hosts = configHosts(join(home, ".ssh", "config"), home)
    expect(hosts).toEqual(["studio", "mini", "gpu", "early", "late", "quoted"])
    expect(suggestedHosts(["Studio", "late"], hosts)).toEqual(["mini", "gpu", "early", "quoted"])
    expect(configHosts(join(home, ".ssh", "none"), home)).toEqual([])
  } finally {
    rmSync(home, { recursive: true, force: true })
  }
})

// (Held wherever the whole checkout is: a release is written from what's built and kept in git,
// which a run sent only the box's own folders hasn't.)
const whole = existsSync(join(import.meta.dir, "..", "dist", "kuvo-link.js")) && existsSync(join(import.meta.dir, "..", "dist", "kuvo-agents.js"))
test.if(whole)("what a box gives a computer it sets up is a release's own files, each with its mode", () => {
  const release = boxFiles(join(import.meta.dir, "..", ".."), "1.2.3")
  expect([...RELEASE_FILES].map(([path, mode]) => `${path} ${mode.toString(8)}`).sort()).toEqual(release.map(file => `${file.path} ${file.mode.toString(8)}`).sort())
  // (The keeper's mark and the version last: a copy cut short is seen as not done.)
  expect(RELEASE_FILES.slice(-2).map(([path]) => path)).toEqual(["bin/.kuvo-boot", "bin/box-version"])
})

test("the files go as one tar the system's own tar unpacks, each whole and with its mode", () => {
  const folder = realpathSync(mkdtempSync(join(tmpdir(), "kuvo-add-tar-")))
  try {
    const files = [{ path: "bin/kuvo-start", mode: 0o700, data: Buffer.from("#!/bin/sh\necho hi\n") }, { path: "bin/runtime/node", mode: 0o755, data: Buffer.alloc(1300, 7) }, { path: "bin/empty", mode: 0o644, data: Buffer.alloc(0) }]
    const ran = Bun.spawnSync(["tar", "xf", "-", "-C", folder], { stdin: tarOf(files) })
    expect(ran.exitCode, ran.stderr.toString()).toBe(0)
    for (const file of files) {
      expect(Buffer.compare(readFileSync(join(folder, file.path)), file.data)).toBe(0)
      expect(statSync(join(folder, file.path)).mode & 0o777).toBe(file.mode)
    }
  } finally {
    rmSync(folder, { recursive: true, force: true })
  }
})
