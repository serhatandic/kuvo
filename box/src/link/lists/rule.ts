// What each session is, as every client's lists go by it (PROTOCOL.md, "Session lists"): a
// project's session filed under its project, a chat, a bot's conversation, a seat in a group chat, a
// run kept out of sight, or a subagent's; and the projects the lists show. The rule the Mac's sidebar
// went by (`Workspace.makeProjects`, `chats`), here so no client works it out. Pure: what the box
// knows comes in `ListPlace`, so the vectors (`protocol/vectors/session-lists.json`) check it.

import path from "node:path";
import { isObject } from "../../shared/json";
import type { EngineSession } from "../engine/sessions";
import type { BotNow, BotState, ListedProject, NeedsYou, SessionFacts, SessionKind } from "../../shared/routes/sessions";
import type { AskWords } from "../glance/asked";

/** What the rule reads of the box beside a session. */
export interface ListPlace {
  /** Whether a folder is the box's chats folder. */
  isChatsFolder: (directory: string) => boolean;
  /** The box's bots folder: a session in a bot's folder there is the bot's. */
  botsFolder: string;
  /** Its bots' conversations. */
  botSessions: ReadonlySet<string>;
  /** The project each folder sessions run in is filed under, where it isn't its own
   * (`filedProjects`: a worktree's, a project's other folder's). */
  filed: Readonly<Record<string, string>>;
  /** Its projects, as `kuvo/projects` keeps them. */
  projects: readonly { directory: string; name?: string; removed?: boolean }[];
  /** Those at work now. */
  running: ReadonlySet<string>;
  childOnly?: ReadonlySet<string>;
  directAsks?: ReadonlySet<string>;
  wakeTurns?: ReadonlySet<string>;
  /** Since when each waits on you (ms), its subagents' asks counting as its own. */
  asked: ReadonlyMap<string, number>;
  /** Copies that aren't this box's own (moved away, or on their way here). */
  away: ReadonlySet<string>;
  /** What each one waiting asks, in a few words: its oldest ask's (none known: "waits on you"). */
  asks?: ReadonlyMap<string, AskWords>;
  /** The bot each of its bots' conversations is, by its id. */
  botOf?: ReadonlyMap<string, string>;
  /** Those whose model has been reasoning a while (`thinking`, debounced as the box hears it): while
   * they're at work. */
  thinking?: ReadonlySet<string>;
  /** Each one that stopped on an error or a limit and hasn't run since (a hard stop: blocked while
   * you haven't read the turn it ended): what stopped it, and that turn's end (`idle`, where the box
   * knows it: one kept from before it started again). */
  stopped?: ReadonlyMap<string, Stopped>;
  /** What each one that has been retrying a while (since its last step began) retries: while it's at
   * work. */
  retrying?: ReadonlyMap<string, Blocked>;
  /** Those the box reads for you as each turn ends unless it's news to you (an agent's work) and is
   * judging now: no turn of theirs is unread until it's said (`work/tree.ts`, READ_RULE). */
  judging?: (session: string) => boolean;
}

/** What blocks a session: an error, or a usage or rate limit. */
export type Blocked = "error" | "limit";
/** A hard stop: what stopped it, and where known, the end of the turn it stopped. */
export interface Stopped { by: Blocked; idle?: number }

/** A session's folder, as the engine names it. */
export const folderOf = (session: Partial<EngineSession>): string => session.location?.directory ?? session.directory ?? "";

const kuvoOf = (session: Partial<EngineSession>): Record<string, unknown> => (isObject(session.metadata?.kuvo) ? session.metadata.kuvo : {});

/** Archived (here, by a client's note, or by an older opencode). */
export const isArchived = (session: Partial<EngineSession>): boolean =>
  Boolean(session.time?.archived) || session.metadata?.["kuvo.archived"] === true || kuvoOf(session).archived === true;

/** Kept at the top of the lists. */
export const isPinned = (session: Partial<EngineSession>): boolean => session.metadata?.["kuvo.pinned"] === true;

/** Whether `folder` is in the bots folder (a bot's own, even before the bot's listed). */
const inBotsFolder = (folder: string, botsFolder: string): boolean => Boolean(botsFolder) && folder.startsWith(botsFolder.endsWith(path.sep) ? botsFolder : botsFolder + path.sep);

/** What `session` is. */
export function kindOf(session: EngineSession, place: ListPlace): SessionKind {
  if (typeof session.parentID === "string" && session.parentID) return "subagent";
  const kuvo = kuvoOf(session);
  if (kuvo.seat !== undefined && kuvo.seat !== null) return "seat";
  if ((kuvo.engineFor !== undefined && kuvo.engineFor !== null) || (kuvo.reviewPart !== undefined && kuvo.reviewPart !== null)) return "hidden";
  const folder = folderOf(session);
  // (Its bots' as its bots list them: a `kuvo.bot` note alone doesn't make one a bot's.)
  if (place.botSessions.has(session.id) || inBotsFolder(folder, place.botsFolder)) return "bot";
  // A chat as its box marks it (the mark naming the folder it's in), else by its folder: a chat from
  // before the mark, or not marked yet.
  if ((typeof kuvo.chat === "string" && kuvo.chat === folder) || place.isChatsFolder(folder)) return "chat";
  return "project";
}

/** The project a project's session is filed under: as the box files its folder, else as it was
 * filed as it was made (`kuvo.filed`, while it still runs in that folder: a worktree not listed yet,
 * a subfolder), that project as the box files it, else its own folder. */
export function projectOf(session: EngineSession, filed: Readonly<Record<string, string>>): string {
  const folder = folderOf(session);
  const said = filed[folder];
  if (said !== undefined) return said;
  const mark = kuvoOf(session).filed;
  if (isObject(mark) && mark.folder === folder && typeof mark.project === "string" && mark.project && mark.project !== folder) return filed[mark.project] ?? mark.project;
  return folder;
}

/** The facts of one session. */
export function factsOf(session: EngineSession, place: ListPlace): SessionFacts {
  const kind = kindOf(session, place);
  const facts: { -readonly [K in keyof SessionFacts]: SessionFacts[K] } = {
    kind,
    directory: folderOf(session),
    created: Number(session.time?.created) || 0,
    updated: Number(session.time?.updated) || 0,
  };
  if (kind === "project") facts.project = projectOf(session, place.filed);
  if (typeof session.title === "string" && session.title) facts.title = session.title;
  if (kind === "subagent") facts.parent = session.parentID;
  if (kuvoOf(session).reviewPart) facts.reviewPart = true;
  const line = kuvoOf(session).line;
  const starter = kuvoOf(session).startedBy;
  if (Array.isArray(line) && line.length) facts.line = line.filter((id): id is string => typeof id === "string").slice(-20);
  else if (typeof starter === "string" && starter) facts.line = [starter];
  if (isPinned(session)) facts.pinned = true;
  if (isArchived(session)) facts.archived = true;
  if (place.running.has(session.id)) facts.running = true;
  if (place.childOnly?.has(session.id)) facts.childOnly = true;
  const asked = place.asked.get(session.id);
  if (asked !== undefined) facts.asked = asked;
  if (place.directAsks?.has(session.id) && asked === undefined) facts.ownAsk = true;
  if (place.directAsks && asked !== undefined && !place.directAsks.has(session.id)) facts.childAsk = true;
  if (place.away.has(session.id)) facts.away = true;
  const bot = botOf(session, kind, place);
  if (bot) facts.bot = bot;
  // (What it streams counts while it's at work: a run's end that went unheard leaves nothing behind.)
  const running = place.running.has(session.id);
  if (running && place.wakeTurns?.has(session.id)) facts.wakeTurn = true;
  if (running && place.thinking?.has(session.id)) facts.thinking = true;
  // (Read on any client up to where its last turn ended: `time.viewed`, PROTOCOL.md "What's been read".)
  const idle = Number(session.time?.idle) || 0;
  const unread = idle > (Number(session.time?.viewed) || 0) && !place.judging?.(session.id);
  if (unread) facts.unread = idle;
  // (A hard stop blocks it until it runs again or you've read the turn it ended; a retry, while it
  // goes on.)
  const stop = place.stopped?.get(session.id);
  const retry = running ? place.retrying?.get(session.id) : undefined;
  const blocked = retry ?? (unread && stop && (stop.idle === undefined || stop.idle === idle) ? stop.by : undefined);
  if (blocked) facts.blocked = blocked;
  if (retry) facts.retrying = true;
  return facts;
}

/** The bot a session is: whose conversation it is (as its bots list them), else the one it works
 * for (`kuvo.by.bot`, "Work"), a subagent's never (its conversation's counts). */
function botOf(session: EngineSession, kind: SessionKind, place: ListPlace): string | undefined {
  if (kind === "subagent") return undefined;
  const own = place.botOf?.get(session.id);
  if (own) return own;
  const by = kuvoOf(session).by;
  return isObject(by) && typeof by.bot === "string" && by.bot ? by.bot : undefined;
}

/** Whether a session waits on you, or else is at work: never both (one asking does no work until answered). */
const standing = (facts: SessionFacts): "asking" | "working" | null => (facts.asked !== undefined ? "asking" : facts.running ? "working" : null);

/** The sessions waiting on you that the lists show (a project's and a chat, not archived, this
 * box's own copy), the one that asked last first, each with what it asks. */
export function needsYou(sessions: Readonly<Record<string, SessionFacts>>, asks?: ReadonlyMap<string, AskWords>): NeedsYou[] {
  return Object.entries(sessions)
    .filter(([, facts]) => facts.asked !== undefined && (facts.kind === "project" || facts.kind === "chat") && !facts.archived && !facts.away)
    .sort(([a, one], [b, other]) => other.asked! - one.asked! || a.localeCompare(b))
    .map(([session]) => ({ session, asks: asks?.get(session) ?? { kind: "other" as const, summary: "waits on you" } }));
}

/** How many sessions wait on you, for a client's one badge of them (`kuvo/waiting`'s `sessions`):
 * those "Needs you" lists, and the bots' own conversations that wait (a permission or a question
 * there, a goal it put to you, what it asks before it does it), which the lists file under their
 * bots. Each session once, whatever it asks and however many times; not archived, this box's own
 * copy. */
export function waitingSessions(sessions: Readonly<Record<string, SessionFacts>>): number {
  return Object.values(sessions).filter(facts => facts.asked !== undefined && (facts.kind === "project" || facts.kind === "chat" || facts.kind === "bot") && !facts.archived && !facts.away).length;
}

/** What a bot's sessions come to, which its state is worked out from (`botNow`): `now` but its state.
 * `unseen`: when the newest turn you haven't read (since you last saw the bot) of those neither at
 * work, waiting on you nor blocked ended (ms). */
export type BotCounts = Omit<BotNow, "state" | "since"> & { unseen?: number };

/** What each bot is doing now, by its id: how many of its sessions (its conversation, and those
 * working for it) are at work and waiting on you, and, with neither, when the newest of them last
 * changed (`finished`); how many of those at work are thinking and how many are blocked (each left
 * out when none), what blocked the newest blocked one, and when the newest turn you haven't read
 * ended (`unseen`). `seen`: when you last saw each bot (ms, `POST kuvo/bots/<id>/seen`): a turn
 * that ended then or before isn't unseen, and a hard stop it ended on doesn't block the bot (a retry
 * under way still does). Archived sessions and copies that aren't this box's own aren't counted. */
export function botsNow(sessions: Readonly<Record<string, SessionFacts>>, seen?: ReadonlyMap<string, number>): Record<string, BotCounts> {
  const bots: Record<string, { -readonly [K in keyof BotCounts]: BotCounts[K] }> = {};
  const newestBlocked = new Map<string, number>();
  for (const facts of Object.values(sessions)) {
    if (!facts.bot || facts.archived || facts.away) continue;
    const now = (bots[facts.bot] ??= { working: 0, asking: 0 });
    const stands = standing(facts);
    if (stands) now[stands] += 1;
    else if (facts.updated) now.finished = Math.max(now.finished ?? 0, facts.updated);
    if (facts.kind === "bot" && facts.running && facts.wakeTurn) now.waking = true;
    if (stands === "working" && facts.thinking) now.thinking = (now.thinking ?? 0) + 1;
    // (A hard stop is a turn you haven't read, `unread`, which you may have seen since.)
    const seenAt = seen?.get(facts.bot) ?? -Infinity;
    const news = facts.unread !== undefined && facts.unread > seenAt;
    if (facts.blocked && (facts.retrying || news)) {
      now.blocked = (now.blocked ?? 0) + 1;
      if (facts.updated >= (newestBlocked.get(facts.bot) ?? -Infinity)) { newestBlocked.set(facts.bot, facts.updated); now.blockedBy = facts.blocked; }
    } else if (!stands && news) now.unseen = Math.max(now.unseen ?? 0, facts.unread!);
  }
  for (const now of Object.values(bots)) if (now.working || now.asking) delete now.finished;
  return bots;
}

/** What the box knows of a bot beside its sessions, for its state: whether its wake-ups are paused,
 * and its next one (seconds since 1970, `wakes` in `kuvo/bots`). */
export interface BotRest { paused: boolean; wakes?: number }

/** How far off a bot's next wake-up is for it to be asleep (ms). */
export const ASLEEP_AFTER = 12 * 3600_000;

/** The one state a bot is in (PROTOCOL.md, "What a bot is doing now"), at `at` (ms), the first that
 * holds: one of its sessions waits on you; one is blocked; every one at work is thinking; one is at
 * work; one has a turn you haven't seen (that ended since you last saw the bot); it's paused, or its
 * next wake-up is more than 12 hours away; else idle. */
export function botState(counts: BotCounts, rest: BotRest, at: number): BotState {
  if (counts.asking > 0) return "needsYou";
  if ((counts.blocked ?? 0) > 0) return "blocked";
  if (counts.working > 0) return (counts.thinking ?? 0) >= counts.working ? "thinking" : "working";
  if (counts.unseen !== undefined) return "done";
  if (rest.paused || (rest.wakes !== undefined && rest.wakes * 1000 - at > ASLEEP_AFTER)) return "asleep";
  return "idle";
}

/** A bot's `now`, at `at`: what its sessions come to, its state, and since when it's in it: for
 * `done`, when the newest turn you haven't seen ended; else as `before` (what the box last said of it)
 * where that was the same state, else `at`. */
export function botNow(counts: BotCounts, rest: BotRest, at: number, before?: { state: BotState; since: number }): BotNow {
  const state = botState(counts, rest, at);
  const since = state === "done" ? counts.unseen! : before?.state === state ? before.since : at;
  const { unseen: _, ...now } = counts;
  return { ...now, state, since };
}

/** When a bot's state may change with nothing else changing, after `at`: as its next wake-up comes
 * within 12 hours (it was asleep). None for a paused bot or one with none. */
export function botStateTurns(rest: BotRest, at: number): number | undefined {
  if (rest.paused || rest.wakes === undefined) return undefined;
  const turns = rest.wakes * 1000 - ASLEEP_AFTER;
  return turns > at ? turns : undefined;
}

/** The projects the lists show: its list's (not removed), and each one a session listed in a project
 * (not archived) is filed under. Each with its name, where its list gives one, when its newest
 * such session changed, and how many of them wait on you (`asking`) and are at work (`working`,
 * never one that's asking). */
export function listedProjects(sessions: Readonly<Record<string, SessionFacts>>, place: Pick<ListPlace, "projects">): Record<string, ListedProject> {
  const names = new Map<string, string | undefined>();
  for (const project of place.projects) if (!project.removed) names.set(project.directory, project.name);
  const newest = new Map<string, number>();
  const sums = new Map<string, { asking: number; working: number }>();
  for (const facts of Object.values(sessions)) {
    if (facts.kind !== "project" || facts.archived || facts.project === undefined) continue;
    newest.set(facts.project, Math.max(newest.get(facts.project) ?? 0, facts.updated));
    const stands = facts.away ? null : standing(facts);
    if (!stands) continue;
    const sum = sums.get(facts.project) ?? { asking: 0, working: 0 };
    sum[stands] += 1;
    sums.set(facts.project, sum);
  }
  const projects: Record<string, ListedProject> = {};
  for (const directory of [...new Set([...names.keys(), ...newest.keys()])].sort()) {
    // (Named as its list names it, a removed one's too, which a session brings back.)
    const name = names.get(directory) ?? place.projects.find(each => each.directory === directory)?.name;
    const updated = newest.get(directory);
    projects[directory] = { ...(name ? { name } : {}), ...(updated !== undefined ? { updated } : {}), ...(sums.get(directory) ?? { asking: 0, working: 0 }) };
  }
  return projects;
}

/** The sidebar's projects by newest session, with name and directory resolving ties. */
export function projectOrder(projects: Readonly<Record<string, ListedProject>>): string[] {
  const compare = (left: string, right: string) => {
    // Compare names as the sidebar does: numeric runs compare by value, other characters by case.
    const one = left.match(/[0-9]+|[^0-9]/gu) ?? [], two = right.match(/[0-9]+|[^0-9]/gu) ?? [];
    for (let at = 0; at < Math.min(one.length, two.length); at++) {
      const a = one[at]!, b = two[at]!;
      const bothNumbers = /^\d/.test(a) && /^\d/.test(b);
      const x = bothNumbers ? a.replace(/^0+/, "") : a.toLowerCase();
      const y = bothNumbers ? b.replace(/^0+/, "") : b.toLowerCase();
      const difference = (bothNumbers ? x.length - y.length : 0) || Buffer.compare(Buffer.from(x), Buffer.from(y));
      if (difference) return difference;
    }
    return one.length - two.length;
  };
  const name = (folder: string) => projects[folder]?.name ?? (path.basename(folder) || folder);
  return Object.keys(projects).sort((a, b) =>
    (projects[b]?.updated ?? 0) - (projects[a]?.updated ?? 0)
    || compare(name(a), name(b))
    || Buffer.compare(Buffer.from(a), Buffer.from(b)));
}
