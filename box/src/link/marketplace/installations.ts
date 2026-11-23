import fs from "node:fs";
import path from "node:path";
import { removedSkillsFile } from "../../shared/marketplace";
import { isObject } from "../../shared/json";
import { locked, placeWhole } from "../../shared/files";
import { isSkillFolder, MARKETPLACE_SKILL, SKILLS_FOLDER, SKILL_TRASH_FOLDER, skillSource, writeSkillPackage, type SkillPackage } from "../areas/skills";
import { textOf, listOf } from "../files";
import { changeShared, sharedValue } from "../setup/shared";
import { K } from "../home";
import { archiveSkill, archivedSkill, folderSkills, restoreArchive, type BuiltinSkill } from "./skill-sources";
import { readLink } from "../state";
import type { Acted } from "../work-queue/forward";
import { endpoint, itemID, marketplaceFolder, sha256, skillFiles, type App } from "./catalog";

export interface InstalledSkill extends SkillPackage {
  origin: "catalog" | "github" | "folder" | "created";
  version?: string;
}
export type InstalledApp = Omit<App, "logo"> & { origin: "catalog" | "custom" };
export interface RemovedSkill {
  name: string;
  key: string;
  restored?: true;
  builtin?: BuiltinSkill;
  managed?: { origin: InstalledSkill["origin"]; version?: string; source?: InstalledSkill["source"] };
}
export interface Installations { format: 1; apps: InstalledApp[]; skills: InstalledSkill[]; removedSkills?: RemovedSkill[] }
export const removalFor = (state: Installations, name: string) => state.removedSkills?.find(each => each.name === name && !each.restored);
const empty = (): Installations => ({ format: 1, apps: [], skills: [] });
const standalone = path.join(marketplaceFolder, "standalone-v1.json");
const skillLock = path.join(marketplaceFolder, "skills.lock");
const maxBytes = 110 * 1024;
const text = (value: unknown, limit: number): value is string =>
  typeof value === "string" && value.trim().length > 0 && value.length <= limit;
const validID = (value: unknown): value is string => typeof value === "string" && itemID.test(value);
export const no = (status: number, message: string): Acted => ({ status, value: { message } });

export function installedApp(value: unknown): value is InstalledApp {
  return isObject(value) && validID(value.id) && value.id.length <= 52 && text(value.title, 100) && text(value.category, 50)
    && text(value.summary, 200) && endpoint(value.url) && ["open", "token", "oauth"].includes(String(value.auth))
    && (value.origin === "catalog" || value.origin === "custom");
}
export function installedSkill(value: unknown): value is InstalledSkill {
  return isObject(value) && validID(value.name) && skillFiles(value.files)
    && (value.origin === "github" ? skillSource(value.source) : ["catalog", "folder", "created"].includes(String(value.origin)) && value.source === undefined)
    && (value.origin !== "catalog" || text(value.version, 64));
}
function removedSkill(value: unknown): value is RemovedSkill {
  return isObject(value) && isSkillFolder(value.name) && typeof value.key === "string" && /^[a-f0-9-]{36}$/.test(value.key)
    && (value.restored === undefined || value.restored === true)
    && (value.builtin === undefined || isObject(value.builtin) && value.builtin.id === value.name
      && text(value.builtin.title, 1024) && typeof value.builtin.summary === "string" && value.builtin.summary.length <= 4096)
    && (value.managed === undefined || isObject(value.managed)
      && (value.managed.origin === "github" ? skillSource(value.managed.source) : ["catalog", "folder", "created"].includes(String(value.managed.origin)))
      && (value.managed.origin !== "catalog" || text(value.managed.version, 64)));
}

function read(value: unknown): Installations {
  if (value === undefined) return empty();
  if (!isObject(value) || value.format !== 1 || !Array.isArray(value.apps) || !Array.isArray(value.skills)
    || !value.apps.every(installedApp) || !value.skills.every(installedSkill)
    || (value.removedSkills !== undefined && (!Array.isArray(value.removedSkills) || !value.removedSkills.every(removedSkill)))) throw new Error("The saved Marketplace data could not be read.");
  return { format: 1, apps: value.apps, skills: value.skills, ...(value.removedSkills ? { removedSkills: value.removedSkills as RemovedSkill[] } : {}) };
}
export function installations(link = readLink()): Installations {
  if (link?.setup) return read(sharedValue(link, "marketplaceInstallations"));
  try { return read(JSON.parse(fs.readFileSync(standalone, "utf8"))); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return empty();
    throw error;
  }
}
export const skillRevision = (skill: InstalledSkill) => sha256(Buffer.from(JSON.stringify(skill)));

export function saveInstallations(change: (before: Installations) => Installations | Acted): Acted {
  let result = no(503, "The main computer couldn’t save the Marketplace change.");
  const next = (before: Installations) => {
    const after = change(before);
    if ("status" in after) { result = after; return; }
    if (Buffer.byteLength(JSON.stringify(after)) > maxBytes) {
      result = no(413, "Your setup’s Marketplace storage is full. Remove a skill before adding another.");
      return;
    }
    return after;
  };
  if (!readLink()?.setup) {
    fs.mkdirSync(marketplaceFolder, { recursive: true, mode: 0o700 });
    return locked(path.join(marketplaceFolder, "installations.lock"), () => {
      if (readLink()?.setup) return no(409, "Your setup changed. Try again.");
      const after = next(installations(null));
      if (!after) return result;
      placeWhole(standalone, JSON.stringify(after), 0o600, true);
      return { status: 200, value: { installations: after } };
    }, { required: true });
  }
  let after: Installations | undefined;
  const receipt = changeShared("marketplaceInstallations", value => after = next(read(value)));
  return receipt && after ? { status: 200, value: { installations: after, receipt } } : result;
}

export function projectSkills(discovered: string[] = []): string[] {
  fs.mkdirSync(marketplaceFolder, { recursive: true, mode: 0o700 });
  return locked(skillLock, () => {
    const state = installations();
    const problems: string[] = [];
    const removals = new Map(state.removedSkills?.filter(each => !each.restored).map(each => [each.name, each]));
    const names = JSON.stringify([...removals.keys()].sort());
    const blocked = removedSkillsFile(K);
    if ((removals.size || fs.existsSync(blocked)) && textOf(blocked) !== names) placeWhole(blocked, names, 0o600, true);

    if (state.removedSkills?.some(each => each.restored)) {
      const restored = new Map(state.removedSkills.filter(each => each.restored).map(each => [each.key, each]));
      // If sync created more than one copy, restore the newest one at each original path.
      const archives = listOf(SKILL_TRASH_FOLDER).map(id => ({ id, record: archivedSkill(id) }))
        .sort((a, b) => b.id.localeCompare(a.id));
      const returned = new Set<string>();
      for (const { id, record } of archives) {
        if (!record || !restored.has(record.key) || returned.has(record.folder)) continue;
        returned.add(record.folder);
        try {
          const skill = record.managed && state.skills.find(each => each.name === record.name);
          if (skill) placeWhole(path.join(SKILL_TRASH_FOLDER, id, MARKETPLACE_SKILL), skillRevision(skill), 0o600, true);
          restoreArchive(id, record);
        } catch { problems.push(`Couldn’t restore ${record.name} on this computer.`); }
      }
    }

    if (removals.size) for (const skill of folderSkills(discovered, new Set(removals.keys()))) {
      const removed = removals.get(skill.id) ?? removals.get(path.basename(skill.folder));
      if (!removed) continue;
      try { archiveSkill(skill.folder, removed.name, removed.key); }
      catch { problems.push(`Couldn’t move ${skill.title} to Recently Deleted on this computer.`); }
    }
    const wanted = new Set(state.skills.map(each => each.name));
    for (const name of listOf(SKILLS_FOLDER)) {
      const folder = path.join(SKILLS_FOLDER, name);
      if (!wanted.has(name) && itemID.test(name) && fs.lstatSync(folder).isDirectory()
        && textOf(path.join(folder, MARKETPLACE_SKILL)) !== null) {
        try {
          const removal = removals.get(name);
          if (removal) archiveSkill(folder, name, removal.key);
          else fs.rmSync(folder, { recursive: true });
        }
        catch { problems.push(`Couldn’t remove ${name} on this computer.`); }
      }
    }
    for (const skill of state.skills) {
      const marker = path.join(SKILLS_FOLDER, skill.name, MARKETPLACE_SKILL);
      const revision = skillRevision(skill);
      if (textOf(marker) === revision) continue;
      try { writeSkillPackage(skill, revision); }
      catch (error) { problems.push(error instanceof Error ? error.message : String(error)); }
    }
    return problems;
  }, { required: true });
}
