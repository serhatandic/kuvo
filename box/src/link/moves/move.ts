import { mapAttachmentPaths } from "../../shared/attachment-paths";
// Move to Computer, box to box (PROTOCOL.md, `kuvo/moves` and "Peers"): the box a bot, a chat or a
// group chat lives on moves it to another of your computers, over peer mail. That box alone decides
// the switch, so the two copies are never both live: the one it goes to takes what's sent, keeps it
// held, and starts it only when told to.
//
// The box it leaves: holds it (its wake-ups paused with `moving`, its conversations frozen: prompts,
// letters and seats wait), settles it, packs it (`bundle.ts`) and offers it; sends its parts; once
// it's there, settles and reads it again (anything new: it all goes again, three tries, then it's
// taken back); then notes the switch, tells the other box to switch, and lets its own copy go.
// Before the switch, anything that goes wrong takes it back as it was; after it, only forward.

import { waitForPromptWrites } from "./admission";
import crypto from "node:crypto";
import fs from "node:fs";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import path from "node:path";
import { isObject, sortedJSON } from "../../shared/json";
import { moveHold, moveMarker } from "../../shared/move-hold";
import type { Target } from "../../shared/records";
import type { Move, MoveConfirm, MoveKind, MovePlace, MoveStays } from "../../shared/routes/moves";
import { plainName } from "../../shared/text";
import { askGroups } from "../areas/groups";
import { ownComputerName } from "../computer-name";
import { botHere, MOVING, noteBotsChanged } from "../bots/here";
import { signInRefusal } from "../bots/make";
import { settleBot } from "../bots/tell";
import { bridgeTarget, call, readBridgeFile, readEngineFile } from "../engine/client";
import { CantCarry, forgetQueue, moveInbox, packQueues, readQueues, resumeQueue, unpackQueues, watchQueueLimit } from "../engine/queue";
import { syncDurableFile, syncDurableTreeLater, writeDurablePrivate, writePrivate } from "../files";
import { HERE, IS_MAC, K } from "../home";
import { announce, refreshOwn } from "../peers/agents";
import { peerIsAway, readReach } from "../peers/reach";
import { readPeerAgents } from "../peers/directory";
import { mailFor, ownKey, peersOf } from "../peers/known";
import type { MailSender, Peer } from "../peers/known";
import type { LinkHost } from "../relay/host";
import { readLink } from "../state";
import { followedSetup } from "../setup/record";
import { removalsOf } from "../setup/removal-record";
import { checkForwardEvidence, FORWARD_EVIDENCE_FEATURE, packForwardEvidence, placeForwardEvidence,
  type ForwardMoveScope } from "../ownership/forward-evidence";
import { forgetSessionNotes, packSessionNotes, placeSessionNotes, SESSION_NOTE_FEATURE } from "../ownership/session-notes";
import { takesForwarded } from "../work-queue/forward";
import { offlineEditsReady } from "../ownership/store";
import { mayActivate } from "../ownership/resolve";
import { moveReference } from "../ownership/resolve";
import { workAsks } from "../work/asks";
import { ofBot, under, workSessions } from "../work/tree";
import { readBackground } from "../work/background";
import { decidedOf, forgetDecided, isAutopilot, keepDecided, tellAutopilot } from "../work/autopilot";
import { isChatsFolder } from "../engine/chats";
import type { EngineSession } from "../engine/sessions";
import { CHATS, engineWait, MOST_BUNDLE, NotTaken, deleteSession, directoryOf, exportAll, folderOf, groupAction, conversationHash, hashOf, idsOf, importConversation, notPlainIn, ownTree, toTrash } from "./bundle";
import type { Contents } from "./bundle";
import { moveBundleDisk, queueMoveDisk } from "../server/disk-worker";
import { noteQueuesChanged, Refusal as QueueRefusal } from "../work-queue/store";
import { attachmentMappings, carriedSessionRecords, checkedSessionPayloads, requireAttachmentReferences, installSessionPayloads, removeSessionPayloads, verifySessionPayloads, verifySourceSessionPayloads } from "./session-files";
import { AGENT_NAMES, CARRIED_AGENTS } from "../../shared/carry";
import { forgetQuiet, tellMoving } from "./elsewhere";
import { dropOwnServers } from "../bots/own-keys";
import { ASK_LEFT, KEEP_LEFT, forgetLeft, noteLeft, noteRan, readLeft, removeTrashed } from "./left";
import type { LeftEntry } from "./left";
import { MOVES, bundleOf, changeMove, compactDecisionHints, endedAs, endedAt, endedFor, endedLast, forgetMove, forgetMovedAway, liveMoves, moveOf, movesChanged, noteEnded, noteMovedAway, readMove, readMoves, saveMove, sealDecision, shownMove, waitingFor } from "./records";
import type { BundleIdentity, MoveRecord } from "./records";

/** Raw bytes in each part (40,000 characters as base64url, as documents' parts are). */
export const PART_BYTES = 30_000;
/** Parts on their way at once, and how long one goes unanswered before it's sent again. */
const IN_FLIGHT = 16;
const PART_AGAIN = 20_000;
/** A step unanswered is sent again each minute; a first offer unanswered this long: the other
 * computer isn't there. */
const STEP_AGAIN = 60_000;
// A persisted time estimates elapsed time on restart. Once observed, a step uses elapsed time
// on this process's clock, including when the wall clock stops advancing.
const retryClocks = new Map<string, { step: string; stamp: number; since: number }>();
const retryStep = (record: MoveRecord) => `${record.stage}/${record.step}/${record.generation}/${Boolean(record.activation)}/${Boolean(record.dropping)}`;
const retryStamp = (record: MoveRecord) => record.againAt ?? record.sentAt ?? 0;

function markedRetry(record: MoveRecord, ticks = performance.now()): void {
  retryClocks.set(record.id, { step: retryStep(record), stamp: retryStamp(record), since: ticks });
}

function retryDue(record: MoveRecord, now: number, ticks: number, delay = STEP_AGAIN): boolean {
  const stamp = retryStamp(record);
  if (stamp > now) return true;
  const step = retryStep(record);
  let clock = retryClocks.get(record.id);
  if (!clock || clock.step !== step || clock.stamp !== stamp) {
    clock = { step, stamp, since: ticks - (now - stamp) };
    retryClocks.set(record.id, clock);
  }
  return ticks - clock.since >= delay;
}
const NO_ANSWER = 30_000;
/** Before the switch, the other computer quiet this long about a move: it's taken back. */
const QUIET_AT_MOST = 10 * 60_000;
/** A copy there to let go of that it never says it let go of: given up on (it goes there by itself). */
const DROP_AT_MOST = 24 * 3600_000;
// When the other computer last said something of each move here (its parts and their answers come
// often).
const heard = new Map<string, number>();
/** The other computer quiet this long about a move's step: the move shows as waiting for it (longer
 * than a step goes before it's sent again, so one going well never does). */
const WAITING_AFTER = 90_000;
// Since when this box has been looking at its moves (started again, it's heard nothing yet).
let lookingSince = performance.now();
/** How long the box it leaves waits for its conversations to settle, at most. */
const SETTLE_FOR = 60;
/** A copy held here with no word from the box it came from this long goes; one being let go of
 * that still can't all go after this long is said so in the log, and tried again each hour. */
const HELD_AT_MOST = 7 * 24 * 3600_000;
const DROP_AGAIN_LATE = 3600_000;
// The copies being let go of here whose lasting failure has been logged.
const loggedStuck = new Set<string>();
/** The other computer gone from your computers this long (not just left out as a list's read again):
 * a move with it ends. */
const GONE_FOR = 5 * 60_000;
// Since when each move's other computer has been missing from your computers.
const missing = new Map<string, number>();
/** Being got ready to go this long (its engine or group chats stuck): taken back. */
const HOLDING_AT_MOST = 30 * 60_000;
/** A copy being set up here this long (its engine stuck): given up on, and taken back. */
const SET_UP_AT_MOST = 30 * 60_000;
// What gives up on each copy being set up here; and the copies set up here now or next, in their turn.
const settingUp = new Map<string, { stop: AbortController; since: number }>();
const toSetUp = new Map<string, BundleIdentity>();

/** What a move says, in words to show. */
export const say = {
  notConnected: (place: string) => `${place} isn’t connected. Connect to it, then try again.`,
  working: (name: string) => `${name} is working. Move it once it’s done.`,
  cleanupPending: "The original is retired. Kuvo will try again to clean up its files on the source computer.",
  inProject: (name: string) => `${name} works in one of your projects, which stays where it is. Only a bot with a folder of its own can move.`,
  noAgents: (place: string) => `The coding agents on ${place} aren’t running. Try again once they are.`,
  cantCarry: (name: string) => `${name}’s conversation couldn’t be prepared for the move. Try again in a moment.`,
  agentNotThere: (name: string, agent: string, place: string) => `${name} talks with ${agent}, which isn’t installed on ${place}. Install it in Settings › Accounts there, then move ${name}.`,
  agentSignedOut: (name: string, agent: string, place: string) => `${name} talks with ${agent}, which isn’t signed in on ${place}. Sign in to it in Settings › Accounts there, then move ${name}.`,
  otherAccount: (name: string, agent: string, who: string, place: string) => `On ${place}, ${name} uses the ${agent} account signed in there${who}.`,
  modelNotThere: (name: string, place: string) => `${name}’s model needs a sign-in on ${place}. Sign in to it in Settings › Accounts there, or choose another model, then move ${name}.`,
  notSame: (name: string) => `${name}’s conversation didn’t come out the same on the other computer, so it stayed here. Try again.`,
  background: (name: string, what: string) => `${name} has ${what} running. Wait for it to finish, or stop it, then move ${name}.`,
  waiting: (name: string) => `${name} still has messages on their way. Try again in a moment.`,
  notPlain: (name: string, what: { path: string; link: boolean }) => `${name}’s folder has ${what.link ? "a link that wouldn’t work on another computer" : "something that isn’t a file"} (${what.path}), which can’t move to another computer. Remove it, then move ${name}.`,
  busy: (name: string) => `${name} kept getting messages while it moved, so it stayed where it is. Try again in a moment.`,
  botsElsewhere: (chat: string) => `${chat}’s bots live on different computers, so it can’t go there.`,
  inBoxGroup: (name: string, chat: string) => `${name} is in “${chat}”, which lives on ${HERE} with its bots. Move the chat to a Mac first, or take ${name} out of it.`,
  tooBig: (name: string) => `${name} is more than 1 GB, too big to move to another computer, so it stays where it is.`,
  noGroups: (place: string) => `Group chats on ${place} aren’t running right now. Try again once they are.`,
  noEngine: (place: string) => `The engine on ${place} isn’t running. Try again once it is.`,
  needsUpdate: (place: string) => `${place} needs a newer Kuvo Code before things can move to or from it.`,
  botsNotThere: (chat: string, place: string) => `${chat}’s bots don’t all live on ${place}. Move them there first, then the chat can follow.`,
  slow: (name: string) => `${name} took too long to get ready to move on ${HERE}, so it stayed here. Try again in a moment.`,
  oldCopy: (name: string, place: string) => `An earlier copy of ${name} on ${place} couldn’t be let go of yet. Try again in a moment.`,
  moving: MOVING,
};

/** The same, short, for a menu that shows it beside a computer's name. */
const note = {
  inProject: "Works in a project",
  noAgents: "Its coding agents aren’t running",
  background: "Running something",
  notPlain: { link: "Has a link that can’t move", other: "Has something that isn’t a file" },
  inBoxGroup: "In a group chat that lives here",
  needsUpdate: "Needs a newer Kuvo Code",
  botsNotThere: "Its bots don’t all live there",
  moving: "Moving already",
};

/** A move's outcome, for whoever asked for it. */
export type MoveOutcome = { ok: true; move: Move; note?: string; verifiedPayloads?: true } | { ok: false; status: number; message: string };

// Those waiting on a move here, by its ID. How it ended is also noted beside its record for a while
// (`<id>.outcome.json`), for a client that asked through another process (a Mac's `box route`),
// which the running bridge moves it for.
const waiting = new Map<string, ((outcome: MoveOutcome) => void)[]>();
const outcomeFormat = additiveObjectFormat("Move outcome");
const outcomeFile = (id: string) => path.join(MOVES, `${id}.outcome.json`);
export function readOutcome(id: string): MoveOutcome | null {
  if (!/^mov_[A-Za-z0-9]+$/.test(id)) return null;
  try {
    return outcomeFormat.read(JSON.parse(fs.readFileSync(outcomeFile(id), "utf8"))) as unknown as MoveOutcome;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return null;
  }
}

const OUTCOME_KEPT = 90 * 24 * 3600_000;
let lastDecisionSweep = -Infinity;
function ended(id: string, outcome: MoveOutcome): void {
  readOutcome(id);
  writeDurablePrivate(outcomeFile(id), JSON.stringify(outcomeFormat.write(outcome)), path.dirname(MOVES));
  for (const resolve of waiting.get(id) ?? []) resolve(outcome);
  waiting.delete(id);
}
/** Until move `id` ends, here or in the running bridge. */
export function outcomeOf(id: string): Promise<MoveOutcome> {
  return new Promise(resolve => {
    let timer: ReturnType<typeof setInterval> | null = null;
    const done = (outcome: MoveOutcome) => {
      if (timer) clearInterval(timer);
      timer = null;
      resolve(outcome);
    };
    waiting.set(id, [...(waiting.get(id) ?? []), done]);
    if (watching) return;
    // Another process moves it: how it ended, once it's noted.
    timer = setInterval(() => {
      try {
        const outcome = readOutcome(id);
        if (outcome) done(outcome);
      } catch (error) {
        done({ ok: false, status: 409, message: (error as Error).message });
      }
    }, 500);
  });
}

// Each move's steps one at a time, in the order they come (its mail, and what `keepMoving` does for
// it): one never runs while another of the same move is half done.
const queues = new Map<string, Promise<unknown>>();
function inTurn<T>(id: string, step: () => Promise<T>): Promise<T> {
  const before = queues.get(id) ?? Promise.resolve();
  const next = before.catch(() => {}).then(step);
  const kept = next.catch(() => {});
  queues.set(id, kept);
  kept.then(() => { if (queues.get(id) === kept) queues.delete(id); });
  return next;
}

// The relay connection that's up, for mail to the other box, in the process that moves things (the
// running bridge: `watchMoves`); the moves it's carrying on now.
let hostOf: () => Pick<LinkHost, "send"> | null | undefined = () => null;
let watching = false;
const driving = new Map<string, number>();
function drive(id: string): void {
  if (driving.has(id)) return;
  driving.set(id, performance.now());
  inTurn(id, () => carryOn(id).catch(async error => {
    await refuseMove(id, error);
    // Taken back already as a step went on (too long held): what that step left in its folder goes.
    if (!readMove(id)) try { fs.rmSync(bundleOf(id), { recursive: true, force: true }); } catch {}
  })).catch(error => console.error("kuvo-link: could not record a move’s failure", error)).finally(() => driving.delete(id));
}

const nameOf = (key: string): string => plainName(peersOf(readLink()).find(peer => peer.key === key)?.name ?? "") || "the other computer";

// Where a move's mail goes instead of the relay: a test's scripted other computer.
let sendInstead: ((peer: string, move: Record<string, unknown>) => boolean) | null = null;
export function sendMovesWith(send: typeof sendInstead): void {
  sendInstead = send;
}

function tell(peer: string, move: Record<string, unknown>): boolean {
  if (sendInstead) return sendInstead(peer, move);
  const link = readLink();
  const removed = removalsOf(link)[peer]?.peer;
  const to = removed ?? peersOf(link).find(each => each.key === peer);
  const record = link?.removalSource && typeof move.id === "string" ? readMove(move.id) : null;
  const operation = record?.removalOperation;
  const bytes = link && to && (removed || operation
    ? mailFor(to, { scopedMove: operation ? { ...move, kind: record?.kind, thing: record?.thing } : move, operation }, link, true)
    : mailFor(to, { move, at: Date.now() }, link));
  if (!bytes) return false;
  // Sent only when the connection took it: one closing, or not welcomed yet, takes nothing.
  try {
    return hostOf()?.send(bytes) === true;
  } catch {
    return false;
  }
}

// ---- What it is, and where it can go ----

/** What's moving, as this box has it: its name and its conversations' root, or why it can't move. */
interface Movable {
  unresolved?: true;
  name: string;
  root?: string;
  why?: string;
  note?: string;
  /** A group chat's bots, by their IDs. */
  members?: string[];
  /** It talks with a coding agent. */
  lane?: boolean;
  /** The model it talks with, for the computer it goes to to check it can. */
  model?: { providerID: string; id: string };
}


/** The agents bridge's part of a move (`carry.ts`, its `kuvo/carry` routes): what it answers, or why
 * not, in its own words. */
async function carry<T>(bridge: Target, step: string, body: Record<string, unknown>, signal?: AbortSignal): Promise<T> {
  try {
    return await call(bridge, "POST", `/api/kuvo/carry/${step}`, body, engineWait(signal, CARRY_WAIT)) as T;
  } catch (error) {
    if (error instanceof Error && "carry" in error && error.carry === true && "status" in error && typeof error.status === "number") {
      throw new Refused(error.message, error.status);
    }
    throw error;
  }
}
/** How long the bridge has to pack, hash or place a conversation (its whole files, up to 1 GB). */
const CARRY_WAIT = 10 * 60_000;

/** What the bridge says as it packs a conversation. */
interface Packed {
  sessions: { id: string; metadata?: Record<string, unknown> }[];
  digest: string;
  left: unknown[];
}



async function movable(kind: MoveKind, thing: string, signal?: AbortSignal, strict = false): Promise<Movable | null> {
  if (kind === "bot") {
    const here = botHere(thing);
    if (!here) return null;
    const name = here.described.name;
    if (here.described.project) return { name, why: say.inProject(name), note: note.inProject };
    if (here.agents && !bridgeTarget()) return { name, why: say.noAgents(HERE), note: note.noAgents };
    const running = runningOf([here.session]);
    if (running) return { name, why: say.background(name, running), note: note.background };
    // Found now, not by the computer it goes to once it's all there. (A folder too big to look
    // through here is looked through as it's packed.)
    const odd = notPlainIn(here.folder, 50_000);
    if (odd) return { name, why: say.notPlain(name, odd), note: odd.link ? note.notPlain.link : note.notPlain.other };
    // One in a group chat that lives here with its bots, on a machine, stays with it.
    if (!IS_MAC) {
      let bridge: Target | null = null;
      try { bridge = readBridgeFile(); } catch {}
      const listed = bridge ? await askGroups<{ data?: { group?: { id?: string; name?: string; members?: unknown } }[] }>(bridge, "GET", "", undefined, engineWait(signal)).catch(() => null) : null;
      const chat = (listed?.status === 200 && Array.isArray(listed.value?.data) ? listed.value.data : []).find(each => Array.isArray(each?.group?.members) && (each.group.members as unknown[]).includes(thing));
      if (chat) return { name, root: here.session, why: say.inBoxGroup(name, plainName(chat.group?.name ?? "") || "a group chat"), note: note.inBoxGroup };
    }
    return { name, root: here.session, ...(here.agents ? { lane: true } : {}), ...(modelOf(here.described.model) ?? {}) };
  }
  if (kind === "chat") {
    // On the engine, or, talking with a coding agent, on the agents bridge.
    const look = (target: () => Target) => Promise.resolve().then(() => call(target(), "GET", `/api/session/${encodeURIComponent(thing)}`, undefined, engineWait(signal))).catch(() => null);
    let session = await look(readEngineFile);
    const lane = !isObject(session);
    if (lane) session = await look(readBridgeFile);
    if (!isObject(session)) return null;
    const title = typeof session.title === "string" && session.title.trim() ? `“${session.title.trim()}”` : "This chat";
    const directory = isObject(session.location) ? session.location.directory : session.directory;
    if (!isChatsFolder(directory)) return null;
    const running = runningOf([thing]);
    if (running) return { name: title, why: say.background(title, running), note: note.background };
    return { name: title, root: thing, ...(lane ? { lane: true } : {}), ...(modelOf(session.model) ?? {}) };
  }
  let bridge: Target;
  try { bridge = readBridgeFile(); } catch { return null; }
  const listed = await askGroups<{ data?: { group?: { name?: string; members?: unknown } } }>(bridge, "GET", `/${encodeURIComponent(thing)}`, undefined, engineWait(signal)).catch(() => null);
  if (listed?.status === 503) return { name: "This group chat", unresolved: true, why: "This computer couldn’t read the group’s conversation. Restart Kuvo and try again." };
  if (listed?.status !== 200 || (strict && !Array.isArray(listed.value?.data?.group?.members))) return null;
  const members = listed.value?.data?.group?.members;
  return {
    name: `“${plainName(listed.value?.data?.group?.name ?? "") || "Group chat"}”`,
    members: Array.isArray(members) ? members.filter((member): member is string => typeof member === "string") : [],
  };
}

/** What still runs for any of `sessions` besides a turn (a command left running, a subagent sent
 * off), in words; null when nothing does. It would end as it moves, so it's waited for. */
function runningOf(sessions: readonly string[]): string | null {
  const running = readBackground();
  const all = sessions.map(session => running[session]).filter(each => each !== undefined);
  const commands = all.reduce((sum, each) => sum + each.commands, 0), helpers = all.reduce((sum, each) => sum + each.subagents + each.tasks, 0);
  if (!commands && !helpers) return null;
  return [commands ? (commands === 1 ? "a command" : `${commands} commands`) : "", helpers ? (helpers === 1 ? "a subagent" : `${helpers} subagents`) : ""].filter(Boolean).join(" and ");
}

const modelOf = (model: unknown): { model: { providerID: string; id: string } } | null =>
  isObject(model) && typeof model.providerID === "string" && typeof model.id === "string" ? { model: { providerID: model.providerID, id: model.id } } : null;

/** Whether a group chat's bots all live on the machine `peer`, as the computers say: none here, and
 * each in that machine's list of its agents. One that hasn't said what it has isn't judged (it
 * checks itself as the chat comes). */
function botsLiveOn(members: string[], peer: Peer): boolean {
  if (members.some(member => botHere(member))) return false;
  const list = readPeerAgents()[peer.key]?.list;
  if (!Array.isArray(list)) return true;
  return members.every(member => list.some(agent => agent.kind === "bot" && agent.id === member));
}

/** Where `thing` can go: each of your other computers, and why not one where it can't. */
export async function placesFor(kind: MoveKind, thing: string, signal?: AbortSignal, details = true, scopedDestination?: string): Promise<{ places: MovePlace[]; stays?: MoveStays; restricted?: true } | null> {
  const what = await movable(kind, thing, signal, !details);
  if (!what || (!details && what.unresolved)) return null;
  const stays = !details || kind === "group" ? undefined : await staysOf(kind, thing, what, signal);
  const target = what.lane ? bridgeTarget() : engineTarget();
  const busy = details && !what.why && what.root && target ? await activeIn(target, [what.root], signal) : false;
  return { places: placesOf(kind, thing, what, stays, busy, scopedDestination), ...(!details && what.why ? { restricted: true } : {}), ...(stays && Object.keys(stays).length ? { stays } : {}) };
}

/** What of a bot or chat stays here as it moves: none of it goes, and none of it's lost (it's here
 * again if it comes back). Sessions it can't list now aren't counted. */
async function staysOf(kind: MoveKind, thing: string, what: Movable, signal?: AbortSignal): Promise<MoveStays> {
  const stays: MoveStays = {};
  const targets = [readEngineFile, bridgeTarget].flatMap(each => {
    try { return [each()].filter((target): target is Target => Boolean(target)); } catch { return []; }
  });
  const all = await workSessions(targets, signal).catch(() => []);
  const started = new Set([...(what.root ? under(what.root, all) : []), ...(kind === "bot" ? ofBot(thing, all) : [])].map(each => each.data.id));
  if (started.size) stays.sessions = started.size;
  const own = (session: string | undefined, bot: string | undefined) => (kind === "bot" ? bot === thing : session === what.root);
  const asks = workAsks().filter(ask => own(ask.session, ask.bot)).length;
  if (asks) stays.asks = asks;
  // Its coding agent's own conversation: kept here as the way back where it goes too; one that can't
  // go stays, and the agent starts a new one there, handed what was said so far.
  const agent = what.model?.providerID.split("@")[0] ?? "";
  if (what.lane && AGENT_NAMES[agent] && !CARRIED_AGENTS.has(agent)) stays.startsOver = AGENT_NAMES[agent];
  else if (what.lane) stays.agent = true;
  return stays;
}

function question(kind: MoveKind, name: string, peer: Peer, place: string, source: string, stays: MoveStays | undefined, busy: boolean): MoveConfirm {
  const words = [kind === "bot" ? "Its memory, files, wake-ups and conversation move with it."
    : kind === "group" ? "Its messages and meeting move with it. Its bots stay where they are."
    : "Its conversation and wake-ups move with it."];
  if (kind === "group" && peer.kind === "mac") words.push("The group runs while that Mac is on.");
  if (busy) words.push("A turn is running. The move waits up to a minute for it to finish.");
  words.push(kind === "group" ? "The meeting pauses during the move." : "New messages and wake-ups wait during the move.");
  if (stays?.startsOver) words.push(`${stays.startsOver} starts a new conversation there with what was said so far.`);
  if (stays?.agent) words.push(`A copy of the coding agent’s conversation stays on ${source} until a week after it first runs on ${place}.`);
  if (stays?.sessions) words.push(`Sessions it started stay on ${source}. It can still reach them from ${place}.`);
  if (stays?.asks) words.push(`Its unanswered questions stay on ${source}.`);
  if (kind !== "group") words.push(`Vault items, integrations and browser sign-ins stay on ${source}.`);
  const trash = IS_MAC ? "the Trash" : "Kuvo Code’s trash folder";
  words.push(kind === "group" ? "The group opens on the new computer after the move." : `The original goes to ${trash} on ${source}.`);
  const title = kind === "bot" ? `“${name}”` : name;
  return { title: `Move ${title} to ${place}?`, message: words.join(" "), label: "Move" };
}

function placesOf(kind: MoveKind, thing: string, what: Movable, stays: MoveStays | undefined, busy: boolean, scopedDestination?: string): MovePlace[] {
  const moving = moveOf(thing) !== null;
  const link = readLink();
  const own = ownKey(link ?? {});
  const accepted = followedSetup(link)?.checkpointed;
  const told = readPeerAgents(), reach = readReach();
  const source = ownComputerName();
  return peersOf(link).filter(peer => peer.key !== own && (!scopedDestination || peer.key === scopedDestination)).map(peer => {
    const name = plainName(peer.name) || (peer.kind === "mac" ? "Mac" : "Machine");
    // Why not, and the same short: the thing's own reason first, then that computer's.
    const not: [string, string] | null = moving ? [say.moving, note.moving]
      : what.why ? [what.why, what.note ?? what.why]
      // A scoped destination confirmed the accepted plan. Its move checks use live local data.
      : scopedDestination ? null
      : olderThanMoves(peer.key) || (accepted && !told[peer.key]?.features?.includes("moves.two-computers")) || (what.lane && olderThanCarrying(peer.key)) ? [say.needsUpdate(name), note.needsUpdate]
      : peerIsAway(peer.key, told[peer.key]?.heard, reach) ? [say.notConnected(name), "Can’t be reached"]
      // A group chat goes to a machine only where all its bots live (to a Mac, they stay put).
      : kind === "group" && peer.kind === "machine" && !botsLiveOn(what.members ?? [], peer) ? [say.botsNotThere(what.name, name), note.botsNotThere]
      : null;
    return { key: peer.key, name, kind: peer.kind, ok: !not, ...(not ? { why: not[0], note: not[1] } : { confirm: question(kind, what.name, peer, name, source, stays, busy) }) };
  });
}

/** Whether another computer says what it answers, and moves isn't among it (a box from before them). */
function olderThanMoves(key: string): boolean {
  const features = readPeerAgents()[key]?.features;
  return Array.isArray(features) && !features.includes("moves");
}

/** Whether another computer checks it can run what comes before it's held here (`check`). */
const checksFirst = (key: string): boolean => readPeerAgents()[key]?.features?.includes("moves.check") === true;

/** The same, for one that talks with a coding agent. */
function olderThanCarrying(key: string): boolean {
  const features = readPeerAgents()[key]?.features;
  return Array.isArray(features) && !features.includes("moves.agents");
}

// ---- The box it leaves ----

/** Moves `thing` to the box whose key is `to`, as a client asks: until it's there, or why not. */
export async function startMove(kind: MoveKind, thing: string, to: string, signal?: AbortSignal): Promise<MoveOutcome> {
  const begun = await beginMove(kind, thing, to, signal, true);
  return begun.ok ? begun.outcome! : begun;
}

export async function beginMove(kind: MoveKind, thing: string, to: string, signal?: AbortSignal, wait = false, change?: string, removalOperation?: string): Promise<
  { ok: true; move: Move; outcome?: Promise<MoveOutcome> } | Extract<MoveOutcome, { ok: false }>
> {
  const link = readLink();
  const own = link ? ownKey(link) : null;
  const peer = peersOf(link).find(each => each.key === to);
  if (link?.removalSource && (!removalOperation || link.removalSource.operation !== removalOperation || link.removalSource.moveTo !== to))
    return { ok: false, status: 409, message: "Only a move in the accepted removal plan can start." };
  if (!own || !peer || to === own) return { ok: false, status: 404, message: "That computer isn’t one of yours." };
  if (moveOf(thing)) return { ok: false, status: 409, message: say.moving };
  if (!mayActivate(moveReference(kind, thing), { link })) return { ok: false, status: 409, message: "This record is held while its owner changes." };
  if (!removalOperation && (olderThanMoves(to) || (followedSetup(link)?.checkpointed && !readPeerAgents()[to]?.features?.includes("moves.two-computers")))) return { ok: false, status: 409, message: say.needsUpdate(plainName(peer.name) || "That computer") };
  const what = await movable(kind, thing, signal);
  if (!what) return { ok: false, status: 404, message: "It isn’t on this computer any more." };
  if (what.why) return { ok: false, status: 409, message: what.why };
  if (!removalOperation && what.lane && olderThanCarrying(to)) return { ok: false, status: 409, message: say.needsUpdate(plainName(peer.name) || "That computer") };
  const id = change ?? `mov_${crypto.randomBytes(12).toString("base64url").replace(/[-_]/g, "")}`;
  const folder = folderOf(kind, thing);
  const record: MoveRecord = {
    ...(change ? { wipePreparation: true as const } : {}),
    ...(removalOperation ? { removalOperation } : {}),
    id, kind, thing, name: what.name, twoComputers: true, from: own, to, role: "from", stage: "holding", sessions: what.root ? [what.root] : [],
    peer: to, wasPaused: false, tries: 0, started: Date.now(), ...(folder && fs.existsSync(folder) ? { folder } : {}), ...(what.lane ? { lane: true } : {}),
    ...(what.model ? { model: what.model } : {}), ...((removalOperation || checksFirst(to)) && kind !== "group" ? { step: "check" as const } : {}),
  };
  // One move of a thing at a time, whoever asks (a phone and a Mac at once, through this bridge or
  // `box route`): noted under a lock, after a look again.
  if (!noteFirst(record)) return { ok: false, status: 409, message: say.moving };
  const outcome = wait ? outcomeOf(id) : undefined;
  // Moved by this process when it's the bridge; else by the bridge, as it next looks.
  if (watching) drive(id);
  return { ok: true, move: shownMove(record), ...(outcome ? { outcome } : {}) };
}

/** `record` noted, unless a move of its thing is under way already: under a lock of its own (a file
 * made only if it isn't there; one left by a process cut off goes after a minute). */
function noteFirst(record: MoveRecord): boolean {
  fs.mkdirSync(MOVES, { recursive: true, mode: 0o700 });
  const lock = path.join(MOVES, `.${record.thing}.lock`);
  try { if (Date.now() - fs.statSync(lock).mtimeMs > 60_000) fs.rmSync(lock, { force: true }); } catch {}
  let handle: number;
  try { handle = fs.openSync(lock, "wx", 0o600); } catch { return false; }
  try {
    if (moveOf(record.thing) || readMove(record.id) || readOutcome(record.id)) return false;
    saveMove(record);
    return true;
  } finally {
    fs.closeSync(handle);
    fs.rmSync(lock, { force: true });
  }
}

const PAUSED = (folder: string) => path.join(folder, "paused");
const pausedText = (folder: string): string | null => {
  try { return fs.readFileSync(PAUSED(folder), "utf8"); } catch (error) {
    if (["ENOENT", "ENOTDIR"].includes((error as NodeJS.ErrnoException).code ?? "")) return null;
    throw error;
  }
};

/** Journal the previous pause before installing the hold this move owns. */
export function hold(record: MoveRecord): void {
  const folder = record.folder;
  const was = folder ? pausedText(folder) : null;
  const owner = moveHold(was);
  if (owner && owner !== record.id && (owner !== "moving" || record.holdSince === undefined)) throw new Refused(say.moving);
  if (!record.pauseRecorded && !owner) {
    record.wasPaused = was !== null;
    record.pausedAs = was ?? undefined;
  }
  record.pauseRecorded = true;
  record.holdSince = Date.now();
  saveMove(record);
  if (folder && fs.existsSync(folder)) writeDurablePrivate(PAUSED(folder), moveMarker(record.id));
}

/** Its wake-ups as they were before it moved: going again, or paused as you paused them. */
function unhold(record: MoveRecord, folder = record.folder): void {
  if (!folder) return;
  const paused = pausedText(folder);
  const owner = moveHold(paused);
  if (!owner) {
    // A prior attempt may have restored the pause and failed while sealing the directory entry.
    if (record.placed || fs.existsSync(folder)) {
      if (paused === null) syncDurableFile(folder, folder);
      else syncDurableFile(PAUSED(folder));
    }
    return;
  }
  // A legacy marker has no owner. An older failure may never release it across a newer move.
  if (owner !== record.id && !(owner === "moving" && !record.pauseRecorded && !readMoves(true).some(other =>
    other.id !== record.id && other.thing === record.thing && !other.failed
  ))) return;
  if (record.wasPaused) writeDurablePrivate(PAUSED(folder), record.pausedAs ?? "");
  else {
    fs.rmSync(PAUSED(folder), { force: true });
    syncDurableFile(folder, folder);
  }
}

function engineTarget(): Target | null {
  try { return readEngineFile(); } catch { return null; }
}

async function activeIn(target: Target, sessions: readonly string[], signal?: AbortSignal): Promise<boolean> {
  const active = await call(target, "GET", "/api/session/active", undefined, engineWait(signal)).catch(() => ({}));
  return isObject(active) && sessions.some(session => session in active);
}

/** Until none of its conversations is at work and nothing waits to go to them: a minute at most.
 * What the engine holds for one you stopped waits for Resume, and goes with it (`packQueues`). */
async function settle(target: Target, sessions: readonly string[], name: string): Promise<void> {
  // (Still in a turn at the end, it's working; else only messages wait to go to it.)
  let busy = false;
  for (let tries = 0; tries < SETTLE_FOR; tries++) {
    const active = await call(target, "GET", "/api/session/active", undefined, engineWait());
    if (Array.isArray(active)) busy = sessions.some(session => active.includes(session));
    else if (isObject(active)) busy = sessions.some(session => session in active);
    else throw new Error("Active conversations could not be read for a move");
    const queues = readQueues(true);
    let waiting = false;
    for (const session of sessions) {
      const inbox = await moveInbox(target, session);
      if (!queues[session]?.paused && inbox.length) waiting = true;
    }
    if (!busy && !waiting) return;
    await new Promise(resolve => setTimeout(resolve, 1000));
  }
  throw new Refused(busy ? say.working(name) : say.waiting(name));
}

class Refused extends Error {
  constructor(message: string, readonly status = 409) {
    super(message);
  }
}

function refuseMove(id: string, error: unknown): Promise<void> {
  if (error instanceof Refused) return takeBack(id, error.message, error.status);
  if (error instanceof NotTaken) return takeBack(id, error.message);
  if (error instanceof QueueRefusal) return takeBack(id, error.message);
  console.error("kuvo-link: a move stopped unexpectedly", error);
  return takeBack(id, "The source computer couldn’t prepare the move. Your work stayed there. Try again in a moment.");
}

/** What a move carries of its conversations, to see whether anything changed as it went: their
 * messages, word for word (`conversationHash`), or, with a coding agent, those and each agent's own
 * files (the bridge's `digest`), and their queues (what waits for them), but when each was read as
 * waiting. Forwarded decisions must also travel with their latest durable evidence. */
const fingerprint = (conversation: string, queues: Record<string, unknown>, notes?: Contents["sessionNotes"],
  evidence?: Contents["forwardEvidence"]): string =>
  `${conversation}.${crypto.createHash("sha256").update(JSON.stringify(queues, (key, value) => key === "at" ? undefined : value))
    .update(JSON.stringify(notes ?? null)).update(sortedJSON(evidence ?? null)).digest("base64url")}`;

/** What waits for its conversations, as a move carries it (`packQueues`); one the engine held that
 * can't go with it, and it doesn't go yet. */
const queuesOf = (engine: Target, sessions: readonly string[], name: string) => packQueues(engine, sessions).catch(error => {
  if (error instanceof CantCarry) throw new Refused(error.unreadable
    ? `A message waiting for ${name} couldn’t be read. The move stopped. Check the queue before trying again.`
    : say.waiting(name));
  throw error;
});

// What the box it leaves has of each move in memory: the parts the other box has, and those on
// their way (by when they went).
// (`window`: how many may be on their way now: halved as one's lost, one more as each comes, up to
// `IN_FLIGHT`; a slow connection isn't sent everything again and again.)
const outgoing = new Map<string, { got: Set<number>; flying: Map<number, number>; since: number; exports: string; digest?: string; evidenceScope: ForwardMoveScope; payloads: NonNullable<MoveRecord["payloads"]>; window?: number }>();

/** The box it leaves, carrying on from where it is (as it starts, or step by step). */
async function carryOn(id: string): Promise<void> {
  let record = readMove(id);
  if (!record || record.role !== "from" || record.stage !== "holding" || record.failed) return;
  // The computer it goes to says first whether it can run it there: nothing's held here till then.
  if (record.step === "check") {
    const there = await checkThere(record);
    const now = changeMove(id, each => { if (!each.failed && each.stage === "holding") { each.step = undefined; if (there.note) each.note = there.note; } });
    if (!now || now.failed || now.stage !== "holding") return;
    record = now;
  }
  hold(record);
  // Taken back meanwhile (as the box started again, or as getting it ready took too long): it stays
  // taken back, nothing more done or kept of it (a group chat held as it was is looked at again, and
  // goes on).
  const takenBack = (): boolean => {
    const now = readMove(id);
    if (now && !now.failed && now.stage === "holding") return false;
    if (record.kind === "group") groupsToWake = true;
    try { fs.rmSync(bundleOf(id), { recursive: true, force: true }); } catch {}
    return true;
  };
  let exports: unknown[] = [];
  let digest = "";
  let queues: Record<string, unknown> = {};
  const contents: Contents = { kind: record.kind, thing: record.thing, name: record.name, exports, wasPaused: record.wasPaused };
  if (record.kind === "group") {
    contents.group = await groupAction(readBridgeFile(), record.thing, "hold").catch(error => {
      if ((error as { status?: number })?.status === 503) throw new Refused(`${record.name}’s conversation couldn’t be read. Restart Kuvo on this computer and try again.`);
      throw error;
    }) as Contents["group"];
    // Its Work Queue, which its seats keep.
    const packed = await queueMoveDisk.run("packQueuesForMove", [record.thing]);
    if (packed.refused !== undefined) throw new QueueRefusal(packed.refused);
    if (packed.queues) contents.workQueue = packed.queues;
    const members = isObject(contents.group?.group) && Array.isArray(contents.group.group.members) ? contents.group.group.members : [];
    record.members = members.filter((member): member is string => typeof member === "string");
  } else {
    const target = record.lane ? readBridgeFile() : readEngineFile();
    const known = new Set(record.sessions);
    await waitForPromptWrites(record.sessions);
    await settle(target, record.sessions, record.name);
    const running = runningOf(record.sessions);
    if (running) throw new Refused(say.background(record.name, running));
    // Its folder goes as it is, and the computer it goes to takes plain files only: found here first.
    const odd = record.folder && fs.existsSync(record.folder) ? notPlainIn(record.folder) : null;
    if (odd) throw new Refused(say.notPlain(record.name, odd));
    if (record.lane) {
      const packed = await carry<Packed>(target, "pack", { root: record.sessions[0], into: path.join(bundleOf(id), "carry"), most: MOST_BUNDLE }).catch(error => {
        const status = (error as { status?: number } | null)?.status;
        // (Its turn started again as it was packed: the bridge's 423.)
        if (status === 423) throw new Refused(say.working(record.name));
        if (status === 413) throw new Refused(say.tooBig(record.name));
        if (error instanceof Refused) throw error;
        console.error("kuvo-link: a conversation couldn’t be packed for a move:", error);
        throw new Refused(say.cantCarry(record.name));
      });
      digest = packed.digest;
      record.sessions = packed.sessions.map(each => each.id);
      record.leftOf = packed.left;
      contents.lane = { sessions: [...record.sessions], root: record.sessions[0] };
    } else {
      exports = await exportAll(target, record.sessions[0]);
      contents.exports = exports;
      record.sessions = idsOf(exports);
      digest = conversationHash(exports);
    }
    if (record.sessions.some(session => !known.has(session))) {
      if (takenBack()) return;
      // Freeze the discovered conversations before waiting for their inboxes to settle.
      changeMove(id, each => { each.sessions = record.sessions; });
      await waitForPromptWrites(record.sessions);
      await settle(target, record.sessions, record.name);
      const background = runningOf(record.sessions);
      if (background) throw new Refused(say.background(record.name, background));
    }
    // (What its conversation says of where it worked: a bot's folder; a chat's, this box's chats.)
    const worked = record.kind === "bot" ? record.folder : CHATS;
    contents.was = { place: plainName(readLink()?.name ?? "") || (IS_MAC ? "a Mac" : "a machine"), ...(worked ? { folder: worked } : {}) };
    // A conversation you stopped goes stopped, with what waits for it, here and in the engine.
    queues = await queuesOf(target, record.sessions, record.name);
    if (Object.keys(queues).length) contents.queues = queues;
    const decided = decidedOf(record.sessions);
    if (Object.keys(decided).length) contents.decided = decided;
    // Its conversations' Work Queues.
    contents.sessionNotes = await packSessionNotes(record.sessions);
    if (contents.sessionNotes && !takesForwarded(record.peer, SESSION_NOTE_FEATURE))
      throw new Refused("Update the destination computer before moving these ordinary session notes.");
    const packed = await queueMoveDisk.run("packQueuesForMove", record.sessions);
    if (packed.refused !== undefined) throw new QueueRefusal(packed.refused);
    if (packed.queues) contents.workQueue = packed.queues;
    if (record.folder) contents.folder = path.basename(record.folder);
    if (record.kind === "bot") {
    }
  }
  const evidenceScope = forwardMoveScope(record, record.sessions, contents.workQueue);
  contents.forwardEvidence = packForwardEvidence(followedSetup(readLink())?.id, evidenceScope);
  if (contents.forwardEvidence && !takesForwarded(record.peer, FORWARD_EVIDENCE_FEATURE))
    throw new Refused("Update the destination computer before moving these forwarded decisions.");
  if (takenBack()) return;
  const prepared = await moveBundleDisk.run("packForTransfer", id, contents).catch(error => {
    throw new Refused(error instanceof Error ? error.message : "Managed files could not be verified.");
  });
  if (!prepared.bundle) throw new Refused(say.tooBig(record.name));
  const bundle = prepared.bundle;
  if (bundle.size > MOST_BUNDLE) throw new Refused(say.tooBig(record.name));
  if (takenBack()) return;
  const payloads = { sessions: bundle.payloads, queue: bundle.queueFiles };
  const verifyPayloads = record.wipePreparation || payloads.sessions.length > 0 || payloads.queue.length > 0;
  outgoing.set(id, { got: new Set(), flying: new Map(), since: performance.now(),
    exports: fingerprint(digest, queues, contents.sessionNotes, contents.forwardEvidence), digest, evidenceScope, payloads });
  const now = changeMove(id, each => {
    Object.assign(each, { leftOf: record.leftOf, sessions: record.sessions, members: record.members, generation: record.tries + 1, verifyPayloads: verifyPayloads ? true : undefined, typedAttachmentPaths: payloads.sessions.length ? true : undefined, hash: bundle.hash, size: bundle.size, sent: 0, stage: "copying", step: "offer", offered: true, sentAt: Date.now(), againAt: undefined, holdSince: undefined });
  });
  if (now) offer(now);
}

// ---- Whether it can run there ----

/** How long the computer it goes to has to say whether it can run it. */
const CHECK_WAIT = 45_000;
const checking = new Map<string, (answer: Record<string, unknown>) => void>();

/** Asked of the computer it goes to before anything's held: whether it can run it there (its agents
 * or engine running, its coding agent there and signed in, its model usable). Refused there: why,
 * as that computer says it; no answer: it isn't connected. */
async function checkThere(record: MoveRecord): Promise<{ note?: string }> {
  const answer = await new Promise<Record<string, unknown> | null>(resolve => {
    const timer = setTimeout(() => { checking.delete(record.id); resolve(null); }, CHECK_WAIT);
    checking.set(record.id, given => { clearTimeout(timer); checking.delete(record.id); resolve(given); });
    tell(record.peer, { op: "check", id: record.id, kind: record.kind, name: record.name, ...(record.lane ? { lane: true } : {}), ...(record.model ? { model: record.model } : {}) });
  });
  if (!answer) throw new Refused(say.notConnected(nameOf(record.peer)), 503);
  if (answer.ok !== true) throw new Refused(typeof answer.message === "string" && answer.message ? answer.message : `${record.name} can’t run on ${nameOf(record.peer)}.`);
  return typeof answer.note === "string" && answer.note ? { note: answer.note } : {};
}

/** A coding agent's account here, as its bridge lists it (`api/integration`). */
interface AgentAccount {
  id?: string;
  name?: string;
  metadata?: { agent?: string; installed?: boolean; account?: string; accountLabel?: string };
  connections?: unknown[];
}

/** The box it goes to: whether it can run what's coming (`check`), said back at once. */
async function checkHere(sender: MailSender, given: Record<string, unknown>, id: string): Promise<void> {
  const answer = (move: Record<string, unknown>) => void tell(sender.key, { op: "checked", id, ...move });
  const name = typeof given.name === "string" ? given.name : "It";
  const lane = given.lane === true;
  const place = IS_MAC ? "this Mac" : (plainName(readLink()?.name ?? "") || "this machine");
  const wait = AbortSignal.timeout(20_000);
  let target: Target;
  try {
    target = lane ? readBridgeFile() : readEngineFile();
    await call(target, "GET", "/api/session/active", undefined, wait);
  } catch {
    return answer({ ok: false, message: lane ? say.noAgents(place) : say.noEngine(place) });
  }
  const model = isObject(given.model) && typeof given.model.providerID === "string" && typeof given.model.id === "string" ? { providerID: given.model.providerID, id: given.model.id } : null;
  if (!model || given.kind === "group") return answer({ ok: true });
  if (!lane) {
    const refused = await signInRefusal(model, target, wait).catch(() => null);
    return answer(refused ? { ok: false, message: say.modelNotThere(name, place) } : { ok: true });
  }
  // (Its agent's accounts there, by the provider they're all of: `claude-code`, `claude-code@<account>`.)
  const kind = model.providerID.split("@")[0]!;
  // Custom Auto picks among what's signed in there.
  if (kind === "auto") return answer({ ok: true });
  const listed = await call(target, "GET", "/api/integration", undefined, wait).catch(() => null);
  if (!Array.isArray(listed)) return answer({ ok: false, message: say.noAgents(place) });
  const ofKind = (listed as (AgentAccount | null)[]).filter((each): each is AgentAccount => typeof each?.id === "string" && each.id.split("@")[0] === kind);
  // Not a coding agent's: a model the engine's sign-ins give it, as for a bot of the engine's.
  if (!ofKind.length) {
    const refused = await signInRefusal(model, readEngineFile(), wait).catch(() => null);
    return answer(refused ? { ok: false, message: say.modelNotThere(name, place) } : { ok: true });
  }
  const agentName = ofKind[0]!.name ?? "its coding agent";
  if (!ofKind.some(each => each.metadata?.installed)) return answer({ ok: false, message: say.agentNotThere(name, agentName, place) });
  const signedIn = ofKind.filter(each => each.metadata?.installed && Array.isArray(each.connections) && each.connections.length);
  if (!signedIn.length) return answer({ ok: false, message: say.agentSignedOut(name, agentName, place) });
  // Its own account, or another of the same agent there: which it is from then on, said.
  if (signedIn.some(each => each.id === model.providerID)) return answer({ ok: true });
  const uses = signedIn[0]!;
  const who = uses.metadata?.account ? ` (${uses.metadata.account})` : "";
  return answer({ ok: true, note: say.otherAccount(name, agentName, who, place) });
}

const bundleIdentity = ({ generation, hash }: BundleIdentity): BundleIdentity => ({ ...(generation === undefined ? {} : { generation }), ...(hash === undefined ? {} : { hash }) });
const sameBundle = (record: BundleIdentity, given: Record<string, unknown>): boolean => record.generation === given.generation &&
  (record.generation === undefined ? (given.hash === undefined || record.hash === given.hash) : record.hash === given.hash);

function offer(record: MoveRecord, ticks = performance.now()): void {
  markedRetry(record, ticks);
  tell(record.peer, { op: "offer", safeSwitch: true, ...(record.verifyPayloads ? { verifyPayloads: true } : {}), ...(record.twoComputers ? { twoComputers: true } : {}), id: record.id, ...bundleIdentity(record), kind: record.kind, thing: record.thing, name: record.name, size: record.size, wasPaused: record.wasPaused, ...(record.pausedAs !== undefined ? { pausedAs: record.pausedAs } : {}), from: record.from, ...(record.members ? { members: record.members } : {}), ...(record.lane ? { lane: true } : {}) });
}

// The next parts to the other box, up to `IN_FLIGHT` on their way.
function sendParts(record: MoveRecord, ticks = performance.now()): void | Promise<void> {
  const state = outgoing.get(record.id);
  if (!state || !record.size) return;
  const of = Math.ceil(record.size / PART_BYTES);
  const file = path.join(bundleOf(record.id), "bundle.tgz");
  let handle: number | null = null;
  // One unanswered this long is lost (the relay dropped, say): no longer on its way, so it goes again.
  let lost = false;
  for (const [i, when] of state.flying) if (ticks - when >= PART_AGAIN) { state.flying.delete(i); lost = true; }
  if (lost) state.window = Math.max(1, Math.floor((state.window ?? IN_FLIGHT) / 2));
  try {
    for (let i = 0; i < of && state.flying.size < (state.window ?? IN_FLIGHT); i++) {
      if (state.got.has(i) || state.flying.has(i)) continue;
      handle ??= fs.openSync(file, "r");
      const bytes = Buffer.alloc(Math.min(PART_BYTES, record.size - i * PART_BYTES));
      fs.readSync(handle, bytes, 0, bytes.length, i * PART_BYTES);
      if (!tell(record.peer, { op: "part", id: record.id, ...bundleIdentity(record), i, of, d: bytes.toString("base64url") })) break;
      state.flying.set(i, ticks);
    }
  } catch (error) {
    return refuseMove(record.id, error);
  } finally {
    if (handle !== null) fs.closeSync(handle);
  }
}

/** It's there, held: settled and read again; the same, it switches; not, it all goes again. */
async function arrived(record: MoveRecord, placed?: unknown): Promise<void> {
  const still = () => {
    const now = readMove(record.id);
    return now?.role === "from" && now.stage === "copying" && !now.failed && sameBundle(record, bundleIdentity(now)) ? now : null;
  };
  if (record.kind !== "group") {
    const target = record.lane ? readBridgeFile() : readEngineFile();
    await waitForPromptWrites(record.sessions);
    await settle(target, record.sessions, record.name);
    // What's in place there, file by file, is what was sent: or it's not the one to go on from.
    if (record.lane && placed !== outgoing.get(record.id)?.digest) throw new Refused(say.notSame(record.name));
    const again = record.lane
      // (A turn that started just now, the bridge's 423: changed, so it's got ready again.)
      ? (await carry<{ digest: string }>(target, "digest", { root: record.sessions[0] }).catch(error => {
        if ((error as { status?: number } | null)?.status === 423) return { digest: "" };
        throw error;
      })).digest
      : conversationHash(await exportAll(target, record.sessions[0]));
    const before = outgoing.get(record.id);
    // Taken back meanwhile (its copy there couldn't be set up, or it took too long): nothing more.
    if (!still()) return;
    if (!before || fingerprint(
      again, await queuesOf(target, record.sessions, record.name), await packSessionNotes(record.sessions),
      packForwardEvidence(followedSetup(readLink())?.id, before.evidenceScope),
    ) !== before.exports) {
      if (record.tries >= 2) throw new Refused(say.busy(record.name));
      // (Getting it ready again from now: what `keepMoving` counts its half hour from.)
      changeMove(record.id, each => { each.tries += 1; each.stage = "holding"; each.step = undefined; each.holdSince = Date.now(); });
      return drive(record.id);
    }
  }
  if (!still()) return;
  const payloads = outgoing.get(record.id)?.payloads;
  if (record.verifyPayloads && !payloads) throw new Refused("The source could not verify the move’s file inventory.");
  if (payloads) {
    try {
      verifySourceSessionPayloads(payloads.sessions);
      await queueMoveDisk.run("verifyQueuePayloads", payloads.queue);
    } catch (error) {
      throw new Refused(error instanceof Error ? error.message : "The source files changed during the move.");
    }
  }
  switchSource(record);
}

function switchSource(record: MoveRecord): void {
  // The durable switching journal keeps the source fenced before release goes on the wire.
  const now = changeMove(record.id, each => {
    delete each.assigning;
    each.stage = "switching";
    each.step = "switch";
    each.sentAt = each.switchSince = Date.now();
    each.againAt = undefined;
  });
  const sending = outgoing.get(record.id);
  if (sending) sending.since = performance.now();
  if (!now) return;
  markedRetry(now);
  sendSwitch(now);
}

function sendSwitch(record: MoveRecord): void {
  tell(record.peer, { op: "switch", safeSwitch: true, id: record.id, ...bundleIdentity(record) });
}

/** Confirmed ownership, independent of whether the original could be archived yet. */
export function confirmedOutcome(record: MoveRecord, pending: boolean): MoveOutcome & { ok: true } {
  const note = [record.note, pending ? say.cleanupPending : undefined].filter(Boolean).join(" ");
  return { ok: true, move: shownMove(record), ...(record.verifiedPayloads ? { verifiedPayloads: true } : {}), ...(note ? { note } : {}) };
}

/** A confirmed switch retires the original permanently. Cleanup is retried on its own. */
async function finish(id: string, verifiedPayloads = false): Promise<void> {
  const record = changeMove(id, each => { each.stage = "finishing"; each.step = "done"; if (verifiedPayloads) each.verifiedPayloads = true; each.sentAt = Date.now(); each.againAt = undefined; });
  if (!record) return;
  markedRetry(record);
  if (record.kind === "bot") noteBotsChanged();
  outgoing.delete(id);
  try {
    // Acknowledged ownership outlives both the source cleanup and its routing advertisement.
    noteMovedAway([...new Set([record.thing, ...record.sessions])], record.to);
    if (record.kind === "group") {
      await groupAction(readBridgeFile(), record.thing, "release");
      await queueMoveDisk.run("forgetWorkQueues", [record.thing]);
      noteQueuesChanged();
    } else {
      const engine = record.lane ? readBridgeFile() : readEngineFile();
      const place = record.folder ?? path.join(K, `moved-${record.thing}`);
      // A failed export never authorizes deleting sessions. Once the backup is recorded, retries
      // can continue a partial deletion without trying to export it again.
      if (!record.kept) {
        fs.mkdirSync(place, { recursive: true });
        if (record.lane) {
          await carry(engine, "keep", { root: record.sessions[0], into: path.join(place, "conversation") });
        } else {
          const exports = await exportAll(engine, record.sessions[0]);
          writePrivate(path.join(place, "conversation.json"), JSON.stringify(exports, null, 2));
        }
        changeMove(id, each => { each.kept = true; });
      }
      for (const session of [...record.sessions].reverse()) {
        await deleteSession(engine, session);
        forgetDecided(session);
      }
      for (const session of record.sessions) forgetQueue(session);
      await queueMoveDisk.run("forgetWorkQueues", record.sessions);
      noteQueuesChanged();
      await forgetSessionNotes(record.sessions);
      const trash = await toTrash(place);
      noteLeft({ id, thing: record.thing, kind: record.kind, to: record.to, at: Date.now(), sessions: [...record.sessions], ...(record.lane ? { lane: true } : {}), ...(trash ? { trash } : {}), ...(record.leftOf?.length ? { left: record.leftOf } : {}) });
    }
    changeMove(id, each => { each.cleanupPending = false; });
    ended(id, confirmedOutcome(record, false));
    forgetMove(id);
  } catch (error) {
    if (!record.cleanupPending) console.error("kuvo-link: move succeeded; source cleanup will be retried", error);
    changeMove(id, each => { each.cleanupPending = true; });
    ended(id, confirmedOutcome(record, true));
  }
}

/** Taken back before the switch request: the original can go on where it was. */
async function takeBack(id: string, message: string, status = 409): Promise<void> {
  const record = readMove(id);
  if (!record || record.role !== "from" || record.failed || record.assigning || record.stage === "switching" || record.stage === "finishing") return;
  await failSource(record, message, status, record.offered === true);
}

/** Record a terminal failure before releasing either hold. */
async function failSource(record: MoveRecord, message: string, status: number, dropThere: boolean, dropUntilAck = false): Promise<void> {
  const failed = changeMove(record.id, each => {
    if (dropThere && dropUntilAck) {
      each.dropUntilAck = true;
      // An older journal asked for ownership from "switching". The primary
      // has since ruled it out, so retries must send drop mail.
      each.stage = "copying";
    }
    each.failed = message;
    each.failedStatus = status;
    each.step = dropThere ? "drop" : "done";
    each.sentAt = each.dropSince = Date.now();
    each.againAt = undefined;
  });
  if (failed) await settleFailure(failed);
}

async function settleFailure(record: MoveRecord): Promise<void> {
  const { id } = record;
  const dropThere = record.step === "drop" || (record.offered === true && record.step !== "done");
  if (dropThere && record.step !== "drop") changeMove(id, each => { each.step = "drop"; each.dropSince = Date.now(); });
  outgoing.delete(id);
  unhold(record);
  if (record.kind === "group") await wakeGroup(record.thing);
  ended(id, { ok: false, status: record.failedStatus ?? 409, message: record.failed! });
  heard.delete(id);
  if (waitingFor.delete(id)) movesChanged();
  if (dropThere) {
    markedRetry(record);
    tell(record.peer, { op: "drop", id, ...bundleIdentity(record) });
    try { fs.rmSync(bundleOf(id), { recursive: true, force: true }); } catch {}
  } else forgetMove(id);
}

// ---- The box it goes to ----

// What the box it goes to has of each bundle on its way: the parts written so far (as its `got`
// file says, after a restart).
const incoming = new Map<string, Set<number>>();
function partsOf(id: string): Set<number> {
  let parts = incoming.get(id);
  if (!parts) {
    parts = new Set();
    try { for (const line of fs.readFileSync(path.join(bundleOf(id), "got"), "utf8").split("\n")) if (/^\d+$/.test(line)) parts.add(Number(line)); } catch {}
    incoming.set(id, parts);
  }
  return parts;
}

/** What this box keeps in memory of moves under way, gone, as it is when the bridge starts again
 * (a test's restart). */
export function forgetMovesInMemory(): void {
  incoming.clear();
  outgoing.clear();
  heard.clear();
  waitingFor.clear();
  loggedStuck.clear();
  missing.clear();
  orphans.clear();
  retryClocks.clear();
  lookingSince = performance.now();
}

async function takeOffer(sender: MailSender, given: Record<string, unknown>): Promise<void> {
  const id = String(given.id);
  const generation = given.generation;
  const hash = given.hash;
  const answer = (move: Record<string, unknown>) => tell(sender.key, { id, generation, hash, ...move });
  const kind = given.kind as MoveKind;
  const thing = typeof given.thing === "string" ? given.thing : "";
  const name = typeof given.name === "string" ? given.name : "It";
  const size = typeof given.size === "number" ? given.size : 0;
  const lane = given.lane === true;
  if (!["bot", "chat", "group"].includes(kind) || !/^[A-Za-z0-9_]{1,80}$/.test(thing) || typeof hash !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(hash) || size <= 0) return void answer({ op: "refused", message: "That isn’t a move." });
  if (given.safeSwitch !== true || typeof generation !== "number" || !Number.isSafeInteger(generation) || generation < 1) return void answer({ op: "refused", message: "Update Kuvo on the source computer before trying the move again." });
  if (followedSetup(readLink())?.checkpointed && given.twoComputers !== true)
    return void answer({ op: "refused", message: "Update both computers and wait for their current setup before trying the move again." });
  const was = readMove(id);
  if (was && (was.role !== "to" || was.peer !== sender.key || was.thing !== thing || was.kind !== kind)) return void answer({ op: "refused", message: say.moving });
  if (was?.generation !== undefined && generation < was.generation) return;
  if (was?.dropping) return void answer({ op: "refused", message: say.oldCopy(name, nameOf(sender.key)) });
  if (endedAs(id) || was?.activation || (was && uncertainOldCopy(was))) return void answer({ op: "refused", message: say.moving });
  if (was?.generation === generation) {
    if (was.hash === hash) answer({ op: "ready", safeSwitch: true, verifyPayloads: true, typedAttachmentPaths: true });
    return;
  }
  if (size > MOST_BUNDLE) return void answer({ op: "refused", message: say.tooBig(name) });
  const place = IS_MAC ? "this Mac" : (plainName(readLink()?.name ?? "") || "this machine");
  if (kind === "group") {
    let bridge: Target | null = null;
    try { bridge = readBridgeFile(); } catch {}
    const running = bridge ? await askGroups(bridge, "GET", "", undefined, engineWait()).then(answer => answer.status === 200).catch(() => false) : false;
    if (!running) return void answer({ op: "refused", message: say.noGroups(place) });
    // To a machine, only with its bots: they all live there, and stay there while it does.
    const members = Array.isArray(given.members) ? given.members : [];
    if (!IS_MAC && members.some(member => !botHere(member))) return void answer({ op: "refused", message: say.botsElsewhere(name) });
  } else {
    try {
      await call(lane ? readBridgeFile() : readEngineFile(), "GET", "/api/session/active", undefined, engineWait());
    } catch {
      return void answer({ op: "refused", message: lane ? say.noAgents(place) : say.noEngine(place) });
    }
  }
  // Offered again, changed (it got more while it moved): what came of it before goes first; and a
  // move of it this way from the same computer, cut short: this one takes its place (in its own
  // turn: one still being set up is let go of once it's all there). Any other under way (from
  // another computer, or away from here) stays. What can't go yet (its engine or group chats
  // refusing) is said, rather than left unanswered.
  const before = moveOf(thing);
  try {
    if (was) await clearCopy(was);
    if (before && before.id !== id && before.role === "to" && before.peer === sender.key) await inTurn(before.id, () => drop(before.id));
  } catch {
    return void answer({ op: "refused", message: say.oldCopy(name, place) });
  }
  const remaining = moveOf(thing);
  if (remaining && remaining.id !== id) return void answer({ op: "refused", message: say.moving });
  // Never over one that lives here: a copy only comes where there's none.
  if (!was && await livesHere(kind, thing)) return void answer({ op: "refused", message: `${name} is on ${place} already.` });
  fs.rmSync(bundleOf(id), { recursive: true, force: true });
  fs.mkdirSync(bundleOf(id), { recursive: true, mode: 0o700 });
  incoming.set(id, new Set());
  saveMove({
    id, kind, thing, name, ...(given.twoComputers === true ? { twoComputers: true } : {}), from: sender.key, to: ownKey(readLink() ?? {}) ?? "", role: "to", stage: "copying", sessions: [],
    peer: sender.key, ...(kind === "group" ? { importingGroup: false } : { createdSessions: [] }), safeSwitch: true, ...(given.verifyPayloads === true ? { verifyPayloads: true } : {}), wasPaused: given.wasPaused === true, ...(typeof given.pausedAs === "string" ? { pausedAs: given.pausedAs.slice(0, 2000) } : {}), generation, hash, size, sent: 0, tries: 0, started: was?.started ?? Date.now(), ...(lane ? { lane: true } : {}),
  });
  answer({ op: "ready", safeSwitch: true, verifyPayloads: true, typedAttachmentPaths: true });
}

/** Whether a bot, chat or group chat by that ID is on this box now. */
async function livesHere(kind: MoveKind, thing: string): Promise<boolean> {
  if (kind === "group") {
    let bridge: Target | null = null;
    try { bridge = readBridgeFile(); } catch {}
    return bridge ? (await askGroups(bridge, "GET", `/${encodeURIComponent(thing)}`, undefined, engineWait()).catch(() => null))?.status === 200 : false;
  }
  if (fs.existsSync(folderOf(kind, thing)!)) return true;
  const root = kind === "bot" ? null : thing;
  if (!root) return false;
  // On either: a chat's on the agents bridge once it talks with a coding agent.
  for (const target of [readEngineFile, readBridgeFile]) {
    try {
      await call(target(), "GET", `/api/session/${encodeURIComponent(root)}`, undefined, engineWait());
      return true;
    } catch {}
  }
  return false;
}

async function takePart(sender: MailSender, given: Record<string, unknown>): Promise<void> {
  const id = String(given.id);
  const record = readMove(id);
  const i = given.i, of = given.of;
  const expected = Math.ceil((record?.size ?? 0) / PART_BYTES);
  if (!record || record.role !== "to" || record.dropping || record.peer !== sender.key || !sameBundle(record, given) || record.hash !== given.hash || !Number.isInteger(i) || of !== expected
    || (i as number) < 0 || (i as number) >= expected || typeof given.d !== "string" || given.d.length > Math.ceil(PART_BYTES * 4 / 3)) return;
  // All here already, and this one's answer lost: said again, so the box it comes from goes on.
  const identity = bundleIdentity(record);
  if (record.complete) return void tell(sender.key, { op: "got", id, ...identity, i });
  const parts = partsOf(id);
  const bytes = Buffer.from(given.d, "base64url");
  const file = path.join(bundleOf(id), "bundle.tgz");
  const handle = fs.openSync(file, fs.existsSync(file) ? "r+" : "w", 0o600);
  try { fs.writeSync(handle, bytes, 0, bytes.length, (i as number) * PART_BYTES); } finally { fs.closeSync(handle); }
  if (!parts.has(i as number)) {
    parts.add(i as number);
    // Noted beside it too, so a restart here counts what came before.
    fs.appendFileSync(path.join(bundleOf(id), "got"), `${i}\n`, { mode: 0o600 });
  }
  tell(sender.key, { op: "got", id, ...identity, i });
  if (parts.size % 32 === 0 || parts.size === of) changeMove(id, each => { each.sent = Math.min(each.size ?? 0, parts.size * PART_BYTES); });
  if (parts.size !== of) return;
  incoming.delete(id);
  // All here: put in place once, whatever comes again after; in the move's turn, so a step about it
  // that comes meanwhile (a drop) waits until it's in place, then finds all of it.
  // (Since now: one cut short by a restart is known for one, `resumeMoves`.)
  changeMove(id, each => { each.complete = true; each.setUpSince = Date.now(); });
  toSetUp.set(id, identity);
  return inTurn(id, () => setUp(sender, id, file, identity)).finally(() => {
    if (toSetUp.get(id) === identity) toSetUp.delete(id);
  });
}

async function setUp(sender: MailSender, id: string, file: string, identity: BundleIdentity): Promise<void> {
  const record = readMove(id);
  // Let go of before its turn came: nothing to set up.
  if (!record || record.role !== "to" || record.peer !== sender.key || !sameBundle(record, identity)) return;
  // Still at it, said now and each minute (a big one on a small machine takes a while), so the box
  // it came from doesn't take it back for want of an answer.
  tell(sender.key, { op: "working", id, ...identity });
  const beat = setInterval(() => tell(sender.key, { op: "working", id, ...identity }), STEP_AGAIN);
  // Given up on if it takes too long (`keepMoving`), its engine stuck.
  const stop = new AbortController();
  settingUp.set(id, { stop, since: performance.now() });
  changeMove(id, each => { each.setUpSince = Date.now(); });
  try {
    if (await hashOf(file) !== record.hash) throw new NotTaken("What came wasn’t what was sent.");
    const sessions = await bringIn(record, stop.signal);
    stop.signal.throwIfAborted();
    const now = changeMove(id, each => {
      each.sessions = sessions;
      each.setUpSince = undefined;
      each.setUpAt = Date.now();
      each.typedAttachmentPaths = each.payloads?.sessions.length ? true : undefined;
    });
    if (now) tellArrived(now, identity);
  } catch (thrown) {
    const error = stop.signal.aborted ? stop.signal.reason : thrown;
    // What was set up of it goes: the box it came from takes it back, and a switch for it is
    // answered `dropped`. Only what it brought is said back, never what this box found.
    if (!(error instanceof NotTaken)) console.error(`kuvo-link: a move that came couldn't be set up: ${String((error as Error | null)?.message ?? error)}`);
    // No longer at it: no more word that it is while what came goes.
    clearInterval(beat);
    await drop(id).catch(() => {});
    tell(sender.key, { op: "failed", id, ...identity, message: error instanceof NotTaken ? `It couldn’t be set up on ${HERE}: ${error.message}` : `It couldn’t be set up on ${HERE}.` });
  } finally {
    clearInterval(beat);
    settingUp.delete(id);
  }
}

function tellArrived(record: MoveRecord, identity: Record<string, unknown>): void {
  tell(record.peer, { op: "arrived", id: record.id, ...identity,
    ...(record.payloads ? { verifiedPayloads: true } : {}),
    ...(record.typedAttachmentPaths ? { typedAttachmentPaths: true } : {}),
    ...(record.digest ? { digest: record.digest } : {}) });
}

/** Only a definite absence allows an import or counts as a completed rollback. */
async function sessionHere(target: Target, id: string, signal?: AbortSignal): Promise<unknown> {
  try {
    return await call(target, "GET", `/api/session/${encodeURIComponent(id)}`, undefined, engineWait(signal));
  } catch (error) {
    if ((error as { status?: number })?.status === 404) return undefined;
    throw error;
  }
}

const importReceipt = (record: MoveRecord) => `${record.id}:${record.generation}:${record.hash}`;

function forwardMoveScope(record: MoveRecord, sessions: readonly string[], queues: Contents["workQueue"]): ForwardMoveScope {
  const group = record.kind === "group" ? record.thing : undefined;
  const owners = new Set(group ? [group] : sessions);
  const items = new Set<string>();
  for (const item of queues?.items ?? []) if (owners.has(item.owner)) items.add(item.id);
  for (const item of queues?.deletions ?? []) if (owners.has(item.queue)) items.add(item.id);
  return { sessions, group, items };
}

function checkedForwardEvidence(record: MoveRecord, contents: Contents, sessions: readonly string[]) {
  if (contents.forwardEvidence === undefined) return;
  try {
    return checkForwardEvidence(contents.forwardEvidence, followedSetup(readLink())?.id,
      forwardMoveScope(record, sessions, contents.workQueue));
  } catch (error) {
    throw new NotTaken((error as Error).message);
  }
}

/** The bundle put in place here, held: its folder, its conversations, a group chat. */
async function bringIn(record: MoveRecord, signal: AbortSignal): Promise<string[]> {
  const unpacked = await moveBundleDisk.run("unpackForTransfer", record.id);
  if (!unpacked.openedBundle) throw new NotTaken(unpacked.refused);
  const { contents, folder, carry: carried, opened } = unpacked.openedBundle;
  if (contents.sessionNotes !== undefined && (record.kind === "group" || !offlineEditsReady()))
    throw new NotTaken("The ordinary session notes cannot be placed here.");
  if (record.verifyPayloads && !Array.isArray(contents.sessionFiles)) throw new NotTaken("The move did not include its session attachment inventory.");
  if (record.kind === "group") {
    const evidence = checkedForwardEvidence(record, contents, []);
    const held = contents.group;
    if (!held || !isObject(held.group)) throw new NotTaken("The group chat didn’t come with it.");
    changeMove(record.id, each => { each.importingGroup = true; });
    const answer = await askGroups<{ message?: unknown }>(readBridgeFile(), "PUT", `/${encodeURIComponent(record.thing)}`, { group: held.group, messages: held.messages, status: "held", moveImport: importReceipt(record) }, engineWait(signal));
    if (answer.status !== 200) throw new Error(typeof answer.value?.message === "string" ? answer.value.message : `Its group chats answered ${answer.status}.`);
    changeMove(record.id, each => { each.placed = true; });
    changeMove(record.id, each => { each.payloads = { sessions: [], queue: [] }; });
    if (contents.workQueue !== undefined) {
      await queueMoveDisk.run("placeMoveWorkQueues", record.id, contents.workQueue, [record.thing], opened, record.verifyPayloads, []);
      noteQueuesChanged();
    }
    if (evidence) placeForwardEvidence(evidence);
    return [];
  }
  const place = folderOf(record.kind, record.thing)!;
  // Only its own conversations, one of them its root (a chat's is the chat; a bot's, the one its
  // folder names), and none that's here already: it never brings in, or takes away, another one.
  const root = record.kind === "chat" ? record.thing : (() => { try { return folder ? fs.readFileSync(path.join(folder, "session"), "utf8").trim() : ""; } catch { return ""; } })();
  // (It says the same as what was offered, or it's put where it can't run.)
  if (Boolean(contents.lane) !== Boolean(record.lane)) throw new NotTaken("What came isn’t a move.");
  if (contents.lane ? contents.lane.root !== root : !ownTree(contents.exports, root)) throw new NotTaken("Its conversations aren’t its own.");
  const engine = record.lane ? readBridgeFile() : readEngineFile();
  // (Those it says it brings, none of them here: noted before they're brought in, so each is held from
  // the moment it's here. The bridge looks again as it places them.)
  const bringing = contents.lane ? contents.lane.sessions.filter(each => typeof each === "string" && /^ses_[A-Za-z0-9]{1,60}$/.test(each)) : idsOf(contents.exports);
  if (contents.lane && (!bringing.length || bringing.length !== contents.lane.sessions.length)) throw new NotTaken("Its conversations aren’t its own.");
  const evidence = checkedForwardEvidence(record, contents, bringing);
  for (const session of bringing) {
    if (await sessionHere(engine, session, signal) !== undefined) throw new NotTaken(`Its conversations are on ${HERE} already.`);
  }
  signal.throwIfAborted();
  const payloads = checkedSessionPayloads(record.id, contents.sessionFiles ?? [], bringing);
  // A reserved placement belongs only to this move. Journal it before copying so rollback
  // can remove a partial install after a restart.
  const mappings = new Map(attachmentMappings(payloads));
  const records = contents.lane && carried ? carriedSessionRecords(carried, bringing) : contents.exports;
  try { requireAttachmentReferences(records, contents.queues, mappings); }
  catch (error) { throw new NotTaken((error as Error).message); }
  for (const file of mappings.values()) {
    if (fs.lstatSync(file, { throwIfNoEntry: false })) throw new NotTaken("A session attachment is already at its destination.");
  }
  changeMove(record.id, each => { each.payloads = { sessions: payloads, queue: [] }; });
  const attachments = installSessionPayloads(payloads, path.join(opened, "session-files"));
  if (folder) {
    if (fs.existsSync(place)) throw new NotTaken(`${record.name} is on ${HERE} already.`);
    fs.mkdirSync(path.dirname(place), { recursive: true });
    // Publish only a held folder, even if the bridge stops immediately after the rename.
    writeDurablePrivate(PAUSED(folder), moveMarker(record.id));
    // Placed by this move: it goes if the move's taken back.
    changeMove(record.id, each => { each.placed = true; });
    fs.renameSync(folder, place);
    syncDurableFile(PAUSED(place), path.dirname(place));
    // Where a chat's watches' checks run here.
    if (record.kind === "chat") writePrivate(path.join(place, "directory"), CHATS);
  }
  const directory = directoryOf(record.kind, record.thing);
  fs.mkdirSync(directory, { recursive: true });
  const was = contents.was && typeof contents.was.place === "string" ? { place: plainName(contents.was.place), ...(typeof contents.was.folder === "string" ? { folder: contents.was.folder } : {}) } : undefined;
  // Its conversations, noted before they're brought in: any that came go if it's taken back (with a
  // coding agent, what its bridge placed it undoes itself).
  let sessions = bringing;
  changeMove(record.id, each => { each.sessions = sessions; each.was = was; });
  let infos: (Partial<EngineSession> & { id: string })[];
  if (contents.lane) {
    const placed = await carry<{ digest: string; sessions: string[]; model?: { providerID?: string; id?: string }; came?: { providerID?: string; id?: string }; infos: { id: string; metadata?: Record<string, unknown> }[]; typedAttachmentPaths?: true }>(engine, "place", { from: carried, keep: placedOf(record.id), directory, root, chat: isChatsFolder(directory) ? directory : null, ...(attachments.size ? { attachments: [...attachments] } : {}) }, signal).catch(error => {
      if (error instanceof Refused) throw new NotTaken(error.message);
      throw error;
    });
    if (attachments.size && !placed.typedAttachmentPaths) throw new NotTaken("Update the destination coding agents bridge to move attachments.");
    sessions = [...new Set([...sessions, ...placed.sessions])];
    changeMove(record.id, each => { each.digest = placed.digest; each.sessions = sessions; each.createdSessions = placed.sessions; });
    infos = placed.infos as typeof infos;
    // Its account isn't on this computer: the bot goes on with the one its conversation went to. And
    // on the model it ran on, where it named it by an alias that stands for another one here.
    if (record.kind === "bot" && placed.model?.providerID) rewriteModel(place, placed.model, placed.came);
  } else {
    for (const exported of contents.exports) {
      if (!isObject(exported) || !isObject(exported.info) || typeof exported.info.id !== "string") throw new NotTaken("A conversation did not come through.");
      const session = exported.info.id;
      changeMove(record.id, each => { each.importingSession = session; });
      await importConversation(engine, mapAttachmentPaths(exported, attachments, "transcript"), directory, importReceipt(record), engineWait(signal));
      changeMove(record.id, each => {
        each.createdSessions = [...(each.createdSessions ?? []), session];
        each.importingSession = undefined;
      });
    }
    infos = contents.exports.flatMap(exported => isObject(exported) && isObject(exported.info) ? [exported.info as Partial<EngineSession> & { id: string }] : []);
  }
  // What waited for them, kept here paused until the switch.
  let goOn: string[];
  try { goOn = unpackQueues(mapAttachmentPaths(contents.queues, attachments, "queues"), sessions); } catch (error) {
    if (error instanceof CantCarry) throw new NotTaken("Some messages waiting to be sent didn’t come through. Try again.");
    throw error;
  }
  if (goOn.length) changeMove(record.id, each => { each.queuesGoOn = goOn; });
  if (contents.sessionNotes !== undefined) {
    try { await placeSessionNotes(contents.sessionNotes, sessions, importReceipt(record)); }
    catch (error) { throw new NotTaken((error as Error).message); }
  }
  // The calls it made on Autopilot; on it (its flag came with it), its agent is told so here.
  keepDecided(contents.decided, sessions);
  // Its conversations' Work Queues, with their files.
  if (contents.workQueue !== undefined) {
    await queueMoveDisk.run("placeMoveWorkQueues", record.id, contents.workQueue, sessions, opened, record.verifyPayloads, payloads);
    noteQueuesChanged();
  }
  for (const info of infos) {
    if (isAutopilot(info)) await tellAutopilot(engine, info.id, true, signal).catch(error => console.error(`kuvo-link: ${record.name} couldn't be told it's on Autopilot here: ${error}`));
  }
  // A bot from a box that still gave bots keys of their own: its folder's settings name them where
  // it was. Those servers go (it uses this box's integrations, and is told of one that isn't
  // connected here: bots/own-keys.ts); the keys it came with are never kept.
  if (record.kind === "bot" && place) dropOwnServers(place);
  if (folder) await syncDurableTreeLater(place, path.dirname(path.dirname(place)));
  await fs.promises.rm(path.join(bundleOf(record.id), "opened"), { recursive: true, force: true });
  // Keep the fences before reporting arrival. A rollback between arrival and
  // switch must retain them, even if the move is subsequently canceled.
  if (evidence) placeForwardEvidence(evidence);
  return sessions;
}

/** What the bridge keeps of what it placed for a move, until it's over (never in what's opened). */
const placedOf = (id: string) => path.join(bundleOf(id), "carried");

/** The account a moved bot's model is on, as its conversation's now is here (`now`), and the model:
 * the one its conversation went on with, when the bot named the same as its conversation did as it
 * came (`came`; an alias such as Claude Code's `default`, which stands for another model here). */
function rewriteModel(folder: string, now: { providerID?: string; id?: string }, came?: { providerID?: string; id?: string }): void {
  try {
    const file = path.join(folder, "bot.json");
    const described: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    if (!isObject(described) || !isObject(described.model) || typeof now.providerID !== "string") return;
    const model = described.model;
    const same = typeof came?.id === "string" && model.id === came.id && model.providerID === came.providerID;
    const id = same && typeof now.id === "string" && now.id ? now.id : model.id;
    if (model.providerID === now.providerID && model.id === id) return;
    described.model = { ...model, providerID: now.providerID, id };
    writePrivate(file, JSON.stringify(described, null, 2));
  } catch {}
}

/** What it's told as it's switched to: where it is now, for what its conversation still says. */
async function tellMoved(record: MoveRecord): Promise<void> {
  const root = record.sessions[0];
  if (!root || !record.was) return;
  const folder = record.kind === "bot" ? folderOf(record.kind, record.thing) : CHATS;
  const lines = [
    `You moved to this computer from ${record.was.place}. What you did before that happened there.`,
    ...(record.was.folder && folder && record.was.folder !== folder ? [`Paths in your conversation under ${record.was.folder} are under ${folder} now.`] : []),
    ...attachmentMappings(record.payloads?.sessions ?? []).map(([from, to]) => `The attachment at ${from} is at ${to} now.`),
    `Other paths on ${record.was.place} aren't here, and the sessions you started there stay there: reach them with list_agents, send_message and read_conversation.`,
  ];
  const target = record.lane ? readBridgeFile() : readEngineFile();
  await call(target, "PUT", `/api/experimental/session/${encodeURIComponent(root)}/instructions/entries/kuvo.moved`, { value: lines.join(" ") }, engineWait());
}

/** Switched to: the copy here goes on from now, as it was where it came from. */
async function switchHere(record: MoveRecord): Promise<void> {
  if (record.kind === "group") {
    if (!await activateGroup(record.thing, record.id)) throw new Error("Waiting for the accepted move ownership.");
  } else {
    const authority = { link: readLink(), activating: record.id };
    if (![moveReference(record.kind, record.thing), ...record.sessions.map(id => ({ kind: "conversations" as const, id }))]
      .every(ref => mayActivate(ref, authority))) throw new Error("Waiting for the accepted move ownership.");
    await backHere(record.thing).catch(error => console.error(`kuvo-link: what an earlier move of ${record.name} left couldn't all be cleared: ${String((error as Error | null)?.message ?? error)}`));
    // Told where it is now, before anything could run it here.
    await tellMoved(record).catch(error => console.error(`kuvo-link: ${record.name} couldn't be told it moved: ${String((error as Error | null)?.message ?? error)}`));
    // A queue that went on where it came from goes on here (one you stopped stays stopped).
    for (const session of record.queuesGoOn ?? []) await resumeQueue(record.lane ? readBridgeFile() : readEngineFile(), session, record.id);
    for (const session of record.sessions) watchQueueLimit(record.lane ? readBridgeFile : readEngineFile, session);
    if (record.kind === "bot") await settleBot(record.thing, readEngineFile, () => { try { return readBridgeFile(); } catch { return null; } }).catch(() => {});
    unhold(record, folderOf(record.kind, record.thing) ?? undefined);
  }
}

/** Moved back to where an earlier move left it: this is the one now. What's in the trash is older, and
 * goes; each agent's files were replaced by what came, but for the memory a bot now keeps in its folder. */
async function backHere(thing: string): Promise<void> {
  for (const entry of readLeft().filter(each => each.thing === thing)) {
    // Forgotten first: whatever below fails, nothing here is ever removed later as "left" (it runs here now).
    forgetLeft(entry.id);
    removeTrashed(entry.trash);
    const memory = (entry.left ?? []).flatMap(each => isObject(each) && typeof each.memory === "string" ? [{ ...each, paths: [] }] : []);
    const bridge = memory.length ? bridgeTarget() : null;
    if (bridge) await carry(bridge, "remove", { left: memory, sessions: entry.sessions, back: true });
  }
}

// ---- What a move left ----

let leftAskedAt = 0;

/** The newest message of a session here. */
async function newestMessage(lane: boolean, session: string): Promise<string | undefined> {
  const target = lane ? readBridgeFile() : readEngineFile();
  const listed = await call(target, "GET", `/api/session/${encodeURIComponent(session)}/message?limit=1&order=desc`, undefined, engineWait());
  const newest = Array.isArray(listed) ? listed[0] : null;
  return isObject(newest) && typeof newest.id === "string" ? newest.id : undefined;
}

/** When it first ran here since it came at `since`: the first of its replies after `last` (its newest
 * message as it came; replies that came with it keep the other computer's times), on what it's kept on. */
async function ranHereSince(lane: boolean, session: string, since: number, last?: string): Promise<number | null | "gone"> {
  const target = lane ? readBridgeFile() : readEngineFile();
  let listed: unknown;
  try {
    listed = await call(target, "GET", `/api/session/${encodeURIComponent(session)}/message?limit=20&order=desc`, undefined, engineWait());
  } catch (error) {
    if ((error as { status?: number } | null)?.status === 404) return "gone";
    throw error;
  }
  let messages = Array.isArray(listed) ? listed : [];
  // Not among the last 20 when there are more: all of those came after it. Fewer, and it isn't one
  // (gone since, reverted say): by the time, then.
  let came = last ? messages.findIndex(each => isObject(each) && each.id === last) : -1;
  if (came < 0 && messages.length < 20) came = -2;
  if (came >= 0) messages = messages.slice(0, came);
  const byOrder = !!last && came !== -2;
  const replies = messages.flatMap(each => {
    if (!isObject(each) || each.type !== "assistant" || !isObject(each.time) || typeof each.time.created !== "number") return [];
    return byOrder || each.time.created > since ? [Math.max(each.time.created, since)] : [];
  });
  return replies.length ? Math.min(...replies) : null;
}

/** Once in a while: the computers a move went to asked whether it has run there; what's been kept
 * long enough since then, removed. Nothing while a move of it is under way here (it may be coming
 * back, and its way back taken back). */
function keepLeft(now: number): void {
  const entries = readLeft();
  if (!entries.length) return;
  // (A clock set back asks again, rather than not until it's caught up.)
  const asking = now - leftAskedAt >= ASK_LEFT || now < leftAskedAt;
  if (asking) {
    leftAskedAt = now;
    for (const entry of entries) {
      if (entry.ranAt === undefined && entry.sessions[0]) tell(entry.to, { op: "ran?", id: entry.id, session: entry.sessions[0], ...(entry.lane ? { lane: true } : {}) });
    }
  }
  for (const entry of entries) {
    if (entry.ranAt === undefined || now - entry.ranAt < KEEP_LEFT || moveOf(entry.thing)) continue;
    inTurn(entry.id, () => clearLeft(entry)).catch(error => console.error(`kuvo-link: what a move of ${entry.thing} left couldn't be removed yet: ${String((error as Error | null)?.message ?? error)}`));
  }
}

async function clearLeft(entry: LeftEntry): Promise<void> {
  if (moveOf(entry.thing) || !readLeft().some(each => each.id === entry.id)) return;
  if (entry.left?.length) {
    const bridge = bridgeTarget();
    // (Its agents' folders aren't looked at without it: tried again later.)
    if (!bridge) throw new Error("its coding agents aren't running");
    await carry(bridge, "remove", { left: entry.left, sessions: entry.sessions });
  }
  removeTrashed(entry.trash);
  forgetLeft(entry.id);
}

/** A staged copy from before durable activation could have run before a crash. Preserve it
 * unless a source that now speaks the safe protocol commits it. */
function uncertainOldCopy(record: MoveRecord): boolean {
  return !record.safeSwitch && record.complete === true && record.setUpSince === undefined && !record.dropping;
}

const logActivation = (error: unknown) => console.error("kuvo-link: move activation will be retried", error);

async function completeActivation(id: string): Promise<void> {
  const record = readMove(id);
  if (record?.role !== "to" || !record.activation) return;
  const { at, last } = record.activation;
  const decision = endedFor(id, record.peer);
  if (decision) {
    if (decision.as !== "switched" || !sameBundle(record, bundleIdentity(decision))) throw new Error("Move ownership conflicts with its activation record");
    sealDecision(id);
  } else {
    if (record.verifyPayloads && !record.payloads) throw new Error("The move’s file inventory is unavailable.");
    if (record.payloads) {
      verifySessionPayloads(record.payloads.sessions);
      await queueMoveDisk.run("verifyQueuePayloads", record.payloads.queue);
    }
    // Seal the journal if its rename was visible after an interrupted directory sync.
    const pending = changeMove(id, each => { each.sentAt = Date.now(); });
    if (pending) markedRetry(pending);
    const own = ownKey(readLink() ?? {});
    if (own && own === record.to) noteMovedAway([record.thing, ...record.sessions], own, at, { arrived: true });
    else forgetMovedAway([record.thing, ...record.sessions]);
    await switchHere(record);
    noteEnded(id, "switched", at, record.sessions, last, record.peer, bundleIdentity(record), record.verifyPayloads ? record.payloads : undefined);
  }
  if (record.payloads) await queueMoveDisk.run("clearQueuePlacement", record.payloads.queue, id);
  forgetMove(id);
  if (record.kind === "bot") noteBotsChanged();
  // Publish the live copy before the source hears the switch acknowledgement.
  const host = hostOf();
  await (host ? announce(host, readEngineFile) : refreshOwn(readEngineFile)).catch(() => {});
  answerDecision({ key: record.peer }, id, bundleIdentity(record));
}

function answerDecision(sender: Pick<MailSender, "key">, id: string, request: Record<string, unknown>): void {
  const decision = endedFor(id, sender.key);
  if (!decision || !sameBundle(decision, request)) return;
  sealDecision(id);
  const bundle = bundleIdentity(decision);
  if (decision.as === "switched") {
    tell(sender.key, { op: "switched", id, ...bundle, ...(decision.payloads ? { verifiedPayloads: true } : {}) });
  }
  else tell(sender.key, { op: "dropped", id, ...bundle, ...(decision.peer ? { neverSwitched: true } : {}) });
}

/** A copy here that won't be switched to from now on (`dropping`, and noted as dropped): what's
 * here of it stays held until it's gone. */
function letGo(id: string, explicit = false): MoveRecord | null {
  const record = readMove(id);
  const link = readLink();
  if (!record || record.role !== "to" || record.activation || endedAs(id) === "switched" || uncertainOldCopy(record) ||
    (!explicit && (record.twoComputers || followedSetup(link)?.checkpointed) && record.complete && !record.dropping && record.setUpSince === undefined)) return null;
  noteEnded(id, "dropped", Date.now(), record.sessions, undefined, record.peer, bundleIdentity(record));
  if (!record.dropping) changeMove(id, each => { each.dropping = true; each.dropSince = Date.now(); });
  return record;
}

/** A copy here that won't be switched to: gone, as if it never came. One that can't all go now (its
 * engine or group chats down) is kept, held and never switched to, and let go of again each minute
 * (`keepMoving`) until it's all gone: its record goes only then, never leaving a second copy live. */
async function drop(id: string): Promise<void> {
  const record = letGo(id);
  if (!record) return;
  await clearCopy(record);
  forgetMove(id);
  loggedStuck.delete(id);
}

/** What a move brought here, gone: its conversations, the folder it placed, a group
 * chat held for it. It throws while any of it can't go (tried again: what went already is as good
 * as gone). */
async function clearCopy(record: MoveRecord): Promise<void> {
  incoming.delete(record.id);
  if (record.payloads) {
    removeSessionPayloads(record.payloads.sessions);
    if (record.queuePlacement) {
      await queueMoveDisk.run("clearQueuePlacement", record.payloads.queue, record.id, record.queuePlacement);
      noteQueuesChanged();
    }
  }
  if (record.kind === "group") {
    if (record.importingGroup) {
      await groupAction(readBridgeFile(), record.thing, "release", importReceipt(record));
    } else if (record.importingGroup === undefined && record.complete) {
      const existing = await askGroups(readBridgeFile(), "GET", `/${encodeURIComponent(record.thing)}`, undefined, engineWait());
      if (existing.status !== 404) throw new Error("The group copy has no import ownership receipt");
    }
    if (record.placed && !record.payloads) {
      await queueMoveDisk.run("forgetWorkQueues", [record.thing]);
      noteQueuesChanged();
    }
  } else {
    const created = new Set(record.createdSessions);
    if (record.lane) {
      // Only what this move placed, each agent's files too, and what those replaced back.
      if (fs.existsSync(placedOf(record.id))) await carry(readBridgeFile(), "undo", { keep: placedOf(record.id) });
    } else if (record.sessions.length && record.createdSessions === undefined && !record.importingSession) {
      // An old journal named what the source offered, even when the destination never created it.
      throw new Error("The conversation copy has no import ownership receipt");
    } else if (created.size || record.importingSession) {
      const engine = readEngineFile();
      if (record.importingSession) {
        const info = await sessionHere(engine, record.importingSession);
        if (isObject(info) && isObject(info.metadata) && info.metadata["kuvo.moveImport"] === importReceipt(record)) {
          created.add(record.importingSession);
          // Keep the proof even if deleting the session succeeds before the rest of cleanup does.
          changeMove(record.id, each => { each.createdSessions = [...created]; each.importingSession = undefined; });
        }
      }
      for (const session of [...created].reverse()) await deleteSession(engine, session);
    }
    for (const session of created) {
      forgetDecided(session);
      forgetQueue(session);
    }
    await forgetSessionNotes(record.sessions, importReceipt(record));
    if (created.size && !record.payloads) {
      await queueMoveDisk.run("forgetWorkQueues", [...created]);
      noteQueuesChanged();
    }
    const place = folderOf(record.kind, record.thing);
    // Only a copy this move placed, never one that lives here.
    if (place && record.placed && moveHold(pausedText(place)) === record.id) await toTrash(place);
  }
}

// ---- Group chats held ----

// A group chat a move held, to start again (taken back) as its group chats weren't answering: they're
// looked at again each minute until it is, and once as the box starts.
let groupsToWake = false;
let groupsLookedAt = 0;

async function activateGroup(group: string, activating?: string): Promise<boolean> {
  if (!mayActivate(moveReference("group", group), { activating, moves: liveMoves(readMoves(true)) })) return false;
  await groupAction(readBridgeFile(), group, "activate");
  return true;
}

/** A group chat a move held here going on again, as it was; tried again later when it can't now. */
async function wakeGroup(group: string): Promise<void> {
  try {
    await activateGroup(group);
  } catch (error) {
    // Not there any more: nothing to start.
    if ((error as { status?: number } | null)?.status !== 404) groupsToWake = true;
  }
}

/** Each group chat held here that no move under way holds, going on again. */
async function wakeHeldGroups(): Promise<void> {
  groupsToWake = false;
  let bridge: Target;
  try {
    bridge = readBridgeFile();
  } catch {
    groupsToWake = true;
    return;
  }
  const listed = await askGroups<{ data?: { group?: { id?: unknown }; status?: unknown }[] }>(bridge, "GET", "", undefined, engineWait()).catch(() => null);
  // (No group chats run here: none held.)
  if (listed?.status === 404) return;
  if (listed?.status !== 200 || !Array.isArray(listed.value?.data)) {
    groupsToWake = true;
    return;
  }
  for (const chat of listed.value.data) {
    const id = chat?.group?.id;
    // (Looked at as it's started, so one a move holds meanwhile stays held.)
    if (chat?.status === "held" && typeof id === "string") await wakeGroup(id).catch(() => { groupsToWake = true; });
  }
}

// ---- Mail between them ----

/** A move's mail from another box (`{"move": {op, id, …}}`). */
export async function takeMoveMail(sender: MailSender, given: unknown): Promise<void> {
  if (!isObject(given) || typeof given.op !== "string" || typeof given.id !== "string" || !/^mov_[A-Za-z0-9]{1,40}$/.test(given.id)) return;
  // Heard as it comes, not in its turn: one held a while by what this box is doing (settling a
  // conversation to send) doesn't make the other computer look quiet.
  const record = readMove(given.id);
  if (record && record.peer === sender.key && (given.op === "checked" || sameBundle(record, given))) {
    heard.set(given.id, performance.now());
    // Back: clients see it carry on at once.
    if (waitingFor.delete(given.id)) movesChanged();
  }
  // Parts and their answers come many at once, and need no turn of their own (nor does word that a
  // copy's still being set up).
  if (given.op === "part" || given.op === "got" || given.op === "working" || given.op === "arrive") return moveMail(sender, given, given.id);
  // Whether it can run there: asked and answered while the box it leaves waits, in its turn.
  if (given.op === "check") return checkHere(sender, given, given.id);
  if (given.op === "checked") {
    if (record?.role === "from" && record.peer === sender.key) checking.get(given.id)?.(given);
    return;
  }
  return inTurn(given.id, () => moveMail(sender, given, given.id as string));
}

async function moveMail(sender: MailSender, given: Record<string, unknown>, id: string): Promise<void> {
  const record = readMove(id);
  const mine = record && record.peer === sender.key ? record : null;
  const matched = mine && sameBundle(mine, given);
  switch (given.op) {
    // To the box it goes to.
    case "offer": return takeOffer(sender, given);
    case "part": return takePart(sender, given);
    case "switch": {
      if (record && !mine) return;
      if (mine?.activation) { if (matched) return completeActivation(id).catch(logActivation); return; }
      if (endedFor(id, sender.key)) return answerDecision(sender, id, given);
      if (given.safeSwitch !== true || mine?.role !== "to" || !matched || mine.generation === undefined) return;
      if (mine.complete && !mine.dropping && mine.setUpSince === undefined && (mine.sessions.length || mine.kind === "group")) {
        const last = mine.kind === "group" ? undefined : await newestMessage(mine.lane === true, mine.sessions[0]!).catch(() => undefined);
        const committed = changeMove(id, each => { each.activation = { at: Date.now(), ...(last ? { last } : {}) }; });
        if (committed) await completeActivation(id).catch(logActivation);
        return;
      }
      return letGoOf(sender, id, given);
    }
    // A switch from before bundle binding can ask what already happened without starting work.
    case "decision?":
      if (record && !matched) return;
      if (mine?.activation) return completeActivation(id).catch(logActivation);
      return answerDecision(sender, id, given);
    // Asked again whether its copy arrived (that word may have been lost): as it is here.
    case "arrive": {
      if (record && !matched) return;
      const identity = { generation: given.generation, hash: given.hash };
      const failed = () => tell(sender.key, { op: "failed", id, ...identity, message: `It couldn’t be set up on ${HERE}.` });
      if (mine?.role !== "to" || mine.dropping || !mine.complete) return void failed();
      if (mine.activation) return void tell(sender.key, { op: "working", id, ...identity });
      if (mine.setUpSince === undefined) {
        try {
          if (mine.payloads) {
            verifySessionPayloads(mine.payloads.sessions);
            await queueMoveDisk.run("verifyQueuePayloads", mine.payloads.queue);
          }
        } catch { return void failed(); }
        return tellArrived(mine, identity);
      }
      const settingUpBundle = toSetUp.get(id);
      if (settingUpBundle && sameBundle(mine, settingUpBundle)) return void tell(sender.key, { op: "working", id, ...identity });
      // Cut short by a restart (nothing sets it up now): what came of it goes.
      return inTurn(id, async () => {
        const current = readMove(id);
        if (current && sameBundle(current, given)) await drop(id);
      }).then(() => void failed(), () => void failed());
    }
    case "drop":
      if (record && !matched) return;
      if (mine?.activation) return completeActivation(id).catch(logActivation);
      if (mine?.role === "to") return letGoOf(sender, id, given, true);
      return answerDecision(sender, id, given);
    // To the box it leaves.
    case "ready":
      if (mine?.role !== "from" || mine.step !== "offer") return;
      if (given.safeSwitch !== true || (mine.generation !== undefined && given.generation === undefined)) return takeBack(id, say.needsUpdate(nameOf(sender.key)));
      if (!matched) return;
      if ((mine.verifyPayloads && given.verifyPayloads !== true) || (mine.typedAttachmentPaths && given.typedAttachmentPaths !== true)) return takeBack(id, say.needsUpdate(nameOf(sender.key)));
      changeMove(id, each => { each.safeSwitch = true; each.step = "parts"; each.sentAt = Date.now(); each.againAt = undefined; });
      const sending = outgoing.get(id);
      if (sending) sending.since = performance.now();
      return sendParts(readMove(id)!);
    case "got": {
      if (mine?.role !== "from" || !matched || mine.step !== "parts") return;
      const of = Math.ceil((mine.size ?? 0) / PART_BYTES);
      const i = given.i;
      if (typeof i !== "number" || !Number.isInteger(i) || i < 0 || i >= of) return;
      const state = outgoing.get(id);
      if (!state) return;
      // (One lost and then come after all counts too, as it's no longer on its way.)
      const fresh = !state.got.has(i);
      state.got.add(i);
      state.flying.delete(i);
      if (fresh) state.window = Math.min(IN_FLIGHT, (state.window ?? IN_FLIGHT) + 1);
      if (state.got.size === of) {
        const pending = changeMove(id, each => { each.sent = each.size; each.step = "arrive"; each.sentAt = Date.now(); });
        if (pending) markedRetry(pending);
        state.since = performance.now();
      } else {
        if (state.got.size % 32 === 0) changeMove(id, each => { each.sent = Math.min(each.size ?? 0, state.got.size * PART_BYTES); });
        return sendParts(mine);
      }
      return;
    }
    case "arrived":
      // (Of the bundle it sent last: one before it, sent again, is old news.)
      if (mine?.role !== "from" || !matched || mine.stage !== "copying" || mine.failed || mine.assigning) return;
      if (!mine.safeSwitch || (mine.verifyPayloads && given.verifiedPayloads !== true) ||
          (mine.typedAttachmentPaths && given.typedAttachmentPaths !== true)) return takeBack(id, say.needsUpdate(nameOf(sender.key)));
      return arrived(mine, given.digest).catch(error => refuseMove(id, error));
    case "refused":
    case "failed":
      // (One about a bundle before the one sent last, asked again late, is old news.)
      if (mine?.role !== "from" || !matched || mine.stage !== "copying" || mine.failed) return;
      return takeBack(id, typeof given.message === "string" ? given.message : "It couldn’t go there.", 409);
    case "switched":
      if (mine?.role !== "from" || !matched || mine.failed || (mine.stage !== "switching" && mine.stage !== "finishing")) return;
      if (mine.verifyPayloads && given.verifiedPayloads !== true) return;
      return finish(id, given.verifiedPayloads === true).catch(() => {});
    case "dropped":
      // Its copy there is gone before it was switched to: the original here goes on, as it was.
      if (mine?.role === "from" && matched && mine.stage === "switching" && !mine.failed && given.neverSwitched === true && (mine.twoComputers || !followedSetup(readLink())?.checkpointed)) {
        return failSource(mine, `${mine.name} couldn’t be set up there, so it stayed here. Try again.`, 409, false);
      }
      if (mine?.role === "from" && matched && mine.failed && mine.step === "drop" && readOutcome(id)) return void forgetMove(id);
      return;
    // Its copy there still being set up (`heard`, as it came, is all it's for).
    case "working":
      return;
    // Has what came from there run here yet?
    case "ran?": {
      if (typeof given.session !== "string" || !/^ses_[A-Za-z0-9]{1,60}$/.test(given.session)) return;
      const since = endedAt(id);
      // (Only one that came here, from that computer: a word of it is what removes what's there.)
      if (since === null || endedAs(id) !== "switched") return;
      const ran = await ranHereSince(given.lane === true, given.session, since, endedLast(id)).catch(() => null);
      // Gone from here (moved on, or deleted): it's not coming back through that way, so the time starts.
      if (ran !== null) tell(sender.key, { op: "ran", id, at: ran === "gone" ? Date.now() : ran });
      return;
    }
    case "ran":
      if (typeof given.at === "number") noteRan(id, sender.key, given.at);
      return;
  }
}

/** The copy here of a move the box it came from takes back: said `dropped` as soon as it's marked
 * so (it's never switched to after), so that box goes on with its original at once; what's here of
 * it goes now, or, held meanwhile, once its engine or group chats let it (`keepMoving`). */
async function letGoOf(sender: MailSender, id: string, request: Record<string, unknown>, explicit = false): Promise<void> {
  try {
    letGo(id, explicit);
  } catch {
    return;
  }
  answerDecision(sender, id, request);
  await drop(id).catch(() => {});
}

// ---- Keeping them going ----

// Each move's next step, once in a while: a step unanswered sent again; a first offer with no answer
// taken back (the other computer isn't there); one whose other computer was removed, ended. And your
// other computers told what moves here, and what one quiet a while said forgotten (`elsewhere.ts`).
/** Move folders no move here has, by when they were first seen so. */
const orphans = new Map<string, number>();
/** How long one's left first: longer than the bridge's packing or placing is waited for. */
const ORPHAN_AFTER = 2 * CARRY_WAIT;

export function keepMoving(now = Date.now(), ticks = performance.now()): void {
  const records = readMoves();
  if (ticks - lastDecisionSweep >= 3600_000) {
    lastDecisionSweep = ticks;
    compactDecisionHints(now);
  }
  noteWaiting(records, now, ticks);
  keepLeft(now);
  tellMoving(hostOf(), now);
  forgetQuiet(now);
  if (groupsToWake && ticks - groupsLookedAt > STEP_AGAIN) {
    groupsLookedAt = ticks;
    wakeHeldGroups().catch(() => {});
  }
  const link = readLink();
  const peers = new Set(peersOf(link).map(peer => peer.key));
  const removed = removalsOf(link);
  let names: string[] = [];
  try { names = fs.readdirSync(MOVES).filter(name => name.endsWith(".outcome.json")); } catch {}
  for (const name of names) {
    const file = path.join(MOVES, name);
    try {
      if (now - fs.statSync(file).mtimeMs <= OUTCOME_KEPT) continue;
      const id = name.slice(0, -".outcome.json".length);
      if (!/^mov_[A-Za-z0-9]+$/.test(id)) continue;
      readOutcome(id);
      fs.rmSync(file, { force: true });
    } catch (error) {
      if (error instanceof UnsupportedFormatVersion) console.error(error.message);
    }
  }
  // A move's folder no move here has any more (a step under way as it was taken back made it again):
  // gone once it's been seen so for longer than a step's given.
  let folders: string[] = [];
  try { folders = fs.readdirSync(MOVES, { withFileTypes: true }).filter(each => each.isDirectory() && each.name.startsWith("mov_")).map(each => each.name); } catch {}
  const known = new Set(records.map(record => record.id));
  for (const id of retryClocks.keys()) if (!known.has(id)) retryClocks.delete(id);
  for (const name of [...orphans.keys()]) if (!folders.includes(name) || known.has(name)) orphans.delete(name);
  for (const name of folders) {
    if (known.has(name)) continue;
    const since = orphans.get(name) ?? ticks;
    orphans.set(name, since);
    if (ticks - since > ORPHAN_AFTER) {
      try { fs.rmSync(bundleOf(name), { recursive: true, force: true }); } catch {}
      orphans.delete(name);
    }
  }
  for (const record of records) {
    const due = retryDue(record, now, ticks, record.removalOperation ? 3_000 : STEP_AGAIN);
    const sent = outgoing.get(record.id);
    if (record.role === "to" && record.activation) {
      if (due && !queues.has(record.id)) inTurn(record.id, () => completeActivation(record.id)).catch(logActivation);
      continue;
    }
    if (record.role === "from" && record.failed && (!readOutcome(record.id) || record.step !== "drop")) {
      if (!queues.has(record.id)) inTurn(record.id, () => settleFailure(record)).catch(() => {});
      continue;
    }
    if (record.role === "from" && record.stage === "finishing") {
      if (due && !queues.has(record.id)) inTurn(record.id, () => finish(record.id)).catch(() => {});
      continue;
    }
    if (record.role === "from" && record.removalOperation && !record.assigning && record.stage !== "switching" && record.stage !== "finishing" && !record.failed &&
      (link?.removalSource?.operation !== record.removalOperation || link?.removalSource?.moveTo !== record.to)) {
      if (!queues.has(record.id)) void inTurn(record.id, () => failSource(record, "The removal plan changed.", 409, true, true)).catch(console.error);
      continue;
    }
    if (!peers.has(record.peer) && !removed[record.peer]) {
      // Gone a while, not just left out as your computers are read again.
      const since = missing.get(record.id) ?? ticks;
      missing.set(record.id, since);
      if (ticks - since >= GONE_FOR && record.stage !== "switching" && !queues.has(record.id)) inTurn(record.id, () => endWithout(record.id)).catch(() => {});
      continue;
    }
    missing.delete(record.id);
    if (record.role === "to") {
      // A copy that couldn't all be let go of (its engine or group chats down): again, each minute.
      // Still not a week after it began: said in the log, once, and tried each hour (and as the box
      // starts).
      // It's never forgotten while any of it's here, so what's left never goes on as a second copy.
      if (record.dropping) {
        const late = now - (record.dropSince ?? record.started) > HELD_AT_MOST;
        if (retryDue(record, now, ticks, late ? DROP_AGAIN_LATE : STEP_AGAIN)) {
          const pending = changeMove(record.id, each => { each.sentAt = now; });
          if (pending) markedRetry(pending, ticks);
          inTurn(record.id, () => drop(record.id)).catch(error => {
            if (!late || loggedStuck.has(record.id)) return;
            loggedStuck.add(record.id);
            console.error(`kuvo-link: a copy of ${record.name} that came here couldn’t be let go of in a week, so it stays held: ${String((error as Error | null)?.message ?? error)}`);
          });
        }
        continue;
      }
      // Setting it up stuck (its engine not answering): given up on, which lets what came of it go
      // and says it failed, so the box it came from takes it back.
      // (One cut short by a restart was let go of as the box started, `resumeMoves`.)
      const setup = settingUp.get(record.id);
      if (setup) {
        if (ticks - setup.since > SET_UP_AT_MOST) setup.stop.abort(new NotTaken("It took too long to set up."));
      } else if (now - record.started > HELD_AT_MOST) inTurn(record.id, () => drop(record.id)).catch(() => {});
      continue;
    }
    // Getting it ready stuck (its engine or group chats not answering): taken back now, not in its
    // turn (which waits on what's stuck), so whoever asked hears.
    // (One noted by another process, `box route`, and not yet driven has no `holdSince`: counted
    // from when it began.)
    const holdingSince = record.holdSince ?? (!record.step || record.step === "check" ? record.started : undefined);
    const holding = driving.get(record.id);
    const heldFor = holding !== undefined ? ticks - holding : holdingSince === undefined ? 0 : now - holdingSince;
    if (record.stage === "holding" && heldFor > HOLDING_AT_MOST) {
      takeBack(record.id, say.slow(record.name), 409).catch(() => {});
      continue;
    }
    // One asked for through another process (`box route`): moved from here, from its check or its start.
    if (record.stage === "holding" && (!record.step || record.step === "check")) {
      drive(record.id);
      continue;
    }
    // Older sources sometimes left a switch with step "drop". Ask for the decision again.
    if (record.stage === "switching") {
      if (due) {
        const pending = changeMove(record.id, each => { each.sentAt = now; each.againAt = undefined; each.step = "switch"; });
        if (pending) {
          markedRetry(pending, ticks);
          if (pending.generation === undefined) tell(record.peer, { op: "decision?", id: record.id });
          else sendSwitch(pending);
        }
      }
      continue;
    }
    // Before the switch, quiet too long: taken back (one first offered gets 30 seconds, below).
    const sentAgo = sent ? ticks - sent.since : now - (record.sentAt ?? 0);
    const quiet = sentAgo > QUIET_AT_MOST && ticks - (heard.get(record.id) ?? -Infinity) > QUIET_AT_MOST;
    if (quiet && (record.step === "parts" || record.step === "arrive" || (record.step === "offer" && record.tries > 0))) {
      inTurn(record.id, () => takeBack(record.id, `${nameOf(record.peer)} stopped answering, so ${record.name} stayed here. Try again once it’s back.`, 503)).catch(() => {});
      continue;
    }
    // A failed transfer's staged copy is asked to go until the destination answers or times out.
    if (record.step === "drop" && !record.dropUntilAck && now - (record.dropSince ?? record.started) > DROP_AT_MOST) {
      forgetMove(record.id);
      continue;
    }
    switch (record.step) {
      case "offer":
        if (record.tries === 0 && sentAgo > NO_ANSWER && !sent?.got.size) {
          inTurn(record.id, () => takeBack(record.id, say.notConnected(nameOf(record.peer)), 503)).catch(() => {});
        } else if (due) {
          const pending = changeMove(record.id, each => { each.againAt = now; });
          if (pending) offer(pending, ticks);
        }
        break;
      // Its copy all there, and no word of it set up (that word lost, say): asked again.
      case "arrive":
        if (due) {
          const pending = changeMove(record.id, each => { each.againAt = now; });
          if (pending) markedRetry(pending, ticks);
          tell(record.peer, { op: "arrive", id: record.id, ...bundleIdentity(record) });
        }
        break;
      case "parts":
        if (sent) sendParts(record, ticks)?.catch(error => console.error("kuvo-link: could not record a move’s failure", error));
        else inTurn(record.id, () => takeBack(record.id, `${record.name} didn’t get there. Try again.`, 409)).catch(() => {});
        break;
      case "drop":
        if (due) {
          const pending = changeMove(record.id, each => { each.sentAt = now; });
          if (pending) markedRetry(pending, ticks);
          tell(record.peer, { op: "drop", id: record.id, ...bundleIdentity(record) });
        }
        break;
    }
  }
}

/** The moves here waiting on their other computer (`waitingFor`), whose other computer has said
 * nothing of them a while: from here, one that asked a step (its offer sent again, its parts, word
 * it's all there, or the switch); coming here, a copy whose
 * parts or switch don't come. Clients told when that changes. */
function noteWaiting(records: MoveRecord[], now: number, ticks: number): void {
  const waiting = records.filter(record => {
    if (record.failed) return false;
    const sent = outgoing.get(record.id);
    const quiet = (asked: number, delay: number) => {
      const oldEnough = sent ? ticks - sent.since > delay : asked > now || now - asked > delay;
      return oldEnough && ticks - Math.max(heard.get(record.id) ?? -Infinity, lookingSince) > delay;
    };
    if (record.role === "to") {
      // (Being let go of, or set up here now: this box is at it.)
      if (record.dropping || record.setUpSince !== undefined) return false;
      // Its parts, since it was offered; set up, the switch, since it said so (the other box gets
      // the original ready to go first, a minute at most).
      return record.complete
        ? quiet(record.setUpAt ?? record.started, WAITING_AFTER + SETTLE_FOR * 1000)
        : quiet(record.started, WAITING_AFTER);
    }
    const asked = record.step === "switch" || (record.step === "drop" && record.stage === "switching") ? record.switchSince
      : record.step === "parts" || record.step === "arrive" || (record.step === "offer" && record.tries > 0) ? record.sentAt
      : undefined;
    return asked !== undefined && quiet(asked, WAITING_AFTER);
  }).map(record => record.id);
  if (waiting.length === waitingFor.size && waiting.every(id => waitingFor.has(id))) return;
  waitingFor.clear();
  for (const id of waiting) waitingFor.add(id);
  movesChanged();
}

/** A removed peer can end a transfer only before ownership becomes uncertain or committed. */
async function endWithout(id: string): Promise<void> {
  missing.delete(id);
  const record = readMove(id);
  if (!record) return;
  if (record.role === "to") return drop(id);
  if (record.stage === "finishing") return finish(id);
  if (record.failed) return void forgetMove(id);
  if (record.stage === "switching") return;
  return failSource(record, say.notConnected(nameOf(record.peer)), 503, false);
}

/** As the box starts: moves cut short carry on (one switched, forward; one not yet, taken back), and
 * a `moving` hold nothing here moves (one a released Mac's move left as it updated) is lifted. */
export async function resumeMoves(now = Date.now()): Promise<void> {
  for (const record of readMoves()) {
    if (record.role === "to" && record.activation) {
      await inTurn(record.id, () => completeActivation(record.id)).catch(logActivation);
      continue;
    }
    if (record.role === "from" && record.failed) {
      await inTurn(record.id, () => settleFailure(record)).catch(() => {});
      continue;
    }
    // A copy being set up here as the box stopped: nothing sets it up now, so what came of it goes,
    // and the box it came from hears it failed (and keeps its original). One being let go of goes on.
    if (record.role === "to" && (record.dropping || (record.complete && record.setUpSince !== undefined))) {
      await inTurn(record.id, () => drop(record.id)).catch(() => {});
      if (!record.dropping) tell(record.peer, { op: "failed", id: record.id, ...bundleIdentity(record), message: `It couldn’t be set up on ${HERE}.` });
      continue;
    }
    if (record.role !== "from") continue;
    // Each in its turn, as its mail and `keepMoving` are.
    if (record.stage === "finishing") await inTurn(record.id, () => finish(record.id)).catch(() => {});
    // (One asked for just now, through another process, is moved from here.)
    else if (record.stage === "holding" && (!record.step || record.step === "check") && now - record.started < 30_000) continue;
    else if (record.assigning) switchSource(record);
    else if (!record.assigning && record.stage !== "switching") await inTurn(record.id, () => takeBack(record.id, `${record.name} didn’t get there. Try again.`, 409)).catch(() => {});
  }
  const moving = new Set(readMoves(true).map(record => record.thing));
  for (const kind of ["bot", "chat"] as const) {
    const parent = path.dirname(folderOf(kind, "x")!);
    let names: string[] = [];
    try { names = fs.readdirSync(parent); } catch {}
    for (const name of names) {
      const folder = path.join(parent, name);
      if (!moving.has(name)) {
        try {
          if (moveHold(pausedText(folder)) === "moving") fs.rmSync(PAUSED(folder), { force: true });
        } catch (error) { console.error("kuvo-link: couldn’t clear an orphaned move hold", error); }
      }
    }
  }
  // And a group chat held that no move here holds (one taken back as its group chats were down).
  groupsLookedAt = performance.now();
  await wakeHeldGroups().catch(() => {});
  movesChanged();
}

/** Moves go by the relay connection `host` gives (the one that's up), and carry on from where they
 * were as the box starts; each step not answered is sent again. */
export function watchMoves(host: () => Pick<LinkHost, "send"> | null | undefined, signal?: AbortSignal): void {
  hostOf = host;
  watching = true;
  lookingSince = performance.now();
  resumeMoves().catch(() => {});
  const timer = setInterval(() => {
    try { keepMoving(); } catch (error) { console.error("kuvo-link: move recovery will retry", error); }
  }, 3_000);
  signal?.addEventListener("abort", () => {
    clearInterval(timer);
    watching = false;
  }, { once: true });
}
