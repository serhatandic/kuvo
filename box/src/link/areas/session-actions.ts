// A session's own menu, as every client offers it (PROTOCOL.md, "A session's actions"): Pin,
// Rename, Archive and Delete, done by the box on the engine or the coding agents' bridge, whichever
// keeps it, for every coding agent alike. The box writes the notes the lists go by (`kuvo.pinned`,
// `kuvo.archived` in its metadata, in its turn: `../engine/notes.ts`), does what goes with each (an
// archived one's wake-ups held and a run that waits on you stopped; a deleted one's wake-ups gone),
// says what Delete takes with it, and refuses one that's moving to another computer, or lives on
// another now, in words a client shows as they are.

import fs from "node:fs";
import path from "node:path";
import { CHANGED_SINCE } from "./config";
import { isObject, parsed, sortedJSON } from "../../shared/json";
import { withNotes } from "../../shared/session-notes";
import { plainName } from "../../shared/text";
import { call } from "../engine/client";
import { inTurnOn, rewriteOn } from "../engine/notes";
import { isArchived, isPinned, kindOf } from "../lists/rule";
import { listNow, placeNow } from "../lists/sessions";
import { gitLater } from "../git";
import { SESSION_WAKES } from "../home";
import { MOVING } from "../bots/here";
import { placeOfSessionPatiently, isRefused } from "../letters/letter";
import { targetsOf } from "../letters/tools";
import { frozen, movedAway } from "../moves/records";
import { computerName } from "../peers/computer-name";
import { peersOf } from "../peers/known";
import { readLink } from "../state";
import { stopIfWaiting } from "../work/answers";
import { noteWorkChanged } from "../work/changes";
import { worksFor } from "../work/tree";
import { holdArchived } from "./wakes";
import { readRestorableWorktrees, readWorktrees } from "./worktree-records";
import type { EngineSession } from "../engine/sessions";
import type { Target } from "../../shared/records";
import type { SessionAction, SessionActions } from "../../shared/routes/sessions";
import type { AnswerPiece, RouteContext } from "../server/context";

/** What Delete All asks of several sessions at once: `POST kuvo/sessions/deletes {sessions}`. */
export const DELETES_ROUTE = "kuvo/sessions/deletes";

/** `POST kuvo/sessions/deletes {sessions: [<id>]}` → `{title, message, label}`: what Delete asks of
 * those sessions at once, in the box's words. Ids this box doesn't list are left out. */
export async function* deletesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, { message: "Ask with POST." });
  const given = parsed(request.body);
  const ids = isObject(given) && Array.isArray(given.sessions) ? [...new Set(given.sessions.filter((id): id is string => typeof id === "string"))] : null;
  if (!ids?.length) return yield* json(400, { message: "Say which sessions (sessions, their ids)." });
  const list = await listNow(context, signal).catch(() => null);
  if (!list) return yield* json(503, { message: "The engine isn't running." });
  // (A subagent's, a seat or a hidden run isn't deleted on its own: not counted.)
  const parts = ids.filter(id => NOT_CONVERSATIONS.has(list.sessions[id]?.facts.kind ?? ""));
  const deleting = ids.flatMap(id => {
    const facts = list.sessions[id]?.facts;
    return facts && !NOT_CONVERSATIONS.has(facts.kind) ? [{ id, directory: facts.directory, title: facts.title }] : [];
  });
  if (!deleting.length) return yield* json(404, { message: "None of them is here." });
  // (As many as asked about, those this box doesn't list too: a client's list may be older.)
  return yield* json(200, await deleteConfirm(deleting, context, signal, { count: ids.length - parts.length, list }));
}

/** `kuvo/sessions/<id>` (Delete), and its `actions`, `archive`, `pin` and `rename`. */
export const SESSION_ACTION_ROUTE = /^kuvo\/sessions\/(ses[^/]+)(?:\/(actions|archive|pin|rename))?$/;

const UNTITLED = "New Session";

/** A move took it while it waited for its turn. */
class Moving extends Error {}
class Stale extends Error {}

export function sessionMovedElsewhere(computer: string, name = "") {
  return { message: `It’s on ${plainName(name) || "another of your computers"} now. Change it there.`, computer };
}

/** Optional state comparison for settings actions, checked inside the session's existing turn. */
export const sessionActionState = (session: EngineSession, confirm?: unknown) => ({ updated: session.time.updated, archived: isArchived(session), ...(confirm === undefined ? {} : { confirm }) });
const checkWas = (given: Record<string, unknown>, session: EngineSession | null) => {
  if ("was" in given && (!session?.id || sortedJSON(isObject(given.was) ? { updated: given.was.updated, archived: given.was.archived } : given.was) !== sortedJSON(sessionActionState(session)))) throw new Stale();
};

/** Where a session that isn't here lives now, in words, and that computer's key: one moved away
 * from this box in the last two days. Null for one that never lived here. */
function livesElsewhere(session: string): { message: string; computer: string } | null {
  const moved = movedAway().find(each => each.session === session);
  if (!moved) return null;
  const peer = peersOf(readLink()).find(each => each.key === moved.key);
  const name = peer ? computerName(peer) : "";
  return sessionMovedElsewhere(moved.key, name);
}

/** Archived by the engine itself (`time.archived`), which only the engine can undo. */
const engineArchived = (session: Partial<EngineSession>): boolean => Boolean(session.time?.archived);

/** What can be done with a session now, in the order a menu offers them. */
export function actionsOf(session: EngineSession, confirm: { title: string; message: string; label: string }): SessionAction[] {
  if (isArchived(session)) {
    return [
      // (Not one the engine archived itself: only it can bring that back.)
      ...(engineArchived(session) ? [] : [{ id: "unarchive" as const, label: "Unarchive" }]),
      { id: "delete", label: "Delete", destructive: true, confirm },
    ];
  }
  return [
    isPinned(session) ? { id: "unpin", label: "Unpin" } : { id: "pin", label: "Pin" },
    { id: "rename", label: "Rename" },
    { id: "archive", label: "Archive" },
    { id: "delete", label: "Delete", destructive: true, confirm },
  ];
}

/** One session Delete is asked about: its id, its folder and its title. */
export interface Deleting { id: string; directory: string; title?: string }

/** What Delete asks of `deleting` (one session, or several at once, as Delete All has them), by the
 * rule the box's tidying of worktrees goes by (`tidyWorktrees`: only conversations count, never a
 * subagent, a group chat's seat or a hidden run such as a coding agent's engine run), for each
 * worktree they're in: the worktree goes where no other conversation is open in it, and with it
 * what isn't committed there unless an archived one left in it keeps that for Restore Worktree (not
 * known to be clean: git can't say, or something works there now and may still write); a removed
 * worktree's kept changes go with the last conversation that could bring them back. Where the
 * others can't be read, it says the most that may go. */
export async function deleteConfirm(deleting: Deleting[], context: Pick<RouteContext, "readEngine" | "bridge">, signal?: AbortSignal, { count = deleting.length, list: read }: { count?: number; list?: Awaited<ReturnType<typeof listNow>> } = {}): Promise<{ title: string; message: string; label: string }> {
  const inside = (folder: string, root: string) => folder === root || folder.startsWith(root + path.sep);
  const asked = new Set(deleting.map(each => each.id));
  const trees = readWorktrees(), removed = readRestorableWorktrees();
  let list: Awaited<ReturnType<typeof listNow>> = read ?? null;
  const listed = async () => (list ??= await listNow(context, signal).catch(() => null));
  let worktrees = 0, uncommitted = false, kept = 0;
  const roots = new Set<string>();
  for (const one of deleting) {
    const live = one.directory ? trees.find(each => inside(one.directory, each.root)) : undefined;
    const gone = one.directory && !live ? removed.find(each => inside(one.directory, each.root)) : undefined;
    const tree = live ?? gone;
    if (!tree || roots.has(tree.root)) continue;
    roots.add(tree.root);
    const there = Object.entries((await listed())?.sessions ?? {}).filter(([, entry]) => !NOT_CONVERSATIONS.has(entry.facts.kind) && inside(entry.facts.directory, tree.root));
    const others = there.filter(([id]) => !asked.has(id));
    if (gone) {
      if (!others.length && gone.snapshot) kept += 1;
      continue;
    }
    if (others.some(([, entry]) => !entry.facts.archived)) continue;
    worktrees += 1;
    if (others.length) continue;
    // (Not known to be clean, it may have some; at work there, it may write some yet.)
    const status = fs.existsSync(tree.root) ? await gitLater(tree.root, ["status", "--porcelain", "--untracked-files=normal"]) : null;
    if (status !== "" || there.some(([, entry]) => entry.facts.running)) uncommitted = true;
  }
  if (count === 1 && deleting.length === 1) {
    const message = uncommitted ? "Its worktree and changes that weren’t committed are deleted too."
      : worktrees ? "Its worktree is deleted too."
      : kept ? "The changes kept to restore its worktree are deleted too."
      : "This can’t be undone.";
    return { title: `Delete “${deleting[0]!.title || UNTITLED}”?`, message, label: "Delete" };
  }
  const also = [
    worktrees ? (worktrees === 1 ? "The worktree they were in is deleted too" : "The worktrees they were in are deleted too") + (uncommitted ? ", with changes that weren’t committed." : ".") : "",
    kept ? `The changes kept to restore ${kept === 1 ? "their worktree" : "their worktrees"} go too.` : "",
  ].filter(Boolean);
  return { title: count === 1 ? `Delete “${deleting[0]?.title || UNTITLED}”?` : `Delete ${count} sessions?`, message: ["Their messages and undo snapshots go too.", ...also, "This can’t be undone."].join(" "), label: "Delete" };
}

/** What a worktree's tidying doesn't count as a conversation in it. */
const NOT_CONVERSATIONS = new Set(["subagent", "seat", "hidden"]);

/** A deleted session's own wake-ups and watches: gone with it (a bot's are its bot's). */
async function forgetWakes(session: string): Promise<void> {
  if (!/^ses_[A-Za-z0-9]+$/.test(session)) return;
  try { await fs.promises.rm(path.join(SESSION_WAKES, session), { recursive: true, force: true }); } catch {}
}

export async function* sessionActionRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  const method = request.method.toUpperCase();
  const action = match[2] ?? null;
  let id = "";
  try { id = decodeURIComponent(match[1]!); } catch {}
  if (!/^ses[A-Za-z0-9_-]+$/.test(id)) return yield* json(400, { message: "That isn't a conversation's id." });
  if (action === null && method !== "DELETE") return yield* json(405, { message: "Delete it with DELETE." });
  if (action === "actions" && method !== "GET") return yield* json(405, { message: "Ask with GET." });
  if (action !== null && action !== "actions" && method !== "POST") return yield* json(405, { message: "Change it with POST." });
  const decoded = parsed(request.body);
  const given = action === "actions" ? {} : action === null ? (isObject(decoded) ? decoded : {}) : decoded;
  if (!isObject(given)) return yield* json(400, { message: "Send what changes as JSON." });
  if (action === "archive" && typeof given.archived !== "boolean") return yield* json(400, { message: "Say whether it's archived: {\"archived\": true} or false." });
  if (action === "pin" && typeof given.pinned !== "boolean") return yield* json(400, { message: "Say whether it's pinned: {\"pinned\": true} or false." });
  const title = action === "rename" && typeof given.title === "string" ? given.title.trim() : "";
  if (action === "rename" && !title) return yield* json(400, { message: "Give it a name." });

  // Moving to or from another computer: nothing changes it until it's there.
  const moving = frozen().has(id);
  if (moving && action !== "actions") return yield* json(409, { message: MOVING });
  const targets = targetsOf(context);
  if (!targets.length) return yield* json(503, { message: "The engine isn't running." });
  let place: Awaited<ReturnType<typeof placeOfSessionPatiently>>;
  try {
    place = await placeOfSessionPatiently(id, targets);
  } catch (error) {
    return yield* json(503, { message: isRefused(error) ? "The engine isn't running." : "The engine isn't answering. Try again in a moment." });
  }
  if (!place) {
    const elsewhere = livesElsewhere(id);
    if (elsewhere) return yield* json(409, elsewhere);
    // The coding agents' bridge starting again: whether it's one of theirs can't be told yet.
    let record: { port?: number } | null = null;
    try { record = context.readBridge(); } catch {}
    if (record?.port && !context.bridge()) return yield* json(503, { message: "The coding agents on this computer are starting. Try again in a moment." });
    // Deleted already (from another client while this one wasn't listening): done.
    if (action === null) return yield* ("was" in given ? json(409, { message: CHANGED_SINCE }) : json(200, { session: id }));
    return yield* json(404, { message: "That conversation isn't here." });
  }
  const { target } = place;
  const session = place.data as EngineSession;
  // Part of another conversation: it goes, and changes, with that one.
  const kind = kindOf(session as EngineSession & { id: string }, placeNow(new Set(), [session]));
  if (kind === "subagent") return yield* json(409, { message: "It’s a subagent’s, part of the conversation that started it: it goes with that one." });
  if (kind === "seat") return yield* json(409, { message: "It’s a bot’s seat in a group chat: change the group chat instead." });
  if (kind === "hidden") return yield* json(409, { message: "It’s part of another conversation’s work: it goes with that one." });

  if (action === "actions") {
    const deleting = { id, directory: session.location?.directory ?? session.directory ?? "", title: session.title };
    const answer: SessionActions = { session: id, actions: actionsOf(session, await deleteConfirm([deleting], context, signal)) };
    if (moving) answer.held = MOVING;
    return yield* json(200, answer);
  }
  if (action === null) {
    try {
      // (In its turn, so a move that took it meanwhile isn't undercut.)
      await inTurnOn(target, id, async () => {
        if (frozen().has(id)) throw new Moving();
        if ("was" in given) {
          let now: EngineSession | null;
          try { now = await call(target, "GET", `/api/session/${encodeURIComponent(id)}`, undefined, signal) as EngineSession; }
          catch (error) { if ((error as { status?: number }).status === 404) throw new Stale(); throw error; }
          if (!now?.id) throw new Stale();
          const confirm = await deleteConfirm([{ id, directory: now.location?.directory ?? now.directory ?? "", title: now.title }], context, signal);
          if (sortedJSON(given.was) !== sortedJSON(sessionActionState(now, confirm))) throw new Stale();
        }
        try { return await call(target, "DELETE", `/api/session/${encodeURIComponent(id)}`, undefined, signal); }
        catch (error) {
          if ((error as { status?: number }).status !== 404) throw error;
          if ("was" in given) throw new Stale();
          return undefined;
        }
      }, signal);
    } catch (error) {
      if (error instanceof Stale) return yield* json(409, { message: CHANGED_SINCE });
      if (error instanceof Moving) return yield* json(409, { message: MOVING });
      return yield* json(502, { message: `It couldn't be deleted: ${String((error as Error | null)?.message ?? error)}` });
    }
    await forgetWakes(id);
    return yield* json(200, { session: id });
  }

  if (action === "rename") {
    // (Its name only: in its turn all the same, so none of the box's writes of its notes is between.)
    let renamed: unknown;
    try {
      renamed = session.title === title ? session : await inTurnOn(target, id, async () => {
        if (frozen().has(id)) throw new Moving();
        return call(target, "PATCH", `/api/session/${encodeURIComponent(id)}`, { title }, signal);
      }, signal);
    } catch (error) {
      if (error instanceof Moving) return yield* json(409, { message: MOVING });
      return yield* json(502, { message: `It couldn't be renamed: ${String((error as Error | null)?.message ?? error)}` });
    }
    return yield* json(200, { session: id, info: isObject(renamed) ? renamed : { ...session, title } });
  }

  if (action === "archive" && given.archived === false && engineArchived(session)) {
    return yield* json(409, { message: "It was archived by the engine, which keeps it so. Start a new session to carry on." });
  }

  // Pin and Archive: written in its turn, onto its notes as they are then (and not once a move
  // took it meanwhile).
  const key = action === "pin" ? "kuvo.pinned" : "kuvo.archived";
  const value = action === "pin" ? given.pinned : given.archived;
  let done: Awaited<ReturnType<typeof rewriteOn>>;
  try {
    done = await rewriteOn(target, id, now => {
      if (frozen().has(id)) throw new Moving();
      checkWas(given, now);
      if ((action === "pin" ? isPinned(now) : isArchived(now)) === value) return null;
      // (Brought back: an older client's mark in its `kuvo` goes too.)
      const nested = action === "archive" && value === false && isObject(now.metadata?.kuvo) && "archived" in now.metadata.kuvo;
      return { metadata: withNotes(now.metadata, nested ? { archived: undefined } : null, { [key]: value }) };
    }, signal);
  } catch (error) {
    if (error instanceof Stale) return yield* json(409, { message: CHANGED_SINCE });
    if (error instanceof Moving) return yield* json(409, { message: MOVING });
    return yield* json(502, { message: `It couldn't be changed: ${String((error as Error | null)?.message ?? error)}` });
  }
  if (!done) return yield* json(404, { message: "That conversation isn't here." });
  if (action === "archive" && done.written) {
    // Its wake-ups wait while it's archived, and come back with it; one archived while it waits on
    // you stops, rather than wait out of sight.
    try { holdArchived(id, value as boolean); } catch {}
    if (value === true) await stopIfWaiting(target, id);
    const bot = worksFor(done.session)?.bot;
    if (bot) noteWorkChanged(bot);
  }
  return yield* json(200, { session: id, info: done.session });
}
