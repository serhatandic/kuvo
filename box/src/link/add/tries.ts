// The tries at adding, setting up again or uninstalling a computer over SSH (PROTOCOL.md,
// "Computers", Adding one), as this box keeps them for every client to draw: each with its steps,
// what it asks you now, how it failed or what it came to, all in this box's words. Kept in memory by
// the box's long-running process, and noted in `run/adding.json`, so a box that started again
// can recover saves already entrusted to the setup. A try that hadn't reached a save is cut short.
// One thing at a time per computer: what's asked of a computer waits for what's under way there to end.

import fs from "node:fs";
import path from "node:path";
import type { AddQuestion, AddStep, AddTry } from "../../shared/routes/box";
import type { SetupSaveReceipt } from "../../shared/setup-local";
import { newID } from "../../shared/text";
import { writeDurablePrivate, writePrivate } from "../files";
import { RUN } from "../home";
import { peersOf } from "../peers/known";
import { replacementOutcome, type ReplacementEnrollment } from "../setup/leadership";
import type { SaveEnvelope } from "../../shared/routes/setup-save";
import { queryOperation, withSaveOutcome, type Receipt } from "../setup/shared";
import { readLink } from "../state";

export const TRIES_FILE = path.join(RUN, "adding.json");

/** How many tries that are over are kept to show. */
const KEPT = 8;

/** A try being stopped (you cancelled it, or its computer is being uninstalled). */
export class Stopped extends Error {}

/** A try that failed, in words for you: `again`, whether trying again could go otherwise. */
export class Failed extends Error {
  constructor(message: string, readonly again = true, readonly title?: string) {
    super(message);
  }
}

/** A try as the box runs it: what clients see (`seen`), and what ends it and answers it. */
export interface Running {
  seen: AddTry;
  stop: AbortController;
  selectedCredentials?: string[];
  committed?: boolean;
  enrollment?: Enrollment;
  /** The answer its question waits for. */
  answer: ((id: string) => void) | null;
  /** When it ends (however it ends). */
  over: Promise<void>;
}

const running = new Map<string, Running>();
type EnrollmentPeer = { key: string; name: string };
type KeptEnrollmentSave = Receipt | (Omit<Receipt, "setup" | "status"> & { id: string });
type Enrollment = ({ save: KeptEnrollmentSave } | { replacement: ReplacementEnrollment }) & { peer: EnrollmentPeer };
type SavedTry = AddTry & { enrollment?: Enrollment };
let finished: SavedTry[] | null = null;
let asks = 0;
/** Told as any try changes (`kuvo/computers/add/events`). */
export const tryFollowers = new Set<() => void>();
const chains = new Map<string, Promise<void>>();

const isOver = (seen: AddTry): boolean => seen.state === "failed" || seen.state === "done";

// The tries that are over, with those a box that stopped left under way: said cut short.
function kept(): SavedTry[] {
  if (finished) return finished;
  finished = [];
  try {
    const noted = JSON.parse(fs.readFileSync(TRIES_FILE, "utf8")) as SavedTry[];
    for (const seen of Array.isArray(noted) ? noted.slice(0, 40) : []) {
      if (typeof seen?.id !== "string" || typeof seen.destination !== "string" || !Array.isArray(seen.steps)) continue;
      if (isOver(seen) || seen.enrollment) finished.push(seen);
      else {
        const { question: _question, ...rest } = seen;
        finished.push({ ...rest, state: "failed", title: failedTitle(seen.kind, seen.name),
          steps: seen.steps.map(step => (step.state === "now" ? { ...step, state: "failed" as const } : step)),
          failure: { title: failedTitle(seen.kind, seen.name), message: "Kuvo Code on this computer started again before this finished. Try again: it goes on from what's done.", again: true } });
      }
    }
  } catch {}
  return finished;
}

/** Every try, those under way first, then those that are over, newest first. */
export function allTries(): AddTry[] {
  const active = [...running.values()].map(each => each.seen).reverse();
  return [...active, ...kept().map(reconcileEnrollment)].map(attempt => {
    const save = attempt.save;
    if (save?.status !== "pending") return attempt;
    const outcome = queryOperation(save.setup, save.operation, save.proof);
    if (!outcome || outcome.status === "pending") return attempt;
    return { ...attempt, save: withSaveOutcome(save, outcome.status) } satisfies AddTry & SaveEnvelope;
  });
}

function note(durable = false, required = false): void {
  try {
    const active = [...running.values()].reverse().map(each => ({ ...each.seen, enrollment: each.enrollment }));
    (durable ? writeDurablePrivate : writePrivate)(TRIES_FILE, JSON.stringify([...active, ...kept()]));
  } catch (error) { if (required) throw error; }
  for (const tell of [...tryFollowers]) tell();
}

const NAMES = { add: ["Adding", "is one of your computers", "Couldn’t add"], setup: ["Setting up", "is set up again", "Couldn’t set up"], update: ["Updating Kuvo on", "is up to date", "Couldn’t update Kuvo on"], uninstall: ["Uninstalling Kuvo Code from", "no longer has Kuvo Code running", "Couldn’t uninstall from"] } as const;
const workingTitle = (kind: AddTry["kind"], name: string): string => `${NAMES[kind][0]} ${name}…`;
const doneTitle = (kind: AddTry["kind"], name: string): string => `${name} ${NAMES[kind][1]}`;
const failedTitle = (kind: AddTry["kind"], name: string): string => `${NAMES[kind][2]} ${name}`;

/** After restart the proposal is still driven by replication. Its outcome decides the add. */
function reconcileEnrollment(seen: SavedTry): AddTry {
  const { enrollment, ...attempt } = seen;
  if (!enrollment) return attempt;
  const { peer } = enrollment;
  const link = readLink();
  const status = "replacement" in enrollment
    ? replacementOutcome(link, enrollment.replacement)
    : enrollmentOutcome(link, enrollment.save);
  if (status === "pending") return attempt;
  const computer = status === "committed" && (peersOf(link).find(each => each.key === peer.key)
    ?? ("replacement" in enrollment ? peer : null));
  if (computer) return { ...attempt, computer: { key: computer.key, name: computer.name }, state: "done", title: doneTitle(attempt.kind, attempt.name),
    steps: attempt.steps.map(step => step.state === "now" ? { ...step, state: "done" } : step) };
  const title = failedTitle(attempt.kind, attempt.name);
  return { ...attempt, state: "failed", title, steps: attempt.steps.map(step => step.state === "now" ? { ...step, state: "failed" } : step),
    failure: { title, message: `The setup changed before ${peer.name} was saved. Add it again from the main computer.`, again: true } };
}

function enrollmentOutcome(link: ReturnType<typeof readLink>, save: KeptEnrollmentSave) {
  // Older saved attempts used id for the setup and had no outcome in enrollment.
  const setup = "setup" in save ? save.setup : save.id;
  return queryOperation(setup, save.operation, save.proof)?.status
    ?? (link?.setup?.id === setup && link.setup.epoch === save.epoch ? "pending" : "interrupted");
}

/** What a try's work is given to say how it goes. */
export interface Telling {
  signal: AbortSignal;
  /** Stops here if the try was stopped. */
  check(): void;
  /** An irreversible change has been saved; keep the cleanup outcome even if asked to stop. */
  commit?(): void;
  saving(save: Receipt, peer: EnrollmentPeer): void;
  saved(receipt: SetupSaveReceipt): void;
  replacing?(replacement: ReplacementEnrollment, peer: EnrollmentPeer): void;
  choseCredentials(selection: string[]): void;
  /** The step under way now (those before it done), with what it says beside it. */
  step(id: string, detail?: string): void;
  /** How far the step under way has got, in bytes. */
  sent(bytes: number, total: number): void;
  /** A step that needed nothing, with why. */
  skip(id: string, detail: string): void;
  /** Asks you, and waits: the id of the answer you gave. Stopped while it waits, it throws. */
  ask(question: Omit<AddQuestion, "at">): Promise<string>;
  /** The computer it came to (shown with Done). */
  came(computer: { key: string; name: string }, known?: boolean): void;
  rename(name: string): void;
}

/** The try under way for `destination`, if any: the newest, never one that's being stopped (what
 * it runs there now ends first, and whatever's asked next waits its turn behind it). */
export const underWay = (destination: string): Running | undefined => [...running.values()].reverse().find(each => each.seen.destination === destination && !each.stop.signal.aborted);

/** Starts a try: after whatever's under way for that computer has ended, `work` runs, telling how
 * it goes; it ends done, failed (`Failed`'s words, or anything else's as they are) or stopped
 * (gone from the list). */
export function startTry(kind: AddTry["kind"], destination: string, name: string, steps: readonly (readonly [id: string, title: string])[], work: (tell: Telling) => Promise<void>, credentials?: string[]): Running {
  const previousChoice = kind === "add" ? kept().find(each => each.kind === kind && each.destination === destination && each.state === "failed")?.credentials : undefined;
  const choice = previousChoice ?? credentials;
  const seen: AddTry = { ...(choice ? { credentials: choice } : {}), id: newID("add_"), kind, destination, name, state: "working", title: workingTitle(kind, name), steps: steps.map(([id, title]) => ({ id, title, state: "waiting" as const })) };
  const stop = new AbortController();
  const it: Running = { seen, stop, selectedCredentials: credentials, answer: null, over: Promise.resolve() };
  const change = (next: Partial<AddTry>) => {
    it.seen = { ...it.seen, ...next };
    note();
  };
  // (The step that was under way is done: how far it had got goes.)
  const steady = (steps: readonly AddStep[]): AddStep[] => steps.map(step => (step.state === "now" ? { id: step.id, title: step.title, state: "done" as const, ...(step.detail ? { detail: step.detail } : {}) } : step));
  let lastSaid = 0;
  const tell: Telling = {
    signal: stop.signal,
    check() { if (stop.signal.aborted) throw new Stopped(); },
    commit() { it.committed = true; },
    saving(save, peer) {
      it.committed = true;
      it.enrollment = { save, peer };
      it.seen = { ...it.seen, save } satisfies AddTry & SaveEnvelope;
      if (it.seen.state === "stopping") it.seen = { ...it.seen, state: "working", title: workingTitle(kind, name) };
      note(true);
    },
    saved(save) {
      it.seen = { ...it.seen, save } satisfies AddTry & SaveEnvelope;
      note(true);
    },
    replacing(replacement, peer) {
      it.enrollment = { replacement, peer };
      try { note(true, true); } catch (error) {
        delete it.enrollment;
        throw error;
      }
      it.committed = true;
    },
    choseCredentials(selection) {
      it.seen = { ...it.seen, credentials: selection };
      note(true, true);
    },
    step(id, detail) {
      tell.check();
      change({ steps: steady(it.seen.steps).map(step => (step.id === id ? { id: step.id, title: step.title, state: "now" as const, ...(detail ? { detail } : {}) } : step)) });
    },
    sent(bytes, total) {
      // (A line a quarter second at most: a copy tells thousands of times.)
      const now = Date.now();
      if (bytes < total && now - lastSaid < 250) return;
      lastSaid = now;
      const megabytes = (count: number) => Math.max(1, Math.round(count / 1_000_000));
      change({ steps: it.seen.steps.map(step => (step.state === "now" ? { ...step, detail: `${megabytes(bytes)} of ${megabytes(total)} MB`, sent: bytes, total } : step)) });
    },
    skip(id, detail) {
      change({ steps: steady(it.seen.steps).map(step => (step.id === id ? { id: step.id, title: step.title, state: "done" as const, detail } : step)) });
    },
    ask(question) {
      tell.check();
      // (Each ask its own number: an answer meant for an earlier one, late, isn't taken for this.)
      const asked = { ...question, at: ++asks };
      return new Promise<string>((resolve, reject) => {
        const gone = () => { it.answer = null; reject(new Stopped()); };
        stop.signal.addEventListener("abort", gone, { once: true });
        it.answer = id => {
          if (!question.answers.some(answer => answer.id === id)) return;
          stop.signal.removeEventListener("abort", gone);
          it.answer = null;
          const { question: _asked, ...rest } = it.seen;
          it.seen = { ...rest, state: "working" };
          note();
          resolve(id);
        };
        change({ state: "asking", question: asked });
      });
    },
    came(computer, known) { change({ computer, ...(known ? { known: true } : {}) }); },
    rename(name) { change({ name, title: workingTitle(kind, name) }); },
  };
  running.set(seen.id, it);
  const before = chains.get(destination) ?? Promise.resolve();
  it.over = before.then(async () => {
    try {
      tell.check();
      await work(tell);
      const { question: _question, ...rest } = it.seen;
      // (One that was here already says so: the address given is another way to it.)
      const title = kind === "add" && it.seen.known ? `${destination} is ${it.seen.name}, already one of your computers` : doneTitle(kind, it.seen.name);
      it.seen = { ...rest, state: "done", title, steps: steady(it.seen.steps) };
    } catch (error) {
      // (Stopped: gone from the list, unless stopping it left something to say.)
      if (error instanceof Stopped || (stop.signal.aborted && !(error instanceof Failed))) {
        running.delete(seen.id);
        return;
      }
      const failure = error instanceof Failed ? error : new Failed(error instanceof Error && error.message ? error.message : "Something went wrong.");
      const { question: _question, ...rest } = it.seen;
      it.seen = { ...rest, state: "failed", title: failedTitle(kind, it.seen.name),
        steps: it.seen.steps.map(step => (step.state === "now" ? { id: step.id, title: step.title, state: "failed" as const } : step)),
        failure: { title: failure.title ?? failedTitle(kind, it.seen.name), message: failure.message, again: failure.again } };
    }
    if (running.delete(seen.id)) finished = [it.seen, ...kept().filter(each => each.destination !== destination || each.kind !== kind)].slice(0, KEPT);
  }).finally(() => {
    if (chains.get(destination) === it.over) chains.delete(destination);
    note();
  });
  chains.set(destination, it.over);
  note();
  return it;
}

/** Stops the try `id` (at its next step; what it runs there now ends first), or forgets one that's
 * over. False for one there isn't. */
export function endTry(id: string): boolean {
  const it = running.get(id);
  if (it) {
    stopTry(it);
    return true;
  }
  const had = kept();
  if (!had.some(each => each.id === id)) return false;
  finished = had.filter(each => each.id !== id);
  note();
  return true;
}

/** Tells a try under way to stop: it says so until what it runs there now has ended. */
export function stopTry(it: Running): void {
  if (it.committed || it.stop.signal.aborted) return;
  const { question: _question, ...rest } = it.seen;
  it.seen = { ...rest, state: "stopping", title: "Stopping…" };
  it.stop.abort();
  note();
}

/** Answers what the try `id` asks now: null when it asks nothing, or not that. `at` says which ask
 * is answered (its number): one that names another ask is never taken, as two questions of one
 * try can share an answer's id (`trust`). */
export function answerTry(id: string, answer: string, at: number): AddTry | null {
  const it = running.get(id);
  if (!it?.answer || !it.seen.question?.answers.some(each => each.id === answer)) return null;
  if (at !== it.seen.question.at) return null;
  it.answer(answer);
  return it.seen;
}

/** The try `id`, under way or over. */
export const tryOf = (id: string): AddTry | undefined => allTries().find(each => each.id === id);

/** A test's: every try forgotten (those under way stopped first, and waited for). */
export async function forgetTries(): Promise<void> {
  const all = [...running.values()];
  for (const it of all) stopTry(it);
  await Promise.all(all.map(it => it.over));
  running.clear();
  chains.clear();
  finished = [];
  try { fs.rmSync(TRIES_FILE, { force: true }); } catch {}
}
