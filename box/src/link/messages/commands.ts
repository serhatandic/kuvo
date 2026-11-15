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
import { engineGet } from "../engine/client";
import type { Target } from "../../shared/records";

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

/** What goes back in the composer for a message taken back to edit (`removed.draft`): its words with
 * the command or keyword it went with in front, so sending it again does what it did (Undo and a
 * failed send put back the words alone). */
export const draftOf = ({ words, keyword }: Pick<MessageParts, "words" | "keyword">): string =>
  keyword ? `${keyword} ${words}`.trim() : words;

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

// ——— What the engine makes of a command ———

/** One of your commands (Settings › Commands, or a command file in a settings folder) as the engine
 * in `folder` has it: what it says, from the last that has it; none for the engine's own, an MCP
 * server's prompt, or when the engine can't say. */
async function templateOf(engine: Target, folder: string, name: string): Promise<string | undefined> {
  const answer = await engineGet(engine, "api/config", folder || undefined).catch(() => undefined) as unknown;
  if (answer === undefined) return undefined;
  const sources: unknown[] = Array.isArray(answer) ? answer : isObject(answer) && Array.isArray(answer.data) ? answer.data : [{ info: isObject(answer) ? answer.data ?? answer : answer }];
  let template: string | undefined;
  for (const source of sources) {
    if (!isObject(source)) continue;
    const info = isObject(source.info) ? source.info : {};
    for (const list of [info.commands, info.command]) {
      const own = isObject(list) && isObject(list[name]) ? list[name].template : undefined;
      if (typeof own === "string" && own.trim()) template = own.trim();
    }
    if (source.type === "directory" && typeof source.path === "string" && /^[\w.-]+(\/[\w.-]+)*$/.test(name) && !name.split("/").includes("..")) {
      for (const kept of ["command", "commands"]) {
        const text = textOf(path.join(source.path, kept, `${name}.md`));
        const body = text?.replace(/^---\n[\s\S]*?\n---\n?/, "").trim();
        if (body) template = body;
      }
    }
  }
  return template;
}

const NAMED = /\$(?:ARGUMENTS|([1-9]\d*))/g;
const WORD = /(?:\[Image\s+\d+\]|"[^"]*"|'[^']*'|[^\s"']+)/gi;

/** `template` with `text` (what goes after the command's name) filled in, as the engine fills it:
 * `$ARGUMENTS` all of it, `$1`, `$2`… a word each (quotes taken off), the highest the rest; after
 * it, when it names none. */
export function filledIn(template: string, text: string): string {
  const words = (text.match(WORD) ?? []).map(word => word.replace(/^["']|["']$/g, ""));
  const numbered = template.match(/\$([1-9]\d*)/g) ?? [];
  const highest = Math.max(0, ...numbered.map(each => Number(each.slice(1))));
  const filled = template.replace(NAMED, (whole: string, index?: string) => {
    if (whole === "$ARGUMENTS") return text;
    const at = Number(index) - 1;
    if (at >= words.length) return "";
    return Number(index) === highest ? words.slice(at).join(" ") : words[at]!;
  });
  return numbered.length === 0 && !template.includes("$ARGUMENTS") && text.trim() ? `${filled} ${text}`.trim() : filled.trim();
}

/** The prompt the engine in `folder` will make of `command` with `text` after it, where this box can
 * say for sure: one of yours with no shell step in it (`!\`…\``, whose output it can't know); null
 * otherwise. */
export async function madeText(engine: Target, folder: string, command: string, text: string): Promise<string | null> {
  const template = await templateOf(engine, folder, command);
  return template && !template.includes("!`") ? filledIn(template, text) : null;
}

/** A command on its way to the engine, waiting to hear which message it made: what the engine will
 * make of it where this box can say (`made`), when it was sent and, once it was, answered (ms). */
interface Expected {
  session: string;
  parts: MessageParts;
  made: string | null;
  sentAt: number;
  answeredAt?: number;
  heard: (id: string | null) => void;
}

// In the order they went: each session's first takes the first message that's its.
const expected: Expected[] = [];
// Each session's commands not yet given their message.
const pending = new Map<string, Set<Promise<void>>>();

/** An engine event, as far as this reads it: a message waiting in a session's inbox, and when the
 * engine made it (ms; its clock is this computer's). */
export interface CommandEvent {
  type: string;
  created?: unknown;
  data?: { sessionID?: string; inboxID?: string; item?: { type?: string; payload?: { text?: unknown; metadata?: unknown } } };
}

/** Whether the prompt a command made, `text`, could be the one made of `words`, where the box can't
 * say what it is: each word typed after the command is in it, without the quotes that kept them
 * together. One with nothing after it can't be told from any other message: never. */
const madeOf = (text: string, words: string): boolean => {
  const each = words.split(/\s+/).map(word => word.replace(/^["']+|["']+$/g, "")).filter(Boolean);
  return each.length > 0 && each.every(word => text.includes(word));
};

/** Whether `text`, made at `at`, is the message `command` made: made after it was sent and before
 * the engine answered it (the engine makes it before it answers), and the very prompt the box
 * knows it makes, else one with each word typed after it. */
const isMadeBy = (command: Expected, text: string, at: number): boolean =>
  at >= command.sentAt && (command.answeredAt === undefined || at <= command.answeredAt)
  && (command.made !== null ? text.trim() === command.made : madeOf(text, command.parts.words));

/** Each event from the engine: a user message in the inbox that the box didn't mark as its own
 * (`kuvo` in its metadata, its parts or its note) and isn't someone else's (a wake-up, another
 * agent's letter, `writtenByYou`) is the one the oldest command waiting in its session made, if it's
 * that command's (`isMadeBy`). */
export function heardCommand(event: CommandEvent): void {
  if (event.type !== "session.inbox.enqueued" || event.data?.item?.type !== "user") return;
  const { sessionID: session, inboxID: id } = event.data;
  if (!session || !id) return;
  const payload = event.data.item.payload;
  if (isObject(payload?.metadata) && payload.metadata.kuvo !== undefined) return;
  const text = typeof payload?.text === "string" ? payload.text : "";
  if (!writtenByYou(text, payload?.metadata)) return;
  const at = typeof event.created === "number" ? event.created : Date.now();
  const which = expected.findIndex(each => each.session === session && isMadeBy(each, text, at));
  if (which < 0) return;
  const [found] = expected.splice(which, 1);
  found!.heard(id);
}

/** `send` (one of the engine's commands in `session`, `parts` as typed, `made` the prompt the box
 * knows it makes, if it does), and once it's sent, the message it made kept with those parts: heard
 * from the engine meanwhile, or for `within` ms after (its answer doesn't wait for it). It answers
 * as `send` does; a send that fails keeps nothing. */
export async function sendingCommand<T>(session: string, parts: MessageParts, made: string | null, send: () => Promise<T>, sent: (answer: T) => boolean, within = HEARD_WITHIN): Promise<T> {
  let heard: (id: string | null) => void = () => {};
  const message = new Promise<string | null>(resolve => { heard = resolve; });
  const entry: Expected = { session, parts, made: made?.trim() ?? null, sentAt: Date.now(), heard };
  expected.push(entry);
  const drop = () => {
    const at = expected.indexOf(entry);
    if (at >= 0) expected.splice(at, 1);
    heard(null);
  };
  const settled = message.then(id => { if (id) keep(id, parts); });
  const waits = pending.get(session) ?? new Set();
  pending.set(session, waits);
  waits.add(settled);
  settled.finally(() => {
    waits.delete(settled);
    if (!waits.size && pending.get(session) === waits) pending.delete(session);
  });
  let answer: T;
  try {
    answer = await send();
  } catch (error) {
    drop();
    throw error;
  }
  entry.answeredAt = Date.now();
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

/** Once every command on its way in `session` has been given its message (or given up on), or
 * after `within` ms: what `kuvo/messages/parts` waits for when it's asked about one it doesn't
 * know, so a client that sees a command's message before the engine says which it was still reads
 * it as typed. */
export async function commandsSettled(session: string, within = SETTLED_WITHIN): Promise<void> {
  const waits = [...(pending.get(session) ?? [])];
  if (!waits.length) return;
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([Promise.all(waits), new Promise<void>(resolve => { timer = setTimeout(resolve, within); })]);
  clearTimeout(timer);
}
