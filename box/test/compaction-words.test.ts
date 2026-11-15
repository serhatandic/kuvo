import { describe, expect, test } from "bun:test";
import { compactionPage } from "../src/shared/compaction-words";

describe("compaction display words", () => {
  test("old stored messages receive words on both page and single reads", () => {
    const message = { type: "compaction", id: "c", status: "running" };
    const words = { ...message, title: "Compacting conversation…", workingTitle: "Summarizing the conversation…" };
    expect(compactionPage({ data: [message], cursor: { next: "older" } })).toEqual({ data: [words], cursor: { next: "older" } });
    expect(compactionPage({ data: message })).toEqual({ data: words });
  });
  test("all final states have row words and no running label", () => {
    for (const [status, title] of [["completed", "Conversation compacted"], ["failed", "Couldn’t compact conversation"], ["other", "Conversation compaction"]]) {
      expect(compactionPage({ data: { type: "compaction", status } })).toEqual({ data: { type: "compaction", status, title } });
    }
    expect(compactionPage({ data: [{ type: "user", text: "hello" }] })).toEqual({ data: [{ type: "user", text: "hello" }] });
  });
});
