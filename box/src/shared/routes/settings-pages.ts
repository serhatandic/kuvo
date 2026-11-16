// A box's settings, as it describes them to every client (PROTOCOL.md, "A box's settings",
// `settings`): its sections, and each section's page of rows, with their words, values, whether
// each can be changed from where it's asked (and why not), and a revision of each row's own. A
// client draws a row by its `kind` alone, so a new client draws every setting a box has.

import { any, bool, type Infer, list, literal, map, number, object, oneOf, type RouteShape, string, words } from "../shape";

/** The kinds of row, each a control every client has. A box newer than a client may send one it
 * doesn't know: the client draws its title and line, and says to update to change it. */
export const SETTING_KINDS = ["switch", "choice", "text", "number", "folder", "secret", "status", "actions", "list", "meter", "open", "steps", "custom"] as const;

/** Whether a row (or an action) can be changed from where it's asked: `true`, or why not, in words
 * to show beside it. */
export const SETTING_CAN = oneOf(literal(true), object({ why: string }));

/** What's asked before a change or an action, in the box's words. */
export const SETTING_CONFIRM = object({ title: string, body: string, button: string });

/** One of a `choice` row's choices (and a form's `choice` field's). `verified` (a form's): picking
 * it lets something use what's yours more, so a client makes sure it's you first. */
/** A choice of a form's field: `model`, where it's a model (an engine's, `provider/model` in its
 * settings), so a client draws its model picker for the field. */
export const SETTING_CHOICE = object({ id: string, title: string, "line?": string, "verified?": bool, "model?": object({ providerID: string, id: string }) });

/** A folder picker’s words, including how to clear the choice. */
export const SETTING_FOLDER = object({ choose: string, clear: string, prompt: string, message: string });

/** The kinds of a form's field, each a control every client has: one line of text, several
 * (`mono`: in a typeface that keeps a command's flags apart), a segmented pick of `choices`, a
 * switch, a folder (the box's picker, with its `folder` words), names with values (`pairs`, its
 * two placeholders; its value `[{key, value}]`, in order), a secret (`secret`: hidden as it's
 * typed, `multi` for several lines; it opens empty, and empty sends nothing new), and a file of the
 * client's own (`file`, with its `file` words: chosen on the client's computer, sent as `{name,
 * data}`, its bytes as base64). A secret's value, a `secret` pairs' values and a file's bytes are
 * only ever sent with the action itself: a check gets each as `•` once for each character (a file
 * as `{name, size}`), so the box can say what's missing or too short without seeing it. */
export const SETTING_FIELD_KINDS = ["text", "lines", "choice", "switch", "folder", "pairs", "secret", "file"] as const;

/** Where a `secret` field's kept value is read, only when you ask to see it (Show): `GET route` of
 * the same box → `{values}`, of which `values[field]` is shown in the field (the route notes that it
 * was shown, where it keeps such notes). Kept only while the form is open. */
export const SETTING_FIELD_SHOWS = object({ route: string, field: string });

/** One of a form's fields: its `id` (the key of its value sent back), its words, its kind, its
 * value as the form opens, and, with `shown`, drawn only while another `choice` field is one of
 * `is`. `can: {why}` where it can't be changed here (drawn, not hidden). */
export const SETTING_FIELD = object({
  id: string, title: string, kind: words(...SETTING_FIELD_KINDS), "line?": string, "value?": any, "placeholder?": string,
  "choices?": list(SETTING_CHOICE), "mono?": bool, "folder?": SETTING_FOLDER,
  "pairs?": object({ key: string, value: string }),
  "shown?": object({ field: string, is: list(string) }),
  "can?": SETTING_CAN,
  "multi?": bool, "shows?": SETTING_FIELD_SHOWS, "secret?": bool, "verified?": bool,
  "file?": object({ choose: string, none: string }),
});

/** What an action asks for before it's done (Add Server…, Edit…): a sheet with the box's title, its
 * fields in order, a line under them, and its button's words (`submit`). Its values are sent with
 * the action (`values`), and checked as they're typed (`…/check`). */
/** (`revision`: the form's own, of the values it opens with, sent back as `was` with its values and
 * checks, so only a change to what the form shows makes it stale.) */
export const SETTING_FORM = object({ title: string, submit: string, fields: list(SETTING_FIELD), "note?": string, "revision?": string });

/** Something a row offers to do: its `id` (`POST …/:row/:id`), its words, whether it's
 * `destructive`, what's asked first, and whether it can be done from here (`can`, absent: it can).
 * One with `opens` sends nothing: the client opens that address (a web page; an address of the
 * box's own computer is only ever given to a client there). One with `form` asks for its values
 * first. `verified` (an action, or a form's field): it lets something use what's yours from now on
 * (a vault's grant), so a client makes sure it's you first (Touch ID); one that can't draws it,
 * unavailable, and says where it can be done. */
export const SETTING_ACTION = object({
  id: string, title: string, "style?": words("default", "destructive"), "confirm?": SETTING_CONFIRM,
  "opens?": string, "can?": SETTING_CAN, "form?": SETTING_FORM, "verified?": bool,
});

/** How a thing in a list stands, beside its title: a dot of its tone and the box's words. */
export const SETTING_BADGE_TONES = ["quiet", "good", "busy", "warning", "failed"] as const;
export const SETTING_BADGE = object({ words: string, tone: words(...SETTING_BADGE_TONES) });

/** A flow the box leads for a row (signing in to it in the browser): how it stands (`waiting`,
 * `done`, `failed`), the box's words, the page to finish it on (`opens`: the client that started it
 * opens it once; any other draws its actions, one of which `opens` it too), where the page you end
 * on can't reach the box's computer the address to paste back (`code`, sent as `values.code` with
 * the action that takes it), and its actions (cancel, again, dismiss…). */
export const SETTING_FLOW_STATES = ["waiting", "done", "failed"] as const;
export const SETTING_FLOW = object({
  id: string, state: words(...SETTING_FLOW_STATES), title: string, line: string, "opens?": string,
  "code?": object({ title: string, placeholder: string, action: string }),
  actions: list(SETTING_ACTION),
});

/** One of a `meter` row's windows: its name, how much is used (0 to 1), and when it starts over
 * (ms since 1970). */
export const SETTING_WINDOW = object({ title: string, used: number, "resets?": number });

/** One part of a `meter` row's whole (what Kuvo keeps on a computer, by what it holds): its name, how
 * much (`value`, in the row's measure: bytes), its words ("1.2 GB"), and what it's made of. A client
 * draws the parts as one bar, in its own colours, with each part's name and words beside it. */
export const SETTING_PART = object({ id: string, title: string, value: number, words: string, "parts?": list(object({ title: string, value: number, words: string })) });

const ROW_FIELDS = {
  id: string, title: string, "line?": string, "tone?": words("quiet", "warning"),
  kind: words(...SETTING_KINDS),
  /** A `switch`'s true or false, a `choice`'s choice, a field's text or number, a `status`'s words. */
  "value?": any,
  /** Absent for a row with nothing to change (a `status`). */
  "can?": SETTING_CAN,
  /** A hash of the row, including action confirmation words, sent back as `was` with a change to it. */
  revision: string,
  "confirm?": SETTING_CONFIRM,
  /** A change under way there, in words ("Restarting…"), until the page's events say it's done. */
  "busy?": string,
  "choices?": list(SETTING_CHOICE),
  /** Preferred visible lines for a multi-line text editor; presentation only, outside revisions. */
  "lines?": object({ min: number, max: number }),
  /** A folder picker’s words, including how to clear the choice. */
  "folder?": SETTING_FOLDER,
  /** Client-side filtering of a list by its title and its items’ title and line; empty replaces {query} with the query. */
  "search?": object({ placeholder: string, empty: string }),
  /** Display unit, conversion from stored values, and nearest rounding at the given precision. */
  "unit?": object({ name: string, factor: number, decimals: number, rounding: literal("nearest") }),
  "suggestions?": list(object({ value: number, title: string })),
  /** Ask only when the proposed number is below this threshold. */
  "confirmBelow?": object({ value: number, confirm: SETTING_CONFIRM }),
  "placeholder?": string, "min?": number, "max?": number,
  /** A `secret`'s: whether one is set (never its value). */
  "set?": bool,
  "actions?": list(SETTING_ACTION),
  "windows?": list(SETTING_WINDOW),
  /** A `meter` row's parts of a whole (instead of `windows`); never part of its revision. */
  "parts?": list(SETTING_PART),
  /** An `open` row's: a section of this box's, or an address. */
  "section?": string, "address?": string,
  /** A `steps` row's: what starts its flow. */
  "start?": SETTING_ACTION,
  /** A `custom` row's: what only an installed app draws, by name. */
  "name?": string,
  /** How a thing stands, beside its title (a server's "Connected"); never part of its revision. */
  "badge?": SETTING_BADGE,
  /** One more line under it, set apart (the error a server failed with). */
  "notice?": object({ words: string, tone: words("quiet", "warning") }),
  /** A flow the box leads for it now (a sign-in). */
  "flow?": SETTING_FLOW,
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

/** A row change; confirmed is true only after accepting its conditional confirmation. */
export const SETTINGS_CHANGE = object({ row: string, value: any, was: string, "confirmed?": bool });

const REFUSED = object({ message: string });

/** The box's words under one of a form's fields: `quiet` (what it'll do), `good` (it's there),
 * `warning` (what's wrong, which holds Submit). */
export const SETTING_FIELD_WORDS = object({ id: string, words: string, tone: words("quiet", "good", "warning") });

/** An action done (`values`: its form's, or a flow's `code`). */
export const SETTINGS_ACT = object({ was: string, "values?": map(any) });

/** A form's values, checked as they're typed, changing nothing: each field's words, whether it can
 * be submitted (`ready`), and values the box fills in (from a server's setup pasted into a field). */
export const SETTINGS_CHECK = object({ "was?": string, values: map(any) });
export const SETTINGS_CHECKED = object({ fields: list(SETTING_FIELD_WORDS), ready: bool, "values?": map(any) });

/** A form's values refused: why, and each field's words (the form stays open). */
export const SETTINGS_FORM_REFUSED = object({ message: string, "fields?": list(SETTING_FIELD_WORDS) });

export type SettingKind = (typeof SETTING_KINDS)[number];
export type SettingCan = Infer<typeof SETTING_CAN>;
export type SettingAction = Infer<typeof SETTING_ACTION>;
export type SettingField = Infer<typeof SETTING_FIELD>;
export type SettingForm = Infer<typeof SETTING_FORM>;
export type SettingFlow = Infer<typeof SETTING_FLOW>;
export type SettingBadge = Infer<typeof SETTING_BADGE>;
export type SettingFieldWords = Infer<typeof SETTING_FIELD_WORDS>;
export type SettingsChecked = Infer<typeof SETTINGS_CHECKED>;
export type SettingItem = Infer<typeof SETTING_ITEM>;
export type SettingRow = Infer<typeof SETTING_ROW>;
export type SettingGroup = Infer<typeof SETTING_GROUP>;
export type SettingsPage = Infer<typeof SETTINGS_PAGE>;
export type SettingsSection = Infer<typeof SETTINGS_SECTION>;

export const SETTINGS_PAGE_ROUTES: RouteShape[] = [
  { route: "GET kuvo/settings", answers: { 200: SETTINGS_SECTIONS } },
  { route: "GET kuvo/settings/:section", answers: { 200: SETTINGS_PAGE, 404: REFUSED } },
  { route: "GET kuvo/settings/:section/events", answers: { 200: SETTINGS_PAGE, 404: REFUSED } },
  { route: "PATCH kuvo/settings/:section", body: SETTINGS_CHANGE, answers: { 200: SETTINGS_PAGE, 400: REFUSED, 403: REFUSED, 404: REFUSED, 409: SETTINGS_STALE } },
  { route: "POST kuvo/settings/:section/:row/:action", body: SETTINGS_ACT, answers: { 200: SETTINGS_PAGE, 400: SETTINGS_FORM_REFUSED, 403: REFUSED, 404: REFUSED, 409: SETTINGS_STALE } },
  { route: "POST kuvo/settings/:section/:row/:action/check", body: SETTINGS_CHECK, answers: { 200: SETTINGS_CHECKED, 400: REFUSED, 404: REFUSED, 409: SETTINGS_STALE } },
];
