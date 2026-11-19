// Moves under way on this box, as each side keeps them (PROTOCOL.md, `kuvo/moves`): one file each in
// `run/moves`, so a move cut short by a restart finishes or is taken back; who's told as they change;
// and the conversations that don't live here, or moved away from here lately.

import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import type { Move, MoveKind, MoveStage } from "../../shared/routes/moves";
import { syncDurableFile, writeDurablePrivate, writePrivate } from "../files";
import { locked } from "../../shared/files";
import { RUN } from "../home";

export const MOVES = path.join(RUN, "moves");
// Committed departures from this computer. Their routing advertisements expire independently.
const MOVED_FILE = path.join(MOVES, "moved.json");
const DEPARTURE_LOCK = path.join(MOVES, ".departures.lock");
export const MOVED_FOR = 2 * 24 * 3600_000;

/** A move as this box keeps it: what clients see of it (`Move`), and what it needs to carry on. On
 * the box it's leaving (`from`): whether it was paused before, its folder here, and the bundle's
 * hash and size; tries made; when each step was last sent. On the box it's going to (`to`): the
 * folder its copy went in, and the parts of the bundle that came. */
export interface MoveRecord extends Move {
  wipePreparation?: true;
  /** The other box's key. */
  peer: string;
  wasPaused: boolean;
  /** The primary acknowledged this source’s ownership transfer. */
  ownershipCommitted?: true;
  /** The copy arrived and the primary is deciding ownership. */
  assigning?: true;
  /** The original pause was persisted before this source wrote its move marker. */
  pauseRecorded?: boolean;
  folder?: string;
  hash?: string;
  /** Increases each time the source packs a replacement under this move ID. */
  generation?: number;
  /** Where it is between the boxes: the source waits for `ready`, `arrived`, `switched`, `dropped`. */
  step?: "check" | "offer" | "parts" | "arrive" | "switch" | "drop" | "done";
  tries: number;
  /** The box it goes to: all its parts came, and it's being (or was) put in place; parts that come
   * after are ignored. */
  complete?: boolean;
  /** What its folder's `paused` said before it moved (your own pause), to put back. */
  pausedAs?: string;
  /** The box it goes to: its folder was put in place by this move (and goes if it's taken back). */
  placed?: boolean;
  /** Destination sessions this move imported, and the one whose import reply may be missing. */
  createdSessions?: string[];
  importingSession?: string;
  importingGroup?: boolean;
  /** A group chat's bots, which the box it goes to checks it has. */
  members?: string[];
  /** Offered to the other box at least once: a copy may be there, to drop if it's taken back. */
  offered?: boolean;
  sentAt?: number;
  /** The box it leaves: when an offer or an ask whether it arrived was last sent again (quiet is
   * counted from `sentAt`, the step's first, and what was last heard). */
  againAt?: number;
  /** The box it leaves: its conversation kept in its folder as the original goes (a try at that cut
   * short doesn't keep it again, with less of it). */
  kept?: boolean;
  /** Both peers speak the durable switch protocol for this incoming copy. */
  safeSwitch?: boolean;
  /** The destination committed ownership. Activation must finish before it acknowledges. */
  activation?: { at: number; last?: string };
  /** The source has reported success and has local cleanup outstanding. */
  cleanupPending?: boolean;
  /** The box it goes to: its conversations whose queue the move brought going on where it came
   * from (not paused), to go on here once it's switched to. */
  queuesGoOn?: string[];
  /** The box it goes to: its copy is being let go of (it won't be switched to), which is tried again
   * until all of it's gone (its engine or group chats were down). */
  dropping?: boolean;
  /** The box it leaves: since when it's been getting it ready to go (held, settled, packed). */
  holdSince?: number;
  /** The box it goes to: since when it's been setting its copy up. */
  setUpSince?: number;
  /** The box it goes to: when its copy was set up (and said so), to wait for the switch from. */
  setUpAt?: number;
  /** Since when the box it leaves has asked the other to switch to its copy. */
  switchSince?: number;
  /** Since when the box it leaves has asked the other to let its copy go (taken back); on the box it
   * goes to, since when its copy's been let go of. */
  dropSince?: number;
  /** A checkpointed destination holds until it hears the terminal drop. */
  dropUntilAck?: true;
  started: number;
  /** It talks with a coding agent: its conversations are on the agents bridge, not the engine, and
   * each agent's own go with them (`carry.ts`). */
  lane?: boolean;
  /** The box it leaves: where each agent kept what it carried, to clear later (`left.ts`). */
  leftOf?: unknown[];
  /** The box it goes to: the hash of what's in place, said back as it arrives. */
  digest?: string;
  /** The box it goes to: where it was, to tell it as it's switched to. */
  was?: { place: string; folder?: string };
  /** The model it talks with, for the box it goes to to check (`check`). */
  model?: { providerID: string; id: string };
  /** What the box it goes to said it does differently there (another account of its coding agent). */
  note?: string;
  /** Taken back, with why (said to whoever asked). */
  failed?: string;
  /** The status a failure answers: 409 (it can't, now) or 503 (the other computer can't be reached). */
  failedStatus?: number;
}

const journalFormat = additiveObjectFormat("Move journal");
const decisionFormat = additiveObjectFormat("Move ownership receipt");
const hintFormat = additiveObjectFormat("Move routing hint");
const movedAwayFormat = additiveObjectFormat("Moved conversation");

const fileOf = (id: string) => path.join(MOVES, `${id}.json`);
export const bundleOf = (id: string) => path.join(MOVES, id);

const fits = (record: unknown): record is MoveRecord => isObject(record) && typeof record.id === "string" && typeof record.thing === "string"
  && typeof record.peer === "string" && (record.role === "from" || record.role === "to") && Array.isArray(record.sessions);

export function readMoves(strict = false): MoveRecord[] {
  let names: string[] = [];
  try { names = fs.readdirSync(MOVES).filter(name => /^mov_[A-Za-z0-9]+\.json$/.test(name)); } catch (error) {
    if (strict && (error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const records: MoveRecord[] = [];
  for (const name of names) {
    try {
      const value: unknown = JSON.parse(fs.readFileSync(path.join(MOVES, name), "utf8"));
      const record = isObject(value) ? journalFormat.read(value) : null;
      if (fits(record)) records.push(record);
      else if (strict) throw new Error("An ownership record could not be read");
    } catch (error) {
      if (error instanceof UnsupportedFormatVersion || (strict && (error as NodeJS.ErrnoException).code !== "ENOENT")) throw error;
    }
  }
  return records.sort((one, other) => one.started - other.started);
}

export const readMove = (id: string): MoveRecord | null => readMoves().find(record => record.id === id) ?? null;

/** The moves under way: not one taken back, whose copy there is still being let go (`failed`):
 * what it moved is free here again. */
export const liveMoves = (records = readMoves()): MoveRecord[] => records.filter(record => !record.failed);

/** The moves under way that clients see: not a copy here being let go of (the move was taken back,
 * and goes on where it came from), whose conversations alone are left out (`away`) until it's gone. */
export const listedMoves = (records = liveMoves()): MoveRecord[] => records.filter(record => !(record.role === "to" && record.dropping) && !(record.role === "from" && record.stage === "finishing"));

/** The move of `thing` under way here, either way. */
export const moveOf = (thing: string): MoveRecord | null => liveMoves().find(record => record.thing === thing) ?? null;

// Told each time a move changes (`kuvo/moves/events`).
export const moveFollowers = new Set<() => void>();
export function movesChanged(): void {
  for (const follower of [...moveFollowers]) try { follower(); } catch {}
}

function checkJournal(id: string): void {
  const value = jsonIfPresent(fileOf(id));
  if (isObject(value)) journalFormat.read(value);
}

export function saveMove(record: MoveRecord): void {
  checkJournal(record.id);
  writeDurablePrivate(fileOf(record.id), JSON.stringify(journalFormat.write(record)), RUN);
  movesChanged();
}

/** Gone, with what came of its bundle. */
export function forgetMove(id: string): void {
  checkJournal(id);
  try { fs.rmSync(fileOf(id), { force: true }); } catch {}
  try { fs.rmSync(bundleOf(id), { recursive: true, force: true }); } catch {}
  movesChanged();
}

export function changeMove(id: string, change: (record: MoveRecord) => void): MoveRecord | null {
  const record = readMove(id);
  if (!record) return null;
  change(record);
  saveMove(record);
  return record;
}

/** What clients see of it. */
export function shownMove(record: MoveRecord): Move {
  const { id, kind, thing, name, from, to, role, stage, sessions, sent, size } = record;
  return { id, kind, thing, name, from, to, role, stage, sessions, ...(sent !== undefined ? { sent } : {}), ...(size !== undefined ? { size } : {}), ...(waitingFor.has(id) ? { waiting: true } : {}) };
}

/** The moves here whose other computer has gone quiet about them a while (`keepMoving` keeps
 * it): shown as waiting for it. In memory only: a box started again waits a while before it says so. */
export const waitingFor = new Set<string>();

/** The conversations this box's engine has that don't live here: a copy that came, until the box it
 * left switches to it; an original that's switched away, until it's gone. */
export function awaySessions(records = liveMoves()): string[] {
  const away = new Set<string>();
  for (const record of records) {
    const switched = record.stage === "switching" || record.stage === "finishing";
    if ((record.role === "to" && !switched) || (record.role === "from" && switched)) for (const session of record.sessions) away.add(session);
  }
  return [...away];
}

/** The conversations held while they move (nothing reaches them: prompts wait, letters come
 * again later), and the bots and group chats. */
export function frozen(records = liveMoves()): Set<string> {
  // (Not while the box it goes to is only asked whether it can run it: nothing's held yet.)
  return new Set(records.filter(record => record.step !== "check").flatMap(record => [record.thing, ...record.sessions]));
}

/** The box a conversation moving from here is switched to (a box's key), while its original here is
 * still being let go; null for none. Its copy there is held until it's switched to, so whatever
 * comes from it there comes from it as it runs there now. */
export function switchedTo(session: string, records = liveMoves()): string | null {
  return switchedMove(session, records)?.peer ?? null;
}

/** The move switching `session` from here to another box (`switchedTo`), or null. */
export function switchedMove(session: string, records = liveMoves()): MoveRecord | null {
  return records.find(each => each.role === "from" && (each.stage === "switching" || each.stage === "finishing") && each.sessions.includes(session)) ?? null;
}

/** A record moved away from here and where ownership went. */
export interface MovedAway {
  session: string;
  key: string;
  at: number;
}

export function committedDepartures(): MovedAway[] {
  try {
    const kept: unknown = JSON.parse(fs.readFileSync(MOVED_FILE, "utf8"));
    if (!Array.isArray(kept)) return [];
    return kept.flatMap((value): MovedAway[] => {
      if (!isObject(value)) return [];
      const { session, key, at } = movedAwayFormat.read(value);
      return typeof session === "string" && typeof key === "string" && typeof at === "number"
        ? [{ session, key, at }] : [];
    });
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return [];
  }
}

/** Only recent session changes need to be advertised. The activation fence remains. */
export function movedAway(now = Date.now()): MovedAway[] {
  return committedDepartures().filter(each => each.session.startsWith("ses_") && now - each.at < MOVED_FOR);
}

export function noteMovedAway(sessions: readonly string[], key: string, now = Date.now()): void {
  const updated = locked(DEPARTURE_LOCK, () => {
    const current = committedDepartures();
    const changed = new Set(sessions.filter(session => !current.some(each => each.session === session && each.key === key)));
    if (!changed.size) return false;
    const kept = current.filter(each => !changed.has(each.session));
    for (const session of changed) kept.push({ session, key, at: now });
    writeDurablePrivate(MOVED_FILE, JSON.stringify(kept.map(movedAwayFormat.write)), RUN);
    return true;
  }, { required: true });
  if (updated) movesChanged();
}

/** Moved here again (back from where it went): no longer away. */
export function forgetMovedAway(sessions: readonly string[]): void {
  const updated = locked(DEPARTURE_LOCK, () => {
    const kept = committedDepartures();
    if (!kept.some(each => sessions.includes(each.session))) return false;
    writeDurablePrivate(MOVED_FILE, JSON.stringify(kept.filter(each => !sessions.includes(each.session)).map(movedAwayFormat.write)), RUN);
    return true;
  }, { required: true });
  if (updated) movesChanged();
}

export type { Move, MoveKind, MoveStage };

// A durable receipt per move. Offline sources may ask about these decisions at any time.
// Session routing hints expire independently and are the only records we compact.
const LEGACY_DECISIONS = path.join(MOVES, "ended.json");
const DECISIONS = path.join(MOVES, "decisions");
const HINTS = path.join(MOVES, "decision-hints");
const DECISION_ID = /^mov_[A-Za-z0-9]{1,40}$/;
export type BundleIdentity = { generation?: number; hash?: string };
type Decision = BundleIdentity & { as: "switched" | "dropped"; at: number; last?: string; peer?: string };
type Hint = Pick<Decision, "as" | "at"> & { sessions?: string[] };
const isDecision = (one: unknown): one is Decision => isObject(one) && typeof one.at === "number" &&
  (one.as === "switched" || one.as === "dropped");

function jsonIfPresent(file: string): unknown {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
}

const fileForDecision = (id: string) => path.join(DECISIONS, `${id}.json`);

function decision(id: string): { value: Decision; file: string } | null {
  if (!DECISION_ID.test(id)) return null;
  const file = fileForDecision(id);
  const value = jsonIfPresent(file);
  if (value !== undefined) {
    const one = decisionFormat.read(value);
    if (!isDecision(one)) throw new Error("A move’s ownership receipt could not be read");
    return { value: one, file };
  }
  const legacy = jsonIfPresent(LEGACY_DECISIONS);
  if (legacy !== undefined && !isObject(legacy)) throw new Error("Legacy move decisions could not be read");
  const valueBefore = legacy?.[id];
  if (valueBefore === undefined) return null;
  const old = decisionFormat.read(valueBefore);
  if (!isDecision(old)) throw new Error("A legacy move decision could not be read");
  return { value: old, file: LEGACY_DECISIONS };
}

export function endedFor(id: string, peer: string): Decision | null {
  const one = decision(id)?.value;
  return one && (one.peer === undefined || one.peer === peer) ? one : null;
}

export const endedAs = (id: string): Decision["as"] | null => decision(id)?.value.as ?? null;
export const endedAt = (id: string): number | null => decision(id)?.value.at ?? null;
export const endedLast = (id: string): string | undefined => decision(id)?.value.last;

/** Record a switch or drop before its acknowledgement can go out. */
export function noteEnded(id: string, as: Decision["as"], now = Date.now(), sessions: readonly string[] = [], last?: string, peer?: string, bundle?: BundleIdentity): void {
  if (!DECISION_ID.test(id)) throw new Error("That isn’t a move ID");
  const before = decision(id);
  if (before) {
    if (before.value.as !== as || before.value.generation !== bundle?.generation || before.value.hash !== bundle?.hash) {
      throw new Error("A move’s ownership decision cannot change");
    }
    syncDurableFile(before.file, RUN);
    return;
  }
  const hintFile = path.join(HINTS, `${id}.json`);
  if (sessions.length) {
    const hint = jsonIfPresent(hintFile);
    if (hint !== undefined) hintFormat.read(hint);
    writeDurablePrivate(hintFile, JSON.stringify(hintFormat.write({ as, at: now, sessions })), RUN);
  }
  writeDurablePrivate(fileForDecision(id), JSON.stringify(decisionFormat.write({ as, at: now, ...bundle, ...(last ? { last } : {}), ...(peer ? { peer } : {}) })), RUN);
}

/** A rename can be visible after its directory sync failed. Seal the receipt before replying. */
export function sealDecision(id: string): void {
  const receipt = decision(id);
  if (!receipt) throw new Error("A move’s ownership receipt went missing");
  syncDurableFile(receipt.file, RUN);
}

function hintFiles(): string[] {
  try { return fs.readdirSync(HINTS).filter(name => name.endsWith(".json") && DECISION_ID.test(name.slice(0, -5))); }
  catch { return []; }
}

export function compactDecisionHints(now = Date.now()): void {
  for (const name of hintFiles()) {
    const file = path.join(HINTS, name);
    try {
      const value = jsonIfPresent(file);
      const hint = value === undefined ? null : hintFormat.read(value);
      if (isDecision(hint) && now - hint.at >= MOVED_FOR) fs.rmSync(file, { force: true });
    } catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error;
    } // Ownership receipts are independent of these routing hints.
  }
}

function sessionsFrom(hint: unknown, as: Decision["as"], now: number): string[] {
  if (!isDecision(hint) || hint.as !== as || now - hint.at >= MOVED_FOR) return [];
  const { sessions } = hint as Hint;
  return Array.isArray(sessions) ? sessions.filter(each => typeof each === "string") : [];
}

function sessionsDecided(as: Decision["as"], now: number): string[] {
  const sessions: string[] = [];
  for (const name of hintFiles()) {
    try {
      const value = jsonIfPresent(path.join(HINTS, name));
      const hinted = value === undefined ? [] : sessionsFrom(hintFormat.read(value), as, now);
      // An interrupted write can leave a hint for an ownership decision not yet recorded.
      if (hinted.length && decision(name.slice(0, -5))?.value.as === as) sessions.push(...hinted);
    } catch (error) {
      if (error instanceof UnsupportedFormatVersion) throw error;
    }
  }
  try {
    const legacy = jsonIfPresent(LEGACY_DECISIONS);
    if (isObject(legacy)) for (const hint of Object.values(legacy)) sessions.push(...sessionsFrom(decisionFormat.read(hint), as, now));
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
  }
  return sessions;
}

/** Where copy sessions were dropped or switched in the last two days, for peer routing. */
export const droppedHere = (now = Date.now()): string[] => sessionsDecided("dropped", now);
export const switchedHere = (now = Date.now()): Set<string> => new Set(sessionsDecided("switched", now));
