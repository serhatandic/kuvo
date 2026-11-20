import { describe, expect, test } from "bun:test";
import { mergeNumberBlocks, rewriteMergeReferences } from "../src/link/setup/merge-data";
import { mergePlan } from "../src/link/setup/membership";
import { sharedFits } from "../src/link/setup/shared";
import { MOST_DOC } from "../src/link/documents/format";
import { matching } from "../src/link/work-queue/store";

describe("merging setups", () => {
  test("a full setup still has space to save an accepted refusal", () => {
    const entry = { sequence: 1, value: "" };
    const capacity = Math.floor(MOST_DOC / 3);
    const overhead = Buffer.byteLength(JSON.stringify({ setupPersonalities: entry }));
    const full = { setupPersonalities: { ...entry, value: "a".repeat(capacity - overhead) } };
    const denial = { setupMergeDenials: { ...entry, value: { refused: "The combined setup is full." } } };
    expect(sharedFits({ ...full, ...denial })).toBe(true);
    expect(sharedFits({ ...full, setupPersonalities: { ...entry, value: full.setupPersonalities.value + "a" }, ...denial })).toBe(false);
  });

  test("keeps unoccupied numbers and gives the joining ledger room above both", () => {
    expect(mergeNumberBlocks(100, 200)).toEqual({ offset: 100, last: 300 });
    expect(mergeNumberBlocks(200, 100)).toEqual({ offset: 200, last: 300 });
    expect(mergeNumberBlocks(0, 28)).toEqual({ offset: 0, last: 28 });
    expect(mergeNumberBlocks(12, 0)).toEqual({ offset: 12, last: 12 });
    expect(() => mergeNumberBlocks(200, 100, 299)).toThrow("enough Work Queue numbers");
    expect(() => mergeNumberBlocks(500_000, 500_000)).toThrow("enough Work Queue numbers");
  });

  test("the roster accepts a source-bound merge receipt and an unused replacement range", () => {
    const receipt = { operation: "a".repeat(43), source: "set_" + "a".repeat(22), digest: "b".repeat(43), sourceHigh: 100, sourceName: "Old office", offset: 200 };
    expect(mergePlan(receipt)).toEqual(receipt);
    expect(mergePlan({ ...receipt, digest: "old" })).toBeNull();
    expect(mergePlan({ ...receipt, source: ".." })).toBeNull();
    expect(mergePlan({ ...receipt, offset: -1 })).toBeNull();
    expect(mergePlan({ ...receipt, offset: 1_000_000 })).toBeNull();
  });

  test("rewrites Work Queue references and leaves outside issues, code and out-of-range references alone", () => {
    const text = "Look at #1 and #20. See gh:kuvo/work#1 or `#1`. Unknown #45.";
    expect(rewriteMergeReferences(text, { sourceHigh: 30, offset: 40 })).toBe("Look at #41 and #60. See gh:kuvo/work#1 or `#1`. Unknown #45.");
  });

  test("a bare number names the retained item while an origin names the matching alias", () => {
    const origin = "set_" + "a".repeat(22);
    const kept = { id: "retained", number: 1 };
    const from = { id: "joining", number: 2, numberAliases: [{ setup: origin, name: "Old office", number: 1 }] };
    expect(matching([from, kept], { number: 1 })).toEqual([kept]);
    expect(matching([from, kept], { origin, number: 1 })).toEqual([from]);
  });
});
