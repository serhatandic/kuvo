import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DATA_DECLARATIONS, type DataKind } from "../../shared/data-declarations";
import { backupFolderVersion } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { swapUnderWay } from "../../shared/engine-swap";
import { updateUnderWay } from "../../shared/updating";
import { held, ownSign } from "../documents/format";
import { exclusiveWhile, locked, privateFolder, writeDurablePrivate } from "../files";
import { K, RUN, homeFolder, located } from "../home";
import { QUEUES_LOCK } from "../work-queue/migrate";
import { portableApproval } from "../work/approval";
import { mayActivate } from "../ownership/resolve";
import { ownKey, peersOf } from "../peers/known";
import { leadershipDocuments, resolveCheckpoint } from "../setup/checkpoint";
import { followedSetup } from "../setup/record";
import { readLink, updateLink } from "../state";
import { authenticate, archiveContext, newRecoveryKey, seal, type Entry } from "./archive";
import { destinationStore, expiredPoints, separateDisk, verifyBackupComputer, type Destination } from "./destination";
import { projectEntries } from "./projects";
import { scrubEngine, snapshotDatabase } from "./sqlite";
import { durableFiles, inside, recordReferences, roots, stat } from "./locations";

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
export const backupLock = <T>(work: () => Promise<T>) => exclusiveWhile(path.join(BACKUP, ".operation"), async () => {
  for (const name of fs.readdirSync(privateFolder(BACKUP))) {
    if (name.startsWith(".scratch-")) fs.rmSync(path.join(BACKUP, name), { recursive: true, force: true });
  }
  return work();
});
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

export function captureBackup(file: string, key: string, source: string, id: string, now: number,
  projects: readonly string[], scratch: string, insideUpdate = false): Manifest {
  const version = layout(insideUpdate);
  const boundary = acceptedBoundary();
  const manifest: Manifest = { format: 1, source, id, at: now, version, origin: boundary.origin, signer: boundary.signer, setup: boundary.setup,
    paired: boundary.paired, projects: projects.map(project => path.basename(project)) };
  const link = boundary.link;
  const sessionFor = (root: string, id: string) => {
    const base = roots[root];
    const file = base && path.join(base, id, "session");
    return file && stat(file)?.isFile() ? fs.readFileSync(file, "utf8").trim() : id;
  };
  const queueKind = (kind: DataKind) => kind === "queueDepartureFences" || kind === "queueDeletionFences" ||
    (kind.startsWith("queue") && kind !== "queueNumberAllocation");
  const queue = locked(QUEUES_LOCK, () => [...durableFiles(roots, queueKind)].map((entry, index) => {
    const file = path.join(scratch, `queue-${index}`);
    fs.cpSync(entry.file, file, { dereference: false, force: false, errorOnExist: true });
    return { ...entry, file };
  }), { exclusive: true });
  function* entries(): Generator<Entry> {
    yield bytes("meta/manifest.json", manifest);
    yield* boundary.entries;
    let db = 0;
    yield* queue;
    for (const item of durableFiles(roots, kind => !queueKind(kind))) {
      const [root] = item.name.split("/");
      if (!recordReferences(item.name, sessionFor).every(ref => mayActivate(ref, { link }))) continue;
      if (item.name.startsWith("engine/") && item.name.endsWith(".db") || item.name === "agents/usage/insights.sqlite") {
        const copy = path.join(scratch, `db-${db++}`);
        snapshotDatabase(item.file, copy);
        if (root === "engine") scrubEngine(copy, [], false);
        yield { name: item.name, file: copy };
        fs.rmSync(copy);
      } else if (item.name === "home/box/approval.json") {
        yield bytes(item.name, portableApproval(JSON.parse(fs.readFileSync(item.file, "utf8"))));
      } else yield item;
    }
    yield* projectEntries(projects);
  }
  seal(file, key, entries(), archiveContext(source, id));
  if (layout(insideUpdate) !== version) throw new Error("The folder layout changed during backup. Try again.");
  return manifest;
}

async function writeVerifiedPoint(store: ReturnType<typeof destinationStore>, source: string, key: string, id: string,
  now: number, projects: string[], scratch: string, insideUpdate = false): Promise<RestorePoint> {
  const file = path.join(scratch, "archive");
  const { version } = captureBackup(file, key, source, id, now, projects, scratch, insideUpdate);
  await store.put(id, file);
  try { authenticate(await store.get(id, scratch), key, archiveContext(source, id)); }
  catch (error) { await store.erase(id).catch(() => {}); throw error; }
  return { id, at: now, version };
}

export type ExportPoint = { source: string; id: string; at: number };
type ExportTarget = { destination: Destination; recoveryKey: string };

export function verifyExport(point: ExportPoint, target: ExportTarget): Promise<void> {
  return backupLock(async () => {
    const scratch = scratchFolder();
    try {
      const store = destinationStore(target.destination, point.source);
      authenticate(await store.get(point.id, scratch), target.recoveryKey, archiveContext(point.source, point.id));
    } finally { fs.rmSync(scratch, { recursive: true, force: true }); }
  });
}

/** A one-time backup with its own retention and project selection. The caller journals the point
 * before using it, so a crash after publication resumes by authenticating the same archive. */
export function exportBeforeWipe(point: ExportPoint, target: ExportTarget, projects: string[]): Promise<void> {
  return backupLock(async () => {
    const { destination, recoveryKey } = target;
    checkComputer(destination);
    if (destination.kind === "computer") await verifyBackupComputer(destination);
    if (destination.kind === "disk" && projects.some(folder => inside(folder, destination.path) || inside(destination.path, folder))) {
      throw new Error("Choose an export folder separate from the selected projects.");
    }
    const scratch = scratchFolder();
    try {
      const store = destinationStore(destination, point.source);
      if ((await store.list()).includes(point.id)) {
        authenticate(await store.get(point.id, scratch), recoveryKey, archiveContext(point.source, point.id));
      } else {
        await writeVerifiedPoint(store, point.source, recoveryKey, point.id, point.at, projects, scratch);
      }
    } finally { fs.rmSync(scratch, { recursive: true, force: true }); }
  });
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
      const point = await writeVerifiedPoint(store, kept.source, kept.key, pointId(reason, now), now, kept.projects, scratch, insideUpdate);
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
