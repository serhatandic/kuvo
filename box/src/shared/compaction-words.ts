import { isObject } from "./json";

/** Display words for compaction messages, including engine records written before these fields. */
export function compactionWords(value: unknown): unknown {
  if (!isObject(value) || value.type !== "compaction") return value;
  const title = value.status === "running" ? "Summarizing the conversation…"
    : value.status === "completed" ? "Conversation summarized"
    : value.status === "failed" ? "Couldn’t summarize the conversation" : "Conversation summary";
  return { ...value, title, ...(value.status === "running" ? { workingTitle: title } : {}) };
}

/** A message page or a single-message response, preserving its cursor and other fields. */
export function compactionPage(value: unknown): unknown {
  if (!isObject(value)) return value;
  return { ...value, data: Array.isArray(value.data) ? value.data.map(compactionWords) : compactionWords(value.data) };
}

/** Live lifecycle events use the same words as the stored compaction message. */
export function compactionEventWords(type: string, data: Record<string, unknown>): Record<string, unknown> {
  const status = type === "session.compaction.started" ? "running"
    : type === "session.compaction.ended" ? "completed"
    : type === "session.compaction.failed" ? "failed" : undefined;
  if (!status) return data;
  const message = compactionWords({ type: "compaction", status });
  if (!isObject(message)) return data;
  return { ...data, title: message.title, ...(message.workingTitle ? { workingTitle: message.workingTitle } : {}) };
}
