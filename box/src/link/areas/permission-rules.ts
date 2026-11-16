// The engine's permission rules (`permissions`: a list of `{action, resource, effect}`, the last that
// matches winning), as Settings › Permissions and Settings › Agents show them: the kinds of action
// each has a choice for (a rule for everything, "*", at the list's front), and what the engine does
// with none.

import { isObject } from "../../shared/json";

/** The kinds of action with a choice of their own, in the order they're shown. */
export const ACTIONS = [
  { id: "edit", title: "Edit files", line: "Changing, creating or deleting files. Plan mode doesn't, by default." },
  { id: "shell", title: "Run commands", line: "In the terminal." },
  { id: "read", title: "Read files", line: "Files ending in .env ask first unless you allow this." },
  { id: "external_directory", title: "Work outside the project", line: "In folders that aren't the project's." },
  { id: "webfetch", title: "Open web pages" },
  { id: "websearch", title: "Search the web" },
  { id: "subagent", title: "Hand work to subagents" },
  { id: "skill", title: "Use skills" },
] as const;
export const EFFECTS = [{ id: "allow", title: "Allow" }, { id: "ask", title: "Ask First" }, { id: "deny", title: "Don't Allow" }] as const;

/** What the engine does with no rule of yours: everything but working outside the project, which asks. */
export const engineDefault = (action: string) => (action === "external_directory" ? "ask" : "allow");

export type Rule = { action: string; resource: string; effect: string };
export const isRule = (value: unknown): value is Rule => isObject(value) && typeof value.action === "string" && typeof value.resource === "string" && typeof value.effect === "string";

/** The choice for `action` in a rules list: the last rule for everything, as the engine reads them. */
export const choiceIn = (rules: unknown[], action: string) => rules.filter(isRule).findLast(rule => rule.action === action && rule.resource === "*")?.effect;

/** Whether a rule is one of the choices (a kind of action with a choice, for everything). */
export const isChoice = (rule: unknown): boolean => isRule(rule) && rule.resource === "*" && ACTIONS.some(action => action.id === rule.action);
