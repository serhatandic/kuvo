import "./safety"
import { expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { join } from "node:path"
import { computerCompatibility } from "../src/link/areas/compatibility"
import { FEATURES } from "../src/shared/features"
import { MAC_FEATURES, MACHINE_FEATURES, macFeatures } from "../src/shared/routes/mac"

// The lists of what a box answers are kept one name a line, in order, so branches that each add a
// name merge without a conflict (features.ts by union: .gitattributes). A list out of order, or a
// name in it twice, as a merge can leave it, fails here.

const inOrder = <T extends string>(names: readonly T[]): T[] => [...new Set(names)].sort()

test("every box's names, the Mac's own and a machine's own are each in order, each name once", () => {
  expect([...FEATURES]).toEqual(inOrder(FEATURES))
  expect([...MAC_FEATURES]).toEqual(inOrder(MAC_FEATURES))
  expect([...MACHINE_FEATURES]).toEqual(inOrder(MACHINE_FEATURES))
})

test("features.ts has one name a line, so what two branches add merges line by line", () => {
  const text = readFileSync(join(import.meta.dir, "../src/shared/features.ts"), "utf8")
  const list = text.slice(text.indexOf("export const FEATURES = [\n") + "export const FEATURES = [\n".length, text.indexOf("\n] as const;"))
  const lines = list.split("\n")
  for (const line of lines) expect(line).toMatch(/^ {2}"[A-Za-z0-9.-]+",$/)
  expect(lines.map(line => JSON.parse(line.trim().slice(0, -1)))).toEqual([...FEATURES])
})

test("a Mac's box names every box's but a machine's own, and the Mac's own, each once", () => {
  for (const name of MACHINE_FEATURES) expect(FEATURES).toContain(name)
  const mac = macFeatures()
  expect(inOrder(mac)).toEqual(inOrder([...FEATURES.filter(name => !(MACHINE_FEATURES as readonly string[]).includes(name)), ...MAC_FEATURES]))
  expect(new Set(mac).size).toBe(mac.length)
  for (const name of MACHINE_FEATURES) expect(mac).not.toContain(name)
})

test("both kinds of Box advertise the protocol gate requirements", () => {
  const root = join(import.meta.dir, "../..")
  const lines = readFileSync(join(root, "protocol/gate-required.txt"), "utf8").split("\n")
  const protocol = lines.map(line => line.trim()).filter(line => line && !line.startsWith("#"))
  const baseline = lines.find(line => line.startsWith("# Baseline: "))!.slice("# Baseline: ".length).trim().split(/\s+/)
  expect(baseline.length).toBeGreaterThan(0)
  for (const name of baseline) expect(protocol).toContain(name)
  expect(protocol.length).toBeGreaterThan(0)
  expect(new Set(protocol).size).toBe(protocol.length)
  const machine = new Set<string>(FEATURES)
  const mac = new Set(macFeatures())
  expect(protocol.filter(name => !machine.has(name) || !mac.has(name))).toEqual([])
})

test("a computer only holds the gate when a baseline capability is missing", () => {
  expect(computerCompatibility(FEATURES)).toEqual({ state: "open" })
  expect(computerCompatibility(FEATURES.filter(name => name !== "bots.state"))).toEqual({
    state: "behind", missing: ["bots.state"],
  })
  expect(computerCompatibility(FEATURES.filter(name => name !== "messages.send"))).toEqual({
    state: "held", missing: ["messages.send"],
  })
})
