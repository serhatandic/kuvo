import { acquireSessionEngine } from "../engine/lifetime";
// `kuvo/sessions/<id>/draft` and `kuvo/sessions/<id>/submit` (PROTOCOL.md, "What a typed message
// means"): what's written in a conversation's composer, sent once under its id. What's written goes
// to the agent as a message, as written (one starting with "/" or "!" too: Kuvo has no typed
// keywords or commands); where the conversation's model can't run, it goes back, saying why.
// `kuvo/sessions/<id>/typed` stays for installed 0.7.x clients: `POST` answers the same decision
// (the client then sends the message itself), `GET` answers that nothing is offered after "/".

import { draftKey } from "../ownership/forward-evidence";
import fs from "node:fs";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { RUN } from "../home";
import type { Target } from "../../shared/records";
import { isRefused, placeOfSession, placeOfSessionPatiently } from "../letters/letter";
import { messageReached } from "../engine/queue";
import { targetsOf } from "../letters/tools";
import { answering, BUSY, collected, sentOnce } from "../server/sent-once";
import { modelOf } from "./session-models";
import { queuedRefusal, type AnswerPiece, type RouteContext } from "../server/context";
import { mutationSubmitter, submitterOf, SUBMITTER_REVOKED, type Submitter } from "../ownership/authority";
import { hasMutation, MUTATION_ID, runMutation } from "../ownership/mutation";
import { ownershipReady } from "../ownership/store";
import { readSubmissionID, submissionIntent, submissionKey, submissionOwner, SUBMIT_ROUTE } from "../ownership/submissions";
import { ownKey } from "../peers/known";
import { followedSetup } from "../setup/record";
import { readLink } from "../state";
import { SEND_ROUTE, sendRoute } from "./messages";
import type { TypedAnswer } from "../../shared/routes/typed";
import { acceptsMutation } from "../ownership/admission";

export const DRAFT_ROUTE = /^kuvo\/sessions\/(ses_[A-Za-z0-9_-]{1,95})\/draft$/;
export const TYPED_ROUTE = /^kuvo\/sessions\/([^/]+)\/typed$/;

/** One of the box's routes asked from here, as a client would: its status and JSON. */
async function routeAnswer(pieces: AsyncIterable<AnswerPiece>): Promise<{ status: number; value: Record<string, unknown>; notAdmitted?: true }> {
  const answer = await collected(pieces);
  const value = parsed(answer.body);
  return { status: answer.status, value: isObject(value) ? value : {}, notAdmitted: answer.notAdmitted };
}

class DraftNotAdmitted {
  constructor(readonly answer: { status: number; value: unknown; notAdmitted: true }) {}
}

// What a send with its own id was answered, kept on disk an hour (the most recent 1000), so the same
// send again after this box started again is answered as it was.
export const TYPED_SENT = path.join(RUN, "typed-sent.json");
const sentFormat = additiveObjectFormat("Typed send receipts");
const KEPT_MS = 60 * 60_000;
const KEPT_MOST = 1000;
const KEPT_BYTES = 64 * 1024;
type KeptAnswer = { at: number; status: number; headers: Record<string, string>; body: string };

function readKept(): Record<string, KeptAnswer> {
  try {
    const read = JSON.parse(fs.readFileSync(TYPED_SENT, "utf8"));
    return isObject(read) ? sentFormat.read(read) as Record<string, KeptAnswer> : {};
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return {};
  }
}

function keptAnswer(key: string): { status: number; headers: Record<string, string>; body: Buffer } | null {
  const kept = readKept()[key];
  // (One not as written here is none.)
  if (!isObject(kept) || typeof kept.at !== "number" || typeof kept.status !== "number" || typeof kept.body !== "string" || !isObject(kept.headers)) return null;
  if (Date.now() - kept.at > KEPT_MS) return null;
  return { status: kept.status, headers: kept.headers, body: Buffer.from(kept.body, "base64") };
}

function keepAnswer(key: string, answer: { status: number; headers: Record<string, string>; body: Buffer }) {
  // (A big one, a long paste sent back to be sent, isn't kept: it's read whole on every send.)
  if (answer.body.length > KEPT_BYTES) return;
  const now = Date.now();
  const all = Object.entries(readKept()).filter(([, kept]) => now - kept.at <= KEPT_MS);
  all.push([key, { at: now, status: answer.status, headers: answer.headers, body: answer.body.toString("base64") }]);
  const kept = Object.fromEntries(all.slice(-KEPT_MOST));
  // (Two boxes at once, as one is updated, can each write over the other's latest: rare, and then
  // that send is only done again if it's resent after a restart.)
  const temporary = `${TYPED_SENT}.${process.pid}`;
  try {
    fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
    fs.writeFileSync(temporary, JSON.stringify(sentFormat.write(kept)), { mode: 0o600 });
    fs.renameSync(temporary, TYPED_SENT);
  } catch {
    fs.rmSync(temporary, { force: true });
  }
}

// `kuvo/sessions/<id>/typed`. A send with its own id (in its body, or the link's
// `idempotency-key`, with which a phone's link sends it again after its session drops) is answered
// once (`sentOnce`, as a message sent again is): the same send again gets the first's answer; one
// that didn't go (refused, the engine not there) is tried again.
export async function* typedRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request } = context;
  const given = parsed(request.body);
  const header = request.headers["idempotency-key"];
  const named = isObject(given) && typeof given.id === "string" && given.id.trim() ? given.id : typeof header === "string" ? header : "";
  const id = request.method.toUpperCase() === "POST" && named.trim() ? named.trim().slice(0, 200) : null;
  if (!id) return yield* typedAnswer(context, match);
  const key = `typed\n${match[1]}\n${id}`;
  // Answered before this box last started: as it was.
  const kept = keptAnswer(key);
  if (kept) return yield* answering(kept);
  // Done whatever becomes of this request: its link dropping (which ends its signal) is what the
  // same send again is for, and it gets this answer (as a prompt sent with its id is, forward.ts).
  const answer = await sentOnce(key, async () => {
    const answer = await collected(typedAnswer({ ...context, signal: undefined }, match));
    // Kept on disk once it went, before it's answered: a box started again meanwhile has it.
    if (answer.status >= 200 && answer.status < 300) keepAnswer(key, answer);
    return answer;
  });
  return yield* answering(answer);
}

/** Versioned full-draft admission. Legacy /typed remains a decision-only route. */

export function draftClaimedHere(session: string, given: unknown, scopedRoute: boolean): boolean {
  if (!isObject(given) || typeof given.id !== "string" || !MUTATION_ID.test(given.id)) return false;
  const setup = followedSetup(readLink())?.id ?? "";
  const scoped = scopedRoute && readSubmissionID(given.submission);
  return hasMutation(scoped ? submissionKey(setup, session, scoped.submission) : draftKey(setup, session, given.id));
}

export async function* draftRoute(context: RouteContext, match: RegExpMatchArray, forwarded?: Submitter, from?: string): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  if (!ownershipReady()) return yield* json(503, { message: "Wait for the Box update to finish." });
  const given = parsed(request.body);
  const session = match[1]!;
  if (!isObject(given) || typeof given.text !== "string" || typeof given.id !== "string" || !MUTATION_ID.test(given.id)
    || !isObject(given.message) || (request.headers["idempotency-key"] !== undefined && request.headers["idempotency-key"] !== given.id))
    return yield* json(400, { message: "Send a draft with its stable id, text and message fields." });
  const link = readLink();
  const setup = followedSetup(link)?.id ?? "";
  const submitter = forwarded ?? submitterOf(context, link, true);
  if (!submitter) return yield* json(403, { message: "Open the conversation as a paired Kuvo Code client." });
  const hasScope = SUBMIT_ROUTE.test(context.route);
  const scoped = hasScope ? readSubmissionID(given.submission) : undefined;
  const own = link && ownKey(link);
  if (hasScope && (!scoped || scoped.id !== given.id || !own || !setup))
    return yield* json(400, { message: "Use the original owner and stable submission ID." });
  if (scoped && (scoped.setup !== setup || scoped.origin !== own))
    return yield* json(409, { ambiguous: true, message: "Look up that submission on its original computer." });
  const admit: NonNullable<RouteContext["admitQueued"]> = context.admitQueued ?? (() => {
    if (context.signal?.aborted) return { status: 409, value: { message: "The live draft request closed before admission." }, notAdmitted: true };
    const current = readLink();
    if ((followedSetup(current)?.id ?? "") !== setup || !acceptsMutation("queuedSessionWork", { kind: "conversations", id: session }, current) ||
        (scoped && submissionOwner(current, session) !== scoped.origin))
      return { status: 409, value: { message: "The conversation's owner changed. Check the conversation before sending again." } };
    const authority = mutationSubmitter("queuedSessionWork", current, submitter, undefined, true);
    if (authority === "checking") return { status: 503, value: { message: "Checking the submitting device's access." }, notAdmitted: true };
    if (authority !== "allowed") return { status: 403, value: { message: SUBMITTER_REVOKED } };
    return null;
  });
  const key = scoped ? submissionKey(setup, session, scoped.submission) : draftKey(setup, session, given.id);
  const uncertain = () => ({ status: 409, value: { ambiguous: true,
    message: "The Box could not confirm the draft's outcome. Check the conversation before sending again." } });
  const outcome = await runMutation(key, scoped ? submissionIntent(given, submitter, from ?? own!) : { given, submitter }, async () => {
    try {
      const answer = await collected(typedAnswer({ ...context, admitQueued: admit }, match, given));
      return { status: answer.status, value: parsed(answer.body) ?? {}, notAdmitted: answer.notAdmitted };
    } catch (error) {
      if (error instanceof DraftNotAdmitted) return error.answer;
      throw error;
    }
  }, admit, uncertain, scoped && { setup, session, id: scoped.submission, origin: scoped.origin,
    submittedThrough: from ?? own!, submitter, draft: given }, scoped ? draftKey(setup, session, given.id) : undefined,
    undefined, scoped ? undefined : () => orphanedDraft(context, session, given))
    .catch(() => ({ ...uncertain(), notAdmitted: undefined }));
  const acceptedAt = "acceptedAt" in outcome && typeof outcome.acceptedAt === "number" ? outcome.acceptedAt : undefined;
  const value = isObject(outcome.value) && typeof outcome.value.message === "string" && outcome.status === 200 && acceptedAt
    ? { ...outcome.value, acceptedAt } : outcome.value;
  return yield* (queuedRefusal(outcome.notAdmitted ? outcome : null, json) ?? json(outcome.status, value));
}

/** A message whose send an earlier link left unanswered (it stopped partway: an update, a restart),
 * asked again under its id: sent, where it reached its conversation (its answer as then); sent now,
 * where nothing of it did (the agent takes one id once). */
async function orphanedDraft(context: RouteContext, session: string, given: Record<string, unknown>): Promise<{ status: number; value: unknown } | "again" | null> {
  const text = given.text as string;
  const place = await placeOfSession(session, targetsOf(context)).catch(() => null);
  if (!place) return null;
  return await messageReached(place.target, session, given.id as string)
    ? { status: 200, value: { answer: { send: { text, quoted: true } }, message: given.id } }
    : "again";
}

/** A draft's decision carried out: its message sent with the composer's files, quotes and the rest
 * (`kuvo/sessions/<id>/send`), or what was written put back, saying why. */
async function finishDraft(context: RouteContext, session: string, given: Record<string, unknown>, answer: TypedAnswer): Promise<Record<string, unknown>> {
  if (!answer.send) return { answer };
  const veto = context.admitQueued?.();
  if (veto?.notAdmitted) throw new DraftNotAdmitted({ ...veto, notAdmitted: true });
  const input = given.message as Record<string, unknown>;
  const message = {
    id: given.id, words: answer.send.quoted && typeof given.quotedText === "string" ? given.quotedText : answer.send.text,
    files: input.files, notes: input.notes, delivery: context.deliveryMode ?? input.delivery, comments: input.comments, named: input.named, widget: input.widget,
  };
  const route = `kuvo/sessions/${encodeURIComponent(session)}/send`;
  const headers = { ...context.request.headers };
  delete headers["idempotency-key"];
  const result = veto ? { status: veto.status, value: isObject(veto.value) ? veto.value : {} }
    : await routeAnswer(sendRoute({ ...context, route, request: { ...context.request, headers, path: route, method: "POST", body: Buffer.from(JSON.stringify(message)) } }, route.match(SEND_ROUTE)!));
  if ("notAdmitted" in result && result.notAdmitted) throw new DraftNotAdmitted({ ...result, notAdmitted: true });
  if (result.status < 300) return { answer, message: given.id };
  // Keep the owner claim open if the runtime might already have accepted this ID. Its next
  // attempt reconciles the inbox or transcript before deciding whether to send again.
  if (context.durableDelivery && result.status >= 500) throw new Error("The runtime’s acceptance is still being checked.");
  // Not sent: what was written goes back, unless the box can't tell whether it went.
  return { answer: { ...(result.status >= 500 ? { ambiguous: true } : { restore: { text: answer.send.text } }),
    error: typeof result.value.message === "string" ? result.value.message : "Check the conversation to see whether the draft went before sending again." } };
}

async function* typedAnswer(context: RouteContext, match: RegExpMatchArray, draft?: Record<string, unknown>): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, json } = context;
  const method = request.method.toUpperCase();
  if (method !== "GET" && method !== "POST") return yield* json(405, { message: "Send what was typed with POST." });
  let session = "";
  try { session = decodeURIComponent(match[1]); } catch {}
  if (!session) return yield* json(404, { message: "There's no such conversation here." });
  // (For 0.7.x clients, which ask what "/" offers: nothing.)
  if (method === "GET") return yield* json(200, { keywords: [], commands: [] });
  const given = parsed(request.body);
  const body = isObject(given) ? given : {};
  if (typeof body.text !== "string") return yield* json(400, { message: "Send what was typed (text)." });
  const text = body.text;
  // A send it can't answer for: what was written goes back, saying why.
  const unavailable = (status: number, message: string) => {
    const value = { message, restore: { text } };
    return draft && request.fromOwnerPeer
      ? queuedRefusal({ status, value, notAdmitted: true }, json)!
      : json(status, value);
  };
  const release = await acquireSessionEngine(session);
  try {
    const targets = targetsOf({ readEngine, bridge });
    if (!targets.length) return yield* unavailable(503, "The engine isn't running.");
    // (Patiently, as a message's send is: a busy computer's engine can take a while to answer.)
    let place: Awaited<ReturnType<typeof placeOfSessionPatiently>>;
    try {
      place = await placeOfSessionPatiently(session, targets);
    } catch (error) {
      return yield* unavailable(503, isRefused(error) ? "The engine isn't running." : BUSY);
    }
    if (!place) return yield* unavailable(404, "There's no such conversation here.");
    let engine: Target | null = null;
    try { engine = readEngine(); } catch {}
    if (!engine?.port) engine = null;
    // Whether its model can run, as this box says it (`kuvo/sessions/models`). One that can't be said
    // now (a coding agent's own subagent's) is let go, as a client lets it go.
    const model = await modelOf(place, engine, bridge()).catch(() => undefined);
    const veto = queuedRefusal(context.admitQueued?.() ?? null, json);
    if (veto) return yield* veto;
    // Refused for its model: all of it kept, with what to offer doing about it.
    const answer: TypedAnswer = model?.refusal
      ? { restore: { text }, error: model.refusal, ...(model.action ? { action: model.action } : {}) }
      : { send: { text, quoted: true } };
    return yield* json(200, draft ? await finishDraft(context, session, draft, answer) : answer);
  } finally {
    release();
  }
}
