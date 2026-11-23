import { ToolCallPending, toolProgress } from "../server/tool-call";
// Agents writing to each other (PROTOCOL.md, "Agents' tools")
// Any conversation on any of your computers can ask who else there is (list_agents), write to one of
// them (send_message), read one's conversation (read_conversation) and start a session
// (start_session), and run the sessions it started as you would (list_sessions, stop_session, compact_session,
// answer_ask, list_models, make_project, list_projects: `work/tools.ts`, PROTOCOL.md "Work"; and a bot's
// telling you something on
// purpose, notify_user, `notices/notify.ts`; a plan's limits, plan_limits, and using one of its
// resets, use_limit_reset, with your OK, `work/plan-reset.ts`; and the rest of Kuvo Code's own: wake-ups, a bot's job and
// the show tools' checks, `own-tools.ts`). Every box answers them alike (`POST kuvo/agent-tools`): this
// Mac's app and keeper, and a machine's kuvo-browser.js, only pass the call on. A message reaches its conversation as input that
// says who sent it, and the agent there takes it up.
// One on another computer goes there by mail (PROTOCOL.md, "Peers"). You name one with "&".

import { keepsList } from "../lists/sessions";
import { HERE, IS_MAC } from "../home";
import { agentsState, bridgeBack } from "../areas/agents-down";
import { pause } from "../../shared/wait";
import { readKit } from "../bots/kit";
import { listBots } from "../bots/list";
import { asked } from "../engine/client";
import { messageReached, queueOf, sendInto } from "../engine/queue";
import { messageID } from "../engine/sessions";
import { ownAgentsNow, ownAsLast } from "../peers/agents";
import { computerName } from "../peers/computer-name";
import { composeAgents, isPutAway, peerOf, readPeerAgents } from "../peers/directory";
import { movedOf, peersOf } from "../peers/known";
import { frozen } from "../moves/records";
import { readLink } from "../state";
import { folded, READ_BY_MAIL, READ_LIMIT, readConversation, TranscriptRefusal } from "./conversation";
import { letterAnswer, leaveLetter, offRelay } from "./leave";
import { composeLetter, lookupProblem, waitingNote, placeOfSession, placeOfSessionPatiently } from "./letter";
import { allowed, takeBack } from "./limits";
import { OUTSIDE, seatRefusal, startedWords } from "./start";
import { noteOversightBy } from "../work/oversight";
import { noteSent } from "../work/sent";
import { notifyUserTool } from "../notices/notify";
import { planLimitsTool } from "../work/plan-limits";
import { useResetTool } from "../work/plan-reset";
import { answerAskTool, askReset, describeList, leadOf, listModelsTool, listProjectsTool, liveRows, makeProjectTool, compactSessionTool, stopSessionTool } from "../work/tools";
import { under, workSessions } from "../work/tree";
import { rowsBox } from "../work/elsewhere";
import { awaySince } from "../peers/reach";
import { awaitAnswer, boxOf, callPeer, resumeOfferedStart, elsewhereTool, rowsEverywhere } from "./calls";
import { computerOf, readElsewhere, startElsewhere, startToolSession } from "./start-call";
import { resumedWake, setJobTool, showTool, wakeTool } from "./own-tools";
import { SHOW_TOOLS, WAKE_TOOLS } from "./tool-names";
import type { ListedAgent, Target } from "../../shared/records";
import type { SessionPlace } from "./letter";

/** What a tool says back to its agent: its words, and whether it didn't do what was asked. */
export interface ToolAnswer {
  text: string;
  isError: boolean;
}

/** Where a tool's call is answered: the engine here and the coding agents' bridge while it runs,
 * until its asker goes (`signal`); `step`, between looks for an answer (ms; a test's are short). */
export interface ToolPlace {
  readEngine: () => Target;
  bridge: () => Target | null;
  signal?: AbortSignal;
  step?: number;
  /** Whether the user is looking at the calling conversation now, where the client passing the
   * call on can tell (show_in_panel's words). */
  inView?: boolean;
  /** Work Queue reference from a verified start_session call. */
  queueItem?: string | number;
  operation?: string;
}

/** The tools, by name. */
export const AGENT_TOOLS = ["list_agents", "send_message", "read_conversation", "start_session", "list_sessions", "stop_session", "compact_session", "answer_ask", "list_models", "make_project", "list_projects", "notify_user", "plan_limits", "use_limit_reset", ...WAKE_TOOLS, "set_job", ...SHOW_TOOLS] as const;
export type AgentToolName = (typeof AGENT_TOOLS)[number];
export const isAgentTool = (name: unknown): name is AgentToolName => AGENT_TOOLS.includes(name as AgentToolName);

const said = (text: string): ToolAnswer => ({ text, isError: false });
const refused = (text: string): ToolAnswer => ({ text, isError: true });

/** Everyone a tool here can name: this box's own agents and its peers', and this box's name among
 * them (null on a Mac, whose agents name no machine). */
type ToolAgent = ListedAgent & { computer?: string };
export interface Everyone {
  here: string | null;
  agents: ToolAgent[];
  fresh: boolean;
}

/** Where conversations here run: the engine, and the coding agents' bridge while it runs. */
export function targetsOf(place: Pick<ToolPlace, "readEngine" | "bridge">): Target[] {
  const targets: Target[] = [];
  try { targets.push(place.readEngine()); } catch {}
  const bridge = place.bridge();
  if (bridge) targets.push(bridge);
  return targets;
}

/** Everyone, as they are now: this box's own worked out again (as last worked out while its engine
 * can't say), then each peer's it alone lists. */
export async function everyone(place: Pick<ToolPlace, "readEngine">): Promise<Everyone> {
  const link = readLink();
  const fresh = await ownAgentsNow(place.readEngine, link?.id ?? null).catch(() => null);
  const here = IS_MAC ? null : link?.id ?? null;
  const own = fresh ?? ownAsLast(link?.id ?? null);
  const peers = peersOf(link), told = readPeerAgents(), moved = movedOf(link);
  const local = new Set(own.map(agent => agent.session));
  const agents = composeAgents(own, here, peers, told, moved).agents.map(agent => {
    const peer = local.has(agent.session) ? null : peerOf(agent.session, peers, told, moved);
    return peer ? { ...agent, computer: peer.kind === "mac" && agent.machine ? agent.machine : computerName(peer) } : agent;
  });
  return { here, agents, fresh: fresh !== null };
}

/** The conversation a call works for, where it runs: itself, a subagent's (the one that started
 * it), and a coding agent's engine run (Auto's) the conversation it runs for (`kuvo.engineFor`). Null
 * where it isn't one here. */
export async function rootOf(session: string | null, targets: Target[]): Promise<{ session: string; place: SessionPlace } | null> {
  let id = session;
  let found: { session: string; place: SessionPlace } | null = null;
  for (let step = 0; id && step < 6; step++) {
    const place = await placeOfSession(id, targets).catch(() => null);
    if (!place) break;
    found = { session: id, place };
    const kuvo = place.data.metadata?.kuvo;
    const above = place.data.parentID ?? (typeof kuvo?.engineFor === "string" ? kuvo.engineFor : null);
    if (!above) break;
    id = above;
  }
  return found;
}

/** What an agent is, for one here to read: "bot, on the Mac", "session, here, in api", "session, on
 * devbox". */
export function whatAgent(agent: ToolAgent, here: string | null): string {
  const place = agent.computer ? `on ${agent.computer}`
    : (agent.machine ?? null) === here ? (IS_MAC ? "on the Mac" : "here") : agent.machine ? `on ${agent.machine}` : "on the Mac";
  return `${agent.kind}, ${place}${agent.project ? `, in ${agent.project}` : ""}`;
}

/** The list, for an agent to read (not the one asking). */
export function describeAgents(agents: ToolAgent[], caller: string | null, here: string | null): string {
  const others = agents.filter(agent => agent.session !== caller);
  if (!others.length) return "No other agents yet.";
  const lines = others.map(agent => `- ${agent.name} (${whatAgent(agent, here)}) address: ${agent.session}${agent.about ? `: ${agent.about}` : ""}`);
  return "Agents you can write to with send_message, or whose conversation you can read with read_conversation (by name or address):\n" + lines.join("\n");
}

const SESSION_ADDRESS = /^ses_[A-Za-z0-9]{1,60}$/;

type Recipient = { agent: ToolAgent } & ({ local: SessionPlace } | { away: NonNullable<ReturnType<typeof peerFor>> });

// Names use the directory. An address can also reach a conversation that's older than the last
// list. Use the same successful lookup for authorization and for the read or send itself.
async function findAgent(wanted: string, caller: string | null, { agents, here, fresh }: Everyone, place: ToolPlace, reading = false): Promise<Recipient | { error: ToolAnswer }> {
  let name = wanted.trim();
  if (name.startsWith("&") && !agents.some(agent => agent.name === name)) name = name.slice(1);
  let matches = agents.filter(agent => agent.session === name);
  const address = SESSION_ADDRESS.test(name);
  if (!matches.length && !address) matches = agents.filter(agent => folded(agent.name) === folded(name));
  if (matches.length > 1) return { error: refused(`More than one agent is called ${name}: use one's address.\n` + describeAgents(matches, caller, here)) };
  if (!matches.length && !address) {
    if (!fresh) return { error: refused(`Couldn't look up ${name}: this computer's agents didn't finish listing their conversations. Try again, or use the conversation's address.`) };
    // Part of a name ("Release" for "Release notes"): those it could be.
    const close = agents.filter(agent => folded(agent.name).includes(folded(name))).slice(0, 8);
    const hint = close.length ? "Did you mean one of these?\n" + describeAgents(close, caller, here) : "Use list_agents to see who there is.";
    return { error: refused(`There's no agent called ${name}. ` + hint) };
  }
  const listed = matches[0];
  const session = listed?.session ?? name;
  if (session === caller) return { error: refused(reading ? "That's this conversation: you have it already." : "That's you.") };
  const title = listed?.name ?? session;
  const denied = (reason: string) => ({ error: refused(`Couldn't ${reading ? "read" : "write to"} ${title}: ${reason}`) });
  if (!reading && frozen().has(session)) return { error: refused(`${title} is moving to another computer. Try again in a minute.`) };
  const abroad = (): Recipient | null => {
    const away = peerFor(session);
    if (!away) return null;
    const agent = listed ?? { session, name: rowsBox(session)?.row.title ?? "New session", kind: "session" as const };
    return { agent: { ...agent, computer: listed?.computer ?? computerName(away.peer) }, away };
  };
  const targets = () => targetsOf(place);
  let local: SessionPlace | null;
  try {
    local = await placeOfSessionPatiently(session, targets, undefined, place.signal);
    if (!local && !place.bridge() && agentsState({ bridge: place.bridge }).state === "restarting") {
      const recipient = abroad();
      if (recipient) return recipient;
      if (!keepsList() && await bridgeBack(place.bridge, place.signal)) local = await placeOfSessionPatiently(session, targets, undefined, place.signal);
      else return denied("The coding agents are still restarting. Try again in a moment.");
    }
  } catch (error) {
    return abroad() ?? denied(lookupProblem(error));
  }
  if (!local) return abroad() ?? denied("No computer has confirmed where this conversation is. Check its address with list_agents.");
  if (local.data.parentID) return denied("It's a subagent's conversation.");
  if (local.data.metadata?.kuvo?.seat) return denied("It's a seat in a group chat.");
  if (isPutAway(local.data) && !(reading && caller && under(caller, await workSessions(targets(), undefined, { archived: true })).some(each => each.data.id === session))) {
    return denied("It's archived.");
  }
  const bot = listBots(here ?? undefined).some(each => each.session === session);
  return { agent: listed ?? { session, name: local.data.title || "New session", kind: bot ? "bot" : "session", ...(here ? { machine: here } : {}) }, local };
}

// Who a conversation is to the others: its name in the list, else where it is.
const nameOf = (caller: string | null, { agents, here }: Everyone) =>
  agents.find(agent => agent.session === caller)?.name ?? (IS_MAC ? "another agent" : `an agent on ${here ?? "a machine"}`);

// A letter queued while this box is off the relay.
const offlineNote = () => `Your other computers can't be reached through the relay now: it goes once ${HERE}'s back on it, and its answer comes to you as a message.`;

// The box a conversation not here is on, among the peers: the one that lists it, told of it as one
// of the caller's sessions, or where it's known to live now.
function peerFor(session: string) {
  const peer = boxOf(session);
  return peer ? { peer, features: readPeerAgents()[peer.key]?.features ?? [] } : null;
}

// A message from `caller` to the agent `args.to` names, wherever it runs.
async function sendMessage(args: Record<string, unknown>, caller: string | null, all: Everyone, place: ToolPlace): Promise<ToolAnswer> {
  if (typeof args.to !== "string" || !args.to.trim()) return refused("Say who to write to (to).");
  if (typeof args.text !== "string" || !args.text.trim()) return refused("Say what to write (text).");
  const { text } = args;
  const journal = place.operation && toolProgress<{
    session: string; name: string; sender: string; id: string; at: number; peer?: string; machine?: string; answer?: string;
  }>(place.operation);
  const kept = journal ? journal.read() : undefined;
  const found = kept ? undefined : await findAgent(args.to, caller, all, place);
  if (found && "error" in found) return found.error;
  const target = kept ?? found!.agent;
  // A few at a time: two agents shouldn't keep writing to each other unattended. Taken back if it
  // doesn't go, so one tried again isn't counted twice.
  const key = caller ?? "outside";
  const counted = kept ? 0 : allowed("messages", key);
  if (counted === null) return refused("You've sent a lot of messages in the last few minutes. Wait a while, or ask the user how to go on.");
  const failed = (words: string) => {
    if (!kept) takeBack("messages", key, counted);
    return refused(words);
  };
  const sender = kept?.sender ?? nameOf(caller, all);
  const peer = kept?.peer ?? (found && "away" in found ? found.away.peer.key : undefined);
  const at = kept?.at ?? Date.now();
  const id = kept?.id ?? (peer && place.operation ? "ltr_" + place.operation.slice(0, 22) : messageID(at));
  const machine = kept?.machine ?? found?.agent.computer;
  const progress = { session: target.session, name: target.name, sender, id, at, peer, machine };
  if (!kept && journal) journal.keep(progress);
  if (kept?.answer) return said(kept.answer);
  if (!peer) {
    let here = found && "local" in found ? found.local : await placeOfSession(target.session, targetsOf(place)).catch(() => null);
    if (!here) throw new ToolCallPending("Still looking for the recipient's conversation.");
    const message = { id, text: composeLetter(sender, caller ?? "unknown", text, false), delivery: "steer" as const };
    const shown = { from: sender, words: text, kept: message.text };
    let went: "sent" | "waits";
    try {
      if (kept && await messageReached(here.target, target.session, id)) {
        went = queueOf(target.session).waiting?.some(item => item.id === id) ? "waits" : "sent";
      } else went = await sendInto(here.target, target.session, message, shown);
    } catch (error) {
      if (journal) throw new ToolCallPending(`Waiting for ${target.name}'s conversation to confirm delivery.`);
      try {
        await pause(200, place.signal);
        const again = await findAgent(target.session, caller, all, place);
        if ("error" in again || !("local" in again)) return failed(`Delivery to ${target.name} couldn't be confirmed: ${lookupProblem(error)}`);
        here = again.local;
        went = await sendInto(here.target, target.session, message, shown);
      } catch (retryError) {
        return failed(`Delivery to ${target.name} couldn't be confirmed: ${lookupProblem(retryError)}`);
      }
    }
    const answer = went === "waits" ? waitingNote(target.session, target.name) : `Sent to ${target.name}. Its answer, if it writes one, comes to you as a message.`;
    journal && journal.keep({ ...progress, answer });
    noteOversightBy(caller, { ...here.data, id: target.session }, "told");
    if (caller) noteSent(caller, target.session, text);
    return said(answer);
  }
  try {
    leaveLetter({ from: caller, to: target.session, text, peer, wait: false }, at, journal ? id : undefined);
  } catch {
    return failed(`It didn't reach ${target.name}.`);
  }
  if (caller) noteSent(caller, target.session, text);
  const answer = offRelay() ? offlineNote() : `On its way to ${target.name}${machine ? ` on ${machine}` : ""}. Its answer, if it writes one, comes to you as a message.`;
  journal && journal.keep({ ...progress, answer });
  return said(answer);
}

// The conversation of the agent `args.of` names, as you'd read it; a long one as its start and its
// latest part; with `query`, the parts that mention it. One on another computer comes by mail from a
// box that shares them (`letters-read`).
async function readOne(args: Record<string, unknown>, caller: string | null, all: Everyone, place: ToolPlace): Promise<ToolAnswer> {
  const of = typeof args.of === "string" ? args.of.trim() : "";
  if (!of) return refused("Say whose conversation to read (of).");
  const query = typeof args.query === "string" && args.query.trim() ? args.query.trim() : null;
  const found = await findAgent(of, caller, all, place, true);
  if ("error" in found) return found.error;
  const target = found.agent;
  const intro = `${target.name}'s conversation (${whatAgent(target, all.here)}, address ${target.session}), oldest first. What's under **User** is the user's; under **${target.name}**, its agent's.\n\n`;
  if ("local" in found) {
    const read = async (here: SessionPlace) => {
      const words = await readConversation(here.target, target.session, here.data, target.name, query, READ_LIMIT, place.signal);
      noteOversightBy(caller, { ...here.data, id: target.session }, "read");
      return said(intro + words);
    };
    const unreadable = (error: unknown) => refused(`Couldn't read ${target.name}'s conversation: ${lookupProblem(error)}`);
    try {
      return await read(found.local);
    } catch (error) {
      if (error instanceof TranscriptRefusal || place.signal?.aborted) return unreadable(error);
      try {
        const again = await findAgent(target.session, caller, all, place, true);
        if ("local" in again && (again.local.target.port !== found.local.target.port || again.local.target.password !== found.local.target.password)) return await read(again.local);
      } catch (retryError) {
        return unreadable(retryError);
      }
      return unreadable(error);
    }
  }
  const { away } = found;
  if (!caller) return refused(`Couldn't read ${target.name}'s conversation: the other computer needs a conversation to send its answer to.`);
  if (offRelay()) return refused(`Couldn't read ${target.name}'s conversation: this computer isn't connected to the relay.`);
  if (!away.features.includes("letters-read")) return refused(`${target.name}'s computer doesn't share its conversations yet (it runs an older Kuvo Code). Ask it what you need with send_message.`);
  // Its computer isn't there: said at once, not kept (a read wanted now is no use next week).
  const asleep = `${computerName(away.peer)} isn't answering (it may be asleep, off or offline), so ${target.name}'s conversation can't be read now. Try again later.`;
  if (awaySince(away.peer.key) !== null) return refused(asleep);
  let id: string;
  const sent = Date.now();
  try {
    id = leaveLetter({ from: caller, to: target.session, text: query ?? "read", peer: away.peer.key, wait: true, read: query ? { query } : {} });
  } catch {
    return refused(`Couldn't read ${target.name}'s conversation: this computer couldn't save the request to send it.`);
  }
  const { answer, unreachable } = await awaitAnswer(id, away.peer.key, sent, place, { cut: READ_BY_MAIL, every: 1000 });
  if (answer && "text" in answer) return said(intro + answer.text);
  if (answer && "failed" in answer) return refused(`Couldn't read ${target.name}'s conversation: ${answer.failed}`);
  return refused(unreachable ? asleep : `Couldn't read ${target.name}'s conversation: ${computerName(away.peer)} didn't send it back in time.`);
}

/** A tool's call (`name`, its `args`), from the conversation `session` (as the engine names it; null
 * from outside one): what it says back. */
export async function agentTool(name: AgentToolName, args: Record<string, unknown>, session: string | null, place: ToolPlace): Promise<ToolAnswer> {
  if (name === "start_session" && place.operation) {
    const pending = resumeOfferedStart(place.operation, 15_000);
    if (pending) return pending;
  }
  if (place.operation && (name === "schedule_wake_up" || name === "watch_for_change" || name === "cancel_wake_up")) {
    const kept = await resumedWake(place.operation);
    if (kept) return kept;
  }
  const targets = targetsOf(place);
  // Showing a file or a widget needs nothing of the conversation: said at once, whatever its engine.
  if (name === "show_file" || name === "mark_final" || name === "show_widget") return showTool(name, args, { session, own: null, root: null }, place.inView);
  // A subagent writes, reads and starts sessions as its conversation: its name, its folder, its
  // approval, where answers go.
  const root = await rootOf(session, targets);
  const caller = root?.session ?? session;
  // A bot's seat in a group chat only talks and looks things up, whatever agent it runs on: the work
  // happens in each bot's own conversation, handed out after the wrap-up.
  if (root?.place.data.metadata?.kuvo?.seat && (name === "send_message" || name === "start_session")) return refused(seatRefusal().message);
  // Kuvo Code's own, which don't need everyone: for the conversation the call works for.
  switch (name) {
    case "schedule_wake_up":
    case "watch_for_change":
    case "list_wake_ups":
    case "cancel_wake_up":
    case "set_job":
    case "show_in_panel": {
      const own = session === root?.session ? root.place : session ? await placeOfSession(session, targets).catch(() => null) : null;
      const calling = { session, own, root };
      if (name === "set_job") return setJobTool(args, calling, place.readEngine, place.bridge);
      if (name === "show_in_panel") return showTool(name, args, calling, place.inView);
      return wakeTool(name, args, calling, place.bridge(), place.operation);
    }
  }
  const all = await everyone(place);
  switch (name) {
    case "list_agents":
      return said(describeAgents(all.agents, caller, all.here));
    case "send_message":
      return sendMessage(args, caller, all, place);
    case "read_conversation":
      return readOne(args, caller, all, place);
    case "start_session": {
      // On another of your computers (`computer`): checked here, started there.
      const where = computerOf(args.computer);
      if ("refused" in where) return refused(where.refused);
      if ("peer" in where) {
        if (!root) return refused("Only a conversation on this computer can start work on another.");
        const caller = { ...root, name: nameOf(root.session, all) };
        return startElsewhere(where.peer, args, caller, leadOf(caller), place, callPeer);
      }
      // From a conversation this box doesn't have: an agent all the same, counted and approved as
      // one (`outside`).
      const by = root ? { ...root, name: nameOf(root.session, all) } : OUTSIDE;
      const { computer: _here, wait: _wait, ...given } = args;
      const outcome = await startToolSession(given, by, { readEngine: place.readEngine, bridge: place.bridge, step: place.step, signal: place.signal, queueItem: place.queueItem, operation: place.operation });
      return startedWords(outcome);
    }
    case "list_models":
    case "list_projects": {
      const where = computerOf(args.computer);
      if ("refused" in where) return refused(where.refused);
      if ("peer" in where) {
        if (!root) return refused("Only a conversation on this computer can look at another.");
        return readElsewhere(name, where.peer, leadOf({ ...root, name: nameOf(root.session, all) }), place, callPeer);
      }
      if (name === "list_projects") return listProjectsTool();
      let engine: Target | null = null;
      try { engine = place.readEngine(); } catch {}
      return listModelsTool(engine, place.bridge(), place.signal);
    }
    case "plan_limits":
      return planLimitsTool(place.bridge(), place.signal);
    case "use_limit_reset": {
      // Asked of the user first, on the card in the conversation (unless Always Allow for that account).
      const caller = root ? { ...root, name: nameOf(root.session, all) } : null;
      return useResetTool(place.bridge(), args, caller, given => askReset(caller!, given, { signal: place.signal, ...(place.step ? { every: place.step } : {}) }), place.signal);
    }
    case "make_project": {
      if (!root) return refused("Only a conversation on this computer makes projects here.");
      return makeProjectTool({ ...root, name: nameOf(root.session, all) }, args, { signal: place.signal, ...(place.step ? { every: place.step } : {}) });
    }
    case "notify_user":
      if (!root) return refused("Only a conversation on this computer can notify the user from here.");
      return notifyUserTool({ session: root.session, data: root.place.data, name: all.agents.find(agent => agent.session === root.session)?.name ?? null }, args);
    case "list_sessions":
    case "stop_session":
    case "compact_session":
    case "answer_ask": {
      // Only a conversation here has sessions of its own.
      if (!root) return refused("Only a conversation on this computer has sessions of its own.");
      const caller = { ...root, name: nameOf(root.session, all) };
      // Its sessions here, and on your other computers: those there, as they are now, or as of when
      // that computer last told, while it isn't answering.
      if (name === "list_sessions") {
        const rows = await liveRows(caller.session, targets, place.signal);
        return describeList(rows, await rowsEverywhere(leadOf(caller), place));
      }
      // One that isn't here (the engine and the coding agents' bridge say so): on the computer it's on.
      const session = typeof args.session === "string" ? args.session.trim() : "";
      if (/^ses_[A-Za-z0-9]{1,60}$/.test(session) && (await placeOfSession(session, targets).catch(() => undefined)) === null) return elsewhereTool(name, leadOf(caller), args, session, place);
      if (name === "stop_session") return stopSessionTool(caller, args, targets, place.signal);
      if (name === "compact_session") return compactSessionTool(caller, args, targets, place.signal);
      return answerAskTool(leadOf(caller), args, targets, place.signal);
    }
  }
}
