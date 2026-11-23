import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { placeWhole } from "../../shared/files";
import { marketplaceScope, MARKETPLACE_SCOPE, type MarketplaceScope } from "../../shared/marketplace";
import type { Target } from "../../shared/records";
import { botModel, isModel } from "../bots/here";
import { runsWithAgents } from "../bots/kit";
import { acquireEngine } from "../engine/lifetime";
import { listBots } from "../bots/list";
import { makeBot } from "../bots/make";
import { BOTS, HERE } from "../home";
import { listsHere, stateOf } from "../work/model-rule";
import { seedRoutine } from "../areas/wakes";
import { offeredAnswer, plansHere } from "../areas/models-offered";
import { catalog, type Agent } from "./catalog";
import { connectionState, serverName } from "./connections";
import { installations, no, type Installations } from "./installations";
import type { Acted } from "../work-queue/forward";
import { listOf, textOf } from "../files";

const requestID = /^[A-Za-z0-9_-]{22,43}$/;

export function projectBotScope(directory: string, scope: MarketplaceScope, state: Installations): void {
  const file = path.join(directory, "opencode.json");
  const was = textOf(file);
  let config: Record<string, unknown> = {};
  if (was !== null) {
    const parsed: unknown = JSON.parse(was);
    if (!isObject(parsed) || parsed.policies !== undefined && !Array.isArray(parsed.policies)) throw new Error("The workspace’s settings could not be read.");
    config = parsed;
  }
  // Use plain engine policy records. The preceding projection is remembered separately so other rules stay.
  const record = path.join(directory, ".kuvo-marketplace-policies.json");
  const before: unknown = JSON.parse(textOf(record) ?? "[]");
  const previous = new Set(Array.isArray(before) ? before : []);
  const policies = (Array.isArray(config.policies) ? config.policies : []).filter(rule => !previous.has(JSON.stringify(rule)));
  const rules = [
    { action: "mcp.use", resource: serverName("*"), effect: "deny" },
    ...scope.apps.map(id => ({ action: "mcp.use", resource: serverName(id), effect: "allow" })),
    ...state.skills.filter(each => !scope.skills.includes(each.name)).map(each => ({ action: "skill.use", resource: each.name, effect: "deny" })),
  ];
  const output = JSON.stringify({ ...config, policies: [...policies, ...rules] }, null, 2) + "\n";
  if (output === was) return;
  const projected = rules.map(rule => JSON.stringify(rule));
  placeWhole(record, JSON.stringify([...new Set([...previous, ...projected])]));
  placeWhole(file, output);
  placeWhole(record, JSON.stringify(projected));
}

export function projectBotScopes(state: Installations): string[] {
  const problems: string[] = [];
  for (const id of listOf(BOTS)) {
    if (!id.startsWith("bot_")) continue;
    const directory = path.join(BOTS, id, "workspace");
    const scope = marketplaceScope(directory);
    if (!scope) continue;
    try { projectBotScope(directory, scope, state); }
    catch { problems.push(`Couldn’t refresh Marketplace tools for ${id}.`); }
  }
  return problems;
}

export async function botOptions(agent: Agent, engine: Target | null, bridge: Target | null): Promise<Acted> {
  const apps = installations().apps;
  const [lists, plans] = await Promise.all([listsHere(engine, bridge), plansHere()]);
  const offered = offeredAnswer(lists, agent.suggestedModel, [], plans);
  const missing = agent.apps.flatMap(id => {
    const app = apps.find(each => each.id === id);
    return !app || connectionState(app) !== "ready" ? [{ id, title: catalog().apps.find(each => each.id === id)?.title ?? id }] : [];
  });
  const models = offered.models.filter(model => model.providerID !== "auto" && !model.retiring && !model.hidden && stateOf(model, lists)?.runs === true);
  return { status: 200, value: { ...offered, models, missing, routines: agent.routines.map(routine => ({ ...routine, enabled: false })) } };
}

export async function addBot(agent: Agent, body: Record<string, unknown>, readEngine: () => Target, readBridge: () => Target | null): Promise<Acted> {
  const request = body.request;
  if (typeof request !== "string" || !requestID.test(request) || !isModel(body.model) || body.model.providerID === "auto"
    || body.routine !== undefined && typeof body.routine !== "boolean"
    || body.routines !== undefined && !(Array.isArray(body.routines) && body.routines.every(id => agent.routines.some(routine => routine.id === id))))
    return no(400, "Choose a model and account, and send the original Add request ID.");
  if (body.version !== agent.version) return no(409, "This agent changed. Open its details and review the new version.");
  const existing = listBots().find(bot => marketplaceScope(path.join(BOTS, bot.id, "workspace"))?.request === request);
  if (existing) {
    const scope = marketplaceScope(path.join(BOTS, existing.id, "workspace"));
    return scope?.agent === agent.id ? { status: 201, value: { bot: existing, firstMessage: null } } : no(409, "Use a new request ID for another agent.");
  }
  const state = installations();
  const available = (id: string) => state.apps.some(app => app.id === id && connectionState(app) === "ready");
  if (agent.apps.some(id => !available(id))) return no(409, `Connect the required apps on ${HERE} before adding this agent.`);
  if (agent.skills.some(id => !state.skills.some(skill => skill.name === id))) return no(409, "This computer is still receiving the required skills. Try again shortly.");
  let engine: Target | null = null;
  const release = runsWithAgents(body.model) ? undefined : await acquireEngine();
  let listed: Awaited<ReturnType<typeof listsHere>>;
  try {
    try { engine = readEngine(); } catch {}
    listed = await listsHere(engine, readBridge());
  } finally { release?.(); }
  if (stateOf(botModel(body.model), listed)?.runs !== true) return no(422, "Choose a model and account available on this computer.");
  const scope = { agent: agent.id, version: agent.version, request, apps: agent.apps, skills: agent.skills };
  return makeBot({ request, name: body.name ?? agent.title, model: body.model }, readEngine, readBridge, undefined, {
    job: { line: agent.summary, details: agent.instructions },
    async prepare(folder, directory) {
      placeWhole(path.join(directory, MARKETPLACE_SCOPE), JSON.stringify(scope));
      projectBotScope(directory, scope, state);
      if (!agent.routines.length) return;
      // `routines`: the ones turned on, scheduled and running; none on, every one scheduled, paused.
      const chosen = Array.isArray(body.routines) ? agent.routines.filter(routine => (body.routines as unknown[]).includes(routine.id)) : [];
      if (chosen.length) {
        for (const routine of chosen) seedRoutine(folder, routine);
        return;
      }
      fs.writeFileSync(path.join(folder, "paused"), "paused");
      for (const routine of agent.routines) seedRoutine(folder, routine);
      if (body.routine === true) fs.rmSync(path.join(folder, "paused"));
    },
  });
}
