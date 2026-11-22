import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { locked, placeWhole } from "../../shared/files";
import { MARKETPLACE_SKILL, SKILLS_FOLDER, skillSource, writeSkillPackage, type SkillPackage } from "../areas/skills";
import { textOf, listOf } from "../files";
import { changeShared, sharedValue } from "../setup/shared";
import { readLink } from "../state";
import type { Acted } from "../work-queue/forward";
import { endpoint, itemID, marketplaceFolder, sha256, skillFiles, type App } from "./catalog";

export interface InstalledSkill extends SkillPackage {
  origin: "catalog" | "github" | "folder" | "created";
  version?: string;
}
export type InstalledApp = Omit<App, "logo"> & { origin: "catalog" | "custom" };
export interface Installations { format: 1; apps: InstalledApp[]; skills: InstalledSkill[] }
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
function read(value: unknown): Installations {
  if (value === undefined) return empty();
  if (!isObject(value) || value.format !== 1 || !Array.isArray(value.apps) || !Array.isArray(value.skills)
    || !value.apps.every(installedApp) || !value.skills.every(installedSkill)) throw new Error("The saved Marketplace data could not be read.");
  return { format: 1, apps: value.apps, skills: value.skills };
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

export function projectSkills(state = installations()): string[] {
  fs.mkdirSync(marketplaceFolder, { recursive: true, mode: 0o700 });
  return locked(skillLock, () => {
    const problems: string[] = [];
    const wanted = new Set(state.skills.map(each => each.name));
    for (const name of listOf(SKILLS_FOLDER)) {
      const folder = path.join(SKILLS_FOLDER, name);
      if (!wanted.has(name) && itemID.test(name) && fs.lstatSync(folder).isDirectory()
        && textOf(path.join(folder, MARKETPLACE_SKILL)) !== null) {
        try { fs.rmSync(folder, { recursive: true }); }
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
