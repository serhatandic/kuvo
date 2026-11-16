// Settings › Commands: the same forms and actions on every client; all writes use kuvo/config.
import path from "node:path";
import { isObject } from "../../shared/json";
import type { SettingAction, SettingForm, SettingGroup, SettingFieldWords, SettingItem, SettingsChecked, SettingsPage, SettingsSection } from "../../shared/routes/settings-pages";
import type { RouteContext } from "../server/context";
import { call } from "../engine/client";
import { homeFolder } from "../home";
import { claudeCodeRoute } from "./claude-code";
import { CONFIG_FILE, valueAt } from "./config";
import { settingsRoute } from "./settings";
import { asked, item, mtime, noActions, row, staleOrRefusal, unreadPage } from "./settings-kit";
import type { Refused, Section } from "./settings-kit";

const NOTE = "Type / and a command's name at the start of a message to run it. What you type after the name fills in $ARGUMENTS.";
const PROMPT = "What the agent gets. $ARGUMENTS is what you type after the command; $1, $2 and so on are its words. !`command` puts in a command's output.";
const text = (value: unknown) => typeof value === "string" ? value : "";
const commandName = (value: string) => value.trim().replace(/^\/+|\/+$/g, "").replaceAll(" ", "-").replace(/[^\p{L}\p{N}_-]/gu, "");
interface Facts { settings: Record<string, unknown>; commands: any[]; agents: any[]; models: any[]; imports: any[] }
async function facts(context: RouteContext): Promise<Facts | { status: number; value: any }> {
  const config = await asked(context, settingsRoute, "GET", "kuvo/config");
  if (config.status !== 200) return config;
  let commands: any[] = [], agents: any[] = [], models: any[] = [];
  try {
    const engine = context.readEngine();
    const listed = await Promise.all([call(engine, "GET", "/api/command", undefined, AbortSignal.timeout(5000)), call(engine, "GET", "/api/agent", undefined, AbortSignal.timeout(5000)), call(engine, "GET", "/api/model", undefined, AbortSignal.timeout(5000))]);
    commands = Array.isArray(listed[0]) ? listed[0] : [];
    agents = Array.isArray(listed[1]) ? listed[1] : [];
    models = Array.isArray(listed[2]) ? listed[2] : [];
  } catch {}
  const imported = await asked(context, claudeCodeRoute, "GET", "kuvo/claude-code");
  return { settings: config.value, commands, agents, models, imports: imported.status === 200 && Array.isArray(imported.value.commands) ? imported.value.commands : [] };
}
function form(known: Facts, name: string | null, before: Record<string, unknown> = {}): SettingForm {
  const agents = [...new Set([...known.agents.filter(each => !each.hidden).map(each => text(each.id || each.name)), ...(text(before.agent) ? [text(before.agent)] : [])])].filter(Boolean);
  const models = [...new Map([...known.models.filter(each => typeof each.providerID === "string" && typeof each.id === "string").map(each => [`${each.providerID}/${each.id}`, text(each.name) || each.id] as const), ...(text(before.model) ? [[text(before.model), text(before.model)] as const] : [])]).entries()];
  return { title: name ? `/${name}` : "New Command", submit: name ? "Save" : "Create Command", fields: [
    ...(name === null ? [{ id: "name", title: "Name", kind: "text" as const, value: "", placeholder: "test" }] : []),
    { id: "description", title: "Description", kind: "lines", value: text(before.description), placeholder: "Shown in the list when you type /" },
    { id: "agent", title: "Agent", kind: "choice", value: text(before.agent), choices: [{ id: "", title: "The session's agent" }, ...agents.map(id => ({ id, title: id }))] },
    { id: "model", title: "Model", kind: "choice", value: text(before.model), choices: [{ id: "", title: "The agent's or session's model" }, ...models.map(([id, title]) => ({ id, title }))] },
    { id: "subagent", title: "Run as a subagent", kind: "switch", value: before.subagent === true, line: "In the background, without filling up the conversation." },
    { id: "template", title: "Prompt", kind: "lines", mono: true, value: text(before.template), line: PROMPT },
  ] };
}
function readForm(known: Facts, name: string | null, values: Record<string, unknown>, before: Record<string, unknown> = {}): SettingsChecked & { name: string; entry: Record<string, unknown> } {
  const fields: SettingFieldWords[] = [];
  let ready = true;
  const warn = (id: string, words: string) => { fields.push({ id, words, tone: "warning" }); ready = false; };
  const typed = text(values.name).trim().replace(/^\/+|\/+$/g, "");
  const id = name ?? commandName(text(values.name));
  if (name === null) {
    if (!id) { fields.push({ id: "name", words: typed ? "A name needs a letter or a number." : "A short name without spaces: you'll type / and this name.", tone: typed ? "warning" : "quiet" }); ready = false; }
    else if (valueAt(known.settings, ["commands", id]) !== undefined || known.commands.some(each => each.name === id)) warn("name", `There's a command called /${id} already.`);
    else if (id !== typed) fields.push({ id: "name", words: `It'll be called /${id}.`, tone: "quiet" });
  }
  for (const key of ["description", "agent", "model", "template"]) if (typeof values[key] !== "string") warn(key, "Send text.");
  if (typeof values.subagent !== "boolean") warn("subagent", "Send true or false.");
  const choices = form(known, name, before).fields.find(each => each.id === "agent")!.choices!;
  if (typeof values.agent === "string" && !choices.some(each => each.id === values.agent)) warn("agent", "Pick one of its choices.");
  const entry: Record<string, unknown> = { ...before, template: text(values.template) };
  for (const key of ["description", "agent", "model"]) {
    const value = text(values[key]).trim();
    if (value) entry[key] = value; else delete entry[key];
  }
  if (values.subagent === true) entry.subagent = true; else delete entry.subagent;
  return { fields, ready, name: id, entry };
}
async function page(context: RouteContext): Promise<SettingsPage> {
  const known = await facts(context);
  if (!("settings" in known)) return unreadPage("commands", "Commands", known);
  const source = known.settings.commands;
  if (source !== undefined && !isObject(source)) return unreadPage("commands", "Commands", { status: 409, value: { message: "The engine's commands aren't a list of named commands: change them in its settings file." } });
  const commands = isObject(source) ? source : {};
  const items = Object.keys(commands).sort().map(name => {
    const entry = commands[name];
    const valid = isObject(entry);
    const can = valid ? true as const : { why: "This command isn't a set of fields: change it in the settings file." };
    return item({ id: `command-${name}`, title: `/${name}`, kind: "actions", ...(valid && text(entry.description) ? { line: text(entry.description) } : {}), actions: [
      { id: "edit", title: "Edit…", can, form: form(known, name, valid ? entry : {}) },
      { id: "remove", title: "Delete Command…", can, style: "destructive", confirm: { title: `Delete /${name}?`, body: "It's removed from the engine's settings.", button: "Delete" } },
    ] });
  });
  const groups: SettingGroup[] = [{ id: "your-commands", title: "Your Commands", note: NOTE, rows: [row({ id: "commands", title: "Commands", kind: "list", items, empty: "None yet.", add: { id: "add", title: "New Command…", form: form(known, null) } })] }];
  if (known.imports.length) groups.push({ id: "claude-commands", title: "From Claude Code", note: "Commands in ~/.claude/commands. Their prompts come over as they are.", rows: known.imports.map(each => row({ id: `import-${each.name}`, title: `/${each.name}`, line: each.description, kind: each.imported ? "status" : "actions", ...(each.imported ? { value: "Imported" } : { actions: [{ id: "import", title: "Import" }] }) })) });
  const others = known.commands.filter(each => typeof each.name === "string" && !(each.name in commands));
  if (others.length) groups.push({ id: "other-commands", title: "Also Available", rows: others.map(each => row({ id: `available-${each.name}`, title: `/${each.name}`, kind: "status", ...(text(each.description) ? { line: text(each.description).replace(/^./, letter => letter.toUpperCase()) } : {}) })) });
  return { id: "commands", title: "Commands", groups };
}
const write = (context: RouteContext, name: string, value: unknown, was: unknown) => asked(context, settingsRoute, "PATCH", "kuvo/config", { changes: [{ path: ["commands", name], value, was: was ?? null }] });
async function act(context: RouteContext, at: SettingItem, action: SettingAction, values?: Record<string, unknown>): Promise<Refused | null> {
  const known = await facts(context);
  if (!("settings" in known)) return { status: known.status, message: known.value.message ?? "The settings couldn't be read." };
  if (at.id.startsWith("import-") && action.id === "import") {
    const imported = known.imports.find(each => each.name === at.id.slice(7));
    if (!imported) return { status: 404, message: "That command is no longer available." };
    return staleOrRefusal(await write(context, imported.path[1], imported.settings, null));
  }
  const name = at.id.startsWith("command-") ? at.id.slice(8) : null;
  const before = name ? valueAt(known.settings, ["commands", name]) : undefined;
  if (name && action.id === "remove") return staleOrRefusal(await write(context, name, null, before));
  const checked = readForm(known, name, values ?? {}, isObject(before) ? before : {});
  if (!checked.ready) return { status: 400, message: checked.fields.find(each => each.tone === "warning")?.words ?? "Fill in what it needs first.", fields: [...checked.fields] };
  return staleOrRefusal(await write(context, checked.name, checked.entry, before));
}
export const commandsSection: Section & { listed: SettingsSection } = {
  listed: { id: "commands", title: "Commands", group: "Agents & Tools", icon: "terminal", line: "Prompts you use often, run by typing / and their name.", keywords: ["commands", "prompt", "slash", "template", "arguments", "claude", "import"] },
  read: page, change: noActions, act,
  check: async (context, at, _action, values) => {
    const known = await facts(context);
    if (!("settings" in known)) return { fields: [], ready: false };
    const name = at.id.startsWith("command-") ? at.id.slice(8) : null;
    const before = name ? valueAt(known.settings, ["commands", name]) : undefined;
    const { fields, ready } = readForm(known, name, values, isObject(before) ? before : {});
    return { fields, ready };
  },
  look: () => `${mtime(CONFIG_FILE)} ${mtime(path.join(homeFolder(), ".claude", "commands"))} ${Math.floor(Date.now() / 1000)}`,
};
