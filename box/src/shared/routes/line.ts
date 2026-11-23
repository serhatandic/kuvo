// The one line (PROTOCOL.md, "The line"; server/line.ts): every change a client draws, pushed on
// one stream, numbered, with a heartbeat.

import { any, bool, map, number, object, oneOf, type RouteShape, string } from "../shape";

/** The first line: which line this is, whether it starts over, and how often a quiet one beats. */
export const LINE_HELLO = object({ hello: object({ line: string, fresh: bool, beat: number }) });
/** One line after it: what a topic said, that it was turned down, that its events may have a gap,
 * or a heartbeat. */
export const LINE_ITEM = oneOf(
  LINE_HELLO,
  object({ t: string, n: number, d: any }),
  object({ t: string, n: number, refused: object({ status: number, message: string, "said?": any }) }),
  object({ t: string, n: number, gap: bool }),
  object({ beat: number }),
);

export const LINE_ROUTES: RouteShape[] = [
  // The one stream: a hello, then numbered topic lines, refusals, gaps and heartbeats.
  { route: "GET kuvo/line", feature: "line", answers: { 200: LINE_ITEM } },
  // Change an open line's topics; 404 with `fresh` means open the line again with every topic.
  { route: "POST kuvo/line/watch", feature: "line", body: object({ line: string, watch: map(string) }), answers: { 200: object({ line: string }), 404: object({ fresh: bool, message: string }) } },
];
