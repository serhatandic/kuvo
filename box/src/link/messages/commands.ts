// A message that ran a command shows as it was typed ("/review this branch"), not as the prompt the
// command made of it (PROTOCOL.md, "A message's parts"). The coding agents' bridge keeps its parts
// with the message, as any message's; the engine's commands take no metadata and give back no id,
// so for one of the engine's this box finds the message the command made (its inbox item, heard on
// the engine's events: a message's inbox id is its id) and keeps its parts here, by id, in
// `run/commands.json` (the latest `KEPT`), which `kuvo/messages/parts`, the queue's items and its
// DELETE answer with.

import path from "node:path";
import { isObject } from "../../shared/json";
import { textOf, writePrivate } from "../files";
import { RUN } from "../home";
import { writtenByYou } from "../engine/queue";
import type { MessageParts } from "./parts";

const FILE = path.join(RUN, "commands.json");
/** How many commands' messages are kept. */
const KEPT = 2000;
/** How long after its answer a command waits for the engine to say which message it made (ms), at
 * most: the engine says so before it answers, so this is only for its events being slow to come. */
export const HEARD_WITHIN = 3000;
/** How much of what was typed after a command is kept (characters): a long paste shows cut. */
const WORDS_MOST = 4000;

/** The parts of a message that ran `command` with `words` after its name. */
export const commandParts = (command: string, words: string): MessageParts => ({ words: words.trim().slice(0, WORDS_MOST), keyword: `/${command}`, command: true });

/** What goes back in the composer for a message taken back to edit (`removed.draft`): a command's
 * or a goal's words with it in front, as sending them again runs it again; another keyword's words
 * alone, as sending those again shouldn't switch to Plan or set another loop. */
export const draftOf = ({ words, keyword, command }: Pick<MessageParts, "words" | "keyword" | "command">): string =>
  keyword && (command || keyword === "/goal") ? `${keyword} ${words}`.trim() : words;

// What's kept, read once (only this process writes it), oldest first.
let kept: Map<string, MessageParts> | null = null;

function readKept(): Map<string, MessageParts> {
  if (kept) return kept;
  kept = new Map();
  try {
    const read = JSON.parse(textOf(FILE) ?? "{}");
    if (isObject(read)) for (const [id, parts] of Object.entries(read)) if (isObject(parts) && typeof parts.words === "string") kept.set(id, parts as unknown as MessageParts);
  } catch {}
  return kept;
}

/** The parts kept for message `id`, which ran one of the engine's commands; null for any other. */
export function keptCommand(id: string): MessageParts | null {
  return readKept().get(id) ?? null;
}

function keep(id: string, parts: MessageParts): void {
  const all = readKept();
  all.delete(id);
  all.set(id, parts);
  for (const old of [...all.keys()].slice(0, Math.max(0, all.size - KEPT))) all.delete(old);
  writePrivate(FILE, JSON.stringify(Object.fromEntries(all)));
}

/** A command on its way to the engine, waiting to hear which message it made. */
interface Expected {
  session: string;
  parts: MessageParts;
  heard: (id: string | null) => void;
}

// In the order they went: each session's first takes the first message heard there.
const expected: Expected[] = [];
const pending = new Set<Promise<void>>();

/** An engine event, as far as this reads it: a message waiting in a session's inbox. */
export interface CommandEvent {
  type: string;
  data?: { sessionID?: string; inboxID?: string; item?: { type?: string; payload?: { text?: unknown; metadata?: unknown } } };
}

/** Whether the prompt a command made, `text`, could be the one made of `words`: each word typed
 * after the command is in it (a template takes them whole, `$ARGUMENTS`, or one by one, `$1`…,
 * without the quotes that kept them together). */
const madeOf = (text: string, words: string): boolean =>
  words.split(/\s+/).map(word => word.replace(/^["']+|["']+$/g, "")).filter(Boolean).every(word => text.includes(word));

/** Each event from the engine: a user message in the inbox that the box didn't mark as its own
 * (`kuvo` in its metadata, its parts or its note) and isn't someone else's (a wake-up, another
 * agent's letter, `writtenByYou`), with the words typed after the command in it, is the one the
 * oldest such command waiting in its session made. */
export function heardCommand(event: CommandEvent): void {
  if (event.type !== "session.inbox.enqueued" || event.data?.item?.type !== "user") return;
  const { sessionID: session, inboxID: id } = event.data;
  if (!session || !id) return;
  const payload = event.data.item.payload;
  if (isObject(payload?.metadata) && payload.metadata.kuvo !== undefined) return;
  const text = typeof payload?.text === "string" ? payload.text : "";
  if (!writtenByYou(text, payload?.metadata)) return;
  const at = expected.findIndex(each => each.session === session && madeOf(text, each.parts.words));
  if (at < 0) return;
  const [found] = expected.splice(at, 1);
  found!.heard(id);
}

/** `send` (one of the engine's commands in `session`, `parts` as typed), and once it's sent, the
 * message it made kept with those parts: heard from the engine meanwhile, or for `within` ms after
 * (its answer doesn't wait for it). It answers as `send` does; a send that fails keeps nothing. */
export async function sendingCommand<T>(session: string, parts: MessageParts, send: () => Promise<T>, sent: (answer: T) => boolean, within = HEARD_WITHIN): Promise<T> {
  let heard: (id: string | null) => void = () => {};
  const message = new Promise<string | null>(resolve => { heard = resolve; });
  const entry: Expected = { session, parts, heard };
  expected.push(entry);
  const drop = () => {
    const at = expected.indexOf(entry);
    if (at >= 0) expected.splice(at, 1);
    heard(null);
  };
  const settled = message.then(id => { if (id) keep(id, parts); });
  pending.add(settled);
  settled.finally(() => pending.delete(settled));
  let answer: T;
  try {
    answer = await send();
  } catch (error) {
    drop();
    throw error;
  }
  if (!sent(answer)) {
    drop();
    return answer;
  }
  const timer = setTimeout(drop, within);
  settled.finally(() => clearTimeout(timer));
  return answer;
}

/** How long `kuvo/messages/parts` waits, at most, for commands on their way to be given their
 * messages (ms): the engine says which in a moment, and one it never says of shows as it went. */
export const SETTLED_WITHIN = 2000;

/** Once every command on its way has been given its message (or given up on), or after `within`
 * ms: what `kuvo/messages/parts` waits for when it's asked about one it doesn't know, so a client
 * that sees a command's message before the engine says which it was still reads it as typed. */
export async function commandsSettled(within = SETTLED_WITHIN): Promise<void> {
  if (!pending.size) return;
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([Promise.all([...pending]), new Promise<void>(resolve => { timer = setTimeout(resolve, within); })]);
  clearTimeout(timer);
}
