// What's at work in Kuvo on this computer, for what's always on screen there (PROTOCOL.md, "In the
// menu bar", `kuvo/box/at-work`), and stopping it until it next starts (`kuvo/box/stop`). On a Mac
// whose box lives in a home of its own, Kuvo Helper runs it and draws these facts in the menu bar,
// so that you can see Kuvo still works with the app closed, open the app, or stop it. Every fact is
// the box's: its session list (`lists/sessions.ts`), what waits on you (`areas/waiting.ts`), the
// work its sessions left running (`work/background.ts`), and its plan limits' rings (the coding
// agents' bridge). The helper only draws them.
//
// Platform: the facts are the same on macOS and Linux (a machine's box answers `applies: false`:
// nothing of its own is on its screen). Stopping is a Mac's own box's alone: elsewhere nothing
// starts it again at the next login but the machine's own start, so it's refused there.

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../shared/json";
import type { Target } from "../shared/records";
import type { AtWork } from "../shared/routes/box";
import type { SessionFacts } from "../shared/routes/sessions";
import { waitingOf } from "./areas/waiting";
import { botFollowers, botHere } from "./bots/here";
import { call } from "./engine/client";
import { followedLive } from "./follow";
import { APP_FOLDER, K, recordedKind, RUN } from "./home";
import { keptList, listFollowers, listNow, type Kept } from "./lists/sessions";
import { backgroundFollowers, readBackground } from "./work/background";
import type { AnswerPiece, RouteContext } from "./server/context";

export const AT_WORK_ROUTE = /^kuvo\/box\/at-work(\/events)?$/;

/** The most sessions at work, and waiting on you, named (each is counted). */
export const MOST_NAMED = 20;

/** The mark `kuvo-start pause` leaves while Kuvo on this Mac is stopped until it next starts. */
export const PAUSED = (): string => path.join(RUN, "paused");

/** Whether this box is one whose helper draws it in the menu bar: a Mac's own box in a home of its own. */
export const inMenuBar = (): boolean => process.platform === "darwin" && !APP_FOLDER && recordedKind() === "mac";

const titleOf = (facts: SessionFacts): string | undefined => {
  const title = (facts.title ?? "").replace(/\s+/g, " ").trim();
  return title ? title.slice(0, 200) : undefined;
};

/** A bot's name by its id, as its folder here says; none for one that lives elsewhere. */
const botName = (id: string | undefined): string | undefined => (id ? botHere(id)?.described.name : undefined);

/** One of an account's rings, as the bridge's limits say it; none where it can't be read. */
function ringOf(said: unknown): AtWork["rings"][number]["short"] {
  if (!isObject(said) || typeof said.used !== "number" || !Number.isFinite(said.used) || typeof said.level !== "string") return undefined;
  return { used: Math.min(1, Math.max(0, said.used)), level: said.level };
}

/** Every account's rings, in the order the bridge lists them (its limits' `rows`); none while it
 * doesn't run or doesn't answer within five seconds. */
export async function ringsOf(bridge: Target | null, signal?: AbortSignal): Promise<AtWork["rings"]> {
  if (!bridge) return [];
  const limits = await call(bridge, "GET", "/api/kuvo/limits", undefined, signal ? AbortSignal.any([signal, AbortSignal.timeout(5_000)]) : AbortSignal.timeout(5_000)).catch(() => null);
  const rows = isObject(limits) && Array.isArray(limits.rows) ? limits.rows : [];
  return rows.flatMap(row => {
    if (!isObject(row) || typeof row.service !== "string" || typeof row.title !== "string") return [];
    // A plan that ended has no ring (the bridge's `ended`).
    if (row.ended === true) return [];
    const rings = isObject(row.rings) ? row.rings : {};
    const short = ringOf(rings.short);
    const long = ringOf(rings.long);
    return [{ service: row.service, title: row.title, ...(short ? { short } : {}), ...(long ? { long } : {}) }];
  });
}

/** What's at work here, as `list` has it (none known: nothing at work), with `rings`. */
export function atWorkOf(list: Kept | null, rings: AtWork["rings"], background = readBackground()): AtWork {
  const sessions = Object.entries(list?.sessions ?? {}).map(([id, { facts }]) => ({ id, facts }))
    // (Each this box's own copy, not archived; a conversation, never a subagent's or one kept out of sight.)
    .filter(({ facts }) => !facts.archived && !facts.away && facts.kind !== "subagent" && facts.kind !== "hidden");
  const working = sessions.filter(({ facts }) => facts.running && facts.asked === undefined)
    .sort((a, b) => b.facts.updated - a.facts.updated || (a.id < b.id ? -1 : 1));
  const asking = sessions.filter(({ facts }) => facts.asked !== undefined)
    .sort((a, b) => b.facts.asked! - a.facts.asked! || (a.id < b.id ? -1 : 1));
  const asks = new Map((list?.needsYou ?? []).map(each => [each.session, each.asks.summary]));
  const workingIds = new Set(working.map(({ id }) => id));
  // (Work a session left running, a command or a subagent, once its own turn is over.)
  const besides = Object.keys(background).filter(id => !workingIds.has(id)).length;
  return {
    applies: inMenuBar(),
    working: working.length,
    sessions: working.slice(0, MOST_NAMED).map(({ id, facts }) => {
      const title = titleOf(facts);
      const bot = botName(facts.bot);
      return { id, ...(title ? { title } : {}), ...(bot ? { bot } : {}) };
    }),
    asking: asking.slice(0, MOST_NAMED).map(({ id, facts }) => {
      const title = titleOf(facts);
      const said = asks.get(id);
      return { id, ...(title ? { title } : {}), ...(said ? { asks: said } : {}) };
    }),
    waiting: waitingOf(list),
    background: besides,
    busy: working.length > 0 || besides > 0,
    rings,
  };
}

const mtime = (file: string): number => {
  try { return fs.statSync(file).mtimeMs; } catch { return 0; }
};

/** `GET kuvo/box/at-work[/events]`: what's at work here; with `/events`, a line now and one as it
 * changes (its sessions starting, stopping and asking, its bots, the work left running), and the
 * rings again every 15 s. `POST kuvo/box/stop`: below. */
export async function* atWorkRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json, bridge } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  await listNow(context, signal ?? undefined).catch(() => null);
  const read = async () => atWorkOf(keptList(), await ringsOf(bridge(), signal ?? undefined));
  if (!match[1]) return yield* json(200, await read());
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // (Of the list's changes, only what this says: it changes with every step of a run.)
  const said = () => JSON.stringify(atWorkOf(keptList(), []));
  const listen = (told: () => void) => {
    let was = said();
    const changed = () => {
      const now = said();
      if (now === was) return;
      was = now;
      told();
    };
    listFollowers.add(changed);
    backgroundFollowers.add(changed);
    botFollowers.add(changed);
    return () => { listFollowers.delete(changed); backgroundFollowers.delete(changed); botFollowers.delete(changed); };
  };
  return yield* followedLive(read, listen, signal, { look: () => String(mtime(path.join(RUN, "work", "background.json"))) });
}

/** `POST kuvo/box/stop`: Kuvo on this Mac stopped until it next starts (Stop Kuvo on this Mac, in
 * Kuvo Helper's menu): its keeper's `pause`, in a process of its own (it outlasts this server, which
 * goes with the helper), answered at once. Only over this box's own server (its password: the helper
 * or the app on this Mac), never a phone or another computer: it can't be started again from there.
 * Only a Mac's own box in a home of its own, which its helper starts again at your next login, or
 * the app as it opens. Whether to ask you first (`busy`) is the client's to draw, from `at-work`. */
export async function* stopRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  if (request.method.toUpperCase() !== "POST") return yield* json(405, { message: "Only POST." });
  if (request.served !== true) return yield* json(403, { message: "Kuvo on this Mac is stopped only on this Mac." });
  if (!inMenuBar()) return yield* json(409, { message: "Only Kuvo on a Mac, run by Kuvo Helper, is stopped this way." });
  const keeper = path.join(K, "bin", "kuvo-start");
  if (!fs.existsSync(keeper)) return yield* json(500, { message: "Kuvo on this Mac couldn't be stopped: its keeper is missing." });
  try {
    spawn("/bin/sh", [keeper, "pause"], { cwd: K, detached: true, stdio: "ignore", env: { ...process.env, HOME: path.dirname(K) } }).unref();
  } catch (error) {
    return yield* json(500, { message: `Kuvo on this Mac couldn't be stopped: ${(error as Error).message}` });
  }
  return yield* json(202, { stopping: true });
}
