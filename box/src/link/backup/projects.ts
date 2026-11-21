import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { isObject } from "../../shared/json";
import { syncDurableFile } from "../files";
import { archivePath, type Entry } from "./archive";
import { inside, outputPath, stat } from "./locations";

const INDEX = "meta/projects.json";
type ProjectEntry = { name: string; mode: number; type: "file" } | { name: string; mode: number; type: "directory" } | { name: string; type: "link"; target: string };
type Omission = { entry: string; reason: string };

/** Git paths are advisory. Export walks the entire selected folder, including ignored files. */
export function previewProjects(folders: readonly string[]) {
  const deadline = Date.now() + 3000;
  let limit = 200;
  return folders.map(folder => {
    const remaining = deadline - Date.now();
    if (remaining <= 0) return { folder, gitStatus: "unavailable" };
    const status = spawnSync("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "-c", "core.quotePath=false",
      "-C", folder, "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all", "--ignore-submodules=all", "--", "."],
    { encoding: "utf8", maxBuffer: 128 * 1024, timeout: Math.min(remaining, 1500), env: { ...process.env, GIT_TERMINAL_PROMPT: "0" } });
    if (status.status !== 0) return { folder, gitStatus: "unavailable" };
    const entries = status.stdout.split("\0").filter(Boolean);
    const visible = entries.slice(0, limit);
    limit -= visible.length;
    return { folder, gitStatus: "read", truncated: visible.length < entries.length, changes: visible.map(entry => ({
      path: entry.slice(3), status: entry.slice(0, 2), untracked: entry.startsWith("?? "),
    })) };
  });
}

function safeLink(name: string, target: string): boolean {
  if (!target || target.includes("\\") || target.includes("\0") || path.isAbsolute(target)) return false;
  // A ".." after a named component can traverse a symlink before it goes up.
  let named = false;
  for (const part of target.split("/")) {
    if (!part || part === ".") continue;
    if (part === ".." && named) return false;
    named ||= part !== "..";
  }
  const root = name.split("/").slice(0, 2).join("/");
  return inside(root, path.join(path.dirname(name), target));
}

export function* projectEntries(projects: readonly string[]): Generator<Entry> {
  if (!projects.length) return;
  const entries: ProjectEntry[] = [];
  const omissions: Omission[] = [];
  function* walk(file: string, name: string): Generator<Entry> {
    if (!archivePath(name)) {
      omissions.push({ entry: name, reason: "unsupported path" });
      return;
    }
    const info = fs.lstatSync(file);
    if (info.isSymbolicLink()) {
      const target = fs.readlinkSync(file);
      if (safeLink(name, target)) entries.push({ name, target, type: "link" });
      else omissions.push({ entry: name, reason: "unsafe symbolic link" });
    } else if (info.isFile()) {
      entries.push({ name, mode: info.mode & 0o777, type: "file" });
      yield { name, file };
    } else if (info.isDirectory()) {
      entries.push({ name, mode: info.mode & 0o777, type: "directory" });
      for (const child of fs.readdirSync(file).sort()) yield* walk(path.join(file, child), name + "/" + child);
    } else {
      omissions.push({ entry: name, reason: "unsupported filesystem entry" });
    }
  }
  for (const [index, project] of projects.entries()) {
    if (fs.realpathSync(project) !== project || !fs.statSync(project).isDirectory()) throw new Error("A selected project folder is unavailable.");
    yield* walk(project, `projects/${index}`);
  }
  yield { name: INDEX, bytes: Buffer.from(JSON.stringify({ entries, omissions })) };
}

/** The archive carries file bytes separately. Only the held project tree receives links or modes. */
export function restoreProjects(byName: Map<string, string>, recovery: string) {
  const index = byName.get(INDEX);
  if (!index) return [];
  const value: unknown = JSON.parse(fs.readFileSync(index, "utf8"));
  if (!isObject(value) || !Array.isArray(value.entries) || !Array.isArray(value.omissions) ||
      !value.omissions.every(item => isObject(item) && typeof item.entry === "string" && typeof item.reason === "string")) throw new Error("Invalid project index.");
  const out = new Map<string, ProjectEntry>();
  for (const entry of value.entries) {
    if (!isObject(entry) || typeof entry.name !== "string" || !archivePath(entry.name) ||
        !/^projects\/\d+(?:\/|$)/.test(entry.name) || out.has(entry.name)) throw new Error("Invalid project path.");
    const { name, type, mode, target } = entry;
    if (type === "link" && typeof target === "string" && safeLink(name, target)) out.set(name, { name, type, target });
    else if ((type === "file" || type === "directory") && typeof mode === "number" && Number.isInteger(mode) && mode >= 0 && mode <= 0o777)
      out.set(name, { name, type, mode });
    else throw new Error("Invalid project entry.");
  }
  const folders: { target: string; mode: number }[] = [];
  for (const entry of out.values()) {
    const parent = path.dirname(entry.name);
    if (parent !== "projects" && out.get(parent)?.type !== "directory") throw new Error("Invalid project parent.");
    const target = outputPath(recovery, entry.name.split("/"));
    if (entry.type === "file") {
      if (!byName.has(entry.name) || !stat(target)?.isFile()) throw new Error("Missing project file.");
      const fd = fs.openSync(target, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW);
      try { fs.fchmodSync(fd, entry.mode); fs.fsyncSync(fd); }
      finally { fs.closeSync(fd); }
    } else if (entry.type === "directory") {
      fs.mkdirSync(target, { recursive: true, mode: 0o700 });
      folders.push({ target, mode: entry.mode });
    } else {
      fs.mkdirSync(path.dirname(target), { recursive: true, mode: 0o700 });
      const existing = stat(target);
      if (!existing) fs.symlinkSync(entry.target, target);
      else if (!existing.isSymbolicLink() || fs.readlinkSync(target) !== entry.target) throw new Error("Conflicting recovered project link.");
      syncDurableFile(path.dirname(target), recovery);
    }
  }
  for (const { target, mode } of folders.sort((a, b) => b.target.length - a.target.length)) {
    const fd = fs.openSync(target, "r");
    try { fs.fchmodSync(fd, mode); fs.fsyncSync(fd); }
    finally { fs.closeSync(fd); }
    syncDurableFile(path.dirname(target), recovery);
  }
  return value.omissions as Omission[];
}
