import { readServices } from "../shared/box-services";
// What's at work in Kuvo on this computer, for what's always on screen there (PROTOCOL.md, "In the
// menu bar", `kuvo/box/at-work`), and stopping it until it next starts (`kuvo/box/stop`). On a Mac
// whose box lives in a home of its own, Kuvo Helper runs it and draws these facts in the menu bar,
// so that you can see Kuvo still works with the app closed, open the app, or stop it. Every fact is
// the box's: its session list (`lists/sessions.ts`), what waits on you (`areas/waiting.ts`), the
// work its sessions left running (`work/background.ts`), and its plan limits' rings (the coding
// agents' bridge). And the same of each of your other computers (`computers`, #781), as they last
// told this box with their agents (`peers/agents.ts`: what's at work there and what waits on you)
// and as its plan limits say whose accounts are signed in where. The helper only draws them.
//
// Platform: the facts are the same on macOS and Linux (a machine's box answers `applies: false`:
// nothing of its own is on its screen). Stopping is a Mac's own box's alone: elsewhere nothing
// starts it again at the next login but the machine's own start, so it's refused there.

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../shared/json";
import type { Target } from "../shared/records";
import type { LinkState } from "./state";
import type { AtWork } from "../shared/routes/box";
import type { SessionFacts } from "../shared/routes/sessions";
import { askingOf, peerReach } from "./areas/computers";
import { waitingOf } from "./areas/waiting";
import { ownComputerName } from "./computer-name";
import { PEER_AGENTS_FILE, readPeerAgents } from "./peers/directory";
import { ownKey, peersOf } from "./peers/known";
import { PEER_REACH } from "./peers/reach";
import { readLink } from "./state";
import { botFollowers, botHere } from "./bots/here";
import { call } from "./engine/client";
import { followedLive } from "./follow";
import { APP_FOLDER, IS_MAC, K, recordedKind, RUN } from "./home";
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

type Rings = AtWork["rings"][number];

/** An account's rings, with the computers it's signed in on (by key; `here` for this box), as its
 * plan limits' row says them. */
export interface RingsOn {
  rings: Rings;
  on: string[];
}

/** Every account's rings, in the order the bridge lists them (its limits' `rows`), each with the
 * computers it's on; none while it doesn't run or doesn't answer within five seconds. */
export async function ringsOn(bridge: Target | null, signal?: AbortSignal): Promise<RingsOn[]> {
  bridge = readServices(RUN) ?? bridge;
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
    const on = (Array.isArray(row.boxes) ? row.boxes : []).flatMap(box => !isObject(box) ? [] : box.here === true ? ["here"] : typeof box.key === "string" ? [box.key] : []);
    return [{ rings: { service: row.service, title: row.title, ...(short ? { short } : {}), ...(long ? { long } : {}) }, on: [...new Set(on)] }];
  });
}

/** Every account's rings, in the order the bridge lists them; none while it doesn't run. */
export async function ringsOf(bridge: Target | null, signal?: AbortSignal): Promise<AtWork["rings"]> {
  return (await ringsOn(bridge, signal)).map(each => each.rings);
}

type Computer = NonNullable<AtWork["computers"]>[number];

/** Your computers as the menu bar shows them (`computers`): this one first, with what `here` says of
 * it, then each of your other computers by name, as it last told this box: whether it's heard from,
 * how many of its sessions are at work and wait on you, and the first few of each (nothing while
 * it's away: what it said then is old; left out where it never said, an older box), and the rings
 * of the accounts signed in there. Never one known by a peer's word alone. */
export function computersOf(here: Omit<AtWork, "computers">, rings: RingsOn[], waitingHere: number, link: LinkState | null = readLinkSafely(), now = Date.now()): Computer[] {
  const own = link ? ownKey(link) : null;
  const ringsFor = (key: string) => rings.filter(each => each.on.includes(key) || (key === "here" && own !== null && each.on.includes(own))).map(each => each.rings);
  const computers: Computer[] = [{
    key: own ?? "", name: ownComputerName(), kind: IS_MAC ? "mac" : "machine", here: true, reach: "connected",
    working: here.working, sessions: here.sessions, waiting: waitingHere, asking: here.asking, rings: ringsFor("here"),
  }];
  if (!link) return computers;
  const told = readPeerAgents();
  const peers = [...peersOf(link)].sort((a, b) => a.name.localeCompare(b.name) || a.key.localeCompare(b.key));
  for (const peer of peers) {
    const said = told[peer.key]?.computer;
    const { away, reach } = peerReach(peer.key, told, undefined, undefined, now);
    const known = !away && said?.working !== undefined;
    computers.push({
      key: peer.key, name: peer.name, kind: peer.kind, reach,
      ...(known ? { working: said!.working! } : {}),
      sessions: known ? (said!.atWork ?? []) : [],
      ...(!away && said?.waiting !== undefined ? { waiting: said.waiting } : {}),
      asking: !away ? (said?.asking ?? []) : [],
      rings: ringsFor(peer.key),
    });
  }
  return computers;
}

const readLinkSafely = (): LinkState | null => { try { return readLink(); } catch { return null; } };

/** What's at work here, as `list` has it (none known: nothing at work), with `rings`. */
export function atWorkOf(list: Kept | null, rings: AtWork["rings"], background = readBackground()): AtWork {
  const sessions = Object.entries(list?.sessions ?? {}).map(([id, { facts }]) => ({ id, facts }))
    // (Each this box's own copy, not archived; a conversation, never a subagent's or one kept out of sight.)
    .filter(({ facts }) => !facts.archived && !facts.away && facts.kind !== "subagent" && facts.kind !== "hidden");
  const working = sessions.filter(({ facts }) => facts.running && facts.asked === undefined)
    .sort((a, b) => b.facts.updated - a.facts.updated || (a.id < b.id ? -1 : 1));
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
    // (Those waiting on you, by the rule your other computers hear them by: `askingOf`.)
    asking: askingOf(list).slice(0, MOST_NAMED),
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
  const read = async () => {
    const rings = await ringsOn(bridge(), signal ?? undefined);
    const list = keptList();
    const here = atWorkOf(list, rings.map(each => each.rings));
    return { ...here, computers: computersOf(here, rings, askingOf(list).length) };
  };
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
  // (And what your other computers tell this box, and whether they're heard from: other processes'.)
  const look = () => [path.join(RUN, "work", "background.json"), PEER_AGENTS_FILE, PEER_REACH].map(mtime).join(" ");
  return yield* followedLive(read, listen, signal, { look });
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
