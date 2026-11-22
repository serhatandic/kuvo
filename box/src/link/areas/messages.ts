import { acquireSessionEngine } from "../engine/lifetime";
// A client's message, as fields (PROTOCOL.md, "A message's parts"): `POST kuvo/sessions/<id>/send`
// writes what the agent reads (messages/parts.ts) and sends it to the conversation, its parts kept
// with it; `POST kuvo/messages/parts` reads the parts of messages from before, which carry none.

import { BAD_WAITING_ID, isWaitingID } from "../../shared/queues";
import { admitPrompt, MoveAdmissionClosed } from "../moves/admission";
import { isObject, parsed } from "../../shared/json";
import { MOVING } from "../bots/here";
import { messageID } from "../engine/sessions";
import { isRefused, placeOfSessionPatiently } from "../letters/letter";
import { heldAsItComes, underWay } from "../messages/arriving";
import { keptCommand } from "../messages/commands";
import { composeWithParts, fieldsOf, partsOf, withParts } from "../messages/parts";
import { withAttachedNote } from "../peers/naming";
import type { MessageFields } from "../messages/parts";
import { keepsList } from "../lists/sessions";
import { frozen } from "../moves/records";
import { keptFiles } from "../server/attached";
import type { PromptFile } from "../server/attached";
import { answerHeaders, bridgeRefusedBeforeExecution, bridgeRequest, modelInput, UnsupportedBridgeRouteVersion } from "../server/forward";
import { agentsAway, AGENTS_RESTARTING, bridgeBack } from "./agents-down";
import { everyoneFor, listedAgents } from "../server/naming";
import { REQUEST_HEADERS } from "../server/paths";
import { holdQueue, keepWaitingLater, keepWaitingHeld, sendNow, swapHoldsFor, type Outgoing } from "../engine/queue";
import { answering, BUSY, SENDING_PATIENCE, sentKey, sentOnce } from "../server/sent-once";
import type { SentAnswer } from "../server/sent-once";
import type { Target } from "../../shared/records";
import { fromAPhone } from "../server/context";
import type { AnswerPiece, LinkRequest, RouteContext } from "../server/context";
import { performsMutation } from "../ownership/admission";

export const SEND_ROUTE = /^kuvo\/sessions\/([^/]+)\/send$/;
export const PARTS_ROUTE = "kuvo/messages/parts";

/** How many messages one `kuvo/messages/parts` reads. */
const PARTS_MOST = 500;

/** A message as it goes to the engine: the text the agent reads, its files, and its parts. */
export interface Composed {
  text: string;
  files: PromptFile[];
  metadata: Record<string, unknown>;
}

/** While the engine is restarting, keep inline files without making a model request. */
function messageFiles(target: Target | null, toAgents: boolean, signal?: AbortSignal) {
  if (!target) return async () => null;
  const base = new URL(`http://127.0.0.1:${target.port}/`);
  const headers = { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}` };
  return modelInput(base, headers, signal, toAgents);
}

/** `fields` (with the `files` it carries, as the engine takes them) as the message the conversation
 * `session` at `target` gets: its files kept here as `attachedIn` keeps them, and its text written
 * for the agent (`composeMessage`), from a phone unless `request` is a Kuvo Code app's. */
export async function composed(fields: MessageFields, files: unknown[], session: string, target: Target | null, toAgents: boolean, request: LinkRequest, readEngine: () => Target, signal?: AbortSignal, metadata?: unknown, authorize?: () => void): Promise<Composed> {
  const kept = await keptFiles(files, session, messageFiles(target, toAgents, signal), toAgents, authorize);
  // (Everyone worked out again only for a message that names someone.)
  const everyone = fields.words.includes("&") ? await everyoneFor(fields.words, readEngine, fields.named ?? []) : listedAgents();
  const phone = fromAPhone(request);
  // One picked from the "&" list that this box doesn't list (yet): said, never dropped.
  const listed = new Set(everyone.agents.map(agent => agent.session));
  if (fields.words.includes("&") && fields.named?.some(picked => !listed.has(picked))) throw new NotListed();
  const { text, parts } = composeWithParts(fields, { excluding: session, here: everyone.machine, agents: everyone.agents }, kept ?? {}, phone);
  return { text, files: (kept?.files ?? files) as PromptFile[], metadata: withParts(metadata, parts) };
}

/** A message sent again as the agent read it (Retry, another agent's letter put back): its text as
 * it was, the files it carries kept here as `attachedIn` keeps them (a picture for a model that
 * doesn't see goes as a path, its note joining the one the text has), and its parts read from it. */
export async function sentAgain(text: string, files: unknown[], session: string, target: Target | null, toAgents: boolean, signal?: AbortSignal, metadata?: unknown, authorize?: () => void): Promise<Composed> {
  const kept = await keptFiles(files, session, messageFiles(target, toAgents, signal), toAgents, authorize);
  const written = kept ? withAttachedNote(text, kept.included, kept.paths, true) : text;
  return { text: written, files: (kept?.files ?? files) as PromptFile[], metadata: withParts(metadata, partsOf(written)) };
}

/** A session picked from the "&" list that this box doesn't list, after waiting for its list. */
class QueuedAdmission {
  constructor(readonly answer: SentAnswer) {}
}

export class NotListed extends Error {
  constructor() { super("An agent you named with & isn't known to this computer yet. Try again in a moment."); }
}

/** Whether `fields` and `files` make no message: nothing written, and nothing with it. */
export const isEmpty = (fields: MessageFields, files: unknown[]): boolean =>
  !fields.words.trim() && fields.widget === undefined && !fields.comments?.length && !files.length
  && !fields.notes?.included?.length && !fields.notes?.paths?.length && !fields.notes?.unsent?.length;

// `kuvo/sessions/<id>/send`: `{id, words, files?, delivery?, widget?, comments?, named?,
// (`delivery`: `steer`, the default; `queue`, after the run; `now`, the step under way stopped for it)
// notes?}` as a message (as written, one starting with "/" or "!" too); `{id, text, files?,
// delivery?}`, one sent before sent again as the agent read it (Retry), its files as they went.
// Answered as the engine (or the coding agents' bridge) answers it; a message sent again under its id goes once (`sentOnce`), whoever sent it
// going meanwhile.
export async function* sendRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, { message: "Send a message with POST." });
  let session = "";
  try { session = decodeURIComponent(match[1]!); } catch {}
  if (!session) return yield* json(404, { message: "There's no such conversation here." });
  // (Under way until it's answered, for a Stop meanwhile to hold it with the rest.)
  const finished = underWay(session);
  const release = swapHoldsFor(null, session) ? undefined : await acquireSessionEngine(session);
  try {
    return yield* sending(context, session);
  } finally {
    release?.();
    finished();
  }
}

async function* sending(context: RouteContext, session: string, retry = true): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, signal, json } = context;
  const given = parsed(request.body);
  const again = isObject(given) && typeof given.text === "string" ? given.text : null;
  const fields = isObject(given) && again === null ? fieldsOf(given) : null;
  if (!isObject(given) || (!fields && again === null)) return yield* json(400, { message: "Send what was written (words)." });
  const files = Array.isArray(given.files) ? given.files : [];
  if (fields ? isEmpty(fields, files) : !again!.trim() && !files.length) return yield* json(400, { message: "There's nothing to send." });
  if (frozen().has(session)) return yield* json(409, { message: MOVING });
  const engineHere = () => { try { return readEngine(); } catch { return null; } };
  const engine = engineHere();
  let agents: Target | null = bridge();
  const targets = [engine, agents].filter((target): target is Target => Boolean(target));
  const notAdmitted = (value: unknown, status = 503): SentAnswer => ({
    status, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify(value)), notAdmitted: true,
  });
  const readSession = async (target: Target, url: URL, signal: AbortSignal) => {
    if (target !== engine) {
      const result = await bridgeRequest(target, url, { signal }, keepsList() ? undefined : bridge, true);
      agents = result.target;
      return result;
    }
    const headers = { authorization: "Basic " + Buffer.from("opencode:" + target.password).toString("base64") };
    return { target, response: await fetch(url, { headers, signal }) };
  };
  let place: Awaited<ReturnType<typeof placeOfSessionPatiently>> = null;
  try {
    if (targets.length) place = await placeOfSessionPatiently(session, targets, readSession);
    if (!place && !agents && !swapHoldsFor(null, session)) {
      agents = bridge(); // It can have published while the engine was answering.
      if (!agents && agentsAway({ bridge })?.reason === "agentsRestarting") {
        agents = keepsList() ? bridge() : await bridgeBack(bridge, signal);
        if (!agents) return yield* answering(notAdmitted({ message: AGENTS_RESTARTING }));
      }
      agents ??= bridge();
      if (agents && !keepsList()) place = await placeOfSessionPatiently(session, [agents], readSession);
    }
  } catch (error) {
    if (retry && engine && !engineHere() && swapHoldsFor(null, session)) return yield* sending(context, session, false);
    if (!(isRefused(error) && !engineHere() && swapHoldsFor(null, session))) {
      if (error instanceof UnsupportedBridgeRouteVersion) return yield* answering(notAdmitted(error.refusal, 502));
      return yield* answering(notAdmitted({ message: isRefused(error) ? "The engine isn't running." : BUSY }));
    }
  }
  // Check the bridge first if it is up. A conversation it doesn't own can still be on the
  // engine whose record the keeper removed for the restart.
  if (!place) {
    const current = engineHere();
    if (retry && current && current.port !== engine?.port) return yield* sending(context, session, false);
    if (current || !swapHoldsFor(null, session)) {
      if (!targets.length && !agents) return yield* answering(notAdmitted({ message: "The engine isn't running." }));
      return yield* answering(notAdmitted({ message: "There's no such conversation here." }, 404));
    }
  }
  const target = place?.target ?? null;
  const toAgents = target !== null && agents?.port === target.port && agents.password === target.password;
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (target) headers.authorization = `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`;
  const asked = Object.fromEntries(Object.entries(request.headers ?? {}).map(([name, value]) => [name.toLowerCase(), value]));
  for (const name of REQUEST_HEADERS) if (typeof asked[name] === "string") headers[name] = asked[name] as string;
  if (given.id !== undefined && !isWaitingID(given.id)) return yield* json(400, { message: BAD_WAITING_ID });
  const id = isWaitingID(given.id) ? given.id : undefined;
  const at = target && new URL(`http://127.0.0.1:${target.port}/api/session/${encodeURIComponent(session)}/prompt`);
  // A message goes on even if whoever sent it goes meanwhile (as `forward` sends one).
  const denied = (): SentAnswer | null => {
    if (!performsMutation("queuedSessionWork", { kind: "conversations", id: session }))
      return notAdmitted({ message: MOVING }, 409);
    const veto = context.admitQueued?.();
    return veto ? { ...notAdmitted(veto.value, veto.status), notAdmitted: veto.notAdmitted } : null;
  };
  const authorize = () => {
    const veto = denied();
    if (veto) throw new QueuedAdmission(veto);
  };
  const send = async (): Promise<SentAnswer> => {
    try {
      authorize?.();
      const message = fields
        ? await composed(fields, files, session, target, toAgents, request, readEngine, undefined, undefined, authorize)
        : await sentAgain(again!, files, session, target, toAgents, undefined, undefined, authorize);
      return await admitPrompt(session, async () => {
        const veto = denied();
        if (veto) return veto;
        // One Stop named as still on its way, looked at as it goes (keeping its files can take a
        // while, Stop meanwhile): held, waiting on the box (the engine would run all it holds with
        // the first thing to reach a stopped conversation), not set going.
        const held = await heldAsItComes(session, id);
        // `now` (Send Now): to go now, the step under way stopped for it once the engine has it.
        const now = !held && given.delivery === "now";
        const delivery = held || given.delivery === "queue" ? "queue" : "steer";
        const body = { id: id ?? messageID(), text: message.text, ...(message.files.length ? { files: message.files } : {}), delivery, metadata: message.metadata };
        if (held && id && await keepWaitingHeld(session, { ...body, id, delivery: "queue" } as Outgoing & { id: string }, typeof given.words === "string" ? given.words : fields?.words)) {
          return { status: 200, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ waits: true })) };
        }
        const engineTarget = toAgents ? undefined : target;
        const holding = engineTarget !== undefined && swapHoldsFor(engineTarget, session);
        // For after this run while its queue is paused (a Stop, from any client), or while its engine
        // is being changed (PROTOCOL.md, "A new engine"): it waits on the box, as what arrives then does
        // (the engine would run all it holds with the next thing it had).
        if ((delivery === "queue" || holding) && await keepWaitingLater(session, body as Outgoing & { id: string }, { words: (typeof given.words === "string" ? given.words : fields?.words ?? message.text).trim() }, undefined, engineTarget)) {
          return { status: 200, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ waits: true })) };
        }
        if (!target || !at) return notAdmitted({ message: "The engine isn't running." });
        let answeredBy = target;
        const post = async (): Promise<SentAnswer> => {
          const deadline = AbortSignal.timeout(SENDING_PATIENCE.ms);
          const options = { method: "POST", headers, body: JSON.stringify(body), signal: deadline };
          authorize?.();
          const result = toAgents ? await bridgeRequest(target, at, options, bridge, false, authorize) : { response: await fetch(at, options), target };
          const { response } = result;
          answeredBy = result.target;
          if (toAgents && bridgeRefusedBeforeExecution(response)) {
            await response.body?.cancel().catch(() => {});
            return notAdmitted({ message: AGENTS_RESTARTING, reason: "agentsRestarting" });
          }
          return { status: response.status, headers: answerHeaders(response, toAgents), body: Buffer.from(await response.arrayBuffer()) };
        };
        const answer = await post();
        // (Taken by the agent meanwhile, it isn't waiting any more: that's as good.)
        if (now && answer.status >= 200 && answer.status < 300) await sendNow(answeredBy, session, body.id).catch(() => false);
        // (Stop hasn't paused it yet: it holds it with the rest, as it names it.)
        if (held && id && answer.status >= 200 && answer.status < 300) await holdQueue(answeredBy, session, undefined, [id]).catch(() => {});
        return answer;
      });
    } catch (error) {
      if (error instanceof QueuedAdmission) return error.answer;
      if (error instanceof NotListed || error instanceof MoveAdmissionClosed) return { status: 409, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ message: error.message })) };
      if (error instanceof UnsupportedBridgeRouteVersion) return { status: 502, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify(error.refusal)) };
      return { status: 503, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ message: toAgents ? "The coding agents aren't answering on this computer. Try again in a moment." : "The engine isn't answering. Try again in a moment." })) };
    }
  };
  const once = sentKey(toAgents ? "agents" : "engine", session, request.body);
  const answer = once ? await sentOnce(once, send) : await send();
  if (!target && retry && answer.status === 503 && engineHere()) return yield* sending(context, session, false);
  return yield* answering(answer);
}

// `kuvo/messages/parts`: `{texts, ids?, session?}`, the texts of messages from before (which carry
// no parts) and their ids, read as `{parts}`, one for each, in order: one that ran one of the
// engine's commands (when Kuvo still sent them) as it was typed (commands.ts), any other from its
// text. (`session`, which older clients send, is no longer needed.)
export async function* partsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, { message: "Send the texts with POST." });
  const given = parsed(request.body);
  const texts = isObject(given) && Array.isArray(given.texts) ? given.texts : null;
  if (!texts || texts.some(text => typeof text !== "string")) return yield* json(400, { message: "Send the messages' texts (texts)." });
  if (texts.length > PARTS_MOST) return yield* json(400, { message: `Send at most ${PARTS_MOST} texts at once.` });
  const ids = isObject(given) && Array.isArray(given.ids) ? given.ids : null;
  if (ids && (ids.length !== texts.length || ids.some(id => typeof id !== "string"))) return yield* json(400, { message: "Send an id for each text (ids)." });
  const commands = ids?.map(id => keptCommand(id as string)) ?? [];
  return yield* json(200, { parts: (texts as string[]).map((text, at) => commands[at] ?? partsOf(text)) });
}
