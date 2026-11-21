// A conversation's subagents, at work and done (PROTOCOL.md, "Work › Background work", Subagents),
// as a side panel lists them: those at work under it, as Background work finds them, and the ones
// whose work ended, read from where they ran (the engine's sessions under it, and the coding agents'
// subagents, which their bridge keeps as sessions under it too), each with how it ended. Worked out
// here, so no client groups, filters or judges them.
//
// Platform: only the engine's and the bridge's APIs: the same on macOS and Linux.

import { isObject } from "../../shared/json";
import type { Target } from "../../shared/records";
import type { Subagents } from "../../shared/routes/work";
import { modelOf } from "../areas/session-models";
import { call } from "../engine/client";
import { isHiddenRun, type EngineSession } from "../engine/sessions";
import { keptList, type Kept } from "../lists/sessions";
import { readSessionBackground, WORK_BACKGROUND } from "./background";

/** How far down the sessions under a conversation are looked for (a subagent's own, and theirs),
 * and how many at most. */
const DEPTH = 3;
const MOST = 200;
/** How many that ended are listed at most, the latest. */
export const DONE_MOST = 100;

type Session = Partial<EngineSession> & { id: string; outcome?: unknown };
/** Where a conversation's subagents can be: the engine, and the coding agents' bridge, each while it runs. */
export interface SubagentPlaces { engine: Target | null; bridge: Target | null }

/** The sessions under `session` at `target`, its subagents and theirs, a few deep; null when it
 * couldn't be read. */
async function sessionsUnder(target: Target, session: string, signal?: AbortSignal): Promise<Session[] | null> {
  const found: Session[] = [];
  let above = [session];
  for (let depth = 0; depth < DEPTH && above.length && found.length < MOST; depth++) {
    const pages = await Promise.all(above.map(parent =>
      call(target, "GET", `/api/session?parentID=${encodeURIComponent(parent)}&limit=100&order=desc`, undefined, signal ? AbortSignal.any([signal, AbortSignal.timeout(10_000)]) : AbortSignal.timeout(10_000)).then(
        page => (Array.isArray(page) ? page : []).filter((each): each is Session => isObject(each) && typeof each.id === "string" && each.parentID === parent),
        () => null,
      )));
    if (pages.includes(null)) return null;
    above = [];
    for (const each of (pages as Session[][]).flat()) {
      if (found.length >= MOST || found.some(seen => seen.id === each.id)) continue;
      found.push(each);
      above.push(each.id);
    }
  }
  return found;
}

/** How a subagent's work ended, as its place says: null for one that hasn't ended (or never began). */
function howEnded(data: Session): Subagents["done"][number]["state"] | null {
  if (data.outcome === "failed") return "failed";
  if (data.outcome === "interrupted") return "stopped";
  return data.outcome === "succeeded" || typeof data.time?.idle === "number" ? "finished" : null;
}

const titleOf = (data: Session | undefined, listed: string | undefined) => data?.title?.trim() || listed?.trim() || "Subagent";

/** `session`'s subagents: at work now (`active`, as `kuvo/work/background/<session>` lists them, with
 * their titles and whether one waits on you, as the session lists say), and done (`done`, those
 * under it whose work ended, a review's parts and other hidden runs left out), each the latest
 * first, with the model each ran on where the box names it. Null while a place that runs didn't
 * answer, so what was said before stands. */
export async function subagentsOf(session: string, places: SubagentPlaces, signal?: AbortSignal, { file = WORK_BACKGROUND, list = keptList() }: { file?: string; list?: Kept | null } = {}): Promise<Subagents | null> {
  const targets = [places.engine, places.bridge].filter((each): each is Target => each !== null);
  const read = await Promise.all(targets.map(async target => ({ target, under: await sessionsUnder(target, session, signal) })));
  if (read.some(each => each.under === null)) return null;
  const at = new Map<string, { target: Target; data: Session }>();
  for (const { target, under } of read) for (const data of under!) if (!at.has(data.id)) at.set(data.id, { target, data });
  // (Named as the box names a session's model, where it can say now.)
  const modelNamed = async (id: string): Promise<{ model?: string }> => {
    const place = at.get(id);
    const said = place ? await modelOf(place, places.engine, places.bridge).catch(() => undefined) : undefined;
    return said?.name ? { model: said.name } : {};
  };
  const listed = list?.sessions ?? {};
  const working = readSessionBackground(session, file).subagents;
  const active = await Promise.all(working.map(async each => {
    const facts = listed[each.id]?.facts;
    return { session: each.id, title: titleOf(at.get(each.id)?.data, facts?.title), since: each.since, asking: facts?.asked !== undefined, ...(await modelNamed(each.id)) };
  }));
  const atWork = new Set(working.map(each => each.id));
  const ended = [...at.values()].flatMap(({ data }) => {
    const state = atWork.has(data.id) || isHiddenRun(data) ? null : howEnded(data);
    return state ? [{ data, state }] : [];
  });
  const endedAt = (data: Session) => data.time?.idle ?? data.time?.updated ?? data.time?.created ?? 0;
  const latest = ended.sort((a, b) => endedAt(b.data) - endedAt(a.data) || (a.data.id < b.data.id ? 1 : -1)).slice(0, DONE_MOST);
  const done = await Promise.all(latest.map(async ({ data, state }) => ({
    session: data.id,
    title: titleOf(data, listed[data.id]?.facts.title),
    started: data.time?.created ?? 0,
    ended: endedAt(data),
    state,
    ...(await modelNamed(data.id)),
  })));
  return { active: active.sort((a, b) => b.since - a.since || (a.session < b.session ? 1 : -1)), done };
}

/** What `subagentsOf` goes by that the box knows without asking the engine: which are at work, and
 * what the session lists say of those and of the ones under it (their titles, whether they wait on
 * you, whether they run). A follower reads again as this changes. */
export function subagentsSaid(session: string, file = WORK_BACKGROUND): string {
  const working = readSessionBackground(session, file).subagents;
  const ids = new Set(working.map(each => each.id));
  const under = Object.entries(keptList()?.sessions ?? {})
    .filter(([id, entry]) => entry.facts.parent === session || ids.has(id))
    .map(([id, { facts }]) => [id, facts.title, facts.asked, facts.running]);
  return JSON.stringify([working, under]);
}
