// A conversation carried on with the coding agents (`kuvo/sessions/<id>/agents`; PROTOCOL.md, "Models"), as
// picking one of their models in it does; a session started with a task (`kuvo/sessions`), and the
// agents' tools every box answers (`kuvo/agent-tools`; PROTOCOL.md, "Work"). A
// conversation's queue, what it asks you ("A conversation's asks"), what "@" and "&" suggest in
// it, and its side threads.

import { any, bool, BYTES, type Infer, list, literal, map, NOTHING, nullable, number, object, oneOf, type RouteShape, string, words } from "../shape";

/** The coding agents' session it goes on in now. */
export const CARRIED = object({ session: string });

export type Carried = Infer<typeof CARRIED>;

export { LIST_COMPUTER };
import { LIST_COMPUTER, PENDING_COMPUTERS } from "./list-computers";

/** A session started: its address, title, folder and project (null for a chat), whether it shows
 * beside the conversation that started it, and with `wait`, its first answer (null for none yet).
 * A client's start with no task: the session as it's made (`info`, as the engine lists it), and the
 * worktree made for it (`worktree`, as `kuvo/worktrees` makes one). */
export const STARTED = object({ session: string, title: nullable(string), folder: string, project: nullable(string), "answer?": nullable(string), "computer?": LIST_COMPUTER, "got?": words("askFirst", "acceptEdits", "auto"), "info?": any, "worktree?": object({ project: string, root: string, "folder?": string, branch: string, "base?": string }) });

export type StartedSession = Infer<typeof STARTED>;

/** A conversation moved into a project (`kuvo/sessions/<id>/move`): where it goes, and the session
 * as it is now (`info`, as the engine lists it; it moves at its next step while it's at work). */
export const SESSION_MOVED = object({ session: string, directory: string, "info?": any });

export type SessionMoved = Infer<typeof SESSION_MOVED>;

/** What a client asks first, all in the box's words: the question, what it comes to, and the
 * words of the button that goes ahead (Delete All's too: `POST kuvo/sessions/deletes`). */
export const CONFIRM = object({ title: string, message: string, label: string });

/** One thing a session's menu offers (`kuvo/sessions/<id>/actions`): its id, the button's words,
 * whether it destroys something, and where a client asks first, the question, what it comes to and
 * the words of the button that goes ahead, all the box's. */
export const SESSION_ACTION = object({
  id: words("pin", "unpin", "rename", "archive", "unarchive", "delete"),
  label: string,
  "destructive?": literal(true),
  "confirm?": CONFIRM,
});

/** What a session's menu offers now, in order; with `held`, why none of it can be done now (it's
 * moving to another computer), in words to show. */
export const SESSION_ACTIONS = object({ session: string, actions: list(SESSION_ACTION), "held?": string });

/** A session pinned, archived or renamed (or brought back, unpinned): the session as it is now
 * (`info`, as the engine or the coding agents' bridge lists it). */
export const SESSION_CHANGED = object({ session: string, info: any });

/** Refused as it's moving to another computer, or lives on another now: in words, and for the
 * latter that computer's key (`computer`), whose box a client asks instead. */
const LIVES_ELSEWHERE = object({ message: string, "computer?": string });

export type SessionAction = Infer<typeof SESSION_ACTION>;
export type SessionActions = Infer<typeof SESSION_ACTIONS>;

/** What an agent's tool says back to it: its words, and whether it didn't do what was asked. */
export const TOOL_ANSWER = object({ text: string, isError: bool });

const REFUSED = object({ message: string });

/** A message waiting on the box while its conversation is paused: who it's from (none for yours),
 * what it says as shown, its files' names, and when it began to wait (ms). */
export const WAITING = object({ id: string, "from?": string, text: string, "files?": list(string), at: number });

/** A session's queue: whether it waits for Resume, what Stop held, in the order it goes, and what
 * waits on the box (left out when nothing does). */
// (`line`: while what waits on the box waits for the engine to change, PROTOCOL.md "A new engine".)
export const QUEUE = object({ paused: bool, held: list(string), "waiting?": list(WAITING), "line?": string });

/** `queue/add`'s answer: the queue, and whether the message waits on the box (else it went on). */
const ADDED = object({ paused: bool, held: list(string), "waiting?": list(WAITING), "line?": string, waits: bool });

/** What `DELETE …/queue/<item>` took away, whole, for Undo to add back (`queue/add` with `kept`),
 * where it was: ahead of `before`, what came after it (none: it was last). */
const REMOVED = object({ id: string, text: string, "files?": list(any), delivery: oneOf(literal("steer"), literal("queue")), "from?": string, words: string, draft: string, "before?": string, "receipt?": string, "metadata?": any, "attached?": list(object({ uri: string, "name?": string })) });

export type SessionQueue = Infer<typeof QUEUE>;

/** One message waiting, as clients show it in the queue (`queue.items`): what you wrote (without
 * what the box added for the agent), its files' names, who it's from
 * (null for yours), where it waits (the engine's or the coding agents' bridge's inbox, or the box),
 * how it goes, and whether Stop held it. */
export const QUEUE_ITEM = object({
  id: string, words: string, files: list(string), from: nullable(string),
  on: words("engine", "box"), delivery: words("steer", "queue"), held: bool,
});

/** A session's queue with every message waiting, in the order clients show them (`items`: null
 * while the place it runs can't be read). */
export const QUEUE_SHOWN = object({ paused: bool, held: list(string), "waiting?": list(WAITING), "line?": string, "deliveryLine?": string, items: nullable(list(QUEUE_ITEM)) });
/** A session's queue as `GET …/queue` answers it: `items` only when asked (`?items=1`). */
export const QUEUE_READ = object({ paused: bool, held: list(string), "waiting?": list(WAITING), "line?": string, "items?": nullable(list(QUEUE_ITEM)) });

export type QueueItem = Infer<typeof QUEUE_ITEM>;
export type QueueShown = Infer<typeof QUEUE_SHOWN>;

/** An answer an ask's card offers, in its order: its words (`named`, what Always names, shown as
 * code after them, and `after`, words after that), the line Instead opens, what it does whole
 * (`help`, its hover text) and how a screen reader says it (`spoken`; null: as `title` says it).
 * `item`: the vault's item it gives (sent back with the answer). `confirm`: a client makes sure
 * it's you first (Touch ID, the device's password), as a grant made in Settings asks. */
export const ASK_CHOICE = object({
  answer: words("once", "always", "reject", "instead"), title: string, named: nullable(string), placeholder: nullable(string),
  help: nullable(string), spoken: nullable(string), "after?": string, "item?": string, "confirm?": bool,
});

/** A permission waiting (`sessions.asks`): the session asking (a subagent's own), the card's title,
 * the engine's words, the whole command line it would run, the changes it would make, what it
 * covers as the card shows it, and its answers. */
export const PERMISSION_ASK = object({
  id: string, kind: literal("permission"), session: string, title: string, message: nullable(string), command: nullable(string),
  files: list(object({ file: string, patch: string, additions: number, deletions: number, status: nullable(words("added", "deleted", "modified")) })),
  resources: list(string), choices: list(ASK_CHOICE),
});

/** One of a question's: what it asks, its choices, whether it takes several, words of your own,
 * and an answer. */
export const ASKED_QUESTION = object({
  key: string, title: nullable(string), question: string,
  options: list(object({ value: string, label: string, description: nullable(string) })),
  multiple: bool, custom: bool, required: bool,
});

/** The agent's questions, answered with choices or in words. */
export const QUESTION_ASK = object({ id: string, kind: literal("question"), session: string, title: string, questions: list(ASKED_QUESTION) });

/** A command the sandbox stopped, which the agent asks to run outside it (`kuvo/sandbox/asks`): the
 * card's title and what stopped it (`message`), the command, what the sandbox refused it (each
 * `{label, detail}`: "Write" and the path), the error it got where that can't be said (Linux), who
 * asks and why, and its answers: Allow Once, Always Allow <place> where the box names one, and
 * Don't Allow. */
export const SANDBOX_ASK_SHOWN = object({
  id: string, kind: literal("sandbox"), session: string, title: string, message: string, command: string,
  stopped: list(object({ label: string, detail: string })), error: nullable(string), asker: string, why: string, choices: list(ASK_CHOICE),
});

/** What the agent asks of the vault (`kuvo/vault/asks`): to use an item it may use only by asking,
 * or an item it needs (`need`); the card's title, its words (`message`), the site it's for, and its
 * answers: for an item it needs, Give and Always Give for each item a client may see (`note` says
 * why there are none). */
export const VAULT_ASK_SHOWN = object({
  id: string, kind: literal("vault"), session: string, need: words("use", "item"), title: string, message: nullable(string), site: nullable(string),
  note: nullable(string), choices: list(ASK_CHOICE),
});

/** Anything else it waits on you for: a form a card can't take (an MCP server's, fields a client
 * can't take). What it asks; `dismiss`: whether it can be put away unanswered from here. */
export const OTHER_ASK = object({ id: string, kind: literal("other"), session: string, title: string, message: string, dismiss: bool });

export const SESSION_ASK = oneOf(PERMISSION_ASK, QUESTION_ASK, SANDBOX_ASK_SHOWN, VAULT_ASK_SHOWN, OTHER_ASK);
/** What a conversation waits on you for, oldest first. */
export const SESSION_ASKS = object({ asks: list(SESSION_ASK) });

export type SessionAsk = Infer<typeof SESSION_ASK>;
export type PermissionAsk = Infer<typeof PERMISSION_ASK>;
export type QuestionAsk = Infer<typeof QUESTION_ASK>;
export type SandboxAskShown = Infer<typeof SANDBOX_ASK_SHOWN>;
export type VaultAskShown = Infer<typeof VAULT_ASK_SHOWN>;
export type AskChoice = Infer<typeof ASK_CHOICE>;
export type AskedQuestion = Infer<typeof ASKED_QUESTION>;

/** What "@" (files) and "&" (agents) suggest as you type (`sessions.mentions`): what goes in the
 * message, the name shown, beside it (a file's folder, an agent's project), what it is, and an
 * agent's session (for the send's `named`). */
export const MENTION = object({ text: string, title: string, detail: nullable(string), kind: words("file", "folder", "bot", "session"), session: nullable(string) });
export const MENTIONS = object({ mentions: list(MENTION) });

export type Mention = Infer<typeof MENTION>;

/** A pull request's checks as the box counts them from GitHub's: skipped and neutral ones count as
 * passed, cancelled, timed out and the like as failed, the rest still under way. */
export const GLANCE_CHECKS = object({ passed: number, failed: number, pending: number });

/** A session's pull request: its number and page, and once GitHub was read, its state, its size
 * and its checks (left out where it has none). */
export const GLANCE_PULL = object({
  number: number, url: string, "state?": words("open", "draft", "merged", "closed"),
  "additions?": number, "deletions?": number, "checks?": GLANCE_CHECKS,
});

/** What a session's row and its card show beyond the session itself (`kuvo/sessions/glance`): the
 * branch it has of its own (a worktree's, or its pull request's) and, without a pull request, the
 * lines it changed since its base; its pull request; the account it runs on; and since when it's
 * waited on you (ms), while it does. Each left out where there's none. */
export const GLANCE = object({
  "branch?": string, "additions?": number, "deletions?": number, "pull?": GLANCE_PULL,
  "account?": object({ name: string, "label?": string }), "asked?": number,
});

/** The sessions asked for, by id: one this box doesn't have is left out. */
export const GLANCES = object({ sessions: map(GLANCE) });

/** What a client offers doing about a model that can't run (`kuvo/sessions/models`' `action`, and
 * the typed send's when it's refused for the model): signing in to its `account`, adding its key,
 * installing its coding agent, or choosing a model; `label`, a button's words ("Sign In"), and
 * `help`, what it does ("Sign in to Anthropic"). */
export const MODEL_ACTION = object({ do: words("signIn", "addKey", "install", "choose"), label: string, help: string });

/** What a session's model button shows and whether its model can run (`kuvo/sessions/models`):
 * the model it runs on (its own, else the engine's default; null for none), its name as its
 * provider or agent lists it, whether it can run here now, and what's missing if not: a model to
 * choose, a sign-in to its account, a key, its coding agent installed, or the model itself, gone
 * from its provider's list. `account`: the account to sign in to, add a key for or install, or the
 * provider it's gone from. `plan`: for a key the engine needs where a plan goes through a coding
 * agent of its own (the engine's xAI, whose SuperGrok plan is Grok's), that plan, its agent and
 * the agent's provider. What that means for a composer, decided and worded by the box
 * (`models.readiness`): `refusal`, why a message on it can't go and what to do (left out where one
 * can); `choose`, the model button asks for a model and Send waits for one (a model gone, or none
 * to run on); `action`, what to offer doing about it. `fixed`: its agent picks it (a coding agent's
 * subagent), so a client names it and offers no choice. */
export const SESSION_MODEL = object({
  model: nullable(object({ providerID: string, id: string, "variant?": string })),
  "name?": string,
  runs: bool,
  "missing?": words("model", "signIn", "key", "install", "gone"),
  "fixed?": literal(true),
  "account?": object({ id: string, name: string, "label?": string }),
  "plan?": object({ name: string, agent: string, provider: string }),
  "connects?": bool,
  "refusal?": string,
  "choose?": bool,
  "action?": MODEL_ACTION,
});

/** The sessions asked for, by id: one the box doesn't have, or can't say about yet, is left out. */
export const SESSION_MODELS = object({ sessions: map(SESSION_MODEL) });

export type SessionModel = Infer<typeof SESSION_MODEL>;
export type ModelAction = Infer<typeof MODEL_ACTION>;

/** The models a session can use here (`kuvo/models/offered`), as a picker offers them: every one
 * that runs now (`retiring`: its provider is retiring it; `hidden`: its agent keeps it out of its
 * picker; a picker leaves either out, though a conversation on it carries on; `side`, where a
 * conversation on it runs, the engine or the coding agents' bridge, Custom Auto's too; `efforts`, its
 * variants; `aliases`, other ids a conversation may name it by; `free`, it needs no account:
 * OpenCode Zen's free models; `speeds`, those it runs at besides its standard one, Codex's Fast,
 * with each provider's picked `speed`, `models.speed`); whether the engine's and the coding agents' could be said
 * (`known`); the coding agents' accounts still listing theirs (`listing`: one of theirs not
 * listed may still run); and the model a new session starts on, for the one wanted (`starting`: null for none
 * that runs; left out while the engine can't say). */
const MODEL_REF = object({ providerID: string, id: string, "variant?": string });
const STARRED = object({ providerID: string, id: string });
const SPEED = object({ id: string, name: string, "description?": string });
export const MODELS_OFFERED = object({
  models: list(object({ providerID: string, id: string, name: string, side: words("engine", "agents"), efforts: list(string), aliases: list(string), "retiring?": bool, "hidden?": bool, "free?": bool, "speeds?": list(SPEED) })),
  known: object({ engine: bool, agents: bool }),
  "providers?": list(object({ id: string, name: string, "label?": string, "speed?": string })),
  "starred?": list(STARRED),
  "listing?": list(string),
  "unavailable?": list(object({ id: string })),
  "generations?": map(string),
  "scoped?": bool,
  "note?": string,
  "starting?": nullable(MODEL_REF),
  "startChoice?": object({ label: string, refusal: string }),
});
/** Your starred models, as `kuvo/models/starred` keeps them (`models.starred`). */
export const MODELS_STARRED = object({ starred: list(STARRED) });
/** The speed picked for a provider, as `kuvo/models/speed` keeps it (`models.speed`): null for Standard. */
export const MODELS_SPEED = object({ providerID: string, speed: nullable(string) });

/** What a session is, as every client's lists go by it (`kuvo/sessions/list`, PROTOCOL.md "Session
 * lists"): a project's session, a chat, a bot's conversation, a bot's seat in a group chat, a run
 * kept out of sight, or a subagent's. */
export const SESSION_KINDS = ["project", "chat", "bot", "seat", "hidden", "subagent"] as const;

/** One session's facts: its kind, the project it's filed under (a project's session's), its
 * folder, title and times (`updated`, its order), the session it works for (a subagent's), whether
 * it's pinned, archived, running, since when it's waited on you (ms), and whether this copy isn't
 * its own (it moved, or is on its way here). `thinking`: its model has been reasoning a while
 * (debounced, `bots.state`); `blocked`: it stopped on an error or a limit you haven't seen, or has
 * been retrying one a while (`retrying`, while it's at work); `unread`: when its turn you haven't
 * read ended (ms, its `time.idle` while that's later than `time.viewed`). `subagents`: a session's
 * subagents' sessions, those at work or waiting on you first, then newest first; `messages`: how a
 * subagent takes your messages (`direct`, through its `parent` that passes them on, or `none`, with
 * `note` saying why). Each flag left out where it isn't. */
const REMOTE_LIST_COMPUTER = object({
  ...LIST_COMPUTER.object, away: bool, "lastSeen?": number, "updating?": bool, actions: list(words("open", "manage")),
});

const CONVERSATION_TOOLS = object({
  directory: nullable(string),
  ready: object({ files: bool, changes: bool, browser: bool, terminal: bool }),
});
export const LOCAL_CONVERSATION_LOCATION = object({ place: words("engine", "agents"), running: bool, "tools?": CONVERSATION_TOOLS });
export const CONVERSATION_LOCATION = oneOf(LOCAL_CONVERSATION_LOCATION, object({ on: REMOTE_LIST_COMPUTER, "tools?": CONVERSATION_TOOLS }));

export const SESSION_FACTS = object({
  kind: words(...SESSION_KINDS),
  "computer?": LIST_COMPUTER, "on?": REMOTE_LIST_COMPUTER,
  "project?": string,
  directory: string,
  "title?": string,
  created: number,
  updated: number,
  "parent?": string,
  "subagents?": list(string),
  "messages?": object({ via: words("direct", "parent", "none"), "note?": string }),
  "line?": list(string),
  "reviewPart?": literal(true),
  "childOnly?": literal(true),
  "ownAsk?": literal(true),
  "childAsk?": literal(true),
  "pinned?": bool,
  "archived?": bool,
  "running?": bool,
  "wakeTurn?": literal(true),
  "asked?": number,
  "away?": bool,
  "bot?": string,
  "thinking?": literal(true),
  "blocked?": words("error", "limit"),
  "retrying?": literal(true),
  "unread?": number,
});

/** What "Show more" under a project's list adds next: `count` more, asked for with `cursor`
 * (`kuvo/sessions/page`). */
export const SHOW_MORE = object({ cursor: string, count: number });

/** A project the lists show: its name where it isn't its folder's, and when its newest session
 * listed in it changed (none for one with none); its list's first sessions in the order it shows
 * them (`shown`: pinned, then those at work or waiting on you, then the newest), how many it lists
 * in all (`total`), and what "Show more" adds (`more`, none once all show). None of the last three
 * from a box before them. */
export const LISTED_PROJECT = object({ "name?": string, "updated?": number, asking: number, working: number, "computer?": LIST_COMPUTER, "on?": REMOTE_LIST_COMPUTER, "shown?": list(string), "total?": number, "more?": SHOW_MORE });

/** A project's list as far as a cursor shows it (`kuvo/sessions/page?project=&cursor=`): its
 * sessions' ids in its order, how many it lists in all, and what "Show more" adds next. */
export const SESSION_PAGE = object({ project: string, sessions: list(string), total: number, more: nullable(SHOW_MORE) });

/** What a session waiting on you asks, in a few words: its kind, and the box's words for it, written
 * to follow the session's or its project's name ("asks to run a command"). A client shows `summary`
 * as it is, and one of a kind it doesn't know the same way. */
export const ASKS_WORDS = object({ kind: words("permission", "question", "sandbox", "vault", "other"), summary: string });
/** A session waiting on you, as "Needs you" lists it. */
export const NEEDS_YOU = object({ session: string, asks: ASKS_WORDS });
/** The one state a bot is in, as the box states it (`bots.state`; PROTOCOL.md, "What
 * waits on you"), first that holds: one of its sessions waits on you, one is blocked, every one at work
 * is thinking, one is at work, one has a turn you haven't seen, it's paused or its next wake-up is
 * more than 12 hours away, else idle. A client adds `offline` itself. */
export const BOT_STATES = ["idle", "working", "thinking", "needsYou", "blocked", "done", "asleep"] as const;
/** What a bot is doing now: how many of its sessions are at work, how many wait on you, and, with
 * neither, when the newest of them last changed (ms); its state (`bots.state`) and since when it's
 * been in it as this box saw it (ms; for `done`, when the newest turn you haven't seen ended), how
 * many of those at work are thinking and how many are blocked (each left out when none), and what
 * blocked the newest one. */
export const BOT_NOW = object({
  working: number, asking: number, "finished?": number, "waking?": literal(true),
  state: words(...BOT_STATES), since: number, "thinking?": number, "blocked?": number, "blockedBy?": words("error", "limit"),
});
/** What waits on you in each area, across the box (`kuvo/waiting`): the sessions "Needs you" lists,
 * the bots with a session waiting, and the Work Queue's items a session linked to them waits for. */
export const WAITING_ON_YOU = object({ sessions: number, bots: number, queue: number, "dock?": number, ...PENDING_COMPUTERS });

/** The list (`kuvo/sessions/list`): where it's at (`since` resumes from it), whether it's whole
 * (`full`) or only what changed since, the sessions by id, those gone since, its projects, and the
 * places it couldn't read now (`bridge`: the coding agents' sessions as they were last read). */
export const SESSION_LIST = object({ ...PENDING_COMPUTERS,
  at: string,
  "full?": bool,
  sessions: map(SESSION_FACTS),
  "gone?": list(string),
  projects: map(LISTED_PROJECT),
  "projectOrder?": list(string),
  "startProject?": nullable(string),
  archivedGroups: list(object({ directory: string, title: string, kind: words("project", "chat", "bot"), sessions: list(string) })),
  needsYou: list(NEEDS_YOU),
  "pending?": list(string),
});

// Full setup lists. Identity and availability are stamped by the receiving box, never by a peer.
const LIST_ORIGIN = { computer: LIST_COMPUTER, away: bool, "lastSeen?": number, "updating?": bool, "here?": bool } as const;
export const SETUP_SESSION = object({
  id: string, facts: SESSION_FACTS, actions: list(words("open", "manage")),
});
export const SETUP_PROJECT = object({ directory: string, facts: LISTED_PROJECT });
export const SETUP_WAIT = object({
  session: string, since: number, "title?": string, asks: ASKS_WORDS, actions: list(words("review")),
});
export const SETUP_QUEUE = object({
  session: string, queue: QUEUE, actions: list(words("open", "pause", "resume", "messages")),
});
const OFFERED_AGENT = object({ id: string, name: string });
export const START_CATALOG = object({ offered: MODELS_OFFERED, agents: list(OFFERED_AGENT) });
export const SETUP_SESSIONS = object({ ...PENDING_COMPUTERS,
  sessions: list(object({ ...SETUP_SESSION.object, ...LIST_ORIGIN })),
  projects: list(object({ ...SETUP_PROJECT.object, ...LIST_ORIGIN })),
});
export const SETUP_WAITING = object({ ...PENDING_COMPUTERS, waiting: list(object({ ...SETUP_WAIT.object, ...LIST_ORIGIN })) });
export const SETUP_QUEUES = object({ ...PENDING_COMPUTERS, queues: list(object({ ...SETUP_QUEUE.object, ...LIST_ORIGIN })) });
/** Where a new session can run: this computer (`here`) and each other computer with its last
 * catalog. One that's away is still listed, with when it was last seen, and can't be started on.
 * `pending`: this computer just started and its catalog is the one it saved last. */
export const RUNS_ON = object({ ...PENDING_COMPUTERS, computers: list(object({ computer: LIST_COMPUTER, "here?": bool, "pending?": bool, "away?": bool, "lastSeen?": number, "updating?": bool, ...START_CATALOG.object })) });

export type SessionKind = (typeof SESSION_KINDS)[number];
export type SessionFacts = Infer<typeof SESSION_FACTS>;
export type ListedProject = Infer<typeof LISTED_PROJECT>;
export type NeedsYou = Infer<typeof NEEDS_YOU>;
export type BotNow = Infer<typeof BOT_NOW>;
export type BotState = (typeof BOT_STATES)[number];
export type WaitingOnYou = Infer<typeof WAITING_ON_YOU>;
export type SessionList = Infer<typeof SESSION_LIST>;

export type Glance = Infer<typeof GLANCE>;
export type GlancePull = Infer<typeof GLANCE_PULL>;
export type GlanceChecks = Infer<typeof GLANCE_CHECKS>;

/** A side question about a conversation (PROTOCOL.md, "Conversations"), as
 * its box's agents bridge keeps it: the side thread it's in (`thread`, its first question's id),
 * where it stands, its answer or why it couldn't be answered, and when it was asked and answered (ms). */
export const SIDE_QUESTION = object({
  id: string, session: string, "thread?": string, question: string, "model?": object({ providerID: string, id: string, "variant?": string }), "files?": list(string),
  state: words("answering", "answered", "failed"),
  "answer?": string, "error?": string, asked: number, "answered?": number,
});
/** A conversation's side threads, each a side chat (`kuvo/agents/api/kuvo/btw/<session>/threads`),
 * the one with the latest activity first: its id, its first question as asked (`title`), when that
 * was asked and when anything in it last happened (ms), whether one in it is still being answered,
 * and its questions, oldest first. */
export const SIDE_THREADS = object({
  threads: list(object({ id: string, title: string, asked: number, latest: number, answering: bool, questions: list(SIDE_QUESTION) })),
});

export type SideThreads = Infer<typeof SIDE_THREADS>;

export const SESSION_ROUTES: RouteShape[] = [
  // The transcript of a coding-agent session, kept in the same format across box updates.
  { route: "GET kuvo/agents/api/session/:id/message", feature: "agents", answers: { 200: object({ data: list(any), cursor: object({ "next?": string }) }), 404: REFUSED, 503: REFUSED } },
  { route: "POST kuvo/agents/api/session/:id/prompt", feature: "agents", body: any, answers: { 200: object({}), 400: REFUSED, 404: REFUSED, 409: REFUSED, 503: REFUSED } },
  { route: "POST kuvo/agents/api/session/:id/interrupt", feature: "agents", answers: { 200: object({ interrupted: bool }), 404: REFUSED, 503: REFUSED } },
  // Stop what a coding-agent conversation left running besides its turn: the commands its agent left running.
  { route: "DELETE kuvo/agents/api/session/:id/background", feature: "agents", answers: { 200: object({}), 404: REFUSED, 503: REFUSED } },
  // Stop one command a coding agent runs (the bars over the composer): one its turn waits on stops that turn.
  { route: "DELETE kuvo/agents/api/shell/:id", feature: "agents", answers: { 200: object({}), 404: REFUSED } },
  // What one printed, from byte `cursor` on (`limit` bytes at most); only its end is kept, so what's asked from before that comes from where it begins.
  { route: "GET kuvo/agents/api/shell/:id/output", feature: "agents", answers: { 200: object({ location: object({ directory: string }), data: object({ output: string, cursor: number, size: number }) }), 404: REFUSED } },
  // The coding-agent conversations with a run under way now (`release-switch`: and the turns that
  // follow an engine model through this bridge).
  { route: "GET kuvo/agents/api/session/active", feature: "agents", answers: { 200: object({ data: any, "engineTurns?": list(string) }), 503: REFUSED } },
  // A coding-agent conversation as clients read it: how full its context is, its model's window
  // (`context=0`: as saved, without asking its agent) and where it's summarized.
  { route: "GET kuvo/agents/api/session/:id", feature: "agents", answers: { 200: object({ data: any }), 404: REFUSED, 503: REFUSED } },
  // A coding-agent conversation's model (with its effort as `variant`) from its next turn.
  { route: "POST kuvo/agents/api/session/:id/model", feature: "agents", body: any, answers: { 200: object({}), 400: REFUSED, 404: REFUSED, 503: REFUSED } },
  // A coding-agent conversation's mode (`build`, `plan`, an agent of yours) from its next message.
  { route: "POST kuvo/agents/api/session/:id/agent", feature: "agents", body: any, answers: { 200: object({}), 404: REFUSED, 503: REFUSED } },
  // Summarize a coding-agent conversation now, or once its turn ends (`waiting`).
  { route: "POST kuvo/agents/api/session/:id/compact", feature: "agents", answers: { 200: object({ data: object({ state: string }) }), 400: REFUSED, 404: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/agents/api/session/:id/permission", feature: "agents", answers: { 200: object({ data: list(any) }), 404: REFUSED, 503: REFUSED } },
  { route: "POST kuvo/agents/api/session/:id/permission/:request/reply", feature: "agents", body: any, answers: { 200: object({}), 400: REFUSED, 403: REFUSED, 404: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/agents/api/session/:id/form", feature: "agents", answers: { 200: object({ data: list(any) }), 404: REFUSED, 503: REFUSED } },
  { route: "POST kuvo/agents/api/session/:id/form/:request/reply", feature: "agents", body: any, answers: { 200: object({}), 404: REFUSED, 503: REFUSED } },
  { route: "DELETE kuvo/agents/api/session/:id/form/:request", feature: "agents", answers: { 200: object({}), 404: REFUSED, 503: REFUSED } },
  // Carry a conversation on with the coding agents, as picking one of their models does.
  { route: "POST kuvo/sessions/:id/agents", feature: "sessions.agents", answers: { 200: CARRIED } },
  // Give a conversation its project's other folders as a client opens it.
  { route: "POST kuvo/sessions/:id/folders", feature: "projects.folders", answers: { 200: object({ session: string }), 404: REFUSED, 405: REFUSED, 502: REFUSED, 503: REFUSED } },
  // Move a conversation into a project, given its folders; one at work moves at its next step.
  { route: "POST kuvo/sessions/:id/move", feature: "sessions.move", answers: { 200: SESSION_MOVED, 400: REFUSED, 404: REFUSED, 405: REFUSED, 409: REFUSED, 502: REFUSED, 503: REFUSED } },
  // Delete All's question for several sessions, in the box's words; then DELETE each.
  { route: "POST kuvo/sessions/deletes", feature: "sessions.actions", answers: { 200: CONFIRM, 400: REFUSED, 404: REFUSED, 405: REFUSED, 503: REFUSED } },
  // What a session's menu offers now, each with any confirm in the box's words; `held` while it moves.
  { route: "GET kuvo/sessions/:id/actions", feature: "sessions.actions", answers: { 200: SESSION_ACTIONS, 400: REFUSED, 404: REFUSED, 405: REFUSED, 409: LIVES_ELSEWHERE, 503: REFUSED } },
  // Archive or bring back (`archived`); with `was`, refused (409) if it changed since it was read.
  { route: "POST kuvo/sessions/:id/archive", feature: "sessions.actions", answers: { 200: SESSION_CHANGED, 400: REFUSED, 404: REFUSED, 405: REFUSED, 409: LIVES_ELSEWHERE, 502: REFUSED, 503: REFUSED } },
  // Pin or unpin (`pinned`); with `was`, refused (409) if it changed since it was read.
  { route: "POST kuvo/sessions/:id/pin", feature: "sessions.actions", answers: { 200: SESSION_CHANGED, 400: REFUSED, 404: REFUSED, 405: REFUSED, 409: LIVES_ELSEWHERE, 502: REFUSED, 503: REFUSED } },
  { route: "POST kuvo/sessions/:id/rename", feature: "sessions.actions", answers: { 200: SESSION_CHANGED, 400: REFUSED, 404: REFUSED, 405: REFUSED, 409: LIVES_ELSEWHERE, 502: REFUSED, 503: REFUSED } },
  // Delete a session where it runs, with its wake-ups and watches; one already gone counts as done.
  { route: "DELETE kuvo/sessions/:id", feature: "sessions.actions", answers: { 200: object({ session: string }), 400: REFUSED, 405: REFUSED, 409: LIVES_ELSEWHERE, 502: REFUSED, 503: REFUSED } },
  // Start a session here, with a task or empty for your first message; Runs on asks the chosen box.
  { route: "POST kuvo/sessions", feature: "sessions.start.empty", answers: { 200: STARTED, 400: REFUSED, 403: REFUSED, 404: REFUSED, 409: oneOf(object({ message: string, reason: words("fullAccessOff") }), object({ message: string, computer: LIST_COMPUTER })), 429: REFUSED, 502: object({ message: string, "session?": string }), 503: REFUSED } },
  // Answer one of Kuvo Code's own agent tools for a conversation; tool servers only pass calls on.
  { route: "POST kuvo/agent-tools", feature: "agent-tools", answers: { 200: TOOL_ANSWER, 400: REFUSED } },
  // Every session here whose queue is paused or holds something, now and at each change.
  { route: "GET kuvo/sessions/queues/events", feature: "queue.items", answers: { 200: object({ queues: map(QUEUE) }) } },
  // Each session's branch, pull request, account and since when it waits on you (`?sessions=`, ≤200).
  { route: "GET kuvo/sessions/glance", feature: "sessions.glance", answers: { 200: GLANCES, 400: REFUSED, 503: REFUSED } },
  // Mark All as Read: every conversation on this box marked read.
  { route: "POST kuvo/sessions/read", feature: "sessions.read", answers: { 200: object({ read: number }), 503: REFUSED } },
  { route: "GET kuvo/sessions/glance/events", feature: "sessions.glance", answers: { 200: GLANCES, 400: REFUSED, 503: REFUSED } },
  // This box's sessions as every client's lists go by them: facts, projects, Needs you.
  { route: "GET kuvo/sessions/list", feature: "sessions.list", answers: { 200: SESSION_LIST, 405: REFUSED, 503: REFUSED } },
  // The list now, then a line as anything changes; `since` resumes from a line's `at`.
  { route: "GET kuvo/sessions/list/events", feature: "sessions.list", answers: { 200: SESSION_LIST, 405: REFUSED, 503: REFUSED } },
  // A project's list as far as "Show more"'s cursor goes, in the order it shows.
  { route: "GET kuvo/sessions/page", feature: "sessions.list.pages", answers: { 200: SESSION_PAGE, 400: REFUSED, 405: REFUSED, 503: REFUSED } },
  // Where a session lives, from this box's kept lists: a runtime here, or the computer it's on.
  { route: "GET kuvo/sessions/:session/location", feature: "sessions.location", answers: { 200: CONVERSATION_LOCATION, 404: REFUSED, 405: REFUSED, 503: REFUSED } },
  // Where the owning computer runs its session (`place`, `running`), read through this box.
  { route: "GET kuvo/peer-session/:computer/:session/location", feature: "sessions.location", answers: { 200: LOCAL_CONVERSATION_LOCATION, 403: REFUSED, 404: REFUSED, 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  // A session on another computer, as its owner has it, read through this box.
  { route: "GET kuvo/peer-session/:computer/:session/session", feature: "line.peer-conversation", answers: { 200: any, 403: REFUSED, 404: REFUSED, 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  // A page of a session's messages from its owning computer (the usual message query).
  { route: "GET kuvo/peer-session/:computer/:session/messages", feature: "line.peer-conversation", answers: { 200: any, 403: REFUSED, 404: REFUSED, 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  // A session's live events from its owning computer, through this box.
  { route: "GET kuvo/peer-session/:computer/:session/events", feature: "line.peer-conversation", answers: { 200: BYTES, 403: REFUSED, 404: REFUSED, 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  // What a session on another computer asks you; answer on the owner's own routes.
  { route: "GET kuvo/peer-session/:computer/:session/asks", feature: "line.peer-conversation", answers: { 200: SESSION_ASKS, 403: REFUSED, 404: oneOf(REFUSED, SESSION_ASKS), 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/peer-session/:computer/:session/asks/events", feature: "line.peer-conversation", answers: { 200: SESSION_ASKS, 403: REFUSED, 404: oneOf(REFUSED, SESSION_ASKS), 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  // A session's queue from its owning computer; queue actions go to the owner's routes.
  { route: "GET kuvo/peer-session/:computer/:session/queue", feature: "line.peer-conversation", answers: { 200: QUEUE_READ, 403: REFUSED, 404: REFUSED, 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/peer-session/:computer/:session/queue/events", feature: "line.peer-conversation", answers: { 200: QUEUE_SHOWN, 403: REFUSED, 404: REFUSED, 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  // Follow a message receipt kept on this box, as `GET kuvo/data/messages/:operation` says it.
  { route: "GET kuvo/data/messages/:operation/events", feature: "typed.delivery", answers: { 200: any, 404: REFUSED, 405: REFUSED } },
  // Leave a composer draft with this box to deliver to its owner (`?computer=`), once per id.
  { route: "POST kuvo/sessions/:id/delivery", feature: "typed.delivery", answers: { 200: any, 202: any, 400: REFUSED, 403: REFUSED, 404: REFUSED, 405: REFUSED, 409: REFUSED, 503: REFUSED } },
  // The drafts still waiting at this box to reach the conversation's owner.
  { route: "GET kuvo/sessions/:id/delivery", feature: "typed.delivery", answers: { 200: QUEUE_SHOWN, 400: REFUSED, 403: REFUSED, 404: REFUSED } },
  // The owner's queue plus drafts waiting here; `deliveryLine` while the owner is away.
  { route: "GET kuvo/sessions/:id/delivery/events", feature: "typed.delivery", answers: { 200: QUEUE_SHOWN, 400: REFUSED, 403: REFUSED, 404: REFUSED } },
  // Take back a draft still waiting here; a `receipt` makes the removal replayable.
  { route: "DELETE kuvo/sessions/:id/delivery/:message", feature: "typed.delivery", answers: { 200: any, 403: REFUSED, 404: REFUSED, 409: REFUSED } },
  // Have a draft still waiting here go to its owner as a steer.
  { route: "POST kuvo/sessions/:id/delivery/:message/steer", feature: "typed.delivery", answers: { 200: any, 403: REFUSED, 404: REFUSED, 409: REFUSED } },
  // Clear a removed draft's recovery copy once the client restored it (`receipt`).
  { route: "POST kuvo/sessions/:id/delivery/:message/ack", feature: "typed.delivery", answers: { 200: any, 403: REFUSED, 404: REFUSED } },
  // Have a draft still waiting here go to its owner as Send Now.
  { route: "POST kuvo/sessions/:id/delivery/:message/now", feature: "typed.delivery", answers: { 200: any, 403: REFUSED, 404: REFUSED, 409: REFUSED } },
  // The session list merged across your computers, in `kuvo/sessions/list`'s shape.
  { route: "GET kuvo/setup/sessions/list/events", feature: "line.merged-sessions", answers: { 200: SESSION_LIST, 405: REFUSED, 503: REFUSED } },
  // Every computer's sessions and projects as rows naming their computer, whole each time.
  { route: "GET kuvo/setup/sessions/events", feature: "line.setup-lists", answers: { 200: SETUP_SESSIONS, 405: REFUSED } },
  // What waits on you across the setup, as one total of sessions, bots and Work Queue items.
  { route: "GET kuvo/setup/waiting/counts", feature: "line.merged-waiting", answers: { 200: WAITING_ON_YOU, 405: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/setup/waiting/counts/events", feature: "line.merged-waiting", answers: { 200: WAITING_ON_YOU, 405: REFUSED } },
  // Every computer's sessions waiting on you, each with its oldest ask in the box's words.
  { route: "GET kuvo/setup/waiting/events", feature: "line.setup-lists", answers: { 200: SETUP_WAITING, 405: REFUSED } },
  // Every computer's paused or holding queues, with the actions each row offers.
  { route: "GET kuvo/setup/queues/events", feature: "line.setup-lists", answers: { 200: SETUP_QUEUES, 405: REFUSED } },
  // Runs on: each computer a new session can start on, with its models and coding agents.
  { route: "GET kuvo/setup/runs-on/events", feature: "line.setup-lists", answers: { 200: RUNS_ON, 405: REFUSED } },
  // How many things wait on you in each area on this box (sessions, bots, Work Queue), for badges.
  { route: "GET kuvo/waiting", feature: "waiting", answers: { 200: WAITING_ON_YOU, 405: REFUSED } },
  { route: "GET kuvo/waiting/events", feature: "waiting", answers: { 200: WAITING_ON_YOU, 405: REFUSED } },
  // Each session's model, whether it runs, and the composer's refusal and action, in the box's words.
  { route: "GET kuvo/sessions/models", feature: "sessions.models", answers: { 200: SESSION_MODELS, 400: REFUSED, 503: REFUSED } },
  // The models a picker offers here, in order, and the one a new session starts on.
  { route: "GET kuvo/models/offered", feature: "models.offered", answers: { 200: MODELS_OFFERED, 404: REFUSED, 503: REFUSED } },
  // Your starred models, kept by the box so every picker shows the same stars.
  { route: "GET kuvo/models/starred", feature: "models.starred", answers: { 200: MODELS_STARRED } },
  // Star or unstar one model (`starred: true|false`), setup-wide: a member passes it to the main.
  { route: "POST kuvo/models/starred", feature: "models.starred", answers: { 200: MODELS_STARRED, 400: REFUSED, 409: REFUSED, 503: REFUSED } },
  // Pick the speed a provider's turns run at on this box (`speed`: a speed's id, null for Standard).
  { route: "POST kuvo/models/speed", feature: "models.speed", answers: { 200: MODELS_SPEED, 400: REFUSED } },
  // A session's queue: paused, held, what waits on the box; `?items=1` adds every message in order.
  { route: "GET kuvo/sessions/:id/queue", feature: "queue.items", answers: { 200: QUEUE_READ } },
  { route: "GET kuvo/sessions/:id/queue/events", feature: "queue.items", answers: { 200: QUEUE_SHOWN } },
  // What a conversation waits on you for, as the cards over its composer show it, oldest first.
  { route: "GET kuvo/sessions/:id/asks", feature: "sessions.asks.box", answers: { 200: SESSION_ASKS, 404: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/sessions/:id/asks/events", feature: "sessions.asks.box", answers: { 200: SESSION_ASKS, 404: REFUSED, 503: REFUSED } },
  // Answer one ask (a choice, words, or dismiss), sent live to where the session runs.
  { route: "POST kuvo/sessions/:id/asks/:ask", feature: "sessions.asks.box", answers: { 200: NOTHING, 400: REFUSED, 403: REFUSED, 404: REFUSED, 409: REFUSED, 423: REFUSED, 502: REFUSED, 503: REFUSED } },
  // What "@" (`for=files`) or "&" (`for=agents`) suggest for what's typed, ranked by the box.
  { route: "GET kuvo/sessions/:id/mentions", feature: "sessions.mentions", answers: { 200: MENTIONS, 400: REFUSED, 404: REFUSED, 503: REFUSED } },
  // Stop: hold and pause the queue, then stop the run; `items` holds sends still on their way.
  { route: "POST kuvo/sessions/:id/stop", feature: "queue.stop.arriving", answers: { 200: QUEUE, 404: REFUSED, 502: REFUSED, 503: REFUSED } },
  // Hold and pause the queue without stopping the run (before an undo or a move).
  { route: "POST kuvo/sessions/:id/queue/hold", feature: "queue.items", answers: { 200: QUEUE, 404: REFUSED, 502: REFUSED, 503: REFUSED } },
  // Move a waiting message before or after another (`item`, `anchor`, `before`).
  { route: "POST kuvo/sessions/:id/queue/reorder", feature: "queue.items", answers: { 200: QUEUE_SHOWN, 400: REFUSED, 404: REFUSED, 409: REFUSED, 502: REFUSED, 503: REFUSED, 504: REFUSED } },
  // Unpause the queue; the next message goes now.
  { route: "POST kuvo/sessions/:id/queue/resume", feature: "queue.items", answers: { 200: QUEUE, 404: REFUSED, 502: REFUSED, 503: REFUSED } },
  // Send Now: unpause, send this message, and stop the run under way to go on with it.
  { route: "POST kuvo/sessions/:id/queue/:item/now", feature: "queue.items", answers: { 200: QUEUE, 404: REFUSED, 502: REFUSED, 503: REFUSED } },
  // Steer: unpause and hand this message to the run under way at its next step.
  { route: "POST kuvo/sessions/:id/queue/:item/steer", feature: "queue.steer", answers: { 200: QUEUE, 404: REFUSED, 502: REFUSED, 503: REFUSED } },
  // Add a message to the queue (Undo puts one back with `before`); it waits on the box while paused.
  { route: "POST kuvo/sessions/:id/queue/add", feature: "queue.items", answers: { 200: ADDED, 400: REFUSED, 404: REFUSED, 502: REFUSED, 503: REFUSED } },
  // Clear a removal's recovery copy (`receipt`) once the client has used it.
  { route: "POST kuvo/sessions/:id/queue/:item/ack", feature: "queue.items", answers: { 200: QUEUE, 400: REFUSED, 502: REFUSED } },
  // (The coding agents' bridge's, passed on by the box's link as it came.)
  { route: "GET kuvo/agents/api/kuvo/btw/:session/threads", feature: "btw.threads.events", answers: { 200: SIDE_THREADS, 404: REFUSED, 503: REFUSED } },
  { route: "GET kuvo/agents/api/kuvo/btw/:session/threads/events", feature: "btw.threads.events", answers: { 200: SIDE_THREADS, 404: REFUSED, 503: REFUSED } },
  // Take a waiting message away, whole, for Edit or Undo; `?receipt=` makes it replayable.
  { route: "DELETE kuvo/sessions/:id/queue/:item", feature: "queue.items", answers: { 200: object({ paused: bool, held: list(string), "waiting?": list(WAITING), "line?": string, "removed?": REMOVED }), 400: REFUSED, 404: object({ message: string, "delivered?": bool }), 409: object({ message: string, "arriving?": bool }), 502: REFUSED, 503: REFUSED } },
];
