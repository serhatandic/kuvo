// The catalogs a box's release carries (`bin/integrations.json`, `bin/mcp-catalog.json`), written
// from the box's one table, are what the route scenarios set their boxes up with
// (box/test/fixtures/routes/box, scripts/route-vectors.sh): a change to the table that isn't
// written there too would leave every box's runner on a catalog no release has.
import "./safety"
import { expect, test } from "bun:test"
import { createHash } from "node:crypto"
import { chmodSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { boxFiles, writeBoxFiles } from "../src/release/files"
import { integrationsCatalog, mcpCatalog } from "../src/shared/integrations/catalogs"

const vector = (name: string) => JSON.parse(readFileSync(join(import.meta.dir, "fixtures/routes/box", name), "utf8"))

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

/** The box's sources as this checkout has them, in a folder of the test's own, for `engine`:
 * what's built isn't looked at here (it changes with every change to the box, and isn't sent
 * where only the box's sources are), so stand-ins. */
function sources(engine: string): string {
  const checkout = join(import.meta.dir, "../..")
  const root = mkdtempSync(join(tmpdir(), "kuvo-release-sources-"))
  for (const folder of ["box/src/keeper", "box/src/plugins"]) cpSync(join(checkout, folder), join(root, folder), { recursive: true })
  cpSync(join(checkout, "box/src/shared/bot-words.json"), join(root, "box/src/shared/bot-words.json"))
  const body = "// the browser\n"
  const built = { "box/dist/kuvo-link.js": "// the link\n", "box/dist/kuvo-agents.js": "// the bridge\n", "box/dist/kuvo-browser.js": `// kuvo-browser ${createHash("sha256").update(body).digest("hex").slice(0, 16)}\n${body}` }
  for (const [file, text] of Object.entries(built)) {
    mkdirSync(dirname(join(root, file)), { recursive: true })
    writeFileSync(join(root, file), text)
  }
  const build = { url: `https://example.invalid/engine-${engine}.tgz`, integrity: `sha512-${"A".repeat(86)}==` }
  writeFileSync(join(root, "box/src/shared/engine-pin.json"), JSON.stringify({ version: engine, linux: { x86_64: build, aarch64: build }, darwin: { x86_64: build, aarch64: build } }))
  return root
}

// The one-time check the move asked for (#715), kept for as long as each file stays what it was:
// what the box writes is, byte for byte and mode for mode, what the retired Swift Mac app last
// wrote into a release, for the engine that release had.
test("a release's files are what the Swift Mac app last wrote, byte for byte, where they haven't been changed on purpose since", () => {
  const made = JSON.parse(readFileSync(join(import.meta.dir, "fixtures/release/swift-made.json"), "utf8")) as { engine: string; changed: string[]; files: Record<string, { sha256: string; mode: string }> }
  // This checkout's sources, with that release's engine pinned in place of today's.
  const root = sources(made.engine)
  try {
    const files = new Map(boxFiles(root, "0.7.2").map(file => [file.path, file]))
    const written = Object.fromEntries(Object.keys(made.files).map(path => {
      const file = files.get(path)
      return [path, file ? { sha256: createHash("sha256").update(file.data).digest("hex"), mode: file.mode.toString(8) } : null]
    }))
    // (A file changed on purpose in box/src: move its line in fixtures/release/swift-made.json from
    // `files` to `changed`, and `bin/.kuvo-boot`'s with a keeper file's. The fixture goes when #713 is done.)
    expect(written).toEqual(made.files)
    // And nothing a release didn't have then, but what's built or named for the version, and what
    // was changed on purpose since.
    expect([...files.keys()].filter(path => !(path in made.files) && !made.changed.includes(path))).toEqual(["bin/box-version", "bin/kuvo-agents.js", "bin/kuvo-browser.js", "bin/kuvo-link.js"])
    expect(made.changed.filter(path => !files.has(path))).toEqual([])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test("writing a release's files changes nothing above their folder, however the folder is named", () => {
  // A private folder, as your temporary folder is, and the release's folder in it, named with a
  // last slash and with two in a row (a Mac's TMPDIR ends in one).
  const parent = mkdtempSync(join(tmpdir(), "kuvo-release-above-")), root = sources("1.1.0")
  try {
    chmodSync(parent, 0o700)
    for (const given of [`${parent}/one/`, `${parent}//two`, `${parent}/deep//three/`]) {
      writeBoxFiles(root, "1.2.3", given)
      expect(statSync(parent).mode & 0o777).toBe(0o700)
    }
    for (const made of ["one", "two", "deep/three"]) {
      expect(statSync(join(parent, made)).mode & 0o777).toBe(0o755)
      expect(statSync(join(parent, made, "bin/kuvo-start")).mode & 0o777).toBe(0o700)
    }
  } finally {
    rmSync(parent, { recursive: true, force: true })
    rmSync(root, { recursive: true, force: true })
  }
})
