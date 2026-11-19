import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { DATA_DECLARATIONS } from "../../shared/data-declarations";
import { backupFolderVersion } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { swapUnderWay } from "../../shared/engine-swap";
import { updateUnderWay } from "../../shared/updating";
import { held, ownSign } from "../documents/format";
import { exclusiveWhile, privateFolder, writeDurablePrivate } from "../files";
import { K, OWN_RUNTIME, RUN, homeFolder, located } from "../home";
import { ownerOf } from "../ownership/resolve";
import { ownKey, peersOf } from "../peers/known";
import { leadershipDocuments, resolveCheckpoint } from "../setup/checkpoint";
import { followedSetup } from "../setup/record";
import { readLink, updateLink } from "../state";
import { authenticate, archiveContext, newRecoveryKey, seal, type Entry } from "./archive";
import { destinationStore, expiredPoints, separateDisk, verifyBackupComputer, type Destination } from "./destination";
import { scrubEngine } from "./sqlite";
import { durableFiles, inside, roots, tree } from "./locations";

export const BACKUP = path.join(K, "backup");
const CONFIG = path.join(BACKUP, "configuration.json");
export type Reason = "daily" | "move" | "wipe" | "update";
export const pointId = (reason: Reason | "vault", now: number) =>
  `${new Date(now).toISOString().replaceAll(":", "")}-${reason}-${crypto.randomBytes(8).toString("hex")}`;

export interface RestorePoint {
  id: string;
  at: number;
  version: number;
}
interface Config {
  version: 1;
  enabled: boolean;
  source: string;
  key: string;
  dest: Destination;
  disk?: number;
  projects: string[];
  last?: RestorePoint;
  lastDaily?: RestorePoint;
  error?: string;
}
export interface Manifest extends RestorePoint {
  format: 1;
  source: string;
  origin: string | null;
  signer: string | null;
  setup: string | null;
  paired: boolean;
  projects: string[];
}

export const bytes = (name: string, value: unknown): Entry => ({ name, bytes: Buffer.from(JSON.stringify(value)) });
export const backupLock = <T>(work: () => Promise<T>) => exclusiveWhile(path.join(BACKUP, ".operation"), work);
export const scratchFolder = () => fs.mkdtempSync(path.join(privateFolder(BACKUP), ".scratch-"));

export function config(): Config | null {
  try {
    const value = JSON.parse(fs.readFileSync(CONFIG, "utf8"));
    if (value?.version !== 1) throw new Error("Unsupported backup configuration.");
    return value;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return null;
  }
}

const save = (value: Config) => writeDurablePrivate(CONFIG, JSON.stringify(value), K);

function checkComputer(dest: Destination) {
  const link = readLink();
  if (dest.kind === "computer" && !peersOf(link).some(peer => peer.key === dest.computer)) {
    throw new Error("Choose a current member of this setup for backups.");
  }
}

export function selectedProjects(given: unknown): string[] {
  if (given === undefined) return [];
  if (!Array.isArray(given) || given.length > 100) throw new Error("Choose up to 100 project folders.");
  return [...new Set(given.map(folder => {
    if (typeof folder !== "string" || !path.isAbsolute(folder)) throw new Error("Choose project folders by their absolute paths.");
    const real = fs.realpathSync(folder);
    const protectedRoots = [...Object.values(roots), homeFolder(), os.tmpdir()].filter((root): root is string => Boolean(root)).map(located);
    if (!fs.statSync(real).isDirectory() || protectedRoots.some(root => inside(real, root)) ||
      Object.values(roots).some(root => root && inside(located(root), real))) throw new Error("Choose individual project folders outside Kuvo’s data.");
    return real;
  }))];
}

export function configure(dest: Destination, projects: string[]) {
  return backupLock(async () => {
    checkComputer(dest);
    if (dest.kind === "disk" && !separateDisk(dest.path)) throw new Error("Choose a folder on a separate disk.");
    if (dest.kind === "disk" && projects.some(folder => inside(folder, dest.path) || inside(dest.path, folder))) {
      throw new Error("Choose a backup folder separate from the selected projects.");
    }
    const previous = config();
    const same = previous && JSON.stringify(previous.dest) === JSON.stringify(dest);
    const value: Config = {
      version: 1, enabled: true, source: previous?.source ?? "b_" + crypto.randomBytes(16).toString("base64url"),
      key: previous?.key ?? newRecoveryKey(), dest, projects,
      ...(dest.kind === "disk" ? { disk: fs.statSync(dest.path).dev } : {}),
      ...(same ? { last: previous.last, lastDaily: previous.lastDaily } : {}),
    };
    if (dest.kind === "computer") await verifyBackupComputer(dest);
    // A failed destination check leaves the old configuration in place.
    await destinationStore(dest, value.source).list();
    save(value);
    return { source: value.source, recoveryKey: value.key };
  });
}

export function disable() {
  return backupLock(async () => {
    const kept = config();
    if (kept) save({ ...kept, enabled: false });
  });
}

export function backupStatus() {
  const kept = config();
  const link = readLink();
  const computers = peersOf(link).map(({ key, name, id }) => ({ key, name, ...(id ? { ssh: id } : {}) }));
  let destinationAvailable: boolean | null = null;
  if (kept?.dest.kind === "disk") {
    try { destinationAvailable = separateDisk(kept.dest.path) && fs.statSync(kept.dest.path).dev === kept.disk; }
    catch { destinationAvailable = false; }
  }
  const dest = kept?.dest;
  if (dest?.kind === "computer") destinationAvailable = computers.some(computer => computer.key === dest.computer);
  return {
    computer: link && ownKey(link), enabled: kept?.enabled ?? false,
    computers, defaultDestination: computers.length ? "computer" : "disk",
    destination: kept?.dest ?? null, source: kept?.source ?? null, projects: kept?.projects ?? [],
    destinationAvailable, lastVerifiedRestorePoint: kept?.last ?? null,
    ...(kept?.error ? { error: kept.error } : {}),
    ...(!kept?.last ? { restorePointStatus: "no separate backup" } : {}),
  };
}

function layout(insideUpdate: boolean): number {
  if (fs.existsSync(path.join(RUN, "update-trial")) || fs.existsSync(path.join(RUN, "update-restore")) ||
    swapUnderWay(RUN) || (!insideUpdate && updateUnderWay(RUN))) {
    throw new Error("Wait for the update to finish before backing up.");
  }
  return backupFolderVersion(K);
}

function acceptedBoundary() {
  let result: { origin: string | null; signer: string | null; setup: string | null; paired: boolean; entries: Entry[]; link: ReturnType<typeof readLink> } | undefined;
  updateLink(link => {
    const setup = followedSetup(link);
    const origin = ownKey(link ?? {});
    const entries: Entry[] = [];
    if (setup?.role === "primary" && DATA_DECLARATIONS.setupLeadership.backup && DATA_DECLARATIONS.setupReplica.backup) {
      const docs = new Map<string, unknown>();
      for (const envelope of Object.values(link?.leadership ?? {})) {
        const certificate = held(envelope);
        if (!certificate || (certificate.body.checkpoint !== undefined && !resolveCheckpoint(certificate))) {
          throw new Error("The accepted setup checkpoint is incomplete.");
        }
        for (const doc of leadershipDocuments(envelope)) docs.set(doc.d, doc);
      }
      for (const envelope of Object.values(link?.docs ?? {})) {
        const doc = held(envelope);
        if (doc?.origin === ownSign(link) && (doc.kind === "peers" || doc.kind === "personalities" || doc.kind === "setup-shared")) docs.set(envelope.d, envelope);
      }
      const shared = link?.sharedSetup === setup.id ? Object.fromEntries(Object.entries(link?.shared ?? {})
        .filter(([kind]) => DATA_DECLARATIONS[kind as keyof typeof DATA_DECLARATIONS]?.backup)) : {};
      entries.push(bytes("meta/accepted.json", { setup, shared, outcomes: link?.sharedOutcomes, history: [...docs.values()] }));
    }
    result = { origin, signer: ownSign(link), setup: setup?.id ?? null, paired: peersOf(link).length > 0 || setup?.role === "member", entries, link };
  });
  return result!;
}

function snapshotDatabase(file: string, copy: string): void {
  const query = "VACUUM INTO '" + copy.replaceAll("'", "''") + "'";
  const result = process.versions.bun
    ? spawnSync(process.execPath, [...OWN_RUNTIME, "-e",
      'const db = new (require("bun:sqlite").Database)(process.argv[1], { readonly: true }); db.run("PRAGMA busy_timeout=5000"); db.run(process.argv[2]);',
      file, query], { env: { ...process.env, BUN_BE_BUN: "1" }, timeout: 60_000 })
    : spawnSync("sqlite3", ["-readonly", "-cmd", ".timeout 5000", file, query], { timeout: 60_000 });
  if (result.status !== 0) throw new Error("Could not take a consistent database snapshot.");
}

export function captureBackup(file: string, key: string, source: string, id: string, now: number,
  projects: readonly string[], scratch: string, insideUpdate = false): Manifest {
  const version = layout(insideUpdate);
  const boundary = acceptedBoundary();
  const manifest: Manifest = { format: 1, source, id, at: now, version, origin: boundary.origin, signer: boundary.signer, setup: boundary.setup,
    paired: boundary.paired, projects: projects.map(project => path.basename(project)) };
  const link = boundary.link;
  function* entries(): Generator<Entry> {
    yield bytes("meta/manifest.json", manifest);
    yield* boundary.entries;
    let db = 0;
    for (const item of durableFiles()) {
      const [root, id] = item.name.split("/");
      if (["bots", "wakes", "groups"].includes(root) || item.name.startsWith("agents/sessions/")) {
        const kind = root === "bots" ? "botSettings" : root === "groups" ? "groupSettings" : "conversations";
        const record = root === "bots" && id.endsWith(".json") ? undefined : root === "bots" || root === "groups" || root === "wakes" ? id : item.name.split("/")[2];
        if (record && ownerOf({ kind, id: record }, { link, localRecord: true })?.local === false) continue;
      }
      if (item.name.startsWith("engine/") && item.name.endsWith(".db") || item.name === "agents/usage/insights.sqlite") {
        const copy = path.join(scratch, `db-${db++}`);
        snapshotDatabase(item.file, copy);
        if (root === "engine") scrubEngine(copy, [], false);
        yield { name: item.name, file: copy };
        fs.rmSync(copy);
      } else yield item;
    }
    for (const [i, project] of projects.entries()) {
      if (fs.realpathSync(project) !== project || !fs.statSync(project).isDirectory()) throw new Error("A selected project folder is unavailable.");
      yield* tree(project, `projects/${i}`, () => true);
    }
  }
  seal(file, key, entries(), archiveContext(source, id));
  if (layout(insideUpdate) !== version) throw new Error("The folder layout changed during backup. Try again.");
  return manifest;
}

export async function backupBefore(reason: Reason, { now = Date.now(), insideUpdate = false } = {}): Promise<RestorePoint | null> {
  if (!config()?.enabled) return null;
  return backupLock(async () => {
    const kept = config();
    if (!kept?.enabled) return null;
    const today = new Date(now).toISOString().slice(0, 10);
    if (reason === "daily" && kept.lastDaily?.id.startsWith(today) && !kept.error) return kept.lastDaily;
    let scratch: string | undefined;
    try {
      checkComputer(kept.dest);
      if (kept.dest.kind === "disk" && fs.statSync(kept.dest.path).dev !== kept.disk) throw new Error("Reconnect the selected backup disk.");
      layout(insideUpdate);
      if (kept.dest.kind === "computer") await verifyBackupComputer(kept.dest);
      const store = destinationStore(kept.dest, kept.source);
      scratch = scratchFolder();
      const file = path.join(scratch, "archive");
      const id = pointId(reason, now);
      const { version } = captureBackup(file, kept.key, kept.source, id, now, kept.projects, scratch, insideUpdate);
      await store.put(id, file);
      try { authenticate(await store.get(id, scratch), kept.key, archiveContext(kept.source, id)); }
      catch (error) { await store.erase(id).catch(() => {}); throw error; }
      const point = { id, at: now, version };
      save({ ...kept, last: point, ...(reason === "daily" ? { lastDaily: point } : {}), error: undefined });
      for (const expired of expiredPoints(await store.list(), now)) await store.erase(expired);
      return point;
    } catch (error) {
      const current = config() ?? kept;
      save({ ...current, error: (error as Error).message });
      throw error;
    } finally {
      if (scratch) fs.rmSync(scratch, { recursive: true, force: true });
    }
  });
}

let timer: ReturnType<typeof setInterval> | undefined;
export function keepDailyBackups(): void {
  if (timer) return;
  const check = () => { void backupBefore("daily").catch(() => {}); };
  setTimeout(check, 60_000).unref();
  timer = setInterval(check, 15 * 60_000);
  timer.unref();
}

export async function erasePoint(dest: Destination, source: string, id: string) {
  return backupLock(async () => {
    await destinationStore(dest, source).erase(id);
    const kept = config();
    if (kept?.source === source && JSON.stringify(kept.dest) === JSON.stringify(dest)) {
      save({ ...kept, ...(kept.last?.id === id ? { last: undefined } : {}), ...(kept.lastDaily?.id === id ? { lastDaily: undefined } : {}) });
    }
  });
}
