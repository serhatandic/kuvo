// The last page of each conversation this box carried from another computer
// (`run/peers/<key>/conversations/<session>.json`), so a client still reads it while that
// computer is restarting, updating or off (PROTOCOL.md, "Opening a conversation through another box").
// Each copy is the newest page only, without inline attachments, and is read alone: never all of them.

import fs from "node:fs/promises";
import path from "node:path";
import { writePrivateLater } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import { RUN } from "../home";

export const KEPT_CONVERSATION_FEATURE = "line.kept-conversation";

/** Conversations kept for each computer: the least recently carried go first. */
const MOST_KEPT = 64;
/** A copy's size: its oldest messages go first. */
const MOST_BYTES = 2 * 1024 * 1024;
/** An inline attachment (a `data:` address) longer than this keeps only its type. */
const INLINE = 4 * 1024;

const folderOf = (key: string) => path.join(RUN, "peers", key, "conversations");
const fileOf = (key: string, session: string) => path.join(folderOf(key), `${session}.json`);

export type KeptPage = { at: number; data: unknown[] };

function withoutInline(value: unknown): unknown {
  if (typeof value === "string")
    return value.length > INLINE && value.startsWith("data:") ? value.slice(0, value.indexOf(",") + 1) : value;
  if (Array.isArray(value)) return value.map(withoutInline);
  if (isObject(value)) return Object.fromEntries(Object.entries(value).map(([name, each]) => [name, withoutInline(each)]));
  return value;
}

// One write at a time for each copy, so an older page never replaces a newer one.
const writing = new Map<string, { at: number; done: Promise<void> }>();

/** Keeps `body`, a newest-first message page the computer `key` answered for `session`. */
export async function keepConversation(key: string, session: string, body: Buffer, at = Date.now()): Promise<void> {
  const page = parsed(body);
  if (!isObject(page) || !Array.isArray(page.data)) return;
  const data = page.data.map(withoutInline);
  let size = data.reduce<number>((total, message) => total + JSON.stringify(message ?? null).length + 1, 0);
  while (size > MOST_BYTES && data.length) size -= JSON.stringify(data.pop() ?? null).length + 1;
  const file = fileOf(key, session);
  const before = writing.get(file);
  if (before && before.at > at) return;
  const done = (before?.done ?? Promise.resolve()).catch(() => {}).then(async () => {
    await writePrivateLater(file, JSON.stringify({ at, data } satisfies KeptPage));
    await prune(key);
  });
  writing.set(file, { at, done });
  try { await done; }
  finally { if (writing.get(file)?.done === done) writing.delete(file); }
}

async function prune(key: string): Promise<void> {
  const folder = folderOf(key);
  const names = (await fs.readdir(folder)).filter(name => name.endsWith(".json"));
  if (names.length <= MOST_KEPT) return;
  const dated = await Promise.all(names.map(async name => {
    try { return { name, at: (await fs.stat(path.join(folder, name))).mtimeMs }; }
    catch { return { name, at: 0 }; }
  }));
  dated.sort((a, b) => a.at - b.at);
  for (const { name } of dated.slice(0, dated.length - MOST_KEPT)) await fs.rm(path.join(folder, name), { force: true });
}

/** The copy kept of `session` from the computer `key`, or null. */
export async function keptConversation(key: string, session: string): Promise<KeptPage | null> {
  try {
    const kept = parsed(await fs.readFile(fileOf(key, session)));
    if (isObject(kept) && typeof kept.at === "number" && Array.isArray(kept.data)) return { at: kept.at, data: kept.data };
  } catch {}
  return null;
}

/** The computer `key` left the setup: its copies go with it. */
export async function forgetConversations(key: string): Promise<void> {
  await fs.rm(folderOf(key), { recursive: true, force: true });
}
