import { draftKey } from "../ownership/forward-evidence";
// `kuvo/sessions/<id>/typed` (PROTOCOL.md, "What a typed message means"): what a conversation's
// composer offers after "/" (`GET`: its keywords, and the engine's commands as "/" lists them), and
// what a typed text means once it's sent (`POST`): the box decides (typed/decide.ts), does what's
// its own to do (a goal set, paused, resumed or taken away; a bot asked for one; Autopilot on; a
// /loop wake-up; a side question, handoff or review started), and answers with what it did and what
// the client does with the rest: send a message, run a command or a "!" command, switch Plan or
// Autopilot, compact, ask the side thread, or put what was written back in the composer, saying why.
// The client sends the files and quotes it has with what it's told to send, and keeps the rest.

import fs from "node:fs";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { RUN } from "../home";
import type { Target } from "../../shared/records";
import type { SessionAutopilot } from "../../shared/routes/autopilot";
import { call } from "../engine/client";
import type { EngineSession } from "../engine/sessions";
import { isRefused, placeOfSessionPatiently } from "../letters/letter";
import { targetsOf } from "../letters/tools";
import { answering, BUSY, collected, sentOnce } from "../server/sent-once";
import { askGoal } from "../work/goals";
import { isAutopilot } from "../work/autopilot";
import { listsHere, offeredHere } from "../work/model-rule";
import { botOfConversation } from "../work/tree";
import { zone } from "../work/clock";
import { decide, needsModel, writtenBack, GOALS_NOT_RUNNING, KEYWORD_UNAVAILABLE, type BoxStep, type Decision, type Restore } from "../typed/decide";
import { KEYWORD_WORDS, KEYWORDS, commandsOffered, splitCommand, parseGoal, parseKeyword, type Command, type Keyword, type ModelRef, type Reader } from "../typed/words";
import { autopilotRoute } from "./autopilot";
import { answerWakes } from "./wakes";
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
import { agentsRoute, engineRoute } from "../server/forward";
import type { TypedAnswer } from "../../shared/routes/typed";
import { acceptsMutation } from "../ownership/admission";

export const DRAFT_ROUTE = /^kuvo\/sessions\/(ses_[A-Za-z0-9_-]{1,95})\/draft$/;
export const TYPED_ROUTE = /^kuvo\/sessions\/([^/]+)\/typed$/;

/** The keywords a conversation offers, as this box runs them: none in a subagent's (its work is the
 * conversation it works for's); goals, side questions, handoffs and reviews while the agents bridge
 * runs; the rest always. */
export function keywordsOffered(session: Pick<EngineSession, "parentID">, bridge: boolean): Keyword[] {
  if (session.parentID) return [];
  return KEYWORDS.filter(keyword => (keyword === "goal" || keyword === "review" || keyword === "btw" || keyword === "handoff" ? bridge : true));
}

/** The engine's commands in `folder`, as it lists them; null when it can't be asked. */
async function engineCommands(engine: Target | null, folder: string | undefined, signal?: AbortSignal): Promise<Command[] | null> {
  if (!engine) return [];
  try {
    const listed = await call(engine, "GET", "/api/command", undefined, within(signal, 10_000), folder);
    return Array.isArray(listed) ? listed.flatMap(each => (isObject(each) && typeof each.name === "string" ? [{ name: each.name, ...(typeof each.description === "string" ? { description: each.description } : {}) }] : [])) : [];
  } catch {
    return null;
  }
}

/** The mode "/plan" alone goes back to from Plan: Build, or the first mode there is where Build is
 * turned off (as a new conversation starts in, where none is chosen). */
export function startingAgent(agents: unknown): string {
  // (Never Plan itself: it's what "/plan" leaves.)
  const modes = Array.isArray(agents) ? agents.flatMap(each => (isObject(each) && typeof each.id === "string" && each.id !== "plan" && each.mode !== "subagent" && each.hidden !== true ? [each.id] : [])) : null;
  if (!modes) return "build";
  return modes.length === 0 || modes.includes("build") ? "build" : modes[0];
}

/** The mode to switch to for "/plan": Plan, or, alone in Plan, back to where it starts. */
async function planAgent(plan: "toggle" | "on", data: Partial<EngineSession>, engine: Target | null, folder: string | undefined, signal?: AbortSignal): Promise<string> {
  if (plan === "on" || data.agent !== "plan") return "plan";
  const agents = engine ? await call(engine, "GET", "/api/agent", undefined, within(signal, 10_000), folder).catch(() => null) : null;
  return startingAgent(agents);
}

/** What "/plan" does in each conversation, one after another: each reads the mode as the one before
 * left it (two at once are two switches, the Mac's Plan Mode item and ⌥⌘2 among them). */
const planning = new Map<string, Promise<unknown>>();
function inTurn<T>(session: string, work: () => Promise<T>): Promise<T> {
  const before = planning.get(session) ?? Promise.resolve();
  const mine = before.then(work, work);
  const settled = mine.catch(() => {});
  planning.set(session, settled);
  void settled.then(() => { if (planning.get(session) === settled) planning.delete(session); });
  return mine;
}

/** The models "/review with …" can name: the engine's you can use now (a review runs beside it). */
async function reviewReaders(engine: Target | null, folder: string | undefined): Promise<Reader[]> {
  if (!engine) return [];
  const lists = await listsHere(engine, null, folder).catch(() => null);
  if (!lists?.engine) return [];
  const accounts = lists.engine.accounts;
  // (Retiring ones too: a conversation on one carries on, and the composer offers them.)
  return offeredHere(lists).filter(model => !model.agents).map(model => ({
    name: model.name,
    ref: { providerID: model.providerID, id: model.id },
    provider: model.providerID === "opencode" ? "OpenCode Zen" : accounts.get(model.providerID)?.name ?? model.providerID,
  }));
}

const refOf = (value: unknown): ModelRef | null =>
  isObject(value) && typeof value.providerID === "string" && typeof value.id === "string" ? { providerID: value.providerID, id: value.id } : null;

/** The request's own signal, and at most `ms` more: a stalled engine or bridge never holds the
 * composer's next message (each client sends it after this answer). */
const within = (signal: AbortSignal | undefined, ms: number): AbortSignal => (signal ? AbortSignal.any([signal, AbortSignal.timeout(ms)]) : AbortSignal.timeout(ms));
/** How long the box waits on its engine or bridge for one step. */
const STEP_PATIENCE = 30_000;

/** `work`, or a failure once `ms` have passed without its answer. */
async function inTime<T>(work: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("Its computer didn’t answer in time. Try again in a moment.")), ms); });
  try {
    return await Promise.race([work, late]);
  } finally {
    clearTimeout(timer);
  }
}

/** A message the bridge (or a route) answered with, as words to show. */
const said = (error: unknown, otherwise: string) => (error instanceof Error && error.message ? error.message : otherwise);

/** What the box did for it, as the answer says it. */
type Done = { goal?: unknown; goalCleared?: true; askedBot?: true; autopilot?: SessionAutopilot; wakeUp?: { due: number; every: string; note: string }; sideQuestion?: unknown; handoff?: unknown; review?: unknown };

/** One of the box's routes asked from here, as a client would: its status and JSON. */
async function routeAnswer(pieces: AsyncIterable<AnswerPiece>): Promise<{ status: number; value: Record<string, unknown>; notAdmitted?: true }> {
  const answer = await collected(pieces);
  const value = parsed(answer.body);
  return { status: answer.status, value: isObject(value) ? value : {}, notAdmitted: answer.notAdmitted };
}

class DraftNotAdmitted {
  constructor(readonly answer: { status: number; value: unknown; notAdmitted: true }) {}
}

function effectFailure(answer: { status: number; value: Record<string, unknown>; notAdmitted?: true }, otherwise: string): never {
  if (answer.notAdmitted) throw new DraftNotAdmitted({ ...answer, notAdmitted: true });
  throw Object.assign(new Error(typeof answer.value.message === "string" ? answer.value.message : otherwise), { status: answer.status });
}

async function doStep(step: BoxStep, session: string, place: { target: Target; data: Partial<EngineSession> }, context: RouteContext): Promise<Done> {
  const { request, readEngine, bridge } = context;
  const signal = within(context.signal, STEP_PATIENCE);
  const agents = bridge();
  const at = (path: string) => `/api/kuvo/${path}/${encodeURIComponent(session)}`;
  switch (step.do) {
    case "goal": {
      if (!agents) throw new Error(GOALS_NOT_RUNNING);
      const { command } = step;
      if (command.kind === "set") return { goal: await call(agents, "PUT", at("goals"), { condition: command.words }, signal) };
      if (command.kind === "pause") return { goal: await call(agents, "POST", `${at("goals")}/pause`, undefined, signal) };
      if (command.kind === "resume") return { goal: await call(agents, "POST", `${at("goals")}/resume`, undefined, signal) };
      await call(agents, "DELETE", at("goals"), undefined, signal);
      return { goalCleared: true };
    }
    case "askBot": {
      // (In its own time: at most 10 s to find the conversation and 30 s for its engine to take
      // the words, so what it says is what happened.)
      const outcome = await askGoal(session, step.words, () => targetsOf({ readEngine, bridge }), () => !context.admitQueued?.());
      if ("refused" in outcome) throw new Error(outcome.refused.message);
      return { askedBot: true };
    }
    case "autopilot": {
      // On already: nothing to change.
      if (isAutopilot(place.data)) return {};
      const route = `kuvo/sessions/${encodeURIComponent(session)}/autopilot`;
      const answer = await inTime(routeAnswer(autopilotRoute({ ...context, signal, route, request: { ...request, method: "PUT", path: route, body: Buffer.from(JSON.stringify({ autopilot: true })) } })), STEP_PATIENCE);
      if (answer.status !== 200) effectFailure(answer, "Autopilot couldn’t be turned on. Try again.");
      return { autopilot: answer.value as unknown as SessionAutopilot };
    }
    case "wake": {
      const route = `kuvo/wakes/${encodeURIComponent(session)}`;
      const body = { due: step.due, every: step.every, note: step.note };
      const answer = await routeAnswer(answerWakes(session, undefined, { ...request, method: "POST", path: route, body: Buffer.from(JSON.stringify(body)) }, readEngine, bridge, signal, context.admitQueued));
      if (answer.status !== 200) effectFailure(answer, "The wake-up couldn’t be saved. Try again.");
      return { wakeUp: body };
    }
    case "btw":
    case "handoff":
    case "review": {
      if (!agents) throw Object.assign(new Error("unavailable"), { unavailable: true });
      if (step.do === "btw") return { sideQuestion: await call(agents, "POST", at("btw"), { question: step.question }, signal) };
      if (step.do === "handoff") return { handoff: await call(agents, "POST", at("handoff"), { next: step.next ?? undefined }, signal) };
      return { review: await call(agents, "POST", at("reviews"), { ...(step.model ? { model: step.model } : {}), ...(step.focus ? { focus: step.focus } : {}), ...(step.scope ? { scope: step.scope } : {}) }, signal) };
    }
  }
}

/** The answer, as clients read it: what the box did, then what the client does (PROTOCOL.md). */
function answerOf(decision: Decision, done: Done, failure: { restore: Restore; error: string } | null) {
  const { box: _box, failed: _failed, restore, error, send, command, ...steps } = decision;
  // Its step failed: nothing of the rest is done, and what was written goes back.
  if (failure) return { ...(decision.keyword ? { keyword: decision.keyword } : {}), restore: failure.restore, error: failure.error };
  return { ...steps, ...done, ...(command ? { command } : {}), ...(send ? { send } : {}), ...(restore ? { restore } : {}), ...(error ? { error } : {}) };
}

// What a send with its own id was answered, kept on disk an hour (the most recent 1000), so the same
// send again after this box started again is answered as it was, its step not done twice.
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
// once (`sentOnce`, as a message sent again is): the same send again gets the first's answer, the
// box's step never done twice; one that didn't go (refused, the engine not there) is tried again.
export async function* typedRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request } = context;
  const given = parsed(request.body);
  const header = request.headers["idempotency-key"];
  const named = isObject(given) && typeof given.id === "string" && given.id.trim() ? given.id : typeof header === "string" ? header : "";
  const id = request.method.toUpperCase() === "POST" && named.trim() ? named.trim().slice(0, 200) : null;
  if (!id) return yield* typedAnswer(context, match);
  const key = `typed\n${match[1]}\n${id}`;
  // Answered before this box last started: as it was, its step not done again.
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
    submittedThrough: from ?? own!, submitter, draft: given }, scoped ? draftKey(setup, session, given.id) : undefined)
    .catch(() => ({ ...uncertain(), notAdmitted: undefined }));
  const acceptedAt = "acceptedAt" in outcome && typeof outcome.acceptedAt === "number" ? outcome.acceptedAt : undefined;
  const value = isObject(outcome.value) && typeof outcome.value.message === "string" && outcome.status === 200 && acceptedAt
    ? { ...outcome.value, acceptedAt } : outcome.value;
  return yield* (queuedRefusal(outcome.notAdmitted ? outcome : null, json) ?? json(outcome.status, value));
}

async function finishDraft(context: RouteContext, session: string, place: { target: Target; data: Partial<EngineSession> }, onAgents: boolean,
  given: Record<string, unknown>, answer: TypedAnswer, original: string): Promise<Record<string, unknown>> {
  if (answer.restore || answer.error) return { answer };
  const hadEffect = Boolean(answer.wakeUp || answer.agent || answer.autopilot || answer.goal || answer.goalCleared);
  const response = async (route: string, method: string, value: unknown, run: (at: RouteContext) => AsyncIterable<AnswerPiece>) => {
    const veto = context.admitQueued?.();
    if (veto?.notAdmitted && !hadEffect) throw new DraftNotAdmitted({ ...veto, notAdmitted: true });
    if (veto) return { status: veto.status, value: isObject(veto.value) ? veto.value : {} };
    const headers = { ...context.request.headers };
    delete headers["idempotency-key"];
    const result = await routeAnswer(run({ ...context, route, request: { ...context.request, headers, path: route, method, body: Buffer.from(JSON.stringify(value)) } }));
    if (result.notAdmitted && !hadEffect) throw new DraftNotAdmitted({ ...result, notAdmitted: true });
    return result;
  };
  const failed = (value: Record<string, unknown>, status: number) => ({
    answer: { ...answer, send: undefined, command: undefined, toggleAutopilot: undefined,
      ...(status >= 500 || hadEffect ? { ambiguous: status >= 500 } : { restore: writtenBack(original) }),
      error: typeof value.message === "string" ? value.message : "Check the conversation to see whether the draft ran before sending again." },
  });
  const here = `${onAgents ? "kuvo/agents/" : ""}api/session/${encodeURIComponent(session)}`;
  if (answer.shell || answer.compact) {
    const result = await response(here + (answer.shell ? "/shell" : "/compact"), "POST",
      answer.shell ? { command: answer.shell } : {}, onAgents ? agentsRoute : engineRoute);
    return result.status < 300 ? { answer } : failed(result.value, result.status);
  }
  if (answer.toggleAutopilot) {
    const result = await response(`kuvo/sessions/${encodeURIComponent(session)}/autopilot`, "PUT", { autopilot: !isAutopilot(place.data) }, autopilotRoute);
    return result.status < 300 ? { answer: { ...answer, toggleAutopilot: undefined, autopilot: result.value } } : failed(result.value, result.status);
  }
  if (answer.sideThread && typeof given.thread === "string" && given.thread.length <= 200) {
    const result = await response(`kuvo/agents/api/kuvo/btw/${encodeURIComponent(session)}`, "POST", { question: original, thread: given.thread }, agentsRoute);
    return result.status < 300 ? { answer: { ...answer, sideThread: undefined, sideQuestion: result.value } } : failed(result.value, result.status);
  }
  if (!answer.send && !answer.command) return { answer };
  const input = given.message as Record<string, unknown>;
  const message = {
    files: input.files, notes: input.notes, delivery: input.delivery,
    ...(answer.send ? {
      id: given.id, words: answer.send.quoted && typeof given.quotedText === "string" ? given.quotedText : answer.send.text,
      keyword: answer.send.typed, comments: input.comments, named: input.named, widget: input.widget,
      ...(answer.send.steer ? { delivery: "steer" } : {}),
    } : { words: answer.command!.arguments, command: answer.command!.name }),
  };
  const route = `kuvo/sessions/${encodeURIComponent(session)}/send`;
  const result = await response(route, "POST", message, at => sendRoute(at, route.match(SEND_ROUTE)!));
  if (result.status >= 300) return failed(result.value, result.status);
  return { answer, ...(answer.send ? { message: given.id } : {}) };
}

async function* typedAnswer(context: RouteContext, match: RegExpMatchArray, draft?: Record<string, unknown>): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, signal, json } = context;
  const method = request.method.toUpperCase();
  if (method !== "GET" && method !== "POST") return yield* json(405, { message: "Read what it offers with GET, or send what was typed with POST." });
  let session = "";
  try { session = decodeURIComponent(match[1]); } catch {}
  if (!session) return yield* json(404, { message: "There's no such conversation here." });
  const given = parsed(request.body);
  const body = isObject(given) ? given : {};
  if (method === "POST" && typeof body.text !== "string") return yield* json(400, { message: "Send what was typed (text)." });
  // A send it can't answer for: what was written goes back as a keyword's does, saying why.
  const back = method === "POST" ? { restore: writtenBack(body.text as string) } : {};
  const targets = targetsOf({ readEngine, bridge });
  const unavailable = (status: number, message: string) => {
    const value = { message, ...back };
    return draft && request.fromOwnerPeer
      ? queuedRefusal({ status, value, notAdmitted: true }, json)!
      : json(status, value);
  };
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
  const agents = bridge();
  const data = place.data;
  const folder = data.location?.directory ?? data.directory;
  const keywords = keywordsOffered(data, agents !== null);

  if (method === "GET") {
    const commands = await engineCommands(engine, folder, signal);
    return yield* json(200, {
      keywords: keywords.map(name => ({ name, ...KEYWORD_WORDS[name] })),
      commands: commandsOffered(commands ?? [], keywords),
    });
  }

  const text = body.text as string;
  // What a "/" message needs: the engine's commands (a message that may name one), the models a
  // review can be read by, whether a goal is there to show.
  const slash = splitCommand(text) !== null;
  const onAgents = agents !== null && place.target.port === agents.port;
  // Whether its model can run, as this box says it (`kuvo/sessions/models`), read meanwhile, where
  // the text needs it. One that can't be said now (a coding agent's own subagent's) is let go, as
  // a client lets it go.
  const modelRead = needsModel(text) ? modelOf(place, engine, agents).catch(() => undefined) : Promise.resolve(undefined);
  const commands = slash ? await engineCommands(engine, folder, signal) : [];
  const typed = parseKeyword(text);
  const readers = typed?.keyword === "review" && !onAgents ? await reviewReaders(engine, folder) : [];
  const hasGoal = parseGoal(text)?.kind === "show" && agents ? await call(agents, "GET", `/api/kuvo/goals/${encodeURIComponent(session)}`, undefined, within(signal, 10_000)).then(() => true, () => false) : false;
  const model = await modelRead;
  const refusal = model?.refusal ?? null;
  const input = draft?.message;
  const hasFiles = isObject(input) && (Array.isArray(input.files) && input.files.length > 0 || isObject(input.notes) && Object.keys(input.notes).length > 0);
  const decision = decide(text, {
    keywords,
    commands: commands ?? [],
    files: draft ? hasFiles : body.files === true,
    side: body.side === true,
    refusal,
    readers,
    reviewer: refOf(body.reviewer),
    subagent: Boolean(data.parentID),
    hasGoal,
    botChat: botOfConversation(session) !== null,
    bridge: agents !== null,
    now: Date.now(),
    timeZone: zone(),
  });
  // The engine's commands unread: a keyword or goal goes on without them; what might have been one
  // of them can't be told from a message, and waits.
  if (commands === null && !decision.keyword && (decision.send || decision.sideThread)) return yield* json(503, { message: "The engine didn't answer. Try again in a moment.", restore: { text } });
  const veto = queuedRefusal(context.admitQueued?.() ?? null, json);
  if (veto) return yield* veto;
  let done: Done = {};
  let failure: { restore: Restore; error: string } | null = null;
  let uncertainStep = false;
  if (decision.box) {
    try {
      done = await doStep(decision.box, session, place, context);
    } catch (error) {
      if (error instanceof DraftNotAdmitted) {
        if (draft) throw error;
        error = new Error("Its computer couldn't admit that action.");
      }
      uncertainStep = !(isObject(error) && typeof error.status === "number" && error.status >= 400 && error.status < 500);
      // The bridge not reached (its record there, and nothing answering, or no answer in time),
      // rather than one that said no: as it isn't running, not in the runtime's words.
      const step = decision.box.do;
      const unreached = !(isObject(error) && typeof error.status === "number");
      const unavailable = (isObject(error) && error.unavailable === true) || ((step === "btw" || step === "handoff" || step === "review") && unreached);
      if (step === "goal" && unreached) error = new Error(GOALS_NOT_RUNNING);
      failure = unavailable
        ? { restore: writtenBack(text), error: KEYWORD_UNAVAILABLE }
        : { restore: decision.failed ?? { text }, error: said(error, "It couldn’t be done. Try again.") };
    }
  }
  // Plan on or off: which mode, as this box has the conversation and its modes, switched to here
  // (where it reads it: two "/plan" in a row don't race a client's own switch), for the client to
  // show; one "/plan" after another in each conversation, each reading the mode as it is then.
  const plan = !failure ? decision.plan : undefined;
  const agent = plan ? await inTurn(session, async () => {
    // (As it is now, after any "/plan" before it: not read, nothing is switched, and it says so.)
    const now = await call(place.target, "GET", `/api/session/${encodeURIComponent(session)}`, undefined, within(signal, 10_000)).catch(() => null) as Partial<EngineSession> | null;
    if (!now) return null;
    const chosen = await planAgent(plan, now, engine, folder, signal);
    if (chosen === now.agent) return chosen;
    if (context.admitQueued?.()) return null;
    const switched = await call(place.target, "POST", `/api/session/${encodeURIComponent(session)}/agent`, { agent: chosen }, within(signal, STEP_PATIENCE)).then(() => true, () => false);
    return switched ? chosen : null;
  }) : null;
  if (plan && !agent) {
    const answer = { ...(decision.keyword ? { keyword: decision.keyword } : {}),
      ...(draft ? { ambiguous: true } : { restore: writtenBack(text) }),
      error: draft ? "The mode change could not be confirmed. Check the conversation before sending again." : "The mode couldn’t be changed: its engine didn’t take it. Try again." };
    return yield* json(200, draft ? { answer } : answer);
  }
  const { plan: _plan, ...answer } = answerOf(decision, done, failure) as Record<string, unknown>;
  // Refused for its model (decide's first rule): what to offer doing about it, with its words.
  const action = refusal && model?.action ? { action: model.action } : {};
  const decisionAnswer = { ...answer, ...action, ...(agent ? { agent } : {}),
    ...(draft && failure && uncertainStep ? { restore: undefined, ambiguous: true } : {}) };
  return yield* json(200, draft ? await finishDraft(context, session, place, onAgents, draft, decisionAnswer as TypedAnswer, text) : decisionAnswer);
}
