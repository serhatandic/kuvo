// Custom Auto's ordered fallback lists, described with ordinary lists and forms. All writes
// use the coding agents' existing routes and their per-list revisions.
import type { AutoEntry, AutoList } from "../../agents/auto";
import type { SettingAction, SettingForm, SettingItem, SettingsChecked, SettingsPage, SettingsSection } from "../../shared/routes/settings-pages";
import type { RouteContext } from "../server/context";
import { modelsOfferedRoute } from "./models-offered";
import { agentsRoute } from "../server/forward";
import { asked, item, noActions, row, STALE, refusal, unreadPage } from "./settings-kit";
import type { Refused, Section } from "./settings-kit";

const ROOT = "kuvo/agents/api/kuvo/auto";
const NOTE = "A model at its limit is skipped until the limit resets; an account at its limit skips all its models.";
const EMPTY = "Add the models to go through, in the order you want them. Pick this list in a session's model picker to use it.";
const request = (context: RouteContext, method: string, suffix = "", body?: unknown) => asked(context, agentsRoute, method, ROOT + suffix, body);
async function facts(context: RouteContext) {
  const answer = await request(context, "GET");
  const data = answer.value?.data ?? answer.value;
  if (answer.status !== 200 || !Array.isArray(data?.lists)) return { answer, lists: null, limits: [], models: [] as Model[] };
  const offered = await asked(context, modelsOfferedRoute, "GET", "kuvo/models/offered");
  return { answer, lists: data.lists as AutoList[], limits: Array.isArray(data.limits) ? data.limits : [], models: (Array.isArray(offered.value?.models) ? offered.value.models : []).filter((model: Model) => model.providerID !== "auto") as Model[] };
}
function nameForm(list?: AutoList): SettingForm {
  return { title: list ? list.name : "New List", submit: list ? "Save" : "New List", fields: [{ id: "name", title: "Name", kind: "text", value: list?.name ?? "", line: "Its name, as the model picker shows it" }] };
}
interface Model { providerID: string; id: string; name: string; efforts: string[] }
const modelKey = (entry: AutoEntry) => JSON.stringify([entry.providerID, entry.modelID]);
function entryForm(models: Model[], entry?: AutoEntry): SettingForm {
  const choices = models.map(model => ({ id: JSON.stringify([model.providerID, model.id]), title: `${model.name} (${model.providerID})` }));
  if (entry && !choices.some(choice => choice.id === modelKey(entry))) choices.push({ id: modelKey(entry), title: `${entry.modelID} (${entry.providerID})` });

  return { title: entry ? "Change the model or its effort" : "Add Model", submit: entry ? "Save" : "Add Model", fields: [
    { id: "model", title: "Model", kind: "choice", value: entry ? modelKey(entry) : choices[0]?.id ?? "", choices },
    ...choices.map((choice, index) => {
      const model = models.find(model => JSON.stringify([model.providerID, model.id]) === choice.id);
      const efforts = [...(model?.efforts ?? [])];
      if (entry && choice.id === modelKey(entry) && entry.variant && !efforts.includes(entry.variant)) efforts.push(entry.variant);
      return { id: `effort-${index}`, title: "Effort", kind: "choice" as const, value: entry && choice.id === modelKey(entry) ? entry.variant ?? "" : "", choices: [{ id: "", title: "Default" }, ...efforts.map(id => ({ id, title: id }))], shown: { field: "model", is: [choice.id] } };
    }),
  ] };
}
const rowId = (id: string) => `list-${id}`;
const entryId = (id: string, index: number) => `entry-${encodeURIComponent(id)}-${index}`;
const moveActions = (index: number, length: number): SettingAction[] => [
  { id: "up", title: "Move Up", ...(index === 0 ? { can: { why: "Already first." } } : {}) },
  { id: "down", title: "Move Down", ...(index === length - 1 ? { can: { why: "Already last." } } : {}) },
];
async function page(context: RouteContext): Promise<SettingsPage> {
  const known = await facts(context);
  if (!known.lists) return unreadPage("custom-auto", "Custom Auto", known.answer);
  const lists = known.lists;
  return { id: "custom-auto", title: "Custom Auto", groups: [
    { id: "auto-lists", title: "Custom Auto", note: NOTE, rows: [row({ id: "lists", title: "Lists", kind: "list", empty: "None yet.", add: { id: "add", title: "New List", form: nameForm() }, items: lists.map((list, index) => item({ id: rowId(list.id), title: list.name, kind: "actions", value: { revision: list.revision, index }, actions: [
      { id: "edit", title: "Edit…", form: nameForm(list) }, ...moveActions(index, lists.length),
      { id: "remove", title: "Delete List", style: "destructive", confirm: { title: `Delete ${list.name}?`, body: "This list will no longer be available in the model picker.", button: "Delete List" }, ...(lists.length === 1 ? { can: { why: "Keep one list at least." } } : {}) },
    ] })) })] },
    ...lists.map(list => ({ id: `entries-${list.id}`, title: list.name, rows: [row({ id: `models-${list.id}`, title: list.name, kind: "list", empty: EMPTY, add: { id: "add-model", title: "Add Model", form: entryForm(known.models) }, items: list.entries.map((entry, index) => {
      const held = known.limits.find((limit: any) => limit.key === entry.providerID || limit.key === `${entry.providerID}/${entry.modelID}`);
      return item({ id: entryId(list.id, index), title: `${index + 1}. ${known.models.find(model => model.providerID === entry.providerID && model.id === entry.modelID)?.name ?? entry.modelID}`, line: [entry.providerID, entry.variant].filter(Boolean).join(" · "), kind: "actions", value: { entry, revision: list.revision }, actions: [
        { id: "edit-model", title: "Change the model or its effort", form: entryForm(known.models, entry) }, ...moveActions(index, list.entries.length),
        { id: "remove-model", title: "Remove", style: "destructive", confirm: { title: `Remove ${entry.modelID}?`, body: "Custom Auto will skip this model.", button: "Remove" } },
      ], ...(held ? { badge: { words: held.scope === "provider" ? "Its account is at its limit" : "At its limit", tone: "warning" as const }, notice: { words: `${held.known ? "Until" : "For now (until about"} ${new Date(held.until).toISOString()}${held.known ? "" : ")"}`, tone: "quiet" as const } } : known.models.some(model => model.providerID === entry.providerID && model.id === entry.modelID) ? {} : { notice: { words: "Not available now: Custom Auto passes it by", tone: "quiet" as const } }) });
    }) })] })),
    ...(known.limits.length ? [{ id: "auto-limits", title: "Held back at a limit", rows: [row({ id: "limits", title: "Held back at a limit", line: "If a limit has reset sooner than its agent said, Custom Auto can try them all again.", kind: "actions", actions: [{ id: "clear", title: "Try All Again" }] })] }] : []),
  ] };
}
function readForm(values: Record<string, unknown>, model: boolean, form?: SettingForm): SettingsChecked {
  const keys = model ? ["model"] : ["name"];
  const fields = keys.flatMap(id => typeof values[id] !== "string" || (id !== "variant" && !(values[id] as string).trim()) ? [{ id, words: id === "name" ? "Send a name." : `Give the ${id === "providerID" ? "provider" : id === "modelID" ? "model" : "effort"} as text.`, tone: "warning" as const }] : []);
  if (model && typeof values.model === "string") {
    try { const ref = JSON.parse(values.model); if (!Array.isArray(ref) || ref.length !== 2 || ref.some(value => typeof value !== "string" || !value.trim())) throw new Error(); }
    catch { fields.push({ id: "model", words: "Choose a model.", tone: "warning" }); }
  }
  if (model && form) {
    const models = form.fields.find(field => field.id === "model")?.choices ?? [];
    if (!models.some(choice => choice.id === values.model)) fields.push({ id: "model", words: "Choose one of the offered models.", tone: "warning" });
    const effort = form.fields.find(field => field.shown?.is.includes(String(values.model)));
    if (effort && !effort.choices?.some(choice => choice.id === (values[effort.id] ?? values.variant ?? ""))) fields.push({ id: effort.id, words: "Choose an effort this model supports.", tone: "warning" });
  }
  return { fields, ready: fields.length === 0 };
}
const written = (answer: {status: number; value: any}) => answer.status === 409 ? STALE : refusal(answer);
async function act(context: RouteContext, at: SettingItem, action: SettingAction, values: Record<string, unknown> = {}): Promise<Refused | null> {
  const checked = action.form ? readForm(values, action.id.includes("model"), action.form) : null;
  if (checked && !checked.ready) return { status: 400, message: checked.fields[0]!.words, fields: [...checked.fields] };
  const known = await facts(context);
  if (!known.lists) return refusal(known.answer);
  if (at.id === "limits") return written(await request(context, "DELETE", "/limits"));
  if (at.id === "lists") return written(await request(context, "POST", "", { id: `auto-${crypto.randomUUID().slice(0, 8)}`, name: (values.name as string).trim(), entries: [] }));
  const list = known.lists.find(list => at.id === rowId(list.id) || at.id === `models-${list.id}` || list.entries.some((_, index) => entryId(list.id, index) === at.id));
  if (!list) return STALE;
  const suffix = `/${encodeURIComponent(list.id)}`;
  const save = (patch: object) => request(context, "PUT", suffix, { revision: list.revision, ...patch }).then(written);
  if (at.id === rowId(list.id)) {
    if (action.id === "edit") return save({ name: (values.name as string).trim() });
    if (action.id === "remove") return written(await request(context, "DELETE", suffix, { revision: list.revision }));
    const ids = known.lists.map(list => list.id);
    const index = ids.indexOf(list.id), other = index + (action.id === "up" ? -1 : 1);
    if (other < 0 || other >= ids.length) return { status: 400, message: "Already at that end." };
    [ids[index], ids[other]] = [ids[other]!, ids[index]!];
    return written(await request(context, "PUT", "/order", { ids, was: known.lists.map(list => list.id) }));
  }
  const entries = [...list.entries];
  const index = entries.findIndex((_, index) => entryId(list.id, index) === at.id);
  if (action.id === "add-model" || action.id === "edit-model") {
    const [providerID, modelID] = JSON.parse(values.model as string) as [string, string];
    const field = action.form?.fields.find(field => field.shown?.is.includes(String(values.model)));
    const variant = String((field ? values[field.id] : undefined) ?? values.variant ?? "").trim();
    const entry = { providerID, modelID, ...(variant ? { variant } : {}) };
    if (action.id === "add-model") entries.push(entry); else entries[index] = entry;
  } else if (action.id === "remove-model") entries.splice(index, 1);
  else {
    const other = index + (action.id === "up" ? -1 : 1);
    if (index < 0 || other < 0 || other >= entries.length) return { status: 400, message: "Already at that end." };
    [entries[index], entries[other]] = [entries[other]!, entries[index]!];
  }
  return save({ entries });
}
export const customAutoSection: Section & { listed: SettingsSection } = {
  listed: { id: "custom-auto", title: "Custom Auto", group: "Models", icon: "sliders", line: "Your lists of models to go through in order when one reaches a usage limit.", keywords: ["auto", "models", "fallback", "limits", "effort"] },
  read: page, change: noActions, act, check: async (_context, _at, action, values) => readForm(values, action.id.includes("model"), action.form),
  look: () => String(Math.floor(Date.now() / 1000)),
};
