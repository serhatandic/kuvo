// How an account stands and what can be done with it (PROTOCOL.md, "Signing in",
// `sign-in.accounts.state`): its `state`, the line under its name, each of its sign-ins' own line,
// and its `actions`, each with its button's words and, where it asks first, its confirm's. Decided
// here from what the engine (or the coding agents' bridge) lists and the box's own record of
// sign-ins that ended, so every client draws the same and keeps no rule of its own. Nothing here
// signs in, installs or removes: an action names what a client then asks through the routes there
// already are. Held to protocol/vectors/sign-in.json (`states`).

import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import type { ListedAccount, OfferedMethods } from "./sign-in";

/** How an account stands: signed in; not signed in; its sign-in ended (signed in again, it works);
 * its program isn't on the computer (a coding agent's, with an installer to run); the agent
 * couldn't say just now; or the sign-in in use says something's wrong, in its own words. */
export type AccountState = "signedIn" | "signedOut" | "ended" | "notInstalled" | "unchecked" | "problem";

/** What a client asks before it does an action: the question, what it comes to, and the words of
 * the button that goes ahead. */
export interface ActionConfirm {
  title: string;
  message: string;
  label: string;
}

/** Something that can be done with an account now: which (`id`), its button's words, `confirm`
 * where a client asks first, and `here` where it only works on this box's computer (a sign-in
 * whose page comes back to it). */
export interface AccountAction {
  id: "signIn" | "signInAgain" | "addAnother" | "disconnect" | "install" | "update" | "signOut" | "rename" | "remove";
  label: string;
  confirm?: ActionConfirm;
  here?: true;
}

/** One of an account's sign-ins or keys, in the engine's order (the first is the one in use): the
 * engine's id for a saved one, how it stands, and who or what it is, in a line. */
export interface SaidConnection {
  id?: string;
  state: "signedIn" | "ended" | "problem";
  line: string;
}

export interface AccountSaid {
  state: AccountState;
  line: string;
  actions: AccountAction[];
  connections: SaidConnection[];
  /** One you added beside a coding agent's own sign-in: listed with its agent whether or not it's
   * signed in. */
  added?: true;
}

interface ListedConnection {
  type?: unknown;
  id?: unknown;
  label?: unknown;
  method?: unknown;
  name?: unknown;
  status?: unknown;
}

const text = (value: unknown): string => (typeof value === "string" ? plainName(value) : "");

/** One sign-in or key: who or what it is ("you@example.com", "API key", "From OPENAI_API_KEY"), and
 * how it stands. `ended`: the box saw runs refused with it (accounts.ts). */
function connectionSaid(connection: ListedConnection, name: string, computer: string, ended: boolean): SaidConnection & { who: string; signedInAs: boolean; trouble: string } {
  const status = isObject(connection.status) ? connection.status : {};
  // (A key the engine named after its provider, or only "API key", says what it is once.)
  const label = text(connection.label);
  const kind = connection.method === "key" ? "API key" : connection.method === "external" ? `Credentials on ${computer}` : "";
  const who = label && label !== name && label.toLowerCase() !== kind.toLowerCase() ? label : "";
  const line = connection.type === "env" ? `From ${text(connection.name) || "its environment"}` : kind ? (who ? `${kind} · ${who}` : kind) : who || "Signed in";
  const trouble = status.status === "failed" ? text(status.message) : "";
  return {
    ...(connection.type === "credential" && typeof connection.id === "string" && connection.id ? { id: connection.id } : {}),
    state: ended || status.status === "needs_auth" ? "ended" : trouble ? "problem" : "signedIn",
    line,
    who,
    signedInAs: connection.type !== "env" && !kind && who !== "",
    trouble,
  };
}

/** `account`'s state, line, sign-ins and actions. `offered`: its ways in, as the box offers them
 * (none: it can't be signed in to from a client). `agents`: a coding agent's account, as its bridge
 * lists it. `ended`: the id of its saved sign-in the box saw end, if any. `computer`: this box's
 * computer, as it's named in a sentence. */
export function accountState(account: ListedAccount, offered: OfferedMethods, { agents = false, ended, computer }: { agents?: boolean; ended?: string; computer: string }): AccountSaid {
  const id = typeof account.id === "string" ? account.id : "";
  const name = text(account.name) || id;
  const meta = isObject(account.metadata) ? account.metadata : {};
  const listed = (Array.isArray(account.connections) ? account.connections : []).filter(isObject) as ListedConnection[];
  const said = listed.map(each => connectionSaid(each, name, computer, !agents && ended !== undefined && each.type === "credential" && each.id === ended));
  const connections = said.map(({ id, state, line }) => ({ ...(id ? { id } : {}), state, line }));
  const inUse = said[0];
  // The saved sign-in in use is the first the box keeps, as `kuvo/accounts/ended` goes by
  // (accounts.ts): ended, its plan's sign-in ended, whatever its shell's environment adds.
  const savedInUse = said[listed.findIndex(each => each.type === "credential")];
  const signsIn = offered.methods.length > 0;
  // One you added beside the agent's own sign-in (`claude-code@1f2e3d4c`).
  const added = agents && (id.includes("@") || (typeof meta.accountID === "string" && meta.accountID !== "" && meta.accountID !== "default"));
  // Its program isn't there and has an installer to run; one Kuvo Code brings itself comes as it
  // signs in.
  const installs = agents && meta.installed === false && typeof meta.install === "string" && meta.install !== "";

  const state: AccountState = installs ? "notInstalled"
    : (agents ? meta.signInEnded === true : inUse?.state === "ended" || savedInUse?.state === "ended") ? "ended"
    : inUse ? (inUse.state === "problem" ? "problem" : "signedIn")
    : agents && meta.unchecked === true ? "unchecked"
    : "signedOut";

  const line = (() => {
    switch (state) {
      case "notInstalled": return `Not installed on ${computer}`;
      case "ended": return "Sign-in ended";
      case "problem": return inUse!.trouble;
      case "unchecked": return "Couldn’t check just now";
      case "signedIn": {
        // (An agent that couldn't say just now, and never said who: as its bridge takes it.)
        if (agents && meta.unchecked === true && !inUse!.who) return "Couldn’t check just now";
        const first = inUse!.signedInAs ? `Signed in as ${inUse!.who}` : inUse!.line;
        return said.length > 1 ? `${first} · ${said.length} accounts` : first;
      }
      case "signedOut":
        if (agents) return "Not signed in";
        if (!signsIn) return offered.environment.length ? `Set ${offered.environment.join(" or ")} in your shell profile` : "Not signed in";
        return offered.hint || "Not signed in";
    }
  })();

  const actions: AccountAction[] = [];
  // A coding agent's sign-in page comes back to its computer, unless its bridge says it can finish
  // on another device.
  const here = agents && meta.remoteSignIn !== true ? { here: true as const } : {};
  const remove: AccountAction = {
    id: "remove",
    label: "Remove Account",
    confirm: { title: `Remove ${name}?`, message: `It’s signed out on ${computer}, and its models leave the model picker. Its sessions stay.`, label: "Remove Account" },
  };
  if (agents) {
    if (installs) actions.push({ id: "install", label: "Install" });
    else if (signsIn) actions.push(said.length || state === "ended" ? { id: "signInAgain", label: "Sign In Again", ...here } : { id: "signIn", label: "Sign In", ...here });
    // Its program's update, on the agent's own sign-in (one program serves every account with it),
    // where the installer that owns it can update it and it isn't updating now.
    const latest = text(meta.latestVersion);
    const updating = isObject(meta.programUpdate) && meta.programUpdate.state === "updating";
    if (!added && !installs && meta.outdated === true && meta.updates === true && latest && !updating) actions.push({ id: "update", label: `Update to ${latest}` });
    if (meta.signsOut === true && said.length) actions.push({ id: "signOut", label: "Sign Out" });
    if (added) actions.push({ id: "rename", label: "Rename" }, remove);
    return { state, line, actions, connections, ...(added ? { added: true as const } : {}) };
  }
  if (!said.length) {
    if (signsIn) actions.push({ id: "signIn", label: "Sign In" });
    return { state, line, actions, connections };
  }
  if (signsIn) {
    // Wherever one of its sign-ins ended, the one in use or another, or the one in use failed.
    if (state === "ended" || state === "problem" || said.some(each => each.state === "ended")) actions.push({ id: "signInAgain", label: "Sign In Again" });
    // (The engine uses one at a time: another is kept beside it.)
    actions.push({ id: "addAnother", label: "Add Another Account" });
  }
  // A sign-in or key this box keeps can be forgotten; one from the shell's environment can't.
  if (listed.some(each => each.type === "credential")) {
    actions.push({
      id: "disconnect",
      label: "Disconnect",
      confirm: { title: `Disconnect ${name}?`, message: `Its sign-ins and keys go from ${computer}, and its models leave the model picker there. Sessions on its models stop until you sign in again.`, label: "Disconnect" },
    });
  }
  return { state, line, actions, connections };
}
