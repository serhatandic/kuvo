// What an earlier Kuvo Code kept in its own files and preferences, brought into the box's own
// files (#713 piece 4). Kuvo Code 0.5 to
// 0.7.2 and 0.8.0-beta.1 had no box: their app kept your projects, machines, worktrees' records,
// group chats, personalities and settings itself, and a later app gave each to its box as it
// opened. No such app runs any more, so the box's move does it: each upgrade reads the app's
// folder and its preferences, never writing there, and writes into the staged copy alone, before
// its paths are rewritten (so what it writes names the old place, as everything cloned does).
//
// Each can run again over what it wrote (a move tried again clones afresh; a box that had some of
// it already, a keeper build's, keeps its own). What isn't carried is said, in words a client
// shows once (`state.json`'s `dropped`).
// Platform: a Mac's alone (the preferences are a property list `plutil` or `defaults` reads).
// COMPAT(0.7 move): brings what Kuvo Code 0.5 to 0.7.2 and 0.8.0-beta.1 kept in the app's own files and preferences into the box's files, as the box moves. Goes on the user's word, once no 0.7 Mac is expected to update.

import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { gitPreferencesFormat } from "../areas/git-words";
import { projectsVersion, readProjectFields } from "../areas/projects";
import { readWorktreeFields, worktreeRecordsFormat } from "../areas/worktree-records";
import { UsageSettings } from "../../usage/settings";
import { syncDurableFile } from "../../shared/files";
import { additiveListFormat, additiveObjectFormat } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { accountHome, homeFolder, located } from "../home";
import { mergePersonalities, personalityLibraryVersion, readPersonalities } from "../personalities/library";
import { DEFAULT_RELAY } from "../mac/link";
import { welcomeFormat } from "../setup/welcomed";
import { placementOf, type MoveContext } from "./rewrite";
import { Said } from "./said";
import type { RunPlan } from "./state";
import { starredFromKeys } from "../areas/models-starred";

type Prefs = Record<string, unknown>;

const exists = (file: string): boolean => { try { fs.lstatSync(file); return true; } catch { return false; } };
const isFolder = (file: string): boolean => { try { return fs.statSync(file).isDirectory(); } catch { return false; } };
const listOf = (folder: string): string[] => { try { return fs.readdirSync(folder); } catch { return []; } };
const legacyImportObject = additiveObjectFormat("Legacy Mac import object");
const legacyImportList = additiveListFormat("Legacy Mac import list");

const readLegacy = (file: string): unknown => {
  const value = readJSON(file);
  if (Array.isArray(value)) return legacyImportList.read(value);
  if (isObject(value)) return legacyImportObject.read(value);
  return value;
};

/** Validate retired Kuvo JSON before the move asks any writer to stop. */
export function checkLegacyImport(folder: string): void {
  for (const name of ["bots.json", "groups.json", "personalities.json", "personalities-removed.json",
    "agent-messages.json", "agent-letters.json", "agent-letters-seen.json", "phone-link.json"]) {
    readLegacy(path.join(folder, name));
  }
  for (const bot of listOf(path.join(folder, "bots"))) readLegacy(path.join(folder, "bots", bot, "bot.json"));
  for (const chat of listOf(path.join(folder, "groups"))) {
    if (chat.endsWith(".json")) readLegacy(path.join(folder, "groups", chat));
  }
}

const readJSON = (file: string): unknown => { try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return undefined; } };
const strings = (value: unknown): string[] => (Array.isArray(value) ? value.filter((each): each is string => typeof each === "string") : []);
const stringMap = (value: unknown): Record<string, string> => (isObject(value) ? Object.fromEntries(Object.entries(value).filter((each): each is [string, string] => typeof each[1] === "string")) : {});
function replaceStaged(file: string, bytes: string | Buffer, mode = 0o600): void {
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = `${file}.${process.pid}.new`;
  const fd = fs.openSync(temporary, "w", mode);
  try {
    fs.writeFileSync(fd, bytes);
    fs.fchmodSync(fd, mode);
    fs.fsyncSync(fd);
  } finally { fs.closeSync(fd); }
  fs.renameSync(temporary, file);
}

// The app's preferences

/** A property list in XML, read: dictionaries, arrays, strings, numbers, booleans, and data
 * (`<data>`: what the app kept as JSON is read as JSON, else left as its bytes' text). */
export function parsePlist(xml: string): unknown {
  const unescape = (text: string) => text.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, "\"").replace(/&apos;/g, "'").replace(/&amp;/g, "&");
  const tokens = /<(\/?)([a-z]+)(\/?)>([^<]*)/g;
  const body = xml.slice(Math.max(0, xml.indexOf("<plist")));
  tokens.lastIndex = body.indexOf(">") + 1;
  const next = () => {
    const found = tokens.exec(body);
    return found ? { closes: found[1] === "/", name: found[2], empty: found[3] === "/", text: found[4] } : null;
  };
  const value = (token: NonNullable<ReturnType<typeof next>>): unknown => {
    if (token.name === "true") return true;
    if (token.name === "false") return false;
    if (token.name === "dict") {
      const dict: Record<string, unknown> = {};
      if (token.empty) return dict;
      for (let each = next(); each && !(each.closes && each.name === "dict"); each = next()) {
        if (each.name !== "key") continue;
        const key = unescape(each.text);
        next();
        const inner = next();
        if (inner) dict[key] = value(inner);
      }
      return dict;
    }
    if (token.name === "array") {
      const list: unknown[] = [];
      if (token.empty) return list;
      for (let each = next(); each && !(each.closes && each.name === "array"); each = next()) list.push(value(each));
      return list;
    }
    const text = token.empty ? "" : token.text;
    if (!token.empty) next();
    if (token.name === "integer" || token.name === "real") return Number(text);
    if (token.name === "data") {
      const bytes = Buffer.from(text.replace(/\s+/g, ""), "base64").toString("utf8");
      try { return JSON.parse(bytes); } catch { return bytes; }
    }
    return unescape(text);
  };
  const first = next();
  return first ? value(first) : {};
}

/** The earlier Kuvo Code's preferences (`~/Library/Preferences/<its ID>.plist`): asked of the
 * system for the real one (what it has may be newer than the file), read from the file for a
 * copy's. Nothing where there are none. */
export function appPreferences(appFolder: string): Prefs {
  const id = path.basename(appFolder);
  const file = path.join(path.dirname(path.dirname(appFolder)), "Preferences", `${id}.plist`);
  const real = located(homeFolder()) === located(accountHome()) && id === "run.kuvo.code";
  // (None kept at all is none, said so by the system or by there being no file: never an error.)
  if (!real && !exists(file)) return {};
  const asked = real
    ? spawnSync("/usr/bin/defaults", ["export", id, "-"], { encoding: "utf8", timeout: 60_000, maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "ignore"] })
    : spawnSync("/usr/bin/plutil", ["-convert", "xml1", "-o", "-", file], { encoding: "utf8", timeout: 60_000, maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "ignore"] });
  // Ones that are there and can't be read stop the move (it goes back, to be tried again): taken
  // for none, your projects, machines and settings would be left behind for good, unsaid.
  const read = asked.status === 0 && asked.stdout ? parsePlist(asked.stdout) : null;
  if (!isObject(read)) throw new Said("Kuvo couldn't read the earlier Kuvo Code's settings, so nothing was moved. Try again.");
  return read;
}

// One upgrade's world

interface Upgrade {
  writeWhole: (file: string, bytes: string | Buffer, mode?: number) => void;
  seal: (entry: string) => void;
  /** The earlier Kuvo Code's folder (read, never written), and where the box will be. */
  S: string;
  home: string;
  prefs: Prefs;
  /** Where a place in the box's home (`box/projects.json`, `groups`) is staged, its folders made. */
  staged(name: string): string;
  /** What wasn't carried, in words a client shows once. */
  dropped: string[];
}

/** A folder as the app named one of this Mac's, or null for a machine's (`ssh://…`, that machine's
 * own) and what isn't a path. */
const here = (folder: unknown): string | null => (typeof folder === "string" && path.isAbsolute(folder) ? folder : null);

// Projects, worktrees, machines

/** The projects you opened (`projectFolders`, in the sidebar's order, and `projectSettings`: each
 * one's name, extra folders and Run command) into the box's list (`box/projects.json`). One the
 * box has already keeps its own. Bots' folders were never projects. */
function projects({ writeWhole, prefs, staged, S }: Upgrade): void {
  const order = strings(prefs.projectFolders);
  const settings = isObject(prefs.projectSettings) ? prefs.projectSettings : {};
  const bots = (folder: string) => folder.startsWith(path.join(S, "bots") + "/") || folder.includes("/.kuvo-code/bots/");
  const folders = [...new Set([...order, ...Object.keys(settings)])].filter(folder => here(folder) && !bots(folder));
  if (!folders.length) return;
  const file = staged("box/projects.json");
  const raw = readJSON(file);
  const kept = isObject(raw) ? readProjectFields(raw) : {};
  const list: Record<string, unknown>[] = Array.isArray(kept.projects) ? kept.projects.filter(isObject) : [];
  const known = new Set(list.map(each => each.directory));
  for (const directory of folders) {
    if (known.has(directory)) continue;
    const given = isObject(settings[directory]) ? settings[directory] : {};
    const name = typeof given.name === "string" ? given.name.trim() : "";
    const run = typeof given.run === "string" ? given.run.trim() : "";
    list.push({
      directory,
      ...(name && name !== path.basename(directory) ? { name } : {}),
      folders: [...new Set(strings(given.folders).filter(folder => path.isAbsolute(folder) && folder !== directory))],
      ...(run ? { run } : {}),
      // (One whose settings were kept for a session's sake, not in the sidebar: removed, as it was.)
      ...(order.includes(directory) ? {} : { removed: true }),
    });
  }
  writeWhole(file, JSON.stringify({ version: projectsVersion, projects: list }, null, 2));
}

/** The worktrees the app kept for its sessions (`worktrees`) and those it removed
 * (`removedWorktrees`, which say where an archived session belongs) into the box's records
 * (`box/worktrees.json`). One whose folder is gone isn't a worktree any more. */
function worktrees({ writeWhole, prefs, staged }: Upgrade): void {
  const kept = isObject(prefs.worktrees) ? prefs.worktrees : {};
  const removed = stringMap(prefs.removedWorktrees);
  if (!Object.keys(kept).length && !Object.keys(removed).length) return;
  const file = staged("box/worktrees.json");
  const had = readWorktreeFields(file);
  const records: Record<string, unknown>[] = Array.isArray(had.worktrees) ? had.worktrees.filter(isObject) : [];
  const handedOver = stringMap(had.handedOver);
  const gone = stringMap(had.removed);
  for (const [folder, record] of Object.entries(kept).sort(([a], [b]) => a.localeCompare(b))) {
    if (!isObject(record)) continue;
    const project = here(record.project), root = here(record.root);
    if (!here(folder) || !project || !root || typeof record.branch !== "string" || !record.branch || !isFolder(root)) continue;
    if (records.some(each => each.root === root)) continue;
    records.push({ project, root, folder, branch: record.branch, ...(typeof record.base === "string" && record.base ? { base: record.base } : {}) });
  }
  for (const [folder, project] of Object.entries(removed).sort(([a], [b]) => a.localeCompare(b))) {
    if (!here(folder) || !here(project) || folder === project) continue;
    if (records.some(each => each.root === folder || each.folder === folder) || folder in gone || folder in handedOver) continue;
    handedOver[folder] = project;
  }
  writeWhole(file, JSON.stringify(worktreeRecordsFormat.write({
    worktrees: records, removed: gone, restorable: Array.isArray(had.restorable) ? had.restorable : [],
    ...(Object.keys(handedOver).length ? { handedOver } : {}),
  }), null, 2));
}

/** Your machines (`machines`: each one's address for `ssh`, its other addresses, its key file)
 * into the box's list (`machines.json`), beside those it has. */
function machines({ writeWhole, prefs, staged }: Upgrade): void {
  const listed = [...(Array.isArray(prefs["machines.unsaved"]) ? prefs["machines.unsaved"] : []), ...(Array.isArray(prefs.machines) ? prefs.machines : [])]
    .filter((each): each is Record<string, unknown> => isObject(each) && typeof each.destination === "string" && each.destination.length > 0);
  const file = staged("machines.json");
  // (With none, an empty list where there's no list yet: no list, beside what's kept of machines
  // it had, `phone-link.json`, would read as one that can't be told. `mac/machines.ts`)
  if (!listed.length) return fs.existsSync(file) ? undefined : writeWhole(file, "[]");
  const had = readJSON(file);
  const list: Record<string, unknown>[] = Array.isArray(had) ? had.filter(isObject) : [];
  for (const machine of listed) {
    if (list.some(each => each.destination === machine.destination)) continue;
    list.push({
      destination: machine.destination,
      ...(strings(machine.otherAddresses).length ? { otherAddresses: strings(machine.otherAddresses) } : {}),
      ...(typeof machine.identity === "string" && machine.identity ? { identity: machine.identity } : {}),
      ...(typeof machine.reachedAt === "string" && machine.reachedAt ? { reachedAt: machine.reachedAt } : {}),
    });
  }
  writeWhole(file, JSON.stringify(list));
}

// Settings

/** The engine's settings in the staged copy, as plain JSON, or null where they aren't (comments, a
 * damaged file): then nothing's written into them. */
function settingsOf(file: string): Record<string, unknown> | null {
  let text = "";
  try { text = fs.readFileSync(file, "utf8"); } catch { return {}; }
  if (!text.trim()) return {};
  const read = readJSON(file);
  return isObject(read) ? read : null;
}

/** A key named where the app wrote its files for the engine (its temporary folder), or in the
 * engine's environment as a version before did, named where this box writes them (`box/keys`, as
 * `areas/config.ts` names them). */
function keysNamedIn(value: unknown, keys: string): unknown {
  if (typeof value === "string") return value.replace(/\{(?:env|file):[^{}]*?(KUVO_KEY_[A-Za-z0-9_]+)\}/g, (_, name) => `{file:${keys}/${name}}`);
  if (Array.isArray(value)) return value.map(each => keysNamedIn(each, keys));
  if (isObject(value)) return Object.fromEntries(Object.entries(value).map(([key, inner]) => [key, keysNamedIn(inner, keys)]));
  return value;
}

/** The engine's settings: integrations' keys named where the box writes them now, and Settings ›
 * Git from the app's preferences (the branch prefix, and what commits and pull requests are told),
 * each only where the settings have none, once (`box/git.json` says it's done). */
function engineSettings({ writeWhole, prefs, staged, home, dropped }: Upgrade): void {
  const file = staged("config/opencode/opencode.json");
  const settings = settingsOf(file);
  const account = homeFolder();
  const shown = home.startsWith(account + "/") ? "~" + home.slice(account.length) : home;
  const git = { branch_prefix: prefs["git.branchPrefix"], commit_instructions: prefs["git.commitInstructions"], pull_request_instructions: prefs["git.pullRequestInstructions"] };
  const offered = Object.entries(git).filter((each): each is [string, string] => typeof each[1] === "string" && (each[0] === "branch_prefix" || each[1].trim() !== ""));
  if (settings === null) {
    if (offered.length) dropped.push("Your Git settings (the branch prefix, and what commits and pull requests are told) weren't brought along, because the engine's settings file couldn't be read. Set them again in Settings › Git.");
    let text = "";
    try { text = fs.readFileSync(file, "utf8"); } catch {}
    if (text.includes("KUVO_KEY_")) dropped.push("Your integrations' keys are safe, but the engine's settings file couldn't be read to point at them: add each integration's key again in Settings › Integrations.");
    return;
  }
  let next = keysNamedIn(settings, `${shown}/box/keys`) as Record<string, unknown>;
  const gitFile = staged("box/git.json");
  const found = readJSON(gitFile);
  const kept = isObject(found) ? gitPreferencesFormat.read(found) : {};
  if (offered.length && kept.moved !== true) {
    for (const [key, value] of offered) if (next[key] === undefined) next = { ...next, [key]: key === "branch_prefix" ? value : value.trim() };
    writeWhole(gitFile, JSON.stringify(gitPreferencesFormat.write({ ...kept, moved: true })));
  }
  if (JSON.stringify(next) !== JSON.stringify(settings)) writeWhole(file, JSON.stringify(next, null, 2) + "\n");
  // A bot's own settings name its keys the same way.
  for (const bot of listOf(staged("bots"))) {
    const own = path.join(staged("bots"), bot, "workspace", ".opencode", "opencode.json");
    const had = exists(own) ? readJSON(own) : undefined;
    if (!isObject(had)) continue;
    const renamed = keysNamedIn(had, `${shown}/box/keys`);
    if (JSON.stringify(renamed) !== JSON.stringify(had)) writeWhole(own, JSON.stringify(renamed, null, 2) + "\n");
  }
}

/** The box's own settings the app kept: Keep This Mac Awake where it was off (`box/work.json`),
 * Ask First where new sessions started with it (`box/approval.json`), and what the app added to
 * the engine's settings once, so that what you took out stays out (`box/defaults-added`). */
function boxSettings({ writeWhole, prefs, staged }: Upgrade): void {
  const merged = (name: string, change: Record<string, unknown>) => {
    const file = staged(name);
    const had = readJSON(file);
    writeWhole(file, JSON.stringify({ ...(isObject(had) ? had : {}), ...change }, null, 2));
  };
  if (prefs.keepsAwake === false) merged("box/work.json", { keepsAwake: false });
  // Your starred models, shared now by every client's picker (`box/models.json`), unless this box
  // already has stars of its own.
  const starred = typeof prefs.favoriteModels === "string" ? starredFromKeys(prefs.favoriteModels) : [];
  if (starred.length) {
    const had = readJSON(staged("box/models.json"));
    if (!(isObject(had) && Array.isArray(had.starred) && had.starred.length)) merged("box/models.json", { starred });
  }
  if (prefs.toolApproval === "askFirst") {
    const had = readJSON(staged("box/approval.json"));
    if (!(isObject(had) && typeof had.last === "string")) merged("box/approval.json", { last: "askFirst" });
  }
  const marks = [...(prefs["engine.defaultsAdded"] === true ? ["websearch"] : []), ...(prefs["engine.searchMovedToExa"] === true ? ["search-exa"] : []), ...(prefs["engine.botMemoryRuleAdded"] === true ? ["bot-memory"] : [])];
  if (marks.length) {
    const file = staged("box/defaults-added");
    let done: string[] = [];
    try { done = fs.readFileSync(file, "utf8").split("\n").filter(Boolean); } catch {}
    writeWhole(file, [...new Set([...done, ...marks])].join("\n") + "\n");
  }
}

/** "Let my devices connect" as this Mac's link had it set (`phoneLink`, and a relay of your own,
 * `phoneLink.relay`), kept as the box keeps what its app said (`run/mac-link.json`). Never set, it
 * isn't written: the box's own start is on. A copy's home takes no relay it wasn't given. */
function link({ writeWhole, prefs, staged, home }: Upgrade): void {
  const set = typeof prefs.phoneLink === "boolean";
  const own = typeof prefs["phoneLink.relay"] === "string" ? (prefs["phoneLink.relay"] as string).replace(/\/+$/, "") : "";
  let relay = "";
  try { if (own && ["ws:", "wss:"].includes(new URL(own).protocol)) relay = own; } catch {}
  if (!set && !relay) return;
  const file = staged("run/mac-link.json");
  if (exists(file)) return;
  const real = located(home) === located(path.join(accountHome(), ".kuvo-code"));
  if (!relay && !real) return;
  const named = spawnSync("/usr/sbin/scutil", ["--get", "ComputerName"], { encoding: "utf8", timeout: 5_000, stdio: ["ignore", "pipe", "ignore"] });
  writeWhole(file, JSON.stringify({ phones: set ? prefs.phoneLink : true, relay: relay || DEFAULT_RELAY, name: (named.status === 0 ? named.stdout.trim().slice(0, 100) : "") || "Mac", app: "" }));
}

/** Someone who used an earlier Kuvo Code has had its first run: the box they move into says so
 * (`run/welcomed.json`, "Your setup"), so they land in their sessions, not in the Welcome's steps. */
function welcomed({ writeWhole, staged }: Upgrade): void {
  const file = staged("run/welcomed.json");
  // No time in it: a move run again writes the same file.
  if (!exists(file)) writeWhole(file, JSON.stringify(welcomeFormat.write({ moved: true })));
}

// Bots, personalities, group chats

interface OldBot { id: string; name: string; job: string; instructions: string; look: unknown; machine?: string; project?: string; folder: string; sessionID: string; created: string; model?: unknown; personality?: string; agents?: boolean; updated?: string }

/** The app's list of bots (`bots.json`), those that read as bots. */
function listedBots(S: string): OldBot[] {
  const list = readLegacy(path.join(S, "bots.json"));
  return (Array.isArray(list) ? list : []).filter((bot): bot is OldBot => isObject(bot) && typeof bot.id === "string" && /^bot_[A-Za-z0-9]+$/.test(bot.id) && typeof bot.name === "string" && typeof bot.folder === "string" && typeof bot.sessionID === "string");
}

/** Each of this Mac's bots described where the box reads it, in its folder: `bot.json` (0.6 wrote
 * none), its session, where it works, and that it runs with the coding agents, each where it's
 * missing. (The list itself moves beside the bots' folder, `bots.json`, where the box takes a
 * job kept only there as it starts.) */
function bots({ writeWhole, S, staged, prefs, dropped }: Upgrade): void {
  if (exists(path.join(S, "bots.json")) && !Array.isArray(readLegacy(path.join(S, "bots.json")))) dropped.push("The earlier Kuvo Code's list of bots couldn't be read. Bots made with a version before 0.7 may be missing: their folders are brought along, in ~/.kuvo-code/bots.");
  // (Before 0.7 a bot was described only in this list: one on a machine has no description there.)
  const theirs = prefs.calibration === undefined ? listedBots(S).filter(bot => bot.machine).length : 0;
  if (theirs) dropped.push(`${theirs === 1 ? "A bot" : `${theirs} bots`} on your machines ${theirs === 1 ? "was" : "were"} described only on this Mac by the earlier Kuvo Code, and may not show there. ${theirs === 1 ? "Its" : "Their"} work is still on ${theirs === 1 ? "that machine" : "those machines"}: make ${theirs === 1 ? "it" : "them"} again if ${theirs === 1 ? "it's" : "they're"} missing.`);
  for (const bot of listedBots(S)) {
    if (bot.machine) continue;
    const folder = path.join(staged("bots"), bot.id);
    if (!isFolder(folder)) continue;
    const described = readLegacy(path.join(folder, "bot.json"));
    if (!(isObject(described) && described.kuvo === 1 && described.id === bot.id && typeof described.name === "string")) {
      writeWhole(path.join(folder, "bot.json"), JSON.stringify({
        created: bot.created, id: bot.id, job: typeof bot.job === "string" ? bot.job : "", kuvo: 1, look: bot.look,
        ...(bot.model !== undefined ? { model: bot.model } : {}), name: bot.name,
        ...(bot.personality ? { personality: bot.personality } : {}), ...(bot.project ? { project: bot.project } : {}),
        updated: bot.updated ?? bot.created,
      }, null, 2));
    }
    const text = (name: string) => { try { return fs.readFileSync(path.join(folder, name), "utf8").trim(); } catch { return ""; } };
    if (bot.sessionID && text("session") !== bot.sessionID) writeWhole(path.join(folder, "session"), bot.sessionID);
    if (!exists(path.join(folder, "directory"))) writeWhole(path.join(folder, "directory"), bot.project ?? path.join(bot.folder, "workspace"));
    if (bot.agents === true && !exists(path.join(folder, "via"))) writeWhole(path.join(folder, "via"), "bridge");
  }
}

/** The personalities you made (`personalities.json`, and from 0.8.0-beta.1 those you removed) met
 * with the box's library (`bots/personalities.json`), by its own rule. */
function personalities({ writeWhole, S, staged }: Upgrade): void {
  const made = readLegacy(path.join(S, "personalities.json")), removed = readLegacy(path.join(S, "personalities-removed.json"));
  if (!Array.isArray(made) && !Array.isArray(removed)) return;
  const file = staged("bots/personalities.json");
  const library = mergePersonalities(readPersonalities(file), {
    personalities: (Array.isArray(made) ? made : []).filter(isObject).map(each => ({ ...each, updated: Number(each.updated) || 0 })),
    removed: Array.isArray(removed) ? removed : [],
  });
  if (!library.personalities.length && !library.removed.length) return;
  writeWhole(file, JSON.stringify({ kuvo: personalityLibraryVersion, ...library }, null, 2) + "\n");
}

const isDate = (value: unknown): value is string => typeof value === "string" && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?(Z|[+-]\d\d:?\d\d)$/.test(value) && Number.isFinite(Date.parse(value));
const KINDS = ["message", "wrapUp", "note", "meeting"], STATES = ["proposed", "waiting", "working", "done", "failed"];

/** A chat's messages as the app kept them (`groups/<id>.json`), those the box's bridge takes: it
 * turns a whole chat down for one message it can't read. */
function chatMessages(file: string): Record<string, unknown>[] {
  const read = readJSON(file);
  const kept: Record<string, unknown>[] = [];
  for (const each of Array.isArray(read) ? read : []) {
    if (!isObject(each) || typeof each.id !== "string" || typeof each.text !== "string" || !isDate(each.time)) continue;
    const message: Record<string, unknown> = { ...each };
    if (message.kind === undefined) message.kind = "message";
    else if (!KINDS.includes(message.kind as string)) continue;
    if (message.author === null) delete message.author;
    else if (message.author !== undefined && typeof message.author !== "string") continue;
    if (message.tasks !== undefined && !(Array.isArray(message.tasks) && message.tasks.every(task => isObject(task) && typeof task.id === "string" && typeof task.bot === "string" && typeof task.task === "string" && STATES.includes(task.state as string)))) continue;
    kept.push(message);
  }
  return kept;
}

/** This Mac's group chats, which the app ran itself (`groups.json`, each chat's messages in
 * `groups/<id>.json`), written as the coding agents' bridge keeps them: each chat's messages
 * first, its record last (`groups/<id>/group.json`). A seat it made on a machine for a bot there
 * is taken out of the chat, so that bot's next turn makes one where it lives. What a bot was
 * still to forget goes with them (`groupMemoryWaiting` → `groups/pending-memory.json`). A chat
 * that's there already is left as it is. */
function groupChats({ writeWhole, seal, S, prefs, staged, dropped }: Upgrade): void {
  const root = staged("groups");
  const waiting = isObject(prefs.groupMemoryWaiting) ? prefs.groupMemoryWaiting : null;
  if (waiting) {
    const file = path.join(root, "pending-memory.json");
    const had = readJSON(file);
    const merged: Record<string, string[]> = {};
    for (const source of [isObject(had) ? had : {}, waiting]) for (const [bot, chats] of Object.entries(source)) merged[bot] = [...new Set([...(merged[bot] ?? []), ...strings(chats)])].sort();
    writeWhole(file, JSON.stringify(Object.fromEntries(Object.entries(merged).sort(([a], [b]) => a.localeCompare(b)))));
  }
  const list = readLegacy(path.join(S, "groups.json"));
  if (!Array.isArray(list)) {
    if (exists(path.join(S, "groups.json"))) dropped.push("Your group chats couldn't be read, and weren't brought along. They're still in the earlier Kuvo Code's folder (groups.json).");
    return;
  }
  const held = new Set((Array.isArray(prefs.movesUnderWay) ? prefs.movesUnderWay : []).filter(each => isObject(each) && each.kind === "group").map(each => (each as { id: unknown }).id));
  const onMachine = new Map(listedBots(S).filter(bot => bot.machine).map(bot => [bot.id, bot.machine!]));
  const before = readJSON(path.join(root, "imported.json"));
  const deleted = new Set(strings(isObject(before) ? before.ids : null));
  let seats = 0, failed = 0, unread = 0;
  for (const chat of list) {
    if (!isObject(chat) || typeof chat.id !== "string" || !/^grp_[a-z0-9]+$/.test(chat.id)) continue;
    const folder = path.join(root, chat.id);
    if (exists(path.join(folder, "group.json")) || deleted.has(chat.id)) continue;
    if (!Array.isArray(chat.members) || !chat.members.every(member => typeof member === "string")) { failed++; continue; }
    // (The old file of its messages is beside its folder now, under the same name: `<id>.json`.)
    const messages = chatMessages(path.join(S, "groups", `${chat.id}.json`));
    const kept = readLegacy(path.join(S, "groups", `${chat.id}.json`));
    unread += Math.max(0, (Array.isArray(kept) ? kept.length : 0) - messages.length);
    const group: Record<string, unknown> = {
      id: chat.id, name: typeof chat.name === "string" && chat.name ? chat.name : "Group Chat", members: chat.members,
      seats: stringMap(chat.seats), heard: stringMap(chat.heard),
      created: isDate(chat.created) ? chat.created : (messages[0]?.time as string | undefined) ?? new Date().toISOString().replace(/\.\d+Z$/, "Z"),
      ...(typeof chat.purpose === "string" && chat.purpose ? { purpose: chat.purpose } : {}),
      ...(Array.isArray(chat.agentSeats) && chat.agentSeats.every(seat => typeof seat === "string") ? { agentSeats: chat.agentSeats } : {}),
      ...(isDate(chat.viewed) ? { viewed: chat.viewed } : {}),
      ...(Object.keys(stringMap(chat.retracted)).length ? { retracted: stringMap(chat.retracted) } : {}),
    };
    const meeting = chat.meeting;
    if (isObject(meeting) && isDate(meeting.next) && typeof meeting.topic === "string") {
      group.meeting = { next: meeting.next, topic: meeting.topic, ...(typeof meeting.every === "string" ? { every: meeting.every } : {}), ...(Number.isInteger(meeting.at) ? { at: meeting.at } : {}), ...(Number.isInteger(meeting.weekday) ? { weekday: meeting.weekday } : {}) };
    }
    // A machine's bot's seat was a session on that machine, which only the app's window knew of.
    for (const [bot, seat] of Object.entries(group.seats as Record<string, string>)) {
      if (!onMachine.has(bot)) continue;
      delete (group.seats as Record<string, string>)[bot];
      delete (group.heard as Record<string, string>)[bot];
      if (Array.isArray(group.agentSeats)) group.agentSeats = group.agentSeats.filter(each => each !== seat);
      seats++;
    }
    writeWhole(path.join(folder, "messages.json"), JSON.stringify(messages, null, 2));
    // (Its old file, cloned with the folder, isn't the bridge's: only the folder is.)
    fs.rmSync(path.join(root, `${chat.id}.json`), { force: true });
    seal(root);
    // Metadata is also the replay guard for this chat. Publish it after its messages are durable.
    writeWhole(path.join(folder, "group.json"), JSON.stringify({ kuvo: 1, status: held.has(chat.id) ? "held" : "active", group }, null, 2));
  }
  if (failed) dropped.push(`${failed === 1 ? "One group chat" : `${failed} group chats`} couldn't be read, and ${failed === 1 ? "wasn't" : "weren't"} brought along. ${failed === 1 ? "It's" : "They're"} still in the earlier Kuvo Code's folder (groups.json).`);
  if (unread) dropped.push(`${unread === 1 ? "One message" : `${unread} messages`} in your group chats couldn't be read, and ${unread === 1 ? "wasn't" : "weren't"} brought along.`);
  if (seats) dropped.push(`In your group chats, ${seats === 1 ? "a bot on one of your machines takes its seat" : `${seats} bots on your machines take their seats`} again at ${seats === 1 ? "its" : "their"} next turn: the conversation ${seats === 1 ? "it" : "they"} had there before stays on that machine, unused.`);
}

// Letters, usage

/** Messages kept for an agent on a machine that was away (`agent-messages.json`), and from
 * 0.8.0-beta.1 the letters on their way and those seen (`agent-letters.json`,
 * `agent-letters-seen.json`), into the box's outbox, for the machines whose boxes talk to others
 * (as the app's own hand-over went: `phone-link.json` says which). The rest are said. */
function letters({ writeWhole, S, staged, dropped }: Upgrade): void {
  const waiting = readLegacy(path.join(S, "agent-messages.json")), mailed = readLegacy(path.join(S, "agent-letters.json")), seen = readLegacy(path.join(S, "agent-letters-seen.json"));
  if (!Array.isArray(waiting) && !Array.isArray(mailed) && !isObject(seen)) return;
  const hosts = readLegacy(path.join(S, "phone-link.json"));
  const machineKeys = isObject(hosts) && isObject(hosts.machines) ? hosts.machines : {};
  const keyOf = (machine: unknown): string | null => {
    const host = typeof machine === "string" ? machineKeys[machine] : null;
    return isObject(host) && host.peers === true && host.anotherMac !== true && typeof host.key === "string" && host.key ? host.key : null;
  };
  const LETTER_ID = /^[A-Za-z0-9_-]{6,80}$/;
  const now = Date.now();
  let left = 0;
  const send = (id: string, letter: { from?: unknown; to: unknown; text: unknown; at?: unknown; answers?: unknown }, machine: unknown) => {
    const peer = keyOf(machine);
    if (!peer || !LETTER_ID.test(id) || typeof letter.to !== "string" || typeof letter.text !== "string") { left++; return; }
    // Keep the wire limit without cutting a surrogate pair in half.
    const limit = (letter.text.codePointAt(31_999) ?? 0) > 0xffff ? 31_999 : 32_000;
    writeWhole(path.join(staged("run"), "outbox", "sending", `${id}.json`), JSON.stringify({
      from: typeof letter.from === "string" ? letter.from : null, to: letter.to, text: letter.text.slice(0, limit), wait: false,
      at: Number.isFinite(letter.at) ? letter.at : now, peer, ...(typeof letter.answers === "string" ? { answers: letter.answers } : {}),
    }));
  };
  for (const each of Array.isArray(mailed) ? mailed.filter(isObject) : []) send(String(each.id ?? ""), each as never, each.machine);
  for (const each of Array.isArray(waiting) ? waiting.filter(isObject) : []) {
    const id = "wtg_" + crypto.createHash("sha256").update(`${typeof each.from === "string" ? each.from : ""}\0${each.to}\0${each.text}`).digest().subarray(0, 16).toString("hex");
    send(id, each as never, each.machine);
  }
  if (isObject(seen)) {
    const file = path.join(staged("run"), "letters-seen.json");
    const had = readJSON(file);
    const marks: Record<string, number> = {};
    for (const source of [isObject(had) ? had : {}, seen]) {
      for (const [id, at] of Object.entries(source)) if (LETTER_ID.test(id) && typeof at === "number" && now - at < 2 * 86_400_000) marks[id] = Math.max(marks[id] ?? 0, at);
    }
    if (Object.keys(marks).length) writeWhole(file, JSON.stringify(marks));
  }
  if (left) dropped.push(`${left === 1 ? "A message" : `${left} messages`} one of your agents sent to an agent on a machine that was away ${left === 1 ? "wasn't" : "weren't"} delivered, and ${left === 1 ? "isn't" : "aren't"} brought along.`);
}

/** The usage panel's settings the app kept (how its cards are laid out, total spend, its
 * notifications, Codex's fallback model) into the box's (`agents/usage/settings.json`), by the
 * box's own import: once, and only the parts no box has set. */
function usage({ seal, prefs, staged }: Upgrade): void {
  const parts: Record<string, unknown> = {};
  const placed = prefs["openusage.layout.v1"];
  if (Array.isArray(placed)) {
    parts.layout = {
      placed: placed.map(each => (isObject(each) ? each.descriptorID : each)).filter(each => typeof each === "string"),
      providerOrder: strings(prefs["openusage.layout.v1.providerOrder"]), metricOrder: isObject(prefs["openusage.layout.v1.metricOrderByProvider"]) ? prefs["openusage.layout.v1.metricOrderByProvider"] : {},
      pinned: strings(prefs["openusage.layout.v1.menuBarPins"]), expanded: strings(prefs["openusage.layout.v1.expandedMetrics"]),
      expandOnEnable: strings(prefs["openusage.layout.v1.expandOnEnable"]), seeded: strings(prefs["openusage.layout.v1.seededDefaults"]),
    };
  }
  const spend = { show: prefs.showTotalSpend, period: prefs["openusage.totalSpend.period"], metric: prefs["openusage.totalSpend.metric"] };
  if (Object.values(spend).some(each => each !== undefined)) {
    parts.totalSpend = { show: typeof spend.show === "boolean" ? spend.show : true, ...(typeof spend.period === "string" ? { period: spend.period } : {}), ...(typeof spend.metric === "string" ? { metric: spend.metric } : {}) };
  }
  const notices = ["underTenPercent", "healthyToClose", "closeToRunningOut"];
  if (notices.some(name => typeof prefs[`openusage.notifications.${name}`] === "boolean")) parts.notifications = Object.fromEntries(notices.map(name => [name, prefs[`openusage.notifications.${name}`] === true]));
  if (typeof prefs["openusage.codex.fallbackModel"] === "string" && prefs["openusage.codex.fallbackModel"]) parts.codexFallback = prefs["openusage.codex.fallbackModel"];
  if (!Object.keys(parts).length) return;
  const file = path.join(staged("agents"), "usage", "settings.json");
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  new UsageSettings(file).take(parts);
  if (exists(file)) seal(file);
}

/** The engine's plugin, which the earlier Kuvo Code wrote into its settings' folder at each start
 * (its own version's): the release's, as every box has it (`config/opencode/plugins/kuvo-code.js`,
 * one of a release's files), in its place. */
function enginePlugin(release: string, { staged, writeWhole }: Upgrade): void {
  const name = "config/opencode/plugins/kuvo-code.js";
  let brought: Buffer;
  try { brought = fs.readFileSync(path.join(release, name)); } catch { return; }
  writeWhole(staged(name), brought, 0o644);
}

// What isn't carried

/** What the earlier Kuvo Code kept that the move doesn't carry, said where it applies to this Mac. */
function said({ S, prefs, dropped }: Upgrade): void {
  const count = (value: unknown) => (isObject(value) ? Object.keys(value).length : Array.isArray(value) ? value.length : 0);
  const drafts = count(prefs.composerDrafts);
  if (drafts) dropped.push(`${drafts === 1 ? "A message" : `${drafts} messages`} you'd typed but not sent ${drafts === 1 ? "wasn't" : "weren't"} brought along.`);
  if (exists(path.join(S, "Browser", "history.json"))) dropped.push("Websites you were signed in to in Kuvo Code's browser need signing in again. Your history comes along.");
  const edits = new Set([...Object.keys(isObject(prefs.botEditsWaiting) ? prefs.botEditsWaiting : {}), ...Object.keys(isObject(prefs.botEditOutbox) ? prefs.botEditOutbox : {})]).size;
  if (edits) dropped.push(`${edits === 1 ? "A change" : `${edits} changes`} you made to ${edits === 1 ? "a bot" : "bots"} on a machine that was away (a name, look, personality or model) ${edits === 1 ? "wasn't" : "weren't"} sent there. Make ${edits === 1 ? "it" : "them"} again once that machine is connected.`);
  const terminals = isObject(prefs.projectSettings) ? Object.values(prefs.projectSettings).filter(each => isObject(each) && typeof each.terminal === "string" && each.terminal).length : 0;
  if (terminals) dropped.push(`The command that opened in place of the chat for ${terminals === 1 ? "a project" : `${terminals} projects`} isn't kept.`);
  const moves = (Array.isArray(prefs.movesUnderWay) ? prefs.movesUnderWay : []).filter(each => isObject(each) && each.stage === "switched").length;
  if (moves) dropped.push(`${moves === 1 ? "A move" : `${moves} moves`} to another computer that hadn't finished cleaning up left ${moves === 1 ? "its" : "their"} original folder where it was.`);
  // What the app still owed machines that were away: an account signed out here, files to clear there.
  const signOuts = count(prefs.machineSignOuts);
  if (signOuts) dropped.push(`You signed out of an account while ${signOuts === 1 ? "a machine was" : `${signOuts} machines were`} away: ${signOuts === 1 ? "it" : "they"} may still be signed in to it. Sign out there too once ${signOuts === 1 ? "it's" : "they're"} connected.`);
  const cleanUps = count(prefs["machines.waiting"]);
  if (cleanUps) dropped.push(`${cleanUps === 1 ? "A machine that was away still has" : `${cleanUps} machines that were away still have`} files of something you removed here (a bot, or Kuvo Code itself): remove them from there once connected.`);
  // What the app kept for your machines (its window gave each to that machine's own as it connected).
  const onMachine = (value: unknown) => (isObject(value) ? Object.keys(value) : strings(value)).filter(folder => folder.startsWith("ssh://")).length;
  const theirProjects = new Set([...(isObject(prefs.projectSettings) ? Object.keys(prefs.projectSettings) : []), ...strings(prefs.projectFolders)].filter(folder => folder.startsWith("ssh://"))).size;
  const theirWorktrees = onMachine(prefs.worktrees) + onMachine(prefs.removedWorktrees);
  if (theirProjects) dropped.push(`${theirProjects === 1 ? "A project" : `${theirProjects} projects`} on your machines may have lost ${theirProjects === 1 ? "its" : "their"} name, extra folders or Run command, which the earlier Kuvo Code kept on this Mac. Set them again in each project's settings.`);
  if (theirWorktrees) dropped.push(`${theirWorktrees === 1 ? "A worktree" : `${theirWorktrees} worktrees`} on your machines may show ${theirWorktrees === 1 ? "its" : "their"} sessions under another project: the earlier Kuvo Code kept which project ${theirWorktrees === 1 ? "it" : "each"} belonged to on this Mac.`);
  if (Object.keys(prefs).some(key => key.startsWith("machines.unreadable-"))) dropped.push("The earlier Kuvo Code had set aside a list of machines it couldn't read. It isn't brought along: add any machine that's missing again.");
  // (API keys pasted into the usage panel, OpenRouter's and Z.ai's, are its own Keychain items,
  // which nothing here can see: said only where the panel had one of those two connected.)
  const keyed = strings(prefs["kuvo.usage.connectedProviders.v1"]).filter(id => /openrouter|zai|z\.ai/i.test(id)).length;
  if (keyed) dropped.push("An API key you pasted into the usage panel (OpenRouter's or Z.ai's) isn't brought along. Paste it again in the usage panel.");
  if (prefs.lastModelRef !== undefined || prefs.lastModel !== undefined) dropped.push("The model new sessions start with is chosen again in the new app.");
  if (prefs["notifications.enabled"] === false || prefs["notifications.sound"] === false || prefs["notifications.badge"] === false) dropped.push("You had turned off some of Kuvo Code's notifications (or their sound or badge). Set that again in the new app.");
  if (prefs["analytics.enabled"] === false) dropped.push("You had turned off sharing anonymous usage in the earlier Kuvo Code. That choice isn't brought along: check it in the new app's settings.");
  if (listOf(S).some(name => /\.unreadable-.*\.json$/.test(name))) dropped.push("The earlier Kuvo Code had set aside a list it couldn't read. It's still in its folder (a file named …unreadable…json).");
  if (["openrouter.json", "zai.json", "proxy.json"].some(name => exists(path.join(S, "Usage", name)))) dropped.push("API keys or a proxy you set up by hand for the usage panel (files in the earlier Kuvo Code's Usage folder) aren't brought along.");
}

/** Every upgrade, in the staged copy: what wasn't carried, in words to show once. One that fails
 * stops the move (it goes back): nothing is left half brought along. */
export function runUpgrades(run: RunPlan, move: MoveContext): string[] {
  const made = new Set<string>();
  const seal = (entry: string) => {
    const placement = move.placements.find(each => entry === each.staged || entry.startsWith(each.staged + path.sep));
    if (!placement) throw new Error("An import attempted to sync outside staging");
    syncDurableFile(entry, path.dirname(placement.staged));
  };
  const upgrade: Upgrade = {
    seal,
    writeWhole(file, bytes, mode) {
      replaceStaged(file, bytes, mode);
      seal(file);
    },
    S: move.appFolder,
    home: move.home,
    prefs: appPreferences(move.appFolder),
    staged(name) {
      const [top, ...rest] = name.split("/");
      const placement = placementOf(move, top);
      if (!placement) throw new Error(`Nothing of the move's goes to ${top}.`);
      // (A place the app never had, made as its folder would be: yours alone.)
      const folder = rest.length ? path.join(placement.staged, ...rest.slice(0, -1)) : path.dirname(placement.staged);
      if (!made.has(folder)) {
        fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
        made.add(folder);
      }
      return path.join(placement.staged, ...rest);
    },
    dropped: [],
  };
  // (Places that are folders: there before what's written in them. Those the app never had are
  // made here, and taken away again below where nothing was written in them: one it had, empty
  // too, was cloned and stays, as everything cloned does.)
  const places = ["box", "run", "bots", "groups", "agents"];
  const madeHere = places.filter(name => !exists(upgrade.staged(name)));
  for (const name of places) fs.mkdirSync(upgrade.staged(name), { recursive: true, mode: 0o700 });
  for (const each of [projects, worktrees, machines, engineSettings, boxSettings, link, welcomed, bots, personalities, groupChats, letters, usage, said]) each(upgrade);
  enginePlugin(run.release, upgrade);
  // (A place nothing was written in isn't made: a Mac with no bots gets no bots' folder from this.)
  for (const name of places) {
    const folder = upgrade.staged(name);
    if (madeHere.includes(name) && name !== "box") try { fs.rmdirSync(folder); } catch {}
    if (exists(folder)) seal(folder);
    else syncDurableFile(path.dirname(folder));
  }
  return upgrade.dropped;
}
