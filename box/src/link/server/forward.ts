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
import { swapLine, swapUnderWay } from "../../shared/engine-swap";
import { keepWaiting, swapHoldsFor } from "../engine/queue";
import { messageID } from "../engine/sessions";
import { holdArchived } from "../areas/wakes";
import { MOVING } from "../bots/here";
import { frozen } from "../moves/records";
import { inTurnOn } from "../engine/notes";
import { NOTES_STEP } from "../../shared/session-notes";
import { pause } from "../../shared/wait";
import { stopIfWaiting } from "../work/answers";
import { FROM_PHONE, withPartsOf } from "../messages/parts";
import { attachedIn } from "./attached";
import { knownTools } from "./known-tools";
import type { ModelInput } from "./attached";
import { namedIn, namingHere } from "./naming";
import { forgetSent, sentKey, sentOnce } from "./sent-once";
import type { SentAnswer } from "./sent-once";
import { agentsAllowed, allowed, canonicalPath, REQUEST_HEADERS, RESPONSE_HEADERS } from "./paths";
import type { EngineRecord, Target } from "../../shared/records";
import type { EngineSession } from "../engine/sessions";
import type { AnswerPiece, LinkRequest, RouteContext } from "./context";
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

class UnsupportedBridgeRouteVersion extends Error {
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

// What the model of `session` takes as input, as the engine at `base` says (its own model, or the
// engine's default): for the files a message carries (`attachedIn`).
export function modelInput(base: URL, headers: Record<string, string>, signal: AbortSignal | undefined, toAgents = false): ModelInput {
  const read = async (at: string): Promise<unknown> => {
    const response = await fetchRoute(new URL(at, base), { headers, signal }, toAgents);
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

// Passes `path` to the engine or the agents' bridge on this machine, with its password, and
// streams back what it answers. What's sent is the path as `canonicalPath` writes it, which is
// what `allows` checks.
export async function* forward(target: Target, path: string, request: LinkRequest, signal: AbortSignal | undefined, allows: (path: string, method: string) => boolean, naming: Naming | null = null, down: () => string = engineDown, toAgents = false): AsyncGenerator<AnswerPiece> {
  const method = request.method.toUpperCase();
  const sent = canonicalPath(path);
  if (sent === null || !allows(sent, method)) {
    yield { head: { status: 403, headers: { "content-type": "text/plain" } } };
    yield { body: Buffer.from("That isn't available over the link.") };
    return;
  }
  const url = new URL(`http://127.0.0.1:${target.port}/${sent}`);
  const headers: Record<string, string> = { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}` };
  const given = Object.fromEntries(Object.entries(request.headers ?? {}).map(([k, v]) => [k.toLowerCase(), v]));
  // (These are single headers, never a list.)
  for (const name of REQUEST_HEADERS) if (given[name]) headers[name] = given[name] as string;
  if (toAgents) headers[ROUTE_VERSION_HEADER] = ROUTE_VERSION;
  // A message or command with files: they're kept here, and the model gets as files what it reads
  // (`attachedIn`), from a phone or a Mac alike. A phone's message also says who it names with "&"
  // (`naming`), and that it's from the phone.
  const sending = method === "POST" && /^\/api\/session\/([^/]+)\/(prompt|command)$/.exec(url.pathname);
  const prompt = sending && sending[2] === "prompt" && !request.fromMac ? sending : null;
  // One moving to or from another computer now takes nothing until it's there.
  let moving = false;
  try { moving = Boolean(sending) && frozen().has(decodeURIComponent(sending ? sending[1]! : "")); } catch {}
  if (moving) {
    yield { head: { status: 409, headers: { "content-type": "application/json" } } };
    yield { body: Buffer.from(JSON.stringify({ message: MOVING })) };
    return;
  }
  // The engine being changed (PROTOCOL.md, "A new engine"): a message sent to it straight waits on
  // the box and goes once the new one runs, as one sent through `kuvo/sessions/<id>/send` does; a
  // command or shell, which would run on the engine stopped next, is said to wait.
  const toEngine = method === "POST" && /^\/api\/session\/([^/]+)\/(prompt|command|shell|compact|summarize)$/.exec(url.pathname);
  let heldSession: string | undefined;
  try { if (toEngine && !toAgents) heldSession = decodeURIComponent(toEngine[1]!); } catch { /* let the engine answer an invalid address */ }
  if (toEngine && heldSession && swapHoldsFor(target, heldSession)) {
    let body: unknown = null;
    try { body = JSON.parse(request.body?.toString("utf8") ?? ""); } catch {}
    const session = heldSession;
    const text = isObject(body) && typeof body.text === "string" ? body.text : null;
    if (toEngine[2] === "prompt" && isObject(body) && body.id !== undefined && !isWaitingID(body.id)) {
      yield { head: { status: 400, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify({ message: BAD_WAITING_ID })) };
      return;
    }
    const id = isObject(body) && isWaitingID(body.id) ? body.id : messageID();
    const message = isObject(body) && text !== null && toEngine[2] === "prompt"
      ? { id, text, delivery: body.delivery === "queue" ? "queue" as const : "steer" as const, ...(Array.isArray(body.files) ? { files: body.files } : {}), ...(isObject(body.metadata) ? { metadata: body.metadata } : {}) }
      : null;
    let kept = false;
    try {
      if (message) kept = await admitPrompt(session, async () => keepWaiting(session, message, {}, undefined, target));
    } catch (error) {
      const moving = error instanceof MoveAdmissionClosed;
      if (!moving) console.error("kuvo-link: could not add a waiting message", error);
      yield { head: { status: moving ? 409 : 500, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify({ message: moving ? error.message : "This computer couldn’t save the message. Try sending it again." })) };
      return;
    }
    if (kept) {
      yield { head: { status: 200, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify({ waits: true })) };
    } else {
      yield { head: { status: 503, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify({ message: swapLine(swapUnderWay(RUN)) ?? "Kuvo is checking the engine’s inbox.", engineChanging: true })) };
    }
    return;
  }
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
  // (as the share sheet and a phone's dropped link do), it goes once (below). A command still stops with its
  // sender: it has no id, and sent again it would run twice.
  const delivering = sending && sending[2] === "prompt" ? undefined : signal;
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
    const body = await attachedIn(request.body, session, modelInput(url, headers, delivering, toAgents), request.fromMac === true, toAgents);
    if (sending[2] !== "prompt") return body;
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
  const admitted = <T>(write: () => Promise<T>): Promise<T> =>
    sending ? admitPrompt(decodeURIComponent(sending[1]!), write) : write();
  const sendingError = (error: unknown): SentAnswer => error instanceof MoveAdmissionClosed
    ? { status: 409, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify({ message: error.message })) }
    : error instanceof UnsupportedBridgeRouteVersion
      ? { status: 502, headers: { "content-type": "application/json" }, body: Buffer.from(JSON.stringify(error.refusal)) }
      : notAnswering();
  const once = sending && sending[2] === "prompt" ? sentKey(toAgents ? "agents" : "engine", decodeURIComponent(sending[1]!), request.body) : null;
  if (once) {
    const answer = await sentOnce(once, async () => {
      try {
        const body = await outgoing();
        return await admitted(async () => {
          const response = await fetchRoute(url, { method, headers, body }, toAgents);
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
    return admitted(() => fetchRoute(url, { method, headers, body, signal: delivering }, toAgents));
  };
  let response: Response;
  try {
    // A session's written in its turn, as the box's own writers of it write (`../engine/notes.ts`),
    // so none of theirs lands between what it read and this; held until it's answered (at most as
    // long as one of theirs may take).
    if (changed) {
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
      if (archived) await stopIfWaiting(target, decodeURIComponent(changed[1]!));
    }
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
  // While it restarts (a hand-over, a crash the keeper is starting it again after), the request
  // waits for it, half a minute at most, rather than failing.
  const target = await bridgeBack(bridge, signal);
  if (!target) {
    yield { head: { status: 503, headers: { "content-type": "application/json" } } };
    yield { body: Buffer.from(JSON.stringify(agentsAway() ?? { message: AGENTS_RESTARTING, reason: "agentsRestarting" })) };
    return;
  }
  const notAnswering = "The coding agents aren't answering on this computer. Try again in a moment.";
  // A read that finds the bridge gone as it asks (it made way that moment) is asked again of the
  // next one, once, while that's restarting; anything else is answered as it came.
  const method = request.method.toUpperCase();
  if ((method === "GET" || method === "HEAD") && !/^\/?api\/event(\?|$)/.test(path)) {
    let failed = false;
    const first = forward(target, path, request, signal, agentsAllowed, namingHere(readEngine), () => ((failed = true), notAnswering), true);
    const head = await first.next();
    if (failed && !signal?.aborted) {
      const next = await bridgeBack(() => { const found = bridge(); return found && (found.port !== target.port || found.password !== target.password) ? found : null; }, signal);
      if (next) {
        await first.return(undefined);
        return yield* forward(next, path, request, signal, agentsAllowed, namingHere(readEngine), () => notAnswering, true);
      }
    }
    if (head.done) return;
    yield head.value;
    return yield* first;
  }
  return yield* forward(target, path, request, signal, agentsAllowed, namingHere(readEngine), () => notAnswering, true);
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
  let engine: EngineRecord | null = null;
  try {
    engine = readEngine();
  } catch {
    // A coding agent's command stops without the engine.
    if (!stop || !bridge()) return yield* json(503, { message: engineDown() });
  }
  if (stop) {
    if (request.method.toUpperCase() !== "POST") return yield* text(405, "Stop it with POST.");
    const headers: Record<string, string> = {};
    const given = Object.fromEntries(Object.entries(request.headers).map(([k, v]) => [k.toLowerCase(), v]));
    for (const name of REQUEST_HEADERS) if (given[name]) headers[name] = given[name] as string;
    // The engine's, else a coding agent's, through their bridge.
    const runners = [engine, bridge()].filter(Boolean) as Target[];
    return yield* stopShell(decodeURIComponent(stop[1]!), runners, headers, signal);
  }
  if (request.method === "GET" && route === "api/integration") {
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
  // (Read: without a stop, one that couldn't be read answered 503 above.)
  return yield* forward(engine!, request.path, request, signal, allowed, namingHere(readEngine));
}
