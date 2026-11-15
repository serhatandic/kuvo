// The catalogs a box's release carries (`bin/integrations.json`, `bin/mcp-catalog.json`), written
// from the box's one table, are what the route scenarios set their boxes up with
// (protocol/vectors/routes/box, scripts/route-vectors.sh): a change to the table that isn't
// written there too would leave every box's runner on a catalog no release has.
import { expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { join } from "node:path"
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
