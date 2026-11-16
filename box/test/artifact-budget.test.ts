import "./safety"
import { expect, test } from "bun:test"
import { fileSize, fileSizeUnit } from "../src/shared/artifact-budget"

test("a size that rounds up to a thousand of its unit is said in the next one", () => {
  expect(fileSize(999_949)).toBe("999.9 KB")
  expect(fileSize(999_999)).toBe("1 MB")
  expect(fileSize(999_999_999)).toBe("1 GB")
  expect(fileSize(999)).toBe("999 bytes")
  expect(fileSize(1_500_000)).toBe("1.5 MB")
  // The unit a client draws a number with says the same.
  expect(fileSizeUnit(999_999)).toMatchObject({ name: "MB", factor: 1_000_000 })
})
