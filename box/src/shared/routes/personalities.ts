// Personalities you make (PROTOCOL.md, "Per area"): the box's copy of the library, changed by
// any client, and the library whole for the app beside the box, which meets it with your other
// computers' copies.

import { type Infer, list, literal, NOTHING, number, object, type RouteShape, string } from "../shape";

/** A personality you made: its id (`custom.…`), name and what it is, the SF Symbol it shows, and
 * its change's count. */
export const PERSONALITY = object({ id: string, name: string, description: string, symbol: string, updated: number });
/** Your personalities, as this box's library has them. */
export const PERSONALITIES = object({ personalities: list(PERSONALITY) });
/** The library whole, as the box keeps and signs it: its personalities, and those removed with
 * their counts, so no copy brings them back (newest first). */
export const LIBRARY = object({
  kuvo: literal(1),
  personalities: list(PERSONALITY),
  removed: list(object({ id: string, removed: number })),
});
/** Another computer's copy met with this one's: the personalities whose take changed here. */
export const TAKEN = object({ changed: list(string) });

export type Personality = Infer<typeof PERSONALITY>;
export type Personalities = Infer<typeof PERSONALITIES>;
export type PersonalityLibraryDocument = Infer<typeof LIBRARY>;
export type PersonalitiesTaken = Infer<typeof TAKEN>;

export const PERSONALITY_ROUTES: RouteShape[] = [
  // The personalities you made, on the computer asked (Kuvo Code's own presets aren't listed).
  { route: "GET kuvo/personalities", feature: "personalities", answers: { 200: PERSONALITIES } },
  { route: "GET kuvo/personalities/events", feature: "personalities.events", answers: { 200: PERSONALITIES } },
  // Makes one (`{name, description}`, id `custom.<uuid>`).
  { route: "POST kuvo/personalities", feature: "personalities", answers: { 200: PERSONALITY } },
  // The library whole, as the box keeps and signs it; only for the app beside the box (`x-kuvo-from: app`).
  { route: "GET kuvo/personalities/document", feature: "personalities", answers: { 200: LIBRARY } },
  // Meets another computer's copy (`{document}`) with this one's; app only; answers the ids that changed here.
  { route: "POST kuvo/personalities/take", feature: "personalities", answers: { 200: TAKEN } },
  { route: "PATCH kuvo/personalities/:id", feature: "personalities", answers: { 200: PERSONALITY } },
  // Removes one you made; kept as removed so no other copy brings it back.
  { route: "DELETE kuvo/personalities/:id", feature: "personalities", answers: { 200: NOTHING } },
];
