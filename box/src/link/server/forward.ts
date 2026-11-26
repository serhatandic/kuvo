import { acquireEngine, engineManaged } from "../engine/lifetime";
import type { ContextSize } from "../../shared/context";
import { currentEngineContext } from "../engine/context";
import { readServices, servicePath } from "../../shared/box-services";
// Forwarding to the engine
// What isn't the box's own goes on to the engine, or to the coding agents' bridge (`kuvo/agents/…`),
// a phone's prompts with who they name told, and that they're from a phone.

import { ROUTE_VERSION, ROUTE_VERSION_HEADER, routeVersionError } from "../../shared/route-version";
import { BAD_WAITING_ID, isWaitingID } from "../../shared/queues";
import { admitPrompt, MoveAdmissionClosed } from "../moves/admission";
import { compactionFeed, compactionPage } from "../../shared/compaction-words";
import { messageWordsPage } from "../../shared/message-words";
import { agentsAway, AGENTS_RESTARTING, bridgeBack } from "../areas/agents-down";
import { engineDown, stopShell } from "../areas/engine";
import { RUN } from "../home";
import { oauthStartBody, workQueueAccount } from "../areas/sign-in";
import { isObject, parsed } from "../../shared/json";
import { swapHolds, swapLine, swapUnderWay } from "../../shared/engine-swap";
import { keepWaitingLater, swapHoldsFor } from "../engine/queue";
import { messageID } from "../engine/sessions";
import { holdArchived } from "../areas/wakes";
import { MOVING } from "../bots/here";
import { frozen } from "../moves/records";
import { dispatchConsent } from "../ownership/authority";
import { inTurnOn } from "../engine/notes";
import { NOTES_STEP } from "../../shared/session-notes";
import { pause } from "../../shared/wait";
import { stopIfWaiting } from "../work/answers";
import { FROM_PHONE, withPartsOf } from "../messages/parts";
import { attachedIn } from "./attached";
import { knownTools } from "./known-tools";
import type { ModelInput } from "./attached";
import { namedIn, namingHere } from "./naming";
import { forgetSent, SENDING_PATIENCE, sentKey, sentOnce } from "./sent-once";
import type { SentAnswer } from "./sent-once";
import { agentsAllowed, allowed, canonicalPath, REQUEST_HEADERS, RESPONSE_HEADERS } from "./paths";
import type { EngineRecord, Target } from "../../shared/records";
import type { EngineSession } from "../engine/sessions";
import { queuedRefusal, type AnswerPiece, type LinkRequest, type RouteContext } from "./context";
import type { Naming } from "./naming";

export const AGENTS = "kuvo/agents/";

// The block a prompt from a phone ends with, for the agent (messages/parts.ts): the user is on their
// iPhone, where a file is shown with show_file. Never shown.
export { FROM_PHONE };

// COMPAT(0.7 own-envelope): a prompt a client wrote the blocks of itself: its parts kept, a phone's named and marked. Goes in #717's next part.
// A prompt's body with the parts of its text kept with it (`metadata.kuvo.parts`, `withPartsOf`),
// for clients to draw it by: one a client wrote the blocks of itself, the way before
// `kuvo/sessions/<id>/send`.
function withPartsIn(body: Buffer | undefined): Buffer | undefined {
  let prompt: { text?: unknown; metadata?: unknown } | null;
  try { prompt = JSON.parse(Buffer.from(body ?? "").toString()); } catch { return body; }
  if (typeof prompt?.text !== "string") return body;
  const given = prompt.metadata && typeof prompt.metadata === "object" && !Array.isArray(prompt.metadata) ? prompt.metadata as Record<string, unknown> : undefined;
  const metadata = withPartsOf(given, prompt.text);
  if (metadata === given) return body;
  return Buffer.from(JSON.stringify({ ...prompt, metadata }));
}

// A prompt's body with the block at the end of its text, once; any other body as it is.
export function fromPhone(body: Buffer | undefined): Buffer | undefined {
  let prompt: { text?: unknown } | null;
  try {
    prompt = JSON.parse(Buffer.from(body ?? "").toString());
  } catch {
    return body;
  }
  if (typeof prompt?.text !== "string" || prompt.text.trimEnd().endsWith("</from-phone>")) return body;
  return Buffer.from(JSON.stringify({ ...prompt, text: `${prompt.text}\n\n${FROM_PHONE}` }));
}

export class UnsupportedBridgeRouteVersion extends Error {
  constructor(readonly refusal: NonNullable<ReturnType<typeof routeVersionError>>) {
    super(refusal.message);
  }
}

// A bridge that predates route versions speaks v1. Validate an explicit version before any
// caller reads the body or forwards its head. Cancel a refused stream without waiting for EOF.
async function fetchRoute(url: URL, options: RequestInit, toAgents: boolean): Promise<Response> {
  const response = await fetch(url, options);
  if (toAgents) {
    const refusal = routeVersionError(response.headers.get(ROUTE_VERSION_HEADER) ?? undefined);
    if (refusal) {
      response.body?.cancel().catch(() => {});
      throw new UnsupportedBridgeRouteVersion(refusal);
    }
  }
  return response;
}

export const bridgeRefusedBeforeExecution = (reply: Response): boolean =>
  reply.status === 503 && reply.headers.get("x-kuvo-bridge-handover") === "1";

/** A mutation can follow the bridge only on an explicit pre-execution refusal. Reads may also
 * follow a lost connection. Return the bridge that answered for actions that follow acceptance. */
export async function bridgeRequest(target: Target, url: URL, options: RequestInit, current?: () => Target | null, read = false, authorize?: () => void): Promise<{ response: Response; target: Target }> {
  const send = (destination: Target) => {
    const address = new URL(url);
    address.port = String(destination.port);
    const headers = new Headers(options.headers);
    headers.set("authorization", "Basic " + Buffer.from("opencode:" + destination.password).toString("base64"));
    headers.set(ROUTE_VERSION_HEADER, ROUTE_VERSION);
    authorize?.();
    return fetchRoute(address, { ...options, headers }, true);
  };
  let response: Response | undefined;
  let failure: unknown;
  try { response = await send(target); }
  catch (error) {
    if (!read || !current || error instanceof UnsupportedBridgeRouteVersion || options.signal?.aborted) throw error;
    failure = error;
  }
  const transferring = response !== undefined && bridgeRefusedBeforeExecution(response);
  if (current && (!response || transferring)) {
    const next = await bridgeBack(() => {
      const found = current();
      return found && (found.port !== target.port || found.password !== target.password) ? found : null;
    }, options.signal ?? undefined, undefined, { transferring });
    if (next) {
      await response?.body?.cancel().catch(() => {});
      return { response: await send(next), target: next };
    }
  }
  if (!response) throw failure;
  return { response, target };
}

// What the model of `session` takes as input, as the engine at `base` says (its own model, or the
// engine's default): for the files a message carries (`attachedIn`).
export function modelInput(base: URL, headers: Record<string, string>, signal: AbortSignal | undefined, toAgents = false): ModelInput {
  const deadline = AbortSignal.timeout(1000);
  const read = async (at: string): Promise<unknown> => {
    const response = await fetchRoute(new URL(at, base), { headers, signal: signal ? AbortSignal.any([signal, deadline]) : deadline }, toAgents);
    return response.ok ? ((await response.json()) as { data?: unknown } | null)?.data ?? null : null;
  };
  type Listed = { id?: unknown; providerID?: unknown; aliases?: unknown; capabilities?: { input?: unknown } } | null;
  return async session => {
    const own = ((await read(`/api/session/${encodeURIComponent(session)}`)) as { model?: Listed } | null)?.model;
    const ref = own ?? ((await read("/api/model/default")) as Listed);
    if (!ref) return null;
    const model = Array.isArray(ref.capabilities?.input) ? ref
      : (((await read("/api/model")) as Listed[] | null) ?? []).find(listed => (listed?.id === ref.id || (Array.isArray(listed?.aliases) && listed.aliases.includes(ref.id))) && listed?.providerID === ref.providerID);
    const input = model?.capabilities?.input;
    return Array.isArray(input) ? input.filter((kind): kind is string => typeof kind === "string") : null;
  };
}

// The headers of an answer that go back to whoever asked.
export function answerHeaders(response: Response, fromAgents = false): Record<string, string> {
  const out: Record<string, string> = {};
  for (const name of RESPONSE_HEADERS) {
    const value = response.headers.get(name);
    if (value) out[name] = value;
  }
  const version = fromAgents ? response.headers.get(ROUTE_VERSION_HEADER) : null;
  if (version !== null) out[ROUTE_VERSION_HEADER] = version;
  return out;
}

async function heldEngineTurn(session: string, kind: string, payload: RequestInit["body"], target: Target | null) {
  if (!swapHoldsFor(target, session)) return;
  let body: unknown = null;
  try { body = JSON.parse(payload?.toString() ?? ""); } catch {}
  if (kind === "prompt" && isObject(body)) {
    if (body.id !== undefined && !isWaitingID(body.id)) return { status: 400, body: { message: BAD_WAITING_ID } };
    if (typeof body.text === "string") {
      const message = {
        id: isWaitingID(body.id) ? body.id : messageID(),
        text: body.text,
        delivery: body.delivery === "queue" ? "queue" as const : "steer" as const,
        ...(Array.isArray(body.files) ? { files: body.files } : {}),
        ...(isObject(body.metadata) ? { metadata: body.metadata } : {}),
      };
      try {
        if (await keepWaitingLater(session, message, {}, undefined, target)) return { status: 200, body: { waits: true } };
      } catch (error) {
        console.error("kuvo-link: could not add a waiting message", error);
        return { status: 500, body: { message: "This computer couldn’t save the message. Try sending it again." } };
      }
      if (target === null && !swapHoldsFor(null, session)) return;
    }
  }
  return { status: 503, body: { message: swapLine(swapUnderWay(RUN)) ?? "Kuvo is checking the engine’s inbox.", engineChanging: true } };
}

const engineTurn = /^\/?api\/session\/([^/?]+)\/(prompt|compact|summarize)$/;

// A command or shell typed into a conversation (the retired "/" and "!"): nothing here runs one any more.
const retiredTurn = /^\/?api\/session\/[^/?]+\/(command|shell)$/;

function liveConsentSession(path: string, method: string) {
  if (method !== "POST" && method !== "DELETE") return;
  const match = /^\/?api\/session\/([^/]+)\/(?:permission|form)\/[^/]+(?:\/reply)?$/.exec(path);
  return match && decodeURIComponent(match[1]!);
}

type Admission = NonNullable<RouteContext["admitQueued"]>;
class ForwardAdmission extends Error {
  constructor(readonly veto: NonNullable<ReturnType<Admission>>) { super("Forward admission closed"); }
}

// Passes `path` to the engine or the agents' bridge on this machine, with its password, and
// streams back what it answers. What's sent is the path as `canonicalPath` writes it, which is
// what `allows` checks.
export async function* forward(target: Target | null, path: string, request: LinkRequest, signal: AbortSignal | undefined, allows: (path: string, method: string) => boolean, naming: Naming | null = null, down: () => string = engineDown, toAgents = false, currentBridge?: () => Target | null, returningEngine?: () => Target, admission?: Pick<RouteContext, "admitQueued" | "json" | "phone">): AsyncGenerator<AnswerPiece> {
  const method = request.method.toUpperCase();
  const sent = canonicalPath(path);
  if (sent === null || !allows(sent, method)) {
    yield { head: { status: 403, headers: { "content-type": "text/plain" } } };
    yield { body: Buffer.from("That isn't available over the link.") };
    return;
  }
  if (method === "POST" && retiredTurn.test(sent.split("?")[0]!)) {
    yield { head: { status: 404, headers: { "content-type": "application/json" } } };
    yield { body: Buffer.from(JSON.stringify({ message: "Typed commands in a conversation are gone from Kuvo, so this computer doesn’t run them." })) };
    return;
  }
  const url = new URL("http://127.0.0.1/" + sent);
  const consent = liveConsentSession(url.pathname, method);
  const permission = consent && /^(.*\/permission)\/([^/]+)\/reply$/.exec(url.pathname);
  const headers: Record<string, string> = {};
  if (target) {
    url.port = String(target.port);
    headers.authorization = "Basic " + Buffer.from("opencode:" + target.password).toString("base64");
  }
  const context = { request, phone: admission?.phone, admitQueued: admission?.admitQueued };
  const native = method === "POST" ? engineTurn.exec(url.pathname) : null;
  const authorize = () => {
    const veto = admission?.admitQueued?.() ?? null;
    if (veto) throw new ForwardAdmission(veto);
  };
  const dispatch = async (body: RequestInit["body"], signal?: AbortSignal) => {
    if (toAgents && target) {
      const read = (method === "GET" || method === "HEAD") && url.pathname !== "/api/event";
      return (await bridgeRequest(target, url, { method, headers, body, signal }, consent ? undefined : currentBridge, read, authorize)).response;
    }
    authorize?.();
    const held = heldSession && toEngine && await heldEngineTurn(heldSession, toEngine[2]!, body, target);
    if (held) return Response.json(held.body, { status: held.status });
    if (!target && toEngine && toEngine[2] !== "prompt") throw new Error(down());
    const live = target ?? returningEngine?.();
    if (!live) throw new Error(down());
    const address = new URL(url);
    address.port = String(live.port);
    const authorization = "Basic " + Buffer.from("opencode:" + live.password).toString("base64");
    return fetchRoute(address, { method, headers: { ...headers, authorization }, body, signal }, false);
  };
  const sendTo = async (body: RequestInit["body"], signal?: AbortSignal) => {
    if (!consent) return dispatch(body, signal);
    // An answer is tied to this native instance. A returning engine must receive a fresh answer.
    if (!target) return Response.json({ message: down() }, { status: 503 });
    const given = permission && parsed(request.body);
    const check = permission && isObject(given) && typeof given.decision === "string"
      ? { target, route: permission[1]!, id: decodeURIComponent(permission[2]!), decision: given.decision, headers }
      : undefined;
    const reply = await dispatchConsent(context, [consent], signal, check, () => dispatch(body, signal));
    return "refusal" in reply ? Response.json(reply.refusal.value, { status: reply.refusal.status }) : reply.result;
  };
  const given = Object.fromEntries(Object.entries(request.headers ?? {}).map(([k, v]) => [k.toLowerCase(), v]));
  // (These are single headers, never a list.)
  for (const name of REQUEST_HEADERS) if (given[name]) headers[name] = given[name] as string;
  if (toAgents) headers[ROUTE_VERSION_HEADER] = ROUTE_VERSION;
  // A message with files: they're kept here, and the model gets as files what it reads
  // (`attachedIn`), from a phone or a Mac alike. A phone's message also says who it names with "&"
  // (`naming`), and that it's from the phone.
  const sending = method === "POST" && /^\/api\/session\/([^/]+)\/prompt$/.exec(url.pathname);
  const prompt = sending && !request.fromMac ? sending : null;
  // One moving to or from another computer now takes nothing until it's there.
  let moving = false;
  try { moving = Boolean(sending) && frozen().has(decodeURIComponent(sending ? sending[1]! : "")); } catch {}
  if (moving) {
    yield { head: { status: 409, headers: { "content-type": "application/json" } } };
    yield { body: Buffer.from(JSON.stringify({ message: MOVING })) };
    return;
  }
  // The engine being changed (PROTOCOL.md, "A new engine"): a message sent to it straight waits on
  // the box and goes once the new one runs, as one sent through `kuvo/sessions/<id>/send` does.
  const toEngine = native;
  let heldSession: string | undefined;
  try { if (toEngine && !toAgents) heldSession = decodeURIComponent(toEngine[1]!); } catch { /* let the engine answer an invalid address */ }
  // A message taken out of the engine's inbox: sent again under its id (Undo), it goes again.
  const cancelled = method === "DELETE" && /^\/api\/session\/([^/]+)\/inbox\/([^/]+)$/.exec(url.pathname);
  if (cancelled) forgetSent(decodeURIComponent(cancelled[1]!), decodeURIComponent(cancelled[2]!));
  // A session's metadata changed: whether it was archived before, as the engine has it (null when
  // it can't say). A phone sends all of it with any change (a pin, say), the flag as it was too.
  const changed = method === "PATCH" && /^\/api\/session\/([^/]+)$/.exec(url.pathname);
  let wasArchived: boolean | null = null;
  const readBefore = async () => {
    try {
      const patience = AbortSignal.timeout(NOTES_STEP);
      const before = await fetchRoute(url, { headers, signal: signal ? AbortSignal.any([signal, patience]) : patience }, toAgents);
      if (before.ok) wasArchived = (await before.json() as { data?: EngineSession } | null)?.data?.metadata?.["kuvo.archived"] === true;
    } catch {}
  };
  // A message that's here whole goes on to its conversation even if whoever sent it goes
  // meanwhile (a phone that gave up waiting while a big file was still on its way, #358): its
  // files are kept here by then, and it isn't lost without a word. Sent again under the same id
  // (as the share sheet and a phone's dropped link do), it goes once (below).
  const deadline = native ? AbortSignal.timeout(SENDING_PATIENCE.ms) : undefined;
  const delivering = sending ? deadline : deadline && signal ? AbortSignal.any([signal, deadline]) : deadline ?? signal;
  // What goes to the engine: a message as above, anything else as it came.
  const outgoing = async (): Promise<Buffer | undefined> => {
    if (method === "POST" && /^\/api\/integration\/[^/]+\/connect\/oauth$/.test(url.pathname)) {
      const given = parsed(request.body);
      if (isObject(given)) {
        headers["content-type"] = "application/json";
        return Buffer.from(JSON.stringify(oauthStartBody(request, given)));
      }
    }
    if (!sending) return request.body;
    const session = decodeURIComponent(sending[1]!);
    const holding = heldSession && swapHoldsFor(target, heldSession);
    if (holding) {
      const given = parsed(request.body);
      if (isObject(given) && given.id !== undefined && !isWaitingID(given.id)) return request.body;
    }
    // During a swap the outgoing engine's model is immaterial. Keep the same attachment
    // fallback whether its record has been removed yet or is still present.
    const input = target && !holding
      ? modelInput(url, headers, delivering, toAgents) : async () => null;
    const body = await attachedIn(request.body, session, input, request.fromMac === true, toAgents);
    // (The desktop app's over the link: who it names, without the phone's block.)
    const named = prompt ? await namedIn(body, session, naming) : body;
    return withPartsIn(prompt && !request.desktop ? fromPhone(named) : named);
  };
  // Not answering (starting again, say).
  const notAnswering = (): SentAnswer => ({ status: 503, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ message: down() })) });
  // A message under an id it was sent with already (its phone's link dropped before the answer
  // came, #144): taken once, every send answered as the one that went (`sentOnce`). However long
  // a busy engine takes to answer, it's waited for, as one without an id is: a repeat meanwhile
  // waits with it, rather than a "busy" sending its words back to be sent again under a new id
  // the engine would take twice. Not answering (the engine started again), a repeat tries again.
  const admittedSession = (consent && consent !== "global" ? consent : undefined)
    ?? heldSession ?? (sending ? decodeURIComponent(sending[1]!) : undefined);
  const admitted = <T>(write: () => Promise<T>): Promise<T> =>
    admittedSession ? admitPrompt(admittedSession, write) : write();
  const sendingError = (error: unknown): SentAnswer => error instanceof MoveAdmissionClosed
    ? { status: 409, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ message: error.message })) }
    : error instanceof UnsupportedBridgeRouteVersion
      ? { status: 502, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify(error.refusal)) }
      : notAnswering();
  const once = sending ? sentKey(toAgents ? "agents" : "engine", decodeURIComponent(sending[1]!), request.body) : null;
  if (once) {
    const answer = await sentOnce(once, async () => {
      try {
        const body = await outgoing();
        return await admitted(async () => {
          const response = await sendTo(body, delivering);
          return { status: response.status, headers: answerHeaders(response, toAgents), body: Buffer.from(await response.arrayBuffer()) };
        });
      } catch (error) {
        return sendingError(error);
      }
    });
    yield { head: { status: answer.status, headers: answer.headers } };
    if (answer.body.length) yield { body: answer.body };
    return;
  }
  const send = async () => {
    const body = method === "GET" || method === "HEAD" ? undefined : await outgoing();
    return admitted(() => sendTo(body, delivering));
  };
  let response: Response;
  try {
    // A session's written in its turn, as the box's own writers of it write (`../engine/notes.ts`),
    // so none of theirs lands between what it read and this; held until it's answered (at most as
    // long as one of theirs may take).
    if (changed && target) {
      const turn: { sent?: Promise<Response> } = {};
      await inTurnOn(target, decodeURIComponent(changed[1]!), async () => {
        await readBefore();
        const sent = turn.sent = send();
        const done = new AbortController();
        await Promise.race([sent.catch(() => {}), pause(NOTES_STEP, done.signal)]);
        done.abort();
      }, signal);
      if (!turn.sent) throw new Error("It wasn't sent.");
      response = await turn.sent;
    } else response = await send();
  } catch (error) {
    if (error instanceof ForwardAdmission) {
      if (admission) return yield* queuedRefusal(error.veto, admission.json)!;
      yield { head: { status: error.veto.status, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify(error.veto.value)) };
      return;
    }
    // Said so, unless the asker went.
    if (signal?.aborted) throw error;
    const answer = sendingError(error);
    yield { head: { status: answer.status, headers: answer.headers } };
    yield { body: answer.body };
    return;
  }
  const out = answerHeaders(response, toAgents);
  // Archived or brought back (`kuvo.archived` in its metadata), here with the Mac closed too: its
  // wake-ups wait, or come back, as a Mac's Archive has it. Only when that changed.
  if (changed && response.ok) {
    let archived: unknown;
    try { archived = JSON.parse(Buffer.from(request.body ?? []).toString("utf8"))?.metadata?.["kuvo.archived"]; } catch {}
    if (typeof archived === "boolean" && archived !== wasArchived) {
      try { holdArchived(decodeURIComponent(changed[1]!), archived); } catch {}
      // Archived while it waits on you: it stops, as a Mac's Archive has it.
      if (archived && target) await stopIfWaiting(target, decodeURIComponent(changed[1]!));
    }
  }
  if (!toAgents && method === "GET" && response.ok && /^\/api\/session\/[^/]+$/.test(url.pathname)) {
    const page = await response.json() as { data?: EngineSession & { context?: ContextSize } } | null;
    const context = await currentEngineContext(url, headers, signal, page?.data);
    if (page?.data) Object.assign(page.data, context);
    delete out["content-length"];
    yield { head: { status: response.status, headers: out } };
    yield { body: Buffer.from(JSON.stringify(page)) };
    return;
  }
  if (method === "GET" && response.ok && /^\/?api\/session\/[^/]+\/message(?:\/[^/?]+)?(?:\?|$)/.test(path)) {
    // With the box's words: compaction's, its tool steps' and its sessions' news (shared/message-words.ts).
    const body = Buffer.from(JSON.stringify(messageWordsPage(compactionPage(await response.json()), knownTools())));
    delete out["content-length"];
    yield { head: { status: response.status, headers: out } };
    yield { body };
    return;
  }
  yield { head: { status: response.status, headers: out } };
  if (!response.body) return;
  const reader = response.body.getReader();
  // The live feed: a compaction event carries the box's words, the engine's own too, as the
  // messages read above do (clients draw them, and the phone needs them to take the event).
  const feed = method === "GET" && response.ok && url.pathname === "/api/event" ? compactionFeed() : null;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        const rest = feed?.rest();
        if (rest?.length) yield { body: rest };
        return;
      }
      const body = feed ? feed.push(value) : Buffer.from(value);
      if (body.length) yield { body };
    }
  } finally {
    reader.cancel().catch(() => {});
  }
}

// The coding agents' sessions, accounts and Auto, through their bridge, as the engine's go
// to it; never where the engine is, nor an agent's update.
export async function* agentsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, readEngine, bridge, text } = context;
  const path = request.path.slice(AGENTS.length);
  if (!agentsAllowed(path, request.method)) return yield* text(403, "That isn't available over the link.");
  if (servicePath(path)) {
    const usage = readServices(RUN);
    if (usage) return yield* forward(usage, path, request, signal, agentsAllowed, null, () => "The box's usage reader is starting. Try again in a moment.", true, () => readServices(RUN) ?? null);
  }
  // Most routes allow the bridge half a minute to return during a restart or handover.
  // The Outputs follower retries side threads itself, so its first answer can leave immediately.
  const canonical = canonicalPath(path)?.split("?", 1)[0] ?? "";
  const method = request.method.toUpperCase();
  const threads = method === "GET" && /^api\/kuvo\/btw\/ses_[A-Za-z0-9_]+\/threads(?:\/events)?$/.test(canonical);
  const target = threads || liveConsentSession(canonical, method) ? bridge() : await bridgeBack(bridge, signal);
  if (!target) {
    yield { head: { status: 503, headers: { "content-type": "application/json" } } };
    yield { body: Buffer.from(JSON.stringify(agentsAway() ?? { message: AGENTS_RESTARTING, reason: "agentsRestarting" })) };
    return;
  }
  const notAnswering = "The coding agents aren't answering on this computer. Try again in a moment.";
  return yield* forward(target, path, request, signal, agentsAllowed, namingHere(readEngine), () => notAnswering, true, threads ? undefined : bridge, undefined, context);
}

// What's left goes to the engine (the paths a phone may ask for), and a command stopped to whichever
// runs it.
export async function* engineRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, route, readEngine, bridge, json, text } = context;
  const stop = route.match(/^kuvo\/shell\/([^/]+)\/stop$/);
  // A route of a box's own that this box doesn't have: 404, as every box answers it (one whose
  // path isn't what it seems is refused, below).
  if (!stop && route.startsWith("kuvo/") && canonicalPath(request.path) === request.path) return yield* json(404, {});
  if (!stop && !allowed(request.path, request.method)) return yield* text(403, "That isn't available over the link.");
  const holding = !stop && request.method.toUpperCase() === "POST" && swapHolds(RUN)
    && engineTurn.test(canonicalPath(request.path)?.split("?")[0] ?? "");
  // An event subscriber is passive. An engine request, including a session open,
  // waits for the shared cold start before dispatching its existing request.
  const passive = route === "api/event" || route === "api/session/active";
  const release = !stop && !holding && !passive ? await acquireEngine() : undefined;
  try {
    yield* forwardEngine(context, stop, holding, passive);
  } finally {
    release?.();
  }
}

async function* forwardEngine(context: RouteContext, stop: RegExpMatchArray | null, holding: boolean, passive: boolean): AsyncGenerator<AnswerPiece> {
  const { request, signal, route, readEngine, bridge, json, text } = context;
  let engine: EngineRecord | null = null;
  try { engine = readEngine(); } catch {}
  if (!engine && passive && engineManaged() && route === "api/session/active") return yield* json(200, { data: [] });

  // A coding agent's command stops without the engine.
  if (!engine && !holding && (!stop || !bridge())) return yield* json(503, { message: engineDown() });
  if (stop) {
    if (request.method.toUpperCase() !== "POST") return yield* text(405, "Stop it with POST.");
    const headers: Record<string, string> = {};
    const given = Object.fromEntries(Object.entries(request.headers).map(([k, v]) => [k.toLowerCase(), v]));
    for (const name of REQUEST_HEADERS) if (given[name]) headers[name] = given[name] as string;
    // The engine's, else a coding agent's, through their bridge.
    const runners = [engine, bridge()].filter(Boolean) as Target[];
    return yield* stopShell(decodeURIComponent(stop[1]!), runners, headers, signal);
  }
  if (engine && request.method === "GET" && route === "api/integration") {
    const github = await workQueueAccount(context);
    if (github) {
      try {
        const url = new URL(`http://127.0.0.1:${engine!.port}/${request.path}`);
        const headers = { authorization: `Basic ${Buffer.from(`opencode:${engine!.password}`).toString("base64")}` };
        const response = await fetch(url, { headers, signal });
        const result = await response.json();
        if (response.ok && isObject(result) && Array.isArray(result.data)) return yield* json(200, { ...result, data: [...result.data, github] });
        return yield* json(response.status, result);
      } catch {
        return yield* json(503, { message: engineDown() });
      }
    }
  }
  return yield* forward(engine, request.path, request, signal, allowed, namingHere(readEngine), engineDown, false, undefined, readEngine, context);
}
