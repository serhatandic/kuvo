// A message that ran one of the engine's commands, from when Kuvo still sent them, shows as it was
// typed ("/review this branch"), not as the prompt the command made of it (PROTOCOL.md, "A
// message's parts"). The engine's commands took no metadata, so this box kept those messages'
// parts here, by id, in `run/commands.json`, which `kuvo/messages/parts`, the queue's items and its
// DELETE still read. Nothing is added to it any more: a message starting with "/" goes as written.

import path from "node:path";
import { isObject } from "../../shared/json";
import { textOf } from "../files";
import { RUN } from "../home";
import type { MessageParts } from "./parts";

const FILE = path.join(RUN, "commands.json");

/** What goes back in the composer for a message taken back to edit (`removed.draft`): its words
 * alone. (A keyword or command it went with isn't put in front: typed again, it would only be
 * words to the agent.) */
export const draftOf = ({ words }: Pick<MessageParts, "words">): string => words;

// What's kept, read once.
let kept: Map<string, MessageParts> | null = null;

function readKept(): Map<string, MessageParts> {
  if (kept) return kept;
  kept = new Map();
  try {
    const read = JSON.parse(textOf(FILE) ?? "{}");
    if (isObject(read)) for (const [id, parts] of Object.entries(read)) if (isObject(parts) && typeof parts.words === "string") kept.set(id, parts as unknown as MessageParts);
  } catch {}
  return kept;
}

/** The parts kept for message `id`, which ran one of the engine's commands; null for any other. */
export function keptCommand(id: string): MessageParts | null {
  return readKept().get(id) ?? null;
}

/** Reads `run/commands.json` afresh next time (tests, which write it after another test read it). */
export function forgetKept(): void {
  kept = null;
}
