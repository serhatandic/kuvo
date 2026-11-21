import { UnsupportedFormatVersion } from "../../shared/format-version";
import { credentialSeedFormat } from "./command-formats";
// Portable account credentials and integration keys selected when adding a computer.
import crypto from "node:crypto";
import fs from "node:fs";
import { exclusiveWhile } from "../../shared/files";
import path from "node:path";
import { isObject } from "../../shared/json";
import { MAC_ONLY_INTEGRATIONS, keyVariable, portableKeyedIntegrations } from "../../shared/integrations/catalogs";
import { MAC_ONLY_SERVERS } from "../../shared/integrations/servers";
import type { Target } from "../../shared/records";
import { changeSettings, readSettings, valueAt } from "../areas/config";
import { asked, readEngineFile } from "../engine/client";
import { writePrivate } from "../files";
import { K, RUN } from "../home";
import { SSH_FRESH_ADD } from "../marks";
import { ownKey } from "../peers/known";
import { acceptedMembership } from "../setup/membership";
import type { LinkState } from "../state";
import { BOT_KEY, STORED_KEY, keyNames, readKey, writeKey } from "../keys";

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
  sourceID?: string;
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
  integrations: { id: string; disabled?: true }[];
  unavailable: boolean;
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
    credentials: inUse.filter(each => !renewsItself(each)).slice(0, 100).map(each => ({ ...(typeof each.id === "string" ? { sourceID: each.id } : {}), integrationID: each.integrationID as string, label: typeof each.label === "string" ? each.label : "", value: each.value as Record<string, unknown> })),
    renewing: inUse.filter(renewsItself).length,
  };
}

const listed = (answer: unknown): unknown[] => (isObject(answer) && Array.isArray(answer.data) ? answer.data : Array.isArray(answer) ? answer : []);

const KEY_REFERENCE = /^\{file:~\/\.kuvo-code\/box\/keys\/(KUVO_KEY_[A-Z0-9_]+)\}$/;
const SELECTION_ID = /^[A-Za-z0-9_-]{43}$/;
const itemID = (kind: string, ...name: string[]) => crypto.createHash("sha256").update(JSON.stringify([kind, ...name])).digest("base64url");
const accountID = (item: SeedCredential) => itemID("sign-in", item.integrationID, item.sourceID ?? item.label);
const keyID = (name: string) => itemID("key", name);
const isPortableKey = (name: string) => STORED_KEY.test(name) && !BOT_KEY.test(name) && !MAC_ONLY.some(id => name.startsWith(variablePrefix(id)));

export function credentialSelection(value: unknown): string[] | null {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 300 || !value.every(id => typeof id === "string" && SELECTION_ID.test(id))) return null;
  return [...new Set(value)];
}

async function engineSignIns(engine: () => Target) {
  const target = engine();
  const [integrations, credentials] = await Promise.all([asked(target, "GET", "api/integration", undefined, 15_000), asked(target, "GET", "api/credential", undefined, 15_000)]);
  const accounts = new Set(listed(integrations).filter(each => isObject(each) && typeof each.id === "string" && !(isObject(each.metadata) && each.metadata.source === "mcp")).map(each => (each as { id: string }).id));
  return seedCredentials(listed(credentials).filter(isObject), accounts);
}

export async function credentialChecklist(engine: () => Target = readEngineFile) {
  const [available, accounts] = await Promise.allSettled([keyNames(), engineSignIns(engine)]);
  const keys = available.status === "fulfilled" ? available.value.filter(isPortableKey).slice(0, 200) : [];
  const signIns = accounts.status === "fulfilled" ? accounts.value.credentials : [];
  return {
    items: [
      ...signIns.map(each => ({ id: accountID(each), kind: "sign-in" as const, label: each.label || each.integrationID, integration: each.integrationID, checked: true as const })),
      ...keys.map(name => ({ id: keyID(name), kind: "key" as const, label: name, checked: true as const })),
    ],
    renewing: accounts.status === "fulfilled" ? accounts.value.renewing : 0,
    unavailable: [...(available.status === "rejected" ? ["keys"] : []), ...(accounts.status === "rejected" ? ["sign-ins"] : [])],
  };
}

/** A settings file can hold inline tokens or commands containing secrets. Carry the model and
 * simple key-backed local integrations whose environment consists entirely of selected key files. */
export function safeStartingSettings(settings: unknown, keys: ReadonlySet<string>): Record<string, unknown> {
  const source = seedSettings(settings);
  const result: Record<string, unknown> = {};
  if (typeof source.model === "string" && /^[\w.-]+\/[\w.:-]{1,150}$/.test(source.model)) result.model = source.model;
  const servers = isObject(source.mcp) && (isObject(source.mcp.servers) ? source.mcp.servers : source.mcp);
  if (!servers) return result;
  const kept: Record<string, unknown> = Object.fromEntries(Object.entries(servers).flatMap(([name, value]) => {
    if (!isObject(value) || value.type !== "local" || !Array.isArray(value.command) || value.command.length !== 1
      || typeof value.command[0] !== "string" || !/^[\w.-]{1,100}$/.test(value.command[0]) || !isObject(value.environment)) return [];
    const environment = Object.entries(value.environment);
    if (!environment.length || !environment.every(([variable, text]) => /^[A-Z][A-Z0-9_]{0,99}$/.test(variable)
      && typeof text === "string" && keys.has(KEY_REFERENCE.exec(text)?.[1] ?? ""))) return [];
    return [[name, { type: "local", command: value.command, environment: value.environment, ...(value.disabled === true ? { disabled: true } : {}) }]];
  }));
  for (const [id, configuration] of Object.entries(portableKeyedIntegrations(keys))) {
    const original = servers[id];
    if (isObject(original) && !Object.hasOwn(kept, id)) kept[id] = { ...configuration, ...(original.disabled === true ? { disabled: true } : {}) };
  }
  if (Object.keys(kept).length) result.mcp = kept;
  return result;
}

/** Resolve only explicitly selected IDs. Renewing sign-ins and keys for Mac-only integrations
 * remain on the source. A missing or changed credential never opts another one in. */
export async function gatherSeed(selection: readonly string[], engine: () => Target = readEngineFile): Promise<Seed> {
  const selected = new Set(selection);
  const seed: Seed = { settings: {}, keys: {}, credentials: [], renewing: 0, integrations: [], unavailable: false };
  if (selected.size) {
    const [available, accounts] = await Promise.allSettled([keyNames(), engineSignIns(engine)]);
    seed.unavailable = available.status === "rejected" || accounts.status === "rejected";
    if (available.status === "fulfilled") for (const name of available.value.filter(isPortableKey).slice(0, 200)) {
      if (!selected.has(keyID(name))) continue;
      try {
        const value = await readKey(name);
        if (value) seed.keys[name] = value;
      } catch { seed.unavailable = true; }
    }
    if (accounts.status === "fulfilled") {
      seed.credentials = accounts.value.credentials.filter(each => selected.has(accountID(each))).map(({ sourceID: _sourceID, ...credential }) => credential);
      seed.renewing = accounts.value.renewing;
    }
  }
  try {
    const read = readSettings();
    if (!read.problem) {
      const keys = new Set(Object.keys(seed.keys));
      seed.settings = safeStartingSettings(read.settings, keys);
      const configured = isObject(seed.settings.mcp) ? seed.settings.mcp : {};
      seed.integrations = Object.keys(portableKeyedIntegrations(keys)).flatMap(id => {
        const value = configured[id];
        return isObject(value) ? [{ id, ...(value.disabled === true ? { disabled: true as const } : {}) }] : [];
      });
    }
  } catch {}
  return seed;
}

const sha256 = (value: unknown) => crypto.createHash("sha256").update(JSON.stringify(value)).digest("hex");
export const sshCopyScope = (setup: string | undefined, issuer: string) => `v1 ${sha256([setup, issuer])} `;
export const sshCopySelection = (selection: readonly string[]) => sha256([...selection].sort());

/** A signed accepted membership ends SSH recovery on the member, even if the SSH client left. */
export function finishSshCredentialCopy(link: LinkState): void {
  if (link.setup?.role !== "member") return;
  const file = path.join(RUN, SSH_FRESH_ADD);
  try {
    if (!fs.lstatSync(file).isFile() || !fs.readFileSync(file, "utf8").startsWith(sshCopyScope(link.setup.id, link.setup.primary))) return;
    const key = ownKey(link);
    if (acceptedMembership(link)?.boxes.some(box => box.key === key)) fs.unlinkSync(file);
  } catch {}
}

export function withCredentialCopyLock<T>(work: () => Promise<T>): Promise<T> {
  // The lock survives until the operation finishes, even when a wipe deletes the box folder.
  return exclusiveWhile(path.resolve(K) + "-credential-copy.lock", work);
}

async function takeKeys(given: Record<string, unknown>): Promise<number> {
  let kept = 0;
  for (const [name, value] of Object.entries(given).slice(0, 200)) {
    if (!isPortableKey(name) || typeof value !== "string") throw new Error("That key could not be copied.");
    if (await readKey(name) === null) await writeKey(name, value);
    kept++;
  }
  return kept;
}

function takeIntegrations(integrations: unknown, keys: Record<string, unknown>): boolean {
  if (!Array.isArray(integrations) || !integrations.length) return true;
  const selected = new Map(integrations.filter(isObject).map(item => [item.id, item.disabled === true]));
  const catalog = portableKeyedIntegrations(new Set(Object.keys(keys)));
  const current = readSettings();
  if (current.problem) return false;
  const root = isObject(valueAt(current.settings, ["mcp", "servers"])) ? ["mcp", "servers"] : ["mcp"];
  const changes = Object.entries(catalog).flatMap(([id, value]) => {
    const path = [...root, id];
    return selected.has(id) && valueAt(current.settings, path) === undefined ? [{ path, value: { ...value, ...(selected.get(id) ? { disabled: true } : {}) }, was: null }] : [];
  });
  return !changes.length || changeSettings(changes).status === 200;
}

export async function applyInstallerSeed(given: unknown): Promise<boolean> {
  if (!isObject(given)) return false;
  const seed = credentialSeedFormat.read(given);
  return isObject(seed.keys) && Array.isArray(seed.credentials) && (await takeSelected(seed, readEngineFile, false)).status === 200;
}

/** Where a computer notes it took the accounts it was added with (once: never again after). */
export const SEEDED_FILE = path.join(RUN, "seeded.json");

/** `kuvo-link.js box seed`, on the computer added, its input as it came (JSON, `{credentials}`): each
 * sign-in stored in its engine as the one in use, unless it has one for that account already (its
 * own stays). Once: a computer that took them before takes none. What it answers says how many,
 * never what. */
export async function takeSeed(input: string, engine: () => Target = readEngineFile, prepare?: () => void): Promise<{ status: number; taken?: number; keys?: number; message?: string }> {
  let given: unknown = null;
  try { given = JSON.parse(input); } catch {}
  if (isObject(given)) {
    try { given = credentialSeedFormat.read(given); }
    catch (error) {
      if (!(error instanceof UnsupportedFormatVersion)) throw error;
      return { status: 400, message: "Unsupported Box seed format. " + error.message };
    }
  }
  if (isObject(given) && Array.isArray(given.credentials)) prepare?.();
  return takeSelected(given, engine, true);
}

async function takeSelected(given: unknown, engine: () => Target, once: boolean): Promise<{ status: number; taken?: number; keys?: number; message?: string }> {
  if (!isObject(given) || !Array.isArray(given.credentials)) return { status: 400, message: "box seed takes JSON on its standard input: {credentials: [{integrationID, label, value}]}." };
  const credentials = given.credentials.slice(0, 100);
  let keys: number | undefined;
  if (isObject(given.keys)) {
    try {
      keys = await takeKeys(given.keys);
      if (!takeIntegrations(given.integrations, given.keys)) return { status: 503, message: "The selected integrations could not be saved." };
    }
    catch { return { status: 503, message: "The selected keys could not be saved." }; }
  }
  const keyCount = keys === undefined ? {} : { keys };
  if (!credentials.length || once && fs.existsSync(SEEDED_FILE)) return { status: 200, taken: 0, ...keyCount };
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
    } catch { return { status: 503, taken, message: "Some sign-ins could not be saved. Try again." }; }
  }
  if (once) try { writePrivate(SEEDED_FILE, JSON.stringify({ at: Date.now(), taken })); } catch {}
  return { status: 200, taken, ...keyCount };
}
