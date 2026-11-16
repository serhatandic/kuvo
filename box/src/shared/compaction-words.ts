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

/** Rewrites a server-sent event feed (`api/event`) as it streams, so every compaction event carries
 * the words: `push` takes bytes as they come and gives what's whole so far, a line cut in two held
 * until the rest of it comes. Everything else goes through as it came. */
export function compactionFeed(): { push(chunk: Uint8Array): Buffer; rest(): Buffer } {
  let held: Buffer = Buffer.alloc(0);
  return {
    push(chunk) {
      const all = held.length ? Buffer.concat([held, chunk]) : Buffer.from(chunk);
      const end = all.lastIndexOf(0x0a) + 1;
      held = all.subarray(end);
      const whole = all.subarray(0, end);
      if (!whole.includes("session.compaction.")) return whole;
      return Buffer.from(whole.toString("utf8").split("\n").map(feedLine).join("\n"));
    },
    /** What was still held when the feed ended (a last line without its end), as it came. */
    rest() {
      const left = held;
      held = Buffer.alloc(0);
      return left;
    },
  };
}

function feedLine(line: string): string {
  if (!line.startsWith("data:") || !line.includes("session.compaction.")) return line;
  const end = line.endsWith("\r") ? "\r" : "";
  try {
    const event = JSON.parse(line.slice(5));
    if (!isObject(event) || typeof event.type !== "string" || !isObject(event.data)) return line;
    const data = compactionEventWords(event.type, event.data);
    return data === event.data ? line : `data: ${JSON.stringify({ ...event, data })}${end}`;
  } catch { return line; }
}
