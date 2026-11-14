// Who a Work Queue item may be for (PROTOCOL.md, "The Work Queue", "Assignee"): one of this box's
// bots, or a coding agent. It's a fact the item states, shown by clients; it starts no work itself.

import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { AGENT_NAMES } from "../../shared/carry";
import { listBots } from "../bots/list";
import { Refusal, type Assignee } from "./store";

/** Someone an item may be for, as a client's menu lists them: `look`, a bot's (its face). */
export interface Assignable { kind: "bot" | "agent"; id: string; name: string; look?: unknown }

const AGENTS: readonly Assignable[] = Object.entries(AGENT_NAMES).map(([id, name]) => ({ kind: "agent" as const, id, name }));

/** Everyone an item here may be for: this box's bots, then the coding agents. */
export function assignable(): Assignable[] {
  let bots: Assignable[] = [];
  try {
    bots = listBots().map(bot => ({ kind: "bot" as const, id: bot.id, name: bot.name, ...(bot.look ? { look: bot.look } : {}) }));
  } catch {}
  return [...bots, ...AGENTS];
}

/** The conversation of the bot an item is for, when it's one of this box's. */
export function botSession(assignee: Assignee | undefined): string | null {
  if (assignee?.kind !== "bot") return null;
  try {
    return listBots().find(bot => bot.id === assignee.id)?.session ?? null;
  } catch {
    return null;
  }
}

/** Who `given` names: `{kind, id}` as a client sends it, or a bot's or an agent's name or id as an
 * agent writes it; null takes it away. Refused for no one this box knows. */
export function assigneeFor(given: unknown): Assignee | null {
  if (given === null || given === "") return null;
  const all = assignable();
  const found = isObject(given) ? all.find(each => each.kind === given.kind && each.id === given.id)
    : typeof given === "string" ? all.find(each => each.id === given.trim()) ?? all.find(each => each.name.toLowerCase() === given.trim().replace(/^&/, "").toLowerCase())
    : undefined;
  if (!found) throw new Refusal(`There's no bot or coding agent ${isObject(given) ? String(given.id ?? "") : String(given)} here to assign it to. It can be: ${all.map(each => each.name).join(", ")}.`);
  return { kind: found.kind, id: found.id };
}

/** An item's assignee as a view names it: by the name it has here, else as it was told (another
 * computer's bot), else by its id. */
export function assigneeNamed(assignee: Assignee, all: Assignable[] = assignable(), told?: unknown): { kind: "bot" | "agent"; id: string; name: string } {
  const here = all.find(each => each.kind === assignee.kind && each.id === assignee.id);
  const name = here?.name ?? (isObject(told) && typeof told.name === "string" ? plainName(told.name).slice(0, 100) : "") ?? "";
  return { kind: assignee.kind, id: assignee.id, name: name || (assignee.kind === "bot" ? "A bot on another computer" : assignee.id) };
}
