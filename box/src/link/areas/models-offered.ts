import { acquireEngine, acquireSessionEngine } from "../engine/lifetime";
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
import { modelSpeeds } from "./models-speed";
import { heldRead } from "../vault/holder";
import { CHOOSE_MODEL } from "../work/model-words";
import { askServices, readServices } from "../../shared/box-services";
import { RUN } from "../home";
import { isObject } from "../../shared/json";

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
  const session = query.get("session")?.trim();
  const release = session ? await acquireSessionEngine(session) : await acquireEngine();
  try {
    const folder = await folderIn(request.path);
    if (!targetsOf({ readEngine, bridge }).length) return yield* json(503, { message: "The engine isn't running." });
    let engine: Target | null = null;
    try { engine = readEngine(); } catch {}
    const agents = bridge();
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
    const [lists, plans] = await Promise.all([
      listsHere(engine, agents, place?.data.location?.directory ?? place?.data.directory ?? folder, false, query.get("refresh") === "1"),
      plansHere(signal),
    ]);
    const answer = offeredAnswer(lists, wantedIn(request.path), starredModels(), plans, modelSpeeds());
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
  } finally {
    release();
  }
}

/** A coding agent's account, as a picker tells your accounts with it apart
 * (`models.offered.accounts`): the agent (`claude`), the account's id with it (`default` for its own
 * sign-in, else the one you added's), its agent's name ("Claude Code"), the account's own name (what
 * you called it, else who's signed in to it, else its agent's), and its plan where the box has read
 * it ("Max"). */
export interface OfferedAccount { agent: string; id: string; agentName: string; name: string; plan?: string }

/** Each provider of `models`, named as its side lists its account (`label`: the account's own, a
 * coding agent's second sign-in), in the order its models come; Custom Auto's lists as "Custom
 * Auto"; one its side doesn't name, by its id. A coding agent's says which of your accounts it is
 * (`account`), with its plan from `plans` (by provider). */
export function providersOf(models: { providerID: string; agents: boolean }[], lists: Lists, plans: Map<string, string> = new Map()): { id: string; name: string; label?: string; account?: OfferedAccount }[] {
  const out: { id: string; name: string; label?: string; account?: OfferedAccount }[] = [];
  for (const model of models) {
    if (out.some(each => each.id === model.providerID)) continue;
    if (model.providerID === "auto") { out.push({ id: "auto", name: "Custom Auto" }); continue; }
    const accounts = model.agents ? lists.bridge?.accounts : lists.engine?.accounts;
    const account = accounts?.get(model.providerID);
    const agent = model.agents ? account?.agent : undefined;
    let offered: OfferedAccount | undefined;
    if (account && agent) {
      // (Its agent's own name is its own sign-in's: an added one's is "Claude Code · Work".)
      const own = [...accounts!.values()].find(each => each.agent?.kind === agent.kind && each.agent.id === "default");
      const agentName = agent.id === "default" ? account.name : own?.name ?? account.name;
      const plan = plans.get(model.providerID);
      offered = { agent: agent.kind, id: agent.id, agentName, name: account.label ?? agent.signedInAs ?? (agent.id === "default" ? agentName : account.name), ...(plan ? { plan } : {}) };
    }
    out.push({ id: model.providerID, name: account?.name ?? model.providerID, ...(account?.label ? { label: account.label } : {}), ...(offered ? { account: offered } : {}) });
  }
  return out;
}

/** Each coding agent's account's plan ("Max"), by provider, as this box's usage reader last read
 * it; none where it hasn't, or doesn't answer at once (a picker shows the accounts without). */
export async function plansHere(signal?: AbortSignal): Promise<Map<string, string>> {
  const plans = new Map<string, string>();
  const reader = readServices(RUN);
  if (!reader) return plans;
  try {
    const reply = await askServices(reader, "api/kuvo/limits", { signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(1_500)]) : AbortSignal.timeout(1_500) });
    if (!reply.ok) { await reply.body?.cancel(); return plans; }
    const said: unknown = await reply.json();
    const accounts = isObject(said) && isObject(said.data) && Array.isArray(said.data.accounts) ? said.data.accounts : [];
    for (const each of accounts) {
      if (isObject(each) && typeof each.provider === "string" && typeof each.plan === "string" && each.plan.trim()) plans.set(each.provider, each.plan.trim());
    }
  } catch {}
  return plans;
}

/** The route's answer from what the engine and the bridge list, for the model `wanted`, with your
 * starred models (`starred`, in the order starred: those that don't run here now too, a client
 * shows only those offered), and the speed you picked for each provider (`speeds`, by provider:
 * null for Standard). */
export function offeredAnswer(lists: Lists, wanted: Ref | null, starred: Starred[] = [], plans: Map<string, string> = new Map(), speeds: Record<string, string | null> = {}) {
  const starting = startingModel([wanted], lists);
  const listing = stillListing(lists);
  const offered = offeredHere(lists);
  return {
    // `side`: where a conversation on it runs (the engine, or the coding agents' bridge, Custom
    // Auto's lists too), so a client offers in place only its own side's; `efforts`, its variants;
    // `aliases`, other ids a conversation may name it by (Claude Code's `default`); `free`, it
    // needs no account (OpenCode Zen's free models); `speeds`, those it runs at besides its
    // standard one (Codex's Fast: `models.speed`).
    models: offered.map(model => ({ providerID: model.providerID, id: model.id, name: model.name, side: model.agents ? "agents" as const : "engine" as const, efforts: model.efforts, aliases: model.aliases, ...(model.retiring ? { retiring: true } : {}), ...(model.hidden ? { hidden: true } : {}), ...(!model.agents && needsNoAccount(model) ? { free: true } : {}), ...(model.speeds?.length ? { speeds: model.speeds } : {}) })),
    // (The bridge not running is one that can't say yet: it's starting, or started again.)
    // Each provider's name, as its side names it (`models.offered.providers`), so a picker names
    // where each model is from, with the speed you picked for it (`speed`, `models.speed`: left
    // out for Standard, or none picked); and your starred models (`models.starred`).
    providers: providersOf(offered, lists, plans).map(provider => (speeds[provider.id] ? { ...provider, speed: speeds[provider.id]! } : provider)),
    starred,
    known: { engine: lists.engine !== null, agents: lists.bridge !== null },
    ...(lists.bridge?.generations ? { generations: lists.bridge.generations } : {}),
    ...(lists.bridge?.unavailable?.length ? { unavailable: lists.bridge.unavailable } : {}),
    ...(listing.length ? { listing } : {}),
    ...(starting !== undefined ? { starting: starting?.model ?? null } : {}),
    ...(starting === null ? { startChoice: { label: CHOOSE_MODEL.action.label, refusal: CHOOSE_MODEL.refusal } } : {}),
  };
}
