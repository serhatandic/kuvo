// Your computers, as this box knows them (PROTOCOL.md, "Computers", `kuvo/computers`): itself, its
// peers (the boxes its Macs name), and the boxes those vouch for (plan limits only). What each is,
// whether it's heard from, what it runs and answers, which of its sessions are at work, which
// services' plan limits it reads, and what can be done with it from here: renamed, removed
// (`peers/manage.ts`, handed to the box that writes its record) or reached again
// (`peers/reach-again.ts`). Every box answers it alike, a Mac's and a machine's, with no client
// there: each tells its peers its own facts with its agents (`peers/agents.ts`), so none is a hub.
// Platform: the system's name is read from macOS's own record of its version, or Linux's
// `os-release`; the rest is the same on both.

import fs from "node:fs";
import os from "node:os";
import { FEATURES } from "../../shared/features";
import { isObject, parsed, type Body } from "../../shared/json";
import type { ListedComputer } from "../../shared/routes/box";
import { macFeatures } from "../../shared/routes/mac";
import type { AtWorkSession, Target, ToldComputer } from "../../shared/records";
import { call, readEngineFile } from "../engine/client";
import { followedLive } from "../follow";
import { IS_MAC, LINK_FILE, MAC_APP_VERSION, MAC_ENGINE_VERSION } from "../home";
import { keptList, listFollowers, listNow } from "../lists/sessions";
import { MOST_AT_WORK, PEER_AGENTS_FILE, readPeerAgents, vouchedOf } from "../peers/directory";
import { isApart } from "../peers/apart";
import { namedBoxes, ownKey, peersOf } from "../peers/known";
import { actionsOf, givenName, MOST_NAME, removeHere, renameHere, whyNot, writerFacts, writerOf } from "../peers/manage";
import { offRelay } from "../letters/leave";
import { plainName } from "../../shared/text";
import { awaySince, PEER_REACH, readReach } from "../peers/reach";
import { askReachAgain, REACH_AGAIN, readTries, tryingSince } from "../peers/reach-again";
import { ownName } from "../letters/start";
import { forwardAct } from "../work-queue/forward";
import { sharedMachine } from "../setup/settle";
import { macLink, readLink } from "../state";
import { boxVersion } from "../updates/update";
import type { AnswerPiece, RouteContext } from "../server/context";

export const COMPUTERS_ROUTE = /^kuvo\/computers(?:(\/events)|\/([A-Za-z0-9_-]{1,60})(\/connect)?)?$/;

/** The most sessions at work this box names of itself (it counts them all). */
const MOST_HERE = 100;

/** How long since a peer's last word (its agents, every ten minutes while it's on the relay) it
 * still counts as heard from. */
const HEARD_LATELY = 30 * 60_000;

/** The system this box runs on, as people know it ("macOS 15.1", "Ubuntu 24.04.1 LTS"); none where it can't be read. */
export function systemName(platform: NodeJS.Platform = process.platform, read: (file: string) => string = file => fs.readFileSync(file, "utf8")): string | undefined {
  try {
    if (platform === "darwin") {
      const version = read("/System/Library/CoreServices/SystemVersion.plist").match(/<key>ProductVersion<\/key>\s*<string>([^<]{1,40})<\/string>/)?.[1];
      return version ? `macOS ${version}` : "macOS";
    }
    if (platform === "linux") {
      const name = read("/etc/os-release").match(/^PRETTY_NAME="?([^"\n]{1,80})"?$/m)?.[1];
      return name || "Linux";
    }
  } catch {
    return platform === "darwin" ? "macOS" : platform === "linux" ? "Linux" : undefined;
  }
  return undefined;
}

let system: { name: string | undefined } | null = null;

/** This box's sessions at work now, as its session list has them, newest first: conversations
 * (not a subagent's or a run kept out of sight), not archived, this box's own copy, and never one
 * that waits on you. */
export function atWorkHere(): AtWorkSession[] {
  return Object.entries(keptList()?.sessions ?? {})
    .filter(([, { facts }]) => facts.running && facts.asked === undefined && !facts.archived && !facts.away && facts.kind !== "subagent" && facts.kind !== "hidden")
    .sort(([one, a], [other, b]) => b.facts.updated - a.facts.updated || (one < other ? -1 : 1))
    .map(([id, { facts }]) => ({ id, ...(plainName(facts.title) ? { title: plainName(facts.title).slice(0, 200) } : {}) }));
}

/** How many of this box's sessions are at work now (`atWorkHere`). */
export const workingHere = (): number => atWorkHere().length;

/** What this box says of itself, here and to its peers (with its agents' mail): Kuvo Code's version
 * and its engine's, its system, and how many of its sessions are at work, and the first few of them. */
export function ownComputer(): ToldComputer {
  system ??= { name: systemName() };
  const atWork = atWorkHere();
  let engine: string | undefined;
  try { engine = readEngineFile()?.version || undefined; } catch {}
  const version = IS_MAC ? macLink.config?.app || MAC_APP_VERSION || undefined : boxVersion() || undefined;
  return {
    ...(version ? { version } : {}),
    ...(engine ?? (IS_MAC ? MAC_ENGINE_VERSION : null) ? { engine: (engine ?? MAC_ENGINE_VERSION)! } : {}),
    ...(system.name ? { system: system.name } : {}),
    working: atWork.length,
    atWork: atWork.slice(0, MOST_AT_WORK),
  };
}

/** What this box answers, as its `kuvo/info` says. */
const ownFeatures = (): string[] => (IS_MAC ? macFeatures() : [...FEATURES]);

/** The services whose plan limits each computer reads, by its key (this box's under `here`), as the
 * coding agents' bridge lists them (`rows`' `boxes`); none while it doesn't run. */
async function limitsRead(bridge: Target | null, signal?: AbortSignal): Promise<Map<string, string[]>> {
  const read = new Map<string, string[]>();
  if (!bridge) return read;
  const limits = await call(bridge, "GET", "/api/kuvo/limits", undefined, signal ? AbortSignal.any([signal, AbortSignal.timeout(5_000)]) : AbortSignal.timeout(5_000)).catch(() => null);
  for (const row of isObject(limits) && Array.isArray(limits.rows) ? limits.rows : []) {
    if (!isObject(row) || typeof row.service !== "string" || !Array.isArray(row.boxes)) continue;
    for (const box of row.boxes) {
      if (!isObject(box)) continue;
      const key = box.here === true ? "here" : typeof box.key === "string" ? box.key : "";
      const services = read.get(key) ?? [];
      if (!services.includes(row.service)) services.push(row.service);
      read.set(key, services);
    }
  }
  return read;
}

/** Your computers: this box first, then its peers by name, then the boxes they vouch for, each
 * with what can be done with it from here. */
export async function listComputers(name: string, bridge: Target | null, signal?: AbortSignal, now = Date.now()): Promise<ListedComputer[]> {
  let link = null as ReturnType<typeof readLink>;
  try { link = readLink(); } catch {}
  const limits = await limitsRead(bridge, signal);
  const own = link ? ownKey(link) : null;
  const told = readPeerAgents();
  const tries = readTries();
  const byName = <T extends { name: string; key: string }>(boxes: T[]) => [...boxes].sort((a, b) => a.name.localeCompare(b.name) || a.key.localeCompare(b.key));
  const { version, engine, system: runs } = ownComputer();
  const atWork = atWorkHere();
  // (A machine by the name its Macs' records give it, renamed there, else its own.)
  const named = !IS_MAC && own ? namedBoxes(link).find(box => box.key === own)?.name : undefined;
  const computers: Omit<ListedComputer, "actions">[] = [{
    key: own ?? "",
    name: (IS_MAC ? macLink.config?.name : named || name) || name || os.hostname(),
    kind: IS_MAC ? "mac" : "machine",
    here: true,
    shared: sharedMachine(link),
    reach: "connected",
    via: "here",
    ...(version ? { version } : {}),
    ...(engine ? { engine } : {}),
    ...(runs ? { system: runs } : {}),
    features: ownFeatures(),
    working: atWork.length,
    atWork: atWork.slice(0, MOST_HERE),
    limits: limits.get("here") ?? (own ? limits.get(own) : undefined) ?? [],
  }];
  const withActions = (): ListedComputer[] => {
    const here = computers[0]!.name;
    const facts = writerFacts(link);
    return computers.map(computer => ({ ...computer, actions: actionsOf(computer as ListedComputer, here, tryingSince(computer.key, now, tries) !== null, link, facts) }));
  };
  // A machine Macs of different setups share lists itself alone: the others there aren't all one
  // person's computers (as `kuvo/setup` refuses there).
  if (sharedMachine(link)) return withActions();
  const peers = peersOf(link);
  const reach = readReach();
  // (Those that aren't your own: a shared machine, a box of another setup.)
  const apart = new Set(peers.filter(peer => isApart(peer.key, link, told)).map(peer => peer.key));
  for (const peer of byName(peers)) {
    const said = told[peer.key];
    const away = awaySince(peer.key, reach) !== null || (said?.heard !== undefined && now - said.heard > HEARD_LATELY);
    // (Connect asked a moment ago, and not heard from since: being reached.)
    const trying = tryingSince(peer.key, now, tries);
    const reaching = trying !== null && !(said?.heard !== undefined && said.heard >= trying);
    const mine = said?.computer && !apart.has(peer.key) ? said.computer : undefined;
    // What it said it answers, whole (a list longer than peers keep can't say what's missing).
    const features = said?.features && !said.featuresCut && !apart.has(peer.key) ? said.features : undefined;
    computers.push({
      key: peer.key,
      name: peer.name,
      kind: peer.kind,
      ...(peer.id ? { id: peer.id } : {}),
      shared: said?.shared === true,
      ...(apart.has(peer.key) ? { apart: true as const } : {}),
      // (Not heard from since this box knew of it, and not found away: still being reached.)
      reach: reaching ? "connecting" : away ? "away" : said?.heard === undefined ? "connecting" : "connected",
      via: "relay",
      ...(mine ? { ...(mine.version ? { version: mine.version } : {}), ...(mine.engine ? { engine: mine.engine } : {}), ...(mine.system ? { system: mine.system } : {}) } : {}),
      ...(features ? { features } : {}),
      // What's at work there, as it last said: nothing seen at work while it's away; nothing said
      // by one that never said it (an older box, or one that isn't yours).
      ...(mine?.working !== undefined ? (away ? { working: 0, atWork: [] } : { working: mine.working, ...(mine.atWork ? { atWork: mine.atWork } : {}) }) : {}),
      limits: limits.get(peer.key) ?? [],
    });
  }
  // The boxes a peer vouches for (another Mac's, beside a machine two Macs share): known by that
  // peer's word alone, for plan limits, so nothing is known of what they run or whether they answer.
  // One only a peer that isn't yours vouches for (the other Mac of a machine two people share)
  // isn't one of your computers: left out.
  for (const box of byName(vouchedOf(link, told))) {
    const through = peers.find(peer => !apart.has(peer.key) && told[peer.key]?.boxes?.some(each => each.key === box.key));
    if (!through) continue;
    computers.push({ key: box.key, name: box.name, kind: box.kind, shared: false, reach: "unknown", via: "peer", through: through.name, limits: limits.get(box.key) ?? [] });
  }
  return withActions();
}

const mtime = (file: string): number => { try { return fs.statSync(file).mtimeMs; } catch { return 0; } };

/** What a client does with one of your computers (`PATCH`/`DELETE kuvo/computers/<key>`, `POST
 * …/connect`): done here, or handed to the box that writes its record, and answered with what that
 * one said, or why it can't be, in words a client shows as they are. */
async function* manage(context: RouteContext, key: string, connect: boolean): AsyncGenerator<AnswerPiece> {
  const { request, signal, json, info } = context;
  const method = request.method.toUpperCase();
  const what = connect ? (method === "POST" ? "connect" : null) : method === "PATCH" ? "rename" : method === "DELETE" ? "remove" : null;
  if (!what) return yield* json(405, { message: connect ? "Only POST." : "Rename (PATCH) or remove (DELETE)." });
  const computers = await listComputers(info.name ?? "", null, signal ?? undefined);
  const computer = computers.find(each => each.key === key);
  if (!computer) return yield* json(404, { message: "That isn't one of your computers." });
  const here = computers[0]!.name;
  if (what === "connect") {
    if (computer.here) return yield* json(409, { message: `${here} is the computer you're asking.` });
    if (computer.via !== "relay") return yield* json(409, { message: `${here} doesn't reach ${computer.name} itself, so it can't connect to it.` });
    if (computer.reach === "connecting") return yield* json(409, { message: `${here} is reaching ${computer.name} now.` });
    if (computer.reach !== "away") return yield* json(409, { message: `${computer.name} is connected.` });
    if (offRelay()) return yield* json(503, { message: `${here} isn't connected to your other computers now, so it can't reach ${computer.name}. Try again in a moment.` });
    if (!askReachAgain(key)) return yield* json(503, { message: `${here} couldn't try now. Try again in a moment.` });
    return yield* json(202, {});
  }
  const name = what === "rename" ? givenName((parsed(request.body) as Body<"name">)?.name) : null;
  if (what === "rename" && !name) return yield* json(400, { message: `Give it a name of up to ${MOST_NAME} characters.` });
  const link = readLink();
  const why = whyNot(computer, what, here, link);
  if (why) return yield* json(409, { message: why });
  const writer = writerOf(link)!;
  if (writer.here) {
    const done = what === "rename" ? renameHere(key, name!) : removeHere(key);
    return yield* json(done.status, done.value);
  }
  // (Handed to the box that writes it: it does it as its own client's, and says how it went.)
  const there = await forwardAct({ key: writer.key, name: writer.name, kind: "mac" }, `${computer.name}`, { area: "computers", item: key, ...(what === "rename" ? { method: "PATCH", body: { name } } : { method: "DELETE" }) }, signal);
  if ("refused" in there) return yield* json(there.status, { message: there.refused });
  return yield* json(there.status, isObject(there.value) ? there.value : {});
}

export async function* computersRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json, info, bridge } = context;
  if (match[2]) return yield* manage(context, match[2], Boolean(match[3]));
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  // (Its own sessions at work, from its list: read first where this process hasn't yet.)
  await listNow(context, signal ?? undefined).catch(() => null);
  const read = async () => ({ computers: await listComputers(info.name ?? "", bridge(), signal ?? undefined) });
  if (!match[1]) return yield* json(200, await read());
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // A line now, and one as it changes: its peers, what they told, which are away (their files),
  // and its own sessions starting and stopping work; and every 15 s for the rest (its limits).
  // (Of its list's changes, only which are at work: it changes with every step of a run.)
  const atWork = () => JSON.stringify(atWorkHere().slice(0, MOST_HERE));
  const listen = (told: () => void) => {
    let was = atWork();
    const listTold = () => {
      const now = atWork();
      if (now === was) return;
      was = now;
      told();
    };
    listFollowers.add(listTold);
    return () => { listFollowers.delete(listTold); };
  };
  return yield* followedLive(read, listen, signal,
    { look: () => `${mtime(PEER_AGENTS_FILE)} ${mtime(PEER_REACH)} ${mtime(LINK_FILE)} ${mtime(REACH_AGAIN)}` });
}
