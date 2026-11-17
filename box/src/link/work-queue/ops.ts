// What changes the Work Queue (PROTOCOL.md, "The Work Queue"): an item added, changed, handed on,
// deleted or brought back, moved by you, and files kept with it. The tools and the routes both come
// here, so an agent's change and yours follow the same rules.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
import { inline } from "../server/attached";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { accountHome, APP_FOLDER, BOTS, homeFolder, K, OWN_HOME_MAC } from "../home";
import { APP_TEMPORARY_PATTERN } from "../fence/policy";
import { labelNamed, MOST_LABELS_EACH } from "./labels";
import { isClosed, priorityNamed, PRIORITIES, statusName, statusNamed, STATUSES } from "./statuses";
import { assigneeFor } from "./assignees";
import { takeNumber, type Primary } from "./numbers";
import {
  changeQueues, dayOf, FILE_NAME, FILES, filesOf, matching, MOST_BODY, MOST_EVENTS, MOST_FILE, MOST_FILES, MOST_ITEMS, MOST_LINKS, MOST_NOTE, MOST_NOTES, MOST_TITLE, newItemID, noteHanded, placeBeside, placeIn, Refusal,
  type Book, type Item, type ItemEvent, type KeptFile, type Source, type Who,
} from "./store";
import { nameOf, refOf } from "./view";
import { doneByRead, seenOf } from "./github";
import { frozen } from "../moves/records";

/** A session is linked to this many items at most: linking one more drops its oldest link. */
export const LINKS_EACH = 3;
const MOST_PROGRESS_NOTE = 300;

export { Refusal };
const refuse = (words: string): never => { throw new Refusal(words); };

// Where files an agent named wait, read, until the change that keeps them.
const STAGING = path.join(path.dirname(FILES), "staging");

/** What's left in the staging folder by a box that stopped midway: gone, once it's an hour old. */
export function clearStaging(now = Date.now()): void {
  for (const name of (() => { try { return fs.readdirSync(STAGING); } catch { return []; } })()) {
    try {
      if (now - fs.statSync(path.join(STAGING, name)).mtimeMs > 3_600_000) fs.rmSync(path.join(STAGING, name), { recursive: true, force: true });
    } catch {}
  }
}

// A file's name, short enough to keep with " 2" added and stay within a name's 255 bytes: its stem
// cut, its extension kept.
const shortName = (name: string): string => {
  const ext = path.extname(name).slice(0, 20);
  let stem = name.slice(0, name.length - path.extname(name).length);
  while (stem && Buffer.byteLength(stem + ext) > 180) stem = Array.from(stem).slice(0, -1).join("");
  return stem + ext;
};

/** Changes to the queues of owners that are moving to another computer wait until they're there:
 * what's changed meanwhile would be left behind. */
export function refuseMoving(owners: (string | undefined)[], moving: Set<string> = frozen()): void {
  for (const owner of owners) if (owner && moving.has(owner)) refuse(owner.startsWith("grp_") ? "This group chat is moving to another computer, with its Work Queue: try again once it's there." : "That conversation is moving to another computer, with its Work Queue: try again once it's there.");
}

/** Changes to an item on its way to another computer wait until it's there: it's that
 * conversation's then, and what's changed here meanwhile would be left behind. */
export function refuseHanding(item: Item): void {
  if (item.handing) refuse(`${nameOf(item)} is being handed to ${item.handing.name || item.handing.to} on another computer: change it there once it's arrived.`);
}

/** Where an agent's files may come from: its conversation's folder, its project's other folders,
 * and the temporary folders; never Kuvo Code's own (but for its own folder there, a bot's). */
export interface FilePlaces { own: string | null; others: readonly string[] }

const real = (place: string): string | null => {
  try { return fs.realpathSync(place); } catch { return null; }
};
const within = (file: string, folder: string) => file === folder || file.startsWith(folder.endsWith(path.sep) ? folder : folder + path.sep);

/** Whether `file` (where it really is) is in this Mac's app's folders, for a Mac's box in a home of its
 * own, whose home isn't in them: any copy's, in your Library (`~/Library/<Application Support,
 * Caches…>/run.kuvo.code…`) or its temporary folder (`/private/var/folders/…/run.kuvo.code…`), as the
 * sandbox keeps them (fence/policy.ts, `kuvoPatterns`). */
function inMacApp(file: string): boolean {
  if (!OWN_HOME_MAC || process.platform !== "darwin") return false;
  if (new RegExp(APP_TEMPORARY_PATTERN, "i").test(file)) return true;
  return [...new Set([homeFolder(), accountHome()].map(each => real(each) ?? each))]
    .some(home => within(file, path.join(home, "Library")) && /^run\.kuvo\.code/i.test(path.relative(path.join(home, "Library"), file).split(path.sep)[1] ?? ""));
}

/** A file an agent named, read into `staging` (outside the queue's lock): checked where it really
 * is (links followed) to be a plain file of a size the queue keeps, in one of `places`, with no
 * hidden folder or file on the way; then opened without following a link or waiting on it, and
 * kept only if what was opened is that same file, with no other name (a hard link elsewhere). */
export function stageFile(given: string, places: FilePlaces, staging: string): Staged {
  if (!path.isAbsolute(given)) refuse(`Give ${given} as a full path.`);
  const file = real(given) ?? refuse(`There's no file at ${given}.`);
  const own = places.own ? real(places.own) : null;
  const roots = [own, ...places.others.map(real), real(os.tmpdir()), real("/tmp")].filter((each): each is string => Boolean(each));
  const root = roots.find(each => within(file, each));
  // Kuvo Code's own folders (on a Mac, the app's around its box's): never, unless the conversation's
  // own folder is itself in them (a bot's, a chat's), and then only what's in that folder. A project
  // that holds them (one at the home folder) doesn't make them its own.
  const boxes = [real(K), real(BOTS), APP_FOLDER ? real(APP_FOLDER) : null].filter((each): each is string => Boolean(each));
  const ownOK = own !== null && root === own && boxes.some(box => within(own, box));
  if (!root || (!ownOK && (boxes.some(box => within(file, box)) || inMacApp(file)))) refuse(`${given} isn't in this conversation's folder, its project or a temporary folder: only files from there are kept with an item.`);
  if (path.relative(root!, file).split(path.sep).some(part => part.startsWith("."))) refuse(`${given} is hidden (a name starting with a dot): it isn't kept with an item.`);
  const checked = fs.lstatSync(file);
  if (!checked.isFile()) refuse(`${given} isn't a file.`);
  if (checked.size > MOST_FILE) refuse(`${given} is over ${Math.round(MOST_FILE / 1_000_000)} MB: too big to keep with an item.`);
  let handle: number;
  try {
    handle = fs.openSync(file, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0) | (fs.constants.O_NONBLOCK ?? 0));
  } catch {
    return refuse(`${given} can't be read.`);
  }
  try {
    const opened = fs.fstatSync(handle);
    // (And still where it was found: a folder on the way swapped for a link meanwhile is said.)
    if (!opened.isFile() || opened.dev !== checked.dev || opened.ino !== checked.ino || real(given) !== file) refuse(`${given} changed as it was read: try again.`);
    if (opened.nlink !== 1) refuse(`${given} has other names (a hard link): it isn't kept with an item.`);
    const size = Math.min(opened.size, MOST_FILE);
    const bytes = Buffer.alloc(size);
    let read = 0;
    while (read < size) {
      const got = fs.readSync(handle, bytes, read, size - read, read);
      if (!got) break;
      read += got;
    }
    fs.mkdirSync(staging, { recursive: true, mode: 0o700 });
    const staged = path.join(staging, `${crypto.randomBytes(8).toString("hex")}`);
    fs.writeFileSync(staged, bytes.subarray(0, read), { mode: 0o600 });
    return { staged, name: path.basename(file), size: read };
  } finally {
    fs.closeSync(handle);
  }
}

/** A file read for an item, waiting in the staging folder until the change that keeps it. */
export interface Staged { staged: string; name: string; size: number; clientIndex?: number }

/** The files an agent named, read before the queue's lock is taken: what was read, and a way to
 * clear what's left of it. */
export function stageFiles(given: unknown, places: FilePlaces | null): { staged: Staged[]; clear: () => void } {
  const named = given === undefined ? [] : (Array.isArray(given) ? given : [given]).map(each => (typeof each === "string" ? each.trim() : "")).filter(Boolean);
  if (named.length && !places) refuse("Only an agent's own files are kept with an item.");
  if (named.length > MOST_FILES) refuse(`An item keeps ${MOST_FILES} files at most.`);
  const staging = path.join(STAGING, crypto.randomBytes(8).toString("hex"));
  const clear = () => fs.rmSync(staging, { recursive: true, force: true });
  try {
    return { staged: named.map(each => stageFile(each, places!, staging)), clear };
  } catch (error) {
    clear();
    throw error;
  }
}

/** A file a client attached, as the composer carries it (`{uri, name?}`): its bytes and name. A
 * local `file://` URL is read only for Kuvo Code's app on this same computer (`local`): from a
 * phone, another computer or a forwarded act it's refused, so nothing can read this computer's
 * files through an item. */
function clientFile(entry: unknown, local: boolean): { bytes: Buffer; name: string } {
  if (!isObject(entry) || typeof entry.uri !== "string") return refuse("A file needs its carried URI.");
  let name = typeof entry.name === "string" ? entry.name : "";
  const carried = inline(entry);
  if (carried) {
    if (carried.bytes.length > MOST_FILE) refuse(`A file is over ${Math.round(MOST_FILE / 1_000_000)} MB: too big to keep with an item.`);
    return { bytes: carried.bytes, name: name || `attachment${Object.entries(TYPES).find(([, mime]) => mime === carried.mime)?.[0] ?? ".bin"}` };
  }
  if (!local || !entry.uri.startsWith("file://")) return refuse("Send the file's contents with it.");
  let handle: number;
  try {
    handle = fs.openSync(fileURLToPath(entry.uri), fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0) | (fs.constants.O_NONBLOCK ?? 0));
  } catch {
    return refuse("That attached file can't be read: attach it again.");
  }
  try {
    const opened = fs.fstatSync(handle);
    if (!opened.isFile()) refuse("That attached file isn't a file.");
    if (opened.size > MOST_FILE) refuse(`A file is over ${Math.round(MOST_FILE / 1_000_000)} MB: too big to keep with an item.`);
    return { bytes: fs.readFileSync(handle), name: name || path.basename(fileURLToPath(entry.uri)) };
  } finally {
    fs.closeSync(handle);
  }
}

/** The files a client attached to an item it adds or changes (`files`), read into `staging` before
 * the queue's lock is taken, as an agent's are (`stageFiles`); the change keeps them with the item. */
export function stageClientFiles(given: unknown, local: boolean): { staged: Staged[]; clear: () => void } {
  if (given === undefined) return { staged: [], clear: () => {} };
  if (!Array.isArray(given)) return refuse("Send files as a list.");
  if (given.length > MOST_FILES) refuse(`An item keeps ${MOST_FILES} files at most.`);
  const folder = path.join(STAGING, crypto.randomBytes(8).toString("hex"));
  const clear = () => fs.rmSync(folder, { recursive: true, force: true });
  try {
    const staged = given.map((entry, index) => {
      const { bytes, name: named } = clientFile(entry, local);
      const name = shortName(path.basename(named.replace(/\\/g, "/")));
      if (!FILE_NAME.test(name)) refuse("That file needs a name.");
      fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
      const staged = path.join(folder, String(index));
      fs.writeFileSync(staged, bytes, { mode: 0o600 });
      return { staged, name, size: bytes.length, clientIndex: index };
    });
    return { staged, clear };
  } catch (error) { clear(); throw error; }
}

/** Files more than can go to another computer at once (413). */
export class TooBig extends Refusal {}

/** What a client attached, for another computer's box (`there`, by name): a local file's bytes
 * carried with it (only Kuvo Code's app here may name one, `local`), as the other box never reads
 * a path it's sent; `most` bytes in all, as much as goes between boxes at once. */
export function carriedFiles(given: unknown, local: boolean, there: string, most: number): unknown {
  if (!Array.isArray(given)) return given;
  if (given.length > MOST_FILES) refuse(`An item keeps ${MOST_FILES} files at most.`);
  let total = 0;
  return given.map(entry => {
    const carried = isObject(entry) && typeof entry.uri === "string" ? inline(entry) : null;
    const { bytes, name } = carried ? { bytes: carried.bytes, name: "" } : clientFile(entry, local);
    total += bytes.length;
    if (total > most) throw new TooBig(`These files are too big to send to ${there} with one change (${most / 1_000_000} MB in all at most). Attach fewer or smaller files.`);
    return carried ? entry : { uri: `data:${typeOf(name)};base64,${bytes.toString("base64")}`, name };
  });
}

const TYPES: Record<string, string> = {
  ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif", ".webp": "image/webp", ".heic": "image/heic", ".bmp": "image/bmp", ".tiff": "image/tiff", ".tif": "image/tiff", ".svg": "image/svg+xml",
  ".pdf": "application/pdf", ".md": "text/markdown", ".txt": "text/plain", ".json": "application/json", ".csv": "text/csv", ".mp4": "video/mp4", ".mov": "video/quicktime",
};
export const typeOf = (name: string) => TYPES[path.extname(name).toLowerCase()] ?? "application/octet-stream";

const text = (value: unknown, most: number, what: string): string => {
  if (typeof value !== "string") return refuse(`${what} should be text.`);
  if (value.length > most) refuse(`${what} is over ${most.toLocaleString("en-US")} characters: make it shorter.`);
  return value;
};

const linksOf = (value: unknown): { url: string; title?: string }[] => {
  const given = Array.isArray(value) ? value : [value];
  return given.map(each => {
    const url = typeof each === "string" ? each : isObject(each) && typeof each.url === "string" ? each.url : "";
    if (!/^https?:\/\/\S+$/i.test(url.trim())) refuse(`${url || "A link"} isn't a web address (https://…).`);
    const title = isObject(each) && typeof each.title === "string" ? plainName(each.title).slice(0, 200) : "";
    return { url: url.trim().slice(0, 2000), ...(title ? { title } : {}) };
  });
};

/** A status an agent or you named for an item, or why not. */
export function statusFor(given: unknown): string {
  const found = statusNamed(given);
  if (!found) return refuse(`There's no status ${String(given ?? "")}: it's one of ${STATUSES.map(each => each.name).join(", ")}.`);
  return found.id;
}

/** A priority named for an item (none kept for Normal), or why not. */
function priorityFor(given: unknown): string | undefined {
  if (given === null) return undefined;
  const found = priorityNamed(given);
  if (!found) return refuse(`There's no priority ${String(given ?? "")}: it's one of ${PRIORITIES.map(each => each.name).join(", ")}.`);
  return found.id === "normal" ? undefined : found.id;
}

/** The labels named for an item (by name or id; all of them, in place of those it had), or why not. */
function labelsFor(given: unknown): string[] {
  const named = given === null ? [] : Array.isArray(given) ? given : [given];
  const ids = named.map(each => labelNamed(each)?.id ?? refuse(`There's no label ${String(each ?? "")}: queue_labels lists them, and makes one.`));
  const once = [...new Set(ids)];
  if (once.length > MOST_LABELS_EACH) refuse(`An item carries ${MOST_LABELS_EACH} labels at most.`);
  return once;
}

/** `session` linked to `item`, its oldest other link dropped when it has `LINKS_EACH` already
 * (said there as its work stopped, where it had said it started). */
export function linkSession(book: Book, item: Item, session: string, now: number): void {
  if (item.sessions.some(each => each.session === session)) return;
  const others = book.items().filter(each => each.id !== item.id && each.sessions.some(link => link.session === session));
  if (others.length >= LINKS_EACH) {
    const oldest = others.sort((a, b) => a.sessions.find(link => link.session === session)!.since - b.sessions.find(link => link.session === session)!.since)[0];
    unlinkSessions(oldest, [session], now);
  }
  item.sessions.push({ session, since: now });
}

// Reorders within one status by the same one, each soon after the last, are one move in the feed.
const FOLD_REORDERS = 10 * 60_000;
// A session that stops and starts again this soon never stopped, as the feed says it.
const FOLD_RESTART = 2 * 60_000;

/** Who did something, the same one: you, or one agent. */
const sameWho = (a: Who, b: Who): boolean => JSON.stringify(a) === JSON.stringify(b);

/** What happened to `item` kept with it, its last `MOST_EVENTS`. A reorder right after the same
 * one's reorder in the same status is folded into it; a start right after the same one's stop
 * is no stop at all: each with nothing said between (no note, no hand-off), and never back in time
 * (a clock ahead on another computer). */
export function noteEvent(item: Item, event: ItemEvent): void {
  const events = [...(item.events ?? [])];
  const last = events.at(-1);
  const reorder = (each: ItemEvent) => each.kind === "moved" && each.from === each.to;
  const quiet = (since: number) => !item.notes.some(note => note.at >= since) && !(item.handed ?? []).some(each => each.at >= since);
  const within = (most: number) => last !== undefined && event.at >= last.at && event.at - last.at < most;
  if (last && reorder(last) && reorder(event) && last.to === event.to && sameWho(last.by, event.by) && within(FOLD_REORDERS) && quiet(last.at)) {
    const { top: _, ...folded } = last;
    events[events.length - 1] = { ...folded, at: event.at, ...(event.top ? { top: true as const } : {}) };
  } else if (last && last.kind === "stopped" && event.kind === "started" && sameWho(last.by, event.by) && within(FOLD_RESTART) && quiet(last.at)) {
    events.pop();
  } else {
    events.push(event);
  }
  item.events = events.slice(-MOST_EVENTS);
}

// The latest word of `session`'s work on `item`, that it started or stopped, since the item was
// last done (Done unlinked everyone, saying no `stopped`: a word from before it is over).
function lastWord(item: Item, session: string): ItemEvent | undefined {
  for (const each of [...(item.events ?? [])].reverse()) {
    if (each.kind === "done") return undefined;
    if ((each.kind === "started" || each.kind === "stopped") && (each.by as { session?: unknown }).session === session) return each;
  }
  return undefined;
}

/** `sessions` unlinked from `item`: each that had said it started is kept as having stopped (as
 * it said it started), when its agent said so or its link was dropped for it. Done and a delete
 * unlink everyone without this: they say it themselves. */
export function unlinkSessions(item: Item, sessions: Iterable<string>, now: number, who?: Who): void {
  for (const session of sessions) {
    if (!item.sessions.some(each => each.session === session)) continue;
    item.sessions = item.sessions.filter(each => each.session !== session);
    const word = lastWord(item, session);
    if (word?.kind === "started") noteEvent(item, { at: now, by: who ?? word.by, kind: "stopped" });
  }
}

/** `session` at work on `item`, as its agent said (`working: true`): linked to it, the item In
 * progress, and kept as having started, unless it's at it already (linked, its last word that it
 * started). */
export function startWork(book: Book, item: Item, session: string, { who, now }: Pick<Acting, "who" | "now">, { keep = false }: { keep?: boolean } = {}): void {
  const linked = item.sessions.some(each => each.session === session);
  linkSession(book, item, session, now);
  // (Work that starts on one not started yet makes it In progress, unless the same call said where
  // it stands, `keep`: one In review, or that needs you, stays as its agent last said.)
  if (!keep && STARTS_FROM.has(item.status)) {
    const from = item.status;
    item.status = "in-progress";
    item.order = placeIn(book.items().filter(each => each.id !== item.id), "in-progress");
    noteEvent(item, { at: now, by: who, kind: "moved", from, to: "in-progress" });
  }
  if (!linked || lastWord(item, session)?.kind !== "started") noteEvent(item, { at: now, by: who, kind: "started" });
}
const STARTS_FROM = new Set(["todo", "backlog", "waiting"]);

/** `session` no longer at work on `item`, as its agent said (`working: false`): unlinked, and kept
 * as having stopped, when its last word was that it started. */
export function stopWork(item: Item, session: string, { who, now }: Pick<Acting, "who" | "now">): void {
  unlinkSessions(item, [session], now, who);
}

/** Who moves an item the box itself moves (one whose status isn't known here goes to Todo). */
export const THE_BOX: Who = { source: "kuvo", name: "Kuvo Code" };

/** `item`, whose status `from` this box doesn't know (a newer box's), in Todo, said in its feed. */
export function intoTodo(item: Item, from: string, now: number): void {
  item.status = "todo";
  noteEvent(item, { at: now, by: THE_BOX, kind: "moved", from, to: "todo" });
}

/** A label taken off every item here (it's being removed): those deleted too. */
export function stripLabel(id: string): void {
  changeQueues(book => {
    for (const item of book.items()) if (item.labels.includes(id)) item.labels = item.labels.filter(each => each !== id);
  });
}

/** What an add or a change gives: the fields, as the tools name them. */
export interface Fields {
  title?: unknown;
  body?: unknown;
  status?: unknown;
  priority?: unknown;
  /** All its labels, in place of those it had. */
  labels?: unknown;
  /** Who it's for; null for no one. */
  assignee?: unknown;
  /** The day it's due (`YYYY-MM-DD`); null for none. */
  due?: unknown;
  top?: unknown;
  before?: unknown;
  after?: unknown;
  progress?: unknown;
  progress_note?: unknown;
  note?: unknown;
  links?: unknown;
  files?: unknown;
  /** Links (by address) and files (by name) to take away: one list for both. */
  remove?: unknown;
}

/** Who acts, when, the files it read for the change (`stageFiles`), and what's to be done on disk
 * once the change is sure (`applyFileOps`). */
export interface Acting { who: Who; now: number; staged?: Staged[]; fileOps: (() => void)[] }

/** What a change does on disk, done once nothing more can refuse it. */
export function applyFileOps(acting: Acting): void {
  for (const op of acting.fileOps.splice(0)) op();
}

// (An issue or a pull request closed on GitHub is Done, whatever status it's kept with.)
const openIn = (book: Book, owner: string) => book.queue(owner).items.filter(item => !item.removed && !isClosed(item.status) && !(item.source && doneByRead(seenOf(item.source)))).length;

/** A new item in `owner`'s queue: numbered now if there's a number to give, else once one comes;
 * an outside source's (`source`, an issue or a pull request) never. */
export function addItem(book: Book, owner: string, fields: Fields, acting: Acting, primary: Primary, source?: Source): Item {
  const title = plainName(text(fields.title ?? "", MOST_TITLE * 4, "The title")).slice(0, MOST_TITLE);
  if (!title) refuse("Say what it is (title), in a line.");
  if (openIn(book, owner) >= MOST_ITEMS) refuse(`Your queue has ${MOST_ITEMS} open items already: mark some done or remove them first.`);
  const status = fields.status === undefined ? "todo" : statusFor(fields.status);
  if (source && isClosed(status)) refuseDone(source.ref);
  const { now, who } = acting;
  const item: Item = {
    id: newItemID(),
    owner,
    title,
    body: "",
    status,
    order: placeIn(book.items(), status, { top: fields.top === true }),
    labels: [],
    notes: [],
    files: [],
    links: [],
    sessions: [],
    created: { at: now, by: who },
    updated: { at: now, by: who },
    ...(isClosed(status) ? { doneAt: now } : {}),
    ...(source ? { source } : {}),
  };
  book.queue(owner).items.push(item);
  // (Its details as a change makes them, so the same checks hold: where it's put isn't a move.)
  changeItem(book, item, { ...fields, title: undefined, status: undefined, top: undefined }, acting, { added: true });
  // (An outside source's item is named by its ref, never numbered.)
  if (item.source) return item;
  const number = takeNumber(primary);
  if (number !== null) item.number = number;
  return item;
}

// Done asked of an issue or a pull request: GitHub's to say.
const refuseDone = (name: string): never => refuse(`${name} is Done or Canceled once it's closed on GitHub (or, a pull request, merged): close it there, or let the pull request that fixes it close it.`);

/** An issue or a pull request GitHub has closed (or that's gone from it) stands as Done until it's
 * open there again: where it stands isn't set here meanwhile (nor, from a client, anything else of
 * it: `can.set`). */
export function refuseClosedThere(item: Item): void {
  if (item.source && doneByRead(seenOf(item.source))) refuse(seenOf(item.source)!.gone
    ? `${nameOf(item)} is gone from GitHub (or gh here can no longer see it), so it's Done: it's back where it stood once GitHub shows it again.`
    : `${nameOf(item)} is closed on GitHub, so it's Done: it's back where it stood once it's reopened there.`);
}

/** `item` changed as `fields` say. */
export function changeItem(book: Book, item: Item, fields: Fields, acting: Acting, { added = false }: { added?: boolean } = {}): void {
  const { who, now } = acting;
  // (Where it is among its status's items: a move that leaves it there isn't one.)
  const placeNow = () => ({ status: item.status, index: book.items().filter(each => each.id !== item.id && !each.removed && each.status === item.status && each.order < item.order).length });
  const was = placeNow();
  if (fields.title !== undefined) {
    const title = plainName(text(fields.title, MOST_TITLE * 4, "The title")).slice(0, MOST_TITLE);
    if (!title) refuse("A title can't be empty.");
    item.title = title;
  }
  if (fields.body !== undefined) item.body = text(fields.body, MOST_BODY, "The description (body)");
  if (fields.status !== undefined) {
    const status = statusFor(fields.status);
    // (An issue's or a pull request's Done is GitHub's: never set, nor left, here.)
    if (item.source && isClosed(status)) refuseDone(nameOf(item));
    refuseClosedThere(item);
    if (status !== item.status) {
      item.status = status;
      item.order = placeIn(book.items().filter(each => each.id !== item.id), status, { top: fields.top === true });
      if (isClosed(status)) {
        item.doneAt = now;
        item.sessions = [];
      } else {
        delete item.doneAt;
      }
    }
  }
  if (fields.priority !== undefined) {
    const priority = priorityFor(fields.priority);
    if (priority) item.priority = priority;
    else delete item.priority;
  }
  if (fields.labels !== undefined) item.labels = labelsFor(fields.labels);
  if (fields.assignee !== undefined) {
    const assignee = assigneeFor(fields.assignee);
    if (assignee) item.assignee = assignee;
    else delete item.assignee;
  }
  if (fields.due !== undefined) {
    if (fields.due === null || fields.due === "") delete item.due;
    else item.due = dayOf(fields.due) ?? refuse("due is a day, like 2026-11-30 (or null for none).");
  }
  const beside = (given: unknown) => {
    const ref = refOf(given);
    const all = ref ? matching(book.items(), ref).filter(each => !each.removed && each.id !== item.id) : [];
    // (Where two computers gave one number twice, the one of this item's status.)
    const found = all.find(each => each.status === item.status) ?? all[0];
    if (!found) refuse(`There's no other item ${String(given)} to put it beside.`);
    if (found!.status !== item.status) refuse(`${nameOf(found!)} is ${statusName(found!.status)}, not ${statusName(item.status)}: give it that status first (status).`);
    return found!;
  };
  if (fields.before !== undefined) item.order = placeBeside(book.items(), beside(fields.before), false, item);
  else if (fields.after !== undefined) item.order = placeBeside(book.items(), beside(fields.after), true, item);
  else if (fields.top === true && fields.status === undefined) item.order = placeIn(book.items().filter(each => each.id !== item.id), item.status, { top: true });
  const placed = placeNow();
  if (added) {
    // (An item just added isn't moved, wherever it's put.)
  } else if (placed.status !== was.status && placed.status === "done") noteEvent(item, { at: now, by: who, kind: "done" });
  else if (placed.status !== was.status || placed.index !== was.index) {
    const top = placed.index === 0 && (fields.top === true || fields.before !== undefined || fields.after !== undefined);
    noteEvent(item, { at: now, by: who, kind: "moved", from: was.status, to: placed.status, ...(top ? { top: true as const } : {}) });
  }
  if (fields.progress !== undefined) {
    if (fields.progress === null) {
      if (item.progress) delete item.progress.value;
    } else if (typeof fields.progress !== "number" || !(fields.progress >= 0 && fields.progress <= 1)) {
      refuse("progress is a number from 0 to 1 (3 of 8 is 0.375), or null to take it away.");
    } else {
      item.progress = { ...(item.progress ?? {}), value: fields.progress };
    }
  }
  if (fields.progress_note !== undefined) {
    const note = fields.progress_note === null ? "" : plainName(text(fields.progress_note, MOST_PROGRESS_NOTE * 2, "progress_note")).slice(0, MOST_PROGRESS_NOTE);
    item.progress = { ...(item.progress ?? {}) };
    if (note) item.progress.note = note;
    else delete item.progress.note;
  }
  if (item.progress && item.progress.value === undefined && item.progress.note === undefined) delete item.progress;
  if (fields.note !== undefined) {
    const note = text(fields.note, MOST_NOTE, "The note").trim();
    if (note) item.notes = [...item.notes, { at: now, by: who, text: note }].slice(-MOST_NOTES);
  }
  const listed = (value: unknown): string[] => (value === undefined ? [] : (Array.isArray(value) ? value : [value]).map(String));
  const removing = listed(fields.remove);
  if (removing.length) {
    const gone = new Set(removing);
    item.links = item.links.filter(link => !gone.has(link.url));
  }
  if (fields.links !== undefined) {
    for (const link of linksOf(fields.links)) if (!item.links.some(each => each.url === link.url)) item.links.push(link);
    if (item.links.length > MOST_LINKS) refuse(`An item keeps ${MOST_LINKS} links at most.`);
  }
  // Files: everything checked first; what's on disk changes only as the change is kept
  // (`applyFileOps`), those taken away before those added.
  const gone = new Set(removing);
  const staged = fields.files === undefined ? [] : acting.staged ?? refuse("Only an agent's own files are kept with an item.");
  const left = item.files.filter(file => !gone.has(file.name));
  if (left.length + staged.length > MOST_FILES) refuse(`An item keeps ${MOST_FILES} files at most: take some away first (remove).`);
  for (const file of item.files) if (gone.has(file.name)) acting.fileOps.push(() => fs.rmSync(path.join(filesOf(item.id), file.name), { force: true }));
  item.files = left;
  const anchors = new Map<number, string>();
  for (const each of staged) {
    const base = shortName(each.name.replace(/[\\\0]/g, "_").replace(/^\.+/, "")) || "file";
    const ext = path.extname(base);
    let name = base;
    for (let n = 2; item.files.some(file => file.name === name); n++) name = `${base.slice(0, base.length - ext.length)} ${n}${ext}`;
    const anchor = each.clientIndex === undefined ? undefined : crypto.randomBytes(12).toString("hex");
    if (anchor && each.clientIndex !== undefined) anchors.set(each.clientIndex, anchor);
    item.files.push({ name, size: each.size, type: typeOf(name), at: now, ...(anchor ? { anchor } : {}) });
    acting.fileOps.push(() => {
      fs.mkdirSync(filesOf(item.id), { recursive: true, mode: 0o700 });
      fs.renameSync(each.staged, path.join(filesOf(item.id), name));
    });
  }
  if (fields.body !== undefined && anchors.size) {
    item.body = text(item.body.replace(/!\[Attachment\]\(kuvo-attachment:draft\/(\d+)\/[^\s)]*\)/g, (marker, index) => {
      const anchor = anchors.get(Number(index));
      const file = anchor && item.files.find(file => file.anchor === anchor);
      if (!file) return marker;
      const label = file.name.replace(/[\\[\]]/g, char => "\\" + char).replace(/[\r\n]/g, " ");
      return `![${label}](kuvo-attachment:file/${anchor})`;
    }), MOST_BODY, "The description (body)");
  }
  item.updated = { at: now, by: who };
}

/** `item` made the outside source's item `source` is (an issue or a pull request): named by it from
 * now on (its number kept, never shown again, so it still finds it), with its title and
 * description; what the item said before is kept as a note. */
export function linkSource(item: Item, source: Source, title: string, body: string, { who, now }: Acting): void {
  if (item.source) refuse(`${nameOf(item)} is ${item.source.ref} already: one item is one issue or pull request.`);
  if (isClosed(item.status)) refuse(`${nameOf(item)} is ${statusName(item.status)}: give it an open status first (status), or add the issue as an item of its own (queue_add with issue).`);
  const before = item.body.trim();
  item.source = source;
  item.title = plainName(title).slice(0, MOST_TITLE) || item.title;
  if (body.trim()) {
    item.body = body.slice(0, MOST_BODY);
    if (before) item.notes = [...item.notes, { at: now, by: who, text: `What it said before it was ${source.ref}:\n\n${before}`.slice(0, MOST_NOTE) }].slice(-MOST_NOTES);
  }
  item.updated = { at: now, by: who };
}

/** `item` deleted: kept 30 days for an undo, no session linked any more. */
export function removeItem(item: Item, who: Who, now: number): void {
  item.removed = { at: now, by: who };
  item.sessions = [];
  item.updated = { at: now, by: who };
}

/** `item` brought back, at the end of its status's items (Todo, for a status this box doesn't know). */
export function restoreItem(book: Book, item: Item, who: Who, now: number): void {
  delete item.removed;
  if (!statusNamed(item.status)) intoTodo(item, item.status, now);
  item.order = placeIn(book.items(), item.status);
  item.updated = { at: now, by: who };
}

/** `item` handed to the conversation `to`: into its queue, linked to it. */
export function handItem(book: Book, item: Item, to: string, who: Who, now: number): void {
  const from = item.owner;
  if (from === to) refuse(`${nameOf(item)} is in that queue already.`);
  if (openIn(book, to) >= MOST_ITEMS) refuse(`That queue has ${MOST_ITEMS} open items already.`);
  book.give(item, to);
  noteHanded(item, from, to, now);
  if (to.startsWith("ses_") && !isClosed(item.status)) linkSession(book, item, to, now);
  item.updated = { at: now, by: who };
}
