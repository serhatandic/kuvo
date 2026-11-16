// How much of this computer's disk Kuvo takes, for Settings › Storage (`kuvo/settings/storage`,
// PROTOCOL.md "A box's settings", a `meter` row's `parts`): its home folder (`~/.kuvo-code`), each
// part of it by what it holds (sessions, worktrees, what agents made, the browser, the rest), and
// worktree folders you chose outside it.
//
// Measured with `du` off the event loop (a home with a few worktrees holds millions of files), once
// as the page is first read, again when it's an hour old or Refresh is pressed (a measure that
// failed isn't tried again before then either, and one taking past ten minutes is stopped); the
// last one is kept in `run/storage-sizes.json`, so whichever of the box's processes reads the page
// shows it. Worktrees are the ones the box keeps records of, each measured by itself (never a
// whole folder they're in, which may hold your projects too). Folders are measured where they
// really are (a home linked to another disk is followed).
// Platform: `du -k -d N` is the same on macOS and Linux (BSD's and GNU's both take it).

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { CHATS, ENGINE_DATA, K, RUN, attachmentRoots } from "../home";
import { writePrivate } from "../../shared/files";
import { readWorktrees } from "./worktree-records";

export const SIZES_FILE = path.join(RUN, "storage-sizes.json");
/** How old a measure is before reading the page measures again by itself. */
const STALE_AFTER = 60 * 60 * 1000;
/** How long one `du` may take before it's stopped. */
const TAKES_AT_MOST = 10 * 60 * 1000;

/** One part of what Kuvo keeps, in bytes, with what it's made of (the rest's own folders). */
export type SizePart = { id: string; title: string; bytes: number; folders?: { title: string; bytes: number }[] };
export type Sizes = { at: number; total: number; home: number; parts: SizePart[]; outside: { path: string; bytes: number; trees: number }[] };

/** What the parts of Kuvo's folder are, by the folder's name in it. */
const NAMED: Record<string, string> = {
  agents: "Coding agents", engine: "The engine", tools: "Tools", cache: "Caches", trash: "Deleted, kept a while", bin: "Programs",
  box: "Settings and keys", run: "Running state", updates: "Updates", search: "Search index", integrations: "Integrations",
  bots: "Bots", groups: "Group chats", wakes: "Wake-ups", workspace: "Workspace", state: "State", config: "Engine settings",
  t: "Temporary files", tmp: "Temporary files", keys: "Keys", attachments: "Attachments", chats: "Chats", data: "Sessions",
};

let measuring: Promise<void> | null = null;
let lastError: string | null = null;
/** When this process's last measure failed (not begun again before that's old, unless Refresh). */
let failedAt = 0;

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

/** Measures again where there's none, it's old, or `now` says to (Refresh); never two at once. */
export function measureSizes(now = false, home = K, trees = (): string[] => readWorktrees().map(each => each.root)): Promise<void> {
  if (measuring) return measuring;
  const last = readSizes();
  const fresh = (at: number) => Date.now() - at < STALE_AFTER;
  if (!now && ((last && fresh(last.at)) || fresh(failedAt))) return Promise.resolve();
  measuring = Promise.resolve().then(() => measured(home, trees()))
    .then(sizes => { lastError = null; failedAt = 0; writePrivate(SIZES_FILE, JSON.stringify(sizes)); })
    .catch(error => { failedAt = Date.now(); lastError = error instanceof Error ? error.message : String(error); })
    .finally(() => { measuring = null; });
  return measuring;
}

/** `du -k -d depth` of each folder: kilobytes by path. Unreadable parts are left out by `du` itself
 * (it still answers for the rest, exiting 1). Stopped past `TAKES_AT_MOST`, said why. */
function du(folders: string[], depth: number): Promise<Map<string, number>> {
  const there = folders.filter(folder => fs.existsSync(folder));
  if (!there.length) return Promise.resolve(new Map());
  return new Promise((resolve, reject) => {
    const child = spawn("du", ["-k", "-d", String(depth), "--", ...there], { stdio: ["ignore", "pipe", "ignore"] });
    let out = "";
    let late = false;
    const limit = setTimeout(() => { late = true; child.kill("SIGKILL"); }, TAKES_AT_MOST);
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", chunk => { out += chunk; });
    child.on("error", error => { clearTimeout(limit); reject(error); });
    child.on("close", () => {
      clearTimeout(limit);
      if (late) return reject(new Error("measuring took too long (a folder on a slow or stalled disk?)."));
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
const real = (folder: string) => { try { return fs.realpathSync(folder); } catch { return path.resolve(folder); } };

/** Kuvo's folder measured, and each part of it; worktrees kept outside it after, by the folder
 * they're in. */
export async function measured(home = K, treeRoots: string[] = []): Promise<Sizes> {
  const root = real(home);
  const sizes = await du([root], 3);
  const of = (folder: string) => sizes.get(folder) ?? 0;
  const homeBytes = of(root);
  // Worktrees: the engine's default folder (in its data), and each one the box keeps a record of
  // elsewhere, measured by itself.
  const fallback = real(path.join(ENGINE_DATA, "worktree"));
  const roots = [...new Set(treeRoots.filter(each => path.isAbsolute(each)).map(real))].filter(each => !inside(each, fallback) && fs.existsSync(each));
  const rootSizes = await du(roots, 0);
  const sized = (each: string) => (inside(each, root) && sizes.has(each) ? of(each) : rootSizes.get(each) ?? 0);
  const treesInside = [...(fs.existsSync(fallback) && inside(fallback, root) ? [fallback] : []), ...roots.filter(each => inside(each, root))];
  const outsideRoots = roots.filter(each => !inside(each, root));
  const byFolder = new Map<string, { bytes: number; trees: number }>();
  for (const each of outsideRoots) {
    const folder = path.dirname(each);
    const kept = byFolder.get(folder) ?? { bytes: 0, trees: 0 };
    byFolder.set(folder, { bytes: kept.bytes + sized(each), trees: kept.trees + 1 });
  }
  const outside = [...byFolder].map(([folder, kept]) => ({ path: folder, ...kept }));
  const sum = (folders: string[]) => folders.reduce((total, each) => total + sized(each), 0);
  const outsideBytes = outside.reduce((total, each) => total + each.bytes, 0);
  // Sessions: the engine's data and your chats and attachments, without worktrees kept among them.
  const sessionFolders = [real(ENGINE_DATA), real(CHATS), ...attachmentRoots().map(real)].filter(each => inside(each, root));
  const sessions = Math.max(0, sessionFolders.reduce((total, each) => total + of(each), 0) - sum(treesInside.filter(each => sessionFolders.some(folder => inside(each, folder)))));
  const made = of(path.join(root, "artifacts"));
  const browser = of(path.join(root, "browser"));
  // The rest: what's left of Kuvo's folder, and its own folders for the breakdown, with what's in
  // none of those named (files at its top, folders that hold a part counted above) as one more.
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
  const rest = other - folders.reduce((total, each) => total + each.bytes, 0);
  if (rest > 0) folders.push({ title: "Other files", bytes: rest });
  const parts: SizePart[] = [
    { id: "sessions", title: "Sessions", bytes: sessions },
    { id: "worktrees", title: "Worktrees", bytes: sum(treesInside) + outsideBytes },
    { id: "made", title: "What agents made", bytes: made },
    { id: "browser", title: "Browser", bytes: browser },
    { id: "other", title: "Other", bytes: other, folders },
  ];
  return { at: Date.now(), total: homeBytes + outsideBytes, home: homeBytes, parts, outside };
}

function safeList(folder: string): string[] {
  try { return fs.readdirSync(folder).filter(name => !name.startsWith(".")); } catch { return []; }
}
