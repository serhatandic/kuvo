// The box's queues as its files keep them (PROTOCOL.md, "The queue"), for every part of the box
// that sends into a conversation on its own: the link (`link/engine/queue.ts`, which carries the
// queues on) and the coding agents' bridge (a group chat's part, to a bot's own conversation). Kept
// in the box's run folder: `queues.json`, by session, and what waits whole in `waiting/<id>.json`,
// changed one process at a time. A client following the queues sees another process's change at
// its next look (`followed`).

import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "./format-version";
import { isObject } from "./json";
import { locked, lockedWhile, writeDurablePrivate, writeDurablePrivateLater } from "./files";

/** Where a box keeps its queues: its run folder's. */
export interface QueueFiles {
  file: string;
  /** What waits on the box for a paused conversation, whole, as it goes to the engine: `<message id>.json`. */
  waiting: string;
  lock: string;
}

export const queueFiles = (run: string): QueueFiles => ({
  file: path.join(run, "queues.json"),
  waiting: path.join(run, "waiting"),
  lock: path.join(run, ".queues.lock"),
});

/** A session's queue as the box keeps it: whether it waits for Resume, and what goes first, in its
 * order (ahead of the queue): what Stop held for the next step, and one Undo put back where it was
 * (in the engine, or waiting on the box). */
export interface QueueState {
  paused: boolean;
  held: string[];
  /** A Stop explicitly requested by the user. An older queue may have only `paused`. */
  userPaused?: true;
  /** An explicit Resume supersedes the last historical limit even if no message ran yet. */
  resumedLimitAt?: number;
  /** The usage limit that stopped this queue. A user's Stop clears it. */
  limitStop?: QueueLimitStop;
  /** Explicit order among messages still waiting, separate from Stop’s held state. */
  order?: string[];
  /** When Resume or Send Now last went on with it (ms): a Stop's look at an interrupt from before
   * that leaves it going. Kept a minute, never in a route's answer. */
  resumed?: number;
  /** What waits on the box until the queue goes on, first come first (none: left out). */
  waiting?: Waiting[];
  /** Box backups whose handoff was attempted and may already be in the conversation. */
  handoff?: string[];
  /** A send attempt the bridge can definitively cancel if the prompt is still absent. */
  handoffTokens?: Record<string, string>;
  /** Sends whose HTTP request received a response or definitively failed before connecting. */
  handoffSettled?: string[];
  /** Native engine process that received an attempt without a cancel token. */
  handoffPids?: Record<string, number | undefined>;
  /** The saved prefix of an interrupted engine inbox transfer, in delivery order. */
  inboxMoved?: string[];
  /** Messages a Stop named as still on its way (`stop {items}`), held as they come, each until when
   * (ms); never in a route's answer. */
  arriving?: Record<string, number>;
  /** Those of `held` that were held as they came, in that order (for where the next one goes). */
  cameHeld?: string[];
}

export interface QueueLimitStop {
  message: string;
  resetsAt?: number;
  /** The reset passed, but sending the next message may still need a retry. */
  releasing?: true;
}

export const keptLimitStop = (value: unknown): QueueLimitStop | undefined =>
  isObject(value) && typeof value.message === "string" ? {
    message: value.message,
    ...(typeof value.resetsAt === "number" && Number.isFinite(value.resetsAt) ? { resetsAt: value.resetsAt } : {}),
    ...(value.releasing === true ? { releasing: true as const } : {}),
  } : undefined;

/** A message waiting on the box for a paused conversation, as clients show it: who it's from (an
 * agent's name, "Wake-up"…; none for one of yours), what it says, the names of the files it
 * carries, and when it came (ms). */
export interface Waiting {
  id: string;
  from?: string;
  text: string;
  files?: string[];
  at: number;
}

/** A message to a conversation, as the engine's `prompt` takes it. */
export interface Outgoing {
  id?: string;
  text: string;
  files?: unknown[];
  delivery: "steer" | "queue";
  /** What the engine keeps with it: `boxNote`'s mark, for one the box writes. */
  metadata?: Record<string, unknown>;
}

/** The mark on a message the box writes to an agent (a goal's word, its sessions' news, your answer
 * told late), which the engine keeps with it (`metadata.kuvo.note`): it's never your words, so a
 * client shows `line` from `from` in its place (KuvoClient's BoxNote), and the agent reads the
 * whole text. */
export const boxNote = (from: string, line: string) => ({ kuvo: { note: { from, line } } });

/** The mark on a wake-up's message (`metadata.kuvo.wake`), with the summary its agent gave it, which
 * clients show in its card in place of the whole note (KuvoClient's WakeMessage); the agent reads the
 * whole text. Kept by `kuvo-wake` (`summaries/<letters>`, a watch's `summary`), which sends it. */
export const wakeMark = (summary: string) => ({ kuvo: { wake: { summary } } });

/** The wake-up mark in `metadata`, as `wakeMark` made it, and nothing else of it; null for none. */
export function wakeMarkOf(metadata: unknown): ReturnType<typeof wakeMark> | null {
  const wake = isObject(metadata) && isObject(metadata.kuvo) && isObject(metadata.kuvo.wake) ? metadata.kuvo.wake : null;
  const summary = typeof wake?.summary === "string" ? wake.summary.replace(/\s+/g, " ").trim().slice(0, 300) : "";
  return summary ? wakeMark(summary) : null;
}

/** Whether a message the engine has (its `metadata`) is one the box wrote (`boxNote`). */
export const isBoxNote = (metadata: unknown): boolean => isObject(metadata) && isObject(metadata.kuvo) && isObject(metadata.kuvo.note);

const ANSWERED = "[The user answered] ";
const SESSION_NEWS = /^\[Session (finished|still working|failed|asks|stopped)\] "([^"\n]*)"/;

/** The note a user message in a conversation is, if the box wrote it: who it's from and its line,
 * as its mark says, or, from before it was marked, as clients read it by how it opens. */
export function boxNoteOf(text: string, metadata: unknown): { from: string; line: string } | null {
  const note = isBoxNote(metadata) ? (metadata as { kuvo: { note: Record<string, unknown> } }).kuvo.note : null;
  if (note && typeof note.from === "string" && typeof note.line === "string") return { from: note.from, line: note.line };
  if (text.startsWith(ANSWERED)) return { from: "Your answer", line: `You answered: ${text.slice(ANSWERED.length)}` };
  const news = SESSION_NEWS.exec(text);
  if (!news) return note ? { from: "Kuvo Code", line: text } : null;
  // (As `wakeLineShown` says it: one session's, or how many.)
  const told = text.split("\n\n").filter(part => SESSION_NEWS.test(part)).length;
  if (told !== 1) return { from: "Its sessions", line: `${told} updates from its sessions` };
  const what = news[1] === "finished" ? "finished" : news[1] === "still working" ? "still working" : news[1] === "failed" ? "stopped with an error" : news[1] === "stopped" ? "stopped by you" : "asks something";
  return { from: "Its sessions", line: `“${news[2] || "New Session"}” ${what}` };
}

/** Whether a user message in a conversation is one the box wrote: marked, or from before it was. */
export const isBoxWritten = (text: string, metadata: unknown): boolean => boxNoteOf(text, metadata) !== null;

/** How a message waiting on the box is shown and kept: who it's from, what it says as clients show
 * it, and what goes instead of its text once it waits (a letter whose sender stops waiting). */
export interface Shown {
  from?: string;
  words?: string;
  kept?: string;
}

// (A message's id names its file in `waiting`.)
const WAITING_ID = /^msg_[A-Za-z0-9]{1,80}$/;
export const isWaitingID = (id: unknown): id is string => typeof id === "string" && WAITING_ID.test(id);
export const BAD_WAITING_ID = "This message couldn’t be queued. Send it again as a new message.";
export class InvalidWaitingID extends Error {
  constructor() { super(BAD_WAITING_ID); }
}

/** What waits on the box, as a list in a file (or another box's move) gives it: each that's whole. */
export const keptWaiting = (value: unknown): Waiting[] => (Array.isArray(value) ? value : []).flatMap(item => {
  if (!isObject(item) || !isWaitingID(item.id) || typeof item.text !== "string") return [];
  const files = Array.isArray(item.files) ? item.files.filter((name): name is string => typeof name === "string") : [];
  return [{ id: item.id, ...(typeof item.from === "string" ? { from: item.from } : {}), text: item.text, ...(files.length ? { files } : {}), at: typeof item.at === "number" ? item.at : 0 }];
});

const queueFormat = additiveObjectFormat("Conversation queue");
const payloadFormat = additiveObjectFormat("Waiting message");

const keptState = (value: unknown): QueueState => {
  const waiting = keptWaiting(isObject(value) ? value.waiting : undefined);
  const handoff = isObject(value) && Array.isArray(value.handoff)
    ? value.handoff.filter((id): id is string => typeof id === "string" && waiting.some(item => item.id === id)) : [];
  const tokens = isObject(value) && isObject(value.handoffTokens) ? value.handoffTokens : {};
  const handoffTokens = Object.fromEntries(handoff.flatMap(id => typeof tokens[id] === "string" ? [[id, tokens[id]]] : []));
  const handoffSettled = isObject(value) && Array.isArray(value.handoffSettled)
    ? handoff.filter(id => (value.handoffSettled as unknown[]).includes(id)) : [];
  const pids = isObject(value) && isObject(value.handoffPids) ? value.handoffPids : {};
  const handoffPids = Object.fromEntries(handoff.flatMap(id => typeof pids[id] === "number" ? [[id, pids[id]]] : []));
  const inboxMoved = isObject(value) && Array.isArray(value.inboxMoved)
    ? value.inboxMoved.filter((id): id is string => typeof id === "string" && waiting.some(item => item.id === id)) : [];
  const limitStop = isObject(value) ? keptLimitStop(value.limitStop) : undefined;
  return {
    paused: isObject(value) && value.paused === true,
    held: isObject(value) && Array.isArray(value.held) ? value.held.filter((id): id is string => typeof id === "string") : [],
    ...(isObject(value) && typeof value.resumed === "number" ? { resumed: value.resumed } : {}),
    ...(waiting.length ? { waiting } : {}),
    ...(limitStop && (limitStop.releasing || (isObject(value) && value.paused === true)) ? { limitStop } : {}),
    ...(isObject(value) && value.paused === true && value.userPaused === true ? { userPaused: true as const } : {}),
    ...(isObject(value) && typeof value.resumedLimitAt === "number" ? { resumedLimitAt: value.resumedLimitAt } : {}),
    ...(handoff.length ? { handoff } : {}),
    ...(Object.keys(handoffTokens).length ? { handoffTokens } : {}),
    ...(handoffSettled.length ? { handoffSettled } : {}),
    ...(Object.keys(handoffPids).length ? { handoffPids } : {}),
    ...(isObject(value) && Array.isArray(value.order) && value.order.length
      ? { order: [...new Set(value.order.filter((id): id is string => typeof id === "string"))] } : {}),
    ...(inboxMoved.length ? { inboxMoved } : {}),
    ...(isObject(value) && isObject(value.arriving) && Object.keys(value.arriving).length
      ? { arriving: Object.fromEntries(Object.entries(value.arriving).filter((each): each is [string, number] => typeof each[1] === "number")) } : {}),
    ...(isObject(value) && Array.isArray(value.cameHeld) && value.cameHeld.length ? { cameHeld: value.cameHeld.filter((id): id is string => typeof id === "string") } : {}),
  };
};

/** Whether a client is shown it (`kuvo/sessions/queues/events`): paused, or holding something. */
export const showsQueue = (state: QueueState) => state.paused || state.held.length > 0 || Boolean(state.waiting?.length);
const RESUMED_FOR = 60_000;

/** Every session's queue the box keeps (one that's neither paused nor holds anything isn't kept). */
export function readQueues(files: QueueFiles, strict = false): Record<string, QueueState> {
  let all: unknown = {};
  try { all = JSON.parse(fs.readFileSync(files.file, "utf8")); } catch (error) {
    if (strict && (error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  return decodedQueues(all, strict);
}

export async function readQueuesLater(files: QueueFiles, strict = false): Promise<Record<string, QueueState>> {
  let all: unknown = {};
  try { all = JSON.parse(await fs.promises.readFile(files.file, "utf8")); } catch (error) {
    if (strict && (error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  return decodedQueues(all, strict);
}

function decodedQueues(all: unknown, strict: boolean): Record<string, QueueState> {
  if (!isObject(all)) {
    if (strict) throw new Error("The queue index could not be read");
    return {};
  }
  return Object.fromEntries(Object.entries(all).map(([session, saved]) => {
    const state = isObject(saved) ? queueFormat.read(saved) : null;
    if (strict && (!state || (state.waiting !== undefined &&
      (!Array.isArray(state.waiting) || keptWaiting(state.waiting).length !== state.waiting.length)))) {
      throw new Error("The queue index could not be read");
    }
    return [session, keptState(state)];
  }));
}

function indexQueue(all: Record<string, QueueState>, session: string, before: QueueState, next: QueueState, reseal: boolean) {
  const state = keptState(next);
  state.held = [...new Set(state.held)];
  // (What came held and isn't held any more is no longer among those, whatever let it go.)
  if (state.cameHeld) state.cameHeld = state.cameHeld.filter(id => state.held.includes(id));
  if (!state.cameHeld?.length) delete state.cameHeld;
  const made = JSON.stringify(state) !== JSON.stringify(before);
  if (!made && !reseal) return { state, made, data: undefined };
  all[session] = state;
  // Kept while it's paused, holds something, was resumed a moment ago, or waits for a message
  // a Stop named (until it's held, or its time is up).
  for (const [id, each] of Object.entries(all)) {
    if (each.resumed && Date.now() - each.resumed >= RESUMED_FOR) delete each.resumed;
    if (each.arriving) {
      for (const [message, until] of Object.entries(each.arriving)) if (until <= Date.now()) delete each.arriving[message];
      if (!Object.keys(each.arriving).length) delete each.arriving;
    }
    if (!showsQueue(each) && !each.order?.length && !each.resumed && !each.arriving && !each.limitStop?.releasing && !each.resumedLimitAt) delete all[id];
  }
  const indexed = Object.fromEntries(Object.entries(all).map(([id, state]) => [id, queueFormat.write(state)]));
  return { state, made, data: JSON.stringify(indexed) };
}

type Reseal = boolean | (() => boolean);
const emptyQueue = (): QueueState => ({ paused: false, held: [] });

/** Change one queue and publish its new index under the existing cross-process lock. */
export function changeQueue(files: QueueFiles, session: string, change: (state: QueueState) => QueueState, changed?: () => void, reseal: Reseal = false): QueueState {
  const after = locked(files.lock, () => {
    const all = readQueues(files, true);
    const before = all[session] ?? emptyQueue();
    const next = change(before);
    const result = indexQueue(all, session, before, next, typeof reseal === "function" ? reseal() : reseal);
    if (result.data !== undefined) writeDurablePrivate(files.file, result.data);
    return result;
  }, { required: true });
  if (after.made) changed?.();
  return after.state;
}

export async function changeQueueLater(files: QueueFiles, session: string, change: (state: QueueState) => QueueState | Promise<QueueState>,
  changed?: () => void, reseal: Reseal = false): Promise<QueueState> {
  const after = await lockedWhile(files.lock, async () => {
    const all = await readQueuesLater(files, true);
    const before = all[session] ?? emptyQueue();
    const next = await change(before);
    const result = indexQueue(all, session, before, next, typeof reseal === "function" ? reseal() : reseal);
    if (result.data !== undefined) await writeDurablePrivateLater(files.file, result.data);
    return result;
  }, { required: true });
  if (after.made) changed?.();
  return after.state;
}

export const waitingFile = (files: QueueFiles, id: string) => path.join(files.waiting, `${id}.json`);

export function readWaitingFile(file: string): Record<string, unknown> | null {
  try {
    const saved: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    return isObject(saved) ? payloadFormat.read(saved) : null;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return null;
  }
}

export async function readWaitingFileLater(file: string): Promise<Record<string, unknown> | null> {
  try {
    const value: unknown = JSON.parse(await fs.promises.readFile(file, "utf8"));
    return isObject(value) ? payloadFormat.read(value) : null;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return null;
  }
}

/** Check before replacing a payload, including on a retry with the same ID. */
export function writeWaitingFile(file: string, message: Outgoing, root: string): void {
  readWaitingFile(file);
  writeDurablePrivate(file, JSON.stringify(payloadFormat.write(message)), root);
}

// The names of the files a message carries, as clients show them.
const fileNames = (files: unknown[] | undefined): string[] => (files ?? []).flatMap(file => {
  if (!isObject(file)) return [];
  if (typeof file.name === "string" && file.name) return [file.name];
  return typeof file.uri === "string" && !file.uri.startsWith("data:") ? [path.basename(decodeURIComponent(file.uri.replace(/^file:\/\//, "")))] : ["file"];
});

/** Where one kept waiting goes in `state` (`waiting` without it): last, unless it's put back where
 * it was (Undo, `link/engine/queue.ts`). */
export type Place = (state: QueueState, item: Waiting) => QueueState;
const last: Place = (state, item) => ({ ...state, waiting: [...(state.waiting ?? []), item] });

function waitingEntry(message: Outgoing & { id: string }, { from, words }: Shown): Waiting {
  const names = fileNames(message.files);
  return { id: message.id, ...(from ? { from } : {}), text: words ?? message.text, ...(names.length ? { files: names } : {}), at: Date.now() };
}

/** `message` kept on the box while `session`'s queue is paused (see `Shown`), or while its engine is
 * being changed (`holds`, PROTOCOL.md "A new engine"), or while it retries a limit's release.
 * Otherwise it isn't kept. `changed`: as `changeQueue`'s. */
export function keepWaiting(files: QueueFiles, session: string, message: Outgoing & { id: string }, shown: Shown = {}, changed?: () => void, place: Place = last, holds = false): boolean {
  let kept = false;
  changeQueue(files, session, state => {
    if (!state.paused && !state.limitStop?.releasing && !holds) return state;
    if (!isWaitingID(message.id)) throw new InvalidWaitingID();
    writeWaitingFile(waitingFile(files, message.id), shown.kept === undefined ? message : { ...message, text: shown.kept }, path.dirname(files.file));
    kept = true;
    const waiting = waitingEntry(message, shown);
    return place({ ...state, waiting: (state.waiting ?? []).filter(each => each.id !== message.id) }, waiting);
  }, changed, () => kept);
  return kept;
}

/** Same payload-before-index ordering as keepWaiting, with disk I/O off the resident loop. */
export async function keepWaitingLater(files: QueueFiles, session: string, message: Outgoing & { id: string }, shown: Shown = {},
  changed?: () => void, place: Place = last, holds: () => boolean = () => false): Promise<boolean> {
  let kept = false;
  await changeQueueLater(files, session, async state => {
    if (!state.paused && !state.limitStop?.releasing && !holds()) return state;
    if (!isWaitingID(message.id)) throw new InvalidWaitingID();
    const file = waitingFile(files, message.id);
    await readWaitingFileLater(file);
    const payload = shown.kept === undefined ? message : { ...message, text: shown.kept };
    await writeDurablePrivateLater(file, JSON.stringify(payloadFormat.write(payload)), path.dirname(files.file));
    kept = true;
    return place({ ...state, waiting: (state.waiting ?? []).filter(each => each.id !== message.id) }, waitingEntry(message, shown));
  }, changed, () => kept);
  return kept;
}
