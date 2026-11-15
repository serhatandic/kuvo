// A session's own menu, as every client offers it (PROTOCOL.md, "A session's actions"): Pin,
// Rename, Archive and Delete, done by the box on the engine or the coding agents' bridge, whichever
// keeps it, for every coding agent alike. The box writes the notes the lists go by (`kuvo.pinned`,
// `kuvo.archived` in its metadata, in its turn: `../engine/notes.ts`), does what goes with each (an
// archived one's wake-ups held and a run that waits on you stopped; a deleted one's wake-ups gone),
// says what Delete takes with it, and refuses one that's moving to another computer, or lives on
// another now, in words a client shows as they are.

import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { withNotes } from "../../shared/session-notes";
import { plainName } from "../../shared/text";
import { call } from "../engine/client";
import { inTurnOn, rewriteOn } from "../engine/notes";
import { isArchived, isPinned } from "../lists/rule";
import { listNow } from "../lists/sessions";
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
import { holdArchived } from "./wakes";
import { readRestorableWorktrees, readWorktrees } from "./worktree-records";
import type { EngineSession } from "../engine/sessions";
import type { Target } from "../../shared/records";
import type { SessionAction, SessionActions } from "../../shared/routes/sessions";
import type { AnswerPiece, RouteContext } from "../server/context";

/** `kuvo/sessions/<id>` (Delete), and its `actions`, `archive`, `pin` and `rename`. */
export const SESSION_ACTION_ROUTE = /^kuvo\/sessions\/(ses[^/]+)(?:\/(actions|archive|pin|rename))?$/;

const UNTITLED = "New Session";

/** A move took it while it waited for its turn. */
class Moving extends Error {}

/** Where a session that isn't here lives now, in words, and that computer's key: one moved away
 * from this box in the last two days. Null for one that never lived here. */
function livesElsewhere(session: string): { message: string; computer: string } | null {
  const moved = movedAway().find(each => each.session === session);
  if (!moved) return null;
  const peer = peersOf(readLink()).find(each => each.key === moved.key);
  const name = peer ? computerName(peer) : "";
  return { message: `It’s on ${plainName(name) || "another of your computers"} now. Change it there.`, computer: moved.key };
}

/** Archived by the engine itself (`time.archived`), which only the engine can undo. */
const engineArchived = (session: Partial<EngineSession>): boolean => Boolean(session.time?.archived);

/** What can be done with a session now, in the order a menu offers them. */
export function actionsOf(session: EngineSession, deletes: string): SessionAction[] {
  const title = session.title || UNTITLED;
  if (isArchived(session)) {
    return [
      // (Not one the engine archived itself: only it can bring that back.)
      ...(engineArchived(session) ? [] : [{ id: "unarchive" as const, label: "Unarchive" }]),
      { id: "delete", label: "Delete", destructive: true, confirm: { title: `Delete “${title}”?`, message: deletes, label: "Delete" } },
    ];
  }
  return [
    isPinned(session) ? { id: "unpin", label: "Unpin" } : { id: "pin", label: "Pin" },
    { id: "rename", label: "Rename" },
    { id: "archive", label: "Archive" },
    { id: "delete", label: "Delete", destructive: true, confirm: { title: `Delete “${title}”?`, message: deletes, label: "Delete" } },
  ];
}

/** What Delete says goes with a session, by the rule the box's tidying of worktrees goes by
 * (`tidyWorktrees`: only conversations count, never a subagent, a group chat's seat or a hidden run
 * such as a coding agent's engine run): its worktree where no other conversation is open in it,
 * and what isn't committed there unless an archived one left in it keeps that for Restore Worktree;
 * a removed worktree's kept changes, where it's the last one that could bring them back; else that
 * it can't be undone. Where the others can't be read, it says the most that may go. */
export async function deletesOf(session: EngineSession, context: Pick<RouteContext, "readEngine" | "bridge">, signal?: AbortSignal): Promise<string> {
  const plain = "This can’t be undone.";
  const directory = session.location?.directory ?? session.directory ?? "";
  const inside = (folder: string, root: string) => folder === root || folder.startsWith(root + path.sep);
  const tree = directory ? readWorktrees().find(each => inside(directory, each.root)) : undefined;
  const removed = directory && !tree ? readRestorableWorktrees().find(each => inside(directory, each.root)) : undefined;
  if (!tree && !removed) return plain;
  const root = (tree ?? removed)!.root;
  const list = await listNow(context, signal).catch(() => null);
  const others = Object.entries(list?.sessions ?? {}).filter(([id, entry]) => id !== session.id && !NOT_CONVERSATIONS.has(entry.facts.kind) && inside(entry.facts.directory, root));
  if (removed) return others.length || !removed.snapshot ? plain : "The changes kept to restore its worktree are deleted too.";
  if (others.some(([, entry]) => !entry.facts.archived)) return plain;
  if (others.length) return "Its worktree is deleted too.";
  // (Not known to be clean, it may have some.)
  const status = fs.existsSync(root) ? await gitLater(root, ["status", "--porcelain", "--untracked-files=normal"]) : null;
  return status === "" ? "Its worktree is deleted too." : "Its worktree and changes that weren’t committed are deleted too.";
}

/** What a worktree's tidying doesn't count as a conversation in it. */
const NOT_CONVERSATIONS = new Set(["subagent", "seat", "hidden"]);

/** A deleted session's own wake-ups and watches: gone with it (a bot's are its bot's). */
function forgetWakes(session: string): void {
  if (!/^ses_[A-Za-z0-9]+$/.test(session)) return;
  try { fs.rmSync(path.join(SESSION_WAKES, session), { recursive: true, force: true }); } catch {}
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
  const given = action === "actions" || action === null ? {} : parsed(request.body);
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
    if (action === null) return yield* json(200, { session: id });
    return yield* json(404, { message: "That conversation isn't here." });
  }
  const { target } = place;
  const session = place.data as EngineSession;

  if (action === "actions") {
    const answer: SessionActions = { session: id, actions: actionsOf(session, await deletesOf(session, context, signal)) };
    if (moving) answer.held = MOVING;
    return yield* json(200, answer);
  }
  if (action === null) {
    try {
      // (In its turn, so a move that took it meanwhile isn't undercut.)
      await inTurnOn(target, id, async () => {
        if (frozen().has(id)) throw new Moving();
        return call(target, "DELETE", `/api/session/${encodeURIComponent(id)}`, undefined, signal);
      }, signal);
    } catch (error) {
      if (error instanceof Moving) return yield* json(409, { message: MOVING });
      return yield* json(502, { message: `It couldn't be deleted: ${String((error as Error | null)?.message ?? error)}` });
    }
    forgetWakes(id);
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
      if ((action === "pin" ? isPinned(now) : isArchived(now)) === value) return null;
      // (Brought back: an older client's mark in its `kuvo` goes too.)
      const nested = action === "archive" && value === false && isObject(now.metadata?.kuvo) && "archived" in now.metadata.kuvo;
      return { metadata: withNotes(now.metadata, nested ? { archived: undefined } : null, { [key]: value }) };
    }, signal);
  } catch (error) {
    if (error instanceof Moving) return yield* json(409, { message: MOVING });
    return yield* json(502, { message: `It couldn't be changed: ${String((error as Error | null)?.message ?? error)}` });
  }
  if (!done) return yield* json(404, { message: "That conversation isn't here." });
  if (action === "archive" && done.written) {
    // Its wake-ups wait while it's archived, and come back with it; one archived while it waits on
    // you stops, rather than wait out of sight.
    try { holdArchived(id, value as boolean); } catch {}
    if (value === true) await stopIfWaiting(target, id);
  }
  return yield* json(200, { session: id, info: done.session });
}
