// Edits to a bot whose computer is away, held by the box that was asked (PROTOCOL.md, `kuvo/bots`,
// "Held for another computer"): a client asks any box to change or delete a bot that lives on
// another of your computers, and that box takes it at once, keeps it for that computer, and hands
// it on by mail when that computer answers. So no client keeps an outbox of its own.
//
// One a bot, in `run/bots-held/<bot id>.json`: each field the last you gave it, and a delete over
// all of it. Sealed with this box's own secret, as a route left it: what anything else on this
// computer leaves there never goes out as you (this Mac's box keeps its keys in your Keychain).
// Kept on disk, as a machine's routes from a Mac over SSH are answered by a process of their own
// (`box route`) with no relay connection: its link sends what's held, each round.
//
// The computer that keeps the bot does it as Edit Bot or Delete Bot… would there, once for each
// edit, and answers; what it refuses is let go, and said, in plain words, to every client that
// follows this box's bots.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { LOOKS } from "../../shared/routes/bots";
import { knownElsewhere, shownName } from "./elsewhere";
import type { KnownBot } from "./elsewhere";
import { listOf, locked, textOf, writePrivate } from "../files";
import { IS_MAC, RUN } from "../home";
import { readPeerAgents } from "../peers/directory";
import { mailFor, peersOf } from "../peers/known";
import { ownName } from "../letters/start";
import { macLink, readLink } from "../state";
import { botHere, isModel, noteBotsChanged } from "./here";
import { editedTimes } from "./edited";
import { givenJob, givenJobFields } from "./jobs";
import { fitsLook } from "./make";
import type { BotChange } from "./edit";
import type { HeldEdit, RefusedEdit } from "../../shared/routes/bots";
import type { MailSender, Peer } from "../peers/known";
import type { ToldAgents } from "../../shared/records";
import type { LinkHost } from "../relay/host";
import type { LinkState } from "../state";

/** What a box's agents' mail says (`kuvo/info`'s `features` too) when it holds edits for your other
 * computers and takes those held for it. */
export const HELD_FEATURE = "bots.held";

export const HELD = path.join(RUN, "bots-held");
const LOCK = path.join(RUN, ".bots-held.lock");
const REFUSED_FILE = path.join(HELD, "refused.json");
// What this box did with edits held for it elsewhere, by asker and id: each done once.
const TAKEN_FILE = path.join(RUN, "bots-held-taken.json");
const BOT_ID = /^bot_[A-Za-z0-9]{1,60}$/;
const EDIT_ID = /^bte_[A-Za-z0-9_-]{16,40}$/;
/** What's held: Edit Bot's fields. A bot's work settings (its limits, what it may do without asking)
 * aren't: they change only where it lives, while that computer answers. */
const FIELDS = ["name", "look", "personality", "job", "jobLine", "jobDetails", "model", "effort"] as const;
/** What a computer says it answers (`features`) once it takes an `effort` in a change: one held
 * with it waits for a computer that doesn't yet, which would drop it and call the change done. */
const EFFORT_FEATURE = "bots.effort";
const JOB_FIELDS_FEATURE = "bots.job-fields";

/** How often what's held goes again to a computer that answers (the relay keeps nothing), and how
 * long a refusal shows. */
const AGAIN_AFTER = 60_000;
const REFUSAL_SHOWS = 86_400_000;
const MOST_REFUSALS = 50;
/** How long a keeper remembers what it did with each held edit (one sent again comes within
 * minutes; a few days covers a holder that was itself off meanwhile), and how many at most. */
const TAKEN_MEMORY = 7 * 86_400_000;
const MOST_TAKEN = 1_000;
/** The most a held change may be, in bytes as JSON (what's held goes in one mail). */
const MOST_CHANGE = 32_000;
/** How many times a computer may say a bot isn't there before a change held for it is refused,
 * while none lists it yet (one moving there). */
const NOT_THERE_TRIES = 3;

/** An edit held here: the bot's computer by its key (empty while this box doesn't know it: one you
 * removed, named by the client in `on`), the change (`PATCH kuvo/bots/<id>`'s fields) and when each
 * field was given (`times`, by this box's clock), or a delete, since when it's held and when it last
 * changed, this version's id, and its seal. */
interface Held {
  id: string;
  bot: string;
  to: string;
  on?: string;
  change?: BotChange;
  times?: Record<string, number>;
  delete?: true;
  since: number;
  at: number;
  /** The bot's name and look, as its computer last told this box by the time this was held (kept
   * for when that computer no longer lists it: one deleted there, one you removed). */
  known?: KnownBot;
  seal?: string;
}

/** How what's held goes at once, where this process sends it (this Mac's box, a machine's link). */
export const heldSoon: { send: () => void } = { send: () => {} };

const fileOf = (bot: string) => path.join(HELD, `${bot}.json`);
const newEditID = () => `bte_${crypto.randomBytes(16).toString("base64url")}`;

// The seal: an HMAC of the record with a key from this box's secret, which only it has (this Mac's
// box from your Keychain, in memory).
// This box's secret: this Mac's only as its app gave it (in memory), never one in a file there.
const secretOf = (link: LinkState | null): string | undefined => (IS_MAC ? macLink.overlay?.secret : link?.secret);

function sealWith(value: unknown, link: LinkState | null): string | null {
  const secret = secretOf(link);
  if (!secret) return null;
  const key = crypto.createHmac("sha256", Buffer.from(secret, "base64url")).update("kuvo bot edits held").digest();
  return crypto.createHmac("sha256", key).update(JSON.stringify(value)).digest("base64url");
}

const sealOf = (held: Omit<Held, "seal">, link: LinkState | null): string | null => {
  const { id, bot, to, on, change, times, since, at, known } = held;
  // (What it knew of the bot only where it's kept: one held before that is sealed as it was.)
  return sealWith([id, bot, to, on ?? null, change ?? null, times ?? null, held.delete === true, since, at, ...(known ? [known] : [])], link);
};

const sealed = (given: unknown, seal: string | null): boolean => typeof given === "string" && Boolean(seal) && given.length === seal!.length && crypto.timingSafeEqual(Buffer.from(seal!), Buffer.from(given));

function readHeld(bot: string, link: LinkState | null = readLink()): Held | null {
  let kept: unknown;
  try { kept = JSON.parse(textOf(fileOf(bot)) ?? ""); } catch { return null; }
  if (!isObject(kept) || kept.bot !== bot || typeof kept.id !== "string" || !EDIT_ID.test(kept.id) || typeof kept.to !== "string") return null;
  if (kept.on !== undefined && typeof kept.on !== "string") return null;
  if (kept.times !== undefined && !isObject(kept.times)) return null;
  if (typeof kept.since !== "number" || typeof kept.at !== "number" || typeof kept.seal !== "string") return null;
  if (kept.delete !== true && !isObject(kept.change)) return null;
  if (kept.known !== undefined && !(isObject(kept.known) && typeof kept.known.name === "string")) return null;
  const held = kept as unknown as Held;
  return sealed(held.seal, sealOf(held, link)) ? held : null;
}

function writeHeld(held: Omit<Held, "seal">, link: LinkState | null): boolean {
  const seal = sealOf(held, link);
  if (!seal) return false;
  writePrivate(fileOf(held.bot), JSON.stringify({ ...held, seal }));
  return true;
}

/** Every edit held here, as kept (unsealed ones left out). */
function allHeld(link: LinkState | null = readLink()): Held[] {
  return listOf(HELD).map(name => /^(bot_[A-Za-z0-9]{1,60})\.json$/.exec(name)?.[1]).filter((bot): bot is string => Boolean(bot))
    .map(bot => readHeld(bot, link)).filter((held): held is Held => Boolean(held)).sort((a, b) => a.since - b.since);
}

/** The computer that keeps `bot`, as its box last told this one (its agents), or else as the client
 * named it (`on`: its key, or its id on its Mac): one of this box's peers, never this box. */
export function keeperOf(bot: string, on: string | null | undefined, link: LinkState | null = readLink(), except?: string): Peer | null {
  const told = readPeerAgents();
  const peers = peersOf(link).filter(peer => peer.key !== except);
  // (Two that list it, one moving it to the other: the one that told this box last.)
  const listing = peers.filter(peer => told[peer.key]?.list?.some(agent => agent.kind === "bot" && agent.id === bot))
    .sort((a, b) => (told[b.key]?.heard ?? told[b.key]?.at ?? 0) - (told[a.key]?.heard ?? told[a.key]?.at ?? 0))[0];
  if (listing) return listing;
  if (!on) return null;
  return peers.find(peer => peer.key === on || (peer.kind === "machine" && peer.id === on)) ?? null;
}

const nameOf = (held: Pick<Held, "to" | "on">, link: LinkState | null): string => {
  const peer = held.to ? peersOf(link).find(each => each.key === held.to) : null;
  return peer ? shownName(peer) : held.on || "another computer";
};

// The bot a held edit or a refusal is for, as its computer says it now, else as this box knew it then.
const botOf = (bot: string, kept: KnownBot | undefined, known: Map<string, KnownBot>): { name?: string; look?: KnownBot["look"] } => {
  const { name, look } = known.get(bot) ?? kept ?? {};
  return { ...(name ? { name } : {}), ...(look ? { look } : {}) };
};

// An older computer would silently drop these fields, so the edit waits until it can take them.
function missingFeature(change: BotChange | undefined, features: readonly string[]): string | undefined {
  if (!change) return undefined;
  if ("effort" in change && !features.includes(EFFORT_FEATURE)) return "an effort";
  if (("jobLine" in change || "jobDetails" in change) && !features.includes(JOB_FIELDS_FEATURE)) return "separate Job and Instructions changes";
  return undefined;
}

const waitsOf = (held: Held, link: LinkState | null, told: Record<string, ToldAgents>): string | undefined => {
  const peer = told[held.to];
  if (!peer) return undefined;
  const missing = missingFeature(held.change, peer.features ?? []);
  if (!missing) return undefined;
  const computer = nameOf(held, link);
  return `${computer} needs an update before it takes ${missing}: this goes once Kuvo Code is updated on ${computer}.`;
};

const viewOf = (held: Held, link: LinkState | null, known: Map<string, KnownBot> = knownElsewhere(link), told: Record<string, ToldAgents> = readPeerAgents()): HeldEdit => ({
  bot: held.bot,
  box: held.to,
  computer: nameOf(held, link),
  ...botOf(held.bot, held.known, known),
  ...(waitsOf(held, link, told) ? { waits: waitsOf(held, link, told) } : {}),
  ...(held.delete ? { delete: true } : { change: held.change as Record<string, unknown> }),
  since: held.since,
});

/** What's held here for your other computers, and what they refused lately, as `kuvo/bots` lists
 * them for clients to show; neither while this box can't tell its own from what's planted (this
 * Mac's box before its app gives it its keys), so a client keeps what it last heard. */
export function heldLists(link: LinkState | null = readLink(), now = Date.now(), known?: Map<string, KnownBot>): { held?: HeldEdit[]; refused?: RefusedEdit[] } {
  if (!secretOf(link)) return {};
  const told = readPeerAgents();
  known ??= knownElsewhere(link, told);
  return {
    held: allHeld(link).map(each => viewOf(each, link, known, told)),
    refused: readRefused(now, link).map(({ bot, box, computer, name, look, message, at }) => ({ bot, box, computer, ...botOf(bot, name ? { name, look } : undefined, known), message, at })),
  };
}

// A change as Edit Bot gives it, checked as far as this box can without the bot (its computer
// checks the rest as it takes it): only the fields a change has, each of the right kind.
function checkedChange(given: unknown): { change: BotChange } | { error: string } {
  if (!isObject(given)) return { error: "That isn’t a change to a bot." };
  const change: Record<string, unknown> = {};
  for (const field of FIELDS) if (field in given) change[field] = given[field];
  if (!Object.keys(change).length) return { error: "That isn’t a change to a bot." };
  if ("name" in change) {
    change.name = plainName(change.name);
    if (!change.name) return { error: "A bot needs a name." };
  }
  if ("look" in change && !fitsLook({ colors: [...LOOKS.colors], shapes: [...LOOKS.shapes], faces: [...LOOKS.faces] }, change.look)) return { error: "That isn’t a look a bot can have." };
  if ("personality" in change && change.personality !== null && typeof change.personality !== "string") return { error: "That isn’t a personality." };
  if ("job" in change) {
    const checked = givenJob(change.job);
    if (checked.error) return { error: checked.error };
  }
  const jobProblem = givenJobFields(change).error;
  if (jobProblem) return { error: jobProblem };
  if ("model" in change && !isModel(change.model)) return { error: "That isn’t a model." };
  if ("effort" in change && change.effort !== null && typeof change.effort !== "string") return { error: "That isn’t an effort." };
  return { change: change as BotChange };
}

/** What `holdEdit` answers: held (202), or why not. */
export type HoldAnswer = { status: 202; value: { held: HeldEdit } } | { status: 400 | 404 | 409 | 503; value: { message: string } };

/** A change to `bot` (or its delete), which lives on another of your computers, held here for it:
 * on top of what's held already, a field at a time, a delete over all of it. `on`: the computer as
 * the client names it, for one this box hasn't heard of the bot from; with `removed`, one this box
 * doesn't know now (a machine you removed), held until it's one of its computers again. */
export function holdEdit(bot: string, given: { change: unknown } | { delete: true }, on: string | null | undefined, { removed = false, now = Date.now() }: { removed?: boolean; now?: number } = {}): HoldAnswer {
  const no = (status: 400 | 404 | 409 | 503, message: string): HoldAnswer => ({ status, value: { message } });
  if (!BOT_ID.test(bot)) return no(404, "That bot isn’t there any more.");
  const link = readLink();
  const keeper = keeperOf(bot, on, link);
  const named = removed && on ? plainName(on).slice(0, 200) : "";
  if (!keeper && !named) return no(404, "That bot isn’t there any more.");
  const computer = keeper ? shownName(keeper) : named;
  if ("change" in given && isObject(given.change) && "work" in given.change) return no(409, `${computer} isn’t connected: change its work settings once it’s back.`);
  const checked = "change" in given ? checkedChange(given.change) : null;
  if (checked && "error" in checked) return no(400, checked.error);
  let answer: HoldAnswer | null = null;
  try {
    fs.mkdirSync(HELD, { recursive: true, mode: 0o700 });
    locked(LOCK, () => {
      const before = readHeld(bot, link);
      // (Who it is, as its computer says now, else as was known when it was first held.)
      const known = knownElsewhere(link).get(bot) ?? before?.known;
      if (before?.delete && checked) {
        answer = no(404, `That bot is deleted: it goes from ${computer} once it’s back.`);
        return;
      }
      const given = checked ? Object.fromEntries(Object.keys(checked.change).map(field => [field, now])) : {};
      // (A model given without an effort runs at its own, as one changed where it lives does: an
      // effort held for the model it had doesn't go on to the new one.)
      const dropsEffort = Boolean(checked) && "model" in checked!.change && !("effort" in checked!.change);
      const { effort: _effort, ...changeBefore } = (before?.change ?? {}) as Record<string, unknown>;
      const { effort: _effortAt, ...timesBefore } = before?.times ?? {};
      const kept = dropsEffort
        ? { change: changeBefore as BotChange, times: timesBefore }
        : { change: { ...before?.change }, times: { ...before?.times } };
      if (checked && "job" in checked.change) {
        for (const field of ["jobLine", "jobDetails"] as const) {
          if (field in checked.change) continue;
          delete kept.change[field];
          delete kept.times[field];
        }
      }
      // (What's held goes in one mail: one too big for it never would.)
      if (checked && Buffer.byteLength(JSON.stringify({ ...kept.change, ...checked.change })) > MOST_CHANGE) {
        answer = no(400, "That’s too much to change at once.");
        return;
      }
      const held: Held = {
        id: newEditID(),
        bot,
        to: keeper?.key ?? "",
        ...(keeper ? {} : { on: named }),
        ...(checked ? { change: { ...kept.change, ...checked.change }, times: { ...kept.times, ...given } } : { delete: true as const }),
        since: before?.since ?? now,
        at: now,
        ...(known ? { known } : {}),
      };
      if (!writeHeld(held, link)) {
        answer = no(503, `${ownName()} can’t keep it now. Try again in a moment.`);
        return;
      }
      sent.delete(bot);
      notThere.delete(bot);
      dropRefusal(bot, link);
      answer = { status: 202, value: { held: viewOf(held, link) } };
    });
  } catch {
    return no(503, `${ownName()} can’t keep it now. Try again in a moment.`);
  }
  noteBotsChanged();
  heldSoon.send();
  return answer!;
}

/** For a client: whether `bot` lives here (its route changes it) or is held for another computer. */
export const livesHere = (bot: string): boolean => Boolean(botHere(bot));

// When each held edit last went, in this process, by bot.
const sent = new Map<string, { id: string; at: number }>();
// How many times computers said a bot a change is held for isn't there, by bot.
const notThere = new Map<string, number>();

/** `key` answers again: what's held for it goes now, not at its next minute. */
export function heldBack(key: string): void {
  let any = false;
  for (const held of allHeld()) {
    if (held.to !== key) continue;
    sent.delete(held.bot);
    any = true;
  }
  if (any) heldSoon.send();
}

/** What's held here, sent by this process's relay connection to each computer it's for (each
 * round, and as it's held): again each minute until it answers, and at once when it's back. One
 * whose bot another of your computers lists now goes there instead (it moved). Only to a computer
 * that takes it (`HELD_FEATURE`); one that doesn't yet waits until it's updated. */
export function sendHeld(host: Pick<LinkHost, "send" | "welcomed"> | null | undefined, link: LinkState | null = readLink(), now = Date.now()): void {
  if (!host?.welcomed || !link) return;
  const told = readPeerAgents();
  for (let held of allHeld(link)) {
    // (Another computer only once the one it's held for doesn't list it: one that said it isn't there
    // is left for its answer, `takeBotEdited`.)
    const stillThere = Boolean(held.to) && peersOf(link).some(peer => peer.key === held.to) && told[held.to]?.list?.some(agent => agent.kind === "bot" && agent.id === held.bot);
    const keeper = stillThere ? null : keeperOf(held.bot, held.on, link);
    if (keeper && keeper.key !== held.to) {
      const moved = locked(LOCK, () => {
        const now = readHeld(held.bot, link);
        if (now?.id !== held.id) return null;
        const { on: _, ...rest } = held;
        const next = { ...rest, to: keeper.key };
        return writeHeld(next, link) ? next : null;
      });
      if (!moved) continue;
      held = moved;
      sent.delete(held.bot);
      noteBotsChanged();
    }
    const last = sent.get(held.bot);
    if (last?.id === held.id && now - last.at < AGAIN_AFTER) continue;
    const peer = peersOf(link).find(each => each.key === held.to);
    if (!peer || !(told[peer.key]?.features ?? []).includes(HELD_FEATURE)) continue;
    if (missingFeature(held.change, told[peer.key]?.features ?? [])) continue;
    // (How long ago each field was given, not when: the computers' clocks may not agree.)
    const ages = Object.fromEntries(Object.entries(held.times ?? {}).map(([field, at]) => [field, Math.max(0, now - at)]));
    const bytes = mailFor(peer, { botEdit: { id: held.id, bot: held.bot, ...(held.delete ? { delete: true } : { change: held.change, ages }), at: held.at }, at: now }, link);
    if (!bytes) continue;
    try {
      host.send(bytes);
      sent.set(held.bot, { id: held.id, at: now });
    } catch {}
  }
}

// What the computers held edits went to refused lately, the newest first (sealed as what's held is).
function readRefused(now = Date.now(), link: LinkState | null = readLink()): RefusedEdit[] {
  try {
    const kept: unknown = JSON.parse(textOf(REFUSED_FILE) ?? "");
    if (!isObject(kept) || !Array.isArray(kept.refused) || !sealed(kept.seal, sealWith(kept.refused, link))) return [];
    return kept.refused.filter((each): each is RefusedEdit => isObject(each) && typeof each.bot === "string" && typeof each.box === "string" && typeof each.computer === "string"
      && typeof each.message === "string" && typeof each.at === "number" && now - each.at < REFUSAL_SHOWS);
  } catch {
    return [];
  }
}

function writeRefused(refused: RefusedEdit[], link: LinkState | null): void {
  if (!refused.length) return void fs.rmSync(REFUSED_FILE, { force: true });
  const kept = refused.slice(0, MOST_REFUSALS);
  const seal = sealWith(kept, link);
  if (seal) writePrivate(REFUSED_FILE, JSON.stringify({ refused: kept, seal }));
}

// (Under LOCK.)
function dropRefusal(bot: string, link: LinkState | null): void {
  const refused = readRefused(Date.now(), link);
  if (refused.some(each => each.bot === bot)) writeRefused(refused.filter(each => each.bot !== bot), link);
}

/** The answer from the computer a held edit went to, from that computer only: done, it's let go;
 * refused for good, it's let go and said; one it can't take now (the bot moving, its engine
 * starting) goes again. A bot that's no longer there: a delete is done; a change goes to the
 * computer that lists it now, or is refused. */
export function takeBotEdited(sender: MailSender, given: unknown, now = Date.now()): void {
  if (!isObject(given) || typeof given.id !== "string" || !EDIT_ID.test(given.id) || typeof given.bot !== "string" || !BOT_ID.test(given.bot)) return;
  const status = given.status;
  if (typeof status !== "number" || !Number.isInteger(status) || status < 200 || status > 599) return;
  const link = readLink();
  const bot = given.bot;
  let changed = false;
  try {
    locked(LOCK, () => {
      const held = readHeld(bot, link);
      if (!held || held.id !== given.id || held.to !== sender.key) return;
      if (status === 409 || status >= 500) {
        sent.set(bot, { id: held.id, at: now });
        return;
      }
      if (status === 404 && !held.delete) {
        // (No more than a few times, wherever it's sent: two computers whose lists both still name
        // it don't pass it between them for ever.)
        const tries = (notThere.get(bot) ?? 0) + 1;
        if (tries < NOT_THERE_TRIES) {
          notThere.set(bot, tries);
          const keeper = keeperOf(bot, null, link, sender.key);
          if (keeper) {
            // (Moved meanwhile: it goes where the bot is now, as a new edit, which that computer
            // hasn't answered yet.)
            changed = writeHeld({ ...held, id: newEditID(), to: keeper.key }, link);
            sent.delete(bot);
            return;
          }
          // (No other lists it yet: it may be on its way to one. Asked again in a while.)
          sent.set(bot, { id: held.id, at: now });
          return;
        }
      }
      notThere.delete(bot);
      fs.rmSync(fileOf(bot), { force: true });
      sent.delete(bot);
      changed = true;
      const why = isObject(given) && typeof given.message === "string" ? plainName(given.message).slice(0, 500) : "";
      // (Done: but for some of it, said.)
      if ((status < 300 && !why) || (status === 404 && held.delete)) return;
      const computer = nameOf(held, link);
      const message = held.delete
        ? `${computer} didn’t delete the bot as you asked while it was away.${why ? ` ${why}` : ""}`
        : status < 300
          ? `${computer} didn’t take all the changes you made while it was away.${why ? ` ${why}` : ""}`
          : `${computer} didn’t take the changes you made while it was away.${why ? ` ${why}` : ""}`;
      // (With who it is, as known now: its computer may not list it any more by the time it shows.)
      const known = knownElsewhere(link).get(bot) ?? held.known;
      writeRefused([{ bot, box: held.to, computer, ...(known?.name ? { name: known.name, ...(known.look ? { look: known.look } : {}) } : {}), message, at: now }, ...readRefused(now, link).filter(each => each.bot !== bot)], link);
    });
  } catch {}
  if (changed) {
    noteBotsChanged();
    if (status === 404) heldSoon.send();
  }
}

// What this box did with each held edit sent to it, by asker and id.
interface Taken { until: number; status: number; message?: string }

function readTaken(now: number): Record<string, Taken> {
  try {
    const kept: unknown = JSON.parse(textOf(TAKEN_FILE) ?? "");
    if (!isObject(kept)) return {};
    return Object.fromEntries(Object.entries(kept).filter(([, each]) => isObject(each) && typeof each.until === "number" && each.until > now && typeof each.status === "number")) as Record<string, Taken>;
  } catch {
    return {};
  }
}

function keepTaken(key: string, taken: Taken, now: number): void {
  try {
    const kept = readTaken(now);
    delete kept[key];
    kept[key] = taken;
    const entries = Object.entries(kept);
    writePrivate(TAKEN_FILE, JSON.stringify(Object.fromEntries(entries.slice(Math.max(0, entries.length - MOST_TAKEN)))));
  } catch {}
}

// Those under way in this process, by asker and id (one sent again meanwhile isn't done twice).
const taking = new Set<string>();

/** What this box does with a held edit: as Edit Bot changes a bot (as of `at`, when you made it),
 * or Delete Bot… deletes it. */
export interface EditSteps {
  change: (bot: string, change: BotChange, at: Record<string, number>) => Promise<{ status: number; value: unknown }>;
  delete: (bot: string) => Promise<{ status: number; value: unknown }>;
}

/** An edit another of your computers held for this one, done here as a client's would be, once for
 * each id, and answered (`reply`); one sent again is answered as it went. One this box can't do
 * now (the bot moving, its engine starting) isn't remembered: it's done when it comes again. */
export async function takeBotEdit(sender: MailSender, given: unknown, reply: (payload: Record<string, unknown>) => void, steps: EditSteps, now = Date.now()): Promise<void> {
  if (!isObject(given) || typeof given.id !== "string" || !EDIT_ID.test(given.id) || typeof given.bot !== "string" || !BOT_ID.test(given.bot)) return;
  if (given.delete !== true && !isObject(given.change)) return;
  const link = readLink();
  // (Only from one of your computers that's still a peer.)
  const peer = peersOf(link).find(each => each.key === sender.key);
  if (!peer) return;
  const { id, bot } = given;
  const answer = (status: number, message?: string) => reply({ botEdited: { id, bot, status, ...(message ? { message } : {}) } });
  const key = `${sender.key} ${id}`;
  if (taking.has(key)) return;
  const kept = readTaken(now)[key];
  if (kept) return answer(kept.status, kept.message);
  taking.add(key);
  try {
    let done: { status: number; value: unknown };
    try {
      if (given.delete === true) done = await steps.delete(bot);
      else {
        // Only what was given after it last changed here (a change made here meanwhile stays), and
        // never its work settings, which change only where it lives. When each was given is worked
        // out here from how long ago the holder said it was (its clock may not agree with this one's).
        const ages = isObject(given.ages) ? given.ages : {};
        const clock = Date.now();
        const times: Record<string, number> = {};
        for (const [field, age] of Object.entries(ages)) if (typeof age === "number" && Number.isFinite(age)) times[field] = clock - Math.max(0, age);
        const change = { ...given.change as Record<string, unknown> };
        // A whole job from an older sender may be fresh for only one of its fields.
        if ("job" in change) {
          const legacy = givenJob(change.job).job;
          if (legacy) {
            for (const [field, value] of [["jobLine", legacy.line], ["jobDetails", legacy.details]] as const) {
              if (field in change) continue;
              change[field] = value;
              if (times.job !== undefined) times[field] = times.job;
            }
            delete change.job;
          }
        }
        const edited = editedTimes(bot);
        // (An effort is its model's: one given before its model last changed here is stale too.)
        const lastEdited = (field: string): number => {
          if (field === "effort") return Math.max(edited.model ?? 0, edited.effort ?? 0);
          if (field === "jobLine" || field === "jobDetails") return Math.max(edited.job ?? 0, edited[field] ?? 0);
          if (field === "job") return Math.max(edited.job ?? 0, edited.jobLine ?? 0, edited.jobDetails ?? 0);
          return edited[field] ?? 0;
        };
        const fresh = Object.entries(change).filter(([field]) => (FIELDS as readonly string[]).includes(field)
          && !(times[field] !== undefined && times[field] <= lastEdited(field)));
        // (An effort given with a model is that model's: not for the one it has once that model isn't taken.)
        const taken = "model" in change && !fresh.some(([field]) => field === "model") ? fresh.filter(([field]) => field !== "effort") : fresh;
        done = taken.length ? await changeWhatItCan(bot, taken, times, steps) : { status: 200, value: {} };
      }
    } catch {
      done = { status: 503, value: { message: `Something went wrong on ${ownName()}.` } };
    }
    const message = isObject(done.value) && typeof done.value.message === "string" ? done.value.message : undefined;
    // (Not one it couldn't do now, nor a change for a bot not here yet: done when it comes again.)
    if (done.status !== 409 && done.status < 500 && !(done.status === 404 && given.delete !== true)) keepTaken(key, { until: now + TAKEN_MEMORY, status: done.status, ...(message ? { message } : {}) }, now);
    answer(done.status, message);
  } finally {
    taking.delete(key);
  }
}

// A held change done as far as it can be: whole, or, where that's refused for what's in it (a
// personality there isn't, a model it can't use), each field on its own, what's refused said.
async function changeWhatItCan(bot: string, fields: [string, unknown][], times: Record<string, number>, steps: EditSteps): Promise<{ status: number; value: unknown }> {
  const whole = await steps.change(bot, Object.fromEntries(fields) as BotChange, times);
  if (fields.length < 2 || (whole.status !== 400 && whole.status !== 422)) return whole;
  const refused: string[] = [];
  let took = false;
  // (A model and the effort given with it are one: an effort alone would be the old model's.)
  const together = fields.some(([field]) => field === "model") ? fields.filter(([field]) => field === "model" || field === "effort") : [];
  const parts = [...fields.filter(field => !together.includes(field)).map(field => [field]), ...(together.length ? [together] : [])];
  if (parts.length < 2) return whole;
  for (const part of parts) {
    const one = await steps.change(bot, Object.fromEntries(part) as BotChange, times);
    // (One that can't be done now, its engine starting say: all of it again later, what was done
    // now skipped then by its time.)
    if (one.status === 409 || one.status >= 500) return one;
    if (one.status < 300) took = true;
    else if (isObject(one.value) && typeof one.value.message === "string") refused.push(one.value.message);
  }
  return took ? { status: 200, value: refused.length ? { message: refused.join(" ") } : {} } : whole;
}

/** All this process remembers forgotten (a test's: what's on disk stays, as for a box that started
 * again). */
export function forgetHeldSends(): void {
  sent.clear();
  notThere.clear();
  taking.clear();
}
