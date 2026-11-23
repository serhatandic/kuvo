// The sandbox agents' commands run in on a box (PROTOCOL.md, "Per area").

import { any, bool, type Infer, list, literal, NOTHING, number, object, type RouteShape, string, words } from "../shape";

/** Whether every command the engine runs here goes through the sandbox (checked; it's on wherever
 * it can run, with no switch), why not, whether this computer can at all, and whether the coding agents' own programs'
 * commands are sandboxed too (`outside`: one started before it was on still runs outside it), and whether it asks you to restart those started before it was on
 * (`restart`, once: `POST kuvo/sandbox/restart`, `{restart}` true or false). When it isn't on: why, as a word a client maps (`reason`), what you
 * can do (`action`), the words of its button (`actionLabel`) and the box's route that does it (`route`), or, with none, the address
 * its button opens (`opens`: the docs on what a machine needs, or this computer's own Software Update, which opens only on this
 * computer itself, `opensHere`); what the computer needs first
 * (`needs`), which part of the check failed (`check`), and how many checks failed in a row. While
 * it's on, what it can't keep commands from here (`gaps`: `services`, the services your login runs,
 * where commands can't have a namespace of their own; `needs` says what for; `git`, git's own files in
 * the repositories commands write, where a folder can't be made read-only in their namespace, so
 * commands in a repository don't run there). With `services`, it says why (`why`) and the docs' page
 * on what a machine needs (`action` `help`, `actionLabel`, `opens`). Once its check failed twice in a
 * row, what a report of it says (`report`): a client offers Report a Problem then. Whether its row
 * warns (`needsYou`: off, or on with `services`), the one fact every client draws it by. */
export const SANDBOX = object({
  on: bool, "why?": string, able: bool, agents: bool, "outside?": bool, "restart?": bool,
  "reason?": words("windows", "kernel", "namespaces", "macos", "engineMissing", "checking", "checkFailed", "plugin", "engineDown", "waiting"),
  "action?": words("help", "softwareUpdate", "retry", "fix", "restartEngine"),
  "route?": string, "actionLabel?": string, "opens?": string, "opensHere?": literal(true), "needs?": string,
  "check?": words("folder", "tempFiles", "readsOwn", "writesOwn", "services", "didntRun", "setup"),
  "failures?": number, "report?": string,
  "gaps?": list(words("services", "git")),
  "needsYou?": literal(true),
});

export type SandboxState = Infer<typeof SANDBOX>;

/** What the sandbox refused a command, from the system's own log (a Mac's): a write, a read, a
 * connection, starting a job or an app; with its path where it has one. */
export const SANDBOX_STOP = object({ kind: words("write", "read", "connect", "job", "app", "other"), "path?": string });

/** Whose a conversation's folder is, as Also Allowed groups its rules and an ask's card names it: a
 * project's, a bot's own (`id`, the bot's; its folder's workspace) or the chats' (one folder, so a
 * rule there is every chat's). `name`: a project's folder's name, the bot's, or "Chats". */
export const SANDBOX_OWNER = object({ kind: words("project", "bot", "chats"), "id?": string, name: string });

/** A rule kept in Also Allowed: a folder (or file) a project's commands may write in the sandbox,
 * or a bot's own, or the chats'. (Never a command run outside it without asking: whatever a
 * command runs, its scripts, its project's settings, is what the sandbox writes, so that would let
 * anything out.) `owner`: whose (a box from before it says none); `gone`: its project's folder
 * isn't there any more (a bot moved away, a project deleted), shown so it can be removed. */
export const SANDBOX_RULE = object({
  id: string, project: string, kind: words("write"), path: string,
  at: number, "by?": string, "owner?": SANDBOX_OWNER, "gone?": bool,
});
/** `owners`: the bots here working in their own folder and the chats, which a client may add a
 * folder for (`POST` with `owner`), besides the projects it knows. */
export const SANDBOX_ALLOWED = object({ allowed: list(SANDBOX_RULE), "owners?": list(SANDBOX_OWNER) });

/** Why Auto's reviewer brought something to you: in a sentence to show, and as a word a client
 * maps (`always`: it always came to you, on a bot's, before bridge 0.9.189, and which kind; `reviewer`: the reviewer asked;
 * `unread`: a script it runs couldn't be read whole first; `unreadable`, `noModel`, `failed`: it
 * couldn't be reviewed; `paused`: Auto is paused there). */
export const REVIEW_ASKED = object({
  reason: string,
  why: words("always", "reviewer", "unread", "unreadable", "noModel", "failed", "paused"),
  "always?": words("payment", "delete", "permissions", "account", "message", "credentials", "gitConfig"),
});

/**
 * A command an agent wants to run outside the sandbox (its `sandbox_ask`), waiting for your answer:
 * the conversation and its project, its bot, the command, why, what the sandbox stopped (`stopped`,
 * from a Mac's log) or the error the command got (`error`), and what Always would allow (`always`:
 * the one folder it was stopped writing in, when the box knows it and it may be given; none, and
 * it's only once or no, for anything else). `machine`: the
 * computer's name, on a machine. `owner`: whose its folder is, which Always would give it to.
 * `review`: in Auto, why its reviewer brought it to you (`reason`, a sentence to show; `why`: it
 * came to you on a bot's before bridge 0.9.189, the reviewer asked, or it couldn't be reviewed,
 * and what kind for the first).
 */
export const SANDBOX_ASK = object({
  id: string, at: number,
  "session?": string, "sessionName?": string, "bot?": string, "botName?": string,
  project: string, command: string, why: string,
  "stopped?": list(SANDBOX_STOP), "error?": string,
  "always?": object({ kind: words("write"), path: string }),
  "machine?": string, "owner?": SANDBOX_OWNER, "review?": REVIEW_ASKED,
});
/**
 * How an ask to work outside a conversation's folder was taken up, in Auto: being checked by its
 * reviewer, allowed (by it, or in the conversation's own project), held back for you to approve,
 * refused, or left for you to answer (`yours`: in Auto, a client shows its card only then). `call`:
 * the tool call it’s for. The latest two hundred, from bridge 0.9.192.
 */
export const ASK_REVIEW = object({
  id: string, session: string, "call?": string,
  state: words("checking", "allowed", "heldBack", "refused", "yours"), at: number,
});
/**
 * An ask Auto's reviewer was checking or had left for you that's over without its word: `answered`
 * (by you, on any client, or by its engine) or `gone` (its run ended, its session was deleted, or
 * its engine let it go while the box wasn't listening). A client never takes one listed so for one
 * that waits, heard or not. The latest two hundred (`sandbox.reviews.settled`).
 */
export const SETTLED_REVIEW = object({
  id: string, session: string, "call?": string,
  state: words("answered", "gone"), at: number,
});
export const SANDBOX_ASKS = object({ asks: list(SANDBOX_ASK), "restart?": bool, "reviews?": list(ASK_REVIEW), "settled?": list(SETTLED_REVIEW) });

/** The agents' sandbox tool (`sandbox_ask`), as a tool server lists it, and what a call answers. */
export const SANDBOX_TOOL_LIST = object({ tools: list(any) });
export const SANDBOX_TOOL = object({ text: string, isError: bool });
/** A command the sandbox stopped, as the engine's plugin tells the box (`POST kuvo/sandbox/stopped`,
 * `{session, input, output}`): in Auto, what the agent gets instead, its reviewer having run it
 * again outside the sandbox (`text`, and how it ended: `exit`), or why not, added after what it
 * printed (`text` alone); `{}` leaves it as it was. */
export const SANDBOX_STOPPED = object({ "text?": string, "exit?": number });

export type SandboxStop = Infer<typeof SANDBOX_STOP>;
export type SandboxRule = Infer<typeof SANDBOX_RULE>;
export type SandboxOwner = Infer<typeof SANDBOX_OWNER>;
export type SandboxAsk = Infer<typeof SANDBOX_ASK>;

export const SANDBOX_ROUTES: RouteShape[] = [
  // Whether agents' commands here run in the sandbox, why not, and what a client offers to put it right.
  { route: "GET kuvo/sandbox", feature: "sandbox", answers: { 200: SANDBOX } },
  // Sets the sandbox up and checks it again now; answers the state.
  { route: "POST kuvo/sandbox/check", feature: "sandbox.check", answers: { 200: SANDBOX } },
  // Answers its ask to restart agents started before it was on (`{restart}`: true restarts them, false not).
  { route: "POST kuvo/sandbox/restart", feature: "sandbox", answers: { 200: SANDBOX, 503: object({ message: string }) } },
  // Commands agents ask to run outside the sandbox, and how Auto's reviewer took up folder asks.
  { route: "GET kuvo/sandbox/asks", feature: "sandbox.asks", answers: { 200: SANDBOX_ASKS } },
  { route: "GET kuvo/sandbox/asks/events", feature: "sandbox.asks", answers: { 200: SANDBOX_ASKS } },
  // Answers one (`{answer: once | always | no}`); `always` without the ask's `always` counts as once.
  { route: "POST kuvo/sandbox/asks/:id", feature: "sandbox.asks", answers: { 200: NOTHING } },
  // Also Allowed: the folders a project's, a bot's or the chats' commands may write in the sandbox.
  { route: "GET kuvo/sandbox/allowed", feature: "sandbox", answers: { 200: SANDBOX_ALLOWED } },
  // Adds a folder to Also Allowed (at most 8 a project; never home, system or Kuvo Code folders, or secrets).
  { route: "POST kuvo/sandbox/allowed", feature: "sandbox", answers: { 200: SANDBOX_ALLOWED } },
  { route: "DELETE kuvo/sandbox/allowed/:id", feature: "sandbox", answers: { 200: SANDBOX_ALLOWED } },
  // The agents' `sandbox_ask` tool, for a tool server to list (only while the sandbox is on).
  { route: "GET kuvo/sandbox/tools", feature: "sandbox", answers: { 200: SANDBOX_TOOL_LIST } },
  // Runs a `sandbox_ask` call (`{session, name, arguments}`), only while that conversation is making it.
  { route: "POST kuvo/sandbox/tools", feature: "sandbox", answers: { 200: SANDBOX_TOOL } },
  // From the engine's plugin: a command the sandbox stopped; in Auto its reviewer may run it again outside.
  { route: "POST kuvo/sandbox/stopped", feature: "sandbox", answers: { 200: SANDBOX_STOPPED } },
];
