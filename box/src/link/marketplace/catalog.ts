import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import embedded from "../../../../catalog/index.json";
import { isObject } from "../../shared/json";
import { placeWhole } from "../../shared/files";
import { IN_TEST_FOLDER, K } from "../home";

export const catalogURL = "https://raw.githubusercontent.com/serhatandic/kuvo-catalog/main/index.json";
export const marketplaceFolder = path.join(K, "box", "marketplace");
const cache = path.join(marketplaceFolder, "catalog-v1.json");
export const itemID = /^[a-z0-9][a-z0-9-]{0,62}$/;
export const sha256 = (value: Uint8Array) => createHash("sha256").update(value).digest("hex");
const maxCatalog = 2 * 1024 * 1024;

interface Card { id: string; title: string; category: string; summary: string }
export type App = Card & { url: string; auth: "open" | "oauth" | "token" | "broker"; logo?: string };
export interface SkillFile { path: string; data: string; sha256: string }
export type CatalogSkill = Card & { version: string; files: SkillFile[] };
export interface Routine { id: string; title: string; every: "1d" | "1w"; note: string }
export type Agent = Card & {
  version: string; instructions: string; apps: string[]; skills: string[];
  suggestedModel: { id: string; providerID: string }; routines: Routine[];
};
interface Catalog { format: 1; apps: App[]; agents: Agent[]; skills: CatalogSkill[] }

export function endpoint(value: unknown): value is string {
  if (typeof value !== "string" || value.length > 1024 || /[{}\s]/.test(value)) return false;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && !url.username && !url.password && !url.search && !url.hash;
  } catch { return false; }
}
export function relativeFile(value: unknown): value is string {
  return typeof value === "string" && value.length <= 240 && value.split("/").every(part =>
    part.length > 0 && !part.startsWith(".") && !/[\\\x00-\x1f\x7f:]/.test(part));
}
const text = (value: unknown, max: number): value is string => typeof value === "string" && !!value.trim() && value.length <= max;
const id = (value: unknown): value is string => typeof value === "string" && itemID.test(value);
const card = (value: unknown): value is Card => isObject(value) && id(value.id)
  && text(value.title, 100) && text(value.category, 50) && text(value.summary, 200);
const distinct = (ids: string[]) => new Set(ids).size === ids.length;
const app = (value: unknown): value is App => card(value) && value.id.length <= 52 && isObject(value) && endpoint(value.url)
  && ["open", "oauth", "token", "broker"].some(kind => kind === value.auth);
function logo(value: unknown): boolean {
  if (!text(value, 170_000)) return false;
  const bytes = Buffer.from(value, "base64");
  return bytes.toString("base64") === value && bytes.subarray(0, 8).equals(Buffer.from("89504e470d0a1a0a", "hex"))
    && bytes.length >= 24 && bytes.readUInt32BE(16) > 0 && bytes.readUInt32BE(16) <= 1024
    && bytes.readUInt32BE(20) > 0 && bytes.readUInt32BE(20) <= 1024;
}
export function skillFiles(value: unknown): value is SkillFile[] {
  if (!Array.isArray(value) || value.length > 100) return false;
  let size = 0;
  const paths = new Set<string>();
  for (const each of value) {
    if (!isObject(each) || !relativeFile(each.path) || typeof each.data !== "string" || each.data.length > 90_000) return false;
    const bytes = Buffer.from(each.data, "base64");
    size += bytes.length;
    if (size > 64 * 1024 || bytes.toString("base64") !== each.data || sha256(bytes) !== each.sha256 || paths.has(each.path)) return false;
    paths.add(each.path);
  }
  return paths.has("SKILL.md") && [...paths].every(file =>
    file.split("/").slice(0, -1).every((_, n, parts) => !paths.has(parts.slice(0, n + 1).join("/"))));
}
const ids = (value: unknown): value is string[] => Array.isArray(value) && value.every(id) && distinct(value);
function parseCatalog(value: unknown): Catalog {
  if (!isObject(value) || value.format !== 1 || !Array.isArray(value.apps) || !Array.isArray(value.skills) || !Array.isArray(value.agents))
    throw new Error("The catalog has an unsupported format.");
  const { apps, skills, agents } = value;
  if (apps.length > 100 || skills.length > 100 || agents.length > 100
    || !apps.every(each => app(each) && logo(each.logo))
    || !skills.every(each => card(each) && isObject(each) && text(each.version, 64) && skillFiles(each.files))
    || !agents.every(each => card(each) && isObject(each) && text(each.version, 64) && text(each.instructions, 8000)
      && isObject(each.suggestedModel) && text(each.suggestedModel.id, 200) && text(each.suggestedModel.providerID, 80)
      && ids(each.apps) && each.apps.every(name => apps.some(entry => entry.id === name))
      && ids(each.skills) && each.skills.every(name => skills.some(entry => entry.id === name))
      && Array.isArray(each.routines) && each.routines.length <= 10 && each.routines.every(routine =>
        isObject(routine) && id(routine.id) && text(routine.title, 100) && text(routine.note, 2000) && ["1d", "1w"].includes(String(routine.every)))
      && distinct(each.routines.map(routine => routine.id)))
    || [apps, agents, skills].some(list => !distinct(list.map(each => each.id)))) throw new Error("The catalog contains invalid items.");
  const entries = { apps, agents, skills } as Pick<Catalog, "apps" | "agents" | "skills">;
  return {
    format: 1,
    apps: entries.apps.map(({ id, title, category, summary, url, auth, logo }) => ({ id, title, category, summary, url, auth, logo })),
    agents: entries.agents.map(({ id, title, category, summary, version, instructions, apps, skills, suggestedModel, routines }) => ({
      id, title, category, summary, version, instructions, apps, skills,
      suggestedModel: { id: suggestedModel.id, providerID: suggestedModel.providerID },
      routines: routines.map(({ id, title, every, note }) => ({ id, title, every, note })),
    })),
    skills: entries.skills.map(({ id, title, category, summary, version, files }) => ({
      id, title, category, summary, version, files: files.map(({ path, data, sha256 }) => ({ path, data, sha256 })),
    })),
  };
}
const snapshot = parseCatalog(embedded);
let cached: Catalog | undefined;
let checked = false;
export function catalog(): Catalog {
  if (!checked) {
    checked = true;
    try { cached = parseCatalog(JSON.parse(fs.readFileSync(cache, "utf8"))); } catch {}
  }
  return cached ?? snapshot;
}
export async function limitedBytes(response: Response, limit: number): Promise<Buffer> {
  const read = response.body?.getReader();
  const chunks: Buffer[] = [];
  let size = 0;
  if (read) {
    try {
      while (true) {
        const next = await read.read();
        if (next.done) break;
        size += next.value.byteLength;
        if (size > limit) throw new Error("The download is too large.");
        chunks.push(Buffer.from(next.value));
      }
    } finally { await read.cancel().catch(() => {}); }
  }
  return Buffer.concat(chunks);
}
let refreshing: Promise<boolean> | undefined;
export function refreshCatalog(): Promise<boolean> {
  const url = (IN_TEST_FOLDER ? process.env.KUVO_TEST_MARKETPLACE_CATALOG : undefined) ?? catalogURL;
  if (!url) return Promise.resolve(false);
  return refreshing ??= (async () => {
    const response = await fetch(url, { redirect: "error", signal: AbortSignal.timeout(15_000) });
    if (!response.ok) throw new Error("The catalog download failed.");
    const bytes = await limitedBytes(response, maxCatalog);
    const next = parseCatalog(JSON.parse(bytes.toString("utf8")));
    fs.mkdirSync(marketplaceFolder, { recursive: true, mode: 0o700 });
    placeWhole(cache, bytes, 0o600, true);
    cached = next;
    checked = true;
    return true;
  })().finally(() => { refreshing = undefined; });
}
