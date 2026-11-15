// Bots' own keys, gone (#726; PROTOCOL.md, "Bots have no keys of their own"). A bot once could have
// a key of its own for an integration you bring a key for, used only in its conversation: the key
// kept where its box keeps keys (`<variable>__BOT_<ID>`), and a server for the integration, naming
// it, in its folder's settings (`workspace/.opencode/opencode.json`). Now a bot uses the
// integrations of the box it lives on, as every session there does.
//
// So a box takes what's left of them away, as it starts and as an update is confirmed, with no
// client there: each bot's folder's settings lose the servers that name such a key (its
// conversation then gets this box's own server for that integration, your connection), and the
// keys go from where they were kept (`dropBotKeys`). An integration a bot had its own key for is
// noted in its folder (`keys-gone.json`), and while this box hasn't that integration connected,
// the bot is told so, to say it plainly when its work needs it (`keysGoneNote`, with what it's
// told: tell.ts).

import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import type { EngineRecord } from "../../shared/records";
import { soon } from "../../shared/wait";
import { ownsSettings, readSettings, SERVERS, valueAt } from "../areas/config";
import { integrationCatalog } from "../areas/integrations";
import { listOf, textOf, writePrivate } from "../files";
import { BOTS, HERE, K, OWN_HOME_MAC, RUN } from "../home";
import { KeysUnavailable } from "../host-interface";
import { dropBotKeys, keyFilesWait, keyWork } from "../keys";
import { readRecord } from "../rehome/state";
import { updateOnTrial } from "../updates/update";

/** The integrations a bot had a key of its own for, noted in its folder (`{"integrations": [ids]}`)
 * until each is connected on the box it lives on. It goes with the bot to another computer. */
const GONE_FILE = "keys-gone.json";

/** A server an earlier box wrote in a bot's folder's settings for its own keys: it names one. */
const namesBotKey = (server: unknown): boolean => /KUVO_KEY_[A-Z0-9_]+__BOT_[A-Z0-9]+/.test(JSON.stringify(server) ?? "");

/** A settings file's text as the engine reads it (JSONC: comments, and a comma after the last of a
 * list): its value. Throws for what isn't that either. */
export function settingsValue(text: string): unknown {
  try { return JSON.parse(text); } catch {}
  let plain = "";
  for (let i = 0; i < text.length; i++) {
    const char = text[i]!;
    if (char === '"') {
      // A string, whole, with what it escapes.
      let end = i + 1;
      while (end < text.length && text[end] !== '"') end += text[end] === "\\" ? 2 : 1;
      plain += text.slice(i, end + 1);
      i = end;
    } else if (char === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i++;
      plain += "\n";
    } else if (char === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i + 2);
      if (end < 0) throw new SyntaxError("A comment isn't closed.");
      i = end + 1;
      plain += " ";
    } else if (char === ",") {
      // (One before what closes its list goes: looked for past the space and comments after it.)
      const rest = text.slice(i + 1).replace(/^(\s|\/\/[^\n]*(\n|$)|\/\*[\s\S]*?\*\/)*/, "");
      if (!/^[\]}]/.test(rest)) plain += char;
    } else {
      plain += char;
    }
  }
  return JSON.parse(plain);
}

const settingsFileOf = (folder: string): string => path.join(folder, "workspace", ".opencode", "opencode.json");

/** The keys of its own a bot's folder's settings name while they can't be read at all (not JSON,
 * nor as the engine reads them): its settings can't be put right, so those keys stay, and the bot
 * is told (`keysGoneNote`). None otherwise. */
export function keysNamedUnread(folder: string): string[] {
  const text = textOf(settingsFileOf(folder));
  if (!text?.trim()) return [];
  try { settingsValue(text); return []; } catch {}
  return [...new Set(text.match(/KUVO_KEY_[A-Z0-9_]+__BOT_[A-Z0-9]+/g) ?? [])];
}

/** The integrations a bot (its folder) had a key of its own for, as noted there. */
function keysGoneOf(folder: string): string[] {
  try {
    const noted: unknown = JSON.parse(textOf(path.join(folder, GONE_FILE)) ?? "");
    const ids = isObject(noted) && Array.isArray(noted.integrations) ? noted.integrations : [];
    return [...new Set(ids.filter((id): id is string => typeof id === "string" && /^[A-Za-z0-9._-]{1,100}$/.test(id)))];
  } catch {
    return [];
  }
}

function noteKeysGone(folder: string, ids: string[]): void {
  const file = path.join(folder, GONE_FILE);
  if (!ids.length) return fs.rmSync(file, { force: true });
  writePrivate(file, JSON.stringify({ integrations: ids }));
}

/** A bot's folder's settings without the servers that name a key of its own, the rest of the file
 * as it was (what you wrote there by hand stays; a file left empty goes): the integrations they
 * were for, which are noted in its folder. Read as the engine reads it (comments, a last comma),
 * and written plain where a server goes (your comments there go with it); settings that can't be
 * read at all are left alone, with the keys they name (`keysNamedUnread`). Throws where the file
 * can't be written. */
export function dropOwnServers(folder: string): string[] {
  const file = settingsFileOf(folder);
  const settingsFolder = path.dirname(file);
  let settings: unknown;
  const text = textOf(file);
  try {
    settings = settingsValue(text ?? "");
  } catch {
    if (text && namesBotKey(text) && !saidNotPlain.has(file) && saidNotPlain.add(file)) console.error(`kuvo-link: ${file} can't be read as settings and names a key a bot had of its own: that key stays until the file is put right.`);
    return [];
  }
  const servers = isObject(settings) && isObject(settings.mcp) && isObject(settings.mcp.servers) ? settings.mcp.servers : null;
  if (!isObject(settings) || !servers) return [];
  const gone = Object.keys(servers).filter(id => namesBotKey(servers[id]));
  if (!gone.length) return [];
  const kept = Object.fromEntries(Object.entries(servers).filter(([id]) => !gone.includes(id)));
  const mcp: Record<string, unknown> = { ...(settings.mcp as Record<string, unknown>) };
  if (Object.keys(kept).length) mcp.servers = kept;
  else delete mcp.servers;
  const next: Record<string, unknown> = { ...settings };
  if (Object.keys(mcp).length) next.mcp = mcp;
  else delete next.mcp;
  // Noted before its settings change: a box stopped between the two notes it again the next time
  // (its servers are still there), never loses it.
  noteKeysGone(folder, [...new Set([...keysGoneOf(folder), ...gone])]);
  if (Object.keys(next).length) {
    writePrivate(file, JSON.stringify(next, null, 2) + "\n");
  } else {
    fs.rmSync(file, { force: true });
    try { fs.rmdirSync(settingsFolder); } catch {}
  }
  return gone;
}

/** Of the integrations noted for a bot, those this box hasn't connected now; null while that can't
 * be told (its settings unreadable, or not its own yet), so nothing false is said or forgotten. */
function stillToConnect(folder: string): { noted: string[]; left: string[] } | null {
  const noted = keysGoneOf(folder);
  if (!noted.length) return { noted, left: [] };
  const { settings } = readSettings();
  if (!settings || !ownsSettings()) return null;
  return { noted, left: noted.filter(id => valueAt(settings, [...SERVERS, id]) === undefined) };
}

/** Of the integrations a bot had a key of its own for, those this box hasn't connected now, by
 * their names. Only integrations this box's catalog has: its note is a file in its own folder,
 * which nothing else may put words in what it's told through. */
function notConnected(folder: string): string[] {
  const left = stillToConnect(folder)?.left ?? [];
  if (!left.length) return [];
  const catalog = integrationCatalog();
  return left.flatMap(id => { const title = catalog.find(entry => entry.id === id)?.title; return typeof title === "string" && title ? [title] : []; });
}

/** Its note without the integrations this box has connected since (the bot uses them, as any
 * session does, and nothing more is said of them). As its run starts. */
export function forgetConnectedKeys(folder: string): void {
  const now = stillToConnect(folder);
  if (now && now.left.length !== now.noted.length) try { noteKeysGone(folder, now.left); } catch {}
}

const listed = (names: string[]): string => names.length < 3 ? names.join(" and ") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;

/** What a bot is told, after the rest, while an integration it had a key of its own for isn't
 * connected on this box: that it's so, and to say it plainly when its work needs it. Nothing
 * otherwise. */
export function keysGoneNote(folder: string): string {
  return notConnectedNote(folder) + unreadNote(folder);
}

/** While its folder's settings can't be read and name a key of its own (which stays for that). */
function unreadNote(folder: string): string {
  if (!keysNamedUnread(folder).length) return "";
  return `\n\nYour folder's settings file (${settingsFileOf(folder)}) can't be read: it isn't valid JSON. It names a key of your own for one of its servers, and Kuvo Code no longer keeps keys for single bots, so that server and its key are to go, which Kuvo Code can't do while the file can't be read. Tell the user so plainly the next time you talk: once the file is put right, or that server is taken out of it, the key is removed and you use the integrations connected on ${HERE}, as every session there does.`;
}

function notConnectedNote(folder: string): string {
  const names = notConnected(folder);
  if (!names.length) return "";
  const one = names.length === 1;
  return `\n\nYou used to have a key of your own for ${listed(names)}. Kuvo Code no longer keeps keys for single bots: that key was removed, and you now use the integrations connected on ${HERE}, as every session there does. ${listed(names)} ${one ? "isn't" : "aren't"} connected there, so you have no tools for ${one ? "it" : "them"} now. When your work needs ${one ? "it" : "one of them"}, don't look for another way around: say plainly that it isn't connected on ${HERE}, and ask the user to connect it in Settings › Integrations.`;
}

/** Its server connected again in a bot's folder a moment later, as its settings are now. */
function reconnectIn(engine: EngineRecord, folder: string, id: string): void {
  soon(() => fetch(`http://127.0.0.1:${engine.port}/api/experimental/mcp/${encodeURIComponent(id)}/connect`, {
    method: "POST",
    headers: { authorization: `Basic ${Buffer.from(`opencode:${engine.password}`).toString("base64")}`, "x-opencode-directory": encodeURIComponent(path.join(folder, "workspace")) },
  }).catch(() => {}), 2_000);
}

/** Every bot's folder's settings here without the servers that name a key of its own (not while an
 * update is on trial). By itself as this Mac's box starts, before it says where it is, so before
 * its engine starts: its keys, which its app may not answer for yet, go after. */
export function dropBotsOwnServers(engine: () => EngineRecord | null = () => null): { all: boolean; keep: string[] } {
  if (updateOnTrial()) return { all: false, keep: [] };
  let all = true;
  const keep: string[] = [];
  for (const bot of listOf(BOTS)) {
    const folder = path.join(BOTS, bot);
    let gone: string[] = [];
    try {
      gone = dropOwnServers(folder);
    } catch (error) {
      // (Its settings still name its keys: none goes until they're put right, tried again.)
      all = false;
      console.error(`kuvo-link: ${bot}'s folder's settings couldn't be put right: ${String((error as Error | null)?.message ?? error)}`);
    }
    keep.push(...keysNamedUnread(folder));
    if (!gone.length) continue;
    // (A server already running there has its key in memory: connected again, it's the box's own.)
    let running: EngineRecord | null = null;
    try { running = engine(); } catch {}
    if (running?.port) for (const id of gone) reconnectIn(running, folder, id);
  }
  return { all, keep };
}

/** Whether this Mac's box has just moved into a home of its own and its app's program still owes
 * the Keychain: it deletes its originals only once it finds each one's copy beside it (the box's
 * helper's), so no copy goes before that, or the move would never settle. */
function keychainOwed(): boolean {
  if (!OWN_HOME_MAC) return false;
  const record = readRecord(K);
  // (Only after a move here: one moved back leaves the same words in a record nothing settles.)
  return !record.movedBack && Boolean(record.owed?.includes("keychain"));
}

/** What's left here of bots' own keys, gone: every bot's folder's settings first (so none names a
 * key that's gone: where one couldn't be written, no key goes yet, and one that can't be read
 * keeps the keys it names), then the keys, then the note of deleted bots' keys still to go
 * (`run/bot-keys-left.json`, an earlier box's). Whether it's all done: not while an update is on
 * trial (a key can't come back if the version before does: the box does this once the update is
 * confirmed), nor while this Mac's keys can't be reached or its move into a home of its own still
 * owes the Keychain (tried again: `keepBotsOwnKeysGone`).
 * Nothing to do, and nothing written, once it's done. `engine`: this box's, when it runs, for a
 * bot's conversation that's open there. */
export function dropBotsOwnKeys(engine: () => EngineRecord | null = () => null): Promise<boolean> {
  return keyWork(async () => {
    if (updateOnTrial()) return false;
    const { all, keep } = dropBotsOwnServers(engine);
    if (!all) return false;
    if (keychainOwed()) {
      if (!saidOwed) console.error("kuvo-link: bots' own keys wait in this Mac's Keychain until Kuvo Code has finished moving its keys to its box: tried again later.");
      saidOwed = true;
      return false;
    }
    try {
      await dropBotKeys(new Set(keep));
    } catch (error) {
      if (!(error instanceof KeysUnavailable)) throw error;
      return false;
    }
    fs.rmSync(path.join(RUN, "bot-keys-left.json"), { force: true });
    return true;
  });
}

// Said once while this box runs, each.
let saidOwed = false;
const saidNotPlain = new Set<string>();

let again: ReturnType<typeof setTimeout> | null = null;
/** The same while this box runs (`serve`): now, and again while this Mac's keys can't be reached (a
 * minute after the first time, twice as long after each one more, twice a day at most). */
export function keepBotsOwnKeysGone(engine: () => EngineRecord | null = () => null, retry = 60_000): void {
  if (again) return;
  let failed = 0;
  const round = async () => {
    again = null;
    if (await dropBotsOwnKeys(engine).catch(() => false) || updateOnTrial()) return;
    failed++;
    again = setTimeout(() => void round(), keyFilesWait(failed, undefined, retry));
    again.unref?.();
  };
  again = setTimeout(() => void round(), 0);
  again.unref?.();
}
