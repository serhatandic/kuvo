// Folders worth starting in (PROTOCOL.md, `kuvo/folders/suggested`, #733): on this box's computer,
// where its agents worked lately (its sessions, the engine's and the coding agents'), newest first,
// then the git repositories directly in ~/Projects and ~/Developer. Facts for a first run's or a new
// session's folder picker; a client draws them, and `kuvo/folders` still browses any other. Fast and
// bounded: one level of two folders, a few hundred entries read at most, nothing deeper.

import fs from "node:fs";
import path from "node:path";
import { promptedFolders } from "../../shared/platform";
import { isChatsFolder } from "../engine/chats";
import { homeFolder, K, located } from "../home";
import { keptList } from "../lists/sessions";
import type { AnswerPiece, RouteContext } from "../server/context";

export const FOLDERS_SUGGESTED_ROUTE = "kuvo/folders/suggested";

/** How many folders are suggested at most, and how many of them from the sessions. */
export const MOST_SUGGESTED = 20;
const MOST_WORKED = 12;
/** The folders looked in for repositories, from the home folder, one level each. */
export const LOOKED_IN = ["Projects", "Developer"];
/** How many entries of each of them are read at most (a folder of thousands stays quick). */
const MOST_READ = 500;
/** The kind of session whose folder you'd start in again: a project's (never a chat's, whose
 * folder is the box's chats folder, a bot's, its seat's, one kept out of sight, or a subagent's,
 * which work where they're put). */
const WORKED_KINDS = new Set(["project"]);

/** A folder suggested: its full path, its name, whether it's a git repository (left out where it
 * isn't, or isn't looked into: one the system would ask you about first), and when an agent last
 * worked there (ms; null for a repository found only by looking). */
export interface Suggested {
  path: string;
  name: string;
  repository?: true;
  seen: number | null;
}

/** A session, as far as this goes: its kind, its project's folder (a worktree's session is filed
 * under the project it's of) and its own, and when it last changed. */
export interface WorkedIn {
  kind: string;
  project?: string;
  directory: string;
  updated: number;
  archived?: boolean;
}

const isFolder = (at: string): boolean => {
  try {
    return fs.statSync(at).isDirectory();
  } catch {
    return false;
  }
};
const isRepository = (at: string): boolean => fs.existsSync(path.join(at, ".git"));

/** Whether `at` is inside `folder` (or is it). */
const within = (at: string, folder: string): boolean => at === folder || at.startsWith(folder.endsWith(path.sep) ? folder : folder + path.sep);

/** The folders suggested on this computer, from `sessions` and what's in `home`'s ~/Projects and
 * ~/Developer. `box`: this box's own folder, whose sessions' folders (a bot's, a chat's of its own)
 * aren't yours to start in. */
export function suggestedFolders(home: string, sessions: WorkedIn[], box: string = K, platform: NodeJS.Platform = process.platform): Suggested[] {
  // Platform: on a Mac, looking into these would have macOS ask you first (a phone asking, you
  // away), as it does of another disk (`/Volumes`): a folder there an agent worked in is suggested
  // as it is, not looked into.
  const prompted = [...promptedFolders(platform)].flatMap(name => [path.join(home, name), path.join(located(home), name)]);
  if (platform === "darwin") prompted.push("/Volumes");
  const asks = (at: string) => prompted.some(folder => within(at, folder));
  // Compared as the system names them (a home reached through a link, as the engine records it).
  const realHome = located(home), realBox = located(box);
  const real = (at: string) => (asks(at) ? at : located(at));
  const latest = new Map<string, number>();
  for (const session of sessions) {
    const folder = typeof session.project === "string" && path.isAbsolute(session.project) ? session.project : session.directory;
    if (!WORKED_KINDS.has(session.kind) || session.archived || typeof folder !== "string" || !path.isAbsolute(folder)) continue;
    const at = real(path.resolve(folder));
    // Not the home folder itself (nothing to start in), the system's top, the box's own folders
    // nor its chats folder (on a Mac, the app's).
    if (at === realHome || at === home || at === path.parse(at).root || within(at, realBox) || within(at, box) || (!asks(at) && isChatsFolder(at))) continue;
    if ((latest.get(at) ?? -Infinity) < session.updated) latest.set(at, session.updated);
  }
  const suggested: Suggested[] = [];
  const taken = new Set<string>();
  for (const [at, seen] of [...latest].sort((a, b) => b[1] - a[1])) {
    if (suggested.length >= MOST_WORKED) break;
    const unasked = !asks(at);
    // (One gone since isn't suggested; one the system asks about first is taken as there.)
    if (unasked && !isFolder(at)) continue;
    suggested.push({ path: at, name: path.basename(at), ...(unasked && isRepository(at) ? { repository: true as const } : {}), seen });
    taken.add(at);
  }
  const found: { at: string; changed: number }[] = [];
  for (const top of LOOKED_IN) {
    // (Under the home folder as the system names it, as the sessions' folders are.)
    const folder = path.join(realHome, top);
    let listing: fs.Dir;
    try {
      listing = fs.opendirSync(folder);
    } catch {
      continue;
    }
    try {
      for (let read = 0; read < MOST_READ; read++) {
        const entry = listing.readSync();
        if (!entry) break;
        if (entry.name.startsWith(".")) continue;
        const at = path.join(folder, entry.name);
        // A link is followed only where it doesn't lead somewhere macOS asks about.
        if (entry.isSymbolicLink()) {
          let to: string;
          try {
            to = path.resolve(folder, fs.readlinkSync(at));
          } catch {
            continue;
          }
          if (asks(to)) continue;
        }
        // (A link, or an entry whose kind the disk doesn't say, is asked of.)
        const isDirectory = entry.isDirectory() || (!entry.isFile() && isFolder(at));
        if (taken.has(located(at)) || !isDirectory || !isRepository(at)) continue;
        let changed = 0;
        try {
          changed = fs.statSync(at).mtimeMs;
        } catch {}
        found.push({ at, changed });
      }
    } finally {
      listing.closeSync();
    }
  }
  // The repositories found, the one changed latest first (by its folder: a file added or removed).
  for (const { at } of found.sort((a, b) => b.changed - a.changed || a.at.localeCompare(b.at))) {
    if (suggested.length >= MOST_SUGGESTED) break;
    suggested.push({ path: at, name: path.basename(at), repository: true, seen: null });
  }
  return suggested;
}

/** `GET kuvo/folders/suggested`: `{home, folders}`, for this box's computer. */
export async function* foldersSuggestedRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  // As the system names it, as every folder suggested is.
  const home = located(homeFolder());
  const sessions = Object.values(keptList()?.sessions ?? {}).map(each => each.facts);
  return yield* json(200, { home, folders: suggestedFolders(home, sessions) });
}
