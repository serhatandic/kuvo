// The Library (PROTOCOL.md, "Library"): what was handed over to you on each of your computers (a
// file an agent showed, a picture it generated, a file you attached), gathered by the box a client
// asks; an item's file and thumbnail; removing one from it. Times are seconds since 1970.

import { bool, BYTES, list, nullable, number, object, type RouteShape, string, words } from "../shape";

/** One thing handed over, with the computer it's on. */
export const LIBRARY_ITEM = object({
  id: string,
  name: string,
  kind: words("image", "document", "page", "other"),
  type: string,
  how: words("shown", "generated", "attached"),
  path: string,
  "bot?": string,
  "session?": string,
  "group?": string,
  "message?": string,
  "part?": string,
  at: number,
  "hidden?": number,
  there: bool,
  thumbnail: bool,
  "computer?": object({ key: string, name: string, "here?": bool }),
});

/** The Library: its newest items across your computers, the bots with something in it and whether
 * your own sessions have, and how each computer's part stands. */
export const LIBRARY = object({
  items: list(LIBRARY_ITEM),
  more: bool,
  sources: object({ bots: list(object({ id: string, name: nullable(string), "look?": object({ "color?": string, "shape?": string, "face?": string }), computer: string })), you: bool }),
  computers: list(object({ key: string, name: string, "here?": bool, state: words("here", "there", "away", "older", "failed"), "message?": string })),
});

export const LIBRARY_ROUTES: RouteShape[] = [
  { route: "GET kuvo/library", feature: "library", answers: { 200: LIBRARY } },
  { route: "GET kuvo/library/:id", feature: "library", answers: { 200: LIBRARY_ITEM } },
  { route: "GET kuvo/library/:id/file", feature: "library", answers: { 200: BYTES } },
  { route: "GET kuvo/library/:id/thumbnail", feature: "library", answers: { 200: BYTES } },
  { route: "POST kuvo/library/:id/hide", feature: "library", answers: { 200: LIBRARY_ITEM } },
];
