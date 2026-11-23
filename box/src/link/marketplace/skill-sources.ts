// Skill discovery and archiving belong to the computer holding the files.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { Accounts } from "../../agents/accounts";
import { placeWhole, textOf } from "../../shared/files";
import { isObject } from "../../shared/json";
import { readSettings } from "../areas/config";
import { readProjects } from "../areas/projects";
import { isSkillFolder, MARKETPLACE_SKILL, SKILLS_FOLDER, SKILL_TRASH_FOLDER, skillAbout, skillTrashEntry, skillTrashRecord } from "../areas/skills";
import { call, pluginsSetUp } from "../engine/client";
import { APP_FOLDER, homeFolder, K } from "../home";
import type { RouteContext } from "../server/context";

const agentPlaces = [".claude", ".codex", ".agents", ".cursor", ".grok", ".gemini", ".opencode"];
const skillRoots = () => {
  const home = homeFolder();
  const bridge = process.env.KUVO_AGENTS_HOME || path.join(APP_FOLDER ?? K, "agents");
  const accounts = new Accounts(bridge);
  const native = accounts.all().flatMap(account => {
    const folder = accounts.home(account);
    if (account.kind === "antigravity") {
      const profile = folder ? path.join(folder, "profile") : path.join(bridge, "antigravity", "profile");
      return [path.join(profile, "antigravity-cli", "skills")];
    }
    return folder ? [path.join(folder, "skills")] : [];
  });
  const projectPlaces = readProjects().flatMap(project => [project.directory, ...project.folders]).filter(folder => path.isAbsolute(folder));
  const settings = readSettings().settings;
  const places = Array.isArray(settings?.skills) ? settings.skills.flatMap(place =>
    typeof place === "string" && /^(?:\/|~\/)/.test(place) ? [place.replace(/^~(?=\/)/, home)] : []) : [];
  const roots = [
    SKILLS_FOLDER, ...native,
    ...[home, ...projectPlaces].flatMap(folder => agentPlaces.map(agent => path.join(folder, agent, "skills"))),
    ...[process.env.CLAUDE_CONFIG_DIR, process.env.CODEX_HOME, process.env.GROK_HOME].flatMap(folder =>
      folder && path.isAbsolute(folder) ? [path.join(folder, "skills")] : []),
  ];
  return {
    roots: [...new Set(roots.map(folder => path.resolve(folder)))],
    places: places.map(place => path.resolve(place)).filter(place => place !== home && !home.startsWith(place + path.sep) && place !== path.parse(place).root),
  };
};

export interface FolderSkill {
  id: string;
  title: string;
  summary: string;
  folder: string;
  text: string;
}

function readSkill(folder: string): FolderSkill | undefined {
  const dir = fs.lstatSync(folder, { throwIfNoEntry: false });
  // Keep links to skills removable, while leaving their target alone.
  if (!dir?.isDirectory() && !dir?.isSymbolicLink()) return;
  const file = path.join(folder, "SKILL.md");
  const info = fs.statSync(file, { throwIfNoEntry: false });
  if (!info?.isFile() || info.size > 1024 * 1024) return;
  const text = textOf(file);
  if (text === null) return;
  const about = skillAbout(text);
  const id = isSkillFolder(about.name) ? about.name : path.basename(folder);
  return isSkillFolder(id) ? { id, title: about.name || id, summary: about.description ?? "", folder, text } : undefined;
}

export interface BuiltinSkill { id: string; title: string; summary: string }
export const builtinPath = (place: string) => place.startsWith("skill://") || place.startsWith("/builtin/");

// Trust only real skill folders found in agent skill trees. In particular an engine reply
// can't point a destructive action at an arbitrary folder carrying a SKILL.md.
function discoveredFolder(file: string): string | undefined {
  if (!path.isAbsolute(file) || path.basename(file) !== "SKILL.md") return;
  const folder = path.dirname(path.resolve(file));
  const parts = folder.split(path.sep);
  return parts.some((part, index) => part === "skills" && index + 1 < parts.length && agentPlaces.includes(parts[index - 1]!)) ? folder : undefined;
}

export async function engineSkillOffers(context: RouteContext) {
  const builtins: (BuiltinSkill & { text?: string })[] = [];
  const folders: string[] = [];
  const { places } = skillRoots();
  try {
    const engine = context.readEngine();
    const signal = AbortSignal.timeout(5_000);
    await pluginsSetUp(engine, undefined, signal);
    const list = await call(engine, "GET", "/api/skill", undefined, signal);
    if (Array.isArray(list)) for (const entry of list) {
      if (!isSkillFolder(entry?.id) || typeof entry.path !== "string") continue;
      if (builtinPath(entry.path)) builtins.push({
        id: entry.id, title: typeof entry.name === "string" && entry.name ? entry.name : entry.id,
        summary: typeof entry.description === "string" ? entry.description : "",
        text: typeof entry.content === "string" ? entry.content : undefined,
      });
      else {
        const file = path.resolve(entry.path);
        const folder = discoveredFolder(entry.path) ?? (path.isAbsolute(entry.path) && path.basename(file) === "SKILL.md"
          && places.some(place => file.startsWith(place + path.sep)) ? path.dirname(file) : undefined);
        if (folder) folders.push(folder);
      }
    }
  } catch {}
  return { builtins, folders };
}

export function folderSkills(discovered: string[] = [], only?: Set<string>): FolderSkill[] {
  const found = new Map<string, FolderSkill>();
  const seen = new Set<string>();
  const visit = (folder: string, depth: number) => {
    try {
      const canonical = path.join(fs.realpathSync(path.dirname(folder)), path.basename(folder));
      const info = fs.lstatSync(canonical);
      if (seen.has(canonical)) return;
      seen.add(canonical);
      const skill = readSkill(canonical);
      if (skill) {
        if (!only || only.has(skill.id) || only.has(path.basename(canonical))) found.set(canonical, skill);
        return;
      }
      if (!info.isDirectory() || depth === 0) return;
      for (const entry of fs.readdirSync(canonical, { withFileTypes: true })) {
        if (!isSkillFolder(entry.name) || !entry.isDirectory() && !entry.isSymbolicLink()) continue;
        visit(path.join(canonical, entry.name), depth - 1);
      }
    } catch (error) {
      if (!["ENOENT", "ENOTDIR", "EACCES", "ELOOP", "EPERM"].includes((error as NodeJS.ErrnoException).code ?? "")) throw error;
    }
  };
  for (const folder of discovered) visit(folder, 0);
  const { roots, places } = skillRoots();
  for (const place of places) visit(place, 0);
  for (const root of [...new Set([...roots, ...places])]) {
    // If the skills root is a symlink, scan its entries via the configured root.
    try {
      if (!found.has(root) && fs.statSync(root).isDirectory()) {
        for (const name of fs.readdirSync(root).filter(isSkillFolder)) visit(path.join(root, name), 7);
      }
    } catch (error) {
      if (!["ENOENT", "ENOTDIR", "EACCES", "ELOOP", "EPERM"].includes((error as NodeJS.ErrnoException).code ?? "")) throw error;
    }
  }
  return [...found.values()];
}

export interface ArchivedSkill { name: string; key: string; folder: string; managed: boolean }
export function archivedSkill(id: unknown): ArchivedSkill | undefined {
  if (typeof id !== "string" || !skillTrashEntry(id)) return;
  try {
    const record = JSON.parse(fs.readFileSync(skillTrashRecord(id), "utf8"));
    if (isObject(record) && isSkillFolder(record.name) && typeof record.key === "string" && typeof record.folder === "string"
      && path.isAbsolute(record.folder) && typeof record.managed === "boolean") return record as unknown as ArchivedSkill;
  } catch {}
  return undefined;
}

function moveFolder(from: string, to: string): void {
  try { fs.renameSync(from, to); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EXDEV") throw error;
    const staged = path.join(path.dirname(to), `.kuvo-skill-${crypto.randomUUID()}`);
    try {
      fs.cpSync(from, staged, { recursive: true, verbatimSymlinks: true, preserveTimestamps: true });
      fs.renameSync(staged, to);
      fs.rmSync(from, { recursive: true });
    } finally { fs.rmSync(staged, { recursive: true, force: true }); }
  }
}

export function archiveSkill(folder: string, name: string, key: string): void {
  fs.mkdirSync(SKILL_TRASH_FOLDER, { recursive: true, mode: 0o700 });
  let when = Math.floor(Date.now() / 1000);
  let id = `${name}-${when}`;
  while (fs.lstatSync(path.join(SKILL_TRASH_FOLDER, id), { throwIfNoEntry: false }) || fs.existsSync(skillTrashRecord(id))) id = `${name}-${++when}`;
  const record: ArchivedSkill = { folder, name, key, managed: textOf(path.join(folder, MARKETPLACE_SKILL)) !== null };
  placeWhole(skillTrashRecord(id), JSON.stringify(record), 0o600, true);
  const away = path.join(SKILL_TRASH_FOLDER, id);
  try { moveFolder(folder, away); }
  catch (error) {
    if (!fs.lstatSync(away, { throwIfNoEntry: false })) fs.rmSync(skillTrashRecord(id), { force: true });
    throw error;
  }
}

export function restoreArchive(id: string, record: ArchivedSkill): void {
  fs.mkdirSync(path.dirname(record.folder), { recursive: true });
  if (fs.lstatSync(record.folder, { throwIfNoEntry: false })) throw new Error(`A skill folder already exists at ${record.folder}.`);
  moveFolder(path.join(SKILL_TRASH_FOLDER, id), record.folder);
  fs.rmSync(skillTrashRecord(id), { force: true });
}
