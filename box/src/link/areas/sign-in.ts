// Signing in to an account (PROTOCOL.md, "Signing in"): the ways a client offers, in the box's order
// and words, for one account (`kuvo/sign-in/methods`) or every one listed (`kuvo/sign-in/accounts`,
// what Accounts' Sign In buttons and hints go by), and how a started sign-in is going, with what to say when it
// didn't work (`kuvo/sign-in/status`), so every client offers and says the same. The engine (or
// the coding agents' bridge, for an agent's account) does the signing in; the box only orders and
// words what it offers. Held to protocol/vectors/sign-in.json.

import { isObject, parsed } from "../../shared/json";
import { ENGINE_SERVICES } from "../../shared/limits";
import type { Target } from "../../shared/records";
import { pause } from "../../shared/wait";
import { call } from "../engine/client";
import { planHides } from "../work/model-rule";
import { readEndedSignIns } from "./accounts";
import { agentsAway, type AgentsAway, AGENTS_RESTARTING, bridgeBack } from "./agents-down";
import { engineDown } from "./engine";
import type { AnswerPiece, RouteContext } from "../server/context";
import { plainName } from "../../shared/text";
import { computerInSentence } from "../computer-name";
import { accountState } from "./sign-in-state";

export const SIGN_IN_METHODS_ROUTE = "kuvo/sign-in/methods";
export const SIGN_IN_STATUS_ROUTE = "kuvo/sign-in/status";
export const SIGN_IN_ACCOUNTS_ROUTE = "kuvo/sign-in/accounts";
export const SIGN_IN_START_ROUTE = "kuvo/sign-in/start";

/** A way to sign in as the engine (or the bridge) lists it (`api/integration`'s `methods`). */
interface ListedMethod {
  type?: unknown;
  id?: unknown;
  label?: unknown;
  form?: unknown;
  names?: unknown;
}

/** An account as the engine (or the bridge) lists it: its ways in, and how it's signed in now. */
export interface ListedAccount {
  id?: unknown;
  name?: unknown;
  methods?: unknown;
  connections?: unknown;
  metadata?: unknown;
}

/** What an account the engine lists is, to Accounts: a `provider` (models to run), a web `search`
 * service (its key is Settings › Engine's), an `mcp` server's sign-in (an integration's), or the
 * engine's own way to a plan a coding agent brings (`agentDuplicate`: "Claude Agent", which Kuvo
 * Code runs through Claude Code instead). */
export type AccountKind = "provider" | "search" | "mcp" | "agentDuplicate";

/** Web search services the engine lists among its sign-ins. */
const SEARCH_SERVICES = new Set(["exa", "firecrawl", "parallel", "tavily", "tinyfish"]);
/** The engine's own way to a coding agent's plan. */
const AGENT_DUPLICATES = new Set(["claude-agent"]);

/** What `account` is, and whether Accounts lists it: every provider, never a search service or an
 * MCP server's sign-in, and the engine's own way to a coding agent's plan only while it's signed in
 * (one added before stays, to sign out of). The coding agents' accounts (`agents`) are all listed. */
export function accountKind(account: ListedAccount, agents = false): { kind: AccountKind; listed: boolean } {
  const id = typeof account.id === "string" ? account.id : "";
  const source = (account.metadata as { source?: unknown } | null | undefined)?.source;
  if (agents) return { kind: "provider", listed: true };
  if (source === "mcp") return { kind: "mcp", listed: false };
  if (SEARCH_SERVICES.has(id)) return { kind: "search", listed: false };
  if (AGENT_DUPLICATES.has(id)) return { kind: "agentDuplicate", listed: Array.isArray(account.connections) && account.connections.length > 0 };
  return { kind: "provider", listed: true };
}

/** An account's mark, by the plan limits' service ids (PROTOCOL.md, "Plan limits", `services`): a
 * coding agent's is its agent's (`metadata.agent`, as its bridge says); the engine's by its
 * provider (`ENGINE_SERVICES`). None for one with no mark of its own: a client draws its letter. */
export function serviceOf(account: ListedAccount, agents = false): string | undefined {
  const id = typeof account.id === "string" ? account.id : "";
  if (agents) {
    const agent = (account.metadata as { agent?: unknown } | null | undefined)?.agent;
    const kind = typeof agent === "string" && agent ? agent : id.split("@")[0]!.replace(/-code$/, "");
    return AGENT_SERVICES.has(kind) ? kind : undefined;
  }
  return ENGINE_SERVICES[id];
}

const AGENT_SERVICES = new Set(["claude", "codex", "grok", "cursor", "antigravity"]);

/** The code a device sign-in's page asks for ("Enter code: ABCD-1234"), where its attempt has one:
 * as it says it (`userCode`, the coding agents' bridge's), else from its page's address
 * (`user_code`), else from its instructions (what follows the last "code:", where that's one
 * word, else a code like ABCD-1234 standing by itself), as the engine leaves it for its own plans (Copilot, OpenCode, SuperGrok). */
export function userCodeOf(said: { userCode?: unknown; url?: unknown; instructions?: unknown } | null | undefined): string | undefined {
  if (typeof said?.userCode === "string" && said.userCode) return said.userCode;
  if (typeof said?.url === "string") {
    try {
      const code = new URL(said.url).searchParams.get("user_code");
      if (code) return code;
    } catch {}
  }
  const instructions = typeof said?.instructions === "string" ? said.instructions : "";
  // What follows the last "code:", where that's one word (its full stop left off); else a code
  // by its shape, whole (never part of a longer word: "GITHUB-COPILOT" isn't one).
  const at = instructions.toLowerCase().lastIndexOf("code:");
  const after = at < 0 ? "" : instructions.slice(at + "code:".length).trim().replace(/[.,;:!]+$/, "");
  if (after && !/\s/.test(after) && after.length <= 40) return after;
  return instructions.match(/(?<![A-Za-z0-9-])[A-Z0-9]{4}-[A-Z0-9]{4,5}(?![A-Za-z0-9-])/)?.[0];
}

/** A way to sign in, as a client offers it: the engine's own (`type`, `id`, `form`, as a client
 * starts it with), with the box's `label`, and `legacy` for one its maker keeps for older setups,
 * which a client offers apart, after the others. */
export interface OfferedMethod {
  type: "oauth" | "external" | "key";
  id?: string;
  label: string;
  form?: unknown[];
  legacy?: true;
}

/** Signing in with an account leads, then credentials the computer already has, then a key. */
const ORDER = { oauth: 0, external: 1, key: 2 } as const;

/** What a key's button says when the engine gives it no words of its own. */
export const KEY_LABEL = "Use an API key";
/** What a sign-in's button says when the engine gives it no words of its own. */
export const SIGN_IN_LABEL = "Sign in";

/** An account's ways in, as a client offers them, and `hint`, the way in its card names under its
 * name ("Sign in with ChatGPT", "API key"; "Connect" for another way, or none). */
export interface OfferedMethods {
  methods: OfferedMethod[];
  environment: string[];
  hint: string;
}

/** The ways a client offers to sign in to `account`, in order (each kind by `ORDER`, the engine's
 * order within a kind, legacy ones last), and the environment variables that sign in to it too. A
 * plan whose sign-in goes through a coding agent (ChatGPT's through Codex, SuperGrok's through Grok)
 * isn't offered for the engine's provider, unless it's signed in that way already (it can be again). */
export function signInMethods(account: ListedAccount): OfferedMethods {
  const provider = typeof account.id === "string" ? account.id : "";
  const listed = (Array.isArray(account.methods) ? account.methods : []).filter((method): method is ListedMethod => !!method && typeof method === "object");
  const connections = Array.isArray(account.connections) ? account.connections : [];
  const signedInWithPlan = connections.some(connection => (connection as { method?: unknown } | null)?.method === "oauth");
  const offered: OfferedMethod[] = [];
  for (const method of listed) {
    const type = method.type;
    if (type !== "oauth" && type !== "external" && type !== "key") continue;
    const id = typeof method.id === "string" ? method.id : undefined;
    // (The engine's oauth and external ways always have an id: one without can't be started.)
    if (type !== "key" && !id) continue;
    if (type === "oauth" && !signedInWithPlan && planHides(provider, id!)) continue;
    const given = typeof method.label === "string" ? method.label.trim() : "";
    const label = given || (type === "key" ? KEY_LABEL : SIGN_IN_LABEL);
    offered.push({
      type,
      ...(type !== "key" ? { id } : {}),
      label,
      ...(Array.isArray(method.form) ? { form: method.form } : {}),
      ...(/legacy/i.test(label) ? { legacy: true as const } : {}),
    });
  }
  // A stable sort: the engine's order within each kind.
  const methods = offered
    .map((method, at) => ({ method, at }))
    .sort((a, b) => Number(!!a.method.legacy) - Number(!!b.method.legacy) || ORDER[a.method.type] - ORDER[b.method.type] || a.at - b.at)
    .map(each => each.method);
  const environment = listed
    .filter(method => method.type === "env" && Array.isArray(method.names))
    .flatMap(method => (method.names as unknown[]).filter((name): name is string => typeof name === "string" && name !== ""));
  return { methods, environment, hint: hintOf(methods, typeof account.name === "string" ? account.name : provider) };
}

/** The way in a card names: its first sign-in that isn't legacy, in one wording ("Login with" is the
 * engine's for some), or plain "Sign in" where its words only name the account again ("OpenCode
 * Console account" under OpenCode Console); else "API key" for keys alone, else "Connect". */
function hintOf(methods: OfferedMethod[], name: string): string {
  const signIn = methods.find(method => method.type === "oauth" && !method.legacy);
  if (signIn) return name && signIn.label.toLowerCase().includes(name.toLowerCase()) ? "Sign in" : signIn.label.replaceAll("Login with", "Sign in with");
  // (The computer's credentials, a key perhaps beside them: connected, as its button says.)
  return methods.some(method => method.type === "key") && !methods.some(method => method.type === "external") ? "API key" : "Connect";
}

/** Whether an account is a plan you sign in to (one of its ways in is its sign-in page), rather
 * than one you enter a key or the computer's credentials for: a coding agent's, ChatGPT's or
 * Copilot's are plans; OpenAI's key, DeepSeek's or Amazon Bedrock's credentials aren't. From its
 * ways in as the box offers them, so OpenAI, whose ChatGPT plan comes through Codex, is a key. */
export const isPlan = (offered: OfferedMethods): boolean => offered.methods.some(method => method.type === "oauth");

/** What a client says of a sign-in that ended without signing in: `expired`, or `failed` with the
 * engine's `message` (a sign-in page that sends back an error hands the engine its OAuth code, as
 * `access_denied` when you turn it down there, which the engine passes on as it is). */
export function failureWords(status: "failed" | "expired", message?: unknown): string {
  if (status === "expired") return "The sign-in timed out. Try again.";
  const said = typeof message === "string" ? message.trim() : "";
  if (!said) return "Signing in didn’t work.";
  const lower = said.toLowerCase();
  if (lower.includes("access_denied")) return "The sign-in was turned down on its page. Try again to sign in.";
  if (lower.includes("server_error") || lower.includes("temporarily_unavailable")) return "The service couldn’t sign you in just now. Try again in a moment.";
  // The page's answer was for another try: an address pasted from an earlier sign-in.
  if (lower.includes("state mismatch") || lower.includes("state_mismatch")) return "That page was from an earlier sign-in. Try again to sign in.";
  // Another OAuth code, on its own: in words.
  if (/^[a-z]+(_[a-z]+)+$/i.test(said)) return `Signing in didn’t work (${lower.replaceAll("_", " ")}).`;
  return said;
}

/** Where `query` asks about: the coding agents' bridge (`agents=1`, for an agent's account), else
 * the engine; or why it can't be asked now. */
async function placeOf(query: URLSearchParams, context: RouteContext): Promise<{ target: Target } | { down: AgentsAway | { message: string } }> {
  if (query.get("agents") === "1") {
    // (While the bridge restarts, waited for: PROTOCOL.md, "The coding agents' state".)
    const bridge = await bridgeBack(context.bridge, context.signal);
    return bridge ? { target: bridge } : { down: agentsAway() ?? { message: AGENTS_RESTARTING, reason: "agentsRestarting" } };
  }
  try {
    return { target: context.readEngine() };
  } catch {
    return { down: { message: engineDown() } };
  }
}

/** The longest a status ask waits for a change (`wait`), in seconds. */
const MOST_WAIT = 25;

/** `GET kuvo/sign-in/accounts[?agents=1]`: every account the engine (or the bridge) lists, with its
 * ways in, how it stands and what can be done with it (sign-in-state.ts). */
async function* accountsRoute(context: RouteContext, query: URLSearchParams): AsyncGenerator<AnswerPiece> {
  const { signal, json } = context;
  const place = await placeOf(query, context);
  if ("down" in place) return yield* json(503, place.down);
  const deadline = AbortSignal.timeout(15_000);
  try {
    const listed = await call(place.target, "GET", "/api/integration", undefined, signal ? AbortSignal.any([signal, deadline]) : deadline);
    const accounts = (Array.isArray(listed) ? listed : []).filter((each): each is ListedAccount => typeof (each as ListedAccount | null)?.id === "string");
    const agents = query.get("agents") === "1";
    // The engine's plans whose saved sign-in runs were refused with, as the box keeps them.
    const ended = agents ? new Map<string, string>() : readEndedSignIns();
    const computer = computerInSentence(context.request);
    return yield* json(200, {
      accounts: accounts.map(account => {
        const service = serviceOf(account, agents);
        const offered = signInMethods(account);
        return { id: account.id as string, ...offered, ...accountKind(account, agents), plan: isPlan(offered), ...(service ? { service } : {}), ...accountState(account, offered, { agents, ended: ended.get(account.id as string), computer }) };
      }),
    });
  } catch (error) {
    if (deadline.aborted) return yield* json(504, { message: "It didn't answer in time. Try again." });
    const status = (error as { status?: unknown } | null)?.status;
    return yield* json(typeof status === "number" && status >= 400 ? status : 502, { message: error instanceof Error && error.message ? error.message : "It didn't answer." });
  }
}

/** `POST kuvo/sign-in/start?integration=<id>[&agents=1]` `{methodID, answer?}`: starts a sign-in in
 * the browser where the engine (or the bridge) does, as its own `connect/oauth` does, and answers
 * what that answered with the code its page asks for apart (`userCode`), where it has one. */
async function* startRoute(context: RouteContext, query: URLSearchParams): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const integration = query.get("integration")?.trim() ?? "";
  const given = parsed(request.body);
  if (!integration || !isObject(given) || typeof given.methodID !== "string" || !given.methodID) return yield* json(400, { message: "Say which account (integration) and which way in (methodID)." });
  const place = await placeOf(query, context);
  if ("down" in place) return yield* json(503, place.down);
  // (A coding agent's sign-in may download its program first: longer than a read.)
  const deadline = AbortSignal.timeout(120_000);
  try {
    const started = await call(place.target, "POST", `/api/integration/${encodeURIComponent(integration)}/connect/oauth`, { methodID: given.methodID, ...(isObject(given.answer) ? { answer: given.answer } : {}) }, signal ? AbortSignal.any([signal, deadline]) : deadline);
    if (!isObject(started)) return yield* json(502, { message: "The sign-in couldn't be started." });
    const userCode = userCodeOf(started);
    return yield* json(200, { ...started, ...(userCode ? { userCode } : {}) });
  } catch (error) {
    if (deadline.aborted) return yield* json(504, { message: "It didn't answer in time. Try again." });
    const status = (error as { status?: unknown } | null)?.status;
    return yield* json(typeof status === "number" && status >= 400 ? status : 502, { message: error instanceof Error && error.message ? error.message : "It didn't answer." });
  }
}

/** `GET kuvo/sign-in/methods?integration=<id>[&agents=1]` and `GET kuvo/sign-in/status?integration=<id>&attempt=<id>[&agents=1][&wait=<seconds>]`:
 * with `wait`, a sign-in still going is asked again each second, answered as soon as what it says
 * changes or it ends, else once that long has passed (for a client that asks through a process run
 * each time, as a Mac asks a machine's box over SSH). */
export async function* signInRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, signal, json } = context;
  const query = new URLSearchParams(request.path.split("?")[1] ?? "");
  if (route === SIGN_IN_START_ROUTE) return yield* startRoute(context, query);
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  if (route === SIGN_IN_ACCOUNTS_ROUTE) return yield* accountsRoute(context, query);
  const integration = query.get("integration")?.trim() ?? "";
  const attempt = route === SIGN_IN_STATUS_ROUTE ? query.get("attempt")?.trim() ?? "" : null;
  if (!integration || attempt === "") return yield* json(400, { message: attempt === "" ? "Say which sign-in (integration, attempt)." : "Say which account (integration)." });
  const place = await placeOf(query, context);
  if ("down" in place) return yield* json(503, place.down);
  // (A status ask has a limit for each read instead: one that waits for a change takes longer.)
  const deadline = AbortSignal.timeout(15_000);
  const asking = signal ? AbortSignal.any([signal, deadline]) : deadline;
  try {
    if (attempt === null) {
      // The whole list, as clients list accounts (the bridge reads each agent's anew for one alone).
      const listed = await call(place.target, "GET", "/api/integration", undefined, asking);
      const account = (Array.isArray(listed) ? listed : []).find(each => (each as ListedAccount | null)?.id === integration) as ListedAccount | undefined;
      if (!account) return yield* json(404, { message: `There's no ${integration} to sign in to here.` });
      return yield* json(200, signInMethods(account));
    }
    const statusRoute = `/api/integration/${encodeURIComponent(integration)}/connect/oauth/${encodeURIComponent(attempt)}`;
    const read = async (): Promise<Record<string, unknown> | null> => {
      const each = AbortSignal.timeout(15_000);
      return (await call(place.target, "GET", statusRoute, undefined, signal ? AbortSignal.any([signal, each]) : each)) as Record<string, unknown> | null;
    };
    let said = await read();
    const wait = Math.min(Math.max(Number(query.get("wait")) || 0, 0), MOST_WAIT) * 1000;
    if (wait > 0) {
      const first = JSON.stringify(said), until = Date.now() + wait;
      while (said?.status === "pending" && JSON.stringify(said) === first && Date.now() < until && !signal?.aborted) {
        await pause(1000, signal);
        if (signal?.aborted) break;
        said = await read();
      }
    }
    const status = said?.status;
    if (status !== "pending" && status !== "complete" && status !== "failed" && status !== "expired") return yield* json(502, { message: "The sign-in's status couldn't be read." });
    const userCode = userCodeOf(said);
    return yield* json(200, { ...said, ...(userCode ? { userCode } : {}), ...(status === "failed" || status === "expired" ? { failure: failureWords(status, said?.message) } : {}) });
  } catch (error) {
    if (attempt === null ? deadline.aborted : (error as Error | null)?.name === "TimeoutError") return yield* json(504, { message: "It didn't answer in time. Try again." });
    const status = (error as { status?: unknown } | null)?.status;
    const message = error instanceof Error && error.message ? error.message : "It didn't answer.";
    return yield* json(typeof status === "number" && status >= 400 ? status : 502, { message });
  }
}
