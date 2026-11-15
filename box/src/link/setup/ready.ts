// Whether a first run has anything to ask about your accounts (PROTOCOL.md, "Your setup",
// `kuvo/setup/ready`, `setup.ready`): a model on one of your accounts runs here already and no plan
// needs you, so a client's first run goes straight on to where to work. Decided here, from what the
// box already reads (the models offered, model-rule.ts; each account's state, sign-in-state.ts), so
// no client keeps the rule.

import { isPlan, accountKind, signInMethods, type ListedAccount } from "../areas/sign-in";
import { accountState } from "../areas/sign-in-state";
import { needsNoAccount, offeredHere, type Lists } from "../work/model-rule";

/** What the box says of a first run's accounts step. */
export interface FirstRunReady {
  /** Nothing to ask: a model on an account of yours runs, no plan needs you, and both the engine
   * and the coding agents could say. */
  ready: boolean;
  /** A model that needs an account runs here now (not only the free ones). */
  runs: boolean;
  /** The plans whose sign-in ended or says something's wrong, by id, the coding agents' first. */
  needsYou: string[];
}

/** One side's accounts as it lists them (`api/integration`); null where it couldn't say. */
export type Listed = ListedAccount[] | null;

/** The plans that need you among `accounts`: listed (as Accounts lists them), a plan you sign in
 * to, not one you added beside an agent's own (a first run lists each agent once), whose sign-in
 * ended or reports a problem. */
function needing(accounts: ListedAccount[], agents: boolean, ended: Map<string, string>, computer: string): string[] {
  return accounts.flatMap(account => {
    const id = typeof account.id === "string" ? account.id : "";
    if (!id || !accountKind(account, agents).listed) return [];
    const offered = signInMethods(account);
    if (!isPlan(offered)) return [];
    const said = accountState(account, offered, { agents, ended: ended.get(id), computer });
    return !said.added && (said.state === "ended" || said.state === "problem") ? [id] : [];
  });
}

/** `kuvo/setup/ready`'s answer, from what the engine and the bridge list (`lists`: their models;
 * `engine`, `agents`: their accounts; `ended`: the engine's plans whose saved sign-in the box saw
 * end). A side that can't say now is never ready: a first run then shows its accounts step, which
 * says so. */
export function firstRunReady(lists: Lists, engine: Listed, agents: Listed, ended: Map<string, string>, computer: string): FirstRunReady {
  // A model that runs now, as a picker offers it, on an account: one of the engine's that isn't
  // free, or any of the coding agents' (a Custom Auto list is only as good as what's in it).
  const runs = offeredHere(lists).some(model => !model.retiring && !model.hidden && model.providerID !== "auto" && (model.agents || !needsNoAccount(model)));
  const needsYou = [...needing(agents ?? [], true, new Map(), computer), ...needing(engine ?? [], false, ended, computer)];
  const said = lists.engine !== null && lists.bridge !== null && engine !== null && agents !== null;
  return { ready: said && runs && needsYou.length === 0, runs, needsYou };
}
