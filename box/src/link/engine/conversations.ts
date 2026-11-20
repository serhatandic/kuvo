// A box's sessions and their conversations read whole, page after page, as the engine (and the
// coding agents' bridge, in the same shape) pages them: what carrying one on and searching them read.

import type { Target } from "../../shared/records";
import { callReply, engineGet } from "./client";
import type { EngineSession } from "./sessions";

/** A message of a conversation's, as the engine lists it: read whole, whatever else it has. */
export interface EngineMessage {
  id: string;
  [field: string]: unknown;
}

/** A page of the engine's, whole: what was asked for, and where the next page starts. */
interface Page<T> {
  data?: T[];
  cursor?: { next?: string; previous?: string } | null;
}

/** Every session `target` lists, subagents' too, newest first. */
export async function everySession(target: Target): Promise<EngineSession[]> {
  const all: EngineSession[] = [];
  let page = await engineGet(target, "api/session?limit=100&order=desc") as Page<EngineSession> | null;
  for (let pages = 0; pages < 1000 && page?.data?.length; pages++) {
    all.push(...page.data);
    const cursor = page.cursor?.next;
    if (page.data.length < 100 || !cursor) break;
    page = await engineGet(target, `api/session?limit=100&cursor=${encodeURIComponent(cursor)}`) as Page<EngineSession> | null;
  }
  return all;
}

/** A session's whole conversation, oldest first (a great many messages at most). */
export async function conversationOf(target: Target, id: string, signal?: AbortSignal): Promise<EngineMessage[]> {
  const route = `api/session/${encodeURIComponent(id)}`;
  const get = async (query: string) => {
    const endpoint = `${route}/message?limit=200&${query}`;
    const page = await (signal ? callReply(target, "GET", "/" + endpoint, undefined, signal) : engineGet(target, endpoint)) as Page<EngineMessage> | null;
    if (signal && !Array.isArray(page?.data)) throw new Error("The conversation could not be read.");
    return page;
  };
  let page = await get("order=desc");
  let messages = [...(page?.data ?? [])].reverse();
  const seen = new Set<string>();
  while ((signal || (page?.data?.length ?? 0) >= 200) && messages.length < 20_000) {
    const cursor = page!.cursor?.next ?? page!.cursor?.previous;
    if (!cursor) break;
    if (seen.has(cursor)) throw new Error("The conversation’s page cursor repeated.");
    seen.add(cursor);
    page = await get(`cursor=${encodeURIComponent(cursor)}`);
    if (!page?.data?.length) break;
    messages = [...page.data].reverse().concat(messages);
  }
  if (signal && messages.length >= 20_000 && (page?.cursor?.next || page?.cursor?.previous)) throw new Error("The conversation’s file references could not be read completely.");
  return messages;
}
