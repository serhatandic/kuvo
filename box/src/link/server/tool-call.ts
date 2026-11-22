import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { writeDurablePrivate } from "../../shared/files";
import { RUN } from "../home";
import { isObject, parsed } from "../../shared/json";
import { MUTATION_ID, mutationKey, mutationNonce, runMutation } from "../ownership/mutation";
import type { LinkRequest } from "./context";

const reads = new Set([
  "list_agents", "read_conversation", "list_sessions", "list_models", "list_projects", "plan_limits",
  "list_wake_ups", "show_in_panel", "show_file", "show_widget", "mark_final",
  "queue_list", "queue_show", "vault_items",
]);

export function toolRead(name: unknown, args: Record<string, unknown>): boolean {
  return typeof name === "string" && (reads.has(name) || name === "queue_labels" &&
    !["add", "rename", "recolor", "remove"].some(key => typeof args[key] === "string" && (args[key] as string).trim()));
}

/** Only finite local tool routes reconnect after admission. Other SSH traffic may stream or
 * perform a write without a durable identity. */
export function localToolRequest(input: Buffer): { input: Buffer; replay: boolean } {
  const request = parsed(input);
  const ordinary = { input, replay: false };
  if (!isObject(request)) return ordinary;
  const path = request.path;
  if (path !== "kuvo/agent-tools" && path !== "kuvo/work-queue/tools" && path !== "kuvo/vault/tools" && path !== "kuvo/sandbox/tools") return ordinary;
  if (request.method === "GET") return { input, replay: true };
  if (request.method !== "POST" || !isObject(request.body)) return ordinary;
  const { name, arguments: args, operation } = request.body;
  if (toolRead(name, isObject(args) ? args : {})) return { input, replay: true };
  if (operation !== undefined) return { input, replay: typeof operation === "string" && MUTATION_ID.test(operation) };
  return { input: Buffer.from(JSON.stringify({ ...request, body: { ...request.body, operation: randomUUID() } })), replay: true };
}

/** The same session and operation may ask for the stored answer while the engine is restarting.
 * The digest binds the number to what was asked. Older MCP servers can still call without it. */
export async function answerToolCall(request: LinkRequest, given: Record<string, unknown>,
  run: (operation?: string) => Promise<unknown>, recover = false) {
  const args = isObject(given.arguments) ? given.arguments : {};
  if (toolRead(given.name, args)) return { status: 200, value: await run() };
  const nonce = mutationNonce(request, given);
  if (nonce === null) return { status: 400, value: { message: "Invalid tool operation id." } };
  if (nonce === undefined) return { status: 200, value: await run() };
  const key = mutationKey(JSON.stringify(["tool", request.path, given.session ?? null]), nonce);
  try {
    const answer = await runMutation(key, given, async () => ({ status: 200, value: await run(key) }),
      undefined, undefined, undefined, undefined, undefined, recover ? async () => "resume" : undefined);
    if (recover && answer.status === 200) {
      try { fs.rmSync(progressFile(key), { force: true }); } catch {}
    }
    return answer;
  } catch (error) {
    if (!(error instanceof ToolCallPending)) throw error;
    return { status: 503, value: { retryToolCall: true, message: error.message } };
  }
}


export class ToolCallPending extends Error {}

/** An adapter's durable evidence, written before calling the downstream service. */
const progressFile = (key: string) => path.join(RUN, "tool-calls", key + ".json");

export function toolProgress<T>(key: string) {
  const file = progressFile(key);
  return {
    read(): T | undefined {
      try { return JSON.parse(fs.readFileSync(file, "utf8")); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
        return undefined;
      }
    },
    keep(value: T) { writeDurablePrivate(file, JSON.stringify(value), RUN); },
  };
}
