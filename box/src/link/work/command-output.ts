// What a command a session left running has printed (PROTOCOL.md, "Work › Background work", a
// command's output), as a client's Output shows it: the end of it, read where it runs (the engine,
// in the folder it knows it in; a coding agent's, through their bridge) and cut here, so no client
// reads or cuts it itself. Followed, it's read about once a second while someone follows it, and
// never while no one does.
//
// Platform: only the engine's and the bridge's APIs: the same on macOS and Linux.

import type { Target } from "../../shared/records";
import type { BackgroundOutput } from "../../shared/routes/work";
import { pause } from "../../shared/wait";
import { call } from "../engine/client";
import type { AnswerPiece } from "../server/context";
import { readSessionBackground, WORK_BACKGROUND } from "./background";

/** How much of the end of what it printed is shown, in bytes. */
export const SHOWN_OUTPUT = 24_000;
/** How often a followed one is read again (ms). */
export const OUTPUT_EVERY = 1_000;

type Command = NonNullable<BackgroundOutput["command"]>;
/** Where a command can be read: the engine, and the coding agents' bridge, each while it runs. */
export interface OutputPlaces { engine: Target | null; bridge: Target | null }

const withoutNewlines = (text: string) => text.replace(/^[\n\r]+|[\n\r]+$/g, "");

/** What's shown of a stretch read from `cut` into the output: from its first whole line (with none,
 * all but a character the cut left half of), without newlines at either end. */
export function endOf(output: string, cut: boolean): string {
  let shown = output;
  if (cut) {
    const at = output.search(/[\n\r]/);
    shown = at >= 0 && withoutNewlines(output.slice(at + 1)) !== "" ? output.slice(at + 1) : output.replace(/^�+/, "");
  }
  return withoutNewlines(shown);
}

// The engine's (or the bridge's) own word that it has no such command, not any failure on the way.
const isGone = (error: unknown) => (error as { status?: unknown })?.status === 404 && /^(Shell command not found|No such command)/.test(String((error as Error)?.message ?? ""));

/** One read of `command` at `target`: whether it still runs, and the end of what it printed; for one
 * gone, or that couldn't be read, nothing printed (what was seen last stands). */
async function readOnce(command: Command, target: Target | null, signal?: AbortSignal): Promise<{ state: "running" | "ended"; output: string; cut: boolean } | { state: "ended" | "unread" }> {
  if (!target) return { state: "unread" };
  const route = `/api/shell/${encodeURIComponent(command.id)}`;
  const read = (path: string) => call(target, "GET", path, undefined, signal, command.agents ? undefined : command.folder) as Promise<Record<string, unknown> | null>;
  try {
    const job = await read(route);
    const size = (await read(`${route}/output?limit=1`))?.size;
    const start = Math.max(0, (typeof size === "number" ? size : 0) - SHOWN_OUTPUT);
    const stretch = (await read(`${route}/output?limit=${SHOWN_OUTPUT}${start ? `&cursor=${start}` : ""}`))?.output;
    // (An engine that doesn't say how it stands has it running.)
    const ended = typeof job?.status === "string" && job.status !== "running";
    return { state: ended ? "ended" : "running", output: endOf(typeof stretch === "string" ? stretch : "", start > 0), cut: start > 0 };
  } catch (error) {
    return { state: isGone(error) ? "ended" : "unread" };
  }
}

// The commands each session listed lately, by session and id (the latest 200, in this process):
// one that's no longer listed (it ended) is still read where it was.
const listed = new Map<string, Command>();

/** `session`'s command `id`, as the box lists what it left running (and as it last listed it, once
 * it no longer does); null for one it doesn't know. */
export function commandOf(session: string, id: string, file = WORK_BACKGROUND): Command | null {
  const key = JSON.stringify([session, id]);
  const found = readSessionBackground(session, file).commands.find(each => each.id === id);
  if (!found) return listed.get(key) ?? null;
  const command: Command = { id: found.id, command: found.command, started: found.started, ...(found.folder ? { folder: found.folder } : {}), ...(found.agents ? { agents: true } : {}) };
  listed.delete(key);
  listed.set(key, command);
  if (listed.size > 200) listed.delete(listed.keys().next().value!);
  return command;
}

/** A reader of what `command` printed, as the route answers it: each read given what was said last,
 * whose output stands where nothing could be read (it's gone, or unread for now). */
export function outputReader(command: Command | null, places: () => OutputPlaces, signal?: AbortSignal): (last?: BackgroundOutput) => Promise<BackgroundOutput> {
  return async last => {
    if (!command) return { command: null, output: "", cut: false, state: "ended" };
    const { engine, bridge } = places();
    const now = await readOnce(command, command.agents ? bridge : engine, signal);
    return "output" in now ? { command, ...now } : { command, output: last?.output ?? "", cut: last?.cut ?? false, state: now.state };
  };
}

/** What `read` says, a line now and again about once a second while its client follows (`wait`:
 * the pause between, a test's own), each time it says something new; once it says it ended, that
 * line's the last. Nothing's read while no one follows. */
export async function* followOutput(read: (last?: BackgroundOutput) => Promise<BackgroundOutput>, signal: AbortSignal | undefined, wait = (ms: number) => pause(ms, signal)): AsyncGenerator<AnswerPiece> {
  let last: BackgroundOutput | undefined;
  let line = "";
  while (!signal?.aborted) {
    const now = await read(last);
    if (signal?.aborted) return;
    last = now;
    const said = JSON.stringify(now);
    if (said !== line) {
      line = said;
      yield { body: Buffer.from(said + "\n") };
    }
    if (now.state === "ended") return;
    await wait(OUTPUT_EVERY);
  }
}
