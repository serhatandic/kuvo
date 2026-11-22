// Work fields kept beside the session list. The list keeper already reads each place and takes
// session events, so a glance can use its last answer without asking the place again.
import path from "node:path";
import type { SessionFacts } from "../../shared/routes/sessions";
import type { WorkAnswer, WorkSession } from "../../shared/routes/work";
import type { EngineSession } from "../engine/sessions";
import { isHiddenRun } from "../engine/sessions";
import { answersOn } from "./answers";
import { depthOf, startedByOf, worksFor, type Listed } from "./tree";

export interface ListedWork {
  idle?: number;
  outcome?: string;
  model?: WorkSession["model"];
  ranOn?: string;
  bot?: string;
  starter?: string;
  depth?: number;
  mode?: string;
  tokens?: number;
  cost?: number;
  answered?: WorkAnswer[];
}

const details = new WeakMap<EngineSession, ListedWork | undefined>();

export function listedWork(data: Listed["data"]): ListedWork | undefined {
  if (details.has(data)) return details.get(data);
  const starter = startedByOf(data);
  const bot = !data.parentID && starter ? worksFor(data)?.bot : undefined;
  let row: ListedWork | undefined;
  if (bot || (data.parentID && !isHiddenRun(data))) {
    const model = data.model;
    const ranOn = data.metadata?.kuvo?.ranOn;
    row = {
      idle: data.time?.idle, outcome: data.outcome,
      ...(model?.id && model.providerID ? { model: { id: model.id, providerID: model.providerID, ...(model.variant ? { variant: model.variant } : {}) } } : {}),
      ...(data.parentID && typeof ranOn === "string" && ranOn.trim() ? { ranOn: ranOn.trim() } : {}),
    };
    if (bot && starter) {
      const answered = answersOn(data.metadata);
      Object.assign(row, {
        bot, starter, depth: depthOf(data) || 1, mode: data.agent,
        tokens: (data.tokens?.input ?? 0) + (data.tokens?.output ?? 0) + (data.tokens?.reasoning ?? 0),
        ...(typeof data.cost === "number" && data.cost > 0 ? { cost: data.cost } : {}),
        ...(answered.length ? { answered } : {}),
      });
    }
  }
  details.set(data, row);
  return row;
}

export function workBot(facts: SessionFacts, work?: ListedWork): string | undefined {
  if (facts.away || facts.archived || facts.parent) return undefined;
  return work?.bot ?? (facts.line?.length ? facts.bot : undefined);
}

export function finishedState(outcome?: string, idle?: number): WorkSession["state"] {
  if (outcome === "failed") return "failed";
  if (outcome === "interrupted") return "stopped";
  return outcome === "succeeded" || idle ? "finished" : "idle";
}

export function workRow(
  session: string, facts: SessionFacts, work: ListedWork | undefined, projectName: string | undefined,
  { state, asks, stopped }: { state?: "working" | "asking"; asks?: string; stopped?: number },
): WorkSession {
  const ended = work?.idle ?? facts.unread;
  const project = facts.kind === "chat" ? undefined : projectName || path.basename(facts.project ?? facts.directory);
  return {
    session, title: facts.title || "New Session", folder: facts.directory,
    ...(project ? { project } : {}),
    startedBy: work?.starter ?? facts.line?.at(-1) ?? "",
    depth: work?.depth ?? facts.line?.length ?? 1,
    ...(work?.model ? { model: work.model } : {}),
    ...(work?.mode ? { mode: work.mode } : {}),
    state: state ?? (stopped !== undefined && (ended ?? 0) < stopped ? "stopped" : finishedState(work?.outcome, ended)),
    ...(asks ? { asks } : {}),
    started: facts.created, tokens: work?.tokens ?? 0,
    ...(work?.cost ? { cost: work.cost } : {}),
  };
}
