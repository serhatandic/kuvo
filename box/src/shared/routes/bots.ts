// Bots (PROTOCOL.md, "Per area"): the bots that live on the box asked, New Bot's templates, a bot
// made, changed, deleted, brought here from another computer and told what it is now.

import { any, bool, type Infer, list, literal, map, NOTHING, nullable, number, object, type RouteShape, string, words } from "../shape";
import { PENDING_COMPUTERS } from "./list-computers";
import { LIMITS } from "./work";
import { BOT_NOW } from "./sessions";

/** How a bot looks: its colour, shape and face, by their names (`kuvo/bots/templates`' `looks`). */
/** The colors, shapes and faces a bot can have: those the apps draw (bot-words.json's `looks`). */
export const LOOKS = {
  colors: ["mint", "sky", "indigo", "violet", "pink", "coral", "orange", "amber", "lime", "graphite"],
  shapes: ["round", "squircle", "pebble", "gem", "bloom", "loaf", "capsule", "dome"],
  faces: ["calm", "happy", "sleepy", "wink", "curious", "bright"],
} as const;
export const LOOK = object({ color: words(...LOOKS.colors), shape: words(...LOOKS.shapes), face: words(...LOOKS.faces) });
/** A model as a bot names it: its id, its provider, and the variant (effort) it runs at, if any. */
export const MODEL = object({ id: string, providerID: string, "variant?": string });

/** What deleting a bot takes with it, in the words of the box that says it (`bots.deletes`): one
 * line for each thing, by what it is (`conversation`, `folder`, `wakes`, `groups`,
 * `sessions`; a client draws one it doesn't know all the same), `kept` for what stays. */
export const DELETES = list(object({ what: string, words: string, "kept?": literal(true) }));

/** A bot that lives here, by its session: its name and look; its job's line (what shows under its
 * name) and all of it; whether its wake-ups are paused, the next one (seconds since 1970, left out
 * when none waits) and how many things it watches; whether it runs with the coding agents; its
 * personality's id and its model, left out when it has none; its folder, the project it works in
 * (left out for a folder of its own) and when it was made, and the limits you set for its work
 * (left out for none). `machine`: the machine's name, left out on a Mac. `now`, in `kuvo/bots` and
 * its events (`bots.now`): what it's doing, its sessions at work and waiting on you. `deletes`: what
 * deleting it takes with it here (`bots.deletes`). */
export const BOT = object({
  id: string,
  session: string,
  name: string,
  look: LOOK,
  "machine?": string,
  job: string,
  jobDetails: string,
  paused: bool,
  "wakes?": number,
  watching: number,
  agents: bool,
  "personality?": string,
  "model?": MODEL,
  folder: string,
  "project?": string,
  "created?": string,
  "limits?": LIMITS,
  "allowed?": list(words("makeProject", "cloneProject")),
  "now?": BOT_NOW,
  deletes: DELETES,
});
/** A bot that lives on another of your own computers (`bots.elsewhere`), as a client draws it: as
 * that computer last told this box, and as you left it (its `name` and `look` those a change held
 * here gives it, over that computer's). Its id, conversation and job's line, that computer by its
 * key and by its name here, what kind it is, and what deleting it takes with it there; `waiting`
 * while this box holds an edit for it, `refused` when that computer refused one lately. `unlisted`:
 * no computer of yours lists it now, and it's here only by what's held or was refused (its
 * conversation, kind and `deletes` left out, and its `name` and `look` where this box never knew
 * them): nothing more is taken for it from here but what's held. `away` while that computer isn't
 * answering (as `kuvo/computers` says it), with when it was last heard from (`lastSeen`, ms);
 * `updating`, not away, while it only switches to a new release. */
export const BOT_ACTIVITY = object({ session: string, now: BOT_NOW, "paused?": bool, "wakes?": number });

export const BOT_ELSEWHERE = object({
  id: string,
  "session?": string,
  "name?": string,
  "look?": LOOK,
  "job?": string,
  box: string,
  computer: string,
  "kind?": words("mac", "machine"),
  "deletes?": DELETES,
  "waiting?": literal(true),
  "refused?": literal(true),
  "unlisted?": literal(true),
  "away?": literal(true),
  "lastSeen?": number,
  "updating?": literal(true),
  "now?": BOT_NOW, "paused?": bool, "wakes?": number,
});
/** A change to a bot (or its delete) that lives on another of your computers, held here until that
 * computer answers (PROTOCOL.md, "Per area"): the bot, that computer by its key (empty for one this box
 * doesn't know now, as the client named it: a machine you removed) and by its name here, the change as `PATCH kuvo/bots/<id>` takes it (each field the last given) or a delete, and
 * since when it's held (ms). `name` and `look`: the bot's, as its computer last told this box
 * (`bots.elsewhere`; left out for one no computer of yours ever listed to it). `waits`: why it
 * hasn't gone to a computer that answers, in words (that computer needs an update first). */
export const HELD_EDIT = object({ bot: string, box: string, computer: string, "name?": string, "look?": LOOK, "change?": map(any), "delete?": literal(true), since: number, "waits?": string });
/** What another computer refused of an edit held here for it, lately (a day): the bot, that
 * computer, the words to show, and when (ms); the bot's `name` and `look` as a held edit has them. */
export const REFUSED_EDIT = object({ bot: string, box: string, computer: string, "name?": string, "look?": LOOK, message: string, at: number });
/** The bots that live here, the first made first; the edits held here for bots on your other
 * computers, and what those refused lately (`bots.held`); and the bots that live on your other
 * computers, as each last told this box (`bots.elsewhere`). None of the three while this box
 * can't check what it holds: a client keeps what it last heard. */
export const BOTS = object({ ...PENDING_COMPUTERS, bots: list(BOT), "held?": list(HELD_EDIT), "refused?": list(REFUSED_EDIT), "elsewhere?": list(BOT_ELSEWHERE) });
/** An edit held here for the computer its bot lives on. */
export const HELD = object({ held: HELD_EDIT });

/** What New Bot offers here: its templates (a template's `personality` left out when it has
 * none), the personalities a bot can have (Kuvo Code's own, then yours, `custom`), the looks it
 * can have, the model a new bot starts on by its place (`mac`, or the machine's name; null for
 * none) and the projects it can work in (each with its machine's name, left out on a Mac). */
export const TEMPLATES = object({
  templates: list(object({ id: string, name: string, job: string, instructions: string, look: LOOK, "personality?": string, wantsProject: bool })),
  personalities: list(object({ id: string, name: string, description: string, "custom?": bool })),
  looks: object({ colors: list(string), shapes: list(string), faces: list(string) }),
  startingModel: map(nullable(MODEL)),
  projects: list(object({ directory: string, name: string, "machine?": string })),
});

/** A bot just made, and its job for your composer to send (from a template or a shared bot's
 * job), or null when it writes first. */
export const MADE = object({ bot: BOT, firstMessage: nullable(string) });
/** A bot as it is after a change, or once it's settled here from another computer (null should it
 * not list now). */
export const CHANGED = object({ bot: nullable(BOT) });

export type BotLook = Infer<typeof LOOK>;
export type BotModel = Infer<typeof MODEL>;
export type ListedBot = Infer<typeof BOT>;
export type Bots = Infer<typeof BOTS>;
export type BotTemplates = Infer<typeof TEMPLATES>;
export type MadeBot = Infer<typeof MADE>;
export type ChangedBot = Infer<typeof CHANGED>;
export type BotDeletes = Infer<typeof DELETES>;
export type BotElsewhere = Infer<typeof BOT_ELSEWHERE>;
export type HeldEdit = Infer<typeof HELD_EDIT>;
export type RefusedEdit = Infer<typeof REFUSED_EDIT>;

export const BOT_ROUTES: RouteShape[] = [
  { route: "GET kuvo/bots", feature: "bots", answers: { 200: BOTS } },
  { route: "GET kuvo/bots/events", feature: "bots.events", answers: { 200: BOTS } },
  // Makes a bot here, as New Bot does; the same `request` id gets the same answer for an hour.
  { route: "POST kuvo/bots", feature: "bots.make", answers: { 201: MADE } },
  // What New Bot offers here: templates, personalities, looks, the starting model and the projects.
  { route: "GET kuvo/bots/templates", feature: "bots.make", answers: { 200: TEMPLATES } },
  // Tells each bot here what it is now (its instructions from the box's words), after they changed.
  { route: "POST kuvo/bots/retell", feature: "bots.elsewhere", answers: { 200: NOTHING } },
  // Changes a bot as Edit Bot does; one on another of your computers is held for it (202).
  { route: "PATCH kuvo/bots/:id", feature: "bots.edit", answers: { 200: CHANGED, 202: HELD, 403: object({ message: string }) } },
  // Deletes a bot: its conversation, wake-ups and folder (to the trash); one elsewhere is held for it (202).
  { route: "DELETE kuvo/bots/:id", feature: "bots.edit", answers: { 200: NOTHING, 202: HELD, 403: object({ message: string }) } },
  // Makes a bot a client just moved here this box's own: its folder, clock, rules and what it's told.
  { route: "POST kuvo/bots/:id/arrived", feature: "bots", answers: { 200: CHANGED } },
  // Reads a bot's own conversation, never the sessions working for it.
  { route: "POST kuvo/bots/:id/seen", feature: "bots.state", answers: { 200: NOTHING, 404: object({ message: string }), 405: NOTHING, 500: object({ message: string }), 502: object({ message: string }), 503: object({ message: string }) } },
];
