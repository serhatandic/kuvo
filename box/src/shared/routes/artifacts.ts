// What a box's agents made (PROTOCOL.md, "Per area"): a place's list and Timeline, one thing
// with its versions, a version's bytes and what it changed, pinning, putting a version back, and
// what the copies take. The coding agents' bridge keeps the record and answers
// (src/agents/artifacts/routes.ts), and the box passes its answers on as they are: these are the
// shapes it answers in. Times are seconds since 1970.

import { bool, BYTES, list, nullable, number, object, oneOf, type RouteShape, string, words } from "../shape";

const KIND = words("memory", "skill", "image", "page", "document", "download", "code", "file");

/** One version: what one turn (or you, or something outside a turn) made of it. */
export const VERSION = object({
  n: number,
  at: nullable(number),
  by: object({ kind: words("agent", "you", "outside"), "session?": string, "bot?": string, "agent?": string }),
  "turn?": object({ session: string, "message?": string }),
  "group?": object({ id: string, "message?": string }),
  "trigger?": words("you", "wake", "group", "agent"),
  via: list(string),
  status: words("added", "modified", "deleted"),
  edits: number,
  "lines?": object({ added: number, removed: number }),
  size: number,
  hash: nullable(string),
  kept: bool,
  "reason?": words("secret", "ignored", "git", "too-big", "low-disk", "cleared"),
  "patch?": string,
  "restored?": number,
  "before?": bool,
  "handoff?": object({ from: string }),
});

const HEADER = {
  id: string,
  path: string,
  name: string,
  kind: KIND,
  type: string,
  where: object({ bots: list(string), "project?": string, sessions: list(string), groups: list(string) }),
  pinned: nullable(object({ by: words("bot", "you"), at: nullable(number), "note?": string })),
  gone: nullable(number),
  "shown?": nullable(number),
  "living?": bool,
  "git?": bool,
  restorable: bool,
  updated: nullable(number),
} as const;

/** One thing with every version, oldest first. */
export const ARTIFACT = object({ ...HEADER, versions: list(VERSION) });

/** A place's things, pinned first, then the latest changed, each with its newest version. */
export const ARTIFACT_LIST = object({ artifacts: list(object({ ...HEADER, versions: number, latest: VERSION })), next: nullable(string) });

/** Every version made in a place, newest first, each with its thing. */
export const ARTIFACT_TIMELINE = object({ entries: list(object({ artifact: object(HEADER), version: VERSION })), next: nullable(string) });

/** What the copies take, in bytes: all of them, older versions', the budget, the disk's free space
 * and the floor kept free; `low` while it's under the floor. */
export const ARTIFACT_STORAGE = object({ copies: number, older: number, budget: number, free: number, floor: number, low: bool });

/** What a version changed since the one before: words in prose, lines in code, or the two to show
 * side by side (a picture, a PDF). */
export const ARTIFACT_CHANGES = oneOf(
  object({ style: words("words"), lines: list(object({ kind: string, "text?": string, "runs?": list(object({ t: string, text: string })), "lines?": number })), added: number, removed: number, from: nullable(number), to: number }),
  object({ style: words("lines"), patch: string, added: number, removed: number, from: nullable(number), to: number }),
  object({ style: words("pair", "none"), from: nullable(number), to: number }),
);

export const ARTIFACT_ROUTES: RouteShape[] = [
  // A scope's things (one of `session=`, `bot=`, `group=`, `project=`), pinned first; page with `before`.
  { route: "GET kuvo/artifacts", feature: "artifacts", answers: { 200: ARTIFACT_LIST } },
  // Every version made in a scope, newest first, each with its thing; `by=<bot>` for one bot's.
  { route: "GET kuvo/artifacts/timeline", feature: "artifacts", answers: { 200: ARTIFACT_TIMELINE } },
  // What the copies of versions take on disk, the budget for older ones, and whether free space is low.
  { route: "GET kuvo/artifacts/storage", feature: "artifacts", answers: { 200: ARTIFACT_STORAGE } },
  // Sets the budget for older versions' copies (`{budget}`); older ones past it go at once.
  { route: "POST kuvo/artifacts/storage", feature: "artifacts", answers: { 200: ARTIFACT_STORAGE } },
  // Clears older versions' copies (`{before?}`, seconds).
  { route: "POST kuvo/artifacts/storage/clear", feature: "artifacts", answers: { 200: object({ cleared: number, copies: number, older: number, budget: number, free: number, floor: number, low: bool }) } },
  { route: "GET kuvo/artifacts/:id", feature: "artifacts", answers: { 200: ARTIFACT } },
  // Pins it for you, or not (`{pinned, note?}`).
  { route: "POST kuvo/artifacts/:id/pin", feature: "artifacts", answers: { 200: ARTIFACT } },
  // Writes a version back to the file (`{version}`); what it replaced is kept as a version, to undo it.
  { route: "POST kuvo/artifacts/:id/restore", feature: "artifacts", answers: { 200: ARTIFACT } },
  // A version's bytes; 410 when it has no copy or was a deletion.
  { route: "GET kuvo/artifacts/:id/versions/:n", feature: "artifacts", answers: { 200: BYTES } },
  // What a version changed since the one before (or `from=<m>`): words, lines, or a pair to show side by side.
  { route: "GET kuvo/artifacts/:id/versions/:n/changes", feature: "artifacts", answers: { 200: ARTIFACT_CHANGES } },
];
