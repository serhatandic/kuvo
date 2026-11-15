import { expect, test } from "bun:test";
import { computerWords } from "../src/link/areas/computers";

test("computer reach and working display words are supplied by the box", () => {
  for (const reach of ["connected", "connecting", "away", "unknown"] as const) {
    const words = computerWords({ reach, name: "mini", through: "devbox", working: 2 }, "Studio");
    expect(words.title.length).toBeGreaterThan(0);
    expect(words.workingTitle).toBe("2 sessions at work");
    expect(Boolean(words.notice)).toBe(reach !== "connected");
    if (words.notice) expect(words.detail).toBe(words.notice.detail);
  }
  expect(computerWords({ reach: "connected", name: "mini", working: 1 }, "Studio").workingTitle).toBe("1 session at work");
  expect(computerWords({ reach: "unknown", name: "mini" }, "Studio").workingTitle).toBeUndefined();
});
