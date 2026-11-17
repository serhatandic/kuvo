// `kuvo/moves`: Move to Computer, as a client asks this box for it (PROTOCOL.md, `kuvo/moves`):
// where a bot, a chat or a group chat can go, a move (answered once it's there, or why not), and the
// moves under way here with the conversations that don't live here, and on your other computers.

import { isObject } from "../../shared/json";
import type { Move, MoveKind, MovePlaces, MoveReceipt, Moves } from "../../shared/routes/moves";
import { plainName } from "../../shared/text";
import { followed } from "../follow";
import { beginMove, placesFor, readOutcome, startMove } from "../moves/move";
import { movesElsewhere } from "../moves/elsewhere";
import { awaySessions, listedMoves, moveFollowers, movedAway, readMove, shownMove } from "../moves/records";
import { peersOf } from "../peers/known";
import type { AnswerPiece, RouteContext } from "../server/context";
import { readLink } from "../state";

const KINDS: MoveKind[] = ["bot", "chat", "group"];

// A move's progress and the other computer's name, for clients to show as received.
function withWords(move: Move, peerName?: string, destination?: string): Move {
  const place = plainName(peerName ?? move.place ?? "") || "the other computer";
  const to = move.role === "from" ? place : destination ?? "this computer";
  const progress = move.waiting ? `Waiting for ${place}…`
    : move.stage === "holding" ? `Getting ready to move to ${to}…`
    : move.stage === "copying" ? `Moving to ${to}…` : `Finishing the move to ${to}…`;
  return { ...move, place, progress };
}

const movesNow = (): Moves => {
  const peers = peersOf(readLink());
  const nameOf = (key: string) => peers.find(peer => peer.key === key)?.name;
  const elsewhere = movesElsewhere(peers);
  const moved = movedAway().map(({ session, key, at }) => ({ session, key, at }));
  return {
    moves: listedMoves().map(record => withWords(shownMove(record), nameOf(record.peer))),
    away: awaySessions(),
    ...(elsewhere.length ? { elsewhere: elsewhere.map(heard => ({ ...heard, moves: heard.moves.map(move => {
      const other = move.role === "from" ? move.to : move.from;
      return withWords(move, nameOf(other), plainName(nameOf(move.to) ?? "") || "the other computer");
    }) })) } : {}),
    ...(moved.length ? { moved } : {}),
  };
};

export async function* moveReceiptRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const id = new URLSearchParams(request.path.split("?")[1] ?? "").get("id") ?? "";
  const answer = readOutcome(id);
  if (answer && !answer.ok) return yield* json(200, { state: "failed", message: answer.message } satisfies MoveReceipt);
  const record = answer ? null : readMove(id);
  const move = answer?.move ?? (record ? shownMove(record) : null);
  if (!move || move.role !== "from") {
    return yield* json(404, { message: "The move’s result isn’t available. Check where it is before trying again." });
  }
  const place = peersOf(readLink()).find(peer => peer.key === move.to)?.name;
  const shown = withWords(move, place);
  if (!answer) return yield* json(200, { state: "moving", move: shown } satisfies MoveReceipt);
  return yield* json(200, { state: "done", move: shown, message: `Moved to ${shown.place}.`, ...(answer.note ? { note: answer.note } : {}) } satisfies MoveReceipt);
}

export async function* movesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  const method = request.method.toUpperCase();
  if (method === "GET") return yield* json(200, movesNow());
  if (method !== "POST") return yield* json(405, {});
  let given: unknown;
  try { given = JSON.parse(Buffer.from(request.body ?? []).toString("utf8") || "{}"); } catch { given = null; }
  if (!isObject(given) || !KINDS.includes(given.kind as MoveKind) || typeof given.id !== "string" || typeof given.to !== "string") {
    return yield* json(400, { message: `Say what moves where: {"kind": "bot" | "chat" | "group", "id": …, "to": <its computer's key>}.` });
  }
  if (given.receipt === true) {
    const begun = await beginMove(given.kind as MoveKind, given.id, given.to);
    if (!begun.ok) return yield* json(begun.status, { message: begun.message });
    const place = peersOf(readLink()).find(peer => peer.key === begun.move.to)?.name;
    return yield* json(202, { move: withWords(begun.move, place) });
  }
  // It goes on if the client goes: the box it leaves sees it through.
  const outcome = await startMove(given.kind as MoveKind, given.id, given.to);
  if (signal?.aborted) return;
  return yield* (outcome.ok ? json(200, { move: outcome.move, ...(outcome.note ? { note: outcome.note } : {}) }) : json(outcome.status, { message: outcome.message }));
}

export async function* moveEventsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  return yield* followed(movesNow, moveFollowers, signal);
}

export async function* movePlacesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const query = new URLSearchParams(request.path.split("?")[1] ?? "");
  const kind = query.get("kind") as MoveKind, id = query.get("id");
  if (!KINDS.includes(kind) || !id) return yield* json(400, { message: "Say what: ?kind=bot|chat|group&id=…" });
  const places = await placesFor(kind, id, signal);
  if (!places) return yield* json(404, { message: "It isn’t on this computer any more." });
  return yield* json(200, places satisfies MovePlaces);
}
