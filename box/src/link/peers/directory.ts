// Agents on other boxes (PROTOCOL.md, "Peers")
// Each box tells its peers who its agents are (its bots, its latest titled sessions), and keeps
// what each told it, for list_agents here (`run/agents.json`, which kuvo-browser.js reads): those
// files, and what's kept of each agent.

import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { locked, writePrivate } from "../files";
import { RUN } from "../home";
import { KEY } from "./known";
import { SETUP_ID } from "../setup/record";
import type { AgentsFile, AskingSession, AtWorkSession, ListedAgent, ToldAgents, ToldComputer } from "../../shared/records";
import type { EngineSession } from "../engine/sessions";
import { type BotLook, LOOKS as LOOK_WORDS } from "../../shared/routes/bots";
import type { Peer } from "./known";
import { AGENTS_DOWN_WHY } from "../areas/agents-down";
import { FLEET_UPDATE, type FleetUpdate } from "../../shared/routes/box";
import { fitsShape } from "../../shared/shape";

export const PEER_AGENTS_FILE = path.join(RUN, "peer-agents.json");
// COMPAT(beta.1 mac-agents): what a Mac connected over SSH said its agents were, for when the relay couldn't be reached; no Mac writes it now, and it's removed. Goes 3 months after 0.8.0.
export const MAC_AGENTS_FILE = path.join(RUN, "mac-agents.json");
export const AGENTS_FILE = path.join(RUN, "agents.json");

export const PEER_LOCK = path.join(RUN, ".peers.lock");

import { cannotFact, releaseRef } from "../updates/facts";

const MOST_AGENTS = 120;

/** An agent as another box gave it, once `fitsAgent` has checked its session, name and kind: the
 * rest is cleaned as it's kept (`keptAgent`). */
export interface GivenAgent {
  session: string;
  name: string;
  kind: "bot" | "session";
  id?: unknown;
  look?: unknown;
  project?: unknown;
  about?: unknown;
  machine?: unknown;
  marketplaceAgent?: unknown;
}

/** A conversation as the engine (or the agents bridge) lists it, as far as whether it's put away. */
export type PutAwayable = Pick<Partial<EngineSession>, "metadata" | "time">;

export const fitsAgent = (agent: unknown): agent is GivenAgent => isObject(agent) && typeof agent.session === "string" && /^ses_[A-Za-z0-9]{1,60}$/.test(agent.session)
  && typeof agent.name === "string" && (agent.kind === "bot" || agent.kind === "session");
// A bot's `id` too (`bot_…`, as group chats name their members), and how it looks (`look`: color,
// shape, face), so a chat on another computer can have it and show it.
// Only colors, shapes and faces the apps draw (BotLook, its words in shared/routes/bots): another (a
// newer computer's) is the plain one's, so a phone can always read it.
const LOOKS: { [Part in keyof BotLook]: readonly string[] } = { color: LOOK_WORDS.colors, shape: LOOK_WORDS.shapes, face: LOOK_WORDS.faces };
const PLAIN_LOOK: BotLook = { color: "graphite", shape: "round", face: "calm" };
const keptLook = (look: unknown): BotLook | undefined => (isObject(look)
  ? Object.fromEntries((Object.keys(LOOKS) as (keyof BotLook)[]).map(key => [key, LOOKS[key].includes(look[key] as string) ? look[key] as string : PLAIN_LOOK[key]])) as unknown as BotLook : undefined);
export const keptAgent = ({ session, name, kind, project, about, machine, id, look, marketplaceAgent }: GivenAgent): ListedAgent => ({ session, name: plainName(name).slice(0, 200), kind,
  ...(kind === "bot" && typeof id === "string" && /^bot_[A-Za-z0-9]{1,60}$/.test(id) ? { id } : {}),
  ...(kind === "bot" && typeof marketplaceAgent === "string" && /^[a-z0-9][a-z0-9-]{0,62}$/.test(marketplaceAgent) ? { marketplaceAgent } : {}),
  ...(kind === "bot" && keptLook(look) ? { look: keptLook(look) } : {}),
  ...(typeof project === "string" && project ? { project: project.slice(0, 200) } : {}), ...(typeof about === "string" && about ? { about: about.slice(0, 500) } : {}),
  ...(typeof machine === "string" && machine ? { machine: machine.slice(0, 200) } : {}) });
/** The most feature names kept of what a peer says it answers. */
export const MOST_FEATURES = 300;
// What a peer says it answers (`features`, as kuvo/info's), kept with its agents.
export const keptFeatures = (features: unknown): string[] => (Array.isArray(features) ? features.filter((each): each is string => typeof each === "string" && each.length <= 60).slice(0, MOST_FEATURES) : []);
/** Whether what a peer says it answers is more than this box keeps of it (`keptFeatures`). */
const featuresCut = (features: unknown[]): boolean => features.filter(each => typeof each === "string" && each.length <= 60).length > MOST_FEATURES;

// The conversations a peer says it moved away, and where each lives now (PROTOCOL.md, "Peers").
const keptMoved = (moved: unknown[]): NonNullable<ToldAgents["moved"]> => moved.filter((each): each is { session: string; key: string; at: number } => isObject(each)
  && typeof each.session === "string" && /^ses_[A-Za-z0-9]{1,60}$/.test(each.session) && typeof each.key === "string" && KEY.test(each.key) && typeof each.at === "number").slice(0, 400)
  .map(({ session, key, at }) => ({ session, key, at }));

/** How long each ID's digest is in what a computer says it has (`owns`), and the most it says,
 * the newest first: enough to fit one mail beside its agents. */
export const OWNED_DIGEST = 6;
export const MOST_OWNED = 5_000;
const keptOwns = (given: unknown): given is string => typeof given === "string" && given.length % OWNED_DIGEST === 0
  && given.length <= OWNED_DIGEST * MOST_OWNED && /^[A-Za-z0-9_-]*$/.test(given);

// What a peer said its agents are, the newest it said: false for an older list.
export function takeAgents(sender: { key: string; kind: "mac" | "machine"; id?: string }, given: unknown, file = PEER_AGENTS_FILE, lock = PEER_LOCK): boolean {
  if (!isObject(given) || typeof given.at !== "number" || !Array.isArray(given.list)) return false;
  return locked(lock, () => {
    let all: Record<string, ToldAgents> = {};
    try { all = JSON.parse(fs.readFileSync(file, "utf8")); } catch {}
    if ((given.at as number) <= (all[sender.key]?.at ?? 0)) {
      // (Older by its clock, which may have been set right: heard all the same.)
      if (all[sender.key]) {
        all[sender.key].heard = Date.now();
        writePrivate(file, JSON.stringify(all));
      }
      return false;
    }
    const computer = keptComputer(given.computer);
    all[sender.key] = { at: given.at as number, heard: Date.now(), kind: sender.kind, ...(sender.id ? { id: sender.id } : {}), list: (given.list as unknown[]).filter(fitsAgent).slice(0, MOST_AGENTS).map(keptAgent),
      ...(Array.isArray(given.features) ? { features: keptFeatures(given.features), ...(featuresCut(given.features) ? { featuresCut: true as const } : {}) } : {}), ...(Array.isArray(given.moved) ? { moved: keptMoved(given.moved) } : {}),
      ...(Array.isArray(given.groups) ? { groups: given.groups.filter((id): id is string => typeof id === "string" && /^grp_[a-z0-9]{1,60}$/.test(id)).slice(0, 100) } : {}),
      ...(keptOwns(given.owns) ? { owns: given.owns } : {}),
      ...(typeof given.setup === "string" && SETUP_ID.test(given.setup) ? { setup: given.setup } : {}),
      ...(given.trash === "trash" || given.trash === "kuvo" ? { trash: given.trash } : {}),
      ...(computer ? { computer } : {}) };
    writePrivate(file, JSON.stringify(all));
    return true;
  });
}

/** How often mail that carries no agents moves when a peer was last heard from (`heardFrom`). */
const HEARD_EVERY = 5 * 60_000;

/** Mail from the peer `key`, whatever it carries: heard from now, as its agents would say (they come
 * as it connects and every ten minutes; this mail shows it answers before then). Kept at most every
 * five minutes. When it was last heard from before, where that moved; else null. */
export function heardFrom(key: string, now = Date.now(), file = PEER_AGENTS_FILE, lock = PEER_LOCK): number | null {
  const before = readPeerAgents(file)[key]?.heard;
  if (before === undefined || now - before < HEARD_EVERY) return null;
  return locked(lock, () => {
    const all = readPeerAgents(file);
    const told = all[key];
    if (told?.heard === undefined || now - told.heard < HEARD_EVERY) return null;
    const was = told.heard;
    told.heard = now;
    writePrivate(file, JSON.stringify(all));
    return was;
  });
}

const short = (value: unknown, most: number): string | undefined => (typeof value === "string" && value && value.length <= most ? value : undefined);

/** The most sessions at work a box names, to its peers and in `kuvo/computers` (it counts them all). */
export const MOST_AT_WORK = 20;

/** Sessions at work as a box said them, checked: each an id with a title of a size it would have. */
export const keptAtWork = (given: unknown[]): AtWorkSession[] => given.filter((each): each is { id: string; title?: unknown } => isObject(each) && typeof each.id === "string" && /^ses_[A-Za-z0-9]{1,60}$/.test(each.id))
  .slice(0, MOST_AT_WORK).map(({ id, title }) => ({ id, ...(typeof title === "string" && plainName(title) ? { title: plainName(title).slice(0, 200) } : {}) }));

/** The most sessions waiting on you a box names to its peers (it counts them all). */
export const MOST_ASKING = 10;

/** Sessions waiting on you as a box said them, checked: an id, a title and what it asks, each of a
 * size it would have. */
export const keptAsking = (given: unknown[]): AskingSession[] => given.filter((each): each is { id: string; title?: unknown; asks?: unknown } => isObject(each) && typeof each.id === "string" && /^ses_[A-Za-z0-9]{1,60}$/.test(each.id))
  .slice(0, MOST_ASKING).map(({ id, title, asks }) => ({ id,
    ...(typeof title === "string" && plainName(title) ? { title: plainName(title).slice(0, 200) } : {}),
    ...(typeof asks === "string" && plainName(asks) ? { asks: plainName(asks).slice(0, 200) } : {}) }));

/** What a peer said of itself, checked: only what's of a size it would have. What it didn't say
 * (an older box says no `working`) is left out, never taken as none. */
// A version as a peer says it (`computer.kuvo`, `computer.release`), or undefined.
const toldVersion = (given: unknown): string | undefined => (typeof given === "string" && /^\d+\.\d+\.\d+(-beta\.\d+)?$/.test(given) ? given : undefined);

export function keptComputer(given: unknown): ToldComputer | undefined {
  if (!isObject(given)) return undefined;
  const version = short(given.version, 40), engine = short(given.engine, 40), system = short(given.system, 80);
  const count = (value: unknown) => typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= 10_000 ? value : undefined;
  const working = count(given.working), waiting = count(given.waiting);
  const layout = count(given.layout);
  const installedSha256 = typeof given.installedSha256 === "string" && /^[a-f0-9]{64}$/.test(given.installedSha256) ? given.installedSha256 : undefined;
  const fleetUpdate = fitsShape(FLEET_UPDATE, given.fleetUpdate) ? given.fleetUpdate as FleetUpdate : undefined;
  const update = { staged: releaseRef(given.staged), cannot: cannotFact(given.cannot), candidate: toldVersion(given.candidate),
    releaseManifest: short(given.releaseManifest, 16_384), runningManifest: short(given.runningManifest, 16_384) };
  return { ...Object.fromEntries(Object.entries(update).filter(([, value]) => value !== undefined)) as Partial<typeof update>,
    ...(installedSha256 ? { installedSha256 } : {}), ...(layout !== undefined ? { layout } : {}), ...(fleetUpdate ? { fleetUpdate } : {}), ...(version ? { version } : {}), ...(engine ? { engine } : {}), ...(system ? { system } : {}), ...(working !== undefined ? { working } : {}),
    ...(working !== undefined && Array.isArray(given.atWork) ? { atWork: keptAtWork(given.atWork) } : {}),
    ...(waiting !== undefined ? { waiting, asking: Array.isArray(given.asking) ? keptAsking(given.asking) : [] } : {}), ...(given.oldApp === true ? { oldApp: true as const } : {}),
    ...(Array.isArray(given.addresses) ? { addresses: given.addresses.filter((each): each is string => typeof each === "string" && /^[A-Za-z0-9][A-Za-z0-9.-]{0,99}$/.test(each)).slice(0, 8) } : {}),
    ...(toldVersion(given.kuvo) ? { kuvo: toldVersion(given.kuvo) } : {}), ...(toldVersion(given.release) ? { release: toldVersion(given.release) } : {}),
    ...(keptAgents(given.agents) ? { agents: keptAgents(given.agents) } : {}) };
}

/** How a peer said its coding agents stand (`computer.agents`), checked: one of the three states,
 * and a reason of the box's own words for it. */
function keptAgents(given: unknown): ToldComputer["agents"] {
  if (!isObject(given) || !["running", "restarting", "down"].includes(given.state as string)) return undefined;
  const state = given.state as "running" | "restarting" | "down";
  return { state, ...(state === "down" && (AGENTS_DOWN_WHY as readonly string[]).includes(given.why as string) ? { why: given.why as string } : {}) };
}

// What each peer told, by its key, as it last said it by mail.
export function readPeerAgents(file = PEER_AGENTS_FILE): Record<string, ToldAgents> {
  let all: Record<string, ToldAgents> = {};
  try { all = JSON.parse(fs.readFileSync(file, "utf8")) ?? {}; } catch {}
  return all;
}

// The box a session is on, among the peers: where the box that moved it says it went (or a Mac
// says, `movedOf`), else the one that lists it. One listed by more than one is a Mac's when a Mac lists it, else nobody's (one of them
// could be claiming another's).
export function peerOf(session: string, peers: Peer[], told: Record<string, ToldAgents>, moved = new Map<string, string>()): Peer | null {
  if (moved.has(session)) return peers.find(peer => peer.key === moved.get(session)) ?? null;
  const listing = peers.filter(peer => (told[peer.key]?.list ?? []).some(agent => agent.session === session));
  if (listing.length <= 1) return listing[0] ?? null;
  const macs = listing.filter(peer => peer.kind === "mac");
  return macs.length === 1 ? macs[0] : null;
}

// Put away: archived, as a Mac archives one (`kuvo.archived` in its metadata) or the engine does.
export const isPutAway = (session: PutAwayable | null | undefined) => session?.metadata?.["kuvo.archived"] === true || Boolean(session?.metadata?.kuvo?.archived) || Boolean(session?.time?.archived);

// Everyone, for list_agents here: this box's own (`machine` its name on its Macs), then each peer's
// it alone lists (`peerOf`): a machine's by its name; a Mac's its own (none), and the agents of
// machines it speaks for (those whose link doesn't talk to other computers yet), by theirs.
export function composeAgents(own: ListedAgent[], ownID: string | null, peers: Peer[], told: Record<string, ToldAgents>, moved = new Map<string, string>()): AgentsFile {
  const agents: ListedAgent[] = own.map(agent => ({ ...keptAgent({ ...agent, machine: undefined }), ...(ownID ? { machine: ownID } : {}) }));
  const seen = new Set(agents.map(agent => agent.session));
  for (const peer of peers) {
    for (const agent of told[peer.key]?.list ?? []) {
      if (seen.has(agent.session) || peerOf(agent.session, peers, told, moved)?.key !== peer.key) continue;
      seen.add(agent.session);
      const { machine, ...rest } = agent;
      agents.push(peer.kind === "machine" ? { ...rest, ...(peer.id ? { machine: peer.id } : {}) } : { ...rest, ...(machine ? { machine } : {}) });
    }
  }
  return { machine: ownID ?? null, agents };
}
