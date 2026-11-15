// The tries at adding, setting up again or uninstalling a computer over SSH (PROTOCOL.md,
// "Computers", Adding one), as this box keeps them for every client to draw: each with its steps,
// what it asks you now, how it failed or what it came to, all in this box's words. Kept in memory by
// the box's long-running process, and noted in `run/adding.json`, so a box that started again
// says a try was cut short (it's never taken up again by itself: you try again). One thing at a
// time per computer: what's asked of a computer waits for what's under way there to end.

import fs from "node:fs";
import path from "node:path";
import type { AddQuestion, AddStep, AddTry } from "../../shared/routes/box";
import { newID } from "../../shared/text";
import { writePrivate } from "../files";
import { RUN } from "../home";

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
  /** The answer its question waits for. */
  answer: ((id: string) => void) | null;
  /** When it ends (however it ends). */
  over: Promise<void>;
}

const running = new Map<string, Running>();
let finished: AddTry[] | null = null;
/** Told as any try changes (`kuvo/computers/add/events`). */
export const tryFollowers = new Set<() => void>();
const chains = new Map<string, Promise<void>>();

const isOver = (seen: AddTry): boolean => seen.state === "failed" || seen.state === "done";

// The tries that are over, with those a box that stopped left under way: said cut short.
function kept(): AddTry[] {
  if (finished) return finished;
  finished = [];
  try {
    const noted = JSON.parse(fs.readFileSync(TRIES_FILE, "utf8")) as AddTry[];
    for (const seen of Array.isArray(noted) ? noted.slice(0, 40) : []) {
      if (typeof seen?.id !== "string" || typeof seen.destination !== "string" || !Array.isArray(seen.steps)) continue;
      if (isOver(seen)) finished.push(seen);
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
  return [...[...running.values()].map(each => each.seen).reverse(), ...kept()];
}

function note(): void {
  try { writePrivate(TRIES_FILE, JSON.stringify(allTries())); } catch {}
  for (const tell of [...tryFollowers]) tell();
}

const NAMES = { add: ["Adding", "is one of your computers", "Couldn’t add"], setup: ["Setting up", "is set up again", "Couldn’t set up"], uninstall: ["Uninstalling Kuvo Code from", "no longer has Kuvo Code running", "Couldn’t uninstall from"] } as const;
const workingTitle = (kind: AddTry["kind"], name: string): string => `${NAMES[kind][0]} ${name}…`;
const doneTitle = (kind: AddTry["kind"], name: string): string => `${name} ${NAMES[kind][1]}`;
const failedTitle = (kind: AddTry["kind"], name: string): string => `${NAMES[kind][2]} ${name}`;

/** What a try's work is given to say how it goes. */
export interface Telling {
  signal: AbortSignal;
  /** Stops here if the try was stopped. */
  check(): void;
  /** The step under way now (those before it done), with what it says beside it. */
  step(id: string, detail?: string): void;
  /** How far the step under way has got, in bytes. */
  sent(bytes: number, total: number): void;
  /** A step that needed nothing, with why. */
  skip(id: string, detail: string): void;
  /** Asks you, and waits: the id of the answer you gave. Stopped while it waits, it throws. */
  ask(question: AddQuestion): Promise<string>;
  /** The computer it came to (shown with Done). */
  came(computer: { key: string; name: string }, known?: boolean): void;
  rename(name: string): void;
}

/** The try under way for `destination`, if any. */
export const underWay = (destination: string): Running | undefined => [...running.values()].find(each => each.seen.destination === destination);

/** Starts a try: after whatever's under way for that computer has ended, `work` runs, telling how
 * it goes; it ends done, failed (`Failed`'s words, or anything else's as they are) or stopped
 * (gone from the list). */
export function startTry(kind: AddTry["kind"], destination: string, name: string, steps: readonly (readonly [id: string, title: string])[], work: (tell: Telling) => Promise<void>): Running {
  const seen: AddTry = { id: newID("add_"), kind, destination, name, state: "working", title: workingTitle(kind, name), steps: steps.map(([id, title]) => ({ id, title, state: "waiting" as const })) };
  const stop = new AbortController();
  const it: Running = { seen, stop, answer: null, over: Promise.resolve() };
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
    step(id, detail) {
      tell.check();
      change({ steps: steady(it.seen.steps).map(step => (step.id === id ? { id: step.id, title: step.title, state: "now" as const, ...(detail ? { detail } : {}) } : step)) });
    },
    sent(bytes, total) {
      // (A line a quarter second at most: a copy tells thousands of times.)
      const now = Date.now();
      if (bytes < total && now - lastSaid < 250) return;
      lastSaid = now;
      change({ steps: it.seen.steps.map(step => (step.state === "now" ? { ...step, sent: bytes, total } : step)) });
    },
    skip(id, detail) {
      change({ steps: steady(it.seen.steps).map(step => (step.id === id ? { id: step.id, title: step.title, state: "done" as const, detail } : step)) });
    },
    ask(question) {
      tell.check();
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
        change({ state: "asking", question });
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
      it.seen = { ...rest, state: "done", title: doneTitle(kind, it.seen.name), steps: steady(it.seen.steps) };
    } catch (error) {
      if (error instanceof Stopped || stop.signal.aborted) {
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
  if (it.stop.signal.aborted) return;
  const { question: _question, ...rest } = it.seen;
  it.seen = { ...rest, state: "stopping", title: "Stopping…" };
  it.stop.abort();
  note();
}

/** Answers what the try `id` asks now: false when it asks nothing, or not that. */
export function answerTry(id: string, answer: string): AddTry | null {
  const it = running.get(id);
  if (!it?.answer || !it.seen.question?.answers.some(each => each.id === answer)) return null;
  it.answer(answer);
  return it.seen;
}

/** The try `id`, under way or over. */
export const tryOf = (id: string): AddTry | undefined => running.get(id)?.seen ?? kept().find(each => each.id === id);

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
