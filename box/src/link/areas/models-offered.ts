// The models a session can use here, and the one a new session starts on (PROTOCOL.md, "The models
// offered"): `GET kuvo/models/offered[?folder=<path>]`, decided by the box's one rule
// (work/model-rule.ts), so every client's picker offers the same, and a new session starts by the
// same rule whoever starts it, in a project's folder by what its own settings add too.

import { sessionHere, Unreached } from "./glance";
import fs from "node:fs";
import path from "node:path";
import type { Target } from "../../shared/records";
import { keptList } from "../lists/sessions";
import { readProjects } from "./projects";
import { readWorktrees } from "./worktree-records";
import { targetsOf } from "../letters/tools";
import { listsHere, needsNoAccount, offeredHere, startingModel, stillListing, type Lists, type Ref } from "../work/model-rule";
import type { AnswerPiece, RouteContext } from "../server/context";
import { starredModels, type Starred } from "./models-starred";
import { heldRead } from "../vault/holder";
import { CHOOSE_MODEL } from "../work/model-words";

export const MODELS_OFFERED_ROUTE = "kuvo/models/offered";

/** The model a client would start a new session on (`provider`, `model`, `variant`: the one picked
 * last there); none named, none wanted. */
function wantedIn(requestPath: string): Ref | null {
  const query = new URLSearchParams(requestPath.split("?")[1] ?? "");
  const providerID = query.get("provider")?.trim(), id = query.get("model")?.trim(), variant = query.get("variant")?.trim();
  return providerID && id ? { providerID, id, ...(variant ? { variant } : {}) } : null;
}

/** The folder asked about (`folder`: a project's, or a conversation's), where the engine's lists
 * are read, a project's own settings adding providers, models or its default: only one this box
 * knows (a project's, one of its worktrees, or a session's) that's still a folder here. None named,
 * or any other (a worktree removed since, a folder of no project), the box's own: the engine isn't
 * started anywhere it wouldn't be otherwise. */
async function folderIn(requestPath: string): Promise<string | undefined> {
  const folder = new URLSearchParams(requestPath.split("?")[1] ?? "").get("folder")?.trim();
  if (!folder || !path.isAbsolute(folder)) return undefined;
  const resolved = path.resolve(folder);
  if (!isKnown(resolved)) return undefined;
  return (await fs.promises.stat(resolved).then(found => found.isDirectory(), () => false)) ? resolved : undefined;
}

/** Whether `folder` (resolved) is one of this box's: a project's not removed (or one of its
 * folders), a worktree it keeps, or one of its sessions' as its list has them; each as resolved
 * too (one kept with a trailing "/" is the same folder). */
function isKnown(folder: string): boolean {
  const same = (other: unknown) => typeof other === "string" && path.isAbsolute(other) && path.resolve(other) === folder;
  if (readProjects().some(project => !project.removed && (same(project.directory) || project.folders.some(same)))) return true;
  if (readWorktrees().some(worktree => same(worktree.root) || same(worktree.folder))) return true;
  const list = keptList();
  return list ? Object.values(list.sessions).some(each => same(each.facts.directory)) : false;
}

export async function* modelsOfferedRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, json, signal } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const query = new URLSearchParams(request.path.split("?")[1] ?? "");
  if (query.get("refresh") === "1" && (yield* heldRead(request, signal))) return;
  const folder = await folderIn(request.path);
  if (!targetsOf({ readEngine, bridge }).length) return yield* json(503, { message: "The engine isn't running." });
  let engine: Target | null = null;
  try { engine = readEngine(); } catch {}
  const agents = bridge();
  const session = query.get("session")?.trim();
  let place: Awaited<ReturnType<typeof sessionHere>> = null;
  if (session) {
    try {
      place = await sessionHere([engine, agents].filter((target): target is Target => target !== null), session, signal);
    } catch (error) {
      if (error instanceof Unreached) return yield* json(503, { message: error.message });
      throw error;
    }
    if (!place) return yield* json(404, { message: "This conversation is no longer available." });
  }
  const lists = await listsHere(engine, agents, place?.data.location?.directory ?? place?.data.directory ?? folder, false, query.get("refresh") === "1");
  const answer = offeredAnswer(lists, wantedIn(request.path), starredModels());
  if (query.get("for") === "defaults") {
    const models = answer.models.filter(model => model.side === "engine" && !model.hidden && !model.retiring);
    return yield* json(200, { ...answer, models, scoped: true });
  }
  if (!place) return yield* json(200, answer);
  const bot = typeof place.data.metadata?.kuvo?.bot === "string";
  const side = agents && place.target.port === agents.port ? "agents" : "engine";
  return yield* json(200, {
    ...answer,
    scoped: true,
    models: side === "agents" ? answer.models : answer.models.filter(model => model.side === side),
    ...(side === "engine" ? { note: bot
      ? "To use a coding agent in this bot, change its model in Edit Bot."
      : "This conversation uses provider models. Start a new conversation to use a coding agent’s models." } : {}),
  });
}

/** Each provider of `models`, named as its side lists its account (`label`: the account's own, a
 * coding agent's second sign-in), in the order its models come; Custom Auto's lists as "Custom
 * Auto"; one its side doesn't name, by its id. */
export function providersOf(models: { providerID: string; agents: boolean }[], lists: Lists): { id: string; name: string; label?: string }[] {
  const out: { id: string; name: string; label?: string }[] = [];
  for (const model of models) {
    if (out.some(each => each.id === model.providerID)) continue;
    if (model.providerID === "auto") { out.push({ id: "auto", name: "Custom Auto" }); continue; }
    const account = (model.agents ? lists.bridge?.accounts : lists.engine?.accounts)?.get(model.providerID);
    out.push({ id: model.providerID, name: account?.name ?? model.providerID, ...(account?.label ? { label: account.label } : {}) });
  }
  return out;
}

/** The route's answer from what the engine and the bridge list, for the model `wanted`, with your
 * starred models (`starred`, in the order starred: those that don't run here now too, a client
 * shows only those offered). */
export function offeredAnswer(lists: Lists, wanted: Ref | null, starred: Starred[] = []) {
  const starting = startingModel([wanted], lists);
  const listing = stillListing(lists);
  const offered = offeredHere(lists);
  return {
    // `side`: where a conversation on it runs (the engine, or the coding agents' bridge, Custom
    // Auto's lists too), so a client offers in place only its own side's; `efforts`, its variants;
    // `aliases`, other ids a conversation may name it by (Claude Code's `default`); `free`, it
    // needs no account (OpenCode Zen's free models).
    models: offered.map(model => ({ providerID: model.providerID, id: model.id, name: model.name, side: model.agents ? "agents" : "engine", efforts: model.efforts, aliases: model.aliases, ...(model.retiring ? { retiring: true } : {}), ...(model.hidden ? { hidden: true } : {}), ...(!model.agents && needsNoAccount(model) ? { free: true } : {}) })),
    // (The bridge not running is one that can't say yet: it's starting, or started again.)
    // Each provider's name, as its side names it (`models.offered.providers`), so a picker names
    // where each model is from; and your starred models (`models.starred`).
    providers: providersOf(offered, lists),
    starred,
    known: { engine: lists.engine !== null, agents: lists.bridge !== null },
    ...(lists.bridge?.generations ? { generations: lists.bridge.generations } : {}),
    ...(lists.bridge?.unavailable?.length ? { unavailable: lists.bridge.unavailable } : {}),
    ...(listing.length ? { listing } : {}),
    ...(starting !== undefined ? { starting: starting?.model ?? null } : {}),
    ...(starting === null ? { startChoice: { label: CHOOSE_MODEL.action.label, refusal: CHOOSE_MODEL.refusal } } : {}),
  };
}
