// The queue (PROTOCOL.md, "The queue")
// The engine keeps what you send in a session's inbox: a message for the agent's next step
// (`steer`), or one that waits for the run to end (`queue`). The engine doesn't start the next one
// after an interrupt, nor in every case after a run, so every box carries its queue on itself, the
// same for every client and with none connected: once a run ends, after a moment, the next one
// goes. A run that fails, a Stop (from any client), and a queue found with nothing running once
// the engine started again pause it until Resume; Stop holds what was for the next step ahead of
// the queue, the latest Stop's first. Kept in `run/queues.json`, by session.

import { keepsList } from "../server/resident-state";
import { admitPrompt } from "../moves/admission";
import { keptParts, partsOf, withPartsOf } from "../messages/parts";
import crypto from "node:crypto";
import fs from "node:fs";
import { exclusiveWhile, lockedWhile, syncDurableFileLater, writeDurablePrivateLater } from "../../shared/files";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { isObject } from "../../shared/json";
import { resetTime } from "../../shared/limits";
import { processRuns } from "../../shared/platform";
import { writePrivate } from "../files";
import { RUN } from "../home";
import { asked, call, readBridgeFile, readEngineFile } from "./client";
import { swapHolds, swapLine, swapUnderWay } from "../../shared/engine-swap";
import { carryUnderWay } from "./carrying";
import { wakeCard } from "../../shared/message-words";
import { summaryAtEnd } from "../../shared/summaries";
import { messageID } from "./sessions";
import * as files from "../../shared/queues";
import { boxNoteOf, isBoxNote, showsQueue, wakeMarkOf, type Outgoing, type QueueLimitStop, type QueueState, type Shown, type Waiting } from "../../shared/queues";
import type { QueueItem } from "../../shared/routes/sessions";
import type { Target } from "../../shared/records";
import { errorOf, resetOf, scopeOf } from "../work/unanswering";

export type { Outgoing, QueueState, Shown, Waiting } from "../../shared/queues";
export { showsQueue };

const FILES = files.queueFiles(RUN);
export const QUEUES_FILE = FILES.file;
/** What waits on the box for a paused conversation, whole, as it goes to the engine: `<message id>.json`. */
export const WAITING_DIR = FILES.waiting;
/** How long after a run ends (or a message is queued while none runs) the next one goes: the engine
 * may still be handing over what it had. */
export const DRAIN_AFTER = 1500;

/** A message in a session's inbox, as far as the queue reads it (and a move carries it): what it
 * says and the files it brings, in the engine's form. */
interface InboxEntry {
  id: string;
  delivery: "steer" | "queue";
  type?: string;
  text?: unknown;
  payload?: { text?: unknown; files?: unknown; metadata?: unknown };
}

export function queueLimitLine(stop: QueueLimitStop): string {
  return `${stop.message} ${stop.resetsAt === undefined ? "The provider didn't say when it resets." : `It resets at ${resetTime(stop.resetsAt)}.`}`;
}

/** A queue as a client reads it, with the box's words for a usage limit or an engine change. */
export const shownQueue = ({ paused, held, waiting, inboxMoved, limitStop }: QueueState): QueueState & { line?: string } => {
  const line = paused && limitStop ? queueLimitLine(limitStop) : waiting?.length && !paused
    ? (swapHolds(RUN) ? swapLine(swapUnderWay(RUN)) : inboxMoved?.length ? "Messages wait while Kuvo checks the engine’s inbox." : null) : null;
  return { paused, held, ...(waiting?.length ? { waiting } : {}), ...(line ? { line } : {}) };
};

/** Whether what's sent to `target` waits on the box now: it's the engine (not the coding agents'
 * bridge, whose turns an engine change never touches) and its change holds (PROTOCOL.md, "A new
 * engine"). */
export function swapHoldsFor(target: Target | null, session?: string): boolean {
  if (!target) return swapHolds(RUN);
  if (!swapHolds(RUN) && !(session && queueOf(session).inboxMoved?.length)) return false;
  try { return readBridgeFile()?.port !== target.port; } catch { return true; }
}

/** Every session's queue the box keeps (one that's neither paused nor holds anything isn't kept). */
export const readQueues = (strict = false): Record<string, QueueState> => files.readQueues(FILES, strict);

/** The queue overview shared by both the local and setup-wide lists. */
export const shownQueues = () => Object.fromEntries(Object.entries(readQueues())
  .filter(([, state]) => showsQueue(state)).map(([session, state]) => [session, shownQueue(state)]));

export const queueOf = (session: string, strict = false): QueueState => readQueues(strict)[session] ?? { paused: false, held: [] };

/** Told of each change this process makes (`kuvo/sessions/queues/events`). */
export const queueFollowers = new Set<() => void>();
const tellFollowers = () => { for (const tell of queueFollowers) tell(); };

/** `session`'s queue as `change` makes it, read again just before, under its lock. */
export const changeQueue = (session: string, change: (state: QueueState) => QueueState, reseal: boolean | (() => boolean) = false): QueueState => files.changeQueue(FILES, session, change, tellFollowers, reseal);

export async function changeQueueLater(session: string, change: (state: QueueState) => QueueState, reseal: boolean | (() => boolean) = false): Promise<QueueState> {
  return keepsList() ? files.changeQueueLater(FILES, session, change, tellFollowers, reseal) : changeQueue(session, change, reseal);
}

const address = (session: string) => encodeURIComponent(session);

async function inboxOf(target: Target, session: string, patience?: number, strict = false): Promise<InboxEntry[]> {
  const answer = await asked(target, "GET", `api/session/${address(session)}/inbox`, undefined, patience) as { data?: unknown } | null;
  const valid = (item: unknown): item is InboxEntry => isObject(item) && typeof item.id === "string" && (item.delivery === "steer" || item.delivery === "queue");
  if (strict && (!Array.isArray(answer?.data) || !answer.data.every(valid))) throw new Error("The engine did not list the whole inbox.");
  return Array.isArray(answer?.data) ? answer.data.filter(valid) : [];
}

export async function moveInbox(target: Target, session: string): Promise<InboxEntry[]> {
  const inbox = await call(target, "GET", `/api/session/${address(session)}/inbox`, undefined, AbortSignal.timeout(10_000));
  if (!Array.isArray(inbox) || !inbox.every((item): item is InboxEntry => isObject(item) &&
    typeof item.id === "string" && (item.delivery === "steer" || item.delivery === "queue"))) throw new Error("The inbox could not be read for a move");
  return inbox;
}

// (As the engine and the agents' bridge answer it: the sessions at work, as a list or by id.)
async function isRunning(target: Target, session: string): Promise<boolean> {
  const active = (await asked(target, "GET", "api/session/active") as { data?: string[] | Record<string, unknown> } | null)?.data ?? {};
  return Array.isArray(active) ? active.includes(session) : Boolean(active[session]);
}

// A summary asked for waits in an inbox too, but it's no message of yours: Stop and a failed run
// let it go, and nothing waits for Resume on its account.
const messagesIn = (inbox: InboxEntry[]) => inbox.filter(item => item.type !== "compaction");

/** What a run's end leaves of a summary that was waiting: on the bridge it's gone by then, with a
 * note in the conversation; the engine would run it unseen with whatever comes next. */
async function letSummariesGo(target: Target, session: string): Promise<void> {
  for (const item of await inboxOf(target, session)) {
    if (item.type === "compaction") await asked(target, "DELETE", `api/session/${address(session)}/inbox/${address(item.id)}`).catch(() => {});
  }
}

/** Whether a failed run was only a summary: the conversation is as it was. */
async function onlyASummaryFailed(target: Target, session: string): Promise<boolean> {
  const page = await asked(target, "GET", `api/session/${address(session)}/message?limit=6&order=desc`).catch(() => null) as { data?: unknown } | null;
  return summaryAtEnd(page?.data) === "failed";
}

const setDelivery = (target: Target, session: string, item: string, delivery: InboxEntry["delivery"]) =>
  asked(target, "PATCH", `api/session/${address(session)}/inbox/${address(item)}`, { delivery });
const interrupt = (target: Target, session: string, resume = false) =>
  asked(target, "POST", `api/session/${address(session)}/interrupt${resume ? "?resume=true" : ""}`);

/** What waits, by its ID, in the order it goes, as clients show it: what's held, in its order (in
 * the engine's inbox, or on the box), then the engine's for the next step, then its queue, then what
 * waits on the box. */
export function orderOf(inbox: InboxEntry[], { held, waiting = [], order = [] }: QueueState): string[] {
  const heldSet = new Set(held);
  const inboxById = new Map(inbox.map(item => [item.id, item]));
  const waitingIds = new Set(waiting.map(item => item.id));
  const base = [...new Set([
    ...held.filter(id => inboxById.has(id) || waitingIds.has(id)),
    ...inbox.filter(item => item.delivery === "steer" && !heldSet.has(item.id)).map(item => item.id),
    ...inbox.filter(item => item.delivery === "queue" && !heldSet.has(item.id)).map(item => item.id),
    ...waitingIds,
  ])];
  if (!order.length) return base;
  const fixed = new Set(inbox.filter(item => item.type === "compaction" || (item.delivery === "steer" && !heldSet.has(item.id))).map(item => item.id));
  const movable = new Set(base.filter(id => !fixed.has(id)));
  const ranked = order.filter(id => movable.delete(id)).concat([...movable]);
  let index = 0;
  return base.map(id => fixed.has(id) ? id : ranked[index++]!);
}

/** Move a waiting message relative to another. A replay of the same move is harmless. */
export async function reorderQueue(target: Target, session: string, item: string, anchor: string, before: boolean): Promise<{ state: QueueState; items: QueueItem[] } | null> {
  return admitPrompt(session, async () => {
    const inbox = await inboxNow(target, session);
    let moved = false;
    const state = await changeQueueLater(session, state => {
      checkWaitingFormats(state.waiting);
      const steer = new Set(inbox.filter(each => each.delivery === "steer" && !state.held.includes(each.id)).map(each => each.id));
      const order = orderOf(inbox, state).filter(id => !steer.has(id));
      if (item === anchor || !order.includes(item) || !order.includes(anchor)) return state;
      order.splice(order.indexOf(item), 1);
      order.splice(order.indexOf(anchor) + (before ? 0 : 1), 0, item);
      moved = true;
      return { ...state, order };
    }, () => moved);
    return moved ? { state, items: queueItems(inbox, state) } : null;
  });
}

const withoutOrder = (state: QueueState, id: string): QueueState => ({
  ...state, ...(state.order ? { order: state.order.filter(each => each !== id) } : {}),
});

function letQueueGo(state: QueueState): QueueState {
  const now = Date.now();
  return { ...state, paused: false, resumed: now, limitStop: undefined,
    ...(state.limitStop ? { resumedLimitAt: now } : {}) };
}

const waitingFile = (id: string) => files.waitingFile(FILES, id);

/** `message` kept on the box while `session`'s queue is paused (see `Shown`), or while `target`, the
 * engine, is being changed (`swapHoldsFor`): true. Neither, false: it isn't kept. `place`: where it goes
 * (last, else). */
export const keepWaiting = (session: string, message: Outgoing & { id: string }, shown: Shown = {}, place?: files.Place, target?: Target | null): boolean =>
  files.keepWaiting(FILES, session, message, shown, tellFollowers, place, target !== undefined && swapHoldsFor(target, session));

export async function keepWaitingLater(session: string, message: Outgoing & { id: string }, shown: Shown = {}, place?: files.Place, target?: Target | null): Promise<boolean> {
  return keepsList()
    ? files.keepWaitingLater(FILES, session, message, shown, tellFollowers, place, () => target !== undefined && swapHoldsFor(target, session))
    : keepWaiting(session, message, shown, place, target);
}

/** A message of yours a Stop named as still on its way, as it comes (`kuvo/sessions/<id>/stop
 * {items}`): kept on the box while the queue is paused (as what arrives then is) and held, after
 * those held as they came before it, ahead of the rest. False when the queue isn't paused (yet). */
export const keepWaitingHeld = (session: string, message: Outgoing & { id: string }, words?: string): Promise<boolean> =>
  keepWaitingLater(session, message, words === undefined ? {} : { words: words.trim() }, (state, waiting) => {
    const came = (state.cameHeld ?? []).filter(id => id !== waiting.id);
    const held = state.held.filter(id => id !== waiting.id);
    const ahead = held.findIndex(id => !came.includes(id));
    const at = ahead < 0 ? held.length : ahead;
    return { ...state, held: [...held.slice(0, at), waiting.id, ...held.slice(at)], cameHeld: [...came, waiting.id], waiting: [...(state.waiting ?? []), waiting] };
  });

/** Where one put back (Undo) goes: just ahead of `before`, as `orderOf` shows them with `inbox`:
 * ahead of it in what waits on the box, or in what's held, one the engine has holding what's ahead
 * of it too (so they go in the same order); last when it isn't there any more. */
export const placedBefore = (inbox: InboxEntry[], before: string | undefined): files.Place => (given, item) => {
  const state = { ...given, held: given.held.filter(id => id !== item.id) };
  const waiting = state.waiting ?? [];
  const order = orderOf(inbox, state);
  const at = before && before !== item.id ? order.indexOf(before) : -1;
  if (at < 0) return { ...state, waiting: [...waiting, item] };
  if (state.order?.includes(before!)) {
    const index = state.order.indexOf(before!);
    const order = state.order.filter(id => id !== item.id);
    order.splice(index, 0, item.id);
    return { ...state, order, waiting: [...waiting, item] };
  }
  const inHeld = state.held.indexOf(before!);
  if (inHeld >= 0) return { ...state, held: [...state.held.slice(0, inHeld), item.id, ...state.held.slice(inHeld)], waiting: [...waiting, item] };
  if (inbox.some(entry => entry.id === before)) {
    // (Not held: only the engine's are ahead of it.)
    return { ...state, held: [...state.held, ...order.slice(0, at).filter(id => !state.held.includes(id)), item.id], waiting: [...waiting, item] };
  }
  const there = waiting.findIndex(each => each.id === before);
  return { ...state, waiting: [...waiting.slice(0, there), item, ...waiting.slice(there)] };
};

/** `keepWaiting`, put back just ahead of `before` (`placedBefore`), as the engine's inbox is now. */
export async function keepWaitingBefore(target: Target, session: string, message: Outgoing & { id: string }, shown: Shown, before?: string): Promise<boolean> {
  const inbox = before ? await inboxOf(target, session).catch(() => []) : [];
  return keepWaitingLater(session, message, shown, placedBefore(inbox, before), target);
}

/** `message` to `session`, from the box itself (another agent's letter, a wake-up, a bot's work
 * news, an answer's note): while its queue is paused, kept to go once it goes on ("waits", see
 * `keepWaiting`); else sent now ("sent"). It throws when the engine doesn't take it. */
export async function sendInto(target: Target, session: string, message: Outgoing, shown: Shown = {}): Promise<"sent" | "waits"> {
  // (Its parts kept with it, for clients to draw it by, as `kuvo/sessions/<id>/send`'s are, where
  // there's more to them than its text: a message from another agent, one with a client's blocks.
  // Never the box's own notes, which clients draw as notes; a plain one a client asks about once.)
  const metadata = isBoxNote(message.metadata) ? message.metadata : withPartsOf(message.metadata, message.text);
  const whole = { ...message, id: message.id ?? messageID(), ...(metadata !== undefined ? { metadata } : {}) };
  return admitPrompt(session, async () => {
    if (await keepWaitingLater(session, whole, shown, undefined, target)) {
      watchQueueLimit(target, session);
      return "waits";
    }
    await asked(target, "POST", `api/session/${address(session)}/prompt`, whole);
    return "sent";
  });
}

export class WaitingHandoff extends Error {}

function waitingPayload(id: string, strict = false): (Outgoing & { id: string }) | null {
  return decodedWaiting(id, files.readWaitingFile(waitingFile(id)), strict);
}

function decodedWaiting(id: string, whole: unknown, strict: boolean): (Outgoing & { id: string }) | null {
  return isObject(whole) && (!strict || (whole.id === id && (whole.files === undefined || Array.isArray(whole.files)))) &&
    typeof whole.text === "string" && (whole.delivery === "steer" || whole.delivery === "queue")
    ? { id, text: whole.text, delivery: whole.delivery, ...(Array.isArray(whole.files) ? { files: whole.files } : {}), ...(isObject(whole.metadata) ? { metadata: whole.metadata } : {}) }
    : null;
}

/** `from`'s queue as `to`'s, its conversation going on in another session (carried on with the
 * coding agents): stopped, it stays stopped, and what waited on the box goes with it. `renamed`:
 * what the engine held, by its id where the conversation goes on. */
export async function carryQueue(from: string, to: string, renamed: Record<string, string>): Promise<void> {
  // Once for each carry: a conversation brought back keeps its mark, which comes again as it's
  // archived again (a client sends its metadata whole), and its queue is its own by then.
  const again = await lockedWhile(FILES.lock, async () => {
    let carried: Record<string, unknown> = {};
    try { carried = carriedFormat.read(JSON.parse(fs.readFileSync(CARRIED_FILE, "utf8"))); } catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error;
    }
    if (carried[from] === to) return true;
    delete carried[from];
    carried[from] = to;
    const kept = Object.entries(carried).slice(-MOST_CARRIED);
    try { writePrivate(CARRIED_FILE, JSON.stringify(carriedFormat.write(Object.fromEntries(kept)))); } catch {}
    return false;
  });
  if (again) return;
  let was: QueueState = { paused: false, held: [] };
  await changeQueueLater(from, state => { was = state; return { paused: false, held: [] }; });
  if (!showsQueue(was) && !was.order?.length) return;
  // (What waits on the box goes as it is, by its own ID.)
  const held = was.held.flatMap(id => renamed[id] ? [renamed[id]] : was.waiting?.some(each => each.id === id) ? [id] : []);
  const order = was.order?.flatMap(id => renamed[id] ? [renamed[id]] : was.waiting?.some(each => each.id === id) ? [id] : []);
  await changeQueueLater(to, state => ({ ...state, paused: state.paused || was.paused, held: [...state.held, ...held],
    ...(!state.paused && was.limitStop ? { limitStop: was.limitStop } : {}),
    ...(!state.paused && was.userPaused ? { userPaused: was.userPaused } : {}),
    ...(order?.length ? { order: [...(state.order ?? []), ...order] } : {}),
    ...(was.waiting?.length ? { waiting: [...(state.waiting ?? []), ...was.waiting] } : {}) }));
}

// The carries made, from one session to the one it went on in, the latest few hundred.
const carriedFormat = additiveObjectFormat("Conversation queue carry receipts");
const CARRIED_FILE = path.join(RUN, "queues-carried.json");
const MOST_CARRIED = 300;

/** What waits for a conversation, as a move carries it to another box (`move.json`'s `queues`):
 * whether it's paused, what's held, and what waits (on the box, and what the engine held for it), as
 * it's shown and whole, in its order. */
export interface CarriedQueue {
  paused: boolean;
  held: string[];
  order?: string[];
  limitStop?: QueueLimitStop;
  userPaused?: true;
  waiting: { shown: Waiting; message: unknown }[];
}

// What a file a carried message brings can weigh (as a model reads one, `attachedIn`'s); a
// bigger one stays behind.
const CARRIED_FILE_BYTES = 20 * 1024 * 1024;
const FILE_TYPES: Record<string, string> = {
  ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif", ".webp": "image/webp", ".heic": "image/heic",
  ".pdf": "application/pdf", ".txt": "text/plain", ".md": "text/markdown", ".json": "application/json",
};

/** Snapshot the files a queued message promises. Missing or oversized entries hold the move. */
function carriedFiles(given: unknown): { uri: string; name?: string }[] {
  if (given === undefined) return [];
  if (!Array.isArray(given)) throw new CantCarry("attachment list", true);
  return given.map(file => {
    if (!isObject(file)) throw new CantCarry("attachment", true);
    const name = typeof file.name === "string" ? { name: file.name } : {};
    const uri = isObject(file.source) && typeof file.source.uri === "string" ? file.source.uri : file.uri;
    const type = typeof file.mime === "string" && file.mime ? file.mime : null;
    if (typeof uri === "string" && BASE64_DATA.test(uri)) return { uri, ...name };
    if (typeof file.data === "string") return { uri: `data:${type ?? "application/octet-stream"};base64,${file.data}`, ...name };
    if (typeof uri !== "string" || !uri.startsWith("file:")) throw new CantCarry("attachment", true);
    try {
      const at = fileURLToPath(uri);
      const stat = fs.statSync(at);
      if (!stat.isFile() || stat.size > CARRIED_FILE_BYTES) throw new Error();
      const bytes = fs.readFileSync(at);
      if (bytes.length !== stat.size) throw new Error();
      return { uri: `data:${type ?? FILE_TYPES[path.extname(at).toLowerCase()] ?? "application/octet-stream"};base64,${bytes.toString("base64")}`, name: name.name ?? path.basename(at) };
    } catch {
      throw new CantCarry(name.name ?? "attachment", true);
    }
  });
}

/** How a message the engine held is shown waiting (as one the box kept would be): another agent's
 * letter or a wake-up by who it's from, in its own words; yours as it is. */
function shownOfHeld(text: string): Shown {
  const mail = /^\[Message from (.*) · (?:ses_\S+|unknown)\] \([^\n]*\)\n\n([\s\S]*)$/.exec(text);
  if (mail) return { from: mail[1], words: mail[2] };
  if (text.startsWith("[Wake-up] ")) return { from: "Wake-up", words: text.slice("[Wake-up] ".length) };
  return {};
}

const fileNamesOf = (carried: { name?: string }[]): string[] => carried.map(file => file.name ?? "file");
// A file as a `data:` link with what's in it, as a carried message's files are.
const BASE64_DATA = /^data:[^,]*;base64,/;

/** One the engine held that a move can't carry (not a message, or one without words): the move
 * waits rather than leave it behind. */
export class CantCarry extends Error {
  constructor(message: string, readonly unreadable = false) { super(message); }
}

/** The queues of `sessions` that are paused or have something waiting on the box, for a move
 * (`unpackQueues` sets them up on the box it goes to): what waits on the box, and for one that's
 * paused, what its engine (`target`) held, in the order they'd go (`orderOf`), with their files.
 * One not paused has nothing in the engine by then (a move waits for its inbox to empty). */
export async function packQueues(target: Target, sessions: readonly string[]): Promise<Record<string, CarriedQueue>> {
  const all = readQueues(true);
  const packed: Record<string, CarriedQueue> = {};
  for (const session of sessions) {
    const state = all[session];
    const inbox = await moveInbox(target, session);
    if (!state?.paused && inbox.length) throw new CantCarry(session);
    if (!state || (!state.paused && !state.waiting?.length)) continue;
    const indexed = state.waiting ?? [];
    if (new Set(indexed.map(item => item.id)).size !== indexed.length) throw new CantCarry(session);
    const renamed: Record<string, string> = {};
    const waiting = [...new Set(orderOf(inbox, state))].map(id => {
      const shown = state.waiting?.find(each => each.id === id);
      if (shown) {
        const message = waitingPayload(id, true);
        if (!message) throw new CantCarry(id, true);
        if (!message.files) return { shown, message };
        // Shown with the files that go with it.
        const carried = carriedFiles(message.files);
        const { files: _names, ...rest } = shown;
        return { shown: { ...rest, ...(carried.length ? { files: fileNamesOf(carried) } : {}) }, message: { ...message, files: carried } };
      }
      // One the engine held: waiting on the box there, as one kept here would.
      const held = inbox.find(item => item.id === id);
      const text = typeof held?.text === "string" ? held.text : typeof held?.payload?.text === "string" ? held.payload.text : null;
      if (!held || held.type !== "user" || text === null) throw new CantCarry(id);
      // (An ID the box can't name a file by: one of its own, the same each time it's packed.)
      const ownID = /^msg_[A-Za-z0-9]{1,80}$/.test(id) ? id : (renamed[id] = `msg_${crypto.createHash("sha256").update(id).digest("hex").slice(0, 32)}`);
      const carried = carriedFiles(held.payload?.files);
      const { from, words } = shownOfHeld(text);
      return {
        shown: { id: ownID, ...(from ? { from } : {}), text: words ?? text, ...(carried.length ? { files: fileNamesOf(carried) } : {}), at: Date.now() },
        message: { id: ownID, text, delivery: "queue", ...(carried.length ? { files: carried } : {}), ...(isObject(held.payload?.metadata) ? { metadata: held.payload.metadata } : {}) },
      };
    });
    const ids = new Set(waiting.map(each => each.shown.id));
    if (ids.size !== waiting.length) throw new CantCarry(session);
    packed[session] = { paused: state.paused, held: state.held.map(id => renamed[id] ?? id).filter(id => ids.has(id)), waiting,
      ...(state.limitStop ? { limitStop: state.limitStop } : {}),
      ...(state.userPaused ? { userPaused: true } : {}),
      ...(state.order?.length ? { order: waiting.map(each => each.shown.id) } : {}) };
  }
  return packed;
}

/** The queues a move brought (`packQueues`'s), for `sessions` only, kept here paused while its copy
 * is held, in place of any it had: every indexed message, whole and in order. The sessions whose queue went on where it came from, to
 * go on here once it's switched to. */
export function unpackQueues(carried: unknown, sessions: readonly string[]): string[] {
  if (carried === undefined) return [];
  if (!isObject(carried)) throw new CantCarry("queues");
  const goOn: string[] = [];
  const known = new Set(sessions);
  const seen = new Set<string>();
  const prepared = Object.entries(carried).map(([session, queue]) => {
    if (!known.has(session) || !isObject(queue) || typeof queue.paused !== "boolean" || !Array.isArray(queue.waiting) || !Array.isArray(queue.held)) throw new CantCarry(session);
    const waiting = queue.waiting.map(each => {
      const [shown] = files.keptWaiting([isObject(each) ? each.shown : null]);
      const message = isObject(each) && isObject(each.message) ? each.message : null;
      if (!shown || !message || message.id !== shown.id || typeof message.text !== "string" ||
        (message.delivery !== "steer" && message.delivery !== "queue") || seen.has(shown.id)) throw new CantCarry(session);
      seen.add(shown.id);
      const brought = message.files ?? [];
      if (!Array.isArray(brought) || !brought.every(file => isObject(file) && typeof file.uri === "string" && BASE64_DATA.test(file.uri))) throw new CantCarry(session);
      const payload: Outgoing = { id: shown.id, text: message.text, delivery: message.delivery,
        ...(brought.length ? { files: brought } : {}), ...(isObject(message.metadata) ? { metadata: message.metadata } : {}) };
      return { shown, message: payload };
    });
    const ids = new Set(waiting.map(each => each.shown.id));
    if (!queue.held.every((id): id is string => typeof id === "string" && ids.has(id))) throw new CantCarry(session);
    const order = queue.order;
    if (order !== undefined && (!Array.isArray(order) || !order.every((id): id is string => typeof id === "string" && ids.has(id)) ||
      new Set(order).size !== order.length)) throw new CantCarry(session);
    return { session, waiting, held: queue.held, paused: queue.paused, order, limitStop: files.keptLimitStop(queue.limitStop), userPaused: queue.userPaused === true };
  });
  const current = readQueues(true);
  for (const { session, waiting } of prepared) {
    checkWaitingFormats(current[session]?.waiting);
    for (const { shown } of waiting) files.readWaitingFile(waitingFile(shown.id));
  }
  for (const { session, waiting, held, paused, order, limitStop, userPaused } of prepared) {
    forgetQueue(session);
    for (const { shown, message } of waiting) files.writeWaitingFile(waitingFile(shown.id), message, RUN);
    changeQueue(session, () => ({ paused: true, held, waiting: waiting.map(each => each.shown), ...(order?.length ? { order } : {}),
      ...(paused && limitStop ? { limitStop } : {}), ...(paused && userPaused ? { userPaused: true } : {}) }), true);
    if (!paused) goOn.push(session);
  }
  return goOn;
}

/** `session`'s queue forgotten, and what waited on the box for it (its conversation is gone). */
export function forgetQueue(session: string): void {
  let gone: Waiting[] = [];
  changeQueue(session, state => {
    gone = state.waiting ?? [];
    checkWaitingFormats(gone);
    return { paused: false, held: [] };
  });
  for (const each of gone) fs.rmSync(waitingFile(each.id), { force: true });
}

async function forgetQueueLater(session: string): Promise<void> {
  let gone: Waiting[] = [];
  await changeQueueLater(session, state => {
    gone = state.waiting ?? [];
    checkWaitingFormats(gone);
    return { paused: false, held: [] };
  });
  for (const each of gone) await fs.promises.rm(waitingFile(each.id), { force: true });
}

function checkWaitingFormats(waiting: Waiting[] = []): void {
  for (const { id } of waiting) files.readWaitingFile(waitingFile(id));
}

/** One taken away from what waits on the box (Remove, or Edit): how it was shown, what would have
 * gone, and what came after it, as `orderOf` shows them with `inbox` (for Undo to put back as it
 * was, where it was); null when it isn't there. */
export async function dropWaiting(session: string, item: string, inbox: InboxEntry[] = [], checkedHandoff = false,
  saveRemoved?: (removed: RemovedWaiting) => Promise<void>): Promise<boolean> {
  let dropped = false;
  await files.changeQueueLater(FILES, session, async state => {
    const shown = state.waiting?.find(each => each.id === item);
    if (!shown) return state;
    if (!checkedHandoff && state.handoff?.includes(item)) throw new WaitingHandoff();
    const message = decodedWaiting(item, await files.readWaitingFileLater(waitingFile(item)), false);
    if (saveRemoved) {
      if (!message) throw new Error("The waiting draft couldn't be read.");
      const order = orderOf(inbox, state);
      const before = order[order.indexOf(item) + 1];
      const { from, text: words } = shown;
      await saveRemoved({ ...message, ...(from ? { from } : {}), words,
        draft: words, ...(before ? { before } : {}) });
    }
    dropped = true;
    return { ...withoutOrder(state, item), held: state.held.filter(id => id !== item), waiting: state.waiting!.filter(each => each !== shown) };
  }, tellFollowers);
  if (dropped) await fs.promises.rm(waitingFile(item), { force: true });
  return dropped;
}

/** The engine's inbox for `session`, as far as the queue reads it (none, when it can't be read). */
export const inboxFor = (target: Target, session: string): Promise<InboxEntry[]> => inboxOf(target, session).catch(() => []);

/** The engine's inbox for `session` as it is now, its messages only (no summary waiting); it throws
 * when it can't be read. */
export const inboxNow = (target: Target, session: string, patience?: number): Promise<InboxEntry[]> => inboxOf(target, session, patience, true).then(messagesIn);

/** A box-held prompt can still have its handoff backup while its message is already in the
 * conversation. Show only pending items, without dropping the backup before its durable receipt. */
export async function pendingQueue(target: Target, session: string, inbox: InboxEntry[], state: QueueState, patience: number): Promise<QueueState> {
  if (!state.handoff?.length) return state;
  const candidates = state.handoff.filter(id => !inbox.some(entry => entry.id === id));
  const signal = AbortSignal.timeout(patience);
  const delivered = new Set((await Promise.all(candidates.map(async id =>
    await hasMessage(target, session, "message", id, signal) ? id : null))).filter(id => id !== null));
  const current = queueOf(session, true);
  return { ...current, waiting: current.waiting?.filter(item => !delivered.has(item.id)), held: current.held.filter(id => !delivered.has(id)) };
}

/** Told, with its session, each time an engine's (or the coding agents' bridge's) inbox changes
 * (`session.inbox.*`), in the box's long-running process. */
export const inboxFollowers = new Set<(session: string) => void>();

/** A message the engine holds, as clients show it waiting (as one the box keeps is): who it's from
 * (another agent, a wake-up, the box's own note; none for yours), what you wrote (its kept parts'
 * words, or its text without what the box added for the agent), the keyword it was sent with, and
 * its files' names. */
function shownOfInbox(entry: InboxEntry, summarizeWake = false): { from: string | null; words: string; files: string[] } {
  const text = typeof entry.payload?.text === "string" ? entry.payload.text : typeof entry.text === "string" ? entry.text : "";
  const metadata = entry.payload?.metadata;
  const files = (Array.isArray(entry.payload?.files) ? entry.payload.files : []).flatMap(file => {
    if (!isObject(file)) return [];
    const uri = isObject(file.source) && typeof file.source.uri === "string" ? file.source.uri : typeof file.uri === "string" ? file.uri : "";
    if (typeof file.name === "string" && file.name) return [file.name];
    // (A link the system can't read as a path here, `file://host/…`: named as a file.)
    try { return [uri.startsWith("file:") ? path.basename(fileURLToPath(uri)) : "file"]; } catch { return ["file"]; }
  });
  const note = boxNoteOf(text, metadata);
  if (note) return { from: note.from, words: note.line, files };
  // (One that runs one of the engine's commands, as it was typed.)
  const parts = keptParts(metadata) ?? partsOf(text);
  if (parts.mail) return { from: parts.mail.name, words: parts.words, files };
  const wake = summarizeWake ? wakeCard(text, metadata) : null;
  if (wake) return { from: "Wake-up", words: wake.summary, files };
  const held = shownOfHeld(text);
  if (held.from) return { from: held.from, words: held.words ?? text, files };
  if (wakeMarkOf(metadata)) return { from: "Wake-up", words: parts.words, files };
  return { from: null, words: parts.words, files };
}

/** Every message waiting for `session`, in the order clients show them (`orderOf`), with `inbox`
 * (the engine's, its messages only) and what waits on the box (`state`). */
export function queueItems(inbox: InboxEntry[], state: QueueState): QueueItem[] {
  return orderOf(inbox, state).flatMap((id): QueueItem[] => {
    const held = state.held.includes(id);
    const kept = state.waiting?.find(each => each.id === id);
    if (kept) {
      let delivery: "steer" | "queue" = "queue";
      let words = kept.text;
      const message = files.readWaitingFile(waitingFile(id));
      if (message?.delivery === "steer") delivery = "steer";
      const wake = wakeCard(typeof message?.text === "string" ? message.text : "", message?.metadata);
      if (wake) words = wake.summary;
      return [{ id, words, files: kept.files ?? [], from: kept.from ?? null, on: "box", delivery, held }];
    }
    const entry = inbox.find(each => each.id === id);
    return entry ? [{ id, ...shownOfInbox(entry, true), on: "engine", delivery: entry.delivery, held }] : [];
  });
}

/** The files a message the engine holds brings, as a send takes them (`{uri, name}`), so they can
 * go again: by where they are, or, sent in the message itself, with what's in them. */
export function sendableFiles(given: unknown): { uri: string; name?: string }[] {
  return (Array.isArray(given) ? given : []).flatMap(file => {
    if (!isObject(file)) return [];
    const name = typeof file.name === "string" ? { name: file.name } : {};
    const uri = isObject(file.source) && typeof file.source.uri === "string" ? file.source.uri : file.uri;
    if (typeof uri === "string" && uri) return [{ uri, ...name }];
    if (typeof file.data === "string") return [{ uri: `data:${typeof file.mime === "string" && file.mime ? file.mime : "application/octet-stream"};base64,${file.data}`, ...name }];
    return [];
  });
}

export type RemovedWaiting = {
  id: string; text: string; files?: unknown[]; delivery: "steer" | "queue";
  from?: string; words: string; draft: string; before?: string;
} & Pick<Outgoing, "metadata">;

type Undo = { posted: boolean; settled?: boolean; pid?: number };
type Removal = { from: "inbox" | "box"; removed: RemovedWaiting; token: string; complete: boolean; undo?: Undo };
const removalFormat = additiveObjectFormat("Queue removal receipt");
const handoffKey = (session: string, item: string) => crypto.createHash("sha256").update(JSON.stringify([session, item])).digest("hex");
const removalFile = (session: string, item: string) => path.join(RUN, "queue-removals", handoffKey(session, item) + ".json");
const spentRemoval = (session: string, item: string, token: string) => path.join(RUN, "queue-removals", "spent", handoffKey(handoffKey(session, item), token));

function readRemoval(session: string, item: string): Removal | undefined {
  let text: string;
  try { text = fs.readFileSync(removalFile(session, item), "utf8"); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
    throw error;
  }
  const receipt = removalFormat.read(JSON.parse(text)) as unknown as Removal;
  return removalSpent(session, item, receipt.token) ? undefined : receipt;
}

function removalSpent(session: string, item: string, token: string): boolean {
  return fs.statSync(spentRemoval(session, item, token), { throwIfNoEntry: false }) !== undefined;
}

function spendRemoval(session: string, item: string, token: string): Promise<void> {
  return writeDurablePrivateLater(spentRemoval(session, item, token), "", RUN);
}

function writeRemoval(session: string, item: string, receipt: Removal) {
  return writeDurablePrivateLater(removalFile(session, item), JSON.stringify(removalFormat.write(receipt)), RUN);
}

async function forgetRemoval(session: string, item: string, token: string): Promise<void> {
  // Keep only the operation ID, so an old retry can't delete a new same-ID Undo.
  await spendRemoval(session, item, token);
  const file = removalFile(session, item);
  await fs.promises.unlink(file);
  await syncDurableFileLater(path.dirname(file), RUN);
}

export function acknowledgeRemoval(session: string, item: string, token: string): Promise<void> {
  return duringHandoff(session, item, async () => {
    const receipt = readRemoval(session, item);
    if (receipt?.complete && receipt.token === token) await forgetRemoval(session, item, receipt.token);
  });
}

/** Undo starts a new occurrence under the same ID. Keep the draft until the new occurrence is
 * confirmed, including if the link is interrupted after the Add commits. */
export function undoRemoval(target: Target, session: string, message: Outgoing & { id: string }, add: (post?: () => Promise<void>) => Promise<boolean>): Promise<boolean> {
  const item = message.id;
  return duringHandoff(session, item, async () => {
    const receipt = readRemoval(session, item);
    if (receipt && !receipt.complete) throw new Error("Kuvo is still checking whether the message was taken back.");
    if (!receipt) return add();
    if (receipt.undo) {
      const restored = await restoredByUndo(target, session, item, receipt.undo);
      if (restored !== null) {
        await forgetRemoval(session, item, receipt.token);
        return restored;
      }
    }
    const remember = (undo: Undo) => writeRemoval(session, item, { ...receipt, undo });
    await remember({ posted: false });
    const result = await add(async () => {
      const undo = { posted: true, pid: target.pid };
      await remember(undo);
      try {
        await call(target, "POST", `/api/session/${address(session)}/prompt`, message);
      } catch (error) {
        if (handoffSettled(error)) await remember({ ...undo, settled: true });
        throw error;
      }
    });
    await forgetRemoval(session, item, receipt.token);
    return result;
  });
}

/** If Undo put the message back, whether it now waits on the box. Null means it never went. */
async function restoredByUndo(target: Target | undefined, session: string, item: string, undo: Undo): Promise<boolean | null> {
  if (queueOf(session, true).waiting?.some(entry => entry.id === item)) return true;
  if (!undo.posted) return null;
  if (!target) throw new WaitingHandoff("The engine isn't answering.");
  const fenced = undo.settled || handoffEngineGone(undo.pid);
  if ((await inboxNow(target, session, 10_000)).some(entry => entry.id === item) ||
      await hasMessage(target, session, "message", item, AbortSignal.timeout(10_000))) return false;
  if (!fenced) throw new WaitingHandoff("Undo may still be on its way.");
  return null;
}

function removedInbox(entry: InboxEntry, before: string | undefined): RemovedWaiting {
  const shown = shownOfInbox(entry);
  const text = typeof entry.payload?.text === "string" ? entry.payload.text : typeof entry.text === "string" ? entry.text : "";
  const files = sendableFiles(entry.payload?.files);
  const metadata = entry.payload?.metadata;
  return {
    id: entry.id, text, ...(files.length ? { files } : {}), delivery: entry.delivery,
    ...(isObject(metadata) ? { metadata } : {}), ...(shown.from ? { from: shown.from } : {}),
    words: shown.words,
    draft: shown.words,
    ...(before ? { before } : {}),
  };
}

/** Edit/Remove writes the draft before touching the destination. A retry can finish checking
 * the outcome after an interrupted DELETE or read, or replay a completed receipt. */
export function takeBackQueued(session: string, item: string, token: string | undefined, locate: () => Promise<{ target?: Target; bridge: boolean }>): Promise<(RemovedWaiting & { receipt: string }) | null> {
  return duringHandoff(session, item, async () => {
    if (token && removalSpent(session, item, token)) return null;
    const absent = async () => {
      if (token) await spendRemoval(session, item, token);
      return null;
    };
    let receipt = readRemoval(session, item);
    const place = receipt && !receipt.undo && receipt.complete ? undefined : await locate();
    const { target, bridge } = place ?? { bridge: false };
    if (receipt?.undo && await restoredByUndo(target, session, item, receipt.undo) !== null) {
      const previous = receipt.token;
      await forgetRemoval(session, item, previous);
      if (token === previous) return null;
      receipt = undefined;
    }
    if (token && receipt && receipt.token !== token) return absent();
    if (receipt?.complete) return { ...receipt.removed, receipt: receipt.token };
    const state = queueOf(session, true);
    if (state.waiting?.some(entry => entry.id === item)) {
      const inbox = await checkBoxRemoval(target, session, item, bridge, state);
      if (inbox === null) return absent();
      await dropWaiting(session, item, inbox, true, async removed => {
        receipt = { from: "box", removed, token: receipt?.token ?? token ?? crypto.randomUUID(), complete: false };
        await writeRemoval(session, item, receipt);
      });
    } else if (receipt?.from !== "box") {
      if (!target) throw new Error("The engine isn't answering.");
      let inbox = await inboxNow(target, session, 10_000);
      const entry = inbox.find(each => each.id === item);
      if (entry) {
        if (!receipt) {
          const order = orderOf(inbox, state);
          receipt = { from: "inbox", token: token ?? crypto.randomUUID(), complete: false, removed: removedInbox(entry, order[order.indexOf(item) + 1]) };
          await writeRemoval(session, item, receipt);
        }
        try {
          await call(target, "DELETE", `/api/session/${address(session)}/inbox/${address(item)}`, undefined, AbortSignal.timeout(10_000));
        } catch (error) {
          if ((error as { status?: number }).status !== 404) throw error;
        }
        inbox = await inboxNow(target, session, 10_000);
        if (inbox.some(each => each.id === item)) throw new Error("It couldn't be taken back. Try again in a moment.");
      }
      if (await hasMessage(target, session, "message", item, AbortSignal.timeout(10_000))) {
        if (receipt) {
          await forgetRemoval(session, item, receipt.token);
          return null;
        }
        return absent();
      }
    }
    if (!receipt) return absent();
    await changeQueueLater(session, state => ({ ...withoutOrder(state, item), held: state.held.filter(id => id !== item) }));
    await writeRemoval(session, item, { ...receipt, complete: true });
    return { ...receipt.removed, receipt: receipt.token };
  });
}

/** `session`'s next waiting message started, unless its queue is paused or a run is under way: as
 * `orderOf` has them, one in the engine's inbox by steering it (which starts a run) if it's queued,
 * by resuming if it's for the next step; one waiting on the box, sent to go now. */
export async function drain(target: Target, session: string): Promise<void> {
  await admitPrompt(session, () => drainAdmitted(target, session));
}

const limitReleases = new Map<string, { at: number; timer: ReturnType<typeof setTimeout> }>();

/** A queue that moved here still waits for the same limit, on its new box. */
export function watchQueueLimit(target: Target | (() => Target), session: string): void {
  const at = queueOf(session).limitStop?.resetsAt;
  if (at !== undefined) scheduleLimitRelease(typeof target === "function" ? target() : target, session, at);
}

/** A known limit holds the queue through its reset. The persisted stop is checked again before
 * releasing it, so an explicit Stop or Resume (or a newer limit) wins over an old timer. */
function scheduleLimitRelease(target: Target, session: string, at: number): void {
  const key = session;
  const before = limitReleases.get(key);
  if (before?.at === at) return;
  if (before) clearTimeout(before.timer);
  const timer = setTimeout(() => {
    limitReleases.delete(key);
    if (Date.now() < at) return scheduleLimitRelease(target, session, at);
    void releaseLimit(target, session, at).catch(() => {
      if (queueOf(session).limitStop?.resetsAt === at) scheduleLimitRelease(target, session, at);
    });
  }, Math.min(Math.max(at - Date.now(), 1000), 2 ** 31 - 1));
  timer.unref();
  limitReleases.set(key, { at, timer });
}

async function limitTarget(target: Target, session: string): Promise<Target> {
  const candidates = [target];
  for (const read of [readBridgeFile, readEngineFile]) {
    try {
      const current = read();
      if (!candidates.some(each => each.port === current.port && each.password === current.password)) candidates.push(current);
    } catch {}
  }
  let trouble: unknown;
  for (const candidate of candidates) {
    try {
      const answer = await call(candidate, "GET", `/api/session/${address(session)}`, undefined, AbortSignal.timeout(5000));
      if (isObject(answer) && answer.id === session) return candidate;
      trouble = new Error("The engine didn't confirm the session.");
    } catch (error) { trouble = error; }
  }
  throw trouble;
}

async function releaseLimit(target: Target, session: string, at: number): Promise<void> {
  await admitPrompt(session, async () => {
    if (queueOf(session).limitStop?.resetsAt !== at || Date.now() < at) return;
    const live = await limitTarget(target, session);
    let released = false;
    await changeQueueLater(session, state => {
      if (state.limitStop?.resetsAt !== at) return state;
      released = true;
      return { ...state, paused: false, resumed: Date.now(), limitStop: { ...state.limitStop, releasing: true } };
    });
    if (!released) return;
    if (await drainAdmitted(live, session) === "retry") {
      if (queueOf(session).limitStop?.resetsAt === at) scheduleLimitRelease(live, session, at);
      return;
    }
    await changeQueueLater(session, state => state.limitStop?.releasing && state.limitStop.resetsAt === at ? { ...state, limitStop: undefined } : state);
  });
}

async function drainAdmitted(target: Target, session: string, activating?: string): Promise<"retry" | void> {
  // (Nothing starts on an engine being changed: what waits goes once the new one runs.)
  if (queueOf(session).paused || swapHoldsFor(target, session) || (await isRunning(target, session))) return;
  const inbox = await inboxOf(target, session);
  // Re-read the saved order after the final asynchronous check, so a concurrent move wins.
  if (await isRunning(target, session)) return;
  const state = queueOf(session);
  if (state.paused || swapHoldsFor(target, session)) return;
  const first = orderOf(inbox, state)[0];
  if (!first) {
    if (state.order?.length) await changeQueueLater(session, current => current.order?.every(id => state.order!.includes(id)) ? { ...current, order: [] } : current);
    return;
  }
  const next = inbox.find(item => item.id === first);
  if (readRemoval(session, first)?.complete === false) return "retry";
  if (!next) {
    // The head can finish being removed before the send takes its lock.
    if (!await sendWaiting(target, session, first, { activating })) return "retry";
    return;
  }
  if (queueOf(session).waiting?.some(item => item.id === next.id) && await acceptedWaiting(target, session, next.id)) await confirmWaiting(session, next.id);
  if (next.delivery === "queue") await setDelivery(target, session, next.id, "steer");
  else await interrupt(target, session, true);
  await changeQueueLater(session, state => ({ ...withoutOrder(state, next.id), held: state.held.filter(id => id !== next.id) }));
}

// A send can lose its HTTP reply after the engine accepted it. Look in the inbox first (an item
// can move from there to the conversation between these reads), then look up its exact message ID.
function isBridge(target: Target): boolean {
  try { return readBridgeFile().port === target.port; } catch { return false; }
}

/** Whether message `item` reached `session` where it runs (its inbox, its conversation, or what
 * waits for it on this box): a send whose answer a restart lost is settled by this. */
export async function messageReached(target: Target, session: string, item: string): Promise<boolean> {
  if (queueOf(session).waiting?.some(each => each.id === item)) return true;
  if ((await inboxOf(target, session, undefined, true)).some(entry => entry.id === item)) return true;
  return acceptedWaiting(target, session, item);
}

async function acceptedWaiting(target: Target, session: string, item: string): Promise<boolean> {
  const bridge = isBridge(target);
  if (!bridge && (await inboxOf(target, session)).some(entry => entry.id === item)) return true;
  const route = bridge ? "prompt-receipt" : "message";
  return hasMessage(target, session, route, item);
}

async function hasMessage(target: Target, session: string, route: "message" | "prompt-receipt", item: string, signal?: AbortSignal): Promise<boolean> {
  const stored = await call(target, "GET", `/api/session/${address(session)}/${route}/${address(item)}`, undefined, signal).catch(error => {
    if ((error as { status?: number })?.status === 404) return undefined;
    throw error;
  });
  if (stored === undefined) return false;
  if (isObject(stored) && (stored.id === item || (isObject(stored.info) && stored.info.id === item))) return true;
  throw new Error("The engine did not confirm the message by ID.");
}

async function confirmWaiting(session: string, item: string): Promise<void> {
  await changeQueueLater(session, state => {
    files.readWaitingFile(waitingFile(item));
    return { ...withoutOrder(state, item), held: state.held.filter(id => id !== item), waiting: state.waiting?.filter(each => each.id !== item) };
  }, true);
  await fs.promises.rm(waitingFile(item), { force: true });
}

/** Send and Delete share this lock across the link and update workers. */
function duringHandoff<T>(session: string, item: string, work: () => Promise<T>): Promise<T> {
  return exclusiveWhile(path.join(RUN, ".queue-handoffs", handoffKey(session, item)), work);
}

async function checkBoxRemoval(target: Target | undefined, session: string, item: string, bridge: boolean, state: QueueState): Promise<InboxEntry[] | null> {
  if (!state.handoff?.includes(item)) return target ? await inboxFor(target, session) : [];
  if (!target) throw new WaitingHandoff("The engine isn't answering.");
  const token = bridge ? state.handoffTokens?.[item] : undefined;
  if (!token && !state.handoffSettled?.includes(item) && !handoffEngineGone(state.handoffPids?.[item])) {
    throw new WaitingHandoff("The previous send may still be on its way.");
  }
  const inbox = await inboxNow(target, session, 10_000);
  if (inbox.some(entry => entry.id === item)) throw new WaitingHandoff("The engine still holds this message.");
  if (await hasMessage(target, session, "message", item, AbortSignal.timeout(10_000))) return null;
  if (token) {
    // Absence alone cannot rule out a POST still in the bridge after the sender disconnected.
    await asked(target, "POST", `api/session/${address(session)}/prompt-receipt/${address(item)}/cancel`, { token }, 10_000);
  }
  return inbox;
}

/** A dead destination process cannot accept a delayed request. Require ESRCH, not a changed
 * port or a temporarily missing record. */
function handoffEngineGone(pid: number | undefined): boolean {
  if (!Number.isSafeInteger(pid) || !pid || pid < 1) return false;
  try { process.kill(pid, 0); }
  catch (error) { return (error as NodeJS.ErrnoException).code === "ESRCH"; }
  return false;
}

function handoffSettled(error: unknown): boolean {
  const failure = error as { status?: number; code?: string; cause?: { code?: string } };
  return !!failure.status || [failure.code, failure.cause?.code].some(code => code === "ECONNREFUSED" || code === "ConnectionRefused");
}

export const queueTiming = { sendNow: 30_000 };

export async function waitForSendNow<T>(work: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([work, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error("Kuvo is still checking whether the message was sent.")), queueTiming.sendNow);
    })]);
  } finally {
    clearTimeout(timer);
  }
}

const waitingSends = new Map<string, { item: string; sent: Promise<boolean> }>();
export const sendingWaiting = (session: string, item: string): boolean => waitingSends.get(session)?.item === item;

// Keep the index and payload until acceptance. A process killed before that point can retry with
// the exact same ID, including when the engine took the prompt but the reply was lost.
async function sendWaiting(target: Target, session: string, item: string, {
  whilePaused = false, steer = true, releasingHold = false, activating,
}: { whilePaused?: boolean; steer?: boolean; releasingHold?: boolean; activating?: string } = {}): Promise<boolean> {
  const pending = waitingSends.get(session);
  if (pending) {
    if (pending.item === item) return pending.sent;
    await pending.sent.catch(() => false);
    return sendWaiting(target, session, item, { whilePaused, steer, releasingHold, activating });
  }
  const sent = admitPrompt(session, () => duringHandoff(session, item, async () => {
    if (readRemoval(session, item)?.complete === false) return false;
    if (!await acceptedWaiting(target, session, item)) {
      const state = queueOf(session);
      if ((!whilePaused && state.paused) || !state.waiting?.some(each => each.id === item)) return false;
      if (!releasingHold && swapHoldsFor(target, session)) return false;
      const message = waitingPayload(item);
      if (!message) throw new Error("A waiting message could not be read");
      const bridge = isBridge(target);
      const mayStillArrive = state.handoff?.includes(item) && !state.handoffSettled?.includes(item) && !handoffEngineGone(state.handoffPids?.[item]);
      if (mayStillArrive && !state.handoffTokens?.[item]) throw new WaitingHandoff("The previous send may still be on its way.");
      const handoffToken = bridge ? state.handoffTokens?.[item] ?? (mayStillArrive ? undefined : crypto.randomUUID()) : undefined;
      const handing = await changeQueueLater(session, state => {
        const handoffTokens = { ...state.handoffTokens };
        if (handoffToken) handoffTokens[item] = handoffToken;
        else delete handoffTokens[item];
        return {
          ...state, handoff: [...new Set([...(state.handoff ?? []), item])], handoffTokens,
          handoffSettled: state.handoffSettled?.filter(id => id !== item),
          handoffPids: { ...state.handoffPids, [item]: handoffToken ? undefined : target.pid },
        };
      });
      if (!handing.handoff?.includes(item)) return false;
      const settled = async () => {
        if (!mayStillArrive) await changeQueueLater(session, state => ({
          ...state, handoffSettled: [...new Set([...(state.handoffSettled ?? []), item])],
        }));
      };
      try {
        // The route can time out while this operation still owns the lock. Only the bridge
        // can fence an aborted POST by token. For a native engine, wait for its own answer.
        await call(target, "POST", `/api/session/${address(session)}/prompt`, {
          ...message, ...(handoffToken ? { handoffToken } : {}), ...(steer ? { delivery: "steer" } : {}),
        }, handoffToken ? AbortSignal.timeout(30_000) : undefined);
      } catch (error) {
        if (handoffSettled(error)) await settled();
        throw error;
      }
      await settled();
      if (!await acceptedWaiting(target, session, item)) throw new Error("A waiting message was not confirmed");
    }
    await confirmWaiting(session, item);
    return true;
  }), activating).finally(() => waitingSends.delete(session));
  waitingSends.set(session, { item, sent });
  return sent;
}

/** What waits for `session`'s next step held in its queue, ahead of what was queued (the latest
 * first), and the queue paused when anything waits, in the engine or on the box: what Stop does
 * before it stops the run, and a failed run's. */
// `since`: for an interrupt then (ms), unless Resume or Send Now went on with it after. `items`:
// messages a client held as they went (queued or not), put first, in that order.
export async function holdQueue(target: Target, session: string, since?: number, items: string[] = [], limitStop?: QueueLimitStop): Promise<QueueState> {
  const resumedSince = (state: QueueState) => since !== undefined &&
    Math.max(state.resumed ?? 0, limitStop ? state.resumedLimitAt ?? 0 : 0) >= since;
  if (limitStop && (limitStop.resetsAt === undefined || limitStop.resetsAt > Date.now())) {
    // An inbox read can fail or stall. Keep the reason before asking it to move its messages.
    const state = await changeQueueLater(session, state =>
      state.userPaused || state.heldFor || resumedSince(state)
        ? state : { ...state, paused: true, limitStop });
    if (state.limitStop?.resetsAt !== undefined) scheduleLimitRelease(target, session, state.limitStop.resetsAt);
  }
  const inbox = messagesIn(await inboxOf(target, session));
  if (resumedSince(queueOf(session))) return queueOf(session);
  const held = queueOf(session).held;
  const named = items.filter(id => inbox.some(item => item.id === id));
  const steering = inbox.filter(item => item.delivery === "steer" && !held.includes(item.id) && !named.includes(item.id)).map(item => item.id);
  // Held only once it's in the queue (the agent may take one first).
  const moved: string[] = [];
  for (const id of [...named, ...steering]) {
    if (inbox.find(item => item.id === id)?.delivery !== "steer") moved.push(id);
    else if (await setDelivery(target, session, id, "queue").then(() => true, () => false)) moved.push(id);
  }
  const waiting = await inboxOf(target, session).then(messagesIn, () => inbox);
  const state = await changeQueueLater(session, state => {
    // (One held that waits on the box stays held.)
    const there = (id: string) => waiting.some(item => item.id === id) || Boolean(state.waiting?.some(each => each.id === id));
    if (resumedSince(state)) return state;
    const newlyHeld = moved.filter(there);
    return { ...state, paused: state.paused || waiting.length > 0 || Boolean(state.waiting?.length),
      held: [...newlyHeld, ...state.held.filter(id => !moved.includes(id) && there(id))],
      ...(state.order && newlyHeld.length ? { order: [...newlyHeld, ...state.order.filter(id => !moved.includes(id) && there(id))] } : {}) };
  });
  return state;
}

/** A user pause supersedes an automatic usage-limit release. */
export async function pauseForUser(session: string, { ifLimited = false } = {}): Promise<QueueState> {
  return await changeQueueLater(session, state => ifLimited && !state.limitStop ? state : { ...state, paused: true, userPaused: true, limitStop: undefined });
}

/** Stop: what waits held, the queue paused whether anything waits or not (what arrives later waits
 * on the box), then the run stopped. `holds` false (a bot stopping a session of its work, which it
 * may write to again): paused only when something waits, as a failed run's. `items`: messages a
 * client sent as Stop came, held first, in that order. */
export async function stopSession(target: Target, session: string, holds = true, items: string[] = []): Promise<QueueState> {
  if (holds) await pauseForUser(session);
  const held = await holdQueue(target, session, undefined, items);
  await interrupt(target, session);
  return held;
}

/** Resume: the queue goes on, its next message now. */
export async function resumeQueue(target: Target, session: string, activating?: string): Promise<QueueState> {
  return admitPrompt(session, async () => {
    await changeQueueLater(session, current => {
      checkWaitingFormats(current.waiting);
      return letQueueGo(current);
    }, true);
    await drainAdmitted(target, session, activating);
    return queueOf(session);
  }, activating);
}

/** Send Now: `item` to the agent right away, the step under way stopped for it (one that runs goes
 * on from there with it), the queue no longer paused. False when it isn't waiting. */
export async function sendNow(target: Target, session: string, item: string): Promise<boolean> {
  return admitPrompt(session, async () => {
    if (readRemoval(session, item)?.complete === false) return false;
    // While the engine is being changed: no longer paused, and it goes first once the new one runs.
    if (swapHoldsFor(target, session)) return goesFirst(session, item);
    if (queueOf(session).waiting?.some(each => each.id === item)) {
      const running = await isRunning(target, session);
      if (swapHoldsFor(target, session)) return goesFirst(session, item);
      if (!(await sendWaiting(target, session, item, { whilePaused: true }))) return false;
      await changeQueueLater(session, letQueueGo, true);
      if (running) await interrupt(target, session, true);
      return true;
    }
    const entry = (await inboxOf(target, session)).find(each => each.id === item);
    if (swapHoldsFor(target, session)) return goesFirst(session, item);
    if (!entry) return false;
    // (What waits on the box stays, to go after it.)
    await changeQueueLater(session, state => ({ ...letQueueGo(state), held: state.held.filter(id => id !== item) }));
    const running = await isRunning(target, session);
    // Steering wakes an idle session; a running one gets it once the step stops.
    if (entry.delivery === "queue") await setDelivery(target, session, item, "steer");
    if (running || entry.delivery === "steer") await interrupt(target, session, true);
    return true;
  });
}

/** Steer: `item`, queued, to the agent at its next step in the run under way, without stopping it
 * (the engine takes it then; the coding agents' bridge hands it to an agent that takes one partway,
 * Claude Code's and Codex's, else it goes next, once the turn ends). One waiting on the box, or
 * with nothing running, goes as Send Now sends it. A queue a Stop holds stays held. `toAgents`:
 * `target` is the bridge. False when it isn't waiting; it throws when the inbox can't be read. */
export async function steerQueued(target: Target, session: string, item: string, toAgents: boolean): Promise<boolean> {
  return admitPrompt(session, async () => {
    if (readRemoval(session, item)?.complete === false) return false;
    if (swapHoldsFor(target, session)) return goesFirst(session, item);
    // (The engine's inbox as it is: one that can't be read says so, rather than Send Now stopping
    // the step you meant to keep going.)
    const entry = (await inboxOf(target, session)).find(each => each.id === item);
    if (!entry || !(await isRunning(target, session))) return sendNow(target, session, item);
    // Only this one is let go of what a Stop held: the rest wait for Resume, as 0.7's Steer left them.
    await changeQueueLater(session, state => ({ ...withoutOrder(state, item), held: state.held.filter(id => id !== item) }));
    if (entry.delivery === "steer" && !toAgents) return true;
    try {
      await asked(target, "PATCH", `api/session/${address(session)}/inbox/${address(item)}`, { delivery: "steer", ...(toAgents ? { into: true } : {}) });
    } catch (error) {
      // (The agent took it meanwhile.)
      if (/ 404\.$/.test(String((error as Error | null)?.message))) return false;
      throw error;
    }
    return true;
  });
}

// Send Now or Steer while the engine is being changed: `item`, waiting on the box, goes first once
// the new engine runs, the queue no longer paused; false when it isn't waiting on the box.
async function goesFirst(session: string, item: string): Promise<boolean> {
  let found = false;
  await changeQueueLater(session, state => {
    const shown = state.waiting?.find(each => each.id === item);
    if (!shown) return state;
    files.readWaitingFile(waitingFile(item));
    found = true;
    return { ...letQueueGo(state), held: state.held.filter(id => id !== item), waiting: [shown, ...state.waiting!.filter(each => each !== shown)],
      order: [item, ...(state.order ?? []).filter(id => id !== item)] };
  });
  return found;
}

/** What `target`'s inbox holds for `session` (messages, not a summary waiting) moved onto the box,
 * waiting there in the order they'd have gone, held as they were: before an engine is stopped for
 * another (PROTOCOL.md, "A new engine"), so nothing it held is lost with it. One the engine gave to
 * the agent first is left. It throws when the inbox can't be read. */
export async function moveInboxToBox(target: Target, session: string): Promise<number> {
  const claim = path.join(RUN, ".inbox-transfers", crypto.createHash("sha256").update(session).digest("hex"));
  const taken = await lockedWhile(FILES.lock, async () => {
    let owner = 0;
    try { owner = Number(fs.readFileSync(claim, "utf8")); } catch {}
    if (owner === process.pid || processRuns(owner, "kuvo-link")) return false;
    writePrivate(claim, String(process.pid));
    return true;
  });
  if (!taken) throw new Error("The engine’s inbox is already being moved.");
  try {
    return await transferInbox(target, session);
  } finally {
    fs.rmSync(claim, { force: true });
  }
}

async function transferInbox(target: Target, session: string): Promise<number> {
  const inbox = messagesIn(await inboxOf(target, session, undefined, true));
  if (!inbox.length) {
    if (queueOf(session).inboxMoved) await changeQueueLater(session, state => ({ ...state, inboxMoved: undefined }));
    return 0;
  }
  let moved = 0;
  // (In the order they'd go, ahead of what waits on the box already.)
  const remaining = new Set(inbox.map(entry => entry.id));
  const order = orderOf(inbox, queueOf(session)).filter(id => remaining.has(id));
  for (const id of order) {
    const entry = inbox.find(each => each.id === id)!;
    const text = typeof entry.payload?.text === "string" ? entry.payload.text : typeof entry.text === "string" ? entry.text : null;
    if (text === null) continue;
    // (An ID the box can't name a file by: one of its own, the same each time.)
    const own = /^msg_[A-Za-z0-9]{1,80}$/.test(id) ? id : `msg_${crypto.createHash("sha256").update(id).digest("hex").slice(0, 32)}`;
    const attachments = sendableFiles(entry.payload?.files);
    const message = { id: own, text, delivery: entry.delivery, ...(attachments.length ? { files: attachments } : {}), ...(isObject(entry.payload?.metadata) ? { metadata: entry.payload!.metadata as Record<string, unknown> } : {}) };
    const shown = shownOfInbox(entry);
    const waiting: Waiting = { id: own, ...(shown.from ? { from: shown.from } : {}), text: shown.words, ...(shown.files.length ? { files: shown.files } : {}), at: Date.now() };
    // Kept on the box, in its place (after those moved before it, ahead of what waited there), before
    // it's taken from the engine: cut off between the two, it's in both rather than in neither.
    const saved = waitingFile(own);
    const alreadyKept = fs.existsSync(saved);
    await changeQueueLater(session, state => {
      files.writeWaitingFile(saved, message, RUN);
      const ranked = state.order && orderOf(inbox.filter(entry => remaining.has(entry.id)), state);
      const prefix = new Set(state.inboxMoved).add(own);
      const pending = new Map((state.waiting ?? []).map(item => [item.id, item]));
      pending.set(own, waiting);
      return {
        ...state,
        held: state.held.map(each => (each === id ? own : each)),
        inboxMoved: [...prefix],
        ...(ranked ? { order: [...new Set(ranked.map(each => each === id ? own : each))] } : {}),
        waiting: [...Array.from(prefix, id => pending.get(id)!), ...Array.from(pending.values()).filter(item => !prefix.has(item.id))],
      };
    }, true);
    if (!queueOf(session).inboxMoved?.includes(own)) throw new Error("The box couldn’t save the inbox order.");
    try {
      await asked(target, "DELETE", `api/session/${address(session)}/inbox/${address(id)}`);
    } catch (error) {
      // Lost replies can follow a successful delete. Keep the saved copy and defer the restart.
      if (alreadyKept || !(error instanceof Error) || error.message !== "It answered 404.") throw error;
      // (The agent took it first.)
      await changeQueueLater(session, state => ({
        ...withoutOrder(state, own),
        held: state.held.filter(each => each !== own),
        inboxMoved: state.inboxMoved?.filter(each => each !== own),
        waiting: (state.waiting ?? []).filter(each => each.id !== own),
      }), true);
      fs.rmSync(saved, { force: true });
      remaining.delete(id);
      continue;
    }
    remaining.delete(id);
    moved++;
  }
  if (messagesIn(await inboxOf(target, session, undefined, true)).length) throw new Error("The engine still holds pending messages.");
  await changeQueueLater(session, state => ({ ...state, inboxMoved: undefined }));
  return moved;
}

/** What waits on the box for each conversation on `target` that isn't paused, sent there, each once
 * and in its order (the first to go now, the rest as each was sent): once an engine change is done,
 * or let go of (PROTOCOL.md, "A new engine"). One the engine doesn't confirm stays in place,
 * and that conversation's rest waits for the next look. Sessions the
 * engine doesn't have (the coding agents') are left. How many went. */
export async function releaseWaiting(target: Target, releasingHold = false): Promise<number> {
  let sent = 0;
  for (const [session, state] of Object.entries(readQueues())) {
    if (state.paused || !state.waiting?.length) continue;
    try {
      await asked(target, "GET", `api/session/${address(session)}`);
      // A failed transfer may have left the same item on both sides. Finish it before sending
      // even the saved prefix, and leave this session held if the inbox still can't be cleared.
      if (queueOf(session).inboxMoved?.length) {
        if (await isRunning(target, session)) continue;
        await moveInboxToBox(target, session);
      }
    } catch {
      continue;
    }
    // The first starts a turn where none runs; into one that runs, each goes as it was sent.
    const running = await isRunning(target, session).catch(() => undefined);
    if (running === undefined) continue;
    let steer = !running;
    for (;;) {
      const now = queueOf(session);
      const next = orderOf([], now)[0];
      if (now.paused || !next) break;
      if (!(await sendWaiting(target, session, next, { steer, releasingHold }).catch(() => false))) break;
      steer = false;
      sent++;
    }
  }
  return sent;
}

/** An event, as far as the queue reads it. */
export interface QueueEvent {
  type: string;
  data?: { sessionID?: string; inboxID?: string; item?: { type?: string; delivery?: string; payload?: { text?: unknown; metadata?: unknown } }; reason?: string; metadata?: Record<string, unknown>; error?: unknown };
}

function limitStopOf(value: unknown, at: number): QueueLimitStop | undefined {
  const error = errorOf(value, at);
  if (!error || error.type === "provider.auth" || !scopeOf(error)) return undefined;
  // ACP failure messages already include the reset for the conversation. Keep the queue's
  // message and the absolute timestamp separately so the box can render the time in your zone.
  const message = error.limit?.resetsAt ? error.message.replace(/ It resets at .+\.$/, "") : error.message;
  return { message: message || "The model reached a usage limit.", resetsAt: resetOf(error) };
}

// A message you wrote, not one Kuvo Code sent the agent for you or another (an agent's letter, a
// wake-up, a group chat's work, a note).
// (Those marked as the box's own, and the ones a box sent before it marked them.)
const NOT_YOURS = ["[Message from ", "[Wake-up]", "[Work from ", "[Session ", "[Goal started]", "[Goal changed]", "[Goal's end]", "[Goal resumed]", "[Not now]", "[Limit reset]", "[The user answered]"];
export const writtenByYou = (text: unknown, metadata: unknown) => !isBoxNote(metadata) && !NOT_YOURS.some(start => String(text ?? "").startsWith(start));
// What the box says of its own accord (what another session did, a limit that reset), rather than
// passes on from someone: a letter or work from an agent, or what you answered, approved or picked.
const FROM_SOMEONE = ["[Message from ", "[Work from ", "[The user ", "[Decision changed]"];
export const isBoxsOwnWord = (text: unknown, metadata: unknown) => !writtenByYou(text, metadata) && !FROM_SOMEONE.some(start => String(text ?? "").startsWith(start));
const isYours = (item: NonNullable<NonNullable<QueueEvent["data"]>["item"]>) => item.type === "user" && writtenByYou(item.payload?.text, item.payload?.metadata);

/** How the queue's watcher waits (a test's clock). */
export interface QueueWatch {
  after?: number;
  /** The sessions `scanQueues` found at work as this stream connected (`ScanFound`): each settled
   * as it would have, once the session says it's free. */
  found?: ScanFound;
  schedule?: (run: () => void, ms: number) => unknown;
  cancel?: (timer: unknown) => void;
}

/** What the box does with its engine's (or the coding agents' bridge's) events for the queue. A
 * run that ends well: the next message, a moment later. Stopped (the engine says by you, or as it
 * shut down; the bridge says nothing): held and paused a moment later, unless a run started again
 * meanwhile (Send Now's resume does at once), whichever client stopped it; one the engine ended
 * itself (superseded, idle too long) goes on as one that ended well. Failed: held and paused. A message queued while nothing runs: it goes, a moment
 * later; one for the next step: the queue isn't paused any more (you sent a message to go now).
 * One delivered or taken back leaves what's held. Paused, it stays so until Resume, Send Now or a
 * message of yours to go now, though nothing is left waiting: what arrives then waits on the box. */
export function queueWatcher(target: Target, { after = DRAIN_AFTER, schedule = setTimeout, cancel = clearTimeout as (timer: unknown) => void, found }: QueueWatch = {}): (event: QueueEvent) => void {
  const drains = new Map<string, unknown>();
  const stops = new Map<string, unknown>();
  const later = (timers: Map<string, unknown>, session: string, work: () => Promise<unknown>) => {
    if (timers.has(session)) cancel(timers.get(session));
    timers.set(session, schedule(() => {
      timers.delete(session);
      work().catch(() => {});
    }, after));
  };
  const forget = (timers: Map<string, unknown>, session: string) => {
    if (timers.has(session)) cancel(timers.get(session));
    timers.delete(session);
  };
  // A run's end, a moment on: `then`, once the session's no longer at work (`waited`: it said it
  // was free since). Still at work (the coding agents' bridge keeping the files a run changed),
  // looked at again once it says it's free (`session.idle`). Only the latest end, until a run starts.
  interface End { timers: Map<string, unknown>; then: (waited: boolean) => Promise<unknown> }
  const ends = new Map<string, End>();
  const look = (session: string, end: End, waited: boolean) => later(end.timers, session, async () => {
    if (ends.get(session) !== end) return;
    const running = await isRunning(target, session).catch(() => false);
    if (running || ends.get(session) !== end) return;
    ends.delete(session);
    await end.then(waited);
  });
  const ended = (session: string, timers: Map<string, unknown>, then: End["then"]) => {
    const end = { timers, then };
    ends.set(session, end);
    look(session, end, false);
  };
  return event => {
    const session = event.data?.sessionID;
    if (!session) return;
    // (What a client follows of the queue, read again.)
    if (event.type.startsWith("session.inbox.")) for (const told of inboxFollowers) try { told(session); } catch {}
    switch (event.type) {
      case "session.execution.started":
        forget(drains, session);
        forget(stops, session);
        ends.delete(session);
        if (queueOf(session).resumedLimitAt) void changeQueueLater(session, state => ({ ...state, resumedLimitAt: undefined })).catch(() => {});
        break;
      case "session.execution.succeeded":
        forget(stops, session);
        ended(session, drains, () => drain(target, session).catch(() => drain(target, session)));
        break;
      case "session.execution.interrupted": {
        if (event.data?.reason === "superseded" || event.data?.reason === "inactivity" || event.data?.reason === "agent-exit") {
          ended(session, drains, () => drain(target, session));
          break;
        }
        // The coding agents' bridge stopping with it (an update, a restart): not a Stop. It keeps
        // what waits for the next bridge, which sends it as it starts; nothing's held or paused.
        if (event.data?.reason === "restart") break;
        // A Stop: held and paused, unless it was resumed since (Resume, or Send Now's stop-and-go,
        // which go on by themselves). Resumed by the time the bridge says it's free (Resume's own
        // look may have found it still at work), its next goes now.
        const at = Date.now();
        ended(session, stops, async waited => {
          if ((queueOf(session).resumed ?? 0) < at) {
            await holdQueue(target, session, at);
            await letSummariesGo(target, session);
          } else if (waited) await drain(target, session);
        });
        break;
      }
      case "session.idle": {
        // Free now (the coding agents' bridge, once it kept what a run changed or put files
        // back): what its end left to do; one this stream connected to while it was at work (a
        // run's end not heard), as `scanQueues` would have.
        const end = ends.get(session);
        const known = found?.get(session);
        found?.delete(session);
        if (end) look(session, end, true);
        else later(drains, session, async () => {
          if (await isRunning(target, session)) return;
          if (known !== undefined) await settleFound(target, session, known);
          else if (queueOf(session).waiting?.length || (await inboxOf(target, session)).length) {
            if (!(await queueEnd(target, session)).failed) await drain(target, session);
          }
        });
        break;
      }
      case "session.execution.failed": {
        forget(stops, session);
        // (Not when resumed since.) Resumed while the bridge was still keeping what it changed:
        // its next goes once it's free.
        const at = Date.now();
        const limitStop = limitStopOf(event.data?.error, at);
        // Only a summary that failed: what's queued goes on, as after any turn.
        const onlySummary = onlyASummaryFailed(target, session);
        onlySummary.then(async only => {
          if (only) return;
          await holdQueue(target, session, at, [], limitStop);
          await letSummariesGo(target, session);
        }).catch(() => {});
        ended(session, stops, async waited => {
          if (await onlySummary) await drain(target, session);
          else if (waited && (queueOf(session).resumed ?? 0) >= at) await drain(target, session);
        });
        break;
      }
      case "session.inbox.enqueued":
        // You writing to it for now: it goes on. (Not a letter, a wake-up or a note.)
        if (event.data?.item?.delivery === "steer" && isYours(event.data.item)) void changeQueueLater(session, state => state.limitStop ? letQueueGo(state) : { ...state, paused: false }).catch(() => {});
        else if (!queueOf(session).paused) later(drains, session, () => drain(target, session));
        break;
      case "session.metadata.updated": {
        // Carried on in another session (moved to the coding agents, by a client or this box): its
        // queue goes on there. (Only while it's put away as carried: one brought back keeps the
        // mark, which a client sends again with every later change, and its queue is its own.)
        const metadata = event.data?.metadata;
        const to = metadata?.["kuvo.carriedTo"];
        // (Not while this box carries it: the carry takes its queue itself, with what it renames.)
        if (metadata?.["kuvo.archived"] === true && typeof to === "string" && to.startsWith("ses_") && to !== session && !carryUnderWay(session)) void carryQueue(session, to, {}).catch(() => {});
        break;
      }
      case "session.deleted":
        // Gone: its queue and what waited for it with it.
        forget(drains, session);
        forget(stops, session);
        ends.delete(session);
        void forgetQueueLater(session).catch(() => {});
        break;
      case "session.inbox.delivered":
      case "session.inbox.cancelled": {
        const gone = event.data?.inboxID;
        if (!queueOf(session).held.length) break;
        inboxOf(target, session).then(inbox => changeQueueLater(session, state => ({
          ...state,
          // (One held that waits on the box stays held.)
          held: state.held.filter(id => (id !== gone && inbox.some(item => item.id === id)) || Boolean(state.waiting?.some(each => each.id === id))),
        }))).catch(() => {});
        break;
      }
    }
  };
}

// The engine (or the agents' bridge) whose queues were last looked over, by its port, kept in
// `run/` so a box that starts again beside the same engine doesn't take it for a new one.
const scannedFormat = additiveObjectFormat("Conversation queue scan receipts");
const SCANNED_FILE = path.join(RUN, "queues-scanned.json");

// Whether `session`'s last run was stopped or failed, as its newest message says (the run's end,
// as the engine and the bridge mark it, or an answer that ended in an error); unknown, it was.
async function queueEnd(target: Target, session: string): Promise<{ failed: boolean; resumable?: boolean; open?: boolean; limitStop?: QueueLimitStop; at?: number }> {
  const page = await asked(target, "GET", `api/session/${address(session)}/message?limit=1&order=desc`).catch(() => null) as { data?: { type?: string; outcome?: unknown; reason?: unknown; error?: unknown; time?: { created?: number } }[] } | null;
  if (!Array.isArray(page?.data)) return { failed: true };
  const last = page.data[0];
  // (The process went away: what waits goes on.)
  if (last?.type === "idle" && last.outcome === "interrupted" && (last.reason === "restart" || last.reason === "agent-exit")) return { failed: false, resumable: true, at: last.time?.created };
  const failed = last?.type === "idle" ? last.outcome !== "succeeded" : last?.type === "assistant" && isObject(last.error);
  const at = last?.time?.created;
  const open = last?.type === "user" || last?.type === "assistant";
  const resumable = last?.type === "idle" && last.outcome === "succeeded";
  return { failed, open, resumable, at, ...(failed ? { limitStop: limitStopOf(last.error, at ?? Date.now()) } : {}) };
}

/** Sessions with messages waiting that `scanQueues` found at work: whether it knew their engine
 * (else it started anew), for the watcher to settle each once it's free. */
export type ScanFound = Map<string, boolean>;

// A queue continues after a reported process exit or an open turn left by a replaced engine.
// Explicit stops and failures still hold it.
async function settleFound(target: Target, session: string, known: boolean): Promise<void> {
  const end = await queueEnd(target, session);
  if (end.limitStop?.resetsAt !== undefined && end.limitStop.resetsAt <= Date.now()) {
    const saved = queueOf(session).limitStop?.resetsAt;
    if (saved !== undefined) await releaseLimit(target, session, saved);
    else await drain(target, session);
  } else if ((known || end.resumable || end.open) && !end.failed) await drain(target, session);
  else await holdQueue(target, session, end.limitStop ? end.at : undefined, [], end.limitStop);
}

/** After reconnecting, settle waiting messages from the last end or the transcript of the turn
 * the process left open. Sessions still at work go in `found`, for the watcher. */
export async function scanQueues(target: Target & { pid?: number }, found?: ScanFound): Promise<void> {
  const seen = await lockedWhile(FILES.lock, async () => {
    let scanned: Record<string, unknown> = {};
    try { scanned = scannedFormat.read(JSON.parse(fs.readFileSync(SCANNED_FILE, "utf8"))); } catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error;
    }
    if (target.pid !== undefined && scanned[target.port] === target.pid) return true;
    scanned[target.port] = target.pid;
    try { writePrivate(SCANNED_FILE, JSON.stringify(scannedFormat.write(scanned))); } catch {}
    return false;
  });
  // The coding agents' bridge keeps what waits across its restarts (an update, a restart: its
  // sessions' inboxes are on disk), and says when a Stop ended a run: its queues go on as the same
  // one's would, a Stop's or a failure's held and paused.
  const known = seen || isBridge(target);
  const page = await asked(target, "GET", "api/session?parentID=null&limit=100&order=desc") as { data?: { id?: string; outcome?: unknown; time?: { updated?: number } }[] } | null;
  const week = Date.now() - 7 * 86_400_000;
  for (const session of page?.data ?? []) {
    if (typeof session.id !== "string" || (session.time?.updated ?? 0) < week) continue;
    try {
      if (!(await inboxOf(target, session.id)).length) {
        // A failed turn on the agents' bridge can leave its inbox empty until the next letter.
        if (session.outcome === "failed" && !(await isRunning(target, session.id))) {
          const end = await queueEnd(target, session.id);
          const stop = end.limitStop;
          if (stop && (stop.resetsAt === undefined || stop.resetsAt > Date.now())) await holdQueue(target, session.id, end.at, [], stop);
        }
        continue;
      }
      if (await isRunning(target, session.id)) {
        found?.set(session.id, known);
        // (Its word that it's free may have come before it was found: free now, it's settled here,
        // unless the watcher heard that and took it.)
        if (!found || (await isRunning(target, session.id)) || !found.delete(session.id)) continue;
      }
      await settleFound(target, session.id, known);
    } catch {}
  }
  // What waits on the box for one that isn't paused (the box stopped before it went): it goes now.
  // (A session of the engine's or the bridge's: the other one doesn't have it, and keeps it.)
  for (const [session, state] of Object.entries(readQueues())) {
    const reset = state.limitStop?.resetsAt;
    if (reset === undefined && (state.paused || !state.waiting?.length)) continue;
    try {
      await asked(target, "GET", `api/session/${address(session)}`);
    } catch {
      continue;
    }
    if (reset === undefined) await drain(target, session).catch(() => {});
    else if (reset <= Date.now()) await releaseLimit(target, session, reset).catch(() => scheduleLimitRelease(target, session, reset));
    else scheduleLimitRelease(target, session, reset);
  }
}
