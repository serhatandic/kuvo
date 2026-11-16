import "./safety"
import { describe, expect, test } from "bun:test";
import { compactionPage } from "../src/shared/compaction-words";

describe("compaction display words", () => {
  test("old stored messages receive words on both page and single reads", () => {
    const message = { type: "compaction", id: "c", status: "running" };
    const words = { ...message, title: "Summarizing the conversation…", workingTitle: "Summarizing the conversation…" };
    expect(compactionPage({ data: [message], cursor: { next: "older" } })).toEqual({ data: [words], cursor: { next: "older" } });
    expect(compactionPage({ data: message })).toEqual({ data: words });
  });
  test("all final states have row words and no running label", () => {
    for (const [status, title] of [["completed", "Conversation summarized"], ["failed", "Couldn’t summarize the conversation"], ["other", "Conversation summary"]]) {
      expect(compactionPage({ data: { type: "compaction", status } })).toEqual({ data: { type: "compaction", status, title } });
    }
    expect(compactionPage({ data: [{ type: "user", text: "hello" }] })).toEqual({ data: [{ type: "user", text: "hello" }] });
  });
});


test("live compaction lifecycle events supply the stored message’s display words", async () => {
  const { emit, subscribe } = await import("../src/agents/events");
  const received: unknown[] = [];
  const stop = subscribe(event => received.push(event));
  try {
    for (const [type, status] of [["started", "running"], ["ended", "completed"], ["failed", "failed"]]) {
      const event = emit(`session.compaction.${type}`, { sessionID: "s", inputID: "i" }, "/project");
      const page = compactionPage({ data: { type: "compaction", status } }) as { data: { title: string; workingTitle?: string } };
      expect(event.data).toEqual({ sessionID: "s", inputID: "i", title: page.data.title, ...(page.data.workingTitle ? { workingTitle: page.data.workingTitle } : {}) });
      expect(received.at(-1)).toBe(event);
    }
  } finally { stop(); }
});
