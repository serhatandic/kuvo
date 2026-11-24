import { queuedRefusal } from "../server/context";
// A session's queue, as every client reads and changes it (PROTOCOL.md, "The queue"): whether it
// waits for Resume, what Stop held and what waits on the box, followed as it changes; Stop, Resume,
// Send Now, and a client's own message to wait with it.

import { UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject, parsed } from "../../shared/json";
import { BAD_WAITING_ID, isWaitingID, wakeMarkOf } from "../../shared/queues";
import { MOVING } from "../bots/here";
import { admitPrompt, MoveAdmissionClosed } from "../moves/admission";
import { frozen, liveMoves } from "../moves/records";
import { followed, followedLive } from "../follow";
import { isRefused, placeOfSession, placeOfSessionPatiently } from "../letters/letter";
import { keepsList, keptSessions } from "../lists/sessions";
import { targetsOf } from "../letters/tools";
import { asked } from "../engine/client";
import { acknowledgeRemoval, holdQueue, inboxFollowers, inboxNow, keepWaitingBefore, pendingQueue, queueFollowers, queueItems, queueOf, reorderQueue, resumeQueue, sendableFiles, sendNow, sendingWaiting, shownQueues, shownQueue, steerQueued, takeBackQueued, undoRemoval, waitForSendNow } from "../engine/queue";
import type { RemovedWaiting } from "../engine/queue";
import type { QueueShown } from "../../shared/routes/sessions";
import { forgetStopped } from "../work/background";
import { forgetArriving, holdArriving, sendsSettled } from "../messages/arriving";
import { stopConversation } from "../work/stopping";
import { messageID } from "../engine/sessions";
import { attachedIn } from "../server/attached";
import { fieldsOf, keptParts, partsOf, withParts, withPartsOf } from "../messages/parts";
import { composed, isEmpty, NotListed, sentAgain } from "./messages";
import type { Composed } from "./messages";
import { fromPhone, modelInput } from "../server/forward";
import { namedIn, namingHere } from "../server/naming";
import { answering, BUSY, collected, forgetSent, SENDING_PATIENCE, sentKey, sentOnce, wentAlready } from "../server/sent-once";
import { fromAPhone } from "../server/context";
import type { AnswerPiece, RouteContext } from "../server/context";
import type { Target } from "../../shared/records";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { attachmentRoots } from "../home";

export const QUEUE_ROUTE = /^kuvo\/sessions\/(?:queues\/events|([^/]+)\/(stop|queue(?:\/(hold|resume|add|events|reorder))?|queue\/([^/]+)(?:\/(now|steer|ack))?))$/;

export function queueHoldWhileSettling(session: string): boolean {
  return liveMoves().some(move => move.role === "from" && move.stage === "holding"
    && (move.thing === session || move.sessions.includes(session)));
}

// Give the session lookup and inbox read time to answer while the bridge saves a long transcript.
// An incomplete read leaves the last complete queue in place.
const ITEMS_WAIT = 10_000;

/** `session`'s queue as a client shows it, with every message waiting (`items`, in their order;
 * null while the engine or the coding agents' bridge, where it may be, can't be read). */
async function queueShown(session: string, targets: Target[], following = false): Promise<QueueShown> {
  try {
    const target = keepsList() ? targets.find(each => keptSessions(each)?.has(session)) : undefined;
    const place = target ? { target } : targets.length ? await placeOfSession(session, targets, ITEMS_WAIT) : undefined;
    if (place) {
      const inbox = await inboxNow(place.target, session, ITEMS_WAIT);
      const state = await pendingQueue(place.target, session, inbox, queueOf(session, true), ITEMS_WAIT);
      return { ...shownQueue(state), items: queueItems(inbox, state) };
    }
    if (place === null && !following) {
      const state = queueOf(session, true);
      return { ...shownQueue(state), items: queueItems([], state) };
    }
  } catch {}
  return { ...shownQueue(queueOf(session)), items: null };
}


// The same snapshot serves a one-shot read and the initial line of an existing queue follow.
type QueueSnapshot = { targets: Target[]; items: QueueShown["items"]; refreshing?: Promise<QueueShown> };
const snapshots = new Map<string, QueueSnapshot>();

function refreshQueue(session: string, targets: Target[]): QueueSnapshot {
  let saved = snapshots.get(session);
  if (!saved || targets.length !== saved.targets.length || targets.some((target, i) =>
    target.port !== saved!.targets[i]!.port || target.password !== saved!.targets[i]!.password)) {
    saved = { targets, items: null };
  }
  snapshots.delete(session);
  snapshots.set(session, saved);
  if (snapshots.size > 1000) snapshots.delete(snapshots.keys().next().value!);
  const snapshot = saved;
  snapshot.refreshing ??= queueShown(session, targets, true).then(queue => {
    if (queue.items !== null) snapshot.items = queue.items;
    return queue;
  }).finally(() => { delete snapshot.refreshing; });
  return snapshot;
}

// `kuvo/sessions/queues/events`: every queue that's paused or holds something, a line now and one
// each time that changes. `GET kuvo/sessions/<id>/queue`: one session's. `POST …/stop`: what waits
// held and paused, then the run stopped. `POST …/queue/hold`: held and paused without stopping.
// `POST …/queue/resume`: the queue goes on. `POST …/queue/<item>/now`: that message right away.
// `POST …/queue/<item>/steer`: that message at the agent's next step, the run going on.
// `POST …/queue/add`: a client's message for the queue, waiting on the box while it's paused (else
// to the engine). `DELETE …/queue/<item>`: one waiting on the box taken away.
export async function* queueRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, signal, json } = context;
  const method = request.method.toUpperCase();
  const [, rawSession, action, step, item, now] = match;
  if (!rawSession) {
    if (method !== "GET") return yield* json(405, {});
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followed(() => ({ queues: shownQueues() }), queueFollowers, signal);
  }
  const session = decodeURIComponent(rawSession);
  if (action === "queue" && !step && !item) {
    if (method !== "GET") return yield* json(405, {});
    // Its items only when asked (`?items=1`): a client that doesn't draw them (the Mac's, a
    // phone's) gets the box's own state at once, without the engine asked.
    const wantsItems = new URLSearchParams(request.path.split("?")[1] ?? "").get("items") === "1";
    if (!wantsItems) return yield* json(200, shownQueue(queueOf(session)));
    const targets = targetsOf({ readEngine, bridge });
    if (!keepsList()) return yield* json(200, await queueShown(session, targets));
    const snapshot = refreshQueue(session, targets);
    return yield* json(200, { ...shownQueue(queueOf(session)), items: snapshot.items });
  }
  // One session's queue followed, its items too: again as the box changes it, or the engine's
  // inbox for it does (`session.inbox.*`). An unreadable inbox, or an owner missing during a
  // restart, leaves the last complete list in place.
  if (step === "events") {
    if (method !== "GET") return yield* json(405, {});
    const look = () => JSON.stringify(queueOf(session));
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followedLive(async () => {
      const targets = targetsOf({ readEngine, bridge });
      // Await an in-flight read first, then sample after the event that woke this follow.
      if (keepsList()) await snapshots.get(session)?.refreshing;
      const queue = await (keepsList() ? refreshQueue(session, targets).refreshing! : queueShown(session, targets, true));
      return queue.items === null ? undefined : queue;
    }, told => {
      const heard = (changed: string) => { if (changed === session) told(); };
      let state = look();
      const changed = () => {
        const next = look();
        if (state !== next) {
          state = next;
          told();
        }
      };
      queueFollowers.add(changed);
      inboxFollowers.add(heard);
      return () => { queueFollowers.delete(changed); inboxFollowers.delete(heard); };
    }, signal ?? undefined, { heartbeat: 15_000, discardStale: true, look });
  }
  // One moving to or from another computer: what waits for it goes as it was read for the move. A
  // Stop still stops it while the box it leaves waits for its turn to end, before it's read.
  const stopping = action === "stop" || step === "hold";
  if (frozen().has(session) && !(stopping && queueHoldWhileSettling(session))) return yield* json(409, { message: MOVING });
  if (item && now === "ack") {
    if (method !== "POST") return yield* json(405, {});
    const given = parsed(request.body);
    if (!isObject(given) || typeof given.receipt !== "string") return yield* json(400, { message: "Give the removal receipt." });
    try { await acknowledgeRemoval(session, decodeURIComponent(item), given.receipt); }
    catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error;
      return yield* json(502, { message: "Kuvo couldn't acknowledge the queue removal. Try again in a moment." });
    }
    return yield* json(200, shownQueue(queueOf(session)));
  }
  if (item && !now) {
    if (method !== "DELETE") return yield* json(405, {});
    const id = decodeURIComponent(item);
    const token = new URLSearchParams(request.path.split("?")[1] ?? "").get("receipt") ?? undefined;
    if (token !== undefined && !isWaitingID(token)) return yield* json(400, { message: "Give a valid removal receipt." });
    if (sendingWaiting(session, id)) return yield* json(502, { message: "Kuvo is handing this message over. Try again in a moment." });
    try {
      const removed = await takeBackQueued(session, id, token, async () => {
        const place = await placeOfSession(session, targetsOf({ readEngine, bridge })).catch(() => undefined);
        return { target: place?.target, bridge: !!place && bridge()?.port === place.target.port };
      });
      if (!removed) {
        // One the client still shows on its way (`arriving=1`): taken out as it arrives, or, gone
        // to the agent already, said so. Never a message on screen that can't be acted on.
        const arriving = new URLSearchParams(request.path.split("?")[1] ?? "").get("arriving") === "1";
        if (arriving && wentAlready(session, id)) return yield* json(404, { message: "It was already sent to the agent.", delivered: true });
        if (arriving) {
          await holdArriving(session, [id]);
          return yield* json(409, { message: "It’s still on its way, and is taken out as it arrives.", arriving: true });
        }
        return yield* json(404, { message: "That message isn't waiting any more." });
      }
      forgetSent(session, id);
      return yield* json(200, { ...shownQueue(queueOf(session)), removed: { ...removed, attached: attachedOf(removed) } });
    } catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error;
      return yield* json(502, { message: String((error as Error | null)?.message ?? error) });
    }
  }
  if (method !== "POST") return yield* json(405, {});
  // Only an engine that isn't here isn't running: one that's only slow to answer (a busy computer)
  // is asked again, and still slow, says so, for its client to try again (#443).
  const targets = targetsOf({ readEngine, bridge });
  if (!targets.length) return yield* json(503, { message: "The engine isn't running." });
  let place: Awaited<ReturnType<typeof placeOfSessionPatiently>>;
  try {
    place = await placeOfSessionPatiently(session, targets);
  } catch (error) {
    // Nothing listening where its record says: it isn't running. Else it's there, and too busy.
    if (isRefused(error)) return yield* json(503, { message: "The engine isn't running." });
    return yield* json(504, { message: BUSY, busy: true });
  }
  if (!place) return yield* json(404, { message: "There's no such conversation here." });
  const veto = queuedRefusal(context.admitQueued?.() ?? null, json);
  if (veto) return yield* veto;
  try {
    if (action === "stop") {
      // (`{items}`: messages the client sent that may still be on their way: held as they come.
      // What the box is sending meanwhile reaches the engine first, and is held with the rest.)
      const given = parsed(request.body);
      const items = isObject(given) && Array.isArray(given.items) ? given.items.filter((id): id is string => typeof id === "string") : [];
      await holdArriving(session, items);
      await sendsSettled(session);
      return yield* json(200, shownQueue(await stopConversation(place.target, { ...place.data, id: session }, items)));
    }
    if (step === "hold") {
      // (`{items}`: messages a client held as they went, first.)
      const given = parsed(request.body);
      const items = isObject(given) && Array.isArray(given.items) ? given.items.filter((id): id is string => typeof id === "string") : [];
      return yield* json(200, shownQueue(await holdQueue(place.target, session, undefined, items)));
    }
    if (step === "add") {
      // Sent again under its id (the phone's link dropped before the answer came, #144): added once,
      // every send answered as the one that went (`sentOnce`).
      const once = sentKey("queue", session, request.body);
      const target = place.target;
      if (!once) return yield* add(context, target, session);
      return yield* answering(await sentOnce(once, () => collected(add(context, target, session))));
    }
    if (step === "reorder") {
      const given = parsed(request.body);
      if (!isObject(given) || typeof given.item !== "string" || typeof given.anchor !== "string" || typeof given.before !== "boolean")
        return yield* json(400, { message: "Give the message, its neighbor, and which side to put it on." });
      const reordered = await reorderQueue(place.target, session, given.item, given.anchor, given.before);
      if (!reordered)
        return yield* json(409, { message: "Those messages can’t be moved now. Check the queue and try again." });
      return yield* json(200, { ...shownQueue(reordered.state), items: reordered.items });
    }
    // (You set it going: if it was stopped, what comes of it is news again, and what Stop named as
    // on its way goes as it was sent when it comes.)
    forgetStopped(session);
    await forgetArriving(session);
    if (step === "resume") return yield* json(200, shownQueue(await resumeQueue(place.target, session)));
    const go = now === "steer" ? steerQueued(place.target, session, decodeURIComponent(item), bridge()?.port === place.target.port) : waitForSendNow(sendNow(place.target, session, decodeURIComponent(item)));
    if (!(await go)) return yield* json(404, { message: "That message isn't waiting any more." });
    return yield* json(200, shownQueue(queueOf(session)));
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    if (error instanceof MoveAdmissionClosed) return yield* json(409, { message: error.message });
    return yield* json(502, { message: String((error as Error | null)?.message ?? error) });
  }
}

/** Everything a message taken back went with, for Edit to put back in the composer
 * (`removed.attached`): the files it carries, then those this box kept for the agent to open (its
 * note names them, and `files` doesn't carry them), each as a send takes it. Undo keeps to `files`:
 * its text names the kept ones already. */
export function attachedOf(removed: Pick<RemovedWaiting, "files" | "text" | "metadata">): { uri: string; name?: string }[] {
  const carried = sendableFiles(removed.files);
  const roots = attachmentRoots();
  const kept = ((keptParts(removed.metadata) ?? partsOf(removed.text)).paths ?? [])
    .filter(at => roots.some(root => at.startsWith(root + path.sep)) && fs.statSync(at, { throwIfNoEntry: false })?.isFile())
    .map(at => ({ uri: pathToFileURL(at).href, name: path.basename(at) }));
  const seen = new Set(carried.map(file => file.uri));
  return [...carried, ...kept.filter(file => !seen.has(file.uri) && seen.add(file.uri))];
}

// A client's own message for the queue (Undo, or one sent after this run while it's paused): its
// files kept on the box as a message's are (`attachedIn`), then waiting there while the queue is
// paused, else to the engine as it asked. Its forms (`id?`, `files?`, `delivery?`, `from?`,
// `before?` and `metadata?` with each):
// - `{words, widget?, comments?, named?, notes?}`: a message's fields
//   (`kuvo/sessions/<id>/send`'s); the box writes what the agent reads.
// - `{text, again: true, words?}`: one sent before, sent again as the agent read it (`words`: how
//   it's shown); its parts read from it.
// - `{text, kept: true, words?, metadata?}`: one a DELETE gave back (Undo), as it was kept already,
//   with the parts it had.
// `before`: Undo's, what came after it, which it goes back ahead of; `metadata`: a wake-up's mark
// (`wakeMark`), kept with it. `waits` says whether it waits on the box.
async function* add({ request, bridge, readEngine, json, signal }: RouteContext, target: Target, session: string): AsyncGenerator<AnswerPiece> {
  const given = parsed(request.body);
  if (!isObject(given)) return yield* json(400, { message: "Send what was written (words)." });
  if (given.id !== undefined && !isWaitingID(given.id)) return yield* json(400, { message: BAD_WAITING_ID });
  const id = given.id ?? messageID();
  const files = Array.isArray(given.files) ? given.files : [];
  const toAgents = bridge()?.port === target.port;
  const mark = wakeMarkOf(given.metadata) ?? undefined;
  const shownAs = (fallback: string | undefined) => (typeof given.words === "string" ? given.words : fallback);
  // A message's fields.
  const fields = typeof given.text !== "string" && given.kept !== true ? fieldsOf(given) : null;
  if (fields) {
    if (isEmpty(fields, files)) return yield* json(400, { message: "There's nothing to send." });
    let message: Composed;
    try {
      message = await composed(fields, files, session, target, toAgents, request, readEngine, signal, mark);
    } catch (error) {
      if (error instanceof NotListed) return yield* json(409, { message: error.message });
      throw error;
    }
    return yield* added({ json }, target, session, given, { id, text: message.text, files: message.files, metadata: message.metadata }, fields.words);
  }
  if (typeof given.text !== "string" || (!given.text.trim() && !files.length)) return yield* json(400, { message: "Send what was written (words)." });
  // Sent again as the agent read it.
  if (given.again === true) {
    const message = await sentAgain(given.text, files, session, target, toAgents, signal, mark);
    return yield* added({ json }, target, session, given, { id, text: message.text, files: message.files, metadata: message.metadata }, shownAs(undefined));
  }
  // Put back as it was kept, with its parts.
  if (given.kept === true) {
    const parts = keptParts(given.metadata);
    const metadata = parts ? withParts(mark, parts) : withPartsOf(mark, given.text);
    return yield* added({ json }, target, session, given, { id, text: given.text, files, metadata }, shownAs(undefined));
  }
  // COMPAT(0.7 own-envelope): one a client wrote the blocks of itself (`text`, no `again`). Goes in #717's next part.
  const delivery = given.delivery === "steer" ? "steer" : "queue";
  const base = new URL(`http://127.0.0.1:${target.port}/`);
  const headers = { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}` };
  const asGiven = Buffer.from(JSON.stringify({ id, text: given.text, ...(files.length ? { files } : {}), delivery, ...(mark ? { metadata: mark } : {}) }));
  // (A phone's also says who it names with "&", and that it's from the phone, as its prompt would;
  // the desktop app's over the link, only who it names.)
  const overTheLink = request.fromMac !== true && request.fromApp !== true;
  const attached = await attachedIn(asGiven, session, modelInput(base, headers, signal), request.fromMac === true, !toAgents);
  const named = overTheLink ? await namedIn(attached, session, namingHere(readEngine)) : attached;
  const body = fromAPhone(request) ? fromPhone(named) : named;
  const message = parsed(body) as { id: string; text: string; files?: unknown[]; delivery: "steer" | "queue"; metadata?: Record<string, unknown> };
  // Shown as written (`words`: without what the client added for the agent, like its files' notes).
  return yield* added({ json }, target, session, given, { id: message.id, text: message.text, files: message.files, metadata: withPartsOf(message.metadata, message.text) }, shownAs(given.text));
}

// What `add` adds, its text written: waiting on the box while the queue is paused, else to the
// engine, as `given` asked (its id, delivery, `from` and `before`); `words`, how it's shown.
async function* added({ json }: Pick<RouteContext, "json">, target: Target, session: string, given: Record<string, unknown>, written: { id: string; text: string; files?: unknown[]; metadata?: Record<string, unknown> }, words: string | undefined): AsyncGenerator<AnswerPiece> {
  const { id } = written;
  const delivery = given.delivery === "steer" ? "steer" : "queue";
  const from = typeof given.from === "string" && given.from.trim() ? given.from.trim().slice(0, 80) : undefined;
  const message = { id, text: written.text, ...(written.files?.length ? { files: written.files } : {}), delivery, ...(written.metadata ? { metadata: written.metadata } : {}) } as { id: string; text: string; files?: unknown[]; delivery: "steer" | "queue"; metadata?: Record<string, unknown> };
  const before = typeof given.before === "string" && /^[A-Za-z0-9_]{1,100}$/.test(given.before) ? given.before : undefined;
  try {
    const add = async (post = async () => { await asked(target, "POST", `api/session/${encodeURIComponent(session)}/prompt`, message, SENDING_PATIENCE.ms); }) => {
      if (await keepWaitingBefore(target, session, message, { from, words }, before)) return true;
      await post();
      return false;
    };
    const waits = await admitPrompt(session, () => given.kept === true ? undoRemoval(target, session, message, add) : add());
    return yield* json(200, { ...shownQueue(queueOf(session)), waits });
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    if (error instanceof MoveAdmissionClosed) return yield* json(409, { message: error.message });
    console.error("kuvo-link: could not add a waiting message", error);
    return yield* json(500, { message: "This computer couldn’t save the message. Try sending it again." });
  }
}
