// Settings › Skills: the reusable skills routes own the files, collections and trash.
import path from "node:path";
import { isObject } from "../../shared/json";
import type { SettingAction, SettingForm, SettingItem, SettingsChecked, SettingsPage, SettingsSection } from "../../shared/routes/settings-pages";
import type { RouteContext } from "../server/context";
import { call } from "../engine/client";
import { CONFIG_FILE, valueAt } from "./config";
import { skillsRoute, skillName, skillAbout, SKILLS_FOLDER, SKILL_TRASH_FOLDER } from "./skills";
import { settingsRoute } from "./settings";
import { asked, item, row, mtime, refusal, said, staleOrRefusal, unreadPage } from "./settings-kit";
import type { Section } from "./settings-kit";

const NOTE = "The agent loads a skill when a task calls for it. Skills in ~/.claude/skills and ~/.agents/skills, and in a project's .opencode/skills, are found on their own.";
const DESCRIPTION = "The agent reads this to decide when the skill is needed, and loads the instructions only then.";
const INSTRUCTIONS = "Markdown. Scripts and other files can go in the skill's folder; mention them here so the agent knows to use them.";
const DELETION = "Its folder, with anything else in it, is kept for 30 days in Recently Deleted, where you can restore it.";

async function facts(context: RouteContext) {
  const own = await asked(context, skillsRoute, "GET", "kuvo/skills");
  if (own.status !== 200) return { problem: own };
  const config = await asked(context, settingsRoute, "GET", "kuvo/config");
  if (config.status !== 200) return { problem: config };
  const files = await Promise.all((own.value.folders as string[]).map(async name => ({ name, answer: await asked(context, skillsRoute, "GET", `kuvo/skills/${encodeURIComponent(name)}`) })));
  // A folder can be empty, use a filename the skill route does not read, or have an invalid
  // name. It still reserves that name, but must not hide the other readable skills.
  const readable = files.filter(each => each.answer.status === 200 && typeof each.answer.value?.text === "string");
  let engine: any[] = [];
  try { const target = context.readEngine(); const listed: any = await call(target, "GET", "/api/skill", undefined, AbortSignal.timeout(5000)); if (Array.isArray(listed)) engine = listed; } catch {}
  return { own: own.value, config: config.value, files: readable.map(each => ({ name: each.name, text: each.answer.value.text as string })), engine };
}

/** Preserve unknown front matter while editing the two fields Settings owns. */
export function skillDocument(name: string, description: string, body: string, existing = ""): string {
  const front = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(existing);
  const others: string[] = [];
  let skipping = false;
  for (const line of (front?.[1] ?? "").split(/\r?\n/)) {
    if (/^(name|description):/.test(line)) { skipping = true; continue; }
    if (skipping && /^\s/.test(line)) continue;
    skipping = false;
    if (line) others.push(line);
  }
  return `---\nname: ${JSON.stringify(name)}\ndescription: ${JSON.stringify(description.trim())}\n${others.length ? others.join("\n") + "\n" : ""}---\n\n${body.trim()}\n`;
}
function form(name: string | null, text = ""): SettingForm {
  const about = skillAbout(text);
  const body = text.replace(/^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/, "").trim();
  return { title: name ?? "New Skill", submit: name ? "Save" : "Create Skill", fields: [
    ...(!name ? [{ id: "name", title: "Name", kind: "text" as const, value: "", placeholder: "release-notes" }] : []),
    { id: "description", title: "Use it when", kind: "lines", value: about.description ?? "", placeholder: "Writing release notes from the commits since the last tag", line: DESCRIPTION },
    { id: "instructions", title: "Instructions", kind: "lines", mono: true, value: body, line: INSTRUCTIONS },

  ] };
}
function readForm(name: string | null, values: Record<string, unknown>, taken: string[]): SettingsChecked & { name: string; text: string } {
  const fields: Array<SettingsChecked["fields"][number]> = [];
  const typed = typeof values.name === "string" ? values.name.trim() : "";
  const id = name ?? skillName(typed);
  let ready = true;
  if (!name) {
    const warning = typed && !id ? "Names use the letters a to z and digits, and this one has none." : taken.includes(id) ? `There’s a skill called ${id} already.` : null;
    fields.push({ id: "name", words: warning ?? (id && id !== typed ? `It's saved as ${id}.` : "Names use lowercase letters a to z, digits and hyphens."), tone: warning ? "warning" : "quiet" });
    if (!id || warning) ready = false;
  }
  if (typeof values.description !== "string" || !values.description.trim()) { ready = false; fields.push({ id: "description", words: "Say when the agent should use it.", tone: "warning" }); }
  if (typeof values.instructions !== "string") { ready = false; fields.push({ id: "instructions", words: "Instructions must be text.", tone: "warning" }); }
  return { ready, fields, name: id, text: skillDocument(id, typeof values.description === "string" ? values.description : "", typeof values.instructions === "string" ? values.instructions : "") };
}
/** This computer's skills as Marketplace › Skills lists them beside its own: each skill the agent
 * can use here (its own folder's and those its engine finds elsewhere), whether it's on, Recently
 * Deleted and Other Places. The same facts the Settings section was made from. */
export async function skillsHere(context: RouteContext) {
  const known = await facts(context);
  if (known.problem) return { problem: said(known.problem, "This computer’s skills couldn’t be read.") };
  const own = new Set(known.files!.map(each => each.name));
  const found = known.engine!.filter(each => typeof each.id === "string" && !own.has(each.id)).map(each => {
    const where = typeof each.path === "string" ? each.path : undefined;
    return { id: each.id as string, title: typeof each.name === "string" && each.name ? each.name : each.id as string,
      summary: typeof each.description === "string" ? each.description : "", location: where?.startsWith("/builtin/") ? "Built in" : where };
  });
  const trash = await asked(context, skillsRoute, "GET", "kuvo/skill-trash");
  const places = valueAt(known.config, ["skills"]);
  return {
    problem: undefined,
    enabled: (name: string) => !disabled(known.config, name),
    canSwitch: known.config.permissions === undefined || Array.isArray(known.config.permissions),
    found,
    deleted: trash.status === 200 ? (trash.value.entries as any[]).map(each => ({ id: String(each.id), title: String(each.name), summary: typeof each.description === "string" ? each.description : "" })) : [],
    places: Array.isArray(places) ? places.map(String) : [],
  };
}
const disabled = (config: any, name: string) => (Array.isArray(config.permissions) ? config.permissions : []).some((each: any) => each?.action === "skill" && each.resource === name && typeof each.effect === "string" && !["allow", "ask"].includes(each.effect));

async function page(context: RouteContext): Promise<SettingsPage> {
  const known = await facts(context);
  if (known.problem) return unreadPage("skills", "Skills", known.problem);
  const yours = new Map(known.files!.map(each => [each.name, each.text]));
  const names = [...new Set([...yours.keys(), ...known.engine!.map(each => each.id).filter((id): id is string => typeof id === "string")])].sort();
  const skills = names.map(name => {
    const text = yours.get(name);
    const source = known.own.github[name];
    const about = skillAbout(text ?? "");
    const discovered = known.engine!.find(each => each.id === name);
    const title = about.name || (typeof discovered?.name === "string" && discovered.name) || name;
    const description = about.description || (typeof discovered?.description === "string" ? discovered.description : undefined);
    const origin = source?.repository || (text !== undefined ? "Added here" : typeof discovered?.path === "string" && discovered.path.startsWith("/builtin/") ? "Built in" : undefined);
    const location = typeof discovered?.path === "string" ? discovered.path : undefined;
    const actions: SettingAction[] = text === undefined ? [] : [
      ...(!source ? [{ id: "edit", title: "Edit…", form: form(name, text) }] : []),
      { id: "remove", title: source ? "Remove…" : "Delete Skill…", style: "destructive", confirm: { title: `Delete ${name}?`, body: DELETION, button: "Delete" } },
    ];
    return item({ id: `skill-${name}`, title, kind: "switch", value: !disabled(known.config, name), can: known.config.permissions === undefined || Array.isArray(known.config.permissions) ? true : { why: "Permissions are set as something other than a list. Change them in the settings file." }, ...(description ? { line: description } : {}), ...(origin ? { badge: { words: origin, tone: "quiet" as const } } : location ? { notice: { words: location, tone: "quiet" as const } } : {}), actions });
  });
  const trash = await asked(context, skillsRoute, "GET", "kuvo/skill-trash");
  const places = valueAt(known.config, ["skills"]);
  if (places !== undefined && !Array.isArray(places)) return unreadPage("skills", "Skills", { status: 409, value: { message: "Other Places is set as something other than a list. Change it in the settings file." } });
  return { id: "skills", title: "Skills", groups: [
    { id: "available-skills", title: "Skills the Agent Can Use", note: NOTE, rows: [row({ id: "skills", title: "Skills", kind: "list", items: skills, ...(skills.length > 6 ? { search: { placeholder: "Search skills", empty: "No skills match “{query}”." } } : {}), empty: "None yet.", add: { id: "add", title: "New Skill…", form: form(null) } })] },
    ...(trash.status === 200 && trash.value.entries.length ? [{ id: "deleted-skills", title: "Recently Deleted", note: DELETION, rows: [row({ id: "deleted", title: "Recently Deleted", kind: "list", ...(trash.value.entries.length > 6 ? { search: { placeholder: "Search recently deleted", empty: "No deleted skill matches “{query}”." } } : {}), items: trash.value.entries.map((each: any) => item({ id: `trash-${each.id}`, title: each.name, line: each.description, kind: "actions", actions: [{ id: "restore", title: "Restore" }] })) })] }] : []),
    { id: "get-skills", title: "Get Skills", note: "These come from their authors on GitHub, not from Kuvo Code. A skill can include scripts the agent may run, so look at what's in one before you add it.", rows: [row({ id: "install", title: "Get Skills", kind: "actions", actions: [{ id: "install", title: "Add Skill…", form: { title: "Add Skill", submit: "Add Skill", fields: [
      { id: "repository", title: "Repository", kind: "text", value: "", placeholder: "Another repository on GitHub, like owner/skills" },
      { id: "path", title: "Skill’s folder in the repository", kind: "text", value: "", placeholder: "skills/release-notes" },
    ] } }] })] },
    { id: "other-places", title: "Other Places", note: "A catalog is a web address with an index.json listing its skills.", rows: [row({ id: "places", title: "Other Places", kind: "list", items: (Array.isArray(places) ? places : []).map((value: unknown, index: number) => item({ id: `place-${index}`, title: String(value), kind: "actions", actions: [{ id: "remove", title: "Remove…", confirm: { title: "Remove this place?", body: String(value), button: "Remove" } }] })), add: { id: "add", title: "Add…", form: { title: "Other Places", submit: "Add", fields: [{ id: "place", title: "A folder, or the address of a skill catalog", kind: "text", value: "" }] } } })] },
  ] };
}
const writeConfig = (context: RouteContext, key: string, value: unknown, was: unknown) => asked(context, settingsRoute, "PATCH", "kuvo/config", { changes: [{ path: [key], value, was: was ?? null }] });
/** Repository names and GitHub links use the same install route as the later Marketplace. */
export function skillInstall(values: Record<string, unknown>) {
  const typed = typeof values.repository === "string" ? values.repository.trim() : "";
  const parts = typed.replace(/^(?:https?:\/\/)?github\.com\//i, "").replace(/\/$/, "").split("/");
  const repository = parts.slice(0, 2).join("/").replace(/\.git$/, "");
  const folder = typeof values.path === "string" ? values.path.trim() : "";

  const fields: Array<SettingsChecked["fields"][number]> = [];
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository) || (parts.length > 2 && !["tree", "blob"].includes(parts[2]!))) fields.push({ id: "repository", words: "Give a GitHub link or a repository like owner/skills.", tone: "warning" });
  if (["tree", "blob"].includes(parts[2] ?? "")) fields.push({ id: "repository", words: "Use the repository’s main link and give the skill’s folder separately. Links to a branch or file aren’t supported here.", tone: "warning" });
  if (typeof values.path !== "string") fields.push({ id: "path", words: "Give the skill’s folder as text, or leave it empty for the repository itself.", tone: "warning" });
  return { repository, path: folder, fields, ready: fields.length === 0 };
}
async function checked(context: RouteContext, at: SettingItem, action: SettingAction, values: Record<string, unknown>): Promise<SettingsChecked> {
  if (at.id === "install") { const { ready, fields } = skillInstall(values); return { ready, fields }; }
  if (at.id === "places") return { ready: typeof values.place === "string" && !!values.place.trim(), fields: [] };
  const known = await facts(context);
  if (known.problem) return { ready: false, fields: [] };
  const { ready, fields } = readForm(action.id === "edit" ? at.id.slice(6) : null, values, [...known.own.folders, ...known.engine!.map(each => each.id)]);
  return { ready, fields };
}
export const skillsSection: Section & { listed: SettingsSection } = {
  listed: { id: "skills", title: "Skills", group: "Agents & Tools", icon: "pencil", line: "Instructions and scripts agents load when a task calls for them.", keywords: ["skills", "instructions", "scripts", "recently deleted", "catalog"] },
  read: page,
  async change(context, at, value) {
    const known = await facts(context);
    if (known.problem) return refusal(known.problem);
    const before = known.config.permissions;
    const kept = (Array.isArray(before) ? before : []).filter((each: any) => !(each?.action === "skill" && each.resource === at.id.slice(6) && typeof each.effect === "string" && !["allow", "ask"].includes(each.effect)));
    if (!value) kept.push({ action: "skill", resource: at.id.slice(6), effect: "deny" });
    return staleOrRefusal(await writeConfig(context, "permissions", kept, before));
  },
  check: checked,
  async act(context, at, action, values = {}) {
    const known = await facts(context);
    if (known.problem) return refusal(known.problem);
    if (at.id === "install") {
      const install = skillInstall(values);
      if (!install.ready) return { status: 400, message: install.fields[0]!.words, fields: install.fields };
      return refusal(await asked(context, skillsRoute, "POST", "kuvo/skill-library/install", { repository: install.repository, path: install.path }));
    }
    if (at.id.startsWith("trash-")) return refusal(await asked(context, skillsRoute, "POST", `kuvo/skill-trash/${encodeURIComponent(at.id.slice(6))}/restore`));
    if (at.id === "places" || at.id.startsWith("place-")) {
      const before = known.config.skills;
      const places = Array.isArray(before) ? [...before] : [];
      if (action.id === "add") { if (typeof values.place !== "string" || !values.place.trim()) return { status: 400, message: "Give a folder or catalog address." }; places.push(values.place.trim()); }
      else places.splice(Number(at.id.slice(6)), 1);
      return staleOrRefusal(await writeConfig(context, "skills", places, before));
    }
    const name = at.id.startsWith("skill-") ? at.id.slice(6) : null;
    const before = known.files!.find(each => each.name === name)?.text;
    if (action.id === "remove") return staleOrRefusal(await asked(context, skillsRoute, "DELETE", `kuvo/skills/${encodeURIComponent(name!)}`, { was: before }));
    const result = readForm(name, values, [...known.own.folders, ...known.engine!.map(each => each.id)]);
    if (!result.ready) return { status: 400, message: result.fields.find(each => each.tone === "warning")?.words ?? "Fill in what it needs first.", fields: [...result.fields] };
    const text = skillDocument(result.name, values.description as string, values.instructions as string, before);
    return staleOrRefusal(await asked(context, skillsRoute, "PUT", `kuvo/skills/${encodeURIComponent(result.name)}`, { text, new: name === null, was: before ?? null }));
  },
  look: () => `${mtime(CONFIG_FILE)} ${mtime(SKILLS_FOLDER)} ${mtime(SKILL_TRASH_FOLDER)} ${Math.floor(Date.now() / 1000)}`,
};
