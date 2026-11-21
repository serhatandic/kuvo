// What a conversation waits on you for (PROTOCOL.md, "A conversation's asks"): every permission and
// question waiting in it and in its subagents, worded as the cards over the composer show them, so
// every client draws them alike; followed as they change; and answered here, where they were asked.

import { isObject, parsed } from "../../shared/json";
import { outsideShown } from "../../shared/text";
import { call } from "../engine/client";
import fs from "node:fs";
import { ASKS_FILE, askFollowers, askTimes } from "../glance/asked";
import { answerSandboxAsk, listSandboxAsks } from "../fence/outside";
import { answer as answerVaultAsk, listAsks as listVaultAsks } from "../vault/asks";
import { askHolder, holdsVault } from "../vault/holder";
import { isLocked, itemByID, onHost, primeGenerations, readItems } from "../vault/store";
import { clientName } from "./sandbox";
import { clientOf } from "./vault";
import { homeFolder } from "../home";
import { targetsOf } from "../letters/tools";
import { alwaysWords, describeForm, type EngineForm } from "../notices/news";
import { askersOf, callOf, sessionsWaitingOnServer } from "../work/answers";
import { listAskReviews } from "../work/reviewing";
import { admitPrompt, MoveAdmissionClosed } from "../moves/admission";
import { dispatchConsent, liveMutationRefusal, relayedConsent } from "../ownership/authority";
import { followedLive } from "../follow";
import { sessionHere, Unreached } from "./glance";
import type { AskChoice, AskedQuestion, PermissionAsk, SandboxAskShown, SessionAsk, VaultAskShown } from "../../shared/routes/sessions";
import type { SandboxAsk, SandboxStop } from "../../shared/routes/sandbox";
import type { VaultAsk, VaultItem } from "../../shared/routes/vault";
import type { Target } from "../../shared/records";
import type { EngineSession } from "../engine/sessions";
import type { AnswerPiece, RouteContext } from "../server/context";

export const SESSION_ASKS_ROUTE = /^kuvo\/sessions\/([^/]+)\/asks(?:\/([^/]+))?$/;

/** A permission as the engine (or the coding agents' bridge) lists it. */
interface EnginePermission {
  id: string;
  sessionID?: string;
  action?: unknown;
  resources?: unknown;
  save?: unknown;
  metadata?: { files?: unknown };
  source?: { messageID?: unknown; id?: unknown };
  message?: unknown;
  time?: { created?: unknown };
}

/** One of a form's fields, as this reads it. */
interface Field {
  key?: unknown;
  type?: unknown;
  title?: unknown;
  description?: unknown;
  options?: unknown;
  custom?: unknown;
  required?: unknown;
  hidden?: unknown;
  when?: unknown;
  format?: unknown;
  default?: unknown;
}

/** An ask found: as clients get it, and what answering it needs (where, and the form it is). */
interface Found {
  shown: SessionAsk;
  target: Target;
  asker: string;
  form?: { fields: Field[] };
  /** For an MCP server's form, filed under no session (`global`): the folder it's in. */
  folder?: string;
}

const text = (value: unknown): string | null => (typeof value === "string" ? value : null);
const strings = (value: unknown): string[] => (Array.isArray(value) ? value.filter((each): each is string => typeof each === "string") : []);

/** `file` as the card shows it: from the conversation's folder where it's in it. */
export function relativeTo(folder: string, file: string): string {
  if (!folder) return file;
  const base = folder.endsWith("/") ? folder : folder + "/";
  return file.startsWith(base) ? file.slice(base.length) : file;
}

/** A permission's card title, as the Mac's (AttentionCards.swift): what it would do, to the first
 * thing it covers (and how many more). */
export function permissionTitle(action: string, resources: string[], folder: string, command: string | null): string {
  const subject = resources.length ? relativeTo(folder, resources[0]!) + (resources.length > 1 ? ` and ${resources.length - 1} more` : "") : "this";
  switch (action) {
    case "edit": case "patch": return `Allow editing ${subject}?`;
    case "write": return `Allow writing ${subject}?`;
    case "read": return `Allow reading ${subject}?`;
    case "shell": case "bash": return resources.length > 1 && command === null ? "Allow running these commands?" : "Allow running this command?";
    case "external_directory": return "Allow working outside the project?";
    case "subagent": return `Allow starting the ${subject} agent?`;
    case "webfetch": return `Allow fetching ${subject}?`;
    default: return `Allow ${action.replaceAll("_", " ")}?`;
  }
}

/** A permission's answers, in the cards' order (KuvoClient's PermissionAnswer): Always's words as
 * its button draws them, with what it does whole and as it's said. Always lasts for the
 * conversation where a coding agent asks (`forConversation`), else for the project. */
export function choicesOf(request: { action: string; resources: string[]; save?: unknown }, home: string | null, forConversation = false): PermissionAsk["choices"] {
  const always = alwaysWords(request, home, forConversation);
  return [
    { answer: "once", title: "Allow Once", named: null, placeholder: null, help: null, spoken: null },
    { answer: "always", title: always.title, named: always.named, placeholder: null, help: always.help, spoken: always.spoken },
    { answer: "reject", title: "Don’t Allow", named: null, placeholder: null, help: null, spoken: null },
    { answer: "instead", title: "Tell It What to Do Instead…", named: null, placeholder: "Tell it what to do instead…", help: null, spoken: null },
  ];
}

const STATUSES = new Set(["added", "deleted", "modified"]);

/** A permission as clients draw it. `command`: the whole command line its tool call runs. */
export function permissionAsk(request: EnginePermission, asker: string, folder: string, command: string | null, home: string, forConversation = false): PermissionAsk {
  const action = String(request.action ?? "");
  const resources = strings(request.resources);
  const files = (Array.isArray(request.metadata?.files) ? request.metadata.files : []).flatMap(file => (isObject(file) && typeof file.file === "string"
    ? [{ file: file.file, patch: String(file.patch ?? ""), additions: Number(file.additions) || 0, deletions: Number(file.deletions) || 0, status: STATUSES.has(file.status as string) ? file.status as "added" | "deleted" | "modified" : null }]
    : []));
  return {
    id: request.id,
    kind: "permission",
    session: asker,
    title: permissionTitle(action, resources, folder, command),
    message: text(request.message),
    command,
    files,
    resources: resources.map(each => (action === "external_directory" ? outsideShown(each, home) : relativeTo(folder, each))),
    choices: choicesOf({ action, resources, ...(request.save !== undefined ? { save: request.save } : {}) }, home, forConversation),
  };
}

// A question the cards take: a string (not a password) or a pick of several, shown as it is.
const takesChoiceOrWords = (field: Field) => (field.type === "string" && field.format !== "password") || field.type === "multiselect";
const shownFields = (fields: Field[]) => fields.filter(field => field.hidden !== true);

/** The agent's questions (its question tool's form), as the cards ask them, one at a time; null for
 * any other form (an MCP server's, a field a card can't take, one shown only after another answer). */
export function questionsOf(form: { metadata?: { kind?: unknown }; fields?: unknown; title?: unknown }): AskedQuestion[] | null {
  const fields = (Array.isArray(form.fields) ? form.fields : []).filter(isObject) as Field[];
  if (form.metadata?.kind !== "question" || !fields.length || !fields.every(takesChoiceOrWords)) return null;
  const shown = shownFields(fields);
  if (!shown.length || shown.some(field => Array.isArray(field.when) && field.when.length) || shown.some(field => typeof field.key !== "string")) return null;
  return shown.map(field => {
    const options = (Array.isArray(field.options) ? field.options : []).filter(isObject).map(option => ({
      value: String(option.value ?? option.label ?? ""), label: String(option.label ?? option.value ?? ""), description: text(option.description),
    }));
    const multiple = field.type === "multiselect";
    return {
      key: field.key as string,
      title: text(field.title),
      question: text(field.description) ?? text(field.title) ?? text(form.title) ?? "",
      options,
      multiple,
      // (Words of your own: where it takes them, or a question in words with no choices.)
      custom: field.custom === true || (!multiple && !options.length),
      required: field.required === true,
    };
  });
}

// MARK: What the box asks you itself

const choice = (answer: AskChoice["answer"], title: string, more: Partial<AskChoice> = {}): AskChoice => ({ answer, title, named: null, placeholder: null, help: null, spoken: null, ...more });

/** A path with its home folder as `~`: a Mac's, a Linux machine's or root's (KuvoClient's
 * `SandboxStop.short`). */
export function shortPath(path: string): string {
  for (const prefix of ["/Users/", "/home/"]) {
    if (!path.startsWith(prefix)) continue;
    const rest = path.slice(prefix.length);
    const slash = rest.indexOf("/");
    return slash < 0 ? "~" : "~" + rest.slice(slash);
  }
  if (path === "/root") return "~";
  if (path.startsWith("/root/")) return "~" + path.slice("/root".length);
  return path;
}

const STOP_LABELS: Record<SandboxStop["kind"], string> = { write: "Write", read: "Read", connect: "Connect", job: "Job", app: "App", other: "Other" };
const folderName = (folder: string) => folder.replace(/\/+$/, "").split("/").pop() || folder;

/** A command the sandbox stopped, as its card shows it (the Mac's and the phone's card A): who wants
 * what, what the sandbox stopped, and its answers, Always naming the one place it keeps. */
export function sandboxAskShown(asked: SandboxAsk & { session: string }): SandboxAskShown {
  const asker = asked.botName ?? asked.sessionName ?? "An agent";
  const place = asked.owner?.kind === "bot" ? `${asked.owner.name || asker}’s folder` : asked.owner?.kind === "chats" ? "Chats" : folderName(asked.project);
  const first = asked.stopped?.[0];
  const message = first
    ? { write: `The sandbox stopped it from changing a file outside ${place}.`, read: "The sandbox stopped it from reading a file it keeps private.", connect: "The sandbox stopped it from making a connection.", job: "The sandbox stopped it from starting a background job.", app: "The sandbox stopped it from opening or talking to an app.", other: "The sandbox stopped it." }[first.kind] ?? "The sandbox stopped it."
    : asked.error ? (asked.machine ? `On ${asked.machine}, the sandbox stopped it with this error.` : "The sandbox stopped it with this error.") : "The sandbox stopped it.";
  const stopped = (asked.stopped ?? []).map(stop => ({
    label: STOP_LABELS[stop.kind] ?? "Other",
    detail: stop.path ? shortPath(stop.path) : stop.kind === "job" ? "Start a background job" : stop.kind === "app" ? "Open or talk to an app" : stop.kind === "connect" ? "Make a connection" : "Something the sandbox keeps out",
  }));
  const choices: AskChoice[] = [choice("once", "Allow Once", { help: "Runs it outside the sandbox this once", spoken: "Allow once: run it outside the sandbox" })];
  if (asked.always) {
    const path = shortPath(asked.always.path);
    const chats = asked.owner?.kind === "chats";
    const whose = asked.owner?.kind === "bot" ? `${asked.owner.name || asker}’s commands` : chats ? "every chat’s commands" : `${folderName(asked.project)}’s commands`;
    choices.push(choice("always", "Always Allow", {
      named: path, ...(chats ? { after: "in every chat" } : {}),
      help: `Runs it, and from now on lets ${whose} write in ${path} too, in the sandbox.`, spoken: `Always Allow ${path}${chats ? " in every chat" : ""}`,
    }));
  }
  choices.push(choice("reject", "Don’t Allow", { help: "It goes on without running it" }));
  return {
    id: asked.id, kind: "sandbox", session: asked.session, title: `${asker} wants to run a command outside the sandbox`, message, command: asked.command,
    stopped, error: stopped.length ? null : asked.error ?? null, asker, why: asked.why, choices,
  };
}

/** What the vault has to give the client asking: its items, or why there are none to give (`note`);
 * or, to one the vault doesn't answer, why it can't answer at all (`refused`). */
type Givable = { items: Pick<VaultItem, "id" | "name">[] } | { note: string } | { refused: string };

const NOT_A_CLIENT = "Answer it on your iPhone: only a device this computer’s vault knows can answer it.";

/** What the agent asks of the vault, as its card shows it: to use an item (Allow Once, Always Allow
 * where it may be given for good, Don't Allow), or an item it needs (Give, and Always Give, for each
 * the vault has). Always is a grant: a client makes sure it's you first. */
export function vaultAskShown(asked: VaultAsk & { session: string }, givable: Givable): VaultAskShown {
  const asker = asked.botName ?? asked.sessionName ?? "An agent";
  // Always only where it gives something for good, as the box decided when it asked (a bot's seat
  // in a group chat, or a conversation in a home folder, is answered only once).
  const forGood = asked.always === true && !asked.group;
  const who = asked.botName ?? "this project";
  const reject = choice("reject", "Don’t Allow", { help: "It goes on without it" });
  const title = asked.kind === "use" ? `${asker} wants to use ${asked.itemName ?? "an item"}` : `${asker} needs ${asked.need ?? "something from your vault"}`;
  // (One the vault doesn't answer is offered nothing to answer with.)
  if ("refused" in givable) return { id: asked.id, kind: "vault", session: asked.session, need: asked.kind, title, message: asked.why ?? null, site: asked.site ?? null, note: givable.refused, choices: [] };
  if (asked.kind === "use") {
    const item = asked.itemName ? `“${asked.itemName}”` : "it";
    const choices = [choice("once", "Allow Once", { help: `Lets it use ${item} this once`, spoken: `Allow once: let it use ${item}` })];
    if (forGood) choices.push(choice("always", "Always Allow", { help: `Lets ${who} use ${item} from now on, without asking`, spoken: `Always allow ${who} to use ${item}`, confirm: true }));
    choices.push(reject);
    return { id: asked.id, kind: "vault", session: asked.session, need: "use", title, message: asked.why ?? null, site: asked.site ?? null, note: null, choices };
  }
  const choices: AskChoice[] = [];
  for (const item of "items" in givable ? givable.items : []) {
    choices.push(choice("once", "Give", { named: item.name, item: item.id, help: `Gives it “${item.name}” this once`, spoken: `Give ${item.name}, this once` }));
    if (forGood) choices.push(choice("always", "Always Give", { named: item.name, item: item.id, help: `Gives it “${item.name}”, and lets ${who} use it from now on, without asking`, spoken: `Always give ${item.name}`, confirm: true }));
  }
  choices.push(reject);
  const note = "note" in givable ? givable.note : givable.items.length ? null : "There’s nothing in this computer’s vault to give it yet.";
  return { id: asked.id, kind: "vault", session: asked.session, need: "item", title, message: asked.why ?? null, site: asked.site ?? null, note, choices };
}

/** What the vault has to give the client asking: nothing to one it doesn't answer (only a phone,
 * this Mac's app, the Mac proving itself); else its items by name while it's open, those for the
 * site asked about first. */
async function givableTo(client: VaultClient, site: string | undefined): Promise<Givable> {
  if (!client) return { refused: NOT_A_CLIENT };
  if (isLocked()) return { note: "This computer’s vault is locked: unlock it to give it an item." };
  const forSite = (sites: readonly string[]) => !!site && sites.some(each => each === site || site.endsWith("." + each) || each.endsWith("." + site));
  return {
    items: readItems()
      .sort((a, b) => Number(forSite(b.sites)) - Number(forSite(a.sites)) || a.name.localeCompare(b.name))
      .map(item => ({ id: item.id, name: item.name })),
  };
}

/** The client asking, as the vault takes it (a proof is good once, so worked out once a request). */
type VaultClient = ReturnType<typeof clientOf>;

/** Everything `session` (on `target`, in `folder`) and its subagents wait on you for, oldest first:
 * not what Auto's reviewer took up. Throws `Unreached` when one asking can't be read. */
async function asksOf(target: Target, session: string, folder: string, signal?: AbortSignal, place: { forConversation?: boolean; client?: VaultClient; liveOnly?: boolean } = {}): Promise<Found[]> {
  const wait = () => (signal ? AbortSignal.any([signal, AbortSignal.timeout(5_000)]) : AbortSignal.timeout(5_000));
  const reviewed = new Set(listAskReviews().filter(review => review.state !== "yours").map(review => review.id));
  const noted = askTimes();
  const home = homeFolder();
  const found: { at: number; ask: Found }[] = [];
  const timeOf = (id: string, given: unknown) => noted.get(id) ?? (typeof given === "number" ? given : 0);
  // (A subagent listing that fails: the place isn't answering, so what was said last stands.)
  const askers = await askersOf(target, session, undefined, { wait }).catch(() => { throw new Unreached("The engine isn't answering."); });
  for (const asker of askers) {
    const at = `/api/session/${encodeURIComponent(asker)}`;
    let permissions: unknown, forms: unknown;
    try {
      [permissions, forms] = await Promise.all([call(target, "GET", `${at}/permission${place.liveOnly ? "?live=1" : ""}`, undefined, wait()), call(target, "GET", `${at}/form`, undefined, wait())]);
    } catch (error) {
      // (A subagent gone meanwhile asks nothing.)
      if ((error as { status?: number }).status === 404 && asker !== session) continue;
      throw new Unreached("The engine isn't answering.");
    }
    for (const request of (Array.isArray(permissions) ? permissions : []) as EnginePermission[]) {
      if (!isObject(request) || typeof request.id !== "string" || reviewed.has(request.id)) continue;
      const action = String(request.action ?? "");
      const source = isObject(request.source) ? request.source : null;
      const seen = (action === "shell" || action === "bash") && typeof source?.id === "string"
        ? await callOf(target, { asker, call: source.id, ...(typeof source.messageID === "string" ? { message: source.messageID } : {}) }, signal).catch(() => null)
        : null;
      const command = typeof seen?.input.command === "string" ? seen.input.command : null;
      found.push({ at: timeOf(request.id, request.time?.created), ask: { shown: permissionAsk(request, asker, folder, command, home, place.forConversation), target, asker } });
    }
    for (const form of (Array.isArray(forms) ? forms : []) as (EngineForm & { fields?: unknown; time?: { created?: unknown } })[]) {
      if (!isObject(form) || typeof form.id !== "string") continue;
      const fields = ((Array.isArray(form.fields) ? form.fields : []) as unknown[]).filter(isObject) as Field[];
      const questions = questionsOf(form);
      const title = text(form.title) || "Question";
      const shown: SessionAsk = questions
        ? { id: form.id, kind: "question", session: asker, title, questions }
        : { id: form.id, kind: "other", session: asker, title, message: describeForm(form as EngineForm), dismiss: true };
      found.push({ at: timeOf(form.id, form.time?.created), ask: { shown, target, asker, form: { fields } } });
    }
  }
  // What MCP servers ask in its folder (filed under no session): one is the conversation's whose
  // step runs that server's tool, decided once for the folder as its notification decides it.
  const shared = folder ? await call(target, "GET", "/api/session/global/form", undefined, wait(), folder).catch(() => { throw new Unreached("The engine isn't answering."); }) : null;
  const get = (route: string) => call(target, "GET", `/${route}`, undefined, wait()).then(data => ({ data }));
  for (const form of (Array.isArray(shared) ? shared : []) as (EngineForm & { time?: { created?: unknown } })[]) {
    const server = isObject(form) && isObject(form.metadata) ? text((form.metadata as Record<string, unknown>).server) : null;
    if (!isObject(form) || typeof form.id !== "string" || !server) continue;
    const owner = (await sessionsWaitingOnServer(server, folder, get))[0];
    if (!owner || !askers.includes(owner)) continue;
    const shown: SessionAsk = { id: form.id, kind: "other", session, title: text(form.title) || server, message: describeForm(form), dismiss: true };
    found.push({ at: timeOf(form.id, form.time?.created), ask: { shown, target, asker: "global", folder } });
  }
  // What the box itself asks you for it: a command outside the sandbox, something from the vault
  // (kept by its long-running process: this one, asked here or through `box route`).
  const asking = new Set(askers);
  for (const asked of listSandboxAsks()) {
    if (!asked.session || !asking.has(asked.session)) continue;
    found.push({ at: asked.at, ask: { shown: sandboxAskShown({ ...asked, session: asked.session }), target, asker: asked.session } });
  }
  const vault = listVaultAsks().filter(asked => asked.session && asking.has(asked.session));
  // (This Mac's vault read as its app last wrote it, once a read.)
  if (vault.length && place.client && onHost()) await primeGenerations().catch(() => {});
  for (const asked of vault) {
    const givable = await givableTo(place.client ?? null, asked.kind === "item" ? asked.site : undefined);
    found.push({ at: asked.at, ask: { shown: vaultAskShown({ ...asked, session: asked.session! }, givable), target, asker: asked.session! } });
  }
  // Those a client answers first, oldest first; then those answered elsewhere (`other`, a vault ask
  // it may not answer), so a card a client can answer shows ahead of one it can't. (Stable: what came at the same time, or
  // unnoted, stays in the order it was listed.)
  const elsewhere = (ask: Found) => (ask.shown.kind === "other" || (ask.shown.kind === "vault" && !ask.shown.choices.length) ? 1 : 0);
  return found.sort((a, b) => elsewhere(a.ask) - elsewhere(b.ask) || a.at - b.at).map(each => each.ask);
}

/** The session here, where it runs and its folder; null for none. Throws `Unreached`. */
async function placeOf(context: RouteContext, session: string) {
  const targets = targetsOf({ readEngine: context.readEngine, bridge: context.bridge });
  if (!targets.length) throw new Unreached("The engine isn't running.");
  const here = await sessionHere(targets, session, context.signal ?? undefined);
  if (!here) return null;
  const data: EngineSession = here.data;
  // (A coding agent's conversation, on their bridge: Always lasts for it.)
  const bridge = context.bridge();
  return { target: here.target, folder: data.location?.directory ?? data.directory ?? "", forConversation: !!bridge && bridge.port === here.target.port };
}

const NONE = { message: "There's no such conversation here." };
const GONE = { message: "It isn't waiting any more." };

/** The long-running process's answer, or null where none answered it: nothing listens, or one from
 * before it answered these routes (`404 {}`, until its keeper starts it again), so it's read here. */
function heard(held: Awaited<ReturnType<typeof askHolder>>): Awaited<ReturnType<typeof askHolder>> {
  if (!held) return null;
  if (held.status === 404 && typeof (parsed(Buffer.from(held.body, "base64").toString("utf8")) as { message?: unknown } | null)?.message !== "string") return null;
  return held;
}

/** A client's answer to a question, checked against what it asks, with the defaults of what it
 * didn't answer (its hidden fields' too), as the engine takes it; or why it doesn't fit. */
export function questionAnswer(fields: Field[], questions: AskedQuestion[], given: unknown): { answer: Record<string, unknown> } | { refused: string } {
  if (!isObject(given)) return { refused: "Send the answers, by each question's key." };
  const answer: Record<string, unknown> = {};
  for (const field of fields) if (typeof field.key === "string" && field.default !== undefined) answer[field.key] = field.default;
  for (const [key, value] of Object.entries(given)) {
    const question = questions.find(each => each.key === key);
    if (!question) return { refused: `It doesn't ask "${key}".` };
    const picked = question.multiple ? (typeof value === "string" ? [value] : value) : value;
    const each = question.multiple ? picked : [picked];
    if (!Array.isArray(each) || !each.every(one => typeof one === "string")) return { refused: `Answer "${question.title ?? key}" with ${question.multiple ? "a list of words" : "words"}.` };
    if (!question.custom && question.options.length && !each.every(one => question.options.some(option => option.value === one))) return { refused: `Pick one of the choices for "${question.title ?? key}".` };
    answer[key] = picked;
  }
  for (const question of questions) {
    if (!question.required) continue;
    const value = answer[question.key];
    const answered = Array.isArray(value) ? value.length > 0 : typeof value === "string" ? value.trim() !== "" : value !== undefined;
    if (!answered) return { refused: `Answer "${question.title ?? question.question}": it needs an answer.` };
  }
  return { answer };
}

// `GET kuvo/sessions/<id>/asks`, `…/asks/events`, and `POST …/asks/<ask>` to answer one.
export async function* sessionAsksRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  const method = request.method.toUpperCase();
  const session = decodeURIComponent(match[1]!);
  const ask = match[2] ? decodeURIComponent(match[2]) : null;
  const events = ask === "events" && method === "GET";
  if (ask && !events ? method !== "POST" : method !== "GET") return yield* json(405, {});
  if (method === "POST") {
    signal?.throwIfAborted();
    const refusal = liveMutationRefusal("consent", context, session);
    if (refusal) return yield* json(refusal.status, refusal.value);
  }
  // The box's own asks wait in its long-running process (vault/holder.ts): another (`box route`, a
  // machine's kept server) has it read and answer them. With none listening, there are none, and
  // what the engine asks is read here.
  if (!events && !holdsVault()) {
    const consent = method === "POST" ? relayedConsent(context) : undefined;
    const held = heard(await askHolder(request, signal, consent));
    if (held) return yield* [{ head: { status: held.status, headers: { "content-type": held.type || "application/json" } } }, { body: Buffer.from(held.body, "base64") }];
  }
  let place: Awaited<ReturnType<typeof placeOf>>;
  try {
    place = await placeOf(context, session);
  } catch (error) {
    if (error instanceof Unreached) return yield* json(503, { message: error.message });
    throw error;
  }
  if (!place) return yield* json(404, NONE);
  const { target, folder } = place;
  const client = clientOf({ request, phone: context.phone });
  const read = async () => ({ asks: (await asksOf(target, session, folder, signal ?? undefined, { forConversation: place.forConversation, client })).map(each => each.shown) });
  // Followed, where it runs is found again each time (its engine started again on another port);
  // in a process that doesn't keep the box's own asks, read by the one that does.
  const readAgain = async (): Promise<{ asks: SessionAsk[] } | undefined> => {
    if (!holdsVault()) {
      const held = heard(await askHolder({ ...request, method: "GET", path: `kuvo/sessions/${encodeURIComponent(session)}/asks`, body: Buffer.alloc(0) }, signal ?? undefined));
      if (held?.status === 200) return parsed(Buffer.from(held.body, "base64").toString("utf8")) as { asks: SessionAsk[] };
      if (held?.status === 404) return { asks: [] };
      if (held) return undefined;
    }
    const now = await placeOf(context, session);
    return now ? { asks: (await asksOf(now.target, session, now.folder, signal ?? undefined, { forConversation: now.forConversation, client })).map(each => each.shown) } : { asks: [] };
  };
  if (!ask) {
    try {
      return yield* json(200, await read());
    } catch (error) {
      if (error instanceof Unreached) return yield* json(503, { message: error.message });
      throw error;
    }
  }
  if (events) {
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    // (A place not answering: what was said last stands.)
    // Told as an ask comes or goes in this process, and a look each second at the asks another
    // process noted (a machine's kept server isn't the one hearing its engine).
    const look = () => { try { return String(fs.statSync(ASKS_FILE).mtimeMs); } catch { return ""; } };
    return yield* followedLive(() => readAgain().catch(error => { if (error instanceof Unreached) return undefined; throw error; }), told => {
      askFollowers.add(told);
      return () => { askFollowers.delete(told); };
    }, signal ?? undefined, { look });
  }
  let found: Found | undefined;
  try {
    found = (await asksOf(target, session, folder, signal, { forConversation: place.forConversation, client, liveOnly: true })).find(each => each.shown.id === ask);
  } catch (error) {
    if (error instanceof Unreached) return yield* json(503, { message: error.message });
    throw error;
  }
  if (!found) return yield* json(404, GONE);
  signal?.throwIfAborted();
  const refusal = liveMutationRefusal("consent", context, session, found.asker);
  if (refusal) return yield* json(refusal.status, refusal.value);
  const given = parsed(request.body);
  // What the box asks itself is answered here, by one of the answers its card offers (the item it
  // gives with it): never put away unanswered.
  if (found.shown.kind === "sandbox" || found.shown.kind === "vault") {
    // The vault answers only its clients, as `kuvo/vault/asks` does.
    if (found.shown.kind === "vault" && !client) return yield* json(403, { message: NOT_A_CLIENT });
    const answer = isObject(given) ? given.answer : undefined;
    const item = isObject(given) && typeof given.item === "string" ? given.item : undefined;
    const picked = found.shown.choices.find(each => each.answer === answer && each.item === item);
    if (!picked || picked.answer === "instead") {
      // (An item it no longer offers: the vault locked meanwhile, or the item's gone.)
      if (found.shown.kind === "vault" && item && isLocked()) return yield* json(423, { message: "This computer’s vault is locked: unlock it to give it an item." });
      if (found.shown.kind === "vault" && item && !itemByID(item)) return yield* json(404, { message: "That item isn't in this vault." });
      const offered = [...new Set(found.shown.choices.map(each => each.answer))].join(", ");
      return yield* json(400, { message: found.shown.kind === "vault" && found.shown.need === "item" && !item && answer !== "reject" ? "Say which item to give it." : `Answer ${offered}.` });
    }
    const as = picked.answer === "reject" ? "no" : picked.answer;
    if (found.shown.kind === "sandbox") {
      return answerSandboxAsk(found.shown.id, { answer: as, client: clientName(context) }, () => !liveMutationRefusal("consent", context, session, found.asker)) ? yield* json(200, {}) : yield* json(404, GONE);
    }
    return answerVaultAsk(found.shown.id, { answer: as, ...(picked.item ? { item: picked.item } : {}), client: client!.name, by: client!.by }) ? yield* json(200, {}) : yield* json(404, GONE);
  }
  const at = `/api/session/${encodeURIComponent(found.asker)}`;
  let sent: { method: string; route: string; body?: unknown };
  if (found.shown.kind === "permission") {
    const answer = isObject(given) ? given.answer : undefined;
    if (answer === "once" || answer === "always" || answer === "reject") sent = { method: "POST", route: `${at}/permission/${encodeURIComponent(found.shown.id)}/reply`, body: { decision: answer } };
    else if (answer === "instead") {
      const words = isObject(given) && typeof given.message === "string" ? given.message.trim() : "";
      if (!words) return yield* json(400, { message: "Say what it should do instead." });
      sent = { method: "POST", route: `${at}/permission/${encodeURIComponent(found.shown.id)}/reply`, body: { decision: "reject", message: words } };
    } else return yield* json(400, { message: "Answer once, always, reject or instead." });
  } else if (isObject(given) && given.dismiss === true && (found.shown.kind !== "other" || found.shown.dismiss)) {
    sent = { method: "DELETE", route: `${at}/form/${encodeURIComponent(found.shown.id)}` };
  } else if (found.shown.kind === "question") {
    const checked = questionAnswer(found.form?.fields ?? [], found.shown.questions as AskedQuestion[], isObject(given) ? given.answers : undefined);
    if ("refused" in checked) return yield* json(400, { message: checked.refused });
    sent = { method: "POST", route: `${at}/form/${encodeURIComponent(found.shown.id)}/reply`, body: { answer: checked.answer } };
  } else {
    return yield* json(400, { message: "This one is answered where it was asked: only dismissing it goes from here." });
  }
  try {
    const permission = found.shown.kind === "permission" && isObject(sent.body) && typeof sent.body.decision === "string"
      ? { target: found.target, route: `${at}/permission`, id: found.shown.id, decision: sent.body.decision, folder: found.folder }
      : undefined;
    const send = () => dispatchConsent(context, [session, found.asker], signal, permission, () => {
      const deadline = AbortSignal.timeout(15_000);
      return call(found.target, sent.method, sent.route, sent.body, signal ? AbortSignal.any([signal, deadline]) : deadline, found.folder);
    });
    const reply = await admitPrompt(session, () => found.asker !== session && found.asker !== "global" ? admitPrompt(found.asker, send) : send());
    if ("refusal" in reply) return yield* json(reply.refusal.status, reply.refusal.value);
  } catch (error) {
    if (signal?.aborted) throw error;
    if (error instanceof MoveAdmissionClosed) return yield* json(409, { message: error.message });
    const status = (error as { status?: number }).status;
    if (status === 404) return yield* json(404, GONE);
    if (status === undefined) return yield* json(503, { message: "The engine isn't answering." });
    if (status === 403 || status === 503) return yield* json(status, { message: String((error as Error).message) });
    return yield* json(502, { message: String((error as Error).message) });
  }
  return yield* json(200, {});
}
