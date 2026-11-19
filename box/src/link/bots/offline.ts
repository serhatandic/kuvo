import { plainName } from "../../shared/text";
import { isObject } from "../../shared/json";
import type { Target } from "../../shared/records";
import { editView, fingerprint, mergeEdit, observe, rememberEdit, replay, type EditHistory, type Fields } from "../ownership/merge";
import type { EditReceipt, EditTarget, OfflineEdit } from "../ownership/edit-types";
import { Refusal } from "../work-queue/store";
import { changeBot } from "./edit";
import { botHere, botLook, botModel, changeDescription, isModel, isMoving, type BotDescription, type BotHere } from "./here";
import { fitsLook } from "./make";
import { readKit } from "./kit";
import { botPersonalities } from "../personalities/library";
import { limitsFrom } from "../work/limits";
import { givenJobFields, jobOf, type Job } from "./jobs";

const editable = ["name", "look", "personality", "jobLine", "jobDetails", "model", "limits"];
const botFields = (here: BotHere, job: Job): Fields => ({
  name: here.described.name,
  look: here.described.look ?? null,
  personality: here.described.personality ?? null,
  model: here.described.model ?? null,
  limits: isObject(here.described.work) ? here.described.work.limits ?? null : null,
  jobLine: job.line,
  jobDetails: job.details,
});

const botPassage = (field: string) => field === "jobDetails";

function botHistory(here: BotHere, job: Job) {
  return observe(here.described.offlineEdits as EditHistory | undefined, here.described.id, botFields(here, job), false, undefined, botPassage);
}

export function botEditView(ref: EditTarget, owns: (ref: EditTarget) => boolean) {
  const here = botHere(ref.id);
  if (!here || !owns(ref) || isMoving(here.folder)) return null;
  jobOf(here); // Import an old job before taking the description lock.
  let view: ReturnType<typeof editView> | undefined;
  changeDescription(here.folder, now => {
    if (!now || !owns(ref) || isMoving(here.folder)) throw new Refusal("The bot is no longer here.");
    const latest = { ...here, described: now };
    const history = botHistory(latest, jobOf(latest, false));
    view = editView(history);
    return history === now.offlineEdits ? now : { ...now, offlineEdits: history };
  }, true);
  return view;
}

export async function editBot(edit: OfflineEdit, owns: (ref: EditTarget) => boolean, engine: () => Target, bridge: () => Target | null): Promise<EditReceipt | false | undefined> {
  const { ref, operation } = edit;
  const here = botHere(ref.id);
  if (!here || !owns(ref) || isMoving(here.folder)) return;
  if (Object.keys(edit.fields ?? {}).some(field => !editable.includes(field))) throw new Refusal("That is not an offline bot setting.");
  const digest = fingerprint(edit);
  const history = botHistory(here, jobOf(here));
  const earlier = replay(history, operation, digest);
  if (earlier !== undefined) return earlier;
  const fields = { ...edit.fields };
  const normal = givenJobFields(fields);
  if (normal.error) throw new Refusal(normal.error);
  Object.assign(fields, normal.job);
  if (Object.hasOwn(fields, "name")) {
    fields.name = plainName(fields.name);
    if (!fields.name) throw new Refusal("A bot needs a name.");
  }
  if (Object.hasOwn(fields, "model")) {
    if (!isModel(fields.model)) throw new Refusal("That isn't a bot model.");
    fields.model = botModel(fields.model);
  }
  if (Object.hasOwn(fields, "limits")) fields.limits = limitsFrom(fields.limits);
  if (Object.hasOwn(fields, "look") || Object.hasOwn(fields, "personality")) {
    const kit = readKit();
    if (!kit) return;
    if (Object.hasOwn(fields, "look")) {
      if (!fitsLook(kit.looks, fields.look)) throw new Refusal("That isn't a look a bot can have.");
      fields.look = botLook(fields.look);
    }
    if (Object.hasOwn(fields, "personality") && fields.personality !== null
      && !botPersonalities(kit).some(personality => personality.id === fields.personality)) throw new Refusal("That personality is unavailable.");
  }
  const merged = mergeEdit(history, { ...edit, fields }, botPassage);
  const changes = { ...merged.changes };
  if ("limits" in changes) {
    changes.work = { limits: changes.limits };
    delete changes.limits;
  }

  let decision: EditReceipt | undefined;
  const commit = (now: BotDescription | null, next: BotDescription, job?: Job) => {
    if (!now || !owns(ref) || isMoving(here.folder)) throw new Refusal("The bot is no longer here.");
    const current = { ...here, described: now };
    const currentJob = jobOf(current, false);
    const latest = botHistory(current, currentJob);
    // Another process, including a legacy writer, may have edited while the engine was checked.
    if (fingerprint(latest) !== fingerprint(history)) throw new Refusal("The bot changed while the edit was being checked.");
    const after = job ? { ...next, offlineJob: job, job: job.line } : next;
    const observed = observe({ ...latest, choices: merged.choices }, ref.id!,
      botFields({ ...here, described: after }, job ?? currentJob), false, undefined, botPassage);
    decision = rememberEdit(observed, operation, digest, merged.status, merged.added);
    return { ...after, offlineEdits: observed };
  };

  if (!Object.keys(changes).length) {
    try { changeDescription(here.folder, now => commit(now, now!), true); } catch (error) {
      if (!(error instanceof Refusal)) throw error;
      return;
    }
  } else {
    const answer = await changeBot(ref.id!, changes, engine, bridge, undefined, undefined, Date.now(), commit);
    if (answer.status >= 400 && answer.status < 500 && answer.status !== 409 && "message" in answer.value)
      throw new Refusal(answer.value.message);
    if (answer.status !== 200) return;
  }
  return decision;
}
