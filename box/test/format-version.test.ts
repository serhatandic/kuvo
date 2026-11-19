import "./safety"
import { expect, test } from "bun:test";
import { objectFormat, upgradeFormat } from "../src/shared/format-version";

test("format steps run in version order and an unknown version does not enter a step", () => {
  const steps = [(n: number) => n + 2, (n: number) => n * 3];
  expect(upgradeFormat("example", undefined, 1, steps)).toEqual(9);
  expect(upgradeFormat("example", 1, 3, steps)).toEqual(9);
  expect(upgradeFormat("example", 2, 9, steps)).toEqual(9);
  for (const version of [3, -1, 1.5, "1", null]) {
    expect(() => upgradeFormat("example", version, 1, steps)).toThrow(/example format version .* is unsupported/);
  }
});

test("an additive stamp preserves original fields and upgrades on a copy", () => {
  const format = objectFormat("example", 1, (fields, from) => from === 0 ? { ...fields, enabled: true } : fields);
  const old = { name: "original", extension: { data: [1, 2] } };
  const upgraded = { ...old, enabled: true };
  expect(format.read(old)).toEqual(upgraded);
  expect(old).toEqual({ name: "original", extension: { data: [1, 2] } });
  expect(format.write(upgraded)).toEqual({ ...upgraded, formatVersion: 1 });
  expect(format.read(format.write(upgraded))).toEqual(upgraded);
});

test("compatibility decoders read a specific family version without replaying earlier decoders", () => {
  const seen: number[] = [];
  const reader = objectFormat("example", 2, (fields, version) => {
    seen.push(version);
    return fields;
  });
  expect(reader.read({ formatVersion: 1, content: "kept" })).toEqual({ content: "kept" });
  expect(() => reader.read({ formatVersion: 3 })).toThrow(/unsupported/);
  expect(seen).toEqual([1]);
});
