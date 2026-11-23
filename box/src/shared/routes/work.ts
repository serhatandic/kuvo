// A bot's work (PROTOCOL.md, "Work"): the sessions it started and theirs, where each stands, the
// asks it answered for them, what it started today, and the limits you set for it, if any; a
// client stopping all of it; what's asked of a bot by its sessions, answered on the box; what a bot
// asks you before it does it (making a project), and where new projects go.

import { bool, type Infer, list, map, nullable, number, object, type RouteShape, string, words } from "../shape";
import { PROJECT } from "./projects";

/** Limits you set for a bot (Edit Bot › Work), each left out when you set none: sessions working
 * at once, started in a day, how deep its sessions may start theirs, and tokens in a day. */
export const LIMITS = object({ "atOnce?": number, "perDay?": number, "depth?": number, "tokensPerDay?": number });

/** The bot a session works for, as every session under it is marked (`kuvo.by`). */
export const WORKS_FOR = object({ bot: string, name: string });

/** An ask one of a bot's sessions had, as the bot answered it: the ask, the tool call it was for
 * (left out when the engine didn't say), what it asked in words, the answer and why, who answered
 * (a bot by its id and name; a session that isn't a bot's, by its name) and when (ms since 1970). */
export const ANSWER = object({
  "session?": string,
  ask: string,
  "call?": string,
  what: string,
  decision: words("allow", "deny", "answer"),
  reason: string,
  by: object({ "bot?": string, name: string }),
  at: number,
});

/** The account a session runs on, as Settings › Accounts lists it where it runs: its provider's
 * name ("Claude Code", "GitHub Copilot"), and for a coding agent's account you added, the name you
 * gave it (`label`, "Work"). */
export const WORK_ACCOUNT = object({ name: string, title: string, "label?": string });

/** One of a bot's sessions: its address, title, folder and project (left out for a chat), the
 * session that started it, how deep it is (1 for the bot's own), its model and mode (left out when
 * the engine has none), the account it runs on (left out on Custom Auto, which picks one each turn,
 * or when where it runs can't say), where it stands (and, while it asks, what), when it started (ms), tokens
 * and cost so far (cost where the engine prices it), and the first lines of its last reply. */
export const WORK_SESSION = object({
  session: string,
  title: string,
  folder: string,
  "project?": string,
  startedBy: string,
  depth: number,
  "model?": object({ id: string, providerID: string, "variant?": string }),
  "mode?": string,
  "account?": WORK_ACCOUNT,
  state: words("working", "asking", "finished", "failed", "stopped", "idle"),
  "asks?": string,
  started: number,
  tokens: number,
  "cost?": number,
  "lastReply?": string,
});

/** Something a bot (or another conversation) asks you before it does it (`kuvo/work/asks`): making
 * a project, or cloning one into a new project. Who asks (its conversation, by its name, and the
 * bot, for a bot's), the project's name, the repository for a clone, the folder's name it gets and
 * where it would go (null while no projects folder is picked here, with a folder to offer, when
 * there's one), and when (ms). */
export const WORK_ASK = object({
  id: string,
  action: words("makeProject", "cloneProject", "resetLimits"),
  session: string,
  name: string,
  "bot?": string,
  /** The project's name; for a reset, the account's ("Codex"). */
  project: string,
  "clone?": string,
  folderName: string,
  folder: nullable(string),
  "suggested?": string,
  /** For a reset (`use_limit_reset`): the account (its row in "Plan limits"), how many resets it has, what one refills, and the computer of yours that uses it where that isn't this one. */
  "reset?": object({ row: string, account: string, service: string, count: number, "refills?": list(string), "on?": string }),
  /** What a notification of it says, in the box's words (a reset's). */
  "words?": object({ title: string, body: string }),
  at: number,
});
/** What's asked here now, the oldest first. */
export const WORK_ASKS = object({ asks: list(WORK_ASK) });

/** How your answer went: what was done, in the words its asker reads, and whether it failed. */
export const WORK_ASK_ANSWERED = object({ answer: words("allow", "always", "deny"), text: string, isError: bool });

/** Where new projects go here (null until you pick a folder), and a folder to offer without one; on a
 * Mac's box that keeps its Mac awake itself (one in a home of its own), whether it does while agents
 * work, and whether it is now. */
export const WORK_SETTINGS = object({ projectsFolder: nullable(string), "suggested?": string, "keepsAwake?": bool, "awakeNow?": bool });

/** A model, or an account's models, that can't answer now (`GET kuvo/work/unanswering`), as the
 * box holds them back from its bots: the provider, the model (left out for all of the account's,
 * or `only` its paid or free ones, then the `models` that covers, by id, when its engine could
 * say), why in the engine's words, since and until when (ms; sooner when a session on it answers,
 * or its key changes). */
export const UNANSWERING = object({ provider: string, "model?": string, "only?": words("paid", "free"), "models?": list(string), why: string, since: number, until: number });
export const UNANSWERING_LIST = object({ held: list(UNANSWERING) });

/** A conversation's own turn and the sessions below it, on any computer. */
export const ACTIVITY = object({
  working: number, asking: number,
  sessions: list(object({ session: string, title: string, state: words("working", "asking"), "computer?": string, "subagent?": bool })),
});
export type Activity = Infer<typeof ACTIVITY>;

/** A bot's work, as `GET kuvo/work?bot=` answers: its sessions (the latest first), the asks it
 * answered for them (the latest first), what it started today and how many work now, and the
 * limits you set (null for none). */
export const WORK = object({
  bot: string,
  "activity?": ACTIVITY,
  sessions: list(WORK_SESSION),
  answered: list(ANSWER),
  today: object({ started: number, "answered?": number, working: number, tokens: number, "cost?": number }),
  limits: nullable(LIMITS),
  "asks?": list(WORK_ASK),
  "projects?": list(PROJECT),
});

/** What a bot just did to one of its sessions (`kuvo/work/oversight`): started it, wrote to it
 * (`told`), read it, answered what it asked, stopped it, or saw it finish. Each with an `id` of its
 * own, so a client plays each once, and when (ms). */
export const OVERSIGHT_MARK = object({ id: string, bot: string, session: string, kind: words("started", "told", "read", "answered", "stopped", "finished"), at: number });
/** The marks of the latest minute, the oldest first. */
export const OVERSIGHT = object({ marks: list(OVERSIGHT_MARK) });

/** When each bot's work last changed (`kuvo/work/events`), in ms by its id: a client reads a bot's
 * work again when its time moves. */
export const WORK_CHANGED = object({ changed: map(number) });

/** What still runs for each session here besides its own turn (`kuvo/work/background`, PROTOCOL.md
 * "Work › Background work"), by its id: how many commands it left running, subagents at work under
 * it and other tasks its agent keeps going, and since when (ms). One here counts as at work; one that
 * never ends stops counting a while after it started. Clients take this as it is. */
export const BACKGROUND_LIST = object({ sessions: map(object({ commands: number, subagents: number, tasks: number, since: number })) });

/** What runs for one session besides its own turn (`kuvo/work/background/<session>`), each one, for
 * the bars over a client's composer: the commands it left running (when each started, ms; the
 * folder the engine knows it in; the ports a service listens on; `agents`, a coding agent's, read
 * and stopped through their bridge), and each subagent at work under it (since when, its title,
 * and whether it waits on you), the oldest first. A session with nothing has both empty. */
export const SESSION_BACKGROUND = object({
  "activity?": ACTIVITY,
  commands: list(object({ id: string, command: string, started: number, "folder?": string, "ports?": list(number), "agents?": bool })),
  subagents: list(object({ session: string, since: number, "title?": string, "asking?": bool })),
});

/** What a command a session left running has printed (`kuvo/work/background/<session>/commands/<id>/
 * output`), as a client's Output shows it: the command, as `kuvo/work/background/<session>` lists it
 * (null for one the box doesn't know); the end of what it printed, its last 24,000 bytes, from the
 * first whole line in them, without newlines at either end (`cut`: there was more before); and
 * whether it's still `running`, `ended` (it stopped, or its engine no longer has it: what it printed
 * last stands) or `unread` (it couldn't be read now; a later read may). */
export const BACKGROUND_OUTPUT = object({
  command: nullable(object({ id: string, command: string, started: number, "folder?": string, "agents?": bool })),
  output: string,
  cut: bool,
  state: words("running", "ended", "unread"),
});

/** A conversation's subagents (`kuvo/work/subagents/<session>`), as a side panel lists them, each
 * list newest first: those at work under it now, as `kuvo/work/background/<session>` lists them
 * (since when, ms, and whether one waits on you), and those whose work ended (when each started and
 * ended, ms, and how: `finished`, `failed` or `stopped`); each with its title and, where the box names
 * it, the model it ran on, as `kuvo/sessions/models` names it. */
export const SUBAGENTS = object({
  active: list(object({ session: string, title: string, since: number, asking: bool, "model?": string })),
  done: list(object({ session: string, title: string, started: number, ended: number, state: words("finished", "failed", "stopped"), "model?": string })),
});

const REFUSED = object({ message: string });

export type BackgroundList = Infer<typeof BACKGROUND_LIST>;
export type BackgroundOutput = Infer<typeof BACKGROUND_OUTPUT>;
export type Subagents = Infer<typeof SUBAGENTS>;
export type WorkLimits = Infer<typeof LIMITS>;
export type WorksFor = Infer<typeof WORKS_FOR>;
export type WorkAnswer = Infer<typeof ANSWER>;
export type WorkSession = Infer<typeof WORK_SESSION>;
export type WorkAccount = Infer<typeof WORK_ACCOUNT>;
export type BotWork = Infer<typeof WORK>;
export type UnansweringList = Infer<typeof UNANSWERING_LIST>;
export type WorkAskShape = Infer<typeof WORK_ASK>;
export type OversightMark = Infer<typeof OVERSIGHT_MARK>;
export type OversightKind = OversightMark["kind"];

export const WORK_ROUTES: RouteShape[] = [
  { route: "GET kuvo/work", feature: "work", answers: { 200: WORK, 400: REFUSED, 404: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/work/events", feature: "work", answers: { 200: WORK_CHANGED } },
  { route: "GET kuvo/work/background", feature: "work.background.items", answers: { 200: BACKGROUND_LIST } },
  { route: "GET kuvo/work/background/events", feature: "work.background.items", answers: { 200: BACKGROUND_LIST } },
  { route: "GET kuvo/work/background/:session", feature: "work.background.items", answers: { 200: SESSION_BACKGROUND } },
  { route: "GET kuvo/work/background/:session/events", feature: "work.background.items", answers: { 200: SESSION_BACKGROUND } },
  { route: "GET kuvo/work/background/:session/commands/:id/output", feature: "work.background.output", answers: { 200: BACKGROUND_OUTPUT } },
  { route: "GET kuvo/work/background/:session/commands/:id/output/events", feature: "work.background.output", answers: { 200: BACKGROUND_OUTPUT } },
  { route: "GET kuvo/work/subagents/:session", feature: "work.subagents", answers: { 200: SUBAGENTS, 503: REFUSED } },
  { route: "GET kuvo/work/subagents/:session/events", feature: "work.subagents", answers: { 200: SUBAGENTS } },
  { route: "POST kuvo/bots/:id/stop-all", feature: "work", answers: { 200: object({ stopped: number }), 404: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/work/oversight", feature: "work", answers: { 200: OVERSIGHT } },
  { route: "GET kuvo/work/oversight/events", feature: "work", answers: { 200: OVERSIGHT } },
  { route: "GET kuvo/work/asks", feature: "work.projects", answers: { 200: WORK_ASKS } },
  { route: "GET kuvo/work/asks/events", feature: "work.projects", answers: { 200: WORK_ASKS } },
  { route: "POST kuvo/work/asks/:id", feature: "work.projects", answers: { 200: WORK_ASK_ANSWERED, 400: REFUSED, 404: REFUSED } },
  { route: "GET kuvo/work/settings", feature: "work.projects", answers: { 200: WORK_SETTINGS } },
  { route: "PATCH kuvo/work/settings", feature: "work.projects", answers: { 200: WORK_SETTINGS, 400: REFUSED, 409: REFUSED } },
  { route: "GET kuvo/work/unanswering", feature: "work.unanswering", answers: { 200: UNANSWERING_LIST } },
  { route: "POST kuvo/projects/new", feature: "work.projects", answers: { 201: object({ project: PROJECT, text: string }), 400: REFUSED, 403: object({ message: string, needs: string }), 409: REFUSED, 502: REFUSED } },
];
