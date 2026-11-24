// A box's folders to choose one from (`kuvo/folders`), those worth starting in
// (`kuvo/folders/suggested`), every file under one
// (`kuvo/folders/tree`), the files under one that what's typed finds (`kuvo/files/find`), npm's plugins its engine can
// load (`kuvo/plugins/search`), an MCP server's program (`kuvo/programs`), a provider of your
// own's models (`kuvo/providers/models`), a sign-in's return loaded on the computer it's for
// (`kuvo/sign-in/return`), and the ways to sign in to an account and how a sign-in is going, in the
// box's words (`kuvo/sign-in/methods`, `kuvo/sign-in/status`).

import { any, bool, type Infer, list, literal, nullable, number, object, type RouteShape, string, words } from "../shape";

/** The folders in one of this box's folders: its path, the home folder, the one it's in (null for
 * the top), and the folders in it, ordered as Finder orders names (`repository`: a git
 * repository's own checkout); asked with `files=1`, its files too, as the Files panel lists them. */
export const FOLDER_LIST = object({
  path: string,
  home: string,
  parent: nullable(string),
  folders: list(object({ name: string, "path?": string, "repository?": bool })),
  "files?": list(string),
  "fileEntries?": list(object({ name: string, path: string })),
});
/** Every file under one of this box's folders, by its path there, as a project's file filter and
 * "@" list them: git's list in a repository (`git`), else a walk that skips the usual heavy folders;
 * asked with `dated=1`, when each last changed (milliseconds since 1970, null where it can't say). */
export const FOLDER_TREE = object({ path: string, files: list(string), git: bool, "modified?": list(nullable(number)) });
/** Folders worth starting in on this box's computer (`kuvo/folders/suggested`): the home folder,
 * and each folder's full path, its name, `repository` where it's a git repository, and when an
 * agent last worked there (`seen`, ms; null for a repository in ~/Projects or ~/Developer no
 * session here worked in), those worked in first, newest first. */
export const FOLDERS_SUGGESTED = object({ home: string, folders: list(object({ path: string, name: string, "repository?": bool, seen: nullable(number) })) });
/** Why a folder couldn't be read, as the computer says it ("Permission denied"). */
export const FOLDER_REFUSED = object({ message: string, "reason?": string });
/** A new folder, made where it was asked. */
export const FOLDER_MADE = object({ path: string });
/** The files under one of this box's folders that what's typed finds, best first
 * (`kuvo/files/find`): each one's full path, its name, and the folder it's in there ("" at the
 * top); and the web address the text reads as, with its scheme (`url`, null where it reads as none). */
export const FILES_FOUND = object({ url: nullable(string), files: list(object({ path: string, name: string, folder: string })) });

/** npm's packages a search finds, the one it names exactly first: each one's name, version, what
 * it says it is and its downloads last week, where npm says; `missing`, a package (or version)
 * named that npm doesn't have. */
export const PLUGIN_SEARCH = object({
  packages: list(object({ name: string, "version?": string, "description?": string, "weekly?": number })),
  "missing?": string,
});

/** A program an MCP server's command names, and where this box runs it from (null: it hasn't it,
 * or, `unknown`, it can't tell from here). */
export const PROGRAM_FOUND = object({ name: string, path: nullable(string), "unknown?": bool });

/** A provider of your own's models, as it lists them to this box: `reached` and the models, or
 * `needsKey` when it asks for its key first; not reached, what's wrong. */
export const PROVIDER_MODELS = object({ reached: bool, "models?": list(string), "needsKey?": bool, "message?": string });

/** The status the computer's own address answered a sign-in's return with (2xx or 3xx: it took it). */
export const SIGN_IN_RETURNED = object({ status: number });

/** The ways to sign in to an account, in the order a client offers them: each the engine's (`type`,
 * `id`, `form`) with the box's `label`, `legacy` for one offered apart, after the others; and the
 * environment variables that sign in to it too. */
const SIGN_IN_WAYS = {
  methods: list(object({ type: string, "id?": string, label: string, "form?": list(any), "legacy?": bool })),
  environment: list(string),
  hint: string,
} as const;
/** (`hint`: the way in an account's card names under its name: "Sign in with ChatGPT", "API key".) */
export const SIGN_IN_METHODS = object(SIGN_IN_WAYS);
/** Every account the engine (or the coding agents' bridge) lists, each with its ways in; what it is
 * (`kind`: a provider, a web search service, an MCP server's sign-in, or the engine's own way to a
 * plan a coding agent brings) and whether Accounts lists it (`listed`); and its mark, by the plan
 * limits' service ids (`service`, left out for one with no mark: a client draws its letter). How it
 * stands (`state`), the line under its name (`line`), each of its sign-ins and keys with its own
 * line (`connections`, the one in use first), and what can be done with it now (`actions`: each
 * with its button's words, `confirm` where a client asks first, in the box's words, and `here` where
 * it only works on this box's computer), as the box decides them (`sign-in.accounts.state`);
 * `added` for an account you added beside a coding agent's own sign-in. */
const SIGN_IN_STATE = {
  state: words("signedIn", "signedOut", "ended", "notInstalled", "unchecked", "problem"),
  line: string,
  actions: list(object({
    id: words("signIn", "signInAgain", "addAnother", "disconnect", "install", "update", "signOut", "rename", "remove"),
    label: string,
    "sheetTitle?": string,
    "confirm?": object({ title: string, message: string, label: string }),
    "here?": literal(true),
  })),
  connections: list(object({ "id?": string, state: words("signedIn", "ended", "problem"), line: string })),
  "added?": literal(true),
} as const;
const SIGN_IN_ACCOUNT = { id: string, ...SIGN_IN_WAYS, kind: words("provider", "search", "mcp", "agentDuplicate"), listed: bool, plan: bool, "service?": string, ...SIGN_IN_STATE } as const;
export const SIGN_IN_ACCOUNTS = object({ accounts: list(object(SIGN_IN_ACCOUNT)) });

const CATALOG_ACCOUNT = object({
  ...SIGN_IN_ACCOUNT,
  name: string,
  section: words("codingAgents", "providers", "addProvider"),
  metadata: object({ "accountLabel?": string, "version?": string, "programUpdate?": object({ state: string, "version?": string, "message?": string }) }),
});
const CATALOG_UNAVAILABLE = object({ message: string, "reason?": string, "action?": object({ label: string, route: string }) });
const ACCOUNT_CATALOG = object({ agents: list(CATALOG_ACCOUNT), engine: list(CATALOG_ACCOUNT), "agentsDown?": CATALOG_UNAVAILABLE, "engineDown?": CATALOG_UNAVAILABLE });

/** A sign-in started in the browser (`POST kuvo/sign-in/start`): what the engine (or the coding
 * agents' bridge) answered (`attemptID`, its page's `url`, `instructions`, how it ends: `mode`), and
 * the code its page asks for, apart (`userCode`), where it has one. */
export const SIGN_IN_STARTED = object({ "attemptID?": string, "url?": string, "instructions?": string, "mode?": string, "acceptsCode?": bool, "pasteAddress?": bool, "returnToBox?": string, "userCode?": string, "time?": any });

/** How a started sign-in is going, as the engine (or the coding agents' bridge) says, with, once it
 * ended without signing in (`failed`, `expired`), what a client says of it (`failure`). Pending, how
 * it ends as it stands now (`mode`: `code`, on a code to paste here; `auto`, it comes back by
 * itself), which a page that comes late can change, and whether a code can be pasted (`acceptsCode`). */
export const SIGN_IN_STATUS = object({
  status: string,
  "message?": string,
  "url?": string,
  "instructions?": string,
  "mode?": string,
  "acceptsCode?": bool,
  "userCode?": string,
  "pasteAddress?": bool,
  "returnToBox?": string,
  "failure?": string,
});

export type FolderList = Infer<typeof FOLDER_LIST>;
export type FolderMade = Infer<typeof FOLDER_MADE>;
export type FoldersSuggested = Infer<typeof FOLDERS_SUGGESTED>;
export type FolderRefused = Infer<typeof FOLDER_REFUSED>;
export type FilesFound = Infer<typeof FILES_FOUND>;
export type PluginSearch = Infer<typeof PLUGIN_SEARCH>;
export type ProgramFound = Infer<typeof PROGRAM_FOUND>;
export type ProviderModels = Infer<typeof PROVIDER_MODELS>;
export type SignInReturned = Infer<typeof SIGN_IN_RETURNED>;
export type SignInMethods = Infer<typeof SIGN_IN_METHODS>;
export type SignInAccounts = Infer<typeof SIGN_IN_ACCOUNTS>;
export type SignInStatus = Infer<typeof SIGN_IN_STATUS>;

/** Sessions whose conversation has what a search asked for, the latest first (`kuvo/search`):
 * each one's ID, the words around the first mention, and the session as the engine lists it
 * (`info`), for a client that doesn't list it (an older one, archived). Words a subagent's
 * conversation has find the one it works for, with that subagent's session as listed
 * (`subagent`). */
const SEARCH_COMPUTER = object({ id: string, name: string });
export const SEARCH_FOUND = object({
  results: list(object({ session: string, snippet: string, info: any, "subagent?": any, "computer?": SEARCH_COMPUTER })),
  "awayComputers?": list(SEARCH_COMPUTER), "unavailableComputers?": list(SEARCH_COMPUTER),
});

export type SearchFound = Infer<typeof SEARCH_FOUND>;

export const MISC_ROUTES: RouteShape[] = [
  // The folders in one of this computer's folders, to choose one from (`files=1`: its files too).
  { route: "GET kuvo/folders", feature: "folders", answers: { 200: FOLDER_LIST, 403: FOLDER_REFUSED, 404: FOLDER_REFUSED, 500: FOLDER_REFUSED } },
  // Make a new folder where asked.
  { route: "POST kuvo/folders", feature: "folders", answers: { 200: FOLDER_MADE } },
  // Folders worth starting in here: those agents worked in first, newest first, then repositories.
  { route: "GET kuvo/folders/suggested", feature: "folders.suggested", answers: { 200: FOLDERS_SUGGESTED } },
  // Every file under a folder, for a project's file filter and "@" (`dated=1`: when each changed).
  { route: "GET kuvo/folders/tree", feature: "folders.tree", answers: { 200: FOLDER_TREE, 400: object({ message: string }), 403: FOLDER_REFUSED, 404: FOLDER_REFUSED, 500: FOLDER_REFUSED, 503: object({ message: string }) } },
  // The files under a folder that what's typed finds, best first, and the web address it reads as.
  { route: "GET kuvo/files/find", feature: "files.find", answers: { 200: FILES_FOUND, 400: object({ message: string }), 403: FOLDER_REFUSED, 404: FOLDER_REFUSED, 500: FOLDER_REFUSED, 503: object({ message: string }) } },
  // npm packages a search finds, for plugins the engine can load.
  { route: "GET kuvo/plugins/search", feature: "plugins.search", answers: { 200: PLUGIN_SEARCH } },
  // Whether this computer has the program an MCP server's command names, found as the engine finds it.
  { route: "GET kuvo/programs", feature: "programs", answers: { 200: PROGRAM_FOUND } },
  // The models a provider of your own lists at its address, asked from this computer.
  { route: "POST kuvo/providers/models", feature: "providers.models", answers: { 200: PROVIDER_MODELS } },
  // Load a sign-in's return address on the computer it's for, so the sign-in finishes there.
  { route: "POST kuvo/sign-in/return", feature: "sign-in.return", body: object({ url: string, "integration?": string, "attempt?": string, "agents?": bool }), answers: { 200: SIGN_IN_RETURNED, 400: object({ message: string }), 502: object({ message: string }) } },
  // The ways to sign in to an account, in the order a client offers them.
  { route: "GET kuvo/sign-in/methods", feature: "sign-in.methods", answers: { 200: SIGN_IN_METHODS } },
  // Every account the engine or the coding agents list, how it stands and what can be done now.
  { route: "GET kuvo/sign-in/accounts", feature: "sign-in.accounts.state", answers: { 200: SIGN_IN_ACCOUNTS } },
  // One account catalog for Settings and the first run, each source's refusal where it's down.
  { route: "GET kuvo/accounts/catalog", feature: "accounts.catalog", answers: { 200: ACCOUNT_CATALOG } },
  // Complete or cancel a sign-in, add a key, or connect credentials the computer already has.
  { route: "POST kuvo/sign-in/action", feature: "sign-in.action", body: object({
    action: words("cancel", "complete", "addKey", "external"),
    "attempt?": string, "code?": string, "key?": string, "methodID?": string, "answer?": any,
  }), answers: { 200: any, 400: object({ message: string }) } },
  // How a started sign-in is going, and what a client says once it ended without signing in.
  { route: "GET kuvo/sign-in/status", feature: "sign-in.codes", answers: { 200: SIGN_IN_STATUS } },
  // Start signing in by one of an account's ways: its page, code and how it ends.
  { route: "POST kuvo/sign-in/start", feature: "sign-in.methods", body: object({ methodID: string, "answer?": any, "browserReturn?": bool }), answers: { 200: SIGN_IN_STARTED, 400: object({ message: string }), 409: object({ message: string }), 502: object({ message: string }), 503: object({ message: string }), 504: object({ message: string }) } },
  // The native sign-in for a coding-agent account.
  { route: "POST kuvo/agents/api/integration/:provider/connect/oauth", feature: "agents.accounts", body: object({ methodID: string, "remote?": bool, "browserReturn?": bool }), answers: { 200: object({ data: SIGN_IN_STARTED, location: object({ directory: string }) }), 400: object({ message: string }), 404: object({ message: string }), 503: object({ message: string }) } },
  { route: "GET kuvo/agents/api/integration/:provider/connect/oauth/:attempt", feature: "agents.accounts", answers: { 200: object({ data: SIGN_IN_STATUS, location: object({ directory: string }) }), 404: object({ message: string }), 503: object({ message: string }) } },
  { route: "POST kuvo/agents/api/integration/:provider/connect/oauth/:attempt/complete", feature: "agents.accounts", body: object({ code: string }), answers: { 200: object({}), 400: object({ message: string }), 404: object({ message: string }), 503: object({ message: string }) } },
  { route: "DELETE kuvo/agents/api/integration/:provider/connect/oauth/:attempt", feature: "agents.accounts", answers: { 200: object({}), 404: object({ message: string }), 503: object({ message: string }) } },
  { route: "POST kuvo/agents/api/integration/:provider/connect/key", feature: "agents.accounts", body: object({ key: string }), answers: { 200: object({}), 400: object({ message: string }), 404: object({ message: string }), 503: object({ message: string }) } },
  { route: "POST kuvo/agents/api/integration/:provider/disconnect", feature: "agents.accounts", answers: { 200: object({}), 400: object({ message: string }), 404: object({ message: string }), 503: object({ message: string }) } },
  // Searches reachable computers when asked and names computers whose search could not run.
  { route: "GET kuvo/setup/search", feature: "search.setup", answers: { 200: SEARCH_FOUND } },
  // Sessions whose conversation has the words searched for, the latest first.
  { route: "GET kuvo/search", feature: "search", answers: { 200: SEARCH_FOUND } },
];
