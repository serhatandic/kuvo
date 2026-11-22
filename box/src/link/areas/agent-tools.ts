// Agents' tools and starting sessions (PROTOCOL.md, "Agents' tools"): `POST kuvo/agent-tools`, which
// the tool servers on every box pass an agent's call to, and `POST kuvo/sessions`, a session started
// with a task by any client.

import { isObject, parsed } from "../../shared/json";
import { listComputer } from "../lists/computer";
import { startSession } from "../letters/start";
import { AGENT_TOOLS, agentTool, everyone, isAgentTool, rootOf, targetsOf } from "../letters/tools";
import { isCalling } from "../vault/who";
import { startItem } from "../work-queue/start";
import type { AnswerPiece, RouteContext } from "../server/context";

import { answerToolCall } from "../server/tool-call";

const ADDRESS = /^ses_[A-Za-z0-9]{1,60}$/;

// An agent's call to one of its tools (`{name, arguments, session, inView?}`): what the tool says
// back, `{text, isError}`. One that waits for an answer holds the request (up to eleven minutes); the
// request going stops the wait, and an answer that comes later reaches its conversation as a message.
// `inView`: whether the user is looking at the calling conversation now, where the client passing the
// call on can tell (show_in_panel says "Shown" or "Kept for later" then, else "Sent").
export async function* agentToolsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, readBridge, bridge, signal, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isObject(given) || !isAgentTool(given.name)) return yield* json(400, { message: `Send {name, arguments, session}: name one of ${AGENT_TOOLS.join(", ")}.` });
  const args = isObject(given.arguments) ? given.arguments : {};
  const session = typeof given.session === "string" && ADDRESS.test(given.session) ? given.session : null;
  const inView = typeof given.inView === "boolean" ? given.inView : undefined;
  const name = given.name;
  const recovers = name === "send_message" || name === "start_session" || name === "schedule_wake_up" || name === "watch_for_change" || name === "cancel_wake_up";
  const answer = await answerToolCall(request, given, async operation => {
    const item = name === "start_session" ? startItem(args) : undefined;
    // Only a real tool call may grant the new session write access to the starter's item.
    const queueItem = item !== undefined && session && await isCalling(session, ["start_session"], args, readEngine, readBridge) ? item : undefined;
    return agentTool(name, args, session, { readEngine, bridge, signal: operation && recovers ? undefined : signal ?? undefined, inView, queueItem, operation });
  }, recovers);
  return yield* json(answer.status, answer.value);
}

// A session started with a task (`{task, title?, folder?, startedBy?, beside?, wait?}`): by the
// conversation `startedBy` (as its agent's start_session does), or by a client, the task as the
// user's own message. What it is, `{session, title, folder, project, beside}`, with `answer` when
// it's waited for.
export async function* startRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, signal, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isObject(given)) return yield* json(400, { message: "Send {task}, and title, folder, startedBy, wait." });
  // The client addresses the destination over its normal transport. The chosen computer also
  // goes in the request, so sending it to the wrong box can never start a session there.
  const addressed = typeof given.computer === "string" && given.computer !== "" && given.computer !== "here";
  const computer = addressed ? listComputer() : null;
  if (computer && computer.id !== given.computer)
    return yield* json(409, { message: "Send the start request to the computer you chose.", computer });
  let by = null;
  if (given.startedBy !== undefined && given.startedBy !== null) {
    const root = typeof given.startedBy === "string" && ADDRESS.test(given.startedBy) ? await rootOf(given.startedBy, targetsOf({ readEngine, bridge })) : null;
    if (!root) return yield* json(404, { message: "There's no such conversation here." });
    const all = await everyone({ readEngine });
    by = { ...root, name: all.agents.find(agent => agent.session === root.session)?.name ?? "another agent" };
  }
  const outcome = await startSession(computer ? { ...given, computer: "here" } : given, by, { readEngine, bridge, signal: signal ?? undefined });
  if ("refused" in outcome) {
    const { status, ...refusal } = outcome.refused;
    return yield* json(status, refusal);
  }
  return yield* json(200, computer ? { ...outcome.started, computer } : outcome.started);
}
