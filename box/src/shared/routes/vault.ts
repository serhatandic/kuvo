// The vault (PROTOCOL.md, "Per area"): sign-ins, keys, files and notes this box keeps for its agents,
// who may use which, what was used, what agents ask for, and the agents' own door to it. No answer
// an agent can get holds a value; those that do (`…/values`, `kuvo/vault/key`) answer only clients.

import { any, bool, type Infer, list, map, number, NOTHING, object, type RouteShape, string, words } from "../shape";

export const ITEM_KINDS = ["login", "key", "file", "note"] as const;
export const GRANT_HOWS = ["own", "ask"] as const;

/** One item, without its values: what it's called, what kind, where it may be used (a sign-in's
 * sites, a key's hosts), its username (not a secret), which fields it holds, a file's name, whether
 * agents may read a note (then it isn't a secret), whether a sign-in is only for signing in, in the
 * browser (never handed to a command), and when it was made, changed and last used. */
export const VAULT_ITEM = object({
  id: string,
  kind: words(...ITEM_KINDS),
  name: string,
  sites: list(string),
  "username?": string,
  fields: list(string),
  "fileName?": string,
  "readable?": bool,
  "signInOnly?": bool,
  created: number,
  changed: number,
  "used?": number,
});

/** Who a grant is for: a bot (by its id), the sessions of a project (its folder), or everyone. */
export const VAULT_WHO = object({ "bot?": string, "project?": string, "everyone?": bool });
/** A grant: an item, who, and how (on its own, or asking you each time). */
export const VAULT_GRANT = object({ item: string, who: VAULT_WHO, how: words(...GRANT_HOWS), at: number });

/** How the vault stands: whether it's made yet, locked (a machine's after a restart, until a
 * client unlocks it), paused for everyone or for some bots (by id) or conversations that aren't a
 * bot's (by root session), with why, where its values are kept (this Mac's Keychain, or a
 * machine's encrypted store), how long activity is kept, and whether its files were changed
 * outside Kuvo Code (then it uses none of them). `agents`: whether agents may use it here (only
 * where their commands are fenced; until then it's yours alone, and agents sign in through you);
 * `agentsWaiting`: the sandbox is on, and agents started before it came on still run. */
export const VAULT_STATE = object({
  made: bool,
  locked: bool,
  paused: bool,
  pausedBots: map(string),
  store: words("keychain", "encrypted"),
  keptDays: number,
  "agents?": bool,
  "agentsWaiting?": bool,
  "tampered?": bool,
  "generations?": map(number),
});

/** Everything a client's Vault page shows at once. */
export const VAULT = object({ state: VAULT_STATE, items: list(VAULT_ITEM), grants: list(VAULT_GRANT) });
/** An item made or changed; `key` is the unlock key, given only to the client whose item made the vault. */
export const VAULT_ITEM_ANSWER = object({ item: VAULT_ITEM, "key?": string });
/** An item's values, by field (a file's as base64 in `file`). */
export const VAULT_VALUES = object({ values: map(string) });
export const VAULT_GRANTS = object({ grants: list(VAULT_GRANT) });

/** One line of activity: when, what happened, the item (by id and its name then), who (a bot, a
 * session and its name, or you on a client), where it went (a site, a program, a folder), how it
 * was allowed, and why the agent said it needed it. Never a value. `mine`: the client reading it
 * did it (by who it is, not its name, which phones may share). A line an older box kept without
 * who did it has no `client`. */
export const VAULT_ACTIVITY = object({
  id: string,
  at: number,
  what: words("used", "asked", "allowed", "declined", "refused", "added", "changed", "removed", "granted", "ungranted", "paused", "resumed", "shown"),
  "item?": string,
  "itemName?": string,
  "kind?": words(...ITEM_KINDS),
  "bot?": string,
  "botName?": string,
  "session?": string,
  "sessionName?": string,
  "client?": string,
  "mine?": bool,
  "site?": string,
  "program?": string,
  "how?": string,
  "why?": string,
});
export const VAULT_ACTIVITIES = object({ entries: list(VAULT_ACTIVITY) });

/** What an agent asks for: to use an item it may only use by asking (`use`), or an item it needs
 * and hasn't got (`item`: what for, in its words, and where). */
export const VAULT_ASK = object({
  id: string,
  kind: words("use", "item"),
  at: number,
  "session?": string,
  "sessionName?": string,
  "bot?": string,
  "botName?": string,
  "item?": string,
  "itemName?": string,
  "need?": string,
  "why?": string,
  "site?": string,
  "machine?": string,
  /** From a bot's seat in this group chat, where it shows: answered only once, never for good. */
  "group?": string,
  /** Whether Always would give it for good (to its bot, or its project): false where it can only be
   * once (a seat, a conversation in a home folder or at the disk's top), decided as it's asked. */
  "always?": bool,
});
export const VAULT_ASKS = object({ asks: list(VAULT_ASK) });

/** The unlock key (base64url), for a client to keep. */
export const VAULT_KEY = object({ key: string });
/** The agents' vault tools, as a tool server lists them (MCP's `tools/list` entries). */
export const VAULT_TOOL_LIST = object({ tools: list(any) });
/** What an agent's tool answers: words for it, and whether it failed. */
export const VAULT_TOOL = object({ text: string, isError: bool });
/** A fill for the browser's own process: the item, its username, and the values it fills. */
export const VAULT_FILL = object({ item: object({ id: string, name: string }), "username?": string, values: map(string) });

export type VaultItem = Infer<typeof VAULT_ITEM>;
export type VaultWho = Infer<typeof VAULT_WHO>;
export type VaultGrant = Infer<typeof VAULT_GRANT>;
export type VaultState = Infer<typeof VAULT_STATE>;
export type Vault = Infer<typeof VAULT>;
export type VaultActivity = Infer<typeof VAULT_ACTIVITY>;
export type VaultAsk = Infer<typeof VAULT_ASK>;
export type VaultFill = Infer<typeof VAULT_FILL>;

export const VAULT_ROUTES: RouteShape[] = [
  // Everything a Vault page shows: its state, its items without their values, and its grants.
  { route: "GET kuvo/vault", feature: "vault", answers: { 200: VAULT } },
  // Adds an item (`{kind, name, values, …}`); names are unique; a sign-in needs a site.
  { route: "POST kuvo/vault/items", feature: "vault", answers: { 200: VAULT_ITEM_ANSWER } },
  // Changes an item; a value `null` takes it away.
  { route: "PATCH kuvo/vault/items/:id", feature: "vault", answers: { 200: VAULT_ITEM_ANSWER } },
  // Removes an item, with its values and grants.
  { route: "DELETE kuvo/vault/items/:id", feature: "vault", answers: { 200: NOTHING } },
  // An item's values, for Show on a client (noted `shown`); never answered to an agent.
  { route: "GET kuvo/vault/items/:id/values", feature: "vault", answers: { 200: VAULT_VALUES } },
  // Whether the supplied fields match the saved values, without showing them.
  { route: "POST kuvo/vault/items/:id/same", feature: "vault", answers: { 200: object({ same: bool }) } },
  // Who may use which item, and how (on its own, or asking you each time).
  { route: "GET kuvo/vault/grants", feature: "vault", answers: { 200: VAULT_GRANTS } },
  // Grants an item (`{item, who, how}`); `how: null` takes the grant away.
  { route: "POST kuvo/vault/grants", feature: "vault", answers: { 200: VAULT_GRANTS } },
  // What happened with the vault, newest first, never a value; `mine` marks what the asking client did.
  { route: "GET kuvo/vault/activity", feature: "vault", answers: { 200: VAULT_ACTIVITIES } },
  // Pauses or resumes agents' use (`{paused, bot?}`): everyone's, or one bot's or conversation's.
  { route: "POST kuvo/vault/pause", feature: "vault", answers: { 200: VAULT_STATE } },
  // The machine vault's unlock key, for a client to keep; makes the vault if there's none. 409 on a Mac.
  { route: "POST kuvo/vault/key", feature: "vault", answers: { 200: VAULT_KEY } },
  // Unlocks a machine's vault after a restart (`{key}`); 403 for a key that isn't this vault's.
  { route: "POST kuvo/vault/unlock", feature: "vault", answers: { 200: VAULT_STATE } },
  // What agents wait on you for: to use an item they may only use by asking, or one they need.
  { route: "GET kuvo/vault/asks", feature: "vault", answers: { 200: VAULT_ASKS } },
  { route: "GET kuvo/vault/asks/events", feature: "vault", answers: { 200: VAULT_ASKS } },
  // Answers one (`{answer: once | always | no, item?}`); unanswered after 15 minutes counts as no.
  { route: "POST kuvo/vault/asks/:id", feature: "vault", answers: { 200: NOTHING } },
  // The agents' vault tools, for a tool server to list; none where agents' commands aren't all sandboxed.
  { route: "GET kuvo/vault/tools", feature: "vault", answers: { 200: VAULT_TOOL_LIST } },
  // Runs an agent's vault tool call (`{name, arguments, session}`), only while that conversation is making it.
  { route: "POST kuvo/vault/tools", feature: "vault", answers: { 200: VAULT_TOOL } },
  // Values for the browser's own process to fill, only on an https page of the item's sites, under its grants.
  { route: "POST kuvo/vault/fill", feature: "vault", answers: { 200: VAULT_FILL } },
];
