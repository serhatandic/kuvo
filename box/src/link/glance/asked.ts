// When what a session waits on you for came (`asked` in `kuvo/sessions/glance`): a permission or a
// question it or one of its subagents asks, noted as its event comes from the engine or the coding
// agents' bridge, and let go as it's answered, its run ends or it's deleted; and what the box itself
// asks you for it (to run a command outside the sandbox, a sign-in or a key from the vault, what an
// agent asks while it browses, what a bot asks before it does it, a goal a bot puts to you). An ask
// Auto's reviewer takes up isn't yours, and isn't counted. The box's long-running process notes them,
// in `run/glance/asks.json`, so a route answered in a process of its own (`box route`) reads them
// too. An ask made while the box wasn't listening has no time, and a client says how long since the
// session last changed instead.

import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { writePrivate } from "../files";
import { RUN } from "../home";
import { call } from "../engine/client";
import { listAskReviews, onAskReviewsChanged } from "../work/reviewing";
import { listSandboxAsks, onSandboxAsksChanged } from "../fence/outside";
import { listAsks as listVaultAsks, onAsksChanged as onVaultAsksChanged } from "../vault/asks";
import { readAsks as readBrowserAsks, type BrowserAsk } from "../areas/asks";
import { askFollowers as workAskFollowers, workAsks } from "../work/asks";
import { goalFollowers, proposedGoals } from "../work/goals";
import type { Target } from "../../shared/records";

export const ASKS_FILE = path.join(RUN, "glance", "asks.json");

/** An ask's event, as this reads it: what it is, its id (`requestID` once answered), and the
 * session asking (a form's under `form`, or as it is). */
export interface AskEvent {
  type: string;
  data?: { id?: unknown; requestID?: unknown; sessionID?: unknown; form?: unknown; action?: unknown; resources?: unknown };
}

/** What an ask asks, in a few words (`needsYou` in `kuvo/sessions/list`): its kind, and the box's
 * words for it, written to follow the session's name ("asks to run a command"). */
export interface AskWords {
  kind: "permission" | "question" | "sandbox" | "vault" | "other";
  summary: string;
}

/** `text`, 60 characters of it at most: a summary is a few words, whatever a tool names. */
const cut = (text: string, most: number): string => (text.length > most ? text.slice(0, most - 1) + "…" : text);
const lastPart = (file: string): string => file.replace(/\/+$/, "").split("/").pop() || file;

/** A permission in a few words, by what it would do (as its card's title goes by it, `permissionTitle`). */
export function permissionWords(action: unknown, resources: unknown): AskWords {
  const files = Array.isArray(resources) ? resources.filter((each): each is string => typeof each === "string") : [];
  const what = files.length > 1 ? `${files.length} files` : files.length === 1 ? cut(lastPart(files[0]!), 60) : "a file";
  const summary = (() => {
    switch (action) {
      case "edit": case "patch": case "write": return `asks to edit ${what}`;
      case "read": return `asks to read ${what}`;
      case "shell": case "bash": return "asks to run a command";
      case "external_directory": return "asks to work outside the project";
      case "subagent": return "asks to start an agent";
      case "webfetch": return "asks to fetch a page";
      default: return typeof action === "string" && action ? `asks to ${cut(action.replaceAll("_", " "), 60)}` : "asks for your OK";
    }
  })();
  return { kind: "permission", summary };
}

/** A form in a few words: the agent's question, or anything else that needs your answer (an MCP server's). */
export function formWords(form: { metadata?: unknown }): AskWords {
  const kind = isObject(form.metadata) ? form.metadata.kind : undefined;
  return kind === "question" ? { kind: "question", summary: "has a question for you" } : { kind: "other", summary: "needs your answer" };
}

export const SANDBOX_WORDS: AskWords = { kind: "sandbox", summary: "asks to run a command outside the sandbox" };
export const VAULT_WORDS: AskWords = { kind: "vault", summary: "asks for something from the vault" };
/** What an agent asks while it browses (`kuvo/asks`), in a few words. */
export const browserWords = (ask: Pick<BrowserAsk, "kind" | "host">): AskWords => ({
  kind: "other",
  summary: ask.kind === "signIn" ? `asks you to sign in${ask.host ? ` to ${cut(ask.host, 40)}` : ""}` : ask.kind === "code" ? `asks for a code${ask.host ? ` from ${cut(ask.host, 40)}` : ""}` : "asks you to do a step in its browser",
});
/** What a bot asks before it does it (`kuvo/work/asks`), in a few words. */
export const workWords = (action: string): AskWords => ({
  kind: "other",
  summary: action === "makeProject" ? "asks to make a project" : action === "cloneProject" ? "asks to clone a project" : action === "resetLimits" ? "asks to use a limit reset" : "asks for your OK",
});
export const GOAL_WORDS: AskWords = { kind: "other", summary: "proposes a goal" };
/** An ask heard without what it asks (noted by a box from before these words). */
export const WAITS_WORDS: AskWords = { kind: "other", summary: "waits on you" };

/** An ask waiting: the session asking (a subagent's own), when it came (ms), and the place it was
 * asked on ("engine", "bridge"), whose restart lets it go. */
interface OpenAsk {
  session: string;
  at: number;
  place?: string;
  /** What it asks, in a few words. */
  asks?: AskWords;
}

const ASKED = new Set(["permission.asked", "form.created"]);
const ANSWERED = new Set(["permission.replied", "permission.rejected", "form.replied", "form.cancelled"]);
const ENDS = new Set(["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted", "session.deleted"]);

/** Each ask waiting, by its id, as this process heard them. */
const open = new Map<string, OpenAsk>();
/** The session each subagent works for, as its engine said: a session's never changes. */
const parents = new Map<string, string | null>();
/** Whether this process has noted anything yet. */
let started = false;
/** How many times what's asked changed in this process: a follower that read it before a change
 * reads it again. */
export let askChanges = 0;
/** Told as an ask comes or goes. */
export const askFollowers = new Set<() => void>();

/** Takes in an event from the engine or the coding agents' bridge (`place`): an ask noted as it
 * comes, let go as it's answered, and all of a session's as its run ends (the engine lets them go
 * with it, without a word of each) or it's deleted. */
export function noteAskEvent(event: AskEvent, place?: string, now = Date.now()): void {
  const data = event.data ?? {};
  let changed = false;
  const asked = event.type === "form.created" && isObject(data.form) ? data.form : data;
  if (ASKED.has(event.type)) {
    if (typeof asked.id === "string" && typeof asked.sessionID === "string" && asked.sessionID.startsWith("ses") && !open.has(asked.id)) {
      const asks = event.type === "form.created" ? formWords(asked as { metadata?: unknown }) : permissionWords(asked.action, asked.resources);
      open.set(asked.id, { session: asked.sessionID, at: now, ...(place ? { place } : {}), asks });
      changed = true;
    }
  } else if (ANSWERED.has(event.type)) {
    const id = data.requestID ?? data.id;
    changed = typeof id === "string" && open.delete(id);
  } else if (ENDS.has(event.type) && typeof data.sessionID === "string") {
    for (const [id, ask] of open) if (ask.session === data.sessionID) changed = open.delete(id) || changed;
  }
  if (changed) keep();
}

/** What waits on you now, for every process, and those following told: the engine's asks but those
 * Auto's reviewer took up, and the box's own (the sandbox's, the vault's). */
function keep(): void {
  started = true;
  const reviewed = new Set(listAskReviews().filter(each => each.state !== "yours").map(each => each.id));
  const asks: Record<string, OpenAsk> = {};
  for (const [id, ask] of open) if (!reviewed.has(id)) asks[id] = ask;
  for (const ask of listSandboxAsks()) if (ask.session) asks[ask.id] = { session: ask.session, at: ask.at, asks: SANDBOX_WORDS };
  for (const ask of listVaultAsks()) if (ask.session) asks[ask.id] = { session: ask.session, at: ask.at, asks: VAULT_WORDS };
  const others = othersNow();
  othersKept = [...others.keys()].sort().join("\n");
  for (const [id, ask] of others) asks[id] = ask;
  try {
    writePrivate(ASKS_FILE, JSON.stringify({ asks }) + "\n");
  } catch {}
  askChanges += 1;
  for (const told of askFollowers) try { told(); } catch {}
}

/** When each of the browser's asks was first seen here (the browser keeps no time for one). */
const browserSeen = new Map<string, number>();

/** What else waits on you for a session, kept by the box outside the engine's asks: what an agent
 * asks while it browses (a sign-in, a code, a step of yours), what a bot asks before it does it
 * (a project, a limit reset), and a goal a bot proposed, which waits in its own conversation. Each
 * by an id of its own, with the session it waits in. */
function othersNow(now = Date.now()): Map<string, OpenAsk> {
  const asks = new Map<string, OpenAsk>();
  const browsing = (readBrowserAsks() as BrowserAsk[]).filter(ask => typeof ask.session === "string" && ask.session);
  for (const id of browserSeen.keys()) if (!browsing.some(ask => ask.id === id)) browserSeen.delete(id);
  for (const ask of browsing) {
    if (!browserSeen.has(ask.id)) browserSeen.set(ask.id, now);
    asks.set(`browser:${ask.id}`, { session: ask.session!, at: browserSeen.get(ask.id)!, asks: browserWords(ask) });
  }
  for (const ask of workAsks()) if (ask.session) asks.set(`work:${ask.id}`, { session: ask.session, at: ask.at, asks: workWords(ask.action) });
  for (const goal of proposedGoals()) if (goal.session) asks.set(`goal:${goal.id}`, { session: goal.session, at: goal.proposed, asks: GOAL_WORDS });
  return asks;
}

/** Looks again at what the box keeps outside the engine's asks, and keeps what waits when it
 * changed: the browser's asks are another process's file, and a route answered in a process of its
 * own (`box route`) changes the others'. */
let othersKept = "";
export function lookAtOthers(): void {
  const now = [...othersNow().keys()].sort().join("\n");
  if (now === othersKept) return;
  keep();
}

/** The box's long-running process: what it asks you itself, and what Auto's reviewer takes up,
 * kept as they change; what's kept in files another process writes, looked at every two seconds
 * (as the browser's asks are for their notifications). */
export function followAsks(): () => void {
  const stops = [onAskReviewsChanged(keep), onSandboxAsksChanged(keep), onVaultAsksChanged(keep)];
  workAskFollowers.add(lookAtOthers);
  goalFollowers.add(lookAtOthers);
  const timer = setInterval(lookAtOthers, 2000);
  timer.unref?.();
  return () => {
    for (const stop of stops) stop();
    workAskFollowers.delete(lookAtOthers);
    goalFollowers.delete(lookAtOthers);
    clearInterval(timer);
  };
}

/** A place heard again after it couldn't be (it restarted on another port, its stream dropped): what
 * it asked meanwhile went by unheard, and what it asked before may be gone with it, so what was
 * noted of it goes, rather than say an answered ask still waits. */
export function forgetAsksOn(place: string): void {
  // (What a process before this one noted goes the first time: none of it was heard here.)
  let changed = !started;
  for (const [id, ask] of open) if (ask.place === place) changed = open.delete(id) || changed;
  if (changed) keep();
}

/** The asks waiting, as the long-running process noted them: the session asking (a subagent's own)
 * and when (ms). */
export function readAsks(): OpenAsk[] {
  try {
    const kept = JSON.parse(fs.readFileSync(ASKS_FILE, "utf8")).asks;
    return isObject(kept)
      ? Object.values(kept).filter((ask): ask is OpenAsk => isObject(ask) && typeof ask.session === "string" && typeof ask.at === "number").map(({ asks, ...ask }) => ({
          ...ask,
          ...(isObject(asks) && typeof asks.kind === "string" && typeof asks.summary === "string" ? { asks: { kind: asks.kind, summary: asks.summary } as AskWords } : {}),
        }))
      : [];
  } catch {
    return [];
  }
}

/** When each ask waiting came (ms), by its id, as the long-running process noted them. */
export function askTimes(): Map<string, number> {
  try {
    const kept = JSON.parse(fs.readFileSync(ASKS_FILE, "utf8")).asks;
    return new Map(isObject(kept) ? Object.entries(kept).flatMap(([id, ask]) => (isObject(ask) && typeof ask.at === "number" ? [[id, ask.at] as [string, number]] : [])) : []);
  } catch {
    return new Map();
  }
}

/** The conversation `session` is part of (a subagent's, the one it works for), as `targets` say. */
async function rootOf(targets: Target[], session: string, signal?: AbortSignal): Promise<string> {
  let at = session;
  for (let step = 0; step < 10; step++) {
    if (!parents.has(at)) {
      let found: unknown;
      for (const target of targets) {
        found = await call(target, "GET", `/api/session/${encodeURIComponent(at)}`, undefined, signal ? AbortSignal.any([signal, AbortSignal.timeout(5_000)]) : AbortSignal.timeout(5_000)).catch(() => undefined);
        if (found !== undefined) break;
      }
      if (found === undefined) return at;
      parents.set(at, isObject(found) && typeof found.parentID === "string" ? found.parentID : null);
    }
    const parent = parents.get(at);
    if (!parent) return at;
    at = parent;
  }
  return at;
}

/** Since when each of `sessions` has waited on you (ms), its subagents' asks counting as its own:
 * the oldest ask still open. */
export async function askedSince(sessions: string[], targets: Target[], signal?: AbortSignal): Promise<Map<string, number>> {
  const wanted = new Set(sessions);
  const since = new Map<string, number>();
  for (const ask of readAsks()) {
    const root = wanted.has(ask.session) ? ask.session : await rootOf(targets, ask.session, signal);
    if (wanted.has(root)) since.set(root, Math.min(since.get(root) ?? Infinity, ask.at));
  }
  return since;
}

/** Forgets every ask (a test's fresh start). */
export function forgetAsks(): void {
  open.clear();
  parents.clear();
  browserSeen.clear();
  othersKept = "";
  started = false;
  try { fs.rmSync(ASKS_FILE, { force: true }); } catch {}
}
