// The scope carried with a Marketplace bot's workspace, read by the box and coding agents.
import fs from "node:fs";
import path from "node:path";

export const MARKETPLACE_SERVER_PREFIX = "marketplace-";
export const MARKETPLACE_SCOPE = ".kuvo-marketplace.json";
export const MARKETPLACE_SKILL_MARKER = ".kuvo-marketplace.json";

export interface MarketplaceScope {
  agent: string;
  version: string;
  request: string;
  apps: string[];
  skills: string[];
}
export function marketplaceScope(directory: string): MarketplaceScope | undefined {
  try {
    const value = JSON.parse(fs.readFileSync(path.join(directory, MARKETPLACE_SCOPE), "utf8"));
    if (typeof value?.request === "string" && typeof value.agent === "string" && typeof value.version === "string"
      && Array.isArray(value.apps) && value.apps.every((app: unknown) => typeof app === "string")
      && Array.isArray(value.skills) && value.skills.every((skill: unknown) => typeof skill === "string")) return value;
  } catch {}
  return undefined;
}
