// The catalogs a box's release carries (`bin/integrations.json`, `bin/mcp-catalog.json`), written
// from the box's one table, are what the route scenarios set their boxes up with
// (protocol/vectors/routes/box, scripts/route-vectors.sh): a change to the table that isn't
// written there too would leave every box's runner on a catalog no release has.
import { expect, test } from "bun:test"
import "./safety"
import { createHash } from "node:crypto"
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { boxFiles } from "../src/release/files"
import { integrationsCatalog, mcpCatalog } from "../src/shared/integrations/catalogs"

const vector = (name: string) => JSON.parse(readFileSync(join(import.meta.dir, "../../protocol/vectors/routes/box", name), "utf8"))

test("the route scenarios' boxes have the integrations and popular servers a release brings (scripts/route-vectors.sh after a change to the table)", () => {
  expect(JSON.parse(integrationsCatalog(false))).toEqual(vector("integrations.json"))
  expect(JSON.parse(mcpCatalog())).toEqual(vector("mcp-catalog.json"))
})

test("a Mac's box is offered every integration a machine is, and those that use a Mac's own apps", () => {
  const machines = JSON.parse(integrationsCatalog(false)).integrations, macs = JSON.parse(integrationsCatalog(true)).integrations
  expect(macs.filter((each: { macOnly?: boolean }) => !each.macOnly)).toEqual(machines)
  expect(macs.filter((each: { macOnly?: boolean }) => each.macOnly).map((each: { id: string }) => each.id)).toEqual(["apple-calendar"])
  // Each key where a box keeps its keys, by the name a box stores it under.
  for (const each of macs) {
    for (const variable of Object.values(each.variables ?? {})) {
      expect(variable).toMatch(/^KUVO_KEY_[A-Z0-9_]+$/)
      expect(JSON.stringify(each.settings)).toContain(`{file:~/.kuvo-code/box/keys/${variable}}`)
    }
  }
})

// While the Swift pin is still there (the retired Mac app's and the Swift client's,
// docs/upstreams.md): an engine update that moved one and not the other would have a build of
// that app carry a box release for another engine. Goes with the Swift pin.
test("the engine a release pins is the one the Swift side pins: its version and each build's integrity", () => {
  const pin = JSON.parse(readFileSync(join(import.meta.dir, "../src/shared/engine-pin.json"), "utf8"))
  const swift = (file: string) => readFileSync(join(import.meta.dir, "../../swift/OpenCodeKit/Sources/OpenCodeKit", file), "utf8")
  expect(swift("OpenCodeKit.swift")).toContain(`public static let version = "${pin.version}"`)
  const installer = swift("Engine/EngineInstaller.swift")
  for (const system of ["linux", "darwin"]) {
    for (const kind of ["x86_64", "aarch64"]) {
      expect(installer).toContain(`"${pin[system][kind].url}"`)
      expect(installer).toContain(`"${pin[system][kind].integrity}"`)
    }
  }
})

// The one-time check the move asked for (#715), kept for as long as each file stays what it was:
// what the box writes is, byte for byte and mode for mode, what the retired Swift Mac app last
// wrote into a release, for the engine that release had.
test("a release's files are what the Swift Mac app last wrote, byte for byte, where they haven't been changed on purpose since", () => {
  const made = JSON.parse(readFileSync(join(import.meta.dir, "fixtures/release/swift-made.json"), "utf8")) as { engine: string; files: Record<string, { sha256: string; mode: string }> }
  const checkout = join(import.meta.dir, "../..")
  // This checkout's sources, with that release's engine pinned in place of today's.
  const root = mkdtempSync(join(tmpdir(), "kuvo-swift-made-"))
  try {
    for (const folder of ["box/src/keeper", "box/src/plugins"]) cpSync(join(checkout, folder), join(root, folder), { recursive: true })
    cpSync(join(checkout, "box/src/shared/bot-words.json"), join(root, "box/src/shared/bot-words.json"))
    // (What's built isn't compared, as it changes with every change to the box: stand-ins.)
    const body = "// the browser\n"
    const built = { "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js": "// the link\n", "box/dist/kuvo-agents.js": "// the bridge\n", "box/dist/kuvo-browser.js": `// kuvo-browser ${createHash("sha256").update(body).digest("hex").slice(0, 16)}\n${body}` }
    for (const [file, text] of Object.entries(built)) {
      mkdirSync(dirname(join(root, file)), { recursive: true })
      writeFileSync(join(root, file), text)
    }
    const build = { url: `https://example.invalid/engine-${made.engine}.tgz`, integrity: `sha512-${"A".repeat(86)}==` }
    writeFileSync(join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: made.engine, linux: { x86_64: build, aarch64: build }, darwin: { x86_64: build, aarch64: build } }))
    const files = new Map(boxFiles(root, "0.7.2").map(file => [file.path, file]))
    const written = Object.fromEntries(Object.keys(made.files).map(path => {
      const file = files.get(path)
      return [path, file ? { sha256: createHash("sha256").update(file.data).digest("hex"), mode: file.mode.toString(8) } : null]
    }))
    expect(written).toEqual(made.files)
    // And nothing a release didn't have then, but what's built or named for the version.
    expect([...files.keys()].filter(path => !(path in made.files))).toEqual(["bin/box-version", "bin/kuvo-agents.js", "bin/kuvo-browser.js", "bin/kuvo-link.js"])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
