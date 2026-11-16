// Settings › Agents (`kuvo/settings/agents`, PROTOCOL.md "A box's settings"): this computer's
// agents, described as a flow section (a list, a form to add one and edit one, Reset and Delete), so
// every client draws it by kind. The agents are the engine's: its built-in ones (Build, Plan,
// General, Explore) and yours, each an entry in its settings' `agents` (read and written through
// `kuvo/config`, with `was`), which the engine reads as it is (`mode`, `description`, `model`,
// `disabled`, `system`, `permissions`; what else an entry has, `steps` say, is kept as it is). Its
// "From Claude Code" list is `kuvo/claude-code`'s (`claudeCodeList`): the subagents in this
// computer's own ~/.claude/agents, brought over as the Mac's Settings did.
// Platform: the same on a Mac's box and a machine's; each lists its own ~/.claude.

import path from "node:path";
import type { SettingAction, SettingField, SettingFieldWords, SettingForm, SettingGroup, SettingItem, SettingsChecked, SettingsPage, SettingsSection } from "../../shared/routes/settings-pages";
import { isObject } from "../../shared/json";
import type { Target } from "../../shared/records";
import { call } from "../engine/client";
import { homeFolder } from "../home";
import { usableModels } from "../work/models";
import type { RouteContext } from "../server/context";
import { claudeCodeList } from "./claude-code";
import { CHANGED_SINCE, CONFIG_FILE } from "./config";
import { ACTIONS, EFFECTS, choiceIn, engineDefault, isChoice, isRule } from "./permission-rules";
import type { Rule } from "./permission-rules";
import { settingsRoute } from "./settings";
import { asked, item, mtime, refusal, row, said, STALE, staleOrRefusal, unreadPage } from "./settings-kit";
import type { Refused, Section } from "./settings-kit";

const TITLE = "Agents";

/** The engine's own agents people pick and hand work to, in the order they're listed; and those that
 * write titles, summaries and compactions, which aren't listed (Models sets the titles' model). */
const BUILT_IN = ["build", "plan", "general", "explore"] as const;
const HIDDEN = new Set(["compaction", "title", "summary"]);
const isBuiltIn = (id: string) => (BUILT_IN as readonly string[]).includes(id) || HIDDEN.has(id);

/** What the built-in agents do, in Kuvo's words (the engine's are written for the model), and how
 * they're used when nothing says otherwise. */
const BUILT_IN_WORDS: Record<string, string> = {
  build: "The default agent: it can change files and run commands.",
  plan: "Plans and answers without changing files.",
  general: "Researches questions and runs multi-step tasks for another agent.",
  explore: "Finds files and code quickly, without changing anything.",
};
const BUILT_IN_MODES: Record<string, string> = { build: "primary", plan: "primary", general: "subagent", explore: "subagent" };

/** How an agent can be used: as a mode a session works in, as a subagent another hands work to, or both. */
const MODES = [
  { id: "primary", title: "Mode", line: "A mode new sessions can start in." },
  { id: "subagent", title: "Subagent", line: "It works when you name it in a message, or when another agent hands it part of the work." },
  { id: "all", title: "Both", line: "A mode, and a subagent other agents hand work to." },
] as const;
const MODE_LINES: Record<string, string> = { primary: "A mode", subagent: "A subagent", all: "A mode and a subagent" };
const MODE_DETAIL: Record<string, string> = {
  primary: "New sessions can start in it, from the mode menu.",
  subagent: "It works when you name it in a message, or when another agent hands it part of the work; its description tells them when.",
  all: "New sessions can start in it, and it works when you name it or another agent hands it part of the work.",
};

/** The rules for particular commands, files or sites, by what they do: the form's three fields, in
 * the order they're written (the last that matches wins, so Never Allowed wins over the others). */
const RULE_FIELDS = [
  { id: "allowed", effect: "allow", title: "Allowed Without Asking" },
  { id: "asks", effect: "ask", title: "Asks First" },
  { id: "never", effect: "deny", title: "Never Allowed" },
] as const;
const RULE_EXAMPLE = "Run commands: npm test *";
/** Kinds of action the engine has rules for besides those with a choice, as a rule names them. */
const OTHER_ACTIONS = ["question", "browser", "todowrite", "tool.use", "provider.use", "integration.use"];

const computerHere = (context: RouteContext): string =>
  context.request.fromApp === true ? (process.platform === "darwin" ? "this Mac" : "this computer") : context.info.name || "this computer";

// MARK: Reading

/** An agent as the engine lists it (`api/agent`). */
interface EngineAgent {
  id?: string;
  name?: string;
  mode?: string;
  description?: string;
  hidden?: boolean;
  permissions?: unknown[];
}

/** What the page is made from: the engine's settings, its agents and models as it has them now
 * (null: it couldn't say), and Claude Code's agents on this computer. */
interface Facts {
  settings: Record<string, unknown>;
  engine: Target | null;
  agents: EngineAgent[] | null;
  models: { id: string; title: string; line: string }[] | null;
}

async function facts(context: RouteContext): Promise<Facts | { status: number; value: any }> {
  const config = await asked(context, settingsRoute, "GET", "kuvo/config");
  if (config.status !== 200) return config;
  let engine: Target | null = null;
  try { engine = context.readEngine(); } catch {}
  if (!engine?.port) engine = null;
  const [listed, models] = engine
    ? await Promise.all([
      call(engine, "GET", "/api/agent", undefined, AbortSignal.timeout(5_000)).catch(() => null),
      usableModels(engine, null, AbortSignal.timeout(5_000)).catch(() => null),
    ])
    : [null, null];
  return {
    settings: config.value, engine,
    agents: Array.isArray(listed) ? (listed as unknown[]).filter(isObject) as EngineAgent[] : null,
    models: models ? models.map(each => ({ id: `${each.providerID}/${each.id}`, title: each.name, line: each.providerID })) : null,
  };
}

/** The engine settings' `agents`, if they're an object (one written by hand as something else can't be changed here). */
const agentsIn = (settings: Record<string, unknown>): Record<string, unknown> | null => {
  const agents = settings.agents;
  return agents === undefined ? {} : isObject(agents) ? agents : null;
};
const entryOf = (known: Facts, id: string): Record<string, unknown> | undefined => {
  const entry = agentsIn(known.settings)?.[id];
  return isObject(entry) ? entry : undefined;
};

/** The agents listed: the built-in ones, then yours by name. */
function listedAgents(known: Facts): string[] {
  const yours = Object.keys(agentsIn(known.settings) ?? {}).filter(id => !isBuiltIn(id) && isObject(agentsIn(known.settings)![id]));
  return [...BUILT_IN, ...yours.sort((a, b) => a.localeCompare(b, undefined, { sensitivity: "base" }))];
}

const engineAgent = (known: Facts, id: string) => known.agents?.find(each => each.id === id);
const nameOf = (known: Facts, id: string) => (isBuiltIn(id) ? engineAgent(known, id)?.name || id.charAt(0).toUpperCase() + id.slice(1) : id);
const text = (value: unknown): string => (typeof value === "string" ? value : "");

/** How an agent is used: its own setting, else how the engine has it, else how it's built in. */
const modeOf = (known: Facts, id: string, entry?: Record<string, unknown>): string =>
  text(entry?.mode) || engineAgent(known, id)?.mode || BUILT_IN_MODES[id] || "all";

/** The built-in agents' own rules for the kinds of action with a choice, where they differ from the
 * engine's default, as engine 2.2.6 lists them (`api/agent`): Plan doesn't edit, General and Explore
 * hand no work on, and Explore only reads, searches and runs commands. */
// (Settings › Permissions' rules come after these in what the engine has for each agent, so its
// choice wins: checked against 2.2.6, `api/agent`. The engine's own list can't stand in for this
// table: it says nothing of working outside the project, which asks by another rule.)
const BUILT_IN_RULES: Record<string, Record<string, string>> = {
  plan: { edit: "deny" },
  general: { subagent: "deny" },
  explore: { edit: "deny", shell: "allow", read: "allow", external_directory: "allow", webfetch: "allow", websearch: "allow", subagent: "deny", skill: "deny" },
};

/** What applies to agent `id` for `action` with no choice of its own: Settings › Permissions' choice
 * (which applies in every mode), else how the agent is built, else the engine's default. */
function inherited(known: Facts, id: string, action: string): string {
  const global = Array.isArray(known.settings.permissions) ? known.settings.permissions : [];
  return choiceIn(global, action) ?? BUILT_IN_RULES[id]?.[action] ?? engineDefault(action);
}

// MARK: Rules as lines

const titleOf = (action: string) => ACTIONS.find(each => each.id === action)?.title ?? action;

/** An agent's own rules (not the choices), and whether the form can show its permissions as it
 * writes them: the choices first, then each rule with an effect it knows, allowed, asked, never.
 * Any other order is kept as it is (a choice after a rule, or a rule allowing after one denying,
 * would mean something else rewritten), and the form says so. */
function ownRules(entry: Record<string, unknown> | undefined): { rules: unknown[]; editable: boolean; list: boolean } {
  const permissions = entry?.permissions;
  if (permissions !== undefined && !Array.isArray(permissions)) return { rules: [], editable: false, list: false };
  const all = permissions ?? [];
  const firstRule = all.findIndex(each => !isChoice(each));
  const choicesFirst = firstRule < 0 || all.slice(firstRule).every(each => !isChoice(each));
  const rules = all.filter(each => !isChoice(each));
  const rank = (rule: unknown) => (isRule(rule) ? RULE_FIELDS.findIndex(field => field.effect === rule.effect) : -1);
  const ordered = rules.every((rule, at) => rank(rule) >= 0 && (at === 0 || rank(rules[at - 1]) <= rank(rule)));
  return { rules, editable: choicesFirst && ordered, list: true };
}

const rulesText = (rules: unknown[], effect: string) =>
  rules.filter(isRule).filter(rule => rule.effect === effect).map(rule => `${titleOf(rule.action)}: ${rule.resource}`).join("\n");

/** A field's lines as rules: each "what it's for: pattern", what it's for by its title ("Run
 * commands") or the engine's name for it ("shell"), whatever the case; or why one can't be. */
function parseRules(typed: string, effect: string, known: readonly string[]): { rules: Rule[]; why?: string } {
  const rules: Rule[] = [];
  for (const raw of typed.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    const lower = line.toLowerCase();
    const names = [
      ...ACTIONS.flatMap(action => [[action.title, action.id], [action.id, action.id]]),
      ...known.map(action => [action, action]),
    ];
    const match = names.filter(([name]) => lower.startsWith(`${name.toLowerCase()}:`)).sort((a, b) => b[0].length - a[0].length)[0];
    if (!match) return { rules, why: `“${line}” needs what it's for first, like ${RULE_EXAMPLE}.` };
    const resource = line.slice(match[0].length + 1).trim();
    if (!resource) return { rules, why: `Add a pattern after ${match[0]}:, like ${RULE_EXAMPLE}.` };
    if (resource === "*" && ACTIONS.some(action => action.id === match[1])) return { rules, why: `For all of them, set ${titleOf(match[1])} above.` };
    rules.push({ action: match[1], resource, effect });
  }
  return { rules };
}

// MARK: The form

/** An agent's form: New Agent's (`id` null) or one's own (Edit…), with its values now. */
function agentForm(context: RouteContext, known: Facts, id: string | null): SettingForm {
  const entry = id === null ? undefined : entryOf(known, id);
  const builtIn = id !== null && isBuiltIn(id);
  const mode = id === null ? "subagent" : modeOf(known, id, entry);
  const permissions = entry?.permissions;
  const { rules, editable, list } = ownRules(entry);
  const own = Array.isArray(permissions) ? permissions : [];
  const locked = !list ? { why: "Its permissions in the settings file aren't a list of rules: change them there." }
    : editable ? undefined : { why: "Its permissions are in an order only the settings file keeps: change them there." };
  const model = text(entry?.model);
  const models: { id: string; title: string; line?: string }[] = [
    { id: "", title: "Default", line: "A mode uses the session's model; a subagent the one Settings › Models gives subagents." },
    ...(known.models ?? []).map(each => ({ id: each.id, title: each.title, line: each.line })),
    ...(model && !(known.models ?? []).some(each => each.id === model) ? [{ id: model, title: model, line: known.models ? "Not available here now" : "As the settings name it" }] : []),
  ];
  const fields: SettingField[] = [
    ...(id === null ? [{ id: "name", title: "Name", kind: "text" as const, value: "", placeholder: "Bug Hunter" }] : []),
    { id: "mode", title: "Use as", kind: "choice", value: mode, choices: MODES.map(each => ({ ...each })) },
    {
      id: "description", title: "Description", kind: "lines", value: text(entry?.description),
      placeholder: builtIn ? BUILT_IN_WORDS[id!] ?? "" : "What it's for, like: Finds bugs and explains them, without changing code",
      line: "Other agents read it to know when to hand it work.",
    },
    { id: "model", title: "Model", kind: "choice", value: model, choices: models },
    ...(id === null ? [] : [{ id: "on", title: "Use this agent", kind: "switch" as const, value: entry?.disabled !== true }]),
    {
      id: "system", title: "Instructions", kind: "lines", value: text(entry?.system),
      placeholder: builtIn ? "Its built-in instructions" : "How it works: what to look for, what to leave alone, how to report back.",
      line: builtIn
        ? "Leave empty to keep its built-in instructions. Anything here replaces them; the project's AGENTS.md, skills and MCP servers still apply."
        : "The project's AGENTS.md, skills and MCP servers apply too.",
    },
    ...ACTIONS.map((action, at): SettingField => {
      const chosen = choiceIn(own, action.id);
      const from = inherited(known, id ?? "", action.id);
      return {
        id: `may-${action.id}`, title: action.title, kind: "choice", value: chosen ?? from,
        choices: [
          ...EFFECTS.map(effect => (effect.id === from ? { id: effect.id, title: `${effect.title} (Default)` } : { id: effect.id, title: effect.title })),
          // (One a newer engine set, shown as it is, and kept unless another is picked.)
          ...(chosen && !EFFECTS.some(effect => effect.id === chosen) ? [{ id: chosen, title: "Set by a Newer Engine" }] : []),
        ],
        ...(at === 0 ? { line: "Default does what Settings › Permissions says. A choice here wins over it for this agent." } : {}),
        ...(locked ? { can: locked } : {}),
      };
    }),
    ...RULE_FIELDS.map((field, at): SettingField => ({
      id: field.id, title: field.title, kind: "lines", mono: true, value: editable ? rulesText(rules, field.effect) : "",
      placeholder: at === 0 ? RULE_EXAMPLE : at === 1 ? "Run commands: git push *" : "Edit files: *.pem",
      ...(at === 0 ? { line: "For particular commands, files or sites, one a line: what it's for, then a pattern. * matches anything." } : {}),
      ...(at === RULE_FIELDS.length - 1 ? { line: "Never Allowed wins over Asks First, which wins over Allowed Without Asking." } : {}),
      ...(locked ? { can: locked } : {}),
    })),
  ];
  return id === null
    ? { title: "New Agent", submit: "Create Agent", fields, note: `On ${computerHere(context)}, for every project.` }
    : { title: nameOf(known, id), submit: "Save", fields };
}

/** A new agent's name as you typed it: one space between words, and only letters, digits, spaces,
 * hyphens and underscores (the engine takes it as it is, as a mode and a subagent). */
const agentId = (typed: string): string => typed.replace(/[^\p{L}\p{N}_ -]/gu, "").split(" ").filter(Boolean).join(" ");

/** One change to the engine's settings, from what was there. */
type Change = { path: string[]; value: unknown; was: unknown };

/** The form's values read: each field's words, whether it can be saved, and the changes it makes
 * to the engine's settings (each from what was there), with the new agent's id. */

function readForm(known: Facts, id: string | null, values: Record<string, unknown>): SettingsChecked & { changes?: Change[]; id?: string } {
  const fields: SettingFieldWords[] = [];
  const say = (field: string, words: string, tone: SettingFieldWords["tone"]) => fields.push({ id: field, words, tone });
  let ready = true;
  const agents = agentsIn(known.settings);
  if (!agents) return { fields: [{ id: "name", words: "The engine's agents in its settings file aren't in a form Kuvo can change: change them there.", tone: "warning" }], ready: false };

  let target = id;
  if (id === null) {
    const typed = text(values.name).trim();
    const made = agentId(typed);
    const lower = made.toLowerCase();
    const taken = Object.keys(agents).some(each => each.toLowerCase() === lower) || isBuiltIn(lower)
      || (known.agents ?? []).some(each => each.id?.toLowerCase() === lower || each.name?.toLowerCase() === lower);
    if (!made) {
      say("name", typed ? "A name needs a letter or a number." : "How it shows in the composer, and how other agents ask for it.", typed ? "warning" : "quiet");
      ready = false;
    } else if (made.length > 64) {
      say("name", "A name of 64 characters at most.", "warning");
      ready = false;
    } else if (made in Object.prototype) {
      // (A name every object has: the settings can't keep an agent by it.)
      say("name", "Pick another name.", "warning");
      ready = false;
    } else if (taken) {
      say("name", `There's an agent called ${made} already.`, "warning");
      ready = false;
    } else if (made !== typed) {
      say("name", `It'll be called ${made}.`, "quiet");
    }
    target = made || null;
  }

  const before = id === null ? undefined : entryOf(known, id);
  const mode = text(values.mode);
  if (!MODES.some(each => each.id === mode)) {
    say("mode", "Pick how it's used.", "warning");
    ready = false;
  } else {
    say("mode", MODE_DETAIL[mode]!, "quiet");
  }
  const model = text(values.model);
  const offered = !model || model === text(before?.model) || (known.models ?? []).some(each => each.id === model);
  if (!offered) {
    say("model", `${model} isn't a model here.`, "warning");
    ready = false;
  }

  // Its permissions: a choice for each kind of action at the front, then the rules, by what they do.
  const { rules: kept, editable } = ownRules(before);
  const own = Array.isArray(before?.permissions) ? before!.permissions as unknown[] : [];
  // (Permissions the form can't show as it writes them are kept as they are.)
  let permissions: unknown = before?.permissions;
  if (editable) {
    const choices: Rule[] = [];
    for (const action of ACTIONS) {
      const picked = values[`may-${action.id}`];
      const chosen = choiceIn(own, action.id);
      const from = inherited(known, target ?? "", action.id);
      if (typeof picked !== "string" || (!EFFECTS.some(effect => effect.id === picked) && picked !== chosen)) {
        say(`may-${action.id}`, "Pick one.", "warning");
        ready = false;
        continue;
      }
      // What applies anyway writes nothing, unless a rule of yours says so already.
      if (picked === from && chosen !== picked) continue;
      choices.push({ action: action.id, resource: "*", effect: picked });
    }
    const named = [...new Set([...OTHER_ACTIONS, ...kept.filter(isRule).map(rule => rule.action)])];
    const rules = RULE_FIELDS.flatMap(field => {
      const read = parseRules(text(values[field.id]), field.effect, named);
      if (read.why) {
        say(field.id, read.why, "warning");
        ready = false;
      }
      return read.rules;
    });
    const made = [...choices, ...rules];
    permissions = made.length ? made : undefined;
  }

  const entry: Record<string, unknown> = { ...before };
  const builtIn = target !== null && isBuiltIn(target);
  // (A built-in one used as it's built in has nothing written for it.)
  if (builtIn && mode === BUILT_IN_MODES[target!] && before?.mode === undefined) delete entry.mode; else entry.mode = mode;
  for (const [key, value] of [["description", text(values.description).trim()], ["model", model], ["system", text(values.system).trim()]] as const) {
    // (One written by hand as something the form can't show, a model as an object say, opens empty
    // and is kept unless something's put in its place.)
    if (value) entry[key] = value; else if (typeof before?.[key] === "string") delete entry[key];
  }
  if (id !== null && values.on === false) entry.disabled = true; else delete entry.disabled;
  if (permissions === undefined) delete entry.permissions; else entry.permissions = permissions;
  if (!ready || target === null) return { fields, ready };

  const empty = Object.keys(entry).length === 0;
  const changes: Change[] = [{ path: ["agents", target], value: builtIn && empty ? null : entry, was: before ?? null }];
  // One that can't start a session any more stops being the one new sessions start in, in the same write.
  if ((entry.disabled === true || entry.mode === "subagent") && known.settings.default_agent === target) changes.push({ path: ["default_agent"], value: null, was: target });
  return { fields, ready, changes, id: target };
}

// MARK: The page

async function agentsPage(context: RouteContext): Promise<SettingsPage> {
  const known = await facts(context);
  if (!("settings" in known)) return unreadPage("agents", TITLE, known);
  const agents = agentsIn(known.settings);
  const where = computerHere(context);
  const notList = { why: "The engine's agents in its settings file aren't in a form Kuvo can change: change them there." };

  const items: SettingItem[] = listedAgents(known).map(id => {
    const entry = entryOf(known, id);
    const builtIn = isBuiltIn(id);
    const mode = modeOf(known, id, entry);
    const description = text(entry?.description) || (builtIn ? BUILT_IN_WORDS[id] : "") || "";
    const name = nameOf(known, id);
    const actions: SettingAction[] = [
      { id: "edit", title: "Edit…", form: agentForm(context, known, id) },
      ...(builtIn && entry ? [{ id: "reset", title: "Reset to Built-In…", style: "destructive" as const, confirm: { title: `Reset ${name} to how it's built in?`, body: "Your changes to it are removed.", button: "Reset" } }] : []),
      ...(!builtIn ? [{ id: "delete", title: "Delete Agent…", style: "destructive" as const, confirm: { title: `Delete ${name}?`, body: `It's removed from ${where}'s settings.`, button: "Delete" } }] : []),
    ];
    const kept = item({
      id: `agent-${id}`, title: name, line: [MODE_LINES[mode] ?? mode, description.split("\n")[0]].filter(Boolean).join(" · "),
      kind: "switch", value: entry?.disabled !== true, can: agents ? true : notList, actions,
    });
    // (How it stands, beside its name: a built-in one you changed; one the engine can't use.)
    const badge = builtIn && entry ? { words: "Changed", tone: "quiet" as const } : undefined;
    const unusable = !MODE_LINES[mode] ? `The engine leaves it out: “${mode}” isn't a way to use an agent. Pick one in Edit….` : undefined;
    return { ...kept, ...(badge ? { badge } : {}), ...(unusable ? { notice: { words: unusable, tone: "warning" as const } } : {}) };
  });

  const groups: SettingGroup[] = [{
    id: "agents", title: "Agents",
    note: "Plan is what /plan turns on. Your own agents work when you name one in a message, or when an agent hands one part of a task; Claude Code, Codex, Cursor, Grok and Antigravity get your subagents too.",
    rows: [row({
      id: "agents", title: "Agents", kind: "list", items, empty: "None yet.",
      ...(agents ? { add: { id: "add", title: "New Agent…", form: agentForm(context, known, null) } } : {}),
    })],
  }];

  // Claude Code's subagents on this computer, to bring over.
  const claude = (await claudeCodeList(path.join(homeFolder(), ".claude"), known.settings, new Set()).catch(() => null))?.agents ?? [];
  if (claude.length) {
    groups.push({
      id: "claude-code", title: "From Claude Code",
      note: `Agents in ~/.claude/agents on ${where}, brought over as subagents with their description and instructions. They use the session's model and the permissions here, not the tools and model Claude Code gives them.`,
      rows: claude.map(each => {
        const id = each.path[1]!;
        const line = each.description.split("\n")[0] || undefined;
        return each.imported
          ? row({ id: `claude-${id}`, title: each.name, ...(line ? { line } : {}), kind: "status", value: "Imported" })
          : row({ id: `claude-${id}`, title: each.name, ...(line ? { line } : {}), kind: "actions", actions: [{ id: "import", title: "Import", ...(agents ? {} : { can: notList }) }] });
      }),
    });
  }

  const notes = [
    ...(agents ? [] : [{ words: notList.why, tone: "warning" as const }]),
    ...(known.agents ? [] : [{ words: `The engine on ${where} isn’t answering, so the models to pick from and what each agent may do by default aren’t known. The agents are listed as the settings have them.`, tone: "warning" as const }]),
  ];
  return { id: "agents", title: TITLE, groups, ...(notes.length ? { notes } : {}) };
}

// MARK: Changes

/** Changes to the engine's settings, each from what was there (`was`): a 409 for one made since. */
const write = (context: RouteContext, changes: Change[]) =>
  asked(context, settingsRoute, "PATCH", "kuvo/config", { changes });

const idOf = (at: SettingItem) => (at.id.startsWith("agent-") ? at.id.slice("agent-".length) : null);

async function agentsChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  const known = await facts(context);
  if (!("settings" in known)) return refusal(known) ?? { status: 503, message: said(known, "This computer's settings couldn't be read.") };
  const id = idOf(at);
  if (id === null || !listedAgents(known).includes(id)) return { status: 400, message: "That isn't something to change." };
  const entry = entryOf(known, id);
  if ((entry?.disabled !== true) !== at.value) return STALE;
  if (value === at.value) return null;
  // (Turned on, one of yours with nothing else written keeps its entry, which is the agent.)
  const keeps = !isBuiltIn(id) && entry !== undefined && Object.keys(entry).length === 1;
  const changes: Change[] = [{ path: ["agents", id, "disabled"], value: value ? (keeps ? false : null) : true, was: entry?.disabled ?? null }];
  if (value === false && known.settings.default_agent === id) changes.push({ path: ["default_agent"], value: null, was: id });
  return staleOrRefusal(await write(context, changes));
}

/** A form's values refused: why, and each field's words. */
const formRefused = (checked: SettingsChecked): Refused => {
  const first = checked.fields.find(each => each.tone === "warning");
  return { status: 400, message: first?.words ?? "Fill in what it needs first.", fields: [...checked.fields] };
};

async function agentsAct(context: RouteContext, at: SettingItem, action: SettingAction, values?: Record<string, unknown>): Promise<Refused | null> {
  const known = await facts(context);
  if (!("settings" in known)) return refusal(known) ?? { status: 503, message: said(known, "This computer's settings couldn't be read.") };

  if (at.id === "agents" && action.id === "add") {
    const checked = readForm(known, null, values ?? {});
    if (!checked.changes || !checked.id) return formRefused(checked);
    const written = await write(context, checked.changes);
    const taken = `There's an agent called ${checked.id} already.`;
    if (written.status === 409 && written.value?.message === CHANGED_SINCE) return { status: 400, message: taken, fields: [{ id: "name", words: taken, tone: "warning" }] };
    return refusal(written);
  }
  if (at.id.startsWith("claude-") && action.id === "import") {
    const claude = await claudeCodeList(path.join(homeFolder(), ".claude"), known.settings, new Set());
    const found = claude.agents.find(each => each.path[1] === at.id.slice("claude-".length));
    if (!found || found.imported) return STALE;
    return staleOrRefusal(await write(context, [{ path: [...found.path], value: found.settings, was: null }]));
  }
  const id = idOf(at);
  if (id === null || !listedAgents(known).includes(id)) return { status: 404, message: "That isn't something it does." };
  const entry = entryOf(known, id);
  switch (action.id) {
    case "edit": {
      const checked = readForm(known, id, values ?? {});
      if (!checked.changes) return formRefused(checked);
      return staleOrRefusal(await write(context, checked.changes));
    }
    case "reset":
    case "delete": {
      if (!entry || (action.id === "reset") !== isBuiltIn(id)) return STALE;
      const changes: Change[] = [{ path: ["agents", id], value: null, was: entry }];
      // (A built-in one reset is still there to start in; one of yours deleted isn't.)
      if (action.id === "delete" && known.settings.default_agent === id) changes.push({ path: ["default_agent"], value: null, was: id });
      return staleOrRefusal(await write(context, changes));
    }
  }
  return { status: 404, message: "That isn't something it does." };
}

async function agentsCheck(context: RouteContext, at: SettingItem, action: SettingAction, values: Record<string, unknown>): Promise<SettingsChecked> {
  const known = await facts(context);
  if (!("settings" in known)) return { fields: [], ready: false };
  const { fields, ready } = readForm(known, action.id === "edit" ? idOf(at) : null, values);
  return { fields, ready };
}

export const agentsSection: Section & { listed: SettingsSection } = {
  listed: {
    id: "agents", title: TITLE, group: "Agents & Tools", icon: "bot",
    line: "The built-in agents and your own: how each is used, its model, instructions and what it may do.",
    keywords: ["agents", "subagents", "modes", "build", "plan", "instructions", "prompt", "model", "permissions", "rules", "claude code", "import"],
  },
  read: agentsPage, change: agentsChange, act: agentsAct, check: agentsCheck,
  // (The engine's models and defaults change without the file: read again every ten seconds, sent only when the page changed.)
  look: () => `${mtime(CONFIG_FILE)} ${mtime(path.join(homeFolder(), ".claude", "agents"))} ${Math.floor(Date.now() / 10_000)}`,
};

