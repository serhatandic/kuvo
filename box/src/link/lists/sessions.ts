// Every client's session lists, from the box (PROTOCOL.md, "Session lists"): `GET kuvo/sessions/list`
// and `…/events`, each session's facts (what it is, its project, its flags, whether it runs and
// asks you) and the projects the lists show, then what changes. The list is kept in
// `run/lists/sessions.json`, one for the box whichever process works it out, under its lock: each
// entry with the change it came with (`seq`), and those gone kept a while, so a client that lost its
// stream resumes from where it was (`since`), across the box's restarts too, rather than read
// everything again. The box's long-running process works it out as its engine's and its coding
// agents' events come (`noteListEvent`), from each place read whole as it's heard
// (`listPlaceHeard`) and every five minutes; a route answered in a process of its own (`box route`,
// a machine reached over SSH) reads the places itself.

import fs from "node:fs";
import { keepsList, setKeepsList } from "../server/resident-state";
import path from "node:path";
import { isObject } from "../../shared/json";
import { listOf, locked, writePrivate } from "../files";
import { BOTS, BOX, RUN } from "../home";
import { watchFolders } from "../follow";
import { watchFolder, type SharedWatch } from "../../shared/watch";
import { bridgeTarget, call, engineGet } from "../engine/client";
import { processRuns } from "../../shared/platform";
import { isChatsFolder } from "../engine/chats";
import { filedProjects, projectFollowers, readProjects } from "../areas/projects";
import { botFollowers, botHere } from "../bots/here";
import { awaySessions, moveFollowers, MOVES } from "../moves/records";
import { askFollowers, readAsks, type AskWords } from "../glance/asked";
import { factsOf, folderOf, kindOf, listedProjects, needsYou, projectOrder, withSubagents, type ListPlace, type Stopped } from "./rule";
import { isJudged, judgingFollowers } from "../work/tree";
import { listedWork, workBot, type ListedWork } from "../work/listed-work";
import { inTime, repositoryOf } from "../work/project-trees";
import { wakeTurnIsOpen } from "../work/wakes";
import { Moments } from "./moments";
import type { EngineSession } from "../engine/sessions";
import { engineListFacts, withEngineActivity, type EngineListFacts } from "../engine/list-facts";
import type { ListedProject, NeedsYou, SessionFacts, SessionList } from "../../shared/routes/sessions";
import type { Target } from "../../shared/records";
import type { AnswerPiece, RouteContext } from "../server/context";
import { backgroundFollowers, readBackground } from "../work/background";

export const LIST_ROUTE = /^kuvo\/sessions\/list(\/events)?$/;
export const LIST_FILE = path.join(RUN, "lists", "sessions.json");

/** How many sessions gone are kept for `since`, and for how long. */
export const GONE_KEPT = 2000;
const GONE_FOR = 24 * 3600_000;
/** How often the list is read whole while the box runs. */
const READ_EVERY = 5 * 60_000;
/** A session's events that change what's listed of it: it's read again. */
const RELOADS = new Set(["session.created", "session.updated", "session.shell.started", "session.shell.ended", "session.renamed", "session.moved", "session.metadata.updated", "session.revert.staged", "session.revert.cleared", "session.revert.committed"]);
const STARTS = "session.execution.started";
const DELIVERED = "session.inbox.delivered";
const ENDS = new Set(["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted"]);

/** One place's sessions (the engine's, or the coding agents' bridge's), and those at work there. */
interface Place {
  target: Target;
  sessions: Map<string, EngineSession>;
  engineActivity?: boolean;
  running: Set<string>;
  wakeReads: Map<string, object>;
  wakeRetries: Map<string, ReturnType<typeof setTimeout>>;
  waking: Set<string>;
}

/** The list as it's kept: where it's at, each session's facts with the change it came with and the
 * place it's on, those gone since (with theirs), the oldest change `since` can resume from, its
 * projects, the places it couldn't read (their sessions as they were last read), and the box's
 * long-running process that keeps it (`owner`), while it runs. */
export interface Kept {
  epoch: string;
  seq: number;
  floor: number;
  sessions: Record<string, { seq: number; facts: SessionFacts; place?: string; work?: ListedWork; workSeq?: number }>;
  gone: Record<string, { seq: number; at: number }>;
  workByBot?: Record<string, string[]>;
  projects: Record<string, ListedProject>;
  /** The sessions waiting on you, as "Needs you" lists them (none in a file from before it). */
  needsYou?: NeedsYou[];
  pending?: string[];
  owner?: number;
  /** Which start of `owner` it is (the same process id may come again after a restart). */
  started?: string;
}

/** How far the list's count moves on as it's taken over from a long-running process that went:
 * past any change that one told but didn't write (its writes are a moment apart). */
const LOST_GAP = 1_000_000;
/** This process's own start, told apart from an earlier one with the same id. */
const STARTED = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;

/** What a list is worked out to: each session's facts, the place each is on, the projects, and the
 * places that couldn't be read. */
interface Listed {
  sessions: Record<string, SessionFacts>;
  projects: Record<string, ListedProject>;
  needsYou?: NeedsYou[];
  placeOf?: Record<string, string>;
  work?: Record<string, ListedWork>;
  pending?: string[];
}

/** The list as this process last worked it out. */
let latest: Kept | null = null;
const places = new Map<string, Place>();
/** Told as the list changes (each `…/events` stream). */
export const listFollowers = new Set<() => void>();
/** What each session's events say from moment to moment (thinking, blocked), taken in as they come. */
const moments = new Moments(() => scheduleWorkOut());

/** A cursor: the epoch it's of and the change it's at. */
const cursorOf = (list: Pick<Kept, "epoch" | "seq">) => `${list.epoch}.${list.seq}`;

function parseCursor(given: string | null): { epoch: string; seq: number } | null {
  const match = given?.match(/^([A-Za-z0-9-]+)\.(\d+)$/);
  return match ? { epoch: match[1]!, seq: Number(match[2]) } : null;
}

/** What the box knows beside its sessions, read from its files: kept by the long-running process
 * until one of them changes (`keepList`), so a run's every step reads none of them. (A project's
 * git worktree made outside Kuvo Code, `linkedTrees`, is seen as it's read again: at the next
 * whole read, a route's ask or a stream's minute.) */
export interface BoxKnows {
  projects: ReturnType<typeof readProjects>;
  filed: Record<string, string>;
  botSessions: Set<string>;
  /** The bot each of its bots' conversations is. */
  botOf: Map<string, string>;
  away: Set<string>;
  asks: ReturnType<typeof readAsks>;
}

// A session can be started straight in a Git worktree, without any Kuvo project or worktree record.
let engineWorktrees: Record<string, string> = {};
const gitProjects = new Map<string, { project?: string; readAt: number }>();

async function discoverProjects(sessions: readonly EngineSession[]): Promise<void> {
  const { projects, botSessions } = readBoxKnows();
  const filed = filedProjects(projects);
  const place = { isChatsFolder, botsFolder: BOTS, botSessions };
  const now = Date.now();
  const directories = sessions.filter(session => kindOf(session, place) === "project").map(folderOf);
  const folders = [...new Set([...directories, ...projects.filter(project => !project.removed).map(project => project.directory)])]
    .filter(folder => path.isAbsolute(folder) && !filed[folder] && (gitProjects.get(folder)?.readAt ?? -Infinity) < now - READ_EVERY);
  for (let from = 0; from < folders.length; from += 24) {
    await Promise.all(folders.slice(from, from + 24).map(async folder => {
      const repository = await inTime(repositoryOf(folder), null, 1500);
      const previous = gitProjects.get(folder)?.project;
      const project = repository ? (repository.top === repository.main ? undefined : repository.main) : previous;
      gitProjects.set(folder, { project, readAt: now });
      if (project !== previous) known = null;
    }));
  }
}

export function readBoxKnows(): BoxKnows {
  const projects = readProjects();
  const botSessions = new Set<string>();
  const botOf = new Map<string, string>();
  for (const id of listOf(BOTS)) {
    const bot = botHere(id);
    if (bot) { botSessions.add(bot.session); botOf.set(bot.session, id); }
  }
  const filed = filedProjects(projects);
  for (const [folder, { project }] of gitProjects) {
    if (project && !filed[folder]) filed[folder] = filed[project] ?? project;
  }
  for (const [folder, project] of Object.entries(engineWorktrees)) filed[folder] ??= filed[project] ?? project;
  return { projects, filed, botSessions, botOf, away: new Set(awaySessions()), asks: readAsks() };
}

/** What the box knows beside its sessions, as it is now (`known`: as it was read last). At work
 * (`running`): a session whose own turn runs, and one whose turn is over while something it left
 * goes on, a command or a subagent (Background work, `work/background.ts`), so a session's row, its
 * project's count and its bot's `now.working` say what every client's At work says. */
export function placeNow(turns: ReadonlySet<string>, sessions: Iterable<EngineSession>, known: BoxKnows = readBoxKnows(), background?: Iterable<string>, stopped: ReadonlyMap<string, Stopped> = moments.stopped): ListPlace {
  const counted = background === undefined ? readBackground() : null;
  const running = new Set([...turns, ...(background ?? Object.keys(counted ?? {}))]);
  const childOnly = new Set(Object.entries(counted ?? {}).filter(([id, count]) => !turns.has(id) && !count.commands && !count.tasks && count.subagents > 0).map(([id]) => id));
  const parents = new Map<string, string>();
  for (const session of sessions) if (session.parentID) parents.set(session.id, session.parentID);
  // (A subagent's ask is the conversation's it works for.)
  // (What it asks: its oldest ask's words, the one `asked` is since.)
  const asked = new Map<string, number>();
  const directAsks = new Set<string>();
  const asks = new Map<string, AskWords>();
  for (const ask of known.asks) {
    directAsks.add(ask.session);
    let root = ask.session;
    for (let step = 0; step < 10 && parents.has(root); step++) root = parents.get(root)!;
    if (ask.at >= (asked.get(root) ?? Infinity)) continue;
    asked.set(root, ask.at);
    if (ask.asks) asks.set(root, ask.asks);
    else asks.delete(root);
  }
  return {
    isChatsFolder, botsFolder: BOTS, botSessions: known.botSessions, botOf: known.botOf, filed: known.filed, projects: known.projects, running, childOnly, directAsks, asked, asks, away: known.away,
    thinking: moments.thinking, stopped, retrying: moments.retrying, judging: isJudged,
  };
}

// Each session's facts as last worked out, while what they come from is the same (the session as
// read, what the box knew then, whether it runs, asks or is away, and its moments): not worked out
// again each time.
interface Made { known: BoxKnows; running: boolean; childOnly: boolean; directAsk: boolean; wakeTurn: boolean; asked?: number; away: boolean; thinking: boolean; stopped?: Stopped; retrying?: string; judged: boolean; facts: SessionFacts }
const factsMade = new WeakMap<EngineSession, Made>();

function factsNow(session: EngineSession, place: ListPlace, known?: BoxKnows): SessionFacts {
  if (!known) return factsOf(session, place);
  const id = session.id;
  const now: Omit<Made, "facts"> = {
    known, running: place.running.has(id), childOnly: place.childOnly?.has(id) ?? false, directAsk: place.directAsks?.has(id) ?? false, wakeTurn: place.wakeTurns?.has(id) ?? false, asked: place.asked.get(id), away: place.away.has(id),
    thinking: place.thinking?.has(id) ?? false, stopped: place.stopped?.get(id), retrying: place.retrying?.get(id), judged: place.judging?.(id) ?? false,
  };
  const made = factsMade.get(session);
  if (made && (Object.keys(now) as (keyof typeof now)[]).every(key => made[key] === now[key])) return made.facts;
  const facts = factsOf(session, place);
  factsMade.set(session, { ...now, facts });
  return facts;
}

/** `read` as it's taken in over `before` (the same session as it was): seen as far as either says
 * (a look told meanwhile isn't undone by a read begun before it). */
function withViewed(read: EngineSession, before: EngineSession | undefined): EngineSession {
  const viewed = Number(before?.time?.viewed) || 0;
  return viewed > (Number(read.time?.viewed) || 0) ? { ...read, time: { ...read.time, viewed } } : read;
}

/** Every session in `sessions` (the engine's or the bridge's, each once) with its facts. */
function sessionFacts(sessions: EngineSession[], place: ListPlace, known?: BoxKnows): Record<string, SessionFacts> {
  const facts: Record<string, SessionFacts> = {};
  for (const session of sessions) if (typeof session?.id === "string" && !(session.id in facts)) facts[session.id] = factsNow(session, place, known);
  return facts;
}

/** Every session on `target`, subagents' too, page after page; null where its list can't be read. */
export async function readSessions(target: Target, signal?: AbortSignal, strict = false): Promise<EngineSession[] | null> {
  const all: EngineSession[] = [];
  const seen = new Set<string>();
  let query: string | null = "limit=200&order=desc";
  for (let pages = 0; query && pages < 500; pages++) {
    if (signal?.aborted) return null;
    const page = await engineGet(target, `api/session?${query}`).catch(() => null) as { data?: unknown; cursor?: { next?: unknown } | null } | null;
    const listed: unknown[] | null = Array.isArray(page?.data) ? page.data : null;
    if (!listed || (strict && listed.some(each => !isObject(each) || typeof each.id !== "string"))) return null;
    const fresh = listed.filter((each): each is EngineSession => isObject(each) && typeof each.id === "string" && !seen.has(each.id));
    for (const each of fresh) seen.add(each.id);
    all.push(...fresh);
    const next = page?.cursor?.next;
    if (strict && typeof next === "string" && next && !fresh.length) return null;
    // (On as long as the engine says where the next page starts: a page may be shorter than asked.)
    query = listed.length && fresh.length && typeof next === "string" && next ? `limit=200&cursor=${encodeURIComponent(next)}` : null;
  }
  return strict && query ? null : all;
}

/** Those at work on `target`; null where it can't say. */
export async function readRunning(target: Target, signal?: AbortSignal): Promise<Set<string> | null> {
  try {
    const active: unknown = await call(target, "GET", "/api/session/active", undefined, signal ? AbortSignal.any([signal, AbortSignal.timeout(15_000)]) : AbortSignal.timeout(15_000));
    if (Array.isArray(active)) return new Set(active.filter((id): id is string => typeof id === "string"));
    return isObject(active) ? new Set(Object.keys(active)) : null;
  } catch {
    return null;
  }
}

// MARK: The list, kept

/** The list in its file; null where there's none yet (or it can't be read). */
export function readList(file = LIST_FILE): Kept | null {
  try {
    const read = JSON.parse(fs.readFileSync(file, "utf8"));
    return isObject(read) && typeof read.epoch === "string" && typeof read.seq === "number" && typeof read.floor === "number" && isObject(read.sessions) && isObject(read.gone) && isObject(read.projects)
      ? read as unknown as Kept : null;
  } catch {
    return null;
  }
}

/** The list with `listed` taken in, as it is now, under its lock: each session whose facts changed
 * with the next change's number, each one no longer there kept as gone (a day, the latest 2000; a
 * `since` from before the oldest let go reads it all). A list that isn't there starts a new epoch. */
export function commit(listed: Listed, now = Date.now(), file = LIST_FILE, owner?: number): Kept {
  return locked(file + ".lock", () => {
    const list: Kept = readList(file) ?? { epoch: `${now.toString(36)}-${process.pid.toString(36)}`, seq: 0, floor: 0, sessions: {}, gone: {}, projects: {} };
    // A route's, while the long-running process keeps it (it started since the route looked): its
    // list stands, mark and all.
    if (owner === undefined && list.owner && list.owner !== process.pid && processRuns(list.owner, "kuvo-link")) return list;
    // Taken over from a long-running process that went (this one started again, or a route
    // answers in its place): it may have told changes it hadn't written yet. Numbered on past them,
    // and every cursor from before reads it all (one whole read after a restart).
    if (list.owner !== undefined && (list.owner !== owner || list.started !== STARTED)) {
      list.seq += LOST_GAP;
      list.floor = list.seq;
    }
    const { changed, noted } = takeIn(list, listed, now, owner);
    if (changed || noted) try { writePrivate(file, JSON.stringify(list)); } catch {}
    return list;
  });
}

/** Whether two sessions' facts say the same (but `updated`, with `ignoring`). */
function sameFacts(a: SessionFacts, b: SessionFacts, ignoring?: "updated"): boolean {
  if (a === b) return true;
  const keys = Object.keys(a) as (keyof SessionFacts)[];
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every(key => {
    if (key === ignoring || a[key] === b[key]) return true;
    // These are rebuilt as the list is worked out. Their values, including subagent order,
    // determine what clients need to hear again.
    if (key === "subagents" || key === "line") {
      const before = a[key], after = b[key];
      return before !== undefined && after !== undefined &&
        before.length === after.length && before.every((id, at) => id === after[at]);
    }
    if (key === "messages") return a.messages?.via === b.messages?.via && a.messages?.note === b.messages?.note;
    return false;
  });
}

const projectsSaid = (projects: Record<string, ListedProject>, withUpdated: boolean) =>
  JSON.stringify(withUpdated ? projects : Object.fromEntries(Object.entries(projects).map(([folder, { updated: _, ...rest }]) => [folder, rest])));

/** `listed` taken into `list` (changed in place): each session whose facts changed with the next
 * change's number, each one no longer there kept as gone (a day, the latest 2000; a `since` from
 * before the oldest let go reads it all). Whether it changed, whether only what the file says did
 * (`noted`), and whether anything but when sessions last changed did (`telling`). */
function takeIn(list: Kept, listed: Listed, now: number, owner?: number): { changed: boolean; noted: boolean; telling: boolean } {
  const next = list.seq + 1;
  let changed = false, telling = false;
  // (What only the file says, not the list: written, with no change to tell.)
  let noted = list.owner !== owner;
  if (owner === undefined) { delete list.owner; delete list.started; }
  else { noted ||= list.started !== STARTED; list.owner = owner; list.started = STARTED; }
  for (const [id, facts] of Object.entries(listed.sessions)) {
    const before = list.sessions[id];
    const place = listed.placeOf?.[id];
    const work = listed.work?.[id];
    const sameWork = before?.work === work || JSON.stringify(before?.work) === JSON.stringify(work);
    if (before && sameFacts(before.facts, facts) && sameWork) {
      if (place && before.place !== place) { before.place = place; noted = true; }
      continue;
    }
    const significant = !before || !sameFacts(before.facts, facts, "updated") || !sameWork;
    if (significant) telling = true;
    const workSeq = work?.bot ? significant ? next : before?.workSeq ?? next : undefined;
    list.sessions[id] = { seq: next, facts, ...(place ? { place } : {}), ...(work ? { work } : {}), ...(workSeq !== undefined ? { workSeq } : {}) };
    delete list.gone[id];
    changed = true;
  }
  for (const id of Object.keys(list.sessions)) {
    if (id in listed.sessions) continue;
    delete list.sessions[id];
    list.gone[id] = { seq: next, at: now };
    changed = telling = true;
  }
  if (projectsSaid(list.projects, true) !== projectsSaid(listed.projects, true)) {
    if (projectsSaid(list.projects, false) !== projectsSaid(listed.projects, false)) telling = true;
    list.projects = listed.projects;
    changed = true;
  }
  const needs = listed.needsYou ?? needsYou(listed.sessions);
  if (JSON.stringify(list.needsYou) !== JSON.stringify(needs)) {
    list.needsYou = needs;
    changed = telling = true;
  }
  const pending = listed.pending?.length ? [...listed.pending].sort() : undefined;
  if (JSON.stringify(list.pending) !== JSON.stringify(pending)) {
    if (pending) list.pending = pending;
    else delete list.pending;
    changed = telling = true;
  }
  if (changed || !list.workByBot) {
    noted ||= !list.workByBot;
    const workByBot: Record<string, string[]> = {};
    for (const [id, { facts, work }] of Object.entries(list.sessions)) {
      const bot = workBot(facts, work);
      if (bot && /^bot_[A-Za-z0-9]+$/.test(bot)) (workByBot[bot] ??= []).push(id);
    }
    list.workByBot = workByBot;
  }
  if (!changed) return { changed, noted, telling };
  list.seq = next;
  const gone = Object.entries(list.gone).sort((a, b) => b[1].seq - a[1].seq);
  for (const [index, [id, { seq, at }]] of gone.entries()) {
    if (index < GONE_KEPT && now - at < GONE_FOR) continue;
    delete list.gone[id];
    list.floor = Math.max(list.floor, seq);
  }
  return { changed, noted, telling };
}

/** The list from `list`: what changed after `since` (in its epoch, not before the oldest change it
 * can still say), else all of it. */
/** The archived list, whole in every answer; clients draw these groups in this order. */
export function archivedGroups(sessions: Record<string, SessionFacts>, projects: Record<string, ListedProject>, savedProjects: readonly { directory: string; name?: string }[] = []): SessionList["archivedGroups"] {
  const groups = new Map<string, Omit<SessionList["archivedGroups"][number], "sessions"> & { sessions: string[] }>();
  const rows = Object.entries(sessions).filter(([, facts]) => facts.archived && !facts.away && ["project", "chat", "bot"].includes(facts.kind))
    .sort((a, b) => b[1].updated - a[1].updated || a[0].localeCompare(b[0]));
  for (const [id, facts] of rows) {
    const directory = facts.project ?? facts.directory;
    const key = `${facts.kind}:${directory}`;
    const kind = facts.kind as "project" | "chat" | "bot";
    const group = groups.get(key) ?? { directory, kind, title: kind === "chat" ? "Chats" : projects[directory]?.name ?? savedProjects.find(project => project.directory === directory)?.name ?? path.basename(directory), sessions: [] };
    group.sessions.push(id);
    groups.set(key, group);
  }
  return [...groups.values()];
}

export function answerFrom(list: Kept, since: string | null): SessionList {
  const from = parseCursor(since);
  const whole = !from || from.epoch !== list.epoch || from.seq < list.floor || from.seq > list.seq;
  const sessions: Record<string, SessionFacts> = {};
  for (const [id, entry] of Object.entries(list.sessions)) if (whole || entry.seq > from!.seq) sessions[id] = entry.facts;
  const pending = list.pending?.length ? { pending: list.pending } : {};
  // (A file from before "Needs you" says none of what each asks until it's worked out again.)
  const needs = list.needsYou ?? needsYou(Object.fromEntries(Object.entries(list.sessions).map(([id, entry]) => [id, entry.facts])));
  // (And its projects' sums as none, where that file had none.)
  const projects = Object.fromEntries(Object.entries(list.projects).map(([folder, project]) => [folder, { ...project, asking: project.asking ?? 0, working: project.working ?? 0 }]));
  const ordered = projectOrder(projects);
  const archived = archivedGroups(Object.fromEntries(Object.entries(list.sessions).map(([id, entry]) => [id, entry.facts])), projects, known?.projects ?? readProjects());
  const projectFacts = { projects, projectOrder: ordered, startProject: ordered[0] ?? null };
  if (whole) return { archivedGroups: archived, at: cursorOf(list), full: true, sessions, ...projectFacts, needsYou: needs, ...pending };
  const gone = Object.entries(list.gone).filter(([, entry]) => entry.seq > from!.seq).map(([id]) => id).sort();
  return { archivedGroups: archived, at: cursorOf(list), sessions, ...(gone.length ? { gone } : {}), ...projectFacts, needsYou: needs, ...pending };
}

// MARK: The long-running process's list

/** The box's files a list goes by, which another process may change (a `box route` over SSH, a
 * move, kuvo-wake): its projects and worktrees (`box`, only those files), its bots and its moves. */
const LIST_FILES = new Set(["projects.json", "worktrees.json"]);
const BOT_FILES = new Set(["bot.json", "session"]);

/** Starts keeping the list here (the box's long-running process), worked out again as its
 * projects, worktrees, bots, moves or asks change, here or in another process (as the system says
 * their files changed). How to stop. */
export function keepList(): () => void {
  setKeepsList(true);
  latest = null;
  places.clear();
  known = null;
  lastWrite = -Infinity;
  // (Those stopped on an error or a limit, as the list was written: blocked still, till they run
  // again or you've seen them.)
  moments.clear();
  moments.seed(wasStopped(readList()));
  // (What the box knows read again, once, at the next working out.)
  const again = () => { known = null; scheduleWorkOut(); };
  // (And as what a session left running starts and ends: it's at work meanwhile.)
  const followers = [projectFollowers, botFollowers, moveFollowers, askFollowers, backgroundFollowers];
  for (const each of followers) each.add(again);
  // (And as the box has judged an agent's work, read for you or not: whether its turn is unread.)
  judgingFollowers.add(scheduleWorkOut);
  // (Only what a list goes by: the box's other files, and what a bot keeps in its folder as it
  // works, change often and say nothing of it.)
  const box = watchFolders([BOX], (...told: unknown[]) => { if (typeof told[1] !== "string" || LIST_FILES.has(told[1])) again(); }, { recursive: false, missed: again });
  const moves = watchFolders([MOVES], again, { recursive: false });
  // Each bot's own folder, not what's in it (where its agent works: a repository, its packages,
  // which on Linux would each take a watch of the user's): watched as bots come and go.
  const botWatches = new Map<string, SharedWatch>();
  const watchBots = () => {
    const now = new Set(listOf(BOTS).filter(id => /^bot_[A-Za-z0-9]+$/.test(id)));
    for (const [id, watch] of botWatches) if (!now.has(id)) { watch.close(); botWatches.delete(id); }
    for (const id of now) {
      if (botWatches.has(id)) continue;
      const watch = watchFolder(path.join(BOTS, id), (_, name) => {
        if (!name || BOT_FILES.has(name)) again();
      }, { missed: again });
      botWatches.set(id, watch);
    }
  };
  const mac = process.platform === "darwin";
  const bots = mac
    ? watchFolders([BOTS], (_, name) => {
        const [id, file, ...inside] = name?.split(path.sep) ?? [];
        if (!id || (!inside.length && (!file || BOT_FILES.has(file)))) again();
      }, { recursive: true, missed: again })
    : watchFolders([BOTS], () => { watchBots(); again(); }, { recursive: false });
  if (!mac) watchBots();
  // (A watch the system couldn't keep: what the box knows read each time instead.)
  watchesWhole = () => box.whole && moves.whole && bots.whole && [...botWatches.values()].every(watch => watch.whole);
  const whole = setInterval(() => { for (const [name, place] of placeTargets) readPlace(name, place).catch(() => {}); }, READ_EVERY);
  whole.unref?.();
  return () => {
    for (const watch of [box, moves, bots, ...botWatches.values()]) watch.close();
    botWatches.clear();
    for (const each of followers) each.delete(again);
    judgingFollowers.delete(scheduleWorkOut);
    clearInterval(whole);
    clearTimeout(workOutTimer);
    workOutTimer = undefined;
    // (What's not written yet, written now: a box started again goes on from it.)
    if (writeTimer) writeNow();
    setKeepsList(false);
    latest = null;
    known = null;
    watchesWhole = () => true;
    asked.clear();
    deleted.clear();
    moments.clear();
    for (const place of places.values()) for (const timer of place.wakeRetries.values()) clearTimeout(timer);
    places.clear();
    runningReads.clear();
    placeTargets.clear();
    reading.clear();
    wholeReads.clear();
  };
}

// Each place as it was last heard, to read it whole again now and then.
const placeTargets = new Map<string, Target>();
// The sessions each place's events named while it was read whole, to read again once it's in.
const reading = new Map<string, Set<string>>();
// Each place's whole read under way: one at a time, and one more after it if it's asked for meanwhile.
const runningReads = new Map<string, Set<string>>();
const wholeReads = new Map<string, { done: Promise<void>; again: boolean; target: Target }>();

/** A place heard (it started, or its events came back after they couldn't be): read whole now, as
 * what it did meanwhile went by unheard. */
export function listPlaceHeard(name: string, target: Target): Promise<void> {
  if (!keepsList()) return Promise.resolve();
  placeTargets.set(name, target);
  return readPlace(name, target);
}

/** `name` read whole: after the one under way, if there is one (so an older read never lands over a
 * newer one), and once however often it's asked for meanwhile. */
function readPlace(name: string, target: Target): Promise<void> {
  const under = wholeReads.get(name);
  if (under) {
    under.again = true;
    under.target = target;
    return under.done;
  }
  const read = { done: Promise.resolve(), again: false, target };
  read.done = (async () => {
    try {
      do {
        read.again = false;
        await readOnce(name, read.target);
      } while (read.again && keepsList());
    } finally {
      if (wholeReads.get(name) === read) wholeReads.delete(name);
    }
  })();
  wholeReads.set(name, read);
  return read.done;
}

async function readRunningSnapshot(name: string, target: Target, alongside?: Promise<unknown>) {
  const changed = new Set<string>();
  runningReads.set(name, changed);
  try {
    const [running] = await Promise.all([readRunning(target), alongside]);
    return { running, changed };
  }
  finally { if (runningReads.get(name) === changed) runningReads.delete(name); }
}

function applyRunning(place: Place, snapshot: Awaited<ReturnType<typeof readRunningSnapshot>>): void {
  if (!snapshot.running) return;
  const { running, changed } = snapshot;
  for (const id of changed) {
    if (place.running.has(id)) running.add(id);
    else running.delete(id);
  }
  place.running = running;
}

async function readOnce(name: string, target: Target): Promise<void> {
  const touched = new Set<string>();
  reading.set(name, touched);
  let sessions: EngineSession[] | null = null;
  let running: Awaited<ReturnType<typeof readRunningSnapshot>>;
  let facts: EngineListFacts | undefined;
  try {
    const listed = readSessions(target).then(async sessions => {
      if (sessions) await discoverProjects(sessions);
      return sessions;
    });
    [sessions, running, facts] = await Promise.all([
      listed, readRunningSnapshot(name, target, listed), name === "engine" ? readEngineFacts() : undefined,
    ]);
  } finally {
    if (reading.get(name) === touched) reading.delete(name);
  }
  if (!keepsList() || !sessions || placeTargets.get(name) !== target) return;
  // (Only a whole read makes a place: until then its events have nothing to change.)
  const place: Place = places.get(name) ?? { target, sessions: new Map(), running: new Set(), wakeReads: new Map(), wakeRetries: new Map(), waking: new Set() };
  // (Not one deleted as it was read: a read begun before says it's there.)
  const gone = deleted.get(name);
  place.target = target;
  if (facts?.available) {
    engineWorktrees = facts.worktrees;
    known = null;
    place.engineActivity = true;
    sessions = withEngineActivity(sessions, facts);
  }
  place.sessions = new Map(sessions.filter(each => !gone?.has(each.id)).map(each => [each.id, withViewed(each, place.sessions.get(each.id))]));
  deleted.delete(name);
  // (Its events' count starts again: a read under way for an earlier one answers for nothing now.)
  for (const key of asked.keys()) if (key.startsWith(name + " ")) asked.delete(key);
  // And what the box knows read again, in case a watch missed a change.
  known = null;
  applyRunning(place, running);
  places.set(name, place);
  // What its events said meanwhile may be newer than what was read: read those again.
  if (touched.size) {
    await Promise.all([...touched].map(id => readOne(name, target, id)));
    const refreshed = await readRunningSnapshot(name, target);
    if (!keepsList() || places.get(name) !== place) return;
    applyRunning(place, refreshed);
  }
  for (const id of new Set([...place.wakeReads.keys(), ...place.wakeRetries.keys(), ...place.waking])) if (!place.running.has(id)) clearWakeTurn(place, id);
  const bots = readBoxKnows().botSessions;
  await Promise.all([...place.running].filter(id => bots.has(id) && !place.wakeReads.has(id)).map(id => readWakeTurn(place, target, id)));
  workOut();
}

function clearWakeTurn(place: Place, id: string): void {
  place.wakeReads.delete(id);
  clearTimeout(place.wakeRetries.get(id));
  place.wakeRetries.delete(id);
  place.waking.delete(id);
}

async function readWakeTurn(place: Place, target: Target, id: string): Promise<void> {
  // An end (or the next start) invalidates a read of the preceding turn, even if it lands later.
  clearTimeout(place.wakeRetries.get(id));
  place.wakeRetries.delete(id);
  const read = {};
  place.wakeReads.set(id, read);
  const waking = await wakeTurnIsOpen(target, id).catch(() => null);
  if (place.wakeReads.get(id) !== read || !place.running.has(id)) return;
  place.wakeReads.delete(id);
  if (waking === null) {
    if (!keepsList() || ![...places.values()].includes(place)) return;
    const retry = setTimeout(() => void readWakeTurn(place, target, id), 250);
    retry.unref?.();
    place.wakeRetries.set(id, retry);
    return;
  }
  if (waking) place.waking.add(id);
  else place.waking.delete(id);
  scheduleWorkOut();
}

// Each session's events so far, by place, so a read answers only for the latest (one read after an
// earlier event, landing after a later one's, says nothing); and the sessions deleted since each
// place was last read whole, which no read brings back until the next whole read.
const asked = new Map<string, number>();
const deleted = new Map<string, Set<string>>();
let eventCount = 0;

/** One session of `name`'s read again: as it is now, or gone; unless another event came for it
 * since (a later read answers for that), or it was deleted. Whether it was taken in as read now. */
async function readOne(name: string, target: Target, id: string, metadataOnly = false): Promise<boolean> {
  const key = `${name} ${id}`;
  const at = asked.get(key);
  const current = () => asked.get(key) === at && !deleted.get(name)?.has(id) && sameTarget(places.get(name)?.target, target);
  try {
    const session = await call(target, "GET", `/api/session/${encodeURIComponent(id)}${name === "bridge" ? "?context=0" : ""}`, undefined, AbortSignal.timeout(10_000));
    if (!current() || !isObject(session) || session.id !== id) return false;
    const read = session as unknown as EngineSession;
    const folder = folderOf(read);
    if (!gitProjects.has(folder) && !(known ??= readBoxKnows()).filed[folder]) await discoverProjects([read]);
    const place = places.get(name);
    const before = place?.sessions.get(id);
    if (place?.engineActivity && read.time?.activity === undefined) {
      const previous = before?.time?.activity ?? read.time.created;
      const activity = metadataOnly ? previous : await recentEngineActivity(target, id, read.time.created, previous);
      read.time = { ...read.time, activity };
    }
    if (!current() || !place || places.get(name) !== place) return false;
    place.sessions.set(id, withViewed(read, place.sessions.get(id)));
    return true;
  } catch (error) {
    if (current() && (error as { status?: number }).status === 404) places.get(name)?.sessions.delete(id);
    return false;
  }
}

async function recentEngineActivity(target: Target, id: string, created: number, previous: number): Promise<number> {
  const page = await engineGet(target, `api/session/${encodeURIComponent(id)}/message?limit=1&order=desc`).catch(() => null) as
    { data?: { time?: { created?: number } }[] } | null;
  if (!Array.isArray(page?.data)) return previous;
  return page.data[0]?.time?.created ?? created;
}

// A start answers before the list's next read. Keep the runtime the box already heard
// during that gap, including when it heard the engine's event before the start answered.
const justCreated = new Map<string, { place: "engine" | "agents"; at: number; directory?: string }>();

export function noteSessionCreated(id: string, place: "engine" | "agents", directory?: string, now = Date.now()): void {
  justCreated.set(id, { place, at: now, directory: directory ?? createdDirectory(id, now) });
  if (justCreated.size > 256) justCreated.delete(justCreated.keys().next().value!);
}

export function createdRuntime(id: string, now = Date.now()) {
  const created = justCreated.get(id);
  if (created && now - created.at < 60_000) return { place: created.place, running: false };
  return undefined;
}

export function createdDirectory(id: string, now = Date.now()) {
  const created = justCreated.get(id);
  return created && now - created.at < 60_000 ? created.directory : undefined;
}

/** Takes in an event from `name` (the engine, or the bridge, at `target`): a session read again as
 * it's made or changes, its run starting or ending; dropped as it's deleted. What it says from
 * moment to moment (thinking, blocked, `moments.ts`) and a look at it on any client
 * (`session.viewed`) are taken in as they are, with no read. A place not read whole yet takes
 * nothing (its whole read reads it again). Settled once what it read is in (a test waits on it);
 * worked out a moment after, so a few events at once are one. */
export async function noteListEvent(name: string, target: Target, event: { type?: string; data?: { sessionID?: unknown; info?: { id?: unknown }; error?: unknown; idle?: unknown } }): Promise<void> {
  if (!keepsList() || typeof event?.type !== "string") return;
  const id = event.data?.sessionID ?? event.data?.info?.id;
  const finishes = ENDS.has(event.type) || (name === "bridge" && event.type === "session.idle");
  if (typeof id !== "string" || !id.startsWith("ses")) return;
  if (event.type === "session.created") {
    // A move can bring the same session ID back after this engine deleted it.
    // That fresh creation supersedes the deletion fence for a whole-list read.
    if (sameTarget(placeTargets.get(name), target)) deleted.get(name)?.delete(id);
    noteSessionCreated(id, name === "bridge" ? "agents" : "engine");
  }
  if (event.type === "session.deleted") justCreated.delete(id);
  if (event.type === "session.viewed") {
    // (Read whole now: it's read again once that's in, so the look isn't lost.)
    reading.get(name)?.add(id);
    return noteViewed(name, id, Number(event.data?.idle));
  }
  if (event.type === DELIVERED) {
    const place = places.get(name);
    if (place?.running.has(id) && !place.waking.has(id) && (known ??= readBoxKnows()).botSessions.has(id)) await readWakeTurn(place, target, id);
    return;
  }
  // (What it streams: worked out again only where a fact changed, as `moments` tells it.)
  const moment = moments.hear(event.type, id, event.data);
  if (moment && event.type !== "session.deleted" && event.type !== STARTS && !finishes) return;
  // Only those that change what's listed of it count (not what it streams meanwhile: a read for a
  // pin or a move still out isn't undone by the next word it says).
  if (event.type !== "session.deleted" && event.type !== STARTS && !finishes && !RELOADS.has(event.type)) return;
  // (Read whole now: it's read again once that's in.)
  reading.get(name)?.add(id);
  if (event.type === STARTS || finishes || event.type === "session.deleted") runningReads.get(name)?.add(id);
  const mine = ++eventCount;
  asked.set(`${name} ${id}`, mine);
  if (event.type === "session.deleted") {
    deleted.set(name, (deleted.get(name) ?? new Set()).add(id));
    if (deleted.get(name)!.size > 5000) deleted.get(name)!.clear();
  }
  const place = places.get(name);
  if (!place || !sameTarget(place.target, target)) return;
  if (event.type === "session.deleted") {
    place.sessions.delete(id);
    place.running.delete(id);
    clearWakeTurn(place, id);
    return scheduleWorkOut();
  }
  if (event.type === STARTS) {
    clearWakeTurn(place, id);
    place.running.add(id);
    if ((known ??= readBoxKnows()).botSessions.has(id)) void readWakeTurn(place, target, id);
  } else if (finishes) {
    place.running.delete(id);
    clearWakeTurn(place, id);
  }
  scheduleWorkOut();
  // As it is now: its folder, title, notes and times.
  const stop = moments.stopped.get(id);
  const landed = await readOne(name, target, id, event.type === "session.renamed" || event.type === "session.metadata.updated");
  // (A stop heard before this read: the turn it ended is the one read, its end as the run's end
  // left it (as READ_RULE reads it too), unless another event came since. Known, it's let go once
  // it can no longer block: `letGoOfStops`.)
  const read = places.get(name)?.sessions.get(id);
  if (landed && stop && asked.get(`${name} ${id}`) === mine && moments.stopped.get(id) === stop && read) moments.pin(id, Number(read.time?.idle));
  scheduleWorkOut();
}

/** `id` on `name` looked at up to `idle` (ms) on some client (`session.viewed`, or the box's own
 * look): kept as its `time.viewed` where that's later, with no read (a hard stop it ended on, seen,
 * no longer blocks it: `factsOf`). */
export function noteViewed(name: string, id: string, idle: number): void {
  const session = places.get(name)?.sessions.get(id);
  if (!keepsList() || !session || !Number.isFinite(idle) || idle <= (Number(session.time?.viewed) || 0)) return;
  places.get(name)!.sessions.set(id, { ...session, time: { ...session.time, viewed: idle } });
  scheduleWorkOut();
}

/** When the turns of `bot`'s sessions the box is judging now ended (ms): none of them is unread yet,
 * as the box hasn't said whether it's news to you (READ_RULE). The long-running process's; none
 * elsewhere. */
export function judgedTurns(bot: string): number[] {
  if (!keepsList() || !latest) return [];
  const ends: number[] = [];
  for (const place of places.values()) {
    for (const session of place.sessions.values()) {
      if (!isJudged(session.id) || latest.sessions[session.id]?.facts.bot !== bot) continue;
      const idle = Number(session.time?.idle) || 0;
      if (idle > (Number(session.time?.viewed) || 0)) ends.push(idle);
    }
  }
  return ends;
}

/** Those `list` has stopped on an error or a limit (blocked by a turn you haven't read, not a
 * retry), each with what stopped it and the turn it stopped (one run again since, unheard, isn't
 * blocked by it). */
const wasStopped = (list: Kept | null): [string, Stopped][] =>
  Object.entries(list?.sessions ?? {}).flatMap(([id, { facts }]) => (facts.blocked && !facts.retrying && facts.unread !== undefined ? [[id, { by: facts.blocked, idle: facts.unread }] as [string, Stopped]] : []));

/** The hard stops that can no longer block their sessions let go, so they don't only grow: each
 * whose turn (`idle`, where known) has been read past, or that a later turn has ended since, and
 * each whose session neither the engine nor the bridge has, both read whole (gone unheard). (One
 * whose turn isn't known, its read not in yet, is kept: it goes as it runs again or is deleted.) */
function letGoOfStops(): void {
  const whole = places.has("engine") && places.has("bridge");
  for (const [id, stop] of [...moments.stopped]) {
    let session: EngineSession | undefined;
    for (const place of places.values()) if ((session = place.sessions.get(id))) break;
    if (!session) {
      if (whole) moments.forget(id);
      continue;
    }
    if (stop.idle === undefined) continue;
    // (A later turn only: a read begun before it failed, landing late, says an earlier one.)
    if ((Number(session.time?.viewed) || 0) >= stop.idle || (Number(session.time?.idle) || 0) > stop.idle) moments.forget(id);
  }
}

let workOutTimer: ReturnType<typeof setTimeout> | undefined;
/** Worked out again in a moment: a few changes at once are one. */
function scheduleWorkOut(): void {
  if (!keepsList() || workOutTimer) return;
  workOutTimer = setTimeout(() => { workOutTimer = undefined; workOut(); }, 50);
  workOutTimer.unref?.();
}

/** The places that run whose sessions aren't read whole (the coding agents' bridge, running but
 * not answering yet): their sessions stay as they were last read. */
const pendingPlaces = (): string[] => (places.has("bridge") || bridgeTarget() === null ? [] : ["bridge"]);

/** `sessions` (each with its place) as listed, with the sessions of `pending` places carried over as
 * `before` had them. */
function withPending(sessions: EngineSession[], placeOf: Record<string, string>, running: Set<string>, pending: string[], before: Kept | null, knows?: BoxKnows, stopped?: ReadonlyMap<string, Stopped>, wakeTurns?: ReadonlySet<string>): Listed {
  const box = placeNow(running, sessions, knows, undefined, stopped);
  const listed = sessionFacts(sessions, { ...box, wakeTurns }, knows);
  const work: Record<string, ListedWork> = {};
  for (const session of sessions) {
    const entry = listedWork(session);
    if (entry) work[session.id] ??= entry;
  }
  if (pending.length && before) {
    for (const [id, entry] of Object.entries(before.sessions)) {
      if (id in listed || !entry.place || !pending.includes(entry.place)) continue;
      const { wakeTurn: _, ...facts } = entry.facts;
      listed[id] = facts;
      placeOf[id] = entry.place;
      if (entry.work) work[id] = entry.work;
    }
  }
  const nested = withSubagents(listed);
  return { sessions: nested, projects: listedProjects(nested, box), needsYou: needsYou(nested, box.asks), placeOf, pending, work };
}

/** Every session's facts worked out again from what's kept of its places, once the engine's has
 * been read whole, and kept; followers told of a change. */
export function workOut(now = Date.now()): void {
  if (!keepsList()) return;
  clearTimeout(workOutTimer);
  workOutTimer = undefined;
  if (!places.has("engine")) return;
  letGoOfStops();
  const sessions: EngineSession[] = [];
  const placeOf: Record<string, string> = {};
  const running = new Set<string>();
  const wakeTurns = new Set<string>();
  for (const [name, place] of places) {
    for (const id of place.waking) if (place.running.has(id)) wakeTurns.add(id);
    for (const session of place.sessions.values()) {
      sessions.push(session);
      placeOf[session.id] ??= name;
    }
    for (const id of place.running) running.add(id);
  }
  const was = latest ? cursorOf(latest) : null;
  if (!watchesWhole()) known = null;
  known ??= readBoxKnows();
  const listed = withPending(sessions, placeOf, running, pendingPlaces(), latest ?? readList(), known, undefined, wakeTurns);
  if (latest?.owner === process.pid && latest.started === STARTED) {
    // Its own, as it keeps it: what changed taken in here, the file written a moment after (a few
    // changes at once are one write; one of when sessions last changed alone, more slowly).
    const { changed, noted, telling } = takeIn(latest, listed, now, process.pid);
    if (changed || noted) scheduleWrite(telling || noted);
  } else {
    latest = commit(listed, now, LIST_FILE, process.pid);
    lastWrite = performance.now();
  }
  if (was === cursorOf(latest)) return;
  for (const tell of [...listFollowers]) try { tell(); } catch {}
}

// What's not written yet, written as the process ends (it's stopped: the keeper, an update).
process.once("exit", () => { if (writeTimer) writeNow(); });

export { keepsList };

function sameTarget(before: Target | undefined, after: Target): boolean {
  return before?.port === after.port && before.password === after.password;
}

/** Raw session records already maintained by the engine and bridge event followers. */
export function keptSessions(target: Target): ReadonlyMap<string, EngineSession> | undefined {
  for (const place of places.values()) if (sameTarget(place.target, target)) return place.sessions;
  return undefined;
}

/** The list as this process keeps it (the long-running one's), else as its file has it. */
export const keptList = (): Kept | null => latest ?? readList();

/** The list as last saved, before this process has read its engine: every place said to be as
 * last read (`pending`). None the first time ever. */
function savedList(): Kept | null {
  const file = readList();
  if (!file) return null;
  return { ...file, pending: [...new Set([...(file.pending ?? []), "engine"])] };
}

// What the box knows beside its sessions, as last read (`readBoxKnows`): null to read again.
let known: BoxKnows | null = null;
let watchesWhole = (): boolean => true;

/** How often the file is written at most: once a second, or every ten while only when sessions
 * last changed did (a run's every step). */
const WRITE_EVERY = 1000;
const QUIET_WRITE_EVERY = 10_000;
let lastWrite = -Infinity;
let writeTimer: ReturnType<typeof setTimeout> | undefined;
let writeDue = Infinity;

function scheduleWrite(soon: boolean): void {
  const now = performance.now();
  const due = Math.max(now, lastWrite + (soon ? WRITE_EVERY : QUIET_WRITE_EVERY));
  if (due <= now) return writeNow();
  if (writeTimer && writeDue <= due) return;
  clearTimeout(writeTimer);
  writeDue = due;
  writeTimer = setTimeout(writeNow, due - now);
  writeTimer.unref?.();
}

function writeNow(): void {
  clearTimeout(writeTimer);
  writeTimer = undefined;
  writeDue = Infinity;
  lastWrite = performance.now();
  const kept = latest;
  if (!kept) return;
  try { locked(LIST_FILE + ".lock", () => writePrivate(LIST_FILE, JSON.stringify(kept))); } catch {}
}

/** The engine's place read whole, where this process hasn't yet (it just started), and the bridge's
 * begun beside it if it runs (not waited for: a bridge that doesn't answer holds nothing up). */
async function readPlaces(context: Pick<RouteContext, "readEngine" | "bridge">): Promise<void> {
  let engine: Target | null = null;
  try { engine = context.readEngine(); } catch {}
  const bridge = context.bridge();
  if (bridge && !places.has("bridge")) listPlaceHeard("bridge", bridge).catch(() => {});
  if (engine && !places.has("engine")) await listPlaceHeard("engine", engine);
}

function readEngineFacts(): Promise<EngineListFacts | undefined> {
  return inTime(engineListFacts(), undefined);
}

/** The list now, for a route; null while the engine can't be read. In the long-running process: as
 * it keeps it, the engine read first where it hasn't been (it just started), worked out again with
 * the box's files as they are now (a project another process changed). In any other: as the
 * long-running process keeps it, while it runs; else read from the places now (the bridge's
 * sessions as last read where it doesn't answer), and kept. */
export async function listNow(context: Pick<RouteContext, "readEngine" | "bridge">, signal?: AbortSignal): Promise<Kept | null> {
  if (keepsList()) {
    if (!latest) await readPlaces(context);
    if (workOutTimer) workOut();
    return latest;
  }
  const file = readList();
  if (file?.owner && file.owner !== process.pid && processRuns(file.owner, "kuvo-link")) return file;
  let engine: Target;
  try { engine = context.readEngine(); } catch { return null; }
  const bridge = context.bridge();
  const [engineSessions, engineRunning, bridgeSessions, bridgeRunning] = await Promise.all([
    readSessions(engine, signal), readRunning(engine, signal),
    bridge ? readSessions(bridge, signal) : Promise.resolve(null), bridge ? readRunning(bridge, signal) : Promise.resolve(null),
  ]);
  if (!engineSessions) return null;
  const facts = await readEngineFacts();
  if (facts?.available) engineWorktrees = facts.worktrees;
  const sessions = facts ? withEngineActivity(engineSessions, facts) : [...engineSessions];
  const placeOf: Record<string, string> = Object.fromEntries(engineSessions.map(each => [each.id, "engine"]));
  if (bridgeSessions) for (const each of bridgeSessions) if (!(each.id in placeOf)) { sessions.push(each); placeOf[each.id] = "bridge"; }
  // (Those at work where it couldn't say: as last read.)
  const wasRunning = (name: string) => Object.entries(file?.sessions ?? {}).filter(([, entry]) => entry.place === name && entry.facts.running).map(([id]) => id);
  const running = new Set([...(engineRunning ?? wasRunning("engine")), ...(bridgeRunning ?? (bridge ? wasRunning("bridge") : []))]);
  const pending = bridge && !bridgeSessions ? ["bridge"] : [];
  // (Those stopped on an error or a limit, as last listed: this process hears no events.)
  await discoverProjects(sessions);
  const knows = readBoxKnows();
  const wakeTurns = new Set<string>();
  await Promise.all([...knows.botSessions].map(async id => {
    const target = placeOf[id] === "bridge" ? bridge : engine;
    const active = placeOf[id] === "bridge" ? bridgeRunning : engineRunning;
    if (target && active?.has(id) && await wakeTurnIsOpen(target, id).catch(() => false)) wakeTurns.add(id);
  }));
  return commit(withPending(sessions, placeOf, running, pending, file, knows, new Map(wasStopped(file)), wakeTurns));
}

export async function* listRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  const since = new URLSearchParams(request.path.split("?")[1] ?? "").get("since");
  // Just started, its engine not read yet: the list as last saved, said to be as last read
  // (`pending`), so a client draws your sessions at once. The engine is read meanwhile, and the
  // live list follows on the same stream.
  const saved = keepsList() && !latest ? savedList() : null;
  if (saved) listNow(context).catch(() => {});
  const list = saved ?? await listNow(context, signal ?? undefined);
  if (!list) return yield* json(503, { message: "The engine isn’t running." });
  if (!match[1]) return yield* json(200, answerFrom(list, since));
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // (Streamed only where it's kept: elsewhere its first line, as `box route` answers.)
  if (!keepsList()) return yield { body: Buffer.from(JSON.stringify(answerFrom(list, since)) + "\n") };
  let wake: (() => void) | null = null;
  let dirty = false;
  const told = () => { dirty = true; wake?.(); };
  listFollowers.add(told);
  signal?.addEventListener("abort", () => wake?.(), { once: true });
  try {
    let at: string | null = since;
    let first = true;
    while (!signal?.aborted) {
      dirty = false;
      const now = latest ?? (first ? saved : null);
      if (!keepsList()) return;
      // (The engine's read not done yet, after the saved list: the next look waits for it.)
      const answer = now ? answerFrom(now, at) : null;
      // (Nothing changed since: no line, but the first.)
      if (answer && (first || answer.full || at !== answer.at)) yield { body: Buffer.from(JSON.stringify(answer) + "\n") };
      first = false;
      if (answer) at = answer.at;
      if (dirty) continue;
      // A minute at most between looks: worked out again then, with the box's files as they are.
      const woke = await new Promise<boolean>(done => {
        const timer = setTimeout(() => done(false), 60_000);
        wake = () => { clearTimeout(timer); done(true); };
      });
      wake = null;
      if (!woke) { known = null; workOut(); }
    }
  } finally {
    listFollowers.delete(told);
  }
}
