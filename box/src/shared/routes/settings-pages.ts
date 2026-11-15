// A box's settings, as it describes them to every client (PROTOCOL.md, "A box's settings",
// `settings`): its sections, and each section's page of rows, with their words, values, whether
// each can be changed from where it's asked (and why not), and a revision of each row's own. A
// client draws a row by its `kind` alone, so a new client draws every setting a box has.

import { any, bool, type Infer, list, literal, number, object, oneOf, type RouteShape, string, words } from "../shape";

/** The kinds of row, each a control every client has. A box newer than a client may send one it
 * doesn't know: the client draws its title and line, and says to update to change it. */
export const SETTING_KINDS = ["switch", "choice", "text", "number", "folder", "secret", "status", "actions", "list", "meter", "open", "steps", "custom"] as const;

/** Whether a row (or an action) can be changed from where it's asked: `true`, or why not, in words
 * to show beside it. */
export const SETTING_CAN = oneOf(literal(true), object({ why: string }));

/** What's asked before a change or an action, in the box's words. */
export const SETTING_CONFIRM = object({ title: string, body: string, button: string });

/** Something a row offers to do: its `id` (`POST …/:row/:id`), its words, whether it's
 * `destructive`, what's asked first, and whether it can be done from here (`can`, absent: it can).
 * One with `opens` sends nothing: the client opens that address (a web page; an address of the
 * box's own computer is only ever given to a client there). */
export const SETTING_ACTION = object({
  id: string, title: string, "style?": words("default", "destructive"), "confirm?": SETTING_CONFIRM,
  "opens?": string, "can?": SETTING_CAN,
});

/** One of a `choice` row's choices. */
export const SETTING_CHOICE = object({ id: string, title: string, "line?": string });

/** One of a `meter` row's windows: its name, how much is used (0 to 1), and when it starts over
 * (ms since 1970). */
export const SETTING_WINDOW = object({ title: string, used: number, "resets?": number });

const ROW_FIELDS = {
  id: string, title: string, "line?": string, "tone?": words("quiet", "warning"),
  kind: words(...SETTING_KINDS),
  /** A `switch`'s true or false, a `choice`'s choice, a field's text or number, a `status`'s words. */
  "value?": any,
  /** Absent for a row with nothing to change (a `status`). */
  "can?": SETTING_CAN,
  /** A hash of the row as the box gave it, sent back as `was` with a change to it. */
  revision: string,
  "confirm?": SETTING_CONFIRM,
  /** A change under way there, in words ("Restarting…"), until the page's events say it's done. */
  "busy?": string,
  "choices?": list(SETTING_CHOICE),
  "placeholder?": string, "min?": number, "max?": number,
  /** A `secret`'s: whether one is set (never its value). */
  "set?": bool,
  "actions?": list(SETTING_ACTION),
  "windows?": list(SETTING_WINDOW),
  /** An `open` row's: a section of this box's, or an address. */
  "section?": string, "address?": string,
  /** A `steps` row's: what starts its flow. */
  "start?": SETTING_ACTION,
  /** A `custom` row's: what only an installed app draws, by name. */
  "name?": string,
} as const;

/** A row in a `list`. */
export const SETTING_ITEM = object(ROW_FIELDS);

/** A row, a `list`'s with its rows (`items`), what adds one (`add`), and what it says with none (`empty`). */
export const SETTING_ROW = object({ ...ROW_FIELDS, "items?": list(SETTING_ITEM), "add?": SETTING_ACTION, "empty?": string });

/** A card of rows on a page, with its heading and a line under it. */
export const SETTING_GROUP = object({ id: string, "title?": string, "note?": string, rows: list(SETTING_ROW) });

/** Words on a page, below its groups. */
export const SETTING_NOTE = object({ words: string, tone: words("quiet", "warning") });

/** A section's page. */
export const SETTINGS_PAGE = object({ id: string, title: string, "line?": string, groups: list(SETTING_GROUP), "notes?": list(SETTING_NOTE) });

/** One of a box's sections, for a sidebar and search: its id, title, the heading it's under, its
 * icon (by name: `shield`, `globe`…, a set every client has), its line, and words it's found by. */
export const SETTINGS_SECTION = object({ id: string, title: string, group: string, icon: string, line: string, keywords: list(string) });

/** A box's sections, in order. */
export const SETTINGS_SECTIONS = object({ sections: list(SETTINGS_SECTION) });

/** A change refused because a row changed since it was read: the page as it is now (`page`). A 409
 * without one is the route underneath refusing (a settings file that can't be used), in words. */
export const SETTINGS_STALE = object({ message: string, "page?": SETTINGS_PAGE });

const REFUSED = object({ message: string });

export type SettingKind = (typeof SETTING_KINDS)[number];
export type SettingCan = Infer<typeof SETTING_CAN>;
export type SettingAction = Infer<typeof SETTING_ACTION>;
export type SettingItem = Infer<typeof SETTING_ITEM>;
export type SettingRow = Infer<typeof SETTING_ROW>;
export type SettingGroup = Infer<typeof SETTING_GROUP>;
export type SettingsPage = Infer<typeof SETTINGS_PAGE>;
export type SettingsSection = Infer<typeof SETTINGS_SECTION>;

export const SETTINGS_PAGE_ROUTES: RouteShape[] = [
  { route: "GET kuvo/settings", answers: { 200: SETTINGS_SECTIONS } },
  { route: "GET kuvo/settings/:section", answers: { 200: SETTINGS_PAGE, 404: REFUSED } },
  { route: "GET kuvo/settings/:section/events", answers: { 200: SETTINGS_PAGE, 404: REFUSED } },
  { route: "PATCH kuvo/settings/:section", answers: { 200: SETTINGS_PAGE, 400: REFUSED, 403: REFUSED, 404: REFUSED, 409: SETTINGS_STALE } },
  { route: "POST kuvo/settings/:section/:row/:action", answers: { 200: SETTINGS_PAGE, 400: REFUSED, 403: REFUSED, 404: REFUSED, 409: SETTINGS_STALE } },
];
