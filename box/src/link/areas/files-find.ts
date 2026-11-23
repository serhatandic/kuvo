// Finding files (PROTOCOL.md, `kuvo/files/find`): what a Files tab's filter and a new tab's "Search
// files or enter a URL" show as you type, worked out here, so a client draws the list it's sent and
// decides nothing: the files under a folder whose path there matches the text, best first, and the
// web address the text reads as, if it reads as one.
//
// Platform: the folder's files are `kuvo/folders/tree`'s (git's list, else a walk), the same on
// macOS and Linux; the rest is plain text.

import path from "node:path";
import type { FilesFound } from "../../shared/routes/misc";
import type { AnswerPiece, RouteContext } from "../server/context";
import { listTree, TREE_LIMIT, type FolderTree } from "./folders";

export const FILES_FIND_ROUTE = "kuvo/files/find";

/** How many files an answer names unless asked for fewer or more, and at most. */
export const FOUND_DEFAULT = 50;
export const FOUND_MOST = 200;

// Whether `letters` come in `text` in that order, with anything between.
function inOrder(text: string, letters: string): boolean {
  let at = 0;
  for (const letter of text) if (letter === letters[at] && ++at === letters.length) return true;
  return letters.length === 0;
}

/** Those of `paths` (each relative to its folder) that `typed` matches, best first, `limit` at most:
 * with no regard for case, the text in a file's name (the name itself, then a name that starts with
 * it, then one with it inside), then in its path, then its letters in order in its name, then in its
 * path (spaces in what's typed left out of those); each the shorter path first, then by path. Blank
 * text matches nothing. */
export function rankFiles(paths: readonly string[], typed: string, limit: number): string[] {
  const text = typed.trim().toLowerCase();
  if (!text) return [];
  const letters = text.replace(/\s+/g, "");
  const found: { at: string; rank: number }[] = [];
  for (const at of paths) {
    const lower = at.toLowerCase();
    const name = lower.slice(lower.lastIndexOf("/") + 1);
    const rank = name === text ? 0 : name.startsWith(text) ? 1 : name.includes(text) ? 2 : lower.includes(text) ? 3
      : inOrder(name, letters) ? 4 : inOrder(lower, letters) ? 5 : -1;
    if (rank >= 0) found.push({ at, rank });
  }
  found.sort((a, b) => a.rank - b.rank || a.at.length - b.at.length || (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
  return found.slice(0, limit).map(each => each.at);
}

// The last part of a name that's a file's kind, never a web address's: `signup.ts`, `notes.md`.
// (`sh` and `md` are countries too, and `rs`, `py`, `pl`: as typed here, files.)
const FILE_KINDS = new Set([
  "bash", "bat", "c", "cc", "cfg", "cjs", "cmd", "conf", "cpp", "cs", "css", "csv", "dart", "env", "ex", "exs", "gif", "go",
  "gql", "gradle", "graphql", "gz", "h", "hpp", "hs", "htm", "html", "ini", "ipynb", "java", "jpeg", "jpg", "js", "json",
  "jsx", "kt", "less", "lock", "log", "lua", "md", "mdx", "mjs", "ml", "mod", "nix", "pdf", "php", "pl", "plist", "png",
  "proto", "ps", "py", "rb", "rs", "sass", "scala", "scss", "sh", "sql", "sum", "svelte", "svg", "swift", "tar", "tf",
  "toml", "ts", "tsx", "txt", "vue", "xml", "yaml", "yml", "zig", "zip", "zsh",
]);
// The endings a bare name (no port, no path) reads as a web address with.
const WEB_ENDINGS = new Set([
  "ai", "app", "biz", "blog", "ca", "ch", "cloud", "co", "com", "de", "dev", "edu", "eu", "fm", "fr", "gg", "gov", "info",
  "io", "jp", "ly", "me", "net", "nl", "org", "page", "run", "site", "so", "tech", "to", "tv", "uk", "us", "xyz",
]);

const isIPv4 = (host: string) => {
  const parts = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host);
  return parts !== null && parts.slice(1).every(part => Number(part) <= 255);
};

/** The web address `typed` reads as, with its scheme, or null where it doesn't read as one. With
 * `http://` or `https://`, as typed (one that isn't a whole address is none); without a scheme,
 * `localhost` or an IP address (with a port, a path, or neither), `http://` before it, and a name
 * with a dot (`example.com`, `docs.example.dev:8080/a`), `https://`; but never one whose last part
 * is a file's kind (`signup.ts`, `notes.md`, `main.go:12`), and a bare name (no port or path) only
 * with a common web ending (`kuvo.run`, `example.com`). Text with a space in it is none. */
export function webAddress(typed: string): string | null {
  const text = typed.trim();
  if (!text || /\s/.test(text)) return null;
  if (/^https?:\/\//i.test(text)) {
    try {
      return new URL(text).hostname ? text : null;
    } catch {
      return null;
    }
  }
  // (Another scheme, or a word and a colon, `mailto:`, `note:`: not a web page to open.)
  if (/^[a-z][a-z0-9+.-]*:(?!\d)/i.test(text)) return null;
  const [, hostAndPort = "", rest = ""] = /^([^/?#]*)(.*)$/.exec(text) ?? [];
  const parts = /^(\[[0-9a-f:.]+\]|[^:[\]]+)(?::(\d{1,5}))?$/i.exec(hostAndPort);
  if (!parts) return null;
  const host = parts[1]!.toLowerCase();
  const port = parts[2];
  if (port !== undefined && (Number(port) < 1 || Number(port) > 65_535)) return null;
  if (host === "localhost" || isIPv4(host) || host.startsWith("[")) return `http://${text}`;
  const labels = host.split(".");
  if (labels.length < 2 || !labels.every(label => /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label))) return null;
  const ending = labels.at(-1)!;
  if (!/^[a-z]{2,63}$/.test(ending) || FILE_KINDS.has(ending)) return null;
  const bare = port === undefined && !rest.startsWith("/");
  return bare && !WEB_ENDINGS.has(ending) ? null : `https://${text}`;
}

/** A file found under `top` (a full path), by its path there. */
function found(top: string, at: string): FilesFound["files"][number] {
  const cut = at.lastIndexOf("/");
  return { path: path.join(top, at), name: at.slice(cut + 1), folder: cut < 0 ? "" : at.slice(0, cut) };
}

/** `GET kuvo/files/find?directory=<folder>&q=<text>&limit=<n>`: the files under `directory` that the
 * text matches (`rankFiles`), and the web address it reads as (`webAddress`). */
export async function* filesFindRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json, signal } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const query = new URLSearchParams(request.path.includes("?") ? request.path.slice(request.path.indexOf("?") + 1) : "");
  const directory = query.get("directory")?.trim() ?? "";
  if (!directory) return yield* json(400, { message: "Name the folder to look in: directory=<its full path>." });
  const typed = query.get("q") ?? "";
  const asked = Number(query.get("limit") ?? "");
  const limit = Number.isInteger(asked) && asked > 0 ? Math.min(asked, FOUND_MOST) : FOUND_DEFAULT;
  const url = webAddress(typed);
  // (Nothing typed finds nothing: the folder isn't read for it.)
  if (!typed.trim()) return yield* json(200, { url, files: [] } satisfies FilesFound);
  // Its files as `kuvo/folders/tree` lists them, refused as it refuses a folder it can't read.
  const tree = await listTree(`kuvo/folders/tree?path=${encodeURIComponent(directory)}`, TREE_LIMIT, signal ?? undefined);
  if (tree.status !== 200) return yield* json(tree.status, tree.value);
  const { path: top, files } = tree.value as FolderTree;
  return yield* json(200, { url, files: rankFiles(files, typed, limit).map(at => found(top, at)) } satisfies FilesFound);
}
