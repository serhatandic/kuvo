// What a computer newly added starts with (PROTOCOL.md, "Settings per box"; "Messages in a
// session", Plan accounts): a copy of this box's engine settings and integrations' keys, where it
// has none of its own; and of its accounts' keys and the sign-ins that stay the same (GitHub
// Copilot's), once, as it's added. Never a sign-in that renews itself (ChatGPT's, GitLab's): a
// service hands a new refresh token to whichever computer renews first and refuses the other, so
// each computer signs in to those itself. From then on what's on the computer is its own.
// The box that adds gathers it (`gatherSeed`) and gives it over SSH on a script's standard input,
// never in its arguments or in anything said of the try; the computer added takes it itself
// (`kuvo-link.js box adopt`, as before, for its settings and keys; `box seed`, `takeSeed`, for its
// accounts), in files and an engine only you can read there.
// Platform: the same on macOS and Linux (a Mac's keys come from its Keychain through its helper,
// a machine's from its files: `keys.ts`).

import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { MAC_ONLY_INTEGRATIONS, keyVariable } from "../../shared/integrations/catalogs";
import { MAC_ONLY_SERVERS } from "../../shared/integrations/servers";
import type { Target } from "../../shared/records";
import { readSettings } from "../areas/config";
import { asked, readEngineFile } from "../engine/client";
import { writePrivate } from "../files";
import { RUN } from "../home";
import { BOT_KEY, STORED_KEY, keyNames, readKey } from "../keys";

/** One of the engine's sign-ins as it keeps it (`api/credential`): for which account, its label,
 * whether it's the one in use, and what it signs in with (a key, or an OAuth sign-in). */
interface Credential {
  id?: unknown;
  integrationID?: unknown;
  label?: unknown;
  active?: unknown;
  value?: unknown;
}

/** A sign-in given to a computer as it's added. */
export interface SeedCredential {
  integrationID: string;
  label: string;
  value: Record<string, unknown>;
}

/** What a new computer starts with: for `box adopt` (`settings`, `keys`) and `box seed` (`credentials`). */
export interface Seed {
  settings: Record<string, unknown>;
  keys: Record<string, string>;
  credentials: SeedCredential[];
  /** How many of this box's sign-ins renew themselves, and so weren't taken. */
  renewing: number;
}

/** A sign-in that gets a new refresh token each time it's renewed, the old one then refused: two
 * computers can't share one. GitHub Copilot's keeps its token. */
export const renewsItself = (credential: { integrationID?: unknown; value?: unknown }): boolean =>
  isObject(credential.value) && typeof credential.value.refresh === "string" && credential.value.refresh.length > 0 && credential.integrationID !== "github-copilot";

const MAC_ONLY = [...MAC_ONLY_INTEGRATIONS, ...MAC_ONLY_SERVERS];
/** A key's variable begins so for an integration (`KUVO_KEY_<ID>_<NAME>`). */
const variablePrefix = (integration: string): string => keyVariable(integration, "");

// A key's file named in settings, wherever this box keeps its keys, as the computer added keeps its own.
const theirKeyPaths = (value: unknown): unknown => {
  if (typeof value === "string") return value.replace(/\{file:[^{}]*\/(KUVO_KEY_[A-Za-z0-9_]+)\}/g, "{file:~/.kuvo-code/box/keys/$1}");
  if (Array.isArray(value)) return value.map(theirKeyPaths);
  if (isObject(value)) return Object.fromEntries(Object.entries(value).map(([key, inner]) => [key, theirKeyPaths(inner)]));
  return value;
};

/** This box's settings as another computer could start with them: not what names this computer
 * alone (its shell, its worktrees' folder), nor the integrations that use a Mac's own apps. */
export function seedSettings(settings: unknown): Record<string, unknown> {
  if (!isObject(settings)) return {};
  const { $schema: _schema, shell: _shell, worktree: _worktree, ...rest } = settings as Record<string, unknown>;
  if (isObject(rest.mcp)) {
    const without = (servers: Record<string, unknown>) => Object.fromEntries(Object.entries(servers).filter(([id]) => !MAC_ONLY.includes(id)));
    rest.mcp = isObject(rest.mcp.servers) ? { ...rest.mcp, servers: without(rest.mcp.servers) } : without(rest.mcp);
  }
  return theirKeyPaths(rest) as Record<string, unknown>;
}

/** Of the engine's sign-ins, those a new computer starts with: each account's one in use (never an
 * integration's server's, which signs in on each computer), a key or a sign-in that stays the same. */
export function seedCredentials(credentials: readonly Credential[], accounts: ReadonlySet<string>): { credentials: SeedCredential[]; renewing: number } {
  const inUse = credentials.filter(each => each.active === true && typeof each.integrationID === "string" && accounts.has(each.integrationID) && isObject(each.value));
  return {
    credentials: inUse.filter(each => !renewsItself(each)).slice(0, 100).map(each => ({ integrationID: each.integrationID as string, label: typeof each.label === "string" ? each.label : "", value: each.value as Record<string, unknown> })),
    renewing: inUse.filter(renewsItself).length,
  };
}

const listed = (answer: unknown): unknown[] => (isObject(answer) && Array.isArray(answer.data) ? answer.data : Array.isArray(answer) ? answer : []);

/** What this box gives a computer it adds. Each part is what could be read now: one that
 * couldn't (its engine not answering, its keys not reachable) is left out, and the computer added
 * is set up all the same, to be signed in there. */
export async function gatherSeed(engine: () => Target = readEngineFile): Promise<Seed> {
  const seed: Seed = { settings: {}, keys: {}, credentials: [], renewing: 0 };
  try {
    const read = readSettings();
    if (!read.problem) seed.settings = seedSettings(read.settings);
  } catch {}
  try {
    const mine = MAC_ONLY.map(variablePrefix);
    for (const name of (await keyNames()).slice(0, 200)) {
      if (!STORED_KEY.test(name) || BOT_KEY.test(name) || mine.some(prefix => name.startsWith(prefix))) continue;
      const value = await readKey(name);
      if (value) seed.keys[name] = value;
    }
  } catch {}
  try {
    const target = engine();
    const [integrations, credentials] = await Promise.all([asked(target, "GET", "api/integration", undefined, 15_000), asked(target, "GET", "api/credential", undefined, 15_000)]);
    const accounts = new Set(listed(integrations).filter(each => isObject(each) && typeof each.id === "string" && !(isObject(each.metadata) && each.metadata.source === "mcp")).map(each => (each as { id: string }).id));
    Object.assign(seed, seedCredentials(listed(credentials) as Credential[], accounts));
  } catch {}
  return seed;
}

/** Whether there's anything in a seed to give. */
export const seedIsEmpty = (seed: Seed): boolean => !Object.keys(seed.settings).length && !Object.keys(seed.keys).length && !seed.credentials.length;

/** Where a computer notes it took the accounts it was added with (once: never again after). */
export const SEEDED_FILE = path.join(RUN, "seeded.json");

/** `kuvo-link.js box seed`, on the computer added, its input as it came (JSON, `{credentials}`): each
 * sign-in stored in its engine as the one in use, unless it has one for that account already (its
 * own stays). Once: a computer that took them before takes none. What it answers says how many,
 * never what. */
export async function takeSeed(input: string, engine: () => Target = readEngineFile): Promise<{ status: number; taken?: number; message?: string }> {
  if (fs.existsSync(SEEDED_FILE)) return { status: 200, taken: 0 };
  let given: unknown = null;
  try { given = JSON.parse(input); } catch {}
  const credentials = isObject(given) && Array.isArray(given.credentials) ? given.credentials.slice(0, 100) : null;
  if (!credentials) return { status: 400, message: "box seed takes JSON on its standard input: {credentials: [{integrationID, label, value}]}." };
  let target: Target;
  let has: Set<unknown>;
  try {
    target = engine();
    has = new Set(listed(await asked(target, "GET", "api/credential", undefined, 15_000)).map(each => (isObject(each) ? each.integrationID : undefined)));
  } catch {
    return { status: 503, message: "Its engine isn't answering yet." };
  }
  let taken = 0;
  for (const each of credentials) {
    if (!isObject(each) || typeof each.integrationID !== "string" || !/^[A-Za-z0-9._-]{1,100}$/.test(each.integrationID) || !isObject(each.value) || has.has(each.integrationID) || renewsItself(each)) continue;
    try {
      await asked(target, "POST", "api/credential", { integrationID: each.integrationID, label: typeof each.label === "string" ? each.label.slice(0, 200) : "", value: each.value, activate: true }, 15_000);
      has.add(each.integrationID);
      taken++;
    } catch {}
  }
  try { writePrivate(SEEDED_FILE, JSON.stringify({ at: Date.now(), taken })); } catch {}
  return { status: 200, taken };
}
