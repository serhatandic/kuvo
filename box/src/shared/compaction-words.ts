import { isObject } from "./json";

/** Display words for compaction messages, including engine records written before these fields. */
export function compactionWords(value: unknown): unknown {
  if (!isObject(value) || value.type !== "compaction") return value;
  const title = value.status === "running" ? "Compacting conversation…"
    : value.status === "completed" ? "Conversation compacted"
    : value.status === "failed" ? "Couldn’t compact conversation" : "Conversation compaction";
  return { ...value, title, ...(value.status === "running" ? { workingTitle: "Summarizing the conversation…" } : {}) };
}

/** A message page or a single-message response, preserving its cursor and other fields. */
export function compactionPage(value: unknown): unknown {
  if (!isObject(value)) return value;
  return { ...value, data: Array.isArray(value.data) ? value.data.map(compactionWords) : compactionWords(value.data) };
}
