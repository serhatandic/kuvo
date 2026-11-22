// A conversation's subagents, from the box's session list and background work snapshots.
import type { Subagents } from "../../shared/routes/work";
import { modelName } from "../../usage/insights-names";
import { keptList, type Kept } from "../lists/sessions";
import { readSessionBackground, WORK_BACKGROUND } from "./background";
import { finishedState } from "./listed-work";
import { lastName } from "./model-rule";

const DEPTH = 3;
const MOST = 200;
const DONE_MOST = 100;

export function subagentsOf(session: string, { file = WORK_BACKGROUND, list = keptList() }: { file?: string; list?: Kept | null } = {}): Subagents {
  const listed = list?.sessions ?? {};
  const displayModel = (id: string) => {
    const name = modelName(id).name;
    return /(?:^|[./-])claude(?:-|$)/i.test(id) && !name.startsWith("Claude") ? `Claude ${name}` : name;
  };
  const modelOf = (id: string) => {
    const work = listed[id]?.work;
    if (work?.ranOn) return displayModel(work.ranOn);
    if (work?.requestedModel) return `${displayModel(work.requestedModel)} (requested)`;
    return work?.model ? lastName(work.model) ?? displayModel(work.model.id) : undefined;
  };
  const titleOf = (id: string) => listed[id]?.facts.title?.trim() || "Subagent";
  const working = readSessionBackground(session, file).subagents;
  const active = working.map(({ id, since }) => {
    const model = modelOf(id);
    return { session: id, title: titleOf(id), since, asking: listed[id]?.facts.asked !== undefined, ...(model ? { model } : {}) };
  }).sort((a, b) => b.since - a.since || b.session.localeCompare(a.session));
  const atWork = new Set(working.map(each => each.id));
  const seen = new Set([session]);
  const done: Subagents["done"][number][] = [];
  let above = [session];
  for (let depth = 0; depth < DEPTH && above.length && seen.size <= MOST; depth++) {
    const children = above.flatMap(id => (listed[id]?.facts.subagents ?? []).slice(0, 100));
    above = [];
    for (const id of children) {
      if (seen.size > MOST) break;
      if (seen.has(id) || !listed[id]) continue;
      seen.add(id);
      above.push(id);
      const { facts, work } = listed[id];
      if (atWork.has(id) || facts.running || facts.away || facts.reviewPart) continue;
      const state = finishedState(work?.outcome, work?.idle ?? facts.unread);
      if (state !== "finished" && state !== "failed" && state !== "stopped") continue;
      const model = modelOf(id);
      done.push({ session: id, title: titleOf(id), started: facts.created, ended: work?.idle ?? facts.unread ?? facts.updated, state, ...(model ? { model } : {}) });
    }
  }
  done.sort((a, b) => b.ended - a.ended || b.session.localeCompare(a.session));
  return { active, done: done.slice(0, DONE_MOST) };
}
