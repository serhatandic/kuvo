// `kuvo/settings`: this box's settings as it describes them (PROTOCOL.md, "A box's settings"), so
// every client draws each of them by its kind, with the box's words: its sections, each section's
// page, a change to one row (checked against that row's own revision, so two rows never conflict),
// what a row offers to do, and the page again each time it changes.
//
// Each page is made from the box's existing routes, and each change is made through them, as a
// client of them would (`kuvo/approval`, `kuvo/sandbox…`, `kuvo/config`, `kuvo/browser/settings`):
// what they check, keep and do follows, the same for every client.
// Platform: the same on a Mac's box and a machine's. What only opens on the box's own computer (a
// Mac's Software Update) is offered only to a client there (`fromApp`: the app beside its box,
// over its own server), and said plainly elsewhere.

import crypto from "node:crypto";
import fs from "node:fs";
import { isObject, parsed, sortedJSON } from "../../shared/json";
import type { SettingAction, SettingCan, SettingGroup, SettingItem, SettingRow, SettingsPage, SettingsSection } from "../../shared/routes/settings-pages";
import { SEARCH_ENGINES } from "../../shared/routes/browser";
import { approvalRoute } from "./approval";
import { CHANGED_SINCE, CONFIG_FILE } from "./config";
import { engineControl } from "./engine";
import { sandboxRoute } from "./sandbox";
import { settingsRoute } from "./settings";
import { BROWSER_DATA_ROUTE, browserDataRoute } from "../browser/routes";
import { browserSettingsFiles, offerAgain } from "../browser/settings";
import { ALLOWED_FILE } from "../fence/allowed";
import { fenceState, SOFTWARE_UPDATE } from "../fence/state";
import { followedLive } from "../follow";
import { APPROVAL_FILE } from "../work/approval";
import { IS_MAC } from "../home";
import { boxAnswers } from "../mac/keeper-runs";
import { askHolder, holdsVault } from "../vault/holder";
import type { AnswerPiece, RouteContext } from "../server/context";

export const SETTINGS_PAGE_ROUTE = /^kuvo\/settings(?:\/([^/]+)(?:\/(events)|\/([^/]+)\/([^/]+))?)?$/;

const SECTIONS: SettingsSection[] = [
  {
    id: "permissions", title: "Permissions", group: "Agents & Tools", icon: "shield",
    line: "What agents may do without asking, and the sandbox their commands run in.",
    keywords: ["approval", "full access", "auto", "ask first", "allow", "don't allow", "sandbox", "also allowed", "other computers", "rules"],
  },
  {
    id: "browser", title: "Browser", group: "System", icon: "globe",
    line: "The browser's search engine, and offering to save sign-ins.",
    keywords: ["search engine", "google", "duckduckgo", "sign-ins", "passwords", "tabs", "never offered"],
  },
];

// MARK: Rows

/** A row's revision: a hash of what it is and what governs whether it can be changed (its value,
 * `can`, its choices' and actions' ids, a list's items' revisions), never its words, so a computer
 * renamed or a line reworded changes none. A `secret`'s, of whether one is set (it has no value). */
function revised<Row extends Omit<SettingItem, "revision"> & { items?: readonly SettingItem[] }>(row: Row): Row & { revision: string } {
  // (Whether it can be, not why not: a why names the computer.)
  const can = (value: SettingCan | undefined) => (value === undefined ? undefined : value === true);
  const action = (each: SettingAction | undefined) => each && { id: each.id, can: can(each.can), opens: each.opens };
  const governs = {
    kind: row.kind, value: row.kind === "secret" ? undefined : row.value, can: can(row.can), set: row.set,
    choices: row.choices?.map(choice => choice.id), actions: row.actions?.map(action),
    start: action(row.start), section: row.section, address: row.address, name: row.name,
    items: row.items?.map(each => each.revision), add: action((row as { add?: SettingAction }).add),
  };
  const revision = crypto.createHash("sha256").update(sortedJSON(governs)).digest("base64url").slice(0, 16);
  return { ...row, revision };
}

const item = (row: Omit<SettingItem, "revision">): SettingItem => revised(row);
const row = (fields: Omit<SettingRow, "revision">): SettingRow => revised(fields);

/** Each row on a page, a list's items too. */
const rowsOf = (page: SettingsPage): SettingItem[] => page.groups.flatMap(group => group.rows.flatMap(each => [each, ...(each.items ?? [])]));

// MARK: The routes a page is made from

/** What one of the box's own routes answers, asked as this request's client asks (the same `fromApp`,
 * the same phone): its status and its JSON. */
async function asked(context: RouteContext, answer: (context: RouteContext) => AsyncIterable<AnswerPiece>, method: string, path: string, body?: unknown): Promise<{ status: number; value: any }> {
  const request = {
    method, path, headers: {},
    ...(body === undefined ? {} : { body: Buffer.from(JSON.stringify(body)) }),
    ...(context.request.fromApp ? { fromApp: true } : {}),
    ...(context.request.fromMac ? { fromMac: true } : {}),
  };
  let status = 500;
  const chunks: Buffer[] = [];
  for await (const piece of answer({ ...context, request, route: path.split("?")[0]! })) {
    if ("head" in piece) status = piece.head.status;
    if ("body" in piece) chunks.push(Buffer.from(piece.body));
  }
  let value: any = {};
  try { value = JSON.parse(Buffer.concat(chunks).toString() || "{}"); } catch {}
  return { status, value };
}

const browserAnswer = (context: RouteContext) => browserDataRoute(context, context.route.match(BROWSER_DATA_ROUTE)!);

/** Why one of them didn't answer, in words. */
const said = (answer: { status: number; value: any }, fallback: string): string =>
  typeof answer.value?.message === "string" && answer.value.message ? answer.value.message : fallback;

/** A change or an action refused, in words: its status, and why; `stale` where the row changed since
 * it was read (answered with the page now). */
type Refused = { status: number; message: string; stale?: true };
const refusal = (answer: { status: number; value: any }): Refused | null =>
  answer.status >= 200 && answer.status < 300 ? null : { status: answer.status, message: said(answer, `It couldn't be changed (${answer.status}).`) };
const STALE: Refused = { status: 409, message: CHANGED_SINCE, stale: true };

// MARK: Permissions

const MODE_NAMES: Record<string, string> = { askFirst: "Ask First", acceptEdits: "Accept Edits", auto: "Auto", fullAccess: "Full Access" };

/** The kinds of action Settings › Permissions has a choice for, each a rule for everything ("*") at
 * the front of the engine's `permissions`, as the Mac's Settings writes them; and what the engine
 * does with none. */
const ACTIONS = [
  { id: "edit", title: "Edit files", line: "Changing, creating or deleting files. Plan mode doesn't, by default." },
  { id: "shell", title: "Run commands", line: "In the terminal." },
  { id: "read", title: "Read files", line: "Files ending in .env ask first unless you allow this." },
  { id: "external_directory", title: "Work outside the project", line: "In folders that aren't the project's." },
  { id: "webfetch", title: "Open web pages" },
  { id: "websearch", title: "Search the web" },
  { id: "subagent", title: "Hand work to subagents" },
  { id: "skill", title: "Use skills" },
] as const;
const EFFECTS = [{ id: "allow", title: "Allow" }, { id: "ask", title: "Ask First" }, { id: "deny", title: "Don't Allow" }] as const;
const engineDefault = (action: string) => (action === "external_directory" ? "ask" : "allow");

const NOT_A_LIST = "The engine's permissions here aren't a list of rules: change them in its settings file.";

type Rule = { action: string; resource: string; effect: string };
const isRule = (value: unknown): value is Rule => isObject(value) && typeof value.action === "string" && typeof value.resource === "string" && typeof value.effect === "string";
/** The choice for `action` in the engine's `permissions`: the last rule for everything, as the engine reads them. */
const choiceIn = (rules: unknown[], action: string) => rules.filter(isRule).findLast(rule => rule.action === action && rule.resource === "*")?.effect;

async function permissionsPage(context: RouteContext): Promise<SettingsPage> {
  const name = context.info.name || "this computer";
  const here = context.request.fromApp === true;
  const [approval, sandbox, allowed, config] = [
    await asked(context, approvalRoute, "GET", "kuvo/approval"),
    await asked(context, sandboxRoute, "GET", "kuvo/sandbox"),
    await asked(context, sandboxRoute, "GET", "kuvo/sandbox/allowed"),
    await asked(context, settingsRoute, "GET", "kuvo/config"),
  ];
  const groups: SettingGroup[] = [];
  const notes: { words: string; tone: "quiet" | "warning" }[] = [];

  if (approval.status === 200) {
    const { fullAccess, default: mode, auto, otherComputers } = approval.value;
    const rows: SettingRow[] = [
      row({ id: "default-mode", title: "New sessions start in", line: "The mode you picked last on this computer. Change it in a conversation's mode menu.", kind: "status", value: MODE_NAMES[mode] ?? String(mode) }),
      row({
        id: "full-access", title: "Full Access", kind: "switch", value: fullAccess === true,
        line: fullAccess ? "Conversations set to Full Access work here without asking." : "Asked the first time you pick it in a conversation on this computer.",
        can: fullAccess ? true : { why: "Turn it on in a conversation: pick Full Access in its mode menu, and confirm it there." },
        ...(fullAccess ? { confirm: { title: "Turn Off Full Access?", body: `Conversations on ${name} in Full Access go on in Auto. You're asked again the next time you pick it.`, button: "Turn Off" } } : {}),
      }),
    ];
    for (const [side, words] of [["engine", "the engine"], ["agents", "the coding agents"]] as const) {
      const held = auto?.[side]?.held;
      if (held) rows.push(row({ id: `auto-held-${side}`, title: `Auto, for ${words}`, line: held.why ?? held.reason, tone: "warning", kind: "status", value: "Held" }));
    }
    if (typeof otherComputers === "boolean") {
      rows.push(row({ id: "other-computers", title: "Let your other computers start work here", line: `Their bots and agents can start sessions on ${name}.`, kind: "switch", value: otherComputers, can: true }));
    }
    groups.push({ id: "approval", title: "Approval", rows });
  } else {
    notes.push({ words: `Approval couldn't be read: ${said(approval, "it didn't answer.")}`, tone: "warning" });
  }

  if (config.status === 200) {
    const rules = Array.isArray(config.value.permissions) ? config.value.permissions : [];
    // (Written by hand as something else than a list of rules: left as it is, shown as it is not
    // known here, and said.)
    const listed = config.value.permissions === undefined || Array.isArray(config.value.permissions);
    const can = listed ? true as const : { why: NOT_A_LIST };
    const rows = ACTIONS.map(action => {
      const chosen = choiceIn(rules, action.id);
      // What it does with no rule of yours is marked: picking that takes your rule away, so the
      // engine's own rules (Plan mode doesn't edit) stay.
      // A rule of yours that says what applies anyway still counts in every mode (Plan mode edits
      // under an Allow of yours): shown apart, with the default to take it away.
      const ownDefault = chosen === engineDefault(action.id);
      const choices = [
        ...EFFECTS.map(effect => effect.id !== engineDefault(action.id) ? effect
          : ownDefault ? { ...effect, title: `${effect.title} (Your Rule)`, line: "Applies in every mode, Plan mode too." }
          : { ...effect, title: `${effect.title} (Default)` }),
        // (One a newer engine set, shown as it is, and kept unless another is picked.)
        ...(chosen && !EFFECTS.some(effect => effect.id === chosen) ? [{ id: chosen, title: "Set by a Newer Engine" }] : []),
        ...(ownDefault ? [{ id: "default", title: `${EFFECTS.find(effect => effect.id === chosen)!.title} (Default)` }] : []),
      ];
      if (!listed) return row({ id: `when-${action.id}`, title: action.title, ...("line" in action ? { line: action.line } : {}), kind: "status", value: "Set in Its Settings File", can });
      return row({ id: `when-${action.id}`, title: action.title, ...("line" in action ? { line: action.line } : {}), kind: "choice", value: chosen ?? engineDefault(action.id), choices, can });
    });
    groups.push({ id: "when", title: "When the Agent Wants To", note: listed ? "For every session and mode. Don't Allow holds even in Full Access. Codex, Cursor, Grok and Antigravity may read files without asking." : NOT_A_LIST, rows });
  } else {
    notes.push({ words: said(config, "The engine's settings couldn't be read."), tone: "warning" });
  }

  if (sandbox.status === 200) {
    groups.push({ id: "sandbox", title: "Sandbox", rows: [row(sandboxRow(sandbox.value, name, here))] });
  } else {
    notes.push({ words: `The sandbox couldn't be read: ${said(sandbox, "it didn't answer.")}`, tone: "warning" });
  }

  if (allowed.status === 200) {
    const items = (Array.isArray(allowed.value.allowed) ? allowed.value.allowed : []).map((rule: any) => {
      const whose = rule.owner?.name ? ` · for ${rule.owner.name}` : "";
      return item({
        id: `allowed-${rule.id}`, title: rule.path, line: rule.gone ? `Gone from this computer${whose}` : `Can write${whose}`,
        kind: "actions", actions: [{ id: "remove", title: "Remove" }],
      });
    });
    groups.push({
      id: "allowed", rows: [row({
        id: "allowed", title: "Also Allowed", line: "Folders agents' commands may write in besides their own. Choosing Always on a stopped command's card adds one.",
        kind: "list", items, empty: "Nothing else is allowed yet.",
      })],
    });
  } else {
    notes.push({ words: `What's also allowed couldn't be read: ${said(allowed, "it didn't answer.")}`, tone: "warning" });
  }

  notes.push({ words: "Rules for particular commands or files, approvals saved with Always, and blocks aren't on this page yet.", tone: "quiet" });
  return { id: "permissions", title: "Permissions", groups, notes };
}

const CHECK = "POST kuvo/sandbox/check";
const RESTART = "POST kuvo/engine/restart";

/** Whether this box restarts its engine itself: a machine's always; a Mac's while its keeper runs
 * it (`boxAnswers`), else the app that runs it does. */
const restartsItself = () => !IS_MAC || boxAnswers("kuvo/engine/restart");

/** The sandbox's row, as `kuvo/sandbox` says it: a quiet line, or while it needs you (`needsYou`),
 * why (a warning) and its button. */
export function sandboxRow(state: any, name: string, here: boolean) {
  const needsYou = state.needsYou === true;
  const action = needsYou ? sandboxAction(state, name, here) : null;
  return {
    id: "sandbox", title: "Sandbox", line: needsYou ? String(state.why ?? "") : "Agents' commands here run in it.",
    ...(needsYou ? { tone: "warning" as const } : {}),
    kind: action ? "actions" as const : "status" as const, value: state.on ? "On" : "Off",
    ...(action ? { actions: [action] } : {}),
  };
}

/** The sandbox's button while it needs you, as the box names it (`kuvo/sandbox`'s `action`): one that
 * runs a route (checking again, restarting the engine), one that opens a page, or this computer's
 * own Software Update, only from a client on it. */
export function sandboxAction(state: any, name: string, here: boolean, restarts = restartsItself()): SettingAction | null {
  if (!state.action || !state.actionLabel) return null;
  const title = String(state.actionLabel);
  if (state.route === RESTART && !restarts) return { id: String(state.action), title, can: { why: `Kuvo Code on ${name} restarts its engine: restart it there.` } };
  if (state.route) {
    const confirm = state.action === "restartEngine" && title.endsWith("…")
      ? { confirm: { title: `Restart ${name}'s engine?`, body: "What agents are doing there now stops. Commands run in the sandbox once it's back.", button: "Restart Engine" } }
      : {};
    return { id: String(state.action), title, ...confirm };
  }
  if (typeof state.opens !== "string") return null;
  if (state.opensHere && state.opens === SOFTWARE_UPDATE && !here) return { id: String(state.action), title, can: { why: `Open Software Update on ${name} itself.` } };
  return { id: String(state.action), title, opens: state.opens };
}

async function permissionsChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  if (at.id === "full-access") {
    if (value !== false) return { status: 400, message: "Full Access is only turned off here." };
    return refusal(await asked(context, approvalRoute, "PATCH", "kuvo/approval", { fullAccess: false }));
  }
  if (at.id === "other-computers") return refusal(await asked(context, approvalRoute, "PATCH", "kuvo/approval", { otherComputers: value }));
  const action = at.id.startsWith("when-") ? at.id.slice(5) : null;
  if (action) {
    // Only this one's rule; the rest, those written by hand too, stay as they are. Written from
    // what's there just before (`was`): one changed meanwhile is read again, and this one's put to it.
    for (let tries = 0; tries < 3; tries++) {
      const read = await asked(context, settingsRoute, "GET", "kuvo/config");
      if (read.status !== 200) return refusal(read);
      const was = read.value.permissions ?? null;
      if (was !== null && !Array.isArray(was)) return { status: 403, message: NOT_A_LIST };
      // (This one changed since it was read, by hand or another client: refused, the page now.)
      if ((choiceIn(was ?? [], action) ?? engineDefault(action)) !== at.value) return STALE;
      // Picking what applies already writes nothing (a rule of yours that says it stays); the
      // default takes yours away, never writing one for it.
      if (value === at.value) return null;
      const items = (Array.isArray(was) ? was : []).filter((each: unknown) => !(isRule(each) && each.action === action && each.resource === "*"));
      if (value !== "default" && value !== engineDefault(action)) items.unshift({ action, resource: "*", effect: value });
      if (sortedJSON(items) === sortedJSON(was ?? [])) return null;
      const written = await asked(context, settingsRoute, "PATCH", "kuvo/config", { changes: [{ path: ["permissions"], value: items.length ? items : null, was }] });
      if (written.status === 409 && written.value?.problem === undefined && written.value?.message === CHANGED_SINCE) continue;
      return refusal(written);
    }
    return { ...STALE, message: "The engine's settings keep changing: try again in a moment." };
  }
  return { status: 400, message: "That isn't something to change." };
}

async function permissionsAct(context: RouteContext, at: SettingItem, action: SettingAction): Promise<Refused | null> {
  if (at.id === "sandbox") {
    // As the sandbox names it now (the row's revision says it's the one that was read).
    const state = await asked(context, sandboxRoute, "GET", "kuvo/sandbox");
    if (state.status !== 200) return refusal(state);
    if (String(state.value.action) !== action.id) return STALE;
    return sandboxDoes(context, state.value.route);
  }
  if (at.id.startsWith("allowed-") && action.id === "remove") {
    return refusal(await asked(context, sandboxRoute, "DELETE", `kuvo/sandbox/allowed/${encodeURIComponent(at.id.slice(8))}?all`));
  }
  return { status: 404, message: "That isn't something it does." };
}

/** Runs the sandbox's button's route: checking it again, or restarting the engine where this box
 * does that itself (begun: `202`, done). */
export async function sandboxDoes(context: RouteContext, route: unknown, restarts = restartsItself()): Promise<Refused | null> {
  if (route === CHECK) return refusal(await asked(context, sandboxRoute, "POST", "kuvo/sandbox/check", {}));
  if (route === RESTART) {
    if (!restarts) return { status: 403, message: `Kuvo Code on ${context.info.name || "this Mac"} restarts its engine: restart it there.` };
    const { status, message } = await engineControl("restart");
    return refusal({ status, value: message ? { message } : {} });
  }
  return { status: 404, message: "That isn't something it does." };
}

// MARK: Browser

async function browserPage(context: RouteContext): Promise<SettingsPage> {
  const read = await asked(context, browserAnswer, "GET", "kuvo/browser/settings");
  if (read.status !== 200) {
    return { id: "browser", title: "Browser", groups: [], notes: [{ words: `The browser's settings couldn't be read: ${said(read, "they didn't answer.")}`, tone: "warning" }] };
  }
  const settings = read.value;
  const engines: { id: string; title: string }[] = Array.isArray(settings.searchEngines) ? settings.searchEngines : SEARCH_ENGINES.map(({ id, title }) => ({ id, title }));
  const never: string[] = Array.isArray(settings.never) ? settings.never : [];
  return {
    id: "browser", title: "Browser",
    groups: [
      {
        id: "browser", rows: [
          row({ id: "search-engine", title: "Search engine", line: "For anything typed in the address field that isn't an address.", kind: "choice", value: settings.searchEngine, choices: engines.map(({ id, title }) => ({ id, title })), can: true }),
          row({ id: "tidies", title: "Close agents' tabs you haven't looked at", line: "An hour after an agent last used one, so tabs don't pile up.", kind: "switch", value: settings.tidiesAgentTabs === true, can: true }),
          row({ id: "offers", title: "Offer to save sign-ins", line: "When you sign in to a site in the browser.", kind: "switch", value: settings.offersToSave === true, can: true }),
        ],
      },
      {
        id: "never", rows: [row({
          id: "never", title: "Never Offered", line: "Sites you said Never to when it offered to save a sign-in.", kind: "list", empty: "None yet.",
          items: never.map(host => item({ id: `never-${host}`, title: host, kind: "actions", actions: [{ id: "offer-again", title: "Offer Again" }] })),
        })],
      },
    ],
  };
}

const BROWSER_FIELDS: Record<string, string> = { "search-engine": "searchEngine", tidies: "tidiesAgentTabs", offers: "offersToSave" };

async function browserChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  const field = BROWSER_FIELDS[at.id];
  if (!field) return { status: 400, message: "That isn't something to change." };
  return refusal(await asked(context, browserAnswer, "PATCH", "kuvo/browser/settings", { [field]: value }));
}

async function browserAct(context: RouteContext, at: SettingItem, action: SettingAction): Promise<Refused | null> {
  if (!at.id.startsWith("never-") || action.id !== "offer-again") return { status: 404, message: "That isn't something it does." };
  // (That one alone: a site said Never to meanwhile stays.)
  offerAgain(at.id.slice(6));
  return null;
}

// MARK: Sections

interface Section {
  read: (context: RouteContext) => Promise<SettingsPage>;
  change: (context: RouteContext, at: SettingItem, value: unknown) => Promise<Refused | null>;
  act: (context: RouteContext, at: SettingItem, action: SettingAction) => Promise<Refused | null>;
  /** What's looked at each second for a change another process made (cheap). */
  look: () => string;
}

const mtime = (file: string): number => { try { return fs.statSync(file).mtimeMs; } catch { return 0; } };

const PAGES: Record<string, Section> = {
  permissions: {
    read: permissionsPage, change: permissionsChange, act: permissionsAct,
    look: () => {
      let fence = "";
      try { const { on, reason, action } = fenceState(); fence = `${on} ${reason ?? ""} ${action ?? ""}`; } catch {}
      return `${mtime(APPROVAL_FILE)} ${mtime(CONFIG_FILE)} ${mtime(ALLOWED_FILE)} ${fence}`;
    },
  },
  browser: { read: browserPage, change: browserChange, act: browserAct, look: () => browserSettingsFiles().map(mtime).join(" ") },
};

// Told as a change is made here, so a page followed shows it at once.
const followers = new Set<() => void>();
const changed = () => { for (const told of [...followers]) told(); };

/** Whether a value fits a row of its kind: a switch's true or false, one of a choice's choices. */
function fits(at: SettingItem, value: unknown): boolean {
  if (at.kind === "switch") return typeof value === "boolean";
  if (at.kind === "choice") return typeof value === "string" && (at.choices ?? []).some(choice => choice.id === value);
  return false;
}

// Each section's changes here, one after another (a change waits for the one before it to end).
const turns = new Map<string, Promise<unknown>>();
function inTurn<T>(key: string, work: () => Promise<T>): Promise<T> {
  const mine = (turns.get(key) ?? Promise.resolve()).then(work, work);
  const settled = mine.catch(() => {});
  turns.set(key, settled);
  void settled.then(() => { if (turns.get(key) === settled) turns.delete(key); });
  return mine;
}

const whyNot = (can: SettingCan | undefined): string | null => (can === undefined || can === true ? null : can.why);

const idOf = (part: string | undefined): string | null => {
  try { return decodeURIComponent(part ?? ""); } catch { return null; }
};

export async function* settingsPageRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, json, signal } = context;
  const method = request.method.toUpperCase();
  const [, sectionPart, events, rowPart, actionPart] = match;
  if (sectionPart === undefined) {
    if (method !== "GET") return yield* json(405, { message: "Read them with GET." });
    return yield* json(200, { sections: SECTIONS });
  }
  const id = idOf(sectionPart);
  const section = id && Object.hasOwn(PAGES, id) ? PAGES[id] : undefined;
  if (!section) return yield* json(404, { message: "This computer has no settings by that name." });

  if (events) {
    if (method !== "GET") return yield* json(405, { message: "Follow it with GET." });
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    const listen = (told: () => void) => { followers.add(told); return () => { followers.delete(told); }; };
    return yield* followedLive(() => section.read(context), listen, signal, { look: section.look });
  }

  if (method === "GET" && rowPart === undefined) return yield* json(200, await section.read(context));
  if (rowPart !== undefined ? method !== "POST" : method !== "PATCH") {
    return yield* json(405, { message: rowPart !== undefined ? "Do it with POST." : "Read it with GET, or change a row with PATCH." });
  }
  // Made by the box's long-running process, whichever is asked (`box route` over SSH passes it on),
  // so every change to a section is checked in turn against the page as it is then. With none
  // running (a machine with no phones, peers or vault, reached over SSH alone), here.
  if (!holdsVault()) {
    const answer = await askHolder(request, signal);
    if (answer) {
      // (A holder from before this, as the box updates, has no such route: said in words.)
      const body = answer.status === 404 && !/"message"/.test(Buffer.from(answer.body, "base64").toString())
        ? Buffer.from(JSON.stringify({ message: "This computer's box is updating: try again in a moment." }))
        : Buffer.from(answer.body, "base64");
      return yield* [{ head: { status: answer.status, headers: { "content-type": answer.type || "application/json" } } }, { body }];
    }
  }
  const body = parsed(request.body);
  const given = isObject(body) ? body : {};
  const rowId = rowPart !== undefined ? idOf(rowPart) : given.row;
  if (typeof rowId !== "string" || !rowId) return yield* json(400, { message: "Name the row to change (row)." });
  if (typeof given.was !== "string") return yield* json(400, { message: "Send the row's revision as it was read (was)." });
  // One change to a section at a time, each checked against the page as it is then: the row's own
  // revision, whether it can be changed from here, the value; then made.
  const { status, value } = await inTurn(id!, async (): Promise<{ status: number; value: unknown }> => {
    const page = await section.read(context);
    const at = rowsOf(page).find(each => each.id === rowId);
    if (!at) return { status: 404, value: { message: "That isn't on this page now." } };
    if (at.revision !== given.was) return { status: 409, value: { message: CHANGED_SINCE, page } };
    let done: Refused | null;
    if (rowPart !== undefined) {
      const action = (at.actions ?? []).find(each => each.id === idOf(actionPart));
      if (!action) return { status: 404, value: { message: "That isn't something it does." } };
      if (action.opens !== undefined) return { status: 400, value: { message: "That opens a page: open it in the app." } };
      const why = whyNot(action.can);
      if (why) return { status: 403, value: { message: why } };
      done = await section.act(context, at, action);
    } else {
      if (at.can === undefined) return { status: 400, value: { message: "That isn't something to change." } };
      const why = whyNot(at.can);
      if (why) return { status: 403, value: { message: why } };
      if (!("value" in given) || !fits(at, given.value)) return { status: 400, value: { message: at.kind === "choice" ? "Pick one of its choices." : "Send true or false." } };
      done = await section.change(context, at, given.value);
    }
    if (done?.stale) return { status: 409, value: { message: done.message, page: await section.read(context) } };
    if (done) return { status: done.status, value: { message: done.message } };
    changed();
    return { status: 200, value: await section.read(context) };
  });
  return yield* json(status, value);
}
