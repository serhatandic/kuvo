import { acquireEngine, engineManaged } from "../engine/lifetime";
import { queuedRefusal } from "../server/context";
// Wake-ups and watches (PROTOCOL.md, `kuvo/wakes`): a conversation's or a bot's, kept beside it as
// `kuvo-wake` keeps them, listed, added, held, cancelled and sent now from any client.

import { randomBytes } from "node:crypto";
import { execFile } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import type { BridgeRecord, EngineRecord, Target } from "../../shared/records";
import type { Wake, WakeList, Wakes, Watch } from "../../shared/routes/wakes";
import { wakeSummary } from "../../shared/message-words";
import { wakeMark } from "../../shared/queues";
import { oneLine } from "../../shared/text";
import { sleep } from "../../shared/wait";
import { awaySessions, moveFollowers, MOVES } from "../moves/records";
import { isMoving, MOVING } from "../bots/here";
import { engineGet } from "../engine/client";
import type { EngineAnswer } from "../engine/client";
import { keepWaitingLater } from "../engine/queue";
import { messageID } from "../engine/sessions";
import type { EngineSession } from "../engine/sessions";
import { listOf, textOf } from "../files";
import { watchFolder, type SharedWatch } from "../../shared/watch";
import { watchBotSchedules } from "../bots/watch";
import { followed, followersOf, watchFolders } from "../follow";
import { stopIfWaiting } from "../work/answers";
import { BOTS, OWN_RUNTIME, RUN, SCRIPT, SESSION_WAKES, WAKE_COMMAND } from "../home";
import type { AnswerPiece, LinkRequest, RouteContext } from "../server/context";
import { agentsAway, agentsLine, bridgeBack } from "./agents-down";

// What a wake-up's message starts with, as the Mac words it.
export const WAKE_MARKER = "[Wake-up] ";

// How often a wake-up comes back, as its name says (the app's Wakes.Repeat): 30m, 2h, 1d, 1w or
// weekdays, within a year; null for anything else.
export function repeatText(text: string | null | undefined): string | null {
  if (text === "weekdays") return text;
  const match = /^([1-9][0-9]{0,5})([mhdw])$/.exec(text ?? "");
  if (!match) return null;
  const count = Number(match[1]);
  const fits = { m: count >= 5 && count <= 525_600, h: count <= 8_760, d: count <= 366, w: count <= 52 }[match[2] as "m" | "h" | "d" | "w"];
  return fits ? text! : null;
}

/** A wake-up as its file names it: its file's name (`id`), when it's due, its note, how often it
 * comes back (null for once), and the letters that name it for good. */
export interface PendingWake {
  id: string;
  due: number;
  note: string;
  every: string | null;
  letters: string;
  summary?: string;
  cancelKey?: string;
}

// A wake-up from its file's name (the app's Wakes.Pending): when it's due, in seconds since 1970,
// the letters that name it for good, and how often it comes back; null for another file.
export function wakeOf(name: string, note = ""): PendingWake | null {
  const [due, letters = "", ...more] = name.split("-");
  if (!/^[0-9]+(\.[0-9]+)?$/.test(due ?? "")) return null;
  const repeating = more.join("-");
  return { id: name, due: Number(due), note, every: repeating ? repeatText(repeating.split("@")[0]) : null, letters };
}

/**
 * A conversation archived (or brought back) from a phone: its wake-ups wait while it's archived,
 * as WakeUps.setPaused has a Mac hold them, and come back with it. A hold of your own (`paused`)
 * stays; a move's (`moving`) becomes archiving's.
 */
export function holdArchived(session: string, archived: boolean): void {
  if (!/^ses_[A-Za-z0-9]+$/.test(session)) return;
  const folder = path.join(SESSION_WAKES, session);
  const file = path.join(folder, "paused");
  const held = textOf(file);
  if (archived) {
    if (!fs.existsSync(folder) || (held !== null && held !== "moving")) return;
    fs.writeFileSync(file, "archived");
  } else if (held === "archived" || held === "") {
    fs.rmSync(file, { force: true });
  }
}

// The folder keeping a session's wake-ups: its bot's, or its own in `wakes`; null for none.
export function wakeFolder(session: string): string | null {
  if (!/^[A-Za-z0-9_-]{1,100}$/.test(session)) return null;
  if (awaySessions().includes(session)) return null;
  for (const id of listOf(BOTS)) {
    if ((textOf(path.join(BOTS, id, "session")) ?? "").trim() === session) return path.join(BOTS, id);
  }
  const own = path.join(SESSION_WAKES, session);
  return textOf(path.join(own, "session")) === null ? null : own;
}

// Each session here with a wake-up or a watch, as the Mac lists them in `kuvo/wakes`: its soonest
// wake-up, whether it watches, and its wake-ups, watches and pause whole. Bots' come with `kuvo/bots`.
export function listWakes(): WakeList["sessions"] {
  const away = new Set(awaySessions());
  const sessions: WakeList["sessions"][number][] = [];
  for (const name of listOf(SESSION_WAKES)) {
    const folder = path.join(SESSION_WAKES, name);
    const session = (textOf(path.join(folder, "session")) ?? "").trim();
    if (!session || away.has(session)) continue;
    const { wakes, watches, paused } = readWakes(folder);
    if (!wakes.length && !watches.length) continue;
    sessions.push({ session, ...(wakes.length ? { next: wakes[0].due } : {}), watching: watches.length > 0, wakes: wakes.map(listedWake), watches, paused });
  }
  return sessions;
}

/** A folder's wake-ups as `readWakes` reads them: each as its file names it, the watches, and the pause. */
export interface FolderWakes {
  wakes: PendingWake[];
  watches: Watch[];
  paused: boolean;
}

// A folder's waiting wake-ups (soonest first), its watches, and whether you paused them: their
// notes and checks on a line each, or `whole`.
export function readWakes(folder: string, { whole = false }: { whole?: boolean } = {}): FolderWakes {
  const line = (text: string, limit: number) => whole ? text : oneLine(text, limit);
  const wakes = (listOf(path.join(folder, "wakes"))
    .map(name => {
      const wake = wakeOf(name, textOf(path.join(folder, "wakes", name)) ?? "");
      if (!wake) return null;
      const cancelKey = LETTERS.test(wake.letters) ? wake.letters : undefined;
      const summary = wakeSummary(wake.note, cancelKey ? textOf(path.join(folder, "summaries", cancelKey)) : null);
      return { ...wake, note: line(wake.note, 400), summary, cancelKey };
    })
    .filter(Boolean) as PendingWake[])
    .sort((a, b) => a.due - b.due);
  const watches = listOf(path.join(folder, "watches")).sort().map((letters): Watch | null => {
    const at = path.join(folder, "watches", letters);
    const check = textOf(path.join(at, "check")) ?? "";
    if (!check) return null;
    const note = textOf(path.join(at, "note")) ?? "";
    const next = (textOf(path.join(at, "next")) ?? "").trim();
    const due = /^[0-9]+$/.test(next) ? Number(next) : 0;
    return { letters, every: (textOf(path.join(at, "every")) ?? "").trim(), check: line(check, 300), note: line(note, 400),
      summary: wakeSummary(note, textOf(path.join(at, "summary"))), ...(due > 0 && Number.isFinite(due) ? { next: due } : {}) };
  }).filter(Boolean) as Watch[];
  return { wakes, watches, paused: fs.existsSync(path.join(folder, "paused")) };
}

// A wake-up as a client takes it.
export const listedWake = ({ id, due, note, every, summary, cancelKey }: PendingWake): Wake => ({ id, due, note, summary, cancelKey, every: every ?? undefined });

/** A conversation's wake-ups, watches and pause, with its session, as `kuvo/wakes/events` tells them. */
export interface SessionWakes extends Wakes {
  session: string;
  next?: number;
  watching: boolean;
}

// Every conversation here with wake-ups, watches or a pause, a bot's or its own, whole: what a
// client keeps of them, followed (`kuvo/wakes/events`).
export function allWakes(): SessionWakes[] {
  const away = new Set(awaySessions());
  const sessions: SessionWakes[] = [];
  const add = (folder: string) => {
    const session = (textOf(path.join(folder, "session")) ?? "").trim();
    if (!session || away.has(session)) return;
    const { wakes, watches, paused } = readWakes(folder, { whole: true });
    // (Its soonest and whether it watches too, as `kuvo/wakes` says them: the route's shape.)
    if (wakes.length || watches.length || paused) sessions.push({ session, ...(wakes.length ? { next: wakes[0].due } : {}), watching: watches.length > 0, wakes: wakes.map(listedWake), watches, paused });
  };
  for (const id of listOf(BOTS)) add(path.join(BOTS, id));
  for (const name of listOf(SESSION_WAKES)) add(path.join(SESSION_WAKES, name));
  return sessions;
}

/** Which worker a due wake or a watch needs for this round. */
function wakeDemand(now: number): "engine" | "bridge" | null {
  let bridge = false;
  const check = (folder: string) => {
    if (fs.existsSync(path.join(folder, "paused")) || textOf(path.join(folder, "session")) === null) return false;
    const retry = Number((textOf(path.join(folder, ".retry")) ?? "").trim().split(/\s+/)[0]);
    if (retry > now) return false;
    const due = listOf(path.join(folder, "watches")).length > 0 ||
      listOf(path.join(folder, "wakes")).some(name => (wakeOf(name)?.due ?? Infinity) <= now);
    if (!due) return false;
    if (textOf(path.join(folder, "via"))?.trim() !== "bridge") return true;
    bridge = true;
    return false;
  };
  return [BOTS, SESSION_WAKES].some(root => listOf(root).some(name => check(path.join(root, name))))
    ? "engine" : bridge ? "bridge" : null;
}

// When `kuvo-wake fire` next has something to send: the earliest unpaused wake, including retries.
export function nextWake(): number | null {
  let soonest: number | null = null;
  const look = (folder: string) => {
    if (fs.existsSync(path.join(folder, "paused")) || textOf(path.join(folder, "session")) === null) return;
    const [retry = 0] = (textOf(path.join(folder, ".retry")) ?? "").trim().split(/\s+/).map(Number);
    for (const name of listOf(path.join(folder, "wakes"))) {
      const due = wakeOf(name)?.due;
      if (due === undefined) continue;
      const at = Math.max(due, Number.isFinite(retry) ? retry : 0);
      if (soonest === null || at < soonest) soonest = at;
    }
  };
  for (const id of listOf(BOTS)) look(path.join(BOTS, id));
  for (const name of listOf(SESSION_WAKES)) look(path.join(SESSION_WAKES, name));
  return soonest;
}

interface WakeSchedule {
  watch?: typeof watchSchedules;
  schedule?: (run: () => void, ms: number) => unknown;
  cancel?: (timer: unknown) => void;
}

/** This computer's wake-ups and watches sent by its box, as a machine's keeper sends its own with
 * the same `kuvo-wake fire` (which keeps two runs apart itself): at the soonest one's time, at
 * least each minute (for watches), and again as they change, here or by an agent's kuvo-wake. One
 * still due after a run that was due before it (it couldn't go, or its conversation is archived)
 * waits for the next minute; one that came due during it goes right after. The returned function
 * stops it: nothing runs after, and its promise settles once a run already going is over. */
export function sendWakes({
  schedule: later = setTimeout,
  cancel = clearTimeout as (timer: unknown) => void,
  watch = watchSchedules,
}: WakeSchedule = {}): () => Promise<void> {
  let timer: unknown, settling: unknown;
  let running = false, again = false, stopped = false, started = 0, said = false, over: (() => void) | null = null, ending: Promise<void> | null = null;
  const schedule = (afterFire = false) => {
    if (stopped) return;
    cancel(timer);
    const due = nextWake(), now = Date.now() / 1000;
    const missed = afterFire && due !== null && due <= now && due <= started;
    const wait = due === null || missed ? 60_000 : Math.min(60_000, Math.max(500, (due + 1 - now) * 1000));
    timer = later(fire, wait);
  };
  const fire = async () => {
    if (stopped) return;
    if (running) return void (again = true);
    running = true;
    started = Date.now() / 1000;
    // (With this box's own command, which a wake-up due in a conversation you stopped is handed to,
    // to wait there: `box route`.)
    const env = { ...process.env, KUVO_LINK_RUNTIME: process.execPath, KUVO_LINK_FLAGS: OWN_RUNTIME.join(" "), KUVO_LINK_SCRIPT: SCRIPT };
    const demand = engineManaged() ? wakeDemand(started) : "bridge";
    if (!demand) {
      running = false;
      schedule(true);
      return;
    }
    const release = demand === "engine" ? await acquireEngine() : undefined;
    execFile(WAKE_COMMAND, ["fire", BOTS, path.join(RUN, "engine.json")], { env }, error => {
      release?.();
      running = false;
      if (error?.code === "ENOENT" && !said) {
        said = true;
        console.error(`Wake-ups can't be sent: there's no kuvo-wake at ${WAKE_COMMAND}.`);
      }
      if (stopped) {
        over?.();
      } else if (again) {
        again = false;
        fire();
      } else {
        schedule(true);
      }
    });
  };
  // A change, here or beside it, once the folders settle (a wake-up is a file or two).
  const changed = () => {
    cancel(settling);
    settling = later(schedule, 200);
  };
  wakeFollowers.add(changed);
  const watching = watch(changed);
  schedule();
  return () => {
    stopped = true;
    cancel(timer);
    cancel(settling);
    wakeFollowers.delete(changed);
    watching.close();
    if (!running) return Promise.resolve();
    return ending ??= new Promise<void>(resolve => { over = resolve; });
  };
}

// Those following them (`kuvo/wakes/events`), told of each change made here; `kuvo-wake fire`'s
// as the system sees their folders change.
export const wakeFollowers = new Set<() => void>();
export const wakesChanged = (session?: string) => {
  schedules?.changed(session);
  for (const tell of wakeFollowers) tell();
};

const LETTERS = /^[A-Za-z0-9]+$/;
// The most wake-ups a conversation keeps waiting, as the Mac's (Wakes.limit).
const WAKE_LIMIT = 50;

// A folder for a session's wake-ups here, made as kuvo-wake makes one with a session's first wake-up: its
// session, its folder, whether the coding agents run it, and held while it's archived. Null when
// it isn't a conversation here.
async function wakeFolderFor(session: string, readEngine: () => EngineRecord, bridge: () => BridgeRecord | null, beforeWrite?: () => boolean): Promise<string | null> {
  const where = await whereIs(session, readEngine, bridge);
  if (!where || beforeWrite?.() === false) return null;
  const { found, agents } = where;
  const folder = path.join(SESSION_WAKES, session);
  fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
  fs.writeFileSync(path.join(folder, "session"), session);
  fs.writeFileSync(path.join(folder, "directory"), found.location?.directory ?? found.directory ?? "");
  if (agents) fs.writeFileSync(path.join(folder, "via"), "bridge");
  if (found.metadata?.["kuvo.archived"] === true) fs.writeFileSync(path.join(folder, "paused"), "archived");
  return folder;
}

// The session as the engine or the agents' bridge has it (whole), and which: null for neither.
async function whereIs(session: string, readEngine: () => EngineRecord, bridge: () => BridgeRecord | null): Promise<{ target: Target; found: EngineSession; agents: boolean } | null> {
  if (!/^ses_[A-Za-z0-9]+$/.test(session)) return null;
  const ask = async (target: Target | null): Promise<EngineSession | null> => target ? (await engineGet(target, `api/session/${encodeURIComponent(session)}`).catch(() => null) as EngineAnswer<EngineSession> | null)?.data ?? null : null;
  let engine: EngineRecord | null = null;
  try { engine = readEngine(); } catch {}
  const here = await ask(engine);
  if (engine && here) return { target: engine, found: here, agents: false };
  const agents = bridge();
  const there = await ask(agents);
  return agents && there ? { target: agents, found: there, agents: true } : null;
}

/** Seed a routine before a new bot becomes visible to the wake worker. The keeper fills in the
 * local time of day and weekly anchor when it next advances this standard wake file. */
export function seedRoutine(folder: string, routine: { every: "1d" | "1w"; title: string; note: string }, now = Date.now()): void {
  const due = Math.floor(now / 1000) + (routine.every === "1w" ? 7 : 1) * 86_400;
  const letters = randomBytes(8).toString("hex");
  const summaries = path.join(folder, "summaries");
  fs.mkdirSync(summaries, { recursive: true, mode: 0o700 });
  fs.writeFileSync(path.join(summaries, letters), routine.title, { mode: 0o600 });
  fs.writeFileSync(path.join(folder, "wakes", `${due}-${letters}-${routine.every}`), routine.note, { mode: 0o600 });
}

// Takes a wake-up off the schedule by its letters, which a repeating one keeps as its time moves on,
// with its summary.
function cancelWake(folder: string, letters: string): void {
  if (!LETTERS.test(letters)) return;
  for (const name of listOf(path.join(folder, "wakes"))) {
    if (wakeOf(name)?.letters === letters) fs.rmSync(path.join(folder, "wakes", name), { force: true });
  }
  fs.rmSync(path.join(folder, "summaries", letters), { force: true });
}

// Until the MCP servers of the folder a conversation works in are connected, twenty seconds at
// most, as the app and the engine's watchdog wait before a wake-up: one sent sooner reaches a
// model without their tools.
async function waitForTools(port: number, authorization: string, directory: string, signal: AbortSignal | undefined): Promise<void> {
  const started = Date.now();
  let waited = false;
  while (Date.now() - started < 20_000) {
    let listed: string;
    try {
      const response = await fetch(`http://127.0.0.1:${port}/api/mcp`, { headers: { authorization, "x-opencode-directory": encodeURIComponent(directory) }, signal });
      if (!response.ok) return;
      listed = await response.text();
    } catch {
      return;
    }
    // A folder it has only just opened lists none for a moment before they're pending.
    const none = !listed.includes('"status"');
    if (!listed.includes('"pending"') && !(none && Date.now() - started < 3_000)) {
      if (waited && !none) await sleep(1_500);
      return;
    }
    waited = true;
    await sleep(none ? 100 : 250);
  }
}

// Wake-ups being sent with Wake Up Now, so a second tap doesn't send one twice.
const wakingNow = new Set<string>();

/** What can be done to a conversation's wake-ups past reading them (`kuvo/wakes/<session>/…`). */
export type WakeAction = "cancel" | "now" | "pause" | "watch";

/** Why its kuvo-wake said no: it isn't there, it refused (with what it said), or it failed. */
interface KuvoWakeRefusal {
  missing: boolean;
  refused: boolean;
  said: string;
}

// `kuvo/wakes/<session>`: its wake-ups, watches and pause; `…/cancel` with {"wake"} or {"watch"};
// `…/now` with {"wake"}, sent at once (one that repeats still comes back at its time); `…/watch`
// with {"every", "check", "note"}, a watch made as its agent makes one.
export async function* answerWakes(session: string, action: WakeAction | undefined, request: LinkRequest, readEngine: () => EngineRecord, bridge: () => BridgeRecord | null, signal: AbortSignal | undefined, admit?: RouteContext["admitQueued"]): AsyncGenerator<AnswerPiece> {
  const changed = function* (status: number, value: Wakes | Record<string, never>) {
    wakesChanged(session);
    yield* json(status, value);
  };
  const json = (status: number, value: unknown): AnswerPiece[] => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const method = request.method.toUpperCase();
  let folder = wakeFolder(session);
  if (method === "POST" && awaySessions().includes(session)) return yield* json(409, { message: MOVING });
  const listed = () => listedWakes(folder);
  if (!action && (method === "GET" || method === "HEAD")) return yield* json(200, listed());
  if (method !== "POST") return yield* json(405, {});
  // (What a client sent, each field checked as it's read.)
  let given: Record<string, unknown> = {};
  try {
    given = JSON.parse(Buffer.from(request.body ?? []).toString("utf8") || "{}") ?? {};
  } catch {}
  // Its own kuvo-wake (this computer's clock, its weekends), as its agent runs it: its message when it
  // says no, else null.
  const kuvoWake = (args: string[]) => new Promise<KuvoWakeRefusal | null>(resolve => execFile(WAKE_COMMAND, [folder!, ...args], { encoding: "utf8", timeout: 30_000 },
    (error, _out, said) => resolve(error ? { missing: error.code === "ENOENT", refused: error.code === 2, said: String(said ?? "").trim() } : null)));
  const unavailable = (failed: KuvoWakeRefusal, what: string) => failed.missing
    ? json(503, { message: "Wake-ups aren’t set up on this computer." })
    : json(failed.refused && failed.said ? 400 : 503, { message: failed.refused && failed.said ? failed.said : `${what} couldn’t be saved. Try again.` });
  if (action === "watch") {
    const every = typeof given.every === "string" ? given.every.trim() : "";
    const check = typeof given.check === "string" ? given.check.trim() : "";
    const note = typeof given.note === "string" ? given.note.trim() : "";
    if (!every || !check || !note) return yield* json(400, { message: "Send how often (`every`, like 5m, 2h or 1d), the check to run (`check`) and what to do when it changes (`note`)." });
    if (!folder) {
      const held: { veto?: AnswerPiece[] | null } = {};
      const made = await wakeFolderFor(session, readEngine, bridge, () => {
        held.veto = queuedRefusal(admit?.() ?? null, json);
        return !held.veto;
      });
      if (held.veto) return yield* held.veto;
      if (!made) return yield* json(404, {});
      folder = made;
    }
    if (isMoving(folder)) return yield* json(409, { message: MOVING });
    const failed = await kuvoWake(["watch", every, check, note]);
    if (failed) return yield* unavailable(failed, "The watch");
    return yield* changed(200, listed());
  }
  if (!action) {
    // One added as the bar over the composer adds one.
    const due = Number(given.due), note = typeof given.note === "string" ? given.note.trim() : "";
    const every = given.every == null ? null : String(given.every);
    if (!Number.isFinite(due) || !note) return yield* json(400, { message: "Send when it comes (`due`, seconds since 1970) and what it says (`note`)." });
    if (every !== null && !/^(weekdays|[1-9][0-9]{0,5}[mhdw])$/.test(every)) return yield* json(400, { message: "That isn’t a repeat: 30m, 2h, 1d, 1w or weekdays." });
    if (due < Date.now() / 1000 + 60) return yield* json(400, { message: "Pick a time at least a minute from now." });
    if (!folder) {
      const held: { veto?: AnswerPiece[] | null } = {};
      const made = await wakeFolderFor(session, readEngine, bridge, () => {
        held.veto = queuedRefusal(admit?.() ?? null, json);
        return !held.veto;
      });
      if (held.veto) return yield* held.veto;
      if (!made) return yield* json(404, {});
      folder = made;
    }
    if (isMoving(folder)) return yield* json(409, { message: MOVING });
    if (readWakes(folder).wakes.length >= WAKE_LIMIT) return yield* json(409, { message: `There are ${WAKE_LIMIT} wake-ups waiting already. Cancel some first.` });
    const veto = queuedRefusal(admit?.() ?? null, json);
    if (veto) return yield* veto;
    const failed = await kuvoWake(["add", String(Math.floor(due)), ...(every ? ["--every", every] : []), note]);
    if (failed) return yield* unavailable(failed, "The wake-up");
    return yield* changed(200, listed());
  }
  // Held while it's archived, and let go when it's back, as its archiving (the Mac's, or a phone's
  // through this box) says: a hold of your own stays through both.
  if (action === "pause" && typeof given.archived === "boolean") {
    if (folder && isMoving(folder)) return yield* json(409, { message: MOVING });
    holdArchived(session, given.archived);
    // Archived while it waits on you: it stops, as a phone's Archive has it. (Not for a hold that
    // only follows an archive from before, `stop: false`: a first wake-up made for it.) Only while
    // its engine still has it archived: a Mac that hasn't heard it was brought back since stops nothing.
    const where = given.archived && given.stop !== false ? await whereIs(session, readEngine, bridge) : null;
    if (where?.found.metadata?.["kuvo.archived"] === true) await stopIfWaiting(where.target, session);
    return yield* changed(200, listed());
  }
  if (action === "pause" && typeof given.paused !== "boolean") return yield* json(400, { message: "Send {\"paused\": true} or false." });
  if (!folder) return yield* json(action === "pause" ? 200 : 404, action === "pause" ? listed() : {});
  if (isMoving(folder)) return yield* json(409, { message: MOVING });
  if ((action === "now" || action === "pause" && given.paused === false)
    && textOf(path.join(folder, "paused")) === "restore-review" && given.confirmRestoreRetry !== true) {
    return yield* json(409, { message: "Check whether this restored work already ran before retrying.", confirmRestoreRetry: true });
  }
  if (action === "pause") {
    // A hold of your own, until you take it away (archiving's and a move's are theirs: one
    // resumed while it's archived waits until it's back).
    if (given.paused) fs.writeFileSync(path.join(folder, "paused"), "paused");
    else if (textOf(path.join(folder, "paused")) !== "archived") fs.rmSync(path.join(folder, "paused"), { force: true });
    return yield* changed(200, listed());
  }
  const { wakes, watches } = readWakes(folder);
  const chosen = typeof given.wake === "string" ? wakes.find(each => each.id === given.wake || each.letters === given.wake) : undefined;
  if (action === "cancel") {
    if (chosen && LETTERS.test(chosen.letters)) {
      // The keeper holds this directory around both delivery and advancing the filename.
      const lock = path.join(path.dirname(folder), ".firing");
      const until = Date.now() + 2000;
      for (;;) {
        try { fs.mkdirSync(lock); break; }
        catch (error) {
          if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
          if (signal?.aborted || Date.now() >= until) return yield* json(409, { message: "A wake-up is being sent. Try canceling again in a moment." });
          await sleep(25);
        }
      }
      let moved: boolean;
      let veto: AnswerPiece[] | null;
      try {
        veto = queuedRefusal(admit?.() ?? null, json);
        moved = isMoving(folder) || awaySessions().includes(session);
        if (!moved && !veto) cancelWake(folder, chosen.letters);
      } finally {
        fs.rmdirSync(lock);
      }
      if (veto) return yield* veto;
      if (moved) return yield* json(409, { message: MOVING });
      return yield* changed(200, {});
    }
    const watch = typeof given.watch === "string" ? watches.find(each => each.letters === given.watch) : undefined;
    if (watch && LETTERS.test(watch.letters)) {
      await fs.promises.rm(path.join(folder, "watches", watch.letters), { recursive: true, force: true });
      return yield* changed(200, {});
    }
    return yield* json(404, {});
  }
  if (!chosen) return yield* json(404, {});
  const key = `${folder}/${chosen.id}`;
  if (wakingNow.has(key)) return yield* json(200, {});
  wakingNow.add(key);
  try {
    // The whole note, as its time would send it, not the line it's listed by.
    const note = textOf(path.join(folder, "wakes", chosen.id));
    if (note === null) return yield* json(404, {});
    const agents = (textOf(path.join(folder, "via")) ?? "").trim() === "bridge";
    let target: Target | null = null;
    try {
      target = agents ? await bridgeBack(bridge, signal) : readEngine();
    } catch {}
    if (!target) return yield* json(503, agents ? agentsAway() ?? { message: agentsLine() } : { message: "The engine isn’t running." });
    const authorization = `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`;
    const directory = textOf(path.join(folder, "directory"));
    if (!agents && directory) await waitForTools(target.port, authorization, directory, signal);
    // With the summary its agent gave it, as kuvo-wake sends it at its time.
    const summary = LETTERS.test(chosen.letters) ? (textOf(path.join(folder, "summaries", chosen.letters)) ?? "").trim() : "";
    const message = { id: messageID(), text: WAKE_MARKER + note, delivery: "queue" as const, ...(summary ? { metadata: wakeMark(summary) } : {}) };
    // Its conversation stopped by you: it waits there with the queue, and goes once it goes on.
    if (!await keepWaitingLater(session, message, { from: "Wake-up", words: note }, undefined, target)) {
      let response: Response;
      try {
        response = await fetch(`http://127.0.0.1:${target.port}/api/session/${encodeURIComponent(session)}/prompt`, {
          method: "POST",
          headers: { authorization, "content-type": "application/json" },
          body: JSON.stringify(message),
          signal,
        });
      } catch {
        return yield* json(503, { message: "The wake-up couldn’t be sent." });
      }
      if (!response.ok) return yield* json(503, { message: `The wake-up couldn’t be sent (${response.status}).` });
    }
    if (!chosen.every) cancelWake(folder, chosen.letters);
    return yield* changed(200, {});
  } finally {
    wakingNow.delete(key);
  }
}

export async function* wakesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, text } = context;
  if (!["GET", "HEAD"].includes(request.method.toUpperCase())) return yield* text(405, "Read them with GET.");
  yield { head: { status: 200, headers: { "content-type": "application/json" } } };
  yield { body: Buffer.from(JSON.stringify({ sessions: listWakes() } satisfies WakeList)) };
  return;
}

// Every conversation's wake-ups and watches followed: a line now, and again as they change
// (added, fired, canceled, paused), as a Mac shows them (PROTOCOL.md, "Wake-ups").
export async function* wakeEventsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  return yield* followed(() => ({ sessions: allWakes() }), followersOf(wakeFollowers, moveFollowers), signal, {
    coalesce: true,
    watch: (_, changed) => {
      const schedules = watchSchedules(changed);
      const moves = watchFolders([MOVES], changed, { recursive: false });
      return { get whole() { return schedules.whole && moves.whole; }, close() { schedules.close(); moves.close(); } };
    },
  });
}

function listedWakes(folder: string | null): Wakes {
  if (!folder) return { wakes: [], watches: [], paused: false };
  const { wakes, watches, paused } = readWakes(folder);
  return { wakes: wakes.map(listedWake), watches, paused };
}

function watchSchedules(changed: () => void): SharedWatch {
  const bots = watchBotSchedules(BOTS, changed);
  const sessions = watchBotSchedules(SESSION_WAKES, changed);
  return { get whole() { return bots.whole && sessions.whole; }, close() { bots.close(); sessions.close(); } };
}

// One directory index and watcher per process, shared by the open per-session streams.
class Schedules {
  private readonly byFolder = new Map<string, string>();
  private readonly bySession = new Map<string, string>();
  private readonly listeners = new Map<string, Set<() => void>>();
  private away = new Set<string>();
  private readonly watchers: SharedWatch[] = [];
  private readonly timer: ReturnType<typeof setInterval>;

  constructor() {
    this.refresh();
    for (const root of [BOTS, SESSION_WAKES]) {
      this.watchers.push(watchBotSchedules(root, name => {
        if (!name) return this.refresh(true);
        const [id, file] = name.split(path.sep);
        if (!id || id.startsWith(".")) return;
        const folder = path.join(root, id);
        if (root === SESSION_WAKES) {
          if (!file || ["session", "wakes", "watches", "summaries", "paused"].includes(file)) this.changed(id);
        } else if (!file || file === "session") {
          this.index(folder);
        } else if (["wakes", "watches", "summaries", "paused"].includes(file)) {
          this.changed(this.byFolder.get(folder) ?? null);
        }
      }));
    }
    this.watchers.push(watchFolder(MOVES, this.ownership, { create: true }));
    moveFollowers.add(this.ownership);
    let ticks = 0;
    this.timer = setInterval(() => {
      if (++ticks % 30 === 0 || this.watchers.some(watch => !watch.whole)) this.refresh(true);
    }, 1_000);
    this.timer.unref?.();
  }

  private index(folder: string) {
    const before = this.byFolder.get(folder);
    const session = (textOf(path.join(folder, "session")) ?? "").trim();
    if (before && before !== session) {
      this.byFolder.delete(folder);
      if (this.bySession.get(before) === folder) this.bySession.delete(before);
      this.changed(before);
    }
    if (!session) return;
    this.byFolder.set(folder, session);
    if (!this.bySession.has(session) || path.dirname(folder) === BOTS) this.bySession.set(session, folder);
    this.changed(session);
  }

  private refresh(tell = false) {
    this.byFolder.clear();
    this.bySession.clear();
    for (const id of listOf(BOTS)) this.index(path.join(BOTS, id));
    this.ownership();
    if (tell) this.changed();
  }

  private ownership = () => {
    const away = new Set(awaySessions());
    const before = this.away;
    this.away = away;
    for (const session of this.listeners.keys()) if (away.has(session) !== before.has(session)) this.changed(session);
  };

  folder(session: string): string | null {
    if (this.away.has(session) || !/^[A-Za-z0-9_-]{1,100}$/.test(session)) return null;
    const bot = this.bySession.get(session);
    if (bot && path.dirname(bot) === BOTS) return bot;
    const own = path.join(SESSION_WAKES, session);
    return textOf(path.join(own, "session")) === null ? null : own;
  }

  changed(session?: string | null) {
    if (session === null) return;
    const listeners = session === undefined ? this.listeners.values() : [this.listeners.get(session)];
    for (const list of listeners) if (list) for (const tell of list) tell();
  }

  follow(session: string, changed: () => void) {
    const list = this.listeners.get(session) ?? new Set();
    list.add(changed);
    this.listeners.set(session, list);
    return () => {
      list.delete(changed);
      if (!list.size) this.listeners.delete(session);
      if (this.listeners.size) return;
      clearInterval(this.timer);
      moveFollowers.delete(this.ownership);
      for (const watcher of this.watchers) watcher.close();
      schedules = null;
    };
  }
}

let schedules: Schedules | null = null;
function followSchedule(session: string, changed: () => void) {
  schedules ??= new Schedules();
  return { whole: true, close: schedules.follow(session, changed) };
}

export async function* sessionWakesRoute(context: RouteContext, wakes: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, readEngine, bridge, text } = context;
  let session: string;
  try {
    session = decodeURIComponent(wakes[1]);
  } catch {
    return yield* text(404, "No such session.");
  }
  if (wakes[2] === "events") {
    if (request.method.toUpperCase() !== "GET") return yield* context.json(405, {});
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    const followers = new Set<() => void>();
    return yield* followed(() => listedWakes(schedules ? schedules.folder(session) : wakeFolder(session)), followers, signal, {
      coalesce: true,
      beforeWatch: true,
      watch: (_, changed) => followSchedule(session, changed),
    });
  }
  return yield* answerWakes(session, wakes[2] as WakeAction | undefined, request, readEngine, bridge, signal);
}
