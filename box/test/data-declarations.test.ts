import "./safety";
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { DATA_DECLARATIONS } from "../src/shared/data-declarations";

test("every stored format in the version inventory has a data declaration", () => {
  const inventory = readFileSync(new URL("../../.claude/notes/848-U-versions-inventory.md", import.meta.url), "utf8");
  const stored = inventory.split("## Stored formats\n")[1]?.split("\n## ")[0] ?? "";
  const families = Array.from(stored.matchAll(/^\| ([^|]+) \|/gm), row => row[1]!)
    .filter(name => name !== "Family" && !/^:?-+:?$/.test(name));
  expect(families.length).toBeGreaterThan(0);
  expect(new Set(families).size).toBe(families.length);

  const declared: string[] = Object.values(DATA_DECLARATIONS).flatMap(data => [...data.families]);
  expect([...new Set(declared)].sort()).toEqual([...families].sort());
});
