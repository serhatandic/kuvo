// How much of this computer's disk Kuvo takes, for Settings › Storage (`kuvo/settings/storage`,
// PROTOCOL.md "A box's settings", a `meter` row's `parts`): its home folder (`~/.kuvo-code`), each
// part of it by what it holds (sessions, worktrees, what agents made, the browser, the rest), and
// worktree folders you chose outside it.
//
// Measured with `du` off the event loop (a home with a few worktrees holds millions of files), once
// as the page is first read, again when it's an hour old or Refresh is pressed; the last measure is
// kept in `run/storage-sizes.json`, so whichever of the box's processes reads the page shows it.
// Platform: `du -k -d N` is the same on macOS and Linux (BSD's and GNU's both take it).

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { CHATS, ENGINE_DATA, K, RUN, attachmentRoots } from "../home";
import { writePrivate } from "../../shared/files";

export const SIZES_FILE = path.join(RUN, "storage-sizes.json");
/** How old a measure is before reading the page measures again by itself. */
const STALE_AFTER = 60 * 60 * 1000;

/** One part of what Kuvo keeps, in bytes, with what it's made of (the rest's own folders). */
export type SizePart = { id: string; title: string; bytes: number; folders?: { title: string; bytes: number }[] };
export type Sizes = { at: number; total: number; home: number; parts: SizePart[]; outside: { path: string; bytes: number }[] };

/** What the parts of Kuvo's folder are, by the folder's name in it. */
const NAMED: Record<string, string> = {
  agents: "Coding agents", engine: "The engine", tools: "Tools", cache: "Caches", trash: "Deleted, kept a while", bin: "Programs",
  box: "Settings and keys", run: "Running state", updates: "Updates", search: "Search index", integrations: "Integrations",
  bots: "Bots", groups: "Group chats", wakes: "Wake-ups", workspace: "Workspace", state: "State", config: "Engine settings",
  t: "Temporary files", tmp: "Temporary files", keys: "Keys", attachments: "Attachments", chats: "Chats", data: "Sessions",
};

let measuring: Promise<void> | null = null;
let lastError: string | null = null;

/** The last measure, from any process. */
export function readSizes(file = SIZES_FILE): Sizes | null {
  try {
    const kept = JSON.parse(fs.readFileSync(file, "utf8"));
    return kept && typeof kept.total === "number" && Array.isArray(kept.parts) ? kept as Sizes : null;
  } catch {
    return null;
  }
}

/** Whether this process is measuring now. */
export const isMeasuring = (): boolean => measuring !== null;
/** Why the last measure didn't finish, in words. */
export const measureProblem = (): string | null => lastError;

/** Measures again where there's none, it's old, or `now` says to; never two at once. */
export function measureSizes(now = false, worktreeFolders: string[] = [], home = K): Promise<void> {
  if (measuring) return measuring;
  const last = readSizes();
  if (!now && last && Date.now() - last.at < STALE_AFTER) return Promise.resolve();
  measuring = measured(home, worktreeFolders)
    .then(sizes => { lastError = null; writePrivate(SIZES_FILE, JSON.stringify(sizes)); })
    .catch(error => { lastError = error instanceof Error ? error.message : String(error); })
    .finally(() => { measuring = null; });
  return measuring;
}

/** `du -k -d depth` of each folder: kilobytes by path. Unreadable parts are left out by `du` itself
 * (it still answers for the rest, exiting 1). */
function du(folders: string[], depth: number): Promise<Map<string, number>> {
  const there = folders.filter(folder => fs.existsSync(folder));
  if (!there.length) return Promise.resolve(new Map());
  return new Promise((resolve, reject) => {
    const child = spawn("du", ["-k", "-d", String(depth), "--", ...there], { stdio: ["ignore", "pipe", "ignore"] });
    let out = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", chunk => { out += chunk; });
    child.on("error", reject);
    child.on("close", () => {
      const sizes = new Map<string, number>();
      for (const line of out.split("\n")) {
        const match = line.match(/^(\d+)\t(.+)$/);
        if (match) sizes.set(path.resolve(match[2]!), Number(match[1]) * 1024);
      }
      resolve(sizes);
    });
  });
}

const inside = (folder: string, root: string) => folder === root || folder.startsWith(root + path.sep);

/** Kuvo's folder measured, and each part of it; worktree folders outside it after. */
export async function measured(home = K, worktreeFolders: string[] = []): Promise<Sizes> {
  const root = path.resolve(home);
  const sizes = await du([root], 3);
  const of = (folder: string) => sizes.get(path.resolve(folder)) ?? 0;
  const homeBytes = of(root);
  // Worktrees: the default folder in the engine's data, and any chosen, inside Kuvo's folder or not.
  const trees = [...new Set([path.join(ENGINE_DATA, "worktree"), ...worktreeFolders].map(each => path.resolve(each)))];
  const outsideTrees = trees.filter(each => !inside(each, root));
  const outsideSizes = await du(outsideTrees, 0);
  const treesOutside = outsideTrees.map(each => ({ path: each, bytes: outsideSizes.get(each) ?? 0 })).filter(each => fs.existsSync(each.path));
  const sum = (folders: string[]) => folders.reduce((total, each) => total + of(each), 0);
  // Sessions: the engine's data and your chats and attachments, without worktrees kept among them.
  const sessionFolders = [path.resolve(ENGINE_DATA), path.resolve(CHATS), ...attachmentRoots().map(each => path.resolve(each))].filter(each => inside(each, root));
  const treesInside = trees.filter(each => inside(each, root));
  const treesInSessions = treesInside.filter(each => sessionFolders.some(folder => inside(each, folder)));
  const sessions = Math.max(0, sum(sessionFolders) - sum(treesInSessions));
  const made = of(path.join(root, "artifacts"));
  const browser = of(path.join(root, "browser"));
  // The rest: what's left of Kuvo's folder, and its own folders for the breakdown.
  const other = Math.max(0, homeBytes - sessions - made - browser - sum(treesInside));
  const counted = [...sessionFolders, ...treesInside, path.join(root, "artifacts"), path.join(root, "browser")];
  const folders: { title: string; bytes: number }[] = [];
  for (const name of safeList(root)) {
    const folder = path.join(root, name);
    if (counted.some(each => inside(each, folder))) continue;
    const bytes = of(folder);
    if (bytes > 0) folders.push({ title: NAMED[name] ?? name, bytes });
  }
  folders.sort((a, b) => b.bytes - a.bytes);
  const parts: SizePart[] = [
    { id: "sessions", title: "Sessions", bytes: sessions },
    { id: "worktrees", title: "Worktrees", bytes: sum(treesInside) + treesOutside.reduce((sum, each) => sum + each.bytes, 0) },
    { id: "made", title: "What agents made", bytes: made },
    { id: "browser", title: "Browser", bytes: browser },
    { id: "other", title: "Other", bytes: other, folders },
  ];
  return { at: Date.now(), total: homeBytes + treesOutside.reduce((sum, each) => sum + each.bytes, 0), home: homeBytes, parts, outside: treesOutside };
}

function safeList(folder: string): string[] {
  try { return fs.readdirSync(folder).filter(name => !name.startsWith(".")); } catch { return []; }
}
